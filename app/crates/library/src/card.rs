//! 视频封面卡片（首页 / 收藏 / 历史 / 搜索 / 动态共用）。
//!
//! 卡片本身只负责「画」和把点击 / hover 转成宿主动作；播放器、路由、
//! UP 主浮窗由实现 [`CardHost`] 的组装点处理，所以 library 不反向依赖 app。

use std::cell::{Cell, RefCell};
use std::rc::Rc;

use gpui::assets::IconName;
use gpui::component::Icon;
use gpui::prelude::*;
use gpui::{InteractiveElement, StatefulInteractiveElement, *};
use yk_domain::models::CoverItem;
use yk_player::CoverSource;
use yk_theme::theme::*;

use crate::cover::{cover_art, cover_visual, CoverVisual};
use crate::image::ImageResidency;

/// 卡片需要宿主提供的能力：播放态、hover 状态机、打开播放器、UP 主头像。
pub trait CardHost: Sized + 'static {
    fn card_is_playing(&self) -> bool;
    fn card_close_requested(&self) -> bool;
    fn card_scroll_settling(&self) -> bool;
    /// 返回是否发生了状态变化（需要 `cx.notify()`）。
    fn card_update_hover(&mut self, idx: usize, hovered: bool, animate: bool) -> bool;
    /// 整卡点击：打开播放器并带上封面入场几何。
    fn card_open(
        &mut self,
        bvid: String,
        title: String,
        entrance: Option<CoverSource>,
        window: &mut Window,
        cx: &mut Context<Self>,
    );
    /// UP 主头像（跳转浮窗的锚点探针也在这里）。
    fn card_up_avatar(
        id: SharedString,
        mid: i64,
        name: String,
        face: String,
        size_px: f32,
        child: AnyElement,
        cx: &mut Context<Self>,
    ) -> AnyElement;
}

/// 署名行固定高度（头像 / 文字行高）。卡片高度推导也用它，改一处即可。
const CARD_BYLINE_HEIGHT: f32 = 18.0;

/// 卡片高度：封面按 16:9 跟着卡片宽度长高，封面以下的文字区固定。
///
/// = 上下各 8 内边距 + 封面 + 标题（上边距 4 + 2×18 行高）
/// + 署名行（上边距 2 + 18）。宽度变了高度不跟，标题 / 署名会被裁掉。
pub fn card_height(card_w: f32) -> f32 {
    let cover_w = (card_w - SPACE_2 * 2.0).max(1.0);
    cover_w * (COVER_ITEM_HEIGHT / COVER_ITEM_WIDTH)
        + SPACE_2 * 2.0
        + SPACE_1
        + LINE_HEIGHT_CONTROL * 2.0
        + 2.0
        + CARD_BYLINE_HEIGHT
}

/// 封面卡片：默认 208 × 184（`COVER_CELL_WIDTH` / `COVER_CELL_HEIGHT`）。
///
/// 实际宽度由首页网格按窗口宽度分配（见 `yk_library::home::card_width`），
/// 封面按 16:9 跟着宽度长高，封面以下的文字区高度固定。
///
/// 结构对应设计文档第 04 节「封面卡片解剖」：
///
/// ```text
/// 208×184
///   ├─ 8 padding
///   ├─ 封面 192×108（16:9，圆角 12）
///   │    ├─ 渐变卡面 + 大标题（真实封面加载前/失败时的兜底）
///   │    ├─ 真实封面图
///   │    ├─ 底部压暗渐变（保证数据层可读）
///   │    ├─ 左下 播放量 + 弹幕数 · 右下 时长
///   │    ├─ 悬停操作（稍后再看 / 下载 / 更多）
///   │    └─ 观看进度条（仅看过的视频）
///   ├─ 标题 2 行（13 / 18）
///   └─ 署名行：UP 头像 18 + 昵称 + 相对时间
/// ```
///
/// 渐变卡面是**自绘**的：主题令牌只有单色，设计稿的 135° 两色渐变
/// 由占位色派生暗/亮两档后用 `linear_gradient` 画出来。
pub fn card<V: CardHost>(
    t: &YkTheme,
    card_w: f32,
    item: &CoverItem,
    hovered: bool,
    leaving: bool,
    hover_generation: u64,
    material: SurfaceMaterial,
    root_bounds: Rc<Cell<Option<Bounds<Pixels>>>>,
    load_cover: bool,
    active_images: Rc<RefCell<ImageResidency>>,
    cx: &mut Context<V>,
) -> impl IntoElement {
    let s = &t.surfaces;
    let p = &t.palette;
    let cover_w = (card_w - SPACE_2 * 2.0).max(1.0);
    let card_h = card_height(card_w);
    let title = item.title.clone();
    let bvid = item.bvid.clone();
    let idx = item.index;
    // 选中（hover）时的表面：实色版是 fill_2 底 + border_strong 边 + 玻璃投影；
    // 液态玻璃版换成渐变玻璃面 + 白色倒角，底子仍取 fill_2 免得比原先亮太多。
    let glass = material.is_glass();
    let hover_panel = GlassSurface::new(t, s.fill_2, 0.04, 0.40, 0.52);
    let hover_shadow = if glass {
        hover_panel.shadows.clone()
    } else {
        t.shadows.shadow_glass_card.clone()
    };
    let hover_border = if glass {
        hover_panel.border
    } else {
        s.border_strong
    };
    let hover_bg = if glass {
        hover_panel.fill
    } else {
        s.fill_2.into()
    };
    // 当前帧封面几何：prepaint 写入，点击时读取；不 notify。
    let cover_source: Rc<RefCell<Option<CoverSource>>> = Rc::new(RefCell::new(None));
    let cover_image_url = item.cover_thumb_url();
    let cover_placeholder = item.placeholder_color.clone();

    // 封面只有「卡面」参与 1.04 缓推：渐变/真实封面在 art 层，
    // 数据层、时长、操作按钮、进度条都留在固定层上，不跟着放大。
    // 封面走公共组件：同一个 `cover_art` + `cover_visual`，
    // UP 主浮窗里的投稿/动态也调这两个，样式不会漂。
    let cover_url = item.cover_thumb_url();
    let show_cover = load_cover && !cover_url.is_empty();
    if show_cover {
        active_images.borrow_mut().mark(&cover_url);
    }
    let art = cover_art(
        &title,
        if show_cover { cover_url.as_str() } else { "" },
        Some(&item.placeholder_color),
        s.fill_3,
    );
    let art = animate_cover_zoom(art, hovered, leaving, idx, hover_generation);
    let mut cover = cover_visual(
        t,
        CoverVisual {
            art: art.into_any_element(),
            views: item.view_count,
            danmaku: item.danmaku_count,
            duration_secs: item.duration_secs,
            is_live: item.is_live,
            show_stats: true,
        },
        COVER_ITEM_WIDTH / COVER_ITEM_HEIGHT,
    );

    // 悬停操作：进入 150ms 浮出（opacity 0→1 + 4px 下移到位）；
    // 离开也保留 150ms 收尾。只在 hovered / leaving 时挂进树里。
    if hovered || leaving {
        let actions = div()
            .absolute()
            .top(px(6.))
            .right(px(6.))
            .flex()
            .gap(px(SPACE_1))
            .child(hover_action(t, idx, "later", IconName::Clock, cx))
            .child(hover_action(t, idx, "download", IconName::Download, cx))
            .child(hover_action(t, idx, "more", IconName::Ellipsis, cx));
        cover = cover.child(animate_hover_actions(
            actions,
            hovered,
            leaving,
            idx,
            hover_generation,
        ));
    }

    // 观看进度：仅当本地有进度时出现（数据字段已在 CoverItem 里）。
    if item.progress_secs > 0 && item.duration_secs > 0 {
        let ratio = (item.progress_secs as f32 / item.duration_secs as f32).clamp(0., 1.);
        cover = cover.child(
            div()
                .absolute()
                .left_0()
                .right_0()
                .bottom_0()
                .h(px(3.))
                .bg(rgba(0xFFFFFF40))
                .child(div().h_full().w(relative(ratio)).bg(p.bili_pink)),
        );
    }

    let source_bvid = bvid.clone();
    let source_title = title.clone();
    let source_for_prepaint = cover_source.clone();
    let root_for_prepaint = root_bounds.clone();
    let content = div()
        .absolute()
        .inset_0()
        .flex()
        .flex_col()
        // 8 padding：封面宽度 = 卡片宽 − 16，卡片高度由 `card_height` 推导。
        .p(px(SPACE_2))
        .child(cover)
        .child(
            div()
                // 标题/署名跟随封面宽度：卡片 hover 放大时文本不再重排。
                .w(px(cover_w))
                .flex()
                .flex_col()
                .child(
                    div()
                        .mt(px(SPACE_1))
                        .h(px(LINE_HEIGHT_CONTROL * 2.))
                        .text_size(px(FONT_SIZE_CONTROL))
                        .line_height(px(LINE_HEIGHT_CONTROL))
                        .font_weight(FontWeight::MEDIUM)
                        .text_color(s.text_primary)
                        .line_clamp(2)
                        .child(title.clone()),
                )
                .child(byline(t, item, load_cover, active_images.clone(), cx)),
        )
        // 子元素 0 是封面。记录实际布局（含 hover 放大后的 inset），
        // 点击时用它作为浮窗展开的起点；这里只写 Rc，不触发重绘。
        .on_children_prepainted(move |children, _window, _cx| {
            let Some(cover) = children.first().copied() else {
                return;
            };
            let Some(root) = root_for_prepaint.get() else {
                return;
            };
            *source_for_prepaint.borrow_mut() = Some(CoverSource {
                bounds: Bounds {
                    origin: point(
                        cover.origin.x - root.origin.x,
                        cover.origin.y - root.origin.y,
                    ),
                    size: cover.size,
                },
                root_size: root.size,
                bvid: source_bvid.clone(),
                title: source_title.clone(),
                image_url: cover_image_url.clone(),
                placeholder_color: cover_placeholder.clone(),
            });
        });

    // 卡片整体 1.03：设计稿 150ms / EaseStandard，用内层负 inset 近似。
    // 封面 1.04 缓推在 art 层单独做（300ms），互不抢一条动画。
    let content = animate_card_scale(content, hovered, leaving, idx, hover_generation);

    // **固定热区**：hover 挂在这个 208×184 的透明层上，点击挂在同尺寸外框上；
    // 两者都不参与缩放动画。内容层可以放大、封面可以缓推，但鼠标的命中范围始终不变，
    // 不会出现「鼠标停在边界 → 动画放大 → 再次命中 → 反复触发」的循环。
    let source_for_click = cover_source.clone();
    let hit_layer = div()
        .id(("cover-hit", idx))
        .absolute()
        .inset_0()
        .on_hover(cx.listener(move |host, hovered: &bool, _window, cx| {
            // 播放浮窗打开期间，不允许下层封面响应 hover。
            if host.card_is_playing() {
                return;
            }
            // 滚动期间卡片从指针下滑过，GPUI 会连续给出 true/false。
            // 状态仍然要跟（指针最后停在谁上面），但**不推进动画代际**：
            // 渲染侧同时把 hover/leaving 视觉暂停，滚动时就不会反复重置缩放。
            let animate = !host.card_scroll_settling();
            if host.card_update_hover(idx, *hovered, animate) {
                cx.notify();
            }
        }));

    div()
        .id(("cover", idx))
        .relative()
        .w(px(card_w))
        .h(px(card_h))
        .rounded(px(CARD_RADIUS))
        .overflow_hidden()
        // 默认态透明（设计稿 `.card` 默认无底色），悬停时才浮出表面色。
        .bg(rgba(0x00000000))
        .border_1()
        .border_color(rgba(0x00000000))
        // 阴影/描边/背景随 hovered 状态切；缩放/缓推/操作由三条独立动画负责。
        .when(hovered, move |el| {
            el.bg(hover_bg)
                .shadow(hover_shadow)
                .border_color(hover_border)
        })
        // 点击挂在固定外框上（动作按钮是它的后代，点按钮时 stop_propagation）。
        // 这样整卡点击不依赖会缩放的内容层。
        // 已存在就复用：重建会换掉 PlayerView 里的 InputState，弹幕输入跟着丢（附录 F L-1）。
        .on_click(cx.listener(move |host, _: &ClickEvent, window, cx| {
            // 播放浮窗/遮罩已经打开时，落到下层的点击一律忽略；
            // 否则点浮窗外侧关闭的同一拍可能又打开一张封面卡。
            if host.card_is_playing() || host.card_close_requested() {
                return;
            }
            // 必须在打开播放器、网格重排之前冻结封面几何。
            let mut entrance = source_for_click
                .borrow()
                .clone()
                .filter(|source| source.bvid == bvid);
            if let (Some(source), Some(root)) = (entrance.as_mut(), root_bounds.get()) {
                // 窗口在 home 渲染后 resize 过：目标宽度取最新根尺寸。
                source.root_size = root.size;
            }
            host.card_open(bvid.clone(), title.clone(), entrance, window, cx);
        }))
        // 先挂固定热区，再挂会动画的内容；热区在下、动作按钮在上。
        .child(hit_layer)
        .child(content)
}
/// 卡片整体 1.03：208×3%÷2 ≈ 3.12，150ms / EaseStandard。
const CARD_HOVER_INSET: f32 = 3.12;
/// 封面缓推 1.04：宽高各放大 4%，300ms / EaseStandard。
const COVER_HOVER_GROW: f32 = 0.04;
fn card_scale(el: Div, t: f32) -> Div {
    el.inset(px(-CARD_HOVER_INSET * t))
}
pub fn cover_scale(el: Div, t: f32) -> Div {
    let grow = COVER_HOVER_GROW * t;
    el.w(relative(1.0 + grow))
        .h(relative(1.0 + grow))
        .left(relative(-grow / 2.0))
        .top(relative(-grow / 2.0))
}
fn actions_scale(el: Div, t: f32, entering: bool) -> Div {
    let opacity = if entering { t } else { 1.0 - t };
    let el = el.top(px(2.0 + 4.0 * opacity)).opacity(opacity);
    // 收尾动画结束时隐藏，避免透明按钮继续吃点击。
    if opacity <= 0.001 {
        el.invisible()
    } else {
        el
    }
}
fn animate_card_scale(
    el: Div,
    hovered: bool,
    leaving: bool,
    idx: usize,
    generation: u64,
) -> AnyElement {
    if hovered {
        el.with_animation(
            format!("card-scale-in-{idx}-{generation}"),
            Animation::new(DURATION_FAST).with_easing(ease_standard()),
            |el, t| card_scale(el, t),
        )
        .into_any_element()
    } else if leaving {
        el.with_animation(
            format!("card-scale-out-{idx}-{generation}"),
            Animation::new(DURATION_FAST).with_easing(ease_standard()),
            |el, t| card_scale(el, 1.0 - t),
        )
        .into_any_element()
    } else {
        card_scale(el, 0.0).into_any_element()
    }
}
pub fn animate_cover_zoom(
    el: Div,
    hovered: bool,
    leaving: bool,
    idx: usize,
    generation: u64,
) -> AnyElement {
    if hovered {
        el.with_animation(
            format!("cover-zoom-in-{idx}-{generation}"),
            Animation::new(DURATION_MODERATE).with_easing(ease_standard()),
            |el, t| cover_scale(el, t),
        )
        .into_any_element()
    } else if leaving {
        el.with_animation(
            format!("cover-zoom-out-{idx}-{generation}"),
            Animation::new(DURATION_MODERATE).with_easing(ease_standard()),
            |el, t| cover_scale(el, 1.0 - t),
        )
        .into_any_element()
    } else {
        cover_scale(el, 0.0).into_any_element()
    }
}
fn animate_hover_actions(
    el: Div,
    hovered: bool,
    leaving: bool,
    idx: usize,
    generation: u64,
) -> AnyElement {
    if hovered {
        el.with_animation(
            format!("card-actions-in-{idx}-{generation}"),
            Animation::new(DURATION_FAST).with_easing(ease_standard()),
            |el, t| actions_scale(el, t, true),
        )
        .into_any_element()
    } else if leaving {
        el.with_animation(
            format!("card-actions-out-{idx}-{generation}"),
            Animation::new(DURATION_FAST).with_easing(ease_standard()),
            |el, t| actions_scale(el, t, false),
        )
        .into_any_element()
    } else {
        el.invisible().into_any_element()
    }
}
/// 封面右上角的悬停操作（稍后再看 / 下载 / 更多）。
///
/// 三个动作的**后端**还没接；这里先把点击从整卡手势里截走，
/// 避免点「下载」却打开播放浮窗。接入真实动作时只改这个函数。
fn hover_action<V: CardHost>(
    t: &YkTheme,
    idx: usize,
    kind: &'static str,
    icon: IconName,
    cx: &mut Context<V>,
) -> impl IntoElement {
    let primary = t.palette.theme_primary;
    div()
        .id((kind, idx))
        .w(px(28.))
        .h(px(28.))
        .flex()
        .items_center()
        .justify_center()
        .rounded(px(RADIUS_MD))
        .bg(rgba(0x0000008C))
        .cursor_pointer()
        .hover(move |style| style.bg(primary))
        .child(
            Icon::new(icon)
                .size(px(ICON_SIZE_SM))
                .text_color(rgb(0xFFFFFF)),
        )
        .on_click(cx.listener(move |_this, _: &ClickEvent, _window, cx| {
            cx.stop_propagation();
            eprintln!("[card] 悬停操作 {kind} · 卡片 #{idx}（后端动作未接）");
        }))
}
/// 署名行：UP 头像 18 + 昵称 + 相对时间。
fn byline<V: CardHost>(
    t: &YkTheme,
    item: &CoverItem,
    load_image: bool,
    active_images: Rc<RefCell<ImageResidency>>,
    cx: &mut Context<V>,
) -> impl IntoElement {
    let s = &t.surfaces;
    let p = &t.palette;
    let face_url = if load_image {
        item.up_face.clone()
    } else {
        String::new()
    };
    if !face_url.is_empty() {
        active_images.borrow_mut().mark(&face_url);
    }
    let name = if item.up_name.is_empty() {
        "未知 UP 主".to_string()
    } else {
        item.up_name.clone()
    };
    let published = item.published_text();

    div()
        .flex()
        .items_center()
        .gap(px(6.))
        .h(px(CARD_BYLINE_HEIGHT))
        .mt(px(2.))
        .text_size(px(FONT_SIZE_CAPTION))
        .text_color(s.text_tertiary)
        .child(V::card_up_avatar(
            SharedString::from(format!("byline-up-{}", item.bvid)),
            item.up_mid,
            item.up_name.clone(),
            item.up_face.clone(),
            18.0,
            // 头像兜底：蓝→粉渐变圆；有 up_face 时真图盖在上面。
            yk_components::avatar::Avatar::new(18.0)
                .url(face_url.clone())
                .background(linear_gradient(
                    135.,
                    linear_color_stop(p.bili_blue, 0.),
                    linear_color_stop(p.bili_pink, 1.),
                ))
                .build(),
            cx,
        ))
        .child(
            div()
                .flex_1()
                .min_w_0()
                .truncate()
                .text_color(s.text_tertiary)
                .child(name),
        )

        .when(!published.is_empty(), |el| {
            el.child(
                div()
                    .flex_shrink_0()
                    .text_color(s.text_quaternary)
                    .child("·"),
            )
            .child(
                div()
                    .flex_shrink_0()
                    .text_color(s.text_quaternary)
                    .child(published),
            )
        })
}
