//! 根布局：背景 + 内容列 + Dock。
//!
//! 只负责叠放顺序，不生产任何页面内容。内容列与 Dock 由组装点提供。

use gpui::prelude::*;
use gpui::*;
use yk_theme::theme::*;

/// 根视图骨架。
pub struct ShellFrame {
    /// 顶栏（窗口 chrome）。
    pub top_bar: AnyElement,
    /// 页面内容（由 app 组装）。
    pub body: AnyElement,
    /// 底部 Dock。
    pub dock: AnyElement,
}

impl ShellFrame {
    pub fn render(self, t: &YkTheme) -> Div {
        div()
            .relative()
            .size_full()
            // 背景必须最先画，后面的内容与浮层才不会透出窗口底色。
            .child(crate::root::background(t))
            .child(
                div()
                    .relative()
                    .size_full()
                    .flex()
                    .flex_col()
                    .child(self.top_bar)
                    .child(self.body),
            )
            .child(self.dock)
    }
}
