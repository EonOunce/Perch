//! 目标（goalHistory）的生效规则。DESIGN §3.3：习惯上的 `targetMode` / `weeklyTarget`
//! 必须等于 `goalHistory` 最后一项；改目标是追加一条带生效日的记录，不覆盖历史。

use crate::date_boundary::{local_date, parse_date, parse_instant};
use crate::model::{GoalEntry, Habit, TargetMode};

/// 某一天生效的目标。
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct EffectiveGoal {
    pub mode: TargetMode,
    pub weekly_target: Option<i32>,
    /// 这条目标从哪天开始生效，用来判断「本周过半时目标只算一半」。
    pub effective_from: String,
}

/// 取 `effectiveFrom <= date` 的最后一条。
/// 历史为空、或查的日期比最早一条还早时，退回习惯上的当前值（老数据或手改过没写历史），
/// 不猜一个生效日，也不把今天的目标当成一开始的目标。
pub fn goal_on(habit: &Habit, date: &str) -> EffectiveGoal {
    let Some(day) = parse_date(date) else {
        return current(habit);
    };
    let mut best: Option<&GoalEntry> = None;
    for entry in &habit.goal_history {
        let Some(from) = parse_date(&entry.effective_from) else {
            continue;
        };
        if from > day {
            continue;
        }
        // 同一天有多条时取数组里靠后的那条，与「最后一项是当前值」的约定一致。
        if best.is_none_or(|b| {
            parse_date(&b.effective_from)
                .map(|d| d <= from)
                .unwrap_or(true)
        }) {
            best = Some(entry);
        }
    }
    best.map(|e| EffectiveGoal {
        mode: e.target_mode,
        weekly_target: e.weekly_target,
        effective_from: e.effective_from.clone(),
    })
    .unwrap_or_else(|| current(habit))
}

/// 历史里找不到更早的条目时用这个：习惯上的当前值，生效日留空。
/// 空串意思是「不知道这套目标从哪天起算」，比编一个日期诚实——专业视图靠 `first_day` 截断，
/// 不会因为这里猜了个生效日就把还没开始的日子算进达标。
fn current(habit: &Habit) -> EffectiveGoal {
    EffectiveGoal {
        mode: habit.target_mode,
        weekly_target: habit.weekly_target,
        effective_from: String::new(),
    }
}

/// 习惯参与统计的第一个业务日：`createdAt` 那天与最早一条目标的生效日，取更早的那个。
/// 两边都读不出来时返回 None，调用方按「没有历史」处理。
pub fn first_day(habit: &Habit, offset_secs: i32) -> Option<String> {
    let created = parse_instant(&habit.created_at)
        .map(|ms| local_date(ms, offset_secs, crate::date_boundary::DAY_START_HOUR));
    let earliest_goal = habit
        .goal_history
        .iter()
        .filter(|e| parse_date(&e.effective_from).is_some())
        .map(|e| e.effective_from.clone())
        .min_by_key(|s| parse_date(s).unwrap_or(i64::MAX));
    created.into_iter().chain(earliest_goal).min()
}

/// 还没生效的条目（`effectiveFrom > 今天`）。界面用它说「下周一开始」。
pub fn pending<'a>(habit: &'a Habit, today: &str) -> Vec<&'a GoalEntry> {
    let Some(day) = parse_date(today) else {
        return Vec::new();
    };
    habit
        .goal_history
        .iter()
        .filter(|e| parse_date(&e.effective_from).is_some_and(|f| f > day))
        .collect()
}

/// 写一条目标：同一个生效日改成它，更早的插到前面，其余追加到最后；
/// 习惯上的当前值始终同步成最后一项。日期非法时不写，返回 false。
pub fn push_goal(habit: &mut Habit, entry: GoalEntry) -> bool {
    let Some(day) = parse_date(&entry.effective_from) else {
        return false;
    };
    // 同一个生效日只留最后一次的打算：一周里改两次目标，「待生效」里不该留下互相矛盾的两行。
    // 读不出日期的老条目不参与这个判断，原地留着，由 goal_on 自己跳过。
    match habit
        .goal_history
        .iter()
        .position(|e| parse_date(&e.effective_from) == Some(day))
    {
        Some(same_day) => habit.goal_history[same_day] = entry,
        None => {
            match habit
                .goal_history
                .iter()
                .position(|e| parse_date(&e.effective_from).unwrap_or(i64::MIN) > day)
            {
                // 补一条更早的：插到前面，当前值不变。
                Some(at) => habit.goal_history.insert(at, entry),
                None => habit.goal_history.push(entry),
            }
        }
    }
    if let Some(last) = habit.goal_history.last() {
        habit.target_mode = last.target_mode;
        habit.weekly_target = last.weekly_target;
    }
    true
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::model::ConfirmationTiming;

    const CST: i32 = 8 * 3600;

    fn habit(created: &str) -> Habit {
        Habit {
            id: "h1".to_string(),
            name: "阅读".to_string(),
            icon: String::new(),
            color: String::new(),
            operational_definition: String::new(),
            unit: "次".to_string(),
            target_count: 1,
            confirmation_timing: ConfirmationTiming::NextDay,
            target_mode: TargetMode::Weekly,
            weekly_target: Some(4),
            if_then: String::new(),
            reminder_time: None,
            sort_order: 0,
            archived_at: None,
            stats_visible: true,
            goal_history: Vec::new(),
            created_at: format!("{created}T21:00:00.000+08:00"),
            updated_at: format!("{created}T21:00:00.000+08:00"),
        }
    }

    fn goal(from: &str, mode: TargetMode, weekly: Option<i32>) -> GoalEntry {
        GoalEntry {
            effective_from: from.to_string(),
            target_mode: mode,
            weekly_target: weekly,
        }
    }

    #[test]
    fn empty_history_falls_back_to_current_fields() {
        let h = habit("2026-10-20");
        let g = goal_on(&h, "2026-10-26");
        assert_eq!(g.mode, TargetMode::Weekly);
        assert_eq!(g.weekly_target, Some(4));
        assert_eq!(g.effective_from, "", "没有历史就不编一个生效日");
    }

    #[test]
    fn the_last_entry_not_after_that_day_wins() {
        let mut h = habit("2026-10-01");
        h.goal_history = vec![
            goal("2026-10-01", TargetMode::RecordOnly, None),
            goal("2026-10-19", TargetMode::Weekly, Some(4)),
            goal("2026-10-26", TargetMode::Weekly, Some(6)),
        ];
        assert_eq!(goal_on(&h, "2026-10-18").mode, TargetMode::RecordOnly);
        let mid = goal_on(&h, "2026-10-19");
        assert_eq!((mid.mode, mid.weekly_target), (TargetMode::Weekly, Some(4)));
        assert_eq!(goal_on(&h, "2026-10-25").weekly_target, Some(4));
        assert_eq!(goal_on(&h, "2026-10-26").weekly_target, Some(6));
        assert_eq!(goal_on(&h, "2026-12-01").weekly_target, Some(6));
    }

    #[test]
    fn same_effective_day_takes_the_later_array_item() {
        let mut h = habit("2026-10-01");
        h.goal_history = vec![
            goal("2026-10-19", TargetMode::Weekly, Some(4)),
            goal("2026-10-19", TargetMode::Nightly, Some(1)),
        ];
        assert_eq!(goal_on(&h, "2026-10-19").mode, TargetMode::Nightly);
    }

    #[test]
    fn unreadable_entries_are_skipped_not_sorted_to() {
        let mut h = habit("2026-10-01");
        h.goal_history = vec![
            goal("2026-10-19", TargetMode::Weekly, Some(4)),
            goal("下周二", TargetMode::Nightly, Some(1)),
        ];
        assert_eq!(goal_on(&h, "2026-10-26").weekly_target, Some(4));
        assert_eq!(
            pending(&h, "2026-10-26").len(),
            0,
            "读不出来的日期不算待生效"
        );
    }

    #[test]
    fn a_bad_query_date_reads_the_current_goal() {
        let mut h = habit("2026-10-01");
        h.target_mode = TargetMode::Nightly;
        h.weekly_target = Some(1);
        h.goal_history = vec![goal("2026-10-19", TargetMode::Nightly, Some(1))];
        // 日期本身就读不出来时不猜哪天，直接退回习惯上的当前值。
        let g = goal_on(&h, "junk");
        assert_eq!((g.mode, g.weekly_target), (TargetMode::Nightly, Some(1)));
    }

    #[test]
    fn pending_entries_stay_out_of_the_past() {
        let mut h = habit("2026-10-01");
        h.goal_history = vec![
            goal("2026-10-19", TargetMode::Weekly, Some(4)),
            goal("2026-11-02", TargetMode::Weekly, Some(6)),
        ];
        let names: Vec<&str> = pending(&h, "2026-10-26")
            .iter()
            .map(|e| e.effective_from.as_str())
            .collect();
        assert_eq!(names, vec!["2026-11-02"]);
        assert!(
            pending(&h, "2026-11-02").is_empty(),
            "生效当天就不再是待生效"
        );
    }

    #[test]
    fn first_day_is_the_earlier_of_created_and_goal() {
        let mut h = habit("2026-10-20");
        assert_eq!(first_day(&h, CST).as_deref(), Some("2026-10-20"));
        h.goal_history = vec![goal("2026-10-05", TargetMode::Weekly, Some(4))];
        assert_eq!(
            first_day(&h, CST).as_deref(),
            Some("2026-10-05"),
            "补录的目标能前移起点"
        );
        h.created_at = "2026-10-20T03:00:00.000+08:00".to_string();
        assert_eq!(
            first_day(&h, CST).as_deref(),
            Some("2026-10-05"),
            "凌晨三点建的习惯算前一天，也仍然不越过最早目标"
        );
        let mut broken = habit("2026-10-20");
        broken.created_at = "没有时刻".to_string();
        assert_eq!(first_day(&broken, CST), None);
    }

    #[test]
    fn created_at_before_four_reads_the_previous_day() {
        let mut h = habit("2026-10-26");
        h.created_at = "2026-10-26T03:30:00.000+08:00".to_string();
        assert_eq!(first_day(&h, CST).as_deref(), Some("2026-10-25"));
    }

    #[test]
    fn push_keeps_history_ordered_and_syncs_current() {
        let mut h = habit("2026-10-01");
        h.goal_history = vec![goal("2026-10-19", TargetMode::Weekly, Some(4))];
        assert!(push_goal(
            &mut h,
            goal("2026-10-26", TargetMode::Weekly, Some(6))
        ));
        assert_eq!(h.goal_history.len(), 2);
        assert_eq!(h.target_mode, TargetMode::Weekly);
        assert_eq!(h.weekly_target, Some(6));

        // 补一条更早的：插在中间，当前值不动。
        assert!(push_goal(
            &mut h,
            goal("2026-10-05", TargetMode::RecordOnly, None)
        ));
        let days: Vec<&str> = h
            .goal_history
            .iter()
            .map(|e| e.effective_from.as_str())
            .collect();
        assert_eq!(days, vec!["2026-10-05", "2026-10-19", "2026-10-26"]);
        assert_eq!(h.weekly_target, Some(6), "补历史不该改当前目标");
        assert_eq!(goal_on(&h, "2026-10-10").mode, TargetMode::RecordOnly);
    }

    #[test]
    fn push_rejects_an_unusable_date_without_touching_anything() {
        let mut h = habit("2026-10-01");
        h.goal_history = vec![goal("2026-10-19", TargetMode::Weekly, Some(4))];
        assert!(!push_goal(
            &mut h,
            goal("2026-02-30", TargetMode::Nightly, Some(1))
        ));
        assert_eq!(h.goal_history.len(), 1);
        assert_eq!(h.weekly_target, Some(4));
    }

    #[test]
    fn a_second_change_for_the_same_week_replaces_the_first() {
        let mut h = habit("2026-10-01");
        h.goal_history = vec![goal("2026-10-26", TargetMode::Weekly, Some(3))];
        assert!(push_goal(
            &mut h,
            goal("2026-11-02", TargetMode::Weekly, Some(5))
        ));
        // 同一周里又改一次：待生效只留最后一次，否则面板上会出现两行互相矛盾的话。
        assert!(push_goal(
            &mut h,
            goal("2026-11-02", TargetMode::Weekly, Some(4))
        ));
        let days: Vec<&str> = h
            .goal_history
            .iter()
            .map(|e| e.effective_from.as_str())
            .collect();
        assert_eq!(days, vec!["2026-10-26", "2026-11-02"]);
        assert_eq!(h.weekly_target, Some(4), "最后一项仍是当前值");
        assert_eq!(goal_on(&h, "2026-10-26").weekly_target, Some(3));
        assert_eq!(goal_on(&h, "2026-11-02").weekly_target, Some(4));
        assert_eq!(pending(&h, "2026-10-26").len(), 1);
    }
}
