//! 离线 / 首启的占位推荐数据。
//!
//! 只用于「没有任何缓存」的兜底与视觉验收，不进真实取数路径。
//! 产出接口原始模型（`RecommendVideoItem`），让样例数据也走一遍
//! `CoverItem::from_recommend` 的真实映射。

use yk_infra::api::models::{RecommendVideoItem, VideoOwner, VideoStat};

/// 占位数据。真实数据在 M1 的 `CoverItem::from_recommend` 里，
/// 外壳阶段先用与设计稿一致的样例值，避免网络依赖挡住视觉验证。
///
/// 注意：这里刻意产出 **接口原始模型**（`RecommendVideoItem`）而不是 `CoverItem`，
/// 让样例数据也走一遍真实的映射路径 —— 否则 `from_recommend` 丢字段的 bug
/// 在 mock 阶段就发现不了。
pub(crate) fn recommend(n: usize) -> Vec<RecommendVideoItem> {
    const TITLES: [&str; 8] = [
        "生肉烹饪师和他的猫：一顿饭的成本到底有多少",
        "一碗甜子的深夜食堂 · 第 12 期",
        "班长captain 的极限运动合集",
        "用 Rust 从零写一个视频播放器",
        "当我把整个 B 站的热门都下载下来",
        "三分钟看懂 GPUI 的渲染管线",
        "这个夏天最值得去的十个地方",
        "为什么你的代码总是越写越乱",
    ];
    const UPS: [&str; 4] = ["生肉烹饪师", "一碗甜子", "班长captain", "技术宅小明"];
    const VIEWS: [u64; 6] = [124_000, 2_600, 3_000_000, 862, 112_000, 15_190];
    const DANMAKU: [u64; 6] = [144, 0, 1_200, 3, 124, 61];
    const DURATION: [u64; 6] = [754, 1_320, 245, 3_725, 96, 480];
    let now = yk_infra::api::client::now_wts();

    (0..n)
        .map(|i| RecommendVideoItem {
            id: i as i64,
            bvid: format!("BV1xx411c7m{i}"),
            title: TITLES[i % TITLES.len()].to_string(),
            pic: String::new(),
            desc: String::new(),
            duration: DURATION[i % 6],
            pubdate: now.saturating_sub((i as u64 % 8) * 3_600 + 1_800),
            goto: "av".into(),
            owner: Some(VideoOwner {
                name: UPS[i % UPS.len()].to_string(),
                face: String::new(),
                ..Default::default()
            }),
            stat: Some(VideoStat {
                view: VIEWS[i % 6],
                danmaku: DANMAKU[i % 6],
                ..Default::default()
            }),
        })
        .collect()
}
