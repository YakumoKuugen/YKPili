//! 关注动态（B 站「动态」流）的数据模型与解析。
//!
//! 这一层只做两件事：
//! 1. 把 `/x/polymer/web-dynamic/v1/feed/all` 的**模块化**响应收敛成 UI 直接可用的
//!    `DynamicItem`（作者 / 正文 / 图片 / 视频 / 统计 / 评论目标）；
//! 2. 保留瀑布流需要的稳定字段（id、图片宽高、发布时间），让视图能**先算高度再排版**。
//!
//! B 站动态的 JSON 有几个长期存在的不一致，解析时必须同时兼容：
//! - `module_author.pub_ts` 有时是字符串、有时是数字；
//! - `module_stat.*.count` / `major.archive.stat.*` 同样是「字符串或数字」；
//! - 图片动态既可能走 `major.draw.items`（旧版），也可能走 `major.opus.pics`（Opus 版）。
//!
//! 这里**不依赖 gpui**，所有解析都有单测，UI 之外也能复用。

use crate::api::models::{DynamicItemRaw, DynamicModules, ReplyRaw};
use crate::feed::PLACEHOLDER_PALETTE;
use crate::format;

/// 动态的展示类型。只区分 UI 真正画法不同的几类，不照搬接口的全部枚举。
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum DynamicKind {
    /// 视频投稿：点击直接打开播放器。
    Video,
    /// 图文 / 纯图：打开详情浮层。
    Images,
    /// 纯文字：打开详情浮层。
    Text,
    /// 转发：卡片上带一段原文引用。
    Forward,
    /// 文章 / 直播 / 专栏等链接类内容，按图文卡兜底展示。
    Link,
}

impl DynamicKind {
    pub fn label(self) -> &'static str {
        match self {
            Self::Video => "视频",
            Self::Images => "图文",
            Self::Text => "文字",
            Self::Forward => "转发",
            Self::Link => "链接",
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Default)]
pub struct DynamicAuthor {
    pub mid: i64,
    pub name: String,
    pub face: String,
}

#[derive(Debug, Clone, PartialEq, Eq, Default)]
pub struct DynamicImage {
    pub url: String,
    /// 原图宽高。接口偶尔给 0，视图侧会退回 4:3。
    pub width: u32,
    pub height: u32,
}

impl DynamicImage {
    /// 卡面缩略图：限制宽度，避免为 400px 的卡片解码 1448px 的原图。
    pub fn thumb_url(&self, width: u32) -> String {
        asset_url(&self.url, width)
    }

    /// 按实际显示尺寸取图：显示宽 × 屏幕缩放比，再向上对齐到 16px。
    ///
    /// `display_px` 是逻辑像素（卡片/格子宽），Retina 上实际需要 2 倍物理像素。
    /// 结果同时受原图宽度约束，小图不会被强行放大。
    pub fn fit_url(&self, display_px: f32, scale_factor: f32) -> String {
        let width = fitted_width(display_px, scale_factor, self.width);
        asset_url(&self.url, width)
    }

    /// 详情大图：限制宽度但保留比例。
    pub fn detail_url(&self) -> String {
        asset_url(&self.url, 1280)
    }

    /// 宽高比（宽 / 高）；数据缺失时按 4:3 处理。
    pub fn aspect(&self) -> f32 {
        if self.width > 0 && self.height > 0 {
            self.width as f32 / self.height as f32
        } else {
            4.0 / 3.0
        }
    }
}

/// 把 B 站图床地址补成 https，并追加缩放后缀。
///
/// 动态图床（`i0.hdslb.com/bfs/new_dyn/...`）支持 `@480w.webp` 这类后缀；
/// 已经带 `@` 或是外链的地址原样返回。
/// 按显示尺寸计算缩略图宽度。
///
/// 规则：
/// - `display_px × 屏幕缩放比` 是实际物理像素；
/// - 向上取整到 16px，减少窗口尺寸变化时的 URL 碎片；
/// - 不超过原图宽度（接口给了宽高时），避免小图被放大；
/// - 最终夹在 64～1280 之间，防止异常窗口尺寸请求超大图。
pub fn fitted_width(display_px: f32, scale_factor: f32, original_width: u32) -> u32 {
    let desired = (display_px.max(1.0) * scale_factor.clamp(1.0, 2.0)).ceil() as u32;
    let target = if original_width > 0 {
        desired.min(original_width)
    } else {
        desired
    };
    // 原图比目标还小：直接用原图宽度，不做放大。
    if original_width > 0 && target >= original_width {
        return original_width;
    }
    let aligned = target.div_ceil(16) * 16;
    aligned.clamp(64, 1280)
}

pub fn asset_url(url: &str, width: u32) -> String {
    let url = url.trim();
    if url.is_empty() {
        return String::new();
    }
    let mut out = if let Some(rest) = url.strip_prefix("http://") {
        format!("https://{rest}")
    } else {
        url.to_string()
    };
    if out.contains('@') || out.contains('?') || !out.contains("hdslb.com") {
        return out;
    }
    out.push_str(&format!("@{width}w.webp"));
    out
}

#[derive(Debug, Clone, PartialEq, Eq, Default)]
pub struct DynamicVideo {
    pub bvid: String,
    pub title: String,
    pub cover: String,
    pub desc: String,
    pub duration_text: String,
    pub play_count: u64,
    pub danmaku_count: u64,
}

impl DynamicVideo {
    /// 视频封面按显示尺寸取图；卡片上是缩略图，播放器入场时可用更大一档。
    pub fn cover_url(&self, display_px: f32, scale_factor: f32) -> String {
        asset_url(&self.cover, fitted_width(display_px, scale_factor, 0))
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Default)]
pub struct DynamicStats {
    pub forward: u64,
    pub comment: u64,
    pub like: u64,
}

/// 评论接口需要的目标（`type` + `oid`）。
#[derive(Debug, Clone, PartialEq, Eq, Default)]
pub struct CommentTarget {
    pub oid: String,
    pub kind: i64,
}

#[derive(Debug, Clone, PartialEq, Eq, Default)]
pub struct DynamicLink {
    pub title: String,
    pub desc: String,
    pub cover: String,
}

impl DynamicLink {
    pub fn cover_url(&self, display_px: f32, scale_factor: f32) -> String {
        asset_url(&self.cover, fitted_width(display_px, scale_factor, 0))
    }
}

/// 一条动态（已从接口模型收敛）。
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct DynamicItem {
    /// 列表中的序号，用于瀑布流的事件 id。
    pub index: usize,
    pub id: String,
    pub kind: DynamicKind,
    pub author: DynamicAuthor,
    pub pub_ts: u64,
    /// 接口自带的相对时间文案（「21分钟前」）；0 时间戳时用它兜底。
    pub pub_time: String,
    /// 「投稿了视频」「发布了动态」等动作文案。
    pub action: String,
    pub title: String,
    pub text: String,
    pub images: Vec<DynamicImage>,
    pub video: Option<DynamicVideo>,
    pub link: Option<DynamicLink>,
    pub stats: DynamicStats,
    pub comment_target: Option<CommentTarget>,
    /// 转发动态的原动态。
    pub origin: Option<Box<DynamicItem>>,
    pub placeholder_color: String,
}

impl Default for DynamicItem {
    fn default() -> Self {
        Self {
            index: 0,
            id: String::new(),
            kind: DynamicKind::Text,
            author: DynamicAuthor::default(),
            pub_ts: 0,
            pub_time: String::new(),
            action: String::new(),
            title: String::new(),
            text: String::new(),
            images: Vec::new(),
            video: None,
            link: None,
            stats: DynamicStats::default(),
            comment_target: None,
            origin: None,
            placeholder_color: PLACEHOLDER_PALETTE[0].to_string(),
        }
    }
}

impl DynamicItem {
    /// UI 是否需要展示这条动态。
    ///
    /// 动态流里会混进已删除 / 仅粉丝可见 / 纯直播推荐等条目；没有正文、没有图、
    /// 没有视频的条目直接丢掉，避免瀑布流里出现空白卡片。
    pub fn is_renderable(&self) -> bool {
        !self.text.trim().is_empty()
            || !self.title.trim().is_empty()
            || !self.images.is_empty()
            || self.video.is_some()
            || self.link.is_some()
            || (self.kind == DynamicKind::Forward && self.origin.is_some())
    }

    /// 头部第二行：`21分钟前 · 投稿了视频`。
    pub fn meta_line(&self) -> String {
        let time = self.published_text();
        if self.action.is_empty() {
            time
        } else if time.is_empty() {
            self.action.clone()
        } else {
            format!("{time} · {}", self.action)
        }
    }

    pub fn published_text(&self) -> String {
        if !self.pub_time.trim().is_empty() {
            self.pub_time.trim().to_string()
        } else {
            format::relative_time(self.pub_ts)
        }
    }

    pub fn author_name(&self) -> &str {
        if self.author.name.is_empty() {
            "未知 UP 主"
        } else {
            &self.author.name
        }
    }

    pub fn first_image(&self) -> Option<&DynamicImage> {
        self.images.first()
    }

    /// 作者头像按实际显示尺寸取图（卡片 36px / 详情 40px）。
    pub fn author_face_url(&self, display_px: f32, scale_factor: f32) -> String {
        asset_url(&self.author.face, fitted_width(display_px, scale_factor, 0))
    }

    /// 从接口原始条目构造；不支持的条目返回 `None`。
    pub fn from_raw(raw: &DynamicItemRaw, index: usize) -> Option<Self> {
        let author = raw
            .modules
            .module_author
            .as_ref()
            .map(|a| DynamicAuthor {
                mid: a.mid,
                name: format::html_decode(a.name.trim()),
                face: normalize_image_host(&a.face, 0),
            })
            .unwrap_or_default();

        let dynamic = raw.modules.module_dynamic.as_ref();
        let desc_text = dynamic
            .and_then(|d| d.desc.as_ref())
            .map(rich_text)
            .unwrap_or_default();
        let major = dynamic.and_then(|d| d.major.as_ref());

        let mut title = String::new();
        let mut text = desc_text;
        let mut images = Vec::new();
        let mut video = None;
        let mut link = None;

        if let Some(major) = major {
            if let Some(archive) = major.archive.as_ref() {
                let cover = normalize_image_host(&archive.cover, 0);
                video = Some(DynamicVideo {
                    bvid: archive.bvid.clone(),
                    title: format::html_decode(archive.title.trim()),
                    cover,
                    desc: format::html_decode(archive.desc.trim()),
                    duration_text: archive.duration_text.clone(),
                    play_count: archive.stat.as_ref().map(|s| s.play).unwrap_or_default(),
                    danmaku_count: archive.stat.as_ref().map(|s| s.danmaku).unwrap_or_default(),
                });
            }
            if let Some(opus) = major.opus.as_ref() {
                title = format::html_decode(opus.title.trim());
                if text.is_empty() {
                    text = opus.summary.as_ref().map(rich_text).unwrap_or_default();
                }
                images = opus
                    .pics
                    .iter()
                    .map(|p| DynamicImage {
                        url: normalize_image_host(&p.url, 0),
                        width: p.width,
                        height: p.height,
                    })
                    .filter(|p| !p.url.is_empty())
                    .collect();
            }
            if let Some(draw) = major.draw.as_ref() {
                images = draw
                    .items
                    .iter()
                    .map(|p| DynamicImage {
                        url: normalize_image_host(&p.src, 0),
                        width: p.width,
                        height: p.height,
                    })
                    .filter(|p| !p.url.is_empty())
                    .collect();
            }

            // 文章 / 直播 / 专栏 / 通用卡片：统一当链接卡兜底，不让关注流缺一块。
            if let Some(article) = major.article.as_ref() {
                title = format::html_decode(article.title.trim());
                if text.is_empty() {
                    text = format::html_decode(article.desc.trim());
                }
                link = Some(DynamicLink {
                    title: title.clone(),
                    desc: format::html_decode(article.desc.trim()),
                    cover: article
                        .covers
                        .first()
                        .map(|c| normalize_image_host(c, 0))
                        .unwrap_or_default(),
                });
            } else if let Some(live) = major.live.as_ref() {
                title = format::html_decode(live.title.trim());
                if text.is_empty() {
                    text = format::html_decode(live.desc_first.trim());
                }
                link = Some(DynamicLink {
                    title: title.clone(),
                    desc: text.clone(),
                    cover: normalize_image_host(&live.cover, 0),
                });
            } else if let Some(common) = major.common.as_ref() {
                title = format::html_decode(common.title.trim());
                if text.is_empty() {
                    text = format::html_decode(common.desc.trim());
                }
                link = Some(DynamicLink {
                    title: title.clone(),
                    desc: text.clone(),
                    cover: normalize_image_host(&common.cover, 0),
                });
            }
        }

        // 转发：递归解析原动态；原动态为空时仍保留转发文案。
        let origin = raw
            .orig
            .as_ref()
            .and_then(|o| DynamicItem::from_raw(o, index).map(Box::new));

        let kind = match raw.kind.as_str() {
            "DYNAMIC_TYPE_AV" => {
                if video.is_some() {
                    DynamicKind::Video
                } else {
                    DynamicKind::Link
                }
            }
            "DYNAMIC_TYPE_DRAW" => {
                if images.is_empty() {
                    DynamicKind::Text
                } else {
                    DynamicKind::Images
                }
            }
            "DYNAMIC_TYPE_WORD" => DynamicKind::Text,
            "DYNAMIC_TYPE_FORWARD" => DynamicKind::Forward,
            _ => {
                if link.is_some() || !images.is_empty() {
                    DynamicKind::Link
                } else if !text.is_empty() || !title.is_empty() {
                    DynamicKind::Text
                } else {
                    // 纯挂件 / 空动态：不渲染。
                    return None;
                }
            }
        };

        // 统计模块。
        let stat = raw.modules.module_stat.as_ref();
        let count = |s: Option<&crate::api::models::DynamicCountRaw>| {
            s.map(|s| s.count).unwrap_or_default()
        };
        let stats = DynamicStats {
            forward: count(stat.and_then(|s| s.forward.as_ref())),
            comment: count(stat.and_then(|s| s.comment.as_ref())),
            like: count(stat.and_then(|s| s.like.as_ref())),
        };

        let comment_target = {
            let basic = &raw.basic;
            let kind = basic.comment_type;
            let oid = if !basic.comment_id_str.is_empty() {
                basic.comment_id_str.clone()
            } else {
                basic.rid_str.clone()
            };
            if kind > 0 && !oid.is_empty() {
                Some(CommentTarget { oid, kind })
            } else {
                None
            }
        };

        let placeholder = PLACEHOLDER_PALETTE[index % PLACEHOLDER_PALETTE.len()].to_string();
        let item = Self {
            index,
            id: raw.id_str.clone(),
            kind,
            author,
            pub_ts: parse_pub_ts(&raw.modules),
            pub_time: raw
                .modules
                .module_author
                .as_ref()
                .map(|a| a.pub_time.trim().to_string())
                .unwrap_or_default(),
            action: raw
                .modules
                .module_author
                .as_ref()
                .map(|a| format::html_decode(a.pub_action.trim()))
                .unwrap_or_default(),
            title,
            text,
            images,
            video,
            link,
            stats,
            comment_target,
            origin,
            placeholder_color: placeholder,
        };

        if item.is_renderable() {
            Some(item)
        } else {
            None
        }
    }
}

/// 富文本 → 纯文本。
///
/// 动态正文里的自定义表情是**图片**，无法在卡片文本里内联；这里把表情节点
/// 收敛成一个空格（避免「文字/表情/文字」粘在一起），其余节点保留原文。
fn rich_text(raw: &crate::api::models::RichTextRaw) -> String {
    if raw.rich_text_nodes.is_empty() {
        return format::html_decode(raw.text.trim());
    }
    let mut out = String::new();
    for node in &raw.rich_text_nodes {
        if node.kind == "RICH_TEXT_NODE_TYPE_EMOJI" {
            if !out.ends_with(' ') {
                out.push(' ');
            }
        } else {
            out.push_str(&node.text);
        }
    }
    format::html_decode(out.trim())
}

/// 接口里的链接图（头像 / 封面）可能带 `bfs_style` 查询参数；统一补 https 并按需缩放。
fn normalize_image_host(url: &str, width: u32) -> String {
    if url.trim().is_empty() {
        return String::new();
    }
    if width > 0 {
        asset_url(url, width)
    } else {
        let url = url.trim();
        if let Some(rest) = url.strip_prefix("http://") {
            format!("https://{rest}")
        } else {
            url.to_string()
        }
    }
}

fn parse_pub_ts(modules: &DynamicModules) -> u64 {
    modules
        .module_author
        .as_ref()
        .map(|a| a.pub_ts)
        .unwrap_or(0)
}

/// 详情浮层里的一条评论（已收敛）。
#[derive(Debug, Clone, PartialEq, Eq, Default)]
pub struct DynamicComment {
    pub author: String,
    pub face: String,
    pub text: String,
    pub time: String,
    pub like: u64,
}

impl DynamicComment {
    pub fn from_raw(raw: &ReplyRaw) -> Self {
        let time = raw
            .reply_control
            .as_ref()
            .and_then(|c| c.time_desc.clone())
            .filter(|t| !t.is_empty())
            .unwrap_or_else(|| format::relative_time(raw.ctime));
        Self {
            author: format::html_decode(raw.member.uname.trim()),
            face: raw
                .member
                .face
                .as_deref()
                .map(|f| normalize_image_host(f, 64))
                .unwrap_or_default(),
            text: format::html_decode(raw.content.message.trim()),
            time,
            like: raw.like,
        }
    }
}

/// 一页关注动态。
#[derive(Debug, Clone, Default)]
pub struct DynamicPage {
    pub items: Vec<DynamicItem>,
    pub offset: String,
    pub has_more: bool,
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::api::models::{DynamicFeedResponse, ReplyResponse};

    const FEED: &str = r#"{
      "code": 0,
      "data": {
        "offset": "1263055396797415427",
        "has_more": true,
        "items": [
          {
            "id_str": "1263055396797415427",
            "type": "DYNAMIC_TYPE_AV",
            "basic": {"comment_type": 1, "comment_id_str": "117499903416536", "rid_str": "117499903416536"},
            "modules": {
              "module_author": {"mid": 273710185, "name": "桉慕然", "face": "http://i1.hdslb.com/bfs/face/a.jpg",
                "pub_time": "21分钟前", "pub_ts": "1792916401", "pub_action": "投稿了视频"},
              "module_dynamic": {
                "desc": null,
                "major": {
                  "type": "MAJOR_TYPE_ARCHIVE",
                  "archive": {"bvid": "BV1G1HG62EdL", "title": "一家人就是要整整齐齐！", "cover": "http://i2.hdslb.com/bfs/archive/a.jpg",
                    "desc": "-", "duration_text": "08:01", "stat": {"play": "6442", "danmaku": "145"}}
                }
              },
              "module_stat": {
                "forward": {"count": 0}, "comment": {"count": 56}, "like": {"count": 617}
              }
            }
          },
          {
            "id_str": "1263045106052628519",
            "type": "DYNAMIC_TYPE_DRAW",
            "basic": {"comment_type": 11, "comment_id_str": "413280576", "rid_str": "413280576"},
            "modules": {
              "module_author": {"mid": 1440295, "name": "一个凡老师", "face": "http://i1.hdslb.com/bfs/face/b.jpg",
                "pub_time": "1小时前", "pub_ts": 1792914005, "pub_action": ""},
              "module_dynamic": {
                "desc": null,
                "major": {
                  "type": "MAJOR_TYPE_OPUS",
                  "opus": {
                    "title": "",
                    "summary": {"text": "今天来参加交流日啦！\n还带着小朋友一起感受了快乐！"},
                    "pics": [
                      {"url": "http://i0.hdslb.com/bfs/new_dyn/a.png", "width": 1448, "height": 1086},
                      {"url": "http://i0.hdslb.com/bfs/new_dyn/b.png", "width": 1086, "height": 1448}
                    ]
                  }
                }
              },
              "module_stat": {"forward": {"count": 1}, "comment": {"count": 2}, "like": {"count": 75}}
            }
          },
          {
            "id_str": "1263000000000000000",
            "type": "DYNAMIC_TYPE_WORD",
            "basic": {"comment_type": 17, "comment_id_str": "1263000000000000000", "rid_str": ""},
            "modules": {
              "module_author": {"mid": 42, "name": "纯文字", "face": "", "pub_time": "", "pub_ts": 1700000000, "pub_action": ""},
              "module_dynamic": {"desc": {"text": "一句话动态 &amp; 测试"}, "major": null},
              "module_stat": {"forward": {"count": 0}, "comment": {"count": 0}, "like": {"count": 3}}
            }
          }
        ]
      }
    }"#;

    #[test]
    fn parses_video_image_and_word() {
        let parsed: DynamicFeedResponse = serde_json::from_str(FEED).unwrap();
        let data = parsed.data.unwrap();
        assert_eq!(data.items.len(), 3);
        let items: Vec<_> = data
            .items
            .iter()
            .enumerate()
            .filter_map(|(i, raw)| DynamicItem::from_raw(raw, i))
            .collect();
        assert_eq!(items.len(), 3);

        assert_eq!(items[0].kind, DynamicKind::Video);
        assert_eq!(items[0].video.as_ref().unwrap().bvid, "BV1G1HG62EdL");
        assert_eq!(items[0].video.as_ref().unwrap().play_count, 6442);
        assert_eq!(items[0].meta_line(), "21分钟前 · 投稿了视频");
        assert_eq!(items[0].author.name, "桉慕然");

        assert_eq!(items[1].kind, DynamicKind::Images);
        assert_eq!(items[1].images.len(), 2);
        assert_eq!(items[1].images[0].aspect(), 1448.0 / 1086.0);
        assert!(items[1].images[0].thumb_url(480).ends_with("@480w.webp"));

        assert_eq!(items[2].kind, DynamicKind::Text);
        assert_eq!(items[2].text, "一句话动态 & 测试");
    }

    #[test]
    fn fitted_width_matches_display_and_native_size() {
        assert_eq!(fitted_width(36.0, 2.0, 0), 80);
        assert_eq!(fitted_width(120.0, 2.0, 0), 240);
        assert_eq!(fitted_width(372.0, 2.0, 1448), 752);
        // 原图更小：不放大
        assert_eq!(fitted_width(372.0, 2.0, 700), 700);
        assert_eq!(fitted_width(1.0, 2.0, 0), 64);
        assert_eq!(fitted_width(2000.0, 2.0, 0), 1280);
    }

    #[test]
    fn image_fit_url_tracks_display_width() {
        let image = DynamicImage {
            url: "https://i0.hdslb.com/bfs/new_dyn/a.png".into(),
            width: 1448,
            height: 1086,
        };
        assert!(image.fit_url(372.0, 2.0).ends_with("@752w.webp"));
        assert!(image.fit_url(120.0, 2.0).ends_with("@240w.webp"));
        // 原图更窄：直接取原图宽度，不放大
        let small = DynamicImage {
            url: "https://i0.hdslb.com/bfs/new_dyn/b.png".into(),
            width: 300,
            height: 300,
        };
        assert!(small.fit_url(372.0, 2.0).ends_with("@300w.webp"));
    }

    #[test]
    fn image_hosts_are_normalized() {
        assert_eq!(
            asset_url("http://i0.hdslb.com/bfs/new_dyn/a.png", 480),
            "https://i0.hdslb.com/bfs/new_dyn/a.png@480w.webp"
        );
        // 已带缩放后缀不重复追加
        let already = "https://i0.hdslb.com/bfs/new_dyn/a.png@100w.webp";
        assert_eq!(asset_url(already, 480), already);
        assert_eq!(asset_url("", 480), "");
    }

    #[test]
    fn null_replies_become_empty_list() {
        let parsed: ReplyResponse =
            serde_json::from_str(r#"{"code":0,"data":{"replies":null,"page":{"count":0}}}"#)
                .unwrap();
        assert!(parsed.data.unwrap().replies.is_empty());
    }

    #[test]
    fn empty_dynamic_is_dropped() {
        let raw: DynamicItemRaw =
            serde_json::from_str(r#"{"id_str":"1","type":"DYNAMIC_TYPE_NONE","modules":{}}"#)
                .unwrap();
        assert!(DynamicItem::from_raw(&raw, 0).is_none());
    }
}
