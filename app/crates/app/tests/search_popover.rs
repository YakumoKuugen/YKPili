//! 顶栏搜索浮层的行为验收：点胶囊展开，Esc 走收回动画。
//!
//! 头部动作（开 / 合）靠真实元素树验证：胶囊必须吞掉 mouse_down（否则
//! 会走窗口拖动，test platform 的 start_window_move 会直接 panic），
//! 展开后的输入框要进元素树，Esc 才能沿着它找到顶栏的 capture 监听。

use gpui::component::ThemeMode;
use gpui::prelude::*;
use gpui::{
    point, px, size, Bounds, MouseButton, MouseDownEvent, MouseUpEvent, TestAppContext,
    VisualTestContext, WindowBounds, WindowOptions,
};

#[gpui::test]
fn pill_opens_and_escape_starts_close_animation(cx: &mut TestAppContext) {
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
    let app = window.root(&mut cx).expect("window root");
    cx.update(|window, cx| {
        window.draw(cx).clear(cx);
    });
    assert!(!app.read_with(&cx, |app, _| app.search_open_for_test()));

    // 收起态胶囊中心：1280 宽窗口，(1280 − 260) / 2 + 130 = 640；胶囊高 24，中心 y = 16。
    let at = point(px(640.), px(16.));
    let modifiers = Default::default();
    cx.simulate_event(MouseDownEvent {
        button: MouseButton::Left,
        position: at,
        modifiers,
        click_count: 1,
        first_mouse: false,
    });
    cx.simulate_event(MouseUpEvent {
        button: MouseButton::Left,
        position: at,
        modifiers,
        click_count: 1,
    });
    assert!(
        app.read_with(&cx, |app, _| app.search_open_for_test()),
        "点胶囊应该展开搜索浮层"
    );

    // 浮层里的输入框要进元素树，Esc 才能沿着它找到顶栏。
    cx.update(|window, cx| {
        window.draw(cx).clear(cx);
    });
    cx.simulate_keystrokes("escape");
    assert!(
        !app.read_with(&cx, |app, _| app.search_open_for_test()),
        "Esc 应该收起搜索浮层"
    );
    assert!(
        app.read_with(&cx, |app, _| app.search_closing_for_test()),
        "收起必须先播回收动画，而不是直接消失"
    );
}

/// 收回动画落地后，焦点不能悬在已经卸载的输入框上：
/// 否则 ⌘Q 的根节点 capture 不在派发路径里，退出提示会失灵。
#[gpui::test]
fn closing_search_returns_focus_to_root(cx: &mut TestAppContext) {
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
    let app = window.root(&mut cx).expect("window root");
    cx.update(|window, cx| {
        window.draw(cx).clear(cx);
    });

    // 直接走「静态展开」路径，省掉点击胶囊那一步。
    app.update_in(&mut cx, |app, window, cx| {
        app.focus_search_for_test(window, cx);
    });
    cx.update(|window, cx| {
        window.draw(cx).clear(cx);
    });
    cx.simulate_keystrokes("escape");
    assert!(app.read_with(&cx, |app, _| app.search_closing_for_test()));

    // 把收回动画的定时器跑完，再画一帧让浮层真正卸载。
    for _ in 0..12 {
        cx.cx
            .executor()
            .advance_clock(std::time::Duration::from_millis(30));
        cx.run_until_parked();
    }
    cx.update(|window, cx| {
        window.draw(cx).clear(cx);
    });
    assert!(!app.read_with(&cx, |app, _| app.search_closing_for_test()));
    assert!(cx.update(|window, cx| window.focused(cx)).is_some());

    // 根节点重新拿到焦点后，⌘Q 的两次提示机制必须照常工作。
    let quit = if cfg!(target_os = "windows") {
        "alt-q"
    } else {
        "cmd-q"
    };
    assert!(!app.read_with(&cx, |app, _| app.quit_hint_visible()));
    cx.simulate_keystrokes(quit);
    assert!(
        app.read_with(&cx, |app, _| app.quit_hint_visible()),
        "收起搜索后 ⌘Q 仍然要能叫出退出提示"
    );
}
