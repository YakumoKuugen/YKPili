//! 弹幕数据与轨道分配。
//!
//! 对应设计文档第 14 节「弹幕渲染」。这里只做**可测的纯逻辑**：
//! 解析 B 站弹幕、按轨道排布、按时间算出每条弹幕当前该在哪。
//! 真正画到屏幕上由视图层负责（GPUI 的文本渲染）。
//!
//! B 站弹幕 XML 的 `<d>` 属性格式（p 属性，逗号分隔）：
//!
//! ```text
//! time, mode, fontsize, color, timestamp, pool, uid_hash, row_id
//!   0     1       2        3        4        5      6         7
//! ```
//!
//! `mode`：1/2/3 = 滚动（右→左），4 = 底部固定，5 = 顶部固定。
//! `color` 是十进制 RGB（不是 hex）。
//!
//! 轨道分配的核心约束：同一轨道上，后一条弹幕的左边缘不能追上
//! 前一条的右边缘（否则文字会叠在一起）。

/// 弹幕显示模式。
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum DanmakuMode {
    /// 滚动（右 → 左）。B 站有 1/2/3 三档，行为一致，只是字号/速度不同。
    Scroll,
    /// 顶部固定。
    Top,
    /// 底部固定。
    Bottom,
}

impl DanmakuMode {
    pub fn from_code(code: i32) -> Self {
        match code {
            4 => Self::Bottom,
            5 => Self::Top,
            _ => Self::Scroll,
        }
    }

    pub fn is_scroll(self) -> bool {
        matches!(self, Self::Scroll)
    }
}

/// 一条弹幕。
#[derive(Debug, Clone, PartialEq)]
pub struct Danmaku {
    /// 出现时间（秒）。
    pub time: f64,
    pub mode: DanmakuMode,
    /// 字号（B 站原值通常是 25）。
    pub font_size: f32,
    /// RGB 颜色（各 0–255）。
    pub color: (u8, u8, u8),
    pub text: String,
    /// 估算的文本宽度（像素）。真实宽度要靠文本排版算，
    /// 这里先用「字符数 × 字号 × 系数」估算，够轨道分配用。
    pub width: f32,
}

impl Danmaku {
    /// 按字符数估算宽度。
    ///
    /// 中日文字符约等于 1 个字号宽，ASCII 约 0.55。这是估算，
    /// 但轨道分配只需要「相对宽度」，估偏一点不会导致重叠判断失效。
    pub fn estimate_width(text: &str, font_size: f32) -> f32 {
        text.chars()
            .map(|c| if c.is_ascii() { font_size * 0.55 } else { font_size })
            .sum()
    }

    /// 解析 XML 里 `<d p="...">text</d>` 的 p 属性。
    pub fn parse_p(p: &str, text: &str) -> Option<Self> {
        let parts: Vec<&str> = p.split(',').collect();
        if parts.len() < 4 {
            return None;
        }
        let time: f64 = parts[0].parse().ok()?;
        let mode = DanmakuMode::from_code(parts[1].parse().unwrap_or(1));
        let font_size: f32 = parts[2].parse().unwrap_or(25.0);
        let color_num: u32 = parts[3].parse().unwrap_or(0xFFFFFF);
        let color = (
            ((color_num >> 16) & 0xFF) as u8,
            ((color_num >> 8) & 0xFF) as u8,
            (color_num & 0xFF) as u8,
        );
        let width = Self::estimate_width(text, font_size);
        Some(Self { time, mode, font_size, color, text: text.to_string(), width })
    }

    /// 颜色转 `0xRRGGBB`（给视图层用）。
    pub fn rgb(&self) -> u32 {
        ((self.color.0 as u32) << 16) | ((self.color.1 as u32) << 8) | self.color.2 as u32
    }
}

/// 弹幕池：按时间排序的一整轨弹幕。
#[derive(Debug, Clone, Default)]
pub struct DanmakuSet {
    items: Vec<Danmaku>,
}

impl DanmakuSet {
    pub fn new() -> Self {
        Self::default()
    }

    /// 从 `<d>` 列表构造，内部按时间排序（B 站 XML 不保证顺序）。
    pub fn from_raw(raw: impl IntoIterator<Item = (String, String)>) -> Self {
        let mut items: Vec<Danmaku> = raw
            .into_iter()
            .filter_map(|(p, text)| Danmaku::parse_p(&p, &text))
            .collect();
        items.sort_by(|a, b| a.time.partial_cmp(&b.time).unwrap_or(std::cmp::Ordering::Equal));
        Self { items }
    }

    pub fn len(&self) -> usize {
        self.items.len()
    }

    pub fn is_empty(&self) -> bool {
        self.items.is_empty()
    }

    pub fn items(&self) -> &[Danmaku] {
        &self.items
    }

    /// 取出 `[start, end)` 时间窗口内的弹幕索引（二分定位，O(log n + k)）。
    ///
    /// 播放时每帧都要「现在该显示哪些」，用二分而不是全表过滤。
    pub fn window(&self, start: f64, end: f64) -> &[Danmaku] {
        let lo = self.items.partition_point(|d| d.time < start);
        let hi = self.items.partition_point(|d| d.time < end);
        &self.items[lo..hi]
    }
}

// ================= 轨道分配 =================

/// 滚动弹幕的轨道状态。
#[derive(Debug, Clone)]
struct LaneState {
    /// 上一条弹幕的「尾巴离开屏幕右边缘」的时间（秒）。
    /// 在这之前，新弹幕如果进这一轨道，就可能追上它。
    tail_clear_at: f64,
}

/// 滚动弹幕的轨道分配器。
///
/// 策略：对每条弹幕，找**第一条**「尾巴已清空」的轨道；都没有就新建。
/// 判据用「弹幕完全离开右边界的时刻」，比「左边缘进入」保守，
/// 不会出现文字互相覆盖。
#[derive(Debug, Clone, Default)]
pub struct LaneAllocator {
    lanes: Vec<LaneState>,
    /// 屏幕宽度（像素），用于计算滚动耗时。
    screen_width: f32,
    /// 滚动速度（像素/秒）。
    speed: f32,
}

impl LaneAllocator {
    pub fn new(screen_width: f32, speed: f32) -> Self {
        Self { lanes: Vec::new(), screen_width, speed: speed.max(1.0) }
    }

    pub fn lane_count(&self) -> usize {
        self.lanes.len()
    }

    /// 一条弹幕从右边缘完全进入屏幕所需时间（秒）。
    fn enter_duration(&self, width: f32) -> f64 {
        (width / self.speed) as f64
    }

    /// 一条弹幕从右边缘到完全离开左边缘的总时长。
    fn total_duration(&self, width: f32) -> f64 {
        ((self.screen_width + width) / self.speed) as f64
    }

    /// 给一条滚动弹幕分配轨道，返回轨道号。
    pub fn assign(&mut self, d: &Danmaku) -> usize {
        // 先算好（避免在 iter_mut 里再借用 self）
        let enter = self.enter_duration(d.width);
        for (i, lane) in self.lanes.iter_mut().enumerate() {
            if d.time >= lane.tail_clear_at {
                // 这条轨道空出来了：记录「它的尾巴离开右边缘的时刻」
                lane.tail_clear_at = d.time + enter;
                return i;
            }
        }
        // 没空轨道，新建一条
        self.lanes.push(LaneState { tail_clear_at: d.time + enter });
        self.lanes.len() - 1
    }

    /// 弹幕在 `now` 时刻的左边缘 x 坐标。
    ///
    /// 从 `screen_width` 开始向左走，走完 `screen_width + width` 即完全离开。
    pub fn x_at(&self, d: &Danmaku, now: f64) -> f32 {
        let elapsed = (now - d.time).max(0.0) as f32;
        self.screen_width - self.speed * elapsed
    }

    /// 该弹幕是否还应显示。
    pub fn visible_at(&self, d: &Danmaku, now: f64) -> bool {
        let elapsed = now - d.time;
        elapsed >= 0.0 && elapsed <= self.total_duration(d.width)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn dm(time: f64, text: &str) -> Danmaku {
        Danmaku::parse_p(&format!("{time},1,25,16777215,0,0,0,0"), text).unwrap()
    }

    #[test]
    fn parse_p_extracts_all_fields() {
        let d = Danmaku::parse_p("12.5,5,25,255,1700000000,0,abc,1", "顶部弹幕").unwrap();
        assert!((d.time - 12.5).abs() < 1e-9);
        assert_eq!(d.mode, DanmakuMode::Top);
        assert_eq!(d.font_size, 25.0);
        assert_eq!(d.color, (0, 0, 255));
        assert_eq!(d.text, "顶部弹幕");
    }

    #[test]
    fn mode_codes_map_correctly() {
        assert_eq!(DanmakuMode::from_code(1), DanmakuMode::Scroll);
        assert_eq!(DanmakuMode::from_code(2), DanmakuMode::Scroll);
        assert_eq!(DanmakuMode::from_code(3), DanmakuMode::Scroll);
        assert_eq!(DanmakuMode::from_code(4), DanmakuMode::Bottom);
        assert_eq!(DanmakuMode::from_code(5), DanmakuMode::Top);
    }

    #[test]
    fn malformed_p_is_rejected_not_panicked() {
        assert!(Danmaku::parse_p("", "x").is_none());
        assert!(Danmaku::parse_p("abc,1,25,255", "x").is_none());
        assert!(Danmaku::parse_p("1,1", "x").is_none());
    }

    #[test]
    fn set_sorts_by_time() {
        let set = DanmakuSet::from_raw(vec![
            ("10,1,25,16777215,0,0,0,0".to_string(), "c".to_string()),
            ("1,1,25,16777215,0,0,0,0".to_string(), "a".to_string()),
            ("5,1,25,16777215,0,0,0,0".to_string(), "b".to_string()),
        ]);
        let times: Vec<f64> = set.items().iter().map(|d| d.time).collect();
        assert_eq!(times, vec![1.0, 5.0, 10.0]);
    }

    #[test]
    fn window_selects_by_time_range() {
        let set = DanmakuSet::from_raw(
            (0..10)
                .map(|i| (format!("{i},1,25,16777215,0,0,0,0"), format!("d{i}")))
                .collect::<Vec<_>>(),
        );
        let w = set.window(2.0, 5.0);
        assert_eq!(w.len(), 3, "应取到 2/3/4 三条");
        assert_eq!(w[0].text, "d2");
        assert_eq!(w[2].text, "d4");
    }

    #[test]
    fn width_estimate_handles_cjk_and_ascii() {
        let cjk = Danmaku::estimate_width("中文", 25.0);
        let ascii = Danmaku::estimate_width("ab", 25.0);
        assert!(cjk > ascii, "中文字符应比 ASCII 宽（{cjk} vs {ascii}）");
        assert!((cjk - 50.0).abs() < 1e-3);
    }

    #[test]
    fn same_time_goes_to_different_lanes() {
        let mut a = LaneAllocator::new(1280.0, 200.0);
        let d = dm(1.0, "同时段");
        let l1 = a.assign(&d);
        let l2 = a.assign(&d);
        let l3 = a.assign(&d);
        assert_eq!((l1, l2, l3), (0, 1, 2), "同一时刻的弹幕必须分到不同轨道");
        assert_eq!(a.lane_count(), 3);
    }

    #[test]
    fn lane_is_reused_after_tail_clears() {
        let mut a = LaneAllocator::new(1280.0, 200.0);
        let d = dm(0.0, "短");
        let first = a.assign(&d);
        // 等足够久（尾巴完全进屏），同一轨道应可再用
        let later = dm(10.0, "短");
        let second = a.assign(&later);
        assert_eq!(first, second, "尾巴清空后应复用轨道");
        assert_eq!(a.lane_count(), 1);
    }

    #[test]
    fn x_position_moves_left_over_time() {
        let a = LaneAllocator::new(1280.0, 200.0);
        let d = dm(0.0, "移动");
        let x0 = a.x_at(&d, 0.0);
        let x1 = a.x_at(&d, 1.0);
        assert!((x0 - 1280.0).abs() < 1e-3, "起点应在右边缘");
        assert!((x0 - x1 - 200.0).abs() < 1e-3, "1 秒应左移一个 speed");
    }

    #[test]
    fn visibility_window_covers_whole_travel() {
        let a = LaneAllocator::new(1280.0, 200.0);
        let d = dm(0.0, "w");
        assert!(!a.visible_at(&d, -0.1), "还没到时间不应显示");
        assert!(a.visible_at(&d, 0.0));
        let total = ((1280.0 + d.width) / 200.0) as f64;
        assert!(a.visible_at(&d, total - 0.01));
        assert!(!a.visible_at(&d, total + 0.1), "完全离开后不应显示");
    }

    /// 关键性质：同一轨道上，后来的弹幕不能追上前面的。
    #[test]
    fn no_overlap_within_a_lane() {
        let mut a = LaneAllocator::new(1280.0, 200.0);
        let screen_w = 1280.0f32;
        // 密集发射 20 条，记录各自轨道
        let mut placed: Vec<(usize, Danmaku)> = Vec::new();
        for i in 0..20 {
            let d = dm(i as f64 * 0.3, "中等长度的弹幕");
            let lane = a.assign(&d);
            placed.push((lane, d));
        }
        // 同轨道相邻两条：后者的左边缘在任意时刻都应在前者右边缘之右
        for lane in 0..a.lane_count() {
            let in_lane: Vec<&Danmaku> =
                placed.iter().filter(|(l, _)| *l == lane).map(|(_, d)| d).collect();
            for w in in_lane.windows(2) {
                let (prev, next) = (w[0], w[1]);
                // 后者出现时，前者的尾巴必须已经完全进入屏幕
                let prev_right = a.x_at(prev, next.time) + prev.width;
                assert!(
                    prev_right <= screen_w + 1.0,
                    "轨道 {lane}：后一条出现时前一条尾巴还没进屏（{prev_right} > {screen_w}）"
                );
            }
        }
    }

    #[test]
    fn many_danmaku_creates_enough_lanes() {
        let mut a = LaneAllocator::new(1280.0, 200.0);
        for i in 0..10 {
            a.assign(&dm(0.0, &format!("同时{i}")));
        }
        assert_eq!(a.lane_count(), 10, "10 条同时刻弹幕应有 10 条轨道");
    }
}
