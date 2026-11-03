//! 封面视觉：卡片、排行榜、收藏夹、UP 主投稿共用同一份实现。
//!
//! 这里只负责「把一条媒体数据画成封面」：渐变兜底、真实封面图、直播角标、
//! 播放 / 弹幕数、时长。点击、hover、入场动画由各页面的宿主负责，
//! 避免把播放器或路由逻辑带进视觉组件。

use gpui::assets::IconName;
use gpui::component::Icon;
use gpui::prelude::*;
use gpui::*;
use yk_theme::theme::*;

/// 封面「画」：渐变卡面 + 大标题 + 真实封面图。
///
/// 首页卡片与 UP 主浮窗的投稿/动态共用这一份——封面只有一个实现，
/// 改一次三处同步，不会出现「同一条视频两个封面样式」。
pub fn cover_art(
    title: &str,
    image_url: &str,
    placeholder: Option<&str>,
    fallback: Hsla,
) -> Div {
    let bg = match placeholder {
        Some(color) => yk_components::cover::placeholder_gradient(color, fallback),
        None => linear_gradient(
            135.0,
            linear_color_stop(fallback, 0.0),
            linear_color_stop(fallback.opacity(0.72), 1.0),
        ),
    };
    let mut art = div()
        .absolute()
        .top_0()
        .left_0()
        .w_full()
        .h_full()
        .bg(bg)
        .child(
            div()
                .absolute()
                .inset_0()
                .flex()
                .items_center()
                .justify_center()
                .p(px(SPACE_3))
                .child(
                    div()
                        .text_size(px(16.))
                        .line_height(px(20.))
                        .font_weight(FontWeight::BOLD)
                        .text_color(rgb(0xFFFFFF))
                        .text_center()
                        .line_clamp(3)
                        .child(title.to_string()),
                ),
        );
    if let Some(image) = yk_components::media::cover_image(image_url) {
        art = art.child(image);
    }
    art
}

/// 封面外壳：圆角裁切 + 直播角标 + 底部压暗 + 左下播放/弹幕 + 右下时长。
///
/// `ratio` 是 CSS 语义的「宽 / 高」（16/9 就是 16:9），
/// 高度由 `aspect_ratio` 按宽度算出来——不要用 `relative()`，
/// 那是父容器**高度**的百分比，父容器高度不定时会塌成一条线。
pub struct CoverVisual {
    pub art: AnyElement,
    pub views: u64,
    pub danmaku: u64,
    pub duration_secs: u64,
    pub is_live: bool,
    /// 有统计口径才画左下那行；历史记录这类没有播放/弹幕数据的列表传 false，
    /// 否则会渲染成「▶0 💬0」这种假数据。
    pub show_stats: bool,
}

pub fn cover_visual(t: &YkTheme, v: CoverVisual, ratio: f32) -> Div {
    let mut cover = div()
        .relative()
        .w_full()
        .aspect_ratio(ratio)
        .rounded(px(MEDIA_RADIUS))
        .overflow_hidden()
        .child(v.art);
    if v.is_live {
        cover = cover.child(live_badge(t));
    }
    cover = cover.child(
        div()
            .absolute()
            .left_0()
            .right_0()
            .bottom_0()
            .h(px(40.))
            .bg(linear_gradient(
                180.,
                linear_color_stop(rgba(0x00000000), 0.),
                linear_color_stop(rgba(0x000000B3), 1.),
            )),
    );
    if v.show_stats {
        cover = cover.child(
            div()
                .absolute()
                .left(px(SPACE_2))
                .bottom(px(6.))
                .flex()
                .items_center()
                .gap(px(SPACE_3))
                .child(cover_stat(
                    IconName::Play,
                    yk_domain::format::count(v.views),
                ))
                .child(cover_stat(
                    IconName::MessageCircle,
                    yk_domain::format::count(v.danmaku),
                )),
        );
    }
    cover = cover
        .when(v.duration_secs > 0, |el| {
            el.child(
                div()
                    .absolute()
                    .right(px(SPACE_2))
                    .bottom(px(6.))
                    .text_size(px(FONT_SIZE_DATA_OVERLAY))
                    .font_weight(FontWeight::MEDIUM)
                    .text_color(rgb(0xFFFFFF))
                    .child(yk_domain::format::duration(v.duration_secs)),
            )
        });
    cover
}

/// 封面数据层的一个计数：图标 + 数值，白字压在封面下沿。
fn cover_stat(icon: IconName, value: String) -> impl IntoElement {
    div()
        .flex()
        .items_center()
        .gap(px(3.))
        .child(Icon::new(icon).size(px(14.)).text_color(rgb(0xFFFFFF)))
        .child(
            div()
                .text_size(px(FONT_SIZE_DATA_OVERLAY))
                .font_weight(FontWeight::MEDIUM)
                .text_color(rgb(0xFFFFFF))
                .child(value),
        )
}

/// 直播角标：粉色实心胶囊 + 白字 + 18 高。
fn live_badge(t: &YkTheme) -> impl IntoElement {
    let p = &t.palette;
    div()
        .absolute()
        .top(px(SPACE_2))
        .left(px(SPACE_2))
        .h(px(18.))
        .px(px(6.))
        .flex()
        .items_center()
        .gap(px(3.))
        .rounded_full()
        .bg(p.bili_pink)
        .text_size(px(FONT_SIZE_MICRO))
        .font_weight(FontWeight::SEMIBOLD)
        .text_color(rgb(0xFFFFFF))
        .child(
            Icon::new(IconName::Radio)
                .size(px(12.))
                .text_color(rgb(0xFFFFFF)),
        )
        .child("直播中")
}

