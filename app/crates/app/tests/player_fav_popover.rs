//! 互动弹层（投币 / 收藏夹 / 分享）的行为验收：
//!
//! 1. **浮层命中**：弹层走 `deferred + anchored` 画在正常元素树之外，
//!    点卡片上的按钮必须还能点中（画在上层 ≠ 命中也落在上层）。
//! 2. **点外面关闭**：卡片外垫了一层整窗点击层，点视频 / 面板空白都应该收起弹层。
//!
//! 本地验收：`cargo test -p yk-app --test player_fav_popover`

use gpui::test::TestWindowExt;
use gpui::{
    point, px, size, AnyWindowHandle, AppContext, Bounds, Entity, Modifiers, ScrollDelta,
    ScrollWheelEvent, TestAppContext, TestDispatcher, TouchPhase, VisualTestContext, WindowBounds,
    WindowOptions,
};
use yk_app::player::{set_test_cursor, PlayerView};
use yk_app::AppRoot;

const WINDOW_W: f32 = 1280.0;
const WINDOW_H: f32 = 820.0;

/// 开一个 1280×820 的播放器窗口，收藏夹弹层默认展开。
fn open_player(cx: &mut TestAppContext) -> (Entity<PlayerView>, AnyWindowHandle) {
    cx.update(|cx| {
        let theme = yk_theme::theme::YkTheme::dark();
        let player = cx.new(|cx| {
            PlayerView::new(theme.clone(), cx)
                .with_open_panels(false, false, true)
                .with_expanded(true)
        });
        let app = cx.new(|_| AppRoot::new().with_player(player.clone()));
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
                gpui::component::Theme::change(gpui::component::ThemeMode::Dark, Some(window), cx);
                yk_theme::gpui_theme::apply(cx, &theme);
                app.clone()
            },
        )
        .unwrap();
        (player, handle)
    })
}

/// 渲染一帧，并把指针停在展开后的面板里（避免 hover 看门狗把侧栏收回去）。
fn settle(vcx: &mut VisualTestContext, panel_left: f32) {
    let cursor = point(px(panel_left + 40.0), px(320.0));
    set_test_cursor(Some(cursor));
    vcx.simulate_mouse_move(cursor, None, Modifiers::default());
    vcx.update(|window, cx| {
        window.render_frame(cx);
    });
}

fn main() {
    let mut cx = TestAppContext::build(TestDispatcher::new(0), None);
    cx.update(|cx| {
        gpui::init(cx);
        gpui::component::init(cx);
    });

    // PlayerView 全进程只能有一个实例（输入框状态跟着实例走），
    // 所以两个场景复用同一个窗口，中间用测试钩子重新打开弹层。
    let (player, handle) = open_player(&mut cx);
    let mut vcx = VisualTestContext::from_window(handle, &cx);
    let viewport = vcx.update(|window, _| window.viewport_size());
    let panel_left = viewport.width.as_f32() - 16.0 - 400.0;
    settle(&mut vcx, panel_left);

    // ---------- ① 点卡片里的关闭按钮 ----------
    assert!(
        vcx.update(|_, cx| player.read(cx).action_panels_open().1),
        "夹具没把收藏夹弹层打开"
    );
    let bounds = vcx
        .update(|_, cx| player.read(cx).test_action_row_bounds())
        .expect("互动行几何没被 prepaint 记录，浮层就没有定位依据");
    // 卡片：left = 互动行左缘 + 16，top = 互动行下沿 + 4；
    // 头行 32 高、卡片内边距 4、关闭按钮 22 宽贴右。
    let card_left = bounds.origin.x.as_f32() + 16.0;
    let card_top = bounds.origin.y.as_f32() + bounds.size.height.as_f32() + 4.0;
    let close = point(px(card_left + 345.0), px(card_top + 20.0));
    eprintln!("[fav-popover] ① 关闭按钮 = {close:?}");

    vcx.simulate_click(close, Modifiers::default());
    vcx.update(|window, cx| {
        window.render_frame(cx);
    });
    assert!(
        !vcx.update(|_, cx| player.read(cx).action_panels_open().1),
        "点浮层里的关闭按钮没反应：deferred 浮层没接住命中，点击穿透到下面了"
    );
    eprintln!("[fav-popover] ① 浮层命中正常");

    // ---------- ② 弹层开着时，指针离开面板也不收起面板 ----------
    vcx.update(|_, cx| {
        player.update(cx, |view, cx| view.test_open_fav_overlay(cx));
    });
    settle(&mut vcx, panel_left);
    assert!(
        vcx.update(|_, cx| player.read(cx).action_panels_open().1),
        "重新打开收藏夹弹层失败"
    );

    // 指针跑到视频中间（明显在面板 hover 区外），等过收起延迟再看。
    let away = point(px(320.0), px(320.0));
    set_test_cursor(Some(away));
    vcx.simulate_mouse_move(away, None, Modifiers::default());
    vcx.update(|window, cx| {
        window.render_frame(cx);
    });
    vcx.executor().advance_clock(std::time::Duration::from_millis(800));
    vcx.update(|window, cx| {
        window.render_frame(cx);
    });
    assert!(
        vcx.update(|_, cx| player.read(cx).panel_expanded()),
        "弹层还没收回，面板就因为指针离开收起了"
    );
    eprintln!("[fav-popover] ② 弹层开着时面板被钉住");

    // ---------- ③ 点卡片外面：关弹层，随后面板照常收起 ----------

    // 视频区中央：离收藏夹卡片很远，只会命中整窗点击层。
    let outside = point(px(viewport.width.as_f32() * 0.3), px(300.0));
    eprintln!("[fav-popover] ③ 外点 = {outside:?}");
    vcx.simulate_click(outside, Modifiers::default());
    vcx.update(|window, cx| {
        window.render_frame(cx);
    });
    assert!(
        !vcx.update(|_, cx| player.read(cx).action_panels_open().1),
        "点弹层外面没有收起弹层"
    );

    // 弹层关了就没理由再把面板钉住：指针还在外面，等过收起延迟应当收回去。
    vcx.executor().advance_clock(std::time::Duration::from_millis(800));
    vcx.update(|window, cx| {
        window.render_frame(cx);
    });
    assert!(
        !vcx.update(|_, cx| player.read(cx).panel_expanded()),
        "弹层关掉之后面板还赖着不收起"
    );
    eprintln!("[fav-popover] ③ 外点关闭 + 面板回收正常");

    // ---------- ④ 收藏夹长列表可以在卡片内部滚动 ----------
    player.update_in(&mut vcx, |view, _, cx| {
        view.test_open_fav_overlay(cx);
    });
    settle(&mut vcx, panel_left);
    player.update_in(&mut vcx, |view, _, cx| {
        view.test_expand_fav_list(cx);
    });
    vcx.update(|window, cx| {
        window.render_frame(cx);
    });
    let bounds = vcx
        .update(|_, cx| player.read(cx).test_action_row_bounds())
        .expect("互动行几何没被 prepaint 记录");
    let card_left = bounds.origin.x.as_f32() + 16.0;
    let card_top = bounds.origin.y.as_f32() + bounds.size.height.as_f32() + 4.0;
    let before = player.read_with(&vcx, |view, _| view.test_fav_scroll_offset());
    let scroll_at = point(px(card_left + 180.0), px(card_top + 300.0));
    set_test_cursor(Some(scroll_at));
    vcx.simulate_mouse_move(scroll_at, None, Modifiers::default());
    vcx.simulate_event(ScrollWheelEvent {
        position: scroll_at,
        delta: ScrollDelta::Pixels(point(px(0.0), px(-240.0))),
        modifiers: Modifiers::default(),
        touch_phase: TouchPhase::Moved,
    });
    vcx.update(|window, cx| {
        window.render_frame(cx);
    });
    let after = player.read_with(&vcx, |view, _| view.test_fav_scroll_offset());
    assert!(
        (after - before).abs() > 1.0,
        "收藏夹展开后应该能滚动（before={before}, after={after}）"
    );
    eprintln!("[fav-popover] ④ 收藏夹列表滚动正常（{before} -> {after}）");

    eprintln!("[fav-popover] 全部通过");
}
