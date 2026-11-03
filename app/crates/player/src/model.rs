//! 播放器视图模型。
//!
//! 只放「界面需要知道、但不依赖 GPUI 生命周期」的播放状态：
//! 控制栏是否可见、当前音量、取流/播放错误。
//!
//! 真正的解码与时间轴在 `video_worker`（mpv）里；这里不重复持有帧，
//! 也不做渲染。`PlayerView`（`lib.rs`）持有一份 `PlayerModel`，
//! 控制器改它、视图读它，两边不会各存一套状态。

use yk_domain::player::PlayerState;

/// 播放器视图模型。
#[derive(Debug)]
pub(crate) struct PlayerModel {
    /// 播放状态（音量 / 倍速 / 进度 / 全屏），来自领域层的 `PlayerState`。
    pub(crate) state: PlayerState,
    /// 控制栏（连同底部黑框 / 左下角标）是否可见。
    pub(crate) controls_visible: bool,
    /// 5 秒空闲检测任务的代际：换源 / 隐藏时作废旧任务。
    pub(crate) controls_timer_generation: u64,
    /// 当前是否有一个空闲检测任务在跑，避免鼠标移动时重复 spawn。
    pub(crate) controls_timer_running: bool,
    /// 取流 / 播放失败的原因；`None` = 没有错误。
    ///
    /// 卡片没有 bvid、需要登录、视频受限都写在这里，视图只读这一份。
    pub(crate) error: Option<String>,
}

impl Default for PlayerModel {
    fn default() -> Self {
        Self::new()
    }
}

impl PlayerModel {
    pub(crate) fn new() -> Self {
        let mut state = PlayerState::new();
        // 演示数据：与旧 PlayerView 的初始值保持一致，视觉测试依赖这组数字。
        state.duration_secs = 754.0;
        state.position_secs = 256.0;
        state.playing = true;
        Self {
            state,
            controls_visible: true,
            controls_timer_generation: 0,
            controls_timer_running: false,
            error: None,
        }
    }

    /// 设置音量。只改视图模型；解码端的音量由控制器负责同步。
    pub(crate) fn set_volume(&mut self, volume: f64) {
        self.state.volume = volume.clamp(0.0, 1.0);
        if self.state.volume > 0.0 {
            self.state.muted = false;
        }
    }

    /// 取流失败原因。
    pub(crate) fn error(&self) -> Option<&str> {
        self.error.as_deref()
    }

    pub(crate) fn set_error(&mut self, error: impl Into<String>) {
        self.error = Some(error.into());
    }

    pub(crate) fn clear_error(&mut self) {
        self.error = None;
    }
}
