//! 「正在关注 · 动态 / 直播」分栏的离屏验收。
//!
//! 验收点（对应 `docs/关注页-动态与直播分栏.md` 10.2）：
//! ① 点 Dock「正在关注」展开分栏胶囊；
//! ② 切到直播：直播卡出现、动态卡片消失；
//! ③ 切回动态：动态卡片回来、直播列表条数不变（没有重新请求）；
//! ④ 再点一次 Dock：分栏收起，当前内容不变；
//! ⑤ 空态给「查看关注动态」并且能切回动态；
//! ⑥ 未登录时两个分栏都能切，内容是登录卡。
//!
//! 本地验收：`cargo test -p yk-app --test following_live_tabs`

use gpui::component::ThemeMode;
use gpui::prelude::*;
use gpui::test::TestWindowExt;
use gpui::{
    px, size, Bounds, TestAppContext, VisualTestContext, WindowBounds, WindowOptions,
};
use yk_app::{AppRoot, FollowingTab, Route};
use yk_infra::dynamic::{DynamicAuthor, DynamicItem, DynamicKind, DynamicStats, DynamicVideo};
use yk_infra::live::LiveRoom;

fn live_room() -> LiveRoom {
    LiveRoom {
        room_id: 1001,
        uid: 11,
        uname: "生肉烹饪师".into(),
        face: String::new(),
        title: "今天也在认真打排位".into(),
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

fn dynamic_item() -> DynamicItem {
    DynamicItem {
        index: 0,
        id: "dynamic-tab-item".into(),
        kind: DynamicKind::Video,
        author: DynamicAuthor {
            mid: 2,
            name: "视频作者".into(),
            face: String::new(),
        },
        pub_ts: 1_793_000_000,
        pub_time: "2 小时前".into(),
        action: "投稿了视频".into(),
        title: "动态分栏里的视频".into(),
        text: String::new(),
        images: Vec::new(),
        video: Some(DynamicVideo {
            bvid: "BV1xx411c7mD".into(),
            title: "动态分栏里的视频".into(),
            cover: String::new(),
            desc: String::new(),
            duration_text: "12:34".into(),
            play_count: 1_000,
            danmaku_count: 10,
        }),
        link: None,
        stats: DynamicStats::default(),
        comment_target: None,
        origin: None,
        placeholder_color: "#334455".into(),
    }
}

fn open_window(
    cx: &mut TestAppContext,
    app: AppRoot,
) -> (gpui::WindowHandle<AppRoot>, gpui::Entity<AppRoot>) {
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
    let root = window.root(cx).expect("window root");
    (window, root)
}

fn draw(cx: &mut VisualTestContext) {
    cx.update(|window, cx| {
        window.draw(cx).clear(cx);
    });
}

fn has(cx: &mut VisualTestContext, id: impl Into<gpui::ElementId>) -> bool {
    let id = id.into();
    cx.update(|window, _| window.try_find(id).is_some())
}

#[gpui::test]
fn live_tab_switches_lists_without_refetching(cx: &mut TestAppContext) {
    let app = AppRoot::new()
        .without_login_prompt()
        .with_signed_in()
        .with_following(vec![dynamic_item()])
        .with_following_live(vec![live_room()]);
    let (window, app) = open_window(cx, app);
    let mut cx = VisualTestContext::from_window(window.into(), cx);
    draw(&mut cx);

    // 初始停在动态分栏，分页还没展开。
    assert_eq!(
        app.read_with(&cx, |app, _| app.following_tab_for_test()),
        FollowingTab::Videos
    );
    assert!(!app.read_with(&cx, |app, _| app.dock_tabs_open_for_test()));
    assert_eq!(app.read_with(&cx, |app, _| app.following_len_for_test()), 1);
    assert!(!has(&mut cx, ("live-card", 0usize)));

    // ① 再点一次 Dock「正在关注」→ 展开分栏。
    cx.update(|window, cx| {
        app.update(cx, |app, cx| app.dock_select_for_test(1, window, cx));
    });
    draw(&mut cx);
    assert!(app.read_with(&cx, |app, _| app.dock_tabs_open_for_test()));

    // ② 切到直播：直播卡出现，动态卡片消失。
    cx.update(|window, cx| {
        app.update(cx, |app, cx| app.dock_tab_select_for_test(1, window, cx));
    });
    draw(&mut cx);
    assert_eq!(
        app.read_with(&cx, |app, _| app.following_tab_for_test()),
        FollowingTab::Live
    );
    assert!(has(&mut cx, ("live-card", 0usize)), "直播卡没有出现");
    assert_eq!(app.read_with(&cx, |app, _| app.following_len_for_test()), 1);

    // ③ 切回动态：动态列表回来，直播列表条数不变（没有重新请求）。
    cx.update(|window, cx| {
        app.update(cx, |app, cx| app.dock_tab_select_for_test(0, window, cx));
    });
    draw(&mut cx);
    assert_eq!(
        app.read_with(&cx, |app, _| app.following_tab_for_test()),
        FollowingTab::Videos
    );
    assert!(!has(&mut cx, ("live-card", 0usize)));
    assert_eq!(app.read_with(&cx, |app, _| app.following_len_for_test()), 1);
    assert_eq!(app.read_with(&cx, |app, _| app.following_live_len_for_test()), 1);

    // ④ 再点一次 Dock：分栏收起，页面内容不变。
    cx.update(|window, cx| {
        app.update(cx, |app, cx| app.dock_select_for_test(1, window, cx));
    });
    draw(&mut cx);
    assert!(!app.read_with(&cx, |app, _| app.dock_tabs_open_for_test()));
    assert_eq!(app.read_with(&cx, |app, _| app.following_len_for_test()), 1);
}

#[gpui::test]
fn empty_live_tab_offers_a_way_back_to_dynamics(cx: &mut TestAppContext) {
    let app = AppRoot::new()
        .without_login_prompt()
        .with_signed_in()
        .with_following(vec![dynamic_item()])
        .with_following_live(Vec::new())
        .with_following_tab(FollowingTab::Live);
    let (window, app) = open_window(cx, app);
    let mut cx = VisualTestContext::from_window(window.into(), cx);
    draw(&mut cx);

    assert_eq!(
        app.read_with(&cx, |app, _| app.following_tab_for_test()),
        FollowingTab::Live
    );
    assert!(has(&mut cx, "following-live-show-videos"), "空态按钮没出现");

    // 点空态按钮：切回动态分栏。
    cx.update(|window, cx| window.click("following-live-show-videos", cx));
    draw(&mut cx);
    assert_eq!(
        app.read_with(&cx, |app, _| app.following_tab_for_test()),
        FollowingTab::Videos
    );
    assert_eq!(app.read_with(&cx, |app, _| app.following_len_for_test()), 1);
}

#[gpui::test]
fn signed_out_live_tab_shows_the_login_card(cx: &mut TestAppContext) {
    // 从首页点进「正在关注」：未登录会弹登录；分栏与排行榜一样默认收起。
    let app = AppRoot::new()
        .without_login_prompt()
        .with_following_live(vec![live_room()])
        .with_page(Route::Hot);
    let (window, app) = open_window(cx, app);
    let mut cx = VisualTestContext::from_window(window.into(), cx);
    draw(&mut cx);

    cx.update(|window, cx| {
        app.update(cx, |app, cx| app.dock_select_for_test(1, window, cx));
    });
    draw(&mut cx);
    assert!(!app.read_with(&cx, |app, _| app.dock_tabs_open_for_test()));

    // 已经在关注页，再点一次入口才展开胶囊。
    cx.update(|window, cx| {
        app.update(cx, |app, cx| app.dock_select_for_test(1, window, cx));
    });
    draw(&mut cx);
    assert!(app.read_with(&cx, |app, _| app.dock_tabs_open_for_test()));

    cx.update(|window, cx| {
        app.update(cx, |app, cx| app.dock_tab_select_for_test(1, window, cx));
    });
    draw(&mut cx);
    assert_eq!(
        app.read_with(&cx, |app, _| app.following_tab_for_test()),
        FollowingTab::Live
    );
    assert!(
        has(&mut cx, "following-live-login-card"),
        "未登录的直播分栏应该画登录卡"
    );
    assert!(!has(&mut cx, ("live-card", 0usize)));
}
