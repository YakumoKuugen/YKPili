//! 播放器键盘快捷键与音量弹出条的行为验收。
//!
//! 覆盖用户反馈的三条交互：
//! - 空格暂停 / 继续；
//! - ← / → 后退 / 前进 5 秒（并夹在片长内）；
//! - ↑ / ↓ 调整音量，音量图标弹出可拖动的音量条。
//!
//! 本地验收：`cargo test -p yk-app --test player_shortcuts`

use gpui::prelude::*;
use gpui::test::TestWindowExt;
use gpui::{
    px, size, AnyWindowHandle, Bounds, TestAppContext, VisualTestContext, WindowBounds,
    WindowOptions,
};
use yk_app::player::{CoverSource, PlayerView};
use yk_app::AppRoot;

fn open_player(cx: &mut TestAppContext) -> (gpui::Entity<PlayerView>, AnyWindowHandle) {
    cx.update(|cx| {
        gpui::init(cx);
        gpui::component::init(cx);
    });
    let theme = yk_theme::theme::YkTheme::dark();
    cx.update(|cx| {
        let player = cx.new(|cx| PlayerView::new(theme.clone(), cx));
        let app = cx.new(|_| AppRoot::new().with_player(player.clone()));
        let (handle, _root) = gpui::open_window(
            WindowOptions {
                window_bounds: Some(WindowBounds::Windowed(Bounds {
                    origin: Default::default(),
                    size: size(px(1280.), px(820.)),
                })),
                focus: false,
                show: false,
                ..Default::default()
            },
            cx,
            |window, cx| {
                gpui::component::Theme::change(
                    gpui::component::ThemeMode::Dark,
                    Some(window),
                    cx,
                );
                yk_theme::gpui_theme::apply(cx, &theme);
                app.clone()
            },
        )
        .unwrap();
        (player, handle)
    })
}

fn cover_source() -> CoverSource {
    CoverSource {
        bounds: Bounds {
            origin: Default::default(),
            size: size(px(320.), px(180.)),
        },
        root_size: size(px(1280.), px(820.)),
        bvid: "BV1test".into(),
        title: "入场动画测试".into(),
        image_url: String::new(),
        placeholder_color: "#334455".into(),
    }
}

fn state(player: &gpui::Entity<PlayerView>, vcx: &mut VisualTestContext) -> (bool, f64, f64, bool) {
    player.read_with(vcx, |player, _| player.test_playback_state())
}

#[gpui::test]
fn keyboard_controls_playback_and_volume(cx: &mut TestAppContext) {
    let (player, handle) = open_player(cx);
    let mut vcx = VisualTestContext::from_window(handle, &cx);
    // 首帧把元素树和焦点挂上，键事件才有派发路径。
    vcx.update(|window, cx| {
        window.render_frame(cx);
    });
    vcx.run_until_parked();

    // 演示数据：playing = true, position = 256s, volume = 1.0。
    assert_eq!(state(&player, &mut vcx).0, true);

    // 空格：播放 → 暂停。
    vcx.simulate_keystrokes("space");
    assert!(!state(&player, &mut vcx).0, "空格应该暂停播放");

    // → 前进 5 秒，← 后退 5 秒。
    let before = state(&player, &mut vcx).1;
    vcx.simulate_keystrokes("right");
    assert!(
        (state(&player, &mut vcx).1 - (before + 5.0)).abs() < 0.01,
        "→ 应该前进 5 秒"
    );
    vcx.simulate_keystrokes("left");
    assert!(
        (state(&player, &mut vcx).1 - before).abs() < 0.01,
        "← 应该后退 5 秒"
    );

    // ↓ 降低 5%，↑ 升回去。
    let volume = state(&player, &mut vcx).2;
    vcx.simulate_keystrokes("down");
    assert!(
        (state(&player, &mut vcx).2 - (volume - 0.05)).abs() < 1e-6,
        "↓ 应该降低音量"
    );
    vcx.simulate_keystrokes("up");
    assert!(
        (state(&player, &mut vcx).2 - volume).abs() < 1e-6,
        "↑ 应该升高音量"
    );

    // 音量图标：点开弹出条，再按 ↓ 音量条里的数值保持同步。
    player.update_in(&mut vcx, |player, _window, cx| {
        player.test_toggle_volume_popover(cx);
    });
    assert!(
        player.read_with(&vcx, |player, _| player.test_volume_popover_open()),
        "点音量图标应该展开弹出条"
    );
    vcx.update(|window, cx| {
        window.render_frame(cx);
    });
    vcx.simulate_keystrokes("down");
    assert!(
        (state(&player, &mut vcx).2 - (volume - 0.05)).abs() < 1e-6,
        "弹出条展开时方向键仍应改音量"
    );

    // 从卡片点开的真实路径：顶层变成入场动画外壳。
    // 根因回归：快捷键曾只挂在「无入场动画」的普通外壳上，卡片打开时完全失效。
    player.update_in(&mut vcx, |player, _window, cx| {
        player.set_entrance(cover_source(), cx);
    });
    vcx.update(|window, cx| {
        window.render_frame(cx);
    });
    vcx.run_until_parked();

    let playing_before = state(&player, &mut vcx).0;
    vcx.simulate_keystrokes("space");
    assert_ne!(
        state(&player, &mut vcx).0,
        playing_before,
        "卡片入场路径下空格也应该暂停 / 继续"
    );
    let before = state(&player, &mut vcx).1;
    vcx.simulate_keystrokes("right");
    assert!(
        (state(&player, &mut vcx).1 - (before + 5.0)).abs() < 0.01,
        "卡片入场路径下 → 也应该前进 5 秒"
    );

    // F：进入全屏。状态立即翻转，窗口的系统全屏由平台自己完成。
    assert!(
        !player.read_with(&vcx, |player, _| player.is_fullscreen()),
        "刚打开播放器时不应是全屏"
    );
    vcx.simulate_keystrokes("f");
    assert!(
        player.read_with(&vcx, |player, _| player.is_fullscreen()),
        "按 F 应该进入全屏（系统窗口 + 播放器铺满）"
    );
}


