//! 同步协议里「两端必须一样」的部分：远端布局、条件写请求头、状态码归类、地址策略（DESIGN §4.2、§4.4）。
//!
//! 这里没有 socket——真正发请求的是宿主实现的 [`Transport`]（Windows 用 ureq，将来 Android 用 OkHttp）。
//! 分成这个文件和 [`engine`](super::engine) 是因为两边性质不同：本文件是 WebDAV 的词汇表，
//! 换服务端也不用改；[`engine`](crate::sync::engine) 是「什么时候 GET、要不要 PUT」的决策。
//!
//! 同步内容是**明文**（§4.1，用户明确决定）：这里只传完整 JSON 快照，不做任何客户端加密，
//! 保护只有两层——传输走 HTTPS，账号密码只在本机 DPAPI 里。

/// 快照在坚果云 DAV 根目录下的相对路径（§4.2）。协议就是这一个文件，不做增量日志。
pub const SNAPSHOT_PATH: &str = "Perch/sync/perch.json";

/// 首次同步按需创建的目录，父在前——`MKCOL` 不会自动建父级。
pub const COLLECTIONS: [&str; 2] = ["Perch", "Perch/sync"];

/// GET 的结果。「远端还没有这个文件」是正常状态（第一次同步），不是错误，所以单独一档。
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Remote {
    Absent,
    Found {
        body: String,
        /// 服务端给的 `ETag`。不给就为 None，条件写退化成裸 PUT（§4.4 末条）。
        etag: Option<String>,
    },
}

/// PUT 带哪个条件头（§4.4 步骤 3d、4）。
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Condition {
    /// 第一次创建：远端必须还不存在（`If-None-Match: *`）。
    Absent,
    /// 覆盖：远端必须还是我读到的那一份（`If-Match: <etag>`）。
    IfMatch(String),
    /// 不带条件头。只在服务端不认 ETag 时用，接受极小概率竞态（§4.4 末条：下次合并修复）。
    Any,
}

impl Condition {
    /// 条件头叫什么。`Any` 不带请求头。
    pub fn header(&self) -> Option<&'static str> {
        match self {
            Condition::Absent => Some("If-None-Match"),
            Condition::IfMatch(_) => Some("If-Match"),
            Condition::Any => None,
        }
    }

    pub fn header_value(&self) -> Option<&str> {
        match self {
            Condition::Absent => Some("*"),
            Condition::IfMatch(etag) => Some(etag),
            Condition::Any => None,
        }
    }
}

/// 一次 HTTP 交换的失败。这里只有「状态码 + 传输层毛病」，不带正文：
/// 远端文件里是日记，任何错误消息都不该把正文捎带出去（§4.7）。
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum NetError {
    /// 连不上：DNS、超时、拒绝、断网。本地优先，应用照常可用（§4.1）。
    Unreachable,
    /// 证书或协议不合 HTTPS（§4.2）。绝不退化成明文重发。
    Tls,
    /// 401：应用专用密码不对。
    Unauthorized,
    /// 403：账号能登录但对这个目录没权限，或坚果云超额之后的拒绝。
    Forbidden,
    /// 404 出现在写路径上：该在的目录不在了。
    NotFound,
    /// 412/409：读完之后被另一台设备抢先写（§4.4 步骤 3e 走重取重合）。
    Conflict,
    /// 405/412/501/513：服务端不认条件头，改用裸 PUT（§4.4 末条）。
    PreconditionUnsupported,
    /// 507：坚果云空间不足。
    InsufficientStorage,
    /// MKCOL 说「已经存在」。对建目录来说是成功，由调用方按幂等处理。
    AlreadyExists,
    /// 其它状态码，`u16` 原样带着，界面上能说「服务器返回 5xx」。
    Other(u16),
}

impl NetError {
    /// GET 的状态码归类。404 由宿主直接表达成 [`Remote::Absent`]，不会走到这里。
    pub fn for_get(status: u16) -> NetError {
        match status {
            401 => NetError::Unauthorized,
            403 => NetError::Forbidden,
            507 => NetError::InsufficientStorage,
            other => NetError::Other(other),
        }
    }

    /// PUT 的状态码归类。412 与 409 都算被抢先写：坚果云用哪个没有定论，两个都走重试最省。
    pub fn for_put(status: u16) -> NetError {
        match status {
            401 => NetError::Unauthorized,
            403 => NetError::Forbidden,
            404 => NetError::NotFound,
            409 | 412 => NetError::Conflict,
            405 | 411 | 501 | 513 => NetError::PreconditionUnsupported,
            507 => NetError::InsufficientStorage,
            other => NetError::Other(other),
        }
    }

    /// MKCOL 的状态码归类。405/423 与 409 在 WebDAV 里既可能是「已存在」也可能是「父级缺失」，
    /// 这里统一当已存在：本函数只在按需建目录时调用，父级缺失下一次 PUT 会用 404/409 报出来。
    pub fn for_mkcol(status: u16) -> NetError {
        match status {
            401 => NetError::Unauthorized,
            403 => NetError::Forbidden,
            405 | 409 | 423 => NetError::AlreadyExists,
            507 => NetError::InsufficientStorage,
            other => NetError::Other(other),
        }
    }

    /// MKCOL 用：2xx 是建成，[`NetError::AlreadyExists`] 是本来就在，两者都算成功。
    pub fn mkcol_outcome(status: u16) -> Result<(), NetError> {
        match status {
            200..=299 => Ok(()),
            other => Err(NetError::for_mkcol(other)),
        }
    }
}

/// 网络侧能力。实现者只负责把一次请求发出去、把状态码归好类，不做任何重试与合并决策。
pub trait Transport {
    /// GET 快照。文件不存在要返回 [`Remote::Absent`]，不要当成错误。
    fn fetch(&self, path: &str) -> Result<Remote, NetError>;

    /// PUT 快照，带上 `cond` 对应的条件头。成功时把响应里的新 ETag 带回来（服务端不给就 None）。
    fn store(&self, path: &str, body: &str, cond: &Condition) -> Result<Option<String>, NetError>;

    /// MKCOL 建目录。已存在返回 [`Err(NetError::AlreadyExists)`]，调用方按幂等处理。
    fn make_collection(&self, path: &str) -> Result<(), NetError>;
}

/// §4.2「只接受 `https://`」。
///
/// `allow_plain_http` 是给本地 WebDAV 桩的显式开关，它只在调试构建里生效，而且只认回环地址——
/// 手滑把服务端填成公网 `http://` 不会被放行，正式构建里填什么都没用。
pub fn endpoint_allowed(endpoint: &str, allow_plain_http: bool) -> bool {
    let Some((scheme, host)) = split_endpoint(endpoint) else {
        return false;
    };
    if scheme.eq_ignore_ascii_case("https") {
        return !host.is_empty();
    }
    scheme.eq_ignore_ascii_case("http")
        && allow_plain_http
        && cfg!(debug_assertions)
        && is_loopback(host)
}

/// 拆出 scheme 与 host（含端口）。带 `user:pass@` 的一律不收：
/// 密码只从本机 DPAPI 来（§4.2），塞进 URL 会跟着崩溃报告和日志一起漏出去。
fn split_endpoint(endpoint: &str) -> Option<(&str, &str)> {
    let (scheme, rest) = endpoint.split_once("://")?;
    let authority = rest.split('/').next()?;
    if authority.is_empty() || authority.contains('@') {
        return None;
    }
    Some((scheme, authority))
}

/// 回环地址：`localhost`、`127.0.0.0/8`、`::1`。本地桩服务器只会用这几个。
fn is_loopback(authority: &str) -> bool {
    let host = authority.trim_matches(|c| c == '[' || c == ']');
    if host == "localhost" || host == "::1" {
        return true;
    }
    // IPv4 带端口时先切掉 `:端口`，再确认是 127.开头且后面只剩数字与点。
    let ip = host.split(':').next().unwrap_or(host);
    ip.strip_prefix("127.").is_some_and(|rest| {
        !rest.is_empty() && rest.bytes().all(|b| b.is_ascii_digit() || b == b'.')
    })
}
