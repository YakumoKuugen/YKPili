//! ⌘Q 退出确认的行为验收：快捷键必须真的能把提示叫出来。
//!
//! 只验证「第一次按键 → 提示可见」，不真的触发第二次（那会退掉测试进程）。

use gpui::component::ThemeMode;
use gpui::prelude::*;
use gpui::{px, size, Bounds, TestAppContext, VisualTestContext, WindowBounds, WindowOptions};

#[gpui::test]
fn cmd_q_shows_quit_hint(cx: &mut TestAppContext) {
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
                yk_ui::gpui_theme::apply(cx, &yk_ui::theme::YkTheme::dark());
                cx.new(|_| yk_app::AppRoot::new().without_login_prompt())
            },
        )
        .unwrap()
    });

    let mut cx = VisualTestContext::from_window(window.into(), cx);
    let app = window.root(&mut cx).expect("window root");
    // 先把首帧画出来，再把焦点放到搜索框：key listener 挂在元素树上，
    // 未绘制 / 无焦点时派发树是空的。
    cx.update(|window, cx| {
        window.draw(cx).clear(cx);
    });
    app.update_in(&mut cx, |app, window, cx| {
        app.focus_search_for_test(window, cx);
    });
    assert!(!app.read_with(&cx, |app, _| app.quit_hint_visible()));

    cx.simulate_keystrokes("cmd-q");

    assert!(
        app.read_with(&cx, |app, _| app.quit_hint_visible()),
        "第一次 ⌘Q 应该显示退出提示，而不是直接退出"
    );
}

/// 没有任何输入框 / 播放器持有焦点时，⌘Q 也必须能叫出退出提示。
///
/// 根因回归：⌘Q 监听挂在 AppRoot 根节点的 capture 阶段；无焦点时 GPUI
/// 只派发到 `base::Root`，AppRoot 不在路径上，快捷键就完全没反应。
/// 以前打开播放器能用，只是因为播放器顺手把焦点放进了 AppRoot 子树。
#[gpui::test]
fn cmd_q_works_without_any_focus(cx: &mut TestAppContext) {
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
                yk_ui::gpui_theme::apply(cx, &yk_ui::theme::YkTheme::dark());
                cx.new(|_| yk_app::AppRoot::new().without_login_prompt())
            },
        )
        .unwrap()
    });

    let mut cx = VisualTestContext::from_window(window.into(), cx);
    let app = window.root(&mut cx).expect("window root");
    // 不点搜索框、不打开播放器：只把首帧画出来，让根节点自己接管焦点。
    cx.update(|window, cx| {
        window.draw(cx).clear(cx);
    });
    cx.run_until_parked();
    assert!(!app.read_with(&cx, |app, _| app.quit_hint_visible()));

    cx.simulate_keystrokes("cmd-q");
    assert!(
        app.read_with(&cx, |app, _| app.quit_hint_visible()),
        "无焦点时第一次 ⌘Q 也应该显示退出提示"
    );
}
