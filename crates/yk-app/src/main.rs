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
        // 卡片封面操作图标（clock / download / message-circle 等）不在
        // 组件库的默认子集里，必须注册完整图标目录，否则 `Icon` 取不到 SVG。
        .with_assets(assets::AllAssets)
        .run(|cx| {
            init(cx);

            // 首屏策略：先把「能立刻显示的内容」铺上，网络请求移到窗口里异步做。
            // - 有缓存：先显示缓存，请求回来后原地替换；
            // - 无缓存：首屏骨架（设计文档第 05 节），请求回来后回填；
            // - YK_OFFLINE：直接用缓存 / 占位数据，不显示骨架。
            let cache = yk_core::feed_cache::FeedCache::new();
            let forced_offline = std::env::var("YK_OFFLINE").is_ok();
            // 空缓存不当作可显示内容，否则首屏会是一张空网格而不是骨架。
            let cached = cache.load().filter(|c| !c.items.is_empty());

            if forced_offline {
                eprintln!("[offline] YK_OFFLINE=1，跳过网络请求");
                match &cached {
                    Some(c) => eprintln!(
                        "[offline] 使用缓存：{} 条（{}）",
                        c.items.len(),
                        yk_core::feed_cache::age_text(c.updated_at)
                    ),
                    None => eprintln!("[offline] 没有可用缓存，使用占位数据"),
                }
            } else if let Some(c) = &cached {
                eprintln!(
                    "[cache] 首屏先显示缓存：{} 条（{}），后台刷新",
                    c.items.len(),
                    yk_core::feed_cache::age_text(c.updated_at)
                );
            } else {
                eprintln!("[api] 无缓存，首屏显示卡片骨架");
            }

            open_window(
                WindowOptions {
                    window_bounds: Some(WindowBounds::Windowed(Bounds {
                        origin: point(px(120.), px(80.)),
                        size: size(px(1280.), px(820.)),
                    })),
                    titlebar: Some(TitlebarOptions {
                        title: Some("YKPili".into()),
                        // 原生标题栏透明 + FullSizeContentView：去掉白色标题条，
                        // 红黄绿按钮直接压在应用顶栏上；顶栏左侧为按钮让出空间。
                        appears_transparent: true,
                        traffic_light_position: Some(point(px(20.), px(18.))),
                    }),
                    // 顶栏自己接管拖动（见 lib.rs `top_bar`）：不交出这一项，
                    // AppKit 只让原生 titlebar 那条窄带可拖，顶栏下半部分按不住，
                    // 而且内容区也会跟系统拖拽争同一串鼠标事件。
                    app_owns_titlebar_drag: true,
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
                    static BUILD_N: std::sync::atomic::AtomicUsize =
                        std::sync::atomic::AtomicUsize::new(0);
                    let n = BUILD_N.fetch_add(1, std::sync::atomic::Ordering::SeqCst) + 1;
                    eprintln!("[build] 窗口 build 闭包第 {n} 次执行");
                    // 窗口创建时构造一次播放器（若指定了 YK_VIDEO）。
                    let theme = yk_ui::theme::YkTheme::dark();
                    let want_player =
                        std::env::var("YK_VIDEO").is_ok() || std::env::var("YK_BVID").is_ok();
                    let player = if want_player {
                        let player = cx.new(|cx| yk_app::player::PlayerView::new(theme, cx));
                        // 验收入口：YK_BVID=BV... 直接走和卡片点击同一条取流链路。
                        if let Ok(bvid) = std::env::var("YK_BVID") {
                            let _ = player.update(cx, |view, cx| view.open_bvid(bvid, cx));
                        }
                        Some(player)
                    } else {
                        None
                    };
                    // 启动时恢复上次登录；无效/损坏的会话自动忽略。
                    let saved_session = if std::env::var("YK_FORCE_LOGIN").is_ok() {
                        None
                    } else {
                        yk_core::session::load(&yk_core::session::session_path())
                            .filter(|s| s.is_valid())
                    };
                    let mut app = if forced_offline {
                        match cached {
                            Some(c) => {
                                AppRoot::with_real_data(&c.items).with_offline(Some(c.updated_at))
                            }
                            None => AppRoot::new().with_offline(None),
                        }
                    } else {
                        match cached {
                            Some(c) => AppRoot::with_real_data(&c.items),
                            None => AppRoot::loading(),
                        }
                    };
                    if let Some(p) = player {
                        app = app.with_player(p);
                    }
                    app = app.with_session(saved_session);
                    // 验收入口：YK_PAGE=following 直接落到关注瀑布流。
                    let start_page =
                        std::env::var("YK_PAGE")
                            .ok()
                            .and_then(|page| match page.as_str() {
                                "following" => Some(yk_app::pages::PageKind::Following),
                                "hot" => Some(yk_app::pages::PageKind::Hot),
                                "rank" => Some(yk_app::pages::PageKind::Rank),
                                "live" => Some(yk_app::pages::PageKind::Live),
                                "history" => Some(yk_app::pages::PageKind::History),
                                _ => None,
                            });
                    if let Some(page) = start_page {
                        app = app.with_page(page);
                    }
                    let entity = cx.new(|_| app);
                    // 页面图片缓存：滚动时按「可视 + 一行」驱逐卡片图，
                    // 同时释放 CPU 解码资源与窗口图集条目。
                    let page_image_cache = yk_app::page_image_cache::PageImageCache::new(cx);
                    let _ = entity.update(cx, |app, _| app.set_page_image_cache(page_image_cache));
                    let _ = entity.update(cx, |app, cx| app.install_player_close_handler(cx));
                    if !forced_offline {
                        // 首屏请求放到窗口里异步跑：窗口先出骨架/缓存，不阻塞启动。
                        let _ = entity.update(cx, |app, cx| app.start_initial_load(cx));
                    }
                    let _ = entity.update(cx, |app, cx| app.load_current_page(cx));
                    entity
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
