//! 前端看到的形状，以及它能调用的动作。
//!
//! 口径全部在这里算完：卡片拿到的是「今天算不算数、本周几个、目标从哪天起」这些已经判好的字段，
//! JS 只把字符串放进 DOM。同一个公式如果在 Rust 和 JS 各写一遍，迟早会算出两个数（DESIGN §5.4）。

use perch_core::edit::{self, NewHabit, Origin};
use perch_core::habits::DayStatus;
use perch_core::model::{CheckinStatus, ConfirmationTiming, EntityType, TargetMode};
use perch_core::{date_boundary, goal, habits, Data};
use serde::{Deserialize, Serialize};
use uuid::Uuid;

use crate::clock;
use crate::store::{Perch, SharedState};

/// 待确认区一次看几天（含今天）。再长就变成一堵墙，不如让人去记录视图翻。
const PENDING_WINDOW_DAYS: i64 = 6;

/// `sameDay` / `nextDay`——拼写必须和 `ConfirmationTiming` 序列化出来的一致，钉在下面那个测试里。
fn timing_text(timing: ConfirmationTiming) -> &'static str {
    match timing {
        ConfirmationTiming::SameDay => "sameDay",
        ConfirmationTiming::NextDay => "nextDay",
    }
}

fn mode_text(mode: TargetMode) -> &'static str {
    match mode {
        TargetMode::RecordOnly => "recordOnly",
        TargetMode::Weekly => "weekly",
        TargetMode::Nightly => "nightly",
        TargetMode::Custom => "custom",
    }
}

/// 界面只有三种一天状态。`void` 已经被 `habits::status_of` 折算成未确认（§3.3），不往外传。
fn status_text(status: DayStatus) -> &'static str {
    match status {
        DayStatus::Done => "done",
        DayStatus::NotDone => "notDone",
        DayStatus::Unconfirmed => "unconfirmed",
    }
}

#[derive(Serialize)]
#[serde(rename_all = "camelCase")]
pub struct HabitCard {
    pub id: String,
    pub name: String,
    pub icon: String,
    pub color: String,
    pub unit: String,
    pub confirmation_timing: &'static str,
    /// 今天生效的目标，而不是习惯上那个「最后一项」——目标可以周中才改（§2.4）。
    pub target_mode: &'static str,
    pub weekly_target: Option<i32>,
    pub target_from: String,
    pub today_status: &'static str,
    /// 次日确认型的习惯当晚不给打卡按钮（§2.3）。这条判断由后端给，界面不自己比日期。
    pub can_tap_today: bool,
    /// 本周一到今天为止的已确认天数。
    pub week_done: i64,
}

#[derive(Serialize)]
#[serde(rename_all = "camelCase")]
pub struct PendingItem {
    pub habit_id: String,
    pub habit_name: String,
    pub color: String,
    pub local_date: String,
    pub is_today: bool,
    /// 能不能就地确认：次日确认型的「今天」还不该有按钮。
    pub confirmable: bool,
}

#[derive(Serialize)]
#[serde(rename_all = "camelCase")]
pub struct NoteRow {
    pub id: String,
    pub text: String,
    pub tags: Vec<String>,
    pub created_at: String,
}

#[derive(Serialize)]
#[serde(rename_all = "camelCase")]
pub struct ViewState {
    pub today: String,
    pub week_start: String,
    pub week_end: String,
    /// 一个在用习惯都没有（§2.4：空状态显示「添加第一个习惯」，不画假数据）。
    pub is_empty: bool,
    pub habits: Vec<HabitCard>,
    pub pending: Vec<PendingItem>,
    /// 折叠态的日期栏要显示待确认个数，不该自己去数。
    pub pending_total: usize,
    /// 今天的心情。没记过就是 `null`，不是 0 分（§2.6）。
    pub mood: Option<i32>,
    pub notes: Vec<NoteRow>,
    /// 文件坏了或版本更新过：界面上说明原因，写入已经被挡住。
    pub storage_error: Option<String>,
}

/// 新增习惯表单。没填的项落在 §2.4 那组默认值上，那份默认值只在 perch-core 写了一次。
#[derive(Debug, Default, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct HabitForm {
    pub name: String,
    #[serde(default)]
    pub operational_definition: Option<String>,
    #[serde(default)]
    pub unit: Option<String>,
    #[serde(default)]
    pub target_count: Option<i32>,
    #[serde(default)]
    pub confirmation_timing: Option<ConfirmationTiming>,
    #[serde(default)]
    pub target_mode: Option<TargetMode>,
    #[serde(default)]
    pub weekly_target: Option<i32>,
    #[serde(default)]
    pub if_then: Option<String>,
}

impl HabitForm {
    fn into_new_habit(self) -> NewHabit {
        let base = NewHabit::default();
        NewHabit {
            name: self.name,
            operational_definition: self
                .operational_definition
                .unwrap_or(base.operational_definition),
            unit: self.unit.unwrap_or(base.unit),
            target_count: self.target_count.unwrap_or(base.target_count),
            confirmation_timing: self.confirmation_timing.unwrap_or(base.confirmation_timing),
            target_mode: self.target_mode.unwrap_or(base.target_mode),
            weekly_target: self.weekly_target.or(base.weekly_target),
            if_then: self.if_then.unwrap_or(base.if_then),
        }
    }
}

#[tauri::command]
pub fn view_state(state: tauri::State<'_, SharedState>) -> Result<ViewState, String> {
    state.with(|perch| Ok(build(perch)))
}

#[tauri::command]
pub fn habit_add(
    form: HabitForm,
    state: tauri::State<'_, SharedState>,
) -> Result<ViewState, String> {
    commit(&state, move |data| {
        edit::add_habit(data, &new_id(), form.into_new_habit(), clock::now())
    })
}

#[tauri::command]
pub fn checkin_set(
    habit_id: String,
    local_date: String,
    status: CheckinStatus,
    state: tauri::State<'_, SharedState>,
) -> Result<ViewState, String> {
    commit(&state, move |data| {
        // 设备 ID 是打开时从 settings.json 写进 `Data` 的那份，合并时用来分来源。
        let device = data.device_id.clone();
        edit::set_checkin(
            data,
            &new_id(),
            &habit_id,
            &local_date,
            status,
            Origin {
                source: "manual",
                recorded_by: &device,
            },
            clock::now(),
        )
    })
}

#[tauri::command]
pub fn mood_set(
    local_date: String,
    score: i32,
    state: tauri::State<'_, SharedState>,
) -> Result<ViewState, String> {
    commit(&state, move |data| {
        edit::set_mood(data, &new_id(), &local_date, score, clock::now())
    })
}

#[tauri::command]
pub fn note_add(text: String, state: tauri::State<'_, SharedState>) -> Result<ViewState, String> {
    commit(&state, move |data| {
        edit::add_note(data, &new_id(), &text, clock::now())
    })
}

/// 一次写入 = 一次落盘 + 一份新快照：前端不需要自己猜「改完之后界面该长什么样」。
fn commit(
    state: &SharedState,
    edit_fn: impl FnOnce(&mut Data) -> Result<(), perch_core::EditError>,
) -> Result<ViewState, String> {
    state.with(|perch| {
        perch.commit(edit_fn)?;
        Ok(build(perch))
    })
}

fn new_id() -> String {
    Uuid::new_v4().to_string()
}

/// 把 `Data` 摊成今天视图。这里不出现第二个公式，只调用 perch-core。
fn build(perch: &Perch) -> ViewState {
    let data = perch.data();
    let now = clock::now();
    let today = now.date();
    let week_start = date_boundary::week_start(&today).unwrap_or_else(|| today.clone());
    let week_end = date_boundary::add_days(&week_start, 6).unwrap_or_else(|| week_start.clone());
    let window_from =
        date_boundary::add_days(&today, -PENDING_WINDOW_DAYS).unwrap_or_else(|| today.clone());

    let mut cards: Vec<HabitCard> = Vec::new();
    let mut pending: Vec<PendingItem> = Vec::new();

    for habit in habits::today_list(data) {
        let effective = goal::goal_on(habit, &today);
        let week_done = habits::counts(data, &habit.id, &week_start, &today)
            .map(|counts| counts.done)
            .unwrap_or(0);
        cards.push(HabitCard {
            id: habit.id.clone(),
            name: habit.name.clone(),
            icon: habit.icon.clone(),
            color: habit.color.clone(),
            unit: habit.unit.clone(),
            confirmation_timing: timing_text(habit.confirmation_timing),
            target_mode: mode_text(effective.mode),
            weekly_target: effective.weekly_target,
            target_from: effective.effective_from,
            today_status: status_text(habits::status_on(data, &habit.id, &today)),
            can_tap_today: habit.confirmation_timing == ConfirmationTiming::SameDay,
            week_done,
        });

        // 窗口起点：不早于这个习惯参与统计的第一天（§6.2），也不早于 7 天前。
        let since_first = goal::first_day(habit, now.offset_secs)
            .as_deref()
            .and_then(date_boundary::parse_date)
            .unwrap_or(i64::MIN);
        let from = date_boundary::format_date(
            date_boundary::parse_date(&window_from)
                .unwrap_or(i64::MIN)
                .max(since_first),
        );
        for day in habits::unconfirmed_days(data, &habit.id, &from, &today) {
            let is_today = day == today;
            pending.push(PendingItem {
                habit_id: habit.id.clone(),
                habit_name: habit.name.clone(),
                color: habit.color.clone(),
                local_date: day,
                is_today,
                confirmable: habit.confirmation_timing == ConfirmationTiming::SameDay || !is_today,
            });
        }
    }

    let mood = data
        .daily_moods
        .iter()
        .filter(|m| m.local_date == today)
        .find(|m| !habits::is_tombstoned(data, EntityType::DailyMood, &m.id))
        .map(|m| m.score);

    let mut notes: Vec<NoteRow> = data
        .notes
        .iter()
        .filter(|n| n.local_date == today)
        .filter(|n| !habits::is_tombstoned(data, EntityType::Note, &n.id))
        .map(|n| NoteRow {
            id: n.id.clone(),
            text: n.text.clone(),
            tags: n.tags.clone(),
            created_at: n.created_at.clone(),
        })
        .collect();
    // 按瞬间倒序；同一瞬间按 ID 定序，让两端渲染出同一个顺序。
    fn sort_key(row: &NoteRow) -> (i64, &str) {
        (
            date_boundary::parse_instant(&row.created_at).unwrap_or(i64::MIN),
            &row.id,
        )
    }
    notes.sort_by(|a, b| sort_key(b).cmp(&sort_key(a)));

    let pending_total = pending.len();

    ViewState {
        today,
        week_start,
        week_end,
        is_empty: cards.is_empty(),
        habits: cards,
        pending,
        pending_total,
        mood,
        notes,
        storage_error: perch.read_only_reason().map(str::to_string),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// 上面那几个字符串必须和 perch-core 序列化出来的字段名一致，否则前端拿到的是另一个词。
    /// 拼错一个字母编译不报错，所以把它钉在这里。
    #[test]
    fn card_texts_match_the_serialized_field_names() {
        fn text(value: impl Serialize) -> String {
            serde_json::to_value(value)
                .unwrap()
                .as_str()
                .unwrap()
                .to_string()
        }
        assert_eq!(
            timing_text(ConfirmationTiming::SameDay),
            text(ConfirmationTiming::SameDay)
        );
        assert_eq!(
            timing_text(ConfirmationTiming::NextDay),
            text(ConfirmationTiming::NextDay)
        );
        for mode in [
            TargetMode::RecordOnly,
            TargetMode::Weekly,
            TargetMode::Nightly,
            TargetMode::Custom,
        ] {
            assert_eq!(mode_text(mode), text(mode));
        }
        assert_eq!(status_text(DayStatus::Done), text(CheckinStatus::Done));
        assert_eq!(
            status_text(DayStatus::NotDone),
            text(CheckinStatus::NotDone)
        );
        // 未确认不是核心里的某个状态值，是「没有有效事件」，界面自己定名字。
        assert_eq!(status_text(DayStatus::Unconfirmed), "unconfirmed");
    }

    /// 空表单也要拿到 §2.4 那组默认值，而且和 perch-core 用的是同一份。
    #[test]
    fn empty_form_falls_back_on_the_core_defaults() {
        let habit = HabitForm {
            name: "散步".to_string(),
            ..HabitForm::default()
        }
        .into_new_habit();
        let base = NewHabit::default();
        assert_eq!(habit.name, "散步");
        assert_eq!(habit.unit, base.unit);
        assert_eq!(habit.target_count, base.target_count);
        assert_eq!(habit.confirmation_timing, base.confirmation_timing);
        assert_eq!(habit.target_mode, base.target_mode);
        assert_eq!(habit.weekly_target, base.weekly_target);
    }
}
