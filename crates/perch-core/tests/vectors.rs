//! 共享 JSON 测试向量的执行端（DESIGN §5.4、§10）。
//!
//! `tests/vectors/*.json` 里只有输入和期望，不含任何 Rust 或 Kotlin 类型。
//! Android 端将来读同一批文件、断言同一批期望——口径分歧只能在这里被发现。

use std::collections::BTreeMap;
use std::path::{Path, PathBuf};

use perch_core::date_boundary::{
    self, add_days, days_between, format_instant, local_date, next_week_start, parse_instant,
    week_start, weekday_mon_first, DAY_START_HOUR,
};
use perch_core::model::{Data, GoalEntry, TargetMode};
use perch_core::{goal, habits};
use serde::Deserialize;

#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase")]
struct Vector {
    name: String,
    #[serde(default)]
    #[allow(dead_code)]
    note: String,
    #[serde(default)]
    data: Option<Data>,
    #[serde(default)]
    local_date: Vec<LocalDateCase>,
    #[serde(default)]
    parse_instant: Vec<ParseInstantCase>,
    #[serde(default)]
    parse_instant_rejected: Vec<String>,
    #[serde(default)]
    format_instant: Vec<FormatInstantCase>,
    #[serde(default)]
    dates: Vec<DateCase>,
    #[serde(default)]
    invalid_dates: Vec<String>,
    #[serde(default)]
    date_range: Vec<RangeCase>,
    #[serde(default)]
    days_between: Vec<BetweenCase>,
    #[serde(default)]
    status_on: Vec<StatusCase>,
    #[serde(default)]
    day_counts: Vec<CountsCase>,
    #[serde(default)]
    unconfirmed_days: Vec<DaysListCase>,
    #[serde(default)]
    done_days: Vec<DaysListCase>,
    #[serde(default)]
    today_habit_ids: Vec<String>,
    #[serde(default)]
    management_habit_ids: Vec<String>,
    #[serde(default)]
    stats_habit_ids: Vec<String>,
    #[serde(default)]
    goal_on: Vec<GoalOnCase>,
    #[serde(default)]
    pending_goal_dates: Vec<PendingCase>,
    #[serde(default)]
    first_day: Vec<FirstDayCase>,
    #[serde(default)]
    push_goal: Vec<PushCase>,
    /// 认不出来的键收在这里，测试直接失败——静默忽略等于向量写错了也没人知道。
    #[serde(flatten)]
    unknown: BTreeMap<String, serde_json::Value>,
}

#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase")]
struct LocalDateCase {
    instant_ms: i64,
    offset_secs: i32,
    #[serde(default = "default_day_start_hour")]
    day_start_hour: u32,
    expect: String,
}

fn default_day_start_hour() -> u32 {
    DAY_START_HOUR
}

#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase")]
struct ParseInstantCase {
    iso: String,
    expect_ms: i64,
}

#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase")]
struct FormatInstantCase {
    instant_ms: i64,
    offset_secs: i32,
    expect: String,
}

#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase")]
struct DateCase {
    date: String,
    #[serde(default)]
    expect_weekday: Option<u32>,
    #[serde(default)]
    expect_week_start: Option<String>,
    #[serde(default)]
    expect_next_week_start: Option<String>,
    #[serde(default)]
    expect_plus1: Option<String>,
    #[serde(default)]
    expect_minus1: Option<String>,
}

#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase")]
struct RangeCase {
    from: String,
    to: String,
    expect: Vec<String>,
}

#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase")]
struct BetweenCase {
    from: String,
    to: String,
    expect: i64,
}

#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase")]
struct StatusCase {
    habit_id: String,
    local_date: String,
    expect: String,
}

/// 核心类型不为了测试挂 serde，这里按向量的写法镜像一份。
#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase")]
struct CountsExpect {
    done: i64,
    not_done: i64,
    unconfirmed: i64,
}

#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase")]
struct CountsCase {
    habit_id: String,
    from: String,
    to: String,
    expect: CountsExpect,
}

#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase")]
struct DaysListCase {
    habit_id: String,
    from: String,
    to: String,
    expect: Vec<String>,
}

#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase")]
struct GoalExpect {
    target_mode: TargetMode,
    weekly_target: Option<i32>,
    effective_from: String,
}

#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase")]
struct GoalOnCase {
    habit_id: String,
    date: String,
    expect: GoalExpect,
}

#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase")]
struct PendingCase {
    habit_id: String,
    today: String,
    expect: Vec<String>,
}

#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase")]
struct FirstDayCase {
    habit_id: String,
    offset_secs: i32,
    expect: String,
}

#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase")]
struct PushCase {
    habit_id: String,
    entry: GoalEntry,
    #[serde(default)]
    expect_rejected: bool,
    expect_effective_from: Vec<String>,
    expect_target_mode: TargetMode,
    expect_weekly_target: Option<i32>,
}

fn vectors_dir() -> PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR"))
        .join("../../tests/vectors")
        .canonicalize()
        .expect("tests/vectors 目录应该存在")
}

#[test]
fn shared_vectors_match_the_rust_implementation() {
    let mut files: Vec<PathBuf> = std::fs::read_dir(vectors_dir())
        .expect("读不到向量目录")
        .flatten()
        .map(|e| e.path())
        .filter(|p| p.extension().is_some_and(|e| e == "json"))
        .collect();
    files.sort();
    assert!(
        !files.is_empty(),
        "一条向量都没有，说明 tests/vectors 丢了或路径错了"
    );

    let mut checked = 0usize;
    for path in &files {
        let text = std::fs::read_to_string(path).unwrap();
        let vector: Vector = match serde_json::from_str(&text) {
            Ok(v) => v,
            Err(e) => panic!("{} 读不出来: {e}", display(path)),
        };
        assert!(
            vector.unknown.is_empty(),
            "{} 有不认识的字段 {:?}（写错字段名的话这条向量等于没测）",
            display(path),
            vector.unknown.keys()
        );
        checked += run(&vector, &display(path));
    }
    assert!(
        checked >= 60,
        "断言数量异常少（只有 {checked} 条），向量可能被漏读了"
    );
    println!("共享向量 {checked} 条断言全部通过");
}

fn run(v: &Vector, file: &str) -> usize {
    let ctx = |what: &str, i: usize| format!("{file} · {} · {what}[{i}]", v.name);
    let mut n = 0;

    for (i, c) in v.local_date.iter().enumerate() {
        let got = local_date(c.instant_ms, c.offset_secs, c.day_start_hour);
        assert_eq!(got, c.expect, "{}", ctx("localDate", i));
        n += 1;
    }
    for (i, c) in v.parse_instant.iter().enumerate() {
        let got = parse_instant(&c.iso);
        assert_eq!(got, Some(c.expect_ms), "{}", ctx("parseInstant", i));
        n += 1;
    }
    for (i, iso) in v.parse_instant_rejected.iter().enumerate() {
        assert_eq!(parse_instant(iso), None, "{} 本该读不出来", ctx("拒绝", i));
        n += 1;
    }
    for (i, c) in v.format_instant.iter().enumerate() {
        let got = format_instant(c.instant_ms, c.offset_secs);
        assert_eq!(got, c.expect, "{}", ctx("formatInstant", i));
        n += 1;
        // 写出去的字符串必须能读回来，否则两端各自的格式化代码可以随便错。
        assert_eq!(
            parse_instant(&got),
            Some(c.instant_ms),
            "{} 不是往返一致",
            ctx("formatInstant", i)
        );
        n += 1;
    }
    for (i, c) in v.dates.iter().enumerate() {
        if let Some(want) = c.expect_weekday {
            assert_eq!(
                weekday_mon_first(&c.date),
                Some(want),
                "{}",
                ctx("dates", i)
            );
            n += 1;
        }
        if let Some(want) = &c.expect_week_start {
            assert_eq!(&week_start(&c.date).unwrap(), want, "{}", ctx("dates", i));
            n += 1;
        }
        if let Some(want) = &c.expect_next_week_start {
            assert_eq!(
                &next_week_start(&c.date).unwrap(),
                want,
                "{}",
                ctx("dates", i)
            );
            n += 1;
        }
        if let Some(want) = &c.expect_plus1 {
            assert_eq!(&add_days(&c.date, 1).unwrap(), want, "{}", ctx("dates", i));
            n += 1;
        }
        if let Some(want) = &c.expect_minus1 {
            assert_eq!(&add_days(&c.date, -1).unwrap(), want, "{}", ctx("dates", i));
            n += 1;
        }
    }
    for (i, bad) in v.invalid_dates.iter().enumerate() {
        assert_eq!(
            date_boundary::parse_date(bad),
            None,
            "{file} · 非法日期[{i}]: {bad:?}"
        );
        n += 1;
    }
    for (i, c) in v.date_range.iter().enumerate() {
        let got = date_boundary::date_range(&c.from, &c.to).unwrap();
        assert_eq!(got, c.expect, "{}", ctx("dateRange", i));
        n += 1;
    }
    for (i, c) in v.days_between.iter().enumerate() {
        assert_eq!(
            days_between(&c.from, &c.to),
            Some(c.expect),
            "{}",
            ctx("daysBetween", i)
        );
        n += 1;
    }

    // 下面这些段落都要一份数据快照。
    let needs_data = !(v.status_on.is_empty()
        && v.day_counts.is_empty()
        && v.unconfirmed_days.is_empty()
        && v.done_days.is_empty()
        && v.today_habit_ids.is_empty()
        && v.management_habit_ids.is_empty()
        && v.stats_habit_ids.is_empty()
        && v.goal_on.is_empty()
        && v.pending_goal_dates.is_empty()
        && v.first_day.is_empty()
        && v.push_goal.is_empty());
    let data = match &v.data {
        Some(data) => data,
        None if needs_data => panic!("{file} · 有断言但没有 data 快照"),
        None => return n,
    };

    for (i, c) in v.status_on.iter().enumerate() {
        let got = status_text(habits::status_on(data, &c.habit_id, &c.local_date));
        assert_eq!(got, c.expect.as_str(), "{}", ctx("statusOn", i));
        n += 1;
    }
    for (i, c) in v.day_counts.iter().enumerate() {
        let got = habits::counts(data, &c.habit_id, &c.from, &c.to).expect("范围日期必须合法");
        assert_eq!(
            (got.done, got.not_done, got.unconfirmed),
            (c.expect.done, c.expect.not_done, c.expect.unconfirmed),
            "{}",
            ctx("dayCounts", i)
        );
        assert_eq!(
            got.denominator(),
            c.expect.done + c.expect.not_done + c.expect.unconfirmed,
            "{} 分母不等于三态之和",
            ctx("dayCounts", i)
        );
        n += 1;
    }
    for (i, c) in v.unconfirmed_days.iter().enumerate() {
        assert_eq!(
            habits::unconfirmed_days(data, &c.habit_id, &c.from, &c.to),
            c.expect,
            "{}",
            ctx("unconfirmedDays", i)
        );
        n += 1;
    }
    for (i, c) in v.done_days.iter().enumerate() {
        assert_eq!(
            habits::done_days(data, &c.habit_id, &c.from, &c.to),
            c.expect,
            "{}",
            ctx("doneDays", i)
        );
        n += 1;
    }
    n += check_ids(
        "todayHabitIds",
        &v.today_habit_ids,
        habits::today_list(data),
    );
    n += check_ids(
        "managementHabitIds",
        &v.management_habit_ids,
        habits::management_list(data),
    );
    n += check_ids(
        "statsHabitIds",
        &v.stats_habit_ids,
        habits::stats_list(data),
    );

    for (i, c) in v.goal_on.iter().enumerate() {
        let habit = find_habit(data, &c.habit_id, file);
        let got = goal::goal_on(habit, &c.date);
        assert_eq!(
            (got.mode, got.weekly_target, &got.effective_from),
            (
                c.expect.target_mode,
                c.expect.weekly_target,
                &c.expect.effective_from
            ),
            "{}",
            ctx("goalOn", i)
        );
        n += 1;
    }
    for (i, c) in v.pending_goal_dates.iter().enumerate() {
        let habit = find_habit(data, &c.habit_id, file);
        let got: Vec<String> = goal::pending(habit, &c.today)
            .iter()
            .map(|e| e.effective_from.clone())
            .collect();
        assert_eq!(got, c.expect, "{}", ctx("pendingGoalDates", i));
        n += 1;
    }
    for (i, c) in v.first_day.iter().enumerate() {
        let habit = find_habit(data, &c.habit_id, file);
        assert_eq!(
            goal::first_day(habit, c.offset_secs),
            Some(c.expect.clone()),
            "{}",
            ctx("firstDay", i)
        );
        n += 1;
    }
    for (i, c) in v.push_goal.iter().enumerate() {
        let mut habit = find_habit(data, &c.habit_id, file).clone();
        let rejected = !goal::push_goal(&mut habit, c.entry.clone());
        assert_eq!(rejected, c.expect_rejected, "{}", ctx("pushGoal", i));
        let got: Vec<String> = habit
            .goal_history
            .iter()
            .map(|e| e.effective_from.clone())
            .collect();
        assert_eq!(got, c.expect_effective_from, "{}", ctx("pushGoal", i));
        assert_eq!(
            habit.target_mode,
            c.expect_target_mode,
            "{}",
            ctx("pushGoal", i)
        );
        assert_eq!(
            habit.weekly_target,
            c.expect_weekly_target,
            "{}",
            ctx("pushGoal", i)
        );
        n += 1;
    }
    n
}

fn check_ids(section: &str, want: &[String], got: Vec<&perch_core::model::Habit>) -> usize {
    if want.is_empty() {
        return 0;
    }
    let ids: Vec<String> = got.iter().map(|h| h.id.clone()).collect();
    assert_eq!(ids, want.to_vec(), "{section} 名单不一致");
    1
}

fn find_habit<'a>(data: &'a Data, id: &str, file: &str) -> &'a perch_core::model::Habit {
    data.habits
        .iter()
        .find(|h| h.id == id)
        .unwrap_or_else(|| panic!("{file} · 快照里没有习惯 {id:?}"))
}

fn status_text(s: habits::DayStatus) -> &'static str {
    match s {
        habits::DayStatus::Done => "done",
        habits::DayStatus::NotDone => "notDone",
        habits::DayStatus::Unconfirmed => "unconfirmed",
    }
}

fn display(path: &Path) -> String {
    path.file_name().unwrap().to_string_lossy().into_owned()
}
