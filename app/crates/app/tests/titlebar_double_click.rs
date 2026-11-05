//! 顶栏双击的行为验收：双击要能被顶栏拖动区接住，并且不打断窗口。
//!
//! headless 平台的 `titlebar_double_click` 是 no-op（真机才会按系统设置
//! 缩放 / 最小化 / 填充），所以这里回归的是接线与重入安全：事件走真实
//! 元素树、命中顶栏拖动区、不 panic，双击之后窗口还能继续出帧。
//! 平台动作本身靠手动验收（见 `docs/未完成任务清单.md` 的顶栏条目）。
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

    cx.simulate_event(MouseDownEvent {
        button: MouseButton::Left,
        position,
        modifiers,
        click_count: 2,
        first_mouse: false,
    });
    cx.simulate_event(MouseUpEvent {
        button: MouseButton::Left,
        position,
        modifiers,
        click_count: 2,
    });

    // 平台动作在 headless 下是 no-op；这里真正回归的是「第二次按下没有
    // 落进拖动分支」—— test platform 的 start_window_move 是 unimplemented，
    // 一旦走错分支这条测试会直接 panic。同时窗口不能因此被挪动。
    cx.update(|window, cx| {
        window.draw(cx).clear(cx);
    });
    assert_eq!(
        cx.update(|window, _| window.window_bounds()),
        before,
        "双击不应改变窗口 bounds"
    );
}
