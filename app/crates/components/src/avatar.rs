//! 通用头像：渐变 / 纯色占位 + 首字兜底 + 远程图覆盖。
//!
//! 消息、顶栏、UP 主、动态等处的头像都走这一份，避免每个页面各画一遍
//! 「圆角裁切 + 首字 + 图片盖上去」。

use gpui::prelude::*;
use gpui::*;

use crate::media::square_image;

/// 头像构建器。
pub struct Avatar {
    url: Option<String>,
    initial: String,
    size: f32,
    background: Background,
    fg: Hsla,
    ring: Option<Hsla>,
}

impl Avatar {
    pub fn new(size: f32) -> Self {
        Self {
            url: None,
            initial: String::new(),
            size,
            background: rgb(0x3A3A3A).into(),
            fg: rgb(0xFFFFFF).into(),
            ring: None,
        }
    }

    pub fn url(mut self, url: impl Into<String>) -> Self {
        let url = url.into();
        self.url = (!url.trim().is_empty()).then_some(url);
        self
    }

    pub fn initial(mut self, initial: impl Into<String>) -> Self {
        self.initial = initial.into();
        self
    }

    pub fn background(mut self, background: Background) -> Self {
        self.background = background;
        self
    }

    pub fn text_color(mut self, fg: Hsla) -> Self {
        self.fg = fg;
        self
    }

    pub fn ring(mut self, color: Hsla) -> Self {
        self.ring = Some(color);
        self
    }

    pub fn build(self) -> AnyElement {
        let size = self.size;
        let mut el = div()
            .relative()
            .flex_shrink_0()
            .w(px(size))
            .h(px(size))
            .rounded_full()
            .overflow_hidden()
            .bg(self.background)
            .flex()
            .items_center()
            .justify_center()
            .text_size(px((size * 0.4).max(10.)))
            .font_weight(FontWeight::SEMIBOLD)
            .text_color(self.fg)
            .child(self.initial);
        if let Some(ring) = self.ring {
            el = el.border_1().border_color(ring);
        }
        if let Some(url) = self.url.as_deref() {
            if let Some(image) = square_image(url, size) {
                el = el.child(image);
            }
        }
        el.into_any_element()
    }
}

/// 昵称首字：头像占位与图加载失败时的兜底。
pub fn initial_of(nickname: &str) -> String {
    nickname.chars().next().unwrap_or('?').to_string()
}
