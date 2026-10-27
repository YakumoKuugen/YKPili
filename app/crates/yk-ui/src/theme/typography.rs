//! 排版阶。
//!
//! 对应 `YKPili/Styles/Tokens/Typography.axaml`。
//!
//! **设计意图（来自 BewlyCat，不要改）**：Body 与 Title 共用 15px 基准，
//! 层级差异由行高与字重承担，而不是靠堆叠字号。
//!
//! **GPUI 注意**：字号一律用 `text_size(px(n))` 显式像素，
//! **不要**用 `text_sm` / `text_base` 这类比例 —— GPUI 的 scale 是
//! 12/14/16/18/20/24/30（rem 制），项目的 13/15/28 对不上，套上去会改掉设计。
//! 见设计文档附录 D · D.1 事实 7。

/// 中文字体栈。
///
/// 原先该字符串在 `HomeView.axaml.cs` 与 `VideoPlayerView.axaml` 中各硬编码一份，
/// 此处统一为单一来源。
///
/// 注意：Inter 只提供拉丁字形且不含 CJK，放在中文之前会导致中文回退到系统衬线体。
pub const FONT_FAMILY_UI: &str =
    "PingFang SC, Microsoft YaHei, Noto Sans CJK SC, Source Han Sans SC, sans-serif";

/// 图标字体栈（仅用于极少数符号字形；常规图标走 `svg()`）。
pub const FONT_FAMILY_ICON: &str =
    "Segoe UI Symbol, Apple Symbols, Noto Sans Symbols, sans-serif";

// ================= 字号阶 =================
// 12 / 13 / 15 / 20 / 24 / 28 —— 锁定，不为框架的 rem 比例让路。

pub const FONT_SIZE_CAPTION: f32 = 12.0;
pub const FONT_SIZE_CONTROL: f32 = 13.0;
pub const FONT_SIZE_BODY: f32 = 15.0;
pub const FONT_SIZE_TITLE: f32 = 15.0;
/// 页面内分段标题（搜索结果「影视 / 用户 / 视频」、媒体大卡标题）。
pub const FONT_SIZE_SECTION: f32 = 17.0;
pub const FONT_SIZE_HEADING: f32 = 20.0;
pub const FONT_SIZE_DISPLAY: f32 = 24.0;
pub const FONT_SIZE_DATA_EMPHASIS: f32 = 28.0;
pub const FONT_SIZE_BASE: f32 = 15.0;

/// 角标 / 严重度 chip（`.sev`）。以前是散落的 10px 魔法数，现纳入令牌层。
pub const FONT_SIZE_MICRO: f32 = 10.0;

/// 封面数据层与状态标签（压在封面图上，配文字阴影）。以前是散落的 11px 魔法数。
pub const FONT_SIZE_DATA_OVERLAY: f32 = 11.0;

// ================= 行高 =================

pub const LINE_HEIGHT_CAPTION: f32 = 16.0;
pub const LINE_HEIGHT_CONTROL: f32 = 18.0;
pub const LINE_HEIGHT_BODY: f32 = 24.0;
pub const LINE_HEIGHT_TITLE: f32 = 22.0;
pub const LINE_HEIGHT_HEADING: f32 = 28.0;
pub const LINE_HEIGHT_DATA: f32 = 32.0;

// ================= 字重 =================
// GPUI 侧用 `FontWeight::REGULAR` / `MEDIUM` / `SEMIBOLD` / `BOLD`，
// 这里只记录语义档位，供视图选择。

pub const FONT_WEIGHT_REGULAR: u16 = 400;
pub const FONT_WEIGHT_MEDIUM: u16 = 500;
pub const FONT_WEIGHT_SEMIBOLD: u16 = 600;
pub const FONT_WEIGHT_BOLD: u16 = 700;

// ================= 图标尺寸 =================
// 这是几何尺寸，**不随字号缩放**（见设计文档附录 E · E-11 细则）。

pub const ICON_SIZE_SM: f32 = 16.0;
pub const ICON_SIZE_MD: f32 = 20.0;
pub const ICON_SIZE_LG: f32 = 24.0;
pub const ICON_SIZE_XL: f32 = 32.0;
