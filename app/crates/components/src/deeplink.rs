//! 深链提示气泡。
//!
//! 用户从浏览器点视频 → 系统拉起客户端 → 播放器开始取流，这中间有
//! 几百毫秒「我点的东西去哪了」的空窗。这个组件负责给一句明确反馈：
//! 「已接管 BV 号 / 正在解析短链 / 链接无法识别」。
//!
//! 视觉沿用应用里的浮层语言：深色玻璃、1px 描边、圆角 16、无按钮，
//! 3 秒后由 `AppRoot` 自动收起，不抢焦点。

use gpui::prelude::*;
use gpui::*;

use yk_theme::theme::*;

/// 提示语义。
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum DeepLinkTone {
    /// 已受理，正在打开。
    Success,
    /// 解析中 / 普通提示。
    Info,
    /// 无法识别 / 解析失败。
    Error,
}

/// 一条深链提示的纯数据。`AppRoot` 持有它，渲染时再包成元素。
#[derive(Debug, Clone)]
pub struct DeepLinkNotice {
    pub title: String,
    pub detail: String,
    pub tone: DeepLinkTone,
}

impl DeepLinkNotice {
    pub fn success(title: impl Into<String>, detail: impl Into<String>) -> Self {
        Self {
            title: title.into(),
            detail: detail.into(),
            tone: DeepLinkTone::Success,
        }
    }

    pub fn info(title: impl Into<String>, detail: impl Into<String>) -> Self {
        Self {
            title: title.into(),
            detail: detail.into(),
            tone: DeepLinkTone::Info,
        }
    }

    pub fn error(title: impl Into<String>, detail: impl Into<String>) -> Self {
        Self {
            title: title.into(),
            detail: detail.into(),
            tone: DeepLinkTone::Error,
        }
    }
}

/// GPUI 组件：一条深链提示。
pub struct DeepLinkToast {
    theme: YkTheme,
    notice: DeepLinkNotice,
}

impl DeepLinkToast {
    pub fn new(theme: &YkTheme, notice: &DeepLinkNotice) -> Self {
        Self {
            theme: theme.clone(),
            notice: notice.clone(),
        }
    }

    /// 产出元素。
    ///
    /// 这里用普通方法而不是 `RenderOnce`：应用侧启用 `gpui-fast` 后，
    /// `IntoElement` 来自 gpui-fast，而组件 crate 直接依赖的 `gpui`
    /// 是 gpui-pre；跨 crate 的 `RenderOnce` blanket impl 不会被另一份
    /// trait 认。返回 `impl IntoElement` 的函数则一直可用。
    pub fn build(self) -> impl IntoElement {
        let t = &self.theme;
        let s = &t.surfaces;
        let p = &t.palette;
        let notice = self.notice;

        let (accent, glyph) = match notice.tone {
            DeepLinkTone::Success => (p.bili_blue, "↗"),
            DeepLinkTone::Info => (p.theme_primary, "…"),
            DeepLinkTone::Error => (p.status_error, "!"),
        };

        div()
            .id("yk-deeplink-toast")
            .w(px(420.))
            .px(px(SPACE_4))
            .py(px(SPACE_3))
            .flex()
            .items_center()
            .gap(px(SPACE_3))
            .rounded(px(RADIUS_2XL))
            .bg(s.surface_content_solid)
            .border_1()
            .border_color(s.border_popover)
            .shadow(t.shadows.shadow_2.clone())
            .occlude()
            .child(
                div()
                    .size(px(34.))
                    .flex_shrink_0()
                    .flex()
                    .items_center()
                    .justify_center()
                    .rounded_full()
                    .bg(accent.opacity(0.16))
                    .text_size(px(15.))
                    .text_color(accent)
                    .child(glyph),
            )
            .child(
                div()
                    .flex()
                    .flex_col()
                    .gap(px(2.))
                    .min_w(px(0.))
                    .child(
                        div()
                            .text_size(px(FONT_SIZE_BODY))
                            .text_color(s.text_primary)
                            .child(notice.title),
                    )
                    .child(
                        div()
                            .text_size(px(FONT_SIZE_CAPTION))
                            .text_color(s.text_tertiary)
                            .child(notice.detail),
                    ),
            )
    }
}
