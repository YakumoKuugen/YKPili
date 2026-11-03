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
pub use yk_library::following::FeedState;
use yk_infra::dynamic::{DynamicImage, DynamicItem, DynamicKind};
use yk_domain::format;
use yk_components::status;
use yk_library::dynamic_card::*;
use yk_theme::theme::*;

use crate::dynamic_view::{CardOrigin, DynamicDetailView};
use crate::player::CoverSource;
use crate::{AppRoot, ImageResidency};

/// 瀑布流卡片的目标最小宽度；实际列宽 = 可用宽按这个值均分。
const MIN_COLUMN_WIDTH: f32 = 320.0;
/// 列间距 / 卡片纵向间距。
const CARD_GAP: f32 = SPACE_4;
/// 卡内边距。

/// 一次「正在关注」页面的全部输入。


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
                yk_components::status::SKELETON_PULSE_MS,
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

    // 「打开详情 / 视频」的唯一实现：整卡点击与文本区抬起兜底共用。
    let open_detail = {
        let item = item_for_click.clone();
        let card_bounds = card_bounds_for_click.clone();
        let cover_bounds = cover_bounds_for_click.clone();
        let root = root_for_click.clone();
        move |this: &mut AppRoot, window: &mut Window, cx: &mut Context<AppRoot>| {
            if this.playing.is_some()
                || this.dynamic_detail.is_some()
                || this.close_player_requested
            {
                return;
            }
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
            if item.kind == DynamicKind::Video {
                this.open_dynamic_video(&item, Some(origin), cx);
            } else {
                this.open_dynamic_detail(&item, Some(origin), cx);
            }
            window.refresh();
        }
    };

    let mut body = div().flex().flex_col().w(px(inner)).child(header(
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
                body = body.child(image_grid::<AppRoot>(
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
            let open = open_detail.clone();
            move |this, _: &ClickEvent, window, cx| open(this, window, cx)
        }))
}

/// 可复用的「临时卡片」。
///
/// 动态详情展开动画里，它作为「上一界面」的快照放在最上层：
/// 前 30% 与真实详情页同步缩放，自身 100% → 0 淡出，详情页 0 → 100 淡入；
/// 动画结束后调用方直接不挂它即可。
///
/// 结构与 feed 的卡片一致（材质 / 内边距 / 头部 / 正文 / 媒体 / 底栏），
/// 但不带 hover、点击和任何事件，避免在动画里抢交互。
pub(crate) fn card_ghost(
    t: &YkTheme,
    item: &DynamicItem,
    width: f32,
    height: f32,
    scale_factor: f32,
) -> AnyElement {
    let s = &t.surfaces;
    let inner = (width - CARD_PADDING * 2.0).max(1.0);
    let active = Rc::new(RefCell::new(ImageResidency::default()));

    let mut body = div()
        .flex()
        .flex_col()
        .w_full()
        .child(header::<AppRoot>(t, item, scale_factor, true, active.clone(), None));

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
                body = body.child(image_grid::<AppRoot>(
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
    link: &yk_infra::dynamic::DynamicLink,
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
        author: yk_infra::dynamic::DynamicAuthor {
            mid: 42,
            name: "一句话的 UP 主".to_string(),
            face: String::new(),
        },
        pub_ts: yk_infra::api::client::now_wts(),
        pub_time: "刚刚".to_string(),
        action: String::new(),
        title: String::new(),
        text: "今天路过一条很窄的巷子，风从两边灌进来，像有人把夏天折成了一封信。\n\n走到巷尾才发现，尽头是一棵很大的梧桐树。树叶把光筛成碎金，落在一只睡着的橘猫身上。\n\n它眯着眼看我，像在说：别急，慢慢来。".to_string(),
        images: Vec::new(),
        video: None,
        link: None,
        stats: yk_infra::dynamic::DynamicStats {
            forward: 3,
            comment: 12,
            like: 128,
        },
        comment_target: Some(yk_infra::dynamic::CommentTarget {
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
        // 点详情页大图 → AppRoot 的查看器（缩放 / 拖动 / 左右切换）。
        let app = cx.entity().downgrade();
        let _ = detail.update(cx, |view, _| {
            view.set_image_open_handler(move |cx, window, urls, index| {
                let _ = app.update(cx, |root, cx| {
                    root.open_image_viewer(urls, index, window, cx);
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
