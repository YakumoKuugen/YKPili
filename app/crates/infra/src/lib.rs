//! yk-infra —— 与外部世界打交道的实现层：B 站接口、缓存、会话、平台路径。
//!
//! 依赖 `yk-domain` 的纯逻辑；不依赖 gpui（`http` 模块除外，它是 GPUI 的
//! HTTP 客户端适配）。

// 领域模块在 crate 内部以 `crate::models` 等路径使用，不对外重复导出。
pub(crate) use yk_domain::{api_models, danmaku, format, models, playback, player};

pub mod api;
pub mod auth;
pub mod cache;
pub mod device;
pub mod dynamic;
pub mod favorites;
pub mod feed;
pub mod feed_cache;
pub mod history;
pub mod http;
pub mod image_cache;
pub mod interaction;
pub mod login;
pub mod media;
pub mod messages;
pub mod platform;
pub mod ranking;
pub mod resume;
pub mod search;
pub mod session;
pub mod settings;
pub mod space;
#[cfg(target_os = "windows")]
pub mod win_clipboard;

pub use cache::CacheManager;
pub use yk_domain::danmaku::{Danmaku, DanmakuMode, DanmakuSet, LaneAllocator};
pub use dynamic::{
    CommentTarget, DynamicAuthor, DynamicComment, DynamicImage, DynamicItem, DynamicKind,
    DynamicPage, DynamicStats, DynamicVideo,
};
pub use feed::{Feed, MAX_ITEMS};
pub use favorites::{FavVideo, FavVideoPage, FAV_PAGE_SIZE};
pub use history::{HistoryCursor, HistoryDay, HistoryEntry};
pub use image_cache::ImageDiskCache;
pub use interaction::{FavFolder, InteractionState};
pub use login::{LoginFlow, PollAction, POLL_INTERVAL_MS};
pub use messages::{
    ChatKind, ChatMessage, Conversation, InboxCursor, MessageChannel, MessageEntry, MessageTarget,
    MessageUser, UnreadCounts,
};
pub use yk_domain::models::{CoverItem, LoginState, UserSession};
pub use yk_domain::playback::PlaybackSource;
pub use yk_domain::player::{PlayerPrefs, PlayerState, RATE_OPTIONS};
pub use ranking::{RankItem, RankPage, RankZone, DEFAULT_NOTE as RANK_DEFAULT_NOTE};
pub use resume::{ResumeRecord, ResumeStore};
pub use search::{
    Hotword, SearchArticle, SearchHistory, SearchLive, SearchMedia, SearchPage, SearchResults,
    SearchSort, SearchTab, SearchUser, SearchVideo, SuggestItem,
};
pub use session::{AppShell, Screen};
pub use settings::AppSettings;
