//! 可同步的数据模型，字段与 DESIGN §3.2 的 JSON 一一对应。
//!
//! 未知字段在反序列化时被丢掉、再写回去就没了。V1 单机无所谓，但 Phase 2 的
//! 「新版本写、旧版本读」必须先过 §4.7 的 schemaVersion 检查，不能指望这里兜底。

use serde::{Deserialize, Serialize};

/// 当前写出的 `schemaVersion`。只增不减。
pub const SCHEMA_VERSION: u32 = 1;

pub(crate) fn default_true() -> bool {
    true
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct Data {
    pub schema_version: u32,
    #[serde(default)]
    pub generated_at: Option<String>,
    #[serde(default)]
    pub device_id: String,
    #[serde(default)]
    pub habits: Vec<Habit>,
    #[serde(default)]
    pub markers: Vec<Marker>,
    #[serde(default)]
    pub checkin_events: Vec<CheckinEvent>,
    #[serde(default)]
    pub notes: Vec<Note>,
    #[serde(default)]
    pub daily_moods: Vec<DailyMood>,
    #[serde(default)]
    pub tombstones: Vec<Tombstone>,
}

impl Data {
    /// 首次启动的样子：一个习惯都没有（DESIGN §2.4「不预置任何习惯」）。
    pub fn empty(device_id: impl Into<String>) -> Self {
        Data {
            schema_version: SCHEMA_VERSION,
            generated_at: None,
            device_id: device_id.into(),
            habits: Vec::new(),
            markers: Vec::new(),
            checkin_events: Vec::new(),
            notes: Vec::new(),
            daily_moods: Vec::new(),
            tombstones: Vec::new(),
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub enum ConfirmationTiming {
    #[serde(rename = "sameDay")]
    SameDay,
    #[serde(rename = "nextDay")]
    NextDay,
}

/// 取值与 DESIGN §3.2 / mockup 里的字符串一致：`recordOnly` / `weekly` / `nightly` / `custom`。
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub enum TargetMode {
    RecordOnly,
    Weekly,
    Nightly,
    Custom,
}

/// `Void` 是撤销误点用的内部状态，界面上仍然算未确认（DESIGN §3.3）。
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub enum CheckinStatus {
    Done,
    NotDone,
    Void,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub enum EntityType {
    Habit,
    Marker,
    CheckinEvent,
    Note,
    DailyMood,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct Habit {
    pub id: String,
    pub name: String,
    #[serde(default)]
    pub icon: String,
    #[serde(default)]
    pub color: String,
    #[serde(default)]
    pub operational_definition: String,
    #[serde(default = "default_unit")]
    pub unit: String,
    #[serde(default = "default_one")]
    pub target_count: i32,
    #[serde(default = "default_timing")]
    pub confirmation_timing: ConfirmationTiming,
    #[serde(default = "default_mode")]
    pub target_mode: TargetMode,
    #[serde(default)]
    pub weekly_target: Option<i32>,
    #[serde(default)]
    pub if_then: String,
    #[serde(default)]
    pub reminder_time: Option<String>,
    #[serde(default)]
    pub sort_order: i32,
    #[serde(default)]
    pub archived_at: Option<String>,
    #[serde(default = "default_true")]
    pub stats_visible: bool,
    #[serde(default)]
    pub goal_history: Vec<GoalEntry>,
    pub created_at: String,
    pub updated_at: String,
}

fn default_unit() -> String {
    "次".to_string()
}
fn default_one() -> i32 {
    1
}
fn default_timing() -> ConfirmationTiming {
    ConfirmationTiming::SameDay
}
fn default_mode() -> TargetMode {
    TargetMode::Weekly
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct GoalEntry {
    /// `YYYY-MM-DD`（localDate）。这一条从这里开始生效。
    pub effective_from: String,
    pub target_mode: TargetMode,
    #[serde(default)]
    pub weekly_target: Option<i32>,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct Marker {
    pub id: String,
    pub habit_id: String,
    pub local_date: String,
    pub label: String,
    pub created_at: String,
    pub updated_at: String,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct CheckinEvent {
    pub id: String,
    pub habit_id: String,
    pub local_date: String,
    pub status: CheckinStatus,
    #[serde(default)]
    pub confirmed_at: Option<String>,
    /// `manual` 或 `morningReview`，只做来源统计，不参与口径计算。
    #[serde(default)]
    pub source: String,
    /// 非空表示这条是对另一条的更正，被更正的那条就此失效。
    #[serde(default)]
    pub supersedes_id: Option<String>,
    #[serde(default)]
    pub recorded_by: String,
    #[serde(default)]
    pub note: String,
    pub created_at: String,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct Note {
    pub id: String,
    pub created_at: String,
    pub local_date: String,
    pub text: String,
    #[serde(default)]
    pub tags: Vec<String>,
    pub updated_at: String,
    /// 非空表示这是同步冲突副本，值为原记录 ID。
    #[serde(default)]
    pub conflict_of: Option<String>,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct DailyMood {
    pub id: String,
    pub local_date: String,
    /// 0–100。分数段名字与颜色由它派生，不单独存。
    pub score: i32,
    pub updated_at: String,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct Tombstone {
    pub entity_type: EntityType,
    pub entity_id: String,
    pub deleted_at: String,
    pub deleted_by: String,
}
