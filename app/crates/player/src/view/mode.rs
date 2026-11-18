//! 播放器插槽协议：外壳只读这些「模型」，不认识点播 / 直播业务。
//!
//! 设计见 `docs/播放器-点播与直播共用外壳.md` 4.3 / 4.6：
//! - `ControlBarModel`：控制栏需要的全部数据（进度 / LIVE / 倍速 / 画质）；
//! - `PanelSpec`：面板 Tab 元数据，内容由 `panel_body()` 分派；
//! - `ModeKind`：当前模式，用于动作分派，不用于布局。
//!
//! 真正的布局差异用「插槽有没有」表达：直播没有 `timeline`、没有 `speed`，
//! 而不是在 shell 里写 `if is_live { ... }`。

use crate::PlayerView;

/// 当前播放形态。
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum ModeKind {
    Video,
    Live,
}

impl ModeKind {
    pub(crate) fn is_live(self) -> bool {
        matches!(self, ModeKind::Live)
    }
}

/// 点播进度（直播为 None）。
#[derive(Debug, Clone, Copy)]
pub(crate) struct Timeline {
    pub position: f64,
    pub duration: f64,
}

/// 直播角标：LIVE + 已看时长。
#[derive(Debug, Clone, Copy)]
pub(crate) struct LiveBadge {
    pub watched_secs: f64,
}

#[derive(Debug, Clone)]
pub(crate) struct QualityItem {
    pub id: i32,
    pub label: String,
    pub vip_only: bool,
}

#[derive(Debug, Clone)]
pub(crate) struct QualityMenu {
    pub current: i32,
    pub label: String,
    pub items: Vec<QualityItem>,
    pub switching: bool,
}

/// 控制栏的全部输入。
#[derive(Debug, Clone)]
pub(crate) struct ControlBarModel {
    pub kind: ModeKind,
    pub playing: bool,
    pub fullscreen: bool,
    pub muted: bool,
    pub volume: f64,
    pub rate: f64,
    /// Some = 点播；None = 直播（不画进度轨道）。
    pub timeline: Option<Timeline>,
    /// Some = 直播；控制栏换成 LIVE 胶囊 + 已看时长。
    pub live: Option<LiveBadge>,
    /// 直播为 false：倍速菜单不出现。
    pub speed: bool,
    pub quality: Option<QualityMenu>,
}

/// 面板 Tab（只有元数据，内容由 `PlayerView::panel_body` 渲染）。
#[derive(Debug, Clone)]
pub(crate) struct PanelSpec {
    pub tabs: Vec<&'static str>,
    pub active: usize,
}

/// 画面角标（如「直播中」）。
#[derive(Debug, Clone)]
pub(crate) struct BadgeSpec {
    pub text: String,
    pub tone: BadgeTone,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum BadgeTone {
    Live,
}

impl PlayerView {
    pub(crate) fn mode_kind(&self) -> ModeKind {
        if self.live.is_some() {
            ModeKind::Live
        } else {
            ModeKind::Video
        }
    }

    pub(crate) fn is_live(&self) -> bool {
        self.mode_kind().is_live()
    }

    /// 当前面板 Tab：点播用 `tab`，直播用 `live.tab`，两边互不串。
    pub(crate) fn active_tab(&self) -> usize {
        match self.live.as_ref() {
            Some(live) => live.tab,
            None => self.tab,
        }
    }

    pub(crate) fn set_active_tab(&mut self, index: usize) {
        if let Some(live) = self.live.as_mut() {
            live.tab = index.min(1);
        } else {
            self.tab = index.min(2);
        }
    }

    /// 控制栏插槽：点播与直播共用同一份类型，差异用 Option 表达。
    pub(crate) fn control_bar_model(&self) -> ControlBarModel {
        let source = self.source.as_ref();
        let kind = self.mode_kind();
        let live = self
            .live
            .as_ref()
            .map(|_| LiveBadge {
                // 已看时长直接取播放循环累计的 play_time：暂停后不再增长，
                // 与「进度」语义一致。
                watched_secs: self.play_time.max(0.0),
            });
        let timeline = (!kind.is_live()).then(|| Timeline {
            position: self.model.state.position_secs,
            duration: self.model.state.duration_secs,
        });
        let quality = source.map(|source| QualityMenu {
            current: source.quality_id,
            label: if source.quality.trim().is_empty() {
                "画质".to_string()
            } else {
                source.quality.trim().to_string()
            },
            items: source
                .qualities
                .iter()
                .map(|quality| QualityItem {
                    id: quality.id,
                    label: quality.label.clone(),
                    vip_only: quality.vip_only,
                })
                .collect(),
            switching: self.quality_switching
                || self
                    .live
                    .as_ref()
                    .map(|live| live.quality_switching)
                    .unwrap_or(false),
        });
        ControlBarModel {
            kind,
            playing: self.model.state.playing,
            fullscreen: self.model.state.fullscreen,
            muted: self.model.state.muted,
            volume: self.model.state.volume,
            rate: self.model.state.rate,
            timeline,
            live,
            speed: !kind.is_live(),
            quality,
        }
    }

    /// 面板插槽：点播 2~3 个 Tab，直播固定「弹幕 / 直播信息」。
    pub(crate) fn panel_spec(&self) -> PanelSpec {
        if self.live.is_some() {
            return PanelSpec {
                tabs: vec!["弹幕", "直播信息"],
                active: self.live.as_ref().map(|live| live.tab).unwrap_or(0),
            };
        }
        let multi = self
            .source
            .as_ref()
            .map(|source| source.pages.len() > 1)
            .unwrap_or(false);
        PanelSpec {
            tabs: if multi {
                vec!["评论", "弹幕", "选集"]
            } else {
                vec!["评论", "弹幕"]
            },
            active: self.tab,
        }
    }

    /// 画面角标：直播显示「直播中」，点播不显示。
    pub(crate) fn mode_badge(&self) -> Option<BadgeSpec> {
        self.live.as_ref().map(|_| BadgeSpec {
            text: "直播中".to_string(),
            tone: BadgeTone::Live,
        })
    }
}
