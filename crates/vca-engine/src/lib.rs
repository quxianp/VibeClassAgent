//! # vca-engine
//!
//! 编排层：把「时间表 + 课程表 + 录制 + 窗口 + 悬浮窗 + 清理」串成一个可长期运行的守护进程。
//!
//! 模块划分：
//! - [`worker`]   Python 算法侧 Worker 的 stdio JSON-RPC 客户端
//! - [`pipeline`] 单个作业的处理流水线（转写  提取  关联  文档  推送  清理）
//! - [`daemon`]   调度主循环（按时触发录制、处理窗口、悬浮窗、清理）

#![forbid(unsafe_code)]
#![warn(missing_docs)]

pub mod daemon;
pub mod pipeline;
pub mod worker;

pub use daemon::{Daemon, DaemonConfig};
pub use pipeline::{Pipeline, PipelineOutcome};
pub use worker::WorkerClient;
