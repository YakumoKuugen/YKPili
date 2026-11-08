//! 错误日志：只在报错时落盘，每次启动清空上一次会话。
//!
//! 设计约束：
//! - **只记录错误**：正常运行不产生任何写入，避免日志被进度 / 帧率刷屏；
//! - **启动即清空**：应用启动时删除旧日志，本次运行没有报错就不会留下文件；
//! - **失败静默**：日志 IO 出错绝不能影响播放，所有错误都吞掉，stderr 照常输出；
//! - **有上限**：单次运行超过 2 MB 自动截断，防止循环报错把磁盘写满。
//!
//! 路径：`<cache_dir>/logs/player.log`（macOS `~/Library/Caches/YKPili/logs/`，
//! Windows `%LOCALAPPDATA%\YKPili\Cache\logs\`）。测试 / 验收可用
//! `YK_LOG_DIR` 覆盖目录。
//!
//! 调用方式：
//! ```ignore
//! yk_infra::error_log::init();              // main 启动时一次
//! yk_infra::error_log::error("player", "取流失败：...");
//! ```
//! `error` 同时打印到 stderr，保持 debug 构建 / 终端启动时的原有体验。

use std::fs::{self, File, OpenOptions};
use std::io::Write;
use std::path::PathBuf;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Mutex, OnceLock};

use chrono::Local;

/// 单次运行的日志上限：超过就从头截断（只保留之后的内容）。
const MAX_BYTES: u64 = 2 * 1024 * 1024;

struct Sink {
    file: File,
    path: PathBuf,
}

static ENABLED: AtomicBool = AtomicBool::new(false);
static SINK: OnceLock<Mutex<Option<Sink>>> = OnceLock::new();

fn sink() -> &'static Mutex<Option<Sink>> {
    SINK.get_or_init(|| Mutex::new(None))
}

/// 日志目录。
pub fn log_dir() -> PathBuf {
    if let Ok(dir) = std::env::var("YK_LOG_DIR") {
        if !dir.trim().is_empty() {
            return PathBuf::from(dir);
        }
    }
    crate::platform::cache_dir().join("logs")
}

/// 日志文件路径。
pub fn path() -> PathBuf {
    log_dir().join("player.log")
}

/// 应用启动时调用一次：清空上一次会话的日志，并安装 panic hook。
///
/// 这里**不创建空文件**：本次运行没有报错，日志目录里就不会出现新文件。
/// 重复调用是安全的（例如测试里重建 AppRoot）。
pub fn init() {
    // 先标记启用，再清空文件：即使清空失败，后续报错也会尝试重建。
    ENABLED.store(true, Ordering::Relaxed);
    let path = path();
    let _ = fs::remove_file(&path);
    // 丢弃上一会话已经打开的句柄，下一次报错时按新路径重新打开。
    *sink().lock().unwrap_or_else(|error| error.into_inner()) = None;
    install_panic_hook();
}

/// 记录一条错误。
///
/// `scope` 对应原来 stderr 方括号里的标签（`player` / `video` / `mpv` …），
/// 输出形如 `2026-11-08 21:03:12.345 [player] 取流失败：...`。
pub fn error(scope: &str, message: impl AsRef<str>) {
    let message = message.as_ref();
    // 终端行为保持不变：debug 构建 / 从终端启动仍能实时看到。
    eprintln!("[{scope}] {message}");
    if !ENABLED.load(Ordering::Relaxed) {
        return;
    }
    append(scope, message);
}

/// 落盘；只在 [`error`] 与 panic hook 里调用。
fn append(scope: &str, message: &str) {
    let mut guard = sink().lock().unwrap_or_else(|error| error.into_inner());
    write_locked(&mut guard, scope, message);
}

/// 打开 / 轮转 / 写入。`guard` 由调用方加锁。
fn write_locked(guard: &mut Option<Sink>, scope: &str, message: &str) {
    if guard.is_none() {
        *guard = open_sink();
    }
    let Some(sink) = guard.as_mut() else {
        return;
    };
    if sink.file.metadata().map(|meta| meta.len()).unwrap_or(0) > MAX_BYTES {
        if let Ok(file) = OpenOptions::new()
            .create(true)
            .write(true)
            .truncate(true)
            .open(&sink.path)
        {
            sink.file = file;
            let _ = writeln!(
                sink.file,
                "----- 本次运行日志超过 2 MB，已截断，仅保留之后的内容 -----"
            );
        }
    }
    let now = Local::now().format("%Y-%m-%d %H:%M:%S%.3f");
    let _ = writeln!(sink.file, "{now} [{scope}] {message}");
    // 逐条 flush：崩溃时最后一条错误不能丢在缓冲里。
    let _ = sink.file.flush();
}

fn open_sink() -> Option<Sink> {
    let path = path();
    if let Some(parent) = path.parent() {
        fs::create_dir_all(parent).ok()?;
    }
    let file = OpenOptions::new()
        .create(true)
        .append(true)
        .open(&path)
        .ok()?;
    Some(Sink { file, path })
}

fn install_panic_hook() {
    static INSTALLED: OnceLock<()> = OnceLock::new();
    if INSTALLED.set(()).is_err() {
        return;
    }
    let previous = std::panic::take_hook();
    std::panic::set_hook(Box::new(move |info| {
        // panic 时可能正持有日志锁（或锁已中毒）：用 try_lock，宁可丢这一条
        // 也不能在 panic hook 里再阻塞 / 再 panic。
        if ENABLED.load(Ordering::Relaxed) {
            if let Ok(mut guard) = sink().try_lock() {
                write_locked(&mut guard, "panic", &format!("{info}"));
            }
        }
        // 保留 Rust 默认输出（含 backtrace 开关行为）。
        previous(info);
    }));
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn log_path_ends_with_player_log() {
        assert!(path().ends_with("player.log"));
    }

    #[test]
    fn log_dir_is_under_cache_dir_by_default() {
        // 不设置 YK_LOG_DIR 时，日志跟着缓存目录走。
        if std::env::var_os("YK_LOG_DIR").is_none() {
            assert!(log_dir().starts_with(crate::platform::cache_dir()));
        }
    }
}
