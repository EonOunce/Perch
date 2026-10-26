//! 事件有效性与习惯可见性。DESIGN §3.3 的三条约定都收在这里：
//! 删除只写顶层墓碑、同习惯同晚只有一个有效事件、`void` 在界面上仍是未确认。
//!
//! 完成率的分母口径（§6.2）跟着「一天一个有效状态」走，所以 `counts` 也放在这里，
//! `stats.rs` 只负责呈现，不再重新定义一遍。

use std::collections::{HashMap, HashSet};

use crate::date_boundary::{date_range, parse_instant};
use crate::model::{CheckinEvent, CheckinStatus, Data, EntityType, Habit};

/// 界面上的一天状态。`Unconfirmed` 既包括没有事件，也包括被 `void` 撤销的事件。
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum DayStatus {
    Done,
    NotDone,
    Unconfirmed,
}

/// 一段时间里三种状态各占几天。
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub struct DayCounts {
    pub done: i64,
    pub not_done: i64,
    pub unconfirmed: i64,
}

impl DayCounts {
    /// §6.2 的分母：`done + notDone + 未确认`，恒等于这段时间的天数。0 时界面显示「—」。
    pub fn denominator(&self) -> i64 {
        self.done + self.not_done + self.unconfirmed
    }

    /// `None` = 分母为 0，不是 0%。
    pub fn rate(&self) -> Option<f64> {
        let d = self.denominator();
        (d > 0).then(|| self.done as f64 / d as f64)
    }
}

pub fn is_tombstoned(data: &Data, entity_type: EntityType, id: &str) -> bool {
    data.tombstones
        .iter()
        .any(|t| t.entity_type == entity_type && t.entity_id == id)
}

/// 归档 = `archivedAt` 有值。归档不是删除，不写墓碑（§2.4）。
pub fn is_archived(habit: &Habit) -> bool {
    habit
        .archived_at
        .as_deref()
        .is_some_and(|s| !s.trim().is_empty())
}

/// 移除 = 墓碑里有这个习惯。移除后不进入任何视图与统计（§2.4、§6.2）。
pub fn is_removed(data: &Data, habit: &Habit) -> bool {
    is_tombstoned(data, EntityType::Habit, &habit.id)
}

/// 是否参与统计：未移除，且（未归档 或 `statsVisible` 为开）。
pub fn participates_in_stats(data: &Data, habit: &Habit) -> bool {
    !is_removed(data, habit) && (!is_archived(habit) || habit.stats_visible)
}

/// 今天视图与待确认区能看到的习惯：未移除且未归档。
pub fn today_list(data: &Data) -> Vec<&Habit> {
    sorted(data, |_, h| !is_removed(data, h) && !is_archived(h))
}

/// 习惯管理列表：未移除，归档的也在（带归档标识）。
pub fn management_list(data: &Data) -> Vec<&Habit> {
    sorted(data, |d, h| !is_removed(d, h))
}

/// 统计里出现的习惯（§6.2 的习惯范围选择器）。
pub fn stats_list(data: &Data) -> Vec<&Habit> {
    sorted(data, participates_in_stats)
}

fn sorted(data: &Data, keep: impl Fn(&Data, &Habit) -> bool) -> Vec<&Habit> {
    let mut out: Vec<&Habit> = data.habits.iter().filter(|h| keep(data, h)).collect();
    // `sortOrder` 相同的习惯按 id 排，保证两端列出同一个顺序。
    out.sort_by(|a, b| {
        a.sort_order
            .cmp(&b.sort_order)
            .then_with(|| a.id.cmp(&b.id))
    });
    out
}

/// 每个 `localDate` 唯一的有效事件。
///
/// 三道筛子，顺序有意义：
/// 1. 有墓碑的事件直接淘汰；
/// 2. 被任何幸存事件 `supersedesId` 指向的那条淘汰。用「是否被引用」而不是「引用者是否有效」，
///    这样 `B ← A ← C` 的更正链里 B 不会因 A 失效而复活；
/// 3. 同一天剩下的按 `createdAt` 最新胜出，同一瞬间按 id 大者胜，保证两端收敛。
///
/// `createdAt` 读不出来的事件不丢弃，只排最后——静默丢数据比丢顺序更难查。
pub fn effective_events<'a>(data: &'a Data, habit_id: &str) -> HashMap<&'a str, &'a CheckinEvent> {
    let alive: Vec<&CheckinEvent> = data
        .checkin_events
        .iter()
        .filter(|e| e.habit_id == habit_id)
        .filter(|e| !is_tombstoned(data, EntityType::CheckinEvent, &e.id))
        .collect();

    let superseded: HashSet<&str> = alive
        .iter()
        .filter_map(|e| e.supersedes_id.as_deref())
        .collect();

    let mut best: HashMap<&str, &CheckinEvent> = HashMap::new();
    for event in alive.iter().filter(|e| !superseded.contains(e.id.as_str())) {
        match best.get(event.local_date.as_str()) {
            Some(incumbent) if !newer(event, incumbent) => {}
            _ => {
                best.insert(event.local_date.as_str(), event);
            }
        }
    }
    best
}

fn newer(candidate: &CheckinEvent, incumbent: &CheckinEvent) -> bool {
    fn key(e: &CheckinEvent) -> (i64, &str) {
        (
            parse_instant(&e.created_at).unwrap_or(i64::MIN),
            e.id.as_str(),
        )
    }
    key(candidate) > key(incumbent)
}

/// 一天摊成界面状态。`void` 与「没有事件」都是未确认。
pub fn status_of(event: Option<&CheckinEvent>) -> DayStatus {
    match event.map(|e| e.status) {
        Some(CheckinStatus::Done) => DayStatus::Done,
        Some(CheckinStatus::NotDone) => DayStatus::NotDone,
        Some(CheckinStatus::Void) | None => DayStatus::Unconfirmed,
    }
}

pub fn event_on<'a>(data: &'a Data, habit_id: &str, local_date: &str) -> Option<&'a CheckinEvent> {
    effective_events(data, habit_id).remove(local_date)
}

pub fn status_on(data: &Data, habit_id: &str, local_date: &str) -> DayStatus {
    status_of(event_on(data, habit_id, local_date))
}

/// 含首尾一段时间的三态天数。任一端日期非法返回 `None`，由界面就地提示，不当成「0 天」。
pub fn counts(data: &Data, habit_id: &str, from: &str, to: &str) -> Option<DayCounts> {
    let days = date_range(from, to)?;
    let events = effective_events(data, habit_id);
    let mut out = DayCounts::default();
    for day in &days {
        match status_of(events.get(day.as_str()).copied()) {
            DayStatus::Done => out.done += 1,
            DayStatus::NotDone => out.not_done += 1,
            DayStatus::Unconfirmed => out.unconfirmed += 1,
        }
    }
    Some(out)
}

/// 一段时间里还没确认的日子，从早到晚。待确认区与热力图的描边格都用它。
pub fn unconfirmed_days(data: &Data, habit_id: &str, from: &str, to: &str) -> Vec<String> {
    let Some(days) = date_range(from, to) else {
        return Vec::new();
    };
    let events = effective_events(data, habit_id);
    days.into_iter()
        .filter(|d| status_of(events.get(d.as_str()).copied()) == DayStatus::Unconfirmed)
        .collect()
}

/// 一段时间里 `done` 的日子，从早到晚。热力图填色、周完成数都用它。
pub fn done_days(data: &Data, habit_id: &str, from: &str, to: &str) -> Vec<String> {
    let Some(days) = date_range(from, to) else {
        return Vec::new();
    };
    let events = effective_events(data, habit_id);
    days.into_iter()
        .filter(|d| status_of(events.get(d.as_str()).copied()) == DayStatus::Done)
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::model::{CheckinStatus, Tombstone};

    fn habit(id: &str) -> Habit {
        Habit {
            id: id.to_string(),
            name: id.to_string(),
            icon: String::new(),
            color: String::new(),
            operational_definition: String::new(),
            unit: "次".to_string(),
            target_count: 1,
            confirmation_timing: crate::model::ConfirmationTiming::SameDay,
            target_mode: crate::model::TargetMode::Weekly,
            weekly_target: Some(4),
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

    fn event(id: &str, date: &str, status: CheckinStatus) -> CheckinEvent {
        CheckinEvent {
            id: id.to_string(),
            habit_id: "h1".to_string(),
            local_date: date.to_string(),
            status,
            confirmed_at: None,
            source: "manual".to_string(),
            supersedes_id: None,
            recorded_by: "pc".to_string(),
            note: String::new(),
            created_at: format!("{date}T21:00:00.000+08:00"),
        }
    }

    fn data_with(habits: Vec<Habit>, events: Vec<CheckinEvent>) -> Data {
        let mut d = Data::empty("pc");
        d.habits = habits;
        d.checkin_events = events;
        d
    }

    #[test]
    fn void_and_missing_events_read_as_unconfirmed() {
        let data = data_with(
            vec![habit("h1")],
            vec![
                event("e1", "2026-10-20", CheckinStatus::Void),
                event("e2", "2026-10-21", CheckinStatus::Done),
            ],
        );
        assert_eq!(status_on(&data, "h1", "2026-10-20"), DayStatus::Unconfirmed);
        assert_eq!(status_on(&data, "h1", "2026-10-22"), DayStatus::Unconfirmed);
        assert_eq!(status_on(&data, "h1", "2026-10-21"), DayStatus::Done);
    }

    #[test]
    fn supersedes_chain_keeps_only_the_newest() {
        let mut original = event("e1", "2026-10-20", CheckinStatus::Done);
        original.created_at = "2026-10-20T21:00:00.000+08:00".to_string();
        let mut fix = event("e2", "2026-10-20", CheckinStatus::NotDone);
        fix.created_at = "2026-10-21T08:00:00.000+08:00".to_string();
        fix.supersedes_id = Some("e1".to_string());
        let mut fix2 = event("e3", "2026-10-20", CheckinStatus::Done);
        fix2.created_at = "2026-10-22T08:00:00.000+08:00".to_string();
        fix2.supersedes_id = Some("e2".to_string());
        let data = data_with(
            vec![habit("h1")],
            vec![
                event("e9", "2026-10-25", CheckinStatus::Done),
                fix2,
                fix,
                original,
            ],
        );
        let events = effective_events(&data, "h1");
        assert_eq!(events.len(), 2, "同晚只剩一条有效事件");
        assert_eq!(events.get("2026-10-20").unwrap().id, "e3");
        // B 不因 A 被撤而复活。
        assert_eq!(status_on(&data, "h1", "2026-10-20"), DayStatus::Done);
    }

    #[test]
    fn tombstoned_event_never_wins() {
        let mut a = event("e1", "2026-10-20", CheckinStatus::Done);
        a.created_at = "2026-10-21T21:00:00.000+08:00".to_string();
        let b = event("e2", "2026-10-20", CheckinStatus::NotDone);
        let mut data = data_with(vec![habit("h1")], vec![a, b]);
        data.tombstones.push(Tombstone {
            entity_type: EntityType::CheckinEvent,
            entity_id: "e1".to_string(),
            deleted_at: "2026-10-22T09:00:00.000+08:00".to_string(),
            deleted_by: "pc".to_string(),
        });
        assert_eq!(status_on(&data, "h1", "2026-10-20"), DayStatus::NotDone);
    }

    #[test]
    fn same_instant_breaks_ties_by_id() {
        let a = event("e1", "2026-10-20", CheckinStatus::Done);
        let mut b = event("e2", "2026-10-20", CheckinStatus::NotDone);
        b.created_at = a.created_at.clone();
        let data = data_with(vec![habit("h1")], vec![a, b]);
        assert_eq!(
            effective_events(&data, "h1").get("2026-10-20").unwrap().id,
            "e2"
        );
    }

    #[test]
    fn denominator_always_equals_day_count() {
        let data = data_with(
            vec![habit("h1")],
            vec![
                event("e1", "2026-10-20", CheckinStatus::Done),
                event("e2", "2026-10-21", CheckinStatus::NotDone),
            ],
        );
        let c = counts(&data, "h1", "2026-10-20", "2026-10-26").unwrap();
        assert_eq!((c.done, c.not_done, c.unconfirmed), (1, 1, 5));
        assert_eq!(c.denominator(), 7);
        assert_eq!(
            unconfirmed_days(&data, "h1", "2026-10-20", "2026-10-22"),
            vec!["2026-10-22"]
        );
        assert_eq!(
            done_days(&data, "h1", "2026-10-20", "2026-10-22"),
            vec!["2026-10-20"]
        );
    }

    #[test]
    fn empty_denominator_has_no_rate() {
        let data = data_with(vec![habit("h1")], vec![]);
        assert_eq!(
            counts(&data, "h1", "2026-10-20", "2026-10-19")
                .unwrap()
                .rate(),
            None
        );
        assert!(counts(&data, "h1", "not-a-date", "2026-10-19").is_none());
    }

    #[test]
    fn archive_and_removal_split_the_three_lists() {
        let mut archived = habit("h2");
        archived.archived_at = Some("2026-10-15T09:00:00.000+08:00".to_string());
        archived.stats_visible = false;
        let mut archived_kept = habit("h3");
        archived_kept.archived_at = Some("2026-10-15T09:00:00.000+08:00".to_string());
        archived_kept.stats_visible = true;
        let removed = habit("h4");
        let mut data = data_with(
            vec![habit("h1"), archived, archived_kept, removed],
            Vec::new(),
        );
        data.tombstones.push(Tombstone {
            entity_type: EntityType::Habit,
            entity_id: "h4".to_string(),
            deleted_at: "2026-10-16T09:00:00.000+08:00".to_string(),
            deleted_by: "pc".to_string(),
        });
        let ids = |v: Vec<&Habit>| v.iter().map(|h| h.id.clone()).collect::<Vec<_>>();
        assert_eq!(ids(today_list(&data)), vec!["h1"]);
        assert_eq!(ids(management_list(&data)), vec!["h1", "h2", "h3"]);
        assert_eq!(ids(stats_list(&data)), vec!["h1", "h3"]);
    }

    #[test]
    fn lists_sort_by_sort_order_then_id() {
        let mut a = habit("b");
        a.sort_order = 1;
        let mut b = habit("a");
        b.sort_order = 1;
        let mut c = habit("c");
        c.sort_order = 0;
        let data = data_with(vec![a, b, c], Vec::new());
        let ids: Vec<String> = management_list(&data)
            .iter()
            .map(|h| h.id.clone())
            .collect();
        assert_eq!(ids, vec!["c", "a", "b"]);
    }

    #[test]
    fn other_habits_events_do_not_count() {
        let mut other = event("e1", "2026-10-20", CheckinStatus::Done);
        other.habit_id = "h2".to_string();
        let data = data_with(vec![habit("h1")], vec![other]);
        assert!(effective_events(&data, "h1").is_empty());
        assert_eq!(status_on(&data, "h1", "2026-10-20"), DayStatus::Unconfirmed);
    }
}
