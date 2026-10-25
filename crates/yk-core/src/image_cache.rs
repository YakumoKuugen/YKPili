//! 图片磁盘缓存：按 URL 保存**压缩后的原始字节**。
//!
//! 这一层只负责「URL → 文件」的持久化与容量控制，不关心图片格式：
//! - 图片加载器命中时直接返回本地字节，不再访问 B 站图床；
//! - 解码仍由 GPUI 完成，所以内存里仍然只有可见卡片的那一小部分；
//! - 容量超限时按文件修改时间做近似 LRU 淘汰。
//!
//! 目录默认是 `~/Library/Caches/YKPili/images`（其它平台落到系统临时目录），
//! 与推荐流缓存共用 `CacheManager` 的根目录，但不共用 manifest。
//!
//! 可用环境变量调整：
//! - `YK_IMAGE_CACHE_MB`：容量上限，默认 300，`0` 表示关闭；
//! - `YK_IMAGE_CACHE_DIR`：直接指定图片缓存目录；
//! - `YK_CACHE_DIR`：只指定根目录，图片会写到其下的 `images/`。

use std::fs;
use std::io;
use std::path::{Path, PathBuf};
use std::sync::Mutex;
use std::time::{SystemTime, UNIX_EPOCH};

use crate::cache::CacheManager;

/// 默认容量上限：300 MB。
pub const DEFAULT_MAX_BYTES: u64 = 300 * 1024 * 1024;

#[derive(Debug, Default)]
struct State {
    total_bytes: u64,
}

/// URL 图片磁盘缓存。
pub struct ImageDiskCache {
    root: PathBuf,
    max_bytes: u64,
    enabled: bool,
    state: Mutex<State>,
}

impl ImageDiskCache {
    /// 创建/加载缓存。启动时扫描一次目录，统计已有容量。
    pub fn new(root: impl Into<PathBuf>, max_bytes: u64) -> Self {
        let root = root.into();
        let enabled = match fs::create_dir_all(&root) {
            Ok(()) => true,
            Err(e) => {
                eprintln!("[image-cache] 创建目录失败（{}）：{e}", root.display());
                false
            }
        };
        let mut total = 0_u64;
        if let Ok(entries) = fs::read_dir(&root) {
            for entry in entries.flatten() {
                let path = entry.path();
                if path.extension().and_then(|e| e.to_str()) != Some("img") {
                    continue;
                }
                if let Ok(meta) = entry.metadata() {
                    total = total.saturating_add(meta.len());
                }
            }
        }
        let cache = Self {
            root,
            max_bytes,
            enabled,
            state: Mutex::new(State { total_bytes: total }),
        };
        // 上次退出时如果已经超限，启动就先清一次，避免第一张新图写入前一直占着。
        if enabled && max_bytes > 0 && total > max_bytes {
            let mut state = cache.state.lock().unwrap();
            cache.prune_locked(&mut state);
        }
        cache
    }

    /// 默认位置 + 默认容量（可用 `YK_IMAGE_CACHE_MB` 覆盖，0 表示禁用）。
    pub fn default_cache() -> Self {
        let max_bytes = std::env::var("YK_IMAGE_CACHE_MB")
            .ok()
            .and_then(|value| value.parse::<u64>().ok())
            .map(|mb| mb.saturating_mul(1024 * 1024))
            .unwrap_or(DEFAULT_MAX_BYTES);
        let root = std::env::var_os("YK_IMAGE_CACHE_DIR")
            .map(PathBuf::from)
            .unwrap_or_else(|| {
                std::env::var_os("YK_CACHE_DIR")
                    .map(PathBuf::from)
                    .unwrap_or_else(CacheManager::default_root)
                    .join("images")
            });
        Self::new(root, max_bytes)
    }

    pub fn root(&self) -> &Path {
        &self.root
    }

    pub fn max_bytes(&self) -> u64 {
        self.max_bytes
    }

    fn path_for(&self, url: &str) -> PathBuf {
        self.root.join(format!("{}.img", md5_hex(url)))
    }

    /// 读取缓存；文件损坏/为空时视为未命中并删除。
    pub fn get(&self, url: &str) -> Option<Vec<u8>> {
        if !self.enabled || self.max_bytes == 0 {
            return None;
        }
        let path = self.path_for(url);
        let bytes = match fs::read(&path) {
            Ok(bytes) if !bytes.is_empty() => bytes,
            Ok(_) => {
                let _ = fs::remove_file(&path);
                return None;
            }
            Err(_) => return None,
        };

        // 命中时刷新 mtime，让淘汰顺序更接近 LRU（失败不影响读取）。
        if let Ok(file) = fs::OpenOptions::new().write(true).open(&path) {
            let _ = file.set_modified(SystemTime::now());
        }
        Some(bytes)
    }

    /// 写入缓存。写临时文件再 rename，避免半截文件被读到。
    pub fn put(&self, url: &str, bytes: &[u8]) {
        if !self.enabled || self.max_bytes == 0 || bytes.is_empty() {
            return;
        }
        let path = self.path_for(url);
        if path.is_file() {
            // 同一 URL 的图片不会变；已经存在就不重写。
            return;
        }
        if let Err(e) = self.write_atomic(&path, bytes) {
            eprintln!("[image-cache] 写入失败：{e}");
            return;
        }

        let mut state = self.state.lock().unwrap();
        state.total_bytes = state.total_bytes.saturating_add(bytes.len() as u64);
        if state.total_bytes > self.max_bytes {
            let total = self.prune_locked(&mut state);
            if total > self.max_bytes {
                eprintln!(
                    "[image-cache] 清理后仍超限：{:.1} MB / {:.1} MB",
                    total as f64 / 1048576.0,
                    self.max_bytes as f64 / 1048576.0
                );
            }
        }
    }

    fn write_atomic(&self, path: &Path, bytes: &[u8]) -> io::Result<()> {
        let nonce = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .unwrap_or_default()
            .as_nanos();
        let tmp = self.root.join(format!(
            ".{}.{}.{}.tmp",
            std::process::id(),
            nonce,
            path.file_stem().and_then(|s| s.to_str()).unwrap_or("image")
        ));
        fs::write(&tmp, bytes)?;
        match fs::rename(&tmp, path) {
            Ok(()) => Ok(()),
            Err(err) => {
                let _ = fs::remove_file(&tmp);
                Err(err)
            }
        }
    }

    /// 调用方已持有 state 锁；返回清理后的总大小。
    fn prune_locked(&self, state: &mut State) -> u64 {
        let mut files: Vec<(SystemTime, u64, PathBuf)> = Vec::new();
        if let Ok(entries) = fs::read_dir(&self.root) {
            for entry in entries.flatten() {
                let path = entry.path();
                if path.extension().and_then(|e| e.to_str()) != Some("img") {
                    continue;
                }
                if let Ok(meta) = entry.metadata() {
                    let modified = meta.modified().unwrap_or(UNIX_EPOCH);
                    files.push((modified, meta.len(), path));
                }
            }
        }
        files.sort_by_key(|(modified, _, _)| *modified);

        let target = self.max_bytes.saturating_mul(9) / 10;
        let mut total = files.iter().map(|(_, len, _)| *len).sum::<u64>();
        for (_, len, path) in files {
            if total <= target {
                break;
            }
            if fs::remove_file(&path).is_ok() {
                total = total.saturating_sub(len);
                eprintln!(
                    "[image-cache] 淘汰 {:.0} KB：{}",
                    len as f64 / 1024.0,
                    path.file_name().and_then(|s| s.to_str()).unwrap_or("?")
                );
            }
        }
        state.total_bytes = total;
        total
    }

    /// 清空所有图片文件（设置页「清理图片缓存」）。
    pub fn clear(&self) -> io::Result<()> {
        let mut state = self.state.lock().unwrap();
        let mut first_error = None;
        let mut remaining = 0_u64;
        if let Ok(entries) = fs::read_dir(&self.root) {
            for entry in entries.flatten() {
                let path = entry.path();
                if path.extension().and_then(|e| e.to_str()) != Some("img") {
                    continue;
                }
                let len = entry.metadata().map(|m| m.len()).unwrap_or(0);
                if let Err(e) = fs::remove_file(&path) {
                    remaining = remaining.saturating_add(len);
                    if first_error.is_none() {
                        first_error = Some(e);
                    }
                }
            }
        }
        state.total_bytes = remaining;
        match first_error {
            Some(e) => Err(e),
            None => Ok(()),
        }
    }

    /// 给测试/设置页用：当前缓存文件数与总大小。
    pub fn stats(&self) -> (usize, u64) {
        let mut count = 0;
        let mut bytes = 0_u64;
        if let Ok(entries) = fs::read_dir(&self.root) {
            for entry in entries.flatten() {
                if entry.path().extension().and_then(|e| e.to_str()) != Some("img") {
                    continue;
                }
                if let Ok(meta) = entry.metadata() {
                    count += 1;
                    bytes = bytes.saturating_add(meta.len());
                }
            }
        }
        (count, bytes)
    }
}

fn md5_hex(input: &str) -> String {
    use md5::{Digest, Md5};
    let mut hasher = Md5::new();
    hasher.update(input.as_bytes());
    let mut out = String::with_capacity(32);
    for byte in hasher.finalize() {
        out.push_str(&format!("{byte:02x}"));
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::time::Duration;

    fn temp_cache(name: &str, max_bytes: u64) -> (ImageDiskCache, PathBuf) {
        let root = std::env::temp_dir().join(format!(
            "yk-image-cache-{name}-{}-{}",
            std::process::id(),
            SystemTime::now()
                .duration_since(UNIX_EPOCH)
                .unwrap()
                .as_nanos()
        ));
        let cache = ImageDiskCache::new(&root, max_bytes);
        (cache, root)
    }

    #[test]
    fn put_get_roundtrip() {
        let (cache, root) = temp_cache("roundtrip", 10 * 1024 * 1024);
        assert_eq!(cache.get("https://i0.hdslb.com/a.webp"), None);
        cache.put("https://i0.hdslb.com/a.webp", b"hello-image");
        assert_eq!(
            cache.get("https://i0.hdslb.com/a.webp"),
            Some(b"hello-image".to_vec())
        );
        let _ = fs::remove_dir_all(root);
    }

    #[test]
    fn clear_removes_all_images() {
        let (cache, root) = temp_cache("clear", 10 * 1024 * 1024);
        cache.put("https://i0.hdslb.com/a.webp", b"a");
        cache.put("https://i0.hdslb.com/b.webp", b"b");
        assert_eq!(cache.stats().0, 2);
        cache.clear().unwrap();
        assert_eq!(cache.stats().0, 0);
        assert_eq!(cache.get("https://i0.hdslb.com/a.webp"), None);
        let _ = fs::remove_dir_all(root);
    }

    #[test]
    fn different_urls_use_different_files() {
        let (cache, root) = temp_cache("distinct", 10 * 1024 * 1024);
        cache.put("https://i0.hdslb.com/a.webp", b"a");
        cache.put("https://i0.hdslb.com/b.webp", b"b");
        assert_eq!(
            cache.get("https://i0.hdslb.com/a.webp"),
            Some(b"a".to_vec())
        );
        assert_eq!(
            cache.get("https://i0.hdslb.com/b.webp"),
            Some(b"b".to_vec())
        );
        assert_eq!(cache.stats().0, 2);
        let _ = fs::remove_dir_all(root);
    }

    #[test]
    fn empty_file_is_treated_as_miss() {
        let (cache, root) = temp_cache("empty", 10 * 1024 * 1024);
        cache.put("https://i0.hdslb.com/empty.webp", b"");
        assert_eq!(cache.get("https://i0.hdslb.com/empty.webp"), None);
        let _ = fs::remove_dir_all(root);
    }

    #[test]
    fn prune_keeps_under_limit() {
        let (cache, root) = temp_cache("prune", 12);
        cache.put("https://i0.hdslb.com/1.webp", b"123456");
        std::thread::sleep(Duration::from_millis(20));
        cache.put("https://i0.hdslb.com/2.webp", b"abcdef");
        std::thread::sleep(Duration::from_millis(20));
        cache.put("https://i0.hdslb.com/3.webp", b"ABCDEF");
        let (count, bytes) = cache.stats();
        assert!(bytes <= 12, "still over limit: {bytes}");
        assert!(count < 3, "oldest file should be evicted");
        let _ = fs::remove_dir_all(root);
    }

    #[test]
    fn zero_limit_disables_cache() {
        let (cache, root) = temp_cache("disabled", 0);
        cache.put("https://i0.hdslb.com/a.webp", b"a");
        assert_eq!(cache.get("https://i0.hdslb.com/a.webp"), None);
        let _ = fs::remove_dir_all(root);
    }
}
