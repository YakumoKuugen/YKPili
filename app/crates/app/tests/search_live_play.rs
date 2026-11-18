//! 搜索结果里的直播间点击验收（离屏，不依赖真实网络）。
//!
//! 「直播」Tab 的卡片原本只弹「接口尚未接入」；现在点封面要：
//! ① 创建播放器并进入直播模式，房间号来自搜索结果；
//! ② 结果浮窗同视频起播一样退场，不能继续盖住播放器。
//!
//! 本地验收：`cargo test -p yk-app --test search_live_play`

use std::time::Duration;

use gpui::component::ThemeMode;
use gpui::prelude::*;
use gpui::test::TestWindowExt;
use gpui::{px, size, Bounds, TestAppContext, VisualTestContext, WindowBounds, WindowOptions};
use yk_app::AppRoot;
use yk_infra::search::{SearchLive, SearchResults, SearchTab};

fn results() -> SearchResults {
    SearchResults {
        live: vec![SearchLive {
            roomid: 4321,
            uid: 77,
            title: "搜索结果直播间".into(),
            uname: "测试主播".into(),
            cover_url: String::new(),
            online: 1234,
            area_name: "英雄联盟".into(),
        }],
        ..Default::default()
    }
}

#[gpui::test]
fn clicking_a_live_result_opens_the_player_in_live_mode(cx: &mut TestAppContext) {
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
                let theme = yk_theme::theme::YkTheme::dark();
                yk_theme::gpui_theme::apply(cx, &theme);
                cx.new(|_| {
                    AppRoot::loading()
                        .without_login_prompt()
                        .with_search_results("主播", results(), 1)
                        .with_search_tab(SearchTab::Live)
                })
            },
        )
        .unwrap()
    });

    let mut cx = VisualTestContext::from_window(window.into(), cx);
    let app = window.root(&mut cx).expect("window root");
    cx.update(|window, cx| {
        window.draw(cx).clear(cx);
    });
    assert!(
        app.read_with(&cx, |app, _| app.search_results_open_for_test()),
        "结果浮窗没展开"
    );
    assert!(!app.read_with(&cx, |app, _| app.player_open_for_test()));

    // 点「直播」Tab 里的第一张卡：走真实的整卡点击 → `SearchHost::search_open_live`。
    cx.update(|window, cx| {
        window.click(("search-live", 0usize), cx);
    });
    cx.update(|window, cx| {
        window.draw(cx).clear(cx);
    });

    assert!(
        app.read_with(&cx, |app, _| app.player_open_for_test()),
        "点直播结果没有创建播放器"
    );
    assert!(
        app.read_with(&cx, |app, _| app.search_results_closing_for_test()),
        "起播后结果浮窗没有开始退场：它会一直盖在播放器上"
    );

    let player = app
        .read_with(&cx, |app, _| app.test_player())
        .expect("播放器实体");
    let (is_live, room_id) = player.read_with(&cx, |player, _| {
        (player.test_mode_is_live(), player.test_live_room_id())
    });
    assert!(is_live, "播放器没进入直播模式");
    assert_eq!(room_id, Some(4321), "直播间号没有带进播放器");

    // 退场动画 260ms + 30ms 余量。
    cx.executor().advance_clock(Duration::from_millis(400));
    cx.update(|window, cx| {
        window.draw(cx).clear(cx);
    });
    assert!(
        !app.read_with(&cx, |app, _| app.search_results_open_for_test()),
        "退场动画结束后结果浮窗还在树上"
    );
}
