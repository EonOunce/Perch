//! ureq 版 [`Transport`]（DESIGN §4.2、§4.4）：只把 WebDAV 请求发出去、把状态码归类。
//!
//! 重试、合并、什么时候 PUT 全在 `perch_core::sync::engine` 里；这里不做任何决策，
//! 也不碰磁盘。TLS 用 Windows 自带的 schannel（ureq 的 `native-tls`），不往安装包里
//! 再塞一套 TLS 栈；明文 HTTP 只可能出现在调试构建的本地桩里（`endpoint_allowed` 已经把门）。

use std::time::Duration;

use base64::Engine;
use perch_core::sync::{Condition, NetError, Remote, Transport};
use ureq::http::Response;
use ureq::Body;

/// 一次请求的总预算。坚果云慢的时候 30 秒够传完一份几年的快照。
const TIMEOUT_GLOBAL: Duration = Duration::from_secs(30);
const TIMEOUT_CONNECT: Duration = Duration::from_secs(10);

/// 快照体积上限。§4.6 估算年增 2–3 MB，64 MB 留足了余地；
/// 真正要防的是服务端返回一个无穷流把内存吃光。
const MAX_BODY_BYTES: u64 = 64 * 1024 * 1024;

/// 一个账号 + 地址对应的 WebDAV 客户端。每次同步现建一个，连接池不跨同步保留。
pub struct Webdav {
    agent: ureq::Agent,
    /// 去掉末尾斜杠的 DAV 根地址；请求路径由 `perch-core` 给（`Perch/sync/perch.json`）。
    base: String,
    /// `Basic base64(账号:密码)`。只在内存里，随本次同步的生命周期消失。
    auth: String,
}

impl Webdav {
    /// `allow_plain_http` 只由调试构建的本地桩打开；正式构建里传 false，
    /// 即使有人绕过 `perch-core` 的地址检查，TLS 这一层也会拒绝明文。
    pub fn new(
        endpoint: &str,
        account: &str,
        password: &str,
        allow_plain_http: bool,
    ) -> Result<Webdav, String> {
        let base = endpoint.trim_end_matches('/').to_string();
        if base.is_empty() {
            return Err("同步地址是空的".to_string());
        }
        let agent: ureq::Agent = ureq::Agent::config_builder()
            .timeout_global(Some(TIMEOUT_GLOBAL))
            .timeout_connect(Some(TIMEOUT_CONNECT))
            .https_only(!allow_plain_http)
            .http_status_as_error(true)
            // MKCOL 不在 ureq 认识的 HTTP 方法表里，默认会被当成拼错的请求拒发。
            .allow_non_standard_methods(true)
            // ureq 的默认 provider 仍是 rustls；只开 native-tls feature 不选 provider，
            // 第一次 https 请求就会 panic（本地 http 桩测不到这一层）。
            .tls_config(
                ureq::tls::TlsConfig::builder()
                    .provider(ureq::tls::TlsProvider::NativeTls)
                    .build(),
            )
            .build()
            .into();
        Ok(Webdav {
            agent,
            base,
            auth: basic_auth(account, password),
        })
    }

    fn url(&self, path: &str) -> String {
        format!("{}/{}", self.base, path.trim_start_matches('/'))
    }
}

/// Basic 认证的请求头值。账号里带冒号是 RFC 7617 不允许的，这里不检查——
/// 坚果云账号是邮箱，填错了服务端会用 401 说话。
fn basic_auth(account: &str, password: &str) -> String {
    let raw = format!("{account}:{password}");
    format!(
        "Basic {}",
        base64::engine::general_purpose::STANDARD.encode(raw)
    )
}

impl Transport for Webdav {
    fn fetch(&self, path: &str) -> Result<Remote, NetError> {
        let url = self.url(path);
        let sent = self
            .agent
            .get(&url)
            .header("Authorization", self.auth.as_str())
            .call();
        match sent {
            Ok(response) => read_remote(response),
            // 404 是「远端还没有这份快照」，第一次同步的正常起点（§4.4 步骤 2）。
            Err(ureq::Error::StatusCode(404)) => Ok(Remote::Absent),
            Err(ureq::Error::StatusCode(code)) => Err(NetError::for_get(code)),
            Err(error) => Err(net_error(error)),
        }
    }

    fn store(&self, path: &str, body: &str, cond: &Condition) -> Result<Option<String>, NetError> {
        let url = self.url(path);
        let mut request = self
            .agent
            .put(&url)
            .header("Authorization", self.auth.as_str());
        if let (Some(name), Some(value)) = (cond.header(), cond.header_value()) {
            request = request.header(name, value);
        }
        match request.send(body) {
            Ok(response) => Ok(etag_of(&response)),
            Err(ureq::Error::StatusCode(code)) => Err(NetError::for_put(code)),
            Err(error) => Err(net_error(error)),
        }
    }

    fn make_collection(&self, path: &str) -> Result<(), NetError> {
        let url = self.url(path);
        // ureq 没有 MKCOL 的快捷方法：用它的 http 层拼一个不带正文的请求。
        let request = ureq::http::Request::builder()
            .method("MKCOL")
            .uri(url.as_str())
            .header("Authorization", self.auth.as_str())
            .body(())
            .map_err(|_| NetError::Unreachable)?;
        match self.agent.run(request) {
            Ok(response) => NetError::mkcol_outcome(response.status().as_u16()),
            Err(ureq::Error::StatusCode(code)) => NetError::mkcol_outcome(code),
            Err(error) => Err(net_error(error)),
        }
    }
}

fn read_remote(mut response: Response<Body>) -> Result<Remote, NetError> {
    let etag = etag_of(&response);
    let body = response
        .body_mut()
        .with_config()
        .limit(MAX_BODY_BYTES)
        .read_to_string()
        .map_err(|_| NetError::Unreachable)?;
    Ok(Remote::Found { body, etag })
}

fn etag_of(response: &Response<Body>) -> Option<String> {
    response
        .headers()
        .get("etag")
        .and_then(|value| value.to_str().ok())
        .map(str::to_string)
}

/// 传输层毛病归成两类：证书/协议不合是 `Tls`（绝不退化成明文重发），
/// 其余（DNS、拒绝、超时、代理胡来）都是「现在连不上」，稍后再试。
fn net_error(error: ureq::Error) -> NetError {
    match error {
        ureq::Error::StatusCode(code) => NetError::Other(code),
        // ureq 的 TLS 配置错误与 native-tls 的握手失败（证书链、协议）是同一类：
        // 连接不可信，不能退化成明文重发。底层 IO 与超时已被 ureq 单独保留，不会落到这里。
        ureq::Error::Tls(_) | ureq::Error::NativeTls(_) => NetError::Tls,
        _ => NetError::Unreachable,
    }
}

#[cfg(test)]
mod stub {
    //! 本地 WebDAV 桩（AGENTS.md：自动测试不碰真实网络，同步用本地桩）。
    //! 只实现同步真正会用的三种方法，带 ETag 条件语义——够把 ureq + 引擎整条链路跑通。

    use std::io::{BufRead, BufReader, Read, Write};
    use std::net::{TcpListener, TcpStream};
    use std::sync::{Arc, Mutex};

    #[derive(Default)]
    pub struct Cloud {
        pub snapshot: Option<String>,
        pub etag: Option<String>,
        pub etag_seq: u32,
        pub collections: Vec<String>,
        pub requests: Vec<(String, String)>,
        /// 下一次 PUT 直接回这个状态码（模拟被抢先写），只生效一次。
        pub refuse_put_once: Option<u16>,
    }

    pub struct Stub {
        pub base: String,
        pub cloud: Arc<Mutex<Cloud>>,
    }

    impl Stub {
        pub fn start(authorization: String) -> Stub {
            let listener = TcpListener::bind("127.0.0.1:0").expect("绑定本地桩端口");
            let port = listener.local_addr().unwrap().port();
            let cloud = Arc::new(Mutex::new(Cloud::default()));
            let shared = cloud.clone();
            std::thread::spawn(move || {
                for stream in listener.incoming().flatten() {
                    serve(stream, &shared, &authorization);
                }
            });
            Stub {
                base: format!("http://127.0.0.1:{port}/dav"),
                cloud,
            }
        }

        pub fn body(&self) -> Option<String> {
            self.cloud.lock().unwrap().snapshot.clone()
        }

        pub fn etag(&self) -> Option<String> {
            self.cloud.lock().unwrap().etag.clone()
        }

        pub fn requests(&self) -> Vec<(String, String)> {
            self.cloud.lock().unwrap().requests.clone()
        }

        pub fn count(&self, method: &str) -> usize {
            self.cloud
                .lock()
                .unwrap()
                .requests
                .iter()
                .filter(|(m, _)| m == method)
                .count()
        }

        /// 模拟另一台设备先写了一份。
        pub fn preload(&self, body: &str) {
            let mut cloud = self.cloud.lock().unwrap();
            cloud.etag_seq += 1;
            cloud.etag = Some(format!("\"V{}\"", cloud.etag_seq));
            cloud.snapshot = Some(body.to_string());
        }

        pub fn refuse_next_put(&self, status: u16) {
            self.cloud.lock().unwrap().refuse_put_once = Some(status);
        }
    }

    fn serve(mut stream: TcpStream, cloud: &Arc<Mutex<Cloud>>, authorization: &str) {
        let Some((method, path, headers, body)) = read_request(&mut stream) else {
            return;
        };
        let mut cloud = cloud.lock().unwrap();
        cloud.requests.push((method.clone(), path.clone()));

        let auth_ok = headers
            .iter()
            .any(|(name, value)| name == "authorization" && value == authorization);
        if !auth_ok {
            return respond(&mut stream, 401, None, "");
        }

        let header = |name: &str| {
            headers
                .iter()
                .find(|(key, _)| key == name)
                .map(|(_, value)| value.clone())
        };

        match method.as_str() {
            "GET" => match (&cloud.snapshot, &cloud.etag) {
                (Some(body), etag) => respond(&mut stream, 200, etag.as_deref(), body),
                _ => respond(&mut stream, 404, None, ""),
            },
            "MKCOL" => {
                if cloud.collections.iter().any(|dir| dir == &path) {
                    respond(&mut stream, 405, None, "");
                } else {
                    cloud.collections.push(path.clone());
                    respond(&mut stream, 201, None, "");
                }
            }
            "PUT" => {
                if let Some(status) = cloud.refuse_put_once.take() {
                    return respond(&mut stream, status, None, "");
                }
                if let Some(if_match) = header("if-match") {
                    if cloud.etag.as_deref() != Some(if_match.as_str()) {
                        return respond(&mut stream, 412, None, "");
                    }
                }
                if header("if-none-match").as_deref() == Some("*") && cloud.snapshot.is_some() {
                    return respond(&mut stream, 412, None, "");
                }
                cloud.etag_seq += 1;
                let etag = format!("\"V{}\"", cloud.etag_seq);
                cloud.etag = Some(etag.clone());
                cloud.snapshot = Some(body);
                respond(&mut stream, 201, Some(&etag), "");
            }
            _ => respond(&mut stream, 405, None, ""),
        }
    }

    /// 一条请求拆出来的四样：方法、路径、小写头、正文。
    type Wire = (String, String, Vec<(String, String)>, String);

    fn read_request(stream: &mut TcpStream) -> Option<Wire> {
        let mut reader = BufReader::new(stream.try_clone().ok()?);
        let mut line = String::new();
        reader.read_line(&mut line).ok()?;
        let mut parts = line.split_whitespace();
        let method = parts.next()?.to_string();
        let path = parts.next()?.to_string();
        let mut headers = Vec::new();
        let mut length = 0usize;
        loop {
            let mut head = String::new();
            if reader.read_line(&mut head).ok()? == 0 {
                break;
            }
            let head = head.trim_end();
            if head.is_empty() {
                break;
            }
            let Some((name, value)) = head.split_once(':') else {
                continue;
            };
            let name = name.trim().to_ascii_lowercase();
            let value = value.trim().to_string();
            if name == "content-length" {
                length = value.parse().unwrap_or(0);
            }
            headers.push((name, value));
        }
        let mut body = vec![0u8; length];
        reader.read_exact(&mut body).ok()?;
        Some((
            method,
            path,
            headers,
            String::from_utf8_lossy(&body).into_owned(),
        ))
    }

    fn respond(stream: &mut TcpStream, status: u16, etag: Option<&str>, body: &str) {
        let reason = match status {
            200 => "OK",
            201 => "Created",
            204 => "No Content",
            401 => "Unauthorized",
            404 => "Not Found",
            405 => "Method Not Allowed",
            412 => "Precondition Failed",
            _ => "Error",
        };
        let mut head = format!(
            "HTTP/1.1 {status} {reason}\r\nContent-Length: {}\r\nConnection: close\r\n",
            body.len()
        );
        if let Some(etag) = etag {
            head.push_str(&format!("ETag: {etag}\r\n"));
        }
        head.push_str("\r\n");
        let _ = stream.write_all(head.as_bytes());
        let _ = stream.write_all(body.as_bytes());
        let _ = stream.flush();
    }
}

#[cfg(test)]
mod tests {
    use super::stub::Stub;
    use super::Webdav;
    use base64::Engine;
    use perch_core::merge::fingerprint;
    use perch_core::model::{CheckinStatus, Data, Note};
    use perch_core::sync::{run, Condition, Env, Remote, SyncError, SyncState, Transport};

    const ACCOUNT: &str = "someone@example.com";
    const PASSWORD: &str = "app-specific-password";
    const OFFSET: i32 = 8 * 3600;
    const NOW: i64 = 1_793_160_000_000;

    fn auth_header() -> String {
        let raw = format!("{ACCOUNT}:{PASSWORD}");
        format!(
            "Basic {}",
            base64::engine::general_purpose::STANDARD.encode(raw)
        )
    }

    fn client(stub: &Stub, password: &str) -> Webdav {
        Webdav::new(&stub.base, ACCOUNT, password, true).expect("建客户端")
    }

    /// ureq 默认 provider 仍是 rustls；只开 native-tls feature 而不选 provider，
    /// 第一次 https 请求会 panic。本地 http 桩走不到这一层，所以在这里盯住。
    #[test]
    fn https_uses_the_windows_tls_stack() {
        let client =
            Webdav::new("https://dav.jianguoyun.com/dav", ACCOUNT, PASSWORD, false).unwrap();
        assert_eq!(
            client.agent.config().tls_config().provider(),
            ureq::tls::TlsProvider::NativeTls
        );
    }

    fn note(id: &str, text: &str) -> Note {
        Note {
            id: id.to_string(),
            created_at: "2026-10-20T09:00:00.000+08:00".to_string(),
            local_date: "2026-10-20".to_string(),
            text: text.to_string(),
            tags: vec![],
            updated_at: "2026-10-20T09:00:00.000+08:00".to_string(),
            conflict_of: None,
        }
    }

    fn local_with(id: &str, text: &str) -> Data {
        let mut data = Data::empty("pc-windows");
        data.notes.push(note(id, text));
        data
    }

    /// 一次同步的宿主侧：记录落盘回调、不真睡、抖动取固定值。
    fn run_once(
        stub: &Stub,
        local: &Data,
        base: Option<&Data>,
        password: &str,
        saves: &mut Vec<Data>,
    ) -> Result<perch_core::sync::Outcome, SyncError> {
        let transport = client(stub, password);
        let saved = std::cell::RefCell::new(saves);
        let mut env = Env {
            now_ms: NOW,
            offset_secs: OFFSET,
            allow_plain_http: true,
            save_local: Box::new(|data: &Data| {
                saved.borrow_mut().push(data.clone());
                true
            }),
            sleep: Box::new(|_ms: u64| {}),
            random: Box::new(|| 0.0),
        };
        run(&stub.base, local, base, &transport, &mut env)
    }

    #[test]
    fn a_first_sync_creates_the_folders_and_uploads_the_snapshot() {
        let stub = Stub::start(auth_header());
        let local = local_with("n1", "第一条");
        let mut saves = Vec::new();
        let outcome = run_once(&stub, &local, None, PASSWORD, &mut saves).expect("首同步应当成功");

        assert!(outcome.uploaded);
        assert_eq!(
            stub.requests()
                .iter()
                .map(|(m, _)| m.as_str())
                .collect::<Vec<_>>(),
            ["GET", "MKCOL", "MKCOL", "PUT"],
            "先看远端有没有、再按父子顺序建目录、最后才写"
        );
        assert_eq!(
            stub.requests()[2].1,
            "/dav/Perch/sync",
            "MKCOL 的父目录必须先建"
        );
        let uploaded = stub.body().expect("云上应当有快照");
        let parsed: Data = serde_json::from_str(&uploaded).unwrap();
        assert_eq!(parsed.notes[0].text, "第一条");
        assert!(parsed.generated_at.is_some(), "宿主在落盘/上传前盖时间戳");
        // 上传的 ETag 必须带回来，下一次覆盖才有条件可用。
        assert_eq!(outcome.state.etag, stub.etag());
        assert!(outcome.state.etag.is_some());
    }

    #[test]
    fn a_second_sync_with_no_changes_does_not_put_again() {
        let stub = Stub::start(auth_header());
        let local = local_with("n1", "第一条");
        let mut saves = Vec::new();
        let first = run_once(&stub, &local, None, PASSWORD, &mut saves).unwrap();

        let base = first.state.base.clone();
        let puts_before = stub.count("PUT");
        let second = run_once(&stub, &local, base.as_ref(), PASSWORD, &mut saves).unwrap();

        assert!(!second.uploaded, "内容没变就不该再传一份");
        assert_eq!(stub.count("PUT"), puts_before);
        assert!(
            saves.is_empty(),
            "合并结果与本地一致时不该重写 data.json（免得白转一份备份）"
        );
        assert_eq!(
            fingerprint(&second.merged),
            fingerprint(base.as_ref().unwrap())
        );
    }

    #[test]
    fn a_wrong_password_is_reported_as_a_credentials_error() {
        let stub = Stub::start(auth_header());
        let local = local_with("n1", "第一条");
        let error = run_once(&stub, &local, None, "wrong-password", &mut Vec::new()).unwrap_err();
        assert_eq!(error, SyncError::Credentials);
        assert!(
            error.pauses_auto(),
            "密码错了要停下自动同步，别拿错误密码反复敲门"
        );
        assert_eq!(stub.count("PUT"), 0, "401 之后一个字节都不该写");
    }

    #[test]
    fn a_remote_that_changed_underneath_is_merged_and_retried() {
        let stub = Stub::start(auth_header());
        // 另一台设备已经写过一份：本地第一次 PUT 会被 412 挡回来。
        stub.preload(&serde_json::to_string(&local_with("n2", "手机写的")).unwrap());
        stub.refuse_next_put(412);
        // 第一次 GET 看到的是「手机写的」，合并后 PUT 被拒；第二轮重取、重合、再 PUT。
        let local = local_with("n1", "本机写的");
        let mut saves = Vec::new();
        let outcome =
            run_once(&stub, &local, None, PASSWORD, &mut saves).expect("第二轮应当写成功");

        assert_eq!(outcome.rounds, 2, "被抢先写之后要重取一轮");
        let uploaded: Data = serde_json::from_str(&stub.body().unwrap()).unwrap();
        let texts: Vec<&str> = uploaded.notes.iter().map(|n| n.text.as_str()).collect();
        assert!(texts.contains(&"本机写的"));
        assert!(texts.contains(&"手机写的"), "另一台设备的正文不能丢");
        assert_eq!(stub.count("PUT"), 2);
        assert_eq!(saves.len(), 1, "合并结果与本地不同，落盘一次就够");
        assert_eq!(saves[0].notes.len(), 2);
    }

    #[test]
    fn a_tls_only_transport_refuses_plain_http_even_if_the_engine_let_it_through() {
        let transport = Webdav::new("http://127.0.0.1:9/dav", ACCOUNT, PASSWORD, false).unwrap();
        // 引擎层会先挡掉这种地址；这里直接问传输层，确认第二道门也在。
        assert_eq!(
            transport.fetch("Perch/sync/perch.json").unwrap_err(),
            perch_core::sync::NetError::Unreachable
        );
    }

    #[test]
    fn the_snapshot_body_is_read_back_verbatim() {
        let stub = Stub::start(auth_header());
        let body = "{\"schemaVersion\":1}";
        stub.preload(body);
        let transport = client(&stub, PASSWORD);
        match transport.fetch("Perch/sync/perch.json").unwrap() {
            Remote::Found { body: got, etag } => {
                assert_eq!(got, body);
                assert!(etag.is_some());
            }
            Remote::Absent => panic!("桩里明明有一份"),
        }
    }

    #[test]
    fn the_conditional_headers_reach_the_server() {
        let stub = Stub::start(auth_header());
        let transport = client(&stub, PASSWORD);
        // 远端还没有 → If-None-Match: * 建立。
        transport
            .store("Perch/sync/perch.json", "{}", &Condition::Absent)
            .unwrap();
        let etag = stub.etag().unwrap();
        // 用刚拿到的 ETag 覆盖成功；用旧 ETag 会被 412 挡回。
        transport
            .store(
                "Perch/sync/perch.json",
                "{\"a\":1}",
                &Condition::IfMatch(etag.clone()),
            )
            .unwrap();
        assert_eq!(
            transport
                .store(
                    "Perch/sync/perch.json",
                    "{\"b\":2}",
                    &Condition::IfMatch(etag)
                )
                .unwrap_err(),
            perch_core::sync::NetError::Conflict
        );
        assert_eq!(
            stub.body().as_deref(),
            Some("{\"a\":1}"),
            "被拒的那次不该落盘"
        );
    }

    #[test]
    fn mkcol_says_already_exists_the_second_time() {
        let stub = Stub::start(auth_header());
        let transport = client(&stub, PASSWORD);
        transport.make_collection("Perch").unwrap();
        assert_eq!(
            transport.make_collection("Perch").unwrap_err(),
            perch_core::sync::NetError::AlreadyExists
        );
    }

    #[test]
    fn the_state_survives_a_json_round_trip() {
        // 宿主要把 SyncState 落盘；字段名是跨设备/跨版本约定的一部分，序列化坏了下一次
        // 同步就会失去基线（保守合并会把同一段正文收成副本），所以钉住它。
        let state = SyncState {
            etag: Some("\"V3\"".to_string()),
            base: Some(local_with("n1", "基线")),
            last_synced_at: Some("2026-10-28T12:00:00.000+08:00".to_string()),
            consecutive_failures: 2,
            auto_paused: false,
        };
        let json = serde_json::to_string(&state).unwrap();
        let back: SyncState = serde_json::from_str(&json).unwrap();
        assert_eq!(back, state);
        assert!(json.contains("\"lastSyncedAt\""));
        assert!(json.contains("\"consecutiveFailures\""));
        assert!(json.contains("\"autoPaused\""));
    }

    #[test]
    fn a_checkin_entity_round_trips_through_the_server() {
        // 不只是碎碎念：打卡事件是同步里最容易出「字段丢了」的一类。
        let stub = Stub::start(auth_header());
        let mut local = Data::empty("pc-windows");
        local.checkin_events.push(perch_core::model::CheckinEvent {
            id: "e1".to_string(),
            habit_id: "h1".to_string(),
            local_date: "2026-10-20".to_string(),
            status: CheckinStatus::Done,
            confirmed_at: Some("2026-10-20T09:00:00.000+08:00".to_string()),
            source: "manual".to_string(),
            supersedes_id: None,
            recorded_by: "pc-windows".to_string(),
            note: String::new(),
            created_at: "2026-10-20T09:00:00.000+08:00".to_string(),
        });
        let mut saves = Vec::new();
        run_once(&stub, &local, None, PASSWORD, &mut saves).unwrap();
        let uploaded: Data = serde_json::from_str(&stub.body().unwrap()).unwrap();
        assert_eq!(uploaded.checkin_events, local.checkin_events);
    }
}
