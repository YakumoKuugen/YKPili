//! yk-library —— 媒体库能力层。
//!
//! 负责「浏览内容」：列表、搜索、详情页。把领域模型（`yk-core`）映射成
//! GPUI 元素，只在这里组合组件，不重复实现通用积木。
//!
//! 分层约定：
//! - 页面 / 列表的纯数据与状态放这里；
//! - GPUI 的焦点、hover、布局测量只出现在本 crate 的 view 代码里；
//! - 网络请求走 `yk-core` 的接口封装，不在视图里直接发请求。
//!
//! 迁移进度：`pages`（热门 / 分区 / 排行 / 直播等页面的四态骨架）已迁入；
//! 收藏夹 / 历史 / 搜索 / 关注流仍暂存在 `yk-app`，等视频卡片与 `AppRoot`
//! 解耦后按同一套 host 接口迁入。

pub mod card;
pub mod cover;
pub mod dynamic_card;
pub mod favorites;
pub mod following;
pub mod image;
pub mod history;
pub mod image_viewer;
pub mod page_image_cache;
pub mod messages;
pub mod messages_ui;
pub mod pages;
pub mod search;
pub mod search_ui;
pub mod settings_view;
pub mod rank;
pub mod up_profile;
