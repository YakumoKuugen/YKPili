//! 评论区浮动输入条的交互验收：
//!
//! ① 默认收起成右下角一个圆形按钮，点一下才展开成输入条；
//! ② 评论往下滚、「回到顶部」浮出时，输入条被顶到它上方（42px）；
//! ③ 点回到顶部 / 自己滚回顶部后，「回到顶部」隐藏，输入条落回右下角；
//! ④ 输入条展开时，点它以外的任何位置（含侧边面板以外的视频区 / 顶栏）收回按钮。
//!
//! ②③ 都是逐帧插值，所以这里逐帧渲染、等动画跑完再量位置。
//!
//! 本地验收：`cargo test -p yk-app --test comment_composer`

use std::time::Duration;

use gpui::component::ThemeMode;
use gpui::prelude::*;
use gpui::test::TestWindowExt;
use gpui::{
    point, px, size, Bounds, TestAppContext, VisualTestContext, WindowBounds, WindowOptions,
};
use yk_app::player::{set_test_cursor, PlayerView};
use yk_app::AppRoot;
use yk_core::dynamic::DynamicComment;

const WINDOW_W: f32 = 1280.0;
const WINDOW_H: f32 = 820.0;
/// 收起态圆形按钮直径。
const FAB: f32 = 40.0;
/// 展开后输入条宽 = 面板 400 − 左右各 16。
const BAR_WIDTH: f32 = 400.0 - 32.0;
/// 被「回到顶部」顶起的距离 = 按钮 34 + 间距 8。
const LIFT: f32 = 42.0;

fn comments(count: usize) -> Vec<DynamicComment> {
    (0..count)
        .map(|i| DynamicComment {
            rpid: i as i64 + 1,
            author: format!("用户{i}"),
            face: String::new(),
            text: format!(
                "第 {i} 条评论：再写长一点，保证评论区能滚起来，也能验证滚动后按钮的状态。"
            ),
            time: format!("{} 天前", i % 30 + 1),
            like: i as u64,
            pictures: Vec::new(),
            is_top: false,
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
    let theme = yk_ui::theme::YkTheme::dark();
    let (handle, player) = cx.update(|cx| {
        gpui::init(cx);
        gpui::component::init(cx);
        let theme_for_player = theme.clone();
        let player = cx.new(|cx| {
            PlayerView::new(theme_for_player, cx)
                .with_expanded(true)
                .with_comments(comments(60), 12_345, false)
        });
        let player_for_root = player.clone();
        let (handle, _root) = gpui::open_window(
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
                yk_ui::gpui_theme::apply(cx, &theme);
                cx.new(|_| {
                    AppRoot::new()
                        .without_login_prompt()
                        .with_player(player_for_root)
                })
            },
        )
        .unwrap();
        (handle, player)
    });
    let mut vcx = VisualTestContext::from_window(handle, cx);
    vcx.update(|window, cx| {
        window.render_frame(cx);
    });
    // 光标钉在侧栏里，避免 hover 看门狗把面板收掉、改变布局。
    set_test_cursor(Some(point(px(960.), px(300.))));
    (handle, player, vcx)
}

/// 逐帧渲染一小会儿，把位移 / 宽度动画跑完。
fn settle(vcx: &mut VisualTestContext) {
    // 元素动画读的是真实时钟（测试时钟只推定时器），所以要真的等一小会儿。
    for _ in 0..50 {
        std::thread::sleep(Duration::from_millis(12));
        vcx.cx.executor().advance_clock(Duration::from_millis(16));
        vcx.update(|window, cx| {
            window.render_frame(cx);
        });
    }
}

fn bounds_of(vcx: &mut VisualTestContext, id: &'static str) -> Option<Bounds<gpui::Pixels>> {
    vcx.update(|window, _| window.try_find(id).map(|snapshot| snapshot.bounds()))
}

fn scroll_to(vcx: &mut VisualTestContext, player: &gpui::Entity<PlayerView>, y: f32) {
    vcx.update(|window, cx| {
        player.update(cx, |view, cx| {
            view.set_test_scroll(y);
            cx.notify();
        });
        window.render_frame(cx);
    });
}

#[gpui::test]
fn composer_collapses_expands_and_lifts(cx: &mut TestAppContext) {
    let (_handle, player, mut vcx) = open_comments(cx);

    // ① 默认收起：只有圆形按钮，没有发送按钮。
    let fab = bounds_of(&mut vcx, "comment-fab").expect("默认应显示收起态的圆形按钮");
    assert!(
        (fab.size.width.as_f32() - FAB).abs() <= 2.0,
        "圆形按钮宽度应为 {FAB}，实际 {}",
        fab.size.width.as_f32()
    );
    assert!(
        bounds_of(&mut vcx, "comment-send").is_none(),
        "收起态不该渲染发送按钮"
    );

    // 点一下 → 展开成输入条并直接拿到焦点（否则用户还要再点一次才能打字）。
    vcx.update(|window, cx| window.click("comment-fab", cx));
    settle(&mut vcx);
    assert_eq!(
        vcx.update(|_, cx| player.read(cx).test_composer_state()),
        (true, true),
        "点开输入条后应展开并聚焦"
    );
    assert!(
        bounds_of(&mut vcx, "comment-fab").is_none(),
        "展开后不该再显示圆形按钮"
    );
    assert!(
        bounds_of(&mut vcx, "comment-send").is_some(),
        "展开后应出现发送按钮"
    );
    let bar = bounds_of(&mut vcx, "comment-composer").expect("展开后应找到输入条");
    assert!(
        (bar.size.width.as_f32() - BAR_WIDTH).abs() <= 2.0,
        "展开后输入条宽度应为 {BAR_WIDTH}，实际 {}",
        bar.size.width.as_f32()
    );
    let resting_bottom = bar.bottom();

    // ② 往下滚：回到顶部浮出，输入条被顶起 42px。
    scroll_to(&mut vcx, &player, -800.0);
    assert!(
        bounds_of(&mut vcx, "comment-back-to-top").is_some(),
        "滚动超过阈值后应浮出「回到顶部」"
    );
    // 动画刚起步：这一帧应该还在原位，说明是「滑上去」而不是瞬移。
    let lift_start = bounds_of(&mut vcx, "comment-composer").expect("刚起动画时输入条还在");
    assert!(
        (lift_start.bottom().as_f32() - resting_bottom.as_f32()).abs() < 3.0,
        "顶起动画应从原位开始：静止 {}, 起步 {}",
        resting_bottom.as_f32(),
        lift_start.bottom().as_f32()
    );
    settle(&mut vcx);
    let lifted = bounds_of(&mut vcx, "comment-composer").expect("顶起后输入条还在");
    assert!(
        (resting_bottom.as_f32() - lifted.bottom().as_f32() - LIFT).abs() < 0.5,
        "输入条应被顶起 {LIFT}px：静止 {}, 顶起后 {}",
        resting_bottom.as_f32(),
        lifted.bottom().as_f32()
    );

    // ③ 自己滚回顶部：按钮隐藏，输入条滑回右下角。
    scroll_to(&mut vcx, &player, 0.0);
    assert!(
        bounds_of(&mut vcx, "comment-back-to-top").is_none(),
        "滚回顶部后「回到顶部」应隐藏"
    );
    let drop_start =
        bounds_of(&mut vcx, "comment-composer").expect("刚起落回动画时输入条还在");
    assert!(
        (drop_start.bottom().as_f32() - lifted.bottom().as_f32()).abs() < 3.0,
        "落回动画应从顶起位置开始：顶起 {}, 起步 {}",
        lifted.bottom().as_f32(),
        drop_start.bottom().as_f32()
    );
    settle(&mut vcx);
    let back = bounds_of(&mut vcx, "comment-composer").expect("落回后输入条还在");
    assert!(
        (back.bottom().as_f32() - resting_bottom.as_f32()).abs() < 0.5,
        "输入条应落回原位：期望 {}, 实际 {}",
        resting_bottom.as_f32(),
        back.bottom().as_f32()
    );

    // ④ 再滚下去，这次点「回到顶部」：同样隐藏按钮 + 滑回右下角。
    scroll_to(&mut vcx, &player, -800.0);
    settle(&mut vcx);
    assert!(
        bounds_of(&mut vcx, "comment-back-to-top").is_some(),
        "第二次滚动后应重新浮出「回到顶部」"
    );
    vcx.update(|window, cx| window.click("comment-back-to-top", cx));
    vcx.update(|window, cx| {
        window.render_frame(cx);
    });
    assert!(
        bounds_of(&mut vcx, "comment-back-to-top").is_none(),
        "点了回到顶部后该按钮应隐藏"
    );
    settle(&mut vcx);
    let back = bounds_of(&mut vcx, "comment-composer").expect("落回后输入条还在");
    assert!(
        (back.bottom().as_f32() - resting_bottom.as_f32()).abs() < 0.5,
        "点回到顶部后也应落回原位：期望 {}, 实际 {}",
        resting_bottom.as_f32(),
        back.bottom().as_f32()
    );
    // ⑤ 点输入条以外的位置（视频区 / 顶栏）都收回圆形按钮，点输入条本身不收回。
    vcx.update(|window, cx| window.click("comment-fab", cx));
    settle(&mut vcx);
    assert_eq!(
        vcx.update(|_, cx| player.read(cx).test_composer_state()),
        (true, true),
        "点回到顶部之后应能再次点开输入条"
    );

    let bar = bounds_of(&mut vcx, "comment-composer").expect("应找到输入条");
    let inside = bar.center();
    vcx.simulate_mouse_down(inside, gpui::MouseButton::Left, gpui::Modifiers::default());
    vcx.simulate_mouse_up(inside, gpui::MouseButton::Left, gpui::Modifiers::default());
    vcx.update(|window, cx| {
        window.render_frame(cx);
    });
    assert_eq!(
        vcx.update(|_, cx| player.read(cx).test_composer_state()),
        (true, true),
        "点输入条自身不该收起"
    );

    let video = point(px(320.), px(320.));
    vcx.simulate_mouse_down(video, gpui::MouseButton::Left, gpui::Modifiers::default());
    vcx.simulate_mouse_up(video, gpui::MouseButton::Left, gpui::Modifiers::default());
    vcx.update(|window, cx| {
        window.render_frame(cx);
    });
    assert_eq!(
        vcx.update(|_, cx| player.read(cx).test_composer_state()),
        (false, false),
        "点侧边面板以外（视频区）应收起"
    );
    assert!(
        bounds_of(&mut vcx, "comment-fab").is_some(),
        "收起后应恢复圆形按钮"
    );

    vcx.update(|window, cx| window.click("comment-fab", cx));
    settle(&mut vcx);
    assert_eq!(
        vcx.update(|_, cx| player.read(cx).test_composer_state()),
        (true, true),
        "第二次点圆形按钮应能再展开"
    );
    let top = point(px(700.), px(24.));
    vcx.simulate_mouse_down(top, gpui::MouseButton::Left, gpui::Modifiers::default());
    vcx.simulate_mouse_up(top, gpui::MouseButton::Left, gpui::Modifiers::default());
    vcx.update(|window, cx| {
        window.render_frame(cx);
    });
    assert_eq!(
        vcx.update(|_, cx| player.read(cx).test_composer_state()),
        (false, false),
        "点顶栏也应收起评论输入条"
    );
}
