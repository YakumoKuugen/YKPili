//! 平台相关的用户数据目录。
//!
//! 这里是**唯一**决定「配置 / 缓存落在哪」的地方，session、settings、续播、
//! 搜索历史、图片缓存都从这里取路径。
//!
//! | 平台 | 配置 | 缓存 |
//! | --- | --- | --- |
//! | macOS | `~/Library/Application Support/YKPili` | `~/Library/Caches/YKPili` |
//! | Windows | `%APPDATA%\YKPili` | `%LOCALAPPDATA%\YKPili\Cache` |
//! | 其它 | 系统临时目录下的 `YKPili` / `ykpili-cache`（仅测试兜底） |
//!
//! 迁移原则：**macOS 的路径必须保持原样**，否则老用户升级后会丢登录态与续播。

use std::path::PathBuf;

/// 配置目录：登录态、设置、续播、搜索历史。
pub fn config_dir() -> PathBuf {
    #[cfg(target_os = "macos")]
    {
        if let Ok(home) = std::env::var("HOME") {
            return PathBuf::from(home).join("Library/Application Support/YKPili");
        }
    }
    #[cfg(target_os = "windows")]
    {
        if let Some(appdata) = std::env::var_os("APPDATA") {
            return PathBuf::from(appdata).join("YKPili");
        }
        // APPDATA 异常时退回用户目录，避免把登录态写进临时目录。
        if let Some(profile) = std::env::var_os("USERPROFILE") {
            return PathBuf::from(profile)
                .join("AppData")
                .join("Roaming")
                .join("YKPili");
        }
    }
    std::env::temp_dir().join("YKPili")
}

/// 缓存目录：推荐流缓存与图片缓存。
pub fn cache_dir() -> PathBuf {
    #[cfg(target_os = "macos")]
    {
        if let Ok(home) = std::env::var("HOME") {
            return PathBuf::from(home).join("Library/Caches/YKPili");
        }
    }
    #[cfg(target_os = "windows")]
    {
        if let Some(local) = std::env::var_os("LOCALAPPDATA") {
            return PathBuf::from(local).join("YKPili").join("Cache");
        }
    }
    std::env::temp_dir().join("ykpili-cache")
}

/// 配置目录下的一个文件。
pub fn config_file(name: &str) -> PathBuf {
    config_dir().join(name)
}
