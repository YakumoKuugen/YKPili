//! 应用设置（跨启动持久化）。
//!
//! 与 [`crate::player::PlayerPrefs`] 的分工：
//! - `PlayerPrefs` 是**播放器自己的**音量 / 静音 / 倍速，随播放状态一起存；
//! - 这里是**用户显式在设置页里改**的全局开关，改动后立即生效、立即落盘。
//!
//! 存储位置与 `session.json` / `resume.json` 同级：
//! macOS 为 `~/Library/Application Support/YKPili/settings.json`；
//! 其它平台退回系统临时目录。测试 / 便携启动可用 `YK_SETTINGS_PATH` 覆盖。
//!
//! **读写模型**：内存里只保留一份（[`current`] / [`update`]），
//! 设置页改完立刻影响正在播放的播放器 —— 不需要重启，也不需要重启播放器。
//! 文件损坏 / 缺字段时退回默认值，不能让一个坏掉的 json 卡住启动。

use std::io;
use std::path::PathBuf;
use std::sync::{OnceLock, RwLock};

use serde::{Deserialize, Serialize};
use yk_theme::theme::SurfaceMaterial;

/// 应用级设置。
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(default)]
pub struct AppSettings {
    /// 播放结束后自动关闭播放浮窗。
    ///
    /// 默认开启：视频放完时浮窗已经没有继续存在的意义，收起后把页面还给用户。
    /// 想留在最后一帧（截图 / 看片尾）的用户可以在设置里关掉。
    pub close_player_on_finish: bool,
    /// 界面材质：液态玻璃 / 原有材质。
    ///
    /// 影响 Dock 选中态与外壳、搜索框、正在关注卡片、排行榜条目。
    /// 默认玻璃：这是当前设计走的材质，切回实色是留给偏好硬朗对比的用户。
    pub surface_material: SurfaceMaterial,
}

impl Default for AppSettings {
    fn default() -> Self {
        Self {
            close_player_on_finish: true,
            surface_material: SurfaceMaterial::Glass,
        }
    }
}

impl AppSettings {
    /// 默认存储路径。
    ///
    /// 优先 `YK_SETTINGS_PATH`（测试 / 便携模式），其次 macOS 的
    /// Application Support / Windows 的 `%APPDATA%\YKPili`，
    /// 最后退回系统临时目录。
    pub fn default_path() -> PathBuf {
        if let Ok(path) = std::env::var("YK_SETTINGS_PATH") {
            if !path.trim().is_empty() {
                return PathBuf::from(path);
            }
        }
        crate::platform::config_file("settings.json")
    }

    /// 从磁盘加载。文件不存在 / json 损坏 / 字段缺失都退回默认值。
    pub fn load(path: impl Into<PathBuf>) -> Self {
        let path = path.into();
        std::fs::read_to_string(path)
            .ok()
            .and_then(|text| serde_json::from_str(&text).ok())
            .unwrap_or_default()
    }

    /// 用默认路径加载。
    pub fn load_default() -> Self {
        Self::load(Self::default_path())
    }

    /// 写盘。失败不致命（本次运行的内存态仍然生效）。
    pub fn save(&self, path: impl AsRef<std::path::Path>) -> io::Result<()> {
        let path = path.as_ref();
        if let Some(parent) = path.parent() {
            std::fs::create_dir_all(parent)?;
        }
        let json = serde_json::to_string_pretty(self)
            .map_err(|e| io::Error::new(io::ErrorKind::Other, e))?;
        std::fs::write(path, json)
    }
}

/// 进程内的唯一设置副本。
static CURRENT: OnceLock<RwLock<AppSettings>> = OnceLock::new();

fn shared() -> &'static RwLock<AppSettings> {
    CURRENT.get_or_init(|| RwLock::new(AppSettings::load_default()))
}

/// 当前设置的内存快照。
///
/// 每次读取都拿一份克隆（结构很小），调用方无需持有锁。
pub fn current() -> AppSettings {
    shared()
        .read()
        .map(|settings| settings.clone())
        .unwrap_or_default()
}

/// 修改设置：先更新内存，再写盘。
///
/// 写盘失败只打日志 —— 设置页的开关本次运行仍然有效，
/// 下次启动退回上一次成功落盘的值。
pub fn update(change: impl FnOnce(&mut AppSettings)) {
    let mut settings = match shared().write() {
        Ok(guard) => guard,
        // 另一个线程持锁时 panic 过：内存态仍可用，取回内部值继续。
        Err(poisoned) => poisoned.into_inner(),
    };
    change(&mut settings);
    if let Err(error) = settings.save(AppSettings::default_path()) {
        eprintln!("[settings] 写盘失败：{error}");
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn temp_path(name: &str) -> PathBuf {
        std::env::temp_dir()
            .join(format!("yk-settings-{}-{name}", std::process::id()))
            .join("settings.json")
    }

    #[test]
    fn defaults_to_close_on_finish() {
        assert!(AppSettings::default().close_player_on_finish);
    }

    #[test]
    fn roundtrip_keeps_values() {
        let path = temp_path("roundtrip");
        let settings = AppSettings {
            close_player_on_finish: false,
            ..Default::default()
        };
        settings.save(&path).unwrap();
        assert_eq!(AppSettings::load(&path), settings);
        let _ = std::fs::remove_dir_all(path.parent().unwrap());
    }

    #[test]
    fn missing_file_uses_default() {
        let path = temp_path("missing");
        let _ = std::fs::remove_dir_all(path.parent().unwrap());
        assert_eq!(AppSettings::load(&path), AppSettings::default());
    }

    #[test]
    fn corrupt_file_uses_default() {
        let path = temp_path("corrupt");
        std::fs::create_dir_all(path.parent().unwrap()).unwrap();
        std::fs::write(&path, "{ not json").unwrap();
        assert_eq!(AppSettings::load(&path), AppSettings::default());
        let _ = std::fs::remove_dir_all(path.parent().unwrap());
    }

    /// 旧版本 / 手写配置文件缺字段时，用该字段的默认值补齐。
    #[test]
    fn missing_fields_fall_back_to_defaults() {
        let path = temp_path("partial");
        std::fs::create_dir_all(path.parent().unwrap()).unwrap();
        std::fs::write(&path, "{}").unwrap();
        assert!(AppSettings::load(&path).close_player_on_finish);
        // 未来新增字段也不该让旧文件整份失效。
        std::fs::write(&path, r#"{"some_future_field": 1}"#).unwrap();
        assert!(AppSettings::load(&path).close_player_on_finish);
        let _ = std::fs::remove_dir_all(path.parent().unwrap());
    }
}
