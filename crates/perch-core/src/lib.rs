//! 栖迟的口径与数据层。
//!
//! 这里只放「两端必须算出同一个数」的部分：日界、目标生效、事件有效性、原子存储。
//! 系统能力（路径、DPAPI、窗口、通知、时钟）全部由宿主传参数进来，所以
//! 本 crate 永不 `use tauri`（DESIGN §5.4），将来抽 UniFFI 给 Android 复用时不用翻口径。
//!
//! - [`model`]：可同步 JSON 结构与 `schemaVersion`
//! - [`date_boundary`]：04:00 日界与日期算术
//! - [`goal`]：`goalHistory` 生效规则
//! - [`habits`]：墓碑 / 归档可见性与「一天一个有效事件」
//! - [`edit`]：写入侧口径（追加事件、墓碑、标签解析）
//! - [`records`]：记录时间线的分组、搜索与标签筛选
//! - [`stats`]：范围、完成率、热力图档位、记录量与每周文字回顾（DESIGN §6）
//! - [`merge`]：同步合并（LWW、墓碑、冲突副本），DESIGN §4.5
//! - [`storage`]：`data.json` 原子写与滚动备份

pub mod date_boundary;
pub mod edit;
pub mod goal;
pub mod habits;
pub mod merge;
pub mod model;
pub mod records;
pub mod stats;
pub mod storage;

pub use edit::{add_habit, set_checkin, EditError, NewHabit, Now};
pub use habits::{counts, effective_events, status_on, DayCounts, DayStatus};
pub use merge::{fingerprint, merge, unknown_schema};
pub use model::{Data, SCHEMA_VERSION};
pub use stats::{resolve_range, stats, Range, Stats};
pub use storage::{StorageError, Store};
