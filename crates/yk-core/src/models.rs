//! 应用层数据模型。
//!
//! 对应 `YKPili/Models/{CoverItem,LoginState,UserSession}.cs`。
//!
//! 与旧实现最大的差别：**卡片模型是完整的**。
//! 旧 `CoverItem` 只有 Index / Title / PlaceholderColor / ImageUrl / Bvid / Cid，
//! 而接口明明返回了 UP 主与统计数据，映射时被丢掉，导致卡片画不出
//! 「UP 主 · 播放量 · 弹幕数 · 时长」这一行。这里一次性补齐，
//! 并由单测守着「映射不丢字段」。

use crate::api::models::RecommendVideoItem;
use crate::format;

/// 封面缩略图目标宽度（2× 屏 384，设计文档封面解码约束）。
///
/// 卡片只占 192px，没必要下载/解码原图；B 站图床支持在 URL 后加
/// `@384w_216h_1c.webp` 直接取缩略图。
pub const COVER_THUMB_WIDTH: u32 = 384;
/// 16:9 缩略图高度。
pub const COVER_THUMB_HEIGHT: u32 = 216;

/// 把 B 站封面 URL 换成 384×216 的缩略图地址；非 B 站 / 已带后缀的 URL 原样返回。
pub fn cover_thumb_url(url: &str) -> String {
    if url.is_empty() || url.contains('@') || !url.contains("hdslb.com") || url.contains('?') {
        return url.to_string();
    }
    format!("{url}@{COVER_THUMB_WIDTH}w_{COVER_THUMB_HEIGHT}h_1c.webp")
}

/// 首页推荐视频的封面卡片数据。
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct CoverItem {
    /// 在容器池中的索引（用于回收复用）。
    pub index: usize,
    /// 视频标题（HTML 已解码）。
    pub title: String,
    /// 占位色（无封面图时的底色，`#RRGGBB` 或带 alpha）。
    pub placeholder_color: String,
    /// 封面图远程地址。
    pub image_url: String,
    /// B 站视频 ID。
    pub bvid: String,
    /// 分页 CID（可能在请求详情后回填）。
    pub cid: i64,

    // ---- 以下为 M1 补齐的卡片信息层字段 ----
    /// UP 主昵称。
    pub up_name: String,
    /// UP 主头像 URL。
    pub up_face: String,
    /// 播放量（原始值，显示时走 `format::count`）。
    pub view_count: u64,
    /// 弹幕数（原始值）。
    pub danmaku_count: u64,
    /// 时长（秒，原始值，显示时走 `format::duration`）。
    pub duration_secs: u64,
    /// 发布时间（Unix 秒，显示时走 `format::relative_time`）。
    pub pubdate: u64,
    /// 是否为直播卡（推荐接口 `goto == "live"`）。
    pub is_live: bool,
    /// 本地播放进度（秒）。0 表示未看过。
    pub progress_secs: u64,
}

impl Default for CoverItem {
    fn default() -> Self {
        Self {
            index: 0,
            title: String::new(),
            placeholder_color: "#555555".into(),
            image_url: String::new(),
            bvid: String::new(),
            cid: 0,
            up_name: String::new(),
            up_face: String::new(),
            view_count: 0,
            danmaku_count: 0,
            duration_secs: 0,
            pubdate: 0,
            is_live: false,
            progress_secs: 0,
        }
    }
}

impl CoverItem {
    /// 从推荐接口条目构造卡片。
    ///
    /// **每个字段都要带过来** —— 这是 M1 的验收点：
    /// `owner.name` / `stat.view` / `stat.danmaku` / `duration` 一个都不能丢。
    pub fn from_recommend(v: &RecommendVideoItem, index: usize, placeholder: &str) -> Self {
        Self {
            index,
            title: format::html_decode(&v.title),
            placeholder_color: placeholder.to_string(),
            image_url: v.pic.clone(),
            bvid: v.bvid.clone(),
            cid: 0,
            up_name: v.owner.as_ref().map(|o| o.name.clone()).unwrap_or_default(),
            up_face: v.owner.as_ref().map(|o| o.face.clone()).unwrap_or_default(),
            view_count: v.stat.as_ref().map(|s| s.view).unwrap_or(0),
            danmaku_count: v.stat.as_ref().map(|s| s.danmaku).unwrap_or(0),
            duration_secs: v.duration,
            pubdate: v.pubdate,
            is_live: v.goto == "live",
            progress_secs: 0,
        }
    }

    /// 克隆到一个新的池索引（列数变化重建容器池时用）。
    ///
    /// 旧实现 `CloneTo` 只复制 4 个字段，注释里还专门警告「漏掉图址与 BVID
    /// 会整屏空占位」—— 这里直接把整份数据带过去，不再逐字段手抄。
    pub fn clone_to(&self, index: usize) -> Self {
        Self {
            index,
            ..self.clone()
        }
    }

    /// 空位（透明占位）。
    pub fn empty(index: usize) -> Self {
        Self {
            index,
            placeholder_color: "#00000000".into(),
            ..Self::default()
        }
    }

    /// 卡片第二行文案：「UP 主 · 12.4万 · 144弹幕」的信息片段。
    ///
    /// 文案口径见设计文档第 04 节与附录 C.1：
    /// 单位由文案承担，计数本身不带「播放」二字。
    pub fn meta_line(&self) -> String {
        let mut parts = Vec::with_capacity(3);
        if !self.up_name.is_empty() {
            parts.push(self.up_name.clone());
        }
        parts.push(format::count(self.view_count));
        if self.danmaku_count > 0 {
            parts.push(format!("{}弹幕", format::count(self.danmaku_count)));
        }
        parts.join(" · ")
    }

    /// 时长显示形态。
    pub fn duration_text(&self) -> String {
        format::duration(self.duration_secs)
    }

    /// 署名行右侧的相对时间；发布时间未知时返回空串（由视图决定是否显示）。
    pub fn published_text(&self) -> String {
        format::relative_time(self.pubdate)
    }

    /// 卡片实际加载的封面地址：B 站图床走 384×216 缩略图，平衡内存与清晰度。
    pub fn cover_thumb_url(&self) -> String {
        cover_thumb_url(&self.image_url)
    }
}

// ================= 登录状态 =================

/// 登录流程状态机。
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum LoginState {
    #[default]
    Idle,
    QrCodeLoaded,
    QrCodeScanned,
    QrCodeExpired,
    LoginSuccess,
    LoginFailed,
}

// ================= 用户会话 =================

/// 用户登录会话（持久化到本地，下次启动免登录恢复）。
///
/// 序列化键沿用 C# 版的 **PascalCase**（`DedeUserId` / `SessData` …），
/// 这样旧版本写下的 `session.json` 能直接被读出来，不用做迁移。
#[derive(Debug, Clone, Default, PartialEq, serde::Serialize, serde::Deserialize)]
#[serde(rename_all = "PascalCase", default)]
pub struct UserSession {
    /// B 站用户 ID（`DedeUserID`）。
    pub dede_user_id: i64,
    /// `SESSDATA`：登录态核心 Cookie。
    pub sess_data: String,
    /// `bili_jct`：CSRF Token。
    pub bili_jct: String,
    pub nickname: String,
    pub face_url: String,
    pub is_logged_in: bool,
    /// WBI 签名用 img_key。
    pub img_key: Option<String>,
    /// WBI 签名用 sub_key。
    pub sub_key: Option<String>,

    // ---- 账号面板缓存（避免每次点头像都重新请求） ----
    #[serde(default)]
    pub coins: f64,
    #[serde(default)]
    pub bcoin: f64,
    #[serde(default)]
    pub level: u8,
    #[serde(default)]
    pub following: u64,
    #[serde(default)]
    pub followers: u64,
    #[serde(default)]
    pub dynamics: Option<u64>,
    #[serde(default)]
    pub last_login_time: String,
    #[serde(default)]
    pub last_login_geo: String,
    /// 账号面板数据的写入时间（Unix 秒）；0 = 从未拉取过。
    #[serde(default)]
    pub account_updated_at: u64,
}

impl UserSession {
    /// 序列化为标准 Cookie 请求头：`SESSDATA=xxx; bili_jct=xxx; DedeUserID=xxx`。
    ///
    /// 空值不写入；`DedeUserID` 仅在大等于 0 时不写（与旧实现 `> 0` 一致）。
    pub fn cookie_header(&self) -> String {
        let mut parts = Vec::with_capacity(3);
        if !self.sess_data.is_empty() {
            parts.push(format!("SESSDATA={}", self.sess_data));
        }
        if !self.bili_jct.is_empty() {
            parts.push(format!("bili_jct={}", self.bili_jct));
        }
        if self.dede_user_id > 0 {
            parts.push(format!("DedeUserID={}", self.dede_user_id));
        }
        parts.join("; ")
    }

    pub fn is_valid(&self) -> bool {
        self.is_logged_in && !self.sess_data.is_empty()
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::api::models::{VideoOwner, VideoStat};

    fn sample_item() -> RecommendVideoItem {
        RecommendVideoItem {
            id: 114514,
            bvid: "BV1xx411c7mD".into(),
            title: "生肉烹饪师 &amp; 他的猫".into(),
            pic: "https://i0.hdslb.com/a.jpg".into(),
            desc: "简介".into(),
            duration: 754,
            pubdate: 1_700_000_000,
            goto: "av".into(),
            owner: Some(VideoOwner {
                name: "生肉烹饪师".into(),
                face: "https://i1.hdslb.com/f.jpg".into(),
            }),
            stat: Some(VideoStat {
                view: 124_000,
                danmaku: 144,
            }),
        }
    }

    /// **M1 的核心验收**：映射不能丢字段。
    #[test]
    fn from_recommend_keeps_every_field() {
        let v = sample_item();
        let c = CoverItem::from_recommend(&v, 7, "#E57373");

        assert_eq!(c.index, 7);
        assert_eq!(c.title, "生肉烹饪师 & 他的猫", "标题应已解码 HTML 实体");
        assert_eq!(c.placeholder_color, "#E57373");
        assert_eq!(c.image_url, "https://i0.hdslb.com/a.jpg");
        assert_eq!(c.bvid, "BV1xx411c7mD");

        // 旧实现丢掉的四个字段
        assert_eq!(c.up_name, "生肉烹饪师", "UP 主被丢了");
        assert_eq!(c.up_face, "https://i1.hdslb.com/f.jpg", "UP 主头像被丢了");
        assert_eq!(c.view_count, 124_000, "播放量被丢了");
        assert_eq!(c.danmaku_count, 144, "弹幕数被丢了");
        assert_eq!(c.duration_secs, 754, "时长被丢了");
        assert_eq!(c.pubdate, 1_700_000_000, "发布时间被丢了");
        assert!(!c.is_live, "普通视频不应带直播标记");
    }

    #[test]
    fn live_flag_comes_from_goto() {
        let mut v = sample_item();
        v.goto = "live".into();
        let c = CoverItem::from_recommend(&v, 0, "#000");
        assert!(c.is_live, "goto=live 必须映射成直播卡");
    }

    #[test]
    fn cover_thumb_url_uses_low_res_variant() {
        assert_eq!(
            cover_thumb_url("https://i0.hdslb.com/bfs/archive/a.jpg"),
            "https://i0.hdslb.com/bfs/archive/a.jpg@384w_216h_1c.webp"
        );
        // 已带缩放后缀 / 非 B 站 / 空地址都不重复追加
        let already = "https://i0.hdslb.com/bfs/archive/a.jpg@100w.webp";
        assert_eq!(cover_thumb_url(already), already);
        let other = "https://example.com/a.jpg";
        assert_eq!(cover_thumb_url(other), other);
        assert_eq!(cover_thumb_url(""), "");
    }

    #[test]
    fn missing_owner_and_stat_degrade_to_empty() {
        let mut v = sample_item();
        v.owner = None;
        v.stat = None;
        let c = CoverItem::from_recommend(&v, 0, "#000");
        assert_eq!(c.up_name, "");
        assert_eq!(c.view_count, 0);
        assert_eq!(c.danmaku_count, 0);
    }

    #[test]
    fn clone_to_keeps_all_fields() {
        let c = CoverItem::from_recommend(&sample_item(), 3, "#E57373");
        let cloned = c.clone_to(9);
        assert_eq!(cloned.index, 9);
        assert_eq!(cloned.up_name, c.up_name);
        assert_eq!(cloned.view_count, c.view_count);
        assert_eq!(cloned.duration_secs, c.duration_secs);
        assert_eq!(cloned.pubdate, c.pubdate);
        assert_eq!(cloned.image_url, c.image_url);
        assert_eq!(cloned.bvid, c.bvid);
    }

    #[test]
    fn meta_line_shape() {
        let c = CoverItem::from_recommend(&sample_item(), 0, "#000");
        assert_eq!(c.meta_line(), "生肉烹饪师 · 12.4万 · 144弹幕");
        assert_eq!(c.duration_text(), "12:34");
    }

    #[test]
    fn empty_uses_transparent_placeholder() {
        let e = CoverItem::empty(5);
        assert_eq!(e.index, 5);
        assert_eq!(e.placeholder_color, "#00000000");
        assert!(e.title.is_empty());
    }

    // ================= 会话 =================

    #[test]
    fn cookie_header_omits_empty_parts() {
        let s = UserSession::default();
        assert_eq!(s.cookie_header(), "");

        let s = UserSession {
            sess_data: "abc".into(),
            bili_jct: "def".into(),
            dede_user_id: 42,
            is_logged_in: true,
            ..Default::default()
        };
        assert_eq!(
            s.cookie_header(),
            "SESSDATA=abc; bili_jct=def; DedeUserID=42"
        );
        assert!(s.is_valid());
    }

    #[test]
    fn cookie_header_skips_zero_uid() {
        let s = UserSession {
            sess_data: "x".into(),
            ..Default::default()
        };
        assert_eq!(s.cookie_header(), "SESSDATA=x");
    }
}
