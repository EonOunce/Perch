//! 前端看到的形状，以及它能调用的动作。
//!
//! 口径全部在这里算完：卡片拿到的是「今天算不算数、本周几个、目标从哪天起」这些已经判好的字段，
//! JS 只把字符串放进 DOM。同一个公式如果在 Rust 和 JS 各写一遍，迟早会算出两个数（DESIGN §5.4）。

use perch_core::edit::{self, HabitEdit, NewHabit, Origin};
use perch_core::habits::DayStatus;
use perch_core::model::{CheckinStatus, ConfirmationTiming, EntityType, Habit, TargetMode};
use perch_core::{date_boundary, goal, habits, records, stats as core_stats, Data};
use serde::{Deserialize, Serialize};
use uuid::Uuid;

use crate::clock;
use crate::store::{Perch, SharedPrefs, SharedState};

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
    /// 折叠态的日期栏要显示待确认个数，不该自己去数。口径与长条列出的行一致：
    /// 当晚那一格还不算「待确认」（§2.3），不进计数。
    pub pending_total: usize,
    /// 今天的心情。没记过就是 `null`，不是 0 分（§2.6）。
    pub mood: Option<i32>,
    pub notes: Vec<NoteRow>,
    /// 文件坏了或版本更新过：界面上说明原因，写入已经被挡住。
    pub storage_error: Option<String>,
}

/// 还没生效的目标条目。面板只把它念成「X 月 X 日 起 每周 N 晚」，不改当前显示（§2.4）。
#[derive(Serialize)]
#[serde(rename_all = "camelCase")]
pub struct PendingGoal {
    pub effective_from: String,
    pub target_mode: &'static str,
    pub weekly_target: Option<i32>,
}

/// 管理面板里的一行。带着今天视图不需要的字段（定义、if–then、归档时刻、统计开关），
/// 也带着「这条目标今天到底算哪个」已经判好的结果。
#[derive(Serialize)]
#[serde(rename_all = "camelCase")]
pub struct HabitRow {
    pub id: String,
    pub name: String,
    pub icon: String,
    pub color: String,
    pub confirmation_timing: &'static str,
    pub operational_definition: String,
    pub if_then: String,
    pub unit: String,
    pub target_count: i32,
    pub target_mode: &'static str,
    pub weekly_target: Option<i32>,
    pub target_from: String,
    pub pending_goals: Vec<PendingGoal>,
    /// `null` 是在用；有值就是归档时刻，界面念成「X 月 X 日 归档 · 只读」。
    pub archived_at: Option<String>,
    pub stats_visible: bool,
}

#[derive(Serialize)]
#[serde(rename_all = "camelCase")]
pub struct ManageState {
    pub today: String,
    pub active: Vec<HabitRow>,
    /// 归档区：面板下方虚线分隔的那一段（§2.4）。
    pub archived: Vec<HabitRow>,
    pub storage_error: Option<String>,
}

/// 记录视图的一份快照。分组、排序、搜索与标签筛选全在 `perch_core::records` 算完（§2.5）。
#[derive(Serialize)]
#[serde(rename_all = "camelCase")]
pub struct RecordsState {
    pub today: String,
    pub days: Vec<records::DayGroup>,
    /// 标签筛选的候选与条数，按条数排好。
    pub tags: Vec<records::TagCount>,
    /// 当前筛选下有多少条，用来写「N 条」这类中性计数。
    pub shown: usize,
    pub total: usize,
    /// 「以前的一天」：没有够旧的记录时是 `None`；有筛选时界面不渲染它（§2.5）。
    pub memory: Option<records::Memory>,
    pub storage_error: Option<String>,
}

/// 统计视图的一份快照。范围合法时给出整份 `stats`；非法时 `stats` 是 `None`，
/// `range_error` 带上那句要就地显示的话——上一次的有效范围留在前端，不在这里悄悄退回默认档（§6.1）。
#[derive(Serialize)]
#[serde(rename_all = "camelCase")]
pub struct StatsState {
    pub stats: Option<perch_core::Stats>,
    pub range_error: Option<String>,
    pub storage_error: Option<String>,
}

/// 习惯表单，新增与修改共用一张。没填的项落在 §2.4 那组默认值上，那份默认值只在 perch-core 写了一次。
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

    /// 修改走同一张表单，但目标两个字段不在这里传：它们归 `habit_goal`，生效日是下一个周一，
    /// 而新增时的生效日是本周周一。一份输入、两条出口，口径才不会分叉。
    fn into_habit_edit(self) -> HabitEdit {
        HabitEdit {
            name: self.name,
            operational_definition: self.operational_definition.unwrap_or_default(),
            unit: self.unit.unwrap_or_default(),
            confirmation_timing: self
                .confirmation_timing
                .unwrap_or(NewHabit::default().confirmation_timing),
            if_then: self.if_then.unwrap_or_default(),
        }
    }
}

#[tauri::command]
pub fn view_state(state: tauri::State<'_, SharedState>) -> Result<ViewState, String> {
    state.with(|perch| Ok(build(perch)))
}

/// 习惯管理面板的数据。和今天视图分开两份：面板只在打开时才要，卡片每一帧都要 `view_state`。
#[tauri::command]
pub fn manage_state(state: tauri::State<'_, SharedState>) -> Result<ManageState, String> {
    state.with(|perch| Ok(build_manage(perch)))
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

#[tauri::command]
pub fn habit_update(
    habit_id: String,
    form: HabitForm,
    state: tauri::State<'_, SharedState>,
) -> Result<ManageState, String> {
    commit_manage(&state, move |data| {
        edit::update_habit(data, &habit_id, form.into_habit_edit(), clock::now())
    })
}

/// 改目标：追加一条 `goalHistory`，新目标从下一个周一起生效，本周仍按原目标显示（§2.4）。
#[tauri::command]
pub fn habit_goal(
    habit_id: String,
    target_mode: TargetMode,
    weekly_target: Option<i32>,
    state: tauri::State<'_, SharedState>,
) -> Result<ManageState, String> {
    commit_manage(&state, move |data| {
        edit::change_goal(data, &habit_id, target_mode, weekly_target, clock::now())
    })
}

/// 归档 / 恢复。归档写 `archivedAt`，历史与事件都不动（§2.4、§12 #20）。
#[tauri::command]
pub fn habit_archive(
    habit_id: String,
    archived: bool,
    state: tauri::State<'_, SharedState>,
) -> Result<ManageState, String> {
    commit_manage(&state, move |data| {
        edit::set_archived(data, &habit_id, archived, clock::now())
    })
}

/// 「统计里显示」开关，默认开着，所以归档不会悄悄改写过去的完成率。
#[tauri::command]
pub fn habit_stats(
    habit_id: String,
    visible: bool,
    state: tauri::State<'_, SharedState>,
) -> Result<ManageState, String> {
    commit_manage(&state, move |data| {
        edit::set_stats_visible(data, &habit_id, visible, clock::now())
    })
}

/// 移除 = 写习惯墓碑。没有撤销窗口，也不可恢复（§2.4）。
#[tauri::command]
pub fn habit_remove(
    habit_id: String,
    state: tauri::State<'_, SharedState>,
) -> Result<ManageState, String> {
    commit_manage(&state, move |data| {
        let device = data.device_id.clone();
        edit::tombstone(data, EntityType::Habit, &habit_id, &device, clock::now()).map(|_| ())
    })
}

/// 记录视图的一份快照：`query` 是搜索词，`tag` 是选中的标签，两者都空就是看全部。
#[tauri::command]
pub fn records_state(
    query: Option<String>,
    tag: Option<String>,
    state: tauri::State<'_, SharedState>,
) -> Result<RecordsState, String> {
    state.with(|perch| Ok(build_records(perch, query.as_deref(), tag.as_deref())))
}

/// 改一条记录的正文。标签跟着正文重解析，归属日不动（§2.5）。
#[tauri::command]
pub fn note_update(
    note_id: String,
    text: String,
    query: Option<String>,
    tag: Option<String>,
    state: tauri::State<'_, SharedState>,
) -> Result<RecordsState, String> {
    commit_records(&state, query, tag, move |data| {
        edit::update_note(data, &note_id, &text, clock::now())
    })
}

/// 删除一条记录 = 写记录墓碑。界面上留 10 秒撤销窗口（§2.9），撤销走 `note_restore`。
#[tauri::command]
pub fn note_remove(
    note_id: String,
    query: Option<String>,
    tag: Option<String>,
    state: tauri::State<'_, SharedState>,
) -> Result<RecordsState, String> {
    commit_records(&state, query, tag, move |data| {
        let device = data.device_id.clone();
        edit::tombstone(data, EntityType::Note, &note_id, &device, clock::now()).map(|_| ())
    })
}

/// 撤销删除：把墓碑撤掉，记录本身一直在数据里，所以回来的还是原来那条（同一个 ID）。
#[tauri::command]
pub fn note_restore(
    note_id: String,
    query: Option<String>,
    tag: Option<String>,
    state: tauri::State<'_, SharedState>,
) -> Result<RecordsState, String> {
    commit_records(&state, query, tag, move |data| {
        edit::lift_tombstone(data, EntityType::Note, &note_id);
        Ok(())
    })
}

/// 统计视图的一份快照：范围与习惯都从界面传来，解释与计算全在 `perch_core::stats`（§6）。
/// 这里不出现第二个完成率公式，也不第二个日界。
#[tauri::command]
pub fn stats_state(
    range: String,
    from: Option<String>,
    to: Option<String>,
    habit: Option<String>,
    state: tauri::State<'_, SharedState>,
) -> Result<StatsState, String> {
    state.with(|perch| {
        Ok(build_stats(
            perch,
            &range,
            from.as_deref(),
            to.as_deref(),
            habit.as_deref(),
        ))
    })
}

/// 加一个标记（§6.6）：一句话必填、未来不行、早于习惯创建不行；同一习惯同一天只能有一个活标记。
/// 句号和习惯的校验都在 `perch_core::edit`，这里只负责带回去新一版统计快照。
#[tauri::command]
pub fn marker_add(
    habit_id: String,
    local_date: String,
    label: String,
    range: String,
    from: Option<String>,
    to: Option<String>,
    state: tauri::State<'_, SharedState>,
) -> Result<StatsState, String> {
    // 快照永远回这个习惯，不跟当前筛选走：标记只属于一个习惯（§6.6）。
    let snapshot = Some(habit_id.clone());
    commit_stats(&state, range, from, to, snapshot, move |data| {
        edit::add_marker(
            data,
            &new_id(),
            &habit_id,
            &local_date,
            &label,
            clock::now(),
        )
    })
}

/// 删标记 = 写标记墓碑（§6.6）：历史点与分段随之重算，打卡事件一个字节都不动。
#[tauri::command]
pub fn marker_remove(
    marker_id: String,
    range: String,
    from: Option<String>,
    to: Option<String>,
    habit: Option<String>,
    state: tauri::State<'_, SharedState>,
) -> Result<StatsState, String> {
    commit_stats(&state, range, from, to, habit, move |data| {
        let device = data.device_id.clone();
        edit::tombstone(data, EntityType::Marker, &marker_id, &device, clock::now()).map(|_| ())
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

/// 管理面板的写入走同一套「改内存 + 落盘 + 返回新快照」，只是返回的是面板那份。
fn commit_manage(
    state: &SharedState,
    edit_fn: impl FnOnce(&mut Data) -> Result<(), perch_core::EditError>,
) -> Result<ManageState, String> {
    state.with(|perch| {
        perch.commit(edit_fn)?;
        Ok(build_manage(perch))
    })
}

/// 记录视图的写入要把当前筛选一起带回去：筛着「#工作」时改完一条，列表不该整屏跳回全部。
fn commit_records(
    state: &SharedState,
    query: Option<String>,
    tag: Option<String>,
    edit_fn: impl FnOnce(&mut Data) -> Result<(), perch_core::EditError>,
) -> Result<RecordsState, String> {
    state.with(|perch| {
        perch.commit(edit_fn)?;
        Ok(build_records(perch, query.as_deref(), tag.as_deref()))
    })
}

/// 统计面板的写入（标记）也要带着当前范围回来，否则加完一个标记整屏跳回默认。
fn commit_stats(
    state: &SharedState,
    range: String,
    from: Option<String>,
    to: Option<String>,
    habit: Option<String>,
    edit_fn: impl FnOnce(&mut Data) -> Result<(), perch_core::EditError>,
) -> Result<StatsState, String> {
    state.with(|perch| {
        perch.commit(edit_fn)?;
        Ok(build_stats(
            perch,
            &range,
            from.as_deref(),
            to.as_deref(),
            habit.as_deref(),
        ))
    })
}

/// 昨天还剩几项没确认。早晨浮出要知道这个数（§2.3），而数怎么算只有 `build` 一处。
pub fn awaiting_count(perch: &Perch) -> usize {
    build(perch).pending_total
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

    let pending_total = pending.iter().filter(|item| !item.is_today).count();

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

/// 把 `Data` 摊成管理面板。被移除的习惯根本进不来（`management_list` 已经按墓碑过滤），
/// 归档的单独一份，排在面板下方。
fn build_manage(perch: &Perch) -> ManageState {
    let data = perch.data();
    let today = clock::now().date();
    let mut active: Vec<HabitRow> = Vec::new();
    let mut archived: Vec<HabitRow> = Vec::new();
    for habit in habits::management_list(data) {
        let row = manage_row(habit, &today);
        if habits::is_archived(habit) {
            archived.push(row);
        } else {
            active.push(row);
        }
    }
    ManageState {
        today,
        active,
        archived,
        storage_error: perch.read_only_reason().map(str::to_string),
    }
}

fn manage_row(habit: &Habit, today: &str) -> HabitRow {
    let effective = goal::goal_on(habit, today);
    HabitRow {
        id: habit.id.clone(),
        name: habit.name.clone(),
        icon: habit.icon.clone(),
        color: habit.color.clone(),
        confirmation_timing: timing_text(habit.confirmation_timing),
        operational_definition: habit.operational_definition.clone(),
        if_then: habit.if_then.clone(),
        unit: habit.unit.clone(),
        target_count: habit.target_count,
        target_mode: mode_text(effective.mode),
        weekly_target: effective.weekly_target,
        target_from: effective.effective_from,
        pending_goals: goal::pending(habit, today)
            .into_iter()
            .map(|entry| PendingGoal {
                effective_from: entry.effective_from.clone(),
                target_mode: mode_text(entry.target_mode),
                weekly_target: entry.weekly_target,
            })
            .collect(),
        archived_at: habit.archived_at.clone(),
        stats_visible: habit.stats_visible,
    }
}

/// 记录视图的一份快照。时间线怎么分组、搜索命中谁、标签怎么排，都是 `perch_core::records` 说的。
fn build_records(perch: &Perch, query: Option<&str>, tag: Option<&str>) -> RecordsState {
    let data = perch.data();
    let today = clock::now().date();
    let days = records::timeline(data, query.unwrap_or(""), tag.unwrap_or(""));
    let shown = days.iter().map(|group| group.notes.len()).sum();
    let total = data
        .notes
        .iter()
        .filter(|note| !habits::is_tombstoned(data, EntityType::Note, &note.id))
        .count();
    RecordsState {
        memory: records::daily_memory(data, &today),
        today,
        days,
        tags: records::tag_counts(data),
        shown,
        total,
        storage_error: perch.read_only_reason().map(str::to_string),
    }
}

/// 统计视图的一份快照。范围怎么解释、数字怎么算，全在 `perch_core::stats`；
/// 这里只负责读一次时钟、取一次数据。非法范围带回那句现成的文案，不偷偷换成 90 天（§6.1）。
fn build_stats(
    perch: &Perch,
    kind: &str,
    from: Option<&str>,
    to: Option<&str>,
    habit: Option<&str>,
) -> StatsState {
    let now = clock::now();
    let today = now.date();
    let storage_error = perch.read_only_reason().map(str::to_string);
    let scope = habit.unwrap_or(core_stats::SCOPE_ALL);
    match core_stats::resolve_range(&today, kind, from.unwrap_or(""), to.unwrap_or("")) {
        Ok(spec) => StatsState {
            stats: Some(core_stats::stats(
                perch.data(),
                &today,
                now.offset_secs,
                spec,
                scope,
            )),
            range_error: None,
            storage_error,
        },
        Err(message) => StatsState {
            stats: None,
            range_error: Some(message),
            storage_error,
        },
    }
}

/// 开机时界面要知道的几件本地设置。折叠与长条是两根开关（§2.8），位置由窗口事件自己记（main.rs）。
/// 自启两项给首次启动那一次问答用：`autostartAsked` 为真就再也不问（§12 #6）。
/// `aboutHinted` 管「关于栖迟」那个入口提示，同样只提示一次（§2.10）。
#[derive(Serialize)]
#[serde(rename_all = "camelCase")]
pub struct PrefsView {
    pub collapsed: bool,
    pub strip_rows: bool,
    pub autostart: bool,
    pub autostart_asked: bool,
    pub about_hinted: bool,
}

#[tauri::command]
pub fn prefs_get(state: tauri::State<'_, SharedPrefs>) -> PrefsView {
    state
        .with(|prefs| {
            let settings = prefs.settings();
            Ok(PrefsView {
                collapsed: settings.collapsed,
                strip_rows: settings.strip_rows,
                autostart: settings.autostart,
                autostart_asked: settings.autostart_asked,
                about_hinted: settings.about_hinted,
            })
        })
        // 设置读不出来时按默认值画：折叠展开、长条摊开、不自启、还没问过。
        .unwrap_or(PrefsView {
            collapsed: false,
            strip_rows: true,
            autostart: false,
            autostart_asked: false,
            about_hinted: false,
        })
}

/// 只写传进来的那几项：折叠与长条收起各自记住，互不覆盖。
#[tauri::command]
pub fn prefs_set(
    collapsed: Option<bool>,
    strip_rows: Option<bool>,
    about_hinted: Option<bool>,
    state: tauri::State<'_, SharedPrefs>,
) -> Result<(), String> {
    state.with(|prefs| {
        prefs.update(|settings| {
            if let Some(value) = collapsed {
                settings.collapsed = value;
            }
            if let Some(value) = strip_rows {
                settings.strip_rows = value;
            }
            if let Some(value) = about_hinted {
                settings.about_hinted = value;
            }
        })
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::store::Settings;

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

    /// 修改用的那份表单里没有目标字段：目标只能经 `habit_goal` 走 `change_goal`，
    /// 否则「改个名字」会顺手把生效日挪掉。
    #[test]
    fn the_edit_form_carries_metadata_only() {
        let edit = HabitForm {
            name: "读书".to_string(),
            unit: Some("晚".to_string()),
            target_mode: Some(TargetMode::RecordOnly),
            weekly_target: None,
            ..HabitForm::default()
        }
        .into_habit_edit();
        assert_eq!(edit.name, "读书");
        assert_eq!(edit.unit, "晚");
        assert_eq!(edit.operational_definition, "");
        assert_eq!(edit.if_then, "");
        assert_eq!(
            edit.confirmation_timing,
            NewHabit::default().confirmation_timing
        );
    }

    fn temp_perch(tag: &str) -> (Perch, std::path::PathBuf) {
        let mut dir = std::env::temp_dir();
        dir.push(format!("perch-manage-{tag}-{}", Uuid::new_v4()));
        std::fs::create_dir_all(&dir).unwrap();
        (Perch::open_at(dir.clone()).unwrap(), dir)
    }

    fn add(perch: &mut Perch, id: &'static str, name: &'static str) {
        perch
            .commit(|data| {
                edit::add_habit(
                    data,
                    id,
                    NewHabit {
                        name: name.to_string(),
                        ..NewHabit::default()
                    },
                    clock::now(),
                )
            })
            .unwrap();
    }

    /// 折叠日期栏那个数与长条列出的行是同一口径：今晚那一格还不算「待确认」（§2.3），
    /// 它留在 `pending` 里给次日判断用，但不进计数——否则数字永远比看得见的行多一个。
    #[test]
    fn the_strip_count_leaves_out_tonight() {
        let (mut perch, dir) = temp_perch("strip-count");
        let now = clock::now();
        perch
            .commit(|data| {
                edit::add_habit(
                    data,
                    "h1",
                    NewHabit {
                        name: "阅读".to_string(),
                        confirmation_timing: ConfirmationTiming::NextDay,
                        ..NewHabit::default()
                    },
                    perch_core::Now {
                        instant_ms: now.instant_ms - 3 * 86_400_000,
                        offset_secs: now.offset_secs,
                    },
                )
            })
            .unwrap();
        let view = build(&perch);
        let today = view.today.clone();
        assert!(
            view.pending
                .iter()
                .any(|item| item.local_date == today && !item.confirmable),
            "今晚那一格还在列表里，只是还不能就地确认"
        );
        assert_eq!(
            view.pending_total,
            view.pending
                .iter()
                .filter(|i| i.local_date != today)
                .count(),
            "计数只数长条真会列出来的那些行"
        );
        assert_eq!(
            view.pending.len(),
            view.pending_total + 1,
            "整份列表里只有今晚那一格被计数排除"
        );
        std::fs::remove_dir_all(dir).unwrap();
    }

    /// 归档不是删除：习惯从今天视图走开，但留在管理面板下方的归档区，历史也还在。
    #[test]
    fn archiving_moves_a_habit_into_its_own_section() {
        let (mut perch, dir) = temp_perch("archive");
        add(&mut perch, "h1", "阅读");
        let before = build_manage(&perch);
        assert_eq!(before.active.len(), 1);
        assert!(before.archived.is_empty());

        perch
            .commit(|data| edit::set_archived(data, "h1", true, clock::now()))
            .unwrap();
        let after = build_manage(&perch);
        assert!(after.active.is_empty(), "归档后不该还出现在在用列表");
        assert_eq!(after.archived.len(), 1);
        assert!(after.archived[0].archived_at.is_some());
        // 默认开着「统计里显示」，所以归档不会悄悄改写过去的完成率。
        assert!(after.archived[0].stats_visible);
        assert!(build(&perch).habits.is_empty(), "今天视图也不该再列它");
        std::fs::remove_dir_all(dir).unwrap();
    }

    /// 移除写墓碑，从今天、管理列表、归档区一起消失，而且不可恢复（§2.4）。
    #[test]
    fn removing_leaves_the_management_list_too() {
        let (mut perch, dir) = temp_perch("remove");
        add(&mut perch, "h1", "阅读");
        let device = perch.device_id().to_string();
        perch
            .commit(move |data| {
                edit::tombstone(data, EntityType::Habit, "h1", &device, clock::now()).map(|_| ())
            })
            .unwrap();
        let after = build_manage(&perch);
        assert!(after.active.is_empty());
        assert!(after.archived.is_empty());
        assert!(build(&perch).habits.is_empty());
        std::fs::remove_dir_all(dir).unwrap();
    }

    /// 改已有习惯的目标：本周仍按原目标显示，新目标作为「待生效」一起带过去。
    #[test]
    fn a_goal_change_is_pending_until_next_monday_and_does_not_move_this_week() {
        let (mut perch, dir) = temp_perch("goal");
        add(&mut perch, "h1", "阅读");
        let this_week = build_manage(&perch).active[0].weekly_target.unwrap();
        perch
            .commit(|data| {
                edit::change_goal(
                    data,
                    "h1",
                    TargetMode::Weekly,
                    Some(this_week + 2),
                    clock::now(),
                )
            })
            .unwrap();
        let state = build_manage(&perch);
        let row = &state.active[0];
        assert_eq!(
            row.weekly_target,
            Some(this_week),
            "本周的显示值不该被下周的目标顶掉"
        );
        assert_eq!(row.pending_goals.len(), 1);
        assert_eq!(row.pending_goals[0].weekly_target, Some(this_week + 2));
        assert!(
            row.pending_goals[0].effective_from > state.today,
            "待生效条目的日期在该周周一，必然晚于今天"
        );
        std::fs::remove_dir_all(dir).unwrap();
    }

    /// 改名要真的落盘：重启之后仍然是新名字，而且不留下多余的习惯条目。
    #[test]
    fn renaming_survives_a_reopen() {
        let (mut perch, dir) = temp_perch("rename");
        add(&mut perch, "h1", "阅读");
        perch
            .commit(|data| {
                edit::update_habit(
                    data,
                    "h1",
                    HabitEdit {
                        name: "  夜里读书  ".to_string(),
                        operational_definition: String::new(),
                        unit: "晚".to_string(),
                        confirmation_timing: ConfirmationTiming::NextDay,
                        if_then: String::new(),
                    },
                    clock::now(),
                )
            })
            .unwrap();
        let reopened = Perch::open_at(dir.clone()).unwrap();
        let rows = build_manage(&reopened).active;
        assert_eq!(rows.len(), 1);
        assert_eq!(rows[0].name, "夜里读书");
        assert_eq!(rows[0].unit, "晚");
        assert_eq!(rows[0].confirmation_timing, "nextDay");
        std::fs::remove_dir_all(dir).unwrap();
    }

    fn note(perch: &mut Perch, id: &'static str, text: &'static str) {
        perch
            .commit(|data| edit::add_note(data, id, text, clock::now()))
            .unwrap();
    }

    /// 记录视图拿到的就是排好序、去过墓碑的时间线；前端不需要再算一遍（§2.5）。
    #[test]
    fn the_records_snapshot_carries_the_grouped_timeline() {
        let (mut perch, dir) = temp_perch("records");
        note(&mut perch, "n1", "备课到很晚 #工作");
        note(&mut perch, "n2", "散步二十分钟");
        let state = build_records(&perch, None, None);
        assert_eq!(state.today, clock::now().date());
        assert_eq!(state.days.len(), 1, "同一天写的两条进同一组");
        assert_eq!(state.days[0].notes.len(), 2);
        assert_eq!(state.shown, 2);
        assert_eq!(state.total, 2);
        assert_eq!(state.tags[0].tag, "工作");
        assert_eq!(state.tags[0].count, 1);
        std::fs::remove_dir_all(dir).unwrap();
    }

    #[test]
    fn filtering_narrows_the_snapshot_without_touching_the_tag_list() {
        let (mut perch, dir) = temp_perch("filter");
        note(&mut perch, "n1", "备课到很晚 #工作");
        note(&mut perch, "n2", "周末读了一本 #阅读");
        let by_tag = build_records(&perch, None, Some("#工作"));
        assert_eq!(by_tag.shown, 1);
        assert_eq!(by_tag.days[0].notes[0].id, "n1");
        // 标签候选始终是全集：筛中一个之后还能直接换到另一个，不会把自己筛死。
        assert_eq!(by_tag.tags.len(), 2);
        let by_text = build_records(&perch, Some("散步没有"), None);
        assert_eq!(by_text.shown, 0);
        assert_eq!(by_text.days.len(), 0, "没有命中就是空时间线，不是空一天");
        std::fs::remove_dir_all(dir).unwrap();
    }

    #[test]
    fn a_removed_note_leaves_the_timeline_and_comes_back_on_undo() {
        let (mut perch, dir) = temp_perch("undo");
        note(&mut perch, "n1", "一句话");
        perch
            .commit(|data| {
                let device = data.device_id.clone();
                edit::tombstone(data, EntityType::Note, "n1", &device, clock::now()).map(|_| ())
            })
            .unwrap();
        let gone = build_records(&perch, None, None);
        assert!(gone.days.is_empty());
        assert_eq!(gone.total, 0, "已删除的不算进条数");

        perch
            .commit(|data| {
                edit::lift_tombstone(data, EntityType::Note, "n1");
                Ok(())
            })
            .unwrap();
        let back = build_records(&perch, None, None);
        assert_eq!(
            back.days[0].notes[0].id, "n1",
            "撤销回来的是同一条，不是新写的一条"
        );
        assert!(Perch::open_at(dir.clone())
            .unwrap()
            .data()
            .tombstones
            .is_empty());
        std::fs::remove_dir_all(dir).unwrap();
    }

    /// 前端读的是 `prefs.collapsed`、`prefs.stripRows`、`prefs.autostartAsked`、`prefs.aboutHinted`
    /// （collapse.js 开机流程、autostart.js 首启问答、about.js 入口提示），
    /// 字段名拼错不会编译报错，所以钉在这里。
    #[test]
    fn the_prefs_view_uses_the_field_names_the_scripts_read() {
        let json = serde_json::to_value(PrefsView {
            collapsed: true,
            strip_rows: false,
            autostart: true,
            autostart_asked: false,
            about_hinted: true,
        })
        .unwrap();
        assert_eq!(json["collapsed"], true);
        assert_eq!(json["stripRows"], false);
        assert_eq!(json["autostart"], true);
        assert_eq!(json["autostartAsked"], false);
        assert_eq!(json["aboutHinted"], true);
    }

    /// 全新安装必须还没提示过——§2.10 那句「首次打开只提示一次入口存在」的前提
    /// 是这个标志的默认值是 false，而不是某个文件里恰好没写这个键。
    #[test]
    fn the_about_entry_hint_starts_unseen() {
        assert!(
            !Settings::default().about_hinted,
            "默认值写成 true 就等于这个入口永远不提示"
        );
    }

    /// 统计视图的输入契约：非法范围变成一句能就地显示的话，而不是一档悄悄替换掉的默认值；
    /// 快照里那几个 JSON 字段名必须和 `src/stats.js` 读的一致，拼错编译不报错，所以钉在这里。
    #[test]
    fn a_bad_range_is_a_message_not_a_silent_default() {
        let (mut perch, dir) = temp_perch("stats");
        add(&mut perch, "h1", "说话");

        let ok = build_stats(&perch, "7", None, None, None);
        assert!(ok.stats.is_some());
        assert!(ok.range_error.is_none());
        let json = serde_json::to_value(&ok).unwrap();
        assert!(json["stats"]["scopeName"].is_string());
        assert_eq!(json["stats"]["scopeOptions"][0]["id"], "h1");
        assert!(json["stats"]["volume"]["perDay"].is_boolean());
        assert!(json["stats"]["heatmap"]["rows"].is_array());
        assert!(json["rangeError"].is_null());

        let inverted = build_stats(
            &perch,
            "custom",
            Some("2026-10-20"),
            Some("2026-10-01"),
            None,
        );
        assert!(inverted.stats.is_none(), "非法范围不该给半成品快照");
        assert_eq!(
            inverted.range_error.as_deref(),
            Some("起始日期不能晚于结束日期。")
        );
        std::fs::remove_dir_all(dir).unwrap();
    }

    /// 专业视图的字段名同样是前端契约：`src/stats.js` 按 camelCase 读，
    /// 而且「全部习惯」必须是 `null`——分段对比只对单个习惯成立（§6.6）。
    #[test]
    fn only_a_single_habit_gets_the_pro_view_payload() {
        let (mut perch, dir) = temp_perch("pro");
        add(&mut perch, "h1", "阅读");

        let all = serde_json::to_value(build_stats(&perch, "30", None, None, None)).unwrap();
        assert!(all["stats"]["pro"].is_null());

        let one = serde_json::to_value(build_stats(&perch, "30", None, None, Some("h1"))).unwrap();
        let pro = &one["stats"]["pro"];
        assert!(pro["weeks"].is_array());
        assert!(pro["segments"].is_array());
        assert!(pro["markers"].is_array());
        assert!(pro["targetRate"].is_number());
        assert!(pro["latestMean"].is_null(), "没有标记就没有分段均值");
        assert!(pro["diff"].is_null());
        assert!(pro.get("volatility").is_some());
        assert_eq!(pro["segments"].as_array().unwrap().len(), 0);
        std::fs::remove_dir_all(dir).unwrap();
    }
}
