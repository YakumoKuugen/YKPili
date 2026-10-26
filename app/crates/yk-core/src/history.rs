//! 观看历史：接口模型 → UI 条目的映射、按天分组与展示文案。
//!
//! **为什么单独一个模块**：历史条目与推荐卡片不是同一形状——
//! - 推荐卡关心播放量 / 弹幕数 / 发布时间、UP 主头像；
//! - 历史条目关心「什么时间看到哪、看到多少」，需要日期分组与进度；
//!   直播条目没有 bvid / duration，不能套用视频卡的计算方式。
//!
//! 这里只放纯数据与格式化，不依赖 gpui，方便单测。

use chrono::{Datelike, Local, NaiveDate, TimeZone, Weekday};

use crate::api::models::HistoryItemRaw;
use crate::format;

/// 一页历史请求的游标。
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub struct HistoryCursor {
    pub max: i64,
    pub view_at: i64,
}

/// UI 用的观看历史条目。
#[derive(Debug, Clone, Default)]
pub struct HistoryEntry {
    /// 服务端删除用的主键；同一视频多 P 也各自不同。
    pub kid: i64,
    pub title: String,
    pub cover: String,
    pub bvid: String,
    pub cid: i64,
    pub oid: i64,
    pub epid: i64,
    pub page: i64,
    pub videos: u64,
    pub author_name: String,
    pub author_face: String,
    pub author_mid: i64,
    /// 观看时间（Unix 秒）。
    pub view_at: u64,
    /// 已观看秒数；接口可能给 -1（未开始），这里统一钳到 0。
    pub progress_secs: u64,
    pub duration_secs: u64,
    /// 1 = 已看完（接口偶有缺失，进度 ≥ 98% 也按看完处理）。
    pub is_finish: bool,
    /// `archive` / `pgc` / `live` / `article`。
    pub business: String,
    /// 分区名，例如「日常」「游戏」。
    pub tag_name: String,
    /// 接口给的角标文案（可能为空）。
    pub badge: String,
}

impl HistoryEntry {
    pub fn from_raw(raw: HistoryItemRaw) -> Self {
        let is_finish = raw.is_finish != 0;
        Self {
            kid: raw.kid,
            title: format::html_decode(if raw.long_title.trim().is_empty() {
                raw.title.trim()
            } else {
                raw.long_title.trim()
            }),
            cover: raw.cover,
            bvid: raw.history.bvid,
            cid: raw.history.cid,
            oid: raw.history.oid,
            epid: raw.history.epid,
            page: raw.history.page,
            videos: raw.videos,
            author_name: raw.author_name,
            author_face: raw.author_face,
            author_mid: raw.author_mid,
            view_at: raw.view_at,
            progress_secs: raw.progress.max(0) as u64,
            duration_secs: raw.duration,
            is_finish,
            business: if raw.history.business.trim().is_empty() {
                "archive".into()
            } else {
                raw.history.business
            },
            tag_name: raw.tag_name,
            badge: raw.badge,
        }
    }

    /// 封面缩略图。B 站图床在 URL 后加 `@宽w_高h_1c.webp` 取缩略图；
    /// 历史接口给的是 http 原图，顺手升级到 https。
    pub fn cover_thumb_url(&self) -> String {
        thumb_url(&self.cover, "@480w_270h_1c.webp")
    }

    pub fn face_thumb_url(&self) -> String {
        thumb_url(&self.author_face, "@64w_64h_1c.webp")
    }

    pub fn is_live(&self) -> bool {
        self.business == "live"
    }

    /// 只有普通视频能交给播放器（番剧 / 直播走各自链路，当前没有取流实现）。
    pub fn is_playable(&self) -> bool {
        self.business == "archive" && !self.bvid.trim().is_empty()
    }

    pub fn kind_label(&self) -> &'static str {
        match self.business.as_str() {
            "pgc" => "番剧",
            "live" => "直播",
            "article" => "专栏",
            _ => "视频",
        }
    }

    /// 已观看百分比（0–100）。直播 / 时长为 0 的条目没有进度。
    pub fn progress_percent(&self) -> u8 {
        if self.is_live() || self.duration_secs == 0 {
            return if self.is_finish { 100 } else { 0 };
        }
        if self.is_finish || self.progress_secs.saturating_mul(100) >= self.duration_secs * 98 {
            return 100;
        }
        ((self.progress_secs.saturating_mul(100)) / self.duration_secs).min(99) as u8
    }

    pub fn is_finished(&self) -> bool {
        self.progress_percent() >= 100
    }

    /// 卡片元信息行里的状态标签。
    pub fn state_label(&self) -> &'static str {
        if self.is_live() {
            "直播"
        } else if self.is_finished() {
            "已看完"
        } else {
            "已观看"
        }
    }

    /// 封面数据层左下角的进度文案。
    pub fn progress_label(&self) -> String {
        if self.is_live() {
            // 封面上已经有「直播」角标，数据层不再重复一遍。
            return String::new();
        }
        if self.duration_secs == 0 {
            return String::new();
        }
        if self.is_finished() {
            return "已看完".into();
        }
        let percent = self.progress_percent();
        if percent == 0 {
            // 没开始看的条目不要把「已看 0%」怼在封面上；
            // 状态标签已经说明它是看过的记录，封面只留时长。
            return String::new();
        }
        format!("已看 {percent}%")
    }

    pub fn duration_text(&self) -> String {
        if self.is_live() {
            String::new()
        } else {
            format::duration(self.duration_secs)
        }
    }

    /// 署名行的时间：今天 / 昨天显示「今天 14:32」，更早显示日期 + 时刻。
    pub fn watched_text(&self) -> String {
        watched_text(self.view_at)
    }

    /// 分组键：本地日期 `YYYY-MM-DD`。列表按它分组。
    pub fn day_key(&self) -> String {
        day_key(self.view_at)
    }

    /// 分组标题：今天 / 昨天 / 10月21日 周二。
    pub fn day_label(&self) -> String {
        day_label(self.view_at)
    }

    /// 本地搜索：标题、UP 主、分区名。
    pub fn matches_query(&self, query: &str) -> bool {
        let q = query.trim().to_lowercase();
        if q.is_empty() {
            return true;
        }
        self.title.to_lowercase().contains(&q)
            || self.author_name.to_lowercase().contains(&q)
            || self.tag_name.to_lowercase().contains(&q)
    }
}

/// 按天分组后的一行（侧栏日期列表用）。
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct HistoryDay {
    pub key: String,
    pub label: String,
    pub count: usize,
}

/// 把已过滤的条目按天聚合，日期从近到远。
pub fn group_days(entries: &[HistoryEntry]) -> Vec<HistoryDay> {
    let mut out: Vec<HistoryDay> = Vec::new();
    for entry in entries {
        let key = entry.day_key();
        if let Some(day) = out.iter_mut().find(|day| day.key == key) {
            day.count += 1;
        } else {
            out.push(HistoryDay {
                key,
                label: entry.day_label(),
                count: 1,
            });
        }
    }
    out.sort_by(|a, b| b.key.cmp(&a.key));
    out
}

fn thumb_url(url: &str, suffix: &str) -> String {
    let url = url.trim();
    if url.is_empty() {
        return String::new();
    }
    let url = url
        .strip_prefix("http://")
        .map_or_else(|| url.to_string(), |rest| format!("https://{rest}"));
    if url.contains('@') {
        url
    } else {
        format!("{url}{suffix}")
    }
}

fn local_date(ts: u64) -> Option<NaiveDate> {
    if ts == 0 {
        return None;
    }
    Local
        .timestamp_opt(ts as i64, 0)
        .single()
        .map(|dt| dt.date_naive())
}

fn weekday_cn(day: Weekday) -> &'static str {
    match day {
        Weekday::Mon => "周一",
        Weekday::Tue => "周二",
        Weekday::Wed => "周三",
        Weekday::Thu => "周四",
        Weekday::Fri => "周五",
        Weekday::Sat => "周六",
        Weekday::Sun => "周日",
    }
}

pub fn day_key(ts: u64) -> String {
    local_date(ts)
        .map(|d| format!("{:04}-{:02}-{:02}", d.year(), d.month(), d.day()))
        .unwrap_or_default()
}

pub fn day_label(ts: u64) -> String {
    let Some(date) = local_date(ts) else {
        return String::new();
    };
    let today = Local::now().date_naive();
    let delta = (today - date).num_days();
    match delta {
        0 => "今天".into(),
        1 => "昨天".into(),
        _ if date.year() == today.year() => {
            format!(
                "{}月{}日 {}",
                date.month(),
                date.day(),
                weekday_cn(date.weekday())
            )
        }
        _ => format!(
            "{}年{}月{}日 {}",
            date.year(),
            date.month(),
            date.day(),
            weekday_cn(date.weekday())
        ),
    }
}

pub fn watched_text(ts: u64) -> String {
    if ts == 0 {
        return String::new();
    }
    let Some(dt) = Local.timestamp_opt(ts as i64, 0).single() else {
        return String::new();
    };
    let label = day_label(ts);
    let clock = dt.format("%H:%M");
    if label.is_empty() {
        return clock.to_string();
    }
    // 今天 / 昨天带日期前缀读起来更清楚；更早的 label 本身已是日期。
    format!("{label} {clock}")
}

#[cfg(test)]
mod tests {
    use super::*;

    fn entry(ts: u64, duration: u64, progress: u64, finish: bool) -> HistoryEntry {
        HistoryEntry {
            view_at: ts,
            duration_secs: duration,
            progress_secs: progress,
            is_finish: finish,
            business: "archive".into(),
            title: "测试视频".into(),
            bvid: "BV1xx".into(),
            ..Default::default()
        }
    }

    #[test]
    fn progress_uses_seconds_not_percent() {
        // 202 秒的视频看到 154 秒 = 76%，不是 154%。
        assert_eq!(entry(1, 202, 154, false).progress_percent(), 76);
        assert_eq!(entry(1, 202, 154, false).progress_label(), "已看 76%");
    }

    #[test]
    fn near_end_is_finished() {
        assert_eq!(entry(1, 100, 99, false).progress_percent(), 100);
        assert_eq!(entry(1, 100, 0, true).state_label(), "已看完");
    }

    #[test]
    fn live_has_no_duration_progress() {
        let mut e = entry(1, 0, 0, false);
        e.business = "live".into();
        assert_eq!(e.duration_text(), "");
        assert_eq!(e.progress_label(), "");
        assert_eq!(e.state_label(), "直播");
    }

    #[test]
    fn days_are_grouped_newest_first() {
        let now = 1_700_000_000;
        let entries = vec![
            entry(now, 10, 1, false),
            entry(now - 86_400, 10, 1, false),
            entry(now, 10, 2, false),
        ];
        let days = group_days(&entries);
        assert_eq!(days.len(), 2);
        assert_eq!(days[0].count, 2);
    }

    #[test]
    fn thumb_url_upgrades_and_avoids_double_suffix() {
        assert_eq!(
            thumb_url("http://i0.hdslb.com/a.jpg", "@480w_270h_1c.webp"),
            "https://i0.hdslb.com/a.jpg@480w_270h_1c.webp"
        );
        assert_eq!(
            thumb_url("https://i0.hdslb.com/a.jpg@100w.webp", "@480w_270h_1c.webp"),
            "https://i0.hdslb.com/a.jpg@100w.webp"
        );
    }
}
