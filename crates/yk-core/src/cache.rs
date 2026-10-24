//! 本地磁盘缓存（包粒度 + LRU 淘汰）。
//!
//! 对应旧 `YKPili/Services/Cache/CacheManager.cs`。设计要点沿用原实现：
//!
//! 1. **以「包」为粒度**：一个包一个子目录，包内文件由调用方自由组织；
//! 2. `manifest.json` 记录每个包的最后访问时间，用于 LRU 淘汰；
//! 3. 全局容量上限默认 **500 MB**，可自定义；
//! 4. 所有写操作串行化（内部 `Mutex`）；
//! 5. 淘汰时**至少保留一个包**（当前正在使用的那个），避免把自己删掉。
//!
//! 与 C# 版的差别：这里把 `Result` 显式化，不再用异常；时间戳只存 Unix 秒，
//! 便于跨平台与测试注入。

use std::collections::HashMap;
use std::fs;
use std::io;
use std::path::{Path, PathBuf};
use std::sync::Mutex;
use std::time::{SystemTime, UNIX_EPOCH};

use serde::{Deserialize, Serialize};

/// 默认容量上限：500 MB。
pub const DEFAULT_MAX_BYTES: u64 = 500 * 1024 * 1024;

const MANIFEST_FILE: &str = "manifest.json";

#[derive(Debug, Clone, Serialize, Deserialize, Default)]
struct PackageMeta {
    /// 最后访问时间（Unix 秒）。
    last_access: u64,
}

#[derive(Debug, Clone, Serialize, Deserialize, Default)]
struct Manifest {
    packages: HashMap<String, PackageMeta>,
}

impl Manifest {
    fn load(path: &Path) -> Self {
        // manifest 损坏时不能让整个缓存不可用 —— 当作空清单重建。
        fs::read_to_string(path)
            .ok()
            .and_then(|s| serde_json::from_str(&s).ok())
            .unwrap_or_default()
    }

    fn save(&self, path: &Path) -> io::Result<()> {
        let json = serde_json::to_string_pretty(self)
            .map_err(|e| io::Error::new(io::ErrorKind::Other, e))?;
        if let Some(parent) = path.parent() {
            fs::create_dir_all(parent)?;
        }
        fs::write(path, json)
    }
}

/// 缓存管理器。
pub struct CacheManager {
    root: PathBuf,
    max_bytes: u64,
    manifest: Mutex<Manifest>,
}

fn now_secs() -> u64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|d| d.as_secs())
        .unwrap_or(0)
}

impl CacheManager {
    /// 用指定根目录与上限构造，并加载（或重建）manifest。
    pub fn new(root: impl Into<PathBuf>, max_bytes: u64) -> Self {
        let root = root.into();
        let _ = fs::create_dir_all(&root);
        let manifest = Manifest::load(&root.join(MANIFEST_FILE));
        Self { root, max_bytes, manifest: Mutex::new(manifest) }
    }

    /// 默认容量上限 500 MB。
    pub fn with_default_limit(root: impl Into<PathBuf>) -> Self {
        Self::new(root, DEFAULT_MAX_BYTES)
    }

    /// 平台默认缓存根目录：
    /// macOS `~/Library/Caches/YKPili`，其它平台退回系统临时目录下的 `ykpili`。
    pub fn default_root() -> PathBuf {
        #[cfg(target_os = "macos")]
        {
            if let Ok(home) = std::env::var("HOME") {
                return PathBuf::from(home).join("Library/Caches/YKPili");
            }
        }
        std::env::temp_dir().join("ykpili-cache")
    }

    pub fn root(&self) -> &Path {
        &self.root
    }

    pub fn max_bytes(&self) -> u64 {
        self.max_bytes
    }

    fn manifest_path(&self) -> PathBuf {
        self.root.join(MANIFEST_FILE)
    }

    fn package_dir(&self, name: &str) -> PathBuf {
        self.root.join(sanitize(name))
    }

    /// 查询包目录；不存在返回 `None`（**不创建**）。
    pub fn package_path(&self, name: &str) -> Option<PathBuf> {
        let dir = self.package_dir(name);
        if dir.is_dir() {
            Some(dir)
        } else {
            None
        }
    }

    /// 准备包目录：清空旧内容、创建目录、刷新最后访问时间。
    ///
    /// 调用方拿到路径后即可写入新的缓存文件。
    pub fn prepare_package(&self, name: &str) -> io::Result<PathBuf> {
        let dir = self.package_dir(name);
        if dir.exists() {
            // 只清空内容，保留目录本身
            for entry in fs::read_dir(&dir)? {
                let p = entry?.path();
                if p.is_dir() {
                    let _ = fs::remove_dir_all(&p);
                } else {
                    let _ = fs::remove_file(&p);
                }
            }
        } else {
            fs::create_dir_all(&dir)?;
        }

        let mut m = self.manifest.lock().unwrap();
        m.packages.insert(sanitize(name), PackageMeta { last_access: now_secs() });
        m.save(&self.manifest_path())?;
        Ok(dir)
    }

    /// 移除指定包（目录与元信息一并删除）。
    pub fn remove_package(&self, name: &str) -> io::Result<()> {
        let key = sanitize(name);
        let dir = self.package_dir(name);
        if dir.exists() {
            fs::remove_dir_all(&dir)?;
        }
        let mut m = self.manifest.lock().unwrap();
        m.packages.remove(&key);
        m.save(&self.manifest_path())
    }

    /// 清空全部缓存。
    pub fn clear_all(&self) -> io::Result<()> {
        if self.root.exists() {
            for entry in fs::read_dir(&self.root)? {
                let p = entry?.path();
                if p.file_name().map(|n| n == MANIFEST_FILE).unwrap_or(false) {
                    continue;
                }
                if p.is_dir() {
                    let _ = fs::remove_dir_all(&p);
                } else {
                    let _ = fs::remove_file(&p);
                }
            }
        }
        let mut m = self.manifest.lock().unwrap();
        m.packages.clear();
        m.save(&self.manifest_path())
    }

    /// 所有已记录包的总字节数。
    pub fn total_size(&self) -> u64 {
        let m = self.manifest.lock().unwrap();
        m.packages
            .keys()
            .map(|name| dir_size(&self.root.join(name)))
            .sum()
    }

    /// 容量超限时按 LRU 淘汰最旧的包，直到总占用 ≤ 上限。
    ///
    /// **始终保留至少一个包**（即最近访问的那个）—— 与旧实现一致，
    /// 避免把调用方正在写的包删掉。
    pub fn enforce_size_limit(&self) -> io::Result<()> {
        let mut m = self.manifest.lock().unwrap();
        let mut entries: Vec<(String, u64, u64)> = m
            .packages
            .iter()
            .map(|(k, v)| (k.clone(), v.last_access, dir_size(&self.root.join(k))))
            .collect();
        let mut total: u64 = entries.iter().map(|(_, _, sz)| *sz).sum();
        if total <= self.max_bytes {
            return Ok(());
        }

        // 最旧的排前面
        entries.sort_by_key(|(_, last, _)| *last);
        let mut removed = Vec::new();
        for (name, _, size) in &entries {
            if total <= self.max_bytes {
                break;
            }
            // 保底：只剩一个包时停手
            if m.packages.len() - removed.len() <= 1 {
                break;
            }
            let _ = fs::remove_dir_all(self.root.join(name));
            m.packages.remove(name);
            removed.push(name.clone());
            total = total.saturating_sub(*size);
        }
        m.save(&self.manifest_path())
    }
}

/// 目录总字节数（递归）。
fn dir_size(p: &Path) -> u64 {
    let mut total = 0u64;
    let Ok(rd) = fs::read_dir(p) else { return 0 };
    for entry in rd.flatten() {
        let path = entry.path();
        if path.is_dir() {
            total += dir_size(&path);
        } else if let Ok(meta) = entry.metadata() {
            total += meta.len();
        }
    }
    total
}

/// 包名清洗：只保留字母数字与 `-_.`，防止路径穿越（`../`）。
fn sanitize(name: &str) -> String {
    let cleaned: String = name
        .chars()
        .map(|c| if c.is_ascii_alphanumeric() || c == '-' || c == '_' || c == '.' { c } else { '_' })
        .collect();
    if cleaned.is_empty() || cleaned == "." || cleaned == ".." {
        "default".to_string()
    } else {
        cleaned
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn temp_root(tag: &str) -> PathBuf {
        let p = std::env::temp_dir().join(format!(
            "yk-cache-test-{tag}-{}-{}",
            std::process::id(),
            now_secs()
        ));
        let _ = fs::remove_dir_all(&p);
        p
    }

    #[test]
    fn prepare_creates_dir_and_records_manifest() {
        let root = temp_root("prepare");
        let c = CacheManager::with_default_limit(&root);
        let dir = c.prepare_package("recommend").unwrap();
        assert!(dir.is_dir());
        assert!(c.package_path("recommend").is_some());
        // manifest 落盘
        assert!(root.join(MANIFEST_FILE).exists());
        let _ = fs::remove_dir_all(&root);
    }

    #[test]
    fn prepare_clears_previous_content() {
        let root = temp_root("clear-pkg");
        let c = CacheManager::with_default_limit(&root);
        let dir = c.prepare_package("p").unwrap();
        fs::write(dir.join("old.txt"), b"stale").unwrap();
        let _ = c.prepare_package("p").unwrap();
        assert!(!dir.join("old.txt").exists(), "旧内容应被清空");
        let _ = fs::remove_dir_all(&root);
    }

    #[test]
    fn remove_and_clear() {
        let root = temp_root("remove");
        let c = CacheManager::with_default_limit(&root);
        let d1 = c.prepare_package("a").unwrap();
        fs::write(d1.join("f"), vec![0u8; 128]).unwrap();
        let _ = c.prepare_package("b").unwrap();
        assert!(c.total_size() >= 128);

        c.remove_package("a").unwrap();
        assert!(c.package_path("a").is_none());
        assert!(c.package_path("b").is_some());

        c.clear_all().unwrap();
        assert!(c.package_path("b").is_none());
        assert_eq!(c.total_size(), 0);
        let _ = fs::remove_dir_all(&root);
    }

    /// LRU：容量超限时先删最旧的，但至少留一个。
    #[test]
    fn lru_evicts_oldest_but_keeps_one() {
        let root = temp_root("lru");
        // 上限 1KB
        let c = CacheManager::new(&root, 1024);

        let a = c.prepare_package("pkg-a").unwrap();
        fs::write(a.join("f"), vec![0u8; 800]).unwrap();
        std::thread::sleep(std::time::Duration::from_millis(1100));
        let b = c.prepare_package("pkg-b").unwrap();
        fs::write(b.join("f"), vec![0u8; 800]).unwrap();

        assert!(c.total_size() > 1024);
        c.enforce_size_limit().unwrap();

        // 最旧的 pkg-a 应被淘汰，最新的 pkg-b 保留
        assert!(c.package_path("pkg-a").is_none(), "最旧的包应被淘汰");
        assert!(c.package_path("pkg-b").is_some(), "最新的包必须保留");
        let _ = fs::remove_dir_all(&root);
    }

    #[test]
    fn single_package_is_never_evicted() {
        let root = temp_root("keep-one");
        let c = CacheManager::new(&root, 10); // 上限极小
        let a = c.prepare_package("only").unwrap();
        fs::write(a.join("f"), vec![0u8; 4096]).unwrap();
        c.enforce_size_limit().unwrap();
        assert!(c.package_path("only").is_some(), "唯一的包不能被删");
        let _ = fs::remove_dir_all(&root);
    }

    #[test]
    fn package_name_is_sanitized_against_traversal() {
        let root = temp_root("sanitize");
        let c = CacheManager::with_default_limit(&root);
        let dir = c.prepare_package("../../etc/passwd").unwrap();
        // 必须仍落在缓存根目录内
        assert!(dir.starts_with(&root), "包名穿透了缓存根目录：{dir:?}");
        let _ = fs::remove_dir_all(&root);
    }

    #[test]
    fn corrupted_manifest_degrades_to_empty() {
        let root = temp_root("corrupt");
        fs::create_dir_all(&root).unwrap();
        fs::write(root.join(MANIFEST_FILE), b"{ not json").unwrap();
        let c = CacheManager::with_default_limit(&root);
        // 不 panic，且能正常准备新包
        assert!(c.prepare_package("x").is_ok());
        let _ = fs::remove_dir_all(&root);
    }

    #[test]
    fn default_root_is_under_home_or_temp() {
        let p = CacheManager::default_root();
        assert!(p.to_string_lossy().contains("YKPili") || p.to_string_lossy().contains("ykpili"));
    }
}
