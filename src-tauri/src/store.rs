//! 数据的落点与「内存不领先于磁盘」这条规矩。
//!
//! `data.json` 怎么原子写、怎么留备份、版本不认识时怎么拒绝，全在 perch-core 的 `Store` 里。
//! 这一层只做三件事：找到 `%LOCALAPPDATA%\Perch`、记住本机 deviceId、
//! 以及把「改内存」和「落盘」绑成一次动作——写失败时界面上看到的仍然是磁盘上的那份。

use std::fs;
use std::path::{Path, PathBuf};
use std::sync::Mutex;

use perch_core::{Data, EditError, Store};
use serde::{Deserialize, Serialize};

use crate::clock;

/// 应用专用目录名（DESIGN §3.1、§5.1 已定）。
const APP_DIR: &str = "Perch";
const SETTINGS_FILE: &str = "settings.json";

/// 设备本地设置。DESIGN §3.4 列的那些（窗口位置、折叠状态、最后标签）还没做，
/// 先把只有 deviceId 这一件事写实：不同步、不进日志。
#[derive(Debug, Default, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
struct Settings {
    device_id: String,
}

/// 一份运行中的数据 + 它的来源。
pub struct Perch {
    store: Store,
    data: Data,
    device_id: String,
    /// 有些情况下文件不该被覆盖：文件坏了、或它由更新的版本写过。
    /// 非空时界面上显示这句话，所有写入一律拒绝——只读比覆盖一个还能救的文件安全。
    read_only: Option<String>,
}

impl Perch {
    /// 打开（或首次创建）数据。这一步不会写 `data.json`：空状态没必要先占一个文件。
    pub fn open() -> Result<Perch, String> {
        Self::open_at(app_root()?)
    }

    /// 真正打开的地方。目录能指到别处，测试就不用碰用户自己的数据目录。
    pub fn open_at(root: PathBuf) -> Result<Perch, String> {
        let device_id = device_id(&root)?;
        let store = Store::new(root);
        let data = match store.load() {
            Ok(data) => data,
            // 还没有文件 = 首次启动，不是故障。
            Err(perch_core::StorageError::NotFound) => Data::empty(device_id.clone()),
            // 文件坏了或版本比本应用新：一份都不能改，只把话说清楚。
            Err(err) => {
                return Ok(Perch {
                    store,
                    data: Data::empty(device_id.clone()),
                    device_id,
                    read_only: Some(err.user_message()),
                });
            }
        };
        // JSON 里的 `deviceId` 记的是「最后写这份文件的是谁」，本机 ID 仍以 settings.json 为准。
        let mut data = data;
        data.device_id = device_id.clone();
        Ok(Perch {
            store,
            data,
            device_id,
            read_only: None,
        })
    }

    pub fn data(&self) -> &Data {
        &self.data
    }

    pub fn device_id(&self) -> &str {
        &self.device_id
    }

    pub fn read_only_reason(&self) -> Option<&str> {
        self.read_only.as_deref()
    }

    /// 改一条数据并立刻落盘。
    ///
    /// 先改副本：`edit` 报错或写盘失败时 `self.data` 一个字都没变，界面不会出现
    /// 「看着存下来了、重启又没了」。落盘本身是 perch-core 的临时文件 + rename。
    pub fn commit(
        &mut self,
        edit: impl FnOnce(&mut Data) -> Result<(), EditError>,
    ) -> Result<(), String> {
        if let Some(reason) = &self.read_only {
            return Err(reason.clone());
        }
        let mut candidate = self.data.clone();
        edit(&mut candidate).map_err(|err| err.user_message())?;
        if candidate == self.data {
            // 反复点同一个按钮不该多留一份备份。
            return Ok(());
        }
        let now = clock::now();
        match self
            .store
            .save(&candidate, now.instant_ms, now.offset_secs)
            .map_err(|err| err.user_message())
        {
            Ok(()) => {
                self.data = candidate;
                Ok(())
            }
            Err(message) => Err(message),
        }
    }
}

/// Tauri 的托管状态：命令从各个 webview 线程进来，所以要锁。
pub struct SharedState {
    inner: Mutex<Perch>,
}

impl SharedState {
    pub fn new(perch: Perch) -> Self {
        SharedState {
            inner: Mutex::new(perch),
        }
    }

    /// 在一次加锁里读完就返回，命令层不持有锁跨 await。
    pub fn with<R>(&self, f: impl FnOnce(&mut Perch) -> Result<R, String>) -> Result<R, String> {
        // 就算上一次在这里 panic，`Perch` 里的数据仍然完整（写盘是原子的、要么换要么不换），
        // 不值得让一张桌面卡片跟着一起崩。
        let mut guard = self
            .inner
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner());
        f(&mut guard)
    }
}

fn app_root() -> Result<PathBuf, String> {
    let base = std::env::var_os("LOCALAPPDATA")
        .ok_or_else(|| "找不到 %LOCALAPPDATA%，数据没地方放".to_string())?;
    Ok(PathBuf::from(base).join(APP_DIR))
}

/// 本机 deviceId：`settings.json` 里读，没有就新生成一个写回去。
///
/// 这里用的是最普通的写文件，没有临时文件加 rename——写坏了也只是重新生成一个 ID，
/// 不会丢任何记录；而 `deviceId` 参与的是合并时的来源标记，改名不影响已有数据。
fn device_id(root: &Path) -> Result<String, String> {
    let path = root.join(SETTINGS_FILE);
    let from_disk = fs::read_to_string(&path)
        .ok()
        .and_then(|text| serde_json::from_str::<Settings>(&text).ok());
    if let Some(settings) = from_disk.filter(|s| !s.device_id.trim().is_empty()) {
        return Ok(settings.device_id);
    }
    let id = uuid::Uuid::new_v4().to_string();
    let json = serde_json::to_string_pretty(&Settings {
        device_id: id.clone(),
    })
    .map_err(|err| format!("写不出 settings.json: {err}"))?;
    fs::create_dir_all(root).map_err(|err| format!("创建 {} 失败: {err}", root.display()))?;
    fs::write(&path, json).map_err(|err| format!("写 {} 失败: {err}", path.display()))?;
    Ok(id)
}

#[cfg(test)]
mod tests {
    use super::*;
    use perch_core::edit::{self, NewHabit};
    use perch_core::Now;

    const NOW: Now = Now {
        instant_ms: 1_792_958_400_000,
        offset_secs: 8 * 3600,
    };

    fn temp_dir(tag: &str) -> PathBuf {
        let mut path = std::env::temp_dir();
        path.push(format!("perch-{tag}-{}", uuid::Uuid::new_v4()));
        fs::create_dir_all(&path).unwrap();
        path
    }

    fn add_habit(
        id: &'static str,
        name: &'static str,
    ) -> impl FnOnce(&mut Data) -> Result<(), EditError> {
        move |data| {
            edit::add_habit(
                data,
                id,
                NewHabit {
                    name: name.to_string(),
                    ..NewHabit::default()
                },
                NOW,
            )
        }
    }

    #[test]
    fn an_empty_folder_opens_without_creating_data_json() {
        let dir = temp_dir("empty");
        let perch = Perch::open_at(dir.clone()).unwrap();
        // 首次启动不落盘：没有习惯就是没有习惯，不写占位文件（§2.4）。
        assert!(!dir.join("data.json").exists());
        assert!(dir.join("settings.json").exists());
        assert_eq!(perch.data().habits.len(), 0);
        assert!(perch.read_only_reason().is_none());
        let _ = fs::remove_dir_all(dir);
    }

    #[test]
    fn the_device_id_survives_a_reopen() {
        let dir = temp_dir("device");
        let first = Perch::open_at(dir.clone()).unwrap();
        let second = Perch::open_at(dir.clone()).unwrap();
        // 换一次 ID 就等于在合并历史里凭空多出一台设备。
        assert_eq!(first.device_id(), second.device_id());
        let _ = fs::remove_dir_all(dir);
    }

    #[test]
    fn a_successful_commit_lands_on_disk() {
        let dir = temp_dir("ok");
        let mut perch = Perch::open_at(dir.clone()).unwrap();
        perch.commit(add_habit("h1", "阅读")).unwrap();
        let on_disk = Store::new(dir.clone()).load().unwrap();
        assert_eq!(on_disk.habits.len(), 1);
        assert_eq!(on_disk.habits[0].name, "阅读");
        assert_eq!(on_disk.device_id, perch.device_id());
        let _ = fs::remove_dir_all(dir);
    }

    #[test]
    fn a_failed_write_leaves_memory_and_disk_on_the_old_version() {
        let dir = temp_dir("fail");
        let mut perch = Perch::open_at(dir.clone()).unwrap();
        perch.commit(add_habit("h1", "阅读")).unwrap();
        // 用一个叫 backups 的普通文件挡住备份目录：没有恢复点的覆盖必须中止。
        fs::write(dir.join("backups"), "挡住").unwrap();
        let before = fs::read(dir.join("data.json")).unwrap();

        let err = perch.commit(add_habit("h2", "散步")).unwrap_err();
        assert!(err.contains("数据读写失败"), "实际收到：{err}");
        assert_eq!(perch.data().habits.len(), 1, "内存里不该多出一个习惯");
        assert_eq!(
            fs::read(dir.join("data.json")).unwrap(),
            before,
            "磁盘上那份旧数据一个字节都不该动"
        );
        let leftovers: Vec<_> = fs::read_dir(&dir)
            .unwrap()
            .flatten()
            .map(|e| e.file_name().to_string_lossy().to_string())
            .filter(|name| name.ends_with(".tmp"))
            .collect();
        assert!(leftovers.is_empty(), "临时文件没清掉：{leftovers:?}");
        let _ = fs::remove_dir_all(dir);
    }

    #[test]
    fn a_corrupt_file_opens_read_only_and_is_never_overwritten() {
        const BROKEN: &str = "{ 这不是 JSON";
        let dir = temp_dir("corrupt");
        fs::write(dir.join("data.json"), BROKEN).unwrap();
        let mut perch = Perch::open_at(dir.clone()).unwrap();
        let reason = perch.read_only_reason().map(str::to_string);
        assert!(reason.is_some(), "坏文件应当进入只读，而不是当成空数据");

        let err = perch.commit(add_habit("h1", "阅读")).unwrap_err();
        assert_eq!(Some(err), reason);
        assert_eq!(
            fs::read(dir.join("data.json")).unwrap(),
            BROKEN.as_bytes(),
            "坏文件必须原样留着等恢复，不能被空数据覆盖"
        );
        let _ = fs::remove_dir_all(dir);
    }

    #[test]
    fn a_newer_schema_refuses_writes_too() {
        let dir = temp_dir("newer");
        fs::write(dir.join("data.json"), r#"{"schemaVersion": 99}"#).unwrap();
        let mut perch = Perch::open_at(dir.clone()).unwrap();
        let reason = perch.read_only_reason().unwrap().to_string();
        assert!(reason.contains("升级"), "实际收到：{reason}");
        assert!(perch.commit(add_habit("h1", "阅读")).is_err());
        let _ = fs::remove_dir_all(dir);
    }
}
