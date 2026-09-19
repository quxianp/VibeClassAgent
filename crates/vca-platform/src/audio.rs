//! WASAPI 音频采集：系统回环（loopback）与麦克风。
//!
//! # 为什么需要本模块
//!
//! 原实现只通过 ffmpeg 的 `dshow` 打开**麦克风**，采不到一体机自己播放的声音。
//! 课堂场景里教师用扩音设备讲课、播放课件音视频，声音都只出现在**渲染端**，
//! 麦克风要么收不到，要么收到严重失真的环境回声。
//!
//! WASAPI 的 loopback 模式可以直接抓取渲染端混音，**无需安装任何虚拟声卡驱动**，
//! 这是 Windows 上采集系统声音的正确做法。
//!
//! # 设计
//!
//! - 每一路音频（系统回环 / 麦克风）**各自写一个 16-bit PCM 的 WAV 文件**，
//!   不在这里做混音。混音交给录制收尾时的 ffmpeg（`amix`），
//!   这样 Rust 侧只需处理「一路端点 → 一个文件」，逻辑最简单、最不容易错。
//! - COM 按线程初始化，采集线程**自己**初始化 COM（`CoInitializeEx` 是本线程状态）。
//! - 采集循环是阻塞式的，通过 `Arc<AtomicBool>` 接收停止信号。
//!
//! # 已知限制
//!
//! 共享模式下 Windows 的混音格式通常是 32-bit float；本模块统一转换成
//! 16-bit PCM 落盘（语音场景足够，且体积只有一半）。

use std::fs::File;
use std::io::{Seek, SeekFrom, Write};
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::Arc;
use std::time::Instant;

use anyhow::{anyhow, Context, Result};
use windows::Win32::Media::Audio::{
    eCapture, eConsole, eRender, IAudioCaptureClient, IAudioClient, IMMDevice, IMMDeviceEnumerator,
    MMDeviceEnumerator, AUDCLNT_BUFFERFLAGS_SILENT, AUDCLNT_SHAREMODE_SHARED,
    AUDCLNT_STREAMFLAGS_LOOPBACK, WAVEFORMATEX, WAVEFORMATEXTENSIBLE,
};
use windows::Win32::System::Com::{
    CoCreateInstance, CoInitializeEx, CoTaskMemFree, CoUninitialize, CLSCTX_ALL,
    COINIT_MULTITHREADED,
};

/// 一次 `Initialize` 请求的缓冲时长（100ns 单位，这里 1 秒）。
/// loopback 模式要求该值非零，否则 `Initialize` 会失败。
const BUFFER_DURATION_100NS: i64 = 10_000_000;

/// `WAVE_FORMAT_IEEE_FLOAT`
const WAVE_FORMAT_IEEE_FLOAT: u16 = 3;
/// `WAVE_FORMAT_PCM`
const WAVE_FORMAT_PCM: u16 = 1;
/// `WAVE_FORMAT_EXTENSIBLE`
const WAVE_FORMAT_EXTENSIBLE: u16 = 0xFFFE;

/// 音频来源。
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum AudioSource {
    /// 系统回环：一体机播放的声音（loopback）。
    System,
    /// 麦克风：教师现场声音。
    Mic,
    /// 两者同时采集。
    Both,
    /// 不采集音频。
    None,
}

impl AudioSource {
    /// 从配置字符串解析；无法识别时退回 [`AudioSource::Both`]。
    pub fn parse(s: &str) -> Self {
        match s.trim().to_ascii_lowercase().as_str() {
            "system" | "loopback" | "系统" | "系统声音" => Self::System,
            "mic" | "microphone" | "麦克风" => Self::Mic,
            "none" | "off" | "关闭" => Self::None,
            _ => Self::Both,
        }
    }

    /// 是否要采系统回环。
    pub fn wants_system(self) -> bool {
        matches!(self, Self::System | Self::Both)
    }

    /// 是否要采麦克风。
    pub fn wants_mic(self) -> bool {
        matches!(self, Self::Mic | Self::Both)
    }

    /// 配置字符串。
    pub fn as_str(self) -> &'static str {
        match self {
            Self::System => "system",
            Self::Mic => "mic",
            Self::Both => "both",
            Self::None => "none",
        }
    }
}

/// COM 初始化守卫：保证 `drop` 时一定 `CoUninitialize`。
struct ComGuard {
    owned: bool,
}

impl ComGuard {
    /// 在当前线程初始化 COM。
    fn new() -> Result<Self> {
        // 说明：windows crate 的 CoInitializeEx 返回裸 HRESULT（不是 Result），
        // 成功是 S_OK(0)，这里按 HRESULT 数值判断。
        let hr = unsafe { CoInitializeEx(None, COINIT_MULTITHREADED) };
        match hr.0 as u32 {
            0 => Ok(Self { owned: true }),
            // RPC_E_CHANGED_MODE：本线程已用其它模式初始化过 COM。
            // 此时 COM 仍可用，只是不该由我们负责反初始化。
            0x8001_0106 => Ok(Self { owned: false }),
            _ => Err(anyhow!("CoInitializeEx 失败: HRESULT {:#010x}", hr.0 as u32)),
        }
    }
}

impl Drop for ComGuard {
    fn drop(&mut self) {
        if self.owned {
            unsafe { CoUninitialize() };
        }
    }
}

/// 一个音频端点的摘要（供 `vca doctor` 展示）。
#[derive(Debug, Clone)]
pub struct EndpointInfo {
    /// `render`（播放端，loopback 用）或 `capture`（录制端，麦克风用）。
    pub role: &'static str,
    /// WASAPI 端点 id。
    pub id: String,
    /// 混音格式描述，如 `48000 Hz / 2ch / 32bit`。
    pub format: String,
}

/// 创建设备枚举器。
fn enumerator() -> Result<IMMDeviceEnumerator> {
    unsafe { CoCreateInstance(&MMDeviceEnumerator, None, CLSCTX_ALL) }
        .context("创建 MMDeviceEnumerator 失败")
}

/// 取默认端点。`render=true` 取播放端（loopback 用），否则取录制端（麦克风用）。
fn default_device(enumr: &IMMDeviceEnumerator, render: bool) -> Result<IMMDevice> {
    let flow = if render { eRender } else { eCapture };
    unsafe { enumr.GetDefaultAudioEndpoint(flow, eConsole) }
        .context("GetDefaultAudioEndpoint 失败：系统里可能没有可用的音频端点")
}

/// 把端点 id（PWSTR）转成 String 并释放 COM 内存。
fn device_id(device: &IMMDevice) -> Result<String> {
    let pw = unsafe { device.GetId() }.context("IMMDevice::GetId 失败")?;
    let s = unsafe { pw.to_string() }.unwrap_or_default();
    unsafe { CoTaskMemFree(Some(pw.0 as *const _)) };
    Ok(s)
}

/// 描述混音格式（读完后立即释放，避免泄漏）。
fn describe_format(wf: *const WAVEFORMATEX) -> String {
    if wf.is_null() {
        return "未知".to_string();
    }
    let w = unsafe { &*wf };
    // WAVEFORMATEX 是 packed(1) 结构，直接取字段引用会触发 E0793（未对齐引用 UB），
    // 因此先按值拷贝到局部变量再格式化。
    let rate = w.nSamplesPerSec;
    let channels = w.nChannels;
    let bits = w.wBitsPerSample;
    format!("{rate} Hz / {channels}ch / {bits}bit")
}

/// 自检：枚举默认渲染端与捕获端，读出各自的混音格式。
///
/// 这是「能不能录到系统声音」的判据，`vca doctor` 会调用它。
pub fn probe_endpoints() -> Result<Vec<EndpointInfo>> {
    let _com = ComGuard::new()?;
    let enumr = enumerator()?;
    let mut out = Vec::new();

    for (render, role) in [(true, "render"), (false, "capture")] {
        let Ok(device) = default_device(&enumr, render) else {
            continue;
        };
        let Ok(id) = device_id(&device) else { continue };

        let format = unsafe {
            match device.Activate::<IAudioClient>(CLSCTX_ALL, None) {
                Ok(client) => match client.GetMixFormat() {
                    Ok(wf) => {
                        let d = describe_format(wf);
                        CoTaskMemFree(Some(wf as *const _));
                        d
                    }
                    Err(e) => format!("取混音格式失败: {e}"),
                },
                Err(e) => format!("激活 IAudioClient 失败: {e}"),
            }
        };
        out.push(EndpointInfo { role, id, format });
    }
    Ok(out)
}

/// 采样类型（本模块只把这两种转成 16-bit PCM）。
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum SampleKind {
    /// 32-bit float（共享模式最常见）。
    F32,
    /// 16-bit 有符号整数。
    I16,
}

/// 判断混音格式的采样类型。
fn sample_kind(wf: &WAVEFORMATEX) -> SampleKind {
    let bits = wf.wBitsPerSample;
    match wf.wFormatTag {
        WAVE_FORMAT_IEEE_FLOAT => SampleKind::F32,
        WAVE_FORMAT_PCM if bits == 16 => SampleKind::I16,
        WAVE_FORMAT_PCM if bits == 32 => SampleKind::F32,
        WAVE_FORMAT_EXTENSIBLE => {
            // SubFormat 的前 4 字节（data1）低 16 位就是格式码。
            let ext = wf as *const WAVEFORMATEX as *const WAVEFORMATEXTENSIBLE;
            let code = unsafe { (*ext).SubFormat.data1 } as u16;
            match code {
                WAVE_FORMAT_IEEE_FLOAT => SampleKind::F32,
                WAVE_FORMAT_PCM if bits == 16 => SampleKind::I16,
                _ if bits == 32 => SampleKind::F32,
                _ => SampleKind::F32,
            }
        }
        // 兜底：32 位一律按 float 处理；其余（如 24 位）也按 float 读，
        // 宁可音质略降也不能让录制直接失败。
        _ if bits == 16 => SampleKind::I16,
        _ => SampleKind::F32,
    }
}

/// 把一帧原始数据转成 16-bit PCM。
///
/// `mixdown` 为真时把多声道下混成单声道（麦克风场景更省空间）。
fn convert_to_i16(raw: &[u8], kind: SampleKind, channels: u16, mixdown: bool) -> Vec<i16> {
    let ch = channels.max(1) as usize;
    match kind {
        SampleKind::F32 => {
            let mut out = Vec::with_capacity(raw.len() / 4);
            for chunk in raw.chunks_exact(4) {
                let f = f32::from_le_bytes([chunk[0], chunk[1], chunk[2], chunk[3]]);
                let v = (f.clamp(-1.0, 1.0) * 32767.0) as i16;
                out.push(v);
            }
            if mixdown {
                downmix(&out, ch)
            } else {
                out
            }
        }
        SampleKind::I16 => {
            let mut out = Vec::with_capacity(raw.len() / 2);
            for chunk in raw.chunks_exact(2) {
                out.push(i16::from_le_bytes([chunk[0], chunk[1]]));
            }
            if mixdown {
                downmix(&out, ch)
            } else {
                out
            }
        }
    }
}

/// 多声道下混成单声道（简单平均）。
fn downmix(samples: &[i16], channels: usize) -> Vec<i16> {
    if channels <= 1 {
        return samples.to_vec();
    }
    samples
        .chunks(channels)
        .map(|frame| {
            let sum: i32 = frame.iter().map(|&s| s as i32).sum();
            (sum / frame.len() as i32) as i16
        })
        .collect()
}

/// 已落盘的一路音频。
#[derive(Debug, Clone)]
pub struct AudioTrack {
    /// WAV 文件路径。
    pub path: PathBuf,
    /// 来源标识：`system` 或 `mic`。
    pub role: &'static str,
    /// 采样率。
    pub sample_rate: u32,
    /// 声道数（下混后）。
    pub channels: u16,
    /// 实际时长（秒）。
    pub seconds: f64,
}

/// 写 44 字节的标准 WAV 头（PCM）。
fn write_wav_header(f: &mut File, sample_rate: u32, channels: u16, data_len: u32) -> std::io::Result<()> {
    let bits = 16u16;
    let block_align = channels * bits / 8;
    let byte_rate = sample_rate * block_align as u32;

    f.write_all(b"RIFF")?;
    f.write_all(&(36 + data_len).to_le_bytes())?;
    f.write_all(b"WAVE")?;
    f.write_all(b"fmt ")?;
    f.write_all(&16u32.to_le_bytes())?;
    f.write_all(&1u16.to_le_bytes())?; // PCM
    f.write_all(&channels.to_le_bytes())?;
    f.write_all(&sample_rate.to_le_bytes())?;
    f.write_all(&byte_rate.to_le_bytes())?;
    f.write_all(&block_align.to_le_bytes())?;
    f.write_all(&bits.to_le_bytes())?;
    f.write_all(b"data")?;
    f.write_all(&data_len.to_le_bytes())?;
    Ok(())
}

/// 采集一路端点到 WAV，直到 `stop` 置位。
///
/// `render=true` 表示采系统回环（播放端 + LOOPBACK 标志），
/// 否则采麦克风（录制端）。
fn capture_one(
    out_path: &Path,
    render: bool,
    mixdown: bool,
    stop: Arc<AtomicBool>,
) -> Result<AudioTrack> {
    let _com = ComGuard::new()?;
    let enumr = enumerator()?;
    let device = default_device(&enumr, render)?;
    let client: IAudioClient = unsafe { device.Activate(CLSCTX_ALL, None) }
        .context("激活 IAudioClient 失败")?;

    let wf_ptr = unsafe { client.GetMixFormat() }.context("GetMixFormat 失败")?;
    if wf_ptr.is_null() {
        return Err(anyhow!("混音格式为空"));
    }
    let wf = unsafe { *wf_ptr };
    let sample_rate = wf.nSamplesPerSec;
    let src_channels = wf.nChannels;
    let kind = sample_kind(&wf);
    let block_align = wf.nBlockAlign as usize;

    let flags = if render {
        AUDCLNT_STREAMFLAGS_LOOPBACK
    } else {
        0
    };

    let init = unsafe {
        client.Initialize(
            AUDCLNT_SHAREMODE_SHARED,
            flags,
            BUFFER_DURATION_100NS,
            0,
            wf_ptr,
            None,
        )
    };
    unsafe { CoTaskMemFree(Some(wf_ptr as *const _)) };
    init.context("IAudioClient::Initialize 失败")?;

    let capture: IAudioCaptureClient = unsafe { client.GetService() }
        .context("取 IAudioCaptureClient 失败（设备可能不支持采集）")?;

    unsafe { client.Start() }.context("IAudioClient::Start 失败")?;

    let out_channels: u16 = if mixdown { 1 } else { src_channels };
    let mut file = File::create(out_path)
        .with_context(|| format!("创建音频文件失败: {}", out_path.display()))?;
    write_wav_header(&mut file, sample_rate, out_channels, 0)?;

    let started = Instant::now();
    let mut data_len: u32 = 0;

    let result = (|| -> Result<()> {
        while !stop.load(Ordering::Relaxed) {
            let packet = unsafe { capture.GetNextPacketSize() }
                .context("GetNextPacketSize 失败")?;
            if packet == 0 {
                // 没有新数据时小睡，避免空转吃满一个核。
                std::thread::sleep(std::time::Duration::from_millis(5));
                continue;
            }

            let mut data: *mut u8 = std::ptr::null_mut();
            let mut frames: u32 = 0;
            let mut flags: u32 = 0;
            unsafe { capture.GetBuffer(&mut data, &mut frames, &mut flags, None, None) }
                .context("GetBuffer 失败")?;

            if frames > 0 {
                // SILENT：缓冲区内容无效，应当写入等长静音而不是读脏数据。
                let samples: Vec<i16> = if flags & AUDCLNT_BUFFERFLAGS_SILENT.0 as u32 != 0 {
                    let n = frames as usize * out_channels as usize;
                    vec![0i16; n]
                } else {
                    let n_bytes = frames as usize * block_align;
                    let raw = unsafe { std::slice::from_raw_parts(data, n_bytes) };
                    convert_to_i16(raw, kind, src_channels, mixdown)
                };

                let mut bytes = Vec::with_capacity(samples.len() * 2);
                for s in samples {
                    bytes.extend_from_slice(&s.to_le_bytes());
                }
                file.write_all(&bytes)?;
                data_len += bytes.len() as u32;
            }

            unsafe { capture.ReleaseBuffer(frames) }.context("ReleaseBuffer 失败")?;
        }
        Ok(())
    })();

    // 无论成功失败都要停流，否则设备会被一直占用。
    unsafe {
        let _ = client.Stop();
    }
    result?;

    // 回填真正的数据长度。
    file.seek(SeekFrom::Start(0))?;
    write_wav_header(&mut file, sample_rate, out_channels, data_len)?;
    file.flush()?;

    let seconds = started.elapsed().as_secs_f64();
    Ok(AudioTrack {
        path: out_path.to_path_buf(),
        role: if render { "system" } else { "mic" },
        sample_rate,
        channels: out_channels,
        seconds,
    })
}

/// 按来源采集音频，返回每一路的落盘信息。
///
/// 多路会并行采集（各自一个线程），本函数阻塞到 `stop` 置位。
/// 单路失败不影响另一路：失败信息以 `Err` 形式跳过，成功的那路照常返回。
pub fn record_tracks(
    out_dir: &Path,
    source: AudioSource,
    stop: Arc<AtomicBool>,
) -> Result<Vec<AudioTrack>> {
    std::fs::create_dir_all(out_dir)
        .with_context(|| format!("创建音频目录失败: {}", out_dir.display()))?;

    let mut jobs: Vec<(PathBuf, bool, bool)> = Vec::new();
    if source.wants_system() {
        // 系统回环保留立体声（课件音频有左右声道信息），不下混。
        jobs.push((out_dir.join("system.wav"), true, false));
    }
    if source.wants_mic() {
        // 麦克风下混成单声道：人声场景足够，体积减半。
        jobs.push((out_dir.join("mic.wav"), false, true));
    }

    if jobs.is_empty() {
        return Ok(Vec::new());
    }

    let handles: Vec<_> = jobs
        .into_iter()
        .map(|(path, render, mixdown)| {
            let stop = stop.clone();
            std::thread::spawn(move || capture_one(&path, render, mixdown, stop))
        })
        .collect();

    let mut tracks = Vec::new();
    let mut errors = Vec::new();
    for h in handles {
        match h.join() {
            Ok(Ok(t)) => tracks.push(t),
            Ok(Err(e)) => errors.push(e.to_string()),
            Err(_) => errors.push("采集线程 panic".to_string()),
        }
    }

    if tracks.is_empty() {
        return Err(anyhow!("所有音频通道都采集失败：{}", errors.join("; ")));
    }
    Ok(tracks)
}
