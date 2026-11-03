//! 应用级路由。
//!
//! shell 只定义「有哪些页面」与标题，不关心页面内容怎么画；
//! 具体 screen 由组装点映射并渲染。

/// 当前页面。`None` 表示首页（个性推荐）。
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Route {
    Hot,
    Category,
    Rank,
    Live,
    /// 关注动态：底部 Dock 的「正在关注」入口。
    Following,
    History,
    Messages,
    Favorites,
    Mine,
    Settings,
    Search,
}

impl Route {
    pub fn title(self) -> &'static str {
        match self {
            Self::Hot => "热门",
            Self::Category => "分区",
            Self::Rank => "视频排行榜",
            Self::Live => "直播",
            Self::Following => "正在关注",
            Self::History => "观看历史",
            Self::Messages => "消息中心",
            Self::Favorites => "我的收藏",
            Self::Mine => "我的",
            Self::Settings => "设置",
            Self::Search => "搜索结果",
        }
    }
}
