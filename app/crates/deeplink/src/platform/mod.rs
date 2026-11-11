//! 平台分派。只保留最小的条件编译面，让业务代码永远只调用 `lib.rs` 的 API。

#[cfg(target_os = "windows")]
pub mod windows;

#[cfg(target_os = "macos")]
pub mod macos;

pub fn install() {
    #[cfg(target_os = "windows")]
    windows::install();

    #[cfg(target_os = "macos")]
    macos::install();
}

pub fn forward_link_before_ui() -> bool {
    #[cfg(target_os = "windows")]
    {
        return windows::forward_link_before_ui();
    }

    #[cfg(not(target_os = "windows"))]
    {
        false
    }
}
