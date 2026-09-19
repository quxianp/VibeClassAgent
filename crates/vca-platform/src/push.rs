//! 内置推送通道。
//!
//! 设计（策划书 3.2.3 / 5.5）：
//! - 渠道由用户自选；官方渠道与企业微信走各自的原生接口；
//! - **凭据只从参数传入，绝不在代码或配置示例里硬编码**（由 CLI 从环境变量 / secrets.env 读入）；
//! - 第三方协议渠道单独标注风险（见 [`Provider::risk_note`]）；
//! - 所有渠道实现同一个 [`Pusher`] 接口，便于后续改为进程插件。

use std::path::Path;

use crate::http;

/// 推送渠道。
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Provider {
    /// 企业微信机器人（Webhook）：官方支持，可上传文件。
    WeCom,
    /// QQ 官方机器人（QQ 开放平台 API v2）：合规，需审核与 appid/secret。
    QqOfficial,
    /// OneBot 11 协议（NapCat / Lagrange / go-cqhttp 等自建实现）。
    OneBot,
    /// 通用 Webhook：POST 一段 JSON（自建服务、n8n、PushPlus 等）。
    Webhook,
    /// Server 酱（sct.ftqq.com）：表单 POST。
    ServerChan,
    /// 个人微信第三方协议：需用户自建中转服务配合。
    WeChatPersonal,
    /// 只打印不发送，用于联调与自检。
    DryRun,
}

impl Provider {
    /// 从配置字符串解析。
    pub fn parse(s: &str) -> Option<Self> {
        match s.trim().to_ascii_lowercase().as_str() {
            "wecom" | "企业微信" => Some(Self::WeCom),
            "qq" | "qq-official" | "qq_official" | "官方qq" | "qq官方" => {
                Some(Self::QqOfficial)
            }
            "onebot" | "onebot11" | "napcat" | "lagrange" | "gocqhttp" | "第三方qq" => {
                Some(Self::OneBot)
            }
            "webhook" | "generic" => Some(Self::Webhook),
            "serverchan" | "server_chan" | "sct" => Some(Self::ServerChan),
            "wechat-personal" | "wechat_personal" | "个人微信" => Some(Self::WeChatPersonal),
            "dryrun" | "dry-run" | "none" | "" => Some(Self::DryRun),
            _ => None,
        }
    }

    /// 是否官方渠道（不需要额外风险提示）。
    pub fn is_official(self) -> bool {
        matches!(self, Self::WeCom | Self::QqOfficial | Self::ServerChan)
    }

    /// 第三方渠道的风险提示。
    pub fn risk_note(self) -> Option<&'static str> {
        match self {
            Self::WeChatPersonal => {
                Some("使用第三方协议存在账号封禁风险，请自行评估；建议使用小号并定期更换密码。")
            }
            Self::OneBot => Some(
                "OneBot 属于第三方协议实现（非腾讯官方）。使用自动化客户端登录 QQ \
                 存在账号被限制或封禁的风险，请自行评估并优先使用小号。",
            ),
            _ => None,
        }
    }

    /// 显示名。
    pub fn display(self) -> &'static str {
        match self {
            Self::WeCom => "企业微信机器人",
            Self::QqOfficial => "QQ 官方机器人",
            Self::OneBot => "OneBot（自建 QQ 机器人）",
            Self::Webhook => "通用 Webhook",
            Self::ServerChan => "Server 酱",
            Self::WeChatPersonal => "个人微信（第三方协议）",
            Self::DryRun => "仅记录不发送（dry-run）",
        }
    }
}

/// 要推送的内容。
#[derive(Debug, Clone)]
pub struct PushDoc {
    /// 标题（如「课堂纪要 2025-03-18 数学」）。
    pub title: String,
    /// 纯文本摘要（作为消息正文）。
    pub summary: String,
    /// 附件 Word 路径。
    pub docx: Option<String>,
    /// 附件 PDF 路径。
    pub pdf: Option<String>,
    /// 目标会话（群 id / 用户 id，视渠道而定）。
    pub target: Option<String>,
}

impl PushDoc {
    /// 选一个可上传的附件（优先 PDF，其次 Word）。
    pub fn attachment(&self) -> Option<&str> {
        self.pdf.as_deref().or(self.docx.as_deref())
    }
}

/// 推送结果。
#[derive(Debug, Clone)]
pub struct PushOutcome {
    /// 是否成功。
    pub success: bool,
    /// 渠道返回的消息 id。
    pub message_id: Option<String>,
    /// 失败原因。
    pub error: Option<String>,
}

impl PushOutcome {
    /// 构造成功结果。
    pub fn ok(id: impl Into<String>) -> Self {
        Self {
            success: true,
            message_id: Some(id.into()),
            error: None,
        }
    }
    /// 构造失败结果。
    pub fn fail(e: impl Into<String>) -> Self {
        Self {
            success: false,
            message_id: None,
            error: Some(e.into()),
        }
    }
}

/// 推送失败原因。
#[derive(Debug, thiserror::Error)]
pub enum PushError {
    /// 网络/HTTP 错误。
    #[error("网络错误: {0}")]
    Http(#[from] http::HttpError),
    /// 渠道返回了非成功状态。
    #[error("渠道返回错误: {0}")]
    Rejected(String),
    /// 缺少必要配置。
    #[error("缺少配置: {0}")]
    Missing(String),
    /// 文件读写失败。
    #[error("文件错误: {0}")]
    Io(String),
}

/// 推送接口。
pub trait Pusher {
    /// 渠道类型。
    fn provider(&self) -> Provider;
    /// 发送。
    fn send(&self, doc: &PushDoc) -> Result<PushOutcome, PushError>;
}

/// 统一的推送入口配置。
#[derive(Debug, Clone)]
pub struct PushConfig {
    /// 渠道。
    pub provider: Provider,
    /// 地址。
    ///
    /// - 企业微信 / Server 酱 / 通用 Webhook：完整 URL；
    /// - OneBot：基址，如 `http://127.0.0.1:3000`；
    /// - QQ 官方机器人：API 基址，留空用生产地址（沙箱为 `https://sandbox.api.sgroup.qq.com`）。
    pub endpoint: String,
    /// 令牌：企业微信为 Webhook key，Server 酱为 SendKey，OneBot 为 access_token。
    pub token: String,
    /// 目标：QQ 号 / 群号 / 用户 openid。
    pub target: String,
    /// 目标类型：`private`（私聊）或 `group`（群）。
    pub target_type: String,
    /// QQ 官方机器人的 AppID。
    pub app_id: String,
    /// QQ 官方机器人的 AppSecret。
    pub app_secret: String,
    /// 失败重试次数（不含首次）。
    pub max_retries: u32,
    /// 超时（毫秒）。
    pub timeout_ms: i32,
}

impl Default for PushConfig {
    fn default() -> Self {
        Self {
            provider: Provider::DryRun,
            endpoint: String::new(),
            token: String::new(),
            target: String::new(),
            target_type: "private".to_string(),
            app_id: String::new(),
            app_secret: String::new(),
            max_retries: 3,
            timeout_ms: 20_000,
        }
    }
}

impl PushConfig {
    /// 目标是否为群。
    pub fn is_group(&self) -> bool {
        self.target_type.trim().eq_ignore_ascii_case("group")
    }
}

/// 按配置创建推送器。
pub fn make_pusher(cfg: &PushConfig) -> Box<dyn Pusher + Send + Sync> {
    match cfg.provider {
        Provider::WeCom => Box::new(WeComPusher { cfg: cfg.clone() }),
        Provider::QqOfficial => Box::new(QqOfficialPusher {
            cfg: cfg.clone(),
            token_cache: std::sync::Arc::new(std::sync::Mutex::new(None)),
        }),
        Provider::OneBot => Box::new(OneBotPusher { cfg: cfg.clone() }),
        Provider::Webhook => Box::new(WebhookPusher { cfg: cfg.clone() }),
        Provider::ServerChan => Box::new(ServerChanPusher { cfg: cfg.clone() }),
        Provider::WeChatPersonal => Box::new(WeChatPersonalPusher { cfg: cfg.clone() }),
        Provider::DryRun => Box::new(DryRunPusher),
    }
}

/// 带重试的发送。
///
/// 重试策略：最多 `max_retries` 次追加尝试，间隔按 2s / 4s / 8s 递增。
/// **只有全部失败才返回失败**，调用方据此决定「不删除本地录像」。
///
/// 之所以不在这里做「失败落盘排队」：那属于调度层的事（见 vca-engine 的流水线），
/// 这里保持「一次调用一次结论」，便于测试。
pub fn send_with_retry(
    pusher: &(dyn Pusher + Send + Sync),
    doc: &PushDoc,
    max_retries: u32,
) -> PushOutcome {
    let mut last_err: Option<String> = None;
    let attempts = max_retries.saturating_add(1);
    for attempt in 1..=attempts {
        match pusher.send(doc) {
            Ok(o) if o.success => {
                let mut ok = o;
                if attempt > 1 {
                    ok.error = Some(format!("第 {attempt} 次尝试成功"));
                }
                return ok;
            }
            Ok(o) => {
                last_err = o.error.or(Some("渠道返回失败但未给出原因".into()));
            }
            Err(e) => last_err = Some(e.to_string()),
        }
        if attempt < attempts {
            let backoff = 2u64.pow(attempt.min(4));
            tracing::warn!(
                "推送第 {attempt} 次失败（{}），{backoff} 秒后重试",
                last_err.clone().unwrap_or_default()
            );
            std::thread::sleep(std::time::Duration::from_secs(backoff));
        }
    }
    PushOutcome::fail(format!(
        "重试 {attempts} 次后仍失败：{}",
        last_err.unwrap_or_else(|| "未知原因".into())
    ))
}

// ---------------------------------------------------------------------------
// QQ：OneBot 11（自建，第三方协议）
// ---------------------------------------------------------------------------

/// OneBot 11 推送器。
///
/// 适配 NapCat / Lagrange / go-cqhttp 等实现，走它们的 HTTP 接口：
///
/// - 文本：`POST /send_private_msg` 或 `/send_group_msg`；
/// - 文件：`POST /upload_private_file` 或 `/upload_group_file`（按路径上传，
///   宿主与 OneBot 在同一台机器时最省事）。
///
/// ⚠️ OneBot 是**第三方协议实现**，自动化登录 QQ 存在账号风险，
/// 启用前会要求用户确认（见 [`Provider::risk_note`]）。
#[derive(Debug, Clone)]
pub struct OneBotPusher {
    /// 配置。
    pub cfg: PushConfig,
}

impl OneBotPusher {
    /// 目标 id：优先按数字发，非数字时退回字符串（部分实现接受字符串）。
    fn id_value(&self) -> serde_json::Value {
        let t = self.cfg.target.trim();
        match t.parse::<i64>() {
            Ok(n) => serde_json::json!(n),
            Err(_) => serde_json::json!(t),
        }
    }

    /// 调用一个 OneBot action。
    fn call(
        &self,
        action: &str,
        payload: serde_json::Value,
    ) -> Result<serde_json::Value, PushError> {
        let base = self.cfg.endpoint.trim().trim_end_matches('/');
        let url = format!("{base}/{action}");
        let mut headers = String::from("Content-Type: application/json\r\n");
        if !self.cfg.token.trim().is_empty() {
            headers.push_str(&format!(
                "Authorization: Bearer {}\r\n",
                self.cfg.token.trim()
            ));
        }
        let resp = http::request(
            "POST",
            &url,
            &headers,
            payload.to_string().as_bytes(),
            self.cfg.timeout_ms,
        )?;
        if !resp.is_success() {
            return Err(PushError::Rejected(format!(
                "HTTP {}: {}",
                resp.status,
                resp.body.chars().take(200).collect::<String>()
            )));
        }
        let v: serde_json::Value = serde_json::from_str(&resp.body)
            .map_err(|e| PushError::Rejected(format!("响应不是 JSON: {e}")))?;
        // OneBot 用 retcode 表示业务结果：0 = 成功
        if let Some(code) = v.get("retcode").and_then(|c| c.as_i64()) {
            if code != 0 {
                return Err(PushError::Rejected(format!(
                    "retcode={code} {}",
                    v.get("message").and_then(|m| m.as_str()).unwrap_or("")
                )));
            }
        }
        Ok(v)
    }
}

impl Pusher for OneBotPusher {
    fn provider(&self) -> Provider {
        Provider::OneBot
    }

    fn send(&self, doc: &PushDoc) -> Result<PushOutcome, PushError> {
        if self.cfg.endpoint.trim().is_empty() {
            return Err(PushError::Missing(
                "OneBot 服务地址（如 http://127.0.0.1:3000）".into(),
            ));
        }
        if self.cfg.target.trim().is_empty() {
            return Err(PushError::Missing("QQ 号或群号（push.target）".into()));
        }

        let group = self.cfg.is_group();
        let id_key = if group { "group_id" } else { "user_id" };
        let text = format!("{}\n\n{}", doc.title, doc.summary);

        let msg_action = if group {
            "send_group_msg"
        } else {
            "send_private_msg"
        };
        let mut payload = serde_json::json!({
            "message": [{ "type": "text", "data": { "text": text } }]
        });
        payload[id_key] = self.id_value();
        let sent = self.call(msg_action, payload)?;
        let message_id = sent
            .get("data")
            .and_then(|d| d.get("message_id"))
            .map(|m| m.to_string());

        // 文件是「尽力而为」：文本已经送达，文件失败不该让整次推送判定为失败，
        // 否则会触发「不删除本地录像」的策略，把磁盘占满。
        if let Some(path) = doc.attachment() {
            let filename = Path::new(path)
                .file_name()
                .map(|s| s.to_string_lossy().to_string())
                .unwrap_or_else(|| "课堂纪要".into());
            let up_action = if group {
                "upload_group_file"
            } else {
                "upload_private_file"
            };
            let mut up = serde_json::json!({ "file": path, "name": filename });
            up[id_key] = self.id_value();
            match self.call(up_action, up) {
                Ok(_) => {}
                Err(e) => tracing::warn!("OneBot 文件上传失败（文本已送达）：{e}"),
            }
        }

        Ok(PushOutcome::ok(
            message_id.unwrap_or_else(|| "onebot".into()),
        ))
    }
}

// ---------------------------------------------------------------------------
// QQ：官方机器人（QQ 开放平台 API v2）
// ---------------------------------------------------------------------------

/// QQ 官方机器人推送器。
///
/// 流程：`getAppAccessToken` 拿 access_token（缓存 2 小时）→
/// 发文本消息；文件走富媒体上传接口。
///
/// 注意两点：
/// 1. `target` 是**用户 openid 或群 openid**，不是 QQ 号 ——
///    QQ 官方平台不直接暴露 QQ 号，openid 需要从机器人收到的事件里取；
/// 2. 富媒体上传要求 `file_data`（base64）或可公网访问的 `url`，
///    本地文件只能走 base64，因此有体积上限，超限时降级为只发文本。
#[derive(Debug, Clone)]
pub struct QqOfficialPusher {
    /// 配置。
    pub cfg: PushConfig,
    /// access_token 缓存：`(token, 过期时刻)`。
    pub token_cache: std::sync::Arc<std::sync::Mutex<Option<(String, std::time::Instant)>>>,
}

impl QqOfficialPusher {
    /// 生产环境 API 基址。
    const DEFAULT_BASE: &'static str = "https://api.sgroup.qq.com";
    /// 换 token 的地址（与业务 API 不同域）。
    const TOKEN_URL: &'static str = "https://bots.qq.com/app/getAppAccessToken";

    /// 富媒体 base64 上限（官方限制较严，这里保守取 8 MB 原始字节）。
    const MAX_FILE_BYTES: u64 = 8 * 1024 * 1024;

    fn base(&self) -> String {
        let e = self.cfg.endpoint.trim();
        if e.is_empty() {
            Self::DEFAULT_BASE.to_string()
        } else {
            e.trim_end_matches('/').to_string()
        }
    }

    /// 取 access_token（带缓存）。
    fn access_token(&self) -> Result<String, PushError> {
        if let Ok(g) = self.token_cache.lock() {
            if let Some((t, exp)) = g.as_ref() {
                if *exp > std::time::Instant::now() {
                    return Ok(t.clone());
                }
            }
        }
        if self.cfg.app_id.trim().is_empty() || self.cfg.app_secret.trim().is_empty() {
            return Err(PushError::Missing(
                "QQ 官方机器人需要 app_id 与 app_secret".into(),
            ));
        }
        let payload = serde_json::json!({
            "appId": self.cfg.app_id.trim(),
            "clientSecret": self.cfg.app_secret.trim()
        })
        .to_string();
        let resp = http::post_json(Self::TOKEN_URL, &payload, self.cfg.timeout_ms)?;
        if !resp.is_success() {
            return Err(PushError::Rejected(format!(
                "获取 access_token 失败 HTTP {}: {}",
                resp.status,
                resp.body.chars().take(200).collect::<String>()
            )));
        }
        let v: serde_json::Value = serde_json::from_str(&resp.body)
            .map_err(|e| PushError::Rejected(format!("token 响应不是 JSON: {e}")))?;
        let token = v
            .get("access_token")
            .and_then(|t| t.as_str())
            .ok_or_else(|| {
                PushError::Rejected(format!(
                    "token 响应里没有 access_token: {}",
                    resp.body.chars().take(200).collect::<String>()
                ))
            })?
            .to_string();
        let expires = v
            .get("expires_in")
            .and_then(|x| x.as_str())
            .and_then(|s| s.parse::<u64>().ok())
            .or_else(|| v.get("expires_in").and_then(|x| x.as_u64()))
            .unwrap_or(7200);
        // 提前 5 分钟过期，避免边界上用了刚好失效的 token
        let ttl = expires.saturating_sub(300).max(60);
        if let Ok(mut g) = self.token_cache.lock() {
            *g = Some((
                token.clone(),
                std::time::Instant::now() + std::time::Duration::from_secs(ttl),
            ));
        }
        Ok(token)
    }

    /// 目标资源路径前缀。
    fn target_path(&self) -> String {
        if self.cfg.is_group() {
            format!("/v2/groups/{}", self.cfg.target.trim())
        } else {
            format!("/v2/users/{}", self.cfg.target.trim())
        }
    }

    fn post(&self, path: &str, payload: serde_json::Value) -> Result<serde_json::Value, PushError> {
        let token = self.access_token()?;
        let url = format!("{}{path}", self.base());
        let headers = format!("Content-Type: application/json\r\nAuthorization: QQBot {token}\r\n");
        let resp = http::request(
            "POST",
            &url,
            &headers,
            payload.to_string().as_bytes(),
            self.cfg.timeout_ms,
        )?;
        if !resp.is_success() {
            return Err(PushError::Rejected(format!(
                "HTTP {}: {}",
                resp.status,
                resp.body.chars().take(200).collect::<String>()
            )));
        }
        Ok(serde_json::from_str(&resp.body).unwrap_or(serde_json::Value::Null))
    }
}

impl Pusher for QqOfficialPusher {
    fn provider(&self) -> Provider {
        Provider::QqOfficial
    }

    fn send(&self, doc: &PushDoc) -> Result<PushOutcome, PushError> {
        if self.cfg.target.trim().is_empty() {
            return Err(PushError::Missing(
                "目标 openid（push.target，取自机器人收到的事件，不是 QQ 号）".into(),
            ));
        }
        let text = format!("{}\n\n{}", doc.title, doc.summary);

        // 1) 先发文本：这是最可靠的一步
        let mut sid = String::new();
        {
            let v = self.post(
                &format!("{}/messages", self.target_path()),
                serde_json::json!({ "content": text, "msg_type": 0 }),
            )?;
            if let Some(id) = v.get("id").and_then(|x| x.as_str()) {
                sid = id.to_string();
            }
        }

        // 2) 再尽力上传文件（失败只记日志，不影响整体判定）
        if let Some(path) = doc.attachment() {
            match std::fs::metadata(path) {
                Ok(m) if m.len() > Self::MAX_FILE_BYTES => {
                    tracing::warn!(
                        "附件 {:.1} MB 超过富媒体上传上限，仅发送文本",
                        m.len() as f64 / 1048576.0
                    );
                }
                Ok(_) => {
                    let filename = Path::new(path)
                        .file_name()
                        .map(|s| s.to_string_lossy().to_string())
                        .unwrap_or_else(|| "课堂纪要".into());
                    match std::fs::read(path) {
                        Ok(bytes) => {
                            let b64 = base64_encode(&bytes);
                            let up_path = format!("{}/files", self.target_path());
                            let payload = serde_json::json!({
                                "file_type": 4,           // 4 = 文件
                                "file_data": b64,
                                "file_name": filename,
                                "srv_send_msg": false
                            });
                            match self.post(&up_path, payload) {
                                Ok(v) => {
                                    let file_info = v
                                        .get("file_info")
                                        .and_then(|x| x.as_str())
                                        .unwrap_or_default()
                                        .to_string();
                                    if !file_info.is_empty() {
                                        // 富媒体消息 msg_type = 7
                                        let _ = self.post(
                                            &format!("{}/messages", self.target_path()),
                                            serde_json::json!({
                                                "content": " ",
                                                "msg_type": 7,
                                                "media": { "file_info": file_info }
                                            }),
                                        );
                                    }
                                }
                                Err(e) => tracing::warn!("QQ 官方富媒体上传失败：{e}"),
                            }
                        }
                        Err(e) => tracing::warn!("读取附件失败：{e}"),
                    }
                }
                Err(e) => tracing::warn!("读取附件元信息失败：{e}"),
            }
        }

        Ok(PushOutcome::ok(if sid.is_empty() {
            "qq-official".to_string()
        } else {
            sid
        }))
    }
}

/// 标准 base64 编码（不引第三方库：这里只有一处需求）。
pub fn base64_encode(data: &[u8]) -> String {
    const TABLE: &[u8; 64] = b"ABCDEFGHIJKLMNOPQRSTUVWXYZabcdefghijklmnopqrstuvwxyz0123456789+/";
    let mut out = String::with_capacity(data.len().div_ceil(3) * 4);
    for chunk in data.chunks(3) {
        let b = [
            chunk[0],
            *chunk.get(1).unwrap_or(&0),
            *chunk.get(2).unwrap_or(&0),
        ];
        let n = ((b[0] as u32) << 16) | ((b[1] as u32) << 8) | b[2] as u32;
        out.push(TABLE[((n >> 18) & 63) as usize] as char);
        out.push(TABLE[((n >> 12) & 63) as usize] as char);
        out.push(if chunk.len() > 1 {
            TABLE[((n >> 6) & 63) as usize] as char
        } else {
            '='
        });
        out.push(if chunk.len() > 2 {
            TABLE[(n & 63) as usize] as char
        } else {
            '='
        });
    }
    out
}

// ---------------------------------------------------------------------------
// 企业微信机器人
// ---------------------------------------------------------------------------

/// 企业微信机器人推送器。
///
/// 流程：先 `upload_media` 拿到 `media_id`，再以 `msgtype=file` 发送；
/// 若上传失败则降级为 markdown 文本消息（保证内容至少能送达）。
#[derive(Debug, Clone)]
pub struct WeComPusher {
    /// 配置。
    pub cfg: PushConfig,
}

impl WeComPusher {
    fn send_markdown(&self, doc: &PushDoc) -> Result<PushOutcome, PushError> {
        let content = escape_markdown(&format!("**{}**\n\n{}", doc.title, doc.summary));
        let payload = serde_json::json!({
            "msgtype": "markdown",
            "markdown": { "content": content }
        })
        .to_string();
        let resp = http::post_json(&self.cfg.endpoint, &payload, self.cfg.timeout_ms)?;
        check_wecom(&resp)
    }
}

impl Pusher for WeComPusher {
    fn provider(&self) -> Provider {
        Provider::WeCom
    }

    fn send(&self, doc: &PushDoc) -> Result<PushOutcome, PushError> {
        if self.cfg.endpoint.trim().is_empty() {
            return Err(PushError::Missing("企业微信 Webhook 地址".into()));
        }

        let Some(path) = doc.attachment() else {
            return self.send_markdown(doc);
        };
        let data = std::fs::read(path).map_err(|e| PushError::Io(format!("{path}: {e}")))?;
        let filename = Path::new(path)
            .file_name()
            .map(|s| s.to_string_lossy().to_string())
            .unwrap_or_else(|| "课堂纪要".into());

        // 1) 上传临时素材
        let upload_url = format!("{}?type=file", self.cfg.endpoint.trim_end_matches('&'));
        let up = http::post_multipart(
            &with_type_param(&self.cfg.endpoint, "upload_media"),
            &[],
            Some(("media", &filename, &data)),
            self.cfg.timeout_ms,
        );
        let _ = upload_url;

        let media_id = match up {
            Ok(r) if r.is_success() => serde_json::from_str::<serde_json::Value>(&r.body)
                .ok()
                .and_then(|v| v.get("media_id").and_then(|m| m.as_str()).map(String::from)),
            _ => None,
        };

        match media_id {
            Some(id) => {
                let payload = serde_json::json!({
                    "msgtype": "file",
                    "file": { "media_id": id }
                })
                .to_string();
                let resp = http::post_json(&self.cfg.endpoint, &payload, self.cfg.timeout_ms)?;
                let out = check_wecom(&resp)?;
                if out.success {
                    Ok(out)
                } else {
                    // 发送文件失败则退回文本
                    self.send_markdown(doc)
                }
            }
            // 上传失败 -> 直接发文本
            None => self.send_markdown(doc),
        }
    }
}

/// 企业微信上传素材的地址与发消息地址不同，这里做一次转换。
///
/// 传入的 webhook 形如
/// `https://qyapi.weixin.qq.com/cgi-bin/webhook/send?key=XXX`，
/// 上传接口是 `.../webhook/upload_media?key=XXX&type=file`。
fn with_type_param(webhook: &str, _kind: &str) -> String {
    let base = webhook.replace("/webhook/send", "/webhook/upload_media");
    if base.contains("type=") {
        base
    } else if base.contains('?') {
        format!("{base}&type=file")
    } else {
        format!("{base}?type=file")
    }
}

/// 解析企业微信响应（`errcode == 0` 即成功）。
fn check_wecom(resp: &http::HttpResponse) -> Result<PushOutcome, PushError> {
    if !resp.is_success() {
        return Err(PushError::Rejected(format!("HTTP {}", resp.status)));
    }
    let v: serde_json::Value = serde_json::from_str(&resp.body).unwrap_or(serde_json::Value::Null);
    let code = v.get("errcode").and_then(|c| c.as_i64()).unwrap_or(0);
    if code == 0 {
        Ok(PushOutcome::ok(
            v.get("msgid").and_then(|m| m.as_str()).unwrap_or("ok"),
        ))
    } else {
        let msg = v
            .get("errmsg")
            .and_then(|m| m.as_str())
            .unwrap_or("未知错误");
        Err(PushError::Rejected(format!("errcode={code}: {msg}")))
    }
}

fn escape_markdown(s: &str) -> String {
    s.replace('\r', "")
}

// ---------------------------------------------------------------------------
// 通用 Webhook
// ---------------------------------------------------------------------------

/// 通用 Webhook：POST 一段 JSON。
#[derive(Debug, Clone)]
pub struct WebhookPusher {
    /// 配置。
    pub cfg: PushConfig,
}

impl Pusher for WebhookPusher {
    fn provider(&self) -> Provider {
        Provider::Webhook
    }

    fn send(&self, doc: &PushDoc) -> Result<PushOutcome, PushError> {
        if self.cfg.endpoint.trim().is_empty() {
            return Err(PushError::Missing("Webhook 地址".into()));
        }
        let payload = serde_json::json!({
            "title": doc.title,
            "summary": doc.summary,
            "docx": doc.docx,
            "pdf": doc.pdf,
            "target": doc.target,
        })
        .to_string();
        let resp = http::post_json(&self.cfg.endpoint, &payload, self.cfg.timeout_ms)?;
        if resp.is_success() {
            Ok(PushOutcome::ok(format!("HTTP {}", resp.status)))
        } else {
            Err(PushError::Rejected(format!(
                "HTTP {}: {}",
                resp.status,
                resp.body.chars().take(200).collect::<String>()
            )))
        }
    }
}

// ---------------------------------------------------------------------------
// Server 酱
// ---------------------------------------------------------------------------

/// Server 酱推送器（表单 POST）。
#[derive(Debug, Clone)]
pub struct ServerChanPusher {
    /// 配置。
    pub cfg: PushConfig,
}

impl Pusher for ServerChanPusher {
    fn provider(&self) -> Provider {
        Provider::ServerChan
    }

    fn send(&self, doc: &PushDoc) -> Result<PushOutcome, PushError> {
        let send_key = if self.cfg.token.trim().is_empty() {
            self.cfg
                .endpoint
                .rsplit('/')
                .next()
                .unwrap_or("")
                .to_string()
        } else {
            self.cfg.token.clone()
        };
        if send_key.trim().is_empty() {
            return Err(PushError::Missing("Server 酱 SendKey".into()));
        }
        let url = format!("https://sctapi.ftqq.com/{send_key}.send");
        let form = format!(
            "title={}&desp={}",
            http::urlencode(&doc.title),
            http::urlencode(&format!(
                "{}\n\n附件：{}",
                doc.summary,
                doc.attachment().unwrap_or("无")
            ))
        );
        let resp = http::post_form(&url, &form, self.cfg.timeout_ms)?;
        if resp.is_success() {
            Ok(PushOutcome::ok(format!("HTTP {}", resp.status)))
        } else {
            Err(PushError::Rejected(format!("HTTP {}", resp.status)))
        }
    }
}

// ---------------------------------------------------------------------------
// 第三方个人微信（需外部服务）
// ---------------------------------------------------------------------------

/// 个人微信第三方协议：转发给用户自建的中转服务。
///
/// 本项目**不内置**任何非官方协议实现，只把内容以通用格式转发给用户自己的服务，
/// 由用户自行承担合规与风控风险（风险提示见 [`Provider::risk_note`]）。
#[derive(Debug, Clone)]
pub struct WeChatPersonalPusher {
    /// 配置。
    pub cfg: PushConfig,
}

impl Pusher for WeChatPersonalPusher {
    fn provider(&self) -> Provider {
        Provider::WeChatPersonal
    }

    fn send(&self, doc: &PushDoc) -> Result<PushOutcome, PushError> {
        if self.cfg.endpoint.trim().is_empty() {
            return Err(PushError::Missing(
                "第三方中转服务地址（本程序不内置协议实现）".into(),
            ));
        }
        let payload = serde_json::json!({
            "token": self.cfg.token,
            "target": doc.target,
            "title": doc.title,
            "text": doc.summary,
            "docx": doc.docx,
            "pdf": doc.pdf,
        })
        .to_string();
        let resp = http::post_json(&self.cfg.endpoint, &payload, self.cfg.timeout_ms)?;
        if resp.is_success() {
            Ok(PushOutcome::ok(format!("HTTP {}", resp.status)))
        } else {
            Err(PushError::Rejected(format!("HTTP {}", resp.status)))
        }
    }
}

// ---------------------------------------------------------------------------
// Dry-run
// ---------------------------------------------------------------------------

/// 只记录不发送。
#[derive(Debug, Clone, Copy, Default)]
pub struct DryRunPusher;

impl Pusher for DryRunPusher {
    fn provider(&self) -> Provider {
        Provider::DryRun
    }

    fn send(&self, doc: &PushDoc) -> Result<PushOutcome, PushError> {
        tracing::info!(
            "[dry-run 推送] {} | 附件={:?} | 摘要 {} 字",
            doc.title,
            doc.attachment(),
            doc.summary.chars().count()
        );
        Ok(PushOutcome::ok("dry-run"))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn provider_parse_aliases() {
        assert_eq!(Provider::parse("wecom"), Some(Provider::WeCom));
        assert_eq!(Provider::parse("企业微信"), Some(Provider::WeCom));
        assert_eq!(Provider::parse("ServerChan"), Some(Provider::ServerChan));
        assert_eq!(Provider::parse(""), Some(Provider::DryRun));
        assert_eq!(Provider::parse("nope"), None);
    }

    #[test]
    fn official_providers_need_no_risk_note() {
        for p in [Provider::WeCom, Provider::QqOfficial, Provider::ServerChan] {
            assert!(p.is_official(), "{p:?} 应视为官方渠道");
            assert!(p.risk_note().is_none(), "{p:?} 不该有风险提示");
        }
    }

    #[test]
    fn provider_parses_qq_aliases() {
        assert_eq!(Provider::parse("qq"), Some(Provider::QqOfficial));
        assert_eq!(Provider::parse("官方QQ"), Some(Provider::QqOfficial));
        assert_eq!(Provider::parse("qq-official"), Some(Provider::QqOfficial));
        for alias in [
            "onebot",
            "onebot11",
            "napcat",
            "lagrange",
            "gocqhttp",
            "第三方QQ",
        ] {
            assert_eq!(
                Provider::parse(alias),
                Some(Provider::OneBot),
                "别名 {alias}"
            );
        }
    }

    #[test]
    fn onebot_is_marked_third_party() {
        // OneBot 走第三方协议登录 QQ，必须带风险提示
        assert!(!Provider::OneBot.is_official());
        assert!(Provider::OneBot.risk_note().is_some());
    }

    #[test]
    fn base64_matches_rfc4648_vectors() {
        assert_eq!(base64_encode(b""), "");
        assert_eq!(base64_encode(b"f"), "Zg==");
        assert_eq!(base64_encode(b"fo"), "Zm8=");
        assert_eq!(base64_encode(b"foo"), "Zm9v");
        assert_eq!(base64_encode(b"foob"), "Zm9vYg==");
        assert_eq!(base64_encode(b"fooba"), "Zm9vYmE=");
        assert_eq!(base64_encode(b"foobar"), "Zm9vYmFy");
        // 中文按 UTF-8 字节编码
        assert_eq!(base64_encode("中".as_bytes()), "5Lit");
    }

    #[test]
    fn onebot_requires_endpoint_and_target() {
        let mut cfg = PushConfig {
            provider: Provider::OneBot,
            ..Default::default()
        };
        let p = make_pusher(&cfg);
        let d = PushDoc {
            title: "t".into(),
            summary: "s".into(),
            docx: None,
            pdf: None,
            target: None,
        };
        // 没填服务地址时必须在发请求前就报错
        assert!(matches!(p.send(&d), Err(PushError::Missing(_))));

        cfg.endpoint = "http://127.0.0.1:3000".into();
        let p2 = make_pusher(&cfg);
        assert!(matches!(p2.send(&d), Err(PushError::Missing(_))));
    }

    #[test]
    fn qq_official_requires_target_before_token() {
        let cfg = PushConfig {
            provider: Provider::QqOfficial,
            ..Default::default()
        };
        let p = make_pusher(&cfg);
        let d = PushDoc {
            title: "t".into(),
            summary: "s".into(),
            docx: None,
            pdf: None,
            target: None,
        };
        // 目标为空 → 连换 token 都不该尝试
        assert!(matches!(p.send(&d), Err(PushError::Missing(_))));
    }

    #[test]
    fn qq_official_missing_credentials_is_reported() {
        let cfg = PushConfig {
            provider: Provider::QqOfficial,
            target: "openid-x".into(),
            ..Default::default()
        };
        let p = make_pusher(&cfg);
        let d = PushDoc {
            title: "t".into(),
            summary: "s".into(),
            docx: None,
            pdf: None,
            target: None,
        };
        // 有目标但缺 app_id/app_secret —— 应在换 token 这一步就明确报缺配置
        match p.send(&d) {
            Err(PushError::Missing(m)) => assert!(m.contains("app_id")),
            other => panic!("应报缺配置，实际: {other:?}"),
        }
    }

    #[test]
    fn onebot_id_is_numeric_when_possible() {
        let cfg = PushConfig {
            target: "123456".into(),
            ..Default::default()
        };
        let p = OneBotPusher { cfg };
        assert!(p.id_value().is_number());
        // 非数字（某些实现用字符串 id）也不能崩，退回字符串
        let cfg2 = PushConfig {
            target: "abc".into(),
            ..Default::default()
        };
        let p2 = OneBotPusher { cfg: cfg2 };
        assert!(p2.id_value().is_string());
    }

    #[test]
    fn group_target_type_is_recognised() {
        let cfg = PushConfig {
            target_type: "GROUP".into(),
            ..Default::default()
        };
        assert!(cfg.is_group());
        let cfg2 = PushConfig::default();
        assert!(!cfg2.is_group());
    }

    /// 永远失败的推送器，用来测重试逻辑（不发网络请求，所以测试很快）。
    struct AlwaysFail;

    impl Pusher for AlwaysFail {
        fn provider(&self) -> Provider {
            Provider::DryRun
        }
        fn send(&self, _doc: &PushDoc) -> Result<PushOutcome, PushError> {
            Ok(PushOutcome::fail("故意失败"))
        }
    }

    #[test]
    fn retry_with_zero_extra_attempts_reports_failure() {
        let d = PushDoc {
            title: "t".into(),
            summary: "s".into(),
            docx: None,
            pdf: None,
            target: None,
        };
        // max_retries = 0 → 只尝试一次且不 sleep，避免拖慢测试
        let out = send_with_retry(&AlwaysFail, &d, 0);
        assert!(!out.success);
        let err = out.error.unwrap();
        assert!(err.contains("1 次"), "应说明尝试次数: {err}");
        assert!(err.contains("故意失败"), "应保留原始失败原因: {err}");
    }

    #[test]
    fn dry_run_succeeds_through_retry_helper() {
        let d = PushDoc {
            title: "t".into(),
            summary: "s".into(),
            docx: None,
            pdf: None,
            target: None,
        };
        let out = send_with_retry(&DryRunPusher, &d, 3);
        assert!(out.success);
    }

    #[test]
    fn attachment_prefers_pdf() {
        let d = PushDoc {
            title: "t".into(),
            summary: "s".into(),
            docx: Some("a.docx".into()),
            pdf: Some("a.pdf".into()),
            target: None,
        };
        assert_eq!(d.attachment(), Some("a.pdf"));

        let d2 = PushDoc {
            docx: Some("a.docx".into()),
            pdf: None,
            ..d
        };
        assert_eq!(d2.attachment(), Some("a.docx"));
    }

    #[test]
    fn dry_run_always_succeeds() {
        let p = DryRunPusher;
        let d = PushDoc {
            title: "课堂纪要".into(),
            summary: "重点：xxx".into(),
            docx: None,
            pdf: None,
            target: None,
        };
        let out = p.send(&d).unwrap();
        assert!(out.success);
    }

    #[test]
    fn wecom_requires_endpoint() {
        let p = WeComPusher {
            cfg: PushConfig::default(),
        };
        let d = PushDoc {
            title: "t".into(),
            summary: "s".into(),
            docx: None,
            pdf: None,
            target: None,
        };
        assert!(matches!(p.send(&d), Err(PushError::Missing(_))));
    }

    #[test]
    fn upload_url_is_rewritten() {
        let u = with_type_param(
            "https://qyapi.weixin.qq.com/cgi-bin/webhook/send?key=K",
            "file",
        );
        assert!(u.contains("/webhook/upload_media"));
        assert!(u.contains("key=K"));
        assert!(u.contains("type=file"));
    }

    #[test]
    fn wecom_response_parsing() {
        let ok = http::HttpResponse {
            status: 200,
            body: r#"{"errcode":0,"errmsg":"ok","msgid":"m1"}"#.into(),
        };
        assert!(check_wecom(&ok).unwrap().success);

        let bad = http::HttpResponse {
            status: 200,
            body: r#"{"errcode":93000,"errmsg":"invalid webhook url"}"#.into(),
        };
        assert!(check_wecom(&bad).is_err());
    }
}
