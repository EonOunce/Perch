//! 统计口径（DESIGN §6.1–§6.5）：范围解析、完成率、热力图格、记录量、心情与每周文字回顾。
//!
//! 完成率的分母沿用 [`crate::habits`] 那份「一天一个有效状态」的口径，这里只把它摊到范围上，
//! 不重新定义第二遍。前端拿到的是已经判好的数字与档位，只负责排版——
//! 同一个公式如果在 JS 里再算一遍，两端迟早给出两个数（DESIGN §5.4、§5.5）。
//!
//! §6.6 的专业视图是 Phase 3，这里没有它。

use std::collections::{HashMap, HashSet};

use serde::Serialize;

use crate::date_boundary::{
    add_days, civil_from_days, date_range, format_date, parse_date, parse_instant, week_start,
};
use crate::goal::{self, EffectiveGoal};
use crate::habits::{self, DayStatus};
use crate::model::{Data, EntityType, Habit, Note, TargetMode};
use crate::records::{self, TagCount};

/// 一次能看的范围上限。再长下去一格比像素还窄，不如让人分段看。
pub const MAX_RANGE_DAYS: i64 = 366;

/// 「全部习惯」这一档的取值；选择器与聚合行都用它。
pub const SCOPE_ALL: &str = "all";

/// 按 §2.6 的五段计数，顺序与前端 `MOOD_BANDS` 一致。段位只是呈现，均值仍用原始分数。
const MOOD_BANDS: [(i32, i32); 5] = [(0, 19), (20, 39), (40, 59), (60, 79), (80, 100)];

/// 已解析的统计范围：`from` / `to` 含首尾。
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct Range {
    /// `7` / `30` / `90` / `month` / `lastMonth` / `custom`，原样带回去，选择器才知道选了哪档。
    pub kind: String,
    pub from: String,
    pub to: String,
    pub days: usize,
}

impl Range {
    fn build(kind: &str, from: String, to: String) -> Result<Range, String> {
        let (Some(a), Some(b)) = (parse_date(&from), parse_date(&to)) else {
            return Err("起止日期里有读不出来的，先填成 YYYY-MM-DD。".to_string());
        };
        if a > b {
            return Err("起始日期不能晚于结束日期。".to_string());
        }
        let days = b - a + 1;
        if days > MAX_RANGE_DAYS {
            return Err(format!("一次最多看 {MAX_RANGE_DAYS} 天，先分段看。"));
        }
        Ok(Range {
            kind: kind.to_string(),
            from,
            to,
            days: days as usize,
        })
    }
}

/// 把范围选择器那一带换成具体起止日期（§6.1）。
///
/// 本月截到今天，不画未来；上月是完整自然月。自定义原样收下两个输入，
/// 非法或倒置时返回 `Err`，那份文案就地提示——界面保留上一次的有效范围，不悄悄退回默认档。
pub fn resolve_range(today: &str, kind: &str, from: &str, to: &str) -> Result<Range, String> {
    let Some(base) = parse_date(today) else {
        return Err("今天这个日期读不出来，统计先不算。".to_string());
    };
    let (y, m, _) = civil_from_days(base);
    let this_first = first_of_month(y, m);
    match kind {
        "7" | "30" | "90" => {
            let n: i64 = kind.parse().map_err(|_| "范围天数读不出来。".to_string())?;
            Range::build(kind, format_date(base - (n - 1)), format_date(base))
        }
        "month" => Range::build(kind, this_first.clone(), format_date(base)),
        "lastMonth" => {
            let prev_last = parse_date(&this_first).unwrap_or(base) - 1;
            let (py, pm, _) = civil_from_days(prev_last);
            Range::build(kind, first_of_month(py, pm), format_date(prev_last))
        }
        "custom" => {
            if from.trim().is_empty() || to.trim().is_empty() {
                return Err("自定义范围要填起止两个日期。".to_string());
            }
            Range::build(kind, from.trim().to_string(), to.trim().to_string())
        }
        other => Err(format!("不认识的范围「{other}」。")),
    }
}

fn first_of_month(year: i64, month: i64) -> String {
    format_date(parse_date(&format!("{year:04}-{month:02}-01")).unwrap_or(0))
}

/// 一个习惯在这份统计里的全部数字。
#[derive(Debug, Clone, PartialEq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct HabitStat {
    pub id: String,
    pub name: String,
    pub color: String,
    pub icon: String,
    pub unit: String,
    /// 参与统计的第一天；`None` = 连创建时间都读不出来，一格都不画。
    pub first_day: Option<String>,
    pub done: i64,
    pub not_done: i64,
    pub unconfirmed: i64,
    /// 分母 = 上面三项之和（§6.2），恒等于范围内「本该已经知道结果」的天数。
    pub total: i64,
    /// `None` = 分母为 0，界面显示「—」而不是 0%。
    pub rate: Option<f64>,
    pub week_start: String,
    /// 本周一到今天为止的完成数。未过完的周不判失败（§6.2）。
    pub week_done: i64,
    pub week_target: Option<i32>,
    pub week_mode: &'static str,
    /// `None` = 这一周没有周目标可判（只记录阶段）。
    pub week_met: Option<bool>,
    /// 连续达标周数；`None` = 只记录阶段，不计入连续达标。
    pub streak_weeks: Option<i64>,
}

/// 范围选择器上的一个习惯。选择器列的是「所有能进统计的习惯」，
/// 与当前选了哪个无关——否则选中单个习惯后就再也切不回别的了。
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct ScopeOption {
    pub id: String,
    pub name: String,
    pub color: String,
}

/// 热力图的一格。档位只描述这一天完成到什么程度，不含评价。
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct HeatCell {
    pub date: String,
    /// `l2` 达标 / `l1` 部分完成 / `l0` 已确认但没做 / `pending` 只有描边 / `blank` 完全透明。
    pub level: &'static str,
    /// 陶土色描边圈出今天，不改填充（§6.2）。
    pub today: bool,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct HeatRow {
    /// 聚合行的 ID 固定是 `all`。
    pub id: String,
    pub name: String,
    pub color: String,
    pub cells: Vec<HeatCell>,
}

/// 记录量的一格：按日就是一天，按周就是这一周（可能不满 7 天）。
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct VolumeBucket {
    pub start: String,
    pub end: String,
    pub count: usize,
}

/// 时段四段（§6.3）。边界只在下面那一次 `time_bucket` 调用里写，键名给前端配标签。
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct TimeBucket {
    pub key: &'static str,
    pub count: usize,
}

#[derive(Debug, Clone, PartialEq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct Volume {
    /// `true` 按日柱状，`false` 按周聚合（§6.3 用 31 天做分界）。
    pub per_day: bool,
    pub buckets: Vec<VolumeBucket>,
    pub total: usize,
    /// 平均每格几条：按日就是每天，按周就是每周。
    pub average: f64,
    /// 写作连续天数（§6.3）：从「今天或昨天」往前数，与范围无关。
    pub streak_days: i64,
    /// 范围内 Top 5 标签。
    pub tags: Vec<TagCount>,
    pub times: Vec<TimeBucket>,
}

/// 心情折线的一个点。按周时 `value` 是那一周的均值，`days` 是有评分的天数。
#[derive(Debug, Clone, PartialEq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct MoodPoint {
    pub date: String,
    pub value: f64,
    pub days: usize,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct MoodBand {
    pub min: i32,
    pub max: i32,
    pub count: usize,
}

#[derive(Debug, Clone, PartialEq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct Mood {
    /// 范围长于 31 天就按周均值，折线才读得清。
    pub use_weekly: bool,
    pub days: Vec<MoodPoint>,
    pub weeks: Vec<MoodPoint>,
    /// 有评分的天数。未记录的日子不进均值、也不画点（§2.6）。
    pub count: usize,
    pub average: Option<f64>,
    /// 少于 2 个有评分的时间点不画趋势线，只显示数据不足（§6.4）。
    pub enough: bool,
    pub bands: Vec<MoodBand>,
}

/// 高低完成日对比（§6.4）：只对单个习惯做，跨习惯混合没有解释意义。
#[derive(Debug, Clone, PartialEq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct Compare {
    pub habit_id: String,
    pub habit_name: String,
    pub high_days: usize,
    pub high_average: Option<f64>,
    pub low_days: usize,
    pub low_average: Option<f64>,
    /// 任一组不足 4 天就只说「样本还少」，不画对比。界面另写「相关不等于因果」。
    pub enough: bool,
}

/// 每周文字回顾（§6.5）：固定看最近一个完整的周一–周日，与统计范围无关。
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct Review {
    pub week_start: String,
    pub week_end: String,
    /// 已经写好的事实句，逐行呈现。习惯多时按习惯分行，不合并成一句笼统结论。
    pub lines: Vec<String>,
}

/// 一份统计快照。范围与习惯选择都已归一化，前端照着渲染就行。
#[derive(Debug, Clone, PartialEq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct Stats {
    pub today: String,
    /// 昨天。完成率与热力图的「今天之前」都靠它，前端不再自己做日期减法。
    pub yesterday: String,
    pub range: Range,
    /// 归一化后的习惯范围：`all` 或某个真实参与统计的习惯 ID。
    pub scope: String,
    pub scope_name: String,
    /// 当前范围内参与统计的习惯，按 `sortOrder` 排好；选单个习惯时只剩它自己。逐习惯行用它。
    pub habits: Vec<HabitStat>,
    /// 范围选择器的候选：所有能进统计的习惯，与当前范围无关（§6.2）。
    pub scope_options: Vec<ScopeOption>,
    /// 首屏那一个数字的分子与分母：全部 = Σdone / Σ分母，跨习惯按天汇总不按习惯平均（§6.2）。
    pub done: i64,
    pub total: i64,
    pub rate: Option<f64>,
    pub heatmap: Heatmap,
    pub volume: Volume,
    pub mood: Mood,
    /// `None` = 习惯范围是「全部」，不做这个对比。
    pub compare: Option<Compare>,
    pub review: Review,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct Heatmap {
    pub rows: Vec<HeatRow>,
}

/// 当前范围里参与统计的习惯。「全部」给出所有；选中的 ID 已经不参与（被移除，或归档后关了
/// 「统计」开关）就退回「全部」并报告 `all`——不给前端一个空面板，选择器跟着 `scope` 归一化。
pub fn scoped_habits<'a>(data: &'a Data, scope: &str) -> (Vec<&'a Habit>, bool) {
    if scope == SCOPE_ALL {
        return (habits::stats_list(data), true);
    }
    let picked: Vec<&Habit> = habits::stats_list(data)
        .into_iter()
        .filter(|h| h.id == scope)
        .collect();
    if picked.is_empty() {
        (habits::stats_list(data), true)
    } else {
        (picked, false)
    }
}

/// 算整份统计。`offset_secs` 用来把 `createdAt` 换成 `localDate`（§3.3），核心自己不读时钟。
pub fn stats(data: &Data, today: &str, offset_secs: i32, range: Range, scope: &str) -> Stats {
    let yesterday = add_days(today, -1).unwrap_or_else(|| today.to_string());
    let (listed, is_all) = scoped_habits(data, scope);
    let scope_options: Vec<ScopeOption> = habits::stats_list(data)
        .iter()
        .map(|h| ScopeOption {
            id: h.id.clone(),
            name: h.name.clone(),
            color: h.color.clone(),
        })
        .collect();
    let rows: Vec<HabitStat> = listed
        .iter()
        .map(|habit| habit_stat(data, habit, &range, today, &yesterday, offset_secs))
        .collect();

    let done: i64 = rows.iter().map(|r| r.done).sum();
    let total: i64 = rows.iter().map(|r| r.total).sum();
    // 「全部」且确实有多个习惯时才值得一个聚合行；单习惯的聚合与它自己一模一样，不重复画。
    let aggregate = is_all && rows.len() > 1;

    Stats {
        heatmap: heatmap(data, &rows, &range, today, aggregate),
        volume: volume(data, &range, today),
        mood: mood(data, &range),
        // §6.4：习惯范围是「全部」时不做高低完成日对比，跨习惯混合没有解释意义。
        compare: (!is_all).then(|| compare(data, &rows[0], &range, &yesterday)),
        review: review(data, today, offset_secs),
        today: today.to_string(),
        yesterday,
        done,
        total,
        rate: (total > 0).then(|| done as f64 / total as f64),
        scope: if is_all {
            SCOPE_ALL.to_string()
        } else {
            rows[0].id.clone()
        },
        scope_name: if is_all {
            "全部".to_string()
        } else {
            rows[0].name.clone()
        },
        habits: rows,
        scope_options,
        range,
    }
}

fn habit_stat(
    data: &Data,
    habit: &Habit,
    range: &Range,
    today: &str,
    yesterday: &str,
    offset_secs: i32,
) -> HabitStat {
    let first = goal::first_day(habit, offset_secs);
    let end = rate_end(range, yesterday);
    let counts = window(&range.from, first.as_deref(), &end)
        .and_then(|(from, to)| habits::counts(data, &habit.id, &from, &to))
        .unwrap_or_default();

    let this_week = week_start(today).unwrap_or_else(|| today.to_string());
    let week_goal = goal::goal_on(habit, &this_week);
    let week_target = weekly_target_of(&week_goal);
    let week_done = window(&this_week, first.as_deref(), today)
        .and_then(|(from, to)| habits::counts(data, &habit.id, &from, &to))
        .map(|c| c.done)
        .unwrap_or(0);

    HabitStat {
        id: habit.id.clone(),
        name: habit.name.clone(),
        color: habit.color.clone(),
        icon: habit.icon.clone(),
        unit: habit.unit.clone(),
        week_start: this_week,
        week_done,
        week_target,
        week_mode: mode_text(week_goal.mode),
        week_met: week_target.map(|target| week_done >= target as i64),
        streak_weeks: week_target
            .is_some()
            .then(|| streak_weeks(data, habit, today, &first))
            .flatten(),
        first_day: first,
        done: counts.done,
        not_done: counts.not_done,
        unconfirmed: counts.unconfirmed,
        total: counts.denominator(),
        rate: counts.rate(),
    }
}

/// 完成率那个窗口：起点取「范围起点」与「习惯第一天」里较晚的，终点是昨天（§6.2）。
/// 起点已经晚于终点时返回 `None`，调用方按「还没有该确认的天」处理，不硬算成 0 天。
fn window(from: &str, first: Option<&str>, to: &str) -> Option<(String, String)> {
    let end = parse_date(to)?;
    let mut start = parse_date(from)?;
    if let Some(day) = first.and_then(parse_date) {
        start = start.max(day);
    }
    (start <= end).then(|| (format_date(start), to.to_string()))
}

/// 分母的终点：范围终点与昨天里较早的那个。看上月时不能把十月也算进去，
/// 而今天与未来本来就不该进分母（§6.2）。ISO 日期串按字典序比就是按时间比。
fn rate_end(range: &Range, yesterday: &str) -> String {
    if range.to.as_str() > yesterday {
        yesterday.to_string()
    } else {
        range.to.clone()
    }
}

/// 有周目标才谈得上达标；只记录阶段没有目标可判（§2.4、§6.2）。
fn weekly_target_of(goal: &EffectiveGoal) -> Option<i32> {
    match goal.mode {
        TargetMode::Weekly | TargetMode::Nightly => goal.weekly_target.filter(|t| *t > 0),
        TargetMode::RecordOnly | TargetMode::Custom => None,
    }
}

/// 连续达标周数（§6.2）：从上一个已经完整结束的周往前数，进行中的那周既不参与也不打断。
///
/// 遇到只记录阶段、没有目标，或整周都早于习惯第一天的情况就停下——那是「这段时间不适用」，
/// 不是「达标中断」；把它算成中断会把早期历史一律抹成 0。
fn streak_weeks(data: &Data, habit: &Habit, today: &str, first: &Option<String>) -> Option<i64> {
    let current = week_start(today)?;
    let mut cursor = add_days(&current, -7)?;
    let mut streak = 0i64;
    for _ in 0..MAX_RANGE_DAYS / 7 {
        let week_end = add_days(&cursor, 6)?;
        if first.as_deref().is_some_and(|day| day > week_end.as_str()) {
            return Some(streak);
        }
        let goal = goal::goal_on(habit, &cursor);
        let Some(target) = weekly_target_of(&goal) else {
            return Some(streak);
        };
        let done = window(&cursor, first.as_deref(), &week_end)
            .and_then(|(from, to)| habits::counts(data, &habit.id, &from, &to))
            .map(|c| c.done)
            .unwrap_or(0);
        if done < target as i64 {
            return Some(streak);
        }
        streak += 1;
        cursor = add_days(&cursor, -7)?;
    }
    Some(streak)
}

fn heatmap(
    data: &Data,
    rows: &[HabitStat],
    range: &Range,
    today: &str,
    aggregate: bool,
) -> Heatmap {
    let days = date_range(&range.from, &range.to).unwrap_or_default();
    let mut out = Vec::new();
    if aggregate {
        out.push(HeatRow {
            id: SCOPE_ALL.to_string(),
            name: "全部习惯".to_string(),
            color: "sage".to_string(),
            cells: days
                .iter()
                .map(|day| aggregate_cell(data, rows, day, today))
                .collect(),
        });
    }
    for row in rows {
        out.push(HeatRow {
            id: row.id.clone(),
            name: row.name.clone(),
            color: row.color.clone(),
            cells: days
                .iter()
                .map(|day| habit_cell(data, row, day, today))
                .collect(),
        });
    }
    Heatmap { rows: out }
}

/// 这一天落在该习惯的起点之后吗？起点之前一律留白，不算「没做」。
fn started_by(first_day: &Option<String>, day: &str) -> bool {
    first_day
        .as_deref()
        .and_then(parse_date)
        .is_some_and(|first| parse_date(day).unwrap_or(i64::MIN) >= first)
}

/// 一个习惯的一格。习惯开始之前与未来留白（空白不等于没做，§6.2）；今天照画自己那一格，只用描边区分。
fn habit_cell(data: &Data, row: &HabitStat, day: &str, today: &str) -> HeatCell {
    let started = started_by(&row.first_day, day);
    let level = if !started || day > today {
        "blank"
    } else {
        match habits::status_on(data, &row.id, day) {
            DayStatus::Done => "l2",
            DayStatus::NotDone => "l0",
            DayStatus::Unconfirmed => "pending",
        }
    };
    HeatCell {
        date: day.to_string(),
        level,
        today: day == today,
    }
}

/// 「全部」视图顶行的一格：把当天各习惯合起来看，`l1` 就是「做了一部分」。
fn aggregate_cell(data: &Data, rows: &[HabitStat], day: &str, today: &str) -> HeatCell {
    let active: Vec<&HabitStat> = rows
        .iter()
        .filter(|row| started_by(&row.first_day, day))
        .collect();
    let status = |row: &HabitStat| habits::status_on(data, &row.id, day);
    let level = if active.is_empty() || day > today {
        "blank"
    } else {
        let done = active
            .iter()
            .filter(|row| status(row) == DayStatus::Done)
            .count();
        let confirmed = active
            .iter()
            .filter(|row| status(row) != DayStatus::Unconfirmed)
            .count();
        if confirmed == 0 {
            "pending"
        } else if done == active.len() {
            "l2"
        } else if done > 0 {
            "l1"
        } else {
            "l0"
        }
    };
    HeatCell {
        date: day.to_string(),
        level,
        today: day == today,
    }
}

fn volume(data: &Data, range: &Range, today: &str) -> Volume {
    let notes = live_notes(data, &range.from, &range.to);
    let per_day = range.days <= 31;
    let buckets = if per_day {
        date_range(&range.from, &range.to)
            .unwrap_or_default()
            .into_iter()
            .map(|day| VolumeBucket {
                count: notes.iter().filter(|n| n.local_date == day).count(),
                end: day.clone(),
                start: day,
            })
            .collect()
    } else {
        weekly_buckets(&notes, &range.from, &range.to)
    };
    let total = notes.len();
    // 一格一条平均：按日就是每天，按周就是每周。
    let average = if buckets.is_empty() {
        0.0
    } else {
        total as f64 / buckets.len() as f64
    };
    let times = vec![
        time_bucket("dawn", &notes, |h| (4..9).contains(&h)),
        time_bucket("day", &notes, |h| (9..17).contains(&h)),
        time_bucket("evening", &notes, |h| (17..22).contains(&h)),
        time_bucket("night", &notes, |h| !(4..22).contains(&h)),
    ];
    Volume {
        tags: records::tag_counts_from(&notes)
            .into_iter()
            .take(5)
            .collect(),
        buckets,
        average,
        streak_days: writing_streak(data, today),
        total,
        per_day,
        times,
    }
}

/// 按周聚合：从范围起点起每 7 天一格，最后一格可能被范围终点截短。
fn weekly_buckets(notes: &[&Note], from: &str, to: &str) -> Vec<VolumeBucket> {
    let mut out = Vec::new();
    let mut cursor = from.to_string();
    for _ in 0..MAX_RANGE_DAYS / 7 + 1 {
        let proposed = add_days(&cursor, 6).unwrap_or_else(|| cursor.clone());
        let end = if proposed.as_str() > to {
            to.to_string()
        } else {
            proposed
        };
        out.push(VolumeBucket {
            start: cursor.clone(),
            end: end.clone(),
            count: notes
                .iter()
                .filter(|n| {
                    n.local_date.as_str() >= cursor.as_str()
                        && n.local_date.as_str() <= end.as_str()
                })
                .count(),
        });
        if end == *to {
            break;
        }
        cursor = match add_days(&end, 1) {
            Some(next) => next,
            None => break,
        };
    }
    out
}

/// 时段四段里的一段：`keep` 决定哪些钟点算进这一段。
fn time_bucket(key: &'static str, notes: &[&Note], keep: impl Fn(u32) -> bool) -> TimeBucket {
    TimeBucket {
        key,
        count: notes
            .iter()
            .filter(|n| hour_of(n).is_some_and(&keep))
            .count(),
    }
}

/// 写下它的那个小时，取 `createdAt` 自带的本地偏移，不换算到别的时区（§6.3「时段按真实钟点」）。
fn hour_of(note: &Note) -> Option<u32> {
    note.created_at.get(11..13)?.parse().ok()
}

/// 写作连续天数（§6.3）：今天还没有记录时不从今天归零，从昨天往前数。
pub fn writing_streak(data: &Data, today: &str) -> i64 {
    let days: HashSet<&str> = live_note_refs(data)
        .iter()
        .map(|n| n.local_date.as_str())
        .collect();
    if days.is_empty() {
        return 0;
    }
    let mut cursor = today.to_string();
    if !days.contains(cursor.as_str()) {
        cursor = match add_days(&cursor, -1) {
            Some(yesterday) => yesterday,
            None => return 0,
        };
    }
    let mut streak = 0i64;
    for _ in 0..MAX_RANGE_DAYS {
        if !days.contains(cursor.as_str()) {
            break;
        }
        streak += 1;
        cursor = match add_days(&cursor, -1) {
            Some(previous) => previous,
            None => break,
        };
    }
    streak
}

fn live_note_refs(data: &Data) -> Vec<&Note> {
    data.notes
        .iter()
        .filter(|n| !habits::is_tombstoned(data, EntityType::Note, &n.id))
        .collect()
}

fn live_notes<'a>(data: &'a Data, from: &str, to: &str) -> Vec<&'a Note> {
    live_note_refs(data)
        .into_iter()
        .filter(|n| n.local_date.as_str() >= from && n.local_date.as_str() <= to)
        .collect()
}

fn mood(data: &Data, range: &Range) -> Mood {
    let mut scores: Vec<(String, i32)> = data
        .daily_moods
        .iter()
        .filter(|m| !habits::is_tombstoned(data, EntityType::DailyMood, &m.id))
        .filter(|m| m.local_date >= range.from && m.local_date <= range.to)
        .filter(|m| (0..=100).contains(&m.score))
        .map(|m| (m.local_date.clone(), m.score))
        .collect();
    // 一天最多一条是写入侧的保证（§2.6 的自然键是 localDate），这里只排序不合并。
    scores.sort_by(|a, b| a.0.cmp(&b.0));
    let count = scores.len();
    let average =
        (count > 0).then(|| scores.iter().map(|(_, s)| *s as f64).sum::<f64>() / count as f64);
    let bands = MOOD_BANDS
        .iter()
        .map(|(min, max)| MoodBand {
            min: *min,
            max: *max,
            count: scores.iter().filter(|(_, s)| s >= min && s <= max).count(),
        })
        .collect();
    let days = scores
        .iter()
        .map(|(date, s)| MoodPoint {
            date: date.clone(),
            value: *s as f64,
            days: 1,
        })
        .collect();
    let mut by_week: HashMap<String, (i64, usize)> = HashMap::new();
    for (date, s) in &scores {
        let week = week_start(date).unwrap_or_else(|| date.clone());
        let entry = by_week.entry(week).or_default();
        entry.0 += *s as i64;
        entry.1 += 1;
    }
    let mut weeks: Vec<MoodPoint> = by_week
        .into_iter()
        .map(|(week, (sum, n))| MoodPoint {
            date: week,
            value: sum as f64 / n as f64,
            days: n,
        })
        .collect();
    weeks.sort_by(|a, b| a.date.cmp(&b.date));
    Mood {
        use_weekly: range.days > 31,
        days,
        weeks,
        count,
        average,
        enough: count >= 2,
        bands,
    }
}

/// 高低完成日对比（§6.4）：只有同时有心情与所选习惯确认（`done` / `notDone`）的日子才进样本。
fn compare(data: &Data, row: &HabitStat, range: &Range, yesterday: &str) -> Compare {
    let end = rate_end(range, yesterday);
    let days = window(&range.from, row.first_day.as_deref(), &end)
        .and_then(|(from, to)| date_range(&from, &to))
        .unwrap_or_default();
    let mut high = Vec::new();
    let mut low = Vec::new();
    for day in days {
        let Some(score) = mood_on(data, &day) else {
            continue;
        };
        match habits::status_on(data, &row.id, &day) {
            DayStatus::Done => high.push(score),
            DayStatus::NotDone => low.push(score),
            DayStatus::Unconfirmed => {}
        }
    }
    let mean = |v: &[i32]| (!v.is_empty()).then(|| v.iter().sum::<i32>() as f64 / v.len() as f64);
    Compare {
        habit_id: row.id.clone(),
        habit_name: row.name.clone(),
        high_days: high.len(),
        high_average: mean(&high),
        low_days: low.len(),
        low_average: mean(&low),
        enough: high.len() >= 4 && low.len() >= 4,
    }
}

fn mood_on(data: &Data, day: &str) -> Option<i32> {
    data.daily_moods
        .iter()
        .find(|m| m.local_date == day && (0..=100).contains(&m.score))
        .filter(|m| !habits::is_tombstoned(data, EntityType::DailyMood, &m.id))
        .map(|m| m.score)
}

/// 每周文字回顾（§6.5）：与统计范围无关，固定是最近一个完整的周一–周日。
///
/// 「值得保留的一句话」取这一周里最长的一条记录，同长取更早写下的那条：
/// 规则得能被两端复算，所以不选「随机一条」。全部为事实陈述，不推断原因、不评价人。
pub fn review(data: &Data, today: &str, offset_secs: i32) -> Review {
    let empty = Review {
        week_start: String::new(),
        week_end: String::new(),
        lines: vec!["还没有可以回顾的一周。".to_string()],
    };
    let (Some(this_week), Some(week_start)) = (
        week_start(today),
        week_start(today).and_then(|w| add_days(&w, -7)),
    ) else {
        return empty;
    };
    let week_end = add_days(&this_week, -1).unwrap_or_else(|| week_start.clone());

    let mut lines = Vec::new();
    let listed = habits::stats_list(data);
    if listed.is_empty() {
        lines.push("还没有习惯可以回顾。".to_string());
    }
    for habit in &listed {
        let first = goal::first_day(habit, offset_secs);
        let goal = goal::goal_on(habit, &week_start);
        let done = window(&week_start, first.as_deref(), &week_end)
            .and_then(|(from, to)| habits::counts(data, &habit.id, &from, &to))
            .map(|c| c.done)
            .unwrap_or(0);
        lines.push(match weekly_target_of(&goal) {
            Some(target) => format!(
                "{}：完成 {}/{} {}{}",
                habit.name,
                done,
                target,
                habit.unit,
                if done >= target as i64 {
                    "，达标"
                } else {
                    ""
                }
            ),
            None => format!("{}：{} {}", habit.name, done, habit.unit),
        });
    }

    let notes = live_notes(data, &week_start, &week_end);
    if notes.is_empty() {
        lines.push("这一周没有记录。".to_string());
    } else {
        let mut line = format!("记录：{} 条", notes.len());
        if let Some(top) = records::tag_counts_from(&notes).first() {
            line.push_str(&format!("，最常写到 #{}（{} 次）", top.tag, top.count));
        }
        lines.push(line);
        let quote = longest(&notes);
        let text = trimmed(&quote.text);
        if !text.is_empty() {
            lines.push(format!(
                "值得留下的一句：{} —「{}」",
                quote.local_date, text
            ));
        }
    }
    for habit in listed.iter().filter(|h| !h.if_then.trim().is_empty()) {
        lines.push(format!(
            "下周的 if–then：{} — {}",
            habit.name,
            trimmed(habit.if_then.trim())
        ));
    }

    Review {
        week_start,
        week_end,
        lines,
    }
}

fn longest<'a>(notes: &[&'a Note]) -> &'a Note {
    let chars = |s: &str| s.chars().count();
    notes
        .iter()
        .copied()
        .max_by(|a, b| {
            chars(&a.text)
                .cmp(&chars(&b.text))
                // 一样长就取更早写下的那条；再一样按 ID 反序，保证两端同序。
                .then_with(|| instant(b).cmp(&instant(a)))
                .then_with(|| b.id.cmp(&a.id))
        })
        .expect("调用方保证非空")
}

fn instant(note: &Note) -> i64 {
    parse_instant(&note.created_at).unwrap_or(i64::MIN)
}

/// 回顾的一行不放整段：取第一行、限 40 字，超出加省略号，不改写原句。
fn trimmed(text: &str) -> String {
    let line = text.lines().next().unwrap_or("").trim();
    let mut out: String = line.chars().take(40).collect();
    if line.chars().count() > 40 {
        out.push('…');
    }
    out
}

fn mode_text(mode: TargetMode) -> &'static str {
    match mode {
        TargetMode::RecordOnly => "recordOnly",
        TargetMode::Weekly => "weekly",
        TargetMode::Nightly => "nightly",
        TargetMode::Custom => "custom",
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::model::{
        CheckinEvent, CheckinStatus, ConfirmationTiming, DailyMood, GoalEntry, Note, Tombstone,
    };
    use std::collections::HashMap;

    /// 周二。本周一是 10-26，上一个完整的周一是 10-19。
    const TODAY: &str = "2026-10-27";
    const OFFSET: i32 = 8 * 3600;

    fn habit(id: &str, name: &str, target: Option<i32>) -> Habit {
        Habit {
            id: id.to_string(),
            name: name.to_string(),
            icon: "book".to_string(),
            color: "sage".to_string(),
            operational_definition: String::new(),
            unit: "晚".to_string(),
            target_count: 1,
            confirmation_timing: ConfirmationTiming::NextDay,
            target_mode: if target.is_some() {
                TargetMode::Weekly
            } else {
                TargetMode::RecordOnly
            },
            weekly_target: target,
            if_then: String::new(),
            reminder_time: None,
            sort_order: 0,
            archived_at: None,
            stats_visible: true,
            goal_history: Vec::new(),
            created_at: "2026-10-01T09:00:00.000+08:00".to_string(),
            updated_at: "2026-10-01T09:00:00.000+08:00".to_string(),
        }
    }

    fn event(habit_id: &str, day: &str, status: CheckinStatus) -> CheckinEvent {
        CheckinEvent {
            id: format!("{habit_id}-{day}"),
            habit_id: habit_id.to_string(),
            local_date: day.to_string(),
            status,
            confirmed_at: None,
            source: "manual".to_string(),
            supersedes_id: None,
            recorded_by: "pc".to_string(),
            note: String::new(),
            created_at: format!("{day}T21:00:00.000+08:00"),
        }
    }

    fn note(id: &str, day: &str, hour: &str, text: &str, tags: &[&str]) -> Note {
        Note {
            id: id.to_string(),
            created_at: format!("{day}T{hour}:00.000+08:00"),
            local_date: day.to_string(),
            text: text.to_string(),
            tags: tags.iter().map(|t| t.to_string()).collect(),
            updated_at: format!("{day}T{hour}:00.000+08:00"),
            conflict_of: None,
        }
    }

    fn mood(day: &str, score: i32) -> DailyMood {
        DailyMood {
            id: format!("m-{day}"),
            local_date: day.to_string(),
            score,
            updated_at: format!("{day}T22:00:00.000+08:00"),
        }
    }

    fn data(
        habits: Vec<Habit>,
        events: Vec<CheckinEvent>,
        notes: Vec<Note>,
        moods: Vec<DailyMood>,
    ) -> Data {
        let mut data = Data::empty("pc");
        data.habits = habits;
        data.checkin_events = events;
        data.notes = notes;
        data.daily_moods = moods;
        data
    }

    fn custom(from: &str, to: &str) -> Range {
        resolve_range(TODAY, "custom", from, to).expect("范围合法")
    }

    fn run(sample: &Data, range: Range, scope: &str) -> Stats {
        stats(sample, TODAY, OFFSET, range, scope)
    }

    /// 从 `from` 起连续 `count` 天各写一条同样状态的事件。
    fn days(habit_id: &str, from: &str, count: i64, status: CheckinStatus) -> Vec<CheckinEvent> {
        (0..count)
            .filter_map(|n| add_days(from, n))
            .map(|day| event(habit_id, &day, status))
            .collect()
    }

    #[test]
    fn ranges_land_where_the_design_says() {
        let week = resolve_range(TODAY, "7", "", "").unwrap();
        assert_eq!(
            (week.from.as_str(), week.to.as_str(), week.days),
            ("2026-10-21", TODAY, 7)
        );

        let month = resolve_range(TODAY, "month", "", "").unwrap();
        assert_eq!(
            (month.from.as_str(), month.to.as_str()),
            ("2026-10-01", TODAY),
            "本月截到今天"
        );

        let last = resolve_range(TODAY, "lastMonth", "", "").unwrap();
        assert_eq!(
            (last.from.as_str(), last.to.as_str(), last.days),
            ("2026-09-01", "2026-09-30", 30)
        );

        // 跨月换算不能按 28 天硬写：2027 不是闰年，2028 是。
        let feb = resolve_range("2027-02-15", "month", "", "").unwrap();
        assert_eq!(
            (feb.from.as_str(), feb.to.as_str()),
            ("2027-02-01", "2027-02-15")
        );
        let prev = resolve_range("2027-03-05", "lastMonth", "", "").unwrap();
        assert_eq!(
            (prev.from.as_str(), prev.to.as_str()),
            ("2027-02-01", "2027-02-28")
        );
        let leap = resolve_range("2028-03-01", "lastMonth", "", "").unwrap();
        assert_eq!(leap.to, "2028-02-29");
    }

    #[test]
    fn bad_custom_range_is_an_error_not_a_silent_default() {
        let inverted = resolve_range(TODAY, "custom", "2026-10-20", "2026-10-19").unwrap_err();
        assert!(inverted.contains("不能晚于"), "{inverted}");
        assert!(
            resolve_range(TODAY, "custom", "", "").is_err(),
            "空着不算范围"
        );
        assert!(
            resolve_range(TODAY, "custom", "2026-02-30", "2026-03-01").is_err(),
            "不存在的日期"
        );
        let long = resolve_range(TODAY, "custom", "2025-01-01", TODAY).unwrap_err();
        assert!(long.contains("366"), "{long}");
        assert!(resolve_range(TODAY, "week", "", "").is_err());
    }

    #[test]
    fn denominator_starts_at_the_habit_and_stops_at_yesterday() {
        let mut late = habit("h1", "阅读", Some(4));
        late.created_at = "2026-10-24T09:00:00.000+08:00".to_string();
        let sample = data(
            vec![late],
            vec![
                // 习惯开始之前那条不算：分母从它的 `firstDay` 起算。
                event("h1", "2026-10-20", CheckinStatus::Done),
                event("h1", "2026-10-26", CheckinStatus::Done),
                // 今天已经打卡，但今天不进分母（§6.2）。
                event("h1", "2026-10-27", CheckinStatus::Done),
            ],
            Vec::new(),
            Vec::new(),
        );
        let stats = run(
            &sample,
            resolve_range(TODAY, "7", "", "").unwrap(),
            SCOPE_ALL,
        );
        let row = &stats.habits[0];
        assert_eq!(row.first_day.as_deref(), Some("2026-10-24"));
        assert_eq!((row.done, row.total), (1, 3), "10-24/25/26 三天进分母");
        assert_eq!(row.unconfirmed, 2);
        assert_eq!(stats.yesterday, "2026-10-26");
    }

    #[test]
    fn last_month_range_does_not_leak_into_october() {
        let mut early = habit("h1", "阅读", Some(4));
        early.created_at = "2026-08-01T09:00:00.000+08:00".to_string();
        let sample = data(
            vec![early],
            vec![
                event("h1", "2026-09-20", CheckinStatus::Done),
                event("h1", "2026-10-20", CheckinStatus::Done),
            ],
            Vec::new(),
            Vec::new(),
        );
        let stats = run(
            &sample,
            resolve_range(TODAY, "lastMonth", "", "").unwrap(),
            "h1",
        );
        let row = &stats.habits[0];
        assert_eq!((row.done, row.total), (1, 30), "只数九月那 30 天");
    }

    #[test]
    fn zero_denominator_has_no_rate() {
        let sample = data(
            vec![habit("h1", "阅读", Some(4))],
            Vec::new(),
            Vec::new(),
            Vec::new(),
        );
        // 范围整段在未来：没有该确认的天，界面该是「—」而不是 0%。
        let stats = run(&sample, custom("2026-11-01", "2026-11-07"), SCOPE_ALL);
        assert_eq!(stats.total, 0);
        assert_eq!(stats.rate, None);
        assert_eq!(stats.habits[0].rate, None);
    }

    #[test]
    fn aggregate_rate_sums_days_not_averages() {
        let h1 = habit("h1", "阅读", Some(4));
        let mut h2 = habit("h2", "散步", Some(4));
        h2.sort_order = 1;
        h2.color = "clay".to_string();
        let sample = data(
            vec![h1, h2],
            vec![
                event("h1", "2026-10-20", CheckinStatus::Done),
                event("h1", "2026-10-21", CheckinStatus::Done),
                event("h2", "2026-10-20", CheckinStatus::NotDone),
                event("h2", "2026-10-21", CheckinStatus::NotDone),
            ],
            Vec::new(),
            Vec::new(),
        );
        let stats = run(&sample, custom("2026-10-20", "2026-10-21"), SCOPE_ALL);
        // 两个习惯各 2 天：Σdone=2、Σ分母=4 → 50%，而不是 (100% + 0%) 两行并排。
        assert_eq!((stats.done, stats.total), (2, 4));
        assert!((stats.rate.unwrap() - 0.5).abs() < 1e-9);
    }

    #[test]
    fn streak_counts_completed_weeks_and_ignores_this_one() {
        let mut reading = habit("h1", "阅读", Some(2));
        reading.created_at = "2026-09-01T09:00:00.000+08:00".to_string();
        let mut events = days("h1", "2026-10-19", 2, CheckinStatus::Done);
        events.extend(days("h1", "2026-10-12", 2, CheckinStatus::Done));
        events.extend(days("h1", "2026-10-05", 1, CheckinStatus::Done));
        events.extend(days("h1", "2026-10-06", 1, CheckinStatus::NotDone));
        let stats = run(
            &data(vec![reading], events, Vec::new(), Vec::new()),
            resolve_range(TODAY, "30", "", "").unwrap(),
            SCOPE_ALL,
        );
        // 本周（10-26 起）一天都没记，但不打断上个完整周数出来的 2。
        assert_eq!(stats.habits[0].streak_weeks, Some(2));
        assert_eq!(stats.habits[0].week_done, 0);
        assert_eq!(stats.habits[0].week_met, Some(false));
    }

    #[test]
    fn record_only_habit_has_no_target_to_meet() {
        let stats = run(
            &data(
                vec![habit("h1", "呼吸", None)],
                Vec::new(),
                Vec::new(),
                Vec::new(),
            ),
            resolve_range(TODAY, "7", "", "").unwrap(),
            SCOPE_ALL,
        );
        assert_eq!(stats.habits[0].week_target, None);
        assert_eq!(stats.habits[0].week_met, None);
        assert_eq!(stats.habits[0].streak_weeks, None);
        assert_eq!(stats.habits[0].week_mode, "recordOnly");
    }

    #[test]
    fn goal_change_uses_the_week_it_was_effective() {
        let mut reading = habit("h1", "阅读", Some(5));
        reading.created_at = "2026-09-01T09:00:00.000+08:00".to_string();
        // 习惯上的当前值已经是「本周起 1 晚」，上周仍按 5 晚判（§2.4 目标历史）。
        reading.weekly_target = Some(1);
        reading.goal_history = vec![
            GoalEntry {
                effective_from: "2026-09-01".to_string(),
                target_mode: TargetMode::Weekly,
                weekly_target: Some(5),
            },
            GoalEntry {
                effective_from: "2026-10-26".to_string(),
                target_mode: TargetMode::Weekly,
                weekly_target: Some(1),
            },
        ];
        let events = days("h1", "2026-10-19", 4, CheckinStatus::Done);
        let stats = run(
            &data(vec![reading], events, Vec::new(), Vec::new()),
            resolve_range(TODAY, "7", "", "").unwrap(),
            SCOPE_ALL,
        );
        assert_eq!(stats.habits[0].week_target, Some(1), "本周按新目标");
        assert_eq!(
            stats.habits[0].streak_weeks,
            Some(0),
            "上周 4/5 没达标，连续就是 0"
        );
    }

    fn cell_at<'a>(row: &'a HeatRow, day: &str) -> &'a HeatCell {
        row.cells
            .iter()
            .find(|c| c.date == day)
            .unwrap_or_else(|| panic!("{day} 不在格子里"))
    }

    #[test]
    fn heatmap_blanks_future_and_rings_today() {
        let sample = data(
            vec![habit("h1", "阅读", Some(4))],
            vec![
                event("h1", "2026-10-20", CheckinStatus::Done),
                event("h1", "2026-10-21", CheckinStatus::NotDone),
            ],
            Vec::new(),
            Vec::new(),
        );
        let stats = run(&sample, custom("2026-10-20", "2026-10-28"), SCOPE_ALL);
        // 只有一个习惯时不重复画聚合行。
        assert_eq!(stats.heatmap.rows.len(), 1);
        let row = &stats.heatmap.rows[0];
        assert_eq!(cell_at(row, "2026-10-20").level, "l2");
        assert_eq!(cell_at(row, "2026-10-21").level, "l0");
        assert_eq!(cell_at(row, "2026-10-22").level, "pending");
        assert_eq!(
            cell_at(row, "2026-10-28").level,
            "blank",
            "未来留白，不是没做"
        );
        assert!(cell_at(row, "2026-10-27").today, "今天只靠描边认");
        assert!(!cell_at(row, "2026-10-26").today);
    }

    #[test]
    fn cells_stay_blank_before_the_habit_started() {
        let mut late = habit("h1", "新习惯", Some(2));
        late.created_at = "2026-10-24T09:00:00.000+08:00".to_string();
        let stats = run(
            &data(vec![late], Vec::new(), Vec::new(), Vec::new()),
            resolve_range(TODAY, "30", "", "").unwrap(),
            SCOPE_ALL,
        );
        let row = &stats.heatmap.rows[0];
        assert_eq!(row.cells.len(), 30);
        let blank = row.cells.iter().filter(|c| c.level == "blank").count();
        assert_eq!(blank, 26, "09-28~10-23 共 26 格全透明");
        assert_eq!(
            cell_at(row, "2026-10-24").level,
            "pending",
            "开始那天起才有描边"
        );
    }

    #[test]
    fn aggregate_row_reads_partial_progress() {
        let h1 = habit("h1", "阅读", Some(4));
        let mut h2 = habit("h2", "散步", Some(4));
        h2.sort_order = 1;
        let sample = data(
            vec![h1, h2],
            vec![
                event("h1", "2026-10-20", CheckinStatus::Done),
                event("h2", "2026-10-20", CheckinStatus::NotDone),
                event("h1", "2026-10-21", CheckinStatus::Done),
                event("h2", "2026-10-21", CheckinStatus::Done),
            ],
            Vec::new(),
            Vec::new(),
        );
        let stats = run(&sample, custom("2026-10-20", "2026-10-22"), SCOPE_ALL);
        let row = &stats.heatmap.rows[0];
        assert_eq!(row.id, SCOPE_ALL);
        assert_eq!(row.name, "全部习惯");
        assert_eq!(
            cell_at(row, "2026-10-20").level,
            "l1",
            "一个 done 一个没做 = 部分完成"
        );
        assert_eq!(cell_at(row, "2026-10-21").level, "l2", "两个都 done = 达标");
        assert_eq!(cell_at(row, "2026-10-22").level, "pending", "谁都没确认");
        assert_eq!(stats.heatmap.rows.len(), 3, "聚合行 + 每习惯一行");
    }

    #[test]
    fn archived_habit_leaves_stats_when_told_to() {
        let mut gone = habit("h2", "散步", Some(4));
        gone.archived_at = Some("2026-10-25T09:00:00.000+08:00".to_string());
        gone.stats_visible = false;
        let mut kept = habit("h3", "早睡", Some(4));
        kept.archived_at = Some("2026-10-25T09:00:00.000+08:00".to_string());
        kept.sort_order = 2;
        let sample = data(
            vec![habit("h1", "阅读", Some(4)), gone, kept],
            Vec::new(),
            Vec::new(),
            Vec::new(),
        );
        // 选中的习惯已经关了统计开关：退回「全部」，不给一个空面板。
        let stats = run(&sample, resolve_range(TODAY, "7", "", "").unwrap(), "h2");
        assert_eq!(stats.scope, SCOPE_ALL);
        assert_eq!(stats.scope_name, "全部");
        let ids: Vec<&str> = stats.habits.iter().map(|h| h.id.as_str()).collect();
        assert_eq!(ids, vec!["h1", "h3"], "归档但开着统计开关的还在");
        // 选一个真实参与统计的习惯就按它自己算。
        let single = run(&sample, resolve_range(TODAY, "7", "", "").unwrap(), "h3");
        assert_eq!(single.scope, "h3");
        assert_eq!(single.scope_name, "早睡");
        assert_eq!(single.habits.len(), 1);
        // 但范围选择器仍列全量候选，否则切到「早睡」后就再也切不回「阅读」。
        let options: Vec<&str> = single.scope_options.iter().map(|o| o.id.as_str()).collect();
        assert_eq!(options, vec!["h1", "h3"]);
    }

    #[test]
    fn volume_switches_from_days_to_weeks() {
        let notes = vec![
            note("n1", "2026-10-20", "05:00", "早上的第一条", &["阅读"]),
            note(
                "n2",
                "2026-10-20",
                "23:10",
                "深夜又想起一句",
                &["阅读", "情绪"],
            ),
        ];
        let sample = data(Vec::new(), Vec::new(), notes, Vec::new());
        let week = run(&sample, custom("2026-10-20", "2026-10-26"), SCOPE_ALL).volume;
        assert!(week.per_day);
        assert_eq!(week.buckets.len(), 7);
        assert_eq!(week.buckets[0].count, 2);
        assert!(
            (week.average - 2.0 / 7.0).abs() < 1e-9,
            "平均每天 {}/7",
            week.total
        );
        assert_eq!(week.total, 2);

        let months = run(
            &sample,
            resolve_range(TODAY, "90", "", "").unwrap(),
            SCOPE_ALL,
        )
        .volume;
        assert!(!months.per_day);
        assert_eq!(months.buckets.len(), 13, "90 天切成 13 格");
        assert_eq!(months.buckets[0].start, "2026-07-30");
        assert_eq!(months.buckets.last().unwrap().end, TODAY);
        assert_eq!(months.total, 2, "范围里的记录一条不少");
    }

    #[test]
    fn time_buckets_follow_the_real_clock() {
        let notes = vec![
            note("n1", "2026-10-21", "04:30", "四点以后算今天", &[]),
            note("n2", "2026-10-21", "09:00", "上午", &[]),
            note("n3", "2026-10-21", "17:00", "傍晚", &[]),
            note("n4", "2026-10-21", "22:00", "夜里", &[]),
            // 凌晨 02:35 归属前一天（localDate 写入时已算好），但时段仍按真实钟点算深夜。
            note("n5", "2026-10-20", "02:35", "还没睡", &[]),
        ];
        let volume = run(
            &data(Vec::new(), Vec::new(), notes, Vec::new()),
            custom("2026-10-20", "2026-10-21"),
            SCOPE_ALL,
        )
        .volume;
        let counts: HashMap<&str, usize> = volume.times.iter().map(|b| (b.key, b.count)).collect();
        assert_eq!(counts["dawn"], 1);
        assert_eq!(counts["day"], 1);
        assert_eq!(counts["evening"], 1);
        assert_eq!(counts["night"], 2, "22:00 和 02:35 都算深夜");
        assert_eq!(
            volume.times.iter().map(|b| b.count).sum::<usize>(),
            volume.total
        );
    }

    #[test]
    fn top_tags_are_five_and_ordered() {
        let mut notes = Vec::new();
        for (index, tag) in ["工作", "阅读", "家庭", "情绪", "散步", "观察记录"]
            .iter()
            .enumerate()
        {
            let day = add_days("2026-10-20", index as i64).unwrap();
            notes.push(note(&format!("n{index}"), &day, "10:00", "一句", &[tag]));
        }
        notes.push(note("n9", "2026-10-20", "11:00", "再来一条", &["工作"]));
        let volume = run(
            &data(Vec::new(), Vec::new(), notes, Vec::new()),
            custom("2026-10-20", "2026-10-26"),
            SCOPE_ALL,
        )
        .volume;
        let tags: Vec<&str> = volume.tags.iter().map(|t| t.tag.as_str()).collect();
        assert_eq!(tags.len(), 5, "只留 Top 5");
        assert_eq!(tags[0], "工作", "次数多的在前");
        assert_eq!(volume.tags[0].count, 2);
    }

    #[test]
    fn writing_streak_tolerates_a_blank_today() {
        let notes = vec![
            note("n1", "2026-10-25", "10:00", "前天", &[]),
            note("n2", "2026-10-26", "10:00", "昨天", &[]),
        ];
        let sample = data(Vec::new(), Vec::new(), notes, Vec::new());
        // 今天还没写，不该因此归零（§6.3）。
        assert_eq!(writing_streak(&sample, TODAY), 2);
        let mut fuller = sample.clone();
        fuller.notes.push(note("n3", TODAY, "10:00", "今天", &[]));
        assert_eq!(writing_streak(&fuller, TODAY), 3);
        // 中间断了就从头数到这里。
        let mut broken = sample.clone();
        broken.notes.remove(0);
        assert_eq!(writing_streak(&broken, TODAY), 1);
        assert_eq!(writing_streak(&Data::empty("pc"), TODAY), 0);
    }

    #[test]
    fn tombstoned_note_leaves_both_counts() {
        let notes = vec![
            note("n1", "2026-10-25", "10:00", "留着", &[]),
            note("n2", "2026-10-26", "10:00", "删掉", &[]),
        ];
        let mut sample = data(Vec::new(), Vec::new(), notes, Vec::new());
        sample.tombstones.push(Tombstone {
            entity_type: EntityType::Note,
            entity_id: "n2".to_string(),
            deleted_at: "2026-10-26T22:00:00.000+08:00".to_string(),
            deleted_by: "pc".to_string(),
        });
        let volume = run(&sample, custom("2026-10-25", TODAY), SCOPE_ALL).volume;
        assert_eq!(volume.total, 1);
        assert_eq!(volume.streak_days, 0, "昨天那条被删了，连续就断在昨天");
    }

    #[test]
    fn mood_needs_two_points_before_a_line() {
        let one = run(
            &data(
                Vec::new(),
                Vec::new(),
                Vec::new(),
                vec![mood("2026-10-21", 70)],
            ),
            custom("2026-10-20", "2026-10-26"),
            SCOPE_ALL,
        )
        .mood;
        assert!(!one.enough, "只有一个点就不画线");
        assert_eq!(one.count, 1);
        assert!(!one.use_weekly);

        let moods = vec![mood("2026-10-20", 20), mood("2026-10-21", 80)];
        let two = run(
            &data(Vec::new(), Vec::new(), Vec::new(), moods),
            custom("2026-10-20", "2026-10-26"),
            SCOPE_ALL,
        )
        .mood;
        assert!(two.enough);
        assert!((two.average.unwrap() - 50.0).abs() < 1e-9);
        let bands: Vec<usize> = two.bands.iter().map(|b| b.count).collect();
        assert_eq!(bands, vec![0, 1, 0, 0, 1], "20 分在第二段、80 分在第五段");
        assert_eq!(two.days.len(), 2);

        // 长范围按周均值，点的值仍是 0–100 的原始分数。
        let long = run(
            &data(
                Vec::new(),
                Vec::new(),
                Vec::new(),
                vec![mood("2026-10-20", 20), mood("2026-10-21", 80)],
            ),
            resolve_range(TODAY, "90", "", "").unwrap(),
            SCOPE_ALL,
        )
        .mood;
        assert!(long.use_weekly);
        assert_eq!(long.weeks.len(), 1, "两条都在同一个自然周，合成一个点");
        assert_eq!(long.weeks[0].days, 2);
        assert_eq!(long.weeks[0].date, "2026-10-19");
    }

    #[test]
    fn tombstoned_mood_is_not_a_score() {
        let mut sample = data(
            Vec::new(),
            Vec::new(),
            Vec::new(),
            vec![mood("2026-10-21", 70), mood("2026-10-22", 30)],
        );
        sample.tombstones.push(Tombstone {
            entity_type: EntityType::DailyMood,
            entity_id: "m-2026-10-21".to_string(),
            deleted_at: "2026-10-26T22:00:00.000+08:00".to_string(),
            deleted_by: "pc".to_string(),
        });
        let mood = run(&sample, custom("2026-10-20", TODAY), SCOPE_ALL).mood;
        assert_eq!(mood.count, 1, "墓碑那条不算分");
        assert!(!mood.enough, "只剩一个点就不画线");
    }

    #[test]
    fn compare_needs_four_days_on_each_side() {
        let sample = data(
            vec![habit("h1", "阅读", Some(4))],
            vec![
                event("h1", "2026-10-20", CheckinStatus::Done),
                event("h1", "2026-10-21", CheckinStatus::Done),
                event("h1", "2026-10-22", CheckinStatus::NotDone),
            ],
            Vec::new(),
            vec![
                mood("2026-10-20", 80),
                mood("2026-10-21", 90),
                mood("2026-10-22", 10),
            ],
        );
        let single = run(&sample, custom("2026-10-19", "2026-10-25"), "h1");
        let compare = single.compare.expect("单习惯才有对比");
        assert_eq!((compare.high_days, compare.low_days), (2, 1));
        assert!(!compare.enough, "样本还少就别画对比");

        // 凑够 4 + 4。
        let mut events = days("h1", "2026-10-12", 4, CheckinStatus::Done);
        events.extend(days("h1", "2026-10-16", 4, CheckinStatus::NotDone));
        let moods: Vec<DailyMood> = (12..20)
            .map(|d| mood(&format!("2026-10-{d}"), if d < 16 { 80 } else { 20 }))
            .collect();
        let enough = run(
            &data(
                vec![habit("h1", "阅读", Some(4))],
                events,
                Vec::new(),
                moods,
            ),
            custom("2026-10-12", "2026-10-19"),
            "h1",
        )
        .compare
        .expect("单习惯才有对比");
        assert!(
            enough.enough,
            "high {} / low {}",
            enough.high_days, enough.low_days
        );
        assert_eq!(enough.high_average, Some(80.0));
        assert_eq!(enough.low_average, Some(20.0));
    }

    #[test]
    fn aggregate_scope_skips_the_mood_compare() {
        let sample = data(
            vec![habit("h1", "阅读", Some(4)), habit("h2", "散步", Some(4))],
            Vec::new(),
            Vec::new(),
            Vec::new(),
        );
        assert!(run(&sample, custom("2026-10-20", "2026-10-26"), SCOPE_ALL)
            .compare
            .is_none());
    }

    #[test]
    fn review_looks_at_the_last_complete_week() {
        let mut talking = habit("h1", "说话", Some(2));
        talking.if_then = "如果客户先沉默，就等五秒。".to_string();
        let events = vec![
            event("h1", "2026-10-19", CheckinStatus::Done),
            event("h1", "2026-10-20", CheckinStatus::Done),
            // 本周（10-26 起）的打卡不该进上周的回顾。
            event("h1", "2026-10-26", CheckinStatus::Done),
        ];
        let notes = vec![
            note("n1", "2026-10-21", "10:00", "短的一句", &["情绪"]),
            note(
                "n2",
                "2026-10-22",
                "10:00",
                "这一句长一些，值得留下",
                &["情绪"],
            ),
            note("n3", "2026-10-26", "10:00", "本周的不算", &["工作"]),
        ];
        let sample = data(vec![talking], events, notes, Vec::new());
        // 回顾固定在最近一个完整的周一–周日，与范围选择无关（§6.5）。
        let review = run(&sample, custom("2026-09-01", "2026-09-30"), SCOPE_ALL).review;
        assert_eq!(
            (review.week_start.as_str(), review.week_end.as_str()),
            ("2026-10-19", "2026-10-25")
        );
        assert_eq!(
            review.lines,
            vec![
                "说话：完成 2/2 晚，达标",
                "记录：2 条，最常写到 #情绪（2 次）",
                "值得留下的一句：2026-10-22 —「这一句长一些，值得留下」",
                "下周的 if–then：说话 — 如果客户先沉默，就等五秒。",
            ]
        );
    }

    #[test]
    fn review_is_factual_when_nothing_was_recorded() {
        let review = run(
            &Data::empty("pc"),
            resolve_range(TODAY, "7", "", "").unwrap(),
            SCOPE_ALL,
        )
        .review;
        assert_eq!(
            review.lines,
            vec!["还没有习惯可以回顾。", "这一周没有记录。"]
        );
    }

    #[test]
    fn review_reports_a_habit_with_no_target_plainly() {
        let sample = data(
            vec![habit("h1", "呼吸", None)],
            vec![event("h1", "2026-10-20", CheckinStatus::Done)],
            Vec::new(),
            Vec::new(),
        );
        let review = run(&sample, custom("2026-10-19", "2026-10-25"), SCOPE_ALL).review;
        assert_eq!(review.lines[0], "呼吸：1 晚");
    }

    #[test]
    fn long_quote_is_cut_not_rewritten() {
        let long = "一".repeat(60);
        let notes = vec![note("n1", "2026-10-19", "10:00", &long, &[])];
        let mut sample = data(Vec::new(), Vec::new(), Vec::new(), Vec::new());
        sample.notes = notes;
        let review = run(&sample, custom("2026-10-19", "2026-10-25"), SCOPE_ALL).review;
        let quote = review
            .lines
            .iter()
            .find(|l| l.starts_with("值得留下的一句"))
            .expect("有记录就该有一句");
        assert!(
            quote.contains(&"一".repeat(40)) && quote.contains('…'),
            "{quote}"
        );
        assert!(!quote.contains(&"一".repeat(41)), "只留 40 字");
    }
}
