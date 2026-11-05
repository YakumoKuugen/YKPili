//! 首页推荐网格：按视口宽切行，交给 `v_virtual_list` 只画可见行。
//!
//! 交互（打开播放器 / 点头像）仍走 [`CardHost`]，hover 状态由
//! [`HomeHost::home_state`] 提供；这里只负责虚拟列表的几何、骨架与
//! 图片预热的范围计算。

use std::cell::{Cell, RefCell};
use std::rc::Rc;

use gpui::component::{v_virtual_list, VirtualListScrollHandle};
use gpui::prelude::*;
use gpui::*;
use yk_components::status::SKELETON_PULSE_MS;
use yk_domain::models::CoverItem;
use yk_theme::theme::*;

use crate::card::{card, card_height, CardHost};
use crate::image::ImageResidency;

/// 一次首页渲染需要的只读快照。
pub struct HomeState<'a> {
    pub items: &'a [CoverItem],
    pub loading: bool,
    pub hovered: Option<usize>,
    pub leaving: Option<usize>,
    /// hover 动画代际：滚动活跃期内不推进，避免封面缩放重播。
    pub hover_generation: u64,
}

/// 首页网格需要的宿主能力（在封面卡片能力之上）。
pub trait HomeHost: CardHost + Render {
    fn home_state(&self) -> HomeState<'_>;
}

/// 列数 = 可用宽度能放下几个 208 + 16 的槽位；至少 1 列。
pub fn columns(viewport_width: f32) -> usize {
    let available = (viewport_width - SPACE_6 * 2.0).max(COVER_CELL_WIDTH);
    (((available + SPACE_4) / (COVER_CELL_WIDTH + SPACE_4)).floor() as usize).max(1)
}

/// 每张卡片的槽位宽：把可用宽度（扣掉左右各 24 内边距）减去行内间距后，
/// 均分给 [`columns`] 切出来的列。
///
/// 定宽 208 + 居中会在左右各留一条空带（`columns` 取整剩多少就空多少），
/// 窗口越宽空得越明显。均分之后每行恰好铺满内容区，窗口变宽时卡片跟着
/// 变大——上限由「再多一列就放不下 208」约束，最多 208 + 224/cols。
pub fn card_width(viewport_width: f32) -> f32 {
    let cols = columns(viewport_width);
    let available = (viewport_width - SPACE_6 * 2.0).max(COVER_CELL_WIDTH);
    let gaps = SPACE_4 * cols.saturating_sub(1) as f32;
    ((available - gaps) / cols as f32).max(COVER_CELL_WIDTH)
}

/// 首页填满一屏所需的最少卡片数：列数 ×（整行数 + 1 行）。
///
/// 推荐接口一页固定 30 条；窗口大到一页填不满时，内容高度不足 → 滚动条
/// 的 `max_offset` 为 0 → 「触底加载」永远不触发，表现就是没有滚动条、
/// 也拉不到更多封面。这里按视口几何算出「至少几行才超出一屏」，不够就补页。
///
/// 网格容器上内边距 16 + 下内边距 `DOCK_RESERVED`，滚动视口高 = 窗口高 − 顶栏；
/// 所以内容高 `rows × row_h + 112 > 视口高` 等价于 `rows × row_h > available_h`，
/// 即整行数 + 1 行就够了（刚好多出不到一行，保证一定滚得动）。
pub fn min_items(viewport_width: f32, viewport_height: f32) -> usize {
    let cols = columns(viewport_width);
    let row_h = card_height(card_width(viewport_width)) + SPACE_4;
    let available_h = (viewport_height - TOP_BAR_HEIGHT - SPACE_4 - DOCK_RESERVED).max(row_h);
    let full_rows = (available_h / row_h).floor() as usize;
    cols * (full_rows + 1)
}

/// 渲染首页网格（含首屏骨架与虚拟列表）。
pub fn grid<V: HomeHost + 'static>(
    t: &YkTheme,
    state: HomeState<'_>,
    root_bounds: Rc<Cell<Option<Bounds<Pixels>>>>,
    active_images: Rc<RefCell<ImageResidency>>,
    scroll: &VirtualListScrollHandle,
    window: &Window,
    cx: &mut Context<V>,
) -> AnyElement {
    let items = state.items;
    // 每行恰好铺满内容区：卡片宽度按窗口均分，高度按 16:9 封面 + 固定文字区推导。
    // 行高 = 卡片高 + 行间距；行内只画卡片，多出来的 16 就是行距。
    let card_w = card_width(window.viewport_size().width.as_f32());
    let card_h = card_height(card_w);
    let row_h = card_h + SPACE_4;
    let cols = columns(window.viewport_size().width.as_f32());

    // 首屏加载：用与真实卡片完全同几何的骨架卡，避免数据回来时网格跳动。
    // 只用一个 repeat 动画驱动整块网格的 opacity（1 → 0.45 → 1，1.5s），
    // 不在每张骨架卡上各排一条动画，也不引入任何额外颜色。
    if state.loading && items.is_empty() {
        let viewport_h = window.viewport_size().height.as_f32();
        let available_h = (viewport_h - TOP_BAR_HEIGHT - SPACE_4 - DOCK_RESERVED).max(row_h);
        let skeleton_rows = ((available_h / row_h).floor() as usize).max(2);
        let ease = ease_standard();
        return div()
            .flex()
            .flex_col()
            .gap(px(SPACE_4))
            .w_full()
            .px(px(SPACE_6))
            .pt(px(SPACE_4))
            .pb(px(DOCK_RESERVED))
            .children((0..skeleton_rows).map(|_| {
                div()
                    .flex()
                    .justify_center()
                    .gap(px(SPACE_4))
                    .w_full()
                    .h(px(card_h))
                    .children((0..cols).map(|_| skeleton_card(t, card_w, card_h)))
            }))
            .with_animation(
                "home-card-skeleton-pulse",
                Animation::new(std::time::Duration::from_millis(SKELETON_PULSE_MS)).repeat(),
                move |el, delta| {
                    // 三角波：0 → 0.5 → 1 对应 1 → 0.45 → 1。
                    let phase = if delta < 0.5 {
                        delta * 2.0
                    } else {
                        (1.0 - delta) * 2.0
                    };
                    el.opacity(1.0 - 0.55 * ease(phase))
                },
            )
            .into_any_element();
    }

    let available = (window.viewport_size().width.as_f32() - SPACE_6 * 2.0).max(COVER_CELL_WIDTH);
    let rows = items.len().div_ceil(cols);
    let sizes = Rc::new(vec![size(px(available), px(row_h)); rows]);

    // 首页的真实可视范围从滚动偏移算，而不是用虚拟列表的 overdraw 范围：
    // 后者会多给 2 行，拿它当加载范围会让最后一行「可见但没图」。
    let viewport_h = (window.viewport_size().height.as_f32() - TOP_BAR_HEIGHT).max(row_h);
    let offset_y = scroll.base_handle().offset().y.as_f32();
    let first_visible = (((-offset_y) - SPACE_4).max(0.0) / row_h).floor() as usize;
    let last_visible = (((-offset_y) + viewport_h - SPACE_4).max(0.0) / row_h).ceil() as usize;
    let load_start = first_visible.min(rows);
    let load_end = (last_visible + 1).min(rows).max(load_start);

    // 即使虚拟列表这一帧没有调用 render_items，也要让缓存知道当前该保留哪些图。
    for row in load_start..load_end {
        let start = row * cols;
        let end = (start + cols).min(items.len());
        for item in &items[start..end] {
            active_images.borrow_mut().mark(&item.cover_thumb_url());
            if !item.up_face.is_empty() {
                active_images.borrow_mut().mark(&item.up_face);
            }
        }
    }

    let entity = cx.entity();
    let theme = t.clone();
    let active_images_for_grid = active_images.clone();
    let root_bounds_for_grid = root_bounds;
    v_virtual_list(
        entity,
        "home-grid",
        sizes,
        move |this, visible_rows, _window, cx| {
            visible_rows
                .map(|row| {
                    let cur = this.home_state();
                    let all = cur.items;
                    let start = row * cols;
                    let end = (start + cols).min(all.len());
                    // 卡片宽度已经按窗口均分，整行恰好铺满内容区；保留居中
                    // 只是兜住浮点误差，不会再在两侧留出空带。
                    let mut row_el = div()
                        .flex()
                        .justify_center()
                        .gap(px(SPACE_4))
                        .w_full()
                        .h(px(card_h));
                    let settling = this.card_scroll_settling();
                    let hovered_idx = if settling { None } else { cur.hovered };
                    let leaving_idx = if settling { None } else { cur.leaving };
                    let hover_gen = cur.hover_generation;
                    let load_cover = row >= load_start && row < load_end;
                    for item in &all[start..end] {
                        let hovered = hovered_idx == Some(item.index);
                        let leaving = leaving_idx == Some(item.index);
                        row_el = row_el.child(card(
                            &theme,
                            card_w,
                            item,
                            hovered,
                            leaving,
                            hover_gen,
                            root_bounds_for_grid.clone(),
                            load_cover,
                            active_images_for_grid.clone(),
                            cx,
                        ));
                    }
                    row_el
                })
                .collect::<Vec<_>>()
        },
    )
    .track_scroll(scroll)
    .px(px(SPACE_6))
    .pt(px(SPACE_4))
    .pb(px(DOCK_RESERVED))
    .into_any_element()
}

/// 首屏骨架卡：与真实卡片同几何（宽度按窗口均分，封面 16:9，两行标题 + 署名行）。
///
/// 颜色只用 `SkeletonBrush`，脉冲由父级网格统一驱动。
fn skeleton_card(t: &YkTheme, card_w: f32, card_h: f32) -> impl IntoElement {
    let brush = t.surfaces.skeleton_brush;
    let cover_w = (card_w - SPACE_2 * 2.0).max(1.0);
    let cover_h = cover_w * (COVER_ITEM_HEIGHT / COVER_ITEM_WIDTH);
    div()
        .w(px(card_w))
        .h(px(card_h))
        .flex()
        .flex_col()
        .p(px(SPACE_2))
        .child(
            div()
                .w(px(cover_w))
                .h(px(cover_h))
                .rounded(px(MEDIA_RADIUS))
                .bg(brush),
        )
        .child(
            div()
                .mt(px(SPACE_1))
                .h(px(LINE_HEIGHT_CONTROL * 2.))
                .flex()
                .flex_col()
                .gap(px(6.))
                .child(
                    div()
                        .h(px(11.))
                        .w(relative(0.92))
                        .rounded(px(RADIUS_SM))
                        .bg(brush),
                )
                .child(
                    div()
                        .h(px(11.))
                        .w(relative(0.64))
                        .rounded(px(RADIUS_SM))
                        .bg(brush),
                ),
        )
        .child(
            div()
                .mt(px(2.))
                .h(px(18.))
                .flex()
                .items_center()
                .gap(px(6.))
                .child(div().w(px(18.)).h(px(18.)).rounded_full().bg(brush))
                .child(div().h(px(11.)).flex_1().rounded(px(RADIUS_SM)).bg(brush))
                .child(div().h(px(11.)).w(px(32.)).rounded(px(RADIUS_SM)).bg(brush)),
        )
}

#[cfg(test)]
mod tests {
    use super::{card_width, columns, min_items};
    use yk_theme::theme::{COVER_CELL_WIDTH, SPACE_4, SPACE_6};

    #[test]
    fn columns_follow_window_width() {
        // 1280 可用宽 1232 → 5 列；2560 可用宽 2512 → 11 列。
        assert_eq!(columns(1280.0), 5);
        assert_eq!(columns(2560.0), 11);
        // 极窄窗口至少保留 1 列。
        assert_eq!(columns(200.0), 1);
    }

    /// 每行必须正好铺满内容区：两侧不能再留空带（本轮反馈的核心）。
    #[test]
    fn card_width_fills_the_row() {
        for viewport in [1024.0, 1280.0, 1440.0, 1512.0, 1920.0, 2560.0] {
            let cols = columns(viewport);
            let width = card_width(viewport);
            let available = (viewport - SPACE_6 * 2.0).max(COVER_CELL_WIDTH);
            let used = width * cols as f32 + SPACE_4 * (cols - 1) as f32;
            assert!(
                (used - available).abs() < 0.01,
                "{viewport} 宽下两侧还剩 {} 的空带",
                available - used
            );
            assert!(
                width >= COVER_CELL_WIDTH,
                "{viewport} 宽下卡片被压到 {width}，小于 208"
            );
        }
    }

    #[test]
    fn normal_window_fits_in_one_page() {
        // 一页 30 条：普通窗口够用，不该无谓地多请求。
        assert!(min_items(1280.0, 820.0) <= 30);
        assert!(min_items(1440.0, 900.0) <= 30);
    }

    #[test]
    fn large_window_needs_more_than_one_page() {
        // 大窗口下一页 30 条填不满一屏，必须继续补页，否则 max_offset 恒为 0。
        assert!(min_items(2560.0, 1440.0) > 30);
        assert!(min_items(1920.0, 1200.0) > 30);
        // 补页目标不能超过 Feed 的容量上限。
        assert!(min_items(2560.0, 1440.0) <= yk_infra::feed::MAX_ITEMS);
    }

    #[test]
    fn min_items_grows_with_viewport() {
        let base = min_items(1920.0, 1080.0);
        assert!(min_items(1920.0, 1440.0) > base, "更高 → 需要更多行");
        assert!(
            min_items(2560.0, 1080.0) > base,
            "更宽 → 每行更多，同样高度需要更多条"
        );
    }
}
