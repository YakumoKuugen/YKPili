//! 评论区「最热 / 最新」排序的行为验收。
//!
//! 之前这两段只是死文字：请求里 `mode` 固定 3，点哪个都一样。
//! 这里点真实的「最新」tab，断言三件事：
//! ① 排序状态真的切到 `Newest`；
//! ② 旧列表被清空（游标不跨排序复用，必须从第一页重拉）；
//! ③ tab 的命中区在元素树里存在（不是画上去的纯文字）。
//!
//! 本地验收：`cargo test -p yk-app --test comment_sort`

use gpui::component::ThemeMode;
use gpui::prelude::*;
use gpui::test::TestWindowExt;
use gpui::{
    px, size, Bounds, Entity, TestAppContext, VisualTestContext, WindowBounds, WindowOptions,
};
use yk_app::player::PlayerView;
use yk_infra::dynamic::DynamicComment;
use yk_player::comment_section::CommentSort;

fn comments(count: usize) -> Vec<DynamicComment> {
    (0..count)
        .map(|i| DynamicComment {
            rpid: i as i64 + 1,
            author: format!("用户{i}"),
            text: format!("第 {i} 条评论"),
            ..Default::default()
        })
        .collect()
}

fn open_player(cx: &mut TestAppContext) -> (VisualTestContext, Entity<PlayerView>) {
    let (window, player) = cx.update(|cx| {
        gpui::init(cx);
        gpui::open_window(
            WindowOptions {
                window_bounds: Some(WindowBounds::Windowed(Bounds {
                    origin: Default::default(),
                    size: size(px(1280.), px(820.)),
                })),
                ..Default::default()
            },
            cx,
            |window, cx| {
                gpui::component::Theme::change(ThemeMode::Dark, Some(window), cx);
                let theme = yk_theme::theme::YkTheme::dark();
                yk_theme::gpui_theme::apply(cx, &theme);
                cx.new(|cx| {
                    PlayerView::new(theme.clone(), cx)
                        .with_expanded(true)
                        .with_comments(comments(6), 6, false)
                })
            },
        )
        .unwrap()
    });
    let mut vcx = VisualTestContext::from_window(window, cx);
    for _ in 0..3 {
        vcx.update(|window, cx| {
            window.draw(cx).clear(cx);
        });
    }
    (vcx, player)
}

#[gpui::test]
fn newest_tab_switches_sort_and_clears_old_list(cx: &mut TestAppContext) {
    let (mut vcx, player) = open_player(cx);

    assert_eq!(
        vcx.update(|_, cx| player.read(cx).test_comment_sort()),
        CommentSort::Hot,
        "默认排序必须是最热（与旧行为一致）"
    );
    assert_eq!(
        vcx.update(|_, cx| player.read(cx).test_comment_count()),
        6,
        "夹具应该先灌进 6 条评论"
    );

    let tab_id = ("player-comment-sort", 1usize);
    let bounds = vcx
        .update(|window, _| window.try_find(tab_id))
        .expect("找不到「最新」tab：元素没有注册进观测树（缺 test_support？）")
        .bounds();
    eprintln!("[comment-sort] 「最新」tab = {bounds:?}");
    vcx.update(|window, cx| {
        window.click(tab_id, cx);
    });
    vcx.update(|window, cx| {
        window.draw(cx).clear(cx);
    });

    assert_eq!(
        vcx.update(|_, cx| player.read(cx).test_comment_sort()),
        CommentSort::Newest,
        "点了「最新」但排序没变"
    );
    assert_eq!(
        vcx.update(|_, cx| player.read(cx).test_comment_count()),
        0,
        "换排序必须清空旧列表：游标不跨排序复用，不能在原列表上原地重排"
    );
    eprintln!("[comment-sort] 点「最新」后列表已清空，等待 mode=2 的第一页");
}
