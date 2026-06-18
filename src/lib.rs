//! rdm 库根：把各业务模块导出为 pub，供 CLI 二进制和 Tauri GUI 共用。
//!
//! 模块内部的 `crate::xxx` 路径在 lib crate 下自动指向这里，无需改动。

pub mod categories;
pub mod downloader;
pub mod ipc;
pub mod manager;
pub mod meta;
pub mod server;
pub mod store;
pub mod task;
