//! 推荐流的离线缓存。
//!
//! 网络请求失败（或显式 `YK_OFFLINE=1`）时，用上一次成功拉到的推荐流撑住首页，
//! 而不是整页报错。落盘走 `CacheManager` 的包目录，跟封面缓存共用容量与 LRU。
//!
//! 只缓存「喂给卡片的数据」（`RecommendVideoItem`），封面图仍由 HTTP 客户端
//! 自己缓存；离线时封面可能加载不出来，卡片会显示占位色。

use std::fs;
use std::io;
use std::path::PathBuf;
use std::time::{SystemTime, UNIX_EPOCH};

use serde::{Deserialize, Serialize};

use crate::api::models::RecommendVideoItem;
use crate::cache::CacheManager;

const PACKAGE: &str = "feed";
const FILE: &str = "recommend.json";

/// 缓存内容：拉取时间（Unix 秒）+ 条目。
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct CachedFeed {
    pub updated_at: u64,
    pub items: Vec<RecommendVideoItem>,
}

/// 推荐流缓存句柄。
pub struct FeedCache {
    manager: CacheManager,
}

impl Default for FeedCache {
    fn default() -> Self {
        Self::new()
    }
}

impl FeedCache {
    /// 平台默认缓存目录（与封面缓存共用根目录）。
    ///
    /// `YK_CACHE_DIR` 可覆盖根目录：开发/验收时把缓存放到工作区或临时目录，
    /// 避免写系统缓存目录。
    pub fn new() -> Self {
        let root = std::env::var_os("YK_CACHE_DIR")
            .map(PathBuf::from)
            .unwrap_or_else(CacheManager::default_root);
        Self {
            manager: CacheManager::with_default_limit(root),
        }
    }

    /// 读最近一次成功拉取的推荐流。文件不存在 / 损坏都返回 `None`。
    pub fn load(&self) -> Option<CachedFeed> {
        let dir = self.manager.package_path(PACKAGE)?;
        let bytes = fs::read(dir.join(FILE)).ok()?;
        serde_json::from_slice(&bytes).ok()
    }

    /// 覆盖写入最新推荐流。
    pub fn save(&self, items: &[RecommendVideoItem]) -> io::Result<()> {
        let dir = self.manager.prepare_package(PACKAGE)?;
        let payload = CachedFeed {
            updated_at: now_secs(),
            items: items.to_vec(),
        };
        let json = serde_json::to_vec(&payload)
            .map_err(|e| io::Error::new(io::ErrorKind::InvalidData, e))?;
        fs::write(dir.join(FILE), json)
    }

    /// 清空推荐流缓存（设置页「清除缓存」会用到）。
    pub fn clear(&self) -> io::Result<()> {
        self.manager.remove_package(PACKAGE)
    }

    /// 缓存占用与更新时间，供设置页展示。
    ///
    /// 返回 `(字节数, 更新时间)`；文件不存在时返回 `(0, None)`。
    pub fn stats(&self) -> (u64, Option<u64>) {
        let Some(dir) = self.manager.package_path(PACKAGE) else {
            return (0, None);
        };
        let path = dir.join(FILE);
        let bytes = fs::metadata(&path).map(|m| m.len()).unwrap_or(0);
        let updated_at = self.load().map(|cached| cached.updated_at);
        (bytes, updated_at)
    }
}

fn now_secs() -> u64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|d| d.as_secs())
        .unwrap_or(0)
}

/// 把缓存时间写成「刚刚 / N 分钟前 / N 小时前 / N 天前」。
pub fn age_text(updated_at: u64) -> String {
    let age = now_secs().saturating_sub(updated_at);
    if age < 60 {
        "刚刚".to_string()
    } else if age < 3600 {
        format!("{} 分钟前", age / 60)
    } else if age < 86_400 {
        format!("{} 小时前", age / 3600)
    } else {
        format!("{} 天前", age / 86_400)
    }
}
