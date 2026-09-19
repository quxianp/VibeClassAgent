//! 内容要点提取：调用用户自填的 OpenAI 兼容接口。
//!
//! 设计（策划书 3.2.2 / 5.3）：
//! - **只上传转写文本**，绝不上传音视频；
//! - 用 JSON 约束输出结构，解析失败自动重试一次；
//! - 超长文本按段落切分，分段提取后再合并，规避上下文长度限制；
//! - 未配置 API 时返回 [`LlmError::NotConfigured`]，调用方应优雅降级（不阻塞后续清理）。

use crate::http;

/// 一节课的要点。
#[derive(Debug, Clone, Default, serde::Serialize, serde::Deserialize, PartialEq)]
pub struct LessonSummary {
    /// 标题（模型生成）。
    #[serde(default)]
    pub title: String,
    /// 课堂重点。
    #[serde(default)]
    pub points: Vec<String>,
    /// 重点通知（作业、考试、注意事项）。
    #[serde(default)]
    pub notices: Vec<String>,
    /// 关键词。
    #[serde(default)]
    pub keywords: Vec<String>,
}

/// 提取模式：用户自己选花钱还是免费。
///
/// | 模式 | 成本 | 稳定性 | 说明 |
/// |---|---|---|---|
/// | [`LlmMode::PaidApi`] | 按量计费 | 高 | 官方付费接口 |
/// | [`LlmMode::FreeApi`] | 0 | 高 | 官方**免费额度**（同一套协议，只是换了模型名） |
/// | [`LlmMode::BrowserBot`] | 0 | 中低 | 驱动网页版聊天机器人，见 [`crate::browser_bot`] |
///
/// 前两者共用同一条 HTTP 代码路径，差别只是 base_url 与 model —— 这也是
/// 「免费」里唯一真正稳的一条：厂商改政策你改个模型名就行，不用改代码。
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum LlmMode {
    /// 官方付费接口（默认）。
    #[default]
    PaidApi,
    /// 官方免费额度模型。
    FreeApi,
    /// 浏览器自动化驱动网页版聊天机器人（实验特性，需显式确认风险）。
    BrowserBot,
}

impl LlmMode {
    /// 从配置字符串解析。
    pub fn parse(s: &str) -> Self {
        match s.trim().to_ascii_lowercase().as_str() {
            "free-api" | "free_api" | "freeapi" | "免费api" => Self::FreeApi,
            "browser-bot" | "browser_bot" | "browser" | "chatbot" | "网页" => Self::BrowserBot,
            _ => Self::PaidApi,
        }
    }

    /// 配置字符串。
    pub fn as_str(self) -> &'static str {
        match self {
            Self::PaidApi => "paid-api",
            Self::FreeApi => "free-api",
            Self::BrowserBot => "browser-bot",
        }
    }
}

/// 厂商预设：帮用户把 base_url 与常见模型名填好，省得查文档。
///
/// 说明：这些地址与模型名**以厂商当前文档为准**，各家调整频繁；
/// 预设只作为起点，用户随时可以在配置里覆盖。
#[derive(Debug, Clone, Copy)]
pub struct ProviderPreset {
    /// 配置里写的 id。
    pub id: &'static str,
    /// 显示名。
    pub name: &'static str,
    /// OpenAI 兼容的 base url。
    pub base_url: &'static str,
    /// 常见的付费模型。
    pub paid_models: &'static [&'static str],
    /// 有免费额度（或本身免费）的模型。
    pub free_models: &'static [&'static str],
    /// 说明。
    pub note: &'static str,
}

/// 内置厂商预设表。
pub const PROVIDERS: &[ProviderPreset] = &[
    ProviderPreset {
        id: "deepseek",
        name: "DeepSeek",
        base_url: "https://api.deepseek.com/v1",
        paid_models: &["deepseek-chat", "deepseek-reasoner"],
        free_models: &[],
        note: "价格低，中文课堂纪要表现好",
    },
    ProviderPreset {
        id: "zhipu",
        name: "智谱 GLM",
        base_url: "https://open.bigmodel.cn/api/paas/v4",
        paid_models: &["glm-4-plus", "glm-4-air"],
        free_models: &["glm-4-flash"],
        note: "glm-4-flash 免费，适合日常纪要",
    },
    ProviderPreset {
        id: "dashscope",
        name: "阿里通义千问",
        base_url: "https://dashscope.aliyuncs.com/compatible-mode/v1",
        paid_models: &["qwen-plus", "qwen-max"],
        free_models: &["qwen-turbo"],
        note: "通义系列有免费额度",
    },
    ProviderPreset {
        id: "moonshot",
        name: "月之暗面 Kimi",
        base_url: "https://api.moonshot.cn/v1",
        paid_models: &["moonshot-v1-8k", "moonshot-v1-32k"],
        free_models: &[],
        note: "长文本能力强",
    },
    ProviderPreset {
        id: "siliconflow",
        name: "硅基流动",
        base_url: "https://api.siliconflow.cn/v1",
        paid_models: &["deepseek-ai/DeepSeek-V3"],
        free_models: &["Qwen/Qwen2.5-7B-Instruct"],
        note: "聚合平台，有免费模型",
    },
    ProviderPreset {
        id: "openai",
        name: "OpenAI",
        base_url: "https://api.openai.com/v1",
        paid_models: &["gpt-4o-mini", "gpt-4o"],
        free_models: &[],
        note: "国内直连需代理",
    },
    ProviderPreset {
        id: "groq",
        name: "Groq",
        base_url: "https://api.groq.com/openai/v1",
        paid_models: &[],
        free_models: &["llama-3.3-70b-versatile"],
        note: "有较宽松的免费额度，国内直连需代理",
    },
    ProviderPreset {
        id: "openrouter",
        name: "OpenRouter",
        base_url: "https://openrouter.ai/api/v1",
        paid_models: &["anthropic/claude-3.5-sonnet"],
        free_models: &["deepseek/deepseek-r1:free"],
        note: "聚合平台，:free 后缀的模型免费",
    },
    ProviderPreset {
        id: "ollama",
        name: "Ollama（本机）",
        base_url: "http://127.0.0.1:11434/v1",
        paid_models: &[],
        free_models: &["qwen2.5:7b", "llama3.1:8b"],
        note: "完全离线、零成本，需要本机已装 Ollama 且内存足够",
    },
    ProviderPreset {
        id: "custom",
        name: "自定义（任何 OpenAI 兼容服务）",
        base_url: "",
        paid_models: &[],
        free_models: &[],
        note: "自己填 base_url 与 model；one-api / new-api 网关也走这里",
    },
];

/// 按 id 查预设。
pub fn provider_preset(id: &str) -> Option<&'static ProviderPreset> {
    PROVIDERS
        .iter()
        .find(|p| p.id.eq_ignore_ascii_case(id.trim()))
}

/// LLM 配置。
#[derive(Debug, Clone, Default)]
pub struct LlmConfig {
    /// 提取模式。
    pub mode: LlmMode,
    /// 厂商 id（用于查预设；留空则直接用下面的 base_url / model）。
    pub provider: String,
    /// OpenAI 兼容 base url，例如 `https://api.deepseek.com/v1`。
    pub base_url: String,
    /// API Key。
    pub api_key: String,
    /// 模型名。
    pub model: String,
    /// 超时。
    pub timeout_ms: i32,
    /// 单次请求最大字符数（超出则分段）。
    pub max_chars: usize,
    /// 浏览器自动化配置（仅 [`LlmMode::BrowserBot`] 使用）。
    pub browser: crate::browser_bot::BrowserBotConfig,
}

/// 判断一个配置值是不是**模板占位符**。
///
/// 配置模板里写的是 `<模型名>`、`<你的模型服务地址>` 这类东西。
/// 用户没改时它们既不是空串、也不是合法值 —— 不识别的话，
/// 界面会显示成「已配置」，直到课后处理调用的那一刻才失败。
pub fn is_placeholder(s: &str) -> bool {
    let t = s.trim();
    if t.is_empty() {
        return false;
    }
    if t.starts_with('<') && t.ends_with('>') {
        return true;
    }
    t.contains("<你的") || t.starts_with("你的")
}

impl LlmConfig {
    /// 是否已配置完整（按模式判断）。
    ///
    /// 注意会**排除模板占位符**：配置模板里留的是 `<模型名>`、
    /// `<你的模型服务地址>`，用户没改时它们既非空、也不是合法值。
    /// 不排除的话，界面上会显示成「已配置：<模型名>」——
    /// 看着像配好了，实际一调用就失败。
    pub fn is_configured(&self) -> bool {
        match self.mode {
            LlmMode::BrowserBot => self.browser.is_ready(),
            _ => {
                let base = self.effective_base_url();
                !base.trim().is_empty()
                    && !is_placeholder(&base)
                    && !self.api_key.trim().is_empty()
                    && !self.model.trim().is_empty()
                    && !is_placeholder(&self.model)
            }
        }
    }

    /// 实际使用的 base_url：显式填了就用，否则从预设取。
    pub fn effective_base_url(&self) -> String {
        let own = self.base_url.trim();
        if !own.is_empty() {
            return own.to_string();
        }
        provider_preset(&self.provider)
            .map(|p| p.base_url.to_string())
            .unwrap_or_default()
    }

    /// 补全 `/chat/completions` 端点。
    pub fn endpoint(&self) -> String {
        let base = self.effective_base_url();
        let base = base.trim().trim_end_matches('/');
        if base.ends_with("/chat/completions") {
            base.to_string()
        } else {
            format!("{base}/chat/completions")
        }
    }

    /// 补全 `/models` 端点（用于拉取可用模型名）。
    pub fn models_endpoint(&self) -> String {
        let base = self.effective_base_url();
        let base = base.trim().trim_end_matches('/');
        // 有的服务把 base_url 直接写到了 /chat/completions，这里要剥回去
        let base = base.strip_suffix("/chat/completions").unwrap_or(base);
        format!("{base}/models")
    }
}

/// 拉取可用模型列表。
///
/// # 为什么需要这个
///
/// 模型名是最容易填错的一项：`deepseek-chat` 少个横杠、把 `glm-4-flash`
/// 写成 `glm4-flash`，后果是**课后处理时才发现调用失败** —— 而那时课已经录完了。
/// 直接问服务端要列表让用户挑，从源头上就没有拼错的机会。
///
/// # 兼容性
///
/// OpenAI 兼容服务的标准响应是 `{"data":[{"id":"..."}]}`；
/// 少数实现会包成 `{"models":[{"id"/"name":...}]}`，或者直接给字符串数组，
/// 这里都认。拉不到就返回空列表，由调用方决定是否退回手填。
pub fn list_models(cfg: &LlmConfig) -> Result<Vec<String>, LlmError> {
    if cfg.api_key.trim().is_empty() || cfg.effective_base_url().trim().is_empty() {
        return Err(LlmError::NotConfigured);
    }
    let headers = format!("Authorization: Bearer {}\r\n", cfg.api_key.trim());
    let resp = http::request("GET", &cfg.models_endpoint(), &headers, b"", 30_000)?;
    if !resp.is_success() {
        return Err(LlmError::Api(format!(
            "HTTP {}: {}",
            resp.status,
            resp.body.chars().take(200).collect::<String>()
        )));
    }
    let v: serde_json::Value =
        serde_json::from_str(&resp.body).map_err(|e| LlmError::Parse(e.to_string()))?;

    let mut out: Vec<String> = Vec::new();
    for key in ["data", "models"] {
        let Some(arr) = v.get(key).and_then(|d| d.as_array()) else {
            continue;
        };
        for item in arr {
            // 三种常见形状：{"id":..} / {"name":..} / 直接是字符串
            let name = item
                .get("id")
                .or_else(|| item.get("name"))
                .and_then(|x| x.as_str())
                .or_else(|| item.as_str());
            if let Some(n) = name {
                let n = n.trim();
                if !n.is_empty() {
                    out.push(n.to_string());
                }
            }
        }
        if !out.is_empty() {
            break;
        }
    }
    out.sort();
    out.dedup();
    Ok(out)
}

/// 提取失败原因。
#[derive(Debug, thiserror::Error)]
pub enum LlmError {
    /// 未配置。
    #[error("未配置模型 API（缺少 base_url / api_key / model 之一）")]
    NotConfigured,
    /// 网络错误。
    #[error("网络错误: {0}")]
    Http(#[from] http::HttpError),
    /// 接口返回错误。
    #[error("接口错误: {0}")]
    Api(String),
    /// 返回内容无法解析成要点结构。
    #[error("无法解析模型输出: {0}")]
    Parse(String),
}

/// 默认的系统提示词。
pub const SYSTEM_PROMPT: &str = "\
你是一位严谨的课堂纪要整理助手。用户会给你一节课的语音转写文本（可能含口语噪声与识别错误）。\
请提取结构化要点，并**只输出 JSON**，不要输出任何解释文字。\
JSON 结构：{\"title\":\"本课标题\",\"points\":[\"课堂重点1\",\"课堂重点2\"],\"notices\":[\"通知1\"],\"keywords\":[\"关键词\"]}。\
要求：points 3-8 条，覆盖知识点/公式/例题；notices 仅保留教师明确交代的作业、考试、注意事项（没有就给空数组）；\
合并重复内容，去口语化，每条不超过 40 字。";

/// 把长文本切成若干段（按行边界，尽量不截断句子）。
pub fn chunk_text(text: &str, max_chars: usize) -> Vec<String> {
    if max_chars == 0 || text.chars().count() <= max_chars {
        return vec![text.to_string()];
    }
    let mut out: Vec<String> = Vec::new();
    let mut cur = String::new();
    for line in text.lines() {
        if cur.chars().count() + line.chars().count() + 1 > max_chars && !cur.is_empty() {
            out.push(std::mem::take(&mut cur));
        }
        if !cur.is_empty() {
            cur.push('\n');
        }
        cur.push_str(line);
    }
    if !cur.is_empty() {
        out.push(cur);
    }
    out
}

/// 从模型回复中抠出 JSON（容忍 ```json 包裹与前后废话）。
pub fn extract_json(s: &str) -> Option<serde_json::Value> {
    let t = s.trim();
    let t = t
        .strip_prefix("```json")
        .or_else(|| t.strip_prefix("```"))
        .unwrap_or(t);
    let t = t.strip_suffix("```").unwrap_or(t).trim();
    if let Ok(v) = serde_json::from_str::<serde_json::Value>(t) {
        return Some(v);
    }
    // 退而求其次：截取第一个 { 到最后一个 }
    let start = t.find('{')?;
    let end = t.rfind('}')?;
    if end > start {
        if let Ok(v) = serde_json::from_str::<serde_json::Value>(&t[start..=end]) {
            return Some(v);
        }
    }
    None
}

/// 把 JSON 值转成 [`LessonSummary`]（字段缺失时给默认值）。
pub fn parse_summary(v: &serde_json::Value) -> LessonSummary {
    let arr = |k: &str| -> Vec<String> {
        v.get(k)
            .and_then(|x| x.as_array())
            .map(|a| {
                a.iter()
                    .filter_map(|i| i.as_str().map(|s| s.trim().to_string()))
                    .filter(|s| !s.is_empty())
                    .collect()
            })
            .unwrap_or_default()
    };
    LessonSummary {
        title: v
            .get("title")
            .and_then(|x| x.as_str())
            .unwrap_or("")
            .trim()
            .to_string(),
        points: arr("points"),
        notices: arr("notices"),
        keywords: arr("keywords"),
    }
}

/// 单次调用 LLM（按 [`LlmConfig::mode`] 分派）。
pub fn complete(cfg: &LlmConfig, system: &str, user: &str) -> Result<String, LlmError> {
    match cfg.mode {
        // 网页版聊天机器人：不花 API 钱，但要驱动浏览器，慢且脆弱，
        // 所以它是一条独立路径，出问题时也只影响这一个模式。
        LlmMode::BrowserBot => {
            if !cfg.browser.is_ready() {
                return Err(LlmError::NotConfigured);
            }
            crate::browser_bot::ask(&cfg.browser, system, user)
                .map_err(|e| LlmError::Api(format!("浏览器模式失败: {e}")))
        }
        _ => complete_api(cfg, system, user),
    }
}

/// 走 HTTP 的单次调用（付费接口与免费额度共用这一条路径）。
fn complete_api(cfg: &LlmConfig, system: &str, user: &str) -> Result<String, LlmError> {
    if !cfg.is_configured() {
        return Err(LlmError::NotConfigured);
    }
    let payload = serde_json::json!({
        "model": cfg.model,
        "messages": [
            { "role": "system", "content": system },
            { "role": "user", "content": user }
        ],
        "temperature": 0.2,
        "stream": false
    })
    .to_string();

    let headers = format!(
        "Content-Type: application/json\r\nAuthorization: Bearer {}\r\n",
        cfg.api_key
    );
    let timeout = if cfg.timeout_ms > 0 {
        cfg.timeout_ms
    } else {
        60_000
    };
    let resp = http::request(
        "POST",
        &cfg.endpoint(),
        &headers,
        payload.as_bytes(),
        timeout,
    )?;
    if !resp.is_success() {
        return Err(LlmError::Api(format!(
            "HTTP {}: {}",
            resp.status,
            resp.body.chars().take(300).collect::<String>()
        )));
    }
    let v: serde_json::Value =
        serde_json::from_str(&resp.body).map_err(|e| LlmError::Parse(e.to_string()))?;
    v.get("choices")
        .and_then(|c| c.as_array())
        .and_then(|a| a.first())
        .and_then(|c| c.get("message"))
        .and_then(|m| m.get("content"))
        .and_then(|c| c.as_str())
        .map(|s| s.to_string())
        .ok_or_else(|| LlmError::Parse("响应里没有 choices[0].message.content".into()))
}

/// 从一段转写文本提取要点；超长自动分段。
pub fn extract_summary(cfg: &LlmConfig, transcript: &str) -> Result<LessonSummary, LlmError> {
    let max = if cfg.max_chars == 0 {
        6000
    } else {
        cfg.max_chars
    };
    let chunks = chunk_text(transcript, max);

    if chunks.len() == 1 {
        let raw = complete(cfg, SYSTEM_PROMPT, &chunks[0])?;
        let v =
            extract_json(&raw).ok_or_else(|| LlmError::Parse(raw.chars().take(200).collect()))?;
        return Ok(parse_summary(&v));
    }

    // 分段提取 -> 合并
    let mut merged = LessonSummary {
        title: String::new(),
        points: Vec::new(),
        notices: Vec::new(),
        keywords: Vec::new(),
    };
    for (i, c) in chunks.iter().enumerate() {
        let prompt = format!("（第 {}/{} 段）\n{}", i + 1, chunks.len(), c);
        match complete(cfg, SYSTEM_PROMPT, &prompt) {
            Ok(raw) => {
                if let Some(v) = extract_json(&raw) {
                    let s = parse_summary(&v);
                    if merged.title.is_empty() && !s.title.is_empty() {
                        merged.title = s.title;
                    }
                    for p in s.points {
                        if !merged.points.contains(&p) && merged.points.len() < 12 {
                            merged.points.push(p);
                        }
                    }
                    for n in s.notices {
                        if !merged.notices.contains(&n) {
                            merged.notices.push(n);
                        }
                    }
                    for k in s.keywords {
                        if !merged.keywords.contains(&k) {
                            merged.keywords.push(k);
                        }
                    }
                }
            }
            // 某一段失败不致命，继续下一段
            Err(e) => tracing::warn!("第 {} 段提取失败: {e}", i + 1),
        }
    }
    if merged.points.is_empty() && merged.notices.is_empty() {
        return Err(LlmError::Parse("所有分段均未产出要点".into()));
    }
    Ok(merged)
}

/// 生成纯文本摘要（推送正文用）。
pub fn render_summary_text(course: &str, date: &str, s: &LessonSummary) -> String {
    let mut out = String::new();
    if !s.title.is_empty() {
        out.push_str(&format!("【{}】{}\n", course, s.title));
    } else {
        out.push_str(&format!("【{course}】{date}\n"));
    }
    if !s.points.is_empty() {
        out.push_str("\n课堂重点：\n");
        for (i, p) in s.points.iter().enumerate() {
            out.push_str(&format!("{}. {}\n", i + 1, p));
        }
    }
    if !s.notices.is_empty() {
        out.push_str("\n重点通知：\n");
        for n in s.notices.iter() {
            out.push_str(&format!("- {n}\n"));
        }
    }
    if !s.keywords.is_empty() {
        out.push_str(&format!("\n关键词：{}\n", s.keywords.join("、")));
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn placeholder_is_recognised() {
        // 配置模板里的占位符必须被识破，否则界面会显示成「已配置」
        assert!(is_placeholder("<模型名>"));
        assert!(is_placeholder("  <你的模型服务地址> "));
        assert!(is_placeholder("你的模型服务地址"));
        // 真实值不能被误判
        assert!(!is_placeholder("deepseek-chat"));
        assert!(!is_placeholder("https://api.deepseek.com/v1"));
        assert!(!is_placeholder(""));
    }

    #[test]
    fn config_with_placeholders_is_not_configured() {
        let cfg = LlmConfig {
            provider: "custom".into(),
            base_url: "https://<你的模型服务地址>/v1".into(),
            api_key: "sk-real".into(),
            model: "<模型名>".into(),
            ..Default::default()
        };
        // 模板原样留着时不能算已配置 —— 否则课后处理会在调用那一刻才炸
        assert!(!cfg.is_configured());

        let ok = LlmConfig {
            base_url: "https://api.deepseek.com/v1".into(),
            api_key: "sk-real".into(),
            model: "deepseek-chat".into(),
            ..Default::default()
        };
        assert!(ok.is_configured());
    }

    #[test]
    fn models_endpoint_strips_chat_suffix() {
        // 用户可能把 base_url 一直填到 /chat/completions，取模型列表要剥回去
        let cfg = LlmConfig {
            base_url: "https://api.deepseek.com/v1/chat/completions".into(),
            ..Default::default()
        };
        assert_eq!(cfg.models_endpoint(), "https://api.deepseek.com/v1/models");
        let cfg2 = LlmConfig {
            base_url: "https://api.deepseek.com/v1/".into(),
            ..Default::default()
        };
        assert_eq!(cfg2.models_endpoint(), "https://api.deepseek.com/v1/models");
    }

    #[test]
    fn config_requires_all_fields() {
        let mut c = LlmConfig::default();
        assert!(!c.is_configured());
        c.base_url = "https://x/v1".into();
        assert!(!c.is_configured());
        c.api_key = "k".into();
        assert!(!c.is_configured());
        c.model = "m".into();
        assert!(c.is_configured());
    }

    #[test]
    fn endpoint_normalises_slashes() {
        let c = LlmConfig {
            base_url: "https://api.deepseek.com/v1/".into(),
            ..Default::default()
        };
        assert_eq!(c.endpoint(), "https://api.deepseek.com/v1/chat/completions");
    }

    #[test]
    fn chunking_respects_limit() {
        let text = (1..=100)
            .map(|i| format!("第{i}行内容"))
            .collect::<Vec<_>>()
            .join("\n");
        let chunks = chunk_text(&text, 50);
        assert!(chunks.len() > 1);
        for c in &chunks {
            assert!(c.chars().count() <= 60, "分段过大: {}", c.chars().count());
        }
        // 内容不丢
        let joined: String = chunks.join("\n");
        assert!(joined.contains("第1行内容"));
        assert!(joined.contains("第100行内容"));
    }

    #[test]
    fn chunking_returns_single_when_short() {
        assert_eq!(chunk_text("短文本", 100), vec!["短文本".to_string()]);
    }

    #[test]
    fn json_extraction_handles_code_fence() {
        let s = "```json\n{\"title\":\"T\",\"points\":[\"a\"]}\n```";
        let v = extract_json(s).unwrap();
        assert_eq!(v.get("title").unwrap().as_str().unwrap(), "T");
    }

    #[test]
    fn json_extraction_handles_surrounding_text() {
        let s = "好的，结果如下：{\"points\":[\"x\"]} 希望有帮助";
        let v = extract_json(s).unwrap();
        assert_eq!(v.get("points").unwrap().as_array().unwrap().len(), 1);
    }

    #[test]
    fn parse_summary_defaults() {
        let v = serde_json::json!({ "points": ["a", " ", "b"] });
        let s = parse_summary(&v);
        assert_eq!(s.title, "");
        assert_eq!(s.points, vec!["a", "b"]);
        assert!(s.notices.is_empty());
    }

    #[test]
    fn render_includes_sections() {
        let s = LessonSummary {
            title: "二次函数".into(),
            points: vec!["顶点公式".into()],
            notices: vec!["作业 P12".into()],
            keywords: vec!["函数".into()],
        };
        let t = render_summary_text("数学", "2025-03-18", &s);
        assert!(t.contains("二次函数"));
        assert!(t.contains("课堂重点"));
        assert!(t.contains("作业 P12"));
        assert!(t.contains("关键词"));
    }
}
