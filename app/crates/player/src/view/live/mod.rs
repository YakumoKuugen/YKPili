//! 直播模式：会话状态 + 动作 + 面板。
//!
//! `video/*` 与 `live/*` 互不依赖；外壳只通过 `mode.rs` 的插槽与
//! `PlayerView::mode_kind()` 分发。

mod actions;
mod panel;
mod state;

pub(crate) use state::LiveState;
pub(crate) use actions::watched_text;
