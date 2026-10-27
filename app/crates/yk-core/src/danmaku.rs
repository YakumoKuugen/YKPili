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

    /// 从 B 站弹幕 XML（`/x/v1/dm/list.so`）构造。
    ///
    /// 只认 `<d p="...">text</d>` 这一种节点；没有引入 XML 库，是因为弹幕
    /// 结构极其固定，而完整 XML 解析器会让这条纯逻辑多背一个依赖。
    /// 坏节点（缺 `p`、没有闭合标签、时间不是数字）逐条跳过，不整包失败 ——
    /// 线上弹幕里确实会出现半截数据。
    pub fn from_xml(xml: &str) -> Self {
        let mut raw: Vec<(String, String)> = Vec::new();
        let mut cursor = 0usize;
        while let Some(offset) = xml[cursor..].find("<d ") {
            let open_start = cursor + offset;
            let Some(gt) = xml[open_start..].find('>') else {
                break;
            };
            let open_end = open_start + gt;
            let open_tag = &xml[open_start..open_end];
            let Some(p) = xml_attr(open_tag, "p") else {
                cursor = open_end + 1;
                continue;
            };
            // 自闭合的 `<d p="..."/>` 没有正文，跳过。
            if open_tag.ends_with('/') {
                cursor = open_end + 1;
                continue;
            }
            let Some(close) = xml[open_end + 1..].find("</d>") else {
                break;
            };
            let text = &xml[open_end + 1..open_end + 1 + close];
            raw.push((p, unescape_xml(text)));
            cursor = open_end + 1 + close + "</d>".len();
        }
        Self::from_raw(raw)
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

/// 取 XML 开标签里的属性值（兼容单 / 双引号）。
fn xml_attr(tag: &str, name: &str) -> Option<String> {
    for quote in ['"', '\''] {
        let needle = format!("{name}={quote}");
        if let Some(start) = tag.find(&needle) {
            let value_start = start + needle.len();
            if let Some(end) = tag[value_start..].find(quote) {
                return Some(tag[value_start..value_start + end].to_string());
            }
        }
    }
    None
}

/// 解 XML 实体。
///
/// 弹幕正文里最常见的是 `&lt;` / `&gt;` / `&amp;`；数字实体
/// （`&#39;` / `&#x27;`）也一并处理，避免把 `&` 原样画到屏幕上。
fn unescape_xml(text: &str) -> String {
    if !text.contains('&') {
        return text.to_string();
    }
    let mut out = String::with_capacity(text.len());
    let mut rest = text;
    while let Some(idx) = rest.find('&') {
        out.push_str(&rest[..idx]);
        let tail = &rest[idx..];
        let Some(semi) = tail.find(';') else {
            out.push_str(tail);
            return out;
        };
        let entity = &tail[1..semi];
        let decoded = match entity {
            "amp" => Some('&'),
            "lt" => Some('<'),
            "gt" => Some('>'),
            "quot" => Some('"'),
            "apos" => Some('\''),
            _ if entity.starts_with("#x") || entity.starts_with("#X") => {
                u32::from_str_radix(&entity[2..], 16)
                    .ok()
                    .and_then(char::from_u32)
            }
            _ if entity.starts_with('#') => {
                entity[1..].parse::<u32>().ok().and_then(char::from_u32)
            }
            _ => None,
        };
        match decoded {
            Some(c) => {
                out.push(c);
                rest = &tail[semi + 1..];
            }
            None => {
                // 不认识的实体原样保留，别吞字符。
                out.push('&');
                rest = &tail[1..];
            }
        }
    }
    out.push_str(rest);
    out
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
/// 两种策略：
/// - [`Self::assign`]：永远找第一条空轨道。判据是「尾巴已进入屏幕」，
///   不会重叠；但大量短弹幕（满屏的「？」）会一直挤在第 0 条轨道，
///   视觉上全堆在屏幕最上面。
/// - [`Self::assign_spread`]：从上次用过的下一条轨道开始轮转，
///   让 0..max 条轨道轮流使用；轨道数按需增长，满了返回 `None`。
///   播放器用这一种，弹幕才会铺开而不是全挤在顶部。
#[derive(Debug, Clone, Default)]
pub struct LaneAllocator {
    lanes: Vec<LaneState>,
    /// 屏幕宽度（像素），用于计算滚动耗时。
    screen_width: f32,
    /// 滚动速度（像素/秒）。
    speed: f32,
    /// 下一次轮转的起始轨道（仅 [`Self::assign_spread`] 使用）。
    next_lane: usize,
}

impl LaneAllocator {
    pub fn new(screen_width: f32, speed: f32) -> Self {
        Self { lanes: Vec::new(), screen_width, speed: speed.max(1.0), next_lane: 0 }
    }

    pub fn lane_count(&self) -> usize {
        self.lanes.len()
    }

    /// 一条弹幕从右边缘完全进入屏幕所需时间（秒）。
    fn enter_duration(&self, width: f32) -> f64 {
        (width / self.speed) as f64
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

    /// 轮转分配轨道：从 `next_lane` 开始找空轨道，最多用到 `max_lanes` 条。
    ///
    /// 这样即使一条轨道已经空出来，也不会立刻把下一条弹幕塞回去 ——
    /// 轨道会按 0、1、2… 的顺序轮流使用，弹幕自然铺满整个显示区。
    /// 所有轨道都占满时返回 `None`，调用方可以丢弃这条（比叠字好）。
    pub fn assign_spread(&mut self, d: &Danmaku, max_lanes: usize) -> Option<usize> {
        let max_lanes = max_lanes.max(1);
        let enter = self.enter_duration(d.width);
        let start = self.next_lane % max_lanes;

        for offset in 0..max_lanes {
            let lane = (start + offset) % max_lanes;
            if lane >= self.lanes.len() {
                // 轮到的轨道还没建：补齐到这一条。跳过中间的空轨道是允许的，
                // 它们会在后面的轮转里被填上。
                while self.lanes.len() <= lane {
                    self.lanes.push(LaneState { tail_clear_at: f64::NEG_INFINITY });
                }
            }
            if d.time >= self.lanes[lane].tail_clear_at {
                self.lanes[lane].tail_clear_at = d.time + enter;
                self.next_lane = (lane + 1) % max_lanes;
                return Some(lane);
            }
        }
        None
    }

    /// 弹幕在 `now` 时刻的左边缘 x 坐标。
    ///
    /// 从 `screen_width` 开始向左走，走完 `screen_width + width` 即完全离开。
    pub fn x_at(&self, d: &Danmaku, now: f64) -> f32 {
        self.x_at_in(d, now, self.screen_width)
    }

    /// 同 [`Self::x_at`]，但用调用方当前的弹幕区宽度。
    ///
    /// 播放器窗口会缩放，弹幕区宽度不能写死成创建分配器时的 1280：
    /// 窗口比 1280 窄时，写死会让弹幕从面板底下冒出来。
    pub fn x_at_in(&self, d: &Danmaku, now: f64, screen_width: f32) -> f32 {
        let elapsed = (now - d.time).max(0.0) as f32;
        screen_width.max(1.0) - self.speed * elapsed
    }

    /// 该弹幕是否还应显示。
    pub fn visible_at(&self, d: &Danmaku, now: f64) -> bool {
        self.visible_at_in(d, now, self.screen_width)
    }

    /// 同 [`Self::visible_at`]，但用调用方当前的弹幕区宽度。
    pub fn visible_at_in(&self, d: &Danmaku, now: f64, screen_width: f32) -> bool {
        let elapsed = now - d.time;
        elapsed >= 0.0 && elapsed <= self.total_duration_in(d.width, screen_width)
    }

    fn total_duration_in(&self, width: f32, screen_width: f32) -> f64 {
        ((screen_width.max(1.0) + width) / self.speed) as f64
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

    #[test]
    fn xml_parses_real_list_shape() {
        let xml = r#"<?xml version="1.0" encoding="UTF-8"?><i>
            <chatserver>chat.bilibili.com</chatserver>
            <d p="1.50,1,25,16777215,1700000000,0,abc,1,10">a&amp;b&lt;c&gt;</d>
            <d p="2.00,5,25,255,1700000001,0,def,2,10">顶部</d>
            <d p="3.00,4,25,65280,1700000002,0,ghi,3,10">&#39;底部&#x27;</d>
        </i>"#;
        let set = DanmakuSet::from_xml(xml);
        assert_eq!(set.len(), 3);
        assert_eq!(set.items()[0].text, "a&b<c>");
        assert_eq!(set.items()[0].color, (255, 255, 255));
        assert_eq!(set.items()[1].mode, DanmakuMode::Top);
        assert_eq!(set.items()[2].mode, DanmakuMode::Bottom);
        assert_eq!(set.items()[2].text, "'底部'");
    }

    #[test]
    fn xml_skips_broken_nodes_instead_of_failing() {
        let xml = r#"<i><d p="1,1,25,16777215">ok</d><d>缺 p</d><d p="坏,1,25,1">坏时间</d></i>"#;
        let set = DanmakuSet::from_xml(xml);
        assert_eq!(set.len(), 1);
        assert_eq!(set.items()[0].text, "ok");
    }

    #[test]
    fn xml_keeps_text_with_angle_brackets_escaped() {
        let xml = r#"<i><d p="1,1,25,16777215,0,0,a,1,10">&lt;-- 这里 &amp; 那里--&gt;</d></i>"#;
        let set = DanmakuSet::from_xml(xml);
        assert_eq!(set.items()[0].text, "<-- 这里 & 那里-->");
    }

    #[test]
    fn spread_assignment_rotates_lanes_instead_of_stacking() {
        let mut a = LaneAllocator::new(1280.0, 220.0);
        // 5 条间隔 1 秒的短弹幕：旧 assign 会全塞进第 0 条轨道，
        // 轮转分配应依次落到 0..4。
        let lanes: Vec<Option<usize>> = (0..5)
            .map(|i| a.assign_spread(&dm(i as f64, "？"), 14))
            .collect();
        assert_eq!(lanes, vec![Some(0), Some(1), Some(2), Some(3), Some(4)]);
        assert_eq!(a.lane_count(), 5);
    }

    #[test]
    fn spread_assignment_wraps_and_reuses_free_lanes() {
        let mut a = LaneAllocator::new(1280.0, 220.0);
        for i in 0..14 {
            assert_eq!(a.assign_spread(&dm(i as f64 * 0.1, "弹幕"), 14), Some(i));
        }
        // 第 15 条：游标回到 0；时间已经过了 1.4 秒，0 号轨道应能复用。
        assert_eq!(a.assign_spread(&dm(2.0, "弹幕"), 14), Some(0));
        assert_eq!(a.lane_count(), 14);
    }

    #[test]
    fn spread_assignment_returns_none_when_all_lanes_full() {
        let mut a = LaneAllocator::new(1280.0, 220.0);
        let d = dm(0.0, "很长的弹幕内容");
        assert_eq!(a.assign_spread(&d, 2), Some(0));
        assert_eq!(a.assign_spread(&d, 2), Some(1));
        assert_eq!(a.assign_spread(&d, 2), None, "轨道满了应丢弃而不是叠字");
    }

    #[test]
    fn spread_assignment_keeps_no_overlap_property() {
        let mut a = LaneAllocator::new(1280.0, 200.0);
        let screen_w = 1280.0f32;
        let mut placed: Vec<(usize, Danmaku)> = Vec::new();
        for i in 0..30 {
            let d = dm(i as f64 * 0.2, "中等长度的弹幕");
            let lane = a.assign_spread(&d, 14).expect("轨道不应这么快满");
            placed.push((lane, d));
        }
        for lane in 0..a.lane_count() {
            let in_lane: Vec<&Danmaku> =
                placed.iter().filter(|(l, _)| *l == lane).map(|(_, d)| d).collect();
            for w in in_lane.windows(2) {
                let prev_right = a.x_at(w[0], w[1].time) + w[0].width;
                assert!(
                    prev_right <= screen_w + 1.0,
                    "轨道 {lane}：后一条出现时前一条尾巴还没进屏（{prev_right} > {screen_w}）"
                );
            }
        }
    }

    #[test]
    fn x_and_visibility_use_runtime_area_width() {
        let a = LaneAllocator::new(1280.0, 200.0);
        let d = dm(0.0, "x");
        // 窗口缩放后弹幕区从 1280 变 800：x 起点应该跟着变，
        // 而不是还从 1280 开始（否则会从面板底下冒出来）。
        assert!((a.x_at_in(&d, 0.0, 800.0) - 800.0).abs() < 1e-3);
        // 总时长 = (800 + 25) / 200 = 4.125s。
        assert!(a.visible_at_in(&d, 3.0, 800.0));
        assert!(!a.visible_at_in(&d, 5.0, 800.0));
    }
}
