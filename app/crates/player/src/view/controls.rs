//! 控制条：进度、播放按钮、音量 / 倍速小胶囊。

use super::*;
use gpui::component::slider::SliderEvent;

/// 悬浮控制栏：弹幕输入 38 + 操作条 50 = 88。
///
/// 横向固定在 `left:0 / right:200px`（收起态面板宽度）——播放器宽度不随面板伸缩；
/// 面板展开到 400 时允许盖住右端，这是用户主动展开换来的代价。
pub(crate) fn control_bar(
    t: &YkTheme,
    state: &PlayerState,
    danmaku_input: Option<&Entity<InputState>>,
    danmaku_sending: bool,
    source: Option<&yk_domain::playback::PlaybackSource>,
    quality_switching: bool,
    quality_popover_open: bool,
    speed_popover_open: bool,
    video_left: f32,
    video_width: f32,
    progress_track_bounds: &Rc<Cell<Option<Bounds<Pixels>>>>,
    volume_popover_open: bool,
    volume_panel: &Entity<VolumePanelView>,
    volume_button_bounds: &Rc<Cell<Option<Bounds<Pixels>>>>,
    quality_button_bounds: &Rc<Cell<Option<Bounds<Pixels>>>>,
    speed_button_bounds: &Rc<Cell<Option<Bounds<Pixels>>>>,
    control_popover_bounds: &Rc<Cell<Option<Bounds<Pixels>>>>,
    cx: &mut Context<PlayerView>,
    on_play: impl Fn(&ClickEvent, &mut Window, &mut App) + 'static,
) -> impl IntoElement {
    let (time_now, time_total) = state.time_texts();
    let progress = state.progress().clamp(0.0, 1.0) as f32;
    let s = &t.surfaces;
    let p = &t.palette;
    let icon_color = hsla(0.0, 0.0, 1.0, 0.86);
    let rate_text = format!("倍速 {}x", trim_rate(state.rate));
    let volume_icon = if state.muted || state.volume <= 0.001 {
        IconName::VolumeX
    } else {
        IconName::Volume2
    };
    let quality_text = if quality_switching {
        "切换中…".to_string()
    } else {
        source
            .map(|source| source.quality.trim())
            .filter(|quality| !quality.is_empty())
            .map(|quality| quality.to_string())
            .unwrap_or_else(|| "画质".to_string())
    };

    div()
        .absolute()
        // 控制条跟视频实际显示宽度对齐并水平居中；面板展开时允许盖住右端。
        .left(px(video_left))
        .w(px(video_width))
        .bottom_0()
        .h(px(CTRL_H))
        .flex()
        .flex_col()
        // 黑框只包住控制栏本体：88px 实底 + 1px 顶边，不再铺 208px 渐变遮罩。
        .bg(rgba(0x0B0F14E6))
        .border_t_1()
        .border_color(rgba(0xFFFFFF14))
        .on_mouse_move(cx.listener(|view, event: &MouseMoveEvent, window, cx| {
            view.note_pointer_move(event.position, window, cx);
        }))
        // ---- 上半：弹幕礼仪 + 输入 + 发送 38 ----
        .child(
            div()
                .h(px(DANMAKU_INPUT_H))
                .w_full()
                .px(px(SPACE_4))
                .flex()
                .items_center()
                .gap(px(SPACE_2))
                .child(
                    div()
                        .flex_shrink_0()
                        .text_size(px(FONT_SIZE_CAPTION))
                        .text_color(hsla(0.0, 0.0, 1.0, 0.45))
                        .child("弹幕礼仪 ›"),
                )
                .child(match danmaku_input {
                    Some(st) => {
                        // Input 元素本身不 track_focus，键盘事件不会路由过去。
                        // 必须由外层接管焦点：track_focus 让元素参与焦点系统，
                        // 点击时显式 window.focus() 把输入焦点交给它。
                        let handle = st.read(cx).focus_handle(cx);
                        div()
                            .id("danmaku-input")
                            .flex_1()
                            .min_w(px(0.))
                            .h(px(26.))
                            // Enter 的绑定是 `KeyBinding::new("enter", Enter, Some("Input"))`；
                            // 焦点落在这个外层 div 上，必须补 key_context 才能匹配到 Input 的 action。
                            .key_context("Input")
                            .track_focus(&handle)
                            .on_mouse_down(
                                MouseButton::Left,
                                cx.listener({
                                    let handle = handle.clone();
                                    move |view, _ev: &MouseDownEvent, window, cx| {
                                        // 已经聚焦就什么都不做 ——
                                        // 重复 window.focus + notify 会让 Input 重建，
                                        // 表现为「点一下已输入内容的输入框，内容被清掉」。
                                        if handle.is_focused(window) {
                                            return;
                                        }
                                        view.input_focused = true;
                                        window.focus(&handle, cx);
                                        cx.notify();
                                    }
                                }),
                            )
                            // Input 自己不撑满时，hitbox 只有文字那么宽 ——
                            // 点击空白处不会聚焦。这里让它撑满整条。
                            .child(
                                Input::new(st)
                                    .appearance(false)
                                    .bordered(false)
                                    .w_full()
                                    .h_full(),
                            )
                            .into_any_element()
                    }
                    None => div()
                        .flex_1()
                        .min_w(px(0.))
                        .h(px(26.))
                        .px(px(SPACE_2))
                        .flex()
                        .items_center()
                        .rounded(px(RADIUS_SM))
                        .bg(rgba(0xFFFFFF1A))
                        .text_size(px(FONT_SIZE_CAPTION))
                        .text_color(hsla(0.0, 0.0, 1.0, 0.55))
                        .child("发个友善的弹幕见证当下")
                        .into_any_element(),
                })
                .child(
                    div()
                        .id("danmaku-send")
                        .px(px(SPACE_3))
                        .h(px(26.))
                        .flex()
                        .items_center()
                        .rounded(px(RADIUS_SM))
                        .bg(p.theme_primary)
                        // 发送中置灰：请求没回来前再点没有意义（send_danmaku 里也会挡）。
                        .opacity(if danmaku_sending { 0.5 } else { 1.0 })
                        .cursor_pointer()
                        .hover(|style| style.opacity(0.9))
                        .text_size(px(FONT_SIZE_CAPTION))
                        .text_color(rgb(0xFFFFFF))
                        .on_click(cx.listener(|view, _: &ClickEvent, window, cx| {
                            view.send_danmaku(window, cx);
                        }))
                        .child(if danmaku_sending { "发送中" } else { "发送" }),
                ),
        )
        // ---- 下半：播放 / 时间 / 进度 / 画质 / 倍速 / 图标 50 ----
        .child(
            div()
                .h(px(50.))
                .w_full()
                .px(px(SPACE_4))
                .flex()
                .items_center()
                .gap(px(SPACE_2))
                .child(
                    div()
                        .id("play-toggle")
                        .w(px(32.))
                        .h(px(32.))
                        .flex_shrink_0()
                        .flex()
                        .items_center()
                        .justify_center()
                        .rounded_full()
                        .cursor_pointer()
                        .hover(|style| style.bg(rgba(0xFFFFFF1A)))
                        .on_click(on_play)
                        .child(
                            Icon::new(if state.playing {
                                IconName::Pause
                            } else {
                                IconName::Play
                            })
                            .size(px(ICON_SIZE_SM))
                            .text_color(rgb(0xFFFFFF)),
                        ),
                )
                .child(
                    div()
                        .flex_shrink_0()
                        .text_size(px(FONT_SIZE_CAPTION))
                        .text_color(hsla(0.0, 0.0, 1.0, 0.85))
                        .child(format!("{time_now} / {time_total}")),
                )
                // 进度条：左侧已播 + 右侧未播，滑块骑在交界处。
                // 轨道自身记录 bounds，按下 / 拖动时把鼠标 x 换算成 0..1。
                .child({
                    let track_bounds = progress_track_bounds.clone();
                    div()
                        .id("player-progress-track")
                        .relative()
                        .flex_1()
                        .min_w(px(0.))
                        .h(px(12.))
                        .cursor_pointer()
                        // 按下 / 拖动由 `pointer_tracker` 的全局监听处理，
                        // 不走 hover 命中，避免播放中 hover 抖动导致点不中。
                        // 整条底轨：同时用来量轨道宽度。
                        .child(
                            div()
                                .absolute()
                                .left_0()
                                .right_0()
                                .top(px(4.))
                                .h(px(4.))
                                .rounded_full()
                                .bg(rgba(0xFFFFFF33)),
                        )
                        .child(
                            div()
                                .absolute()
                                .left_0()
                                .top(px(4.))
                                .h(px(4.))
                                .w(relative(progress))
                                .rounded_full()
                                .bg(p.theme_primary)
                                .child(
                                    div()
                                        .absolute()
                                        .right(px(-6.))
                                        .top(px(-4.))
                                        .w(px(12.))
                                        .h(px(12.))
                                        .rounded_full()
                                        .bg(rgb(0xFFFFFF)),
                                ),
                        )
                        // 轨道 bounds 由这层透明 canvas 在 prepaint 时写回。
                        .child(
                            canvas(
                                move |bounds, _, _| track_bounds.set(Some(bounds)),
                                |_, _, _, _| {},
                            )
                            .absolute()
                            .inset_0(),
                        )
                })
                .child(quality_control(
                    t,
                    source,
                    quality_text,
                    quality_switching,
                    quality_popover_open,
                    quality_button_bounds,
                    control_popover_bounds,
                    cx,
                ))
                .child(speed_control(
                    t,
                    state.rate,
                    rate_text,
                    speed_popover_open,
                    speed_button_bounds,
                    control_popover_bounds,
                    cx,
                ))
                .child(volume_control(
                    volume_panel,
                    volume_icon,
                    icon_color,
                    volume_popover_open,
                    volume_button_bounds,
                    cx,
                ))
                .child(ctrl_icon(IconName::Captions, icon_color))
                .child(ctrl_icon(IconName::Settings, icon_color))
                .child(ctrl_icon_button(
                    "player-fullscreen",
                    if state.fullscreen {
                        IconName::Minimize
                    } else {
                        IconName::Maximize
                    },
                    icon_color,
                    cx.listener(|view, _: &ClickEvent, window, cx| {
                        view.toggle_fullscreen(window, cx);
                    }),
                )),
        )
        .text_color(s.text_on_accent)
}

/// 倍速文案：整数也保留一位小数（`1.0x`），非整数按实际给出（`1.25x`）。
pub(crate) fn trim_rate(rate: f64) -> String {
    if (rate.fract()).abs() < 1e-6 {
        format!("{rate:.1}")
    } else {
        let text = format!("{rate:.2}");
        text.trim_end_matches('0').to_string()
    }
}

/// 控制栏右侧胶囊（画质 / 倍速）的外壳：点击展开菜单，菜单向上浮出。
///
/// 胶囊本身要留在 `relative` 容器里：GPUI 没有 z-index，菜单作为胶囊的
/// 兄弟节点挂在后面，才能压在控制栏之上。
fn ctrl_pill_shell(
    id: &'static str,
    label: String,
    open: bool,
    opacity: f32,
    button_bounds: &Rc<Cell<Option<Bounds<Pixels>>>>,
    popover: Option<AnyElement>,
    on_click: impl Fn(&ClickEvent, &mut Window, &mut App) + 'static,
) -> AnyElement {
    let bounds = button_bounds.clone();
    let mut root = div()
        .relative()
        .flex_shrink_0()
        .opacity(opacity)
        .child(
            div()
                .id(id)
                .h(px(28.))
                .px(px(SPACE_2))
                .flex()
                .items_center()
                .rounded(px(RADIUS_HALF))
                .bg(if open {
                    rgba(0xFFFFFF38)
                } else {
                    rgba(0xFFFFFF1F)
                })
                .cursor_pointer()
                .hover(|style| style.bg(rgba(0xFFFFFF33)))
                .on_click(on_click)
                // bounds 探针放最前：点胶囊本身属于「切换菜单」，不能被外部点击逻辑关掉。
                .child(
                    canvas(
                        move |probe, _, _| bounds.set(Some(probe)),
                        |_, _, _, _| {},
                    )
                    .absolute()
                    .inset_0(),
                )
                .child(
                    div()
                        .text_size(px(FONT_SIZE_CAPTION))
                        .text_color(hsla(0.0, 0.0, 1.0, 0.90))
                        .child(label),
                ),
        );
    if let Some(popover) = popover {
        root = root.child(popover);
    }
    root.into_any_element()
}

/// 画质胶囊 + 清晰度菜单。
fn quality_control(
    t: &YkTheme,
    source: Option<&yk_domain::playback::PlaybackSource>,
    label: String,
    switching: bool,
    open: bool,
    button_bounds: &Rc<Cell<Option<Bounds<Pixels>>>>,
    popover_bounds: &Rc<Cell<Option<Bounds<Pixels>>>>,
    cx: &mut Context<PlayerView>,
) -> AnyElement {
    let popover = open.then(|| quality_menu(t, source, switching, popover_bounds, cx));
    ctrl_pill_shell(
        "player-quality-pill",
        label,
        open,
        if switching { 0.6 } else { 1.0 },
        button_bounds,
        popover,
        cx.listener(move |view, _: &ClickEvent, _window, cx| {
            view.toggle_quality_popover(cx);
        }),
    )
}

/// 清晰度菜单：按大会员状态生成的可选档位，当前档带对勾。
fn quality_menu(
    t: &YkTheme,
    source: Option<&yk_domain::playback::PlaybackSource>,
    switching: bool,
    popover_bounds: &Rc<Cell<Option<Bounds<Pixels>>>>,
    cx: &mut Context<PlayerView>,
) -> AnyElement {
    let s = &t.surfaces;
    let p = &t.palette;
    let current = source.map(|source| source.quality_id).unwrap_or(0);
    let qualities = source.map(|source| source.qualities.as_slice()).unwrap_or(&[]);
    let bounds = popover_bounds.clone();

    let mut list = div().flex().flex_col().gap(px(SPACE_0_5));
    if qualities.is_empty() {
        list = list.child(
            div()
                .px(px(SPACE_2))
                .py(px(SPACE_2))
                .text_size(px(FONT_SIZE_CAPTION))
                .text_color(hsla(0.0, 0.0, 1.0, 0.55))
                .child("正在获取清晰度…"),
        );
    } else {
        for quality in qualities {
            let id = quality.id;
            let selected = id == current;
            let label = quality.label.clone();
            let vip_only = quality.vip_only;
            list = list.child(
                div()
                    .id(("player-quality-option", id as u64))
                    .h(px(32.))
                    .px(px(SPACE_2))
                    .flex()
                    .items_center()
                    .gap(px(SPACE_2))
                    .rounded(px(RADIUS_SM))
                    .cursor_pointer()
                    .when(selected, |el| el.bg(rgba(0xFFFFFF1A)))
                    .hover(|style| style.bg(rgba(0xFFFFFF1F)))
                    .on_click(cx.listener(move |view, _: &ClickEvent, _window, cx| {
                        view.select_quality(id, cx);
                    }))
                    .child(
                        div()
                            .flex_1()
                            .text_size(px(FONT_SIZE_CAPTION))
                            .text_color(if selected {
                                p.theme_primary
                            } else {
                                hsla(0.0, 0.0, 1.0, 0.90)
                            })
                            .child(label),
                    )
                    .when(vip_only, |el| {
                        el.child(
                            div()
                                .px(px(SPACE_1))
                                .h(px(16.))
                                .flex()
                                .items_center()
                                .rounded(px(RADIUS_SM))
                                .bg(rgba(0xF7BA2AA6))
                                .text_size(px(FONT_SIZE_MICRO))
                                .text_color(rgb(0x1A1206))
                                .child("大会员"),
                        )
                    })
                    .when(selected, |el| {
                        el.child(
                            Icon::new(IconName::Check)
                                .size(px(14.))
                                .text_color(p.theme_primary),
                        )
                    }),
            );
        }
    }

    div()
        .id("player-quality-menu")
        .absolute()
        .right_0()
        .bottom(px(34.))
        .w(px(176.))
        .p(px(SPACE_2))
        .rounded(px(RADIUS_LG))
        .bg(rgba(0x0B0F14F5))
        .border_1()
        .border_color(s.border_popover)
        .shadow(t.shadows.shadow_glass_elevated.clone())
        .occlude()
        // bounds 探针放最前：菜单行画在它上面，点击优先给行。
        .child(
            canvas(
                move |probe, _, _| bounds.set(Some(probe)),
                |_, _, _, _| {},
            )
            .absolute()
            .inset_0(),
        )
        .child(
            div()
                .px(px(SPACE_2))
                .pb(px(SPACE_1))
                .text_size(px(FONT_SIZE_MICRO))
                .text_color(hsla(0.0, 0.0, 1.0, 0.50))
                .child(if switching { "正在切换…" } else { "清晰度" }),
        )
        .child(list)
        .into_any_element()
}

/// 倍速胶囊 + 速度菜单。
fn speed_control(
    t: &YkTheme,
    rate: f64,
    label: String,
    open: bool,
    button_bounds: &Rc<Cell<Option<Bounds<Pixels>>>>,
    popover_bounds: &Rc<Cell<Option<Bounds<Pixels>>>>,
    cx: &mut Context<PlayerView>,
) -> AnyElement {
    let popover = open.then(|| speed_menu(t, rate, popover_bounds, cx));
    ctrl_pill_shell(
        "player-speed-pill",
        label,
        open,
        1.0,
        button_bounds,
        popover,
        cx.listener(move |view, _: &ClickEvent, _window, cx| {
            view.toggle_speed_popover(cx);
        }),
    )
}

/// 倍速菜单：预设档位 + 0.1 步进微调，范围 0.1x–10x。
fn speed_menu(
    t: &YkTheme,
    rate: f64,
    popover_bounds: &Rc<Cell<Option<Bounds<Pixels>>>>,
    cx: &mut Context<PlayerView>,
) -> AnyElement {
    let s = &t.surfaces;
    let p = &t.palette;
    let bounds = popover_bounds.clone();
    let value = format!("{}x", trim_rate(rate));

    let step_button = |id: &'static str, text: &'static str, delta: f64| {
        div()
            .id(id)
            .w(px(30.))
            .h(px(28.))
            .flex()
            .items_center()
            .justify_center()
            .rounded(px(RADIUS_SM))
            .bg(s.fill_3)
            .cursor_pointer()
            .hover(|style| style.bg(rgba(0xFFFFFF2E)))
            .text_size(px(FONT_SIZE_CONTROL))
            .text_color(s.text_primary)
            .child(text)
            .on_click(cx.listener(move |view, _: &ClickEvent, window, cx| {
                view.adjust_rate(delta, window, cx);
            }))
    };

    let stepper = div()
        .flex()
        .items_center()
        .justify_between()
        .child(step_button("player-speed-minus", "−", -yk_domain::player::RATE_STEP))
        .child(
            div()
                .text_size(px(FONT_SIZE_TITLE))
                .font_weight(FontWeight::SEMIBOLD)
                .text_color(p.theme_primary)
                .child(value),
        )
        .child(step_button("player-speed-plus", "+", yk_domain::player::RATE_STEP));

    let mut presets = div().flex().flex_wrap().gap(px(6.));
    for preset in [
        0.1_f64, 0.25, 0.5, 0.75, 1.0, 1.25, 1.5, 2.0, 3.0, 5.0, 8.0, 10.0,
    ] {
        let active = (rate - preset).abs() < 1e-9;
        let text = format!("{}x", trim_rate(preset));
        presets = presets.child(
            div()
                .id(("player-speed-preset", (preset * 100.0) as u64))
                .w(px(46.))
                .h(px(26.))
                .flex()
                .items_center()
                .justify_center()
                .rounded(px(RADIUS_SM))
                .bg(if active {
                    p.theme_primary
                } else {
                    hsla(0.0, 0.0, 1.0, 0.08)
                })
                .cursor_pointer()
                .hover(|style| style.bg(rgba(0xFFFFFF2E)))
                .text_size(px(FONT_SIZE_MICRO))
                .text_color(if active {
                    hsla(0.0, 0.0, 1.0, 1.0)
                } else {
                    hsla(0.0, 0.0, 1.0, 0.86)
                })
                .child(text)
                .on_click(cx.listener(move |view, _: &ClickEvent, window, cx| {
                    view.set_rate(preset, window, cx);
                    view.close_control_popovers(cx);
                })),
        );
    }

    div()
        .id("player-speed-menu")
        .absolute()
        .right_0()
        .bottom(px(34.))
        .w(px(176.))
        .p(px(SPACE_2))
        .flex()
        .flex_col()
        .gap(px(SPACE_2))
        .rounded(px(RADIUS_LG))
        .bg(rgba(0x0B0F14F5))
        .border_1()
        .border_color(s.border_popover)
        .shadow(t.shadows.shadow_glass_elevated.clone())
        .occlude()
        .child(
            canvas(
                move |probe, _, _| bounds.set(Some(probe)),
                |_, _, _, _| {},
            )
            .absolute()
            .inset_0(),
        )
        .child(
            div()
                .px(px(SPACE_2))
                .text_size(px(FONT_SIZE_MICRO))
                .text_color(hsla(0.0, 0.0, 1.0, 0.50))
                .child("播放速度"),
        )
        .child(stepper)
        .child(presets)
        .child(
            div()
                .px(px(SPACE_2))
                .text_size(px(FONT_SIZE_MICRO))
                .text_color(hsla(0.0, 0.0, 1.0, 0.42))
                .child("范围 0.1x – 10x"),
        )
        .into_any_element()
}

/// 音量图标 + 可拖动的音量弹出条。
///
/// 点图标开 / 关弹出条；外部点击、控制栏收起时由控制器关闭。
fn volume_control(
    panel: &Entity<VolumePanelView>,
    icon: IconName,
    color: Hsla,
    open: bool,
    button_bounds: &Rc<Cell<Option<Bounds<Pixels>>>>,
    cx: &mut Context<PlayerView>,
) -> impl IntoElement {
    let button_bounds = button_bounds.clone();
    div()
        .relative()
        .flex_shrink_0()
        .child(
            div()
                .id("player-volume")
                .w(px(32.))
                .h(px(32.))
                .flex()
                .items_center()
                .justify_center()
                .rounded(px(RADIUS_HALF))
                .cursor_pointer()
                .hover(|style| style.bg(rgba(0xFFFFFF1A)))
                .on_click(cx.listener(|view, _: &ClickEvent, _window, cx| {
                    view.toggle_volume_popover(cx);
                }))
                // 先放 bounds 探针，后面图标在命中顺序上更靠前。
                .child(
                    canvas(
                        move |bounds, _, _| button_bounds.set(Some(bounds)),
                        |_, _, _, _| {},
                    )
                    .absolute()
                    .inset_0(),
                )
                .child(Icon::new(icon).size(px(ICON_SIZE_SM)).text_color(color)),
        )
        // 弹出条从图标正上方浮出：右缘对齐，宽度向左展开，不挡控制条。
        .when(open, |this| this.child(panel.clone()))
}

/// 音量弹出条：独立实体，拖动时只重绘这一小块。
///
/// 以前弹出条直接挂在 `PlayerView::render` 里：滑杆每移动 1px 都会让整个
/// 播放器（视频层、控制栏、评论面板）重排一遍，手感明显发黏。拆成独立
/// 实体后，拖动只重绘 172×84 的小面板，播放器主体不参与。
pub(crate) struct VolumePanelView {
    theme: YkTheme,
    slider: Entity<SliderState>,
    player: WeakEntity<PlayerView>,
    bounds: Rc<Cell<Option<Bounds<Pixels>>>>,
    volume: f64,
    muted: bool,
}

impl VolumePanelView {
    pub(crate) fn new(
        theme: YkTheme,
        slider: Entity<SliderState>,
        player: WeakEntity<PlayerView>,
        bounds: Rc<Cell<Option<Bounds<Pixels>>>>,
        volume: f64,
        muted: bool,
        cx: &mut Context<Self>,
    ) -> Self {
        // 滑杆状态变化（拖动 / 键盘改音量）只通知本面板重绘。
        cx.observe(&slider, |_, _, cx| cx.notify()).detach();
        cx.subscribe(&slider, |panel, _slider, event: &SliderEvent, cx| {
            let (raw, release) = match event {
                SliderEvent::Change(value) => (value.end(), false),
                SliderEvent::Release(value) => (value.end(), true),
            };
            panel.volume = (f64::from(raw) / 100.0).clamp(0.0, 1.0);
            // mpv 放开音量后自然解除静音，与控制条图标保持一致。
            if panel.volume > 0.0 {
                panel.muted = false;
            }
            let player = panel.player.clone();
            let volume = panel.volume;
            let _ = player.update(cx, |player, cx| {
                player.set_volume_from_panel(volume, release, cx);
            });
            cx.notify();
        })
        .detach();
        Self {
            theme,
            slider,
            player,
            bounds,
            volume,
            muted,
        }
    }

    /// 键盘调音量 / 外部状态变化：同步滑杆与文案。
    pub(crate) fn set_volume(
        &mut self,
        volume: f64,
        muted: bool,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        self.volume = volume.clamp(0.0, 1.0);
        self.muted = muted;
        let target = (self.volume * 100.0).round() as f32;
        self.slider.update(cx, |state, cx| {
            if (state.value().end() - target).abs() > 0.5 {
                state.set_value(target, window, cx);
            }
        });
        cx.notify();
    }

    /// 静音状态变化（音量不变，不需要动滑杆）。
    pub(crate) fn set_muted(&mut self, muted: bool, cx: &mut Context<Self>) {
        self.muted = muted;
        cx.notify();
    }
}

impl Render for VolumePanelView {
    fn render(&mut self, _window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let percent = (self.volume.clamp(0.0, 1.0) * 100.0).round() as i32;
        let muted = self.muted || self.volume <= 0.001;
        let bounds_cell = self.bounds.clone();
        let accent = self.theme.palette.theme_primary;
        let icon_color = hsla(0.0, 0.0, 1.0, 0.86);
        div()
            .id("player-volume-popover")
            .absolute()
            .right_0()
            .bottom(px(36.))
            .w(px(172.))
            .p(px(SPACE_3))
            .flex()
            .flex_col()
            .gap(px(SPACE_2))
            .rounded(px(RADIUS_SM))
            .bg(rgba(0x0B0F14F2))
            .border_1()
            .border_color(rgba(0xFFFFFF1F))
            .shadow(self.theme.shadows.shadow_glass_elevated.clone())
            // bounds 探针作为第一个子元素：滑杆 / 静音按钮画在它上面，
            // 命中顺序上优先给控件，指针不会被一层透明 canvas 截走。
            .child(
                canvas(
                    move |bounds, _, _| bounds_cell.set(Some(bounds)),
                    |_, _, _, _| {},
                )
                .absolute()
                .inset_0(),
            )
            .child(
                div()
                    .flex()
                    .items_center()
                    .justify_between()
                    .child(
                        div()
                            .id("player-volume-mute")
                            .w(px(24.))
                            .h(px(24.))
                            .flex()
                            .items_center()
                            .justify_center()
                            .rounded(px(RADIUS_SM))
                            .cursor_pointer()
                            .hover(|style| style.bg(rgba(0xFFFFFF1A)))
                            .on_click(cx.listener(|panel, _: &ClickEvent, _window, cx| {
                                let player = panel.player.clone();
                                let Some(player) = player.upgrade() else {
                                    return;
                                };
                                let mut muted = None;
                                let _ = player.update(cx, |player, cx| {
                                    muted = Some(player.toggle_mute_from_panel(cx));
                                });
                                if let Some(muted) = muted {
                                    panel.set_muted(muted, cx);
                                }
                            }))
                            .child(
                                Icon::new(if muted {
                                    IconName::VolumeX
                                } else {
                                    IconName::Volume2
                                })
                                .size(px(16.))
                                .text_color(icon_color),
                            ),
                    )
                    .child(
                        div()
                            .text_size(px(FONT_SIZE_CAPTION))
                            .text_color(hsla(0.0, 0.0, 1.0, 0.86))
                            .child(if muted {
                                "静音".to_string()
                            } else {
                                format!("{percent}%")
                            }),
                    ),
            )
            .child(
                div().w_full().h(px(20.)).flex().items_center().child(
                    Slider::new(&self.slider)
                        .horizontal()
                        .w_full()
                        // 与控制条进度条同一套配色：主题色填充 + 白色滑块。
                        .bg(accent)
                        .text_color(rgb(0xFFFFFF)),
                ),
            )
    }
}

pub(crate) fn ctrl_icon(icon: IconName, color: Hsla) -> impl IntoElement {
    div()
        .w(px(32.))
        .h(px(32.))
        .flex_shrink_0()
        .flex()
        .items_center()
        .justify_center()
        .rounded(px(RADIUS_HALF))
        .cursor_pointer()
        .hover(|style| style.bg(rgba(0xFFFFFF1A)))
        .child(Icon::new(icon).size(px(ICON_SIZE_SM)).text_color(color))
}

/// 可点击的控制栏图标：全屏 / 退出全屏这类带动作的按钮。
pub(crate) fn ctrl_icon_button(
    id: &'static str,
    icon: IconName,
    color: Hsla,
    on_click: impl Fn(&ClickEvent, &mut Window, &mut App) + 'static,
) -> impl IntoElement {
    div()
        .id(id)
        .w(px(32.))
        .h(px(32.))
        .flex_shrink_0()
        .flex()
        .items_center()
        .justify_center()
        .rounded(px(RADIUS_HALF))
        .cursor_pointer()
        .hover(|style| style.bg(rgba(0xFFFFFF1A)))
        .on_click(on_click)
        .child(Icon::new(icon).size(px(ICON_SIZE_SM)).text_color(color))
}

