use super::*;

impl PlayerView {
    /// 面板 Tab 内容：评论 / 弹幕 / 选集。
    pub(crate) fn tab_content(
        &mut self,
        t: &YkTheme,
        active: usize,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) -> AnyElement {
        match active {
            0 => self.comments_tab(t, window, cx),
            1 => self.danmaku_tab(t),
            _ => self.pages_tab(t, cx),
        }
    }

    /// 评论 Tab。
    ///
    /// **窗口化渲染**：评论行按「行高前缀和」绝对定位，只创建当前可见区
    /// （上下各多 `COMMENT_OVERSCAN`）内的行。翻到 1000 条也只存在 ~30 行元素，
    /// 不会因为条目过多在滚动时掉帧。
    pub(crate) fn comments_tab(
        &mut self,
        t: &YkTheme,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) -> AnyElement {
        let s = &t.surfaces;
        let p = &t.palette;
        let total = if self.comment_section.list.total > 0 {
            self.comment_section.list.total
        } else {
            self.source
                .as_ref()
                .map(|source| source.reply_count)
                .unwrap_or(0)
        };
        let retry = self
            .source
            .as_ref()
            .map(|source| (source.aid, source.bvid.clone()));

        // 底部留出浮动评论输入框的高度，最后一条评论能滚到输入框上方。
        // 底部同时给「回到顶部」按钮和浮动输入框留出空间。
        let mut list = div().w_full().px(px(SPACE_4)).pb(px(116.));
        match &self.comments {
            CommentState::Idle => {}
            CommentState::Loading => {
                for _ in 0..3 {
                    list = list.child(comment_skeleton(t));
                }
            }
            CommentState::Error(error) => {
                list = list.child(panel_empty_line(t, "评论加载失败")).child(
                    div()
                        .w_full()
                        .pb(px(SPACE_4))
                        .flex()
                        .justify_center()
                        .child(
                            div()
                                .text_size(px(FONT_SIZE_CAPTION))
                                .text_color(s.text_quaternary)
                                .line_clamp(2)
                                .child(error.clone()),
                        ),
                );
                if let Some((aid, bvid)) = retry {
                    list = list.child(
                        div().w_full().flex().justify_center().child(
                            div()
                                .id("player-comments-retry")
                                .px(px(SPACE_3))
                                .h(px(26.))
                                .flex()
                                .items_center()
                                .rounded(px(RADIUS_MD))
                                .bg(s.fill_2)
                                .cursor_pointer()
                                .hover(|style| style.bg(s.fill_3))
                                .text_size(px(FONT_SIZE_CAPTION))
                                .text_color(p.theme_primary)
                                .child("重试")
                                .on_click(cx.listener(move |view, _: &ClickEvent, _window, cx| {
                                    view.spawn_video_meta(aid, bvid.clone(), cx);
                                })),
                        ),
                    );
                }
            }
            CommentState::Ready => {
                if self.comment_section.list.items.is_empty() {
                    // 首批都没有：只有服务端明确说到头了，才说「没有更多」。
                    let text = if self.comment_section.list.is_end {
                        "没有更多评论"
                    } else {
                        "评论加载中…"
                    };
                    list = list.child(panel_empty_line(t, text));
                } else {
                    let text_width = crate::comment_section::text_width(PLAYER_INFO_PANEL_WIDTH);
                    let weak = cx.entity().downgrade();
                    let copy_notice = Rc::new(move |cx: &mut App| {
                        let _ = weak.update(cx, |view, cx| {
                            view.show_interaction_note("已复制".into(), false, cx);
                        });
                    });
                    let handler = self.image_handler.clone();
                    let on_image = Rc::new(
                        move |urls: Vec<String>,
                              index: usize,
                              window: &mut Window,
                              cx: &mut App| {
                            if let Some(handler) = handler.as_ref() {
                                handler(cx, window, urls, index);
                            }
                        },
                    ) as Rc<dyn Fn(Vec<String>, usize, &mut Window, &mut App)>;
                    list = list.child(self.comment_section.body(
                        t,
                        window,
                        &self.panel_scroll,
                        text_width,
                        "player-comment",
                        copy_notice,
                        Some(on_image),
                        cx,
                    ));
                }
            }
        }

        self.comment_section
            .maybe_load_more(&self.panel_scroll, cx);

        div()
            .w_full()
            .flex()
            .flex_col()
            .child(
                div()
                    .flex()
                    .items_baseline()
                    .justify_between()
                    .gap(px(SPACE_2))
                    .px(px(SPACE_4))
                    .pb(px(SPACE_2))
                    .flex_shrink_0()
                    .child(
                        div()
                            .text_size(px(FONT_SIZE_CONTROL))
                            .font_weight(FontWeight::SEMIBOLD)
                            .text_color(s.text_primary)
                            .child(format!("评论 {}", format::count(total))),
                    )
                    .child(
                        div()
                            .flex()
                            .items_center()
                            .gap(px(SPACE_2))
                            .text_size(px(FONT_SIZE_CAPTION))
                            .text_color(s.text_quaternary)
                            .child(
                                div()
                                    .text_color(s.text_primary)
                                    .font_weight(FontWeight::MEDIUM)
                                    .child("最热"),
                            )
                            .child(div().child("|"))
                            .child(div().child("最新")),
                    ),
            )
            .child(list)
            .into_any_element()
    }

    pub(crate) fn danmaku_tab(&self, t: &YkTheme) -> AnyElement {
        match &self.danmaku_state {
            DanmakuState::Idle => return panel_empty_line(t, "暂无弹幕").into_any_element(),
            DanmakuState::Loading => {
                return panel_empty_line(t, "弹幕加载中…").into_any_element();
            }
            DanmakuState::Error(error) => {
                return panel_empty_line(t, &format!("弹幕加载失败：{error}")).into_any_element();
            }
            DanmakuState::Ready => {}
        }
        if self.danmaku_all.is_empty() {
            return panel_empty_line(t, "暂无弹幕").into_any_element();
        }

        let s = &t.surfaces;
        let mut list = div().w_full().px(px(SPACE_4)).pb(px(SPACE_4));
        for danmaku in self.danmaku_all.iter().take(60) {
            let mode = match danmaku.mode {
                DanmakuMode::Top => Some("顶"),
                DanmakuMode::Bottom => Some("底"),
                DanmakuMode::Scroll => None,
            };
            list = list.child(
                div()
                    .flex()
                    .items_start()
                    .gap(px(SPACE_2))
                    .py(px(5.))
                    .border_b_1()
                    .border_color(s.border_subtle)
                    .child(
                        div()
                            .w(px(40.))
                            .flex_shrink_0()
                            .text_size(px(FONT_SIZE_CAPTION))
                            .text_color(s.text_quaternary)
                            .child(format::duration(danmaku.time.max(0.0) as u64)),
                    )
                    .when_some(mode, |el, mode| {
                        el.child(
                            div()
                                .w(px(16.))
                                .flex_shrink_0()
                                .text_center()
                                .text_size(px(FONT_SIZE_MICRO))
                                .text_color(s.text_quaternary)
                                .child(mode),
                        )
                    })
                    .child(
                        div()
                            .flex_1()
                            .min_w(px(0.))
                            .text_size(px(FONT_SIZE_CAPTION))
                            .text_color(s.text_secondary)
                            .line_clamp(2)
                            .child(danmaku.text.clone()),
                    ),
            );
        }
        list.into_any_element()
    }

    pub(crate) fn pages_tab(&self, t: &YkTheme, cx: &mut Context<Self>) -> AnyElement {
        let s = &t.surfaces;
        let p = &t.palette;
        let source = self.source.as_ref();
        let current = source.map(|source| source.current_page).unwrap_or(1);
        let pages = source.map(|source| source.pages.as_slice()).unwrap_or(&[]);
        let switching = self.page_switching;
        let mut list = div()
            .w_full()
            .px(px(SPACE_4))
            .pb(px(SPACE_4))
            .flex()
            .flex_col()
            .gap(px(4.))
            .when(switching, |el| el.opacity(0.6));
        if pages.is_empty() {
            list = list.child(panel_empty_line(t, "暂无分 P 信息"));
        } else {
            for page in pages {
                let on = page.page == current;
                let cid = page.cid;
                let page_no = page.page;
                list = list.child(
                    div()
                        .id(("player-page-row", page_no as u64))
                        .flex()
                        .items_center()
                        .gap(px(SPACE_2))
                        .px(px(SPACE_2))
                        .py(px(SPACE_2))
                        .rounded(px(RADIUS_MD))
                        .when(on, |el| el.bg(s.fill_2))
                        .cursor_pointer()
                        .hover(|style| style.bg(s.fill_3))
                        .on_click(cx.listener(move |view, _: &ClickEvent, _window, cx| {
                            view.switch_page(cid, cx);
                        }))
                        .child(
                            div()
                                .w(px(16.))
                                .flex_shrink_0()
                                .text_color(p.theme_primary)
                                .when(on, |el| {
                                    el.child(
                                        Icon::new(IconName::Check)
                                            .size(px(14.))
                                            .text_color(p.theme_primary),
                                    )
                                }),
                        )
                        .child(
                            div()
                                .flex_1()
                                .min_w(px(0.))
                                .truncate()
                                .text_size(px(FONT_SIZE_CAPTION))
                                .text_color(if on { s.text_primary } else { s.text_secondary })
                                .child(format!("P{} · {}", page.page, page.part)),
                        )
                        .child(
                            div()
                                .text_size(px(FONT_SIZE_CAPTION))
                                .text_color(s.text_quaternary)
                                .child(if page.duration > 0 {
                                    format::duration(page.duration)
                                } else {
                                    String::new()
                                }),
                        ),
                );
            }
        }
        list.into_any_element()
    }
}

/// 评论加载骨架：圆头像 + 三行灰条。
pub(crate) fn comment_skeleton(t: &YkTheme) -> impl IntoElement {
    let s = &t.surfaces;
    div()
        .flex()
        .items_start()
        .gap(px(SPACE_2))
        .py(px(SPACE_2))
        .border_b_1()
        .border_color(s.border_subtle)
        .child(
            div()
                .w(px(28.))
                .h(px(28.))
                .flex_shrink_0()
                .rounded_full()
                .bg(s.fill_2),
        )
        .child(
            div()
                .flex_1()
                .min_w(px(0.))
                .flex()
                .flex_col()
                .gap(px(6.))
                .child(
                    div()
                        .w(relative(0.36))
                        .h(px(10.))
                        .rounded_full()
                        .bg(s.fill_2),
                )
                .child(
                    div()
                        .w(relative(0.92))
                        .h(px(12.))
                        .rounded_full()
                        .bg(s.fill_1),
                )
                .child(
                    div()
                        .w(relative(0.64))
                        .h(px(12.))
                        .rounded_full()
                        .bg(s.fill_1),
                ),
        )
}

/// 面板里的空态 / 加载文案。
pub(crate) fn panel_empty_line(t: &YkTheme, text: &str) -> impl IntoElement {
    div()
        .w_full()
        .py(px(SPACE_6))
        .text_center()
        .text_size(px(FONT_SIZE_CAPTION))
        .text_color(t.surfaces.text_quaternary)
        .child(text.to_string())
}
