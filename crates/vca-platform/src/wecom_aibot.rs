//! 企业微信**智能机器人**（aibot）推送。
//!
//! # 它和「群机器人 Webhook」是两条完全不同的路
//!
//! | | 群机器人 Webhook | 智能机器人（本模块） |
//! |---|---|---|
//! | 凭据 | 一个 Webhook URL | `bot_id` + `secret` + 会话 id |
//! | 通道 | 一次 HTTP POST | **WebSocket 长连接** |
//! | 能带附件 | 能（文件/图片） | **不能**，只支持 markdown 与模板卡片 |
//!
//! 所以这条通道的定位是「**推一段摘要**」：课后要点的正文直接发在群里，
//! 带图的 Word 仍然落在本机（消息里写明文档在哪）。
//! 需要连文件一起发的时候，请用群机器人 Webhook 或 OneBot。
//!
//! # 协议（对照官方 `@wecom/aibot-node-sdk` 的实现，不是猜的）
//!
//! 1. 连 `wss://openws.work.weixin.qq.com`；
//! 2. 认证帧 `{"cmd":"aibot_subscribe","headers":{"req_id":…},"body":{"bot_id":…,"secret":…}}`；
//! 3. 等同 `req_id` 的回执，`errcode == 0` 才算认证成功；
//! 4. 推送帧 `{"cmd":"aibot_send_msg","headers":{"req_id":…},"body":{"chatid":…,"msgtype":"markdown","markdown":{"content":…}}}`；
//! 5. 等回执，关闭连接。
//!
//! `req_id` 的格式 `{prefix}_{毫秒时间戳}_{8位十六进制}` 抄自官方 SDK 的
//! `generate_req_id`，不是我们另编的。
//!
//! # 为什么是「连一次、发一条、断开」而不是常驻长连接
//!
//! 常驻长连接是为**接收**消息准备的（我们只发不接）。推送发生在流水线跑完时，
//! 一天也就几次，为它挂一条常驻连接（还要处理重连、心跳、掉线）
//! 换不来什么。收发在同一个连接内完成，认证与发送用同一个 `req_id`
//! 机制对齐，逻辑上是自洽的。

use std::net::TcpStream;
use std::sync::atomic::{AtomicU64, Ordering};
use std::time::{Duration, Instant, SystemTime, UNIX_EPOCH};

use anyhow::{anyhow, Context, Result};
use serde_json::{json, Value};
use tungstenite::stream::MaybeTlsStream;
use tungstenite::{connect, Message, WebSocket};

/// 默认的 WebSocket 入口（官方 SDK 内置的就是这个地址）。
pub const DEFAULT_WS_URL: &str = "wss://openws.work.weixin.qq.com";

/// 等回执的超时：认证与发送各自单独计时。
const ACK_TIMEOUT: Duration = Duration::from_secs(20);

/// 一类帧的 WebSocket 连接。
type Ws = WebSocket<MaybeTlsStream<TcpStream>>;

/// 智能机器人的连接与推送凭据。
#[derive(Debug, Clone, Default)]
pub struct AibotConfig {
    /// 机器人 ID（企业微信后台「智能机器人」页里给的）。
    pub bot_id: String,
    /// 机器人 Secret（同上，与 bot_id 成对）。
    pub secret: String,
    /// 会话 id：单聊填对方的 userid，群聊填群的 chatid。
    pub chatid: String,
    /// 自定义 WebSocket 地址；留空用 [`DEFAULT_WS_URL`]。
    pub ws_url: String,
}

impl AibotConfig {
    /// 建一份配置（空串表示用默认值）。
    pub fn new(bot_id: &str, secret: &str, chatid: &str, ws_url: &str) -> Self {
        Self {
            bot_id: bot_id.trim().to_string(),
            secret: secret.trim().to_string(),
            chatid: chatid.trim().to_string(),
            ws_url: ws_url.trim().to_string(),
        }
    }

    /// 实际使用的 WebSocket 地址。
    pub fn endpoint(&self) -> &str {
        if self.ws_url.trim().is_empty() {
            DEFAULT_WS_URL
        } else {
            self.ws_url.trim()
        }
    }

    /// 缺哪个字段就说哪个，别丢一句「配置不完整」让人猜。
    pub fn validate(&self) -> Result<()> {
        let mut missing = Vec::new();
        if self.bot_id.is_empty() {
            missing.push("机器人 ID（bot_id）");
        }
        if self.secret.is_empty() {
            missing.push("机器人 Secret");
        }
        if self.chatid.is_empty() {
            missing.push("会话 id（单聊填 userid，群聊填 chatid）");
        }
        if missing.is_empty() {
            return Ok(());
        }
        Err(anyhow!(
            "企业微信智能机器人还缺：{}。\n\
             这三项都在企业微信后台的「智能机器人」页面里。",
            missing.join("、")
        ))
    }

    /// 只校验「机器人身份」这两项，不管会话 id。
    ///
    /// 拉会话列表时本来就还没有 chatid —— 那正是要去找的东西。
    pub fn validate_credentials(&self) -> Result<()> {
        if self.bot_id.is_empty() || self.secret.is_empty() {
            return Err(anyhow!(
                "拉会话列表需要机器人身份：请先填好机器人 ID 与 Secret（企业微信后台「智能机器人」页）"
            ));
        }
        Ok(())
    }

    /// 凭据是否齐（给界面判断「能不能点发送测试」用）。
    pub fn is_ready(&self) -> bool {
        !self.bot_id.is_empty() && !self.secret.is_empty() && !self.chatid.is_empty()
    }
}

/// 生成 `req_id`：`{prefix}_{毫秒时间戳}_{8位十六进制}`。
///
/// 随机部分用「时间 + 进程内自增计数」拼，不引随机数库：
/// 这个 id 只要在**一条连接内**不重复就够了，不需要密码学强度。
fn req_id(prefix: &str) -> String {
    static SEQ: AtomicU64 = AtomicU64::new(0);
    let ms = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|d| d.as_millis())
        .unwrap_or(0);
    let seq = SEQ.fetch_add(1, Ordering::Relaxed);
    // 混一下，避免同一毫秒内的两个 id 长得太像
    let tail = (ms as u64).wrapping_mul(0x9E37_79B9) ^ seq.wrapping_mul(0x85EB_CA6B);
    format!("{prefix}_{ms}_{:08x}", (tail as u32) as u64)
}

/// 给底层 socket 设读超时。
///
/// 这一条是必须的：`wait_ack` 的逻辑超时靠 `Instant`，但 `read()` 本身是阻塞的，
/// 对方不回帧就会一直挂在那儿 —— 而这段代码跑在流水线里，
/// 挂住等于整条课后处理卡死。
fn apply_timeout(ws: &Ws, d: Duration) {
    let r = match ws.get_ref() {
        MaybeTlsStream::Plain(s) => s.set_read_timeout(Some(d)),
        MaybeTlsStream::Rustls(s) => s.get_ref().set_read_timeout(Some(d)),
        _ => return,
    };
    let _ = r;
}

fn send_frame(ws: &mut Ws, frame: &Value) -> Result<()> {
    ws.send(Message::text(frame.to_string()))
        .context("发送 WebSocket 帧失败")?;
    Ok(())
}

/// 等指定 `req_id` 的回执。
///
/// 期间会收到心跳之类的其它帧，直接跳过 —— 只认 `headers.req_id` 对得上的那一帧。
fn wait_ack(ws: &mut Ws, want: &str, what: &str) -> Result<()> {
    let deadline = Instant::now() + ACK_TIMEOUT;
    loop {
        if Instant::now() >= deadline {
            return Err(anyhow!("等{what}回执超时（{} 秒）", ACK_TIMEOUT.as_secs()));
        }
        let msg = ws.read().context("读取 WebSocket 帧失败")?;
        let text = match msg {
            Message::Text(t) => t.to_string(),
            Message::Binary(b) => String::from_utf8_lossy(&b).to_string(),
            Message::Close(c) => return Err(anyhow!("连接被对方关闭：{c:?}")),
            // Ping/Pong 交给 tungstenite 自己处理，继续等
            _ => continue,
        };
        let Ok(v) = serde_json::from_str::<Value>(&text) else {
            continue;
        };
        let rid = v
            .pointer("/headers/req_id")
            .and_then(|x| x.as_str())
            .unwrap_or("");
        if rid != want {
            continue;
        }
        let code = v.get("errcode").and_then(|x| x.as_i64()).unwrap_or(0);
        if code == 0 {
            return Ok(());
        }
        let msg = v
            .get("errmsg")
            .and_then(|x| x.as_str())
            .unwrap_or("(对方没给 errmsg)");
        return Err(anyhow!("errcode={code} errmsg={msg}"));
    }
}

/// 发一条 Markdown 消息，返回本次的 `req_id`（便于在日志里对上）。
///
/// 一次调用 = 建连 → 认证 → 发送 → 断开，不留常驻连接，
/// 因此可以随时重试、也不会因为掉线需要重连逻辑。
pub fn send_markdown(cfg: &AibotConfig, content: &str) -> Result<String> {
    cfg.validate()?;
    if content.trim().is_empty() {
        return Err(anyhow!("内容为空，不发"));
    }

    let mut ws = connect_and_auth(cfg, ACK_TIMEOUT)?;

    // 推送
    let send_id = req_id("aibot_send_msg");
    send_frame(
        &mut ws,
        &json!({
            "cmd": "aibot_send_msg",
            "headers": { "req_id": send_id },
            "body": {
                "chatid": cfg.chatid,
                "msgtype": "markdown",
                "markdown": { "content": content },
            },
        }),
    )?;
    wait_ack(&mut ws, &send_id, "发送").map_err(|e| {
        anyhow!(
            "{e}\n\
             会话 id 填错时通常也会走到这里：单聊填对方的 userid，群聊填群的 chatid。"
        )
    })?;

    let _ = ws.close(None);
    Ok(send_id)
}

/// 建连并完成认证（发送与拉会话列表共用）。
fn connect_and_auth(cfg: &AibotConfig, read_timeout: Duration) -> Result<Ws> {
    let url = cfg.endpoint().to_string();
    let (mut ws, _resp) = connect(url.as_str()).with_context(|| {
        format!(
            "连不上企业微信的 WebSocket（{url}）。检查网络/代理；内网环境下可能需要放行 wss 出站"
        )
    })?;
    apply_timeout(&ws, read_timeout);

    let sub_id = req_id("aibot_subscribe");
    send_frame(
        &mut ws,
        &json!({
            "cmd": "aibot_subscribe",
            "headers": { "req_id": sub_id },
            "body": { "bot_id": cfg.bot_id, "secret": cfg.secret },
        }),
    )?;
    wait_ack(&mut ws, &sub_id, "认证").map_err(|e| {
        anyhow!(
            "企业微信认证失败：{e}\n\
             bot_id / secret 取自企业微信后台「智能机器人」页，两者任一不对都会停在这一步。"
        )
    })?;
    Ok(ws)
}

/// 会话 id 的候选键名。
///
/// 官方 SDK 在**发送**时用的字段是 `chatid`，回调里大概率同名；
/// 但不同版本可能换名字，所以多认几个 —— 认不出的帧会被原样回显给用户，
/// 而不是静默返回一个空列表让人以为「这个机器人没有会话」。
const CHAT_ID_KEYS: &[&str] = &["chatid", "chat_id", "conversation_id", "conversationid"];

/// 会话名字的候选键名（取到哪个算哪个）。
const CHAT_NAME_KEYS: &[&str] = &[
    "chat_name",
    "chatname",
    "name",
    "from_userid",
    "from_user_id",
    "userid",
];

/// 从一个回调帧的 body 里取出（会话 id, 会话名）。
fn extract_chat(body: &Value) -> Option<(String, String)> {
    let id = CHAT_ID_KEYS.iter().find_map(|k| {
        body.get(*k)
            .and_then(|x| x.as_str())
            .map(str::trim)
            .filter(|x| !x.is_empty())
            .map(String::from)
    })?;

    let name = CHAT_NAME_KEYS
        .iter()
        .find_map(|k| body.get(*k).and_then(|x| x.as_str()))
        .or_else(|| body.pointer("/from/userid").and_then(|x| x.as_str()))
        .or_else(|| body.pointer("/from/name").and_then(|x| x.as_str()))
        .unwrap_or("（未命名会话）")
        .to_string();

    Some((id, name))
}

/// 把一段 JSON 截断成可读的短串（排查用）。
fn clamp_json(v: &Value, max: usize) -> String {
    let s = v.to_string();
    if s.chars().count() <= max {
        return s;
    }
    s.chars().take(max).collect::<String>() + "…"
}

/// 拉会话的结果。
///
/// 用结构体而不是 `(Vec<(String, String)>, Vec<String>)`：
/// 后者连 clippy 都会抱怨「类型太复杂」，人看着更晕。
#[derive(Debug, Clone)]
pub struct ChatList {
    /// 找到的会话：`(会话 id, 名称)`。
    pub chats: Vec<(String, String)>,
    /// 认不出会话 id 的帧样例（给用户排查用）。
    pub unknown_frames: Vec<String>,
}

/// 拉取「最近和机器人有过互动的会话」。
///
/// # 为什么是「监听」而不是「查询」
///
/// 智能机器人是**回调制**的：会话 id 只在别人对机器人说话时随消息送过来，
/// 企业微信**不提供**「列出全部会话」的接口（这是它的隐私设计，不是我们没找到）。
/// 所以这里做的是：连上 → 认证 → 静候 `listen_secs` 秒 → 收集这段时间出现过的会话。
///
/// 一个必须说清的限制：**只能列出「最近和机器人有过互动的会话」**。
/// 想让它认出某个群，先把机器人拉进群、在群里 @ 它一次。
pub fn list_chats(cfg: &AibotConfig, listen_secs: u64) -> Result<ChatList> {
    cfg.validate_credentials()?;
    let listen = listen_secs.clamp(3, 120);

    // 认证本身可能要等几秒，读取超时给足
    let mut ws = connect_and_auth(cfg, Duration::from_secs(listen + 10))?;

    let deadline = Instant::now() + Duration::from_secs(listen);
    let mut found: Vec<(String, String)> = Vec::new();
    let mut unknown: Vec<String> = Vec::new();

    while Instant::now() < deadline {
        let msg = match ws.read() {
            Ok(m) => m,
            // 超时或对端关闭都算「这段时间没有别的事了」，正常收工
            Err(_) => break,
        };
        let text = match msg {
            Message::Text(t) => t.to_string(),
            Message::Binary(b) => String::from_utf8_lossy(&b).to_string(),
            Message::Close(_) => break,
            _ => continue,
        };
        let Ok(v) = serde_json::from_str::<Value>(&text) else {
            continue;
        };
        // 只关心带 body 的回调帧；心跳与回执跳过
        let Some(body) = v.get("body") else { continue };
        if body.get("msgtype").is_none() && body.get("event").is_none() {
            continue;
        }
        match extract_chat(body) {
            Some(item) => {
                if !found.contains(&item) {
                    found.push(item);
                }
            }
            None => {
                if unknown.len() < 3 {
                    unknown.push(clamp_json(body, 400));
                }
            }
        }
    }

    let _ = ws.close(None);
    Ok(ChatList {
        chats: found,
        unknown_frames: unknown,
    })
}

/// 把标题、正文与附件路径拼成一段适合在手机上看的消息。
///
/// 智能机器人发不了文件，所以正文直接进消息体；附件路径写在末尾，
/// 让人知道完整版（带截图的那份）在哪儿。
pub fn compose_markdown(title: &str, body: &str, attachment: Option<&str>) -> String {
    let mut s = String::new();
    if !title.trim().is_empty() {
        s.push_str(&format!("**{}**\n\n", title.trim()));
    }
    let body = body.trim();
    s.push_str(if body.is_empty() {
        "(没有提取到要点)"
    } else {
        body
    });
    if let Some(p) = attachment.map(str::trim).filter(|x| !x.is_empty()) {
        s.push_str(&format!("\n\n完整文档（含截图）：`{p}`"));
    }
    s
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn req_id_shape_matches_the_official_sdk() {
        // 格式 {prefix}_{毫秒}_{8位十六进制}。
        // 注意 prefix 自己就带下划线（aibot_subscribe），所以从右边切，
        // 数 "split('_') 的段数" 会数出 4 段来 —— 这个测试第一版就栽在这。
        let id = req_id("aibot_subscribe");
        assert!(id.starts_with("aibot_subscribe_"), "{id}");
        let tail = id.rsplit('_').next().unwrap();
        assert_eq!(tail.len(), 8, "末段应当是 8 位十六进制：{id}");
        assert!(u64::from_str_radix(tail, 16).is_ok(), "{id}");
        let ms = id
            .strip_prefix("aibot_subscribe_")
            .and_then(|r| r.rsplit('_').nth(1));
        assert!(
            ms.and_then(|x| x.parse::<u128>().ok()).is_some(),
            "中间应当是毫秒时间戳：{id}"
        );
    }

    #[test]
    fn req_ids_do_not_repeat_inside_one_millisecond() {
        // 同一条连接里 req_id 撞车，回执就对不上了
        let a = req_id("aibot_send_msg");
        let b = req_id("aibot_send_msg");
        assert_ne!(a, b);
    }

    #[test]
    fn validate_names_what_is_missing() {
        let e = AibotConfig::default().validate().unwrap_err().to_string();
        assert!(e.contains("bot_id"), "{e}");
        assert!(e.contains("Secret"), "{e}");
        assert!(e.contains("会话 id"), "{e}");
    }

    #[test]
    fn validate_passes_when_all_three_are_set() {
        let c = AibotConfig::new("bot", "sec", "chat", "");
        assert!(c.is_ready());
        assert!(c.validate().is_ok());
        assert_eq!(c.endpoint(), DEFAULT_WS_URL);
    }

    #[test]
    fn endpoint_override_is_honoured() {
        let c = AibotConfig::new("b", "s", "c", "wss://example.test/ws");
        assert_eq!(c.endpoint(), "wss://example.test/ws");
    }

    #[test]
    fn compose_keeps_title_body_and_attachment() {
        let md = compose_markdown(
            "课堂纪要",
            "- 讲了《背影》\n- 作业：背诵第三段",
            Some(r"D:\docs\语文.docx"),
        );
        assert!(md.contains("**课堂纪要**"));
        assert!(md.contains("- 讲了《背影》"));
        assert!(md.contains("完整文档"));
        assert!(md.contains(r"D:\docs\语文.docx"));
    }

    #[test]
    fn compose_without_body_or_attachment_stays_readable() {
        let md = compose_markdown("课堂纪要", "   ", None);
        assert!(md.contains("没有提取到要点"), "{md}");
        assert!(!md.contains("完整文档"));
    }

    #[test]
    fn extract_chat_accepts_the_official_key() {
        let body = json!({ "msgtype": "text", "chatid": "wrOgQhDgAA", "chat_name": "高一(3)班" });
        assert_eq!(
            extract_chat(&body),
            Some(("wrOgQhDgAA".to_string(), "高一(3)班".to_string()))
        );
    }

    #[test]
    fn extract_chat_tolerates_other_key_spellings() {
        // 官方若换字段名，这个功能不该整个失灵
        for key in ["chat_id", "conversation_id", "conversationid"] {
            let body = json!({ "msgtype": "text", key: "abc" });
            assert_eq!(
                extract_chat(&body).map(|x| x.0),
                Some("abc".to_string()),
                "键名 {key} 应当被认出来"
            );
        }
    }

    #[test]
    fn extract_chat_falls_back_to_nested_from() {
        let body = json!({ "msgtype": "text", "chatid": "c1", "from": { "userid": "zhangsan" } });
        assert_eq!(
            extract_chat(&body),
            Some(("c1".to_string(), "zhangsan".to_string()))
        );
    }

    #[test]
    fn extract_chat_returns_none_when_nothing_matches() {
        // 认不出就老实返回 None（由调用方记录原始帧），不要瞎编一个 id
        assert!(extract_chat(&json!({ "msgtype": "text" })).is_none());
        assert!(extract_chat(&json!({ "chatid": "   " })).is_none());
    }

    #[test]
    fn list_chats_needs_credentials_before_touching_the_network() {
        let cfg = AibotConfig::new("", "", "", "");
        let err = list_chats(&cfg, 3).unwrap_err().to_string();
        assert!(err.contains("机器人 ID"), "{err}");
    }

    #[test]
    fn clamp_json_truncates_long_payloads() {
        let v = json!({ "a": "x".repeat(500) });
        let s = clamp_json(&v, 50);
        assert!(s.chars().count() <= 51, "{}", s.chars().count());
        assert!(s.ends_with('…'), "{s}");
    }

    /// 真连一次企业微信的 wss 入口。
    ///
    /// 默认 `#[ignore]`：它要外网，而且**必然**在认证那步失败
    /// （凭据是假的）。但它能证明一件重要的事——
    /// TLS 握手与协议升级这条路是通的（rustls 的 crypto provider 装对了）。
    /// 手动跑：`cargo test -p vca-platform aibot_reaches -- --ignored --nocapture`
    #[test]
    #[ignore = "需要外网；手动跑：cargo test -p vca-platform aibot_reaches -- --ignored --nocapture"]
    fn aibot_reaches_wecom_and_fails_only_at_auth() {
        let cfg = AibotConfig::new(
            "nonexistent-bot-id",
            "nonexistent-secret",
            "nonexistent-chat",
            "",
        );
        let err = send_markdown(&cfg, "hello").unwrap_err().to_string();
        println!("实际错误：{err}");
        // 只要能走到「认证失败」，就说明连接与协议升级都没问题
        assert!(
            err.contains("认证失败"),
            "没走到认证这一步，说明连接本身就有问题：{err}"
        );
    }
}
