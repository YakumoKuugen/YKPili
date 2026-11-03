//! 动态卡片的纯计算：文案清洗、行数估算、图文网格尺寸与卡片高度。
//!
//! 不接触 GPUI 元素，也不依赖宿主；渲染层与占位卡共用同一套测量。

use std::cell::{Cell, RefCell};
use std::rc::Rc;

use gpui::assets::IconName;
use gpui::component::Icon;
use gpui::prelude::*;
use gpui::*;
use yk_domain::format;
use yk_infra::dynamic::{DynamicImage, DynamicItem, DynamicKind, DynamicVideo};

use crate::card::CardHost;

/// 动态卡片需要的宿主能力（在视频卡片能力之上）。
pub trait DynamicCardHost: CardHost {
    /// 打开图片查看器并定位到第 `index` 张。
    fn dynamic_open_images(
        &mut self,
        _urls: Vec<String>,
        _index: usize,
        _window: &mut Window,
        _cx: &mut Context<Self>,
    ) {
    }
}
use crate::image::ImageResidency;
use yk_theme::theme::*;

pub const IMAGE_GAP: f32 = 6.0;
pub const CARD_PADDING: f32 = 14.0;
pub const HEADER_HEIGHT: f32 = 40.0;
pub const FOOTER_HEIGHT: f32 = 38.0;
pub const TITLE_LINE_HEIGHT: f32 = 22.0;
pub const BODY_LINE_HEIGHT: f32 = 20.0;

/// 卡面文本：把换行压成空格。
///
/// GPUI 的 `line_clamp` 对「显式换行分段」的文本只按段生效，一条 10 行的动态
/// 在卡片上会整段铺开，既超出估算高度也破坏瀑布流。卡片只需要摘要，
/// 因此这里统一压成单段，由宽度自然换行后再 clamp。
pub fn card_text(text: &str) -> String {
    text.split_whitespace().collect::<Vec<_>>().join(" ")
}

pub fn text_lines(text: &str, width: f32, font_size: f32, max_lines: usize) -> usize {
    let text = text.trim();
    if text.is_empty() || width <= 0.0 {
        return 0;
    }
    // 中文字宽 ≈ 字号；拉丁字符按 0.55 字宽折算。这个比例只用于布局估算，
    // 不参与最终排版（文本仍由 GPUI 自己换行）。
    let per_line = (width / (font_size * 0.96)).max(3.0);
    let mut lines = 1_usize;
    let mut used = 0.0_f32;
    for ch in text.chars() {
        if ch == '\n' {
            lines += 1;
            used = 0.0;
            continue;
        }
        let w = if ch.is_ascii() { 0.55 } else { 1.0 };
        if used + w > per_line {
            lines += 1;
            used = w;
        } else {
            used += w;
        }
    }
    lines.min(max_lines)
}

pub fn image_metrics(inner: f32, images: &[DynamicImage]) -> (usize, f32, f32) {
    let n = images.len().min(9);
    if n == 0 {
        return (0, 0.0, 0.0);
    }
    if n == 1 {
        let h = (inner / images[0].aspect().clamp(0.5, 3.0)).clamp(150.0, 340.0);
        return (1, inner, h);
    }
    let cols = match n {
        2 => 2,
        3 => 3,
        4 => 2,
        _ => 3,
    };
    let cell = (inner - IMAGE_GAP * (cols - 1) as f32) / cols as f32;
    let rows = n.div_ceil(cols);
    let h = cell * rows as f32 + IMAGE_GAP * (rows - 1) as f32;
    (cols, cell, h)
}

/// 卡片总高度。
///
/// 结构：上下 padding + 头部 + 标题 + 正文 + 媒体 + 底部操作。
pub fn card_height(item: &DynamicItem, width: f32) -> f32 {
    let inner = (width - CARD_PADDING * 2.0).max(80.0);
    let mut h = CARD_PADDING + HEADER_HEIGHT + 10.0;

    let title = card_text(&item.title);
    let text = card_text(&item.text);
    if !title.is_empty() {
        let lines = text_lines(&title, inner, FONT_SIZE_BODY, 2);
        h += lines as f32 * TITLE_LINE_HEIGHT + 6.0;
    }
    if !text.is_empty() {
        let max_lines = if item.images.is_empty() && item.video.is_none() {
            6
        } else {
            4
        };
        let lines = text_lines(&text, inner, FONT_SIZE_CONTROL, max_lines);
        h += lines as f32 * BODY_LINE_HEIGHT + 6.0;
    }

    match item.kind {
        DynamicKind::Video => {
            if item.video.is_some() {
                h += (inner * 0.44 * 9.0 / 16.0).max(96.0) + 8.0;
            }
        }
        DynamicKind::Images => {
            if !item.images.is_empty() {
                let (_, _, image_h) = image_metrics(inner, &item.images);
                h += image_h + 8.0;
            }
        }
        DynamicKind::Forward => {
            if item.origin.is_some() {
                h += 10.0 + 16.0 + 2.0 * 18.0 + 10.0 + 8.0;
            }
        }
        DynamicKind::Link => {
            if item.link.is_some() && item.images.is_empty() {
                h += 96.0 + 8.0;
            }
        }
        DynamicKind::Text => {}
    }

    h + 10.0 + FOOTER_HEIGHT + CARD_PADDING
}

pub fn header<V: crate::card::CardHost + 'static>(
    t: &YkTheme,
    item: &DynamicItem,
    scale_factor: f32,
    load_image: bool,
    active_images: Rc<RefCell<ImageResidency>>,
    // 只有真实卡片能点：ghost 卡片是测量用的，不挂交互。
    on_avatar: Option<&mut Context<V>>,
) -> impl IntoElement {
    let s = &t.surfaces;
    let p = &t.palette;
    let face_url = if load_image {
        item.author_face_url(36.0, scale_factor)
    } else {
        String::new()
    };
    if !face_url.is_empty() {
        active_images.borrow_mut().mark(&face_url);
    }
    div()
        .flex()
        .items_start()
        .gap(px(SPACE_2))
        .h(px(HEADER_HEIGHT))
        .w_full()
        .child({
            // 「正在关注」的卡片也点头像进空间：和首页卡片署名的行为保持一致。
            let avatar = div()
                .relative()
                .w(px(36.))
                .h(px(36.))
                .rounded_full()
                .overflow_hidden()
                .flex_shrink_0()
                .bg(linear_gradient(
                    135.,
                    linear_color_stop(p.bili_blue, 0.),
                    linear_color_stop(p.bili_pink, 1.),
                ))
                .when(!face_url.is_empty(), |el| {
                    el.child(
                        img(ImageSource::from(face_url.clone()))
                            .absolute()
                            .inset_0()
                            .w_full()
                            .h_full()
                            .object_fit(ObjectFit::Cover),
                    )
                })
                .into_any_element();
            match on_avatar {
                Some(cx) => V::card_up_avatar(
                    SharedString::from(format!("following-up-{}", item.index)),
                    item.author.mid,
                    item.author.name.clone(),
                    item.author.face.clone(),
                    36.0,
                    avatar,
                    cx,
                ),
                None => avatar,
            }
        })
        .child(
            div()
                .flex_1()
                .min_w_0()
                .flex()
                .flex_col()
                .gap(px(2.))
                .pt(px(1.))
                .child(
                    div()
                        .text_size(px(FONT_SIZE_CONTROL))
                        .font_weight(FontWeight::SEMIBOLD)
                        .text_color(s.text_primary)
                        .truncate()
                        .child(item.author_name().to_string()),
                )
                .child(
                    div()
                        .text_size(px(FONT_SIZE_MICRO))
                        .text_color(s.text_quaternary)
                        .truncate()
                        .child(item.meta_line()),
                ),
        )
        .child(
            div()
                .w(px(24.))
                .h(px(24.))
                .flex()
                .items_center()
                .justify_center()
                .rounded(px(RADIUS_HALF))
                .text_color(s.text_quaternary)
                .child(Icon::new(IconName::Ellipsis).size(px(ICON_SIZE_SM))),
        )
}

pub fn image_grid<V: crate::dynamic_card::DynamicCardHost + 'static>(
    t: &YkTheme,
    images: &[DynamicImage],
    inner: f32,
    scale_factor: f32,
    load_image: bool,
    active_images: Rc<RefCell<ImageResidency>>,
    idx: usize,
    mut cx: Option<&mut Context<V>>,
) -> impl IntoElement {
    // 查看器要用整组原图（不是卡面缩略图），点哪张就从哪张开始。
    let viewer_urls: Vec<String> = images.iter().map(|image| image.detail_url()).collect();
    let s = &t.surfaces;
    let (cols, cell, total_h) = image_metrics(inner, images);
    let shown = images.len().min(9);
    let hidden = images.len().saturating_sub(shown);
    let mut grid = div()
        .mt(px(8.))
        .flex()
        .flex_wrap()
        .gap(px(IMAGE_GAP))
        .w(px(inner))
        .h(px(total_h))
        .overflow_hidden();

    for (i, image) in images.iter().take(shown).enumerate() {
        let is_last = i + 1 == shown;
        let w = if cols == 1 { inner } else { cell };
        let h = if cols == 1 { total_h } else { cell };
        let image_url = if load_image {
            image.fit_url(w, scale_factor)
        } else {
            String::new()
        };
        let tile_id = ("dynamic-image", (idx << 8) | i);
        let mut tile = div()
            .id(tile_id)
            .relative()
            .w(px(w))
            .h(px(h))
            .flex_shrink_0()
            .rounded(px(RADIUS_MD))
            .overflow_hidden()
            .bg(s.fill_3);
        if !image_url.is_empty() {
            active_images.borrow_mut().mark(&image_url);
            tile = tile.child(
                img(ImageSource::from(image_url.clone()))
                    .absolute()
                    .inset_0()
                    .w_full()
                    .h_full()
                    .object_fit(ObjectFit::Cover),
            );
        }
        if is_last && hidden > 0 {
            tile = tile.child(
                div()
                    .absolute()
                    .inset_0()
                    .flex()
                    .items_center()
                    .justify_center()
                    .bg(rgba(0x000000B3))
                    .text_size(px(FONT_SIZE_BODY))
                    .font_weight(FontWeight::SEMIBOLD)
                    .text_color(rgb(0xFFFFFF))
                    .child(format!("+{hidden}")),
            );
        }
        // 点击放大：打开图片查看器并定位到这一张（ghost 卡片没有交互，不挂）。
        if let Some(cx) = cx.as_deref_mut() {
            let urls = viewer_urls.clone();
            tile = tile
                .cursor_pointer()
                .on_click(cx.listener(move |this, _: &ClickEvent, window, cx| {
                    this.dynamic_open_images(urls.clone(), i, window, cx);
                }));
        }
        grid = grid.child(tile);
    }
    grid
}

pub fn video_block(
    t: &YkTheme,
    video: &yk_infra::dynamic::DynamicVideo,
    inner: f32,
    scale_factor: f32,
    load_image: bool,
    active_images: Rc<RefCell<ImageResidency>>,
    cover_bounds: Rc<Cell<Option<Bounds<Pixels>>>>,
    root_bounds: Rc<Cell<Option<Bounds<Pixels>>>>,
) -> impl IntoElement {
    let s = &t.surfaces;
    let p = &t.palette;
    let thumb_w = (inner * 0.44).max(110.0);
    let thumb_h = (thumb_w * 9.0 / 16.0).max(96.0);
    let cover_url = video.cover_url(thumb_w, scale_factor);
    let root_for_prepaint = root_bounds;
    let cover_for_prepaint = cover_bounds;
    // 封面走公共组件：渐变卡面 + 真实封面 + 播放 / 弹幕 / 时长。
    // 外层固定 w/h 保留原几何（窄卡高度有 96 下限，不能硬套 16:9），
    // 所以按实际 w/h 传 ratio；canvas 继续负责记录播放浮窗的入场矩形。
    if load_image && !cover_url.is_empty() {
        active_images.borrow_mut().mark(&cover_url);
    }
    let art = crate::cover::cover_art(
        &video.title,
        if load_image { cover_url.as_str() } else { "" },
        Some("#333333"),
        s.fill_3,
    )
    .into_any_element();
    let thumb = div()
        .relative()
        .w(px(thumb_w))
        .h(px(thumb_h))
        .flex_shrink_0()
        .rounded(px(RADIUS_LG))
        .overflow_hidden()
        .child(
            canvas(
                move |bounds, _window, _cx| {
                    if let Some(root) = root_for_prepaint.get() {
                        cover_for_prepaint.set(Some(Bounds {
                            origin: point(
                                bounds.origin.x - root.origin.x,
                                bounds.origin.y - root.origin.y,
                            ),
                            size: bounds.size,
                        }));
                    }
                },
                |_, _, _, _| {},
            )
            .absolute()
            .inset_0(),
        )
        .child(crate::cover::cover_visual(
            t,
            crate::cover::CoverVisual {
                art,
                views: video.play_count,
                danmaku: video.danmaku_count,
                duration_secs: yk_infra::search::parse_duration(&video.duration_text),
                is_live: false,
                show_stats: true,
            },
            thumb_w / thumb_h,
        ));

    div()
        .mt(px(8.))
        .flex()
        .items_start()
        .gap(px(SPACE_3))
        .w_full()
        .child(thumb)
        .child(
            div()
                .flex_1()
                .min_w_0()
                .flex()
                .flex_col()
                .gap(px(SPACE_1))
                .child(
                    div()
                        .text_size(px(FONT_SIZE_CONTROL))
                        .line_height(px(LINE_HEIGHT_CONTROL))
                        .font_weight(FontWeight::MEDIUM)
                        .text_color(s.text_primary)
                        .line_clamp(3)
                        .child(card_text(&video.title)),
                )
                .child(
                    div()
                        .flex()
                        .items_center()
                        .gap(px(3.))
                        .text_size(px(FONT_SIZE_MICRO))
                        .text_color(p.theme_primary)
                        .child(
                            Icon::new(IconName::Play)
                                .size(px(11.))
                                .text_color(p.theme_primary),
                        )
                        .child(format!("播放 {}", format::count(video.play_count))),
                ),
        )
}
