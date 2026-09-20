//! # vca-gui
//!
//! VCA 的图形界面。
//!
//! ## 为什么是「内嵌 HTTP 服务 + 系统浏览器」而不是原生 GUI 框架
//!
//! 目标机型是希沃一体机（Win10，配置一般）。三种做法对比：
//!
//! | 方案 | 代价 |
//! |---|---|
//! | Tauri | 要 Node 构建链 + WebView2 SDK，发布链条变长 |
//! | egui / iced | 纯 Rust 很省事，但做不出这种带阴影、圆角、动效的页面 |
//! | **内嵌 HTTP + 浏览器 `--app`** | **零额外依赖，页面完全自由** |
//!
//! 选第三种：`tiny_http` 起一个只监听回环地址的小服务，前端是纯 HTML/CSS/JS
//! （不需要任何构建工具，直接 `include_str!` 编进 exe），
//! 然后用 `msedge --app=http://127.0.0.1:端口` 打开 ——
//! **没有地址栏、独立窗口、有任务栏图标**，观感上就是一个原生应用。
//! Win10 一定自带 Edge，所以目标机上不需要额外安装任何东西。
//!
//! ## 安全边界
//!
//! 服务只绑 `127.0.0.1`，外网访问不到。再加一个每次启动随机生成的 token
//! 放在 URL 里 —— 防的是本机上别的程序（或误开的浏览器标签）来调我们的接口，
//! 而不是防网络攻击。**这个接口能改配置、能触发录制，不能裸奔。**

#![forbid(unsafe_code)]
#![warn(missing_docs)]

pub mod api;
pub mod daemon;
pub mod server;
pub mod web;

pub use server::{serve, ServeOptions};
