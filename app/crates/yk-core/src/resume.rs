//! 续播记录：本地记住每个视频上次看到的位置，下次打开直接续上。
//!
//! **为什么不直接读服务端历史**：服务端历史是「观看历史」页的数据，
//! 只有打开那个页面才在内存里；从推荐页点开一个视频时，客户端并不知道
//! 用户之前看到哪。这里在本地存一份「上次位置」，打开视频时零网络开销地
//! 算出起播点。
//!
//! **与服务端历史的关系**：历史页打开视频时会把服务端进度一起带进来
//! （[`resolve_start_position`] 的 `seed` 参数），两边取更靠后的那个 ——
//! 本地更准（就是上次关掉的地方），服务端更全（别的设备看过）。
//!
//! 规则（[`resume_position_of`]）：
//! - 位置 < 5 秒：刚开始看，从头播；
//! - 距结尾 < 10 秒（或 ≥ 98%）：已看完，从头播；
//! - 其余：从上次的位置续播。

use std::collections::BTreeMap;
use std::fs;
use std::io;
use std::path::{Path, PathBuf};
use std::time::{SystemTime, UNIX_EPOCH};

use serde::{Deserialize, Serialize};

/// 小于这个秒数视为「刚开始看」，下次从头播。
pub const RESUME_MIN_SECS: f64 = 5.0;

/// 距结尾小于这个秒数视为「已看完」，下次从头播。
pub const RESUME_TAIL_SECS: f64 = 10.0;

/// 记录多久之后清理（天）。
const PRUNE_AFTER_DAYS: u64 = 90;

/// 一条续播记录。
#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub struct ResumeRecord {
    /// 上次停留的位置（秒）。
    pub position_secs: f64,
    /// 视频总时长（秒）；0 = 未知。
    pub duration_secs: f64,
    /// 最后更新时间（Unix 秒）。
    pub updated_at: u64,
}

/// 磁盘上的文件格式。
#[derive(Debug, Clone, Default, Serialize, Deserialize)]
struct ResumeFile {
    #[serde(default)]
    records: BTreeMap<String, ResumeRecord>,
}

/// 本地续播记录表。
///
/// 只在内存里维护，写盘由调用方显式触发（[`ResumeStore::save`]）——
/// 播放中每帧写盘没有意义，只在「关闭 / 换源」这种明确的节点写一次。
#[derive(Debug, Clone)]
pub struct ResumeStore {
    path: PathBuf,
    records: BTreeMap<String, ResumeRecord>,
}

impl ResumeStore {
    /// 默认存储路径。
    ///
    /// macOS：`~/Library/Application Support/YKPili/resume.json`；
    /// 其它平台退回系统临时目录。测试可用 `YK_RESUME_PATH` 覆盖。
    pub fn default_path() -> PathBuf {
        if let Ok(path) = std::env::var("YK_RESUME_PATH") {
            if !path.trim().is_empty() {
                return PathBuf::from(path);
            }
        }
        #[cfg(target_os = "macos")]
        {
            if let Ok(home) = std::env::var("HOME") {
                return PathBuf::from(home).join("Library/Application Support/YKPili/resume.json");
            }
        }
        std::env::temp_dir().join("YKPili").join("resume.json")
    }

    /// 从磁盘加载；文件不存在 / 损坏都当作空表（不能让续播功能拖垮播放）。
    pub fn load(path: impl Into<PathBuf>) -> Self {
        let path = path.into();
        let records = fs::read_to_string(&path)
            .ok()
            .and_then(|text| serde_json::from_str::<ResumeFile>(&text).ok())
            .map(|file| file.records)
            .unwrap_or_default();
        Self { path, records }
    }

    /// 用默认路径加载。
    pub fn load_default() -> Self {
        Self::load(Self::default_path())
    }

    /// 存储路径。
    pub fn path(&self) -> &Path {
        &self.path
    }

    /// 写盘。失败不致命（下次没有续播而已）。
    pub fn save(&self) -> io::Result<()> {
        if let Some(parent) = self.path.parent() {
            fs::create_dir_all(parent)?;
        }
        let file = ResumeFile {
            records: self.records.clone(),
        };
        let json = serde_json::to_string_pretty(&file)
            .map_err(|e| io::Error::new(io::ErrorKind::Other, e))?;
        fs::write(&self.path, json)
    }

    /// 查一条记录。
    pub fn get(&self, key: &str) -> Option<&ResumeRecord> {
        self.records.get(key)
    }

    /// 记录一次观看位置。
    ///
    /// - 位置过小（刚开始看）→ 清掉记录，下次从头播；
    /// - 已看到结尾 → 清掉记录，下次从头播；
    /// - 其余 → 写入 / 更新记录。
    pub fn record(&mut self, key: &str, position_secs: f64, duration_secs: f64) {
        if !position_secs.is_finite() {
            return;
        }
        if position_secs < RESUME_MIN_SECS {
            self.records.remove(key);
            return;
        }
        if duration_secs > 0.0 && position_secs >= duration_secs - RESUME_TAIL_SECS {
            // 已经看到结尾：这条记录没有续播价值，直接清掉。
            self.records.remove(key);
            return;
        }
        self.records.insert(
            key.to_string(),
            ResumeRecord {
                position_secs,
                duration_secs,
                updated_at: now_secs(),
            },
        );
        self.prune();
    }

    /// 删掉一条记录（退出登录 / 手动清除历史时用）。
    pub fn remove(&mut self, key: &str) {
        self.records.remove(key);
    }

    /// 清空全部记录。
    pub fn clear(&mut self) {
        self.records.clear();
    }

    /// 记录条数。
    pub fn len(&self) -> usize {
        self.records.len()
    }

    /// 是否为空。
    pub fn is_empty(&self) -> bool {
        self.records.is_empty()
    }

    /// 这个视频下次应该从哪里起播。
    ///
    /// 返回 `None` 表示「从头播」。
    pub fn resume_position(&self, key: &str) -> Option<f64> {
        self.records.get(key).and_then(resume_position_of)
    }

    /// 清理太老的记录，避免文件无限膨胀。
    fn prune(&mut self) {
        let now = now_secs();
        let limit = PRUNE_AFTER_DAYS * 24 * 60 * 60;
        self.records
            .retain(|_, record| now.saturating_sub(record.updated_at) <= limit);
    }
}

/// 记录键：`bvid` + 分 P 的 `cid`。
///
/// 有 cid 时按 `bvid#cid` 存，分 P 之间互不干扰；
/// cid 未知时退化成只用 bvid。
pub fn key(bvid: &str, cid: i64) -> String {
    let bvid = bvid.trim();
    if cid > 0 {
        format!("{bvid}#{cid}")
    } else {
        bvid.to_string()
    }
}

/// 一条记录对应的起播位置；`None` = 从头播。
pub fn resume_position_of(record: &ResumeRecord) -> Option<f64> {
    let position = record.position_secs;
    if !position.is_finite() || position < RESUME_MIN_SECS {
        return None;
    }
    let duration = record.duration_secs;
    if duration > 0.0 {
        if position >= duration - RESUME_TAIL_SECS {
            // 已看完。
            return None;
        }
        if position > duration {
            return None;
        }
    }
    Some(position)
}

/// 算出这次播放的起播位置。
///
/// `candidates` 里放各个来源给出的候选位置（秒），取**最靠后的一个**：
/// - 本地续播记录；
/// - 观看历史页带回来的服务端进度；
/// - 服务端观看历史里查到的进度（本地没记录时才会去查）。
///
/// 都无效（或已看完）时返回 `None`（从头播）。
///
/// `duration_secs` = 视频时长（0 = 未知）；已知时会先把候选位置夹到
/// 「离结尾还有 10 秒」以内，避免起播点落在片尾。
pub fn resolve_start_position(candidates: &[Option<f64>], duration_secs: f64) -> Option<f64> {
    let mut best: Option<f64> = None;
    for candidate in candidates.iter().flatten() {
        if !candidate.is_finite() || *candidate < RESUME_MIN_SECS {
            continue;
        }
        if duration_secs > 0.0 && *candidate >= duration_secs - RESUME_TAIL_SECS {
            // 已看到片尾（或时长对不上）—— 当作看完，从头播。
            continue;
        }
        best = Some(best.map_or(*candidate, |b: f64| b.max(*candidate)));
    }
    best
}

fn now_secs() -> u64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|d| d.as_secs())
        .unwrap_or(0)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn temp_path(tag: &str) -> PathBuf {
        std::env::temp_dir().join(format!(
            "yk-resume-test-{tag}-{}-{}",
            std::process::id(),
            now_secs()
        ))
    }

    #[test]
    fn key_includes_cid_when_known() {
        assert_eq!(key("BV1xx", 42), "BV1xx#42");
        assert_eq!(key("BV1xx", 0), "BV1xx");
        assert_eq!(key(" BV1xx ", -1), "BV1xx");
    }

    #[test]
    fn record_round_trips_through_disk() {
        let path = temp_path("roundtrip");
        let mut store = ResumeStore::load(&path);
        store.record("BV1#1", 120.0, 600.0);
        store.save().unwrap();

        let reloaded = ResumeStore::load(&path);
        assert_eq!(reloaded.resume_position("BV1#1"), Some(120.0));
        let _ = fs::remove_file(&path);
    }

    #[test]
    fn missing_or_corrupt_file_is_empty_store() {
        let path = temp_path("corrupt");
        fs::create_dir_all(path.parent().unwrap()).unwrap();
        fs::write(&path, b"{ not json").unwrap();
        let store = ResumeStore::load(&path);
        assert!(store.is_empty());
        let _ = fs::remove_file(&path);
    }

    #[test]
    fn just_started_is_not_resumed() {
        let mut store = ResumeStore::load(temp_path("tiny"));
        store.record("BV1#1", 2.0, 600.0);
        assert!(store.get("BV1#1").is_none());
        assert_eq!(store.resume_position("BV1#1"), None);
    }

    #[test]
    fn finished_video_is_not_resumed() {
        let mut store = ResumeStore::load(temp_path("done"));
        store.record("BV1#1", 598.0, 600.0);
        assert!(store.get("BV1#1").is_none());
        assert_eq!(store.resume_position("BV1#1"), None);
    }

    #[test]
    fn middle_position_is_resumed() {
        let mut store = ResumeStore::load(temp_path("mid"));
        store.record("BV1#1", 300.0, 600.0);
        assert_eq!(store.resume_position("BV1#1"), Some(300.0));
    }

    #[test]
    fn resolve_prefers_the_later_candidate() {
        assert_eq!(
            resolve_start_position(&[Some(120.0), Some(300.0)], 600.0),
            Some(300.0)
        );
        assert_eq!(
            resolve_start_position(&[Some(300.0), Some(120.0)], 600.0),
            Some(300.0)
        );
        assert_eq!(
            resolve_start_position(&[Some(120.0), None, Some(300.0)], 600.0),
            Some(300.0)
        );
    }

    #[test]
    fn resolve_clamps_to_tail_and_drops_finished() {
        // 服务端说看到 590 秒（600 秒的视频）→ 已看完，从头播。
        assert_eq!(resolve_start_position(&[Some(590.0)], 600.0), None);
        // 时长对不上（进度比时长还长）→ 当作看完。
        assert_eq!(resolve_start_position(&[Some(999.0)], 600.0), None);
    }

    #[test]
    fn resolve_without_duration_keeps_candidate() {
        assert_eq!(resolve_start_position(&[Some(120.0), None], 0.0), Some(120.0));
        assert_eq!(resolve_start_position(&[None, None], 600.0), None);
    }

    #[test]
    fn resolve_ignores_tiny_candidates() {
        assert_eq!(resolve_start_position(&[Some(1.0), Some(2.0)], 600.0), None);
    }
}
