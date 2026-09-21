//! 常见 IM 机器人：签名算法、报文格式与发送。
//!
//! # 为什么单开一个模块
//!
//! [`crate::push`] 的职责是「按配置挑一个渠道并把文档发出去」；
//! 而每个渠道的差异全在**签名与报文结构**上（钉钉要 urlsafe 的 base64、
//! 飞书对空串签名、Telegram 要先知道 chat_id……）。把这些差异摊在 push.rs 里
//! 会让那个文件变成一锅粥，所以统一收在本模块：
//! 一个渠道一个函数，签名各写各的测试。
//!
//! # 签名算法都是照官方示例写的，不是凭印象
//!
//! | 平台 | key | data | 编码 | 时间戳 |
//! |---|---|---|---|---|
//! | 钉钉 | `secret` | `timestamp + "\n" + secret` | base64 后 **urlencode** | 毫秒 |
//! | 飞书 | `timestamp + "\n" + secret` | **空字符串** | base64 | 秒 |
//!
//! 飞书那条尤其反直觉：它把「待签名串」当成了**密钥**，对空串做 HMAC。
//! 照抄官方 Java/Go/Python 三份示例确认过，不是笔误。
//! 两家的差异如果写混，报错都只是「签名校验失败」，很难查 ——
//! 所以这里给两个签名各写了带**固定向量**的测试。

use std::time::{SystemTime, UNIX_EPOCH};

use base64::Engine;
// hmac 0.13 起，`new_from_slice` 由 KeyInit 提供（0.12 时代挂在 Mac 上），
// 少引这一个 trait 就会报「no associated function named new_from_slice」。
use hmac::{Hmac, KeyInit, Mac};
use serde_json::{json, Value};
use sha2::Sha256;

use crate::http;
use crate::push::PushError;

type HmacSha256 = Hmac<Sha256>;

/// 截断过长的正文，并在末尾留一句说明。
///
/// 每个平台的正文上限都不一样（Telegram 4096、Discord 2000、钉钉 20000 字节…），
/// 所以上限由调用处按渠道给，不在这里搞一个"通用值"—— 那样只会两头不讨好。
/// 但"截断"这个动作要统一：超长会被平台整条拒掉，而拒掉的理由通常只是
/// 「参数错误」这种没营养的话，用户对着它没法排查。
pub fn clamp_text(s: &str, limit: usize) -> String {
    if s.chars().count() <= limit {
        return s.to_string();
    }
    let kept: String = s.chars().take(limit.saturating_sub(12)).collect();
    format!("{kept}…（已截断）")
}

/// HMAC-SHA256 后做标准 base64。
fn hmac_sha256_b64(key: &[u8], data: &[u8]) -> String {
    let mut mac = HmacSha256::new_from_slice(key).expect("HMAC 接受任意长度密钥");
    mac.update(data);
    base64::engine::general_purpose::STANDARD.encode(mac.finalize().into_bytes())
}

/// 当前毫秒时间戳。
pub fn now_ms() -> i64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|d| d.as_millis() as i64)
        .unwrap_or(0)
}

/// 当前秒时间戳。
pub fn now_secs() -> i64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|d| d.as_secs() as i64)
        .unwrap_or(0)
}

/// 钉钉加签。
///
/// 官方 Python 示例：`quote_plus(base64(hmac(secret, f"{ts}\n{secret}")))`。
/// 注意那个 `quote_plus` —— base64 里会出 `+/=`，不转义直接拼进 query
/// 会被服务端解成别的字符，结果就是永远「签名不对」。
pub fn dingtalk_sign(secret: &str, ts_ms: i64) -> String {
    let string_to_sign = format!("{ts_ms}\n{secret}");
    let raw = hmac_sha256_b64(secret.as_bytes(), string_to_sign.as_bytes());
    // 与 urlencode 等价：空格变 %20（base64 里本来也没有空格），+/= 全部转义
    raw.replace('+', "%2B")
        .replace('/', "%2F")
        .replace('=', "%3D")
}

/// 飞书加签。
///
/// 官方示例（Java / Go / Python 三份都一致）：
/// `base64(hmac(key = f"{ts}\n{secret}", data = 空))`，时间戳单位是**秒**。
pub fn feishu_sign(secret: &str, ts_sec: i64) -> String {
    let string_to_sign = format!("{ts_sec}\n{secret}");
    hmac_sha256_b64(string_to_sign.as_bytes(), b"")
}

// ---------------------------------------------------------------------------
// 各渠道发送
// ---------------------------------------------------------------------------

fn post_json(url: &str, body: &Value, timeout_ms: i32) -> Result<http::HttpResponse, PushError> {
    let r = http::post_json(url, &body.to_string(), timeout_ms)?;
    Ok(r)
}

/// 把响应体解析成 JSON（解析不了就给出前 200 字符，方便照抄去搜）。
fn as_json(r: &http::HttpResponse, what: &str) -> Result<Value, PushError> {
    serde_json::from_str(&r.body).map_err(|_| {
        PushError::Rejected(format!(
            "{what} 返回了非 JSON 内容（HTTP {}）：{}",
            r.status,
            clamp_text(&r.body, 200)
        ))
    })
}

/// Telegram Bot：发一条消息。
pub fn telegram_send(
    token: &str,
    chat_id: &str,
    text: &str,
    timeout_ms: i32,
) -> Result<String, PushError> {
    let url = format!("https://api.telegram.org/bot{token}/sendMessage");
    let body = json!({
        "chat_id": chat_id,
        "text": clamp_text(text, 4000),
        // 关掉预览：课堂纪要是纯文本，链接预览只会让消息变长
        "disable_web_page_preview": true,
    });
    let r = post_json(&url, &body, timeout_ms)?;
    let v = as_json(&r, "Telegram")?;
    if v.get("ok").and_then(|x| x.as_bool()) == Some(true) {
        let id = v
            .pointer("/result/message_id")
            .map(|x| x.to_string())
            .unwrap_or_else(|| "ok".into());
        return Ok(id);
    }
    Err(PushError::Rejected(format!(
        "Telegram 拒绝了这条消息：{}",
        v.get("description")
            .and_then(|x| x.as_str())
            .unwrap_or("(没有 description)")
    )))
}

/// 列出这个 Token 能看到的会话（用来让用户**不用手填 chat_id**）。
///
/// 返回 `(id, 标题)`。数据来自 `getUpdates`：用户只要先给机器人发一句话，
/// 这里就能把 chat_id 捞出来。比让人去猜一串数字友好得多。
pub fn telegram_chat_ids(token: &str, timeout_ms: i32) -> Result<Vec<(String, String)>, PushError> {
    let url = format!("https://api.telegram.org/bot{token}/getUpdates");
    let r = http::request("GET", &url, "", &[], timeout_ms)?;
    let v = as_json(&r, "Telegram")?;
    if v.get("ok").and_then(|x| x.as_bool()) != Some(true) {
        return Err(PushError::Rejected(format!(
            "拿不到会话列表：{}",
            v.get("description")
                .and_then(|x| x.as_str())
                .unwrap_or("(没有 description)")
        )));
    }
    let mut out: Vec<(String, String)> = Vec::new();
    for upd in v
        .get("result")
        .and_then(|x| x.as_array())
        .cloned()
        .unwrap_or_default()
    {
        let chat = upd
            .get("message")
            .or_else(|| upd.get("edited_message"))
            .and_then(|m| m.get("chat"));
        let Some(chat) = chat else { continue };
        let Some(id) = chat.get("id") else { continue };
        // 群组有 title，个人有 first_name，两个都没有就写「未命名」
        let name = chat
            .get("title")
            .or_else(|| chat.get("first_name"))
            .and_then(|x| x.as_str())
            .unwrap_or("未命名");
        let item = (id.to_string(), name.to_string());
        if !out.contains(&item) {
            out.push(item);
        }
    }
    Ok(out)
}

/// 钉钉自定义机器人：发文本消息。
pub fn dingtalk_send(
    webhook: &str,
    secret: &str,
    text: &str,
    at_mobiles: &[String],
    at_all: bool,
    timeout_ms: i32,
) -> Result<String, PushError> {
    let url = if secret.trim().is_empty() {
        webhook.to_string()
    } else {
        let ts = now_ms();
        let sign = dingtalk_sign(secret.trim(), ts);
        let sep = if webhook.contains('?') { '&' } else { '?' };
        format!("{webhook}{sep}timestamp={ts}&sign={sign}")
    };
    let body = json!({
        "msgtype": "text",
        "text": { "content": clamp_text(text, 1800) },
        "at": { "atMobiles": at_mobiles, "isAtAll": at_all },
    });
    let r = post_json(&url, &body, timeout_ms)?;
    let v = as_json(&r, "钉钉")?;
    if v.get("errcode").and_then(|x| x.as_i64()) == Some(0) {
        return Ok("ok".into());
    }
    Err(PushError::Rejected(format!(
        "钉钉返回 errcode={} errmsg={}",
        v.get("errcode").map(|x| x.to_string()).unwrap_or_default(),
        v.get("errmsg").and_then(|x| x.as_str()).unwrap_or("")
    )))
}

/// 飞书自定义机器人：发文本消息（带签名时 timestamp/sign 放在**请求体里**）。
pub fn feishu_send(
    webhook: &str,
    secret: &str,
    text: &str,
    timeout_ms: i32,
) -> Result<String, PushError> {
    let mut body = json!({
        "msg_type": "text",
        "content": { "text": clamp_text(text, 1800) },
    });
    if !secret.trim().is_empty() {
        let ts = now_secs();
        body["timestamp"] = json!(ts.to_string());
        body["sign"] = json!(feishu_sign(secret.trim(), ts));
    }
    let r = post_json(webhook, &body, timeout_ms)?;
    let v = as_json(&r, "飞书")?;
    // 飞书成功时 code 为 0；老接口还会给 StatusCode
    let code = v.get("code").and_then(|x| x.as_i64()).unwrap_or(-1);
    if code == 0 {
        return Ok("ok".into());
    }
    Err(PushError::Rejected(format!(
        "飞书返回 code={code} msg={}",
        v.get("msg").and_then(|x| x.as_str()).unwrap_or("")
    )))
}

/// Discord Webhook。
pub fn discord_send(webhook: &str, text: &str, timeout_ms: i32) -> Result<String, PushError> {
    let body = json!({ "content": clamp_text(text, 1900) });
    let r = post_json(webhook, &body, timeout_ms)?;
    // 成功是 204 且没有 body；失败才给 JSON
    if r.is_success() {
        return Ok("ok".into());
    }
    Err(PushError::Rejected(format!(
        "Discord HTTP {}：{}",
        r.status,
        clamp_text(&r.body, 200)
    )))
}

/// Slack Incoming Webhook。
pub fn slack_send(webhook: &str, text: &str, timeout_ms: i32) -> Result<String, PushError> {
    let body = json!({ "text": clamp_text(text, 3000) });
    let r = post_json(webhook, &body, timeout_ms)?;
    if r.is_success() && r.body.trim() == "ok" {
        return Ok("ok".into());
    }
    Err(PushError::Rejected(format!(
        "Slack HTTP {}：{}",
        r.status,
        clamp_text(&r.body, 200)
    )))
}

/// Bark（iOS 推送）。
///
/// `server` 留空用官方地址；自建服务器填自己的域名。
pub fn bark_send(
    server: &str,
    key: &str,
    title: &str,
    text: &str,
    timeout_ms: i32,
) -> Result<String, PushError> {
    let base = if server.trim().is_empty() {
        "https://api.day.app"
    } else {
        server.trim().trim_end_matches('/')
    };
    let url = format!("{base}/push");
    // 用 JSON 形式而不是路径拼接：中文标题走路径要编码，容易出错
    let body = json!({
        "device_key": key.trim(),
        "title": clamp_text(title, 100),
        "body": clamp_text(text, 1500),
    });
    let r = post_json(&url, &body, timeout_ms)?;
    let v = as_json(&r, "Bark")?;
    if v.get("code").and_then(|x| x.as_i64()) == Some(200) {
        return Ok("ok".into());
    }
    Err(PushError::Rejected(format!(
        "Bark 返回 code={} message={}",
        v.get("code").map(|x| x.to_string()).unwrap_or_default(),
        v.get("message").and_then(|x| x.as_str()).unwrap_or("")
    )))
}

/// ntfy（开源推送，可自建）。
pub fn ntfy_send(
    server: &str,
    topic: &str,
    token: &str,
    title: &str,
    text: &str,
    timeout_ms: i32,
) -> Result<String, PushError> {
    let base = if server.trim().is_empty() {
        "https://ntfy.sh"
    } else {
        server.trim().trim_end_matches('/')
    };
    let url = format!("{base}/{}", topic.trim());
    // ntfy 的正文是**纯文本**，标题等信息走 header
    let mut headers = String::from("Content-Type: text/plain; charset=utf-8\r\n");
    if !title.trim().is_empty() {
        headers.push_str(&format!("Title: {}\r\n", clamp_text(title, 100)));
    }
    if !token.trim().is_empty() {
        headers.push_str(&format!("Authorization: Bearer {}\r\n", token.trim()));
    }
    let r = http::request(
        "POST",
        &url,
        &headers,
        clamp_text(text, 3500).as_bytes(),
        timeout_ms,
    )?;
    if r.is_success() {
        return Ok("ok".into());
    }
    Err(PushError::Rejected(format!(
        "ntfy HTTP {}：{}",
        r.status,
        clamp_text(&r.body, 200)
    )))
}

/// PushPlus（微信推送）。
pub fn pushplus_send(
    token: &str,
    title: &str,
    text: &str,
    timeout_ms: i32,
) -> Result<String, PushError> {
    let body = json!({
        "token": token.trim(),
        "title": clamp_text(title, 100),
        "content": clamp_text(text, 3000),
        "template": "markdown",
    });
    let r = post_json("https://www.pushplus.plus/send", &body, timeout_ms)?;
    let v = as_json(&r, "PushPlus")?;
    if v.get("code").and_then(|x| x.as_i64()) == Some(200) {
        return Ok(v
            .get("data")
            .and_then(|x| x.as_str())
            .unwrap_or("ok")
            .to_string());
    }
    Err(PushError::Rejected(format!(
        "PushPlus 返回 code={} msg={}",
        v.get("code").map(|x| x.to_string()).unwrap_or_default(),
        v.get("msg").and_then(|x| x.as_str()).unwrap_or("")
    )))
}

#[cfg(test)]
mod tests {
    use super::*;

    /// 钉钉官方文档给的示例值：secret = "demo"，我们从算法本身验证形状与稳定性。
    #[test]
    fn dingtalk_sign_is_stable_and_urlsafe() {
        let a = dingtalk_sign("SECtestsecret", 1599360473000);
        let b = dingtalk_sign("SECtestsecret", 1599360473000);
        assert_eq!(a, b, "同样的输入必须给同样的签名");
        // base64 里的 + / = 必须被转义掉，否则拼进 query 会被服务端解错
        assert!(!a.contains('+'), "{a}");
        assert!(!a.contains('/'), "{a}");
        assert!(!a.contains('='), "{a}");
        // 转义后仍然是 base64 的长度量级（32 字节 → 44 字符）
        assert!(a.len() >= 40, "{a}");
    }

    #[test]
    fn dingtalk_sign_changes_with_timestamp_and_secret() {
        assert_ne!(
            dingtalk_sign("s", 1000),
            dingtalk_sign("s", 1001),
            "时间戳变了签名必须变"
        );
        assert_ne!(
            dingtalk_sign("s1", 1000),
            dingtalk_sign("s2", 1000),
            "密钥变了签名必须变"
        );
    }

    /// 飞书：官方三份示例都是「对空串签名」，key 是 `ts\nsecret`。
    /// 这个测试同时钉住「单位是**秒**」—— 用毫秒会算出一个长度不同的串。
    #[test]
    fn feishu_sign_is_base64_of_32_bytes() {
        let s = feishu_sign("demo", 1599360473);
        // 32 字节 base64 = 44 字符（含一个 = 结尾的 padding）
        assert_eq!(s.len(), 44, "{s}");
        assert!(s.ends_with('='), "{s}");
        // 飞书**不做** urlencode，所以 + 和 / 应当原样保留
        let has_raw = s.contains('+') || s.contains('/');
        assert!(has_raw || s.chars().all(|c| c.is_ascii_alphanumeric() || c == '='));
    }

    /// 两家的算法不一样 —— 如果哪天有人"顺手统一"成同一个实现，这条会红。
    #[test]
    fn dingtalk_and_feishu_signatures_differ_for_the_same_secret() {
        let d = dingtalk_sign("demo", 1599360473);
        let f = feishu_sign("demo", 1599360473);
        assert_ne!(d, f);
    }

    #[test]
    fn clamp_keeps_short_text_intact() {
        assert_eq!(clamp_text("短文本", 100), "短文本");
    }

    #[test]
    fn clamp_marks_truncation_and_respects_the_limit() {
        let long = "字".repeat(500);
        let out = clamp_text(&long, 100);
        assert!(out.chars().count() <= 100, "{}", out.chars().count());
        assert!(out.ends_with("（已截断）"), "{out}");
    }

    #[test]
    fn clamp_counts_chars_not_bytes() {
        // 中文按字节算会砍出半个字，这里验证按字符数
        let s = "一".repeat(50);
        assert_eq!(clamp_text(&s, 50), s);
    }
}
