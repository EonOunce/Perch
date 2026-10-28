//! 数据的落点与「内存不领先于磁盘」这条规矩。
//!
//! `data.json` 怎么原子写、怎么留备份、版本不认识时怎么拒绝，全在 perch-core 的 `Store` 里。
//! 这一层只做四件事：找到 `%LOCALAPPDATA%\Perch`、记住本机 deviceId、
//! 记住窗口位置与折叠状态这些设备本地设置（`settings.json`），
//! 以及把「改内存」和「落盘」绑成一次动作——写失败时界面上看到的仍然是磁盘上的那份。

use std::fs;
use std::path::{Path, PathBuf};
use std::sync::{Arc, Mutex};

use perch_core::{fingerprint, merge, Data, EditError, Store};
use serde::{Deserialize, Serialize};

use crate::clock;

/// 应用专用目录名（DESIGN §3.1、§5.1 已定）。
const APP_DIR: &str = "Perch";
const SETTINGS_FILE: &str = "settings.json";

/// 坚果云的 WebDAV 根地址（DESIGN §4.1）。本来就是这个；单独写成一项是为了
/// 本地桩与将来可能的自建服务能改，而不是让人去改代码。
pub const DEFAULT_SYNC_ENDPOINT: &str = "https://dav.jianguoyun.com/dav/";

/// 设备本地设置。位置、折叠状态与长条收起状态是 §2.8 要求记住的三件事；
/// 自启两项是 §12 #6 的「默认关、只问一次」；早晨浮出两项是 §2.3 的「每天最多一次、可关」。
/// 不同步、不进日志。
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", default)]
pub struct Settings {
    pub device_id: String,
    /// 折叠成一整条；展开时为 false。
    pub collapsed: bool,
    /// 折叠态里长条是否展开（日期栏右边那根箭头只管这个）。
    pub strip_rows: bool,
    /// 上次的位置；从没记住过时为 None，首次启动就落在右下角。
    pub place: Option<Place>,
    /// 开机自启。默认关，且只有她自己点头过才写启动项。
    pub autostart: bool,
    /// 首次启动那一次问过了没有——问过就不再追问（§12 #6）。
    pub autostart_asked: bool,
    /// 早晨自动浮出。默认开，但只在真有待确认项时浮（§2.3）；托盘菜单里可关。
    pub morning_raise: bool,
    /// 上一次浮出是哪一天（`localDate`，04:00 日界）。同一天不第二次（§2.3）。
    pub morning_raise_date: Option<String>,
    /// 「关于栖迟」那个入口提示过一次没有（§2.10：只提示一次，不首启强弹）。
    pub about_hinted: bool,
    /// 自动同步开关。默认关：没登录、没联网也要能把卡片用完（§4.1）。
    pub sync_enabled: bool,
    /// 坚果云账号（邮箱）。不是秘密，可以留在这份设置里；应用专用密码走 DPAPI。
    pub sync_account: String,
    /// WebDAV 根地址。默认坚果云（[`DEFAULT_SYNC_ENDPOINT`]）；改它是为了本地桩。
    pub sync_endpoint: String,
}

impl Default for Settings {
    fn default() -> Self {
        Settings {
            device_id: String::new(),
            collapsed: false,
            // 第一次折叠时默认把长条摊开，不然「收起」反而是默认状态。
            strip_rows: true,
            place: None,
            autostart: false,
            autostart_asked: false,
            morning_raise: true,
            morning_raise_date: None,
            about_hinted: false,
            sync_enabled: false,
            sync_account: String::new(),
            sync_endpoint: DEFAULT_SYNC_ENDPOINT.to_string(),
        }
    }
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct Place {
    /// 显示器设备名；对不上同名显示器时退回亮着的这一块。
    pub monitor: Option<String>,
    /// 相对该显示器工作区左上角的逻辑坐标（换算见 place.rs）。
    pub x: f64,
    pub y: f64,
}

/// 一份运行中的数据 + 它的来源。
pub struct Perch {
    store: Store,
    data: Data,
    device_id: String,
    /// 有些情况下文件不该被覆盖：文件坏了、或它由更新的版本写过。
    /// 非空时界面上显示这句话，所有写入一律拒绝——只读比覆盖一个还能救的文件安全。
    read_only: Option<String>,
    /// 界面上写下一条记录之后的提醒（同步线程据此记「本地有改动」）。
    /// 同步自己把合并结果写回来时**不**触发——那是同步产生的写，不该再触发一次同步。
    on_change: Option<Arc<dyn Fn() + Send + Sync>>,
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
                    on_change: None,
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
            on_change: None,
        })
    }

    /// 装上「数据变了」的提醒。只装一次；没装时所有写入与从前一样。
    pub fn set_on_change(&mut self, notify: Arc<dyn Fn() + Send + Sync>) {
        self.on_change = Some(notify);
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
                if let Some(notify) = &self.on_change {
                    notify();
                }
                Ok(())
            }
            Err(message) => Err(message),
        }
    }

    /// 同步线程把合并结果写回来（DESIGN §4.4 步骤 3d）。
    ///
    /// `incoming` 是引擎算出来的合并结果，基于**发起同步那一刻**的快照。同步要走网络，
    /// 期间界面完全可能又添了一条——所以这里不直接覆盖，而是拿上次同步的基线再合一次，
    /// 刚写下的正文一个字都不能丢。内容与现在一致就不写盘：每轮重试都重写会白转一份备份。
    pub fn apply_merged(&mut self, incoming: &Data, base: Option<&Data>) -> Result<(), String> {
        if let Some(reason) = &self.read_only {
            return Err(reason.clone());
        }
        let mut candidate = merge::merge(&self.data, incoming, base);
        candidate.device_id = self.device_id.clone();
        if fingerprint(&candidate) == fingerprint(&self.data) {
            return Ok(());
        }
        let now = clock::now();
        self.store
            .save(&candidate, now.instant_ms, now.offset_secs)
            .map_err(|err| err.user_message())?;
        self.data = candidate;
        // 故意不触发 on_change：这次写是同步自己产生的，不该再排一次同步。
        Ok(())
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

/// settings.json 的读写。写入走临时文件 + rename：改到一半失败，旧的那份还在。
pub struct Prefs {
    root: PathBuf,
    settings: Settings,
}

impl Prefs {
    pub fn load(root: &Path) -> Prefs {
        Prefs {
            root: root.to_path_buf(),
            settings: read_settings(&settings_path(root)).unwrap_or_default(),
        }
    }

    pub fn settings(&self) -> &Settings {
        &self.settings
    }

    /// 改设置并立刻落盘；写失败时内存里的值回到改之前，
    /// 这样界面不会出现「看着记住了、下次开机又是旧的」。
    pub fn update(&mut self, edit: impl FnOnce(&mut Settings)) -> Result<(), String> {
        let before = self.settings.clone();
        edit(&mut self.settings);
        if self.settings == before {
            return Ok(());
        }
        match write_settings(&self.root, &self.settings) {
            Ok(()) => Ok(()),
            Err(err) => {
                self.settings = before;
                Err(err)
            }
        }
    }
}

/// 窗口事件与命令都会来改设置，和 SharedState 一样用一把锁排队。
pub struct SharedPrefs {
    inner: Mutex<Prefs>,
}

impl SharedPrefs {
    pub fn new(prefs: Prefs) -> Self {
        SharedPrefs {
            inner: Mutex::new(prefs),
        }
    }

    pub fn with<R>(&self, f: impl FnOnce(&mut Prefs) -> Result<R, String>) -> Result<R, String> {
        let mut guard = self
            .inner
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner());
        f(&mut guard)
    }
}

fn settings_path(root: &Path) -> PathBuf {
    root.join(SETTINGS_FILE)
}

fn read_settings(path: &Path) -> Option<Settings> {
    fs::read_to_string(path)
        .ok()
        .and_then(|text| serde_json::from_str::<Settings>(&text).ok())
}

/// 和 data.json 一个规矩：先写临时文件，再一步换上去。
fn write_settings(root: &Path, settings: &Settings) -> Result<(), String> {
    let json = serde_json::to_string_pretty(settings)
        .map_err(|err| format!("写不出 settings.json: {err}"))?;
    fs::create_dir_all(root).map_err(|err| format!("创建 {} 失败: {err}", root.display()))?;
    let path = settings_path(root);
    let tmp = root.join(format!("{SETTINGS_FILE}.tmp"));
    fs::write(&tmp, json).map_err(|err| format!("写 {} 失败: {err}", tmp.display()))?;
    fs::rename(&tmp, &path).map_err(|err| {
        let _ = fs::remove_file(&tmp);
        format!("替换 {} 失败: {err}", path.display())
    })
}

pub(crate) fn app_root() -> Result<PathBuf, String> {
    let base = std::env::var_os("LOCALAPPDATA")
        .ok_or_else(|| "找不到 %LOCALAPPDATA%，数据没地方放".to_string())?;
    Ok(PathBuf::from(base).join(APP_DIR))
}

/// 本机 deviceId：`settings.json` 里读，没有就新生成一个写回去。
///
/// 坏掉的设置文件最多让 ID 重生成一次，不会丢任何记录；而 `deviceId`
/// 参与合并时的来源标记，改名不影响已有数据。
fn device_id(root: &Path) -> Result<String, String> {
    let mut settings = read_settings(&settings_path(root)).unwrap_or_default();
    if !settings.device_id.trim().is_empty() {
        return Ok(settings.device_id);
    }
    settings.device_id = uuid::Uuid::new_v4().to_string();
    write_settings(root, &settings)?;
    Ok(settings.device_id)
}

#[cfg(test)]
mod tests {
    use super::*;
    use perch_core::edit::{self, NewHabit};
    use perch_core::Now;
    use std::sync::atomic::{AtomicUsize, Ordering};

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

    #[test]
    fn the_local_settings_survive_a_reopen() {
        let dir = temp_dir("prefs");
        let perch = Perch::open_at(dir.clone()).unwrap();
        let mut prefs = Prefs::load(&dir);
        assert_eq!(prefs.settings().device_id, perch.device_id());
        assert!(!prefs.settings().collapsed);
        assert!(
            prefs.settings().strip_rows,
            "默认摊开长条，收起要是用户按的"
        );
        assert!(prefs.settings().place.is_none());

        prefs
            .update(|settings| {
                settings.collapsed = true;
                settings.strip_rows = false;
                settings.place = Some(Place {
                    monitor: Some(r"\\.\DISPLAY1".to_string()),
                    x: 12.5,
                    y: -3.0,
                });
            })
            .unwrap();

        let again = Prefs::load(&dir);
        assert!(again.settings().collapsed);
        assert!(!again.settings().strip_rows);
        assert_eq!(again.settings().place, prefs.settings().place);
        // 设备 ID 跟记录合并在意的是来源标记，改设置不能把它弄丢。
        assert_eq!(again.settings().device_id, perch.device_id());
        assert!(
            !dir.join("settings.json.tmp").exists(),
            "临时文件不该留在磁盘上"
        );
        let _ = fs::remove_dir_all(dir);
    }

    #[test]
    fn an_old_settings_file_without_the_new_fields_still_opens() {
        let dir = temp_dir("prefs-old");
        fs::create_dir_all(&dir).unwrap();
        fs::write(
            dir.join("settings.json"),
            r#"{"deviceId":"dev-from-last-version"}"#,
        )
        .unwrap();
        let prefs = Prefs::load(&dir);
        // 旧版本只写过 deviceId；缺的字段要按默认值来，而不是当整份文件坏掉。
        assert_eq!(prefs.settings().device_id, "dev-from-last-version");
        assert!(!prefs.settings().collapsed);
        assert!(prefs.settings().strip_rows);
        let _ = fs::remove_dir_all(dir);
    }

    #[test]
    fn a_failed_settings_write_keeps_the_old_file_and_memory() {
        let dir = temp_dir("prefs-fail");
        fs::create_dir_all(&dir).unwrap();
        const OLD: &str = r#"{"deviceId":"dev-1","collapsed":true}"#;
        fs::write(dir.join("settings.json"), OLD).unwrap();
        let mut prefs = Prefs::load(&dir);
        // 占住临时文件的位置，让写入在这一步失败。
        fs::create_dir(dir.join("settings.json.tmp")).unwrap();

        let err = prefs
            .update(|settings| settings.collapsed = false)
            .unwrap_err();
        assert!(err.contains("settings.json.tmp"), "实际收到：{err}");
        assert!(
            prefs.settings().collapsed,
            "写失败时内存里的值也要回到改之前"
        );
        assert_eq!(fs::read_to_string(dir.join("settings.json")).unwrap(), OLD);
        let _ = fs::remove_dir_all(dir);
    }

    /// 同步的三项设置默认是「关、空账号、坚果云」：没登录也要能把卡片用完（§4.1），
    /// 而旧版本写下的设置文件缺这三项时不能被当成坏文件。
    #[test]
    fn sync_settings_default_to_off_and_jianguoyun() {
        let dir = temp_dir("sync-defaults");
        let prefs = Prefs::load(&dir);
        assert!(!prefs.settings().sync_enabled);
        assert!(prefs.settings().sync_account.is_empty());
        assert_eq!(prefs.settings().sync_endpoint, DEFAULT_SYNC_ENDPOINT);

        fs::create_dir_all(&dir).unwrap();
        fs::write(dir.join("settings.json"), r#"{"deviceId":"dev-old"}"#).unwrap();
        let old = Prefs::load(&dir);
        assert_eq!(old.settings().device_id, "dev-old");
        assert!(!old.settings().sync_enabled);
        assert_eq!(old.settings().sync_endpoint, DEFAULT_SYNC_ENDPOINT);
        let _ = fs::remove_dir_all(dir);
    }

    /// 界面写下一笔之后要告诉同步线程；写失败与「按了没变化」都不算。
    #[test]
    fn a_successful_commit_notifies_the_sync_thread() {
        let dir = temp_dir("notify");
        let mut perch = Perch::open_at(dir.clone()).unwrap();
        let hits = Arc::new(AtomicUsize::new(0));
        let counter = hits.clone();
        perch.set_on_change(Arc::new(move || {
            counter.fetch_add(1, Ordering::SeqCst);
        }));

        perch.commit(add_habit("h1", "阅读")).unwrap();
        assert_eq!(hits.load(Ordering::SeqCst), 1);

        // 写盘失败：数据没变，也不该叫醒同步线程。
        fs::write(dir.join("backups"), "挡住").unwrap();
        assert!(perch
            .commit(|data| edit::add_note(data, "n1", "一句", NOW))
            .is_err());
        assert_eq!(hits.load(Ordering::SeqCst), 1);

        fs::remove_file(dir.join("backups")).unwrap();
        // 同步自己写回来的那份不该再触发一次同步。
        let snapshot = perch.data().clone();
        perch.apply_merged(&snapshot, None).unwrap();
        assert_eq!(hits.load(Ordering::SeqCst), 1, "同步写回不是本地改动");
        let _ = fs::remove_dir_all(dir);
    }

    /// 同步至少要跑几秒网络；这期间用户又写了一条。引擎拿回来的是**旧快照**的合并结果，
    /// 直接覆盖就会把刚写下的正文抹掉——这里钉住「先再合一次」。
    #[test]
    fn an_edit_made_during_a_sync_survives_the_merge_back() {
        let dir = temp_dir("merge-back");
        let mut perch = Perch::open_at(dir.clone()).unwrap();
        perch
            .commit(|data| edit::add_note(data, "n1", "原文", NOW))
            .unwrap();

        let snapshot = perch.data().clone();
        // 同步正在走网络，用户又写了一条。
        perch
            .commit(|data| edit::add_note(data, "n2", "同步期间写的", NOW))
            .unwrap();

        // 引擎基于旧快照算出合并结果，并带回了另一台设备的一条。
        let mut incoming = snapshot.clone();
        edit::add_note(&mut incoming, "n9", "手机上写的", NOW).unwrap();
        perch.apply_merged(&incoming, Some(&snapshot)).unwrap();

        let on_disk = Store::new(dir.clone()).load().unwrap();
        let texts: Vec<&str> = on_disk
            .notes
            .iter()
            .map(|note| note.text.as_str())
            .collect();
        assert!(texts.contains(&"原文"));
        assert!(
            texts.contains(&"同步期间写的"),
            "同步期间的编辑被旧快照抹掉了"
        );
        assert!(texts.contains(&"手机上写的"));
        assert_eq!(on_disk.device_id, perch.device_id(), "写出去的仍是本机的号");

        // 同一份合并结果再写一次：内容没变就不该重写文件。
        let before = fs::read(dir.join("data.json")).unwrap();
        perch.apply_merged(&incoming, Some(&snapshot)).unwrap();
        assert_eq!(fs::read(dir.join("data.json")).unwrap(), before);
        let _ = fs::remove_dir_all(dir);
    }

    /// 只读模式下同步也不能写：文件坏了或版本更该留给人处理。
    #[test]
    fn sync_cannot_write_into_a_read_only_store() {
        let dir = temp_dir("ro-sync");
        fs::write(dir.join("data.json"), "{ 坏掉的").unwrap();
        let mut perch = Perch::open_at(dir.clone()).unwrap();
        assert!(perch.read_only_reason().is_some());
        assert!(perch.apply_merged(&Data::empty("phone"), None).is_err());
        assert_eq!(
            fs::read_to_string(dir.join("data.json")).unwrap(),
            "{ 坏掉的",
            "只读时原文件一个字节都不能动"
        );
        let _ = fs::remove_dir_all(dir);
    }
}
