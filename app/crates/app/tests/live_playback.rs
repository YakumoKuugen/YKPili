//! 直播起播接线验收（离屏，不依赖真实网络）。
//!
//! 点关注页的直播封面 → 创建播放浮窗 → 播放器立刻进入直播模式
//! （`live_room_id` 正确、面板是「弹幕 / 直播信息」、控制栏有 LIVE 角标）。
//!
//! 本地验收：`cargo test -p yk-app --test live_playback`

use gpui::component::ThemeMode;
use gpui::prelude::*;
use gpui::test::TestWindowExt;
use gpui::{px, size, Bounds, TestAppContext, VisualTestContext, WindowBounds, WindowOptions};
use yk_app::{AppRoot, FollowingTab};
use yk_infra::live::LiveRoom;

fn live_room() -> LiveRoom {
    LiveRoom {
        room_id: 1001,
        uid: 11,
        uname: "生肉烹饪师".into(),
        face: String::new(),
        title: "今晚打排位".into(),
        cover: String::new(),
        area_name: "英雄联盟".into(),
        watched: 30_000,
        watched_text: "3.0万".into(),
        watched_full: "3.0万人看过".into(),
        online: 999,
        live_status: 1,
        followed: Some(true),
        started_at: Some(1_793_000_000),
    }
}

#[gpui::test]
fn clicking_a_live_card_opens_the_player_in_live_mode(cx: &mut TestAppContext) {
    let app = AppRoot::new()
        .without_login_prompt()
        .with_signed_in()
        .with_following_live(vec![live_room()])
        .with_following_tab(FollowingTab::Live);
    let window = cx
        .update(|cx| {
            gpui::init(cx);
            cx.open_window(
                WindowOptions {
                    window_bounds: Some(WindowBounds::Windowed(Bounds {
                        origin: Default::default(),
                        size: size(px(1280.), px(820.)),
                    })),
                    ..Default::default()
                },
                move |window, cx| {
                    gpui::component::Theme::change(ThemeMode::Dark, Some(window), cx);
                    let theme = yk_theme::theme::YkTheme::dark();
                    yk_theme::gpui_theme::apply(cx, &theme);
                    cx.new(|_| app)
                },
            )
        })
        .expect("open window");
    let app = window.root(cx).expect("window root");
    let mut cx = VisualTestContext::from_window(window.into(), cx);
    cx.update(|window, cx| {
        window.draw(cx).clear(cx);
    });

    // 点真实的直播封面（整卡点击 → `FollowingLiveHost::live_open_room`）。
    cx.update(|window, cx| window.click(("live-card", 0usize), cx));
    cx.update(|window, cx| {
        window.draw(cx).clear(cx);
    });

    assert!(app.read_with(&cx, |app, _| app.test_player_open()), "播放器没打开");
    let player = app
        .read_with(&cx, |app, _| app.test_player())
        .expect("播放器实体");
    let (is_live, room_id, tabs, control_live) = player.read_with(&cx, |player, _| {
        (
            player.test_mode_is_live(),
            player.test_live_room_id(),
            player.test_panel_tabs(),
            player.test_control_bar_is_live(),
        )
    });
    assert!(is_live, "播放器没进入直播模式");
    assert_eq!(room_id, Some(1001));
    assert_eq!(tabs, vec!["弹幕", "直播信息"]);
    assert!(control_live, "控制栏应换成 LIVE 形态");
    // 关注直播列表天然已关注：按钮应直接是「已关注」，不再等关系接口。
    assert_eq!(player.read_with(&cx, |player, _| player.test_follow_state()), (true, true));

    // 侧边面板聊天：塞两条消息后重绘，确认列表能真实排版（不连网络）。
    player.update(&mut cx, |player, _| {
        player.test_push_live_chat("甲", "你好");
        player.test_push_live_chat("乙", "主播加油");
    });
    cx.update(|window, cx| {
        window.draw(cx).clear(cx);
    });
    assert_eq!(player.read_with(&cx, |player, _| player.test_live_chat_len()), 2);
}
