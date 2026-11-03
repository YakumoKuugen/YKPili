// gpui 的链式构建器 + #[test] 宏展开较深，默认 128 不够。
#![recursion_limit = "512"]

//! yk-theme —— 设计 token 层。
//!
//! 只放颜色、间距、字号、动效、阴影这些**纯令牌**，不含任何业务逻辑，
//! 也不依赖 `yk-core` / 网络 / 存储。视图与组件都从这里取值，保证
//! 「一处改令牌，全应用生效」。
//!
//! 对应 Avalonia 时代的 `YKPili/Styles/`：6 个令牌字典换成了
//! 6 个 Rust 模块；`gpui_theme` 负责把令牌桥接到 gpui-component 的全局主题。

pub mod gpui_theme;
pub mod theme;
