//! yk-shell —— 窗口外壳与全局浮层。
//!
//! 负责「不属于某个页面、但压在所有页面之上」的东西：登录卡片、账号面板、
//! 后续的顶栏 / Dock / 路由 / 窗口框架。
//!
//! 分层约定：
//! - 这里可以有 GPUI 的焦点、遮罩、动画，但不应持有播放器或媒体库状态；
//! - 打开播放器、切页这类跨层动作通过回调交回组装点（`app`），
//!   避免 shell 反向依赖 player / library。
//!
//! 迁移进度：登录卡片与账号面板已迁入；顶栏、Dock、路由仍暂存在
//! `yk-app` 的 `AppRoot` 里，等 `AppRoot` 拆出 `ShellState` 后按同一套
//! 回调接口迁入。

pub mod account_view;
pub mod dock;
pub mod frame;
pub mod top_bar;
pub mod login_view;
pub mod overlay;
pub mod root;
pub mod route;
