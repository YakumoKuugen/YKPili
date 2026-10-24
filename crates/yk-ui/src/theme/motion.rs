//! 动效：时长与缓动曲线。
//!
//! 对应 `YKPili/Styles/Tokens/Motion.axaml`。
//!
//! **GPUI 注意（两条坑，见设计文档附录 D · D.1）**：
//! 1. `gpui_component::Theme::motion_tokens` 的预设是 120 / 180 / 280ms，
//!    与项目的 150 / 200 / 300 **对不上** —— 一律显式构造
//!    `Animation::new(Duration::from_millis(DURATION_FAST_MS))`，禁止用 `duration_fast` 凑近似。
//! 2. `gpui_base::animation::cubic_bezier` 会把输出 clamp 到 [0, 1]，
//!    而「强调」曲线带轻微回弹（y 控制点 1.3 > 1），直接用会被抹平。
//!    因此这里自带一个**不裁剪**的贝塞尔实现。

use std::time::Duration;

/// 150ms —— 悬停、按压等即时反馈。
pub const DURATION_FAST_MS: u64 = 150;
/// 200ms —— 常规过渡（面板展开、状态切换）。
pub const DURATION_NORMAL_MS: u64 = 200;
/// 300ms —— 大块位移、入场。
pub const DURATION_MODERATE_MS: u64 = 300;

pub const DURATION_FAST: Duration = Duration::from_millis(DURATION_FAST_MS);
pub const DURATION_NORMAL: Duration = Duration::from_millis(DURATION_NORMAL_MS);
pub const DURATION_MODERATE: Duration = Duration::from_millis(DURATION_MODERATE_MS);

// 曲线控制点（CSS cubic-bezier 的四个分量），保留原始数值便于比对。
/// `cubic-bezier(0.22, 0.61, 0.36, 1)` —— 标准减速。
pub const EASE_STANDARD: (f32, f32, f32, f32) = (0.22, 0.61, 0.36, 1.0);
/// `cubic-bezier(0.34, 1.3, 0.64, 1)` —— 带轻微回弹，用于交互按压。
pub const EASE_EMPHASIZED: (f32, f32, f32, f32) = (0.34, 1.3, 0.64, 1.0);

/// 标准缓动（`cubic-bezier(0.22, 0.61, 0.36, 1)`）。
pub fn ease_standard() -> impl Fn(f32) -> f32 {
    let (x1, y1, x2, y2) = EASE_STANDARD;
    bezier_ease(x1, y1, x2, y2)
}

/// 强调缓动（带回弹，`cubic-bezier(0.34, 1.3, 0.64, 1)`）。
///
/// **不用框架自带的 cubic_bezier**：它把输出裁到 [0, 1]，会吃掉 y1 = 1.3 的过冲，
/// 表现为「按压没有回弹手感」却很难定位原因。
pub fn ease_emphasized() -> impl Fn(f32) -> f32 {
    let (x1, y1, x2, y2) = EASE_EMPHASIZED;
    bezier_ease(x1, y1, x2, y2)
}

/// 不裁剪的 cubic-bezier 求值器。
///
/// 输入是时间进度 `t ∈ [0, 1]`；先由 x 方向解出曲线参数 `s`
/// （x 在 [0,1] 上单调，用二分），再取 y(s)。y 允许超出 [0, 1]（回弹）。
fn bezier_ease(x1: f32, y1: f32, x2: f32, y2: f32) -> impl Fn(f32) -> f32 {
    move |t: f32| {
        let t = t.clamp(0.0, 1.0);
        let (mut lo, mut hi) = (0.0_f32, 1.0_f32);
        for _ in 0..24 {
            let mid = (lo + hi) * 0.5;
            if cubic(0.0, x1, x2, 1.0, mid) < t {
                lo = mid;
            } else {
                hi = mid;
            }
        }
        let s = (lo + hi) * 0.5;
        cubic(0.0, y1, y2, 1.0, s)
    }
}

/// 一维三次贝塞尔求值。
#[inline]
fn cubic(p0: f32, p1: f32, p2: f32, p3: f32, s: f32) -> f32 {
    let u = 1.0 - s;
    u * u * u * p0 + 3.0 * u * u * s * p1 + 3.0 * u * s * s * p2 + s * s * s * p3
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn emphasized_overshoots() {
        let f = ease_emphasized();
        assert!((f(0.0) - 0.0).abs() < 1e-5);
        assert!((f(1.0) - 1.0).abs() < 1e-3);
        // 曲线中段应出现 > 1 的过冲（被裁剪的实现会恒 <= 1）
        let peak = (1..100).map(|i| f(i as f32 / 100.0)).fold(0.0_f32, f32::max);
        assert!(peak > 1.01, "回弹被抹平了，peak = {peak}");
    }

    #[test]
    fn standard_is_monotonic() {
        let f = ease_standard();
        let mut prev = -1.0;
        for i in 0..=100 {
            let v = f(i as f32 / 100.0);
            assert!(v >= prev - 1e-4);
            prev = v;
        }
    }
}
