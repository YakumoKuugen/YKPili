//! 动态卡片：测量（文案清洗、行数估算、图文网格尺寸、卡片高度）与整卡渲染。
//!
//! 渲染不反向依赖 app：打开播放器 / 详情 / 图片查看器都走
//! [`DynamicCardHost`]，卡片只负责画与交出冻结的几何。
//! 高度估算与渲染共用同一套测量，避免瀑布流「算得矮、画得高」。

use std::cell::{Cell, RefCell};
use std::rc::Rc;

use gpui::assets::IconName;
use gpui::component::Icon;
use gpui::prelude::*;
use gpui::*;
use yk_domain::format;
use yk_infra::dynamic::{DynamicImage, DynamicItem, DynamicKind};

use crate::card::CardHost;
use yk_player::CoverSource;

/// 点击卡片时冻结的几何（页面坐标）。
///
/// `bounds` 是整张卡，`cover_bounds` 是视频缩略图；详情浮层与播放浮窗
/// 的入场动画都从这里起飞。
#[derive(Clone, Copy, Debug)]
pub struct CardOrigin {
    pub bounds: Bounds<Pixels>,
    pub cover_bounds: Option<Bounds<Pixels>>,
    pub root_size: Size<Pixels>,
}

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

    /// 整卡点击：视频动态进播放器，其余进详情浮层。
    ///
    /// 「播放器 / 详情已经在开」这类互斥由宿主自己判定；卡片只负责把
    /// 冻结的几何交出去。
    fn dynamic_open_card(
        &mut self,
        _item: &DynamicItem,
        _origin: CardOrigin,
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

/// 头像圆片：加载中 / 失败都保留蓝→粉渐变兜底。
fn author_avatar(
    t: &YkTheme,
    item: &DynamicItem,
    scale_factor: f32,
    load_image: bool,
    active_images: Rc<RefCell<ImageResidency>>,
) -> AnyElement {
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
        .into_any_element()
}

/// 头部一行：头像 + 昵称 / 时间 + 更多。真实卡与占位卡共用。
fn header_row(t: &YkTheme, item: &DynamicItem, avatar: AnyElement) -> AnyElement {
    let s = &t.surfaces;
    div()
        .flex()
        .items_start()
        .gap(px(SPACE_2))
        .h(px(HEADER_HEIGHT))
        .w_full()
        .child(avatar)
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
        .into_any_element()
}

/// 卡片头部：`on_avatar` 为 `None` 时不挂交互（占位卡）。
pub fn header<V: crate::card::CardHost + 'static>(
    t: &YkTheme,
    item: &DynamicItem,
    scale_factor: f32,
    load_image: bool,
    active_images: Rc<RefCell<ImageResidency>>,
    on_avatar: Option<&mut Context<V>>,
) -> impl IntoElement {
    let avatar = author_avatar(t, item, scale_factor, load_image, active_images);
    let avatar = match on_avatar {
        // 「正在关注」的卡片也点头像进空间：和首页卡片署名的行为保持一致。
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
    };
    header_row(t, item, avatar)
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

// ================= 整卡 =================

/// 引用原动态的引用块（转发动态）。
fn quote_block(t: &YkTheme, origin: &DynamicItem) -> AnyElement {
    let s = &t.surfaces;
    let p = &t.palette;
    let text = {
        let raw = if let Some(video) = origin.video.as_ref() {
            format!("视频：{}", video.title)
        } else if !origin.text.trim().is_empty() {
            origin.text.clone()
        } else if !origin.title.trim().is_empty() {
            origin.title.clone()
        } else if !origin.images.is_empty() {
            format!("[图片动态 · {} 张图]", origin.images.len())
        } else {
            "原动态".to_string()
        };
        card_text(&raw)
    };
    div()
        .mt(px(8.))
        .p(px(10.))
        .rounded(px(RADIUS_LG))
        .bg(s.fill_1)
        .border_1()
        .border_color(s.border_subtle)
        .flex()
        .flex_col()
        .gap(px(2.))
        .w_full()
        .child(
            div()
                .flex()
                .items_center()
                .gap(px(2.))
                .text_size(px(FONT_SIZE_MICRO))
                .text_color(p.theme_primary)
                .child("@")
                .child(origin.author_name().to_string()),
        )
        .child(
            div()
                .text_size(px(FONT_SIZE_CAPTION))
                .line_height(px(18.))
                .text_color(s.text_secondary)
                .line_clamp(2)
                .child(text),
        )
        .into_any_element()
}

/// 外链卡：封面 + 标题 + 「查看详情 ›」。
fn link_block(
    t: &YkTheme,
    link: &yk_infra::dynamic::DynamicLink,
    inner: f32,
    scale_factor: f32,
    load_image: bool,
    active_images: Rc<RefCell<ImageResidency>>,
) -> AnyElement {
    let s = &t.surfaces;
    let p = &t.palette;
    let cover_w = (inner * 0.44).max(110.0);
    let cover_url = link.cover_url(cover_w, scale_factor);
    let mut cover = div()
        .relative()
        .w(px(cover_w))
        .h(px(96.))
        .flex_shrink_0()
        .rounded(px(RADIUS_LG))
        .overflow_hidden()
        .bg(s.fill_3);
    if load_image && !cover_url.is_empty() {
        active_images.borrow_mut().mark(&cover_url);
        cover = cover.child(
            img(ImageSource::from(cover_url.clone()))
                .absolute()
                .inset_0()
                .w_full()
                .h_full()
                .object_fit(ObjectFit::Cover),
        );
    }
    div()
        .mt(px(8.))
        .flex()
        .items_start()
        .gap(px(SPACE_3))
        .w_full()
        .child(cover)
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
                        .child(card_text(&link.title)),
                )
                .child(
                    div()
                        .text_size(px(FONT_SIZE_MICRO))
                        .text_color(p.theme_primary)
                        .child("查看详情 ›"),
                ),
        )
        .into_any_element()
}

/// 底部操作行的一格：图标 + 计数。
///
/// 卡片只做浏览入口，动作属于详情页；这里拦住冒泡即可。
fn card_action<V: DynamicCardHost + 'static>(
    t: &YkTheme,
    idx: usize,
    kind: &'static str,
    icon: IconName,
    count: u64,
    cx: &mut Context<V>,
) -> AnyElement {
    let s = &t.surfaces;
    let color = s.text_quaternary;
    div()
        .id((kind, idx))
        .flex_1()
        .h(px(28.))
        .flex()
        .items_center()
        .justify_center()
        .gap(px(4.))
        .rounded(px(RADIUS_HALF))
        .text_size(px(FONT_SIZE_MICRO))
        .text_color(color)
        .cursor_pointer()
        .hover(|style| style.bg(s.fill_1).text_color(s.text_secondary))
        .child(Icon::new(icon).size(px(14.)).text_color(color))
        .child(format::count(count))
        .on_click(cx.listener(move |_this, _: &ClickEvent, _window, cx| {
            cx.stop_propagation();
        }))
        .into_any_element()
}

/// 动态整卡：头部 + 标题 / 正文 + 媒体 + 操作行。
///
/// 卡片是「进播放器 / 进详情」的唯一入口，具体去哪儿由宿主决定
/// （`DynamicCardHost::dynamic_open_card`）；卡片只负责冻结几何。
pub fn card<V: DynamicCardHost + 'static>(
    t: &YkTheme,
    item: &DynamicItem,
    width: f32,
    height: f32,
    scale_factor: f32,
    load_images: bool,
    active_images: Rc<RefCell<ImageResidency>>,
    root_bounds: Rc<Cell<Option<Bounds<Pixels>>>>,
    cx: &mut Context<V>,
) -> impl IntoElement {
    let s = &t.surfaces;
    let inner = width - CARD_PADDING * 2.0;
    let card_bounds: Rc<Cell<Option<Bounds<Pixels>>>> = Rc::new(Cell::new(None));
    let cover_bounds: Rc<Cell<Option<Bounds<Pixels>>>> = Rc::new(Cell::new(None));

    let idx = item.index;
    let item_for_click = item.clone();
    let card_bounds_for_click = card_bounds.clone();
    let cover_bounds_for_click = cover_bounds.clone();
    let root_for_click = root_bounds.clone();

    // 「打开详情 / 视频」的唯一实现：整卡点击与文本区抬起兜底共用。
    let open_card = {
        let item = item_for_click.clone();
        let card_bounds = card_bounds_for_click.clone();
        let cover_bounds = cover_bounds_for_click.clone();
        let root = root_for_click.clone();
        move |this: &mut V, window: &mut Window, cx: &mut Context<V>| {
            let origin = CardOrigin {
                bounds: card_bounds.get().unwrap_or_else(|| Bounds {
                    origin: point(px(0.), px(0.)),
                    size: size(px(width), px(height)),
                }),
                cover_bounds: cover_bounds.get(),
                root_size: root
                    .get()
                    .map(|b| b.size)
                    .unwrap_or_else(|| size(px(1280.), px(820.))),
            };
            this.dynamic_open_card(&item, origin, window, cx);
            window.refresh();
        }
    };

    let mut body = div().flex().flex_col().w(px(inner)).child(header::<V>(
        t,
        item,
        scale_factor,
        load_images,
        active_images.clone(),
        Some(cx),
    ));

    let card_title = card_text(&item.title);
    let card_body_text = card_text(&item.text);

    // 卡片只做摘要浏览：**不接入选区**。卡片本身是点击进详情的入口，
    // 如果文字能拖动选中，按下会被选区层吞掉，还会在瀑布流里出现
    // 跨卡片的选区；复制统一放到详情浮层（`DynamicDetailView` 的右键菜单）。
    if !card_title.is_empty() {
        body = body.child(
            div()
                .mt(px(6.))
                .text_size(px(FONT_SIZE_BODY))
                .line_height(px(TITLE_LINE_HEIGHT))
                .font_weight(FontWeight::SEMIBOLD)
                .text_color(s.text_primary)
                .line_clamp(2)
                .child(card_title),
        );
    }

    if !card_body_text.is_empty() {
        let max_lines = if item.images.is_empty() && item.video.is_none() {
            6
        } else {
            4
        };
        body = body.child(
            div()
                .mt(px(6.))
                .text_size(px(FONT_SIZE_CONTROL))
                .line_height(px(BODY_LINE_HEIGHT))
                .text_color(s.text_secondary)
                .line_clamp(max_lines)
                .child(card_body_text),
        );
    }

    match item.kind {
        DynamicKind::Video => {
            if let Some(video) = item.video.as_ref() {
                body = body.child(video_block(
                    t,
                    video,
                    inner,
                    scale_factor,
                    load_images,
                    active_images.clone(),
                    cover_bounds.clone(),
                    root_bounds.clone(),
                ));
            }
        }
        DynamicKind::Images => {
            if !item.images.is_empty() {
                // 卡片上的图片不做「点击放大」：整卡是进详情的入口。
                // 放大 / 复制图片都在详情浮层里（那里再给 `Some(cx)`）。
                body = body.child(image_grid::<V>(
                    t,
                    &item.images,
                    inner,
                    scale_factor,
                    load_images,
                    active_images.clone(),
                    idx,
                    None,
                ));
            }
        }
        DynamicKind::Forward => {
            if let Some(origin) = item.origin.as_ref() {
                body = body.child(quote_block(t, origin));
            }
        }
        DynamicKind::Link => {
            if let Some(link) = item.link.as_ref() {
                body = body.child(link_block(
                    t,
                    link,
                    inner,
                    scale_factor,
                    load_images,
                    active_images.clone(),
                ));
            }
        }
        DynamicKind::Text => {}
    }

    body = body.child(
        div()
            .mt_auto()
            .pt(px(10.))
            .border_t_1()
            .border_color(s.border_subtle)
            .flex()
            .items_center()
            .justify_between()
            .w_full()
            .child(card_action(
                t,
                idx,
                "forward",
                IconName::Share2,
                item.stats.forward,
                cx,
            ))
            .child(card_action(
                t,
                idx,
                "comment",
                IconName::MessageCircle,
                item.stats.comment,
                cx,
            ))
            .child(card_action(
                t,
                idx,
                "like",
                IconName::ThumbsUp,
                item.stats.like,
                cx,
            )),
    );

    let card_bounds_for_prepaint = card_bounds.clone();
    let root_for_prepaint = root_bounds.clone();
    div()
        .id(("dynamic-card", idx))
        .relative()
        .w(px(width))
        .h(px(height))
        .flex()
        .flex_col()
        .p(px(CARD_PADDING))
        .rounded(px(RADIUS_XL))
        .overflow_hidden()
        .bg(s.surface_content)
        .border_1()
        .border_color(s.border_subtle)
        .cursor_pointer()
        .hover(move |style| {
            style
                .bg(s.surface_content_hover)
                .border_color(s.border_popover)
                .shadow(t.shadows.shadow_card_hover.clone())
        })
        // 卡片几何探针：动画起点用窗口坐标，点击时换算回页面坐标。
        .child(
            canvas(
                move |bounds, _window, _cx| {
                    if let Some(root) = root_for_prepaint.get() {
                        card_bounds_for_prepaint.set(Some(Bounds {
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
        .child(body)
        .on_click(cx.listener({
            let open = open_card.clone();
            move |this, _: &ClickEvent, window, cx| open(this, window, cx)
        }))
}

/// 占位卡没有宿主：只为复用泛型积木提供一份空实现。
///
/// 占位卡是纯视觉快照，不会触发任何动作，因此这里的方法保持空转。
struct GhostHost;

impl CardHost for GhostHost {
    fn card_is_playing(&self) -> bool {
        false
    }
    fn card_close_requested(&self) -> bool {
        false
    }
    fn card_scroll_settling(&self) -> bool {
        false
    }
    fn card_update_hover(&mut self, _idx: usize, _hovered: bool, _animate: bool) -> bool {
        false
    }
    fn card_open(
        &mut self,
        _bvid: String,
        _title: String,
        _entrance: Option<CoverSource>,
        _window: &mut Window,
        _cx: &mut Context<Self>,
    ) {
    }
    fn card_up_avatar(
        _id: SharedString,
        _mid: i64,
        _name: String,
        _face: String,
        _size_px: f32,
        child: AnyElement,
        _cx: &mut Context<Self>,
    ) -> AnyElement {
        child
    }
}

impl DynamicCardHost for GhostHost {}

/// 可复用的「临时卡片」。
///
/// 动态详情展开动画里，它作为「上一界面」的快照放在最上层：
/// 前 30% 与真实详情页同步缩放，自身 100% → 0 淡出，详情页 0 → 100 淡入；
/// 动画结束后调用方直接不挂它即可。
///
/// 结构与 feed 的卡片一致（材质 / 内边距 / 头部 / 正文 / 媒体 / 底栏），
/// 但不带 hover、点击和任何事件，避免在动画里抢交互。
pub fn card_ghost(
    t: &YkTheme,
    item: &DynamicItem,
    width: f32,
    height: f32,
    scale_factor: f32,
) -> AnyElement {
    let s = &t.surfaces;
    let inner = (width - CARD_PADDING * 2.0).max(1.0);
    let active = Rc::new(RefCell::new(ImageResidency::default()));

    let avatar = author_avatar(t, item, scale_factor, true, active.clone());
    let mut body = div()
        .flex()
        .flex_col()
        .w_full()
        .child(header_row(t, item, avatar));

    let title = card_text(&item.title);
    if !title.is_empty() {
        body = body.child(
            div()
                .mt(px(6.))
                .text_size(px(FONT_SIZE_BODY))
                .line_height(px(TITLE_LINE_HEIGHT))
                .font_weight(FontWeight::SEMIBOLD)
                .text_color(s.text_primary)
                .line_clamp(2)
                .child(title),
        );
    }

    let text = card_text(&item.text);
    if !text.is_empty() {
        body = body.child(
            div()
                .mt(px(6.))
                .text_size(px(FONT_SIZE_CONTROL))
                .line_height(px(BODY_LINE_HEIGHT))
                .text_color(s.text_secondary)
                .line_clamp(4)
                .child(text),
        );
    }

    match item.kind {
        DynamicKind::Video => {
            if let Some(video) = item.video.as_ref() {
                body = body.child(video_block(
                    t,
                    video,
                    inner,
                    scale_factor,
                    true,
                    active.clone(),
                    Rc::new(Cell::new(None)),
                    Rc::new(Cell::new(None)),
                ));
            }
        }
        DynamicKind::Images => {
            if !item.images.is_empty() {
                body = body.child(image_grid::<GhostHost>(
                    t,
                    &item.images,
                    inner,
                    scale_factor,
                    true,
                    active.clone(),
                    item.index,
                    None,
                ));
            }
        }
        DynamicKind::Forward => {
            if let Some(origin) = item.origin.as_ref() {
                body = body.child(quote_block(t, origin));
            }
        }
        DynamicKind::Link => {
            if let Some(link) = item.link.as_ref() {
                body = body.child(link_block(
                    t,
                    link,
                    inner,
                    scale_factor,
                    true,
                    active.clone(),
                ));
            }
        }
        DynamicKind::Text => {}
    }

    let stat = |icon: IconName, count: u64| {
        div()
            .flex()
            .items_center()
            .gap(px(4.))
            .child(Icon::new(icon).size(px(13.)).text_color(s.text_tertiary))
            .child(format::count(count))
    };
    body = body.child(
        div()
            .mt_auto()
            .pt(px(10.))
            .border_t_1()
            .border_color(s.border_subtle)
            .flex()
            .items_center()
            .justify_between()
            .w_full()
            .text_size(px(FONT_SIZE_CAPTION))
            .text_color(s.text_tertiary)
            .child(stat(IconName::Share2, item.stats.forward))
            .child(stat(IconName::MessageCircle, item.stats.comment))
            .child(stat(IconName::ThumbsUp, item.stats.like)),
    );

    div()
        .w(px(width))
        .h(px(height))
        .flex()
        .flex_col()
        .p(px(CARD_PADDING))
        .rounded(px(RADIUS_XL))
        .overflow_hidden()
        .bg(s.surface_content)
        .border_1()
        .border_color(s.border_subtle)
        .child(body)
        .into_any_element()
}
