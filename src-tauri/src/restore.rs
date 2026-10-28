//! 从备份或导出的 JSON 恢复（DESIGN §4.7）。
//!
//! 分两步走：选中的快照先解析、给出摘要，并暂存在这里；用户在界面上再确认一次，
//! 才替换 `data.json`（替换前 `Store::save` 会先把现在这份留成备份）。
//! 路径只来自 `backups/` 列表或系统文件框，前端递不进任意路径，所以这里不再挡 `..`。

use std::fs;
use std::path::Path;
use std::sync::Mutex;

use perch_core::habits::{is_archived, is_removed, is_tombstoned};
use perch_core::model::EntityType;
use perch_core::{storage, Data, StorageError};
use serde::Serialize;
use tauri::Manager;

use crate::probe_log;
use crate::store::SharedState;

/// 导出文件通常不到 1 MB；超过这个就不像栖迟写的，别把整份读进内存。
const MAX_SNAPSHOT_BYTES: u64 = 8 * 1024 * 1024;

/// 已经解析、等待用户二次确认的那一份。
#[derive(Default)]
pub struct RestoreStage {
    inner: Mutex<Option<Staged>>,
}

struct Staged {
    origin: String,
    data: Data,
}

impl RestoreStage {
    fn put(&self, staged: Staged) {
        *self
            .inner
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner()) = Some(staged);
    }

    fn take(&self) -> Option<Staged> {
        self.inner
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner())
            .take()
    }

    fn clear(&self) {
        *self
            .inner
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner()) = None;
    }
}

#[derive(Serialize)]
#[serde(rename_all = "camelCase")]
pub struct BackupEntry {
    pub name: String,
    /// `2026-10-28 18:06:14`，从备份文件名里的本机时间读出来。
    pub label: String,
    pub bytes: u64,
    pub habits: usize,
    pub notes: usize,
    /// 读不出来也照样列出来；真正的错误在点选后说。
    pub readable: bool,
}

#[derive(Serialize)]
#[serde(rename_all = "camelCase")]
pub struct RestorePreview {
    pub origin: String,
    /// `backup` 或 `file`，界面据此说「这份备份」还是「这个文件」。
    pub source_kind: String,
    pub generated_at: Option<String>,
    pub schema_version: u32,
    pub habits: usize,
    pub archived_habits: usize,
    pub notes: usize,
    pub checkins: usize,
    pub moods: usize,
    pub markers: usize,
    pub tombstones: usize,
    pub first_day: Option<String>,
    pub last_day: Option<String>,
}

#[derive(Serialize)]
#[serde(rename_all = "camelCase")]
pub struct RestoreApplied {
    pub origin: String,
    pub habits: usize,
    pub notes: usize,
}

/// 备份文件列表，从新到旧。
#[tauri::command]
pub fn restore_backups(state: tauri::State<'_, SharedState>) -> Result<Vec<BackupEntry>, String> {
    let files = state.with(|perch| perch.backups())?;
    Ok(files.iter().map(|path| backup_entry(path)).collect())
}

/// 选一份本机备份，解析并暂存；真正替换要再调 `restore_apply`。
#[tauri::command]
pub fn restore_preview_backup(
    name: String,
    state: tauri::State<'_, SharedState>,
    stage: tauri::State<'_, RestoreStage>,
) -> Result<RestorePreview, String> {
    stage.clear();
    let files = state.with(|perch| perch.backups())?;
    let path = files
        .into_iter()
        .find(|path| path.file_name().and_then(|name| name.to_str()) == Some(name.as_str()))
        .ok_or_else(|| "找不到这份备份，可能已经被清理了。".to_string())?;
    let data = read_snapshot(&path)?;
    let preview = preview(name.clone(), "backup", &data);
    stage.put(Staged { origin: name, data });
    Ok(preview)
}

/// 打开系统文件框选一份导出的 JSON；取消返回 `None`。
#[tauri::command]
pub async fn restore_pick_file(
    app: tauri::AppHandle,
    stage: tauri::State<'_, RestoreStage>,
) -> Result<Option<RestorePreview>, String> {
    stage.clear();
    let owner = app
        .get_webview_window("main")
        .and_then(|win| win.hwnd().ok())
        .map(|handle| handle.0 as isize);
    let start = crate::export::folder().ok();
    let picked = tauri::async_runtime::spawn_blocking(move || {
        crate::pick::open_json(owner, start.as_deref())
    })
    .await
    .map_err(|err| format!("打开文件选择框失败：{err}"))?;
    let Some(path) = picked else {
        return Ok(None);
    };
    let data = read_snapshot(&path)?;
    let origin = path
        .file_name()
        .and_then(|name| name.to_str())
        .unwrap_or("选中的文件")
        .to_string();
    let preview = preview(origin.clone(), "file", &data);
    stage.put(Staged { origin, data });
    Ok(Some(preview))
}

/// 用暂存的那份替换本机数据。调用前必须已经过界面的二次确认。
#[tauri::command]
pub fn restore_apply(
    app: tauri::AppHandle,
    state: tauri::State<'_, SharedState>,
    stage: tauri::State<'_, RestoreStage>,
) -> Result<RestoreApplied, String> {
    let Some(staged) = stage.take() else {
        return Err("先选一份备份或文件，再点恢复。".to_string());
    };
    let applied = RestoreApplied {
        origin: staged.origin.clone(),
        habits: alive_habits(&staged.data),
        notes: alive_notes(&staged.data),
    };
    let data = staged.data.clone();
    // 恢复会先备份现在这份 data.json（store::Perch::restore），所以这一步也能回头。
    match state.with(|perch| perch.restore(data)) {
        Ok(()) => {
            // 旧 ETag/基线描述的是被替换的那份数据，不能留到下一轮同步（§4.4）。
            if let Some(syncer) = app.try_state::<crate::sync::Syncer>() {
                syncer.forget_baseline_after_restore();
            }
            probe_log::note(format!(
                "[restore] 已用 {} 替换本机数据（{} 个习惯，{} 条记录）",
                applied.origin, applied.habits, applied.notes
            ));
            Ok(applied)
        }
        Err(err) => {
            stage.put(staged);
            Err(err)
        }
    }
}

/// 关掉面板时丢掉暂存，下次打开不会拿旧选择直接恢复。
#[tauri::command]
pub fn restore_cancel(stage: tauri::State<'_, RestoreStage>) {
    stage.clear();
}

fn backup_entry(path: &Path) -> BackupEntry {
    let name = path
        .file_name()
        .and_then(|name| name.to_str())
        .unwrap_or("备份")
        .to_string();
    let bytes = fs::metadata(path).map(|meta| meta.len()).unwrap_or(0);
    match read_snapshot(path) {
        Ok(data) => BackupEntry {
            label: backup_label(&name),
            habits: alive_habits(&data),
            notes: alive_notes(&data),
            readable: true,
            name,
            bytes,
        },
        Err(_) => BackupEntry {
            label: backup_label(&name),
            habits: 0,
            notes: 0,
            readable: false,
            name,
            bytes,
        },
    }
}

fn read_snapshot(path: &Path) -> Result<Data, String> {
    let origin = path
        .file_name()
        .and_then(|name| name.to_str())
        .unwrap_or("这份文件");
    let meta = fs::metadata(path).map_err(|_| "读不了这份文件，可能已经被移走。".to_string())?;
    if meta.len() > MAX_SNAPSHOT_BYTES {
        return Err("这份文件太大了，不像栖迟导出的数据。".to_string());
    }
    let text = fs::read_to_string(path).map_err(|err| match err.kind() {
        std::io::ErrorKind::InvalidData => "这份文件不是 UTF-8 文本，读不出来。".to_string(),
        _ => "读不了这份文件，可能已经被移走。".to_string(),
    })?;
    storage::parse_snapshot(&text, origin).map_err(|err| snapshot_message(err, origin))
}

fn snapshot_message(err: StorageError, origin: &str) -> String {
    if matches!(&err, StorageError::SchemaTooNew { .. }) {
        return err.user_message();
    }
    probe_log::note(format!("[restore] {origin}: {err:?}"));
    "这份文件不像栖迟导出的数据，没有改动本机任何东西。".to_string()
}

fn preview(origin: String, source_kind: &str, data: &Data) -> RestorePreview {
    let mut days: Vec<&str> = Vec::new();
    for note in &data.notes {
        if !is_tombstoned(data, EntityType::Note, &note.id) {
            days.push(&note.local_date);
        }
    }
    for event in &data.checkin_events {
        if !is_tombstoned(data, EntityType::CheckinEvent, &event.id) {
            days.push(&event.local_date);
        }
    }
    for mood in &data.daily_moods {
        if !is_tombstoned(data, EntityType::DailyMood, &mood.id) {
            days.push(&mood.local_date);
        }
    }
    for marker in &data.markers {
        if !is_tombstoned(data, EntityType::Marker, &marker.id) {
            days.push(&marker.local_date);
        }
    }

    RestorePreview {
        origin,
        source_kind: source_kind.to_string(),
        generated_at: data.generated_at.clone(),
        schema_version: data.schema_version,
        habits: alive_habits(data),
        archived_habits: data
            .habits
            .iter()
            .filter(|habit| !is_removed(data, habit) && is_archived(habit))
            .count(),
        notes: alive_notes(data),
        checkins: data
            .checkin_events
            .iter()
            .filter(|event| !is_tombstoned(data, EntityType::CheckinEvent, &event.id))
            .count(),
        moods: data
            .daily_moods
            .iter()
            .filter(|mood| !is_tombstoned(data, EntityType::DailyMood, &mood.id))
            .count(),
        markers: data
            .markers
            .iter()
            .filter(|marker| !is_tombstoned(data, EntityType::Marker, &marker.id))
            .count(),
        tombstones: data.tombstones.len(),
        first_day: days.iter().min().map(|day| day.to_string()),
        last_day: days.iter().max().map(|day| day.to_string()),
    }
}

fn alive_habits(data: &Data) -> usize {
    data.habits
        .iter()
        .filter(|habit| !is_removed(data, habit))
        .count()
}

fn alive_notes(data: &Data) -> usize {
    data.notes
        .iter()
        .filter(|note| !is_tombstoned(data, EntityType::Note, &note.id))
        .count()
}

/// `data-20261028-180614-123.json` → `2026-10-28 18:06:14`。
/// 对不上这个格式就原样显示文件名，不编一个日期。
fn backup_label(name: &str) -> String {
    let raw = name
        .strip_prefix("data-")
        .and_then(|rest| rest.strip_suffix(".json"))
        .unwrap_or(name);
    let parts: Vec<&str> = raw.split('-').collect();
    let digits = parts
        .iter()
        .all(|part| !part.is_empty() && part.bytes().all(|byte| byte.is_ascii_digit()));
    if parts.len() == 3 && parts[0].len() == 8 && parts[1].len() == 6 && digits {
        format!(
            "{}-{}-{} {}:{}:{}",
            &parts[0][0..4],
            &parts[0][4..6],
            &parts[0][6..8],
            &parts[1][0..2],
            &parts[1][2..4],
            &parts[1][4..6]
        )
    } else {
        name.to_string()
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use perch_core::edit::{self, NewHabit};
    use perch_core::{Data, Now};

    const NOW: Now = Now {
        instant_ms: 1_792_958_400_000,
        offset_secs: 8 * 3600,
    };

    fn sample() -> Data {
        let mut data = Data::empty("pc");
        edit::add_habit(
            &mut data,
            "h1",
            NewHabit {
                name: "阅读".to_string(),
                ..NewHabit::default()
            },
            NOW,
        )
        .unwrap();
        edit::add_habit(
            &mut data,
            "h2",
            NewHabit {
                name: "散步".to_string(),
                ..NewHabit::default()
            },
            NOW,
        )
        .unwrap();
        edit::set_archived(&mut data, "h2", true, NOW).unwrap();
        edit::add_note(&mut data, "n1", "留下的一句", NOW).unwrap();
        edit::add_note(&mut data, "n2", "删掉的一句", NOW).unwrap();
        edit::tombstone(&mut data, EntityType::Note, "n2", "pc", NOW).unwrap();
        edit::set_mood(&mut data, "m1", "2026-10-20", 60, NOW).unwrap();
        edit::add_marker(&mut data, "k1", "h1", "2026-10-26", "换回纸质书", NOW).unwrap();
        data
    }

    #[test]
    fn the_preview_counts_only_what_is_still_alive() {
        let view = preview("备份".to_string(), "backup", &sample());
        assert_eq!(view.habits, 2, "归档的习惯仍算在这份数据里");
        assert_eq!(view.archived_habits, 1);
        assert_eq!(view.notes, 1, "墓碑记录不该算进恢复后的记录数");
        assert_eq!(view.moods, 1);
        assert_eq!(view.markers, 1);
        assert_eq!(view.tombstones, 1);
        assert_eq!(view.first_day.as_deref(), Some("2026-10-20"));
        assert_eq!(view.last_day.as_deref(), Some("2026-10-26"));
        assert_eq!(view.schema_version, perch_core::SCHEMA_VERSION);
    }

    #[test]
    fn backup_names_become_readable_local_stamps() {
        assert_eq!(
            backup_label("data-20261028-180614-123.json"),
            "2026-10-28 18:06:14"
        );
        assert_eq!(backup_label("keep-me.json"), "keep-me.json");
        assert_eq!(backup_label("data-坏.json"), "data-坏.json");
    }

    #[test]
    fn a_missing_file_is_reported_without_touching_anything() {
        let missing = std::env::temp_dir().join("perch-restore-not-there.json");
        let _ = fs::remove_file(&missing);
        let err = read_snapshot(&missing).unwrap_err();
        assert!(err.contains("读不了"), "实际收到：{err}");
    }

    #[test]
    fn a_foreign_json_file_is_refused_with_a_neutral_message() {
        let path =
            std::env::temp_dir().join(format!("perch-restore-foreign-{}.json", std::process::id()));
        fs::write(&path, r#"{"hello":1}"#).unwrap();
        let err = read_snapshot(&path).unwrap_err();
        assert!(err.contains("不像栖迟"), "实际收到：{err}");
        let _ = fs::remove_file(&path);
    }
}
