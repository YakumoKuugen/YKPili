//! 分享面板「分享播放进度」按钮的行为验收：
//!
//! 1. 写入的是**哔哩哔哩网页链接**，带当前分 P 与 `t=当前秒`
//!    （`https://www.bilibili.com/video/BV...?p=2&t=90`）——
//!    QQ 等聊天软件不会把 `ykpili://` 渲染成可点链接；
//! 2. 面板里不再有「复制 YKPili 通用链接」这一行；
//! 3. 点完收起分享面板。
//!
//! 这里点的是浮层里真实的按钮（按 `debug_selector` 取几何），
//! 不是直接调控制器方法 —— 要能抓到「按钮没接上」这类问题。
//!
//! 本地验收：`cargo test -p yk-app --test player_share_link`

use gpui::test::TestWindowExt;
use gpui::{
    point, px, size, AnyWindowHandle, AppContext, Bounds, Entity, Modifiers, Pixels, Point,
    TestAppContext, TestDispatcher, VisualTestContext, WindowBounds, WindowOptions,
};
use yk_app::player::{set_test_cursor, PlayerView};
use yk_app::AppRoot;

const WINDOW_W: f32 = 1280.0;
const WINDOW_H: f32 = 820.0;
const BVID: &str = "BV1mG1QBNEFN";
/// 分 P 2 + 播放到 90 秒：两个参数都得出现在链接里。
const PAGE: i32 = 2;
const POSITION: f64 = 90.0;

/// 开一个播放器窗口：分享面板默认展开，播放源只灌分享需要的字段（不取流）。
fn open_player(cx: &mut TestAppContext) -> (Entity<PlayerView>, AnyWindowHandle) {
    cx.update(|cx| {
        let theme = yk_theme::theme::YkTheme::dark();
        let player = cx.new(|cx| {
            PlayerView::new(theme.clone(), cx)
                .with_open_panels(true, false, false)
                .with_share_source(BVID, "", PAGE, POSITION)
        });
        let app = cx.new(|_| AppRoot::new().with_player(player.clone()));
        let (handle, _root) = gpui::open_window(
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
                gpui::component::Theme::change(gpui::component::ThemeMode::Dark, Some(window), cx);
                yk_theme::gpui_theme::apply(cx, &theme);
                app.clone()
            },
        )
        .unwrap();
        (player, handle)
    })
}

/// 渲染一帧，并把指针停在展开后的面板里（避免 hover 看门狗把侧栏收回去）。
fn settle(vcx: &mut VisualTestContext) {
    let viewport = vcx.update(|window, _| window.viewport_size());
    // 信息面板收起态 200 宽、贴窗口右边界。
    let panel_left = viewport.width.as_f32() - 16.0 - 200.0;
    let cursor = point(px(panel_left + 40.0), px(320.0));
    set_test_cursor(Some(cursor));
    vcx.simulate_mouse_move(cursor, None, Modifiers::default());
    vcx.update(|window, cx| {
        window.render_frame(cx);
    });
}

/// 分享面板是 `deferred + anchored` 浮层：先重画几帧等 prepaint 记下几何。
fn row_center(vcx: &mut VisualTestContext, selector: &'static str) -> Point<Pixels> {
    for _ in 0..4 {
        vcx.update(|window, cx| {
            window.render_frame(cx);
        });
        if let Some(bounds) = vcx.debug_bounds(selector) {
            return bounds.center();
        }
    }
    panic!("没量到分享行 {selector} 的几何");
}

fn clipboard_text(vcx: &mut VisualTestContext) -> Option<String> {
    vcx.update(|_, cx| cx.read_from_clipboard())
        .and_then(|item| item.text())
}

fn main() {
    let mut cx = TestAppContext::build(TestDispatcher::new(0), None);
    cx.update(|cx| {
        gpui::init(cx);
        gpui::component::init(cx);
    });

    let (player, handle) = open_player(&mut cx);
    let mut vcx = VisualTestContext::from_window(handle, &cx);
    settle(&mut vcx);

    // ---------- ① 面板里只有 B 站网页链接这一种分享入口 ----------
    assert!(
        vcx.debug_bounds("player-share-app-link").is_none(),
        "「复制 YKPili 通用链接」已移除：QQ 里自定义协议点不动"
    );

    // ---------- ② 分享播放进度：B 站链接 + p / t ----------
    let center = row_center(&mut vcx, "player-share-progress");
    vcx.simulate_click(center, Modifiers::default());
    vcx.update(|window, cx| {
        window.render_frame(cx);
    });
    assert_eq!(
        clipboard_text(&mut vcx).as_deref(),
        Some("https://www.bilibili.com/video/BV1mG1QBNEFN?p=2&t=90"),
        "「分享播放进度」必须写 B 站链接，并用 p / t 带上分 P 与秒数"
    );
    assert!(
        !vcx.update(|_, cx| player.read(cx).action_panels_open().2),
        "分享播放进度后分享面板应当收起"
    );
    eprintln!("[player-share-link] 带 p / t 的 B 站链接写入剪贴板");

    eprintln!("[player-share-link] 全部通过");
}
