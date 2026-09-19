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
            "webhook" | "generic" => Some(Self::Webhook),
            "serverchan" | "server_chan" | "sct" => Some(Self::ServerChan),
            "wechat-personal" | "wechat_personal" | "个人微信" => Some(Self::WeChatPersonal),
            "dryrun" | "dry-run" | "none" | "" => Some(Self::DryRun),
            _ => None,
        }
    }

    /// 是否官方渠道。
    pub fn is_official(self) -> bool {
        matches!(self, Self::WeCom)
    }

    /// 第三方渠道的风险提示（策划书附录 C）。
    pub fn risk_note(self) -> Option<&'static str> {
        match self {
            Self::WeChatPersonal => {
                Some("使用第三方协议存在账号封禁风险，请自行评估；建议使用小号并定期更换密码。")
            }
            _ => None,
        }
    }

    /// 显示名。
    pub fn display(self) -> &'static str {
        match self {
            Self::WeCom => "企业微信机器人",
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
    /// Webhook / API 地址。
    pub endpoint: String,
    /// 令牌（企业微信为 key，Server 酱为 SendKey）。
    pub token: String,
    /// 超时（毫秒）。
    pub timeout_ms: i32,
}

impl Default for PushConfig {
    fn default() -> Self {
        Self {
            provider: Provider::DryRun,
            endpoint: String::new(),
            token: String::new(),
            timeout_ms: 20_000,
        }
    }
}

/// 按配置创建推送器。
pub fn make_pusher(cfg: &PushConfig) -> Box<dyn Pusher + Send + Sync> {
    match cfg.provider {
        Provider::WeCom => Box::new(WeComPusher { cfg: cfg.clone() }),
        Provider::Webhook => Box::new(WebhookPusher { cfg: cfg.clone() }),
        Provider::ServerChan => Box::new(ServerChanPusher { cfg: cfg.clone() }),
        Provider::WeChatPersonal => Box::new(WeChatPersonalPusher { cfg: cfg.clone() }),
        Provider::DryRun => Box::new(DryRunPusher),
    }
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
    fn only_wecom_is_official() {
        assert!(Provider::WeCom.is_official());
        assert!(!Provider::WeChatPersonal.is_official());
    }

    #[test]
    fn third_party_has_risk_note() {
        assert!(Provider::WeChatPersonal.risk_note().is_some());
        assert!(Provider::WeCom.risk_note().is_none());
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
