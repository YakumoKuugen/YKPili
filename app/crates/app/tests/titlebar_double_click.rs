//! 顶栏双击的行为验收：双击要能被顶栏拖动区接住，并且不打断窗口。
//!
//! headless 平台的 `titlebar_double_click` 是 no-op（真机才会按系统设置
//! 缩放 / 最小化 / 填充），而且测试窗口没有原生 HWND，`toggle_maximize`
//! 也是 no-op。所以这里回归的是接线与重入安全：事件走真实元素树、命中顶栏
//! 拖动区、连按两下不 panic、双击之后窗口还能继续出帧。
//! 平台动作本身靠手动验收（见 `docs/未完成任务清单.md` 的顶栏条目）。
//!
//! 判定依据是顶栏自己的「时间 + 位移」追踪器（不是 `click_count`，原因见
//! `shell::top_bar::TitlebarClickTracker` 的注释），所以这里发两次独立的
//! 按下，而不是一条 `click_count = 2` 的合成事件。拖动要等鼠标移动才开始，
//! 这里不模拟移动，所以两下都不会进拖动循环——test platform 的
//! `start_window_move` 是 `unimplemented!()`，走进去这条测试会直接 panic。
//!
//! Linux 跳过：test platform 的 `zoom()` 是 `unimplemented!()`，双击会走到
//! `zoom_window()` 直接 panic；Linux 实机不在本次验收范围。

#![cfg(not(target_os = "linux"))]

use gpui::component::ThemeMode;
use gpui::prelude::*;
use gpui::{
    point, px, size, Bounds, MouseButton, MouseDownEvent, MouseUpEvent, TestAppContext,
    VisualTestContext, WindowBounds, WindowOptions,
};

#[gpui::test]
fn double_click_on_top_bar_is_swallowed_by_drag_region(cx: &mut TestAppContext) {
    let window = cx.update(|cx| {
        gpui::init(cx);
        cx.open_window(
            WindowOptions {
                window_bounds: Some(WindowBounds::Windowed(Bounds {
                    origin: Default::default(),
                    size: size(px(1280.), px(820.)),
                })),
                ..Default::default()
            },
            |window, cx| {
                gpui::component::Theme::change(ThemeMode::Dark, Some(window), cx);
                yk_theme::gpui_theme::apply(cx, &yk_theme::theme::YkTheme::dark());
                cx.new(|_| yk_app::AppRoot::new().without_login_prompt())
            },
        )
        .unwrap()
    });

    let mut cx = VisualTestContext::from_window(window.into(), cx);
    // 首帧画出来，元素树与 hitbox 才会就位。
    cx.update(|window, cx| {
        window.draw(cx).clear(cx);
    });

    // 顶栏空白拖动区：左侧头像胶囊之后、居中搜索胶囊之前，(200, 16)
    // 正好在 32 高顶栏中线，也不会命中任何子控件。
    let position = point(px(200.), px(16.));
    let modifiers = Default::default();
    let before = cx.update(|window, _| window.window_bounds());

    // 连按两下：第一下判成单击（待命拖动），第二下判成双击。
    // 两步都不会真的开拖动循环——拖动要等鼠标移动，这里不模拟移动。
    for _ in 0..2 {
        cx.simulate_event(MouseDownEvent {
            button: MouseButton::Left,
            position,
            modifiers,
            click_count: 1,
            first_mouse: false,
        });
        cx.simulate_event(MouseUpEvent {
            button: MouseButton::Left,
            position,
            modifiers,
            click_count: 1,
        });
    }

    // 窗口不能因此被挪动，而且还得能继续画帧。
    cx.update(|window, cx| {
        window.draw(cx).clear(cx);
    });
    assert_eq!(
        cx.update(|window, _| window.window_bounds()),
        before,
        "双击不应改变窗口 bounds"
    );
}
