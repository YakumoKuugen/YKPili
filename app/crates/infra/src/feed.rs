//! 首页推荐流的数据管理（分页 / 上限 / 触底判断）。
//!
//! 对应旧 `YKPili/ViewModels/HomeViewModel.cs` 里**与渲染无关**的那部分：
//! 数据集合的加载、追加、上限约束与「是否需要继续拉取」。
//!
//! **不迁移的部分**：旧实现的容器池（`_pool` / `FillContainerData` / `CloneTo` /
//! `MakeEmpty` / 按行节流）。那是 Avalonia 时代为了复用控件手写的虚拟化，
//! GPUI 用 `v_virtual_list` 直接做掉，逻辑整个消失 —— 迁过来反而是负担。
//!
//! 一条对旧实现的修正：追加时**按 bvid 去重**。旧代码只做长度截断，
//! 推荐流偶尔返回重复条目时会在界面上出现两张一样的卡片。

use crate::api::models::RecommendVideoItem;
use crate::models::CoverItem;

/// 数据条数上限。
///
/// 旧实现是 200，全屏 2560 宽（11 列）只够约 18 行、两屏多一点，
/// 用户滚到底会以为「下面不加载了」。这里放宽到 1000：
/// 每条只是一个 URL 字符串，1000 条内存可忽略；封面图仍由
/// `PageImageCache` 按可视范围回收，不会因为条数变多而多占图。
pub const MAX_ITEMS: usize = 1000;

/// 占位色调色板（封面加载完成前的底色）。与旧实现同一组色值。
pub const PLACEHOLDER_PALETTE: [&str; 20] = [
    "#E57373", "#F06292", "#BA68C8", "#9575CD", "#7986CB", "#64B5F6", "#4FC3F7", "#4DD0E1",
    "#4DB6AC", "#81C784", "#AED581", "#DCE775", "#FFF176", "#FFD54F", "#FFB74D", "#FF8A65",
    "#A1887F", "#E0E0E0", "#90A4AE", "#BCAAA4",
];

/// 空位（无数据）的透明占位色。
pub const EMPTY_PLACEHOLDER: &str = "#00000000";

/// 推荐流。
#[derive(Debug, Clone)]
pub struct Feed {
    items: Vec<CoverItem>,
    max_items: usize,
}

impl Default for Feed {
    fn default() -> Self {
        Self::new()
    }
}

impl Feed {
    pub fn new() -> Self {
        Self { items: Vec::new(), max_items: MAX_ITEMS }
    }

    pub fn with_limit(max_items: usize) -> Self {
        Self { items: Vec::new(), max_items }
    }

    pub fn items(&self) -> &[CoverItem] {
        &self.items
    }

    pub fn len(&self) -> usize {
        self.items.len()
    }

    pub fn is_empty(&self) -> bool {
        self.items.is_empty()
    }

    /// 是否已达上限（到达后不再追加）。
    pub fn is_full(&self) -> bool {
        self.items.len() >= self.max_items
    }

    pub fn max_items(&self) -> usize {
        self.max_items
    }

    /// 初次加载：整体替换。
    pub fn load(&mut self, videos: &[RecommendVideoItem]) {
        self.items.clear();
        self.append(videos);
    }

    /// 追加一页，返回**实际新增**的条数。
    ///
    /// 三条约束：
    /// 1. 受 `max_items` 上限；
    /// 2. 按 `bvid` 去重（空 bvid 不去重，避免把不同条目误判成重复）；
    /// 3. 占位色按**全局索引**循环，保证追加前后同一位置颜色稳定。
    pub fn append(&mut self, videos: &[RecommendVideoItem]) -> usize {
        let mut added = 0;
        for v in videos {
            if self.items.len() >= self.max_items {
                break;
            }
            if !v.bvid.is_empty() && self.items.iter().any(|c| c.bvid == v.bvid) {
                continue;
            }
            let idx = self.items.len();
            let placeholder = PLACEHOLDER_PALETTE[idx % PLACEHOLDER_PALETTE.len()];
            self.items.push(CoverItem::from_recommend(v, idx, placeholder));
            added += 1;
        }
        added
    }

    /// 「是否应该再拉一页」。
    ///
    /// 旧实现的 `EvaluateLoadMore` 判断「追加后仍不足一屏时继续请求」；
    /// 这里把「一屏要多少条」交给调用方（它才知道当前列数 × 行数）。
    pub fn needs_more(&self, min_items: usize) -> bool {
        !self.is_full() && self.items.len() < min_items
    }

    /// 给虚拟列表用的行尺寸提示：需要知道每行几张卡片。
    pub fn rows(&self, columns: usize) -> usize {
        if columns == 0 {
            0
        } else {
            (self.items.len() + columns - 1) / columns
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn item(bvid: &str) -> RecommendVideoItem {
        RecommendVideoItem {
            bvid: bvid.to_string(),
            title: format!("标题 {bvid}"),
            pic: "pic".into(),
            ..Default::default()
        }
    }

    #[test]
    fn load_replaces_everything() {
        let mut f = Feed::new();
        f.load(&[item("A"), item("B")]);
        assert_eq!(f.len(), 2);
        f.load(&[item("C")]);
        assert_eq!(f.len(), 1);
        assert_eq!(f.items()[0].bvid, "C");
    }

    #[test]
    fn append_dedups_by_bvid() {
        let mut f = Feed::new();
        f.load(&[item("A"), item("B")]);
        let added = f.append(&[item("B"), item("C")]);
        assert_eq!(added, 1, "B 是重复的，只应新增 C");
        assert_eq!(f.len(), 3);
    }

    #[test]
    fn append_respects_max_items() {
        let mut f = Feed::with_limit(3);
        f.load(&[item("A"), item("B"), item("C")]);
        assert!(f.is_full());
        let added = f.append(&[item("D"), item("E")]);
        assert_eq!(added, 0, "满额后不应该再新增");
        assert_eq!(f.len(), 3);
    }

    #[test]
    fn append_partially_fills_remaining_capacity() {
        let mut f = Feed::with_limit(4);
        f.load(&[item("A"), item("B")]);
        let added = f.append(&[item("C"), item("D"), item("E")]);
        assert_eq!(added, 2, "只剩 2 个位置");
        assert!(f.is_full());
    }

    #[test]
    fn placeholder_color_is_stable_across_append() {
        let mut f = Feed::with_limit(10);
        f.load(&[item("A"), item("B")]);
        let first_two: Vec<String> =
            f.items().iter().map(|c| c.placeholder_color.clone()).collect();
        f.append(&[item("C")]);
        let after: Vec<String> =
            f.items().iter().map(|c| c.placeholder_color.clone()).collect();
        assert_eq!(&after[..2], &first_two[..], "已有条目颜色不应因追加而变化");
        assert_eq!(after[2], PLACEHOLDER_PALETTE[2]);
    }

    #[test]
    fn needs_more_when_below_min_and_not_full() {
        let mut f = Feed::with_limit(10);
        f.load(&[item("A")]);
        assert!(f.needs_more(8), "不足一屏应继续拉");
        f.load(&(0..9).map(|i| item(&format!("B{i}"))).collect::<Vec<_>>());
        assert!(!f.needs_more(8), "够一屏就不用拉");
    }

    #[test]
    fn full_feed_never_needs_more() {
        let mut f = Feed::with_limit(2);
        f.load(&[item("A"), item("B")]);
        assert!(!f.needs_more(100), "满额后即使不足 min 也不再拉");
    }

    #[test]
    fn rows_rounds_up() {
        let mut f = Feed::with_limit(10);
        f.load(&[item("A"), item("B"), item("C")]);
        assert_eq!(f.rows(2), 2);
        assert_eq!(f.rows(3), 1);
        assert_eq!(f.rows(0), 0);
    }

    #[test]
    fn empty_bvid_is_not_deduped() {
        let mut f = Feed::with_limit(10);
        f.load(&[item("")]);
        let added = f.append(&[item("")]);
        assert_eq!(added, 1, "空 bvid 不应被当成重复");
    }

    #[test]
    fn palette_has_twenty_colors() {
        assert_eq!(PLACEHOLDER_PALETTE.len(), 20);
        assert_eq!(PLACEHOLDER_PALETTE[0], "#E57373");
    }
}
