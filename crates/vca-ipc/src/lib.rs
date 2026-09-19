//! # vca-ipc
//!
//! 插件通信契约层。定义宿主  插件之间的 JSON-RPC 消息格式与载荷类型。
//!
//! 设计依据（策划书 4.2 / 4.4）：
//! - 插件默认以**独立进程**运行，通过 stdin/stdout 交换 **JSON-RPC 2.0** 消息；
//! - 统一方法：`init` / `run` / `cleanup` / `health_check` / `describe`；
//! - 载荷结构按插件类型固定，宿主与插件共享本 crate 即可避免格式漂移。
//!
//! 本 crate 只定义**契约**；实际编解码与进程管理在 `vca-plugin-host` 的
//! `runner` 模块（插件）与 `vca-engine` 的 `worker` 模块（Python Worker）。

#![forbid(unsafe_code)]
#![warn(missing_docs)]

use serde::{Deserialize, Serialize};

/// JSON-RPC 协议版本字符串。
pub const JSONRPC_VERSION: &str = "2.0";

/// 统一方法名常量。
pub mod methods {
    /// 初始化插件，传入配置。
    pub const INIT: &str = "init";
    /// 执行插件主逻辑。
    pub const RUN: &str = "run";
    /// 释放资源。
    pub const CLEANUP: &str = "cleanup";
    /// 健康检查。
    pub const HEALTH_CHECK: &str = "health_check";
    /// 返回插件能力与配置 Schema。
    pub const DESCRIBE: &str = "describe";
}

/// JSON-RPC 请求。
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Request {
    /// 固定为 `"2.0"`。
    pub jsonrpc: String,
    /// 请求 id。
    pub id: u64,
    /// 方法名。
    pub method: String,
    /// 参数（任意 JSON）。
    #[serde(default)]
    pub params: serde_json::Value,
}

impl Request {
    /// 构造一个符合规范的请求。
    pub fn new(id: u64, method: impl Into<String>, params: serde_json::Value) -> Self {
        Self {
            jsonrpc: JSONRPC_VERSION.to_string(),
            id,
            method: method.into(),
            params,
        }
    }
}

/// JSON-RPC 成功响应。
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Response {
    /// 固定为 `"2.0"`。
    pub jsonrpc: String,
    /// 对应请求 id。
    pub id: u64,
    /// 结果。
    pub result: serde_json::Value,
}

/// JSON-RPC 错误响应。
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ErrorResponse {
    /// 固定为 `"2.0"`。
    pub jsonrpc: String,
    /// 对应请求 id。
    pub id: u64,
    /// 错误体。
    pub error: RpcError,
}

/// JSON-RPC 错误体。
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct RpcError {
    /// 错误码。
    pub code: i32,
    /// 错误信息。
    pub message: String,
    /// 附加数据。
    #[serde(default)]
    pub data: Option<serde_json::Value>,
}

/// 插件类型。决定载荷契约。
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum PluginKind {
    /// 录制插件。
    Recorder,
    /// 转写插件。
    Transcriber,
    /// 内容提取插件。
    Extractor,
    /// 截图关联插件。
    Linker,
    /// 文档生成插件。
    Doc,
    /// 推送插件。
    Pusher,
    /// 清理插件。
    Cleaner,
}

/// `describe` 的返回：插件自述能力与配置 Schema。
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct DescribeResult {
    /// 插件 id。
    pub id: String,
    /// 插件类型。
    pub kind: PluginKind,
    /// 插件版本。
    pub version: String,
    /// 宿主接口大版本，必须匹配才加载。
    pub api_version: String,
    /// 配置项 JSON Schema（供 CLI 生成表单与校验）。
    #[serde(default)]
    pub config_schema: serde_json::Value,
    /// 能力列表。
    #[serde(default)]
    pub capabilities: Vec<String>,
}

/// `health_check` 的返回。
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct HealthStatus {
    /// 是否健康。
    pub healthy: bool,
    /// 说明信息。
    #[serde(default)]
    pub message: Option<String>,
}

/// 推送插件返回：**推送成功即 72 小时倒计时起点**。
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct PushResult {
    /// 是否推送成功。
    pub success: bool,
    /// 消息 id（成功时由渠道返回）。
    #[serde(default)]
    pub message_id: Option<String>,
    /// 失败原因。
    #[serde(default)]
    pub error: Option<String>,
}

/// 文档生成插件返回。
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct DocResult {
    /// 生成的 .docx 路径（可选）。
    #[serde(default)]
    pub docx_path: Option<String>,
    /// 生成的 .pdf 路径（可选）。
    #[serde(default)]
    pub pdf_path: Option<String>,
}

/// 录制插件返回。
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct RecordResult {
    /// 录像文件路径。
    pub video_path: String,
    /// 音频文件路径。
    #[serde(default)]
    pub audio_path: Option<String>,
    /// 截图路径列表。
    #[serde(default)]
    pub screenshots: Vec<String>,
    /// 开始时间（RFC3339）。
    pub started_at: String,
    /// 结束时间（RFC3339）。
    pub ended_at: String,
}

/// 转写插件返回。
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct TranscriptResult {
    /// 转写片段。
    pub segments: Vec<TranscriptSegmentDto>,
}

/// 转写片段（线格式）。
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct TranscriptSegmentDto {
    /// 起始秒。
    pub start: f64,
    /// 结束秒。
    pub end: f64,
    /// 文本。
    pub text: String,
}

/// 清理插件返回。
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct CleanResult {
    /// 已删除文件。
    #[serde(default)]
    pub deleted: Vec<String>,
    /// 删除失败文件。
    #[serde(default)]
    pub failed: Vec<String>,
}
