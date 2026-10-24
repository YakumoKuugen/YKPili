// gpui 的链式构建器 + #[test] 宏展开较深，默认 128 不够。
#![recursion_limit = "512"]

//! yk-ui —— YKPili 的设计系统层（GPUI 版）
//!
//! 对应 Avalonia 时代的 `YKPili/Styles/`：
//!   旧：6 个令牌字典 + 5 个控件样式文件（XAML 资源字典 + 动态资源）
//!   新：6 个令牌模块（Rust 常量 + 主题对象）+ 组件直接用 gpui-component
//!
//! 视觉数值一个都没改 —— 换的只是载体与引用方式。

pub mod gpui_theme;
pub mod status;
pub mod theme;
