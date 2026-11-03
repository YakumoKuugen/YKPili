//! yk-domain —— 领域模型与纯逻辑（对应目录 `crates/core`）。
//!
//! 不依赖 gpui、网络、存储：换一个 UI 框架或宿主仍然成立。
//! 包名用 `yk-domain`，因为 `core` 与 Rust 内置 crate 冲突。

pub mod api_models;
pub mod danmaku;
pub mod format;
pub mod models;
pub mod playback;
pub mod player;
