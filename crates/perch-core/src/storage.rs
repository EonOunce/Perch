//! `data.json` 的读写：原子写 + 滚动备份 + schemaVersion 检查（DESIGN §3.1、§4.7）。
//!
//! 路径由宿主传进来（core 不 `use tauri`、不碰注册表、不猜 `%LOCALAPPDATA%`），
//! 时间戳也一样：调用方给「此刻的毫秒 + UTC 偏移秒」，这里只做纯字符串拼名字。

use std::collections::HashSet;
use std::fs;
use std::io::Write as _;
use std::path::{Path, PathBuf};

use crate::date_boundary::{civil_from_days, format_instant, parse_date, MS_PER_DAY};
use crate::model::{Data, SCHEMA_VERSION};

/// 备份保留份数（§4.7）。
pub const BACKUP_KEEP: usize = 14;

#[derive(Debug, PartialEq)]
pub enum StorageError {
    /// 文件还不存在：首次启动，不是错误，由调用方决定建不建。
    NotFound,
    /// 读或写失败，`String` 是「在哪一步 + OS 报错」。
    Io(String),
    /// JSON 坏了或字段类型不对。调用方应把坏文件挪开再从备份恢复，别覆盖它。
    Parse(String),
    /// 文件版本比本应用新（§4.7）：只读，提示升级，绝不写回降级数据。
    SchemaTooNew { found: u64 },
}

impl StorageError {
    /// 给界面用的一句话，不带路径也不带正文。
    pub fn user_message(&self) -> String {
        match self {
            StorageError::NotFound => "还没有数据文件".to_string(),
            StorageError::Io(_) => "数据读写失败，请检查磁盘空间后重试".to_string(),
            StorageError::Parse(_) => "数据文件读不出来，已保留原文件等待恢复".to_string(),
            StorageError::SchemaTooNew { found } => {
                format!("这份数据由更新版本的栖迟写入（版本 {found}），请先升级应用")
            }
        }
    }
}

/// `<root>/data.json` 与 `<root>/backups/`。
#[derive(Debug, Clone)]
pub struct Store {
    root: PathBuf,
}

impl Store {
    pub fn new(root: impl Into<PathBuf>) -> Self {
        Store { root: root.into() }
    }

    pub fn data_path(&self) -> PathBuf {
        self.root.join("data.json")
    }

    pub fn backups_dir(&self) -> PathBuf {
        self.root.join("backups")
    }

    /// 读一份数据。文件不存在返回 `NotFound`；版本比本地新返回 `SchemaTooNew`。
    pub fn load(&self) -> Result<Data, StorageError> {
        read_at(&self.data_path())
    }

    /// 写一份数据：先把现有文件备份下来，再原子替换，最后清理多余备份。
    ///
    /// 备份失败就中止本次写入——没有恢复点的覆盖才是真正会丢数据的操作。
    /// 备份清理失败只影响目录里多留几个文件，不值得让新数据写不进去。
    pub fn save(&self, data: &Data, instant_ms: i64, offset_secs: i32) -> Result<(), StorageError> {
        let mut snapshot = data.clone();
        snapshot.schema_version = SCHEMA_VERSION;
        snapshot.generated_at = Some(format_instant(instant_ms, offset_secs));
        let json = dump(&snapshot)?;

        let path = self.data_path();
        self.backup(&path, instant_ms, offset_secs)?;
        write_atomically(&path, json.as_bytes())?;
        let _ = self.prune_backups();
        Ok(())
    }

    /// 现有文件还在就复制一份到 `backups/data-YYYYMMDD-HHMMSS.json`。
    fn backup(
        &self,
        path: &Path,
        instant_ms: i64,
        offset_secs: i32,
    ) -> Result<Option<PathBuf>, StorageError> {
        if fs::metadata(path).is_err() {
            return Ok(None);
        }
        let dir = self.backups_dir();
        fs::create_dir_all(&dir).map_err(|e| io_err("创建备份目录", &dir, e))?;
        let to = dir.join(format!(
            "data-{}.json",
            backup_stamp(instant_ms, offset_secs)
        ));
        fs::copy(path, &to).map_err(|e| io_err("写入备份", &to, e))?;
        Ok(Some(to))
    }

    /// 只保留最近 `BACKUP_KEEP` 份，返回删掉了几个。
    pub fn prune_backups(&self) -> Result<usize, StorageError> {
        let files = self.backup_files()?;
        let mut removed = 0;
        for path in files.iter().skip(BACKUP_KEEP) {
            if fs::remove_file(path).is_ok() {
                removed += 1;
            }
        }
        Ok(removed)
    }

    /// 最新一份备份，读坏 `data.json` 之后拿它恢复。
    pub fn latest_backup(&self) -> Option<PathBuf> {
        self.backup_files().ok()?.into_iter().next()
    }

    /// 按文件名从新到旧。文件名里的时间戳是定宽零填充，所以字典序就是时间序。
    pub fn backup_files(&self) -> Result<Vec<PathBuf>, StorageError> {
        let dir = self.backups_dir();
        let mut files: Vec<PathBuf> = Vec::new();
        let entries = match fs::read_dir(&dir) {
            Ok(entries) => entries,
            Err(e) if e.kind() == std::io::ErrorKind::NotFound => return Ok(files),
            Err(e) => return Err(io_err("列出备份", &dir, e)),
        };
        for entry in entries.flatten() {
            let path = entry.path();
            let name = path.file_name().and_then(|n| n.to_str()).unwrap_or("");
            if name.starts_with("data-") && name.ends_with(".json") {
                files.push(path);
            }
        }
        files.sort_by(|a, b| b.file_name().cmp(&a.file_name()));
        Ok(files)
    }
}

fn read_at(path: &Path) -> Result<Data, StorageError> {
    let text = match fs::read_to_string(path) {
        Ok(text) => text,
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => return Err(StorageError::NotFound),
        Err(e) => return Err(io_err("读取数据", path, e)),
    };
    // 先看版本再看正文：版本不认识时报「请升级」，而不是某个字段解析失败的假错。
    let value: serde_json::Value =
        serde_json::from_str(&text).map_err(|e| StorageError::Parse(format!("{path:?}: {e}")))?;
    let found = value
        .get("schemaVersion")
        .and_then(|v| v.as_u64())
        .unwrap_or(0);
    if found > SCHEMA_VERSION as u64 {
        return Err(StorageError::SchemaTooNew { found });
    }
    // 比本地旧的版本按当前字段读（V1 还没有迁移分支，将来在 model.rs 旁边加）。
    serde_json::from_value(value).map_err(|e| StorageError::Parse(format!("{path:?}: {e}")))
}

/// 读一份外来快照：导出的 JSON 或 `backups/` 里的备份，恢复前先过这里。
///
/// 比 [`Store::load`] 严一档：`schemaVersion` 必须存在、至少有一个数据数组，
/// 免得一份随便的 `{}` 被当成「空白快照」把本机数据清掉。
pub fn parse_snapshot(text: &str, origin: &str) -> Result<Data, StorageError> {
    let value: serde_json::Value =
        serde_json::from_str(text).map_err(|e| parse_err(origin, e.to_string()))?;
    let Some(object) = value.as_object() else {
        return Err(parse_err(origin, "顶层不是 JSON 对象"));
    };
    let Some(found) = object.get("schemaVersion").and_then(|v| v.as_u64()) else {
        return Err(parse_err(origin, "缺少 schemaVersion"));
    };
    if found > SCHEMA_VERSION as u64 {
        return Err(StorageError::SchemaTooNew { found });
    }
    const COLLECTIONS: [&str; 6] = [
        "habits",
        "markers",
        "checkinEvents",
        "notes",
        "dailyMoods",
        "tombstones",
    ];
    if !COLLECTIONS.iter().any(|key| object.contains_key(*key)) {
        return Err(parse_err(origin, "没有任何数据数组"));
    }
    let data: Data = serde_json::from_value(value).map_err(|e| parse_err(origin, e.to_string()))?;
    validate_snapshot(&data, origin)?;
    Ok(data)
}

fn parse_err(origin: &str, detail: impl std::fmt::Display) -> StorageError {
    StorageError::Parse(format!("{origin}: {detail}"))
}

/// 恢复前的结构检查。只挡会让界面或统计算不下去的东西，不替用户判断内容对错。
fn validate_snapshot(data: &Data, origin: &str) -> Result<(), StorageError> {
    fn check_ids<'a>(
        ids: impl Iterator<Item = &'a str>,
        what: &str,
        origin: &str,
    ) -> Result<(), StorageError> {
        let mut seen = HashSet::new();
        for id in ids {
            if id.trim().is_empty() {
                return Err(parse_err(origin, format!("{what}里有一条没有 id")));
            }
            if !seen.insert(id) {
                return Err(parse_err(origin, format!("{what}里有重复的 id {id}")));
            }
        }
        Ok(())
    }

    fn check_days<'a>(
        days: impl Iterator<Item = &'a str>,
        what: &str,
        origin: &str,
    ) -> Result<(), StorageError> {
        for day in days {
            if parse_date(day).is_none() {
                return Err(parse_err(origin, format!("{what}里有认不出的日期 {day:?}")));
            }
        }
        Ok(())
    }

    check_ids(data.habits.iter().map(|h| h.id.as_str()), "习惯", origin)?;
    check_ids(data.markers.iter().map(|m| m.id.as_str()), "标记", origin)?;
    check_ids(
        data.checkin_events.iter().map(|e| e.id.as_str()),
        "打卡",
        origin,
    )?;
    check_ids(data.notes.iter().map(|n| n.id.as_str()), "记录", origin)?;
    check_ids(
        data.daily_moods.iter().map(|m| m.id.as_str()),
        "心情",
        origin,
    )?;

    check_days(
        data.markers.iter().map(|m| m.local_date.as_str()),
        "标记",
        origin,
    )?;
    check_days(
        data.checkin_events.iter().map(|e| e.local_date.as_str()),
        "打卡",
        origin,
    )?;
    check_days(
        data.notes.iter().map(|n| n.local_date.as_str()),
        "记录",
        origin,
    )?;
    check_days(
        data.daily_moods.iter().map(|m| m.local_date.as_str()),
        "心情",
        origin,
    )?;

    for habit in &data.habits {
        if habit.name.trim().is_empty() {
            return Err(parse_err(origin, format!("习惯 {} 没有名字", habit.id)));
        }
        if habit.target_count < 1 {
            return Err(parse_err(
                origin,
                format!("习惯「{}」的目标次数小于 1", habit.name),
            ));
        }
        if let Some(target) = habit.weekly_target {
            if target < 1 {
                return Err(parse_err(
                    origin,
                    format!("习惯「{}」的每周目标小于 1", habit.name),
                ));
            }
        }
        for goal in &habit.goal_history {
            if parse_date(&goal.effective_from).is_none() {
                return Err(parse_err(
                    origin,
                    format!("习惯「{}」的目标生效日认不出", habit.name),
                ));
            }
        }
    }
    for mood in &data.daily_moods {
        if !(0..=100).contains(&mood.score) {
            return Err(parse_err(
                origin,
                format!("心情分数 {} 不在 0–100 之间", mood.score),
            ));
        }
    }
    Ok(())
}

/// 同目录临时文件 → 落盘 → rename 覆盖。中途失败时原文件一个字节都没被动过。
fn write_atomically(path: &Path, bytes: &[u8]) -> Result<(), StorageError> {
    let dir = path
        .parent()
        .ok_or_else(|| StorageError::Io("数据文件路径没有父目录，无法安全写入".to_string()))?;
    fs::create_dir_all(dir).map_err(|e| io_err("创建数据目录", dir, e))?;
    let temp = dir.join(format!("{}.{}.tmp", file_name(path), std::process::id()));
    let result = (|| {
        let mut file = fs::File::create(&temp).map_err(|e| io_err("创建临时文件", &temp, e))?;
        file.write_all(bytes)
            .map_err(|e| io_err("写临时文件", &temp, e))?;
        // 不 sync 的 rename 在断电后可能留下空文件；这里宁可慢一点。
        file.sync_all()
            .map_err(|e| io_err("落盘临时文件", &temp, e))?;
        // Windows 上句柄还开着就 rename 会失败。
        drop(file);
        fs::rename(&temp, path).map_err(|e| io_err("替换数据文件", path, e))
    })();
    if result.is_err() {
        let _ = fs::remove_file(&temp);
    }
    result
}

fn dump(data: &Data) -> Result<String, StorageError> {
    serde_json::to_string_pretty(data)
        .map(|s| format!("{s}\n"))
        .map_err(|e| StorageError::Io(format!("序列化失败: {e}")))
}

fn file_name(path: &Path) -> String {
    path.file_name()
        .and_then(|n| n.to_str())
        .unwrap_or("data.json")
        .to_string()
}

fn io_err(step: &str, path: &Path, e: std::io::Error) -> StorageError {
    StorageError::Io(format!("{step} {path:?}: {e}"))
}

/// `20261026-040000-123`。不带冒号和加号——那两个在 Windows 文件名里不能出现。
/// 带毫秒是因为一秒内连存两次时备份名撞车会悄悄吃掉一份历史。
pub fn backup_stamp(instant_ms: i64, offset_secs: i32) -> String {
    let local = instant_ms + offset_secs as i64 * 1000;
    let days = local.div_euclid(MS_PER_DAY);
    let rest = local.rem_euclid(MS_PER_DAY);
    let (y, m, d) = civil_from_days(days);
    format!(
        "{y:04}{m:02}{d:02}-{:02}{:02}{:02}-{:03}",
        rest / 3_600_000,
        rest / 60_000 % 60,
        rest / 1000 % 60,
        rest % 1000
    )
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::model::{CheckinEvent, CheckinStatus, Habit};
    use std::sync::atomic::{AtomicUsize, Ordering};

    static SEQ: AtomicUsize = AtomicUsize::new(0);

    /// 每个测试一个独立目录，跑完删掉。不引 tempfile：一个 `env::temp_dir` 够用。
    fn scratch(tag: &str) -> PathBuf {
        let dir = std::env::temp_dir().join(format!(
            "perch-store-{}-{}-{tag}",
            std::process::id(),
            SEQ.fetch_add(1, Ordering::Relaxed)
        ));
        let _ = fs::remove_dir_all(&dir);
        fs::create_dir_all(&dir).expect("建临时目录");
        dir
    }

    fn habit(id: &str) -> Habit {
        Habit {
            id: id.to_string(),
            name: "阅读".to_string(),
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

    fn sample(device: &str) -> Data {
        let mut data = Data::empty(device);
        data.habits = vec![habit("h1")];
        data.checkin_events = vec![CheckinEvent {
            id: "e1".to_string(),
            habit_id: "h1".to_string(),
            local_date: "2026-10-20".to_string(),
            status: CheckinStatus::Done,
            confirmed_at: None,
            source: "manual".to_string(),
            supersedes_id: None,
            recorded_by: "pc".to_string(),
            note: "晚上读了 20 分钟".to_string(),
            created_at: "2026-10-20T21:00:00.000+08:00".to_string(),
        }];
        data
    }

    const NOW: i64 = 1_792_958_400_000; // 2026-10-26T04:00:00.000+08:00
    const CST: i32 = 8 * 3600;

    #[test]
    fn missing_file_is_not_an_error_state() {
        let dir = scratch("missing");
        let store = Store::new(&dir);
        assert_eq!(store.load(), Err(StorageError::NotFound));
        assert_eq!(store.latest_backup(), None);
        fs::remove_dir_all(&dir).unwrap();
    }

    #[test]
    fn round_trip_keeps_every_field() {
        let dir = scratch("roundtrip");
        let store = Store::new(&dir);
        let data = sample("pc-a");
        store.save(&data, NOW, CST).unwrap();
        let back = store.load().unwrap();
        assert_eq!(back.habits, data.habits);
        assert_eq!(back.checkin_events, data.checkin_events);
        assert_eq!(back.device_id, "pc-a");
        assert_eq!(
            back.generated_at.as_deref(),
            Some(format_instant(NOW, CST).as_str())
        );
        fs::remove_dir_all(&dir).unwrap();
    }

    #[test]
    fn aborted_write_leaves_the_original_intact() {
        let dir = scratch("aborted");
        let store = Store::new(&dir);
        store.save(&sample("pc-a"), NOW, CST).unwrap();
        let before = fs::read(store.data_path()).unwrap();

        // 把备份目录换成一个同名文件：备份这一步必然失败，写入应该就地中止。
        let _ = fs::remove_dir_all(store.backups_dir());
        fs::write(store.backups_dir(), "").unwrap();
        let err = store
            .save(&sample("pc-b"), NOW + 60_000, CST)
            .expect_err("备份写不进去时不该继续覆盖");
        assert!(matches!(err, StorageError::Io(_)));
        assert_eq!(
            fs::read(store.data_path()).unwrap(),
            before,
            "原文件一个字节都没动"
        );
        let leftovers: Vec<String> = fs::read_dir(&dir)
            .unwrap()
            .flatten()
            .map(|e| e.file_name().to_string_lossy().into_owned())
            .filter(|n| n.ends_with(".tmp"))
            .collect();
        assert!(leftovers.is_empty(), "失败后不留临时文件: {leftovers:?}");
        fs::remove_dir_all(&dir).unwrap();
    }

    #[test]
    fn each_save_backs_up_the_previous_file() {
        let dir = scratch("backup");
        let store = Store::new(&dir);
        store.save(&sample("第一版"), NOW, CST).unwrap();
        store.save(&sample("第二版"), NOW + 1_000, CST).unwrap();
        let backups = store.backup_files().unwrap();
        assert_eq!(backups.len(), 1, "首次写入没有东西可备份");
        // 备份里存的还是上一版正文，不是刚写进去的新版。
        let old: serde_json::Value =
            serde_json::from_str(&fs::read_to_string(&backups[0]).unwrap()).unwrap();
        assert_eq!(old["deviceId"], "第一版");
        assert_eq!(store.load().unwrap().device_id, "第二版");
        fs::remove_dir_all(&dir).unwrap();
    }

    #[test]
    fn backups_prune_to_the_newest_fourteen() {
        let dir = scratch("prune");
        let store = Store::new(&dir);
        let backups = store.backups_dir();
        fs::create_dir_all(&backups).unwrap();
        for i in 0..20i64 {
            let name = format!("data-{}.json", backup_stamp(NOW + i * 1000, CST));
            fs::write(backups.join(name), "{}").unwrap();
        }
        // 再加一份不是本应用写的文件，确认不会被顺手删掉。
        fs::write(backups.join("keep-me.json"), "{}").unwrap();
        assert_eq!(store.prune_backups().unwrap(), 6);
        let left = store.backup_files().unwrap();
        assert_eq!(left.len(), BACKUP_KEEP);
        assert!(backups.join("keep-me.json").exists());
        assert_eq!(
            left[0].file_name().unwrap().to_string_lossy(),
            format!("data-{}.json", backup_stamp(NOW + 19_000, CST)),
            "最新的排在第一个"
        );
        assert_eq!(store.latest_backup().unwrap(), left[0]);
        fs::remove_dir_all(&dir).unwrap();
    }

    #[test]
    fn newer_schema_is_refused_before_touching_the_body() {
        let dir = scratch("schema");
        fs::write(
            dir.join("data.json"),
            r#"{"schemaVersion": 99, "deviceId": "phone", "habits": [{"unknownFutureField": 1}]}"#,
        )
        .unwrap();
        let store = Store::new(&dir);
        assert_eq!(
            store.load(),
            Err(StorageError::SchemaTooNew { found: 99 }),
            "缺字段也不能把它当成 0 版读"
        );
        assert!(store.load().unwrap_err().user_message().contains("升级"));
        fs::remove_dir_all(&dir).unwrap();
    }

    #[test]
    fn older_schema_reads_with_current_defaults() {
        let dir = scratch("older");
        fs::write(
            dir.join("data.json"),
            r#"{"schemaVersion": 0, "deviceId": "phone"}"#,
        )
        .unwrap();
        let store = Store::new(&dir);
        let data = store.load().unwrap();
        assert_eq!(data.device_id, "phone");
        assert!(data.habits.is_empty());
        fs::remove_dir_all(&dir).unwrap();
    }

    #[test]
    fn corrupt_json_reports_parse_not_notfound() {
        let dir = scratch("corrupt");
        fs::write(dir.join("data.json"), b"{ not json").unwrap();
        let store = Store::new(&dir);
        assert!(matches!(store.load(), Err(StorageError::Parse(_))));
        fs::remove_dir_all(&dir).unwrap();
    }

    #[test]
    fn stamps_are_windows_legal_and_ordered() {
        // 先把测试用的瞬间钉死，免得常量写错还一路「全绿」。
        assert_eq!(format_instant(NOW, CST), "2026-10-26T04:00:00.000+08:00");
        assert_eq!(backup_stamp(NOW, CST), "20261026-040000-000");
        // 同一瞬间换时区只是换个钟点，日期也会跟着挪一天。
        assert_eq!(backup_stamp(NOW, -5 * 3600), "20261025-150000-000");
        assert!(backup_stamp(NOW, CST)
            .chars()
            .all(|c| c.is_ascii_digit() || c == '-'));
        assert!(backup_stamp(NOW, CST) < backup_stamp(NOW + 1, CST));
    }

    fn json_of(data: &Data) -> String {
        serde_json::to_string(data).unwrap()
    }

    #[test]
    fn a_full_snapshot_round_trips_through_the_import_parser() {
        let mut data = sample("pc-a");
        data.notes.push(crate::model::Note {
            id: "n1".to_string(),
            created_at: "2026-10-20T21:00:00.000+08:00".to_string(),
            local_date: "2026-10-20".to_string(),
            text: "一句".to_string(),
            tags: vec!["书".to_string()],
            updated_at: "2026-10-20T21:00:00.000+08:00".to_string(),
            conflict_of: None,
        });
        data.daily_moods.push(crate::model::DailyMood {
            id: "m1".to_string(),
            local_date: "2026-10-20".to_string(),
            score: 60,
            updated_at: "2026-10-20T21:00:00.000+08:00".to_string(),
        });
        let parsed = parse_snapshot(&json_of(&data), "导出文件").unwrap();
        assert_eq!(parsed.habits, data.habits);
        assert_eq!(parsed.notes, data.notes);
        assert_eq!(parsed.daily_moods, data.daily_moods);
    }

    #[test]
    fn a_snapshot_from_the_future_is_refused() {
        let mut value: serde_json::Value = serde_json::from_str(&json_of(&sample("pc-a"))).unwrap();
        value["schemaVersion"] = serde_json::json!(99);
        assert_eq!(
            parse_snapshot(&value.to_string(), "导出文件"),
            Err(StorageError::SchemaTooNew { found: 99 })
        );
    }

    #[test]
    fn arbitrary_json_is_not_a_snapshot() {
        // 一份空的 `{}` 如果被当成快照，恢复就成了「清空本机数据」。
        for text in [
            "{}",
            r#"{"schemaVersion":1}"#,
            r#"{"habits":[],"notes":[]}"#,
            r#"[{"schemaVersion":1}]"#,
            "不是 JSON",
        ] {
            assert!(
                matches!(
                    parse_snapshot(text, "别的文件"),
                    Err(StorageError::Parse(_))
                ),
                "{text} 不该被当成栖迟的快照"
            );
        }
    }

    #[test]
    fn an_empty_but_complete_snapshot_is_still_valid() {
        let data = Data::empty("pc-a");
        let parsed = parse_snapshot(&json_of(&data), "导出文件").unwrap();
        assert!(parsed.habits.is_empty());
        assert!(parsed.notes.is_empty());
    }

    #[test]
    fn snapshots_with_impossible_values_are_refused() {
        let mut duplicate = sample("pc-a");
        duplicate.habits.push(duplicate.habits[0].clone());
        assert!(matches!(
            parse_snapshot(&json_of(&duplicate), "导出文件"),
            Err(StorageError::Parse(_))
        ));

        let mut mood = sample("pc-a");
        mood.daily_moods.push(crate::model::DailyMood {
            id: "m1".to_string(),
            local_date: "2026-10-20".to_string(),
            score: 101,
            updated_at: "2026-10-20T21:00:00.000+08:00".to_string(),
        });
        assert!(matches!(
            parse_snapshot(&json_of(&mood), "导出文件"),
            Err(StorageError::Parse(_))
        ));

        let mut bad_day = sample("pc-a");
        bad_day.checkin_events[0].local_date = "昨天".to_string();
        assert!(matches!(
            parse_snapshot(&json_of(&bad_day), "导出文件"),
            Err(StorageError::Parse(_))
        ));

        let mut no_name = sample("pc-a");
        no_name.habits[0].name = "  ".to_string();
        assert!(matches!(
            parse_snapshot(&json_of(&no_name), "导出文件"),
            Err(StorageError::Parse(_))
        ));

        let mut zero_target = sample("pc-a");
        zero_target.habits[0].weekly_target = Some(0);
        assert!(matches!(
            parse_snapshot(&json_of(&zero_target), "导出文件"),
            Err(StorageError::Parse(_))
        ));
    }

    #[test]
    fn an_older_snapshot_still_imports() {
        // 旧版本写出的快照缺新字段时按当前默认值读，与 data.json 的规矩一致。
        let mut value: serde_json::Value = serde_json::from_str(&json_of(&sample("pc-a"))).unwrap();
        value["schemaVersion"] = serde_json::json!(0);
        let parsed = parse_snapshot(&value.to_string(), "导出文件").unwrap();
        assert_eq!(parsed.habits.len(), 1);
    }
}
