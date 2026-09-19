//! 采集能力封装：屏幕录制会话。
//!
//! 支持两种引擎，**自动择优**：
//!
//! | 引擎 | 依赖 | 说明 |
//! |---|---|---|
//! | [`CaptureEngine::Ffmpeg`] | 系统里要有 `ffmpeg.exe` | 传统路线，兼容性最好 |
//! | [`CaptureEngine::Python`] | 只需要 Python + PyAV | **PyAV 自带 FFmpeg 库**，部署体积更小 |
//!
//! 默认用 [`CaptureEngine::Auto`]：先探测 `ffmpeg.exe`，找到就用它，
//! 找不到就自动切到 Python+PyAV  这样在没装 ffmpeg 的一体机上也能录。
//!
//! 静默性：两者都以 `CREATE_NO_WINDOW` 启动，不弹任何窗口。
//! 收尾：ffmpeg 发 `q`；Python 进程从 stdin 读到 `stop` 后优雅封装文件。
//! `Drop` 保证进程退出时子进程不残留。

use std::path::{Path, PathBuf};
use std::process::{Child, Command, Stdio};

/// 采集引擎类型。
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum CaptureEngine {
    /// 自动：优先 ffmpeg，找不到则用 Python+PyAV。
    Auto,
    /// 外部 ffmpeg 子进程。
    Ffmpeg,
    /// Python + PyAV（无需外部 ffmpeg）。
    Python,
}

/// 采集参数（极简模式为默认值）。
#[derive(Debug, Clone, PartialEq)]
pub struct CaptureParams {
    /// 帧率（极简默认 8）。
    pub fps: u32,
    /// 输出高度（极简默认 720）。
    pub height: u32,
    /// CRF（极简默认 30）。
    pub crf: u32,
    /// 显示器索引（仅 ffmpeg 引擎使用）。
    pub monitor: u32,
    /// 音频来源：`system` / `mic` / `both` / `none`。
    pub audio_source: String,
    /// 截图间隔（秒）。0 表示不截图。
    pub screenshot_interval_secs: u64,
}

impl Default for CaptureParams {
    fn default() -> Self {
        Self {
            fps: 8,
            height: 720,
            crf: 30,
            monitor: 0,
            audio_source: "both".to_string(),
            screenshot_interval_secs: 180,
        }
    }
}

impl CaptureParams {
    /// 依据负载等级自动降档（策划书 7.2）。
    pub fn degrade(&mut self, level: crate::probe::LoadLevel) {
        match level {
            crate::probe::LoadLevel::Low => {}
            crate::probe::LoadLevel::Medium => {
                self.fps = (self.fps / 2).max(5);
                self.crf = (self.crf + 2).min(35);
            }
            crate::probe::LoadLevel::High => {
                self.fps = 5;
                self.crf = 35;
                self.height = self.height.min(480);
                // 高负载时拉长截图间隔，减少磁盘与 CPU 抖动
                self.screenshot_interval_secs = (self.screenshot_interval_secs * 2).max(300);
            }
        }
    }

    /// 是否采集音频。
    pub fn wants_audio(&self) -> bool {
        self.audio_source != "none"
    }
}

/// `ffmpeg.exe` 是否可用。
pub fn ffmpeg_available() -> bool {
    which("ffmpeg")
}

/// 在 PATH 中查找可执行体。
fn which(name: &str) -> bool {
    if let Ok(path) = std::env::var("PATH") {
        for dir in std::env::split_paths(&path) {
            for ext in ["", ".exe", ".cmd", ".bat"] {
                let cand = dir.join(format!("{name}{ext}"));
                if cand.is_file() {
                    return true;
                }
            }
        }
    }
    false
}

/// 按引擎偏好挑选实际使用的引擎。
pub fn resolve_engine(engine: CaptureEngine) -> CaptureEngine {
    match engine {
        CaptureEngine::Auto => {
            if ffmpeg_available() {
                CaptureEngine::Ffmpeg
            } else {
                CaptureEngine::Python
            }
        }
        other => other,
    }
}

/// 生成 ffmpeg 参数（不含可执行体本身）。
pub fn build_args(
    params: &CaptureParams,
    output: &Path,
    audio: Option<&Path>,
    duration_secs: Option<u32>,
) -> Vec<String> {
    let mut a: Vec<String> = vec![
        "-hide_banner".into(),
        "-loglevel".into(),
        "error".into(),
        "-y".into(),
        "-f".into(),
        "gdigrab".into(),
        "-framerate".into(),
        params.fps.to_string(),
        "-i".into(),
        "desktop".into(),
    ];

    if let Some(audio_path) = audio {
        if params.wants_audio() {
            let dev = if params.audio_source == "mic" {
                "Microphone"
            } else {
                "virtual-audio-capturer"
            };
            a.extend([
                "-f".into(),
                "dshow".into(),
                "-i".into(),
                format!("audio={dev}"),
            ]);
            a.extend([
                "-c:a".into(),
                "aac".into(),
                "-b:a".into(),
                "64k".into(),
                "-ac".into(),
                "1".into(),
            ]);
            a.push(audio_path.to_string_lossy().to_string());
        }
    }

    if let Some(secs) = duration_secs {
        a.extend(["-t".into(), secs.to_string()]);
    }

    a.extend([
        "-vf".into(),
        format!("scale=-2:{}", params.height),
        "-c:v".into(),
        "libx264".into(),
        "-preset".into(),
        "veryfast".into(),
        "-crf".into(),
        params.crf.to_string(),
        "-pix_fmt".into(),
        "yuv420p".into(),
        output.to_string_lossy().to_string(),
    ]);
    a
}

/// 生成 Python 录制器参数。
pub fn build_python_args(
    params: &CaptureParams,
    output: &Path,
    shot_dir: Option<&Path>,
) -> Vec<String> {
    let mut a: Vec<String> = vec![
        "-m".into(),
        "vca_worker".into(),
        "--record".into(),
        "--out".into(),
        output.to_string_lossy().to_string(),
        "--fps".into(),
        params.fps.to_string(),
        "--height".into(),
        params.height.to_string(),
        "--crf".into(),
        params.crf.to_string(),
    ];
    if !params.wants_audio() {
        a.push("--no-audio".into());
    }
    if let Some(d) = shot_dir {
        a.extend(["--shot-dir".into(), d.to_string_lossy().to_string()]);
        a.extend([
            "--shot-interval".into(),
            params.screenshot_interval_secs.to_string(),
        ]);
    }
    a
}

/// Python 录制器写出的结果文件。
#[derive(Debug, Clone, Default, serde::Deserialize, serde::Serialize)]
pub struct RecordReport {
    /// 视频路径。
    #[serde(default)]
    pub video: String,
    /// 截图列表。
    #[serde(default)]
    pub screenshots: Vec<String>,
    /// 时长（秒）。
    #[serde(default)]
    pub seconds: u64,
    /// 帧数。
    #[serde(default)]
    pub frames: u64,
    /// 是否录到音频。
    #[serde(default)]
    pub audio_used: bool,
    /// 备注（如「未找到音频设备」）。
    #[serde(default)]
    pub notes: Vec<String>,
}

/// 录制会话。
#[derive(Debug)]
pub struct CaptureSession {
    /// 请求的引擎。
    pub engine: CaptureEngine,
    /// 实际使用的引擎。
    pub actual: CaptureEngine,
    /// 参数。
    pub params: CaptureParams,
    /// 视频输出路径。
    pub output: PathBuf,
    /// 音频输出路径（仅 ffmpeg 引擎）。
    pub audio: Option<PathBuf>,
    /// 截图目录（仅 Python 引擎）。
    pub shot_dir: Option<PathBuf>,
    /// `python/` 目录（Python 引擎需要）。
    pub python_dir: Option<PathBuf>,
    child: Option<Child>,
}

impl CaptureSession {
    /// 创建会话（不启动）。
    pub fn new(engine: CaptureEngine, params: CaptureParams, output: impl Into<PathBuf>) -> Self {
        let output = output.into();
        let audio = output.with_extension("wav");
        let actual = resolve_engine(engine);
        Self {
            engine,
            actual,
            params,
            output,
            audio: Some(audio),
            shot_dir: None,
            python_dir: None,
            child: None,
        }
    }

    /// 指定截图目录（Python 引擎会按间隔抓图）。
    pub fn with_shot_dir(mut self, dir: impl Into<PathBuf>) -> Self {
        self.shot_dir = Some(dir.into());
        self
    }

    /// 指定 `python/` 目录。
    pub fn with_python_dir(mut self, dir: impl Into<PathBuf>) -> Self {
        self.python_dir = Some(dir.into());
        self
    }

    /// 实际使用的引擎。
    pub fn engine_label(&self) -> &'static str {
        match self.actual {
            CaptureEngine::Ffmpeg => "ffmpeg",
            CaptureEngine::Python => "python+PyAV",
            CaptureEngine::Auto => "auto",
        }
    }

    /// 是否正在录制。
    pub fn is_running(&mut self) -> bool {
        match self.child.as_mut() {
            Some(c) => matches!(c.try_wait(), Ok(None)),
            None => false,
        }
    }

    /// 静默启动录制。
    pub fn start(&mut self) -> std::io::Result<()> {
        if self.is_running() {
            return Ok(());
        }
        if let Some(p) = self.output.parent() {
            std::fs::create_dir_all(p)?;
        }

        let mut cmd = match self.actual {
            CaptureEngine::Python => {
                let dir = self
                    .python_dir
                    .clone()
                    .unwrap_or_else(|| PathBuf::from("python"));
                if !dir.is_dir() {
                    return Err(std::io::Error::other(format!(
                        "找不到 python 目录：{}（用 VCA_PYTHON_DIR 指定）",
                        dir.display()
                    )));
                }
                // 优先用随包携带的嵌入式运行时，保证换机器也能跑
                let python = std::env::var("VCA_PYTHON").unwrap_or_else(|_| {
                    let bundled = dir.join("runtime").join("python.exe");
                    if bundled.is_file() {
                        bundled.to_string_lossy().to_string()
                    } else {
                        "python".to_string()
                    }
                });
                let mut c = Command::new(python);
                c.args(build_python_args(
                    &self.params,
                    &self.output,
                    self.shot_dir.as_deref(),
                ))
                .current_dir(&dir);
                c
            }
            _ => {
                let args = build_args(
                    &self.params,
                    &self.output,
                    if self.params.wants_audio() {
                        self.audio.as_deref()
                    } else {
                        None
                    },
                    None,
                );
                let mut c = Command::new("ffmpeg");
                c.args(args);
                c
            }
        };

        cmd.stdin(Stdio::piped())
            .stdout(Stdio::null())
            .stderr(Stdio::null());
        #[cfg(windows)]
        {
            use std::os::windows::process::CommandExt;
            const CREATE_NO_WINDOW: u32 = 0x0800_0000;
            cmd.creation_flags(CREATE_NO_WINDOW);
        }

        let child = cmd.spawn().map_err(|e| {
            std::io::Error::other(format!("启动 {} 录制失败: {e}", self.engine_label()))
        })?;
        self.child = Some(child);
        Ok(())
    }

    /// 停止录制：先礼貌收尾，超时再强杀。
    pub fn stop(&mut self) -> std::io::Result<()> {
        let Some(mut child) = self.child.take() else {
            return Ok(());
        };

        // 两种引擎的停止指令不同：ffmpeg 认 'q'，Python 录制器认 'stop'
        if let Some(mut stdin) = child.stdin.take() {
            use std::io::Write;
            let cmd: &[u8] = match self.actual {
                CaptureEngine::Python => b"stop\n",
                _ => b"q",
            };
            let _ = stdin.write_all(cmd);
            let _ = stdin.flush();
        }

        // Python 需要时间 flush 编码器与封装 moov，等久一点
        let wait_rounds = match self.actual {
            CaptureEngine::Python => 150, // 15 秒
            _ => 50,                      // 5 秒
        };
        for _ in 0..wait_rounds {
            match child.try_wait() {
                Ok(Some(_)) => return Ok(()),
                Ok(None) => std::thread::sleep(std::time::Duration::from_millis(100)),
                Err(e) => return Err(e),
            }
        }
        let _ = child.kill();
        let _ = child.wait();
        Ok(())
    }

    /// 读取 Python 录制器写出的结果（仅 Python 引擎有）。
    pub fn read_report(&self) -> Option<RecordReport> {
        let p = PathBuf::from(format!("{}.record.json", self.output.to_string_lossy()));
        let text = std::fs::read_to_string(p).ok()?;
        serde_json::from_str(&text).ok()
    }

    /// 按固定时长录制（用于自检与定时录制）。
    pub fn record_for(&mut self, secs: u32) -> std::io::Result<()> {
        self.start()?;
        std::thread::sleep(std::time::Duration::from_secs(secs as u64));
        self.stop()
    }
}

impl Drop for CaptureSession {
    fn drop(&mut self) {
        let _ = self.stop();
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn default_params_match_minimal_profile() {
        let p = CaptureParams::default();
        assert_eq!(p.fps, 8);
        assert_eq!(p.height, 720);
        assert_eq!(p.crf, 30);
        assert!(p.wants_audio());
    }

    #[test]
    fn degrade_lowers_quality_under_load() {
        let mut p = CaptureParams::default();
        p.degrade(crate::probe::LoadLevel::Medium);
        assert!(p.fps <= 8 && p.crf >= 30);

        let mut q = CaptureParams::default();
        let before = q.screenshot_interval_secs;
        q.degrade(crate::probe::LoadLevel::High);
        assert_eq!(q.fps, 5);
        assert_eq!(q.crf, 35);
        assert_eq!(q.height, 480);
        assert!(q.screenshot_interval_secs >= before, "高负载应拉长截图间隔");
    }

    #[test]
    fn audio_can_be_disabled() {
        let p = CaptureParams::default();
        assert!(p.wants_audio());
        let q = CaptureParams {
            audio_source: "none".into(),
            ..p.clone()
        };
        assert!(!q.wants_audio());
    }

    #[test]
    fn ffmpeg_args_are_silent_and_scaled() {
        let p = CaptureParams::default();
        let a = build_args(&p, Path::new("o.mp4"), None, None);
        assert!(a.contains(&"-hide_banner".to_string()));
        assert!(a.contains(&"gdigrab".to_string()));
        assert!(a.iter().any(|s| s.starts_with("scale=-2:")));
        assert_eq!(a.last().unwrap(), "o.mp4");
    }

    #[test]
    fn ffmpeg_duration_flag() {
        let p = CaptureParams::default();
        let a = build_args(&p, Path::new("o.mp4"), None, Some(30));
        let i = a.iter().position(|s| s == "-t").unwrap();
        assert_eq!(a[i + 1], "30");
    }

    #[test]
    fn python_args_include_record_mode() {
        let p = CaptureParams::default();
        let a = build_python_args(&p, Path::new("o.mp4"), Some(Path::new("shots")));
        assert!(a.contains(&"--record".to_string()));
        assert!(a.contains(&"--shot-dir".to_string()));
        assert!(a.contains(&"--shot-interval".to_string()));
        assert!(!a.contains(&"--no-audio".to_string()));
    }

    #[test]
    fn python_args_respect_no_audio() {
        let p = CaptureParams {
            audio_source: "none".into(),
            ..Default::default()
        };
        let a = build_python_args(&p, Path::new("o.mp4"), None);
        assert!(a.contains(&"--no-audio".to_string()));
    }

    #[test]
    fn engine_auto_resolves_to_something() {
        let r = resolve_engine(CaptureEngine::Auto);
        assert!(matches!(r, CaptureEngine::Ffmpeg | CaptureEngine::Python));
        assert_eq!(resolve_engine(CaptureEngine::Python), CaptureEngine::Python);
    }

    #[test]
    fn session_reports_engine_label() {
        let s = CaptureSession::new(CaptureEngine::Python, CaptureParams::default(), "a.mp4");
        assert_eq!(s.engine_label(), "python+PyAV");
        assert!(s.audio.is_some());
    }

    #[test]
    fn report_parses_python_output() {
        let json = r#"{"video":"a.mp4","screenshots":["s1.jpg"],"seconds":6,"frames":48,
                        "audio_used":false,"notes":["无音频设备"]}"#;
        let r: RecordReport = serde_json::from_str(json).unwrap();
        assert_eq!(r.seconds, 6);
        assert_eq!(r.screenshots.len(), 1);
        assert!(!r.audio_used);
        assert_eq!(r.notes.len(), 1);
    }

    #[test]
    fn report_tolerates_missing_fields() {
        let r: RecordReport = serde_json::from_str("{}").unwrap();
        assert_eq!(r.seconds, 0);
        assert!(r.screenshots.is_empty());
    }
}
