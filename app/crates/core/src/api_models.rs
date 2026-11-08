//! B 站接口的响应模型。
//!
//! 对应 `YKPili/Services/Bilibili/Models/*.cs`。
//! 字段名与接口一致（snake_case），不额外做驼峰转换 —— 少一层映射就少一处出错。

use serde::{Deserialize, Serialize};

// ================= 首页推荐 =================

#[derive(Debug, Clone, Deserialize)]
pub struct RecommendResponse {
    pub code: i64,
    pub data: Option<RecommendData>,
}

#[derive(Debug, Clone, Deserialize)]
pub struct RecommendData {
    #[serde(default)]
    pub item: Vec<RecommendVideoItem>,
}

/// 单个推荐视频条目。
///
/// **注意**：旧 C# 版本定义了 `owner` / `stat` 却没在映射时使用，
/// 导致卡片上没有 UP 主与播放量（见 `models::CoverItem::from_recommend` 的注释）。
/// Rust 版必须全量保留。
#[derive(Debug, Clone, Deserialize, Serialize, Default)]
pub struct RecommendVideoItem {
    #[serde(default)]
    pub id: i64,
    #[serde(default)]
    pub bvid: String,
    #[serde(default)]
    pub title: String,
    #[serde(default)]
    pub pic: String,
    #[serde(default)]
    pub desc: String,
    /// 时长（秒）。C# 版本没定义这个字段，所以卡片上一直没有时长。
    #[serde(default)]
    pub duration: u64,
    /// 发布时间（Unix 秒）。推荐接口的 `pubdate`，用于署名行的相对时间。
    #[serde(default)]
    pub pubdate: u64,
    /// 条目类型：普通视频为 `av`，直播卡为 `live`。用于区分直播角标。
    #[serde(default)]
    pub goto: String,
    #[serde(default)]
    pub owner: Option<VideoOwner>,
    #[serde(default)]
    pub stat: Option<VideoStat>,
}

#[derive(Debug, Clone, Deserialize, Serialize, Default)]
pub struct VideoOwner {
    #[serde(default)]
    pub mid: i64,
    #[serde(default)]
    pub name: String,
    #[serde(default)]
    pub face: String,
}

#[derive(Debug, Clone, Deserialize, Serialize, Default)]
pub struct VideoStat {
    #[serde(default)]
    pub view: u64,
    #[serde(default)]
    pub danmaku: u64,
    #[serde(default)]
    pub reply: u64,
    #[serde(default)]
    pub like: u64,
    #[serde(default)]
    pub coin: u64,
    #[serde(default)]
    pub favorite: u64,
    #[serde(default)]
    pub share: u64,
}

// ================= 视频详情 =================

#[derive(Debug, Clone, Deserialize)]
pub struct VideoInfoResponse {
    pub code: i64,
    pub data: Option<VideoInfoData>,
}

#[derive(Debug, Clone, Deserialize, Default)]
pub struct VideoInfoData {
    #[serde(default)]
    pub bvid: String,
    #[serde(default)]
    pub aid: i64,
    #[serde(default)]
    pub cid: i64,
    #[serde(default)]
    pub title: String,
    /// 简介，信息面板的 UP 主签名区直接用它。
    #[serde(default)]
    pub desc: String,
    /// 发布时间（Unix 秒）。
    #[serde(default)]
    pub pubdate: u64,
    /// 封面（原始地址，视图侧按显示尺寸再取）。
    #[serde(default)]
    pub pic: String,
    /// 分区名（如「游戏 · 射击游戏」）。
    #[serde(default)]
    pub tname: String,
    /// 视频时长（秒）。
    #[serde(default)]
    pub duration: u64,
    #[serde(default)]
    pub owner: Option<VideoOwner>,
    #[serde(default)]
    pub stat: Option<VideoStat>,
    #[serde(default)]
    pub pages: Vec<VideoPage>,
}

#[derive(Debug, Clone, Deserialize, Default, PartialEq, Eq)]
pub struct VideoPage {
    #[serde(default)]
    pub cid: i64,
    #[serde(default)]
    pub page: i32,
    #[serde(default)]
    pub part: String,
    /// 该 P 的时长（秒）。
    #[serde(default)]
    pub duration: u64,
}

// ================= 视频标签 =================

#[derive(Debug, Clone, Deserialize)]
pub struct VideoTagsResponse {
    pub code: i64,
    #[serde(default)]
    pub data: Option<Vec<VideoTagItem>>,
}

#[derive(Debug, Clone, Deserialize, Default)]
pub struct VideoTagItem {
    #[serde(default)]
    pub tag_name: String,
}

// ================= 播放地址 =================

#[derive(Debug, Clone, Deserialize)]
pub struct VideoPlayUrlResponse {
    pub code: i64,
    pub data: Option<PlayUrlData>,
}

#[derive(Debug, Clone, Deserialize, Default)]
pub struct PlayUrlData {
    /// 总时长（毫秒）。
    #[serde(default)]
    pub timelength: u64,
    #[serde(default)]
    pub dash: Option<DashData>,
    #[serde(default)]
    pub durl: Vec<DurlItem>,
    /// 当前账号（及当前视频）实际允许请求的画质 id 列表。
    ///
    /// 非大会员通常是 `[16, 32, 64, 80]`，大会员会多出 74/112/116/120/125/126/127。
    /// 画质菜单用它和 [`SupportFormat`] 做交集，避免列出点了也播不了的档位。
    #[serde(default)]
    pub accept_quality: Vec<i32>,
    /// 与 `accept_quality` 一一对应的展示文案（老接口兜底）。
    #[serde(default)]
    pub accept_description: Vec<String>,
    /// 视频本身支持的画质格式：带 `need_vip` / `need_login`，是画质菜单的首选数据源。
    #[serde(default)]
    pub support_formats: Vec<SupportFormat>,
    /// 风控凭证：`code=0` 但没有 dash/durl 时，B 站用它要求先过验证。
    ///
    /// 典型响应：`{"code":0,"data":{"v_voucher":"voucher_..."}}`。
    /// 这不是「视频不可播」：改用不带 WBI 的旧 playurl 端点通常仍能出流。
    #[serde(default)]
    pub v_voucher: Option<String>,
}

/// `data.support_formats[]`：某种画质在当前视频里的可用性与展示文案。
#[derive(Debug, Clone, Deserialize, Default)]
pub struct SupportFormat {
    /// 画质 id（qn），如 80 = 1080P。
    #[serde(default)]
    pub quality: i32,
    /// 新文案，如「1080P 高清」。
    #[serde(default)]
    pub new_description: String,
    /// 老文案，`new_description` 缺失时兜底。
    #[serde(default)]
    pub display_desc: String,
    /// 角标文案（如「大会员」）。
    #[serde(default)]
    pub superscript: String,
    /// 仅大会员可播。
    #[serde(default)]
    pub need_vip: bool,
    /// 需要登录才可播。
    #[serde(default)]
    pub need_login: bool,
    /// 该画质支持的编码（avc1 / hev1 / av01）。
    #[serde(default)]
    pub codecs: Vec<String>,
    #[serde(default)]
    pub format: String,
}

#[derive(Debug, Clone, Deserialize, Default)]
pub struct DashData {
    #[serde(default)]
    pub video: Vec<DashStream>,
    #[serde(default)]
    pub audio: Vec<DashStream>,
}

#[derive(Debug, Clone, Deserialize, Default)]
pub struct DashStream {
    #[serde(default)]
    pub id: i32,
    #[serde(rename = "baseUrl", default)]
    pub base_url: String,
    #[serde(rename = "base_url", default)]
    pub fallback_url: String,
    /// B 站返回的备用 CDN 地址；mcdn 节点经常只回一段，优先用 upos 镜像。
    #[serde(rename = "backupUrl", default)]
    pub backup_urls: Vec<String>,
    #[serde(default)]
    pub codecs: String,
    #[serde(default)]
    pub codec_id: i32,
    #[serde(default)]
    pub width: i32,
    #[serde(default)]
    pub height: i32,
}

#[derive(Debug, Clone, Deserialize, Default)]
pub struct DurlItem {
    #[serde(default)]
    pub url: String,
    #[serde(default)]
    pub length: u64,
}

// ================= 扫码登录 =================

#[derive(Debug, Clone, Deserialize)]
pub struct LoginQrCodeResponse {
    pub code: i64,
    pub data: Option<QrCodeData>,
    #[serde(default)]
    pub message: String,
}

#[derive(Debug, Clone, Deserialize, Default)]
pub struct QrCodeData {
    #[serde(default)]
    pub url: String,
    #[serde(default)]
    pub qrcode_key: String,
}

#[derive(Debug, Clone, Deserialize)]
pub struct LoginPollResponse {
    pub code: i64,
    pub data: Option<PollData>,
    #[serde(default)]
    pub message: String,
}

#[derive(Debug, Clone, Deserialize, Default)]
pub struct PollData {
    #[serde(default)]
    pub code: i64,
    #[serde(default)]
    pub message: String,
    #[serde(default)]
    pub url: String,
}

// ================= 用户信息 =================

#[derive(Debug, Clone, Deserialize)]
pub struct UserInfoResponse {
    pub code: i64,
    pub data: Option<UserInfoData>,
}

#[derive(Debug, Clone, Deserialize, Default)]
pub struct UserInfoData {
    #[serde(rename = "isLogin", default)]
    pub is_login: bool,
    #[serde(default)]
    pub mid: i64,
    #[serde(default)]
    pub uname: String,
    #[serde(default)]
    pub face: String,
    /// 硬币余额。
    #[serde(default)]
    pub money: f64,
    /// 钱包（B 币等）；未登录可能缺省。
    #[serde(default)]
    pub wallet: Option<WalletData>,
    /// 当前等级。
    #[serde(default)]
    pub level_info: Option<LevelInfo>,
    /// 大会员状态；未登录 / 普通用户为 `None`。
    #[serde(default)]
    pub vip: Option<VipInfo>,
    #[serde(default)]
    pub wbi_img: Option<WbiImg>,
}

/// `/x/web-interface/nav` 里的 `vip` 节点。
#[derive(Debug, Clone, Deserialize, Default)]
pub struct VipInfo {
    /// 1 = 大会员有效，0 = 非大会员 / 已过期。
    #[serde(default)]
    pub status: i32,
    /// 会员类型（1 = 月度，2 = 年度…）。
    #[serde(rename = "type", default)]
    pub kind: i32,
    #[serde(default)]
    pub label: Option<VipLabel>,
}

impl VipInfo {
    /// 当前是否处于有效大会员状态。
    pub fn is_active(&self) -> bool {
        self.status == 1
    }

    /// 会员文案（如「年度大会员」）；没有标签时为空串。
    pub fn label_text(&self) -> &str {
        self.label
            .as_ref()
            .map(|label| label.text.trim())
            .unwrap_or("")
    }
}

#[derive(Debug, Clone, Deserialize, Default)]
pub struct VipLabel {
    #[serde(default)]
    pub text: String,
}

#[derive(Debug, Clone, Deserialize, Default)]
pub struct WalletData {
    #[serde(default)]
    pub bcoin_balance: f64,
    #[serde(default)]
    pub coupon_balance: f64,
}

#[derive(Debug, Clone, Deserialize, Default)]
pub struct LevelInfo {
    #[serde(default)]
    pub current_level: u8,
}

/// `/x/web-interface/card`：名片里的 UP 主签名。
#[derive(Debug, Clone, Deserialize, Default)]
pub struct UserCardResponse {
    #[serde(default)]
    pub code: i64,
    pub data: Option<UserCardData>,
}

#[derive(Debug, Clone, Deserialize, Default)]
#[serde(default)]
pub struct UserCardData {
    pub card: Option<UserCardInfo>,
}

#[derive(Debug, Clone, Deserialize, Default)]
#[serde(default)]
pub struct UserCardInfo {
    #[serde(default)]
    pub sign: String,
}

#[derive(Debug, Clone, Deserialize, Default)]
pub struct WbiImg {
    #[serde(default)]
    pub img_url: String,
    #[serde(default)]
    pub sub_url: String,
}

// ================= 观看历史（history/cursor） =================
//
// 接口：`GET /x/web-interface/history/cursor?max=&view_at=&business=`
// 只认 Cookie，不需要 WBI 签名。响应里的 `progress` 是「已观看秒数」，
// 不是百分比；`duration` 为 0 的条目（直播）不能拿来做百分比。
//
// 这里的字段只保留 UI 用得到的那部分；`covers` / `current` / `total`
// 等字段对列表没有帮助，故意不声明。

#[derive(Debug, Clone, Deserialize, Default)]
pub struct HistoryResponse {
    #[serde(default)]
    pub code: i64,
    #[serde(default)]
    pub message: String,
    pub data: Option<HistoryData>,
}

#[derive(Debug, Clone, Deserialize, Default)]
#[serde(default)]
pub struct HistoryData {
    pub cursor: Option<HistoryCursorRaw>,
    #[serde(deserialize_with = "de_vec_or_null")]
    pub list: Vec<HistoryItemRaw>,
}

#[derive(Debug, Clone, Deserialize, Default)]
#[serde(default)]
pub struct HistoryCursorRaw {
    #[serde(deserialize_with = "de_i64_or_string")]
    pub max: i64,
    #[serde(deserialize_with = "de_i64_or_string")]
    pub view_at: i64,
    pub business: String,
    #[serde(deserialize_with = "de_i64_or_string")]
    pub ps: i64,
}

#[derive(Debug, Clone, Deserialize, Default)]
#[serde(default)]
pub struct HistoryItemRaw {
    pub title: String,
    pub long_title: String,
    pub cover: String,
    pub uri: String,
    #[serde(deserialize_with = "de_u64_or_string")]
    pub videos: u64,
    pub author_name: String,
    pub author_face: String,
    #[serde(deserialize_with = "de_i64_or_string")]
    pub author_mid: i64,
    #[serde(deserialize_with = "de_u64_or_string")]
    pub view_at: u64,
    #[serde(deserialize_with = "de_i64_or_string")]
    pub progress: i64,
    pub badge: String,
    pub show_title: String,
    #[serde(deserialize_with = "de_u64_or_string")]
    pub duration: u64,
    #[serde(deserialize_with = "de_i64_or_string")]
    pub is_finish: i64,
    #[serde(deserialize_with = "de_i64_or_string")]
    pub kid: i64,
    pub tag_name: String,
    pub history: HistoryInnerRaw,
}

#[derive(Debug, Clone, Deserialize, Default)]
#[serde(default)]
pub struct HistoryInnerRaw {
    #[serde(deserialize_with = "de_i64_or_string")]
    pub oid: i64,
    #[serde(deserialize_with = "de_i64_or_string")]
    pub epid: i64,
    pub bvid: String,
    #[serde(deserialize_with = "de_i64_or_string")]
    pub page: i64,
    #[serde(deserialize_with = "de_i64_or_string")]
    pub cid: i64,
    pub business: String,
}

// ================= 关注动态（web-dynamic） =================
//
// 动态接口是**模块化**的：每一条的 `modules` 下按需出现 author / dynamic / stat。
// 这里只声明 UI 用得到的字段；未知字段由 serde 忽略。
//
// 两个兼容点必须保留：
// - `pub_ts` 与各类 `count` 都可能是字符串（接口历史行为）；
// - 图片动态可能走 `major.draw.items`（旧）或 `major.opus.pics`（新）。

#[derive(Debug, Clone, Deserialize)]
pub struct DynamicFeedResponse {
    pub code: i64,
    #[serde(default)]
    pub message: String,
    pub data: Option<DynamicFeedData>,
}

#[derive(Debug, Clone, Deserialize, Default)]
#[serde(default)]
pub struct DynamicFeedData {
    #[serde(deserialize_with = "de_vec_or_null")]
    pub items: Vec<DynamicItemRaw>,
    pub offset: String,
    pub has_more: bool,
}

#[derive(Debug, Clone, Deserialize, Default)]
#[serde(default)]
pub struct DynamicItemRaw {
    pub id_str: String,
    #[serde(rename = "type")]
    pub kind: String,
    pub basic: DynamicBasicRaw,
    pub modules: DynamicModules,
    /// 转发动态的原动态（嵌套同构）。
    pub orig: Option<Box<DynamicItemRaw>>,
}

#[derive(Debug, Clone, Deserialize, Default)]
#[serde(default)]
pub struct DynamicBasicRaw {
    pub rid_str: String,
    pub comment_type: i64,
    pub comment_id_str: String,
}

#[derive(Debug, Clone, Deserialize, Default)]
#[serde(default)]
pub struct DynamicModules {
    pub module_author: Option<DynamicAuthorRaw>,
    pub module_dynamic: Option<DynamicContentRaw>,
    pub module_stat: Option<DynamicStatRaw>,
}

#[derive(Debug, Clone, Deserialize, Default)]
#[serde(default)]
pub struct DynamicAuthorRaw {
    pub mid: i64,
    pub name: String,
    pub face: String,
    pub pub_time: String,
    /// 秒级时间戳；接口偶尔给字符串。
    #[serde(deserialize_with = "de_u64_or_string")]
    pub pub_ts: u64,
    pub pub_action: String,
}

#[derive(Debug, Clone, Deserialize, Default)]
#[serde(default)]
pub struct DynamicContentRaw {
    pub desc: Option<RichTextRaw>,
    pub major: Option<DynamicMajorRaw>,
}

#[derive(Debug, Clone, Deserialize, Default)]
#[serde(default)]
pub struct RichTextRaw {
    pub text: String,
    #[serde(deserialize_with = "de_vec_or_null")]
    pub rich_text_nodes: Vec<RichTextNodeRaw>,
}

/// 富文本片段：`RICH_TEXT_NODE_TYPE_TEXT` / `AT` / `EMOJI` / `TOPIC` …
#[derive(Debug, Clone, Deserialize, Default)]
#[serde(default)]
pub struct RichTextNodeRaw {
    pub text: String,
    #[serde(rename = "type")]
    pub kind: String,
}

#[derive(Debug, Clone, Deserialize, Default)]
#[serde(default)]
pub struct DynamicMajorRaw {
    #[serde(rename = "type")]
    pub kind: String,
    pub archive: Option<DynamicArchiveRaw>,
    pub draw: Option<DynamicDrawRaw>,
    pub opus: Option<DynamicOpusRaw>,
    pub article: Option<DynamicArticleRaw>,
    pub live: Option<DynamicLiveRaw>,
    pub common: Option<DynamicCommonRaw>,
}

#[derive(Debug, Clone, Deserialize, Default)]
#[serde(default)]
pub struct DynamicArchiveRaw {
    pub bvid: String,
    pub title: String,
    pub cover: String,
    pub desc: String,
    pub duration_text: String,
    pub stat: Option<DynamicArchiveStatRaw>,
}

#[derive(Debug, Clone, Deserialize, Default)]
#[serde(default)]
pub struct DynamicArchiveStatRaw {
    #[serde(deserialize_with = "de_u64_or_string")]
    pub play: u64,
    #[serde(deserialize_with = "de_u64_or_string")]
    pub danmaku: u64,
}

#[derive(Debug, Clone, Deserialize, Default)]
#[serde(default)]
pub struct DynamicDrawRaw {
    #[serde(deserialize_with = "de_vec_or_null")]
    pub items: Vec<DynamicPictureRaw>,
}

#[derive(Debug, Clone, Deserialize, Default)]
#[serde(default)]
pub struct DynamicOpusRaw {
    pub title: String,
    pub summary: Option<RichTextRaw>,
    #[serde(deserialize_with = "de_vec_or_null")]
    pub pics: Vec<DynamicPictureRaw>,
}

/// 图片项：旧版 draw 用 `src`，新版 opus 用 `url`。
#[derive(Debug, Clone, Deserialize, Default)]
#[serde(default)]
pub struct DynamicPictureRaw {
    pub src: String,
    pub url: String,
    pub width: u32,
    pub height: u32,
}

#[derive(Debug, Clone, Deserialize, Default)]
#[serde(default)]
pub struct DynamicArticleRaw {
    pub title: String,
    pub desc: String,
    #[serde(deserialize_with = "de_vec_or_null")]
    pub covers: Vec<String>,
}

#[derive(Debug, Clone, Deserialize, Default)]
#[serde(default)]
pub struct DynamicLiveRaw {
    pub title: String,
    pub cover: String,
    pub desc_first: String,
}

#[derive(Debug, Clone, Deserialize, Default)]
#[serde(default)]
pub struct DynamicCommonRaw {
    pub title: String,
    pub desc: String,
    pub cover: String,
}

#[derive(Debug, Clone, Deserialize, Default)]
#[serde(default)]
pub struct DynamicStatRaw {
    pub forward: Option<DynamicCountRaw>,
    pub comment: Option<DynamicCountRaw>,
    pub like: Option<DynamicCountRaw>,
}

#[derive(Debug, Clone, Deserialize, Default)]
#[serde(default)]
pub struct DynamicCountRaw {
    #[serde(deserialize_with = "de_u64_or_string")]
    pub count: u64,
    pub hidden: bool,
    pub forbidden: bool,
}

/// 动态评论（`/x/v2/reply`）。
#[derive(Debug, Clone, Deserialize)]
pub struct ReplyResponse {
    pub code: i64,
    #[serde(default)]
    pub message: String,
    pub data: Option<ReplyData>,
}

/// 评论分页（`/x/v2/reply/main`）。
///
/// 旧接口 `/x/v2/reply` 只能稳定翻第 1 页（第 2 页起 `replies` 恒为空），
/// 且 `ps` 最大 20；新版 `main` 走游标翻页，`cursor.is_end` 就是
/// 「真的没有更多评论了」的唯一权威信号。
#[derive(Debug, Clone, Deserialize)]
pub struct ReplyMainResponse {
    pub code: i64,
    #[serde(default)]
    pub message: String,
    pub data: Option<ReplyMainData>,
}

#[derive(Debug, Clone, Deserialize, Default)]
#[serde(default)]
pub struct ReplyMainData {
    #[serde(deserialize_with = "de_vec_or_null")]
    pub replies: Vec<ReplyRaw>,
    /// 置顶评论（UP 主置顶 / 管理员置顶），通常与 `replies` 分开返回。
    #[serde(deserialize_with = "de_vec_or_null")]
    pub top_replies: Vec<ReplyRaw>,
    /// 新版接口的置顶信息：管理员置顶 / UP 主置顶 / 投票置顶各一条。
    pub top: Option<ReplyTopRaw>,
    pub cursor: Option<ReplyCursorRaw>,
}

/// 新版评论区 `data.top`：三种置顶各占一个字段，可能为 null。
#[derive(Debug, Clone, Deserialize, Default)]
#[serde(default)]
pub struct ReplyTopRaw {
    pub admin: Option<ReplyRaw>,
    pub upper: Option<ReplyRaw>,
    pub vote: Option<ReplyRaw>,
}

#[derive(Debug, Clone, Deserialize, Default)]
#[serde(default)]
pub struct ReplyCursorRaw {
    /// 下一页游标（原样回传给 `next`）。
    #[serde(deserialize_with = "de_u64_or_string")]
    pub next: u64,
    /// 是否已经到底。
    pub is_end: bool,
    /// 评论总数。
    #[serde(deserialize_with = "de_u64_or_string")]
    pub all_count: u64,
}

#[derive(Debug, Clone, Deserialize, Default)]
#[serde(default)]
pub struct ReplyData {
    #[serde(deserialize_with = "de_vec_or_null")]
    pub replies: Vec<ReplyRaw>,
    /// 旧接口的置顶评论可能放在这里（新接口放在 `top_replies`）。
    #[serde(deserialize_with = "de_vec_or_null")]
    pub top_replies: Vec<ReplyRaw>,
    /// 旧接口（`/x/v2/reply`）把 UP 主置顶放在 `upper.top`。
    pub upper: Option<ReplyUpperRaw>,
    /// 少数旧响应也会给 `data.top`，结构与新版一致。
    pub top: Option<ReplyTopRaw>,
    pub page: Option<ReplyPageRaw>,
}

/// 旧版评论区里的 UP 主信息；目前只取置顶评论。
#[derive(Debug, Clone, Deserialize, Default)]
#[serde(default)]
pub struct ReplyUpperRaw {
    pub top: Option<ReplyRaw>,
}

#[derive(Debug, Clone, Deserialize, Default)]
#[serde(default)]
pub struct ReplyPageRaw {
    pub count: u64,
}

#[derive(Debug, Clone, Deserialize, Default)]
#[serde(default)]
pub struct ReplyRaw {
    pub rpid: i64,
    #[serde(deserialize_with = "de_u64_or_string")]
    pub like: u64,
    pub ctime: u64,
    pub member: ReplyMemberRaw,
    pub content: ReplyContentRaw,
    pub reply_control: Option<ReplyControlRaw>,
}

#[derive(Debug, Clone, Deserialize, Default)]
#[serde(default)]
pub struct ReplyMemberRaw {
    pub uname: String,
    /// 旧版字段。`/x/v2/reply` 现在多数响应只给 `avatar`，`face` 为空；
    /// 两个字段都接，避免评论区头像整片退化成渐变占位。
    pub face: Option<String>,
    /// 新版字段（与 `face` 同值）。
    pub avatar: Option<String>,
    pub level_info: Option<LevelInfo>,
}

#[derive(Debug, Clone, Deserialize, Default)]
#[serde(default)]
pub struct ReplyContentRaw {
    pub message: String,
    /// 评论里附的图片（`/x/v2/reply/main` 放在 `content.pictures`）。
    #[serde(deserialize_with = "de_vec_or_null")]
    pub pictures: Vec<ReplyPictureRaw>,
}

/// 评论图片。
#[derive(Debug, Clone, Deserialize, Default)]
#[serde(default)]
pub struct ReplyPictureRaw {
    /// 原图地址（通常是 `//i0.hdslb.com/...`）。
    pub img_src: String,
    #[serde(deserialize_with = "de_u64_or_string")]
    pub img_width: u64,
    #[serde(deserialize_with = "de_u64_or_string")]
    pub img_height: u64,
}

#[derive(Debug, Clone, Deserialize, Default)]
#[serde(default)]
pub struct ReplyControlRaw {
    pub time_desc: Option<String>,
    pub location: Option<String>,
}

/// 兼容「数字或数字字符串」的反序列化器。
///
/// B 站接口在同一个字段上会随端/版本在 `123` 与 `"123"` 之间切换；
/// 解析失败时退回 0，不因为一个计数把整页动态打空。
fn de_u64_or_string<'de, D>(deserializer: D) -> Result<u64, D::Error>
where
    D: serde::Deserializer<'de>,
{
    use serde::Deserialize;

    #[derive(Deserialize)]
    #[serde(untagged)]
    enum NumOrStr {
        Num(u64),
        Str(String),
    }

    Ok(match NumOrStr::deserialize(deserializer)? {
        NumOrStr::Num(n) => n,
        NumOrStr::Str(s) => s.trim().parse().unwrap_or(0),
    })
}


/// 兼容「整数或数字字符串」的 i64 反序列化器。
///
/// 历史接口的 `kid` / `max` / `progress` 在不同版本里给过数字与字符串，
/// 与 `de_u64_or_string` 同理；`-1` 是合法值（未开始观看），不能当 0 吞掉。
fn de_i64_or_string<'de, D>(deserializer: D) -> Result<i64, D::Error>
where
    D: serde::Deserializer<'de>,
{
    use serde::Deserialize;

    #[derive(Deserialize)]
    #[serde(untagged)]
    enum NumOrStr {
        Num(i64),
        Str(String),
    }

    Ok(match NumOrStr::deserialize(deserializer)? {
        NumOrStr::Num(n) => n,
        NumOrStr::Str(s) => s.trim().parse().unwrap_or(0),
    })
}

/// 兼容 `counts` 的两种形状：`reply` 给整数，`at` / `like` 给对象。
fn de_counts_or_int<'de, D>(deserializer: D) -> Result<MsgfeedCountsRaw, D::Error>
where
    D: serde::Deserializer<'de>,
{
    use serde::Deserialize;

    #[derive(Deserialize)]
    #[serde(untagged)]
    enum Counts {
        Number(i64),
        Object(MsgfeedCountsRaw),
    }

    Ok(match Counts::deserialize(deserializer)? {
        Counts::Number(n) => MsgfeedCountsRaw {
            reply: n,
            ..MsgfeedCountsRaw::default()
        },
        Counts::Object(counts) => counts,
    })
}

/// 兼容「数组或 null」的反序列化器。
///
/// 动态 / 评论接口在空列表时经常直接给 `null`（例如 `reply.replies`），
/// 而 `#[serde(default)]` 只处理字段缺失，不处理 `null`。这里统一收敛成空 Vec。
fn de_vec_or_null<'de, D, T>(deserializer: D) -> Result<Vec<T>, D::Error>
where
    D: serde::Deserializer<'de>,
    T: serde::Deserialize<'de>,
{
    Ok(Option::<Vec<T>>::deserialize(deserializer)?.unwrap_or_default())
}

// ================= 消息中心（msgfeed / 私信） =================
//
// 四类人际互动：回复我的 / @我的 / 收到的赞 / 私信。
// - 互动走 `/x/msgfeed/*`：`unread` 与 `reply` 字段已核实；
//   `at` / `like` 未被公开文档收录（附录 E · E-25），**按 reply 同构解析、
//   字段缺失就降级**，不写死未核查的字段名。
// - 私信走 `session_svr` / `svr_sync`：字段以设计文档 12.5.4 的核实表为准。

#[derive(Debug, Clone, Deserialize, Default)]
#[serde(default)]
pub struct MsgfeedUnreadResponse {
    pub code: i64,
    pub message: String,
    pub data: Option<MsgfeedUnreadData>,
}

/// `/x/msgfeed/unread` 的 data。
///
/// 只声明 UI 需要的四项：`reply` / `at` / `like` / `recv_like`。
/// `recv_reply = reply + at`，只能用于总数，**不能拿来推分通道数**。
/// 旧接口字段名是 `like`，新接口是 `recv_like`，两个都要读。
#[derive(Debug, Clone, Deserialize, Default)]
#[serde(default)]
pub struct MsgfeedUnreadData {
    #[serde(deserialize_with = "de_i64_or_string")]
    pub reply: i64,
    #[serde(deserialize_with = "de_i64_or_string")]
    pub at: i64,
    #[serde(deserialize_with = "de_i64_or_string")]
    pub like: i64,
    #[serde(deserialize_with = "de_i64_or_string")]
    pub recv_reply: i64,
    #[serde(deserialize_with = "de_i64_or_string")]
    pub recv_like: i64,
}

#[derive(Debug, Clone, Deserialize, Default)]
#[serde(default)]
pub struct MsgfeedInboxResponse {
    pub code: i64,
    pub message: String,
    pub data: Option<MsgfeedInboxData>,
}

#[derive(Debug, Clone, Deserialize, Default)]
#[serde(default)]
pub struct MsgfeedInboxData {
    pub cursor: Option<MsgfeedCursorRaw>,
    #[serde(deserialize_with = "de_vec_or_null")]
    pub items: Vec<MsgfeedItemRaw>,
    /// 顶层已读时间（部分响应放在这里，而不是每条 item 上）。
    #[serde(deserialize_with = "de_i64_or_string")]
    pub last_view_at: i64,
    /// 「收到的赞」的实际响应形状：`latest` + `total`，而不是平铺的 items。
    /// 兼容两套形状，避免接口一变整页失败。
    pub latest: Option<MsgfeedLatestRaw>,
    pub total: Option<MsgfeedLikeTotalRaw>,
}

#[derive(Debug, Clone, Deserialize, Default)]
#[serde(default)]
pub struct MsgfeedLatestRaw {
    #[serde(deserialize_with = "de_vec_or_null")]
    pub items: Vec<MsgfeedItemRaw>,
    #[serde(deserialize_with = "de_i64_or_string")]
    pub last_view_at: i64,
}

#[derive(Debug, Clone, Deserialize, Default)]
#[serde(default)]
pub struct MsgfeedLikeTotalRaw {
    pub cursor: Option<MsgfeedCursorRaw>,
    #[serde(deserialize_with = "de_vec_or_null")]
    pub items: Vec<MsgfeedItemRaw>,
}

#[derive(Debug, Clone, Deserialize, Default)]
#[serde(default)]
pub struct MsgfeedCursorRaw {
    #[serde(deserialize_with = "de_i64_or_string")]
    pub id: i64,
    #[serde(deserialize_with = "de_i64_or_string")]
    pub time: i64,
    pub is_end: bool,
}

/// 一条互动消息。`item` / `user` 都可能整体缺失，全部给默认值。
#[derive(Debug, Clone, Deserialize, Default)]
#[serde(default)]
pub struct MsgfeedItemRaw {
    #[serde(deserialize_with = "de_i64_or_string")]
    pub id: i64,
    pub user: MsgfeedUserRaw,
    /// 赞列表给的是 `users`（一行聚合多人），不是单个 `user`。
    #[serde(deserialize_with = "de_vec_or_null")]
    pub users: Vec<MsgfeedUserRaw>,
    pub item: MsgfeedBodyRaw,
    /// `reply` 的 `counts` 是整数；`at` / `like` 里也可能是对象，两种都接。
    #[serde(default, deserialize_with = "de_counts_or_int")]
    pub counts: MsgfeedCountsRaw,
    #[serde(deserialize_with = "de_i64_or_string")]
    pub is_multi: i64,
    /// 回复 / 点赞 / @ 时间。三个接口用的键名不同：
    /// `reply_time` / `like_time` / `at_time`。
    #[serde(
        default,
        alias = "like_time",
        alias = "at_time",
        deserialize_with = "de_i64_or_string"
    )]
    pub reply_time: i64,
    #[serde(deserialize_with = "de_i64_or_string")]
    pub last_view_at: i64,
}

#[derive(Debug, Clone, Deserialize, Default)]
#[serde(default)]
pub struct MsgfeedUserRaw {
    #[serde(deserialize_with = "de_i64_or_string")]
    pub mid: i64,
    pub nickname: String,
    pub avatar: String,
}

#[derive(Debug, Clone, Deserialize, Default)]
#[serde(default)]
pub struct MsgfeedBodyRaw {
    #[serde(rename = "type")]
    pub kind: String,
    pub business: String,
    #[serde(deserialize_with = "de_i64_or_string")]
    pub business_id: i64,
    /// 赞列表的真实主键在被赞内容上，`subject_id` 可能为 0。
    #[serde(deserialize_with = "de_i64_or_string")]
    pub item_id: i64,
    #[serde(deserialize_with = "de_i64_or_string")]
    pub subject_id: i64,
    #[serde(deserialize_with = "de_i64_or_string")]
    pub root_id: i64,
    #[serde(deserialize_with = "de_i64_or_string")]
    pub source_id: i64,
    #[serde(deserialize_with = "de_i64_or_string")]
    pub target_id: i64,
    pub title: String,
    pub uri: String,
    pub source_content: String,
    pub target_reply_content: String,
    pub root_reply_content: String,
}

#[derive(Debug, Clone, Deserialize, Default)]
#[serde(default)]
pub struct MsgfeedCountsRaw {
    #[serde(deserialize_with = "de_i64_or_string")]
    pub up: i64,
    #[serde(deserialize_with = "de_i64_or_string")]
    pub reply: i64,
    #[serde(deserialize_with = "de_i64_or_string")]
    pub is_up: i64,
}

/// 私信会话列表。会话本身的昵称 / 头像不在响应里，要再用 user cards 补。
#[derive(Debug, Clone, Deserialize, Default)]
#[serde(default)]
pub struct SessionListResponse {
    pub code: i64,
    pub message: String,
    pub data: Option<SessionListData>,
}

#[derive(Debug, Clone, Deserialize, Default)]
#[serde(default)]
pub struct SessionListData {
    #[serde(deserialize_with = "de_vec_or_null")]
    pub session_list: Vec<SessionRaw>,
    pub has_more: i64,
}

#[derive(Debug, Clone, Deserialize, Default)]
#[serde(default)]
pub struct SessionRaw {
    #[serde(deserialize_with = "de_i64_or_string")]
    pub talker_id: i64,
    #[serde(deserialize_with = "de_i64_or_string")]
    pub session_type: i64,
    #[serde(deserialize_with = "de_i64_or_string")]
    pub unread_count: i64,
    /// 微秒级时间戳（**不是秒**）。
    #[serde(deserialize_with = "de_i64_or_string")]
    pub session_ts: i64,
    #[serde(deserialize_with = "de_i64_or_string")]
    pub is_follow: i64,
    #[serde(deserialize_with = "de_i64_or_string")]
    pub can_fold: i64,
    /// 非 0 表示系统通知会话（登录 / 收益 / 预约…），不进消息中心。
    #[serde(deserialize_with = "de_i64_or_string")]
    pub system_msg_type: i64,
    #[serde(deserialize_with = "de_i64_or_string")]
    pub is_intercept: i64,
    #[serde(deserialize_with = "de_i64_or_string")]
    pub top_ts: i64,
    pub last_msg: Option<SessionMsgRaw>,
}

/// 一条私信消息（会话列表的最后一条与聊天记录共用）。
#[derive(Debug, Clone, Deserialize, Default)]
#[serde(default)]
pub struct SessionMsgRaw {
    #[serde(deserialize_with = "de_i64_or_string")]
    pub sender_uid: i64,
    #[serde(deserialize_with = "de_i64_or_string")]
    pub receiver_id: i64,
    #[serde(deserialize_with = "de_i64_or_string")]
    pub receiver_type: i64,
    #[serde(deserialize_with = "de_i64_or_string")]
    pub msg_type: i64,
    #[serde(deserialize_with = "de_i64_or_string")]
    pub msg_status: i64,
    #[serde(deserialize_with = "de_i64_or_string")]
    pub msg_source: i64,
    /// JSON 字符串，例如 `{"content":"hi"}`。
    pub content: String,
    #[serde(deserialize_with = "de_i64_or_string")]
    pub msg_seqno: i64,
    #[serde(deserialize_with = "de_i64_or_string")]
    pub timestamp: i64,
}

#[derive(Debug, Clone, Deserialize, Default)]
#[serde(default)]
pub struct SessionMsgsResponse {
    pub code: i64,
    pub message: String,
    pub data: Option<SessionMsgsData>,
}

#[derive(Debug, Clone, Deserialize, Default)]
#[serde(default)]
pub struct SessionMsgsData {
    #[serde(deserialize_with = "de_vec_or_null")]
    pub messages: Vec<SessionMsgRaw>,
    #[serde(deserialize_with = "de_i64_or_string")]
    pub has_more: i64,
    #[serde(deserialize_with = "de_i64_or_string")]
    pub min_seqno: i64,
    #[serde(deserialize_with = "de_i64_or_string")]
    pub max_seqno: i64,
}

/// 用户名片（补私信会话的昵称与头像）。
#[derive(Debug, Clone, Deserialize, Default)]
#[serde(default)]
pub struct UserCardsResponse {
    pub code: i64,
    pub message: String,
    /// key 是 mid 的字符串形式，值里再带 `mid`；用 serde_json::Value 兜住。
    pub data: Option<serde_json::Value>,
}

/// 互关好友列表（`/x/relation/friends`）。
#[derive(Debug, Clone, Deserialize, Default)]
#[serde(default)]
pub struct RelationFriendsResponse {
    pub code: i64,
    pub message: String,
    pub data: Option<RelationFriendsData>,
}

#[derive(Debug, Clone, Deserialize, Default)]
#[serde(default)]
pub struct RelationFriendsData {
    #[serde(deserialize_with = "de_vec_or_null")]
    pub list: Vec<RelationFriendRaw>,
}

#[derive(Debug, Clone, Deserialize, Default)]
#[serde(default)]
pub struct RelationFriendRaw {
    #[serde(deserialize_with = "de_i64_or_string")]
    pub mid: i64,
    pub uname: String,
    pub face: String,
}

#[derive(Debug, Clone, Deserialize, Default)]
#[serde(default)]
pub struct RelationResponse {
    pub code: i64,
    pub message: String,
    pub data: Option<RelationData>,
}

#[derive(Debug, Clone, Deserialize, Default)]
#[serde(default)]
pub struct RelationData {
    #[serde(deserialize_with = "de_i64_or_string")]
    pub mid: i64,
    /// 6 = 已互粉，128 = 已拉黑。
    #[serde(deserialize_with = "de_i64_or_string")]
    pub attribute: i64,
}

/// 关注 / 粉丝数（`/x/relation/stat?vmid=`）。
#[derive(Debug, Clone, Deserialize, Default)]
#[serde(default)]
pub struct RelationStatResponse {
    pub code: i64,
    pub message: String,
    pub data: Option<RelationStatData>,
}

#[derive(Debug, Clone, Deserialize, Default)]
#[serde(default)]
pub struct RelationStatData {
    #[serde(deserialize_with = "de_i64_or_string")]
    pub mid: i64,
    /// 该用户关注了多少人。
    #[serde(deserialize_with = "de_i64_or_string")]
    pub following: i64,
    /// 该用户的粉丝数。
    #[serde(deserialize_with = "de_i64_or_string")]
    pub follower: i64,
}

impl DashStream {
    /// 用哪条 CDN 地址。
    ///
    /// **优先 upos 镜像**：B 站 playurl 的 `baseUrl` 常给 `*.mcdn.bilivideo.cn`
    /// 边缘节点，实测会出现 HEAD 404、GET 只返回几百 KB（远小于完整 m4s），
    /// ffmpeg 就会报 `Stream ends prematurely at 0, should be N`。
    /// `backupUrl` 里的 `upos-sz-mirror*` 返回完整文件，优先使用。
    pub fn play_url(&self) -> &str {
        if self.base_url.contains("upos") {
            return &self.base_url;
        }
        if let Some(url) = self.backup_urls.iter().find(|u| u.contains("upos")) {
            return url;
        }
        if !self.base_url.is_empty() && !self.base_url.contains("mcdn") {
            return &self.base_url;
        }
        if !self.fallback_url.is_empty() {
            return &self.fallback_url;
        }
        if let Some(url) = self.backup_urls.first() {
            return url;
        }
        &self.base_url
    }

    /// 同 id 下优先 AVC：兼容性最好，mpv 的硬解路径也最稳。
    pub fn codec_rank(&self) -> u8 {
        if self.codecs.starts_with("avc1") {
            2
        } else if self.codecs.starts_with("hvc1") || self.codecs.starts_with("hev1") {
            1
        } else {
            0
        }
    }
}
