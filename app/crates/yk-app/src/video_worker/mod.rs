//! 视频渲染线程（平台分派）。
//!
//! 两个平台共用同一套公开接口（`FrameWorker::{spawn_with_audio, set_paused,
//! seek, finished, latest}`），但帧的形态完全不同：
//!
//! - **macOS**：mpv → RGBA → NV12 → IOSurface-backed `CVPixelBuffer`，
//!   交给 GPUI 的 `surface()` 与控件同管线合成（零拷贝）；
//! - **Windows**：mpv 直接出 BGRA，交给 GPUI 的 `RenderImage` 贴图。
//!
//! 平台实现分文件，互不 `cfg` 嵌套，避免一端的改动影响另一端。

#[cfg(target_os = "macos")]
mod macos;
#[cfg(target_os = "windows")]
mod windows;

#[cfg(target_os = "macos")]
pub use macos::FrameWorker;
#[cfg(target_os = "windows")]
pub use windows::{FrameWorker, VideoFrame};
