//! yk-core —— 数据与业务核心（不依赖 UI）。
//!
//! 对应 Avalonia 时代的 `YKPili/Models/` + `YKPili/Services/` 的纯数据部分：
//! 视图模型（`*ViewModel`）里的**状态**在 GPUI 版由各视图的 `Render` 持有，
//! 而**数据形状与格式化规则**留在这里，因为它们是可单测、与框架无关的。
//!
//! 一条硬规则：本 crate **不依赖 gpui**。这样数据层可以在没有窗口的情况下跑测试，
//! 也能被未来的移动端壳/CLI 复用。

pub mod api;
pub mod cache;
pub mod danmaku;
pub mod feed;
pub mod format;
pub mod login;
pub mod models;
pub mod player;
pub mod session;

pub use cache::CacheManager;
pub use danmaku::{Danmaku, DanmakuMode, DanmakuSet, LaneAllocator};
pub use feed::{Feed, MAX_ITEMS};
pub use login::{LoginFlow, PollAction, POLL_INTERVAL_MS};
pub use session::{AppShell, Screen};
pub use player::{PlayerPrefs, PlayerState, RATE_OPTIONS};
pub use models::{CoverItem, LoginState, UserSession};
