//! 评论区文本选区的行为验收：左右拖动选字时侧栏不应自己滚动。
//!
//! 背景：`gpui-base` 的拖选自动滚动把「最近一层裁切」当成可视区。
//! 评论行曾经用 `overflow_hidden` 把裁切收到正文高度（几十像素），
//! 指针一进正文就落进上下 16px 的触发区，拖动时持续派发滚轮，
//! 虚拟列表被滚出很远。这里用真实 `PlayerView` 复现一遍。
//!
//! 本地验收：`cargo test -p yk-app --test comment_selection`

use std::time::Duration;

use gpui::component::ThemeMode;
use gpui::prelude::*;
use gpui::test::TestWindowExt;
use gpui::{
    point, px, size, Bounds, Modifiers, MouseButton, TestAppContext, VisualTestContext,
    WindowBounds, WindowOptions,
};
use yk_app::player::{set_test_cursor, PlayerView};
use yk_infra::dynamic::DynamicComment;

const WINDOW_W: f32 = 1280.0;
const WINDOW_H: f32 = 820.0;
/// 播放浮窗左右各内缩 16（见 `PlayerView` 的浮窗容器）。
const WINDOW_INSET: f32 = 16.0;
/// 展开态侧栏宽 400。
const PANEL_WIDTH: f32 = 400.0;

fn comments(count: usize) -> Vec<DynamicComment> {
    (0..count)
        .map(|i| {
            let text = if i == 0 {
                // 首条刻意拉长到 3 行，给拖选一个足够大的落点。
                "第 0 条评论：这是一段明显更长的正文，用来验证三行折行的高度计算与\
                 行与行之间的衔接，长到一定会被 line_clamp 截断成三行。"
                    .to_string()
            } else {
                format!("第 {i} 条评论")
            };
            DynamicComment {
                rpid: i as i64 + 1,
                author: format!("用户{i}"),
                face: String::new(),
                text,
                time: format!("{} 天前", i % 30 + 1),
                like: i as u64,
                pictures: Vec::new(),
                is_top: false,
            }
        })
        .collect()
}

fn open_comments(
    cx: &mut TestAppContext,
) -> (
    gpui::AnyWindowHandle,
    gpui::Entity<PlayerView>,
    VisualTestContext,
) {
    let theme = yk_theme::theme::YkTheme::dark();
    let (handle, player) = cx.update(|cx| {
        gpui::init(cx);
        gpui::component::init(cx);
        gpui::open_window(
            WindowOptions {
                window_bounds: Some(WindowBounds::Windowed(Bounds {
                    origin: Default::default(),
                    size: size(px(WINDOW_W), px(WINDOW_H)),
                })),
                focus: false,
                show: false,
                ..Default::default()
            },
            cx,
            |window, cx| {
                gpui::component::Theme::change(ThemeMode::Dark, Some(window), cx);
                yk_theme::gpui_theme::apply(cx, &theme);
                let theme = theme.clone();
                cx.new(|cx| {
                    PlayerView::new(theme, cx)
                        .with_expanded(true)
                        .with_comments(comments(80), 12_345, false)
                })
            },
        )
        .unwrap()
    });
    let mut vcx = VisualTestContext::from_window(handle, cx);
    vcx.update(|window, cx| {
        window.render_frame(cx);
    });
    // 固定光标在侧栏内，避免 hover 收起把布局改掉。
    set_test_cursor(Some(point(px(960.), px(300.))));
    assert!(
        vcx.update(|window, _| window.try_find("comment-composer").is_some()),
        "评论 Tab 底部应该有浮动评论输入框"
    );
    (handle, player, vcx)
}

#[gpui::test]
fn horizontal_drag_in_comment_does_not_scroll_panel(cx: &mut TestAppContext) {
    let (_handle, player, mut vcx) = open_comments(cx);

    let offset =
        |vcx: &mut VisualTestContext| vcx.update(|_, cx| player.read(cx).test_scroll_offset());
    assert_eq!(offset(&mut vcx), 0.0, "初始就不该有滚动");

    // 先滚到评论中段，这样选字时无论自动滚动往上还是往下都有空间可走。
    vcx.update(|window, cx| {
        player.update(cx, |view, cx| {
            view.set_test_scroll(-1000.0);
            cx.notify();
        });
        window.render_frame(cx);
    });
    let before = offset(&mut vcx);
    assert!(before < -900.0, "滚动位置没设上：{before}");

    // 侧栏正文起点：右缘内缩 16 − 面板宽 400，再让开 padding 16 + 头像 28 + 间距 8。
    let text_left = WINDOW_W - WINDOW_INSET - PANEL_WIDTH + 16.0 + 28.0 + 8.0;
    // 取评论正文靠上的位置：修复前它落在「正文裁切区的上 16px」里。
    // 播放浮窗顶部 = TOP_BAR_HEIGHT + 16，这里取浮窗内 205px 处。
    let y = yk_theme::theme::TOP_BAR_HEIGHT + 16.0 + 205.0;

    vcx.simulate_mouse_move(
        point(px(text_left + 10.0), px(y)),
        None,
        Modifiers::default(),
    );
    vcx.simulate_mouse_down(
        point(px(text_left + 10.0), px(y)),
        MouseButton::Left,
        Modifiers::default(),
    );
    // 在正文里左右拖，模拟「选一句话」时的手势。
    for dx in [60.0, 140.0, 220.0, 120.0, 40.0] {
        vcx.simulate_mouse_move(
            point(px(text_left + dx), px(y)),
            Some(MouseButton::Left),
            Modifiers::default(),
        );
    }

    // 让自动滚动的定时器有机会跑几圈；修复前这里会把侧栏滚出很远。
    for _ in 0..8 {
        vcx.cx.executor().advance_clock(Duration::from_millis(30));
        vcx.run_until_parked();
    }
    vcx.update(|window, cx| {
        window.render_frame(cx);
    });

    let selected = vcx.update(|window, cx| gpui::base::TextSelection::selected_text(window, cx));
    assert!(
        !selected.trim().is_empty(),
        "在评论正文上拖动应当选中文字（坐标没落在正文上？）"
    );
    assert_eq!(offset(&mut vcx), before, "左右拖动选字不该让侧栏滚动");

    vcx.simulate_mouse_up(
        point(px(text_left + 40.0), px(y)),
        MouseButton::Left,
        Modifiers::default(),
    );

    // 保留选区直接右键：菜单里的复制必须复制选区，而不是整条评论。
    let selected_press = point(px(text_left + 80.0), px(y));
    vcx.simulate_mouse_down(selected_press, MouseButton::Right, Modifiers::default());
    vcx.simulate_mouse_up(selected_press, MouseButton::Right, Modifiers::default());
    vcx.update(|window, cx| {
        window.render_frame(cx);
    });
    let selected_item = selected_press + point(px(64.), px(17.));
    vcx.simulate_mouse_down(selected_item, MouseButton::Left, Modifiers::default());
    vcx.simulate_mouse_up(selected_item, MouseButton::Left, Modifiers::default());
    vcx.update(|window, cx| {
        window.render_frame(cx);
    });
    let copied_selection = vcx
        .update(|_, cx| cx.read_from_clipboard())
        .and_then(|item| item.text())
        .expect("选中文本后右键复制不应为空");
    assert_eq!(
        copied_selection.trim(),
        selected.trim(),
        "有选区时右键复制必须只复制选区"
    );

    vcx.update(|window, cx| gpui::base::TextSelection::clear(window, cx));

    // 回到顶部，右键第一条评论 → 菜单「复制文本」。
    vcx.update(|window, cx| {
        player.update(cx, |view, cx| {
            view.set_test_scroll(0.0);
            cx.notify();
        });
        window.render_frame(cx);
    });
    let press = point(
        px(text_left + 40.0),
        px(yk_theme::theme::TOP_BAR_HEIGHT + 16.0 + 205.0),
    );
    vcx.simulate_mouse_down(press, MouseButton::Right, Modifiers::default());
    vcx.simulate_mouse_up(press, MouseButton::Right, Modifiers::default());
    vcx.update(|window, cx| {
        window.render_frame(cx);
    });

    // 菜单锚在右键位置，首项在菜单内边距之后（与详情页复制测试一致）。
    let menu_item = press + point(px(64.), px(17.));
    vcx.simulate_mouse_down(menu_item, MouseButton::Left, Modifiers::default());
    vcx.simulate_mouse_up(menu_item, MouseButton::Left, Modifiers::default());
    vcx.update(|window, cx| {
        window.render_frame(cx);
    });

    let copied = vcx
        .update(|_, cx| cx.read_from_clipboard())
        .and_then(|item| item.text());
    let text = copied.expect("评论右键菜单必须能把内容写进剪贴板");
    assert!(
        text.contains("第 0 条评论"),
        "评论右键菜单复制了错误内容：{text:?}"
    );
}
