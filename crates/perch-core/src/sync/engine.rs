//! 一次同步的决策（DESIGN §4.4）。网络、磁盘、时钟都不在这个文件里：
//! HTTP 由宿主实现 [`Transport`]，本地落盘是 [`Env::save_local`]，`now_ms` 由宿主读表传进来。
//! 核心只回答三件事：什么时候 GET、要不要 PUT、用什么条件头；失败了算哪一类、用户下一步做什么。
//!
//! 三条不让步的性质，每条都有测试盯着：
//! - **读不懂就不写**：远端缺字段、坏了、或版本比本机新（§4.7）时一律不 PUT。
//!   误读一次就把另一台设备的数据抹掉了，这是本协议里唯一不可恢复的事故。
//! - **没落盘就不上传**：合并结果先写本地再传（§4.4 末条），传一半断了本地仍是完整的一份。
//! - **失败不动基线**：[`SyncState::base`] 是 §4.5 判「哪一侧改过」的唯一参照，写错它比同步失败更糟。

use serde::{Deserialize, Serialize};

use crate::date_boundary::format_instant;
use crate::merge::{fingerprint, merge, unknown_schema};
use crate::model::{Data, SCHEMA_VERSION};
use crate::sync::webdav::{
    endpoint_allowed, Condition, NetError, Remote, Transport, COLLECTIONS, SNAPSHOT_PATH,
};

/// §4.4 步骤 3e：被抢先写之后最多重试 3 次（也就是最多走 4 轮）。
pub const MAX_RETRIES: u32 = 3;

/// §4.3 的触发节奏：启动/获得焦点至少隔 2 分钟，本地变更后也等 2 分钟（防抖）。
/// 数字只在这里出现一次，shell 与将来手机端都读它。
pub const TRIGGER_GAP_MS: u64 = 120_000;

/// §4.3 的运行期周期同步。设计写的是 10–15 分钟，这里取上限：
/// 坚果云免费额度按 30 分钟 600 请求算（§4.6），宁可收敛慢一点也别撞额度。
pub const PERIODIC_MS: u64 = 900_000;

/// §4.3 失败退避：1 → 5 → 15 分钟，之后一直是 15 分钟。成功或手动同步后回到正常节奏。
pub fn backoff_ms(consecutive_failures: u32) -> u64 {
    match consecutive_failures {
        0 => 0,
        1 => 60_000,
        2 => 300_000,
        _ => 900_000,
    }
}

/// §4.4 步骤 3e 的抖动：0.5–2 秒。随机数由宿主给——核心不自己取随机，测试才能把等待时长钉死。
pub fn jitter_ms(draw: f64) -> u64 {
    500 + (draw.clamp(0.0, 1.0) * 1500.0) as u64
}

/// 要持久化的同步状态。存在本机设置里，**不进 `data.json`、不参与同步**（§3.4、§4.2）。
/// 拆成 `sync-base.json` 还是并到一个 settings 文件由宿主决定，这里只管内容。
#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", default)]
pub struct SyncState {
    /// 最后一次看到的远端 ETag。服务端不给就为 None，条件写退化成裸 PUT（§4.4 末条）。
    pub etag: Option<String>,
    /// 上次同步完的那份快照，即 §4.4 里的 `sync-base.json`。冲突检测的基线。
    pub base: Option<Data>,
    /// 上次同步**成功**的时间。失败时不更新，界面上「上次同步 · 12 分钟前」读它。
    pub last_synced_at: Option<String>,
    /// 连续失败次数，用来算 [`backoff_ms`]。成功一次即清零。
    pub consecutive_failures: u32,
    /// §4.7：密码/权限/版本/坏文件这类错误之后停掉自动同步，直到用户处理或手动同步一次。
    pub auto_paused: bool,
}

impl SyncState {
    /// 失败之后要持久化的那一份。只动计数与暂停位，`etag` 与 `base` 保持上一次成功的值。
    pub fn record_failure(&mut self, kind: &SyncError) {
        self.consecutive_failures = self.consecutive_failures.saturating_add(1);
        if kind.pauses_auto() {
            self.auto_paused = true;
        }
    }

    /// 用户改完设置、或手动点了「立即同步」：暂停位与退避都要清，不然改对了还要空等 15 分钟。
    pub fn resume(&mut self) {
        self.auto_paused = false;
        self.consecutive_failures = 0;
    }

    /// 后台这一次该不该跑（§4.3）。手动同步不看这个，手动随时能按。
    pub fn auto_due(&self, last_attempt_ms: i64, now_ms: i64) -> bool {
        if self.auto_paused {
            return false;
        }
        let wait = if self.consecutive_failures == 0 {
            PERIODIC_MS
        } else {
            backoff_ms(self.consecutive_failures)
        };
        now_ms.saturating_sub(last_attempt_ms) >= wait as i64
    }

    fn succeeded(merged: &Data, etag: Option<String>, now_ms: i64, offset_secs: i32) -> SyncState {
        SyncState {
            etag,
            base: Some(merged.clone()),
            last_synced_at: Some(format_instant(now_ms, offset_secs)),
            consecutive_failures: 0,
            auto_paused: false,
        }
    }
}

/// 同步失败的类别（§4.7）。每条给一句「下一步做什么」，都不带日记正文。
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum SyncError {
    /// §4.2 地址不合：不是 https、缺主机名、或把密码写进了 URL。
    Endpoint,
    /// 连不上（DNS、超时、断网）。应用照常可用，稍后自动再试。
    Offline,
    /// 连接不是可信的 HTTPS（证书、协议）。绝不退化成明文重发（§4.2）。
    InsecureTransport,
    /// 应用专用密码不对（401）。
    Credentials,
    /// 账号能登录，但这个目录不给读写（403/404）。
    Permission,
    /// 坚果云空间不足或超额（507）。
    NoSpace,
    /// 远端由更新版本写入（§4.7）：只读，绝不上传降级数据。
    RemoteTooNew { found: u64 },
    /// 远端那份读不出结构（写坏的文件、或别的程序动过）：不覆盖它，等人处理。
    RemoteUnreadable,
    /// 反复被另一台设备抢先写，重试 3 次仍没传上去。
    Contended,
    /// 服务端不认条件写请求头，连裸写也没成功（§4.4 末条的退化路径也走不通）。
    PreconditionUnsupported,
    /// 本地这份准备不出去：写盘失败，或序列化失败。没落盘就不上传。
    LocalWrite,
    /// 其它服务端返回，`u16` 是状态码。
    Server(u16),
}

impl SyncError {
    /// 给界面用的一句话，带下一步动作（§4.7）。
    pub fn user_message(&self) -> String {
        match self {
            SyncError::Endpoint => {
                "同步地址不合规矩：只用 https:// 的 WebDAV 地址，密码也不要写在地址里。请回设置页改服务端地址。"
                    .to_string()
            }
            SyncError::Offline => {
                "连不上坚果云。本地照常记录，稍后自动再试。".to_string()
            }
            SyncError::InsecureTransport => {
                "和坚果云的连接不是可信的 HTTPS，已停止同步。请检查系统时间与网络代理后重试。"
                    .to_string()
            }
            SyncError::Credentials => {
                "应用专用密码不对，自动同步已暂停。请在设置页重新填一次密码。".to_string()
            }
            SyncError::Permission => {
                "这个账号对坚果云里的 Perch 目录没有写权限，自动同步已暂停。请确认目录还在、用的是自己的账号。"
                    .to_string()
            }
            SyncError::NoSpace => {
                "坚果云空间不够，这次没传上去。清出空间后点「立即同步」。".to_string()
            }
            SyncError::RemoteTooNew { found } => format!(
                "坚果云上那份由更新版本的栖迟写入（版本 {found}），这次只读不写。请先升级本机的栖迟。"
            ),
            SyncError::RemoteUnreadable => {
                "坚果云上那份文件读不出来，已停止上传以免覆盖。请在坚果云里确认 perch.json 是否完整。"
                    .to_string()
            }
            SyncError::Contended => {
                "另一台设备刚刚改过，这次没合上去。本地已经存好，稍后再同步一次就好。".to_string()
            }
            SyncError::PreconditionUnsupported => {
                "坚果云不接受同步用的条件写入请求头，这次没传上去。本地不受影响，稍后自动再试。"
                    .to_string()
            }
            SyncError::LocalWrite => {
                "本地数据文件写不进去（磁盘空间或权限问题），已取消上传。请腾出空间后重试。"
                    .to_string()
            }
            SyncError::Server(code) => format!(
                "坚果云返回了 {code}，这次没同步成。本地不受影响，稍后自动再试。"
            ),
        }
    }

    /// 这类错误重试没有意义，得用户先处理（§4.7「凭据错误时不反复重试」）。
    pub fn pauses_auto(&self) -> bool {
        matches!(
            self,
            SyncError::Endpoint
                | SyncError::Credentials
                | SyncError::Permission
                | SyncError::RemoteTooNew { .. }
                | SyncError::RemoteUnreadable
        )
    }
}

impl From<NetError> for SyncError {
    fn from(e: NetError) -> SyncError {
        match e {
            NetError::Unreachable => SyncError::Offline,
            NetError::Tls => SyncError::InsecureTransport,
            NetError::Unauthorized => SyncError::Credentials,
            NetError::Forbidden => SyncError::Permission,
            NetError::NotFound => SyncError::Permission,
            NetError::Conflict => SyncError::Contended,
            NetError::PreconditionUnsupported => SyncError::PreconditionUnsupported,
            NetError::InsufficientStorage => SyncError::NoSpace,
            NetError::AlreadyExists => SyncError::Permission,
            NetError::Other(code) => SyncError::Server(code),
        }
    }
}

/// 这一次同步要向宿主借的东西：时钟、落盘、等待、随机数。核心一样都不自己拿。
pub struct Env<'a> {
    /// 此刻的 UTC 毫秒（§5.4：宿主传进来，测试才能钉死）。
    pub now_ms: i64,
    /// 此刻的 UTC 偏移秒数，与 `now_ms` 一起用来盖 `generatedAt`。
    pub offset_secs: i32,
    /// 明文 HTTP 的显式开关，只给本地 WebDAV 桩用，且只在调试构建生效（§4.2）。
    pub allow_plain_http: bool,
    /// 写 `data.json`（宿主的 `Store` 负责原子替换与滚动备份）。返回 false 表示没写进去。
    pub save_local: Box<dyn FnMut(&Data) -> bool + 'a>,
    /// 重试前等待。核心只说等多久，真睡的是宿主——测试里换成空操作。
    pub sleep: Box<dyn FnMut(u64) + 'a>,
    /// 抖动用的随机数，0.0–1.0 之外按端点算。
    pub random: Box<dyn FnMut() -> f64 + 'a>,
}

/// 一次同步的结果：本地要落盘什么、有没有传上去、状态该持久化成什么。
#[derive(Debug, Clone, PartialEq)]
pub struct Outcome {
    /// 合并完的那一份，也是 [`SyncState::base`] 的内容。
    pub merged: Data,
    /// 这次有没有重写 `data.json`。本地与远端都没新东西时不重写，免得每次同步轮转一份备份。
    pub local_saved: bool,
    /// 这次有没有把快照写上去（§4.4 步骤 3c 判「内容一致」时为 false）。
    pub uploaded: bool,
    /// 走了几轮。1 = 没被抢写过；>1 说明另一台设备也在同一时间同步。
    pub rounds: u32,
    pub state: SyncState,
}

/// 跑一次同步（§4.4 的 1–5 步）。宿主先把 `data.json` 与 `sync-base` 读出来传进来，
/// 拿到 [`Outcome`] 之后负责落盘：`merged` 已经在函数里通过 [`Env::save_local`] 写过了，
/// `state` 由宿主持久化到本机设置。失败返回 [`SyncError`]，本地数据不动。
pub fn run(
    endpoint: &str,
    local: &Data,
    base: Option<&Data>,
    transport: &dyn Transport,
    env: &mut Env,
) -> Result<Outcome, SyncError> {
    // §4.2 的地址规矩在发第一个请求之前挡住：填错服务端比同步失败更糟。
    if !endpoint_allowed(endpoint, env.allow_plain_http) {
        return Err(SyncError::Endpoint);
    }

    let mut rounds = 0u32;
    // 这一次同步里已经落过盘的内容。重试的每一轮都会重新合并，内容没变就别再转一份本地备份。
    let mut saved: Option<Data> = None;
    loop {
        rounds += 1;
        // 步骤 3e 的抖动等待放在重取之前（第一轮不等待）。
        if rounds > 1 {
            let wait = jitter_ms((env.random)());
            (env.sleep)(wait);
        }

        // 步骤 2：每轮重新 GET。上一轮的 ETag 到这一轮已经不作数了。
        let remote = match transport.fetch(SNAPSHOT_PATH) {
            Ok(Remote::Absent) => None,
            Ok(Remote::Found { body, etag }) => Some((read_snapshot(&body)?, etag)),
            Err(e) => return Err(SyncError::from(e)),
        };

        // 步骤 3a：版本检查在合并之前。读不懂的东西绝不上传（§4.7）。
        if let Some((snapshot, _)) = &remote {
            if unknown_schema(snapshot) {
                return Err(SyncError::RemoteTooNew {
                    found: snapshot.schema_version as u64,
                });
            }
        }

        // 步骤 3b：合并。远端还没有文件时，「合并结果」就是本地这一份的形状化副本。
        let merged = match &remote {
            Some((snapshot, _)) => merge(local, snapshot, base),
            None => normalized(local),
        };

        // 步骤 3c：内容与远端一致就不 PUT（§4.6 额度）。generatedAt 与 deviceId 不算内容。
        let differs_from_remote = match &remote {
            Some((snapshot, _)) => fingerprint(&merged) != fingerprint(snapshot),
            None => true,
        };

        // 步骤 3d 前半：合并结果先落盘，再谈上传（§4.4 末条）。
        let differs_from_local = fingerprint(&merged) != fingerprint(local);
        let worth_saving = differs_from_local
            && !saved
                .as_ref()
                .is_some_and(|prev| fingerprint(prev) == fingerprint(&merged));
        if worth_saving {
            if !(env.save_local)(&merged) {
                return Err(SyncError::LocalWrite);
            }
            saved = Some(merged.clone());
        }
        let local_saved = saved.is_some();
        let seen_etag = remote.as_ref().and_then(|(_, etag)| etag.clone());

        if !differs_from_remote {
            return Ok(done(env, &merged, seen_etag, local_saved, false, rounds));
        }
        // 两边都还没有任何记录时，不在云上留一份空快照，也不建目录（§2.4 首次启动是空的）。
        if remote.is_none() && is_empty_snapshot(&merged) {
            return Ok(done(env, &merged, None, local_saved, false, rounds));
        }

        // 步骤 4：远端不存在，先按需 MKCOL（§4.2），再以 `If-None-Match: *` 创建。
        if remote.is_none() {
            ensure_collections(transport)?;
        }
        let cond = match &remote {
            None => Condition::Absent,
            Some((_, Some(etag))) => Condition::IfMatch(etag.clone()),
            // 服务端这份响应里没带 ETag，就只能裸 PUT（§4.4 末条的退化路径）。
            Some((_, None)) => Condition::Any,
        };
        let body = upload_body(&merged, env)?;
        match write(transport, &body, &cond) {
            Ok(new_etag) => {
                // 步骤 5：成功后把基线换成刚写上去的这份；etag 优先用响应里的新值。
                let etag = new_etag.or_else(|| cond_etag(&cond)).or(seen_etag);
                return Ok(done(env, &merged, etag, local_saved, true, rounds));
            }
            // 被抢先写（412/409）：回到步骤 2 重取重合（§4.4 步骤 3e）。
            Err(NetError::Conflict) | Err(NetError::NotFound) => {
                if rounds > MAX_RETRIES {
                    return Err(SyncError::Contended);
                }
            }
            Err(e) => return Err(SyncError::from(e)),
        }
    }
}

/// 组装成功产出。`state` 里已经清了失败计数、恢复了自动同步。
fn done(
    env: &Env,
    merged: &Data,
    etag: Option<String>,
    local_saved: bool,
    uploaded: bool,
    rounds: u32,
) -> Outcome {
    Outcome {
        merged: merged.clone(),
        local_saved,
        uploaded,
        rounds,
        state: SyncState::succeeded(merged, etag, env.now_ms, env.offset_secs),
    }
}

/// 裸 PUT 成功时没有响应 ETag 可用，就把这次发出去的条件头值当作「我写上去的那份」的标识。
fn cond_etag(cond: &Condition) -> Option<String> {
    match cond {
        Condition::IfMatch(etag) => Some(etag.clone()),
        _ => None,
    }
}

/// 远端快照解析。报错文字里可能捎带日记正文，一律丢掉，只留「读不出来」这一类（§4.7）。
fn read_snapshot(body: &str) -> Result<Data, SyncError> {
    serde_json::from_str(body).map_err(|_| SyncError::RemoteUnreadable)
}

/// 没参与合并的路径也要给出和 [`merge`] 同形状的一份：版本按本机能写的值，`generatedAt` 留给宿主盖。
fn normalized(local: &Data) -> Data {
    Data {
        schema_version: SCHEMA_VERSION,
        generated_at: None,
        ..local.clone()
    }
}

/// 上传前才盖时间戳。另一台设备能看出「这份是谁什么时候写的」，但 [`fingerprint`] 不看它，
/// 所以盖时间戳不会被算成一次改动、不会引起反复上传。
fn upload_body(merged: &Data, env: &Env) -> Result<String, SyncError> {
    let stamped = Data {
        schema_version: SCHEMA_VERSION,
        generated_at: Some(format_instant(env.now_ms, env.offset_secs)),
        ..merged.clone()
    };
    serde_json::to_string(&stamped).map_err(|_| SyncError::LocalWrite)
}

/// 按需建目录。「已经存在」是正常结果——另一台设备可能先建过了（§4.2）。
fn ensure_collections(transport: &dyn Transport) -> Result<(), SyncError> {
    for dir in COLLECTIONS {
        match transport.make_collection(dir) {
            Ok(()) | Err(NetError::AlreadyExists) => {}
            Err(e) => return Err(SyncError::from(e)),
        }
    }
    Ok(())
}

/// PUT，带 §4.4 末条的退化路径：服务端不认条件头时，同一份内容**裸写一次**，
/// 不再重取——已经确认过这次不是被谁抢先改了。裸写再被撞就是 `Conflict`，交回外层重试。
fn write(
    transport: &dyn Transport,
    body: &str,
    cond: &Condition,
) -> Result<Option<String>, NetError> {
    match transport.store(SNAPSHOT_PATH, body, cond) {
        Err(NetError::PreconditionUnsupported) if !matches!(cond, Condition::Any) => {
            transport.store(SNAPSHOT_PATH, body, &Condition::Any)
        }
        other => other,
    }
}

/// 一个业务记录都没有。`deviceId` 不算内容，它只是来源标签。
fn is_empty_snapshot(data: &Data) -> bool {
    data.habits.is_empty()
        && data.markers.is_empty()
        && data.checkin_events.is_empty()
        && data.notes.is_empty()
        && data.daily_moods.is_empty()
        && data.tombstones.is_empty()
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::model::Note;
    use std::cell::{Cell, RefCell};
    use std::collections::VecDeque;

    const OFFSET: i32 = 8 * 3600;
    /// 固定的「此刻」：2026-10-28T12:00:00+08:00。盖时间戳的测试要它一字不差。
    const NOW: i64 = 1_793_160_000_000;
    const DAV: &str = "https://dav.jianguoyun.com/dav/";
    const STAMP: &str = "2026-10-20T09:00:00.000+08:00";

    fn note(id: &str, text: &str, updated: &str) -> Note {
        Note {
            id: id.to_string(),
            created_at: STAMP.to_string(),
            local_date: "2026-10-20".to_string(),
            text: text.to_string(),
            tags: vec!["周末".to_string()],
            updated_at: updated.to_string(),
            conflict_of: None,
        }
    }

    fn device(name: &str, notes: Vec<Note>) -> Data {
        Data {
            device_id: name.to_string(),
            notes,
            ..Data::empty(name)
        }
    }

    fn found(data: &Data, etag: Option<&str>) -> Remote {
        Remote::Found {
            body: serde_json::to_string(data).unwrap(),
            etag: etag.map(str::to_string),
        }
    }

    #[derive(Debug, Clone, PartialEq)]
    enum Call {
        Get,
        Mkcol(&'static str),
        Put(Condition),
    }

    /// 脚本化的假服务端：GET / PUT / MKCOL 各按队列依次回应，队列用完回到默认值。
    /// 自动测试不碰真实网络（AGENTS），坚果云真机验证只手工做。
    #[derive(Default)]
    struct Fake {
        gets: RefCell<VecDeque<Result<Remote, NetError>>>,
        puts: RefCell<VecDeque<Result<Option<String>, NetError>>>,
        mkcols: RefCell<VecDeque<Result<(), NetError>>>,
        calls: RefCell<Vec<Call>>,
        bodies: RefCell<Vec<String>>,
    }

    impl Fake {
        fn on_get(&self, r: Result<Remote, NetError>) -> &Fake {
            self.gets.borrow_mut().push_back(r);
            self
        }

        fn on_put(&self, r: Result<Option<String>, NetError>) -> &Fake {
            self.puts.borrow_mut().push_back(r);
            self
        }

        fn on_mkcol(&self, r: Result<(), NetError>) -> &Fake {
            self.mkcols.borrow_mut().push_back(r);
            self
        }

        fn repeat_get(&self, r: Result<Remote, NetError>, times: usize) -> &Fake {
            for _ in 0..times {
                self.on_get(r.clone());
            }
            self
        }

        fn repeat_put(&self, r: Result<Option<String>, NetError>, times: usize) -> &Fake {
            for _ in 0..times {
                self.on_put(r.clone());
            }
            self
        }

        fn calls(&self) -> Vec<Call> {
            self.calls.borrow().clone()
        }

        fn put_bodies(&self) -> Vec<String> {
            self.bodies.borrow().clone()
        }
    }

    impl Transport for Fake {
        fn fetch(&self, path: &str) -> Result<Remote, NetError> {
            assert_eq!(path, SNAPSHOT_PATH, "只该读写 §4.2 那一个快照文件");
            self.calls.borrow_mut().push(Call::Get);
            self.gets
                .borrow_mut()
                .pop_front()
                .unwrap_or(Ok(Remote::Absent))
        }

        fn store(
            &self,
            path: &str,
            body: &str,
            cond: &Condition,
        ) -> Result<Option<String>, NetError> {
            assert_eq!(path, SNAPSHOT_PATH);
            self.calls.borrow_mut().push(Call::Put(cond.clone()));
            self.bodies.borrow_mut().push(body.to_string());
            self.puts
                .borrow_mut()
                .pop_front()
                .unwrap_or(Ok(Some("\"E-PUT\"".to_string())))
        }

        fn make_collection(&self, path: &str) -> Result<(), NetError> {
            let known = COLLECTIONS
                .iter()
                .find(|dir| **dir == path)
                .copied()
                .expect("只该按需建 §4.2 里那两个目录");
            self.calls.borrow_mut().push(Call::Mkcol(known));
            self.mkcols.borrow_mut().pop_front().unwrap_or(Ok(()))
        }
    }

    /// 宿主那一侧的记录器：本地落过几次盘、等过多久、随机数给多少。
    struct Trace {
        saves: RefCell<Vec<Data>>,
        slept: RefCell<Vec<u64>>,
        save_ok: Cell<bool>,
        draw: Cell<f64>,
    }

    impl Trace {
        fn new() -> Trace {
            Trace {
                saves: RefCell::new(Vec::new()),
                slept: RefCell::new(Vec::new()),
                save_ok: Cell::new(true),
                draw: Cell::new(0.5),
            }
        }

        fn saved(&self) -> Vec<Data> {
            self.saves.borrow().clone()
        }

        fn slept(&self) -> Vec<u64> {
            self.slept.borrow().clone()
        }

        fn env(&self) -> Env<'_> {
            Env {
                now_ms: NOW,
                offset_secs: OFFSET,
                allow_plain_http: false,
                save_local: Box::new(move |data| {
                    if !self.save_ok.get() {
                        return false;
                    }
                    self.saves.borrow_mut().push(data.clone());
                    true
                }),
                sleep: Box::new(move |ms| self.slept.borrow_mut().push(ms)),
                random: Box::new(move || self.draw.get()),
            }
        }
    }

    /// 一个真会按条件头拒绝的内存服务端，用来验两台设备的收敛。
    struct Cloud {
        slot: RefCell<Option<(String, String)>>,
        seq: Cell<u32>,
        /// 永远拒绝写入：模拟「另一台设备一直在写」，让条件写每次都失败。
        refusing_writes: bool,
        fetches: Cell<u32>,
        stores: Cell<u32>,
    }

    impl Cloud {
        fn holding(data: &Data, etag: &str) -> Cloud {
            Cloud {
                slot: RefCell::new(Some((
                    serde_json::to_string(data).unwrap(),
                    etag.to_string(),
                ))),
                seq: Cell::new(1),
                refusing_writes: false,
                fetches: Cell::new(0),
                stores: Cell::new(0),
            }
        }

        fn refusing(data: &Data, etag: &str) -> Cloud {
            let mut cloud = Cloud::holding(data, etag);
            cloud.refusing_writes = true;
            cloud
        }

        fn client(&self) -> CloudClient<'_> {
            CloudClient { cloud: self }
        }

        fn body(&self) -> Data {
            let raw = self.slot.borrow().clone().expect("云端应该有一份快照").0;
            serde_json::from_str(&raw).unwrap()
        }

        fn etag(&self) -> String {
            self.slot.borrow().as_ref().unwrap().1.clone()
        }
    }

    struct CloudClient<'a> {
        cloud: &'a Cloud,
    }

    impl Transport for CloudClient<'_> {
        fn fetch(&self, _: &str) -> Result<Remote, NetError> {
            self.cloud.fetches.set(self.cloud.fetches.get() + 1);
            Ok(match self.cloud.slot.borrow().as_ref() {
                None => Remote::Absent,
                Some((body, etag)) => Remote::Found {
                    body: body.clone(),
                    etag: Some(etag.clone()),
                },
            })
        }

        fn store(&self, _: &str, body: &str, cond: &Condition) -> Result<Option<String>, NetError> {
            self.cloud.stores.set(self.cloud.stores.get() + 1);
            let stale = match (&cond, self.cloud.slot.borrow().as_ref()) {
                (Condition::Absent, None) => false,
                (Condition::Absent, Some(_)) => true,
                (Condition::IfMatch(want), Some((_, have))) => want != have,
                (Condition::IfMatch(_), None) => true,
                (Condition::Any, _) => false,
            };
            if stale || self.cloud.refusing_writes {
                return Err(NetError::Conflict);
            }
            self.cloud.seq.set(self.cloud.seq.get() + 1);
            let etag = format!("\"V{}\"", self.cloud.seq.get());
            *self.cloud.slot.borrow_mut() = Some((body.to_string(), etag.clone()));
            Ok(Some(etag))
        }

        fn make_collection(&self, _: &str) -> Result<(), NetError> {
            Ok(())
        }
    }

    #[test]
    fn a_first_sync_creates_the_folders_and_uploads_the_local_snapshot() {
        let local = device(
            "pc",
            vec![note("n1", "本机写的", "2026-10-22T09:00:00.000+08:00")],
        );
        let fake = Fake::default();
        fake.on_get(Ok(Remote::Absent));
        let trace = Trace::new();
        let mut env = trace.env();

        let out = run(DAV, &local, None, &fake, &mut env).unwrap();
        assert!(out.uploaded);
        assert_eq!(out.rounds, 1);
        assert_eq!(
            fake.calls(),
            vec![
                Call::Get,
                Call::Mkcol("Perch"),
                Call::Mkcol("Perch/sync"),
                Call::Put(Condition::Absent),
            ],
            "父目录要先建，MKCOL 不会自动补父级（§4.2）"
        );
        assert_eq!(out.state.etag.as_deref(), Some("\"E-PUT\""));
        assert_eq!(out.state.base.as_ref(), Some(&out.merged));
        assert_eq!(
            out.state.last_synced_at.as_deref(),
            Some(format_instant(NOW, OFFSET).as_str())
        );
        assert!(
            !out.local_saved,
            "第一次同步没合进新东西，不该白转一份本地备份"
        );
        assert!(trace.saved().is_empty());

        let body: Data = serde_json::from_str(&fake.put_bodies()[0]).unwrap();
        assert_eq!(body.notes[0].text, "本机写的");
        assert_eq!(
            body.generated_at.as_deref(),
            Some(format_instant(NOW, OFFSET).as_str()),
            "传出去的那份由宿主盖时间戳，合并本身不看时钟（§4.5）"
        );
    }

    #[test]
    fn a_brand_new_device_takes_the_remote_snapshot_without_uploading_anything() {
        let shared = device(
            "pc",
            vec![note(
                "n1",
                "另一台设备写的",
                "2026-10-22T09:00:00.000+08:00",
            )],
        );
        let fake = Fake::default();
        fake.on_get(Ok(found(&shared, Some("\"V1\""))));
        let trace = Trace::new();
        let mut env = trace.env();

        let out = run(DAV, &Data::empty("phone"), None, &fake, &mut env).unwrap();
        assert!(
            !out.uploaded,
            "空设备不能把「什么都没有」当成一次改动传上去"
        );
        assert_eq!(out.merged.notes.len(), 1, "远端那份要落到本地");
        assert_eq!(trace.saved()[0].notes[0].text, "另一台设备写的");
        assert_eq!(out.merged.device_id, "phone", "设备标签不能被远端顶掉");
        assert_eq!(fake.calls(), vec![Call::Get]);
        assert_eq!(
            out.state.etag.as_deref(),
            Some("\"V1\""),
            "没写就该留着读到的 ETag"
        );
    }

    #[test]
    fn an_empty_device_does_not_publish_a_blank_snapshot() {
        let fake = Fake::default();
        fake.on_get(Ok(Remote::Absent));
        let trace = Trace::new();
        let mut env = trace.env();

        let out = run(DAV, &Data::empty("pc"), None, &fake, &mut env).unwrap();
        assert!(!out.uploaded);
        assert_eq!(
            fake.calls(),
            vec![Call::Get],
            "两台设备都还没记录时，不在云上建一个空文件（§2.4 首次启动是空的）"
        );
        assert_eq!(out.state.consecutive_failures, 0);
    }

    #[test]
    fn identical_content_skips_the_put_but_keeps_the_baseline() {
        let shared = device(
            "pc",
            vec![note("n1", "同一份正文", "2026-10-22T09:00:00.000+08:00")],
        );
        let fake = Fake::default();
        fake.on_get(Ok(found(&shared, Some("\"V7\""))));
        let trace = Trace::new();
        let mut env = trace.env();

        let out = run(DAV, &shared, Some(&shared), &fake, &mut env).unwrap();
        assert!(!out.uploaded);
        assert_eq!(fake.calls(), vec![Call::Get]);
        assert_eq!(out.state.base, Some(shared), "基线仍要更新成合并结果");
        assert!(!out.local_saved);
    }

    #[test]
    fn a_local_change_uploads_with_the_etag_just_read() {
        let remote = device(
            "pc",
            vec![note("n1", "云端那份", "2026-10-21T09:00:00.000+08:00")],
        );
        let local = device(
            "phone",
            vec![
                note("n1", "云端那份", "2026-10-21T09:00:00.000+08:00"),
                note("n2", "手机上刚写的", "2026-10-28T09:00:00.000+08:00"),
            ],
        );
        let fake = Fake::default();
        fake.on_get(Ok(found(&remote, Some("\"V7\""))));
        fake.on_put(Ok(Some("\"V8\"".to_string())));
        let trace = Trace::new();
        let mut env = trace.env();

        let out = run(DAV, &local, Some(&remote), &fake, &mut env).unwrap();
        assert!(out.uploaded);
        assert_eq!(
            out.state.etag.as_deref(),
            Some("\"V8\""),
            "上传后要用响应里的新 ETag"
        );
        assert_eq!(out.state.base.as_ref(), Some(&out.merged));
        assert_eq!(out.merged.notes.len(), 2);
        assert_eq!(
            fake.calls(),
            vec![
                Call::Get,
                Call::Put(Condition::IfMatch("\"V7\"".to_string()))
            ],
            "覆盖必须带 If-Match（§4.4 步骤 3d）"
        );
        assert!(
            trace.saved().is_empty(),
            "本地本来就装着两边的记录，往外传就够了，不必再写一遍 data.json"
        );
    }

    #[test]
    fn a_stale_write_re_fetches_and_writes_against_the_fresh_etag() {
        let v1 = device(
            "pc",
            vec![note("n1", "云端第一版", "2026-10-21T09:00:00.000+08:00")],
        );
        let v2 = device(
            "pc",
            vec![
                note("n1", "云端第一版", "2026-10-21T09:00:00.000+08:00"),
                note("n9", "另一台写的", "2026-10-27T09:00:00.000+08:00"),
            ],
        );
        let local = device(
            "phone",
            vec![
                note("n1", "云端第一版", "2026-10-21T09:00:00.000+08:00"),
                note("n2", "手机上刚写的", "2026-10-28T09:00:00.000+08:00"),
            ],
        );
        let fake = Fake::default();
        fake.on_get(Ok(found(&v1, Some("\"V1\""))));
        fake.on_get(Ok(found(&v2, Some("\"V2\""))));
        fake.on_put(Err(NetError::Conflict));
        fake.on_put(Ok(Some("\"V3\"".to_string())));
        let trace = Trace::new();
        let mut env = trace.env();

        let out = run(DAV, &local, Some(&v1), &fake, &mut env).unwrap();
        assert!(out.uploaded);
        assert_eq!(out.rounds, 2, "被抢写一次就多走一轮");
        assert_eq!(
            trace.slept(),
            vec![jitter_ms(0.5)],
            "重试前要抖动等待（§4.4 步骤 3e）"
        );
        assert_eq!(
            fake.calls(),
            vec![
                Call::Get,
                Call::Put(Condition::IfMatch("\"V1\"".to_string())),
                Call::Get,
                Call::Put(Condition::IfMatch("\"V2\"".to_string())),
            ],
            "重试必须重新 GET，旧 ETag 不作数"
        );
        assert_eq!(out.merged.notes.len(), 3, "两轮都要把两边的记录并齐");
        assert_eq!(trace.saved().len(), 1, "内容变了才落盘，一轮一份备份没意义");
    }

    #[test]
    fn three_stale_writes_give_up_without_touching_the_baseline() {
        let remote = device(
            "pc",
            vec![note("n1", "云端那份", "2026-10-21T09:00:00.000+08:00")],
        );
        let local = device(
            "phone",
            vec![
                note("n1", "云端那份", "2026-10-21T09:00:00.000+08:00"),
                note("n2", "手机上刚写的", "2026-10-28T09:00:00.000+08:00"),
            ],
        );
        let fake = Fake::default();
        fake.repeat_get(Ok(found(&remote, Some("\"V1\""))), 4);
        fake.repeat_put(Err(NetError::Conflict), 4);
        let trace = Trace::new();
        let mut env = trace.env();

        let err = run(DAV, &local, Some(&remote), &fake, &mut env).unwrap_err();
        assert_eq!(err, SyncError::Contended);
        assert_eq!(trace.slept().len(), MAX_RETRIES as usize, "1 + 3 次重试");
        assert_eq!(fake.calls().len(), 8, "4 轮，每轮 GET + PUT");
        assert!(
            trace.saved().is_empty(),
            "本地这一份已经装着两边的记录了，四轮撞车也不必重复写盘"
        );

        let mut state = SyncState {
            etag: Some("\"V0\"".to_string()),
            base: Some(remote.clone()),
            consecutive_failures: 1,
            ..SyncState::default()
        };
        state.record_failure(&err);
        assert_eq!(state.etag.as_deref(), Some("\"V0\""));
        assert_eq!(
            state.base,
            Some(remote),
            "写不上就不动基线（§4.5 靠它判谁改过）"
        );
        assert_eq!(state.consecutive_failures, 2);
        assert!(!state.auto_paused, "被抢写是可以自动重试的");
    }

    #[test]
    fn a_remote_from_a_newer_schema_blocks_every_write() {
        let mut newer = device(
            "phone",
            vec![note(
                "n1",
                "新版本写的正文",
                "2026-10-27T09:00:00.000+08:00",
            )],
        );
        newer.schema_version = 7;
        let local = device(
            "pc",
            vec![note("n2", "本机写的", "2026-10-28T09:00:00.000+08:00")],
        );
        let fake = Fake::default();
        fake.on_get(Ok(found(&newer, Some("\"V1\""))));
        let trace = Trace::new();
        let mut env = trace.env();

        let err = run(DAV, &local, None, &fake, &mut env).unwrap_err();
        assert_eq!(err, SyncError::RemoteTooNew { found: 7 });
        assert!(err.pauses_auto());
        assert_eq!(fake.calls(), vec![Call::Get], "绝不上传降级数据（§4.7）");
        assert!(trace.saved().is_empty(), "本机这份也不许被合并悄悄改掉");
        assert!(err.user_message().contains("升级"));
    }

    #[test]
    fn an_unreadable_remote_body_blocks_every_write() {
        let local = device(
            "pc",
            vec![note("n2", "本机写的正文", "2026-10-28T09:00:00.000+08:00")],
        );
        for body in [
            "{ 不是 JSON",
            // 缺必填字段（schemaVersion 与 note 的几个字段）的文件：读不懂就别覆盖。
            r#"{"deviceId":"pc","notes":[{"id":"n1"}]}"#,
        ] {
            let fake = Fake::default();
            fake.on_get(Ok(Remote::Found {
                body: body.to_string(),
                etag: Some("\"V1\"".to_string()),
            }));
            let trace = Trace::new();
            let mut env = trace.env();

            let err = run(DAV, &local, None, &fake, &mut env).unwrap_err();
            assert_eq!(err, SyncError::RemoteUnreadable, "对 {body} 的判定");
            assert_eq!(fake.calls(), vec![Call::Get], "读不懂的远端不能覆盖");
            assert!(err.pauses_auto());
            assert!(
                !err.user_message().contains("本机写的正文"),
                "错误文案不许漏出日记正文（§4.7）"
            );
            assert!(trace.saved().is_empty());
        }
    }

    #[test]
    fn a_credentials_error_stops_immediately_and_pauses_auto_sync() {
        let fake = Fake::default();
        fake.on_get(Err(NetError::Unauthorized));
        let trace = Trace::new();
        let mut env = trace.env();

        let err = run(DAV, &Data::empty("pc"), None, &fake, &mut env).unwrap_err();
        assert_eq!(err, SyncError::Credentials);
        assert_eq!(
            fake.calls(),
            vec![Call::Get],
            "密码错了重试没有意义（§4.7）"
        );
        let mut state = SyncState::default();
        state.record_failure(&err);
        assert!(state.auto_paused);
        assert_eq!(state.consecutive_failures, 1);
        assert!(
            !state.auto_due(NOW, NOW + backoff_ms(1) as i64),
            "暂停了就不该再自己跑"
        );
    }

    #[test]
    fn an_offline_error_stays_retryable() {
        let fake = Fake::default();
        fake.on_get(Err(NetError::Unreachable));
        let trace = Trace::new();
        let mut env = trace.env();

        let err = run(DAV, &Data::empty("pc"), None, &fake, &mut env).unwrap_err();
        assert_eq!(err, SyncError::Offline);
        assert!(!err.pauses_auto());
        let msg = err.user_message();
        assert!(
            msg.contains("本地") && msg.contains("再试"),
            "断网要说清楚本地照常可用、稍后自动重试：{msg}"
        );
        let mut state = SyncState::default();
        state.record_failure(&err);
        assert!(!state.auto_paused);
        assert!(state.auto_due(NOW, NOW + backoff_ms(1) as i64));
    }

    #[test]
    fn a_server_that_omits_etags_degrades_to_a_bare_put() {
        let remote = device(
            "pc",
            vec![note("n1", "云端那份", "2026-10-21T09:00:00.000+08:00")],
        );
        let local = device(
            "phone",
            vec![
                note("n1", "云端那份", "2026-10-21T09:00:00.000+08:00"),
                note("n2", "手机上刚写的", "2026-10-28T09:00:00.000+08:00"),
            ],
        );
        let fake = Fake::default();
        fake.on_get(Ok(found(&remote, None)));
        fake.on_put(Ok(None));
        let trace = Trace::new();
        let mut env = trace.env();

        let out = run(DAV, &local, None, &fake, &mut env).unwrap();
        assert_eq!(
            fake.calls(),
            vec![Call::Get, Call::Put(Condition::Any)],
            "服务端不给 ETag 就裸写，接受极小竞态（§4.4 末条）"
        );
        assert!(out.uploaded);
        assert_eq!(out.state.etag, None, "没有可用的 ETag 就别编一个");
    }

    #[test]
    fn a_refused_precondition_gets_one_bare_retry_without_a_new_get() {
        let remote = device(
            "pc",
            vec![note("n1", "云端那份", "2026-10-21T09:00:00.000+08:00")],
        );
        let local = device(
            "phone",
            vec![
                note("n1", "云端那份", "2026-10-21T09:00:00.000+08:00"),
                note("n2", "手机上刚写的", "2026-10-28T09:00:00.000+08:00"),
            ],
        );
        let fake = Fake::default();
        fake.on_get(Ok(found(&remote, Some("\"V1\""))));
        fake.on_put(Err(NetError::PreconditionUnsupported));
        fake.on_put(Ok(Some("\"V2\"".to_string())));
        let trace = Trace::new();
        let mut env = trace.env();

        let out = run(DAV, &local, Some(&remote), &fake, &mut env).unwrap();
        assert_eq!(
            fake.calls(),
            vec![
                Call::Get,
                Call::Put(Condition::IfMatch("\"V1\"".to_string())),
                Call::Put(Condition::Any),
            ],
            "不认条件头不是被抢写：同一份内容直接裸写一次，不必重取"
        );
        assert_eq!(out.rounds, 1);
        assert!(out.uploaded);
        assert_eq!(out.state.etag.as_deref(), Some("\"V2\""));
        assert!(trace.slept().is_empty());
    }

    #[test]
    fn a_server_that_refuses_condition_headers_every_time_says_so() {
        let remote = device(
            "pc",
            vec![note("n1", "云端那份", "2026-10-21T09:00:00.000+08:00")],
        );
        let local = device(
            "phone",
            vec![
                note("n1", "云端那份", "2026-10-21T09:00:00.000+08:00"),
                note("n2", "手机上刚写的", "2026-10-28T09:00:00.000+08:00"),
            ],
        );
        let fake = Fake::default();
        fake.repeat_get(Ok(found(&remote, Some("\"V1\""))), 4);
        fake.repeat_put(Err(NetError::PreconditionUnsupported), 8);
        let trace = Trace::new();
        let mut env = trace.env();

        let err = run(DAV, &local, Some(&remote), &fake, &mut env).unwrap_err();
        assert_eq!(err, SyncError::PreconditionUnsupported);
        assert!(!err.pauses_auto());
        assert_eq!(
            fake.calls(),
            vec![
                Call::Get,
                Call::Put(Condition::IfMatch("\"V1\"".to_string())),
                Call::Put(Condition::Any),
            ],
            "裸写也被拒就是服务端的事，不该一轮轮撞下去"
        );
    }

    #[test]
    fn a_local_write_failure_cancels_the_upload() {
        let remote = device(
            "pc",
            vec![note("n1", "云端那份", "2026-10-21T09:00:00.000+08:00")],
        );
        let fake = Fake::default();
        fake.on_get(Ok(found(&remote, Some("\"V1\""))));
        let trace = Trace::new();
        trace.save_ok.set(false);
        let mut env = trace.env();

        let err = run(DAV, &Data::empty("phone"), None, &fake, &mut env).unwrap_err();
        assert_eq!(err, SyncError::LocalWrite);
        assert_eq!(fake.calls(), vec![Call::Get], "没落盘就不上传（§4.4 末条）");
        assert_eq!(fake.put_bodies().len(), 0);
    }

    #[test]
    fn mkcol_saying_the_folder_already_exists_is_not_a_failure() {
        let local = device(
            "pc",
            vec![note("n1", "本机写的", "2026-10-28T09:00:00.000+08:00")],
        );
        let fake = Fake::default();
        fake.on_get(Ok(Remote::Absent));
        fake.on_mkcol(Err(NetError::AlreadyExists));
        fake.on_mkcol(Ok(()));
        let trace = Trace::new();
        let mut env = trace.env();

        let out = run(DAV, &local, None, &fake, &mut env).unwrap();
        assert!(out.uploaded, "另一台设备先建过目录是常态");
    }

    #[test]
    fn mkcol_being_refused_stops_before_the_upload() {
        let local = device(
            "pc",
            vec![note("n1", "本机写的", "2026-10-28T09:00:00.000+08:00")],
        );
        let fake = Fake::default();
        fake.on_get(Ok(Remote::Absent));
        fake.on_mkcol(Err(NetError::Forbidden));
        let trace = Trace::new();
        let mut env = trace.env();

        let err = run(DAV, &local, None, &fake, &mut env).unwrap_err();
        assert_eq!(err, SyncError::Permission);
        assert!(err.pauses_auto());
        assert_eq!(fake.put_bodies().len(), 0, "建不了目录就别硬传");
    }

    #[test]
    fn an_illegal_endpoint_is_refused_before_the_first_request() {
        let fake = Fake::default();
        let trace = Trace::new();
        let mut env = trace.env();

        let err = run(
            "http://dav.jianguoyun.com/dav/",
            &Data::empty("pc"),
            None,
            &fake,
            &mut env,
        )
        .unwrap_err();
        assert_eq!(err, SyncError::Endpoint);
        assert!(
            fake.calls().is_empty(),
            "地址不合规矩时一个请求都不发（§4.2）"
        );
    }

    #[test]
    fn plain_http_is_only_for_a_loopback_stub_in_debug_builds() {
        let fake = Fake::default();
        let trace = Trace::new();
        let mut env = trace.env();
        env.allow_plain_http = true;

        let result = run(
            "http://127.0.0.1:3080/dav/",
            &Data::empty("pc"),
            None,
            &fake,
            &mut env,
        );
        assert_eq!(
            result.is_err(),
            !cfg!(debug_assertions),
            "本地明文桩只在调试构建里能用（§4.2）"
        );
    }

    #[test]
    fn https_endpoints_are_accepted_and_credentials_in_the_url_are_not() {
        assert!(endpoint_allowed(DAV, false));
        assert!(endpoint_allowed("https://dav.jianguoyun.com", false));
        assert!(endpoint_allowed("HTTPS://dav.jianguoyun.com/dav/", false));
        assert!(!endpoint_allowed("https://", false), "光有协议不算地址");
        assert!(
            !endpoint_allowed("https://user:pw@dav.jianguoyun.com/dav/", false),
            "密码只从 DPAPI 来，塞进 URL 会跟着日志与崩溃报告漏出去"
        );
        assert!(
            !endpoint_allowed("dav.jianguoyun.com/dav/", false),
            "没写协议"
        );
        assert!(!endpoint_allowed("ftp://dav.jianguoyun.com/dav/", false));
        assert!(
            !endpoint_allowed("http://dav.jianguoyun.com/dav/", true),
            "公网明文就算显式开关也不放行"
        );
    }

    #[test]
    fn failures_back_off_one_five_fifteen_minutes() {
        let ladder = (0..5).map(backoff_ms).collect::<Vec<_>>();
        assert_eq!(
            ladder,
            vec![0, 60_000, 300_000, 900_000, 900_000],
            "§4.3 的退避：1 → 5 → 15 分钟，之后一直是 15 分钟"
        );
    }

    #[test]
    fn auto_sync_waits_for_the_backoff_or_the_periodic_cap() {
        let healthy = SyncState::default();
        assert!(!healthy.auto_due(NOW, NOW + PERIODIC_MS as i64 - 1));
        assert!(healthy.auto_due(NOW, NOW + PERIODIC_MS as i64));

        let once = SyncState {
            consecutive_failures: 1,
            ..SyncState::default()
        };
        assert!(!once.auto_due(NOW, NOW + 30_000));
        assert!(once.auto_due(NOW, NOW + backoff_ms(1) as i64));

        let mut paused = SyncState {
            auto_paused: true,
            consecutive_failures: 1,
            ..SyncState::default()
        };
        assert!(!paused.auto_due(NOW, NOW + 10 * 60 * 60 * 1000));

        paused.resume();
        assert!(!paused.auto_paused);
        assert_eq!(paused.consecutive_failures, 0, "改完设置就该能马上重试");
        assert!(!paused.auto_due(NOW, NOW + backoff_ms(1) as i64));
        assert!(paused.auto_due(NOW, NOW + PERIODIC_MS as i64));
    }

    #[test]
    fn jitter_stays_between_half_and_two_seconds() {
        assert_eq!(jitter_ms(0.0), 500);
        assert_eq!(jitter_ms(1.0), 2_000);
        assert_eq!(jitter_ms(-1.0), 500, "超出区间按端点算");
        assert_eq!(jitter_ms(9.0), 2_000);
    }

    #[test]
    fn success_clears_the_failure_counters_and_stamps_the_time() {
        let local = device(
            "pc",
            vec![note("n1", "本机写的", "2026-10-28T09:00:00.000+08:00")],
        );
        let fake = Fake::default();
        fake.on_get(Ok(Remote::Absent));
        let trace = Trace::new();
        let mut env = trace.env();

        let mut state = SyncState {
            consecutive_failures: 3,
            auto_paused: true,
            etag: Some("\"STALE\"".to_string()),
            ..SyncState::default()
        };
        let out = run(DAV, &local, state.base.as_ref(), &fake, &mut env).unwrap();
        state.record_failure(&SyncError::Offline);
        assert_eq!(state.consecutive_failures, 4, "失败只会把退避继续推长");

        state = out.state;
        assert_eq!(state.consecutive_failures, 0, "一次成功就回到正常节奏");
        assert!(!state.auto_paused);
        assert_eq!(state.etag.as_deref(), Some("\"E-PUT\""), "旧 ETag 要换掉");
        assert_eq!(
            state.last_synced_at.as_deref(),
            Some(format_instant(NOW, OFFSET).as_str())
        );
    }

    #[test]
    fn two_devices_converge_after_editing_the_same_note() {
        // §4.5 那条改动要在这里才算验过：两台设备各算各的、各传各的，最后必须落到同一份内容。
        let shared = device(
            "pc",
            vec![note(
                "n1",
                "两端共同的原文",
                "2026-10-20T09:00:00.000+08:00",
            )],
        );
        let cloud = Cloud::holding(&shared, "\"V1\"");

        let pc = device(
            "pc",
            vec![note(
                "n1",
                "本机改过的正文",
                "2026-10-22T09:00:00.000+08:00",
            )],
        );
        let phone = device(
            "phone",
            vec![note(
                "n1",
                "手机上改过的正文",
                "2026-10-21T09:00:00.000+08:00",
            )],
        );

        let pc_trace = Trace::new();
        let mut env = pc_trace.env();
        let first = run(DAV, &pc, Some(&shared), &cloud.client(), &mut env).unwrap();
        assert!(first.uploaded, "本机那一份要先传上去，云端才知道有改动");
        assert_eq!(first.merged.notes.len(), 1, "云端还是原文，此刻谈不上并发");

        let phone_trace = Trace::new();
        let mut env = phone_trace.env();
        let second = run(DAV, &phone, Some(&shared), &cloud.client(), &mut env).unwrap();
        assert!(second.uploaded, "手机那份正文只能靠副本留下");
        assert_eq!(second.merged.notes.len(), 2, "两份正文都不许丢");
        assert_eq!(
            second.merged.notes[0].text, "本机改过的正文",
            "较新的留原 id"
        );
        assert_eq!(second.merged.notes[1].conflict_of.as_deref(), Some("n1"));
        assert_eq!(
            fingerprint(&second.merged),
            fingerprint(&cloud.body()),
            "手机写完之后，它手上的与云端必须是同一份内容"
        );

        // 本机再来一次：云端已经有它那一份，加上手机那份副本，内容一致就该什么都不做。
        let mut env = pc_trace.env();
        let third = run(
            DAV,
            &first.merged,
            first.state.base.as_ref(),
            &cloud.client(),
            &mut env,
        )
        .unwrap();
        assert!(!third.uploaded, "合完与云端一致，不许反复上传");
        assert_eq!(third.rounds, 1);
        assert_eq!(cloud.etag(), "\"V3\"", "没有新内容就不该动云端");
        assert_eq!(third.merged.notes.len(), 2, "副本要落到本机来");
        assert_eq!(pc_trace.saved().len(), 1, "只把副本落一次");
        assert!(pc_trace.slept().is_empty(), "没有抢写就不该等待");
    }

    #[test]
    fn a_sync_that_keeps_failing_does_not_grow_a_copy_each_round() {
        // 条件写一直撞车时：一次同步内部的四轮、以及下次同步再进来的一轮，都不能把同一份正文
        // 积成第二份副本，也不能反复重写本地文件。
        let cloud = Cloud::refusing(
            &device(
                "pc",
                vec![note(
                    "n1",
                    "手机上改过的正文",
                    "2026-10-21T09:00:00.000+08:00",
                )],
            ),
            "\"V1\"",
        );
        let base = device(
            "pc",
            vec![note(
                "n1",
                "两端共同的原文",
                "2026-10-20T09:00:00.000+08:00",
            )],
        );
        let local = device(
            "phone",
            vec![note(
                "n1",
                "本机改过的正文",
                "2026-10-22T09:00:00.000+08:00",
            )],
        );

        let trace = Trace::new();
        let mut env = trace.env();
        let err = run(DAV, &local, Some(&base), &cloud.client(), &mut env).unwrap_err();
        assert_eq!(err, SyncError::Contended);
        assert_eq!(
            (cloud.fetches.get(), cloud.stores.get()),
            (4, 4),
            "1 轮初始 + 3 次重试就收手（§4.4 步骤 3e）"
        );
        assert_eq!(trace.slept().len(), 3);

        let saved = trace.saved();
        assert_eq!(saved.len(), 1, "同一份合并结果只落一次盘");
        assert_eq!(saved[0].notes.len(), 2, "两份正文都留下");
        assert_eq!(
            saved[0]
                .notes
                .iter()
                .filter(|n| n.conflict_of.is_some())
                .count(),
            1,
            "副本只该有一份"
        );

        // 下次同步：本地已经是上一轮的合并结果，再合一次内容不该再变。
        let mut env = trace.env();
        let err = run(DAV, &saved[0], Some(&base), &cloud.client(), &mut env).unwrap_err();
        assert_eq!(err, SyncError::Contended);
        assert_eq!(
            trace.saved().len(),
            1,
            "第二轮没有新东西，不该再写一遍 data.json"
        );
        assert_eq!(cloud.etag(), "\"V1\"", "云端从头到尾没被改动");
    }
}
