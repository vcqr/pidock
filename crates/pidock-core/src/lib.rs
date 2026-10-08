//! PiDock 运行时核心：pi-host supervisor、定时任务调度器、云同步 agent。
//!
//! 与 UI 壳完全解耦（Tauri 桌面壳 / 无头 webhost 壳共用）：
//! - 事件统一走 tokio broadcast（[`CoreCtx::events`]），壳层各自订阅——
//!   桌面壳泵到 webview，webhost 推给浏览器 WS 客户端；
//! - 命令面统一在 [`CoreCtx::handle`]，Tauri 命令与浏览器 WS 桥都是薄转换层；
//! - 壳层只负责：窗口/托盘（桌面）、HTTP+WS（webhost）、原生对话框。

pub mod config;
pub mod ctx;
pub mod scheduler;
pub mod supervisor;
pub mod sync;

pub use ctx::{CoreCtx, CorePaths};
