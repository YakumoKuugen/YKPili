//! yk-core —— 数据与业务核心（不依赖 UI）。
//!
//! 对应 Avalonia 时代的 `YKPili/Models/` + `YKPili/Services/` 的纯数据部分：
//! 视图模型（`*ViewModel`）里的**状态**在 GPUI 版由各视图的 `Render` 持有，
//! 而**数据形状与格式化规则**留在这里，因为它们是可单测、与框架无关的。
//!
//! 一条硬规则：本 crate **不依赖 gpui**。这样数据层可以在没有窗口的情况下跑测试，
//! 也能被未来的移动端壳/CLI 复用。

pub mod api;
pub mod auth;
pub mod cache;
pub mod danmaku;
pub mod dynamic;
pub mod feed;
pub mod feed_cache;
pub mod format;
pub mod history;
pub mod image_cache;
pub mod login;
pub mod messages;
pub mod models;
pub mod playback;
pub mod player;
pub mod resume;
pub mod search;
pub mod session;
pub mod settings;

pub use cache::CacheManager;
pub use danmaku::{Danmaku, DanmakuMode, DanmakuSet, LaneAllocator};
pub use dynamic::{
    CommentTarget, DynamicAuthor, DynamicComment, DynamicImage, DynamicItem, DynamicKind,
    DynamicPage, DynamicStats, DynamicVideo,
};
pub use feed::{Feed, MAX_ITEMS};
pub use history::{HistoryCursor, HistoryDay, HistoryEntry};
pub use image_cache::ImageDiskCache;
pub use login::{LoginFlow, PollAction, POLL_INTERVAL_MS};
pub use messages::{
    ChatKind, ChatMessage, Conversation, InboxCursor, MessageChannel, MessageEntry, MessageTarget,
    MessageUser, UnreadCounts,
};
pub use models::{CoverItem, LoginState, UserSession};
pub use playback::PlaybackSource;
pub use player::{PlayerPrefs, PlayerState, RATE_OPTIONS};
pub use resume::{ResumeRecord, ResumeStore};
pub use search::{
    Hotword, SearchArticle, SearchHistory, SearchLive, SearchMedia, SearchPage, SearchResults,
    SearchSort, SearchTab, SearchUser, SearchVideo, SuggestItem,
};
pub use session::{AppShell, Screen};
pub use settings::AppSettings;
