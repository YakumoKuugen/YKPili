//! 图片查看器开关不能把「半截文本手势」留给评论区。
//!
//! 现象：点评论里的缩略图打开大图、再点背景关掉之后，鼠标在评论区一动，
//! 整段评论正文显示成选中态。
//!
//! 成因：按下缩略图时，窗口级选区层已经进入 `is_selecting`；随后 `on_click`
//! 里的 `stop_propagation` 把 mouse-up 截在选区监听之前，`is_selecting`
//! 永久停在 true，下一次鼠标移动就被当成「从缩略图拖到正文」。
//! 查看器打开 / 关闭时都会清一次窗口选区（见 `ImageViewer::open/close`）。
//!
//! 本地验收：`cargo test -p yk-app --test image_viewer_selection`

use std::time::Duration;

use gpui::component::ThemeMode;
use gpui::prelude::*;
use gpui::test::TestWindowExt;
use gpui::{
    point, px, size, Bounds, Modifiers, TestAppContext, VisualTestContext, WindowBounds,
    WindowOptions,
};
use yk_app::player::{set_test_cursor, PlayerView};
use yk_app::AppRoot;
use yk_infra::dynamic::{DynamicComment, DynamicImage};

const WINDOW_W: f32 = 1280.0;
const WINDOW_H: f32 = 820.0;

fn comments() -> Vec<DynamicComment> {
    vec![DynamicComment {
        rpid: 7,
        author: "用户7".into(),
        face: String::new(),
        text: "这是一条带图片的评论，点开图片之后不应该留下文本选区。".into(),
        time: "1 天前".into(),
        like: 1,
        pictures: vec![DynamicImage {
            url: "https://example.invalid/comment.jpg".into(),
            width: 400,
            height: 400,
        }],
        is_top: false,
    }]
}

#[gpui::test]
fn clicking_comment_image_leaves_no_text_selection(cx: &mut TestAppContext) {
    let theme = yk_theme::theme::YkTheme::dark();
    let (handle, app) = cx.update(|cx| {
        gpui::init(cx);
        gpui::component::init(cx);
        let player = cx.new(|cx| {
            PlayerView::new(theme.clone(), cx)
                .with_expanded(true)
                .with_comments(comments(), 1, true)
        });
        let player_for_root = player.clone();
        let (handle, app) = gpui::open_window(
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
                gpui::component::Theme::change(ThemeMode::Dark, Some(window), cx);
                yk_theme::gpui_theme::apply(cx, &theme);
                cx.new(|cx| {
                    let mut root = AppRoot::new()
                        .without_login_prompt()
                        .with_player(player_for_root);
                    root.install_player_image_handler(cx);
                    root
                })
            },
        )
        .unwrap();
        (handle, app)
    });
    let mut vcx = VisualTestContext::from_window(handle, cx);
    vcx.update(|window, cx| {
        window.render_frame(cx);
    });
    set_test_cursor(Some(point(px(960.), px(300.))));

    // 让入场动画跑完。
    for _ in 0..50 {
        std::thread::sleep(Duration::from_millis(12));
        vcx.cx.executor().advance_clock(Duration::from_millis(16));
        vcx.update(|window, cx| {
            window.render_frame(cx);
        });
    }

    let text_bounds = vcx
        .update(|window, _| {
            window
                .try_find("player-comment-text-7")
                .map(|snapshot| snapshot.bounds())
        })
        .expect("评论文本应当有稳定 id");
    // 缩略图条在正文下方：内容列 gap 2 + 图片条 mt 6，图 72×72。
    let image_bounds = Bounds::new(
        point(text_bounds.left(), text_bounds.bottom() + px(8.0)),
        size(px(72.0), px(72.0)),
    );
    eprintln!("image={image_bounds:?} text={text_bounds:?}");

    vcx.simulate_click(image_bounds.center(), Modifiers::default());
    vcx.update(|window, cx| {
        window.render_frame(cx);
    });
    assert!(
        vcx.update(|_, cx| app.read(cx).test_image_viewer_open(cx)),
        "点评论图片应当打开图片查看器"
    );

    // 真实使用里点完图片，鼠标会继续移动；这一步会把卡住的选区扩到正文上。
    let text_point = point(
        px(text_bounds.left().as_f32() + 80.0),
        text_bounds.center().y,
    );
    vcx.simulate_mouse_move(text_point, None, Modifiers::default());
    vcx.update(|window, cx| {
        window.render_frame(cx);
    });

    let selected = vcx.update(|window, cx| gpui::base::TextSelection::selected_text(window, cx));
    eprintln!("selection after image click = {selected:?}");
    assert!(
        selected.trim().is_empty(),
        "点开图片之后评论区不应出现文本选区：{selected:?}"
    );

    // 点查看器背景关闭：这一次点击同样不能把选区状态留在「拖选中」。
    vcx.simulate_click(point(px(20.0), px(20.0)), Modifiers::default());
    vcx.update(|window, cx| {
        window.render_frame(cx);
    });
    assert!(
        !vcx.update(|_, cx| app.read(cx).test_image_viewer_open(cx)),
        "点查看器背景应当关闭查看器"
    );
    vcx.simulate_mouse_move(text_point, None, Modifiers::default());
    vcx.update(|window, cx| {
        window.render_frame(cx);
    });
    let selected = vcx.update(|window, cx| gpui::base::TextSelection::selected_text(window, cx));
    eprintln!("selection after viewer close = {selected:?}");
    assert!(
        selected.trim().is_empty(),
        "关闭图片查看器之后评论区不应出现文本选区：{selected:?}"
    );
}
