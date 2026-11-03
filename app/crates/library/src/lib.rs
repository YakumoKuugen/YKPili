//! yk-library —— 媒体库能力层。
//!
//! 负责「浏览内容」：列表、搜索、详情页。把领域模型（`yk-domain`）映射成
//! GPUI 元素，只在这里组合组件，不重复实现通用积木。
//!
//! 分层约定：
//! - 页面 / 列表的纯数据与状态放这里；
//! - GPUI 的焦点、hover、布局测量只出现在本 crate 的 view 代码里；
//! - 网络请求走 `yk-domain` 的接口封装，不在视图里直接发请求。
//!
//! 迁移进度：`pages`、收藏夹、历史、搜索、消息、动态（卡片 / 详情 / 关注流）
//! 与首页网格已全部迁入；`yk-app` 只剩组装点与界面装配。

pub mod card;
pub mod cover;
pub mod dynamic_card;
pub mod dynamic_view;
pub mod favorites;
pub mod following;
pub mod image;
pub mod history;
pub mod home;
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
