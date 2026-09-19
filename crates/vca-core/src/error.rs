//! 统一错误类型。
//!
//! 各模块另有自己的细分错误类型（`ImportError` / `LoadError` / `HttpError` /
//! `PushError` / `LlmError` / `RunnerError` / `MarketError`），此处只保留跨模块的通用错误。

use thiserror::Error;

/// vca-core 的统一错误类型。
#[derive(Debug, Error)]
pub enum CoreError {
    /// 配置文件读取或解析失败。
    #[error("配置错误: {0}")]
    Config(String),

    /// 指定的 profile 不存在。
    #[error("profile 不存在: {0}")]
    ProfileNotFound(String),

    /// 路径解析失败。
    #[error("路径错误: {0}")]
    Path(String),

    /// 状态机流转非法。
    #[error("状态流转非法: {from:?} -> {to:?}")]
    InvalidTransition {
        /// 当前状态。
        from: crate::job::JobState,
        /// 目标状态。
        to: crate::job::JobState,
    },

    /// 该能力尚未实现（保留给未来的插件 / 模块使用）。
    #[error("尚未实现: {0}")]
    NotImplemented(String),
}

/// 框架通用 Result 别名。
pub type Result<T> = std::result::Result<T, CoreError>;
