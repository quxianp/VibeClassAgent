//! 采集会话：屏幕录制（ffmpeg）+ 音频采集（WASAPI）。
//!
//! # 为什么重构成这个样子
//!
//! 旧实现有两条引擎：外部 ffmpeg，或 Python+PyAV。后者是为了「机器上没装 ffmpeg」
//! 时兜底，代价是拖进一整套 Python 运行时（263 MB），与「极低占用」直接冲突。
//! 重构后：
//!
//! | 通道 | 实现 | 说明 |
//! |---|---|---|
//! | 视频 | `ffmpeg.exe` 子进程（gdigrab） | 兼容性最好的屏幕采集路径 |
//! | 音频 | **Rust 直接调 WASAPI** | 见 [`crate::audio`]，不需要虚拟声卡驱动 |
//! | 截图 | ffmpeg 第二路输出 | 与视频共用同一次采集，零额外开销 |
//! | 收尾 | ffmpeg 合并 | 视频 + 混音后的音频 → 最终 mp4 |
//!
//! 旧实现用 ffmpeg 的 `dshow` 录音频，有两个硬伤：
//! 1. 设备名硬编码成 `Microphone` / `virtual-audio-capturer`；
//!    前者在中文 Windows 上叫「麦克风」，根本匹配不上；
//!    后者需要用户先装虚拟声卡驱动，恰好是「开箱即用」的反面。
//! 2. 采不到**系统播放的声音**（loopback），而课堂音频大量来自渲染端。
//!
//! # 中间产物与抗中断
//!
//! 录制期间落盘的是**中间产物**，而不是最终文件：
//!
//! ```text
//! <job>/
//!   video.mkv        视频（matroska 容器：被强杀也通常能播放，mp4 会丢 moov 而全损）
//!   audio/system.wav 系统回环
//!   audio/mic.wav    麦克风
//!   shots/*.jpg      定时截图
//!   record.mp4       收尾产物：音视频合并后的最终文件
//! ```
//!
//! 这个设计让「收尾步骤失败」不再是灾难：原始素材都在，
//! 可以重跑一次收尾（`finalize`），不必重录一节课。

use std::path::{Path, PathBuf};
use std::process::{Child, Command, Stdio};
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::Arc;

use crate::audio::{self, AudioSource, AudioTrack};

/// 视频编码器偏好。
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum VideoEncoder {
    /// 自动：优先硬件编码，失败或不可用时退回 x264。
    Auto,
    /// Intel Quick Sync（集显硬件编码，CPU 占用最低）。
    Qsv,
    /// NVIDIA NVENC。
    Nvenc,
    /// AMD AMF。
    Amf,
    /// 软件编码（libx264），兼容性最好。
    X264,
}

impl VideoEncoder {
    /// 从配置字符串解析。
    pub fn parse(s: &str) -> Self {
        match s.trim().to_ascii_lowercase().as_str() {
            "qsv" | "intel" => Self::Qsv,
            "nvenc" | "nvidia" => Self::Nvenc,
            "amf" | "amd" => Self::Amf,
            "x264" | "libx264" | "sw" | "software" => Self::X264,
            _ => Self::Auto,
        }
    }

    /// ffmpeg 的编码器名。
    pub fn ffmpeg_name(self) -> &'static str {
        match self {
            Self::Qsv => "h264_qsv",
            Self::Nvenc => "h264_nvenc",
            Self::Amf => "h264_amf",
            _ => "libx264",
        }
    }

    /// 是否为硬件编码。
    pub fn is_hardware(self) -> bool {
        matches!(self, Self::Qsv | Self::Nvenc | Self::Amf)
    }
}

/// 采集参数（默认值即「极简模式」，面向低配一体机）。
#[derive(Debug, Clone, PartialEq)]
pub struct CaptureParams {
    /// 帧率。
    pub fps: u32,
    /// 输出高度（宽度按比例，保证为偶数）。
    pub height: u32,
    /// 恒定质量系数，越大越糊也越小。
    pub crf: u32,
    /// 显示器索引（多屏时用；当前 gdigrab 走主屏）。
    pub monitor: u32,
    /// 音频来源：`system` / `mic` / `both` / `none`。
    pub audio_source: String,
    /// 截图间隔（秒）。0 表示不截图。
    pub screenshot_interval_secs: u64,
    /// 视频编码器偏好。
    pub encoder: VideoEncoder,
    /// 音频码率（kbps）。
    pub audio_bitrate_kbps: u32,
    /// 是否把系统声音与麦克风混成单轨（否则保留双轨）。
    pub mix_audio_tracks: bool,
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
            encoder: VideoEncoder::Auto,
            audio_bitrate_kbps: 96,
            mix_audio_tracks: true,
        }
    }
}

impl CaptureParams {
    /// 依据负载等级自动降档。
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
        AudioSource::parse(&self.audio_source) != AudioSource::None
    }

    /// 解析后的音频来源。
    pub fn audio_source(&self) -> AudioSource {
        AudioSource::parse(&self.audio_source)
    }
}

/// 定位 `ffmpeg.exe`。
///
/// 查找顺序：环境变量 `VCA_FFMPEG` → 工具目录（见 [`crate::proc::tool_dirs`]）
/// → PATH。自带优先，避免用户的机器上装了个怪版本 ffmpeg 导致行为不一致。
pub fn ffmpeg_path() -> Option<PathBuf> {
    if let Ok(p) = std::env::var("VCA_FFMPEG") {
        let pb = PathBuf::from(p);
        if pb.is_file() {
            return Some(pb);
        }
    }
    if let Some(p) = crate::proc::find_tool("ffmpeg/ffmpeg.exe") {
        return Some(p);
    }
    crate::proc::which("ffmpeg")
}

/// `ffmpeg.exe` 是否可用。
pub fn ffmpeg_available() -> bool {
    ffmpeg_path().is_some()
}

/// 实测某个编码器在**这一台机器上**能否真正工作。
///
/// # 为什么不能查列表
///
/// `ffmpeg -encoders` 里出现 `h264_qsv` 只说明这个 ffmpeg **编译进了** QSV 支持，
/// 不代表运行时能用。实测（i5-10400 + 远程会话）：
///
/// ```text
/// [h264_qsv] Error creating a MFX session: -9.
/// [h264_qsv] The current mfx implementation is not supported
/// [out#0/matroska] Nothing was written into output file
/// ```
///
/// 结果是录出一个 **0 字节**的 mkv，直到收尾阶段才报 `EBML header parsing failed`，
/// 一节四十分钟的课就这样白录了。
///
/// 所以这里**真编一帧**：用 lavfi 造一帧黑图，编码后丢进 null muxer。
/// 只有退出码为 0 才算可用。
pub fn probe_encoder(ffmpeg: &Path, enc: VideoEncoder) -> bool {
    let mut cmd = Command::new(ffmpeg);
    cmd.args([
        "-hide_banner",
        "-loglevel",
        "error",
        "-f",
        "lavfi",
        "-i",
        "color=c=black:s=320x240:r=1",
        "-frames:v",
        "1",
        "-c:v",
        enc.ffmpeg_name(),
    ]);
    // 硬件编码器的恒定质量参数也一并带上，确保探测条件与实际录制一致。
    match enc {
        VideoEncoder::Qsv => cmd.args(["-global_quality", "30"]),
        VideoEncoder::Nvenc => cmd.args(["-cq", "30"]),
        VideoEncoder::Amf => cmd.args(["-qp_i", "30"]),
        _ => cmd.args(["-preset", "veryfast", "-crf", "30"]),
    };
    cmd.args(["-f", "null", "-"])
        .stdin(Stdio::null())
        .stdout(Stdio::null())
        .stderr(Stdio::null());
    #[cfg(windows)]
    {
        use std::os::windows::process::CommandExt;
        const CREATE_NO_WINDOW: u32 = 0x0800_0000;
        cmd.creation_flags(CREATE_NO_WINDOW);
    }
    matches!(cmd.status(), Ok(s) if s.success())
}

/// 选一个**实测可用**的编码器。
///
/// 返回 `(实际编码器, 备注)`；备注会写进日志，便于事后知道为什么没走硬件。
pub fn resolve_encoder(ffmpeg: &Path, want: VideoEncoder) -> (VideoEncoder, Option<String>) {
    match want {
        VideoEncoder::Auto => {
            // 集显 QSV 是一体机上最划算的选择：CPU 占用最低。
            // 依次实测，碰上第一个能用的就收工；全都不能用则退软件编码。
            for cand in [
                VideoEncoder::Qsv,
                VideoEncoder::Nvenc,
                VideoEncoder::Amf,
            ] {
                if probe_encoder(ffmpeg, cand) {
                    return (cand, None);
                }
            }
            if probe_encoder(ffmpeg, VideoEncoder::X264) {
                (
                    VideoEncoder::X264,
                    Some("硬件编码器都不可用，已退回软件编码 libx264（CPU 占用会更高）".into()),
                )
            } else {
                // 连 x264 都编不出来：仍然交给上层，让录制启动报错而不是假装能用
                (
                    VideoEncoder::X264,
                    Some("警告：libx264 实测也不可用，请检查 ffmpeg 是否完整".into()),
                )
            }
        }
        other => {
            if probe_encoder(ffmpeg, other) {
                (other, None)
            } else {
                (
                    VideoEncoder::X264,
                    Some(format!(
                        "指定的编码器 {} 在本机实测不可用，已退回 libx264",
                        other.ffmpeg_name()
                    )),
                )
            }
        }
    }
}

/// 构造录制的 ffmpeg 参数。
///
/// 一次采集、多路输出：视频进 matroska，截图按间隔进 jpeg 序列。
/// 这样截图不需要再开一路屏幕采集，省一次 GPU/CPU 拷贝。
pub fn build_record_args(
    params: &CaptureParams,
    encoder: VideoEncoder,
    video_out: &Path,
    shot_pattern: Option<&Path>,
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
        // ---- 输出 1：视频 ----
        "-map".into(),
        "0:v".into(),
        "-vf".into(),
        format!("scale=-2:{}", params.height),
        "-c:v".into(),
        encoder.ffmpeg_name().into(),
    ];

    // 硬件编码器不认 crf，用各自的恒定质量参数。
    match encoder {
        VideoEncoder::Qsv => a.extend(["-global_quality".into(), params.crf.to_string()]),
        VideoEncoder::Nvenc => a.extend(["-cq".into(), params.crf.to_string()]),
        VideoEncoder::Amf => a.extend(["-qp_i".into(), params.crf.to_string()]),
        _ => a.extend([
            "-preset".into(),
            "veryfast".into(),
            "-crf".into(),
            params.crf.to_string(),
        ]),
    }
    a.extend(["-pix_fmt".into(), "yuv420p".into()]);
    // matroska：中途被强杀也通常能播放（mp4 会丢 moov 而整段作废）
    a.extend(["-f".into(), "matroska".into()]);
    a.push(video_out.to_string_lossy().to_string());

    // ---- 输出 2：定时截图 ----
    if let Some(pattern) = shot_pattern {
        if params.screenshot_interval_secs > 0 {
            a.extend([
                "-map".into(),
                "0:v".into(),
                "-vf".into(),
                format!(
                    "fps=1/{},scale=-2:{}",
                    params.screenshot_interval_secs,
                    params.height.min(720)
                ),
                "-q:v".into(),
                "4".into(),
                "-f".into(),
                "image2".into(),
                pattern.to_string_lossy().to_string(),
            ]);
        }
    }

    a
}

/// 构造「收尾合并」的 ffmpeg 参数：视频 + 若干音频轨 → 最终 mp4。
///
/// `tracks` 为空时只做容器转换（视频原样拷进 mp4）。
pub fn build_finalize_args(
    params: &CaptureParams,
    video: &Path,
    tracks: &[AudioTrack],
    out: &Path,
) -> Vec<String> {
    let mut a: Vec<String> = vec![
        "-hide_banner".into(),
        "-loglevel".into(),
        "error".into(),
        "-y".into(),
        "-i".into(),
        video.to_string_lossy().to_string(),
    ];
    for t in tracks {
        a.extend(["-i".into(), t.path.to_string_lossy().to_string()]);
    }

    a.extend(["-map".into(), "0:v".into()]);

    let mix = params.mix_audio_tracks && tracks.len() > 1;
    if mix {
        // 系统声音与麦克风混成单轨：播放器默认只放第一轨，
        // 双轨会导致「转写时只听到一半」。
        let inputs: Vec<String> = (1..=tracks.len()).map(|i| format!("[{i}:a]")).collect();
        a.extend([
            "-filter_complex".into(),
            format!(
                "{}amix=inputs={}:duration=longest:dropout_transition=0:normalize=0[aout]",
                inputs.join(""),
                tracks.len()
            ),
            "-map".into(),
            "[aout]".into(),
        ]);
    } else {
        for i in 1..=tracks.len() {
            a.extend(["-map".into(), format!("{i}:a")]);
        }
    }

    // 视频是拷流（不重编码）。
    a.extend(["-c:v".into(), "copy".into()]);
    // 音频：**没有音轨时绝不能给编码参数** —— ffmpeg 会因
    // 「编码参数未用于任何输出流」直接报错，导致纯视频录制也收尾失败。
    if !tracks.is_empty() {
        a.extend([
            "-c:a".into(),
            "aac".into(),
            "-b:a".into(),
            format!("{}k", params.audio_bitrate_kbps),
        ]);
    }
    a.extend([
        // faststart：把索引挪到文件头，微信/QQ 里能边下边播
        "-movflags".into(),
        "+faststart".into(),
        out.to_string_lossy().to_string(),
    ]);
    a
}

/// 读日志文件尾部若干非空行（给失败信息附上现场）。
fn read_log_tail(path: &Path) -> String {
    let Ok(text) = std::fs::read_to_string(path) else {
        return "(无日志)".into();
    };
    let lines: Vec<&str> = text.lines().filter(|l| !l.trim().is_empty()).collect();
    let tail: Vec<&str> = lines.iter().rev().take(3).rev().copied().collect();
    if tail.is_empty() {
        "(日志为空)".into()
    } else {
        tail.join(" | ")
    }
}

/// 一次采集会话的状态与产物。
#[derive(Debug, Clone, Default)]
pub struct CaptureOutcome {
    /// 最终 mp4（收尾成功才有）。
    pub video: Option<PathBuf>,
    /// 中间视频文件。
    pub raw_video: Option<PathBuf>,
    /// 音频轨。
    pub audio: Vec<PathBuf>,
    /// 截图。
    pub screenshots: Vec<PathBuf>,
    /// 录制时长（秒）。
    pub seconds: u64,
    /// 备注（降级、失败原因等，便于事后排查）。
    pub notes: Vec<String>,
}

/// 录制会话。
pub struct CaptureSession {
    params: CaptureParams,
    /// 最终输出（mp4）。
    pub output: PathBuf,
    /// 中间产物目录。
    pub work_dir: PathBuf,
    /// 中间视频路径。
    video_temp: PathBuf,
    /// 截图目录。
    shot_dir: Option<PathBuf>,
    /// 已解析的编码器。
    encoder: VideoEncoder,
    /// ffmpeg 可执行体。
    ffmpeg: PathBuf,
    /// 视频子进程。
    child: Option<Child>,
    /// 音频线程停止信号。
    audio_stop: Arc<AtomicBool>,
    /// 音频采集线程。
    audio_handle: Option<std::thread::JoinHandle<anyhow::Result<Vec<AudioTrack>>>>,
    /// 备注。
    notes: Vec<String>,
    /// 开始时刻。
    started_at: Option<std::time::Instant>,
}

impl std::fmt::Debug for CaptureSession {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("CaptureSession")
            .field("output", &self.output)
            .field("work_dir", &self.work_dir)
            .field("encoder", &self.encoder)
            .field("running", &self.child.is_some())
            .finish()
    }
}

impl CaptureSession {
    /// 创建会话（不启动）。
    ///
    /// `output` 是最终 mp4 的路径；中间产物放在它同级的 `_work/` 目录下。
    pub fn new(params: CaptureParams, output: impl Into<PathBuf>) -> std::io::Result<Self> {
        let output = output.into();
        let work_dir = output
            .parent()
            .unwrap_or_else(|| Path::new("."))
            .join(format!(
                "_work_{}",
                output
                    .file_stem()
                    .map(|s| s.to_string_lossy().to_string())
                    .unwrap_or_else(|| "record".into())
            ));
        let ffmpeg = ffmpeg_path().ok_or_else(|| {
            std::io::Error::other(
                "找不到 ffmpeg.exe。请把它放到程序目录的 tools/ffmpeg/ 下，\
                 或用环境变量 VCA_FFMPEG 指定完整路径。",
            )
        })?;
        let (encoder, encoder_note) = resolve_encoder(&ffmpeg, params.encoder);
        let video_temp = work_dir.join("video.mkv");
        let mut notes = Vec::new();
        if let Some(n) = encoder_note {
            notes.push(n);
        }
        Ok(Self {
            params,
            output,
            work_dir,
            video_temp,
            shot_dir: None,
            encoder,
            ffmpeg,
            child: None,
            audio_stop: Arc::new(AtomicBool::new(false)),
            audio_handle: None,
            notes,
            started_at: None,
        })
    }

    /// 指定截图目录。
    pub fn with_shot_dir(mut self, dir: impl Into<PathBuf>) -> Self {
        self.shot_dir = Some(dir.into());
        self
    }

    /// 覆盖 ffmpeg 路径。
    pub fn with_ffmpeg(mut self, path: impl Into<PathBuf>) -> Self {
        self.ffmpeg = path.into();
        let (enc, note) = resolve_encoder(&self.ffmpeg, self.params.encoder);
        self.encoder = enc;
        if let Some(n) = note {
            self.notes.push(n);
        }
        self
    }

    /// 当前使用的编码器。
    pub fn encoder(&self) -> VideoEncoder {
        self.encoder
    }

    /// 引擎标签（供 CLI / 日志展示）。
    pub fn engine_label(&self) -> String {
        format!("ffmpeg + {} + WASAPI", self.encoder.ffmpeg_name())
    }

    /// 采集到的备注。
    pub fn notes(&self) -> &[String] {
        &self.notes
    }

    /// 是否正在录制。
    pub fn is_running(&mut self) -> bool {
        match self.child.as_mut() {
            Some(c) => matches!(c.try_wait(), Ok(None)),
            None => false,
        }
    }

    /// 静默启动录制（无窗口、无提示）。
    pub fn start(&mut self) -> std::io::Result<()> {
        if self.is_running() {
            return Ok(());
        }
        std::fs::create_dir_all(&self.work_dir)?;
        if let Some(d) = &self.shot_dir {
            std::fs::create_dir_all(d)?;
        }

        // ---- 视频 ----
        let shot_pattern = self
            .shot_dir
            .as_ref()
            .filter(|_| self.params.screenshot_interval_secs > 0)
            .map(|d| d.join("shot%05d.jpg"));

        let args = build_record_args(
            &self.params,
            self.encoder,
            &self.video_temp,
            shot_pattern.as_deref(),
        );

        let mut cmd = Command::new(&self.ffmpeg);
        cmd.args(&args);
        // ffmpeg 必须能读 stdin：停止时给它发 'q' 让它优雅收尾并写容器索引。
        cmd.stdin(Stdio::piped()).stdout(Stdio::null());
        // stderr 落盘而不是丢弃：录制失败时这是唯一的诊断线索。
        // （曾经把它设成 null，结果一个 0 字节的 mkv 查了很久才定位到 QSV 初始化失败。）
        match std::fs::File::create(self.work_dir.join("ffmpeg.log")) {
            Ok(f) => {
                cmd.stderr(Stdio::from(f));
            }
            Err(_) => {
                cmd.stderr(Stdio::null());
            }
        }
        #[cfg(windows)]
        {
            use std::os::windows::process::CommandExt;
            const CREATE_NO_WINDOW: u32 = 0x0800_0000;
            cmd.creation_flags(CREATE_NO_WINDOW);
        }

        let child = cmd
            .spawn()
            .map_err(|e| std::io::Error::other(format!("启动 ffmpeg 录制失败: {e}")))?;
        self.child = Some(child);

        // ---- 音频（WASAPI，独立线程） ----
        let src = self.params.audio_source();
        if src != AudioSource::None {
            self.audio_stop = Arc::new(AtomicBool::new(false));
            let stop = self.audio_stop.clone();
            let dir = self.work_dir.join("audio");
            self.audio_handle = Some(std::thread::spawn(move || {
                audio::record_tracks(&dir, src, stop)
            }));
        }

        self.started_at = Some(std::time::Instant::now());
        Ok(())
    }

    /// 停止录制：先让 ffmpeg 优雅收尾（写容器索引），超时再强杀。
    pub fn stop(&mut self) -> std::io::Result<()> {
        let seconds = self
            .started_at
            .map(|t| t.elapsed().as_secs())
            .unwrap_or(0);

        // ---- 先停音频：它写的是 WAV，需要回填长度头 ----
        if let Some(handle) = self.audio_handle.take() {
            self.audio_stop.store(true, Ordering::Relaxed);
            match handle.join() {
                Ok(Ok(tracks)) => {
                    for t in &tracks {
                        self.notes.push(format!(
                            "音频轨 {}：{:.1}s {} Hz {}ch",
                            t.role, t.seconds, t.sample_rate, t.channels
                        ));
                    }
                }
                Ok(Err(e)) => self.notes.push(format!("音频采集失败：{e}")),
                Err(_) => self.notes.push("音频采集线程 panic".into()),
            }
        }

        // ---- 再停视频 ----
        if let Some(mut child) = self.child.take() {
            if let Some(mut stdin) = child.stdin.take() {
                use std::io::Write;
                let _ = stdin.write_all(b"q");
                let _ = stdin.flush();
            }
            // ffmpeg 收到 q 后要 flush 编码器并写 matroska 索引，给足时间
            let mut exited = false;
            for _ in 0..60 {
                match child.try_wait() {
                    Ok(Some(_)) => {
                        exited = true;
                        break;
                    }
                    Ok(None) => std::thread::sleep(std::time::Duration::from_millis(100)),
                    Err(e) => return Err(e),
                }
            }
            if !exited {
                let _ = child.kill();
                let _ = child.wait();
                self.notes
                    .push("ffmpeg 未在 6 秒内退出，已强制结束（视频可能缺尾部索引）".into());
            }
        }

        if seconds > 0 {
            self.notes.push(format!("录制时长：{seconds}s"));
        }
        Ok(())
    }

    /// 收尾：把视频与音频合并成最终 mp4。
    ///
    /// 可以重复调用（比如首次因磁盘满失败，清理后重试），
    /// 因为中间产物不会被本函数删除。
    pub fn finalize(&self) -> std::io::Result<CaptureOutcome> {
        let mut outcome = CaptureOutcome {
            raw_video: self.video_temp.is_file().then(|| self.video_temp.clone()),
            seconds: self
                .started_at
                .map(|t| t.elapsed().as_secs())
                .unwrap_or(0),
            notes: self.notes.clone(),
            ..Default::default()
        };

        // 收集音频轨（按固定顺序，保证可复现）
        let mut tracks: Vec<AudioTrack> = Vec::new();
        for role in ["system", "mic"] {
            let p = self.work_dir.join("audio").join(format!("{role}.wav"));
            if p.is_file() {
                tracks.push(AudioTrack {
                    path: p,
                    role: if role == "system" { "system" } else { "mic" },
                    sample_rate: 0,
                    channels: 0,
                    seconds: 0.0,
                });
            }
        }
        outcome.audio = tracks.iter().map(|t| t.path.clone()).collect();

        if let Some(d) = &self.shot_dir {
            if let Ok(rd) = std::fs::read_dir(d) {
                let mut shots: Vec<PathBuf> = rd
                    .filter_map(|e| e.ok())
                    .map(|e| e.path())
                    .filter(|p| p.extension().is_some_and(|x| x == "jpg"))
                    .collect();
                shots.sort();
                outcome.screenshots = shots;
            }
        }

        let Some(raw) = outcome.raw_video.clone() else {
            outcome.notes.push("没有录到视频文件".into());
            return Ok(outcome);
        };
        if !raw.is_file() {
            outcome.notes.push("没有录到视频文件".into());
            return Ok(outcome);
        }
        // 0 字节 = 编码器一个包都没产出（硬件编码器初始化失败是典型原因）。
        // 这时再让 ffmpeg 去合并，只会得到一句难懂的 "EBML header parsing failed"。
        if std::fs::metadata(&raw).map(|m| m.len()).unwrap_or(0) == 0 {
            outcome.notes.push(format!(
                "视频文件为 0 字节：编码器 {} 没有产出任何数据。ffmpeg 日志尾部：{}",
                self.encoder.ffmpeg_name(),
                read_log_tail(&self.work_dir.join("ffmpeg.log"))
            ));
            return Ok(outcome);
        }

        if let Some(p) = self.output.parent() {
            std::fs::create_dir_all(p)?;
        }
        let args = build_finalize_args(&self.params, &raw, &tracks, &self.output);

        let mut cmd = Command::new(&self.ffmpeg);
        cmd.args(&args)
            .stdin(Stdio::null())
            .stdout(Stdio::null())
            .stderr(Stdio::piped());
        #[cfg(windows)]
        {
            use std::os::windows::process::CommandExt;
            const CREATE_NO_WINDOW: u32 = 0x0800_0000;
            cmd.creation_flags(CREATE_NO_WINDOW);
        }

        let out = cmd.output()?;
        if out.status.success() && self.output.is_file() {
            outcome.video = Some(self.output.clone());
        } else {
            let err = String::from_utf8_lossy(&out.stderr);
            outcome
                .notes
                .push(format!("收尾合并失败：{}", err.trim().chars().take(300).collect::<String>()));
        }
        Ok(outcome)
    }

    /// 录制固定时长（自检与冒烟测试用）。
    pub fn record_for(&mut self, secs: u32) -> std::io::Result<CaptureOutcome> {
        self.start()?;
        std::thread::sleep(std::time::Duration::from_secs(secs as u64));
        self.stop()?;
        self.finalize()
    }
}

impl Drop for CaptureSession {
    fn drop(&mut self) {
        // 不在这里做 finalize：收尾是显式动作，Drop 只保证不留孤儿进程。
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
        assert_eq!(p.screenshot_interval_secs, 180);
        assert!(p.wants_audio());
        assert_eq!(p.audio_source(), AudioSource::Both);
    }

    #[test]
    fn degrade_lowers_quality_under_load() {
        use crate::probe::LoadLevel;
        let mut p = CaptureParams::default();
        p.degrade(LoadLevel::High);
        assert_eq!(p.fps, 5);
        assert_eq!(p.crf, 35);
        assert_eq!(p.height, 480);
        assert!(p.screenshot_interval_secs >= 300);
    }

    #[test]
    fn audio_none_means_no_audio() {
        let p = CaptureParams {
            audio_source: "none".into(),
            ..Default::default()
        };
        assert!(!p.wants_audio());
    }

    #[test]
    fn encoder_parse_accepts_aliases() {
        assert_eq!(VideoEncoder::parse("qsv"), VideoEncoder::Qsv);
        assert_eq!(VideoEncoder::parse("Intel"), VideoEncoder::Qsv);
        assert_eq!(VideoEncoder::parse("libx264"), VideoEncoder::X264);
        assert_eq!(VideoEncoder::parse("whatever"), VideoEncoder::Auto);
        assert!(VideoEncoder::Qsv.is_hardware());
        assert!(!VideoEncoder::X264.is_hardware());
    }

    #[test]
    fn record_args_target_matroska_and_no_dshow() {
        let p = CaptureParams::default();
        let args = build_record_args(
            &p,
            VideoEncoder::X264,
            Path::new("C:/tmp/video.mkv"),
            None,
        );
        let joined = args.join(" ");
        // 中间容器必须是 matroska：mp4 被强杀会丢 moov 导致整段作废
        assert!(joined.contains("matroska"));
        // 旧实现用 dshow 录音频，已被 WASAPI 取代，参数里不该再出现
        assert!(!joined.contains("dshow"));
        assert!(joined.contains("gdigrab"));
        assert!(joined.contains("scale=-2:720"));
    }

    #[test]
    fn record_args_include_screenshot_output_when_requested() {
        let p = CaptureParams::default();
        let args = build_record_args(
            &p,
            VideoEncoder::X264,
            Path::new("v.mkv"),
            Some(Path::new("shots/shot%05d.jpg")),
        );
        let joined = args.join(" ");
        assert!(joined.contains("fps=1/180"));
        assert!(joined.contains("shot%05d.jpg"));
        // 两路输出必须各自 -map，否则 ffmpeg 会抱怨输出未连接
        assert_eq!(args.iter().filter(|a| *a == "-map").count(), 2);
    }

    #[test]
    fn record_args_skip_screenshot_when_interval_zero() {
        let p = CaptureParams {
            screenshot_interval_secs: 0,
            ..Default::default()
        };
        let args = build_record_args(&p, VideoEncoder::X264, Path::new("v.mkv"), None);
        assert_eq!(args.iter().filter(|a| *a == "-map").count(), 1);
    }

    #[test]
    fn hardware_encoders_use_quality_flags_not_crf() {
        let p = CaptureParams::default();
        let qsv = build_record_args(&p, VideoEncoder::Qsv, Path::new("v.mkv"), None).join(" ");
        assert!(qsv.contains("h264_qsv"));
        assert!(qsv.contains("-global_quality"));
        assert!(!qsv.contains("-crf"));
    }

    #[test]
    fn finalize_mixes_two_tracks_into_one() {
        let p = CaptureParams::default();
        let tracks = vec![
            AudioTrack {
                path: PathBuf::from("system.wav"),
                role: "system",
                sample_rate: 48000,
                channels: 2,
                seconds: 10.0,
            },
            AudioTrack {
                path: PathBuf::from("mic.wav"),
                role: "mic",
                sample_rate: 48000,
                channels: 1,
                seconds: 10.0,
            },
        ];
        let args = build_finalize_args(&p, Path::new("video.mkv"), &tracks, Path::new("out.mp4"));
        let joined = args.join(" ");
        assert!(joined.contains("amix=inputs=2"));
        assert!(joined.contains("-c:v copy"));
        assert!(joined.contains("+faststart"));
    }

    #[test]
    fn finalize_keeps_separate_tracks_when_mix_disabled() {
        let p = CaptureParams {
            mix_audio_tracks: false,
            ..Default::default()
        };
        let tracks = vec![
            AudioTrack {
                path: PathBuf::from("system.wav"),
                role: "system",
                sample_rate: 48000,
                channels: 2,
                seconds: 10.0,
            },
            AudioTrack {
                path: PathBuf::from("mic.wav"),
                role: "mic",
                sample_rate: 48000,
                channels: 1,
                seconds: 10.0,
            },
        ];
        let args = build_finalize_args(&p, Path::new("video.mkv"), &tracks, Path::new("out.mp4"));
        assert!(!args.join(" ").contains("amix"));
        assert_eq!(args.iter().filter(|a| *a == "-map").count(), 3);
    }

    #[test]
    fn finalize_without_audio_still_produces_mp4() {
        let p = CaptureParams::default();
        let args = build_finalize_args(&p, Path::new("video.mkv"), &[], Path::new("out.mp4"));
        let joined = args.join(" ");
        assert!(joined.contains("-c:v copy"));
        assert!(!joined.contains("-c:a"));
    }
}
