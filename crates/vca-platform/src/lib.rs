//! # vca-platform
//!
//! 平台适配层：把「Windows 特有的能力」收敛到本 crate，
//! 使上层（调度、插件宿主、CLI）保持平台无关，便于测试与未来移植。
//!
//! 模块划分（对应策划书 4.1 平台层）：
//! - [`capture`]  录屏录音采集（ffmpeg 子进程 / 可选 WGC）
//! - [`session`]  用户会话与进程方式（计划任务 vs 服务）
//! - [`probe`]    硬件与系统负载探测（用于自适应降档）
//! - [`doctor`]   环境自检（依赖、权限、编码器、磁盘）
//! - [`http`]     极简 HTTP 客户端（WinHTTP，零第三方依赖）
//! - [`push`]     内置推送通道（企业微信 / Webhook / Server 酱 / dry-run）
//! - [`llm`]      要点提取（OpenAI 兼容接口，仅上传转写文本）
//! - [`overlay`]  桌面悬浮窗（Win32 无边框置顶小窗）
//! - [`crash`]    崩溃静默退出
//! - [`shutdown`] 优雅退出（Ctrl+C 时先停 ffmpeg 再退）
//! - [`tray`]     系统托盘图标（一个小图标 + 右键菜单，无气泡）
//! - [`clock`]    本机时钟
//! - [`screenshot`] 屏幕截图（GDI，写 BMP）
//!
//! 本 crate 是全项目**唯一**允许出现 `unsafe` 的地方（Win32 FFI），
//! 集中在此便于审计；其余 crate 均保持 `#![forbid(unsafe_code)]`。

// 说明：平台层是全项目**唯一**允许出现 unsafe 的地方（Win32 FFI）。
// 其余 crate 均保持 #![forbid(unsafe_code)]；核心层不含任何 unsafe。
#![warn(missing_docs)]

pub mod audio;
pub mod browser_bot;
pub mod capture;
pub mod clock;
pub mod crash;
pub mod doctor;
pub mod docgen;
pub mod http;
pub mod llm;
pub mod overlay;
pub mod probe;
pub mod proc;
pub mod push;
pub mod screenshot;
pub mod session;
pub mod shots;
pub mod shutdown;
pub mod stt;
pub mod tray;
