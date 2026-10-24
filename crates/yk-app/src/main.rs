//! YKPili 入口。
//!
//! 两处必须按 gpui-kit 的约定来，否则窗口会开起来立刻退出：
//! 1. `gpui::init(cx)` 要在最前面调用（初始化 gpui-base / gpui-component）；
//! 2. 用 gpui-kit 的 `open_window(options, cx, build)`，它会替我们包一层
//!    `base::Root` —— 视图直接挂到 `cx.open_window` 上会缺少 Root 提供的
//!    事件与图层管理。

use gpui::*;
use yk_app::AppRoot;

fn main() {
    application()
        .with_assets(assets::Assets)
        .run(|cx| {
            init(cx);

            // M7：启动时尝试拉一页真实推荐。
            // 失败不阻塞启动 —— 退回占位数据，并在终端留下一行原因。
            let real_items = match yk_core::api::client::ReqwestClient::new() {
                Ok(client) => {
                    match yk_core::api::client::fetch_recommend(
                        &client,
                        "",
                        30,
                        yk_core::api::client::now_wts(),
                    ) {
                        Ok(items) => {
                            eprintln!("[api] 真实推荐拉取成功：{} 条", items.len());
                            Some(items)
                        }
                        Err(e) => {
                            eprintln!("[api] 真实推荐拉取失败，退回占位数据：{e}");
                            None
                        }
                    }
                }
                Err(e) => {
                    eprintln!("[api] HTTP 客户端初始化失败：{e}");
                    None
                }
            };
            open_window(
                WindowOptions {
                    window_bounds: Some(WindowBounds::Windowed(Bounds {
                        origin: point(px(120.), px(80.)),
                        size: size(px(1280.), px(820.)),
                    })),
                    titlebar: Some(TitlebarOptions {
                        title: Some("YKPili".into()),
                        ..Default::default()
                    }),
                    ..Default::default()
                },
                cx,
                |window, cx| {
                    // M4：主题覆盖必须发生在**窗口创建之后**。
                    // `init` 只装了组件库默认主题；窗口创建时会再走一次
                    // Theme::change，把默认色值装回来 —— 在 init 之后、
                    // open_window 之前覆盖会被这次 change 冲刷掉。
                    gpui::component::Theme::change(
                        gpui::component::ThemeMode::Dark,
                        Some(window),
                        cx,
                    );
                    yk_ui::gpui_theme::apply(cx, &yk_ui::theme::YkTheme::dark());
                    match real_items {
                        Some(items) => cx.new(|_| AppRoot::with_real_data(&items)),
                        None => cx.new(|_| AppRoot::new()),
                    }
                },
            )
            .expect("打开窗口失败");
        });
}
