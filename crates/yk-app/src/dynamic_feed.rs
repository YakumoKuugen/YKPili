//! 「正在关注」瀑布流：动态卡片 + 自适应多列布局。
//!
//! 设计要点：
//! - **瀑布流不是网格**：每张卡根据正文行数、图片数量 / 比例算出高度，
//!   再放进当前最矮的一列，列与列之间允许高度不同。
//! - 卡片分三类交互：视频卡直接展开播放器（沿用首页封面卡的入场动画）；
//!   纯文字 / 图文卡展开详情浮层；浮层外点击缩回原卡位置。
//! - 高度必须**先算再排**：GPUI 没有 masonry 布局，也不能在 prepaint 里反过来
//!   改兄弟元素位置，因此卡片内部所有可变高度都由 `card_height()` 统一推导，
//!   渲染时使用同一套行数 / 图片几何。

use gpui::assets::IconName;
use gpui::component::Icon;
use gpui::prelude::*;
use gpui::*;
use std::cell::{Cell, RefCell};
use std::rc::Rc;
use yk_core::dynamic::{DynamicImage, DynamicItem, DynamicKind};
use yk_core::format;
use yk_ui::status;
use yk_ui::theme::*;

use crate::dynamic_view::{CardOrigin, DynamicDetailView};
use crate::player::CoverSource;
use crate::{AppRoot, ImageResidency};

/// 瀑布流卡片的目标最小宽度；实际列宽 = 可用宽按这个值均分。
const MIN_COLUMN_WIDTH: f32 = 320.0;
/// 列间距 / 卡片纵向间距。
const CARD_GAP: f32 = SPACE_4;
/// 卡内边距。
const CARD_PADDING: f32 = 14.0;
const HEADER_HEIGHT: f32 = 40.0;
const FOOTER_HEIGHT: f32 = 38.0;
const TITLE_LINE_HEIGHT: f32 = 22.0;
const BODY_LINE_HEIGHT: f32 = 20.0;
const IMAGE_GAP: f32 = 6.0;

/// 一次「正在关注」页面的全部输入。
pub struct FeedState<'a> {
    pub items: &'a [DynamicItem],
    pub loading: bool,
    pub error: Option<&'a str>,
    /// 是否已经成功拉过一页；用来区分「空列表」与「还没开始」。
    pub loaded: bool,
    pub has_more: bool,
    pub loading_more: bool,
    pub signed_in: bool,
}

/// 渲染关注动态页面（含标题、四态与瀑布流）。
pub(crate) fn page(
    t: &YkTheme,
    state: FeedState<'_>,
    visible_y: std::ops::Range<f32>,
    active_images: Rc<RefCell<ImageResidency>>,
    root_bounds: Rc<Cell<Option<Bounds<Pixels>>>>,
    window: &Window,
    cx: &mut Context<AppRoot>,
) -> AnyElement {
    let s = &t.surfaces;
    let scale_factor = window.scale_factor();
    let available = (window.viewport_size().width.as_f32() - SPACE_6 * 2.0).max(MIN_COLUMN_WIDTH);

    let refresh = div()
        .id("following-refresh")
        .h(px(30.))
        .px(px(SPACE_3))
        .flex()
        .items_center()
        .gap(px(SPACE_1))
        .rounded_full()
        .bg(s.fill_1)
        .text_size(px(FONT_SIZE_CAPTION))
        .text_color(s.text_secondary)
        .cursor_pointer()
        .hover(|style| style.bg(s.fill_2).text_color(s.text_primary))
        .child(Icon::new(IconName::RefreshCw).size(px(14.)))
        .child("刷新")
        .on_click(cx.listener(|this, _: &ClickEvent, window, cx| {
            this.reload_following(cx);
            window.refresh();
        }));

    let header = div()
        .flex()
        .items_end()
        .justify_between()
        .w_full()
        .child(
            div()
                .flex()
                .items_baseline()
                .gap(px(SPACE_3))
                .child(
                    div()
                        .text_size(px(FONT_SIZE_HEADING))
                        .font_weight(FontWeight::SEMIBOLD)
                        .text_color(s.text_primary)
                        .child("正在关注"),
                )
                .child(
                    div()
                        .text_size(px(FONT_SIZE_CAPTION))
                        .text_color(s.text_tertiary)
                        .child(if !state.signed_in {
                            "登录后同步关注动态".to_string()
                        } else if state.loaded {
                            format!("{} 条动态 · 瀑布流", state.items.len())
                        } else {
                            "来自你关注的 UP 主".to_string()
                        }),
                ),
        )
        .children(state.signed_in.then_some(refresh));

    let body: AnyElement = if !state.signed_in {
        login_prompt(t, cx).into_any_element()
    } else if state.loading && !state.loaded {
        skeleton_columns(t, available).into_any_element()
    } else if let Some(error) = state.error {
        error_state(t, error, cx).into_any_element()
    } else if state.items.is_empty() {
        status::empty(
            t,
            "关注流还是空的",
            "关注一些 UP 主后，TA 的视频、图文和一句话动态会出现在这里",
        )
        .into_any_element()
    } else {
        waterfall(
            t,
            state.items,
            available,
            scale_factor,
            visible_y,
            active_images,
            root_bounds,
            cx,
        )
    };

    div()
        .flex()
        .flex_col()
        .gap(px(SPACE_4))
        .w_full()
        .child(header)
        .child(body)
        .when(state.has_more || state.loading_more, |el| {
            el.child(
                div().flex().justify_center().w_full().child(
                    div()
                        .id("following-load-more")
                        .h(px(34.))
                        .px(px(SPACE_5))
                        .flex()
                        .items_center()
                        .justify_center()
                        .rounded_full()
                        .bg(s.fill_1)
                        .text_size(px(FONT_SIZE_CONTROL))
                        .text_color(if state.loading_more {
                            s.text_tertiary
                        } else {
                            s.text_secondary
                        })
                        .cursor_pointer()
                        .when(!state.loading_more, |el| {
                            el.hover(|style| style.bg(s.fill_2).text_color(s.text_primary))
                        })
                        .child(if state.loading_more {
                            "正在加载…"
                        } else {
                            "加载更多"
                        })
                        .on_click(cx.listener(|this, _: &ClickEvent, window, cx| {
                            if !this.following_loading_more {
                                this.load_following_page(true, false, cx);
                            }
                            window.refresh();
                        })),
                ),
            )
        })
        .into_any_element()
}

fn login_prompt(t: &YkTheme, cx: &mut Context<AppRoot>) -> impl IntoElement {
    let s = &t.surfaces;
    let p = &t.palette;
    div()
        .flex()
        .flex_col()
        .items_center()
        .justify_center()
        .gap(px(SPACE_2))
        .h(px(360.))
        .w_full()
        .child(
            div()
                .flex()
                .items_center()
                .justify_center()
                .w(px(48.))
                .h(px(48.))
                .rounded_full()
                .bg(p.theme_primary_20)
                .child(
                    Icon::new(IconName::Users)
                        .size(px(24.))
                        .text_color(p.theme_primary),
                ),
        )
        .child(
            div()
                .text_size(px(FONT_SIZE_BODY))
                .text_color(s.text_secondary)
                .child("登录后查看关注动态"),
        )
        .child(
            div()
                .text_size(px(FONT_SIZE_CAPTION))
                .text_color(s.text_tertiary)
                .child("你关注的 UP 主发布的视频、图文和一句话都会出现在这里"),
        )
        .child(
            div()
                .id("following-login")
                .mt(px(SPACE_2))
                .h(px(32.))
                .px(px(SPACE_5))
                .flex()
                .items_center()
                .rounded_full()
                .bg(p.theme_primary)
                .text_size(px(FONT_SIZE_CAPTION))
                .font_weight(FontWeight::SEMIBOLD)
                .text_color(rgb(0xFFFFFF))
                .cursor_pointer()
                .child("去登录")
                .on_click(cx.listener(|this, _: &ClickEvent, _window, cx| {
                    this.ensure_login(cx);
                })),
        )
}

fn error_state(t: &YkTheme, error: &str, cx: &mut Context<AppRoot>) -> impl IntoElement {
    let s = &t.surfaces;
    div()
        .flex()
        .flex_col()
        .items_center()
        .justify_center()
        .gap(px(SPACE_2))
        .h(px(320.))
        .w_full()
        .child(
            div()
                .text_size(px(FONT_SIZE_BODY))
                .text_color(s.text_secondary)
                .child("关注动态加载失败"),
        )
        .child(
            div()
                .text_size(px(FONT_SIZE_CAPTION))
                .text_color(s.text_tertiary)
                .child(error.to_string()),
        )
        .child(
            div()
                .id("following-retry")
                .mt(px(SPACE_2))
                .h(px(30.))
                .px(px(SPACE_4))
                .flex()
                .items_center()
                .rounded_full()
                .bg(t.palette.theme_primary)
                .text_size(px(FONT_SIZE_CAPTION))
                .font_weight(FontWeight::SEMIBOLD)
                .text_color(rgb(0xFFFFFF))
                .cursor_pointer()
                .child("重试")
                .on_click(cx.listener(|this, _: &ClickEvent, window, cx| {
                    this.reload_following(cx);
                    window.refresh();
                })),
        )
}

fn skeleton_columns(t: &YkTheme, available: f32) -> impl IntoElement {
    let cols = column_count(available);
    let col_w = (available - CARD_GAP * (cols.saturating_sub(1)) as f32) / cols as f32;
    let heights = [268.0, 196.0, 320.0, 232.0, 288.0, 180.0];
    let brush = t.surfaces.skeleton_brush;
    div()
        .flex()
        .flex_row()
        .items_start()
        .gap(px(CARD_GAP))
        .w_full()
        .with_animation(
            "following-skeleton",
            Animation::new(std::time::Duration::from_millis(
                yk_ui::status::SKELETON_PULSE_MS,
            ))
            .repeat(),
            move |el, delta| {
                let opacity = 0.35 + 0.30 * ((delta * std::f32::consts::PI).sin().abs());
                el.opacity(opacity).children((0..cols).map(|c| {
                    div()
                        .flex()
                        .flex_col()
                        .gap(px(CARD_GAP))
                        .w(px(col_w))
                        .children((0..3).map(|r| {
                            let h = heights[(c * 3 + r) % heights.len()];
                            div().w_full().h(px(h)).rounded(px(RADIUS_XL)).bg(brush)
                        }))
                }))
            },
        )
}

// ================= 瀑布流布局 =================

fn column_count(available: f32) -> usize {
    (((available + CARD_GAP) / (MIN_COLUMN_WIDTH + CARD_GAP)).floor() as usize).clamp(1, 4)
}

/// 把卡片按「当前最矮列」分配；返回每列的条目下标和列高。
fn distribute(items: &[DynamicItem], col_w: f32, cols: usize) -> (Vec<Vec<usize>>, Vec<f32>) {
    let mut columns: Vec<Vec<usize>> = vec![Vec::new(); cols];
    let mut heights = vec![0.0_f32; cols];
    for (i, item) in items.iter().enumerate() {
        let target = heights
            .iter()
            .enumerate()
            .min_by(|a, b| a.1.partial_cmp(b.1).unwrap_or(std::cmp::Ordering::Equal))
            .map(|(idx, _)| idx)
            .unwrap_or(0);
        columns[target].push(i);
        heights[target] += card_height(item, col_w) + CARD_GAP;
    }
    (columns, heights)
}

fn waterfall(
    t: &YkTheme,
    items: &[DynamicItem],
    available: f32,
    scale_factor: f32,
    visible_y: std::ops::Range<f32>,
    active_images: Rc<RefCell<ImageResidency>>,
    root_bounds: Rc<Cell<Option<Bounds<Pixels>>>>,
    cx: &mut Context<AppRoot>,
) -> AnyElement {
    let cols = column_count(available);
    let col_w = (available - CARD_GAP * (cols.saturating_sub(1)) as f32) / cols as f32;
    let (columns, _) = distribute(items, col_w, cols);

    // 瀑布流没有严格行，按列计算：每列加载「可视卡片 + 上下各一张」。
    // 而不是用固定像素 margin，否则短卡多的时候会误留十几张。
    let mut load_flags = vec![false; items.len()];
    for column in &columns {
        let mut placements = Vec::with_capacity(column.len());
        let mut top = 0.0_f32;
        for &index in column {
            let height = card_height(&items[index], col_w);
            placements.push((index, top, top + height));
            top += height + CARD_GAP;
        }

        let first = placements
            .iter()
            .position(|(_, top, bottom)| *bottom >= visible_y.start && *top <= visible_y.end);
        let last = placements
            .iter()
            .rposition(|(_, top, bottom)| *bottom >= visible_y.start && *top <= visible_y.end);

        if let (Some(first), Some(last)) = (first, last) {
            let from = first.saturating_sub(1);
            let to = (last + 2).min(placements.len());
            for (index, _, _) in &placements[from..to] {
                load_flags[*index] = true;
            }
        } else if let Some((index, _, _)) = placements.iter().min_by(|a, b| {
            let da = if a.2 < visible_y.start {
                visible_y.start - a.2
            } else {
                a.1 - visible_y.end
            };
            let db = if b.2 < visible_y.start {
                visible_y.start - b.2
            } else {
                b.1 - visible_y.end
            };
            da.partial_cmp(&db).unwrap_or(std::cmp::Ordering::Equal)
        }) {
            // 视口刚好落在列的空隙里：只留离视口最近的一张，避免整列空白。
            load_flags[*index] = true;
        }
    }

    div()
        .flex()
        .flex_row()
        .items_start()
        .gap(px(CARD_GAP))
        .w_full()
        .children(columns.into_iter().map(|indices| {
            div()
                .flex()
                .flex_col()
                .gap(px(CARD_GAP))
                .w(px(col_w))
                .children(indices.into_iter().map(|i| {
                    let item = &items[i];
                    card(
                        t,
                        item,
                        col_w,
                        card_height(item, col_w),
                        scale_factor,
                        load_flags[i],
                        active_images.clone(),
                        root_bounds.clone(),
                        cx,
                    )
                    .into_any_element()
                }))
        }))
        .into_any_element()
}

// ================= 高度估算 =================
//
// 估算与渲染必须使用同一组行数上限，否则卡片会出现「内容比高度多一截」或
// 底部留白。所有文本都通过 `text_lines()` 算行数，图片通过 `image_metrics()`
// 算网格高度。

fn text_lines(text: &str, width: f32, font_size: f32, max_lines: usize) -> usize {
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

fn image_metrics(inner: f32, images: &[DynamicImage]) -> (usize, f32, f32) {
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

/// 卡面文本：把换行压成空格。
///
/// GPUI 的 `line_clamp` 对「显式换行分段」的文本只按段生效，一条 10 行的动态
/// 在卡片上会整段铺开，既超出估算高度也破坏瀑布流。卡片只需要摘要，
/// 因此这里统一压成单段，由宽度自然换行后再 clamp。
fn card_text(text: &str) -> String {
    text.split_whitespace().collect::<Vec<_>>().join(" ")
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

// ================= 卡片 =================

fn card(
    t: &YkTheme,
    item: &DynamicItem,
    width: f32,
    height: f32,
    scale_factor: f32,
    load_images: bool,
    active_images: Rc<RefCell<ImageResidency>>,
    root_bounds: Rc<Cell<Option<Bounds<Pixels>>>>,
    cx: &mut Context<AppRoot>,
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

    let mut body = div().flex().flex_col().w(px(inner)).child(header(
        t,
        item,
        scale_factor,
        load_images,
        active_images.clone(),
    ));

    let card_title = card_text(&item.title);
    let card_body_text = card_text(&item.text);

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
                body = body.child(image_grid(
                    t,
                    &item.images,
                    inner,
                    scale_factor,
                    load_images,
                    active_images.clone(),
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
        .on_click(cx.listener(move |this, _: &ClickEvent, window, cx| {
            if this.playing.is_some()
                || this.dynamic_detail.is_some()
                || this.close_player_requested
            {
                return;
            }
            let origin = CardOrigin {
                bounds: card_bounds_for_click.get().unwrap_or_else(|| Bounds {
                    origin: point(px(0.), px(0.)),
                    size: size(px(width), px(height)),
                }),
                cover_bounds: cover_bounds_for_click.get(),
                root_size: root_for_click
                    .get()
                    .map(|b| b.size)
                    .unwrap_or_else(|| size(px(1280.), px(820.))),
            };
            if item_for_click.kind == DynamicKind::Video {
                this.open_dynamic_video(&item_for_click, Some(origin), cx);
            } else {
                this.open_dynamic_detail(&item_for_click, Some(origin), cx);
            }
            window.refresh();
        }))
}

fn header(
    t: &YkTheme,
    item: &DynamicItem,
    scale_factor: f32,
    load_image: bool,
    active_images: Rc<RefCell<ImageResidency>>,
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
        .child(
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
                }),
        )
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

fn video_block(
    t: &YkTheme,
    video: &yk_core::dynamic::DynamicVideo,
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
    let mut thumb = div()
        .relative()
        .w(px(thumb_w))
        .h(px(thumb_h))
        .flex_shrink_0()
        .rounded(px(RADIUS_LG))
        .overflow_hidden()
        .bg(crate::placeholder_gradient("#333333", s.fill_3))
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
        );
    if load_image && !cover_url.is_empty() {
        active_images.borrow_mut().mark(&cover_url);
        thumb = thumb.child(
            img(ImageSource::from(cover_url.clone()))
                .absolute()
                .inset_0()
                .w_full()
                .h_full()
                .object_fit(ObjectFit::Cover),
        );
    }
    thumb = thumb
        .child(
            div()
                .absolute()
                .right(px(5.))
                .bottom(px(5.))
                .px(px(4.))
                .rounded(px(RADIUS_SM))
                .bg(rgba(0x00000099))
                .text_size(px(FONT_SIZE_MICRO))
                .text_color(rgb(0xFFFFFF))
                .child(video.duration_text.clone()),
        )
        .child(
            div()
                .absolute()
                .left(px(5.))
                .bottom(px(5.))
                .flex()
                .items_center()
                .gap(px(2.))
                .text_size(px(FONT_SIZE_MICRO))
                .text_color(rgb(0xFFFFFF))
                .child(
                    Icon::new(IconName::Play)
                        .size(px(11.))
                        .text_color(rgb(0xFFFFFF)),
                )
                .child(format::count(video.play_count)),
        );

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

fn image_grid(
    t: &YkTheme,
    images: &[DynamicImage],
    inner: f32,
    scale_factor: f32,
    load_image: bool,
    active_images: Rc<RefCell<ImageResidency>>,
) -> impl IntoElement {
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
        let mut tile = div()
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
        grid = grid.child(tile);
    }
    grid
}

fn quote_block(t: &YkTheme, origin: &DynamicItem) -> impl IntoElement {
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
}

fn link_block(
    t: &YkTheme,
    link: &yk_core::dynamic::DynamicLink,
    inner: f32,
    scale_factor: f32,
    load_image: bool,
    active_images: Rc<RefCell<ImageResidency>>,
) -> impl IntoElement {
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
}

fn card_action(
    t: &YkTheme,
    idx: usize,
    kind: &'static str,
    icon: IconName,
    count: u64,
    cx: &mut Context<AppRoot>,
) -> impl IntoElement {
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
            eprintln!("[dynamic] 操作 {kind} · 动态 #{idx}（后端动作未接）");
        }))
}

// ================= 验收入口 =================

/// 验收用：一条纯文字动态。
///
/// 关注流里不一定随时有纯文字动态，而「只有正文 → 左正文右评论」是本次需求
/// 的独立布局分支。`YK_DYNAMIC_TEXT_ONLY=1` 时插到列表最前面，方便截图核对。
#[cfg(debug_assertions)]
pub(crate) fn debug_text_item(index: usize) -> DynamicItem {
    DynamicItem {
        index,
        id: "debug-word-1".to_string(),
        kind: DynamicKind::Text,
        author: yk_core::dynamic::DynamicAuthor {
            mid: 42,
            name: "一句话的 UP 主".to_string(),
            face: String::new(),
        },
        pub_ts: yk_core::api::client::now_wts(),
        pub_time: "刚刚".to_string(),
        action: String::new(),
        title: String::new(),
        text: "今天路过一条很窄的巷子，风从两边灌进来，像有人把夏天折成了一封信。\n\n走到巷尾才发现，尽头是一棵很大的梧桐树。树叶把光筛成碎金，落在一只睡着的橘猫身上。\n\n它眯着眼看我，像在说：别急，慢慢来。".to_string(),
        images: Vec::new(),
        video: None,
        link: None,
        stats: yk_core::dynamic::DynamicStats {
            forward: 3,
            comment: 12,
            like: 128,
        },
        comment_target: Some(yk_core::dynamic::CommentTarget {
            oid: "1263000000000000000".to_string(),
            kind: 17,
        }),
        origin: None,
        placeholder_color: "#4DB6AC".to_string(),
    }
}

// ================= AppRoot 入口 =================

impl AppRoot {
    /// 打开视频动态：和首页封面卡走同一条播放器入场动画。
    pub(crate) fn open_dynamic_video(
        &mut self,
        item: &DynamicItem,
        origin: Option<CardOrigin>,
        cx: &mut Context<Self>,
    ) {
        let Some(video) = item.video.as_ref() else {
            return;
        };
        if video.bvid.trim().is_empty() {
            eprintln!("[dynamic] 视频动态缺少 bvid：{}", item.id);
            return;
        }
        eprintln!("[dynamic] 展开视频动态：{}（{}）", video.title, video.bvid);
        self.ensure_player(cx);
        if let Some(player) = self.playing.clone() {
            if let Some(origin) = origin {
                let bounds = origin.cover_bounds.unwrap_or(origin.bounds);
                let source = CoverSource {
                    bounds,
                    root_size: origin.root_size,
                    bvid: video.bvid.clone(),
                    title: video.title.clone(),
                    image_url: video.cover_url(640.0, 1.0),
                    placeholder_color: item.placeholder_color.clone(),
                };
                let _ = player.update(cx, |view, cx| view.set_entrance(source, cx));
            }
            let _ = player.update(cx, |view, cx| view.open_bvid(video.bvid.clone(), cx));
        }
        cx.notify();
    }

    /// 打开图文 / 文字动态详情浮层。
    pub(crate) fn open_dynamic_detail(
        &mut self,
        item: &DynamicItem,
        origin: Option<CardOrigin>,
        cx: &mut Context<Self>,
    ) {
        if self.dynamic_detail.is_some() {
            return;
        }
        eprintln!(
            "[dynamic] 展开动态详情：{} / {} 张图",
            item.author_name(),
            item.images.len()
        );
        let theme = self.theme.clone();
        let item = item.clone();
        let cookie = self.cookie();
        let detail = cx.new(|cx| DynamicDetailView::new(theme, item, origin, cookie, cx));
        let app = cx.entity().downgrade();
        let _ = detail.update(cx, |view, _| {
            view.set_close_handler(move |cx, finished| {
                if !finished {
                    return;
                }
                let _ = app.update(cx, |root, cx| {
                    root.dynamic_detail_close_requested = true;
                    cx.notify();
                });
            });
        });
        self.dynamic_detail = Some(detail.clone());
        // 验收用：YK_DYNAMIC_AUTOCLOSE_MS=1000 在展开后自动触发一次关闭，
        // 用来核对 260ms 缩回原卡片与实体回收。
        if let Some(ms) = std::env::var("YK_DYNAMIC_AUTOCLOSE_MS")
            .ok()
            .and_then(|value| value.parse::<u64>().ok())
        {
            cx.spawn(async move |_this, cx| {
                cx.background_executor()
                    .timer(std::time::Duration::from_millis(ms))
                    .await;
                let _ = detail.update(cx, |view, cx| view.begin_close(cx));
            })
            .detach();
        }
        cx.notify();
    }
}
