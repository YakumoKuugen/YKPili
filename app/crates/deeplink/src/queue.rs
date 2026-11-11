//! 深链投递队列。
//!
//! 平台适配层（Windows 命名管道线程、macOS AppDelegate 回调）只负责把
//! **原始 URL 字符串**丢进这里；解析和「打开哪个页面」全部交给 GPUI 主线程。
//! 这样后台线程永远不需要碰 `App` / `Window`，也就不会出现跨线程借用。

use std::collections::VecDeque;
use std::sync::{Arc, Mutex, OnceLock};
use std::time::{Duration, Instant};

/// 链接从哪里来。只用于日志与提示文案，路由逻辑完全相同。
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum LinkSource {
    /// Windows：协议启动参数（`ykpili.exe "ykpili://..."`）。
    WindowsCli,
    /// Windows：已有实例，经命名管道转发。
    WindowsPipe,
    /// macOS：`application:openURLs:`（自定义 URL Scheme）。
    MacOpenUrl,
    /// macOS：`application:continueUserActivity:restorationHandler:`（Universal Link）。
    MacUniversalLink,
    /// 手动投递（测试 / 设置页调试按钮）。
    Manual,
}

impl LinkSource {
    pub fn label(self) -> &'static str {
        match self {
            Self::WindowsCli => "Windows 协议启动",
            Self::WindowsPipe => "Windows 单实例转发",
            Self::MacOpenUrl => "macOS URL Scheme",
            Self::MacUniversalLink => "macOS Universal Link",
            Self::Manual => "手动投递",
        }
    }
}

/// 一条待处理的深链。
#[derive(Debug, Clone)]
pub struct IncomingLink {
    pub raw: String,
    pub source: LinkSource,
    pub received_at: Instant,
}

/// 可克隆的队列句柄。`AppRoot` 持有一份，平台线程持有另一份。
#[derive(Clone)]
pub struct DeepLinkHandle {
    inner: Arc<Mutex<VecDeque<IncomingLink>>>,
}

impl DeepLinkHandle {
    fn new() -> Self {
        Self {
            inner: Arc::new(Mutex::new(VecDeque::new())),
        }
    }

    /// 投递一条原始 URL。
    ///
    /// 平台可能重复投递（macOS 有时会同时走 Apple Event 与 openURLs），
    /// 所以 1.5 秒内完全相同的 URL 只保留第一条。
    pub fn push(&self, raw: impl Into<String>, source: LinkSource) {
        let raw = raw.into();
        let raw = raw.trim().to_string();
        if raw.is_empty() {
            return;
        }
        let mut queue = match self.inner.lock() {
            Ok(queue) => queue,
            Err(poisoned) => poisoned.into_inner(),
        };
        if let Some(last) = queue.back() {
            if last.raw == raw && last.received_at.elapsed() < Duration::from_millis(1500) {
                return;
            }
        }
        queue.push_back(IncomingLink {
            raw,
            source,
            received_at: Instant::now(),
        });
        // 极端情况下（脚本狂点）别让队列无限长。
        while queue.len() > 32 {
            queue.pop_front();
        }
    }

    /// 取走当前所有待处理链接。GPUI 主线程轮询调用。
    pub fn drain(&self) -> Vec<IncomingLink> {
        let mut queue = match self.inner.lock() {
            Ok(queue) => queue,
            Err(poisoned) => poisoned.into_inner(),
        };
        queue.drain(..).collect()
    }

    pub fn is_empty(&self) -> bool {
        match self.inner.lock() {
            Ok(queue) => queue.is_empty(),
            Err(poisoned) => poisoned.into_inner().is_empty(),
        }
    }
}

static GLOBAL: OnceLock<DeepLinkHandle> = OnceLock::new();

/// 进程级单例队列。平台回调没有 `App` 上下文，只能走这里。
pub fn global() -> &'static DeepLinkHandle {
    GLOBAL.get_or_init(DeepLinkHandle::new)
}

/// 从任意线程投递 URL。失败只会丢一条链接，不会 panic。
pub fn ingest(raw: impl Into<String>, source: LinkSource) {
    global().push(raw, source);
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn drains_in_order_and_dedupes_immediate_repeats() {
        let handle = DeepLinkHandle::new();
        handle.push("ykpili://video/BV1xx411c7mD", LinkSource::Manual);
        handle.push("ykpili://video/BV1xx411c7mD", LinkSource::Manual);
        handle.push("ykpili://video/BV1xx411c7mE", LinkSource::WindowsPipe);

        let links = handle.drain();
        assert_eq!(links.len(), 2, "1.5 秒内的重复 URL 只保留一条");
        assert_eq!(links[0].raw, "ykpili://video/BV1xx411c7mD");
        assert_eq!(links[1].raw, "ykpili://video/BV1xx411c7mE");
        assert!(handle.is_empty());
    }

    #[test]
    fn caps_the_queue() {
        let handle = DeepLinkHandle::new();
        for index in 0..40 {
            handle.push(
                format!("ykpili://video/BV1xx411c7m{index}"),
                LinkSource::Manual,
            );
        }
        assert_eq!(handle.drain().len(), 32);
    }
}
