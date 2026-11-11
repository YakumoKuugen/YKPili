//! 播放器渲染层：视频 layer + 自定义控制条 + 信息面板。
//!
//! 只读 `PlayerModel`，动作一律走 `controller`；不在这里改播放状态。

use super::*;

mod controls;
mod danmaku;
mod panel;
mod actions;
mod playback;
pub(crate) use actions::*;
mod tabs;
pub(crate) use tabs::{comment_skeleton, panel_empty_line};
pub(crate) use danmaku::{fixed_danmaku_visible, take_fixed_slot};
use controls::*;
pub(crate) use controls::VolumePanelView;
use crate::controller::*;

impl Render for PlayerView {
    fn render(&mut self, window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        self.sync_fullscreen(window, cx);
        let fullscreen = self.model.state.fullscreen;
        let lane_capacity = scroll_lane_capacity(window, fullscreen);
        if lane_capacity != self.scroll_lanes {
            self.scroll_lanes = lane_capacity;
            self.reflow_danmaku();
        }
        if !self.hover_watchdog_started {
            self.hover_watchdog_started = true;
            // 鼠标快速移出窗口时，GPUI 可能来不及派发 MouseMove / MouseExit。
            // 直接周期读取系统鼠标位置（CoreGraphics 的 CGEventGetLocation，
            // 即使指针已经在窗口外也会给出坐标），再走同一个 220ms 延迟收起。
            cx.spawn_in(window, async move |this, cx| loop {
                cx.background_executor()
                    .timer(std::time::Duration::from_millis(100))
                    .await;
                let alive = this.update_in(cx, |view, window, cx| {
                    // 系统全屏可能被绿色按钮 / 手势退出；暂停时没有播放帧，
                    // 靠这个 100ms 巡检把浮窗几何同步回来。
                    view.sync_fullscreen(window, cx);
                    if !view.expanded {
                        return;
                    }
                    let position = cursor_position(window, cx);
                    if view.pointer_in_panel_zone(position, window) {
                        // 已经回到 hover 区：取消可能排着的收起。
                        if view.panel_collapse_pending {
                            view.panel_collapse_pending = false;
                            view.panel_collapse_generation =
                                view.panel_collapse_generation.wrapping_add(1);
                        }
                        return;
                    }
                    // 系统光标已经在 hover 区外：这是真实移出，
                    // 不受「滚动后 400ms 保持」影响，只保留 220ms 延迟。
                    // 直接 arm，避免再用可能过期的 window.mouse_position() 判断一次。
                    view.panel_last_scroll = None;
                    view.arm_panel_collapse(window, cx);
                });
                if alive.is_err() {
                    break;
                }
            })
            .detach();
        }
        if self.needs_focus {
            self.needs_focus = false;
            let handle = self.focus_handle.clone();
            // 等这一帧的元素树挂上 track_focus 后再交焦点，避免首帧焦点丢失。
            cx.defer_in(window, move |_view, window, cx| {
                window.focus(&handle, cx);
            });
        }
        if self.submit_requested {
            self.submit_requested = false;
            cx.defer_in(window, |view, window, cx| view.send_danmaku(window, cx));
        }
        // 起播与播放循环都在 `view/playback.rs`：这里只按帧检查要不要启动。
        #[cfg(any(target_os = "macos", target_os = "windows"))]
        self.ensure_worker(cx);
        #[cfg(any(target_os = "macos", target_os = "windows"))]
        self.ensure_playback_loop(cx);

        // 弹幕输入框：首次 render 才有 window，所以在这里创建。
        // 用 gpui-component 的 Input —— 它内部走 InputHandler，
        // 支持 IME 组合（marked text），中文输入必需。
        if self.danmaku_input.is_none() {
            let state =
                cx.new(|cx| InputState::new(window, cx).placeholder("发个友善的弹幕见证当下"));
            // Enter 走 Input 自己的 Enter action（外层补了 key_context("Input") 才匹配），
            // 由它发出 PressEnter。Shift+Enter 不发送。
            // 中文输入法组合中的 Enter 应由输入法先消费（用于选词）；
            // 这条依赖平台 IME 行为，真机选词场景还要再验一次。
            cx.subscribe(&state, |view, _state, event: &InputEvent, cx| {
                if let InputEvent::PressEnter {
                    secondary: false,
                    shift: false,
                } = event
                {
                    eprintln!("[input] Enter → 发送");
                    view.submit_requested = true;
                    cx.notify();
                }
                if matches!(event, InputEvent::Blur) {
                    // 输入框失焦后不再阻止控制栏自动隐藏。
                    view.input_focused = false;
                    cx.notify();
                }
            })
            .detach();
            eprintln!("[input] InputState 已创建");
            self.danmaku_input = Some(state);
        }

        let t = self.theme.clone();
        // Windows：把最新一帧包成 GPUI 贴图，并回收上一张纹理。
        #[cfg(target_os = "windows")]
        self.sync_video_image(window);
        // 窗口失焦：立即收起，不依赖「下一帧」（暂停时没有视频帧驱动 render）。
        if !self.activation_observed {
            self.activation_observed = true;
            // 放到本帧之后注册：observe_window_activation 会立刻回调一次，
            // 直接在 render 里注册会造成重入。
            cx.defer_in(window, |view, window, cx| {
                view.activation_observer =
                    Some(cx.observe_window_activation(window, |view, window, cx| {
                        if !window.is_window_active() {
                            // 失焦时输入框也拿不到键盘，直接收起。
                            view.input_focused = false;
                            view.hide_controls(cx);
                        }
                    }));
            });
        }
        if !window.is_window_active()
            && self.model.controls_visible
            && !self.force_controls_visible
        {
            self.model.controls_visible = false;
            self.model.controls_timer_generation = self.model.controls_timer_generation.wrapping_add(1);
            self.model.controls_timer_running = false;
        }
        // 窗口可见性（最小化 / 被完全遮挡）：视频线程停帧，UI 也别再空转。
        //
        // 失焦**不**走这里：失焦时窗口仍然可见，视频必须照常按标准帧率播，
        // 否则「点了别的应用之后本应用变卡」。真正看不见了才降载。
        if !self.visibility_observed {
            self.visibility_observed = true;
            cx.defer_in(window, |view, window, cx| {
                view.visibility_observer =
                    Some(cx.observe_window_visibility(window, |view, visibility, _window, cx| {
                        view.set_window_visible(visibility.is_visible(), cx);
                    }));
            });
        }
        // 入场目标宽度跟随当前窗口；封面起点仍是点击时冻结的那一份。
        if let Some(source) = self.entrance_source.as_mut() {
            source.root_size = window.viewport_size();
        }
        // 控制区元素提前建好：可见时才挂树，隐藏时只留一条 3px 进度线。
        let watch_badge = self.model.controls_visible.then(|| self.watch_badge(cx));
        let bar: Option<AnyElement> = if self.model.controls_visible {
            let (video_left, video_width) = self.video_bar_geometry(window);
            Some(
                control_bar(
                    &t,
                    &self.model.state,
                    self.danmaku_input.as_ref(),
                    self.danmaku_sending,
                    self.source.as_ref(),
                    self.quality_switching,
                    self.quality_popover_open,
                    self.speed_popover_open,
                    video_left,
                    video_width,
                    &self.progress_track_bounds,
                    self.volume_popover_open,
                    &self.volume_panel,
                    &self.volume_button_bounds,
                    &self.quality_button_bounds,
                    &self.speed_button_bounds,
                    &self.control_popover_bounds,
                    cx,
                    cx.listener(|this, _: &ClickEvent, window, cx| {
                        this.toggle_play(window, cx);
                    }),
                )
                .into_any_element(),
            )
        } else {
            None
        };
        let mini_progress = (!self.model.controls_visible).then(|| self.mini_progress(&t, window, cx));
        // 弹幕只在「播放层」里跑：窗口态右侧面板占 200/400px；全屏没有面板，
        // 弹幕直接铺满整个窗口。
        let danmaku_width = self.playback_width(window);
        let panel_inset = if fullscreen { 0.0 } else { INFO_PANEL_COLLAPSED };
        // 浮窗内部：绝对铺满窗口。窗口本身负责圆角/裁剪/阴影/遮挡。
        let content = div()
            .id("player-content")
            .absolute()
            .inset_0()
            .bg(rgb(0x0B0F14))
            // ① 视频层：只占红框（播放层）宽度 = 窗口 − 右侧侧栏 200；
            //    侧栏是常驻占位，不算播放层。Contain 在这个红框里居中。
            .child(
                div()
                    .absolute()
                    .top_0()
                    .left_0()
                    .bottom_0()
                    .right(px(panel_inset))
                    .child({
                        #[cfg(target_os = "macos")]
                        {
                            video_layer(&self.frame, self.current.as_ref())
                        }
                        #[cfg(target_os = "windows")]
                        {
                            video_layer(&self.frame, self.current_image.as_ref())
                        }
                        #[cfg(not(any(target_os = "macos", target_os = "windows")))]
                        {
                            video_layer(&self.frame)
                        }
                    }),
            )
            // ①b 弹幕层：gpui-fast 已解决 surface 与文字的合成冲突，
            // 直接 canvas 绘制即可（见附录 D 事实 16）。
            // 左下角眼睛按钮关掉后整层不挂树。
            .children(if self.danmaku_visible {
                Some(self.danmaku_layer(danmaku_width))
            } else {
                None
            })
            // ①c 取流状态：加载中 / 失败说明，不遮挡控制栏与右侧面板
            .children(
                if self.loading_source {
                    Some("正在获取播放地址…".to_string())
                } else {
                    self.model.error.clone()
                }
                .map(|text| {
                    div()
                        .absolute()
                        .top_0()
                        .left_0()
                        .bottom_0()
                        .right(px(panel_inset))
                        .flex()
                        .items_center()
                        .justify_center()
                        .text_size(px(FONT_SIZE_CONTROL))
                        .text_color(rgb(0xFFFFFF))
                        .child(text)
                        .into_any_element()
                }),
            )
            // ①e 暂停态中央播放按钮
            .child(self.center_play(&t, cx))
            // ② 控制栏黑框 + 左下角标：只在鼠标活动时挂树；隐藏时留一条 3px 进度线。
            .children(watch_badge)
            // ③ 控制栏右端压在面板/视频边界上，仍保持可点击。
            .children(bar)
            .children(mini_progress)
            // ④ 信息面板：通高浮层，从播放页层顶一直到底（全屏时收起）
            .children((!fullscreen).then(|| self.info_panel(&t, window, cx)))
            // ⑤ 骑边界按钮：面板之后挂载 → 压在面板之上（GPUI 没有 z-index）
            .children((!fullscreen).then(|| self.panel_toggle(&t, cx)))
            // ⑥ 互动反馈 toast：窗口底部居中，浮在视频与右侧面板之上。
            .children(self.interaction_toast(&t))
            // 鼠标移动 / 点击都算活动：把控制栏叫回来并重新计时 5 秒。
            .on_mouse_move(cx.listener(|view, _: &MouseMoveEvent, window, cx| {
                view.note_activity(window, cx);
            }))
            .on_mouse_down(
                MouseButton::Left,
                cx.listener(|view, _: &MouseDownEvent, window, cx| {
                    view.note_activity(window, cx);
                }),
            )
            // 鼠标离开播放器（去了顶栏 / 页面 / 其它窗口）：控制栏直接收起。
            .on_hover(cx.listener(|view, hovered: &bool, window, cx| {
                if !*hovered {
                    // 用当前光标位置：`window.mouse_position()` 停在最后一次
                    // 事件上，鼠标已经移出窗口时它还是旧位置。
                    let position = cursor_position(window, cx);
                    // 播放中每帧重绘会让 hover 偶发 false；指针仍在播放器里就忽略。
                    if !view.pointer_in_player(position, window) {
                        // 指针真的离开播放器（含移进 16px 边距）：解除滚动保持，
                        // 让 220ms 延迟收起照常生效。
                        view.panel_last_scroll = None;
                        view.hide_controls(cx);
                        view.set_panel_hover(false, window, cx);
                    }
                }
            }))
            // 全局鼠标位置监听：不依赖 hover 命中，面板收/展以它为准。
            .child(self.pointer_tracker(cx))
            .into_any_element();

        // 浮窗外侧点击层：从顶栏下方开始，顶栏本身保持可用。
        let backdrop = div()
            .id("player-backdrop")
            .absolute()
            .top(px(TOP_BAR_HEIGHT))
            .left_0()
            .right_0()
            .bottom_0()
            // 极低透明度背景：确保这层一定参与命中测试（纯 alpha=0 的 div
            // 在部分路径下会被当成不绘制元素，点击直接落到下层封面卡）。
            .bg(rgba(0x00000001))
            // 挡住下层卡片/页面：点浮窗外侧只关闭，不触发新卡片。
            .occlude()
            .on_click(cx.listener(|this, _: &ClickEvent, window, cx| {
                // 关闭浮窗的这一拍不要再把 click 传给下层封面卡。
                cx.stop_propagation();
                this.request_close(window, cx);
            }));

        // 页面遮罩：入场时从透明压到 62%，把「旧页面」压暗；
        // 播放器内容同时在后段淡入，避免卡片放大结束的一瞬间硬切界面。
        // 关闭时反向淡出，页面重新亮起。
        let scrim = div()
            .absolute()
            .top(px(TOP_BAR_HEIGHT))
            .left_0()
            .right_0()
            // 底边停在播放器下沿，Dock 保持原亮度。
            .bottom(px(PLAYER_BOTTOM_INSET))
            .bg(rgb(0x000000));
        let scrim = if self.closing {
            scrim
                .with_animation(
                    format!("player-scrim-close-{}", self.closing_generation),
                    Animation::new(std::time::Duration::from_millis(PLAYER_EXIT_MS)),
                    |el, p| el.opacity(PLAYER_SCRIM_OPACITY * (1.0 - p)),
                )
                .into_any_element()
        } else {
            scrim
                .with_animation(
                    format!("player-scrim-{}", self.entrance_generation),
                    Animation::new(std::time::Duration::from_millis(PLAYER_ENTRANCE_MS)),
                    |el, p| el.opacity(PLAYER_SCRIM_OPACITY * p),
                )
                .into_any_element()
        };

        // 关闭缩回时只保留画面：当前视频帧优先，没有帧就用封面海报。
        // 控制栏/面板不挂树，窗口缩小时不会被重排得乱跳。
        let closing_visual: Option<AnyElement> = if self.closing {
            #[cfg(target_os = "macos")]
            {
                if self.current.is_some() {
                    Some(video_layer(&self.frame, self.current.as_ref()).into_any_element())
                } else {
                    self.entrance_source
                        .as_ref()
                        .map(|source| cover_poster(&t, source))
                }
            }
            #[cfg(target_os = "windows")]
            {
                if self.current_image.is_some() {
                    Some(video_layer(&self.frame, self.current_image.as_ref()).into_any_element())
                } else {
                    self.entrance_source
                        .as_ref()
                        .map(|source| cover_poster(&t, source))
                }
            }
            #[cfg(not(any(target_os = "macos", target_os = "windows")))]
            {
                self.entrance_source
                    .as_ref()
                    .map(|source| cover_poster(&t, source))
            }
        } else {
            None
        };

        // 卡片打开：用同一套窗口外壳，从点击时的封面几何展开到目标尺寸。
        // 动画进度来自稳定 id 的 Animation 时钟；render 里绝不新建起点。
        let window: AnyElement = if self.closing {
            let source = self
                .entrance_source
                .clone()
                .expect("关闭缩回必须有封面快照");
            let exit = PlayerExit {
                theme: t.clone(),
                source,
                visual: closing_visual,
                progress: 0.0,
            };
            exit.with_animation(
                format!("player-exit-{}", self.closing_generation),
                Animation::new(std::time::Duration::from_millis(PLAYER_EXIT_MS)).with_easing(|t| t),
                |el, progress| PlayerExit { progress, ..el },
            )
            .into_any_element()
        } else if let Some(source) = self.entrance_source.clone() {
            let entrance = PlayerEntrance {
                theme: t.clone(),
                source,
                content: Some(content),
                focus_handle: self.focus_handle.clone(),
                on_key: Rc::new(cx.listener(
                    |this, event: &KeyDownEvent, window, cx| {
                        this.handle_player_key(event, window, cx);
                    },
                )),
                fullscreen,
                progress: 0.0,
            };
            entrance
                .with_animation(
                    format!("player-entrance-{}", self.entrance_generation),
                    Animation::new(std::time::Duration::from_millis(PLAYER_ENTRANCE_MS))
                        .with_easing(|t| t),
                    |el, progress| PlayerEntrance { progress, ..el },
                )
                .into_any_element()
        } else {
            div()
                .id("player-window")
                .absolute()
                // 窗口态：顶在顶栏下方 16，底在 Dock 上方 20；
                // 全屏态：铺满整个窗口，去掉圆角 / 描边 / 阴影。
                .top(px(if fullscreen { 0.0 } else { TOP_BAR_HEIGHT + SPACE_4 }))
                .left(px(if fullscreen { 0.0 } else { SPACE_4 }))
                .right(px(if fullscreen { 0.0 } else { SPACE_4 }))
                .bottom(px(if fullscreen { 0.0 } else { PLAYER_BOTTOM_INSET }))
                .rounded(px(if fullscreen { 0.0 } else { CARD_RADIUS }))
                .overflow_hidden()
                .bg(rgb(0x0B0F14))
                .when(!fullscreen, |el| {
                    el.border_1()
                        .border_color(t.surfaces.border_popover)
                        .shadow(t.shadows.shadow_glass_modal.clone())
                })
                // 浮窗本体截断命中的穿透；子控件仍在更上层。
                .occlude()
                .key_context("Player")
                .track_focus(&self.focus_handle)
                .on_key_down(cx.listener(|this, event: &KeyDownEvent, window, cx| {
                    this.handle_player_key(event, window, cx);
                }))
                .child(content)
                .into_any_element()
        };

        let mut root = div().absolute().inset_0();
        if !fullscreen {
            root = root.child(backdrop).child(scrim);
        }
        root.child(window)
            // 第一次 Esc 提示放在浮窗之外：顶栏右侧。
            .children(self.escape_hint_overlay(&t, cx))
            // 评论图片查看器：压在播放器全部内容之上（含侧栏）。

    }
}

// ================= 弹幕层 =================


impl PlayerView {
    /// 当前时刻该显示的滚动弹幕。
    ///
    /// 每帧只为「可见窗口」内的弹幕生成元素，不是全量 —— 一场直播可能有上万条，
    /// 全量遍历会直接掉帧。窗口前半段留 15 秒是因为一条弹幕最长要飞这么久。

    /// 暂停态的中央播放按钮（设计稿的大圆按钮；播放中不挂树）。
    pub(crate) fn center_play(&self, t: &YkTheme, cx: &mut Context<Self>) -> AnyElement {
        if self.source.is_none() || self.model.state.playing {
            return div().into_any_element();
        }
        div()
            .absolute()
            .top_0()
            .left_0()
            .bottom_0()
            .right(px(if self.model.state.fullscreen {
                0.0
            } else {
                INFO_PANEL_COLLAPSED
            }))
            .flex()
            .items_center()
            .justify_center()
            .child(
                div()
                    .id("player-center-play")
                    .w(px(84.))
                    .h(px(84.))
                    .flex()
                    .items_center()
                    .justify_center()
                    .rounded_full()
                    .bg(rgba(0x0000006B))
                    .border_1()
                    .border_color(rgba(0xFFFFFF4D))
                    .cursor_pointer()
                    .hover(|style| style.bg(t.palette.theme_primary_60))
                    .on_click(cx.listener(|view, _: &ClickEvent, window, cx| {
                        view.toggle_play(window, cx);
                    }))
                    .child(
                        Icon::new(IconName::Play)
                            .size(px(34.))
                            .text_color(rgb(0xFFFFFF)),
                    ),
            )
            .into_any_element()
    }

    /// 控制栏隐藏时的简略态：底部 3px 进度线（40px 热区负责把控制栏叫回来）。
    pub(crate) fn mini_progress(&self, t: &YkTheme, window: &Window, cx: &mut Context<Self>) -> AnyElement {
        let progress = self.model.state.progress().clamp(0.0, 1.0) as f32;
        let (left, width) = self.video_bar_geometry(window);
        // 40px hover 热区仍铺满播放层，视觉上的 3px 进度线与视频同宽。
        div()
            .absolute()
            .left_0()
            .right_0()
            .bottom_0()
            .h(px(40.))
            .flex()
            .items_end()
            .bg(rgba(0x00000001))
            .on_mouse_move(cx.listener(|view, event: &MouseMoveEvent, window, cx| {
                view.note_pointer_move(event.position, window, cx);
            }))
            .child(
                div()
                    .absolute()
                    .left(px(left))
                    .bottom_0()
                    .w(px(width))
                    .h(px(3.))
                    .bg(rgba(0xFFFFFF2E))
                    .child(
                        div()
                            .h_full()
                            .w(relative(progress))
                            .bg(t.palette.theme_primary),
                    ),
            )
            .into_any_element()
    }

    /// 画面左下角：在线人数 + 弹幕开关 + 弹幕设置。
    pub(crate) fn watch_badge(&self, cx: &mut Context<Self>) -> AnyElement {
        if self.source.is_none() {
            return div().into_any_element();
        }
        let color = hsla(0.0, 0.0, 1.0, 0.88);
        let eye = if self.danmaku_visible {
            IconName::Eye
        } else {
            IconName::EyeOff
        };
        div()
            .absolute()
            .left(px(SPACE_4))
            .bottom(px(CTRL_H + 10.))
            .child(
                div()
                    .px(px(SPACE_2))
                    .py(px(3.))
                    .rounded(px(RADIUS_HALF))
                    .bg(rgba(0x00000073))
                    .flex()
                    .items_center()
                    .gap(px(SPACE_3))
                    .text_size(px(FONT_SIZE_CAPTION))
                    .text_color(color)
                    .child(div().child("1人正在看"))
                    .child(overlay_icon(
                        "player-danmaku-toggle",
                        eye,
                        color,
                        cx.listener(|view, _: &ClickEvent, _window, cx| {
                            view.danmaku_visible = !view.danmaku_visible;
                            cx.notify();
                        }),
                    ))
                    .child(overlay_icon(
                        "player-danmaku-settings",
                        IconName::Captions,
                        color,
                        cx.listener(|view, _: &ClickEvent, _window, cx| {
                            // 弹幕设置入口：展开面板并切到「弹幕」Tab。
                            view.tab = 1;
                            view.panel_collapse_generation =
                                view.panel_collapse_generation.wrapping_add(1);
                            if !view.expanded {
                                view.expanded = true;
                                view.panel_generation = view.panel_generation.wrapping_add(1);
                            }
                            cx.notify();
                        }),
                    )),
            )
            .into_any_element()
    }











    /// 分享面板开着时按回车 = 复制链接。
    ///
    /// 面板上「复制链接」行尾挂着一个回车键位提示，这个键位得是真的能按 ——
    /// 键盘可达是设计基线里的硬性底线（见 ux-design-preview 第 13.13 节）。
    pub(crate) fn handle_enter(&mut self, cx: &mut Context<Self>) -> bool {
        // 分享面板没开时不抢回车：输入框里的回车（发弹幕）不能被这里截走。
        if !self.share_panel_open || self.input_focused {
            return false;
        }
        self.copy_video_link(cx);
        true
    }





}



/// 复制链接的文本形态：`链接 + 换行 + 标题`。
///
/// 只贴一条裸链接，粘到聊天框里对方看不出是什么视频；带上标题一行，
/// 既认得出内容，链接本身也还能点开（和 B 站网页版复制的形态一致）。
/// 标题为空时退回裸链接，不留一个空行。
pub(crate) fn share_link_text(link: &str, title: &str) -> String {
    let title = title.trim();
    if title.is_empty() {
        link.to_string()
    } else {
        format!("{link}
{title}")
    }
}

/// 三个互动弹层。退场动画期间 bool 已经清零，靠它才知道该重画哪一张卡。
#[derive(Clone, Copy, PartialEq, Eq)]
pub(crate) enum ActionOverlay {
    Coin,
    Fav,
    Share,
}



/// 视频页面链接（分享用）。
pub(crate) fn video_page_url(bvid: &str) -> String {
    format!("https://www.bilibili.com/video/{}", bvid.trim())
}

/// 带分 P / 起播秒数的 B 站视频链接：`.../BV...?p=2&t=90`。
///
/// 分享进度走 B 站网页链接而不是 `ykpili://`：自定义协议在 QQ 里不是可点链接。
/// `p` / `t` 都是 B 站网页认的参数；装了 YKPili 用户脚本时，点开会把链接
/// 转成自有深链交给客户端，没装脚本也能在网页里从这一秒接着看。
pub(crate) fn video_page_url_at(bvid: &str, page: u32, start_secs: Option<f64>) -> String {
    let mut url = video_page_url(bvid);
    let mut query = Vec::new();
    if page > 1 {
        query.push(format!("p={page}"));
    }
    if let Some(seconds) = start_secs.filter(|seconds| seconds.is_finite() && *seconds > 0.0) {
        query.push(format!("t={}", seconds.round() as u64));
    }
    if !query.is_empty() {
        url.push('?');
        url.push_str(&query.join("&"));
    }
    url
}

/// 腾讯 QQ 分享卡片链接。
///
/// QQ 桌面版注册的 `tencent://` / `mqqapi://` 只能跳到指定聊天对象，
/// 没有「选好友分享」的入口；网页端的分享卡片才是腾讯官方给的那条路。
pub(crate) fn qq_share_url(link: &str, title: &str, desc: &str) -> String {
    format!(
        "https://connect.qq.com/widget/shareqq/index.html?url={}&title={}&desc={}&summary={}",
        percent_encode(link),
        percent_encode(title),
        percent_encode(desc),
        percent_encode(desc)
    )
}

/// 极简 percent encode：只保留 URL 里的安全字符，其余按字节转义。
pub(crate) fn percent_encode(text: &str) -> String {
    let mut out = String::with_capacity(text.len());
    for byte in text.as_bytes() {
        match byte {
            b'A'..=b'Z' | b'a'..=b'z' | b'0'..=b'9' | b'-' | b'_' | b'.' | b'~' => {
                out.push(*byte as char)
            }
            _ => out.push_str(&format!("%{byte:02X}")),
        }
    }
    out
}

/// 用系统默认程序打开链接（macOS 用 `open`，其它平台用 `xdg-open`）。
pub(crate) fn open_in_browser(url: &str) -> bool {
    #[cfg(target_os = "macos")]
    let opener = "open";
    #[cfg(not(target_os = "macos"))]
    let opener = "xdg-open";
    std::process::Command::new(opener)
        .arg(url)
        .stdout(std::process::Stdio::null())
        .stderr(std::process::Stdio::null())
        .spawn()
        .is_ok()
}





// ================= 视频层 =================

/// macOS：CVPixelBuffer 交给 GPUI 的 `surface()`，与控件同管线合成。
///
/// 其它平台（含 iOS）：`surface()` 在 gpui-pre 0.3.9 里只在 Apple 桌面端可用，
/// 这里先渲染占位。iOS 的真实链路（libmpv 解码 → 纹理）属于 M8 的后续工作，
/// 解码方案已定：**libmpv**（跨桌面 + 移动端同一套）。
#[cfg(target_os = "macos")]
pub(crate) fn video_layer(
    frame: &TestFrame,
    current: Option<&objc2_core_foundation::CFRetained<objc2_core_video::CVPixelBuffer>>,
) -> impl IntoElement {
    // 有真实帧就用真实的，否则退回 spike 的测试帧
    let buffer = match current {
        Some(b) => b.clone(),
        None => frame.buffer(),
    };
    // 等比 letterbox（L-2）：`surface()` 的 object_fit 默认就是 Contain。
    // 注意 surface 自身带有 CVPixelBuffer 的固有尺寸，直接当普通 child 会贴在
    // 左上角；这里用满尺寸 flex 容器把它居中，视频才真正落在播放器中心。
    div()
        .absolute()
        .inset_0()
        .flex()
        .items_center()
        .justify_center()
        .child(surface(buffer).object_fit(ObjectFit::Contain).size_full())
}

/// Windows：BGRA 帧包成 `RenderImage`，用 `img()` 贴到播放层。
///
/// 视频帧按 Contain 等比缩放；容器满尺寸并居中，和 macOS 的 `surface()`
/// 保持同一套 letterbox 行为。
#[cfg(target_os = "windows")]
pub(crate) fn video_layer(
    _frame: &TestFrame,
    current: Option<&std::sync::Arc<RenderImage>>,
) -> AnyElement {
    let content = match current {
        Some(image) => img(ImageSource::Render(image.clone()))
            .object_fit(ObjectFit::Contain)
            .size_full()
            .into_any_element(),
        None => div().size_full().bg(rgb(0x101014)).into_any_element(),
    };
    div()
        .absolute()
        .inset_0()
        .flex()
        .items_center()
        .justify_center()
        .child(content)
        .into_any_element()
}

/// 其它平台（含 iOS）：`surface()` 在 gpui-pre 0.3.9 里只在 Apple 桌面端可用，
/// Windows 走上面的贴图路径，其余平台先渲染占位。
#[cfg(not(any(target_os = "macos", target_os = "windows")))]
pub(crate) fn video_layer(_frame: &TestFrame) -> impl IntoElement {
    div().absolute().inset_0().bg(rgb(0x101014)).child(
        div()
            .absolute()
            .left(px(12.))
            .bottom(px(12.))
            .px(px(8.))
            .py(px(4.))
            .rounded(px(RADIUS_SM))
            .bg(rgba(0x000000AA))
            .text_size(px(FONT_SIZE_MICRO))
            .text_color(rgb(0xFFFFFF))
            .child("视频层占位 · 该平台待接 libmpv"),
    )
}

// ================= 控制栏 =================





impl crate::comment_section::CommentHost for PlayerView {
    fn comments(&mut self) -> &mut crate::comment_section::Section {
        &mut self.comment_section
    }

    fn comment_notice(&mut self, text: String, error: bool, cx: &mut Context<Self>) {
        self.show_interaction_note(text, error, cx);
    }

    fn comment_focus_root(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        window.focus(&self.focus_handle, cx);
    }
}
