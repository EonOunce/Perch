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
use perch_core::{goal, habits, merge};
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
    merge: Vec<MergeCase>,
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

/// 一条合并用例：两份快照 + 可选基线 + 期望结果（DESIGN §4.5）。
/// 期望只写业务字段，顺序就是合并该产出的稳定顺序（集合并按 id / 自然键排好）。
#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase")]
struct MergeCase {
    label: String,
    local: Data,
    remote: Data,
    #[serde(default)]
    base: Option<Data>,
    #[serde(default)]
    expect: MergeExpect,
    #[serde(flatten)]
    unknown: BTreeMap<String, serde_json::Value>,
}

#[derive(Debug, Default, Deserialize)]
#[serde(rename_all = "camelCase")]
struct MergeExpect {
    #[serde(default)]
    schema_version: Option<u32>,
    /// 远端那份快照本机读不懂吗（§4.7）——断言的是输入，不是合并结果。
    #[serde(default)]
    unknown_schema: Option<bool>,
    #[serde(default)]
    habits: Option<Vec<HabitRow>>,
    #[serde(default)]
    goals: Option<Vec<GoalRow>>,
    #[serde(default)]
    markers: Option<Vec<MarkerRow>>,
    #[serde(default)]
    checkin_events: Option<Vec<EventRow>>,
    #[serde(default)]
    notes: Option<Vec<NoteRow>>,
    #[serde(default)]
    moods: Option<Vec<MoodRow>>,
    #[serde(default)]
    tombstones: Option<Vec<GraveRow>>,
    #[serde(default)]
    removed_habit_ids: Option<Vec<String>>,
    #[serde(default)]
    today_habit_ids: Option<Vec<String>>,
    #[serde(default)]
    stats_habit_ids: Option<Vec<String>>,
    /// 合并后交给读取侧裁决，确认「同晚只有一个有效状态」没被合并破坏。
    #[serde(default)]
    effective_statuses: Option<Vec<StatusCase>>,
    #[serde(flatten)]
    unknown: BTreeMap<String, serde_json::Value>,
}

#[derive(Debug, Deserialize, PartialEq)]
#[serde(rename_all = "camelCase")]
struct HabitRow {
    id: String,
    name: String,
    #[serde(default)]
    unit: String,
    #[serde(default)]
    weekly_target: Option<i32>,
}

#[derive(Debug, Deserialize, PartialEq)]
#[serde(rename_all = "camelCase")]
struct GoalRow {
    habit_id: String,
    effective_from: String,
    target_mode: String,
    #[serde(default)]
    weekly_target: Option<i32>,
}

#[derive(Debug, Deserialize, PartialEq)]
#[serde(rename_all = "camelCase")]
struct MarkerRow {
    id: String,
    label: String,
}

#[derive(Debug, Deserialize, PartialEq)]
#[serde(rename_all = "camelCase")]
struct EventRow {
    id: String,
    local_date: String,
    status: String,
    #[serde(default)]
    supersedes_id: Option<String>,
}

#[derive(Debug, Deserialize, PartialEq)]
#[serde(rename_all = "camelCase")]
struct NoteRow {
    id: String,
    text: String,
    #[serde(default)]
    conflict_of: Option<String>,
}

#[derive(Debug, Deserialize, PartialEq)]
#[serde(rename_all = "camelCase")]
struct MoodRow {
    id: String,
    local_date: String,
    score: i32,
}

#[derive(Debug, Deserialize, PartialEq)]
#[serde(rename_all = "camelCase")]
struct GraveRow {
    entity_type: String,
    entity_id: String,
    deleted_at: String,
    deleted_by: String,
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

    // 合并用例自带两份快照，不依赖上面的 data，所以放在「有 data 才能跑」那段之前。
    for (i, c) in v.merge.iter().enumerate() {
        n += check_merge(c, i, &v.name, file);
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

fn check_merge(c: &MergeCase, i: usize, name: &str, file: &str) -> usize {
    let ctx = || format!("{file} · {name} · merge[{i}]「{}」", c.label);
    assert!(
        c.unknown.is_empty(),
        "{} 有不认识的字段 {:?}",
        ctx(),
        c.unknown.keys()
    );
    assert!(
        c.expect.unknown.is_empty(),
        "{} 的 expect 有不认识的字段 {:?}",
        ctx(),
        c.expect.unknown.keys()
    );

    let merged = merge::merge(&c.local, &c.remote, c.base.as_ref());
    // 两台设备各算各的：交换来路后内容必须一模一样，否则永远合不到一起、每次都「有变化」。
    let flipped = merge::merge(&c.remote, &c.local, c.base.as_ref());
    assert_eq!(
        merge::fingerprint(&merged),
        merge::fingerprint(&flipped),
        "{} 不对称",
        ctx()
    );
    // 幂等：拿合并结果再跟自己合一次，不该变出别的东西。
    let twice = merge::merge(&merged, &merged, Some(&merged));
    assert_eq!(
        merge::fingerprint(&twice),
        merge::fingerprint(&merged),
        "{} 不幂等",
        ctx()
    );
    let mut n = 2;

    if let Some(want) = c.expect.schema_version {
        assert_eq!(merged.schema_version, want, "{} schemaVersion", ctx());
        n += 1;
    }
    if let Some(want) = c.expect.unknown_schema {
        assert_eq!(
            merge::unknown_schema(&c.remote),
            want,
            "{} unknownSchema",
            ctx()
        );
        n += 1;
    }
    if let Some(want) = &c.expect.habits {
        assert_eq!(habit_rows(&merged), *want, "{} habits", ctx());
        n += 1;
    }
    if let Some(want) = &c.expect.goals {
        assert_eq!(goal_rows(&merged), *want, "{} goals", ctx());
        n += 1;
    }
    if let Some(want) = &c.expect.markers {
        assert_eq!(marker_rows(&merged), *want, "{} markers", ctx());
        n += 1;
    }
    if let Some(want) = &c.expect.checkin_events {
        assert_eq!(event_rows(&merged), *want, "{} checkinEvents", ctx());
        n += 1;
    }
    if let Some(want) = &c.expect.notes {
        assert_eq!(note_rows(&merged), *want, "{} notes", ctx());
        n += 1;
    }
    if let Some(want) = &c.expect.moods {
        assert_eq!(mood_rows(&merged), *want, "{} moods", ctx());
        n += 1;
    }
    if let Some(want) = &c.expect.tombstones {
        assert_eq!(grave_rows(&merged), *want, "{} tombstones", ctx());
        n += 1;
    }
    n += check_list(
        "removedHabitIds",
        &c.expect.removed_habit_ids,
        merged
            .habits
            .iter()
            .filter(|h| habits::is_removed(&merged, h))
            .map(|h| h.id.clone())
            .collect(),
    );
    n += check_list(
        "todayHabitIds",
        &c.expect.today_habit_ids,
        habits::today_list(&merged)
            .iter()
            .map(|h| h.id.clone())
            .collect(),
    );
    n += check_list(
        "statsHabitIds",
        &c.expect.stats_habit_ids,
        habits::stats_list(&merged)
            .iter()
            .map(|h| h.id.clone())
            .collect(),
    );
    if let Some(want) = &c.expect.effective_statuses {
        let got: Vec<&str> = want
            .iter()
            .map(|s| status_text(habits::status_on(&merged, &s.habit_id, &s.local_date)))
            .collect();
        let expect: Vec<&str> = want.iter().map(|s| s.expect.as_str()).collect();
        assert_eq!(got, expect, "{} effectiveStatuses", ctx());
        n += 1;
    }
    assert!(
        n > 2,
        "{} 的 expect 一个字段都没写，这条用例等于没测",
        ctx()
    );
    n
}

fn check_list(section: &str, want: &Option<Vec<String>>, got: Vec<String>) -> usize {
    match want {
        Some(want) => {
            assert_eq!(got, *want, "{section} 名单不一致");
            1
        }
        None => 0,
    }
}

/// 枚举与 JSON 里的字符串一律按模型自己的 serde 口径取，测试不再抄一份名字表。
fn json_str<T: serde::Serialize>(v: &T) -> String {
    serde_json::to_value(v)
        .unwrap()
        .as_str()
        .unwrap()
        .to_string()
}

fn habit_rows(d: &Data) -> Vec<HabitRow> {
    d.habits
        .iter()
        .map(|h| HabitRow {
            id: h.id.clone(),
            name: h.name.clone(),
            unit: h.unit.clone(),
            weekly_target: h.weekly_target,
        })
        .collect()
}

fn goal_rows(d: &Data) -> Vec<GoalRow> {
    d.habits
        .iter()
        .flat_map(|h| {
            h.goal_history.iter().map(move |g| GoalRow {
                habit_id: h.id.clone(),
                effective_from: g.effective_from.clone(),
                target_mode: json_str(&g.target_mode),
                weekly_target: g.weekly_target,
            })
        })
        .collect()
}

fn marker_rows(d: &Data) -> Vec<MarkerRow> {
    d.markers
        .iter()
        .map(|m| MarkerRow {
            id: m.id.clone(),
            label: m.label.clone(),
        })
        .collect()
}

fn event_rows(d: &Data) -> Vec<EventRow> {
    d.checkin_events
        .iter()
        .map(|e| EventRow {
            id: e.id.clone(),
            local_date: e.local_date.clone(),
            status: json_str(&e.status),
            supersedes_id: e.supersedes_id.clone(),
        })
        .collect()
}

fn note_rows(d: &Data) -> Vec<NoteRow> {
    d.notes
        .iter()
        .map(|n| NoteRow {
            id: n.id.clone(),
            text: n.text.clone(),
            conflict_of: n.conflict_of.clone(),
        })
        .collect()
}

fn mood_rows(d: &Data) -> Vec<MoodRow> {
    d.daily_moods
        .iter()
        .map(|m| MoodRow {
            id: m.id.clone(),
            local_date: m.local_date.clone(),
            score: m.score,
        })
        .collect()
}

fn grave_rows(d: &Data) -> Vec<GraveRow> {
    d.tombstones
        .iter()
        .map(|t| GraveRow {
            entity_type: json_str(&t.entity_type),
            entity_id: t.entity_id.clone(),
            deleted_at: t.deleted_at.clone(),
            deleted_by: t.deleted_by.clone(),
        })
        .collect()
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
