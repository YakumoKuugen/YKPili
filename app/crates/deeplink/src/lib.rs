//! 跨平台深链适配组件。
//!
//! 目标：**同一套链接模型**同时服务 Windows 和 macOS。
//!
//! | 平台 | 入口 | 实现 |
//! | --- | --- | --- |
//! | Windows | `ykpili://...` | 当前用户注册表 + 命名管道单实例转发 |
//! | macOS | `ykpili://...` | `CFBundleURLTypes` + GPUI `on_open_urls` |
//! | macOS | `https://go.ykpili.app/...` | `continueUserActivity` 运行时注入 + Associated Domains |
//!
//! Universal Link 有一个绕不开的前提：**域名必须由应用开发者控制**，
//! 并且在该域名部署 `apple-app-site-association`。第三方无法给
//! `bilibili.com` 注册 Universal Link，所以 B 站链接在浏览器里走
//! `ykpili://` 自定义协议；`go.ykpili.app` 只用于「自己可控的分享域名」。
//! 详见 `docs/深链与UniversalLink.md`。

pub mod model;
pub mod queue;

mod platform;

pub use model::{first_link_in_args, DeepLink, VideoLink, SCHEME, UNIVERSAL_LINK_HOST};
pub use queue::{DeepLinkHandle, IncomingLink, LinkSource};

/// 进程级队列句柄。`AppRoot::new` 用它，不触发任何平台副作用。
pub fn handle() -> DeepLinkHandle {
    queue::global().clone()
}

/// 初始化平台适配层，并返回可轮询的队列句柄。
///
/// 幂等：重复调用不会重复注册协议 / 启动管道线程。
/// Windows 上必须在 `forward_link_before_ui()` 返回 `false` 之后调用。
pub fn install() -> DeepLinkHandle {
    use std::sync::OnceLock;
    static INSTALLED: OnceLock<()> = OnceLock::new();
    let handle = queue::global().clone();
    if INSTALLED.set(()).is_ok() {
        platform::install();
    }
    handle
}

/// 从任意线程投递一条深链（macOS 的 GPUI `on_open_urls` 回调走这里）。
pub fn ingest(raw: impl Into<String>, source: LinkSource) {
    queue::ingest(raw, source);
}

/// Windows 启动早期：如果已经有一个实例在跑，把命令行里的深链转发过去。
///
/// 返回 `true` 表示「已转发，当前进程应当直接退出」。
/// 其它平台恒为 `false`。
pub fn forward_link_before_ui() -> bool {
    platform::forward_link_before_ui()
}
