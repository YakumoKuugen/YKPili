//! 播放器侧栏（信息面板）hover 展开 / 收起的**行为**验收。
//!
//! 用 GPUI 测试平台模拟真实鼠标移动，复现两条用户反馈：
//! 1. 侧栏展开后，鼠标在面板上从右往左跑，面板不该自己收回；
//! 2. 鼠标进入右侧 strip 展开侧栏后，不该反复弹出 / 回收。
//!
//! 本地验收：`cargo test -p yk-app --test player_hover`

use gpui::test::TestWindowExt;
use gpui::{
    point, px, size, AppContext, Bounds, Modifiers, TestAppContext, TestDispatcher,
    VisualTestContext, WindowBounds, WindowOptions,
};
use yk_app::player::{set_test_cursor, PlayerView};
use yk_app::AppRoot;

const WINDOW_W: f32 = 1280.0;
const WINDOW_H: f32 = 820.0;

fn main() {
    let mut cx = TestAppContext::build(TestDispatcher::new(0), None);
    cx.update(|cx| {
        gpui::init(cx);
        gpui::component::init(cx);
    });
    let theme = yk_theme::theme::YkTheme::dark();

    let (player, handle) = cx.update(|cx| {
        let player = cx.new(|cx| PlayerView::new(theme.clone(), cx));
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
    });

    let mut vcx = VisualTestContext::from_window(handle, &cx);
    vcx.update(|window, cx| {
        window.render_frame(cx);
    });
    let viewport = vcx.update(|window, _| window.viewport_size());
    eprintln!("[player-hover] viewport={viewport:?}");

    // 侧栏右缘 = 窗口右缘内缩 16；收起态面板宽 200，展开态 400。
    let panel_right = viewport.width.as_f32() - 16.0;
    let collapsed_left = panel_right - 200.0;
    let expanded_left = panel_right - 400.0;

    // 光标位置由测试固定：headless 环境下系统光标在别的窗口上，
    // 侧栏判定读到的位置和这里模拟的鼠标对不上。
    let mut move_to = |vcx: &mut VisualTestContext, x: f32, y: f32| {
        set_test_cursor(Some(point(px(x), px(y))));
        vcx.simulate_mouse_move(point(px(x), px(y)), None, Modifiers::default());
        vcx.update(|window, cx| {
            window.render_frame(cx);
        });
    };
    let expanded =
        |vcx: &mut VisualTestContext| vcx.update(|_, cx| player.read(cx).panel_expanded());

    let y = 200.0;

    // ① 鼠标进入右侧 strip → 侧栏展开。
    move_to(&mut vcx, panel_right - 100.0, y);
    eprintln!(
        "[player-hover] 移入 strip 后 expanded={}",
        expanded(&mut vcx)
    );
    assert!(expanded(&mut vcx), "鼠标进入右侧 strip 后面板没有展开");

    // ② 面板展开后，鼠标在面板上从右往左跑（跑到 strip 左边、但仍在面板内）：
    //    面板必须保持展开，不能自己收回。
    move_to(&mut vcx, expanded_left + 40.0, y);
    eprintln!(
        "[player-hover] 移到面板左侧（x={:.0}）后 expanded={}",
        expanded_left + 40.0,
        expanded(&mut vcx)
    );
    assert!(
        expanded(&mut vcx),
        "鼠标在面板内、但跑到 strip 左边时面板被收回了"
    );

    // ③ 跑出面板左侧 → 允许收起（延迟 220ms），这里只看它不会立刻误判。
    move_to(&mut vcx, expanded_left - 80.0, y);
    eprintln!("[player-hover] 跑出面板后 expanded={}", expanded(&mut vcx));

    // ④ 再回到面板内 → 必须能重新展开，不能卡在收起态。
    move_to(&mut vcx, collapsed_left + 50.0, y);
    eprintln!(
        "[player-hover] 回到 strip 内 expanded={}",
        expanded(&mut vcx)
    );
    assert!(expanded(&mut vcx), "鼠标回到面板内后面板没有重新展开");

    eprintln!("[player-hover] 侧栏 hover 行为符合预期");
}
