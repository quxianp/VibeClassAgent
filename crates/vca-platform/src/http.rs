//! HTTP 客户端：`ureq` + `rustls` 的薄封装。
//!
//! # 为什么从自研 WinHTTP 换成 ureq
//!
//! 原实现直接用 WinHTTP 的 FFI（零第三方依赖），但有三个绕不过去的短板：
//!
//! 1. **TLS 走系统 schannel**，在本机被沙箱阻断（`SEC_E_NO_CREDENTIALS`），
//!    而 rustls 是纯 Rust 实现，不依赖系统凭据存储；
//! 2. **不支持代理**，受限网络下（本项目开发机就是）完全没法用；
//! 3. 没有连接池、没有重定向、没有分块传输，每个新需求都要手写一遍 FFI。
//!
//! ureq 是同步阻塞式客户端，不引入 async 运行时，体积与复杂度都可控，
//! 与本项目「后台定时任务」的使用形态正好匹配。
//!
//! # 对外契约保持不变
//!
//! [`request`] / [`post_json`] / [`post_form`] / [`post_multipart`] / [`urlencode`]
//! 的签名与语义与替换前一致，因此 `llm` 与 `push` 两个调用方无需改动。
//!
//! # 代理
//!
//! 代理是可选的，按 `VCA_PROXY` → `HTTPS_PROXY` → `https_proxy` 的顺序取，
//! 也可以用 [`set_proxy`] 在运行时覆盖（CLI 的 `config` 命令会用）。
//! 生产环境（一体机）通常直连，什么都不用配。

use std::collections::HashMap;
use std::path::Path;
use std::sync::{Mutex, OnceLock};
use std::time::Duration;

use ureq::Agent;

/// HTTP 响应。
#[derive(Debug, Clone)]
pub struct HttpResponse {
    /// 状态码。
    pub status: u32,
    /// 响应体（按 UTF-8 尽力解码）。
    pub body: String,
}

impl HttpResponse {
    /// 是否 2xx。
    pub fn is_success(&self) -> bool {
        (200..300).contains(&self.status)
    }
}

/// HTTP 请求失败的原因。
///
/// 注意：**4xx/5xx 不是错误**，它们以正常的 [`HttpResponse`] 返回，
/// 由调用方根据 `status` 决定怎么办（比如 401 说明 API Key 不对）。
/// 这里的错误只涵盖「请求根本没发出去或没收到响应」。
#[derive(Debug, thiserror::Error)]
pub enum HttpError {
    /// 地址无法解析。
    #[error("URL 解析失败: {0}")]
    BadUrl(String),
    /// 不支持的 HTTP 方法。
    #[error("不支持的 HTTP 方法: {0}")]
    Unsupported(String),
    /// 网络层失败（连不上、超时、TLS 握手失败等）。
    #[error("网络错误: {0}")]
    Network(String),
}

// ---------------------------------------------------------------------------
// 代理
// ---------------------------------------------------------------------------

static PROXY: OnceLock<Mutex<Option<String>>> = OnceLock::new();

fn proxy_cell() -> &'static Mutex<Option<String>> {
    PROXY.get_or_init(|| {
        let from_env = std::env::var("VCA_PROXY")
            .ok()
            .or_else(|| std::env::var("HTTPS_PROXY").ok())
            .or_else(|| std::env::var("https_proxy").ok())
            .filter(|s| !s.trim().is_empty());
        Mutex::new(from_env)
    })
}

/// 设置全局代理（如 `http://127.0.0.1:10818`）。传 `None` 表示直连。
pub fn set_proxy(proxy: Option<String>) {
    if let Ok(mut g) = proxy_cell().lock() {
        *g = proxy.filter(|s| !s.trim().is_empty());
    }
    // 代理变了，已缓存的 Agent 必须作废，否则会继续走旧代理。
    if let Ok(mut c) = agents().lock() {
        c.clear();
    }
}

/// 当前生效的代理。
pub fn proxy() -> Option<String> {
    proxy_cell().lock().ok().and_then(|g| g.clone())
}

// ---------------------------------------------------------------------------
// Agent 缓存
// ---------------------------------------------------------------------------

type AgentCache = HashMap<u64, Agent>;

static AGENTS: OnceLock<Mutex<AgentCache>> = OnceLock::new();

fn agents() -> &'static Mutex<AgentCache> {
    AGENTS.get_or_init(|| Mutex::new(HashMap::new()))
}

/// 按超时分组缓存 Agent。
///
/// 缓存的意义是**复用连接池**：一次课后处理会连续打十几次同一个 API，
/// 每次重建 Agent 就等于每次重新做 TLS 握手。
fn agent_for(timeout_ms: i32) -> Result<Agent, HttpError> {
    let key = timeout_ms.max(1_000) as u64;
    if let Ok(c) = agents().lock() {
        if let Some(a) = c.get(&key) {
            return Ok(a.clone());
        }
    }

    let timeout = Duration::from_millis(key);
    let mut b = Agent::config_builder()
        .timeout_global(Some(timeout))
        // 关键：4xx/5xx 不要变成 Err。推送/转写需要读到错误 body
        // （比如企业微信会在 200 里塞 errcode，OpenAI 会在 401 里说明原因）。
        .http_status_as_error(false)
        .user_agent(concat!("VibeClassAgent/", env!("CARGO_PKG_VERSION")));

    if let Some(p) = proxy() {
        let px = ureq::Proxy::new(&p)
            .map_err(|e| HttpError::Network(format!("代理地址不合法（{p}）: {e}")))?;
        b = b.proxy(Some(px));
    }

    let agent = Agent::new_with_config(b.build());
    if let Ok(mut c) = agents().lock() {
        c.insert(key, agent.clone());
    }
    Ok(agent)
}

/// 把 `"K: V\r\nK2: V2\r\n"` 形式的头逐行加到请求上。
fn apply_headers<S>(rb: ureq::RequestBuilder<S>, headers: &str) -> ureq::RequestBuilder<S> {
    let mut rb = rb;
    for line in headers.lines() {
        let line = line.trim();
        if line.is_empty() {
            continue;
        }
        if let Some((k, v)) = line.split_once(':') {
            rb = rb.header(k.trim(), v.trim());
        }
    }
    rb
}

// ---------------------------------------------------------------------------
// 请求
// ---------------------------------------------------------------------------

/// 发送一次请求。
///
/// - `headers`：形如 `"Content-Type: application/json\r\n"`，可为空串；
/// - `body`：请求体字节（GET/HEAD 忽略）；
/// - `timeout_ms`：整体超时（连接 + 传输）。
pub fn request(
    method: &str,
    url: &str,
    headers: &str,
    body: &[u8],
    timeout_ms: i32,
) -> Result<HttpResponse, HttpError> {
    let m = method.trim().to_ascii_uppercase();
    // 先校验动词，再碰任何 IO / 全局配置：
    // 否则一个拼错的动词会被「代理没配好」之类的错误掩盖掉。
    if !matches!(
        m.as_str(),
        "GET" | "HEAD" | "DELETE" | "POST" | "PUT" | "PATCH"
    ) {
        return Err(HttpError::Unsupported(m));
    }

    let agent = agent_for(timeout_ms)?;

    // 各分支的返回类型必须一致：都是 Result<Response<Body>, ureq::Error>。
    let result = match m.as_str() {
        "GET" => apply_headers(agent.get(url), headers).call(),
        "HEAD" => apply_headers(agent.head(url), headers).call(),
        "DELETE" => apply_headers(agent.delete(url), headers).call(),
        "POST" => apply_headers(agent.post(url), headers).send(body),
        "PUT" => apply_headers(agent.put(url), headers).send(body),
        _ => apply_headers(agent.patch(url), headers).send(body),
    };

    match result {
        Ok(mut resp) => {
            let status = resp.status().as_u16() as u32;
            // 读失败（比如响应不是 UTF-8）不该让整个请求失败：
            // 状态码才是调用方真正要的判据。
            let body = resp.body_mut().read_to_string().unwrap_or_default();
            Ok(HttpResponse { status, body })
        }
        Err(e) => Err(HttpError::Network(e.to_string())),
    }
}

/// 一个地址的探测结果（给"测试镜像通不通"用）。
#[derive(Debug, Clone)]
pub struct ProbeInfo {
    /// HTTP 状态码（0 表示没拿到有效响应）。
    pub status: u32,
    /// 判定出来的类型：`文件` / `网页` / `不存在` / `需要授权` / `其他`。
    pub kind: &'static str,
    /// 响应里声明的大小（字节）；拿不到是 0。
    pub size: u64,
}

impl ProbeInfo {
    /// 一句给人看的结论。
    pub fn describe(&self) -> String {
        let size = if self.size > 0 {
            format!("，约 {} MB", self.size / 1048576)
        } else {
            String::new()
        };
        format!(
            "连通正常（HTTP {}，判定为{}{}）",
            self.status, self.kind, size
        )
    }
}

/// 轻量探测一个地址是否可用。
///
/// # 为什么只发 HEAD，拿不到再退化成极小范围 GET
///
/// 目的只是回答"这个地址通不通、是不是一个文件"。**绝不能下载整个文件** ——
/// 镜像测试的对象是几十到上百 MB 的安装包，真下完用户要等好几分钟，
/// 那这个"测试"按钮就没人会点第二次了。
///
/// HEAD 是最省的（只要响应头）。但有些自建镜像/CDN 不支持 HEAD
/// （返回 405 或直接断连），这时用 `Range: bytes=0-0` 只取 1 个字节 ——
/// 依然几乎不耗流量，但能拿到状态码和 `Content-Length`。
///
/// # 判据
///
/// - 2xx → 通过
/// - 206 → 通过（说明支持 Range，正是我们要的效果）
/// - 404 / 403 → **明确报错**，这类是"填错地址"最常见的表现，
///   必须和"网络不通"区分开：前者改地址就好，后者要查网络。
pub fn probe(url: &str, timeout_ms: i32) -> Result<ProbeInfo, HttpError> {
    let agent = agent_for(timeout_ms)?;

    // 先试 HEAD。ureq 自动跟随重定向；拿到的是最终响应。
    let head = agent.head(url).call();
    let resp = match head {
        Ok(r) if r.status().as_u16() != 405 && r.status().as_u16() != 501 => r,
        // HEAD 不被支持，或者网络层就失败了 —— 退化成 Range GET。
        // 这里只拿响应头，**不读取 body**：即使服务器无视 Range 返回 200 + 100MB，
        // drop 响应也不会把整包读进内存。这是“测试按钮”绝不能破坏的边界。
        _ => agent
            .get(url)
            .header("Range", "bytes=0-0")
            .call()
            .map_err(|e| HttpError::Network(e.to_string()))?,
    };

    let status = resp.status().as_u16() as u32;
    let kind = match status {
        200 | 206 => "文件",
        301 | 302 | 303 | 307 | 308 => "跳转",
        401 => "需要授权",
        403 => "拒绝访问",
        404 => "不存在",
        _ if (200..300).contains(&status) => "文件",
        _ => "其他",
    };

    // 4xx/5xx 在这里当成"探测失败"报出去：对用户来说这是同一个问题
    // （"这个地址不能用"），分成两种结果反而让人不知道该怎么办。
    if !(200..300).contains(&status) {
        return Err(HttpError::Network(format!(
            "服务器返回 HTTP {status}（{kind}）"
        )));
    }

    let headers = resp.headers();
    let content_type = headers
        .get("content-type")
        .and_then(|v| v.to_str().ok())
        .unwrap_or("")
        .to_ascii_lowercase();
    // 镜像必须是**文件直链**。校园网登录页/404 美化页经常返回 200 + HTML；
    // 只看状态码会把它误判为可用，真正下载后才因体积不足失败。
    if content_type.contains("text/html") || content_type.contains("application/xhtml+xml") {
        return Err(HttpError::BadUrl(
            "服务器返回的是网页，不是可直接下载的文件".to_string(),
        ));
    }

    let content_len = headers
        .get("content-length")
        .and_then(|v| v.to_str().ok())
        .and_then(|v| v.parse::<u64>().ok())
        .unwrap_or(0);
    // 206 的 Content-Length 往往只是 1（Range 取到的那个字节），
    // 真正文件大小在 `Content-Range: bytes 0-0/12345` 的斜杠后面。
    let range_total = headers
        .get("content-range")
        .and_then(|v| v.to_str().ok())
        .and_then(|v| v.rsplit_once('/').map(|(_, total)| total))
        .and_then(|v| v.parse::<u64>().ok())
        .unwrap_or(0);
    let size = range_total.max(content_len);

    Ok(ProbeInfo { status, kind, size })
}

/// 把响应体**流式**写入文件，返回写入的字节数。
///
/// 为什么不复用 [`request`]：那条路会把整个响应体读成一个 `String`。
/// 下载几十 MB 的安装包时，这既会撑爆内存（本项目的目标机器只有 8 GB），
/// 也会在遇到非 UTF-8 字节时悄悄丢内容（`read_to_string` 失败即返回空串，
/// 而调用方看到的只是一个「空的成功响应」）。
///
/// 重定向交给 ureq 自己跟：GitHub 的 `releases/latest/download/...`
/// 就是 302 到 `objects.githubusercontent.com`，手工跟反而容易做错。
///
/// 失败时会把写了一半的目标文件删掉 —— 留着半个 zip 只会让下一次
/// 「解压失败」的报错更难懂。
pub fn download(url: &str, dest: &Path, timeout_ms: i32) -> Result<u64, HttpError> {
    let agent = agent_for(timeout_ms)?;
    let mut resp = agent
        .get(url)
        .call()
        .map_err(|e| HttpError::Network(e.to_string()))?;

    let status = resp.status().as_u16() as u32;
    if !(200..300).contains(&status) {
        return Err(HttpError::Network(format!("HTTP {status}")));
    }

    // 闭包体里要可变借用 resp（读 body），所以声明必须带 mut
    let mut write = || -> std::io::Result<u64> {
        let mut reader = resp.body_mut().as_reader();
        let mut file = std::fs::File::create(dest)?;
        let n = std::io::copy(&mut reader, &mut file)?;
        file.sync_all()?;
        Ok(n)
    };

    match write() {
        Ok(n) => Ok(n),
        Err(e) => {
            let _ = std::fs::remove_file(dest);
            Err(HttpError::Network(e.to_string()))
        }
    }
}

/// POST 一段 JSON。
pub fn post_json(url: &str, json: &str, timeout_ms: i32) -> Result<HttpResponse, HttpError> {
    let headers = "Content-Type: application/json; charset=utf-8\r\n";
    request("POST", url, headers, json.as_bytes(), timeout_ms)
}

/// POST `application/x-www-form-urlencoded`。
pub fn post_form(url: &str, form: &str, timeout_ms: i32) -> Result<HttpResponse, HttpError> {
    let headers = "Content-Type: application/x-www-form-urlencoded; charset=utf-8\r\n";
    request("POST", url, headers, form.as_bytes(), timeout_ms)
}

/// 构造 `multipart/form-data` 请求体。
///
/// 返回 `(content_type, body)`。
pub fn multipart(fields: &[(&str, &str)], file: Option<(&str, &str, &[u8])>) -> (String, Vec<u8>) {
    const BOUNDARY: &str = "----VibeClassAgentBoundary7MA4YWxkTrZu0gW";
    let mut body: Vec<u8> = Vec::new();

    for (k, v) in fields {
        body.extend_from_slice(format!("--{BOUNDARY}\r\n").as_bytes());
        body.extend_from_slice(
            format!("Content-Disposition: form-data; name=\"{k}\"\r\n\r\n").as_bytes(),
        );
        body.extend_from_slice(v.as_bytes());
        body.extend_from_slice(b"\r\n");
    }

    if let Some((name, filename, data)) = file {
        body.extend_from_slice(format!("--{BOUNDARY}\r\n").as_bytes());
        body.extend_from_slice(
            format!("Content-Disposition: form-data; name=\"{name}\"; filename=\"{filename}\"\r\n")
                .as_bytes(),
        );
        body.extend_from_slice(b"Content-Type: application/octet-stream\r\n\r\n");
        body.extend_from_slice(data);
        body.extend_from_slice(b"\r\n");
    }

    body.extend_from_slice(format!("--{BOUNDARY}--\r\n").as_bytes());
    (
        format!("Content-Type: multipart/form-data; boundary={BOUNDARY}\r\n"),
        body,
    )
}

/// POST 一个 multipart 表单（可带文件）。
pub fn post_multipart(
    url: &str,
    fields: &[(&str, &str)],
    file: Option<(&str, &str, &[u8])>,
    timeout_ms: i32,
) -> Result<HttpResponse, HttpError> {
    let (ct, body) = multipart(fields, file);
    request("POST", url, &ct, &body, timeout_ms)
}

/// 对字符串做 URL 编码（表单用）。
pub fn urlencode(s: &str) -> String {
    let mut out = String::new();
    for b in s.as_bytes() {
        match b {
            b'A'..=b'Z' | b'a'..=b'z' | b'0'..=b'9' | b'-' | b'_' | b'.' | b'~' => {
                out.push(*b as char)
            }
            b' ' => out.push('+'),
            _ => out.push_str(&format!("%{b:02X}")),
        }
    }
    out
}

/// 解析后的 URL。
#[derive(Debug, Clone)]
pub struct ParsedUrl {
    /// 是否 https。
    pub secure: bool,
    /// 主机。
    pub host: String,
    /// 端口。
    pub port: u16,
    /// 路径（含查询串）。
    pub path: String,
}

/// 解析 `http(s)://host[:port]/path`。
///
/// 现在不再用于发请求（ureq 自己解析），而是用于**配置校验**：
/// 用户填错 endpoint 时当场报错，比等到课后处理失败要好得多。
pub fn parse_url(url: &str) -> Result<ParsedUrl, HttpError> {
    let url = url.trim();
    let (secure, rest) = if let Some(r) = url.strip_prefix("https://") {
        (true, r)
    } else if let Some(r) = url.strip_prefix("http://") {
        (false, r)
    } else {
        return Err(HttpError::BadUrl(format!("缺少 http(s):// 前缀: {url}")));
    };
    let (hostport, path) = match rest.find('/') {
        Some(i) => (&rest[..i], &rest[i..]),
        None => (rest, "/"),
    };
    let (host, port) = match hostport.rsplit_once(':') {
        Some((h, p)) => (
            h.to_string(),
            p.parse::<u16>()
                .map_err(|_| HttpError::BadUrl(format!("非法端口: {p}")))?,
        ),
        None => (hostport.to_string(), if secure { 443 } else { 80 }),
    };
    if host.is_empty() {
        return Err(HttpError::BadUrl("主机为空".into()));
    }
    Ok(ParsedUrl {
        secure,
        host,
        port,
        path: path.to_string(),
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    // `set_proxy` / Agent 缓存是进程级全局状态。凡是会真的发本地 HTTP 请求
    // 或修改代理的测试共用这把锁，避免并行测试互相把 Agent 清掉/换代理。
    static HTTP_STATE_TEST_LOCK: std::sync::Mutex<()> = std::sync::Mutex::new(());

    #[test]
    fn parse_https_with_path() {
        let u = parse_url("https://qyapi.weixin.qq.com/cgi-bin/webhook/send?key=abc").unwrap();
        assert!(u.secure);
        assert_eq!(u.host, "qyapi.weixin.qq.com");
        assert_eq!(u.port, 443);
        assert_eq!(u.path, "/cgi-bin/webhook/send?key=abc");
    }

    #[test]
    fn parse_http_with_port() {
        let u = parse_url("http://127.0.0.1:8080/x").unwrap();
        assert!(!u.secure);
        assert_eq!(u.host, "127.0.0.1");
        assert_eq!(u.port, 8080);
        assert_eq!(u.path, "/x");
    }

    #[test]
    fn parse_url_without_scheme_fails() {
        assert!(parse_url("example.com/x").is_err());
    }

    #[test]
    fn parse_url_defaults_root_path() {
        let u = parse_url("https://example.com").unwrap();
        assert_eq!(u.path, "/");
    }

    #[test]
    fn status_success_range() {
        assert!(HttpResponse {
            status: 200,
            body: String::new()
        }
        .is_success());
        assert!(HttpResponse {
            status: 204,
            body: String::new()
        }
        .is_success());
        assert!(!HttpResponse {
            status: 404,
            body: String::new()
        }
        .is_success());
        assert!(!HttpResponse {
            status: 500,
            body: String::new()
        }
        .is_success());
    }

    #[test]
    fn multipart_contains_boundary_and_payload() {
        let (ct, body) = multipart(&[("a", "1")], Some(("file", "x.txt", b"hello")));
        assert!(ct.contains("multipart/form-data"));
        let s = String::from_utf8_lossy(&body);
        assert!(s.contains("name=\"a\""));
        assert!(s.contains("filename=\"x.txt\""));
        assert!(s.contains("hello"));
        assert!(s.contains("--\r\n") || s.ends_with("--\r\n"));
    }

    #[test]
    fn urlencode_escapes_specials() {
        assert_eq!(urlencode("a b"), "a+b");
        assert_eq!(urlencode("中文"), "%E4%B8%AD%E6%96%87");
        assert_eq!(urlencode("k=v&x"), "k%3Dv%26x");
    }

    #[test]
    fn unsupported_method_is_rejected_before_any_io() {
        // 关键：不支持的动词必须在**发请求之前**报错，
        // 否则会拿一个莫名其妙的网络错误掩盖真正的配置问题。
        let e = request("BREW", "http://127.0.0.1:1/x", "", b"", 1000);
        assert!(matches!(e, Err(HttpError::Unsupported(_))));
    }

    #[test]
    fn proxy_is_global_and_validated() {
        let _guard = HTTP_STATE_TEST_LOCK.lock().expect("HTTP 测试锁");
        // 合成一个测试：proxy 是进程级全局状态，拆成多个测试并行跑会互相干扰。
        set_proxy(Some("http://127.0.0.1:9999".into()));
        assert_eq!(proxy().as_deref(), Some("http://127.0.0.1:9999"));

        // 空白串等价于清除，避免用户配置里写了空字符串就被当成代理
        set_proxy(Some("   ".into()));
        assert_eq!(proxy(), None);

        // 非法代理地址应在发起请求时以网络错误暴露，而不是 panic
        set_proxy(Some("这不是一个 URL".into()));
        let r = request("GET", "http://example.com/", "", b"", 1000);
        assert!(matches!(r, Err(HttpError::Network(_))));

        set_proxy(None);
        assert_eq!(proxy(), None);
    }

    /// 镜像探测要读响应头、识别 HTML 登录页，并在 HEAD 不支持时只发 Range GET。
    #[test]
    fn probe_detects_files_html_and_head_fallback() {
        use std::io::{Read as _, Write as _};
        use std::net::TcpListener;

        let _guard = HTTP_STATE_TEST_LOCK.lock().expect("HTTP 测试锁");
        set_proxy(None);

        // 1) HEAD 直接返回文件元数据：大小必须从 Content-Length 读出来。
        let listener = TcpListener::bind("127.0.0.1:0").expect("绑定文件探测端口");
        let port = listener.local_addr().expect("文件探测地址").port();
        let server = std::thread::spawn(move || {
            let (mut s, _) = listener.accept().expect("文件探测请求");
            let mut req = [0u8; 1024];
            let n = s.read(&mut req).unwrap_or(0);
            assert!(String::from_utf8_lossy(&req[..n]).starts_with("HEAD /file.bin "));
            s.write_all(
                b"HTTP/1.1 200 OK\r\nContent-Type: application/octet-stream\r\nContent-Length: 12345\r\nConnection: close\r\n\r\n",
            )
            .expect("写文件响应");
        });
        let info =
            probe(&format!("http://127.0.0.1:{port}/file.bin"), 3_000).expect("文件地址应通过");
        server.join().expect("文件服务器结束");
        assert_eq!(info.status, 200);
        assert_eq!(info.kind, "文件");
        assert_eq!(info.size, 12_345);

        // 2) 校园网登录页常见 200 + text/html，不能误报“镜像可用”。
        let listener = TcpListener::bind("127.0.0.1:0").expect("绑定 HTML 探测端口");
        let port = listener.local_addr().expect("HTML 探测地址").port();
        let server = std::thread::spawn(move || {
            let (mut s, _) = listener.accept().expect("HTML 探测请求");
            let mut req = [0u8; 1024];
            let _ = s.read(&mut req);
            s.write_all(
                b"HTTP/1.1 200 OK\r\nContent-Type: text/html; charset=utf-8\r\nContent-Length: 20\r\nConnection: close\r\n\r\n",
            )
            .expect("写 HTML 响应");
        });
        let err = probe(&format!("http://127.0.0.1:{port}/login"), 3_000)
            .expect_err("HTML 登录页必须被拒绝");
        server.join().expect("HTML 服务器结束");
        assert!(err.to_string().contains("网页"), "错误不够明确：{err}");

        // 3) HEAD=405 时退化为 Range GET；真实大小取 Content-Range 的总数，
        // 不能拿 Content-Length=1 当文件只有 1 字节。
        let listener = TcpListener::bind("127.0.0.1:0").expect("绑定 Range 探测端口");
        let port = listener.local_addr().expect("Range 探测地址").port();
        let server = std::thread::spawn(move || {
            let (mut head, _) = listener.accept().expect("HEAD 请求");
            let mut req = [0u8; 1024];
            let n = head.read(&mut req).unwrap_or(0);
            assert!(String::from_utf8_lossy(&req[..n]).starts_with("HEAD /range.bin "));
            head.write_all(
                b"HTTP/1.1 405 Method Not Allowed\r\nContent-Length: 0\r\nConnection: close\r\n\r\n",
            )
            .expect("写 405");
            drop(head);

            let (mut get, _) = listener.accept().expect("Range GET 请求");
            let mut req = [0u8; 2048];
            let n = get.read(&mut req).unwrap_or(0);
            let text = String::from_utf8_lossy(&req[..n]).to_ascii_lowercase();
            assert!(text.starts_with("get /range.bin "), "不是 GET：{text}");
            assert!(text.contains("range: bytes=0-0"), "没带 Range：{text}");
            get.write_all(
                b"HTTP/1.1 206 Partial Content\r\nContent-Type: application/octet-stream\r\nContent-Length: 1\r\nContent-Range: bytes 0-0/98765\r\nConnection: close\r\n\r\nx",
            )
            .expect("写 Range 响应");
        });
        let info =
            probe(&format!("http://127.0.0.1:{port}/range.bin"), 3_000).expect("Range 回退应通过");
        server.join().expect("Range 服务器结束");
        assert_eq!(info.status, 206);
        assert_eq!(info.size, 98_765);
    }
}
