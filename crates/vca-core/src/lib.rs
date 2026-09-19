//! # vca-core
//!
//! 领域核心层：只放「与具体实现无关」的模型与契约。
//!
//! 本 crate **只做纯逻辑**：不做 IO、不含 `unsafe`、不依赖任何上层 crate，
//! 因此可以脱离 Windows 完整单测（当前 35 个测试）。
//!
//! 模块划分（对应策划书 4.1 核心服务层）：
//! - [`config`]  配置模型与加载（多 profile、密钥分离）
//! - [`model`]   领域模型（时间表、课程表、作业、文档等）
//! - [`job`]     作业状态机（录制  转写  提取  文档  推送  清理）
//! - [`paths`]   数据/配置目录解析与命名规则
//! - [`time`]    本地日期时间与纯日期运算
//! - [`schedule`] 排课计划与悬浮窗时序计算
//! - [`import`]   ClassIsland / CSV 导入与存档
//! - [`setup`]    首次运行引导与自动修复
//! - [`error`]   统一错误类型

#![forbid(unsafe_code)]
#![warn(missing_docs)]

pub mod cleanup;
pub mod config;
pub mod error;
pub mod import;
pub mod job;
pub mod model;
pub mod paths;
pub mod schedule;
pub mod setup;
pub mod store;
pub mod time;
pub mod windows;

/// 项目版本号（取自 Cargo.toml）。
pub const VERSION: &str = env!("CARGO_PKG_VERSION");

/// 产品名占位。策划书未最终确定正式名称，故此处使用占位常量，
/// 后续改名只需修改此常量与 Cargo.toml 中的 description。
pub const PRODUCT_NAME: &str = "<项目名>";

/// 数据目录默认名（用于 `%ProgramData%\\<数据目录名>\\`）。
pub const DEFAULT_DATA_DIR_NAME: &str = "VibeClassAgent";
