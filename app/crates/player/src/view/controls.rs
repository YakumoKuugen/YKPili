//! 控制条：进度、播放按钮、音量 / 倍速小胶囊。

use super::*;

/// 悬浮控制栏：弹幕输入 38 + 操作条 50 = 88。
///
/// 横向固定在 `left:0 / right:200px`（收起态面板宽度）——播放器宽度不随面板伸缩；
/// 面板展开到 400 时允许盖住右端，这是用户主动展开换来的代价。
pub(crate) fn control_bar(
    t: &YkTheme,
    state: &PlayerState,
    danmaku_input: Option<&Entity<InputState>>,
    danmaku_sending: bool,
    quality: &str,
    video_left: f32,
    video_width: f32,
    progress_track_bounds: &Rc<Cell<Option<Bounds<Pixels>>>>,
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
    let quality_text = if quality.is_empty() {
        "画质".to_string()
    } else {
        quality.to_string()
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
                .child(ctrl_pill(quality_text))
                .child(ctrl_pill(rate_text))
                .child(ctrl_icon(volume_icon, icon_color))
                .child(ctrl_icon(IconName::Captions, icon_color))
                .child(ctrl_icon(IconName::Settings, icon_color))
                .child(ctrl_icon(IconName::Maximize, icon_color)),
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

/// 控制栏右侧胶囊（画质 / 倍速）。
pub(crate) fn ctrl_pill(label: String) -> impl IntoElement {
    div()
        .flex_shrink_0()
        .h(px(28.))
        .px(px(SPACE_2))
        .flex()
        .items_center()
        .rounded(px(RADIUS_HALF))
        .bg(rgba(0xFFFFFF1F))
        .cursor_pointer()
        .hover(|style| style.bg(rgba(0xFFFFFF33)))
        .text_size(px(FONT_SIZE_CAPTION))
        .text_color(hsla(0.0, 0.0, 1.0, 0.90))
        .child(label)
}

/// 控制栏右侧图标按钮（音量 / 字幕 / 设置 / 全屏）。
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

