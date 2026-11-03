use super::*;

impl PlayerView {
    pub(crate) fn danmaku_layer(&self, area_width: f32) -> impl IntoElement {
        let now = self.play_time;

        // 先在普通 Rust 里把「这一帧要画什么」算好，交给 paint 闭包。
        // 闭包必须 'static，所以数据要 move 进去。
        struct Drawn {
            text: String,
            size: f32,
            color: gpui::Hsla,
            width: f32,
            /// 滚动弹幕的 x；固定弹幕在 paint 里按画布宽度居中。
            scroll_x: Option<f32>,
            /// 滚动弹幕的 y。
            y: f32,
            /// 固定弹幕：(是否顶部, 槽位)。
            fixed: Option<(bool, usize)>,
        }

        // 时间序已经排好，用二分只取「可能还停在屏幕上」的一段；
        // 每帧全量遍历上万条会白烧 CPU。
        let start = self
            .danmaku
            .partition_point(|p| p.data.time < now - SCROLL_LOOKBACK);
        let end = self.danmaku.partition_point(|p| p.data.time <= now);
        let drawn: Vec<Drawn> = self.danmaku[start..end]
            .iter()
            .filter_map(|placed| {
                let d = &placed.data;
                match placed.lane {
                    DanmakuLane::Scroll(lane) => {
                        if !self.lanes.visible_at_in(d, now, area_width) {
                            return None;
                        }
                        Some(Drawn {
                            text: d.text.clone(),
                            size: d.font_size,
                            color: rgb(d.rgb()).into(),
                            width: d.width,
                            scroll_x: Some(self.lanes.x_at_in(d, now, area_width)),
                            y: 8.0 + lane as f32 * SCROLL_LANE_HEIGHT,
                            fixed: None,
                        })
                    }
                    DanmakuLane::Top(slot) => {
                        if !fixed_danmaku_visible(d, now) {
                            return None;
                        }
                        Some(Drawn {
                            text: d.text.clone(),
                            size: d.font_size,
                            color: rgb(d.rgb()).into(),
                            width: d.width,
                            scroll_x: None,
                            y: 0.0,
                            fixed: Some((true, slot)),
                        })
                    }
                    DanmakuLane::Bottom(slot) => {
                        if !fixed_danmaku_visible(d, now) {
                            return None;
                        }
                        Some(Drawn {
                            text: d.text.clone(),
                            size: d.font_size,
                            color: rgb(d.rgb()).into(),
                            width: d.width,
                            scroll_x: None,
                            y: 0.0,
                            fixed: Some((false, slot)),
                        })
                    }
                }
            })
            // 不要在这里按条数截断。
            //
            // 旧代码是 `.take(80)`，而 `self.danmaku` 按时间升序排列 —— 截掉的
            // 永远是**最新**的 80 条之后的那批，也就是刚要从右边缘进场的那批。
            // 密弹幕（同一秒几十条短弹幕）时同屏可见能到 100–300 条，被截的
            // 弹幕要等老弹幕离场才进绘制集合，那时它已经飞到画面中间了 ——
            // 表现就是「弹幕走到中间才冒出来」。
            //
            // 同屏条数本身由轨道分配约束（`MAX_SCROLL_LANES` × 每条轨道能并存的条数，
            // 上限约 600 条极端情况），实测 100 条/秒的单字弹幕一帧构建也只要 ~3ms，
            // 不需要再按条数封顶。
            .collect();

        // canvas：直接走底层 paint，不经过 div + text 的常规元素路径。
        // 常规路径下文字会破坏 surface 的合成（实测画面全黑），
        // 这条是文档第 14 节指定的方案 ②。
        canvas(
            |_bounds, _window, _cx| (),
            move |bounds, _state, window, cx| {
                let area_width = bounds.size.width.as_f32();
                let area_height = bounds.size.height.as_f32();
                // `ShapedLine::paint` 的 origin 是**窗口坐标**，不是画布内坐标。
                // 弹幕层只盖播放层（左上 = 窗口左 16 / 顶栏下 16），不补
                // `bounds.origin` 整层会整体上移一条顶栏、左移 16px，
                // 看起来就是「弹幕浮在画面上半屏」。
                let origin = bounds.origin;
                let at = |x: f32, y: f32| point(origin.x + px(x), origin.y + px(y));
                for d in &drawn {
                    let (x, y) = match d.fixed {
                        Some((top, slot)) => {
                            let x = ((area_width - d.width) * 0.5).max(8.0);
                            let y = if top {
                                8.0 + slot as f32 * SCROLL_LANE_HEIGHT
                            } else {
                                (area_height - 40.0 - slot as f32 * SCROLL_LANE_HEIGHT).max(8.0)
                            };
                            (x, y)
                        }
                        None => (d.scroll_x.unwrap_or(0.0), d.y),
                    };
                    // 设计稿要求「1px 深色描边 + 投影」，而 ShapedLine 的
                    // 颜色烘在 layout 里、paint 不能重新着色。这里把同一条文本
                    // shape 两次：一次深色（描边/投影），一次原色（字身）。
                    // 描边色必须是深色（#0B0F14），白字 + 白描边在白画面上会糊。
                    let stroke = TextRun {
                        len: d.text.len(),
                        font: gpui::Font::default(),
                        color: rgba(0x0B0F14D9).into(),
                        ..Default::default()
                    };
                    let stroke_line = window.text_system().shape_line(
                        d.text.clone().into(),
                        px(d.size),
                        &[stroke],
                        None,
                    );
                    let line_height = px(d.size * 1.25);

                    // 投影：右下 1,2 px 先画一层，再画 8 方向 1px 描边。
                    let _ = stroke_line.paint(
                        at(x + 1.0, y + 2.0),
                        line_height,
                        TextAlign::Left,
                        None,
                        window,
                        cx,
                    );
                    for (dx, dy) in [
                        (-1.0, 0.0),
                        (1.0, 0.0),
                        (0.0, -1.0),
                        (0.0, 1.0),
                        (-1.0, -1.0),
                        (1.0, -1.0),
                        (-1.0, 1.0),
                        (1.0, 1.0),
                    ] {
                        let _ = stroke_line.paint(
                            at(x + dx, y + dy),
                            line_height,
                            TextAlign::Left,
                            None,
                            window,
                            cx,
                        );
                    }

                    // 字身
                    let run = TextRun {
                        len: d.text.len(),
                        font: gpui::Font::default(),
                        color: d.color,
                        ..Default::default()
                    };
                    let line = window.text_system().shape_line(
                        d.text.clone().into(),
                        px(d.size),
                        &[run],
                        None,
                    );
                    let _ = line.paint(at(x, y), line_height, TextAlign::Left, None, window, cx);
                }
            },
        )
        .absolute()
        // 只覆盖播放层：右侧信息面板展开 400 / 收起 200，都不属于弹幕区。
        .top_0()
        .left_0()
        .bottom_0()
        .right(px(self.panel_width()))
    }
}

/// 占位弹幕（真实数据要从 `/x/v1/dm/list.so` 拉 XML，属于后续）。
/// 给顶部 / 底部固定弹幕找一个空槽位。
///
/// 槽位在这个时间点已被占满时返回 `None`，调用方把那一条留在列表里、
/// 不上屏 —— 比硬挤在同一行、文字互相盖住要好。
pub(crate) fn take_fixed_slot(free: &mut [f64], time: f64) -> Option<usize> {
    let slot = free.iter().position(|end| time >= *end)?;
    free[slot] = time + FIXED_DANMAKU_DURATION;
    Some(slot)
}

/// 顶部 / 底部固定弹幕在 `now` 是否还该显示。
pub(crate) fn fixed_danmaku_visible(d: &Danmaku, now: f64) -> bool {
    now >= d.time && now <= d.time + FIXED_DANMAKU_DURATION
}
