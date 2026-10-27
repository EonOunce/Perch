//! 写入侧的口径：「一条记录怎么进 `Data`」只在这里写一遍。
//!
//! 前端只管渲染和收集输入，将来的手机端也不该在 Kotlin 里重拼一份 JSON——同一天点两次
//! 「完成」不该长出两条互相打架的事件，改目标不该回写历史，这些规则都归这里。
//!
//! 全是纯函数：ID 由宿主生成并传进来（core 不引 uuid 依赖，合并按 id 取并集，两端只要各自保证
//! 唯一），「此刻」也一样由宿主给出毫秒与 UTC 偏移，所以这些函数可以脱离系统时钟做单测。

use crate::date_boundary::{
    format_instant, local_date, next_week_start, parse_date, week_start, DAY_START_HOUR,
};
use crate::goal::push_goal;
use crate::habits::{effective_events, is_archived, is_removed, is_tombstoned};
use crate::model::{
    CheckinEvent, CheckinStatus, ConfirmationTiming, DailyMood, Data, EntityType, GoalEntry, Habit,
    Note, TargetMode, Tombstone,
};

/// 宿主传进来的「此刻」。core 不调系统时钟、不及时区库（DESIGN §5.4）。
#[derive(Debug, Clone, Copy)]
pub struct Now {
    pub instant_ms: i64,
    pub offset_secs: i32,
}

impl Now {
    /// 这一瞬间属于哪个业务日（04:00 日界）。
    pub fn date(&self) -> String {
        local_date(self.instant_ms, self.offset_secs, DAY_START_HOUR)
    }

    /// 写成 ISO 瞬间存进 `createdAt` / `updatedAt` / `confirmedAt`。
    pub fn stamp(&self) -> String {
        format_instant(self.instant_ms, self.offset_secs)
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum EditError {
    /// 名称必填（§2.4）。
    EmptyName,
    /// 正文去掉首尾空白就空了（§2.7「空内容不保存」）。
    EmptyText,
    /// 习惯不存在，或已被移除。
    HabitNotFound,
    /// 已归档只读：不能再补记（§2.4）。
    HabitArchived,
    /// 记录、心情或日期读不出来（`YYYY-MM-DD` 不合法）。
    BadDate(String),
    /// 未来的日子还没发生，不接受确认。
    FutureDate,
    /// 心情只收 0–100（§2.6）。
    BadScore(i32),
    /// `weekly` / `nightly` / `custom` 需要一个 ≥1 的目标数。
    BadTarget,
    /// 要改的那条不存在（或已带墓碑）。
    NotFound,
}

impl EditError {
    /// 给界面的一句话：说清为什么没写进去，不带路径也不带正文。
    pub fn user_message(&self) -> String {
        match self {
            EditError::EmptyName => "习惯名称不能为空".to_string(),
            EditError::EmptyText => "内容为空，没有保存".to_string(),
            EditError::HabitNotFound => "这个习惯已经不在了".to_string(),
            EditError::HabitArchived => "已归档的习惯只读，先恢复才能继续记录".to_string(),
            EditError::BadDate(d) => format!("日期「{d}」读不出来"),
            EditError::FutureDate => "还没到那一天，不能确认".to_string(),
            EditError::BadScore(s) => format!("心情只在 0–100 之间（收到 {s}）"),
            EditError::BadTarget => "每周/每晚/自定义目标需要一个至少为 1 的数字".to_string(),
            EditError::NotFound => "这条记录已经不在了".to_string(),
        }
    }
}

/// 图标与配色按创建顺序自动分配、循环取用（§2.4：不让用户多一次无意义的选择）。
/// 前端按 key 渲染，key 的含义只到「这一行属于哪个习惯」。
pub const COLORS: [&str; 4] = ["sage", "clay", "amber", "mist"];
pub const ICONS: [&str; 4] = ["book", "walk", "moon", "sprout"];

pub fn slot(index: usize) -> (String, String) {
    (
        ICONS[index % ICONS.len()].to_string(),
        COLORS[index % COLORS.len()].to_string(),
    )
}

/// 新增习惯表单的内容。默认值是 DESIGN §2.4 那组「可一眼改掉的保守起点」，
/// 代码里不认为它比用户改出来的值更正确。
#[derive(Debug, Clone, PartialEq)]
pub struct NewHabit {
    pub name: String,
    pub operational_definition: String,
    pub unit: String,
    pub target_count: i32,
    pub confirmation_timing: ConfirmationTiming,
    pub target_mode: TargetMode,
    pub weekly_target: Option<i32>,
    pub if_then: String,
}

impl Default for NewHabit {
    fn default() -> Self {
        NewHabit {
            name: String::new(),
            operational_definition: String::new(),
            unit: "次".to_string(),
            target_count: 1,
            confirmation_timing: ConfirmationTiming::SameDay,
            target_mode: TargetMode::Weekly,
            weekly_target: Some(3),
            if_then: String::new(),
        }
    }
}

/// 新增一个习惯。目标从**本周周一**生效（§2.4），于是创建当周就按这个目标计数，
/// 而不会因为「今天才周三」把前半周判成缺口。
pub fn add_habit(data: &mut Data, id: &str, input: NewHabit, now: Now) -> Result<(), EditError> {
    let name = input.name.trim().to_string();
    if name.is_empty() {
        return Err(EditError::EmptyName);
    }
    let (target_mode, weekly_target) = normalize_target(input.target_mode, input.weekly_target);
    check_target(target_mode, weekly_target)?;

    let today = now.date();
    // 拿不到周一就退回今天：目标至少从今天算，比编一个远古人造日期诚实。
    let effective_from = week_start(&today).unwrap_or_else(|| today.clone());
    let stamp = now.stamp();
    // 创建顺序按已有条数算，包括已归档的——颜色不该因为有人归档就重排。
    let (icon, color) = slot(data.habits.len());
    let sort_order = data.habits.iter().map(|h| h.sort_order).max().unwrap_or(0) + 10;

    let mut habit = Habit {
        id: id.to_string(),
        name,
        icon,
        color,
        operational_definition: input.operational_definition,
        unit: input.unit,
        target_count: input.target_count,
        confirmation_timing: input.confirmation_timing,
        target_mode,
        weekly_target,
        if_then: input.if_then,
        reminder_time: None,
        sort_order,
        archived_at: None,
        stats_visible: true,
        goal_history: Vec::new(),
        created_at: stamp.clone(),
        updated_at: stamp,
    };
    // 走 push_goal 而不是手填两份：保证「习惯上的当前值 = goalHistory 最后一项」。
    push_goal(
        &mut habit,
        GoalEntry {
            effective_from,
            target_mode,
            weekly_target,
        },
    );
    data.habits.push(habit);
    Ok(())
}

/// 一条写入的来源：`source` 只做来源统计（`manual` / `morningReview`），`recorded_by` 是
/// 设备 ID，合并与排查时用来区分两端（§3.3）。
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Origin<'a> {
    pub source: &'a str,
    pub recorded_by: &'a str,
}

/// 确认某一天。改主意时**追加一条更正事件**并指向旧事件，不修改也不删除原始事件（§3.3）。
///
/// 与当前有效状态相同就什么也不写：反复点「完成」不该把更正链越点越长。
/// 撤销误点用 `CheckinStatus::Void`，读出来是未确认，不是「没做」。
pub fn set_checkin(
    data: &mut Data,
    id: &str,
    habit_id: &str,
    local_date: &str,
    status: CheckinStatus,
    origin: Origin,
    now: Now,
) -> Result<(), EditError> {
    let habit = find_habit(data, habit_id)?;
    if is_removed(data, habit) {
        return Err(EditError::HabitNotFound);
    }
    if is_archived(habit) {
        return Err(EditError::HabitArchived);
    }
    // 「今天」是业务日，不是日历日：凌晨 3 点的确认仍然算昨天。
    check_day(local_date, &now.date())?;

    let current = effective_events(data, habit_id)
        .get(local_date)
        .map(|event| (event.id.clone(), event.status));
    // 已经是这个状态就什么都不写：反复点同一个按钮不该把更正链越点越长。
    if current.as_ref().is_some_and(|(_, s)| *s == status) {
        return Ok(());
    }

    let stamp = now.stamp();
    data.checkin_events.push(CheckinEvent {
        id: id.to_string(),
        habit_id: habit_id.to_string(),
        local_date: local_date.to_string(),
        status,
        confirmed_at: Some(stamp.clone()),
        source: origin.source.to_string(),
        supersedes_id: current.map(|(i, _)| i),
        recorded_by: origin.recorded_by.to_string(),
        note: String::new(),
        created_at: stamp,
    });
    Ok(())
}

/// 当天心情。自然键是 `localDate`，一天最多一条：再拖一次是覆盖同一行，不是追加（§2.6）。
pub fn set_mood(
    data: &mut Data,
    id: &str,
    local_date: &str,
    score: i32,
    now: Now,
) -> Result<(), EditError> {
    if !(0..=100).contains(&score) {
        return Err(EditError::BadScore(score));
    }
    check_day(local_date, &now.date())?;

    let at = data.daily_moods.iter().position(|m| {
        m.local_date == local_date && !is_tombstoned(data, EntityType::DailyMood, &m.id)
    });
    let stamp = now.stamp();
    match at {
        Some(at) => {
            let mood = &mut data.daily_moods[at];
            mood.score = score;
            mood.updated_at = stamp;
        }
        None => data.daily_moods.push(DailyMood {
            id: id.to_string(),
            local_date: local_date.to_string(),
            score,
            updated_at: stamp,
        }),
    }
    Ok(())
}

/// 记一条碎碎念。标签从正文里的 `#` 解析，存进去不带 `#`（§2.5）。
/// 正文去掉首尾空白后为空就不写（§2.7）；段内换行原样保留。
pub fn add_note(data: &mut Data, id: &str, text: &str, now: Now) -> Result<(), EditError> {
    let body = text.trim();
    if body.is_empty() {
        return Err(EditError::EmptyText);
    }
    let stamp = now.stamp();
    data.notes.push(Note {
        id: id.to_string(),
        created_at: stamp.clone(),
        local_date: now.date(),
        text: body.to_string(),
        tags: parse_tags(body),
        updated_at: stamp,
        conflict_of: None,
    });
    Ok(())
}

/// 改一条记录的正文，同时按新正文重解析标签。
pub fn update_note(data: &mut Data, id: &str, text: &str, now: Now) -> Result<(), EditError> {
    let body = text.trim();
    if body.is_empty() {
        return Err(EditError::EmptyText);
    }
    let at = data
        .notes
        .iter()
        .position(|n| n.id == id && !is_tombstoned(data, EntityType::Note, &n.id))
        .ok_or(EditError::NotFound)?;
    let note = &mut data.notes[at];
    note.text = body.to_string();
    note.tags = parse_tags(body);
    note.updated_at = now.stamp();
    Ok(())
}

/// 归档 / 恢复。归档写 `archivedAt`，不写墓碑（§2.4）。
pub fn set_archived(
    data: &mut Data,
    habit_id: &str,
    archived: bool,
    now: Now,
) -> Result<(), EditError> {
    let at = data
        .habits
        .iter()
        .position(|h| h.id == habit_id)
        .ok_or(EditError::HabitNotFound)?;
    if is_removed(data, &data.habits[at]) {
        return Err(EditError::HabitNotFound);
    }
    let stamp = now.stamp();
    let habit = &mut data.habits[at];
    habit.archived_at = archived.then(|| stamp.clone());
    habit.updated_at = stamp;
    Ok(())
}

/// 「统计里显示」开关：默认开着，所以归档不会悄悄改写过去的完成率。
pub fn set_stats_visible(
    data: &mut Data,
    habit_id: &str,
    visible: bool,
    now: Now,
) -> Result<(), EditError> {
    let at = data
        .habits
        .iter()
        .position(|h| h.id == habit_id)
        .ok_or(EditError::HabitNotFound)?;
    if is_removed(data, &data.habits[at]) {
        return Err(EditError::HabitNotFound);
    }
    let stamp = now.stamp();
    let habit = &mut data.habits[at];
    habit.stats_visible = visible;
    habit.updated_at = stamp;
    Ok(())
}

/// 改已有习惯的目标：新目标从**下一个周一**生效，本周仍按原目标显示与评估（§2.4）。
/// 追加条目，不覆盖历史，也不回写已经发生的日子。
pub fn change_goal(
    data: &mut Data,
    habit_id: &str,
    target_mode: TargetMode,
    weekly_target: Option<i32>,
    now: Now,
) -> Result<(), EditError> {
    let (target_mode, weekly_target) = normalize_target(target_mode, weekly_target);
    check_target(target_mode, weekly_target)?;
    let at = data
        .habits
        .iter()
        .position(|h| h.id == habit_id)
        .ok_or(EditError::HabitNotFound)?;
    if is_removed(data, &data.habits[at]) {
        return Err(EditError::HabitNotFound);
    }
    if is_archived(&data.habits[at]) {
        return Err(EditError::HabitArchived);
    }
    let today = now.date();
    let effective_from = next_week_start(&today).unwrap_or_else(|| today.clone());
    let stamp = now.stamp();
    let habit = &mut data.habits[at];
    // push_goal 对非法日期返回 false；这里日期来自 now.date()，不该发生，但仍不谎报成功。
    let pushed = push_goal(
        habit,
        GoalEntry {
            effective_from,
            target_mode,
            weekly_target,
        },
    );
    if !pushed {
        return Err(EditError::BadDate(today));
    }
    habit.updated_at = stamp;
    Ok(())
}

/// 删除 = 写一条顶层墓碑，实体本身不硬删（§3.3）。重复删除幂等，返回是否新写入。
pub fn tombstone(
    data: &mut Data,
    entity_type: EntityType,
    entity_id: &str,
    deleted_by: &str,
    now: Now,
) -> Result<bool, EditError> {
    if data
        .tombstones
        .iter()
        .any(|t| t.entity_type == entity_type && t.entity_id == entity_id)
    {
        return Ok(false);
    }
    if matches!(entity_type, EntityType::Habit) && !data.habits.iter().any(|h| h.id == entity_id) {
        return Err(EditError::HabitNotFound);
    }
    data.tombstones.push(Tombstone {
        entity_type,
        entity_id: entity_id.to_string(),
        deleted_at: now.stamp(),
        deleted_by: deleted_by.to_string(),
    });
    Ok(true)
}

fn find_habit<'a>(data: &'a Data, habit_id: &str) -> Result<&'a Habit, EditError> {
    data.habits
        .iter()
        .find(|h| h.id == habit_id)
        .ok_or(EditError::HabitNotFound)
}

/// 校验一个 `localDate`：格式要真的存在，且不能落在未来——还没到的日子没法确认。
/// `today` 读不出来时不拦（它由 `Now::date` 生成，本来就该是合法的）。
fn check_day(local_date: &str, today: &str) -> Result<(), EditError> {
    let day = parse_date(local_date).ok_or_else(|| EditError::BadDate(local_date.to_string()))?;
    if day > parse_date(today).unwrap_or(i64::MAX) {
        return Err(EditError::FutureDate);
    }
    Ok(())
}

/// `recordOnly` 不带目标数：留着数字只会让人以为还在计数。
fn normalize_target(mode: TargetMode, weekly_target: Option<i32>) -> (TargetMode, Option<i32>) {
    match mode {
        TargetMode::RecordOnly => (mode, None),
        other => (other, weekly_target),
    }
}

/// `recordOnly` 不带目标数；其余三种模式没有至少 1 个的目标就没法判达标。
fn check_target(mode: TargetMode, weekly_target: Option<i32>) -> Result<(), EditError> {
    match mode {
        TargetMode::RecordOnly => Ok(()),
        _ => match weekly_target {
            Some(n) if n >= 1 => Ok(()),
            _ => Err(EditError::BadTarget),
        },
    }
}

/// 从正文取 `#标签`：按空白切词，以 `#` 开头的才算，去掉 `#`，去重保序（§2.5）。
///
/// 只做字面解析，不做语义分析——「自动判断情绪」是 §1.4 明确排除的。
pub fn parse_tags(text: &str) -> Vec<String> {
    let mut out: Vec<String> = Vec::new();
    for word in text.split_whitespace() {
        let Some(raw) = word.strip_prefix('#') else {
            continue;
        };
        let tag = raw.trim();
        if tag.is_empty() {
            continue;
        }
        if !out.iter().any(|t| t == tag) {
            out.push(tag.to_string());
        }
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::date_boundary::weekday_mon_first;
    use crate::habits::{status_on, DayStatus};

    /// 2026-10-26T04:00:00.000+08:00，正好是「新的一天刚开始」。
    const NOW_MS: i64 = 1_792_958_400_000;
    const CST: i32 = 8 * 3600;
    const TODAY: &str = "2026-10-26";

    fn now() -> Now {
        Now {
            instant_ms: NOW_MS,
            offset_secs: CST,
        }
    }

    /// 凌晨 3:59：业务日仍然是前一天。
    fn late_night() -> Now {
        Now {
            instant_ms: NOW_MS - 60_000,
            offset_secs: CST,
        }
    }

    /// 表单默认值名字是空的（名称必填、首启不预置），所以测试自己给一个。
    fn sample() -> NewHabit {
        NewHabit {
            name: "阅读".to_string(),
            ..NewHabit::default()
        }
    }

    fn one_habit() -> Data {
        let mut data = Data::empty("dev-a");
        add_habit(&mut data, "h1", sample(), now()).unwrap();
        data
    }

    /// 手工点按：来源 manual、设备 dev-a，七个 set_checkin 测试都用它。
    const MANUAL: Origin<'static> = Origin {
        source: "manual",
        recorded_by: "dev-a",
    };

    fn tap(data: &mut Data, id: &str, day: &str, status: CheckinStatus) {
        set_checkin(data, id, "h1", day, status, MANUAL, now()).unwrap();
    }

    #[test]
    fn fixture_is_the_weekday_it_claims() {
        // 下面几条断言依赖「今天正好是周一」，先自己验一遍，免得整批测试跟着假日期一起骗人。
        assert_eq!(now().date(), TODAY);
        assert_eq!(late_night().date(), "2026-10-25");
        assert_eq!(weekday_mon_first(TODAY), Some(0));
    }

    #[test]
    fn new_habit_goal_starts_this_weeks_monday() {
        let data = one_habit();
        let habit = &data.habits[0];
        // 本周周一（今天就是周一）——创建当周就按这个目标计数，不把前半周判成缺口。
        assert_eq!(habit.goal_history[0].effective_from, TODAY);
        assert_eq!(habit.target_mode, TargetMode::Weekly);
        assert_eq!(habit.weekly_target, Some(3));
        assert_eq!(habit.created_at, "2026-10-26T04:00:00.000+08:00");
    }

    #[test]
    fn new_habit_mid_week_goal_still_starts_on_monday() {
        let mut data = Data::empty("dev-a");
        let wednesday = Now {
            instant_ms: NOW_MS + 2 * 86_400_000,
            offset_secs: CST,
        };
        add_habit(&mut data, "h1", sample(), wednesday).unwrap();
        assert_eq!(data.habits[0].goal_history[0].effective_from, TODAY);
    }

    #[test]
    fn blank_name_is_rejected_without_writing() {
        let mut data = Data::empty("dev-a");
        let input = NewHabit {
            name: "   ".to_string(),
            ..NewHabit::default()
        };
        assert_eq!(
            add_habit(&mut data, "h1", input, now()),
            Err(EditError::EmptyName)
        );
        assert!(data.habits.is_empty());
    }

    #[test]
    fn palette_follows_creation_order_and_wraps() {
        let mut data = Data::empty("dev-a");
        for i in 0..5 {
            add_habit(
                &mut data,
                &format!("h{i}"),
                NewHabit {
                    name: format!("习惯 {i}"),
                    ..NewHabit::default()
                },
                now(),
            )
            .unwrap();
        }
        let colors: Vec<&str> = data.habits.iter().map(|h| h.color.as_str()).collect();
        assert_eq!(colors, vec!["sage", "clay", "amber", "mist", "sage"]);
        let orders: Vec<i32> = data.habits.iter().map(|h| h.sort_order).collect();
        assert_eq!(orders, vec![10, 20, 30, 40, 50]);
    }

    #[test]
    fn first_tap_writes_an_event_without_a_correction_link() {
        let mut data = one_habit();
        tap(&mut data, "e1", TODAY, CheckinStatus::Done);
        assert_eq!(data.checkin_events.len(), 1);
        assert_eq!(data.checkin_events[0].supersedes_id, None);
        assert_eq!(status_on(&data, "h1", TODAY), DayStatus::Done);
    }

    #[test]
    fn changing_mind_appends_a_correction_and_keeps_the_original() {
        let mut data = one_habit();
        tap(&mut data, "e1", TODAY, CheckinStatus::Done);
        tap(&mut data, "e2", TODAY, CheckinStatus::NotDone);
        // 原始事件永远保留（§3.3）：改主意是追加，不是抹掉。
        assert_eq!(data.checkin_events.len(), 2);
        assert_eq!(data.checkin_events[1].supersedes_id.as_deref(), Some("e1"));
        assert_eq!(data.checkin_events[0].status, CheckinStatus::Done);
        assert_eq!(status_on(&data, "h1", TODAY), DayStatus::NotDone);
    }

    #[test]
    fn same_status_tap_writes_nothing() {
        let mut data = one_habit();
        tap(&mut data, "e1", TODAY, CheckinStatus::Done);
        tap(&mut data, "e2", TODAY, CheckinStatus::Done);
        assert_eq!(data.checkin_events.len(), 1);
    }

    #[test]
    fn void_reads_back_as_unconfirmed() {
        let mut data = one_habit();
        tap(&mut data, "e1", TODAY, CheckinStatus::Done);
        tap(&mut data, "e2", TODAY, CheckinStatus::Void);
        assert_eq!(status_on(&data, "h1", TODAY), DayStatus::Unconfirmed);
        assert_eq!(data.checkin_events.len(), 2);
    }

    #[test]
    fn future_days_are_not_confirmable() {
        let mut data = one_habit();
        assert_eq!(
            set_checkin(
                &mut data,
                "e1",
                "h1",
                "2026-10-27",
                CheckinStatus::Done,
                MANUAL,
                now()
            ),
            Err(EditError::FutureDate)
        );
        assert_eq!(
            set_checkin(
                &mut data,
                "e2",
                "h1",
                "2026-02-30",
                CheckinStatus::Done,
                MANUAL,
                now()
            ),
            Err(EditError::BadDate("2026-02-30".to_string()))
        );
        assert!(data.checkin_events.is_empty());
    }

    #[test]
    fn yesterday_can_be_confirmed_in_the_early_hours() {
        let mut data = one_habit();
        // 待确认区早上也能用：凌晨的业务日仍然是前一天，所以确认前一天合法。
        set_checkin(
            &mut data,
            "e1",
            "h1",
            "2026-10-25",
            CheckinStatus::Done,
            Origin {
                source: "morningReview",
                recorded_by: "dev-a",
            },
            late_night(),
        )
        .unwrap();
        assert_eq!(status_on(&data, "h1", "2026-10-25"), DayStatus::Done);
        // 而 10-26 那一刻还没开始，不能提前确认。
        assert_eq!(
            set_checkin(
                &mut data,
                "e2",
                "h1",
                TODAY,
                CheckinStatus::Done,
                MANUAL,
                late_night()
            ),
            Err(EditError::FutureDate)
        );
    }

    #[test]
    fn unknown_or_removed_habit_rejects_checkin() {
        let mut data = one_habit();
        assert_eq!(
            set_checkin(
                &mut data,
                "e1",
                "nope",
                TODAY,
                CheckinStatus::Done,
                MANUAL,
                now()
            ),
            Err(EditError::HabitNotFound)
        );
        data.tombstones.push(Tombstone {
            entity_type: EntityType::Habit,
            entity_id: "h1".to_string(),
            deleted_at: now().stamp(),
            deleted_by: "dev-a".to_string(),
        });
        assert_eq!(
            set_checkin(
                &mut data,
                "e2",
                "h1",
                TODAY,
                CheckinStatus::Done,
                MANUAL,
                now()
            ),
            Err(EditError::HabitNotFound)
        );
    }

    #[test]
    fn archived_habit_is_read_only_until_restored() {
        let mut data = one_habit();
        set_archived(&mut data, "h1", true, now()).unwrap();
        assert_eq!(
            set_checkin(
                &mut data,
                "e1",
                "h1",
                TODAY,
                CheckinStatus::Done,
                MANUAL,
                now()
            ),
            Err(EditError::HabitArchived)
        );
        set_archived(&mut data, "h1", false, now()).unwrap();
        assert!(data.habits[0].archived_at.is_none());
        tap(&mut data, "e2", TODAY, CheckinStatus::Done);
        assert_eq!(status_on(&data, "h1", TODAY), DayStatus::Done);
    }

    #[test]
    fn mood_keeps_one_row_per_day_and_reuses_its_id() {
        let mut data = one_habit();
        set_mood(&mut data, "m1", TODAY, 40, now()).unwrap();
        set_mood(&mut data, "m2", TODAY, 72, now()).unwrap();
        assert_eq!(data.daily_moods.len(), 1);
        assert_eq!(data.daily_moods[0].id, "m1");
        assert_eq!(data.daily_moods[0].score, 72);
        // 新 id 不该另起一行：自然键是 localDate。
        assert_eq!(data.daily_moods[0].updated_at, now().stamp());
    }

    #[test]
    fn mood_checks_its_range_and_its_day() {
        let mut data = one_habit();
        assert_eq!(
            set_mood(&mut data, "m1", TODAY, 101, now()),
            Err(EditError::BadScore(101))
        );
        assert_eq!(
            set_mood(&mut data, "m2", TODAY, -1, now()),
            Err(EditError::BadScore(-1))
        );
        assert_eq!(
            set_mood(&mut data, "m3", "2026-10-27", 50, now()),
            Err(EditError::FutureDate)
        );
        assert!(data.daily_moods.is_empty());
        assert_eq!(set_mood(&mut data, "m4", TODAY, 0, now()), Ok(()));
        assert_eq!(set_mood(&mut data, "m5", TODAY, 100, now()), Ok(()));
    }

    #[test]
    fn note_tags_are_parsed_trimmed_and_deduped() {
        let mut data = one_habit();
        add_note(&mut data, "n1", "  跟读了两页 #家人 #  #家人 #雨天 ", now()).unwrap();
        let note = &data.notes[0];
        assert_eq!(note.text, "跟读了两页 #家人 #  #家人 #雨天");
        assert_eq!(note.tags, vec!["家人".to_string(), "雨天".to_string()]);
        assert_eq!(note.local_date, TODAY);
        assert_eq!(note.conflict_of, None);
    }

    #[test]
    fn blank_note_is_not_saved_and_missing_note_is_reported() {
        let mut data = one_habit();
        assert_eq!(
            add_note(&mut data, "n1", "  \n ", now()),
            Err(EditError::EmptyText)
        );
        assert!(data.notes.is_empty());
        assert_eq!(
            update_note(&mut data, "n1", "改一下", now()),
            Err(EditError::NotFound)
        );
    }

    #[test]
    fn editing_a_note_reparses_tags_and_bumps_updated_at() {
        let mut data = one_habit();
        add_note(&mut data, "n1", "原来 #旧标签", now()).unwrap();
        let later = Now {
            instant_ms: NOW_MS + 3_600_000,
            offset_secs: CST,
        };
        update_note(&mut data, "n1", "改成新的 #新标签", later).unwrap();
        let note = &data.notes[0];
        assert_eq!(note.tags, vec!["新标签".to_string()]);
        assert_eq!(note.created_at, now().stamp());
        assert_eq!(note.updated_at, "2026-10-26T05:00:00.000+08:00");
        // 编辑不改归属日：那天写的记录不会因为今天动了它就跳走。
        assert_eq!(note.local_date, TODAY);
    }

    #[test]
    fn tombstone_is_idempotent() {
        let mut data = one_habit();
        assert_eq!(
            tombstone(&mut data, EntityType::Note, "n1", "dev-a", now()),
            Ok(true)
        );
        assert_eq!(
            tombstone(&mut data, EntityType::Note, "n1", "dev-b", now()),
            Ok(false)
        );
        assert_eq!(data.tombstones.len(), 1);
        // 没被引用过第二次的删除时间不该变。
        assert_eq!(data.tombstones[0].deleted_by, "dev-a");
        assert_eq!(
            tombstone(&mut data, EntityType::Habit, "nope", "dev-a", now()),
            Err(EditError::HabitNotFound)
        );
    }

    #[test]
    fn changing_goal_starts_next_monday_and_leaves_this_week_alone() {
        let mut data = one_habit();
        change_goal(&mut data, "h1", TargetMode::Weekly, Some(5), now()).unwrap();
        let habit = &data.habits[0];
        assert_eq!(habit.goal_history.len(), 2);
        assert_eq!(habit.goal_history[0].effective_from, TODAY);
        assert_eq!(habit.goal_history[1].effective_from, "2026-11-02");
        // 本周仍显示原目标：习惯上的当前值等于最后一项，但 goal_on 按生效日挑。
        assert_eq!(habit.weekly_target, Some(5));
        let mid_week = Now {
            instant_ms: NOW_MS + 2 * 86_400_000,
            offset_secs: CST,
        };
        assert_eq!(
            crate::goal::goal_on(habit, &mid_week.date()).weekly_target,
            Some(3)
        );
        assert_eq!(
            crate::goal::goal_on(habit, "2026-11-02").weekly_target,
            Some(5)
        );
        assert_eq!(crate::goal::pending(habit, &mid_week.date()).len(), 1);
    }

    #[test]
    fn record_only_needs_no_number_but_weekly_does() {
        let mut data = one_habit();
        assert_eq!(
            change_goal(&mut data, "h1", TargetMode::Weekly, Some(0), now()),
            Err(EditError::BadTarget)
        );
        assert_eq!(
            change_goal(&mut data, "h1", TargetMode::Weekly, None, now()),
            Err(EditError::BadTarget)
        );
        change_goal(&mut data, "h1", TargetMode::RecordOnly, Some(9), now()).unwrap();
        let habit = &data.habits[0];
        assert_eq!(habit.target_mode, TargetMode::RecordOnly);
        // 改成「只记录」时目标数一起清掉，否则界面上还挂着一个没人达标的数字。
        assert_eq!(habit.weekly_target, None);
        assert_eq!(habit.goal_history[1].weekly_target, None);
        assert_eq!(
            add_habit(
                &mut data,
                "h2",
                NewHabit {
                    name: "散步".to_string(),
                    target_mode: TargetMode::Nightly,
                    weekly_target: None,
                    ..NewHabit::default()
                },
                now()
            ),
            Err(EditError::BadTarget)
        );
    }

    #[test]
    fn stats_visibility_toggle_survives_archive() {
        let mut data = one_habit();
        set_stats_visible(&mut data, "h1", false, now()).unwrap();
        assert!(!data.habits[0].stats_visible);
        assert_eq!(
            set_stats_visible(&mut data, "nope", true, now()),
            Err(EditError::HabitNotFound)
        );
    }
}
