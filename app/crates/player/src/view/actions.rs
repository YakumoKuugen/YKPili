use super::*;

impl PlayerView {
    /// 投币 / 分享浮层的宽度：固定 200（与互动按钮区同宽）。
    ///
    /// 不能跟着侧栏展开态变 —— 宽度一变，里面的按钮就会挪位，
    /// 用户刚展开就点会点空。
    pub(crate) fn action_panel_width(&self) -> f32 {
        INFO_PANEL_COLLAPSED
    }

    /// 三个互动弹层（投币 / 收藏夹 / 分享）的浮层容器。
    ///
    /// 走 `deferred + anchored` 而不是插进文档流：
    /// - **不占布局**：以前展开面板会把标签、评论区整段往下顶，收起时又跳回来；
    /// - **画在最上层**：GPUI 没有 z-index，`deferred` 让它在所有兄弟节点之后绘制；
    /// - **位置跟着互动行走**：bounds 由互动行 prepaint 时记录，滚动时下一帧自动跟上。
    pub(crate) fn action_overlay(
        &self,
        t: &YkTheme,
        window: &Window,
        cx: &mut Context<Self>,
    ) -> Option<AnyElement> {
        // 开着的那个优先；退场动画期间按缓存的 kind 继续画同一张卡。
        let kind = if self.fav_panel_open {
            ActionOverlay::Fav
        } else if self.coin_panel_open {
            ActionOverlay::Coin
        } else if self.share_panel_open {
            ActionOverlay::Share
        } else {
            self.overlay_closing_kind?
        };
        let closing = self.overlay_closing_kind.is_some();
        // 首帧还没量到互动行：这一帧先不画，下一帧就有位置了。
        let bounds = self.action_row_bounds.get()?;
        // 互动行滚出面板可视区后不再画：浮层是按窗口坐标挂的，
        // 不拦一下会跟着滚到视频画面上，看起来像凭空飘着一张卡。
        let viewport = window.viewport_size();
        let panel_top = px(TOP_BAR_HEIGHT + SPACE_4);
        let panel_bottom = viewport.height - px(PLAYER_BOTTOM_INSET);
        if bounds.origin.y + bounds.size.height < panel_top || bounds.origin.y > panel_bottom {
            return None;
        }
        let panel = match kind {
            ActionOverlay::Fav => self.fav_panel(t, cx),
            ActionOverlay::Coin => self.coin_panel(t, cx),
            ActionOverlay::Share => self.share_panel(t, cx),
        }?;

        // 入场：淡入 + 从上方 6px 落下来；退场反向。GPUI 没有 transform，
        // 位移用负 margin 做，卡片本身不带 margin，anchor 的定位不受影响。
        let animation = if closing {
            Animation::new(DURATION_FAST).with_easing(ease_standard())
        } else {
            Animation::new(DURATION_FAST).with_easing(ease_entrance())
        };
        let animated = div().child(panel).with_animation(
            format!(
                "player-overlay-{}-{}",
                if closing { "out" } else { "in" },
                self.overlay_generation
            ),
            animation,
            move |el, delta| {
                let progress = if closing { 1.0 - delta } else { delta };
                let progress = progress.clamp(0.0, 1.0);
                el.opacity(progress).mt(px(-6.0 * (1.0 - progress)))
            },
        );

        Some(
            deferred(
                // 整窗的点击层垫在浮层下面：点任何别的地方都先把浮层收起来。
                // 浮层卡片自己带 occlude，点卡片上的按钮不会被这层吃掉。
                anchored()
                    .position(point(px(0.), px(0.)))
                    .child(
                        div()
                            .id("player-overlay-catcher")
                            .w(viewport.width)
                            .h(viewport.height)
                            .occlude()
                            .on_mouse_down(
                                MouseButton::Left,
                                cx.listener(|view, _: &MouseDownEvent, _window, cx| {
                                    // 外点只负责关浮层，这一次点击不再落到下面的按钮上。
                                    view.close_action_overlays(cx);
                                    cx.stop_propagation();
                                }),
                            )
                            .child(
                                anchored()
                                    // 与面板文字列对齐（16）；下方放不下时自动翻到上方。
                                    .position(point(
                                        bounds.origin.x + px(SPACE_4),
                                        bounds.origin.y + bounds.size.height + px(SPACE_1),
                                    ))
                                    .snap_to_window_with_margin(px(SPACE_4))
                                    .child(animated),
                            ),
                    ),
            )
            .into_any_element(),
        )
    }

    /// 投币 / 分享这两个小浮层的壳：圆角卡片 + 细边 + 阴影。
    ///
    /// 宽度跟互动按钮区一样是 200：这两个面板只有两三个控件，
    /// 再宽反而和上面的按钮对不齐（收藏夹是清单，另走 368 的卡片）。
    pub(crate) fn action_popover(
        t: &YkTheme,
        id: &'static str,
        width: f32,
        content: AnyElement,
    ) -> AnyElement {
        let s = &t.surfaces;
        div()
            .id(id)
            .w(px(width))
            .px(px(SPACE_3))
            .py(px(SPACE_2))
            .rounded(px(RADIUS_LG))
            // 和收藏夹卡片共用同一档弹层表面：三个面板是一个家族，
            // 不能一个像卡片、另外两个像侧栏里多出来的一行。
            .bg(s.surface_popover)
            .border_1()
            .border_color(s.border_popover)
            .shadow(t.shadows.shadow_glass_elevated.clone())
            // 浮层是实心的：点空白处也不该穿透到下面的标签 / 评论。
            .occlude()
            .child(content)
            .into_any_element()
    }

    /// 投币面板：1 枚 / 2 枚 + 「同时点赞」。
    pub(crate) fn coin_panel(&self, t: &YkTheme, cx: &mut Context<Self>) -> Option<AnyElement> {
        // 退场动画期间 bool 已经清零，但这一帧还得把卡画出来。
        if !self.coin_panel_open && self.overlay_closing_kind != Some(ActionOverlay::Coin) {
            return None;
        }
        let s = &t.surfaces;
        let p = &t.palette;
        let busy = self.interaction_busy;
        let row = div()
            .flex()
            .items_center()
            .gap(px(SPACE_2))
            .child(
                div()
                    .id("player-coin-1")
                    .flex_1()
                    .h(px(30.))
                    .flex()
                    .items_center()
                    .justify_center()
                    .rounded(px(RADIUS_MD))
                    .bg(p.theme_primary)
                    .cursor_pointer()
                    .hover(|style| style.opacity(0.9))
                    .text_size(px(FONT_SIZE_CAPTION))
                    .font_weight(FontWeight::SEMIBOLD)
                    .text_color(rgb(0xFFFFFF))
                    .child("投 1 枚")
                    .when(busy, |el| el.opacity(0.5))
                    .on_click(cx.listener(|view, _: &ClickEvent, _window, cx| {
                        let also_like = view.coin_also_like;
                        view.do_coin(1, also_like, cx);
                    })),
            )
            .child(
                div()
                    .id("player-coin-2")
                    .flex_1()
                    .h(px(30.))
                    .flex()
                    .items_center()
                    .justify_center()
                    .rounded(px(RADIUS_MD))
                    .bg(s.fill_3)
                    .cursor_pointer()
                    .hover(|style| style.opacity(0.9))
                    .text_size(px(FONT_SIZE_CAPTION))
                    .font_weight(FontWeight::SEMIBOLD)
                    .text_color(s.text_primary)
                    .child("投 2 枚")
                    .when(busy, |el| el.opacity(0.5))
                    .on_click(cx.listener(|view, _: &ClickEvent, _window, cx| {
                        let also_like = view.coin_also_like;
                        view.do_coin(2, also_like, cx);
                    })),
            );
        let also_like = div()
            .id("player-coin-also-like")
            .mt(px(SPACE_2))
            .flex()
            .items_center()
            .gap(px(SPACE_2))
            .cursor_pointer()
            .text_size(px(FONT_SIZE_MICRO))
            .text_color(s.text_secondary)
            .child(
                div()
                    .w(px(14.))
                    .h(px(14.))
                    .rounded(px(RADIUS_HALF))
                    .border_1()
                    .border_color(if self.coin_also_like {
                        p.theme_primary
                    } else {
                        s.border_strong
                    })
                    .bg(if self.coin_also_like {
                        p.theme_primary
                    } else {
                        s.fill_3
                    }),
            )
            .child("同时点赞")
            .on_click(cx.listener(|view, _: &ClickEvent, _window, cx| {
                view.coin_also_like = !view.coin_also_like;
                cx.notify();
            }));
        Some(Self::action_popover(
            t,
            "player-coin-panel",
            self.action_panel_width(),
            div().child(row).child(also_like).into_any_element(),
        ))
    }

    /// 收藏夹面板：列出自己的收藏夹，点一下收藏 / 取消收藏。
    ///
    /// 按「多选清单」重做（设计 §13.3 弹出层 / §13.7 勾选）：
    /// - 卡片抬一层：`surface_popover` + `border_popover` + 悬浮阴影，
    ///   压在侧栏内容之上读，不再和普通列表行糊成一片；
    /// - 行高 32（与菜单项同规格），左侧勾选圈表示是否已收藏，数量右对齐。
    ///   原先把「标题 / 数量 / 收藏」三列硬塞进 200 宽，标题和动作词互相裁切；
    /// - 行内不再写「收藏 / 已收藏」：勾选圈 + 底色已经说明状态，
    ///   重复一遍动作词只会挤掉标题；
    /// - 超过一屏先折叠，别把标签 / 评论整个顶出视野；加载 / 空 / 失败三态齐备。
    pub(crate) fn fav_panel(&self, t: &YkTheme, cx: &mut Context<Self>) -> Option<AnyElement> {
        // 退场动画期间 bool 已经清零，但这一帧还得把卡画出来。
        if !self.fav_panel_open && self.overlay_closing_kind != Some(ActionOverlay::Fav) {
            return None;
        }
        let s = &t.surfaces;
        let p = &t.palette;
        let busy = self.interaction_busy;
        let selected = self
            .fav_folders
            .iter()
            .filter(|folder| folder.contains_video())
            .count();

        let close = div()
            .id("player-fav-close")
            .w(px(22.))
            .h(px(22.))
            .flex_shrink_0()
            .flex()
            .items_center()
            .justify_center()
            .rounded(px(RADIUS_HALF))
            .cursor_pointer()
            .hover(|style| style.bg(s.fill_3))
            .child(
                Icon::new(IconName::X)
                    .size(px(ICON_SIZE_SM - 2.))
                    .text_color(s.text_tertiary),
            )
            .on_click(cx.listener(|view, _: &ClickEvent, _window, cx| {
                view.fav_panel_open = false;
                cx.notify();
            }));

        let header = div()
            .flex()
            .items_center()
            .gap(px(SPACE_2))
            .h(px(32.))
            .px(px(SPACE_2))
            .child(
                div()
                    .text_size(px(FONT_SIZE_CONTROL))
                    .font_weight(FontWeight::SEMIBOLD)
                    .text_color(s.text_primary)
                    .child("收藏夹"),
            )
            .child(div().flex_1())
            .children((selected > 0).then(|| {
                div()
                    .text_size(px(FONT_SIZE_MICRO))
                    .text_color(p.theme_primary)
                    .child(format!("已收藏 {selected} 个"))
            }))
            .child(close);

        let divider = div().h(px(1.)).w_full().bg(s.border_subtle);

        let body: AnyElement = if self.fav_loading && self.fav_folders.is_empty() {
            // 骨架行按真实行高占位：数据回来时面板不会「跳一下」。
            div()
                .flex()
                .flex_col()
                .gap(px(SPACE_1))
                .py(px(SPACE_1))
                .children((0..3).map(|_| {
                    div()
                        .h(px(32.))
                        .w_full()
                        .rounded(px(RADIUS_MD))
                        .bg(s.skeleton_brush)
                        .opacity(0.6)
                        .into_any_element()
                }))
                .into_any_element()
        } else if let Some(error) = self
            .fav_error
            .as_ref()
            .filter(|_| self.fav_folders.is_empty())
        {
            div()
                .flex()
                .flex_col()
                .items_center()
                .gap(px(SPACE_1))
                .py(px(SPACE_4))
                .child(
                    div()
                        .text_size(px(FONT_SIZE_CAPTION))
                        .text_color(s.text_secondary)
                        .child("收藏夹没加载出来"),
                )
                .child(
                    div()
                        .max_w(px(280.))
                        .truncate()
                        .text_size(px(FONT_SIZE_MICRO))
                        .text_color(s.text_quaternary)
                        .child(error.clone()),
                )
                .child(
                    div()
                        .id("player-fav-retry")
                        .mt(px(SPACE_1))
                        .px(px(SPACE_3))
                        .h(px(28.))
                        .flex()
                        .items_center()
                        .rounded(px(RADIUS_HALF))
                        .cursor_pointer()
                        .bg(p.theme_primary_10)
                        .hover(|style| style.bg(p.theme_primary_20))
                        .text_size(px(FONT_SIZE_CAPTION))
                        .text_color(p.theme_primary)
                        .child("重试")
                        .on_click(cx.listener(|view, _: &ClickEvent, _window, cx| {
                            view.load_fav_folders(cx);
                        })),
                )
                .into_any_element()
        } else if self.fav_folders.is_empty() {
            div()
                .flex()
                .flex_col()
                .items_center()
                .gap(px(SPACE_1))
                .py(px(SPACE_5))
                .child(
                    Icon::new(IconName::FolderOpen)
                        .size(px(ICON_SIZE_LG))
                        .text_color(s.text_quaternary),
                )
                .child(
                    div()
                        .text_size(px(FONT_SIZE_CAPTION))
                        .text_color(s.text_secondary)
                        .child("还没有收藏夹"),
                )
                .child(
                    div()
                        .text_size(px(FONT_SIZE_MICRO))
                        .text_color(s.text_quaternary)
                        .child("在网页版建好收藏夹后会同步到这里"),
                )
                .into_any_element()
        } else {
            let total = self.fav_folders.len();
            let shown = if self.fav_list_expanded {
                total
            } else {
                total.min(FAV_PANEL_VISIBLE_ROWS)
            };
            let mut list = div().flex().flex_col().gap(px(SPACE_1)).py(px(SPACE_1));
            for folder in self.fav_folders.iter().take(shown) {
                list = list.child(Self::fav_row(t, folder, busy, cx));
            }
            let mut stack = div().flex().flex_col().child(list);
            if total > FAV_PANEL_VISIBLE_ROWS {
                let expanded = self.fav_list_expanded;
                stack = stack.child(
                    div()
                        .id("player-fav-more")
                        .h(px(28.))
                        .w_full()
                        .flex()
                        .items_center()
                        .justify_center()
                        .gap(px(SPACE_1))
                        .rounded(px(RADIUS_HALF))
                        .cursor_pointer()
                        .hover(|style| style.bg(s.fill_3))
                        .text_size(px(FONT_SIZE_MICRO))
                        .text_color(s.text_tertiary)
                        .child(if expanded {
                            "收起".to_string()
                        } else {
                            format!("展开全部 {total} 个")
                        })
                        .child(
                            Icon::new(if expanded {
                                IconName::ChevronUp
                            } else {
                                IconName::ChevronDown
                            })
                            .size(px(12.))
                            .text_color(s.text_quaternary),
                        )
                        .on_click(cx.listener(|view, _: &ClickEvent, _window, cx| {
                            view.fav_list_expanded = !view.fav_list_expanded;
                            cx.notify();
                        })),
                );
            }
            stack.into_any_element()
        };

        Some(
            div()
                .id("player-fav-panel")
                .w(px(PLAYER_INFO_PANEL_WIDTH - SPACE_4 * 2.0))
                .p(px(SPACE_1))
                .flex()
                .flex_col()
                .rounded(px(RADIUS_XL))
                .bg(s.surface_popover)
                .border_1()
                .border_color(s.border_popover)
                .shadow(t.shadows.shadow_glass_elevated.clone())
                .occlude()
                .child(header)
                .child(divider)
                // 列表超过一屏时在卡片内部滚动：收藏夹可能有几十个，
                // 全量展开会把浮层撑出窗口，滚轮 / 触控板都够不到下面的项。
                .child(
                    div()
                        .id("player-fav-scroll")
                        .w_full()
                        .max_h(px(FAV_PANEL_MAX_HEIGHT))
                        .overflow_y_scroll()
                        .track_scroll(&self.fav_scroll)
                        .child(body),
                )
                .into_any_element(),
        )
    }

    /// 收藏夹行：左侧勾选圈 + 标题 + 右侧数量，整行可点，点一下加 / 取消。
    ///
    /// 勾选圈用形状（实心对勾 / 空心圆）而不是只靠颜色区分选中态，
    /// 色觉障碍用户也能看出来哪几个夹子已经收藏。
    pub(crate) fn fav_row(
        t: &YkTheme,
        folder: &yk_infra::FavFolder,
        busy: bool,
        cx: &mut Context<Self>,
    ) -> AnyElement {
        let s = &t.surfaces;
        let p = &t.palette;
        let selected = folder.contains_video();
        let folder_id = folder.id;
        let hover_bg = if selected {
            p.theme_primary_20
        } else {
            s.fill_3
        };
        div()
            .id(SharedString::from(format!("player-fav-folder-{folder_id}")))
            .h(px(32.))
            .w_full()
            .flex()
            .items_center()
            .gap(px(SPACE_2))
            .px(px(SPACE_2))
            .rounded(px(RADIUS_MD))
            .cursor_pointer()
            .text_size(px(FONT_SIZE_CONTROL))
            .when(selected, |el| el.bg(p.theme_primary_10))
            .when(!busy, |el| el.hover(move |style| style.bg(hover_bg)))
            .when(busy, |el| el.opacity(0.55))
            .child(
                div()
                    .w(px(14.))
                    .h(px(14.))
                    .flex_shrink_0()
                    .flex()
                    .items_center()
                    .justify_center()
                    .rounded_full()
                    .when(selected, |el| {
                        el.bg(p.theme_primary)
                            .border_1()
                            .border_color(p.theme_primary)
                    })
                    .when(!selected, |el| el.border_1().border_color(s.border_strong))
                    .children(selected.then(|| {
                        Icon::new(IconName::Check)
                            .size(px(10.))
                            .text_color(rgb(0xFFFFFF))
                    })),
            )
            .child(
                div()
                    .flex_1()
                    .min_w(px(0.))
                    .truncate()
                    .text_color(if selected {
                        s.text_primary
                    } else {
                        s.text_secondary
                    })
                    .child(folder.title.clone()),
            )
            .child(
                div()
                    .flex_shrink_0()
                    .text_size(px(FONT_SIZE_MICRO))
                    .text_color(s.text_tertiary)
                    .child(fav_count_label(folder.media_count)),
            )
            .on_click(cx.listener(move |view, _: &ClickEvent, _window, cx| {
                view.do_fav(folder_id, !selected, cx);
            }))
            .into_any_element()
    }

    /// 分享面板：哔哩哔哩动态 / QQ / 复制链接 / 分享播放进度。
    pub(crate) fn share_panel(&self, t: &YkTheme, cx: &mut Context<Self>) -> Option<AnyElement> {
        // 退场动画期间 bool 已经清零，但这一帧还得把卡画出来。
        if !self.share_panel_open && self.overlay_closing_kind != Some(ActionOverlay::Share) {
            return None;
        }
        let s = &t.surfaces;
        let title = self
            .source
            .as_ref()
            .map(|source| source.title.clone())
            .unwrap_or_default();
        let progress = format::duration(self.current_position_secs().round() as u64);
        let list = div()
            .flex()
            .flex_col()
            .gap(px(SPACE_1))
            .child(
                share_row(
                    "player-share-dynamic",
                    Icon::new(IconName::Share)
                        .size(px(ICON_SIZE_SM))
                        .text_color(s.text_secondary)
                        .into_any_element(),
                    "分享到哔哩哔哩动态",
                    s.fill_3,
                    s.text_primary,
                )
                .on_click(cx.listener(|view, _: &ClickEvent, _window, cx| {
                    view.share_to_dynamic(cx);
                })),
            )
            .child(
                share_row(
                    "player-share-qq",
                    div()
                        .w(px(16.))
                        .h(px(16.))
                        .rounded(px(RADIUS_HALF))
                        .bg(rgb(0x12B7F5))
                        .flex()
                        .items_center()
                        .justify_center()
                        .text_size(px(9.))
                        .font_weight(FontWeight::BOLD)
                        .text_color(rgb(0xFFFFFF))
                        .child("Q")
                        .into_any_element(),
                    "分享到 QQ",
                    s.fill_3,
                    s.text_primary,
                )
                .on_click(cx.listener(|view, _: &ClickEvent, _window, cx| {
                    view.share_to_qq(cx);
                })),
            )
            .child(
                share_row(
                    "player-share-copy",
                    Icon::new(IconName::Link)
                        .size(px(ICON_SIZE_SM))
                        .text_color(s.text_secondary)
                        .into_any_element(),
                    "复制链接",
                    s.fill_3,
                    s.text_primary,
                )
                // 回车键位提示：分享面板开着时回车 = 复制链接。
                .child(div().flex_1().min_w(px(0.)))
                .child(enter_key_hint())
                .on_click(cx.listener(|view, _: &ClickEvent, _window, cx| {
                    view.copy_video_link(cx);
                })),
            )
            .child(
                share_row(
                    "player-share-progress",
                    Icon::new(IconName::Timer)
                        .size(px(ICON_SIZE_SM))
                        .text_color(s.text_secondary)
                        .into_any_element(),
                    "分享播放进度",
                    s.fill_3,
                    s.text_primary,
                )
                // 行尾挂着要分享的时间点，点之前就知道对方从哪开始看。
                .child(div().flex_1().min_w(px(0.)))
                .child(
                    div()
                        .text_size(px(FONT_SIZE_MICRO))
                        .text_color(s.text_quaternary)
                        .child(progress),
                )
                .on_click(cx.listener(|view, _: &ClickEvent, _window, cx| {
                    view.copy_progress_link(cx);
                })),
            );
        let head = if title.is_empty() {
            div().into_any_element()
        } else {
            div()
                .pb(px(SPACE_1))
                .text_size(px(FONT_SIZE_MICRO))
                .text_color(s.text_quaternary)
                .line_clamp(2)
                .child(title)
                .into_any_element()
        };
        Some(Self::action_popover(
            t,
            "player-share-panel",
            self.action_panel_width(),
            div().child(head).child(list).into_any_element(),
        ))
    }

    /// 互动反馈 toast：窗口底部居中的小胶囊，成功 / 失败共用。
    ///
    /// 浮在播放层与右侧面板之上，不参与面板排版，所以复制 / 点赞后
    /// 按钮不会因为提示出现而被顶开。位置抬到控制栏之上，
    /// 控制栏显示或隐藏时都不会压住它。
    ///
    /// 缩放用「字号 / 内边距 / 高度同比插值」实现：这一版 GPUI 没有
    /// 元素级 transform，等比改这几个尺寸在视觉上就是整体缩放。
    pub(crate) fn interaction_toast(&self, t: &YkTheme) -> Option<AnyElement> {
        let (text, is_error) = self.interaction_note.clone()?;
        let s = &t.surfaces;
        let closing = self.interaction_note_closing;
        let scale_from = TOAST_SCALE_FROM;
        // 入场 / 退场各用一个独立的动画 id：切到退场时动画从头开始，
        // 但首帧 scale 正好是 1，和入场结束态无缝衔接。
        let (animation_id, animation) = if closing {
            (
                format!("player-toast-out-{}", self.interaction_note_generation),
                Animation::new(DURATION_FAST).with_easing(ease_standard()),
            )
        } else {
            (
                format!("player-toast-in-{}", self.interaction_note_generation),
                Animation::new(DURATION_FAST).with_easing(ease_entrance()),
            )
        };
        let pill = status::toast_pill(t, text)
            .text_color(if is_error {
                t.palette.status_error
            } else {
                s.text_secondary
            })
            .with_animation(animation_id, animation, move |el, delta| {
                let factor = if closing {
                    1.0 - (1.0 - scale_from) * delta
                } else {
                    scale_from + (1.0 - scale_from) * delta
                };
                let opacity = if closing { 1.0 - delta } else { delta };
                el.max_w(px(status::TOAST_MAX_WIDTH * factor))
                    .min_h(px(status::TOAST_HEIGHT * factor))
                    .px(px(status::TOAST_PADDING_X * factor))
                    .py(px(SPACE_2 * factor))
                    .text_size(px(FONT_SIZE_BODY * factor))
                    .opacity(opacity.clamp(0.0, 1.0))
            });
        Some(
            div()
                .absolute()
                .left_0()
                .right_0()
                // 贴近控制栏上沿：比旧版再往下 12px，但仍留 4px 间隙，
                // 不会盖住弹幕输入 / 发送按钮。
                .bottom(px(CTRL_H + SPACE_1))
                .flex()
                .justify_center()
                .child(pill)
                .into_any_element(),
        )
    }
}

/// 左下角浮层的小图标按钮（24 热区，16 图标）。
pub(crate) fn overlay_icon(
    id: &'static str,
    icon: IconName,
    color: Hsla,
    on_click: impl Fn(&ClickEvent, &mut Window, &mut App) + 'static,
) -> impl IntoElement {
    div()
        .id(id)
        .w(px(24.))
        .h(px(24.))
        .flex()
        .items_center()
        .justify_center()
        .rounded_full()
        .cursor_pointer()
        .hover(|style| style.bg(rgba(0xFFFFFF24)))
        .child(Icon::new(icon).size(px(ICON_SIZE_SM)).text_color(color))
        .on_click(on_click)
}

/// 分享面板的一行：16px 图标 + 文案（超长截断）。
///
/// 返回 `Stateful<Div>`，调用方可以继续链 `.child()` 挂尾部提示、再挂 `.on_click`。
/// 颜色按值传入而不是借 `&YkTheme`：hover 闭包要 `'static`，
/// 借引用会让整个元素被主题生命周期绑住。
fn share_row(
    id: &'static str,
    icon: AnyElement,
    label: &'static str,
    hover: Hsla,
    text: Hsla,
) -> Stateful<Div> {
    div()
        .id(id)
        // 测试按 id 取这一行的实际几何：分享面板是 deferred 浮层，
        // 行坐标不能像普通文档流那样手算。
        .debug_selector(move || id.to_string())
        .h(px(30.))
        .flex()
        .items_center()
        .gap(px(SPACE_2))
        .px(px(SPACE_2))
        .rounded(px(RADIUS_SM))
        .cursor_pointer()
        .hover(move |style| style.bg(hover))
        .text_size(px(FONT_SIZE_CAPTION))
        .text_color(text)
        .child(icon)
        .child(div().min_w(px(0.)).truncate().child(label))
}

/// 元信息行的一项：12px 图标 + 计数。
pub(crate) fn meta_stat(icon: IconName, text: String) -> impl IntoElement {
    div()
        .flex()
        .items_center()
        .gap(px(3.))
        .child(Icon::new(icon).size(px(12.)))
        .child(text)
}

/// 收藏夹条数文案：四位以内给「N 个」，过万用「万」单位，别让数字把标题挤掉。
pub(crate) fn fav_count_label(count: u64) -> String {
    if count >= 10_000 {
        format::count(count)
    } else {
        format!("{count} 个")
    }
}

/// 互动行的一项：图标在上、计数/文案在下。
/// 互动按钮：图标 + 数字；已经互动过（点赞 / 投币 / 收藏）用主色高亮。
pub(crate) fn panel_action_button(
    t: &YkTheme,
    id: &'static str,
    icon: IconName,
    label: String,
    active: bool,
    on_click: impl Fn(&ClickEvent, &mut Window, &mut App) + 'static,
) -> impl IntoElement {
    let s = &t.surfaces;
    let p = &t.palette;
    let color = if active {
        p.theme_primary
    } else {
        s.text_tertiary
    };
    div()
        // id 必须每个按钮都不同：GPUI 用元素 id 做事件索引，
        // 四个按钮共用一个 id 会互相覆盖，点击落不到对应的回调上。
        .id(id)
        .flex()
        .flex_col()
        .items_center()
        .gap(px(2.))
        .px(px(SPACE_2))
        .py(px(SPACE_1))
        .rounded(px(RADIUS_MD))
        .cursor_pointer()
        .hover(|style| style.bg(s.fill_2))
        .text_size(px(FONT_SIZE_MICRO))
        .text_color(color)
        .child(Icon::new(icon).size(px(ICON_SIZE_SM)).text_color(color))
        .child(label)
        .on_click(on_click)
}
