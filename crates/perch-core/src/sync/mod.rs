//! 同步（DESIGN §4）。V1 是「一次一份完整快照」，没有增量日志也没有服务端合并。
//!
//! 拆成两个文件，性质不同：
//! - [`webdav`]：WebDAV 的词汇表——远端布局、条件写请求头、状态码归类、地址规矩。换语言不用改。
//! - [`engine`]：一次同步的决策——什么时候 GET、要不要 PUT、用什么条件头、失败算哪一类。
//!
//! 两个文件都不碰网络也不碰磁盘：宿主实现 [`webdav::Transport`]、给出落盘回调和此刻的时间，
//! 于是这套决策在 Windows、在测试桩里、在将来的 Android 上都是同一段代码（§5.4）。
//!
//! 凭据（坚果云应用专用密码）不在这里：它只在本机 DPAPI 里，由 shell 侧解密后交给传输实现，
//! 不参与同步、不写日志、不导出（§4.1）。

pub mod engine;
pub mod webdav;

pub use engine::{
    backoff_ms, jitter_ms, run, Env, Outcome, SyncError, SyncState, MAX_RETRIES, PERIODIC_MS,
    TRIGGER_GAP_MS,
};
pub use webdav::{
    endpoint_allowed, Condition, NetError, Remote, Transport, COLLECTIONS, SNAPSHOT_PATH,
};
