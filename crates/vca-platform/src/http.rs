//! 极简 HTTP 客户端（WinHTTP）。
//!
//! 为什么自己包一层而不是引入 reqwest/ureq：
//! 1. 本项目只在 Windows 上运行，WinHTTP 是系统自带、原生支持 TLS，**零第三方依赖**；
//! 2. 避免再引入一个庞大的异步运行时依赖链（编译时间与体积都会明显增加）；
//! 3. 我们只需要「POST 一段 JSON / 一段 multipart，读回响应」这一种用法。
//!
//! 注意：本模块是阻塞式的，调用方应在独立线程或处理窗口中执行，不要卡住主循环。

use std::ffi::c_void;

type Handle = *mut c_void;
type Bool = i32;

const WINHTTP_ACCESS_TYPE_AUTOMATIC_PROXY: u32 = 4;
const WINHTTP_FLAG_SECURE: u32 = 0x0080_0000;

#[link(name = "winhttp")]
extern "system" {
    fn WinHttpOpen(
        agent: *const u16,
        access_type: u32,
        proxy: *const u16,
        bypass: *const u16,
        flags: u32,
    ) -> Handle;
    fn WinHttpConnect(session: Handle, server: *const u16, port: u16, reserved: u32) -> Handle;
    fn WinHttpOpenRequest(
        connect: Handle,
        verb: *const u16,
        object: *const u16,
        version: *const u16,
        referrer: *const u16,
        accept_types: *const *const u16,
        flags: u32,
    ) -> Handle;
    fn WinHttpSendRequest(
        request: Handle,
        headers: *const u16,
        headers_len: u32,
        optional: *const c_void,
        optional_len: u32,
        total_len: u32,
        context: usize,
    ) -> Bool;
    fn WinHttpReceiveResponse(request: Handle, reserved: *mut c_void) -> Bool;
    fn WinHttpReadData(request: Handle, buffer: *mut c_void, to_read: u32, read: *mut u32) -> Bool;
    fn WinHttpQueryHeaders(
        request: Handle,
        info_level: u32,
        name: *const u16,
        buffer: *mut c_void,
        buffer_len: *mut u32,
        index: *mut u32,
    ) -> Bool;
    fn WinHttpCloseHandle(handle: Handle) -> Bool;
    fn WinHttpSetTimeouts(
        handle: Handle,
        resolve: i32,
        connect: i32,
        send: i32,
        receive: i32,
    ) -> Bool;
}

/// `WINHTTP_QUERY_STATUS_CODE | WINHTTP_QUERY_FLAG_NUMBER`
const QUERY_STATUS_CODE: u32 = 19 | 0x2000_0000;

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
#[derive(Debug, thiserror::Error)]
pub enum HttpError {
    /// 地址无法解析。
    #[error("URL 解析失败: {0}")]
    BadUrl(String),
    /// WinHTTP 调用失败。
    #[error("WinHTTP 调用失败: {0}")]
    WinHttp(&'static str),
    /// 超时或网络不可达。
    #[error("网络错误: {0}")]
    Network(String),
}

fn wide(s: &str) -> Vec<u16> {
    s.encode_utf16().chain(std::iter::once(0)).collect()
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

/// 发送一次请求。
///
/// - `headers`：形如 `"Content-Type: application/json\r\n"`，可为空串；
/// - `body`：请求体字节；
/// - `timeout_ms`：连接/发送/接收超时。
pub fn request(
    method: &str,
    url: &str,
    headers: &str,
    body: &[u8],
    timeout_ms: i32,
) -> Result<HttpResponse, HttpError> {
    let u = parse_url(url)?;
    let agent = wide("VibeClassAgent/0.2");

    unsafe {
        let session = WinHttpOpen(
            agent.as_ptr(),
            WINHTTP_ACCESS_TYPE_AUTOMATIC_PROXY,
            std::ptr::null(),
            std::ptr::null(),
            0,
        );
        if session.is_null() {
            return Err(HttpError::WinHttp("WinHttpOpen"));
        }
        let _ = WinHttpSetTimeouts(session, timeout_ms, timeout_ms, timeout_ms, timeout_ms);

        let host = wide(&u.host);
        let connect = WinHttpConnect(session, host.as_ptr(), u.port, 0);
        if connect.is_null() {
            WinHttpCloseHandle(session);
            return Err(HttpError::WinHttp("WinHttpConnect"));
        }

        let verb = wide(method);
        let path = wide(&u.path);
        let flags = if u.secure { WINHTTP_FLAG_SECURE } else { 0 };
        let req = WinHttpOpenRequest(
            connect,
            verb.as_ptr(),
            path.as_ptr(),
            std::ptr::null(),
            std::ptr::null(),
            std::ptr::null(),
            flags,
        );
        if req.is_null() {
            WinHttpCloseHandle(connect);
            WinHttpCloseHandle(session);
            return Err(HttpError::WinHttp("WinHttpOpenRequest"));
        }

        let hdr = if headers.is_empty() {
            None
        } else {
            Some(wide(headers))
        };
        let (hp, hl) = match &hdr {
            Some(h) => (h.as_ptr(), (h.len() as u32 - 1) * 2),
            None => (std::ptr::null(), 0),
        };

        let ok = WinHttpSendRequest(
            req,
            hp,
            hl,
            body.as_ptr() as *const c_void,
            body.len() as u32,
            body.len() as u32,
            0,
        );
        if ok == 0 {
            WinHttpCloseHandle(req);
            WinHttpCloseHandle(connect);
            WinHttpCloseHandle(session);
            return Err(HttpError::Network(
                "WinHttpSendRequest 失败（网络不可达或证书问题）".into(),
            ));
        }

        if WinHttpReceiveResponse(req, std::ptr::null_mut()) == 0 {
            WinHttpCloseHandle(req);
            WinHttpCloseHandle(connect);
            WinHttpCloseHandle(session);
            return Err(HttpError::Network("WinHttpReceiveResponse 失败".into()));
        }

        // 状态码
        let mut status: u32 = 0;
        let mut len = std::mem::size_of::<u32>() as u32;
        let _ = WinHttpQueryHeaders(
            req,
            QUERY_STATUS_CODE,
            std::ptr::null(),
            &mut status as *mut u32 as *mut c_void,
            &mut len,
            std::ptr::null_mut(),
        );

        // 响应体
        let mut raw: Vec<u8> = Vec::new();
        loop {
            let mut buf = [0u8; 8192];
            let mut read: u32 = 0;
            if WinHttpReadData(req, buf.as_mut_ptr() as *mut c_void, 8192, &mut read) == 0 {
                break;
            }
            if read == 0 {
                break;
            }
            raw.extend_from_slice(&buf[..read as usize]);
        }

        WinHttpCloseHandle(req);
        WinHttpCloseHandle(connect);
        WinHttpCloseHandle(session);

        Ok(HttpResponse {
            status,
            body: String::from_utf8_lossy(&raw).to_string(),
        })
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

#[cfg(test)]
mod tests {
    use super::*;

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
}
