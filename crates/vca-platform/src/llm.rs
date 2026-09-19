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

/// LLM 配置。
#[derive(Debug, Clone, Default)]
pub struct LlmConfig {
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
}

impl LlmConfig {
    /// 是否已配置完整。
    pub fn is_configured(&self) -> bool {
        !self.base_url.trim().is_empty()
            && !self.api_key.trim().is_empty()
            && !self.model.trim().is_empty()
    }

    /// 补全 `/chat/completions` 端点。
    pub fn endpoint(&self) -> String {
        format!(
            "{}/chat/completions",
            self.base_url.trim().trim_end_matches('/')
        )
    }
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

/// 单次调用 LLM。
pub fn complete(cfg: &LlmConfig, system: &str, user: &str) -> Result<String, LlmError> {
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
