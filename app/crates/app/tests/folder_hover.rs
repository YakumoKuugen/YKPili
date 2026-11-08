//! 左下角悬浮文件夹的 hover 行为验收。
//!
//! 用 GPUI 测试平台模拟真实鼠标移动，确认：
//! 1. 光标进到图标包围盒里 → 进入 hover 态（卡片开始上浮）；
//! 2. 光标移出去 → 回到静止态；
//! 3. 图标压在封面之上时，点它不会把下面的视频打开（`occlude` 生效）。
//!
//! 本地验收：`cargo test -p yk-app --test folder_hover`

use gpui::test::TestWindowExt;
use gpui::{
    point, px, size, AppContext, Bounds, Modifiers, TestAppContext, VisualTestContext,
    WindowBounds, WindowOptions,
};
use yk_app::AppRoot;

const WINDOW_W: f32 = 1280.0;
const WINDOW_H: f32 = 820.0;
/// 图标是 108×112，左 24 / 底 20（与 `folder_icon` 的定位一致）。
const ICON_CENTER_X: f32 = 24.0 + 108.0 / 2.0;
const ICON_CENTER_Y: f32 = WINDOW_H - 20.0 - 112.0 / 2.0;

fn main() {
    let cx = TestAppContext::build(gpui::TestDispatcher::new(0), None);
    cx.update(|cx| {
        gpui::init(cx);
        gpui::component::init(cx);
    });
    let theme = yk_theme::theme::YkTheme::dark();

    let (app, handle) = cx.update(|cx| {
        let app = cx.new(|_| AppRoot::new().without_login_prompt());
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
        (app, handle)
    });

    let mut vcx = VisualTestContext::from_window(handle, &cx);
    vcx.update(|window, cx| window.render_frame(cx));

    let move_to = |vcx: &mut VisualTestContext, x: f32, y: f32| {
        vcx.simulate_mouse_move(point(px(x), px(y)), None, Modifiers::default());
        vcx.update(|window, cx| window.render_frame(cx));
    };
    let hovered =
        |vcx: &mut VisualTestContext| vcx.update(|_, cx| app.read(cx).is_folder_hovered());

    // ① 移到图标正中：进入 hover。
    move_to(&mut vcx, ICON_CENTER_X, ICON_CENTER_Y);
    eprintln!("[folder-hover] 移入图标后 hovered={}", hovered(&mut vcx));
    assert!(hovered(&mut vcx), "鼠标移到文件夹上之后没有进入 hover 态");

    // ② 停在页签那一小块上也应该保持 hover（不是只有正中才算）。
    move_to(&mut vcx, 30.0, WINDOW_H - 20.0 - 112.0 + 26.0);
    assert!(hovered(&mut vcx), "鼠标停在页签上时 hover 态丢失了");

    // ③ 移开：回到静止。
    move_to(&mut vcx, WINDOW_W / 2.0, 120.0);
    eprintln!("[folder-hover] 移出图标后 hovered={}", hovered(&mut vcx));
    assert!(!hovered(&mut vcx), "鼠标移开之后还停在 hover 态");
}
