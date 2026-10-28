//! 同步的运行时（DESIGN §4.3、§4.4、§4.7）。
//!
//! 决策全在 `perch_core::sync::run` 里。这一层只做四件事：
//! ① 从设置与 DPAPI 里凑出「地址 + 账号 + 密码」；
//! ② 把同步放到自己的线程上跑——网络慢不能把卡片卡住；
//! ③ 按 §4.3 的节奏决定下一次什么时候跑（启动/聚焦节流、本地改动防抖、周期、手动）；
//! ④ 把结果与 [`SyncState`] 落盘、把状态与错误交给界面。
//!
//! 同步内容是**明文**（AGENTS.md 不可逆决定 #1）：这里不做任何客户端加密，
//! 界面上也不说「已加密」。只有应用专用密码留在本机 DPAPI 里，且它不参与同步。

use std::fs;
use std::path::{Path, PathBuf};
use std::sync::{Condvar, Mutex};
use std::time::Duration;

use perch_core::sync::{
    backoff_ms, run, Env, Outcome, SyncError, SyncState, PERIODIC_MS, TRIGGER_GAP_MS,
};
use serde::Serialize;
use tauri::{AppHandle, Emitter, Manager};

use crate::clock;
use crate::credential;
use crate::probe_log;
use crate::store::{SharedPrefs, SharedState, DEFAULT_SYNC_ENDPOINT};
use crate::webdav::Webdav;

/// 同步状态与基线。放进本机设置目录，**不进 `data.json`、不参与同步**（§4.4）。
const STATE_FILE: &str = "sync-state.json";

/// 本地 WebDAV 桩的地址开关。只在调试构建里认，且必须是回环地址（§4.2）。
pub const STUB_ENV: &str = "PERCH_SYNC_STUB";

/// 自动同步的运行状态与持久化基线。跨线程共享，后台线程独占「什么时候跑」。
pub struct Syncer {
    root: PathBuf,
    inner: Mutex<Signal>,
    wake: Condvar,
}

struct Signal {
    /// 持久化的那一份：ETag、基线、最后成功时间、连续失败次数、是否暂停。
    state: SyncState,
    /// 上次**真正发请求**的时刻（毫秒）。失败也记，退避靠它。
    last_attempt_ms: i64,
    /// 一次待跑的触发。手动请求会覆盖它。
    pending: Option<Pending>,
    /// 本地数据的改动代数。同步跑完时没变，才把 pending 清掉——
    /// 否则网络那几秒里新写的正文会被当成「已经同步过」。
    change_seq: u64,
    running: bool,
    /// 最近一次失败的原文（§4.7 的「下一步」）。成功即清。
    last_error: Option<String>,
    /// 手动同步没跑起来的原因（没配、正在跑）。不是错误，不按失败算。
    last_note: Option<String>,
}

#[derive(Clone, Copy, PartialEq, Eq)]
enum Pending {
    /// 启动、窗口重新获得焦点：尽快跑，但两次之间至少隔 2 分钟（§4.3）。
    Soon(i64),
    /// 本地写了新东西：等输入停下来 2 分钟再跑，别每敲一个字就排一次（§4.3）。
    Touched(i64),
    /// 手动「立即同步」：不看节流与退避。
    Manual,
}

/// 界面读的那一份状态。字段名就是前端的词。
#[derive(Clone, Debug, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct SyncView {
    pub enabled: bool,
    /// 账号、密码、地址都齐了。缺一样就不会自动跑。
    pub configured: bool,
    pub running: bool,
    pub queued: bool,
    pub auto_paused: bool,
    pub consecutive_failures: u32,
    pub last_synced_at: Option<String>,
    pub last_error: Option<String>,
    pub last_note: Option<String>,
}

/// 设置页读的那一份：账号与地址可以看，口令只看「有没有」。
#[derive(Clone, Debug, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct SyncSettingsView {
    pub enabled: bool,
    pub account: String,
    pub endpoint: String,
    pub has_password: bool,
}

impl Syncer {
    /// 打开同步状态。文件坏了就当没有基线——§4.5 的保守合并会保住双方正文，
    /// 而「因为一份状态文件读不出来就不同步」才是更糟的选择。
    pub fn load(root: PathBuf) -> Syncer {
        let state = read_state(&root);
        Syncer {
            root,
            inner: Mutex::new(Signal {
                state,
                last_attempt_ms: 0,
                pending: None,
                change_seq: 0,
                running: false,
                last_error: None,
                last_note: None,
            }),
            wake: Condvar::new(),
        }
    }

    fn lock(&self) -> std::sync::MutexGuard<'_, Signal> {
        self.inner
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner())
    }

    /// 本地写入了新数据：防抖 2 分钟后再同步（§4.3）。
    ///
    /// 代数只在这里前进：同步跑完时它没变，才说明网络那几秒没人写过东西。
    pub fn touch(&self) {
        let mut sig = self.lock();
        sig.change_seq = sig.change_seq.saturating_add(1);
        if !matches!(sig.pending, Some(Pending::Manual)) {
            sig.pending = Some(Pending::Touched(clock::now().instant_ms));
        }
        self.wake.notify_all();
    }

    /// 启动或窗口重新获得焦点：尽快同步，但节流到 2 分钟一次（§4.3）。
    pub fn soon(&self) {
        let mut sig = self.lock();
        if !matches!(sig.pending, Some(Pending::Manual)) {
            sig.pending = Some(Pending::Soon(clock::now().instant_ms));
        }
        self.wake.notify_all();
    }

    /// 手动「立即同步」。不配置就什么都不跑，由 worker 给出说明。
    pub fn request_manual(&self) {
        let mut sig = self.lock();
        sig.pending = Some(Pending::Manual);
        sig.last_note = None;
        self.wake.notify_all();
    }

    fn change_seq(&self) -> u64 {
        self.lock().change_seq
    }

    fn set_running(&self, running: bool) {
        self.lock().running = running;
    }

    fn resume(&self) {
        // 手动同步或改完设置：暂停位与退避一起清，不然改对了还要空等 15 分钟（§4.7）。
        self.lock().state.resume();
    }

    /// 同步成功：基线、ETag、最后成功时间全换成这一份，失败计数清零。
    fn record_success(&self, outcome: &Outcome, seq: u64, now_ms: i64) {
        let mut sig = self.lock();
        sig.state = outcome.state.clone();
        sig.last_attempt_ms = now_ms;
        sig.last_error = None;
        sig.last_note = None;
        if sig.change_seq == seq {
            sig.pending = None;
        }
        let snapshot = sig.state.clone();
        drop(sig);
        persist_state(&self.root, &snapshot);
    }

    /// 同步失败：只动失败计数与暂停位，`etag` 与基线保持上次成功的值（§4.4）。
    fn record_failure(&self, error: &SyncError, seq: u64, now_ms: i64) {
        let mut sig = self.lock();
        sig.state.record_failure(error);
        sig.last_attempt_ms = now_ms;
        sig.last_error = Some(error.user_message());
        sig.last_note = None;
        if sig.change_seq == seq {
            sig.pending = None;
        }
        let snapshot = sig.state.clone();
        drop(sig);
        persist_state(&self.root, &snapshot);
    }

    fn note(&self, text: String) {
        let mut sig = self.lock();
        sig.last_note = Some(text);
        // 没真的发请求，不算一次尝试：退避与周期都不动。
        if matches!(sig.pending, Some(Pending::Manual)) {
            sig.pending = None;
        }
    }

    /// 下一次该跑的毫秒时刻。`None` 表示现在不该自动跑。
    fn due_at(sig: &Signal, ready: bool) -> Option<i64> {
        if !ready || sig.state.auto_paused {
            return None;
        }
        let wait = if sig.state.consecutive_failures > 0 {
            backoff_ms(sig.state.consecutive_failures) as i64
        } else {
            PERIODIC_MS as i64
        };
        // `last_attempt_ms == 0` 是「这次开机还没跑过」：启动就该同步一次，不等一个周期。
        let periodic = if sig.last_attempt_ms <= 0 {
            0
        } else {
            sig.last_attempt_ms.saturating_add(wait)
        };
        let triggered = sig.pending.map(|pending| {
            let debounce = match pending {
                Pending::Touched(_) => TRIGGER_GAP_MS as i64,
                Pending::Soon(_) => 0,
                Pending::Manual => 0,
            };
            let at = match pending {
                Pending::Manual => i64::MAX,
                Pending::Soon(at) | Pending::Touched(at) => at,
            };
            at.saturating_add(debounce)
                .max(sig.last_attempt_ms.saturating_add(TRIGGER_GAP_MS as i64))
        });
        Some(match triggered {
            Some(triggered) => periodic.min(triggered),
            None => periodic,
        })
    }
}

/// 后台线程：等一个到点或一次提醒，跑一轮，把状态说清楚。
pub fn spawn(app: &AppHandle) {
    let handle = app.clone();
    std::thread::spawn(move || loop {
        let why = wait_for_due(&handle);
        let syncer = handle.state::<Syncer>();
        let seq = syncer.change_seq();
        syncer.set_running(true);
        probe_log::note(format!("[sync] 开始（{:?}）", why));
        let attempt = attempt(&handle, why);
        syncer.set_running(false);
        let now = clock::now().instant_ms;

        match attempt {
            Attempt::Synced(outcome) => {
                probe_log::note(format!(
                    "[sync] 完成：{} 轮，上传={}，本地写入={}",
                    outcome.rounds, outcome.uploaded, outcome.local_saved
                ));
                let saved = outcome.local_saved;
                syncer.record_success(&outcome, seq, now);
                if saved {
                    let _ = handle.emit("perch:changed", ());
                }
            }
            Attempt::Failed(error) => {
                probe_log::note(format!("[sync] 失败：{}", error.user_message()));
                syncer.record_failure(&error, seq, now);
            }
            Attempt::Skipped(note) => {
                probe_log::note(format!("[sync] 没跑：{note}"));
                syncer.note(note);
            }
        }
        let _ = handle.emit("perch:sync", view(&handle));
    });
}

fn wait_for_due(app: &AppHandle) -> Why {
    loop {
        let prefs = read_prefs(app);
        let syncer = app.state::<Syncer>();
        let ready = prefs.ready(&syncer.root);
        let mut sig = syncer.lock();
        let now = clock::now().instant_ms;

        if matches!(sig.pending, Some(Pending::Manual)) {
            sig.pending = None;
            return Why::Manual;
        }
        match Syncer::due_at(&sig, ready) {
            Some(due) if now >= due => {
                let why = match sig.pending.take() {
                    Some(Pending::Touched(_)) => Why::Touched,
                    _ => Why::Soon,
                };
                return why;
            }
            Some(due) => {
                let wait = Duration::from_millis((due - now).clamp(1, 3_600_000) as u64);
                sig = syncer
                    .wake
                    .wait_timeout(sig, wait)
                    .unwrap_or_else(|poisoned| poisoned.into_inner())
                    .0;
            }
            // 没开、没配好、或暂停中：一直等到有人改设置或点手动。
            // 排队的那次自动触发已经没有意义（界面上的「排队中」也不该一直亮着）。
            None => {
                sig.pending = None;
                sig = syncer
                    .wake
                    .wait(sig)
                    .unwrap_or_else(|poisoned| poisoned.into_inner());
            }
        }
    }
}

/// worker 为什么跑这一轮。只影响日志与「手动时清退避」。
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Why {
    Soon,
    Touched,
    Manual,
}

enum Attempt {
    /// 结果带着合并后的整份快照（要拿去当新基线），比另外两种大一个数量级。
    Synced(Box<Outcome>),
    Failed(SyncError),
    Skipped(String),
}

fn attempt(app: &AppHandle, why: Why) -> Attempt {
    let prefs = read_prefs(app);
    let syncer = app.state::<Syncer>();
    let password = match credential::load(&syncer.root) {
        Ok(Some(password)) if !password.is_empty() => password,
        Ok(_) => return Attempt::Skipped("还没填应用专用密码。打开同步设置填一下。".into()),
        Err(err) => return Attempt::Skipped(format!("{err}。请回同步设置重新填一次。")),
    };
    if prefs.account.is_empty() {
        return Attempt::Skipped("还没填坚果云账号。打开同步设置填一下。".into());
    }
    if why == Why::Manual {
        syncer.resume();
    }

    // 本地快照与基线必须成对：基线就是「这份本地快照上次同步到哪」。
    let base = syncer.lock().state.base.clone();
    let local = match app
        .state::<SharedState>()
        .with(|perch| Ok(perch.data().clone()))
    {
        Ok(local) => local,
        Err(err) => return Attempt::Skipped(err),
    };

    let (endpoint, allow_plain_http) = match stub_endpoint() {
        Some(stub) => (stub, true),
        None => (prefs.endpoint.clone(), false),
    };
    let transport = match Webdav::new(&endpoint, &prefs.account, &password, allow_plain_http) {
        Ok(transport) => transport,
        Err(_) => return Attempt::Failed(SyncError::Endpoint),
    };

    let now = clock::now();
    let save_base = base.clone();
    let handle = app.clone();
    let mut env = Env {
        now_ms: now.instant_ms,
        offset_secs: now.offset_secs,
        allow_plain_http,
        save_local: Box::new(move |merged| {
            match handle
                .state::<SharedState>()
                .with(|perch| perch.apply_merged(merged, save_base.as_ref()))
            {
                Ok(()) => true,
                Err(err) => {
                    probe_log::note(format!("[sync] 合并结果没写进本地：{err}"));
                    false
                }
            }
        }),
        sleep: Box::new(|ms| std::thread::sleep(Duration::from_millis(ms))),
        random: Box::new(random_unit),
    };
    match run(&endpoint, &local, base.as_ref(), &transport, &mut env) {
        Ok(outcome) => Attempt::Synced(Box::new(outcome)),
        Err(error) => Attempt::Failed(error),
    }
}

/// 重试前的抖动只要 0.5–2 秒的区分度，不值得为它引一个随机数库：
/// 取当前纳秒的低位足够，而且失败重试本来就不需要不可预测。
fn random_unit() -> f64 {
    use std::time::{SystemTime, UNIX_EPOCH};
    let nanos = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|elapsed| elapsed.subsec_nanos())
        .unwrap_or(0);
    f64::from(nanos % 1_000) / 1_000.0
}

/// 调试构建里可以指到本地 WebDAV 桩（§4.2 的例外，只在回环地址上）。
fn stub_endpoint() -> Option<String> {
    if !cfg!(debug_assertions) {
        return None;
    }
    std::env::var(STUB_ENV)
        .ok()
        .map(|value| value.trim().to_string())
        .filter(|value| !value.is_empty())
}

#[derive(Clone, Default)]
struct Snapshot {
    enabled: bool,
    account: String,
    endpoint: String,
}

impl Snapshot {
    fn ready(&self, root: &Path) -> bool {
        self.enabled
            && !self.account.is_empty()
            && !self.endpoint.is_empty()
            && credential::path_of(root).is_file()
    }
}

fn read_prefs(app: &AppHandle) -> Snapshot {
    app.state::<SharedPrefs>()
        .with(|prefs| {
            let settings = prefs.settings();
            Ok(Snapshot {
                enabled: settings.sync_enabled,
                account: settings.sync_account.trim().to_string(),
                endpoint: settings.sync_endpoint.trim().to_string(),
            })
        })
        .unwrap_or_default()
}

/// 界面要的那一份状态。
pub fn view(app: &AppHandle) -> SyncView {
    let prefs = read_prefs(app);
    let syncer = app.state::<Syncer>();
    let has_password = credential::path_of(&syncer.root).is_file();
    let sig = syncer.lock();
    SyncView {
        enabled: prefs.enabled,
        configured: !prefs.account.is_empty() && !prefs.endpoint.is_empty() && has_password,
        running: sig.running,
        queued: sig.pending.is_some(),
        auto_paused: sig.state.auto_paused,
        consecutive_failures: sig.state.consecutive_failures,
        last_synced_at: sig.state.last_synced_at.clone(),
        last_error: sig.last_error.clone(),
        last_note: sig.last_note.clone(),
    }
}

fn read_state(root: &Path) -> SyncState {
    match fs::read_to_string(root.join(STATE_FILE)) {
        Ok(text) => serde_json::from_str(&text).unwrap_or_else(|err| {
            probe_log::note(format!("[sync] 同步状态读不出来，按没有基线继续：{err}"));
            SyncState::default()
        }),
        Err(_) => SyncState::default(),
    }
}

/// 状态落盘。写失败只记一笔：下一次同步会重算，最坏是多传一次，
/// 但「因为状态没记住就把这次成功当失败」会让界面说谎。
fn persist_state(root: &Path, state: &SyncState) {
    let json = match serde_json::to_string(state) {
        Ok(json) => json,
        Err(err) => {
            probe_log::note(format!("[sync] 同步状态序列化失败：{err}"));
            return;
        }
    };
    let tmp = root.join(format!("{STATE_FILE}.tmp"));
    let result = fs::create_dir_all(root)
        .and_then(|_| fs::write(&tmp, json))
        .and_then(|_| fs::rename(&tmp, root.join(STATE_FILE)));
    if let Err(err) = result {
        let _ = fs::remove_file(&tmp);
        probe_log::note(format!("[sync] 同步状态没写进磁盘：{err}"));
    }
}

#[tauri::command]
pub fn sync_status(app: AppHandle) -> SyncView {
    view(&app)
}

/// 托盘「立即同步」与设置页的按钮都走这里：只排一次请求，不在这里等网络。
#[tauri::command]
pub fn sync_now(app: AppHandle) -> SyncView {
    app.state::<Syncer>().request_manual();
    view(&app)
}

#[tauri::command]
pub fn sync_settings(app: AppHandle) -> SyncSettingsView {
    let prefs = read_prefs(&app);
    let has_password = credential::path_of(&app.state::<Syncer>().root).is_file();
    SyncSettingsView {
        enabled: prefs.enabled,
        account: prefs.account,
        endpoint: prefs.endpoint,
        has_password,
    }
}

/// 改同步设置。`password` 传 `None` 表示不动，传空串表示清掉。
#[tauri::command]
pub fn sync_configure(
    app: AppHandle,
    account: Option<String>,
    endpoint: Option<String>,
    enabled: Option<bool>,
    password: Option<String>,
) -> Result<SyncView, String> {
    let root = app.state::<Syncer>().root.clone();
    if let Some(password) = &password {
        if password.trim().is_empty() {
            credential::forget(&root)?;
        } else {
            credential::save(&root, password)?;
        }
    }
    app.state::<SharedPrefs>().with(|prefs| {
        prefs.update(|settings| {
            if let Some(account) = &account {
                settings.sync_account = account.trim().to_string();
            }
            if let Some(endpoint) = &endpoint {
                let trimmed = endpoint.trim();
                settings.sync_endpoint = if trimmed.is_empty() {
                    DEFAULT_SYNC_ENDPOINT.to_string()
                } else {
                    trimmed.to_string()
                };
            }
            if let Some(enabled) = enabled {
                settings.sync_enabled = enabled;
            }
        })
    })?;

    let syncer = app.state::<Syncer>();
    // 用户刚改过设置：暂停位与退避都清掉，然后马上试一次——改对了不该等 15 分钟。
    syncer.resume();
    syncer.soon();
    Ok(view(&app))
}

#[cfg(test)]
mod tests {
    use super::*;
    use perch_core::model::Data;

    fn signal() -> Signal {
        Signal {
            state: SyncState::default(),
            last_attempt_ms: 0,
            pending: None,
            change_seq: 0,
            running: false,
            last_error: None,
            last_note: None,
        }
    }

    fn temp_dir(tag: &str) -> PathBuf {
        let mut path = std::env::temp_dir();
        path.push(format!("perch-sync-{tag}-{}", uuid::Uuid::new_v4()));
        fs::create_dir_all(&path).unwrap();
        path
    }

    /// 没开、没配好、暂停中，一律不该自动跑。
    #[test]
    fn auto_sync_never_runs_unless_enabled_configured_and_not_paused() {
        let mut sig = signal();
        assert!(Syncer::due_at(&sig, false).is_none(), "没配好不跑");
        assert!(Syncer::due_at(&sig, true).is_some(), "开着就该排");

        sig.state.auto_paused = true;
        assert!(Syncer::due_at(&sig, true).is_none(), "暂停中不跑");
    }

    /// 启动时没有待跑的触发，也该马上跑一次；之后按周期排。
    #[test]
    fn a_fresh_start_syncs_at_once_then_waits_the_periodic_interval() {
        let mut sig = signal();
        assert_eq!(Syncer::due_at(&sig, true), Some(0), "从没跑过，立刻到期");

        sig.last_attempt_ms = 1_000_000;
        assert_eq!(
            Syncer::due_at(&sig, true),
            Some(1_000_000 + PERIODIC_MS as i64)
        );
    }

    /// 本地写了新东西：等输入停下来 2 分钟；同一个防抖也被「距上次同步 2 分钟」压住。
    #[test]
    fn a_local_edit_waits_two_minutes_before_syncing() {
        let mut sig = signal();
        sig.last_attempt_ms = 10_000;
        sig.pending = Some(Pending::Touched(20_000));
        assert_eq!(
            Syncer::due_at(&sig, true),
            Some(20_000 + TRIGGER_GAP_MS as i64),
            "防抖从最后一次改动算起"
        );

        // 刚同步过 1 秒就改了：安静 2 分钟的排期比「距上次同步满 2 分钟」更晚，取更晚的那个。
        sig.pending = Some(Pending::Touched(11_000));
        assert_eq!(
            Syncer::due_at(&sig, true),
            Some(11_000 + TRIGGER_GAP_MS as i64)
        );
    }

    /// 启动/聚焦触发：尽快，但两次同步之间仍要隔 2 分钟。
    #[test]
    fn a_focus_trigger_still_respects_the_two_minute_gap() {
        let mut sig = signal();
        sig.last_attempt_ms = 1_000;
        sig.pending = Some(Pending::Soon(1_500));
        assert_eq!(
            Syncer::due_at(&sig, true),
            Some(1_000 + TRIGGER_GAP_MS as i64)
        );
    }

    /// 失败之后按退避排，而且周期不能把退避盖掉。
    #[test]
    fn a_failed_sync_waits_for_the_backoff_instead_of_the_period() {
        let mut sig = signal();
        sig.last_attempt_ms = 5_000;
        sig.state.consecutive_failures = 1;
        assert_eq!(Syncer::due_at(&sig, true), Some(5_000 + 60_000));
        sig.state.consecutive_failures = 2;
        assert_eq!(Syncer::due_at(&sig, true), Some(5_000 + 300_000));
        sig.state.consecutive_failures = 5;
        assert_eq!(Syncer::due_at(&sig, true), Some(5_000 + 900_000));
    }

    /// 手动请求不看节流：点了就跑。
    #[test]
    fn a_manual_request_is_not_delayed_by_the_gap() {
        let mut sig = signal();
        sig.last_attempt_ms = 999_999;
        sig.pending = Some(Pending::Manual);
        // `due_at` 只管自动那一路；手动由 worker 在 `wait_for_due` 里直接放行，
        // 这里确认它不会被当成「还需要等」的状态卡住。
        assert_eq!(
            Syncer::due_at(&sig, true),
            Some(999_999 + PERIODIC_MS as i64)
        );
    }

    #[test]
    fn failure_keeps_the_baseline_and_success_replaces_it() {
        let mut sig = signal();
        let base = Data::empty("pc");
        sig.state.base = Some(base.clone());
        sig.state.etag = Some("\"V1\"".to_string());
        sig.state.record_failure(&SyncError::Offline);
        assert_eq!(sig.state.base, Some(base), "失败不能动基线");
        assert_eq!(sig.state.etag.as_deref(), Some("\"V1\""));
        assert_eq!(sig.state.consecutive_failures, 1);
        assert!(!sig.state.auto_paused, "断网只退避，不暂停");

        sig.state.record_failure(&SyncError::Credentials);
        assert!(sig.state.auto_paused, "密码错了要停自动同步");
        sig.state.resume();
        assert!(!sig.state.auto_paused);
        assert_eq!(sig.state.consecutive_failures, 0);
    }

    /// 状态落盘再读回来一个字都不差；文件坏了就当没有基线，而不是打不开应用。
    #[test]
    fn the_sync_state_survives_a_disk_round_trip() {
        let dir = temp_dir("state");
        let state = SyncState {
            etag: Some("\"V7\"".to_string()),
            base: Some(Data::empty("pc")),
            last_synced_at: Some("2026-10-28T12:00:00.000+08:00".to_string()),
            ..SyncState::default()
        };
        persist_state(&dir, &state);
        assert_eq!(read_state(&dir), state);

        fs::write(dir.join(STATE_FILE), "{ 坏了").unwrap();
        assert_eq!(read_state(&dir), SyncState::default());
        let _ = fs::remove_dir_all(dir);
    }

    /// 「配好了」= 开关开着 + 账号 + 地址 + 本机真的有一份口令。
    #[test]
    fn ready_needs_a_switch_an_account_an_endpoint_and_a_password() {
        let dir = temp_dir("ready");
        let mut snapshot = Snapshot {
            enabled: true,
            account: "someone@example.com".to_string(),
            endpoint: DEFAULT_SYNC_ENDPOINT.to_string(),
        };
        assert!(!snapshot.ready(&dir), "没有口令就不算配好");
        credential::save(&dir, "app-password").unwrap();
        assert!(snapshot.ready(&dir));

        snapshot.enabled = false;
        assert!(!snapshot.ready(&dir));
        snapshot.enabled = true;
        snapshot.account.clear();
        assert!(!snapshot.ready(&dir));
        let _ = fs::remove_dir_all(dir);
    }

    /// 抖动只用来错开两台设备的重试，0–1 的抽头必须落在 0.5–2 秒之间。
    #[test]
    fn the_jitter_draw_stays_in_range() {
        for _ in 0..20 {
            let value = random_unit();
            assert!((0.0..1.0).contains(&value));
        }
    }

    /// `note` 只给手动没跑起来的情况用：不能把退避当失败记，也不能留着排队状态。
    #[test]
    fn a_skipped_manual_request_only_leaves_a_note() {
        let dir = temp_dir("note");
        let syncer = Syncer::load(dir.join("state"));
        syncer.request_manual();
        syncer.note("还没填账号".to_string());
        let sig = syncer.lock();
        assert_eq!(sig.last_note.as_deref(), Some("还没填账号"));
        assert_eq!(sig.state.consecutive_failures, 0);
        assert!(sig.pending.is_none(), "跳过之后不该还排着队");
        drop(sig);
        let _ = fs::remove_dir_all(dir);
    }

    /// 同步跑的那几秒里又改了一笔：跑完不能把这次改动当成已同步。
    #[test]
    fn a_change_during_the_run_keeps_the_sync_queued() {
        let dir = temp_dir("seq");
        let syncer = Syncer::load(dir.join("state"));
        let seq = syncer.change_seq();
        // 模拟网络期间界面又写了一条。
        syncer.touch();
        assert_ne!(seq, syncer.change_seq(), "改动代数必须前进");
        assert!(syncer.lock().pending.is_some(), "改动还在排队");
        let _ = fs::remove_dir_all(dir);
    }
}
