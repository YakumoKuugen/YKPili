//! 圆角 / 间距 / 控件几何。
//!
//! 对应 `YKPili/Styles/Tokens/Dimensions.axaml`。
//!
//! **设计约定（来自 BewlyCat，不要改）**：
//! - 布局使用 4px 网格；
//! - 2px 这一档保留给边框、描边与视觉微调，不用于常规间距或容器内边距；
//! - 圆角分「基础阶」与「语义别名」两级，组件应引用**语义别名**，
//!   这样卡片、媒体、弹层可以各自演进而不必逐页覆盖。
//!
//! **相对 Avalonia 的两处删除**（GPUI 迁移的必然结果）：
//! - 11 个 `Layer*` 层号：GPUI 没有 z-index，改由元素树顺序 /
//!   `deferred().with_priority()` 表达（设计文档附录 D · D.1 事实 2）。
//! - `PlayerControlBarHeight 50`（Avalonia 让位口径）：控制栏可以直接悬浮在画面下沿，
//!   只保留 `PLAYER_CONTROL_BAR_HEIGHT = 88`。

// ================= 圆角：基础阶 =================

pub const RADIUS_SM: f32 = 4.0;
pub const RADIUS_HALF: f32 = 6.0;
pub const RADIUS_MD: f32 = 8.0;
pub const RADIUS_LG: f32 = 12.0;
pub const RADIUS_XL: f32 = 16.0;
pub const RADIUS_2XL: f32 = 24.0;
pub const RADIUS_3XL: f32 = 32.0;
/// 胶囊/全圆。GPUI 可直接用 `.rounded_full()`，此值仅作语义记录。
pub const RADIUS_FULL: f32 = 9999.0;

// ================= 圆角：语义别名（组件引用这一组） =================

pub const CARD_RADIUS: f32 = 12.0;
pub const CARD_RADIUS_CONTINUOUS: f32 = 16.0;
pub const MEDIA_RADIUS: f32 = 12.0;
pub const MEDIA_RADIUS_CONTINUOUS: f32 = 16.0;
pub const PANEL_RADIUS: f32 = 12.0;
pub const MODAL_RADIUS: f32 = 24.0;
pub const POPOVER_RADIUS: f32 = 16.0;
pub const POPOVER_SURFACE_RADIUS: f32 = 12.0;
pub const INTERACTIVE_RADIUS: f32 = 8.0;
pub const BADGE_RADIUS: f32 = 9999.0;
pub const TOPBAR_MEDIA_RADIUS: f32 = 6.0;

/// 仅上两角圆角（12, 12, 0, 0）—— 顶部封面等。
pub const CORNER_TOP_ONLY: (f32, f32, f32, f32) = (12.0, 12.0, 0.0, 0.0);
/// 仅下两角圆角（0, 0, 12, 12）—— 底部弹层等。
pub const CORNER_BOTTOM_ONLY: (f32, f32, f32, f32) = (0.0, 0.0, 12.0, 12.0);

// ================= 间距：4px 网格 =================

pub const SPACE_0_5: f32 = 2.0;
pub const SPACE_1: f32 = 4.0;
pub const SPACE_2: f32 = 8.0;
pub const SPACE_3: f32 = 12.0;
pub const SPACE_4: f32 = 16.0;
pub const SPACE_5: f32 = 20.0;
pub const SPACE_6: f32 = 24.0;
pub const SPACE_8: f32 = 32.0;
pub const SPACE_10: f32 = 40.0;
pub const SPACE_12: f32 = 48.0;

// ================= 内边距 =================
// 记为 (水平, 垂直)，视图侧展开成 `.px(px(h)).py(px(v))`。

pub const PADDING_CONTROL_X: (f32, f32) = (12.0, 0.0);
pub const PADDING_CARD: (f32, f32) = (16.0, 16.0);
pub const PADDING_PANEL_HEADER: (f32, f32) = (16.0, 10.0);
pub const PADDING_PANEL_FOOTER: (f32, f32) = (12.0, 8.0);
pub const PADDING_BUTTON: (f32, f32) = (16.0, 8.0);

// ================= 控件几何 =================

pub const CONTROL_HEIGHT: f32 = 34.0;
pub const TOP_BAR_CONTROL_HEIGHT: f32 = 46.0;
pub const TOP_BAR_HEIGHT: f32 = 64.0;
pub const SCROLL_BAR_SIZE: f32 = 6.0;

pub const PLAYER_PANEL_WIDTH: f32 = 900.0;
pub const PLAYER_PANEL_HEIGHT: f32 = 550.0;
pub const PLAYER_ICON_BUTTON_SIZE: f32 = 32.0;
pub const PLAYER_TRANSPORT_BUTTON_WIDTH: f32 = 48.0;
pub const PLAYER_TRANSPORT_BUTTON_HEIGHT: f32 = 36.0;
pub const PLAYER_PAUSE_INDICATOR_SIZE: f32 = 80.0;

/// 播放器悬浮控制栏高度 = 弹幕输入 38 + 操作条 50。
///
/// **唯一口径**。旧 `PlayerControlBarHeight 50` 是 Avalonia「让位」时代的值
/// （原生视频视图无法被控件覆盖，只能让出高度），GPUI 下不需要，已删除。
pub const PLAYER_CONTROL_BAR_HEIGHT: f32 = 88.0;

// ================= 封面卡片 =================

pub const COVER_ITEM_WIDTH: f32 = 192.0;
pub const COVER_ITEM_HEIGHT: f32 = 108.0;
pub const COVER_CELL_WIDTH: f32 = 208.0;
/// 卡片单元格高：8 上边距 + 108 封面 + 4 间距 + 36 两行标题
/// + 2 行距 + 18 署名行 + 8 下边距 = 184（设计文档第 04 节的目标值）。
pub const COVER_CELL_HEIGHT: f32 = 184.0;

// ================= 播放浮窗信息面板 =================

/// 展开态面板宽度。
pub const PLAYER_INFO_PANEL_WIDTH: f32 = 400.0;
/// 收起态半宽 —— 这是**用户调好的规格**：收起后仍留 200px，且面板本体即 hover 热区。
/// 不要改成 0（曾经误改成完全隐藏，已回滚）。见设计文档第 06 节。
pub const PLAYER_INFO_PANEL_COLLAPSED_WIDTH: f32 = 200.0;

// ================= 移动端（MOBILE_* 前缀，进同一令牌源） =================
// 本轮只出令牌，M7/M8 才实施。

pub const MOBILE_TOUCH_TARGET_MIN: f32 = 44.0;
pub const MOBILE_TOUCH_GAP_MIN: f32 = 8.0;
pub const MOBILE_TOP_BAR_HEIGHT: f32 = 52.0;
pub const MOBILE_BOTTOM_TAB_BAR_HEIGHT: f32 = 56.0;
pub const MOBILE_PANEL_PADDING: (f32, f32) = (16.0, 0.0);
pub const MOBILE_FOLD_HINGE_GAP: f32 = 24.0;
/// 安全区默认值（编译期），运行期由壳层覆盖。
pub const MOBILE_SAFE_AREA_TOP: f32 = 0.0;
pub const MOBILE_SAFE_AREA_BOTTOM: f32 = 0.0;
pub const MOBILE_SAFE_AREA_LEFT: f32 = 0.0;
pub const MOBILE_SAFE_AREA_RIGHT: f32 = 0.0;
pub const SAFE_CORNER_RADIUS: f32 = 44.0;
