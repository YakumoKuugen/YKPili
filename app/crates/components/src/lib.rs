// gpui 的链式构建器 + #[test] 宏展开较深，默认 128 不够。
#![recursion_limit = "512"]

//! yk-components —— 通用 UI 积木。
//!
//! 这里的组件**没有业务含义**：不知道视频、UP 主、登录态，只接收
//! 主题令牌与基础数据，产出 GPUI 元素。业务页面只调用、不复制，
//! 保证同一个视觉组件全应用只有一份实现。
//!
//! 目前收进来的：加载 / 空 / 错误三态（`status`）。

pub mod avatar;
pub mod cover;
pub mod deeplink;
pub mod media;
pub mod gallery;
pub mod status;
