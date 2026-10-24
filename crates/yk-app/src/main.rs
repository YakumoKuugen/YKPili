//! YKPili 入口。
//!
//! 两处必须按 gpui-kit 的约定来，否则窗口会开起来立刻退出：
//! 1. `gpui::init(cx)` 要在最前面调用（初始化 gpui-base / gpui-component）；
//! 2. 用 gpui-kit 的 `open_window(options, cx, build)`，它会替我们包一层
//!    `base::Root` —— 视图直接挂到 `cx.open_window` 上会缺少 Root 提供的
//!    事件与图层管理。

use gpui::*;
use yk_app::AppRoot;

/// 桌面入口。
///
/// iOS / Android 上 `gpui_kit::application()` 是**不导出**的 ——
/// 移动端由原生宿主（Swift / Kotlin）启动，Rust 侧只需提供视图构造入口。
/// 见下方 `main` 的 iOS 分支。
#[cfg(not(target_os = "ios"))]
fn main() {
    application()
        // 关键：GPUI 默认是 NullHttpClient（不发请求、静默失败），
        // 换成真实的客户端后 img() 才能拉到图床上的封面。
        .with_http_client(yk_app::http::YkHttpClient::shared())
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

/// iOS：没有 `main` 入口，由宿主 App 调用。
///
/// M8 的下一步是在这里导出 `extern "C"` 的启动函数，
/// 由 Swift 侧创建 `UIWindow` 并挂载 GPUI 的视图。
/// 当前先让整个 workspace 能过 `cargo check --target aarch64-apple-ios`。
#[cfg(target_os = "ios")]
fn main() {
    eprintln!("[ios] YKPili 由原生宿主启动（M8 待接入 UIWindow）");
}
