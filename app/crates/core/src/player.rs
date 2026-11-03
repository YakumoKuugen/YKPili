//! 播放器状态（纯逻辑）。
//!
//! 对应旧 `YKPili/ViewModels/VideoPlayerViewModel.cs` 里**与平台播放器无关**的部分：
//! 播放/暂停、音量与静音、倍速档位、全屏、进度换算、HUD 计时、偏好持久化。
//!
//! 不迁的部分：`AttachPlayer` / `IPlatformMediaPlayer` / 位置定时器 —— 那些直接对接
//! AVFoundation / libmpv，在 GPUI 侧由 `surface(CVPixelBuffer)` 那一条链路承担。

use std::path::Path;

use serde::{Deserialize, Serialize};

use crate::format;

/// 可选倍速档位，由快到慢。与 B 站播放器一致。
pub const RATE_OPTIONS: [f64; 6] = [2.0, 1.5, 1.25, 1.0, 0.75, 0.5];

/// 默认倍速。
pub const DEFAULT_RATE: f64 = 1.0;

/// 音量步进（滚轮 / 方向键一格）。
pub const VOLUME_STEP: f64 = 0.05;

/// HUD 显示时长（毫秒）。旧实现是 **1100**，不是 1500 —— 连续操作时只让最后一次生效。
pub const HUD_DURATION_MS: u64 = 1100;

/// 播放器状态。
#[derive(Debug, Clone)]
pub struct PlayerState {
    pub playing: bool,
    pub volume: f64,
    pub muted: bool,
    /// 倍速。恒在 `RATE_OPTIONS` 内，除非从偏好文件读到异常值。
    pub rate: f64,
    pub fullscreen: bool,
    /// 已播放秒数。
    pub position_secs: f64,
    /// 总时长秒数；0 表示平台未提供。
    pub duration_secs: f64,
    /// 静音前的音量，取消静音时恢复。
    mute_memory: f64,
}

impl Default for PlayerState {
    fn default() -> Self {
        Self {
            playing: false,
            volume: 1.0,
            muted: false,
            rate: DEFAULT_RATE,
            fullscreen: false,
            position_secs: 0.0,
            duration_secs: 0.0,
            mute_memory: 1.0,
        }
    }
}

impl PlayerState {
    pub fn new() -> Self {
        Self::default()
    }

    // ---- 播放 ----

    pub fn toggle_play(&mut self) {
        self.playing = !self.playing;
    }

    pub fn play_pause_glyph(&self) -> &'static str {
        if self.playing {
            "❚❚"
        } else {
            "▶"
        }
    }

    // ---- 音量 ----

    /// 静音开关。开启时记住当前音量，关闭时恢复。
    pub fn toggle_mute(&mut self) {
        if self.muted {
            self.muted = false;
            self.volume = self.mute_memory.clamp(0.0, 1.0);
        } else {
            self.mute_memory = self.volume;
            self.muted = true;
        }
    }

    /// 调整音量（delta 可正可负），自动夹到 `0..=1`。
    ///
    /// 与旧实现一致：从静音状态往上调会**自动解除静音**。
    pub fn adjust_volume(&mut self, delta: f64) {
        let target = (self.volume + delta).clamp(0.0, 1.0);
        self.volume = target;
        if self.muted && target > 0.0 {
            self.muted = false;
        }
    }

    pub fn set_volume(&mut self, v: f64) {
        self.volume = v.clamp(0.0, 1.0);
        if self.muted && self.volume > 0.0 {
            self.muted = false;
        }
    }

    pub fn volume_glyph(&self) -> &'static str {
        if self.muted || self.volume <= 0.001 {
            "🔇"
        } else if self.volume < 0.5 {
            "🔉"
        } else {
            "🔊"
        }
    }

    pub fn volume_text(&self) -> String {
        format!("{}%", (self.volume * 100.0).round() as i32)
    }

    // ---- 倍速 ----

    /// 循环到下一档。当前值不在档位里时回到 1.0（与旧实现一致）。
    pub fn cycle_rate(&mut self) {
        let idx = RATE_OPTIONS
            .iter()
            .position(|&r| (r - self.rate).abs() < 1e-9);
        self.rate = match idx {
            Some(i) => RATE_OPTIONS[(i + 1) % RATE_OPTIONS.len()],
            None => DEFAULT_RATE,
        };
    }

    /// 直接设置档位；不在 `RATE_OPTIONS` 内的值会被夹到最近的档位。
    pub fn set_rate(&mut self, rate: f64) {
        let nearest = RATE_OPTIONS
            .iter()
            .copied()
            .min_by(|a, b| {
                (a - rate)
                    .abs()
                    .partial_cmp(&(b - rate).abs())
                    .unwrap_or(std::cmp::Ordering::Equal)
            })
            .unwrap_or(DEFAULT_RATE);
        self.rate = nearest;
    }

    pub fn rate_text(&self) -> String {
        if (self.rate - 1.0).abs() < 1e-9 {
            "倍速".to_string()
        } else {
            format!("{}x", trim_float(self.rate))
        }
    }

    // ---- 全屏 / 进度 ----

    pub fn toggle_fullscreen(&mut self) {
        self.fullscreen = !self.fullscreen;
    }

    /// 播放进度 `0.0..=1.0`。总时长未知时返回 0（界面据此降级）。
    pub fn progress(&self) -> f64 {
        if self.duration_secs <= 0.0 {
            return 0.0;
        }
        (self.position_secs / self.duration_secs).clamp(0.0, 1.0)
    }

    /// 进度条拖动：把 `0..=1` 的位置换算成秒。
    pub fn seek_to_fraction(&mut self, fraction: f64) {
        let f = fraction.clamp(0.0, 1.0);
        self.position_secs = f * self.duration_secs.max(0.0);
    }

    /// `(已播放文本, 总时长文本)`。总时长未知时右侧显示 `--:--`。
    pub fn time_texts(&self) -> (String, String) {
        let left = format::duration(self.position_secs.max(0.0) as u64);
        let right = if self.duration_secs > 0.0 {
            format::duration(self.duration_secs as u64)
        } else {
            "--:--".to_string()
        };
        (left, right)
    }

    /// 暂停时画面压暗的不透明度（0 = 播放，0.5 = 暂停）。
    pub fn dim_opacity(&self) -> f32 {
        if self.playing {
            0.0
        } else {
            0.5
        }
    }
}

/// 去掉浮点数的尾随零：`1.50 → "1.5"`，`2.00 → "2"`。
fn trim_float(v: f64) -> String {
    let s = format!("{v:.2}");
    let s = s.trim_end_matches('0').trim_end_matches('.');
    s.to_string()
}

/// 播放器偏好（跨启动记忆音量 / 静音 / 倍速）。
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct PlayerPrefs {
    pub volume: f64,
    pub muted: bool,
    pub rate: f64,
}

impl Default for PlayerPrefs {
    fn default() -> Self {
        Self { volume: 1.0, muted: false, rate: DEFAULT_RATE }
    }
}

impl PlayerPrefs {
    /// 从状态里提取偏好。
    pub fn from_state(s: &PlayerState) -> Self {
        Self { volume: s.volume, muted: s.muted, rate: s.rate }
    }

    /// 应用到状态。数值会被夹紧，倍速会被对齐到最近档位 ——
    /// 防止手改配置文件写进 `8.0` 这种值。
    pub fn apply_to(&self, s: &mut PlayerState) {
        s.volume = self.volume.clamp(0.0, 1.0);
        s.muted = self.muted;
        s.set_rate(self.rate);
    }

    pub fn load(path: &Path) -> Self {
        std::fs::read_to_string(path)
            .ok()
            .and_then(|t| serde_json::from_str(&t).ok())
            .unwrap_or_default()
    }

    pub fn save(&self, path: &Path) -> std::io::Result<()> {
        if let Some(parent) = path.parent() {
            std::fs::create_dir_all(parent)?;
        }
        let json = serde_json::to_string_pretty(self)
            .map_err(|e| std::io::Error::new(std::io::ErrorKind::Other, e))?;
        std::fs::write(path, json)
    }
}

/// 播放是否已经到达结尾。
///
/// 两个信号取「或」：
/// - `eof_reached`：mpv 的 `eof-reached`，最准确；
/// - `position_secs >= duration_secs - PLAYBACK_END_TOLERANCE_SECS`：
///   `eof-reached` 读不到（老版本 / 文件被提前卸载）时的兜底。
///
/// 时长未知（0）且没有 eof 时**绝不**判定结束 —— 否则刚起播就会被当成播完。
pub fn is_playback_finished(position_secs: f64, duration_secs: f64, eof_reached: bool) -> bool {
    if eof_reached {
        return true;
    }
    if !position_secs.is_finite() || !duration_secs.is_finite() {
        return false;
    }
    duration_secs > 0.0
        && position_secs > 0.0
        && position_secs >= duration_secs - PLAYBACK_END_TOLERANCE_SECS
}

/// 距结尾小于这个秒数视为「已播完」。
///
/// 0.25s 是为了吸收 mpv 的 `time-pos` 与 `duration` 在最后一帧的抖动，
/// 再大就会明显提前掐掉片尾。
pub const PLAYBACK_END_TOLERANCE_SECS: f64 = 0.25;

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn toggle_play_flips_and_glyph_follows() {
        let mut s = PlayerState::new();
        assert_eq!(s.play_pause_glyph(), "▶");
        s.toggle_play();
        assert!(s.playing);
        assert_eq!(s.play_pause_glyph(), "❚❚");
    }

    #[test]
    fn mute_remembers_and_restores_volume() {
        let mut s = PlayerState::new();
        s.volume = 0.6;
        s.toggle_mute();
        assert!(s.muted);
        assert_eq!(s.volume, 0.6, "静音不应清空音量值");
        s.toggle_mute();
        assert!(!s.muted);
        assert_eq!(s.volume, 0.6, "取消静音应恢复原音量");
    }

    #[test]
    fn adjusting_volume_unmutes() {
        let mut s = PlayerState::new();
        s.volume = 0.4;
        s.toggle_mute();
        assert!(s.muted);
        s.adjust_volume(VOLUME_STEP);
        assert!(!s.muted, "从静音往上调应自动解除静音");
        assert!((s.volume - 0.45).abs() < 1e-9);
    }

    #[test]
    fn volume_is_clamped() {
        let mut s = PlayerState::new();
        s.adjust_volume(-5.0);
        assert_eq!(s.volume, 0.0);
        assert_eq!(s.volume_glyph(), "🔇");
        s.adjust_volume(5.0);
        assert_eq!(s.volume, 1.0);
        assert_eq!(s.volume_glyph(), "🔊");
        assert_eq!(s.volume_text(), "100%");
    }

    #[test]
    fn low_volume_glyph() {
        let mut s = PlayerState::new();
        s.set_volume(0.3);
        assert_eq!(s.volume_glyph(), "🔉");
    }

    #[test]
    fn rate_cycles_through_options() {
        let mut s = PlayerState::new();
        assert_eq!(s.rate, 1.0);
        s.cycle_rate(); // 1.0 -> 0.75
        assert_eq!(s.rate, 0.75);
        s.cycle_rate(); // 0.75 -> 0.5
        assert_eq!(s.rate, 0.5);
        s.cycle_rate(); // 0.5 -> 2.0（回绕）
        assert_eq!(s.rate, 2.0);
    }

    #[test]
    fn unknown_rate_falls_back_to_normal() {
        let mut s = PlayerState::new();
        s.rate = 3.7;
        s.cycle_rate();
        assert_eq!(s.rate, DEFAULT_RATE, "非档位值应先回到 1.0");
    }

    #[test]
    fn set_rate_snaps_to_nearest() {
        let mut s = PlayerState::new();
        s.set_rate(1.4);
        assert_eq!(s.rate, 1.5);
        s.set_rate(0.1);
        assert_eq!(s.rate, 0.5);
    }

    #[test]
    fn rate_text_hides_one_x() {
        let mut s = PlayerState::new();
        assert_eq!(s.rate_text(), "倍速");
        s.set_rate(1.5);
        assert_eq!(s.rate_text(), "1.5x");
        s.set_rate(2.0);
        assert_eq!(s.rate_text(), "2x");
    }

    #[test]
    fn progress_and_seek() {
        let mut s = PlayerState::new();
        s.duration_secs = 200.0;
        s.position_secs = 50.0;
        assert!((s.progress() - 0.25).abs() < 1e-9);
        s.seek_to_fraction(0.5);
        assert!((s.position_secs - 100.0).abs() < 1e-9);
    }

    #[test]
    fn unknown_duration_degrades() {
        let mut s = PlayerState::new();
        s.position_secs = 30.0;
        assert_eq!(s.progress(), 0.0, "总时长未知时进度应为 0");
        let (l, r) = s.time_texts();
        assert_eq!(l, "00:30");
        assert_eq!(r, "--:--");
    }

    #[test]
    fn time_texts_and_dim() {
        let mut s = PlayerState::new();
        s.duration_secs = 754.0;
        s.position_secs = 61.0;
        assert_eq!(s.time_texts(), ("01:01".into(), "12:34".into()));
        assert_eq!(s.dim_opacity(), 0.5);
        s.playing = true;
        assert_eq!(s.dim_opacity(), 0.0);
    }

    #[test]
    fn prefs_roundtrip_and_clamp() {
        let dir = std::env::temp_dir().join(format!("yk-prefs-{}", std::process::id()));
        let path = dir.join("player.json");
        let mut s = PlayerState::new();
        s.volume = 0.35;
        s.set_rate(2.0);
        PlayerPrefs::from_state(&s).save(&path).unwrap();

        let mut s2 = PlayerState::new();
        PlayerPrefs::load(&path).apply_to(&mut s2);
        assert!((s2.volume - 0.35).abs() < 1e-9);
        assert_eq!(s2.rate, 2.0);

        // 手改配置成越界值也不能把状态弄坏
        let bad = PlayerPrefs { volume: 9.0, muted: false, rate: 8.0 };
        bad.apply_to(&mut s2);
        assert_eq!(s2.volume, 1.0);
        assert_eq!(s2.rate, 2.0, "8.0 应对齐到最近的档位");
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn hud_duration_matches_legacy() {
        assert_eq!(HUD_DURATION_MS, 1100, "旧实现是 1100ms");
    }

    #[test]
    fn playback_finished_by_eof() {
        assert!(is_playback_finished(0.0, 0.0, true), "eof 单独就够");
    }

    #[test]
    fn playback_finished_by_position() {
        // 距结尾 0.25s 内算播完
        assert!(is_playback_finished(100.0, 100.0, false));
        assert!(is_playback_finished(99.8, 100.0, false));
        // 还差得多：不能提前收
        assert!(!is_playback_finished(99.0, 100.0, false));
        assert!(!is_playback_finished(0.0, 100.0, false));
    }

    #[test]
    fn unknown_duration_never_finishes_without_eof() {
        assert!(!is_playback_finished(30.0, 0.0, false));
        assert!(!is_playback_finished(0.0, 0.0, false));
        assert!(!is_playback_finished(f64::NAN, 100.0, false));
        assert!(!is_playback_finished(100.0, f64::INFINITY, false));
    }
}
