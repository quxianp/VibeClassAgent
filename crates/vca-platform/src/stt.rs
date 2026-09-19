//! 语音转写：本地 whisper.cpp 优先，云端 API 可选。
//!
//! # 为什么默认本地
//!
//! 云端转写按分钟计费，一天六节课（约 270 分钟）大约是每天 1.6 美元、
//! 每月三百多元。对学校场景这是持续支出，而本地转写的边际成本是 0 ——
//! 老师也就更愿意真的把它一直开着。
//!
//! 但本地转写有前提：得有一台跑得动的机器、以及把模型文件准备好，
//! 所以引擎是可选的（[`SttEngine`]），并且 `Auto` 会在本地不可用时自动退到云端。
//!
//! # 两段式流程
//!
//! ```text
//! record.mp4 ──ffmpeg──> 16kHz 单声道 wav ──whisper-cli──> 文本 + 时间戳
//! ```
//!
//! 中间那次转码不是多余的：whisper.cpp **只接受 16 kHz 单声道 PCM**，
//! 直接喂录制出来的 48 kHz 立体声 mp4 会得到一片乱码或直接失败。
//!
//! # 时间戳为什么重要
//!
//! 需求里要「关联对应的课程桌面截图」。有了每段文字的时间区间，
//! 就能回答「老师讲这个重点的时候，屏幕上是什么」，
//! 而不是把六张截图随机贴进文档。所以 [`Transcript`] 保留 [`Segment`] 列表。

use std::path::{Path, PathBuf};
use std::time::Instant;

use anyhow::{anyhow, Context, Result};

use crate::capture::ffmpeg_path;
use crate::http;
use crate::proc;

/// 转写引擎。
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum SttEngine {
    /// 只用本地 whisper.cpp。
    Local,
    /// 只用云端 API。
    Cloud,
    /// 本地可用就用本地，否则退云端。
    #[default]
    Auto,
}

impl SttEngine {
    /// 从配置字符串解析。
    pub fn parse(s: &str) -> Self {
        match s.trim().to_ascii_lowercase().as_str() {
            "local" | "本地" => Self::Local,
            "cloud" | "api" | "云端" => Self::Cloud,
            _ => Self::Auto,
        }
    }

    /// 配置字符串。
    pub fn as_str(self) -> &'static str {
        match self {
            Self::Local => "local",
            Self::Cloud => "cloud",
            Self::Auto => "auto",
        }
    }
}

/// 本地转写配置。
#[derive(Debug, Clone)]
pub struct LocalSttConfig {
    /// `whisper-cli.exe` 路径（留空则自动查找）。
    pub binary: PathBuf,
    /// 模型文件路径（留空则按 `model` 自动查找）。
    pub model_path: PathBuf,
    /// 模型名：`tiny` / `base` / `small`，仅在 `model_path` 为空时用于查找。
    pub model: String,
    /// 线程数；0 表示自动（取 CPU 核数的一半，上限 4）。
    pub threads: u32,
    /// 语言，`auto` 表示自动检测。
    pub language: String,
    /// 时长上限（秒）：超过就不本地跑，避免把上课用的机器占死。0 表示不限。
    pub max_seconds: u64,
}

impl Default for LocalSttConfig {
    fn default() -> Self {
        Self {
            binary: PathBuf::new(),
            model_path: PathBuf::new(),
            model: "tiny".to_string(),
            threads: 0,
            language: "zh".to_string(),
            max_seconds: 2 * 60 * 60,
        }
    }
}

impl LocalSttConfig {
    /// 解析实际使用的线程数。
    pub fn effective_threads(&self) -> u32 {
        if self.threads > 0 {
            return self.threads.min(16);
        }
        let n = std::thread::available_parallelism()
            .map(|v| v.get() as u32)
            .unwrap_or(2);
        // 关键取舍：本地转写发生在午休/放学后，但仍与老师的日常使用共存，
        // 所以默认只吃一半的核心，最多 4 个。
        (n / 2).clamp(1, 4)
    }
}

/// 云端转写配置（OpenAI 兼容的 `/v1/audio/transcriptions`）。
#[derive(Debug, Clone)]
pub struct CloudSttConfig {
    /// 形如 `https://api.openai.com/v1`。
    pub base_url: String,
    /// 模型名。
    pub model: String,
    /// API Key（由调用方从环境变量读入后填入，绝不写进配置文件）。
    pub api_key: String,
    /// 语言。
    pub language: String,
    /// 超时（毫秒）。
    pub timeout_ms: i32,
}

impl Default for CloudSttConfig {
    fn default() -> Self {
        Self {
            base_url: "https://api.openai.com/v1".to_string(),
            model: "whisper-1".to_string(),
            api_key: String::new(),
            language: "zh".to_string(),
            timeout_ms: 300_000,
        }
    }
}

impl CloudSttConfig {
    /// 是否具备调用条件。
    pub fn is_configured(&self) -> bool {
        !self.base_url.trim().is_empty() && !self.api_key.trim().is_empty()
    }

    /// 补全后的转写端点。
    pub fn endpoint(&self) -> String {
        let base = self.base_url.trim_end_matches('/');
        if base.ends_with("/audio/transcriptions") {
            base.to_string()
        } else {
            format!("{base}/audio/transcriptions")
        }
    }
}

/// 转写总配置。
#[derive(Debug, Clone, Default)]
pub struct SttConfig {
    /// 引擎选择。
    pub engine: SttEngine,
    /// 本地配置。
    pub local: LocalSttConfig,
    /// 云端配置。
    pub cloud: CloudSttConfig,
}

impl SttConfig {
    /// 默认：优先本地。
    pub fn new() -> Self {
        Self {
            engine: SttEngine::Auto,
            local: LocalSttConfig::default(),
            cloud: CloudSttConfig::default(),
        }
    }
}

/// 一段带时间戳的转写文本。
#[derive(Debug, Clone, PartialEq, serde::Serialize, serde::Deserialize)]
pub struct Segment {
    /// 起始毫秒。
    pub start_ms: u64,
    /// 结束毫秒。
    pub end_ms: u64,
    /// 文本。
    pub text: String,
}

/// 转写结果。
#[derive(Debug, Clone)]
pub struct Transcript {
    /// 全文。
    pub text: String,
    /// 分段（云端可能为空）。
    pub segments: Vec<Segment>,
    /// 实际使用的引擎描述。
    pub engine: String,
    /// 转写耗时（秒）。
    pub seconds: f64,
}

impl Transcript {
    /// 是否没转出任何内容。
    pub fn is_empty(&self) -> bool {
        self.text.trim().is_empty()
    }
}

// ---------------------------------------------------------------------------
// 组件定位
// ---------------------------------------------------------------------------

/// 定位 `whisper-cli.exe`。
///
/// 顺序：`VCA_WHISPER` → 工具目录（见 [`crate::proc::tool_dirs`]）→ PATH。
pub fn find_whisper() -> Option<PathBuf> {
    if let Ok(p) = std::env::var("VCA_WHISPER") {
        let pb = PathBuf::from(p);
        if pb.is_file() {
            return Some(pb);
        }
    }
    for rel in ["whisper/whisper-cli.exe", "whisper/main.exe"] {
        if let Some(p) = crate::proc::find_tool(rel) {
            return Some(p);
        }
    }
    crate::proc::which("whisper-cli").or_else(|| crate::proc::which("main"))
}

/// 按模型名定位模型文件。
pub fn find_model(name: &str) -> Option<PathBuf> {
    if let Ok(p) = std::env::var("VCA_WHISPER_MODEL") {
        let pb = PathBuf::from(p);
        if pb.is_file() {
            return Some(pb);
        }
    }
    crate::proc::find_tool(&format!("whisper/models/ggml-{name}.bin"))
}

/// 本地引擎当前是否可用（二进制与模型都在）。
pub fn local_available(cfg: &LocalSttConfig) -> bool {
    let bin = if cfg.binary.as_os_str().is_empty() {
        find_whisper()
    } else {
        Some(cfg.binary.clone())
    };
    let model = if cfg.model_path.as_os_str().is_empty() {
        find_model(&cfg.model)
    } else {
        Some(cfg.model_path.clone())
    };
    matches!((bin, model), (Some(b), Some(m)) if b.is_file() && m.is_file())
}

// ---------------------------------------------------------------------------
// 媒体准备
// ---------------------------------------------------------------------------

/// 把任意音视频转成 whisper 要求的 16 kHz 单声道 PCM WAV。
fn to_16k_mono(input: &Path, out: &Path) -> Result<()> {
    let ffmpeg = ffmpeg_path().ok_or_else(|| anyhow!("找不到 ffmpeg.exe，无法为转写准备音频"))?;
    if let Some(p) = out.parent() {
        std::fs::create_dir_all(p)?;
    }
    let status = proc::silent(&ffmpeg)
        .args(["-hide_banner", "-loglevel", "error", "-y"])
        .arg("-i")
        .arg(input)
        .args(["-vn", "-ac", "1", "-ar", "16000", "-c:a", "pcm_s16le"])
        .arg(out)
        .status()
        .context("启动 ffmpeg 转码失败")?;
    if !status.success() {
        return Err(anyhow!("ffmpeg 转码失败（音频不完整或格式不支持）"));
    }
    if !out.is_file() || std::fs::metadata(out).map(|m| m.len()).unwrap_or(0) == 0 {
        return Err(anyhow!("转码后的音频为空"));
    }
    Ok(())
}

/// 把音频压成适合上传的 mp3。
///
/// 云端接口普遍有 25 MB 体积上限：45 分钟的 16 kHz 单声道 WAV 约 86 MB，
/// 直接传必然被拒；转成 64 kbps mp3 后约 21 MB，能塞进去。
fn to_mp3(input: &Path, out: &Path) -> Result<()> {
    let ffmpeg = ffmpeg_path().ok_or_else(|| anyhow!("找不到 ffmpeg.exe，无法为转写准备音频"))?;
    if let Some(p) = out.parent() {
        std::fs::create_dir_all(p)?;
    }
    let status = proc::silent(&ffmpeg)
        .args(["-hide_banner", "-loglevel", "error", "-y"])
        .arg("-i")
        .arg(input)
        .args(["-vn", "-ac", "1", "-ar", "16000", "-b:a", "64k"])
        .arg(out)
        .status()
        .context("启动 ffmpeg 转 mp3 失败")?;
    if !status.success() || !out.is_file() {
        return Err(anyhow!("转 mp3 失败"));
    }
    Ok(())
}

// ---------------------------------------------------------------------------
// 入口
// ---------------------------------------------------------------------------

/// 转写一段媒体文件。
///
/// `work_dir` 用于放中间音频，调用方负责在合适的时候清理。
pub fn transcribe(cfg: &SttConfig, media: &Path, work_dir: &Path) -> Result<Transcript> {
    if !media.is_file() {
        return Err(anyhow!("待转写的文件不存在: {}", media.display()));
    }
    match cfg.engine {
        SttEngine::Local => transcribe_local(&cfg.local, media, work_dir),
        SttEngine::Cloud => {
            if !cfg.cloud.is_configured() {
                return Err(anyhow!("云端转写未配置完整（需要 base_url 与 api_key）"));
            }
            transcribe_cloud(&cfg.cloud, media, work_dir)
        }
        SttEngine::Auto => {
            if local_available(&cfg.local) {
                match transcribe_local(&cfg.local, media, work_dir) {
                    Ok(t) => Ok(t),
                    Err(e) => {
                        if cfg.cloud.is_configured() {
                            tracing::warn!("本地转写失败，改用云端：{e}");
                            transcribe_cloud(&cfg.cloud, media, work_dir)
                        } else {
                            Err(e)
                        }
                    }
                }
            } else if cfg.cloud.is_configured() {
                transcribe_cloud(&cfg.cloud, media, work_dir)
            } else {
                Err(anyhow!(
                    "本地转写不可用（缺少 whisper-cli.exe 或模型文件），且云端未配置。\n\
                     请运行 scripts/fetch-deps.py 获取本地组件，或填写模型 API。"
                ))
            }
        }
    }
}

/// 本地转写。
pub fn transcribe_local(cfg: &LocalSttConfig, media: &Path, work_dir: &Path) -> Result<Transcript> {
    let started = Instant::now();

    let bin = if cfg.binary.as_os_str().is_empty() {
        find_whisper().ok_or_else(|| anyhow!("找不到 whisper-cli.exe（可用 VCA_WHISPER 指定）"))?
    } else {
        cfg.binary.clone()
    };
    let model = if cfg.model_path.as_os_str().is_empty() {
        find_model(&cfg.model).ok_or_else(|| {
            anyhow!(
                "找不到模型 ggml-{}.bin（可用 VCA_WHISPER_MODEL 指定，\
                 或运行 scripts/fetch-deps.py --models {}）",
                cfg.model,
                cfg.model
            )
        })?
    } else {
        cfg.model_path.clone()
    };

    std::fs::create_dir_all(work_dir)
        .with_context(|| format!("创建转写工作目录失败: {}", work_dir.display()))?;
    let wav = work_dir.join("stt_16k.wav");
    to_16k_mono(media, &wav)?;

    // 时长保护：按 16 kHz / 16 bit / 单声道换算（32 KB/s）。
    if cfg.max_seconds > 0 {
        let bytes = std::fs::metadata(&wav).map(|m| m.len()).unwrap_or(0);
        let secs = bytes / 32_000;
        if secs > cfg.max_seconds {
            return Err(anyhow!(
                "音频时长约 {secs} 秒，超过本地转写上限 {} 秒；\
                 如需处理请调高 transcriber.local.max_seconds，或改用云端",
                cfg.max_seconds
            ));
        }
    }

    let prefix = work_dir.join("stt_out");
    let _ = std::fs::remove_file(prefix.with_extension("json"));
    let _ = std::fs::remove_file(prefix.with_extension("txt"));

    let lang = if cfg.language.eq_ignore_ascii_case("auto") {
        "auto".to_string()
    } else {
        cfg.language.clone()
    };

    let mut cmd = proc::silent(&bin);
    cmd.arg("-m")
        .arg(&model)
        .arg("-f")
        .arg(&wav)
        .arg("-l")
        .arg(&lang)
        .arg("-t")
        .arg(cfg.effective_threads().to_string())
        // 同时要 JSON（带时间戳）与纯文本，JSON 缺失时还能退回文本。
        .args(["-oj", "-otxt"])
        .arg("-of")
        .arg(&prefix);

    let out = cmd.output().context("启动 whisper-cli 失败")?;
    if !out.status.success() {
        let err = String::from_utf8_lossy(&out.stderr);
        return Err(anyhow!(
            "whisper-cli 退出码 {:?}：{}",
            out.status.code(),
            err.trim().chars().take(400).collect::<String>()
        ));
    }

    let (text, segments) = read_whisper_output(&prefix);
    if text.trim().is_empty() {
        return Err(anyhow!("whisper 没有转出任何文本（音频可能全程静音）"));
    }

    Ok(Transcript {
        text,
        segments,
        engine: format!("whisper.cpp({})", cfg.model),
        seconds: started.elapsed().as_secs_f64(),
    })
}

/// 读取 whisper-cli 的产出，优先 JSON（含时间戳），退回纯文本。
fn read_whisper_output(prefix: &Path) -> (String, Vec<Segment>) {
    let json_path = prefix.with_extension("json");
    if let Ok(raw) = std::fs::read_to_string(&json_path) {
        if let Ok(v) = serde_json::from_str::<serde_json::Value>(&raw) {
            let mut segments = Vec::new();
            if let Some(items) = v.get("transcription").and_then(|t| t.as_array()) {
                for it in items {
                    let text = it.get("text").and_then(|t| t.as_str()).unwrap_or("");
                    let text = text.trim();
                    if text.is_empty() {
                        continue;
                    }
                    let off = it.get("offsets");
                    let start_ms = off
                        .and_then(|o| o.get("from"))
                        .and_then(|x| x.as_u64())
                        .unwrap_or(0);
                    let end_ms = off
                        .and_then(|o| o.get("to"))
                        .and_then(|x| x.as_u64())
                        .unwrap_or(start_ms);
                    segments.push(Segment {
                        start_ms,
                        end_ms,
                        text: text.to_string(),
                    });
                }
            }
            if !segments.is_empty() {
                let text = segments
                    .iter()
                    .map(|s| s.text.as_str())
                    .collect::<Vec<_>>()
                    .join("\n");
                return (text, segments);
            }
        }
    }

    let txt_path = prefix.with_extension("txt");
    let text = std::fs::read_to_string(&txt_path).unwrap_or_default();
    (text, Vec::new())
}

/// 云端转写（OpenAI 兼容接口）。
pub fn transcribe_cloud(cfg: &CloudSttConfig, media: &Path, work_dir: &Path) -> Result<Transcript> {
    let started = Instant::now();
    let mp3 = work_dir.join("stt_up.mp3");
    to_mp3(media, &mp3)?;

    let data = std::fs::read(&mp3).context("读取上传音频失败")?;
    if data.len() > 24 * 1024 * 1024 {
        return Err(anyhow!(
            "上传音频 {:.1} MB 超过常见接口的 25 MB 上限；\
             请改用本地转写，或先把一节课切成两段",
            data.len() as f64 / 1048576.0
        ));
    }

    let fields: Vec<(&str, &str)> = vec![
        ("model", cfg.model.as_str()),
        ("language", cfg.language.as_str()),
        ("response_format", "json"),
    ];
    let (ct, body) = http::multipart(&fields, Some(("file", "audio.mp3", data.as_slice())));
    let headers = format!("{ct}Authorization: Bearer {}\r\n", cfg.api_key.trim());
    let resp = http::request("POST", &cfg.endpoint(), &headers, &body, cfg.timeout_ms)
        .map_err(|e| anyhow!("调用云端转写失败: {e}"))?;

    if !resp.is_success() {
        return Err(anyhow!(
            "云端转写返回 HTTP {}：{}",
            resp.status,
            resp.body.trim().chars().take(300).collect::<String>()
        ));
    }

    let v: serde_json::Value =
        serde_json::from_str(&resp.body).context("云端转写的响应不是合法 JSON")?;
    let text = v
        .get("text")
        .and_then(|t| t.as_str())
        .unwrap_or_default()
        .trim()
        .to_string();
    if text.is_empty() {
        return Err(anyhow!(
            "云端转写没有返回文本：{}",
            resp.body.trim().chars().take(200).collect::<String>()
        ));
    }

    Ok(Transcript {
        text,
        segments: Vec::new(),
        engine: format!("cloud({})", cfg.model),
        seconds: started.elapsed().as_secs_f64(),
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn engine_parsing() {
        assert_eq!(SttEngine::parse("local"), SttEngine::Local);
        assert_eq!(SttEngine::parse("本地"), SttEngine::Local);
        assert_eq!(SttEngine::parse("cloud"), SttEngine::Cloud);
        assert_eq!(SttEngine::parse("随便写"), SttEngine::Auto);
    }

    #[test]
    fn threads_default_to_half_the_cores_capped_at_four() {
        let cfg = LocalSttConfig::default();
        let t = cfg.effective_threads();
        // 关键约束：绝不能把老师的机器吃满
        assert!((1..=4).contains(&t), "默认线程数应在 1..=4，实际 {t}");
    }

    #[test]
    fn explicit_threads_are_respected_and_clamped() {
        let cfg = LocalSttConfig {
            threads: 3,
            ..Default::default()
        };
        assert_eq!(cfg.effective_threads(), 3);

        // 显式填了离谱的大数字也要夹住，避免把课堂机器打死
        let cfg = LocalSttConfig {
            threads: 999,
            ..Default::default()
        };
        assert_eq!(cfg.effective_threads(), 16);
    }

    #[test]
    fn cloud_endpoint_is_completed_once() {
        // 用结构体初始化而不是 Default 之后逐个赋值（clippy 会提醒，也更好读）
        let mut cfg = CloudSttConfig {
            base_url: "https://api.openai.com/v1".into(),
            ..Default::default()
        };
        assert_eq!(
            cfg.endpoint(),
            "https://api.openai.com/v1/audio/transcriptions"
        );
        // 用户直接填了完整端点时不要拼成 .../audio/transcriptions/audio/transcriptions
        cfg.base_url = "https://x.example/v1/audio/transcriptions".into();
        assert_eq!(cfg.endpoint(), "https://x.example/v1/audio/transcriptions");
        // 末尾斜杠要吃掉
        cfg.base_url = "https://x.example/v1/".into();
        assert_eq!(cfg.endpoint(), "https://x.example/v1/audio/transcriptions");
    }

    #[test]
    fn cloud_requires_key_and_url() {
        let mut cfg = CloudSttConfig::default();
        assert!(!cfg.is_configured());
        cfg.api_key = "sk-x".into();
        cfg.base_url = "".into();
        assert!(!cfg.is_configured());
        cfg.base_url = "https://x.example/v1".into();
        assert!(cfg.is_configured());
    }

    #[test]
    fn missing_file_is_reported_before_any_work() {
        let cfg = SttConfig::new();
        let err = transcribe(&cfg, Path::new("不存在.mp4"), Path::new("."));
        assert!(err.is_err());
    }

    #[test]
    fn cloud_engine_without_key_fails_fast() {
        let cfg = SttConfig {
            engine: SttEngine::Cloud,
            ..Default::default()
        };
        let err = transcribe(&cfg, Path::new("不存在.mp4"), Path::new("."));
        // 文件不存在先被拦下，这里确认不会走到网络层
        assert!(err.is_err());
    }

    #[test]
    fn json_output_is_parsed_with_timestamps() {
        let dir = std::env::temp_dir().join("vca_stt_test_json");
        let _ = std::fs::create_dir_all(&dir);
        let prefix = dir.join("out");
        let json = r#"{
          "transcription": [
            {"offsets": {"from": 0, "to": 3200}, "text": " 同学们好"},
            {"offsets": {"from": 3200, "to": 7600}, "text": " 今天我们讲二次函数"}
          ]
        }"#;
        std::fs::write(prefix.with_extension("json"), json).unwrap();
        let (text, segs) = read_whisper_output(&prefix);
        assert_eq!(segs.len(), 2);
        assert_eq!(segs[0].start_ms, 0);
        assert_eq!(segs[1].end_ms, 7600);
        // 文本要 trim 掉 whisper 习惯加的前导空格
        assert!(text.contains("同学们好"));
        assert!(!text.starts_with(' '));
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn txt_fallback_when_json_missing() {
        let dir = std::env::temp_dir().join("vca_stt_test_txt");
        let _ = std::fs::create_dir_all(&dir);
        let prefix = dir.join("out");
        std::fs::write(prefix.with_extension("txt"), "只有纯文本").unwrap();
        let (text, segs) = read_whisper_output(&prefix);
        assert_eq!(text, "只有纯文本");
        assert!(segs.is_empty());
        let _ = std::fs::remove_dir_all(&dir);
    }
}
