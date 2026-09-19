//! # vca-plugin-host
//!
//! 插件宿主层：负责插件的**发现、校验、加载、运行与市场管理**。
//!
//! 设计依据（策划书 4.2 / 4.3）：
//! - 插件以目录形式分发，含 `manifest.toml`、可执行体、校验文件；
//! - 运行模式默认 `process`（JSON-RPC over stdio），另支持 `cdylib` / `wasm`；
//! - `api_version` 大版本不匹配  拒绝加载；小版本向后兼容；
//! - v1 插件市场**仅本地**，预留远程 URL 配置项但默认留空。
//!
//! 已实现：清单校验（[`manifest`]）、目录发现（[`host`]）、
//! 进程拉起与 JSON-RPC 收发（[`runner`]）、本地市场（[`market`]）。

#![forbid(unsafe_code)]
#![warn(missing_docs)]

pub mod host;
pub mod manifest;
pub mod market;
pub mod runner;

/// 宿主支持的插件接口大版本。插件的 `api_version` 必须与之匹配。
pub const SUPPORTED_API_VERSION: &str = "1";
