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
    #[serde(default)]
    pub wbi_img: Option<WbiImg>,
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

#[derive(Debug, Clone, Deserialize, Default)]
#[serde(default)]
pub struct ReplyData {
    #[serde(deserialize_with = "de_vec_or_null")]
    pub replies: Vec<ReplyRaw>,
    pub page: Option<ReplyPageRaw>,
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
    pub face: Option<String>,
    pub level_info: Option<LevelInfo>,
}

#[derive(Debug, Clone, Deserialize, Default)]
#[serde(default)]
pub struct ReplyContentRaw {
    pub message: String,
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
