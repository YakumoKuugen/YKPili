//! B 站接口的响应模型。
//!
//! 对应 `YKPili/Services/Bilibili/Models/*.cs`。
//! 字段名与接口一致（snake_case），不额外做驼峰转换 —— 少一层映射就少一处出错。

use serde::Deserialize;

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
#[derive(Debug, Clone, Deserialize, Default)]
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
    #[serde(default)]
    pub owner: Option<VideoOwner>,
    #[serde(default)]
    pub stat: Option<VideoStat>,
}

#[derive(Debug, Clone, Deserialize, Default)]
pub struct VideoOwner {
    #[serde(default)]
    pub name: String,
    #[serde(default)]
    pub face: String,
}

#[derive(Debug, Clone, Deserialize, Default)]
pub struct VideoStat {
    #[serde(default)]
    pub view: u64,
    #[serde(default)]
    pub danmaku: u64,
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
    #[serde(default)]
    pub pages: Vec<VideoPage>,
}

#[derive(Debug, Clone, Deserialize, Default)]
pub struct VideoPage {
    #[serde(default)]
    pub cid: i64,
    #[serde(default)]
    pub page: i32,
    #[serde(default)]
    pub part: String,
}

// ================= 播放地址 =================

#[derive(Debug, Clone, Deserialize)]
pub struct VideoPlayUrlResponse {
    pub code: i64,
    pub data: Option<PlayUrlData>,
}

#[derive(Debug, Clone, Deserialize, Default)]
pub struct PlayUrlData {
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
    #[serde(default)]
    pub wbi_img: Option<WbiImg>,
}

#[derive(Debug, Clone, Deserialize, Default)]
pub struct WbiImg {
    #[serde(default)]
    pub img_url: String,
    #[serde(default)]
    pub sub_url: String,
}
