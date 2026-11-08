//! 底部 Dock：全局一级导航 + 快捷动作。
//!
//! Dock 只决定「长什么样、有几个入口」；每个入口点了之后去哪、要不要先登录，
//! 由组装点通过 `on_select(index)` 处理。这样 shell 不持有路由与业务状态。
//!
//! 选中态是**一条会滑动的液态玻璃胶囊**：位置不按按钮各自上色，而是量出
//! 当前按钮的包围盒后，用 `spring` 把一条绝对定位的玻璃胶囊滑过去。弹簧保留
//! 速度，连着切页面时胶囊不会从头重来；滑动越快，横向拉伸越多（甩动感），
//! 停下后回弹到按钮本身的宽度。

use std::cell::RefCell;
use std::rc::Rc;
use std::time::Duration;

use gpui::assets::IconName;
use gpui::base::{spring, ElementExt as _, Spring};
use gpui::component::Icon;
use gpui::prelude::*;
use gpui::{InteractiveElement, StatefulInteractiveElement, *};

use yk_theme::theme::*;

/// 一个 Dock 入口。`active` / `enabled` / `unread` 由组装点按当前页面算好。
#[derive(Clone, Debug)]
pub struct DockItem {
    pub id: &'static str,
    pub label: &'static str,
    pub icon: IconName,
    pub active: bool,
    pub enabled: bool,
    pub unread: bool,
    /// 该入口「是否具备」二级分页（消息 / 排行榜）。决定箭头槽位是否常驻，
    /// 这样展开态切换时胶囊长度才能连续变化，而不是突然多出一截。
    pub has_secondary: bool,
    /// 该入口当前是不是展开态（箭头方向 + 槽位宽度）。
    pub expandable: bool,
    /// 二级分页当前是展开还是收起（决定箭头方向）。
    pub expanded: bool,
}

impl DockItem {
    pub fn nav(id: &'static str, label: &'static str, icon: IconName, active: bool) -> Self {
        Self {
            id,
            label,
            icon,
            active,
            enabled: true,
            unread: false,
            has_secondary: false,
            expandable: false,
            expanded: false,
        }
    }

    pub fn action(
        id: &'static str,
        label: &'static str,
        icon: IconName,
        active: bool,
        enabled: bool,
        unread: bool,
    ) -> Self {
        Self {
            id,
            label,
            icon,
            active,
            enabled,
            unread,
            has_secondary: false,
            expandable: false,
            expanded: false,
        }
    }
}

/// 点击回调：拿到的是「导航项在前、动作项在后」的全局序号。
pub type DockSelect<V> = Rc<dyn Fn(&mut V, usize, &mut Window, &mut Context<V>)>;

/// 二级分页的一项：页内分支（消息中心的四个通道 / 排行榜的二十个分区）。
///
/// 胶囊外形 / active 态由 Dock 统一画；里面的内容（文字 + 未读角标）
/// 由组装点提供，这样 shell 不用知道角标长什么样。
pub struct DockTab {
    /// 在本组 tab 里的序号，点击时原样回传给组装点。
    pub index: usize,
    pub active: bool,
    pub content: AnyElement,
}

/// Dock 的二级分页：展开时是浮在主 Dock 正上方的另一条胶囊。
///
/// 一级 Dock 回答「在哪个页面」，二级分页回答「在页面的哪个分支」。
/// 收起时不占位，也不会把内容顶上去。
pub struct DockTabs {
    pub open: bool,
    pub items: Vec<DockTab>,
    /// 可用宽度（窗口宽 − 左右内边距）。窄窗时胶囊折行，不会越出窗口。
    pub max_width: f32,
}

/// 二级分页的点击回调：只回传 tab 序号，映射交给组装点。
pub type DockTabSelect<V> = Rc<dyn Fn(&mut V, usize, &mut Window, &mut Context<V>)>;

/// 选中态胶囊的测量与动画状态。
///
/// 位置来自 `on_prepaint` 量到的按钮包围盒：指示器画在内层行里，用
/// 「按钮矩形 − 内层行原点」换算成相对坐标。`Rc<RefCell>` 是为了让 prepaint
/// 只写数据、不触发重绘。
#[derive(Default)]
struct DockMetrics {
    /// 内层行（无 padding / border）的包围盒，绝对坐标。
    container: Bounds<Pixels>,
    /// 每个按钮的包围盒，绝对坐标；下标 = 导航项在前、动作项在后。
    items: Vec<Bounds<Pixels>>,
    /// 上一帧指示器的目标矩形 (x, y, w, h)，绝对坐标；用来判断要不要补一帧。
    placed: Option<(f32, f32, f32, f32)>,
    /// 上一帧胶囊的 left，用来算滑动时的「甩动」拉伸。
    last_left: f32,
    /// 当前拉伸量（px）。
    stretch: f32,
}

impl DockMetrics {
    fn resize(&mut self, count: usize) {
        if self.items.len() != count {
            self.items.resize(count, Bounds::default());
        }
    }
}

/// 画 Dock。返回的元素已经绝对定位到窗口底部。
pub fn dock<V: 'static>(
    t: &YkTheme,
    nav: &[DockItem],
    actions: &[DockItem],
    tabs: Option<DockTabs>,
    material: SurfaceMaterial,
    window: &mut Window,
    cx: &mut Context<V>,
    on_select: DockSelect<V>,
    on_tab: DockTabSelect<V>,
) -> AnyElement {
    let s = &t.surfaces;
    let p = &t.palette;
    let base = nav.len();

    // 动画状态按窗口存：Dock 本身是无状态的渲染函数，不能挂在 self 上。
    let metrics = window
        .use_keyed_state("dock-indicator", cx, |_, _| {
            Rc::new(RefCell::new(DockMetrics::default()))
        })
        .read(cx)
        .clone();
    metrics.borrow_mut().resize(nav.len() + actions.len());

    // ---- 选中态：一条会在按钮之间滑动的玻璃胶囊 ----
    let active_flat = nav
        .iter()
        .position(|item| item.active)
        .or_else(|| {
            actions
                .iter()
                .position(|item| item.active)
                .map(|offset| base + offset)
        });
    let mut pill: Option<AnyElement> = None;
    if let Some(flat) = active_flat {
        let measured = {
            let m = metrics.borrow();
            m.items
                .get(flat)
                .copied()
                .filter(|bounds| {
                    bounds.size.width > px(0.) && m.container.size.width > px(0.)
                })
                .map(|bounds| {
                    (
                        bounds,
                        bounds.origin.x - m.container.origin.x,
                        bounds.origin.y - m.container.origin.y,
                    )
                })
        };

        if let Some((bounds, left_target, top_target)) = measured {
            let width_target = bounds.size.width;
            let height_target = bounds.size.height;
            let first_frame = {
                let mut m = metrics.borrow_mut();
                let first = m.placed.is_none();
                m.placed = Some((
                    bounds.origin.x.as_f32(),
                    bounds.origin.y.as_f32(),
                    bounds.size.width.as_f32(),
                    bounds.size.height.as_f32(),
                ));
                first
            };

            // 阻尼略小于 1：到位后有极小的过冲，像液体被推过去再稳住。
            let policy = Spring::new(Duration::from_millis(300))
                .with_damping(0.82)
                .with_epsilon(0.5);
            let left = spring(("dock-pill", "left"), left_target, policy, window, cx);
            let top = spring(("dock-pill", "top"), top_target, policy, window, cx);
            let width = spring(("dock-pill", "width"), width_target, policy, window, cx);
            let height = spring(("dock-pill", "height"), height_target, policy, window, cx);

            // 「液态」甩动：滑动越快，胶囊横向拉得越长，停下后回落。
            //
            // 回落必须能**收到 0**：弹簧整定后不再请求新帧，若这里还留一点
            // 平滑残量，胶囊会带着「变宽 + 左移一半」的偏移定格在按钮上。
            // 所以位移小于 1px 时直接清零，不做平滑。
            let (draw_left, draw_width) = {
                let mut m = metrics.borrow_mut();
                let delta = (left.as_f32() - m.last_left).abs();
                m.last_left = left.as_f32();
                // 弹簧到位的那一帧会返回精确目标值：用这个信号把回弹收干净。
                let settled = left == left_target
                    && top == top_target
                    && width == width_target
                    && height == height_target;
                if first_frame || settled || delta < 1.0 {
                    m.stretch = 0.0;
                } else {
                    let target = (delta * 0.9).clamp(0.0, 14.0);
                    m.stretch = m.stretch * 0.5 + target * 0.5;
                }
                (left.as_f32() - m.stretch / 2.0, width.as_f32() + m.stretch)
            };

            let primary = p.theme_primary;
            let glass = material.is_glass();
            let lens = accent_glass(t);
            let mut pill_el = div()
                .id("dock-pill")
                .absolute()
                .left(px(draw_left))
                .top(px(top.as_f32()))
                .w(px(draw_width))
                .h(px(height.as_f32()))
                .rounded_full()
                .bg(if glass { lens.fill } else { primary.into() });
            if glass {
                pill_el = pill_el
                    .border_1()
                    .border_color(lens.border)
                    .shadow(lens.shadows)
                    // 顶部折射面：一条从上往下的白色高光，玻璃的「液面」。
                    .child(
                        div()
                            .absolute()
                            .inset_0()
                            .rounded_full()
                            .bg(linear_gradient(
                                180.,
                                linear_color_stop(hsla_white(0.24), 0.0),
                                linear_color_stop(hsla_white(0.0), 0.62),
                            )),
                    );
            }
            pill = Some(pill_el.into_any_element());
        }
    }

    // 内层行：无 padding / border，指示器的相对坐标以它的原点为准。
    let metrics_for_container = metrics.clone();
    let has_active = active_flat.is_some();
    let mut items_row = div()
        .relative()
        .flex()
        .flex_wrap()
        .items_center()
        .justify_center()
        .gap(px(SPACE_1))
        .on_prepaint(move |bounds, window, _| {
            let mut m = metrics_for_container.borrow_mut();
            m.container = bounds;
            // 首帧还量不到按钮，指示器没画；这里补一帧，让它出现。
            let need_first = has_active && m.placed.is_none();
            drop(m);
            if need_first {
                let view = window.current_view();
                window.on_next_frame(move |_, cx| cx.notify(view));
            }
        });
    if let Some(pill) = pill {
        items_row = items_row.child(pill);
    }

    for (index, item) in nav.iter().enumerate() {
        let fg = if item.active {
            s.text_on_accent
        } else {
            s.text_secondary
        };
        let hover_bg = s.fill_2;
        let on_select = on_select.clone();
        let metrics_for_item = metrics.clone();
        let active = item.active;
        let button = div()
                .id(item.id)
                .h(px(DOCK_ITEM_HEIGHT))
                .px(px(SPACE_3))
                .flex()
                .items_center()
                .gap(px(SPACE_2))
                .rounded_full()
                .text_size(px(FONT_SIZE_CAPTION))
                .font_weight(if item.active {
                    FontWeight::SEMIBOLD
                } else {
                    FontWeight::MEDIUM
                })
                .text_color(fg)
                .cursor_pointer()
                // 选中项的底色交给滑动的玻璃胶囊；只给未选中项画 hover 底。
                .when(!item.active, |el| {
                    el.hover(move |style| style.bg(hover_bg))
                })
                .child(Icon::new(item.icon).size(px(16.)).text_color(fg))
                .child(item.label)
                // 「视频排行榜」这类导航项还有二级分页：箭头槽位常驻，
                // 展开 / 收起时宽度走弹簧，胶囊长度连续变化。
                .children(
                    item.has_secondary
                        .then(|| chevron_slot(item.id, item.expandable, fg, window, cx)),
                )
                .on_click(cx.listener(move |this, _: &ClickEvent, window, cx| {
                    on_select(this, index, window, cx);
                }));
        // 量按钮必须套一层「无 padding」的壳：`on_prepaint` 的测量画布从
        // 内容盒起算，直接量带 px(SPACE_3) 的按钮会让原点右移一个内边距。
        items_row = items_row.child(
            div()
                .flex_shrink_0()
                .on_prepaint(move |bounds, window, _| {
                    record_item(metrics_for_item, index, active, bounds, window);
                })
                .child(button),
        );
    }

    // 导航与动作之间用 1px 分隔线。
    items_row = items_row.child(
        div()
            .w(px(1.))
            .h(px(20.))
            .mx(px(SPACE_1))
            .bg(s.border_popover),
    );

    for (offset, item) in actions.iter().enumerate() {
        let fg = if !item.enabled {
            s.text_quaternary
        } else if item.active {
            s.text_on_accent
        } else {
            s.text_secondary
        };
        let hover_bg = s.fill_2;
        let hover_fg = if item.active {
            s.text_on_accent
        } else {
            s.text_primary
        };
        let mut action = div()
            .id(item.id)
            .h(px(DOCK_ITEM_HEIGHT))
            .px(px(SPACE_3))
            .flex()
            .items_center()
            .gap(px(SPACE_2))
            .rounded_full()
            .text_size(px(FONT_SIZE_CAPTION))
            .font_weight(if item.active {
                FontWeight::SEMIBOLD
            } else {
                FontWeight::MEDIUM
            })
            .text_color(fg)
            .child(
                div()
                    .relative()
                    .flex()
                    .items_center()
                    .child(Icon::new(item.icon).size(px(16.)).text_color(fg))
                    .when(item.unread, |el| {
                        el.child(
                            div()
                                .absolute()
                                .top(px(-2.))
                                .right(px(-2.))
                                .w(px(6.))
                                .h(px(6.))
                                .rounded_full()
                                .bg(p.bili_pink),
                        )
                    }),
            )
            .child(item.label);
        // 「消息」这类入口还有二级分页：箭头槽位常驻（见 chevron_slot）。
        if item.has_secondary {
            action = action.child(chevron_slot(item.id, item.expandable, fg, window, cx));
        }
        if item.enabled {
            if !item.active {
                action = action.hover(move |style| style.bg(hover_bg).text_color(hover_fg));
            }
            let on_select = on_select.clone();
            let index = base + offset;
            action = action.on_click(cx.listener(move |this, _: &ClickEvent, window, cx| {
                on_select(this, index, window, cx);
            }));
        }
        let metrics_for_item = metrics.clone();
        let active = item.active;
        let index = base + offset;
        items_row = items_row.child(
            div()
                .flex_shrink_0()
                .on_prepaint(move |bounds, window, _| {
                    record_item(metrics_for_item, index, active, bounds, window);
                })
                .child(action),
        );
    }

    // 外壳：玻璃版是半透明冷灰渐变 + 白色细边 + 内高光；实色版回到
    // surface_toolbar + border_strong。两套只差颜色 / 阴影，几何不动。
    //
    // 玻璃底子要够实（0.86 / 0.93）：封面上白字要是能透进来，读起来就不像
    // 玻璃而像一层没擦干净的脏污。
    let shell_glass = GlassSurface::new(t, s.surface_toolbar, 0.06, 0.86, 0.93);
    let mut shell = div()
        .flex()
        .items_center()
        .justify_center()
        .p(px(6.))
        .rounded_full()
        // 浮层必须挡住下层的封面：胶囊的 padding / 按钮间隙没有自己的
        // 点击处理器，不 occlude 的话点缝里会直接点开背后的封面。
        .occlude();
    shell = if material.is_glass() {
        shell
            .bg(shell_glass.fill)
            .border_1()
            .border_color(shell_glass.border)
            .shadow(shell_glass.shadows)
    } else {
        shell
            .bg(s.surface_toolbar)
            .border_1()
            .border_color(s.border_strong)
            .shadow(t.shadows.shadow_glass_elevated.clone())
    };
    let shell = shell.child(items_row);

    // 竖排两层：二级分页在上，主 Dock 在下。
    //
    // 外层保持「整宽 + 居中」，但**不要**在整宽上 occlude：Dock 与左下角
    // 悬浮文件夹在同一高度带，整宽拦截会把图标的 hover / 点击吃掉。
    // 真正需要挡点击的是紧贴内容的这一列（含两级胶囊之间的缝）。
    let mut column = div()
        .flex()
        .flex_col()
        .items_center()
        .gap(px(SPACE_2))
        .occlude();

    if let Some(tabs) = tabs {
        if tabs.open && !tabs.items.is_empty() {
            column = column.child(tabs_row(t, tabs.items, tabs.max_width, material, cx, on_tab));
        }
    }

    div()
        .absolute()
        .left(px(0.))
        .right(px(0.))
        .bottom(px(DOCK_BOTTOM_MARGIN))
        .px(px(SPACE_4))
        .flex()
        .justify_center()
        .child(column.child(shell))
        .into_any_element()
}

/// 记录一个按钮的包围盒；量到的位置和上一帧放胶囊的目标不一致时补一帧。
///
/// 补帧只发生在「首帧 / 窗口缩放 / 折行」这类几何真的变了的时候，
/// 动画本身由 `spring` 自己请求帧，不会在这里反复触发。
fn record_item(
    metrics: Rc<RefCell<DockMetrics>>,
    index: usize,
    active: bool,
    bounds: Bounds<Pixels>,
    window: &mut Window,
) {
    let mut m = metrics.borrow_mut();
    if let Some(slot) = m.items.get_mut(index) {
        *slot = bounds;
    }
    let stale = active
        && m.container.size.width > px(0.)
        && m.placed.is_some_and(|(x, y, w, h)| {
            x != bounds.origin.x.as_f32()
                || y != bounds.origin.y.as_f32()
                || w != bounds.size.width.as_f32()
                || h != bounds.size.height.as_f32()
        });
    drop(m);
    if stale {
        let view = window.current_view();
        window.on_next_frame(move |_, cx| cx.notify(view));
    }
}

/// 二级分页箭头槽位：宽度 0 ↔ 12px，配一条 -8px 的外边距抵掉 flex gap。
///
/// 槽位常驻而不是按需挂载，所以展开 / 收起两个方向都有过渡：胶囊从中间
/// 长出一小截 / 收回去，而不是「啪」地多出 20px。
fn chevron_slot(
    id: &'static str,
    expanded: bool,
    fg: Hsla,
    window: &mut Window,
    cx: &mut App,
) -> Div {
    let policy = Spring::new(Duration::from_millis(260))
        .with_damping(0.9)
        .with_epsilon(0.3);
    let width = spring(
        (id, "dock-chevron-w"),
        px(if expanded { 12.0 } else { 0.0 }),
        policy,
        window,
        cx,
    );
    let pull = spring(
        (id, "dock-chevron-pull"),
        px(if expanded { 0.0 } else { -SPACE_2 }),
        policy,
        window,
        cx,
    );
    div()
        .w(width)
        .ml(pull)
        .flex_shrink_0()
        .overflow_hidden()
        .flex()
        .items_center()
        .justify_end()
        .child(
            Icon::new(if expanded {
                IconName::ChevronUp
            } else {
                IconName::ChevronDown
            })
            .size(px(12.))
            .text_color(fg),
        )
}

/// 二级分页：一条玻璃胶囊，里面是页内分支按钮。
fn tabs_row<V: 'static>(
    t: &YkTheme,
    items: Vec<DockTab>,
    max_width: f32,
    material: SurfaceMaterial,
    cx: &mut Context<V>,
    on_tab: DockTabSelect<V>,
) -> AnyElement {
    let s = &t.surfaces;
    let p = &t.palette;
    let glass = material.is_glass();
    let capsule = GlassSurface::new(t, s.surface_toolbar, 0.07, 0.72, 0.86);
    let mut row = div()
        .id("dock-tabs")
        .flex()
        // 分区有二十个：窄窗里折行，而不是把胶囊撑出窗口。
        .flex_wrap()
        .items_center()
        .justify_center()
        .gap(px(2.))
        .p(px(3.))
        .max_w(px(max_width))
        .rounded_full()
        .occlude();
    row = if glass {
        row.bg(capsule.fill)
            .border_1()
            .border_color(capsule.border)
            .shadow(capsule.shadows)
    } else {
        row.bg(s.surface_toolbar)
            .border_1()
            .border_color(s.border_strong)
            .shadow(t.shadows.shadow_glass_elevated.clone())
    };

    for tab in items {
        let fg = if tab.active {
            p.theme_primary
        } else {
            s.text_tertiary
        };
        let hover_fg = p.theme_primary;
        let index = tab.index;
        let on_tab = on_tab.clone();
        row = row.child(
            div()
                .id(("dock-tab", index))
                .h(px(30.))
                .px(px(14.))
                .flex()
                .items_center()
                .gap(px(6.))
                .rounded_full()
                // active 是一块磨砂玻璃片：半透明白 + 白色细边 + 内高光。
                .bg(match (tab.active, glass) {
                    (true, true) => hsla_white(0.20),
                    (true, false) => s.surface_content_solid,
                    _ => hsla(0., 0., 0., 0.),
                })
                // 边框常驻（非 active 透明）：只在选中时加会挤出 2px，
                // 切换通道时胶囊里的文字会横向抖一下。
                .border_1()
                .border_color(if tab.active && glass {
                    hsla_white(0.16)
                } else {
                    hsla(0., 0., 0., 0.)
                })
                .shadow(if tab.active {
                    t.shadows.shadow_1.clone()
                } else {
                    Vec::new()
                })
                .text_size(px(FONT_SIZE_CONTROL))
                .font_weight(if tab.active {
                    FontWeight::SEMIBOLD
                } else {
                    FontWeight::MEDIUM
                })
                .text_color(fg)
                .cursor_pointer()
                .hover(move |style| style.text_color(hover_fg))
                .child(tab.content)
                .on_click(cx.listener(move |this, _: &ClickEvent, window, cx| {
                    on_tab(this, index, window, cx);
                    window.refresh();
                })),
        );
    }

    row.into_any_element()
}
