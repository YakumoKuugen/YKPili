use super::*;

impl PlayerView {
    /// 骑在面板左边界上的圆形按钮：展开态 ×、收起态 ‹。
    pub(crate) fn panel_toggle(&self, t: &YkTheme, cx: &mut Context<Self>) -> AnyElement {
        // 骑在贴右边界的面板左边界上：right = 面板宽度 - 一半按钮。
        let to_right = self.panel_width() - 16.;
        let from_right = if self.panel_generation == 0 {
            to_right
        } else if self.expanded {
            INFO_PANEL_COLLAPSED - 16.
        } else {
            INFO_PANEL_EXPANDED - 16.
        };
        div()
            .id("player-panel-toggle")
            .absolute()
            .top(px(SPACE_2))
            .right(px(to_right))
            .w(px(32.))
            .h(px(32.))
            .flex()
            .items_center()
            .justify_center()
            .rounded_full()
            .bg(rgba(0x14161AB8))
            .border_1()
            .border_color(t.surfaces.border_popover)
            .cursor_pointer()
            .hover(|style| style.bg(palette().theme_primary))
            // 按钮不再挂 on_hover：展开 / 收起由真实指针坐标驱动，
            // 这里只保留点击切换，避免播放中 hover 重算把面板反复收起。
            .on_click(cx.listener(|view, _: &ClickEvent, _window, cx| {
                view.expanded = !view.expanded;
                view.panel_collapse_generation = view.panel_collapse_generation.wrapping_add(1);
                view.panel_generation = view.panel_generation.wrapping_add(1);
                view.panel_hover_grace_until = if view.expanded {
                    None
                } else {
                    Some(std::time::Instant::now() + std::time::Duration::from_millis(400))
                };
                cx.notify();
            }))
            .child(
                Icon::new(if self.expanded {
                    IconName::X
                } else {
                    IconName::ChevronLeft
                })
                .size(px(ICON_SIZE_SM))
                .text_color(rgb(0xFFFFFF)),
            )
            .with_animation(
                format!("player-panel-btn-{}", self.panel_generation),
                Animation::new(std::time::Duration::from_millis(200)).with_easing(ease_standard()),
                move |el, delta| el.right(px(from_right + (to_right - from_right) * delta)),
            )
            .into_any_element()
    }

    /// 面板 Tab：评论 / 弹幕 / 选集。「选集」只在多 P 时出现。
    pub(crate) fn panel_tabs(&self) -> Vec<&'static str> {
        let multi = self
            .source
            .as_ref()
            .map(|source| source.pages.len() > 1)
            .unwrap_or(false);
        if multi {
            vec!["评论", "弹幕", "选集"]
        } else {
            vec!["评论", "弹幕"]
        }
    }

    /// 信息面板：标题 → 元信息 → UP 主 → 互动 → 标签 → Tab → 内容。
    ///
    /// 内容按 400px 排版，收起态只把右半裁掉（D-05：收起仍是 200px 半宽）。
    pub(crate) fn info_panel(&mut self, t: &YkTheme, window: &mut Window, cx: &mut Context<Self>) -> AnyElement {
        // 头像探针的句柄先取出来，避免在元素链里再借 self
        let avatar_bounds = self.up_avatar_bounds.clone();
        let scale_factor = window.scale_factor();
        let s = &t.surfaces;
        let p = &t.palette;
        let source = self.source.as_ref();

        let title = match source {
            Some(source) if !source.title.is_empty() => source.title.clone(),
            _ if self.loading_source => "正在获取播放地址…".to_string(),
            _ if self.model.error().is_some() => "播放失败".to_string(),
            _ => "视频信息".to_string(),
        };

        let head = div()
            .flex()
            .items_start()
            .pt(px(SPACE_3))
            .pb(px(SPACE_2))
            .pl(px(28.))
            .pr(px(SPACE_3))
            .child(
                div()
                    .flex_1()
                    .min_w(px(0.))
                    .line_height(px(LINE_HEIGHT_TITLE))
                    .text_size(px(FONT_SIZE_BODY))
                    .font_weight(FontWeight::SEMIBOLD)
                    .text_color(s.text_primary)
                    .line_clamp(2)
                    .child(title),
            );

        let meta = source.map(|source| {
            div()
                .flex()
                .items_center()
                .gap(px(SPACE_4))
                .px(px(SPACE_4))
                .pb(px(SPACE_3))
                .text_size(px(FONT_SIZE_CAPTION))
                .text_color(s.text_quaternary)
                .child(meta_stat(IconName::Play, format::count(source.view_count)))
                .child(meta_stat(
                    IconName::MessageCircle,
                    format::count(source.danmaku_count),
                ))
                .when(source.pubdate > 0, |el| {
                    el.child(div().child(format::date_time(source.pubdate)))
                })
        });

        let up = source.map(|source| {
            let face_url = if source.owner_face.is_empty() {
                String::new()
            } else {
                asset_url(&source.owner_face, fitted_width(46.0, scale_factor, 0))
            };
            let name = if source.owner_name.is_empty() {
                "未知 UP 主".to_string()
            } else {
                source.owner_name.clone()
            };
            div()
                .px(px(SPACE_4))
                .pb(px(SPACE_3))
                .child(
                    div()
                        .flex()
                        .items_start()
                        .gap(px(SPACE_3))
                        .child(
                            // 点头像 = 打开 UP 主浮窗（12.9 节）。头像本身是视觉焦点，
                            // 顺手把「这人是谁」做成一跳可达，不打断正在播的视频。
                            div()
                                .id("player-up-avatar")
                                .relative()
                                // 探针：记下头像的屏幕 bounds，浮窗从它长出来
                                .child(
                                    canvas(
                                        {
                                            let sink = avatar_bounds.clone();
                                            move |bounds, _window, _cx| sink.set(Some(bounds))
                                        },
                                        |_, _, _, _| {},
                                    )
                                    .absolute()
                                    .inset_0(),
                                )
                                .w(px(46.))
                                .h(px(46.))
                                .flex_shrink_0()
                                .rounded_full()
                                .overflow_hidden()
                                .cursor_pointer()
                                .hover(|style| style.opacity(0.88))
                                .child(
                                    yk_components::avatar::Avatar::new(46.0)
                                        .url(face_url.clone())
                                        .background(linear_gradient(
                                            135.,
                                            linear_color_stop(p.bili_blue, 0.0),
                                            linear_color_stop(p.bili_pink, 1.0),
                                        ))
                                        .build(),
                                )
                                .on_click(cx.listener(|view, _: &ClickEvent, window, cx| {
                                    cx.stop_propagation();
                                    let Some(source) = view.source.as_ref() else {
                                        return;
                                    };
                                    let mid = source.owner_mid;
                                    if mid <= 0 {
                                        view.show_interaction_note(
                                            "没拿到 UP 主 uid，无法打开空间".into(),
                                            true,
                                            cx,
                                        );
                                        return;
                                    }
                                    let name = source.owner_name.clone();
                                    let face = source.owner_face.clone();
                                    let opener = view.up_avatar_bounds.get().unwrap_or_else(|| {
                                        Bounds {
                                            origin: point(px(0.0), px(TOP_BAR_HEIGHT)),
                                            size: size(px(46.0), px(46.0)),
                                        }
                                    });
                                    if let Some(handler) = view.up_profile_handler.as_ref() {
                                        handler(cx, window, mid, name, face, opener);
                                    }
                                })),
                        )
                        .child(
                            div()
                                .flex_1()
                                .min_w(px(0.))
                                .flex()
                                .flex_col()
                                .gap(px(2.))
                                .child(
                                    div()
                                        .flex()
                                        .items_center()
                                        .gap(px(SPACE_2))
                                        .child(
                                            div()
                                                .min_w(px(0.))
                                                .truncate()
                                                .text_size(px(FONT_SIZE_CONTROL))
                                                .font_weight(FontWeight::SEMIBOLD)
                                                .text_color(s.text_primary)
                                                .child(name),
                                        )
                                        .child(
                                            div()
                                                .px(px(6.))
                                                .h(px(18.))
                                                .flex()
                                                .items_center()
                                                .rounded(px(BADGE_RADIUS))
                                                .border_1()
                                                .border_color(s.border_strong)
                                                .text_size(px(FONT_SIZE_MICRO))
                                                .text_color(s.text_quaternary)
                                                .child("发消息"),
                                        ),
                                )
                                // UP 主自己的签名放在昵称正下方，和视频简介分开。
                                .when(!source.owner_sign.trim().is_empty(), |el| {
                                    el.child(
                                        div()
                                            .mt(px(2.))
                                            .line_height(px(16.))
                                            .text_size(px(FONT_SIZE_MICRO))
                                            .text_color(s.text_quaternary)
                                            .line_clamp(2)
                                            .child(source.owner_sign.clone()),
                                    )
                                }),
                        ),
                )
                .child(
                    div()
                        .flex()
                        .items_center()
                        .gap(px(SPACE_2))
                        .mt(px(SPACE_3))
                        .child(
                            div()
                                .id("player-charge")
                                .flex_shrink_0()
                                .h(px(30.))
                                .px(px(SPACE_4))
                                .flex()
                                .items_center()
                                .rounded(px(RADIUS_MD))
                                .bg(p.theme_primary)
                                .cursor_pointer()
                                .hover(|style| style.opacity(0.9))
                                .text_size(px(FONT_SIZE_CAPTION))
                                .font_weight(FontWeight::SEMIBOLD)
                                .text_color(rgb(0xFFFFFF))
                                .child("充电"),
                        )
                        .child(
                            div()
                                .id("player-follow")
                                .flex_1()
                                .h(px(30.))
                                .flex()
                                .items_center()
                                .justify_center()
                                .gap(px(5.))
                                .rounded(px(RADIUS_MD))
                                .bg(s.fill_2)
                                .when(self.follow_busy, |el| el.opacity(0.55))
                                .cursor_pointer()
                                .hover(|style| style.bg(s.fill_3))
                                .text_size(px(FONT_SIZE_CAPTION))
                                .text_color(if self.followed {
                                    s.text_tertiary
                                } else {
                                    s.text_secondary
                                })
                                .child(
                                    Icon::new(if self.followed {
                                        IconName::UserCheck
                                    } else {
                                        IconName::UserPlus
                                    })
                                    .size(px(ICON_SIZE_SM))
                                    .text_color(if self.followed {
                                        s.text_tertiary
                                    } else {
                                        s.text_secondary
                                    }),
                                )
                                .child(if self.follow_busy {
                                    "处理中…"
                                } else if self.followed {
                                    "已关注"
                                } else {
                                    "关注"
                                })
                                .on_click(cx.listener(|view, _: &ClickEvent, _window, cx| {
                                    view.toggle_follow(cx);
                                })),
                        )
                        .when_some(self.owner_fans, |el, fans| {
                            // 粉丝数来自 `/x/relation/stat`：紧跟关注按钮，
                            // 关注 / 取关成功后跟着 ±1。
                            el.child(
                                div()
                                    .flex_shrink_0()
                                    .text_size(px(FONT_SIZE_MICRO))
                                    .text_color(s.text_quaternary)
                                    .child(format!("{} 粉丝", format::count(fans))),
                            )
                        }),
                )
        });

        // 视频简介单独一行，放在 UP 信息块之后：不塞进 UP 昵称下面的窄列，
        // 也不会把「发消息 / 关注」这类 UP 操作挤成两行。
        let desc_row = source.and_then(|source| {
            let desc = source.desc.trim().to_string();
            if desc.is_empty() {
                return None;
            }
            let weak = cx.entity().downgrade();
            let copy_notice = Rc::new(move |cx: &mut App| {
                let _ = weak.update(cx, |view, cx| {
                    view.show_interaction_note("已复制".into(), false, cx);
                });
            });
            Some(
                div().w_full().px(px(SPACE_4)).pb(px(SPACE_3)).child(
                    div()
                        .id("player-video-desc")
                        .w(px(PLAYER_INFO_PANEL_WIDTH - SPACE_4 * 2.0))
                        .child(
                            gpui::base::SelectableText::new("player-video-desc-text", desc.clone())
                                .text_style(gpui::TextStyleRefinement {
                                    font_size: Some(px(FONT_SIZE_CAPTION).into()),
                                    line_height: Some(px(18.).into()),
                                    color: Some(s.text_tertiary),
                                    line_clamp: Some(3),
                                    ..Default::default()
                                }),
                        )
                        .context_menu(text_copy_menu(desc, copy_notice)),
                ),
            )
        });

        // 互动行：点赞 / 投币 / 收藏 / 分享。
        // 计数为 0 时显示动作名（B 站的做法），有计数时只显示数字。
        let like_count = source.map(|source| source.like_count).unwrap_or(0);
        let coin_count = source.map(|source| source.coin_count).unwrap_or(0);
        let fav_count = source.map(|source| source.favorite_count).unwrap_or(0);
        let share_count = source.map(|source| source.share_count).unwrap_or(0);
        let like_label = if like_count > 0 {
            format::count(like_count)
        } else {
            "点赞".to_string()
        };
        let coin_label = if coin_count > 0 {
            format::count(coin_count)
        } else {
            "投币".to_string()
        };
        let fav_label = if fav_count > 0 {
            format::count(fav_count)
        } else {
            "收藏".to_string()
        };
        let share_label = if share_count > 0 {
            format::count(share_count)
        } else {
            "分享".to_string()
        };
        let action_bounds = self.action_row_bounds.clone();
        let actions = div()
            // 顶边线按 400 宽铺满（面板内容就是 400 宽排版），按钮区另外
            // 固定 200 —— 见下面的说明。
            .w(px(PLAYER_INFO_PANEL_WIDTH))
            .relative()
            .border_t_1()
            .border_color(s.border_subtle)
            .child(
                div()
                    // 按钮区固定 200 宽：面板内容按 400 宽排版，收起态右半
                    // 会被裁掉；按钮区若跟着 400 排，收藏 / 分享两个按钮就整排
                    // 落在窗口外，点了没反应。
                    // 宽度也**不能**跟着展开态变：侧栏一 hover 展开，四个按钮
                    // 就会重新均分、整体挪位，用户瞄准的按钮会从指针下滑走，
                    // 一点就点空（甚至点到隔壁按钮）。
                    .w(px(INFO_PANEL_COLLAPSED))
                    .flex()
                    .items_center()
                    .justify_around()
                    .px(px(SPACE_2))
                    .py(px(2.))
                    .child(panel_action_button(
                        t,
                        "player-action-like",
                        IconName::ThumbsUp,
                        like_label,
                        self.interaction.liked,
                        cx.listener(|view, _: &ClickEvent, _window, cx| {
                            view.toggle_like(cx);
                        }),
                    ))
                    .child(panel_action_button(
                        t,
                        "player-action-coin",
                        IconName::Coins,
                        coin_label,
                        self.interaction.coined,
                        cx.listener(|view, _: &ClickEvent, _window, cx| {
                            view.toggle_coin_panel(cx);
                        }),
                    ))
                    .child(panel_action_button(
                        t,
                        "player-action-fav",
                        IconName::Star,
                        fav_label,
                        self.interaction.favoured,
                        cx.listener(|view, _: &ClickEvent, _window, cx| {
                            view.toggle_fav_panel(cx);
                        }),
                    ))
                    .child(panel_action_button(
                        t,
                        "player-action-share",
                        IconName::Share,
                        share_label,
                        self.share_panel_open,
                        cx.listener(|view, _: &ClickEvent, _window, cx| {
                            view.toggle_share_panel(cx);
                        }),
                    )),
            )
            // 互动行的实际几何由这层透明 canvas 在 prepaint 时写回。
            // 弹层按它挂到窗口上，所以展开面板不会再把下面的内容顶下去。
            .child(
                canvas(
                    move |bounds, _, _| action_bounds.set(Some(bounds)),
                    |_, _, _, _| {},
                )
                .absolute()
                .inset_0(),
            );

        let mut tags: Vec<String> = if !self.tags.is_empty() {
            self.tags.clone()
        } else {
            source
                .map(|source| {
                    if source.category.is_empty() {
                        Vec::new()
                    } else {
                        vec![source.category.clone()]
                    }
                })
                .unwrap_or_default()
        };
        tags.truncate(3);
        let tags_row = (!tags.is_empty()).then(|| {
            let mut row = div()
                .flex()
                .flex_wrap()
                .gap(px(SPACE_2))
                .px(px(SPACE_4))
                .py(px(SPACE_2))
                .border_t_1()
                .border_color(s.border_subtle);
            for tag in tags {
                row = row.child(
                    div()
                        .px(px(10.))
                        .h(px(22.))
                        .flex()
                        .items_center()
                        .rounded_full()
                        .bg(s.fill_2)
                        .text_size(px(FONT_SIZE_CAPTION))
                        .text_color(s.text_tertiary)
                        .child(tag),
                );
            }
            row
        });

        let tabs = self.panel_tabs();
        let active = self.tab.min(tabs.len().saturating_sub(1));
        let mut tabs_row = div()
            .flex()
            .border_t_1()
            .border_color(s.border_subtle)
            .flex_shrink_0();
        for (index, name) in tabs.iter().enumerate() {
            let on = index == active;
            tabs_row = tabs_row.child(
                div()
                    .id(("player-tab", index))
                    .flex_1()
                    .h(px(42.))
                    .flex()
                    .flex_col()
                    .items_center()
                    .justify_center()
                    .gap(px(4.))
                    .cursor_pointer()
                    .text_size(px(FONT_SIZE_CONTROL))
                    .font_weight(if on {
                        FontWeight::SEMIBOLD
                    } else {
                        FontWeight::MEDIUM
                    })
                    .text_color(if on { p.theme_primary } else { s.text_tertiary })
                    .child(*name)
                    .child(div().w(px(28.)).h(px(2.)).rounded_full().bg(if on {
                        p.theme_primary
                    } else {
                        hsla(0.0, 0.0, 0.0, 0.0)
                    }))
                    .on_click(cx.listener(move |view, _: &ClickEvent, _window, cx| {
                        view.tab = index;
                        cx.notify();
                    })),
            );
        }

        // 整个面板只有一个滚动区：图 1 那一段（标题 / UP / 互动 / 标签 / Tab）
        // 不再固定，和下面的评论一起滚。内层单独包一列，避免 flex 子项被压缩后
        // 高度不足、overflow_y_scroll 反而滚不动。
        let content = div()
            .w_full()
            .flex()
            .flex_col()
            .child(head)
            .children(meta)
            .children(up)
            .children(desc_row)
            .child(actions)
            .children(tags_row)
            .child(tabs_row)
            .child(self.tab_content(t, active, window, cx));

        let body = div()
            .id("player-info-scroll")
            .w(px(PLAYER_INFO_PANEL_WIDTH))
            .flex_shrink_0()
            .h_full()
            .overflow_y_scroll()
            .track_scroll(&self.panel_scroll)
            .on_scroll_wheel(cx.listener(|view, event: &ScrollWheelEvent, window, cx| {
                // 滚动事件自带窗口内坐标，用它把「指针还在面板里」钉住：
                // 播放中每帧重绘会让 hover 偶发 false，滚动时不能被它误收起。
                view.panel_last_scroll = Some(std::time::Instant::now());
                view.last_pointer = event.position;
                view.panel_collapse_generation = view.panel_collapse_generation.wrapping_add(1);
                view.panel_hover_grace_until = None;
                if !view.expanded {
                    view.expanded = true;
                    view.panel_generation = view.panel_generation.wrapping_add(1);
                    cx.notify();
                }
                // 滚到底部附近就补下一批评论（每批 1 页 20 条）。
                view.comment_section
                    .maybe_load_more(&view.panel_scroll, cx);
                // 评论是窗口化渲染：可见区随滚动变化，每滚一下都要重算窗口，
                // 否则滚出已创建的 ~27 行之后会看到空白。
                if view.tab == 0 {
                    cx.notify();
                }
                view.note_activity(window, cx);
            }))
            .child(content);

        // 「回到顶部」浮出判定与输入条让位都在共享组件里，这里只喂滚动位置。
        self.comment_section.sync_scroll(&self.panel_scroll);
        if !self.expanded {
            self.comment_section.back_to_top = false;
        }
        // 评论输入条只活在「展开 + 评论 Tab」这一帧。
        let composer_visible = self.tab == 0 && self.expanded;

        let to_width = self.panel_width();
        let from_width = if self.panel_generation == 0 {
            to_width
        } else if self.expanded {
            INFO_PANEL_COLLAPSED
        } else {
            INFO_PANEL_EXPANDED
        };
        div()
            .id("info-panel")
            .absolute()
            .top(px(0.))
            // 侧栏贴着窗口右边界；视频区右缘不铺 hover 热区，避免误触发展开。
            .right(px(0.))
            // 高度通高：从播放页层顶部一直拉到底。
            .bottom(px(0.))
            .w(px(to_width))
            .overflow_hidden()
            // 浮在视频上的面板：GPUI 没有元素级 backdrop blur，用不透明表面。
            .bg(s.surface_elevated_solid)
            .when(self.expanded, |el| {
                el.border_l_1().border_color(s.border_subtle)
            })
            // hover 只负责「进入即展开」的兜底；收起一律由真实指针坐标决定。
            // 播放中每帧重绘都会重算 hover，不能让它驱动收起。
            .on_hover(cx.listener(|view, hovered: &bool, window, cx| {
                // 用当前光标位置判断：`window.mouse_position()` 停在最后一次
                // 事件上，面板尺寸动画把它顶到光标底下时会拿旧位置判定，
                // 于是面板在「指针早已移开」的情况下自己弹回来。
                if *hovered && view.pointer_in_panel_zone(cursor_position(window, cx), window) {
                    view.set_panel_hover(true, window, cx);
                }
            }))
            .on_mouse_move(cx.listener(|view, event: &MouseMoveEvent, window, cx| {
                view.note_pointer_move(event.position, window, cx);
            }))
            .child(body)
            // 互动弹层：deferred 画在所有面板内容之后（GPUI 没有 z-index）。
            .children(self.action_overlay(t, window, cx))
            // 滚动后出现：浮在内容之上，右下角 16px 内边距。
            // 收起时也占住这个子节点位置（渲染成 0 尺寸），别让兄弟节点
            // 数量随滚动变化 —— 子树被重建会让里面的动画从头再播一遍。
            .child(
                self.comment_section
                    .back_to_top(t, &self.panel_scroll, cx),
            )
            // 评论 Tab 的浮动输入条：与动态详情同一份组件。
            .children(composer_visible.then(|| {
                let width = self.panel_width();
                self.comment_section.composer(t, window, cx, width)
            }))
            .with_animation(
                format!("player-panel-{}", self.panel_generation),
                Animation::new(std::time::Duration::from_millis(200)).with_easing(ease_standard()),
                move |el, delta| {
                    el.w(px(from_width + (to_width - from_width) * delta))
                },
            )
            .into_any_element()
    }
}
