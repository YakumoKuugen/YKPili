//! 卡片点击后的播放源选择。
//!
//! B 站 `playurl` 现在主要返回 DASH：视频、音频是两条独立的 m4s 流，
//! 不能直接把视频流丢给 mpv 就期待有声音。这里把响应收敛成
//! 「一条视频 URL + 可选一条音频 URL」交给播放器。

use crate::api_models::{DashStream, PlayUrlData, VideoInfoData, VideoPage};

/// 一档可选清晰度（画质菜单的一行）。
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct PlaybackQuality {
    /// B 站画质 id（qn）。
    pub id: i32,
    /// 展示文案，如「1080P 高清」。
    pub label: String,
    /// 是否只有大会员能播。
    pub vip_only: bool,
}

/// 画质 id 的静态定义。
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct QualityDef {
    pub id: i32,
    pub label: &'static str,
    pub vip_only: bool,
}

/// 非大会员可选的清晰度（从低到高）。
///
/// 对应 B 站网页端普通账号能看到的档位；真正的可用性还要和 playurl 返回的
/// `accept_quality` / `support_formats` 取交集（未登录时 1080P 也可能不可用）。
pub const QUALITY_NON_VIP: &[QualityDef] = &[
    QualityDef { id: 16, label: "360P 流畅", vip_only: false },
    QualityDef { id: 32, label: "480P 清晰", vip_only: false },
    QualityDef { id: 64, label: "720P 高清", vip_only: false },
    QualityDef { id: 80, label: "1080P 高清", vip_only: false },
];

/// 大会员额外解锁的清晰度（从低到高）。
///
/// 大会员菜单 = [`QUALITY_NON_VIP`] + 这一份；非大会员菜单只有前一份。
pub const QUALITY_VIP_ONLY: &[QualityDef] = &[
    QualityDef { id: 74, label: "720P 60帧", vip_only: true },
    QualityDef { id: 112, label: "1080P 高码率", vip_only: true },
    QualityDef { id: 116, label: "1080P 60帧", vip_only: true },
    QualityDef { id: 120, label: "4K 超清", vip_only: true },
    QualityDef { id: 125, label: "HDR 真彩", vip_only: true },
    QualityDef { id: 126, label: "杜比视界", vip_only: true },
    QualityDef { id: 127, label: "8K 超高清", vip_only: true },
];

/// 按 id 查静态定义。
pub fn quality_def(id: i32) -> Option<&'static QualityDef> {
    QUALITY_NON_VIP
        .iter()
        .chain(QUALITY_VIP_ONLY.iter())
        .find(|def| def.id == id)
}

/// 一次选流的结果（只含播放器换流真正需要的字段）。
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct SelectedStream {
    pub video_url: String,
    pub audio_url: Option<String>,
    pub width: u32,
    pub height: u32,
    /// 实际选中的画质 id；`durl` 渐进流拿不到时按请求值兜底。
    pub quality_id: i32,
    /// 实际选中的画质文案。
    pub quality_label: String,
}

/// 一次播放所需的全部来源信息。
#[derive(Debug, Clone, PartialEq, Eq, Default)]
pub struct PlaybackSource {
    /// 视频流地址。
    pub video_url: String,
    /// 音频流地址；DASH 才有，渐进式 durl 为 None。
    pub audio_url: Option<String>,
    /// 视频标题（详情接口返回）。
    pub title: String,
    /// 简介（详情接口返回）。
    pub desc: String,
    /// 发布时间（Unix 秒）。
    pub pubdate: u64,
    /// 分区名（如「游戏 · 射击游戏」），标签拉取失败时作为标签兜底。
    pub category: String,
    /// 选中的画质文案（如「1080P 高清」）。
    pub quality: String,
    /// 选中的画质 id（qn）。
    pub quality_id: i32,
    /// 当前账号 + 当前视频可选的清晰度列表（已按大会员状态过滤）。
    pub qualities: Vec<PlaybackQuality>,
    /// 时长（秒）。
    pub duration_secs: u64,
    /// 视频像素宽（DASH 响应；0 = 未知）。
    pub video_width: u32,
    /// 视频像素高（DASH 响应；0 = 未知）。
    pub video_height: u32,
    /// 视频 aid；观看历史心跳上报 / 评论 oid 用。
    pub aid: i64,
    /// 视频 bvid；观看历史心跳上报用。
    pub bvid: String,
    /// 当前分 P 的 cid；观看历史心跳上报用。
    pub cid: i64,
    /// UP 主 uid；查关注状态 / 粉丝数 / 关注 / 取关都用它（0 = 没拿到）。
    pub owner_mid: i64,
    /// UP 主昵称。
    pub owner_name: String,
    /// UP 主头像 URL（原始地址，视图侧再按尺寸取）。
    pub owner_face: String,
    /// UP 主签名（来自 `/x/web-interface/card`，可能为空）。
    pub owner_sign: String,
    /// 播放量。
    pub view_count: u64,
    /// 弹幕数。
    pub danmaku_count: u64,
    /// 评论数。
    pub reply_count: u64,
    /// 点赞数。
    pub like_count: u64,
    /// 投币数。
    pub coin_count: u64,
    /// 收藏数。
    pub favorite_count: u64,
    /// 分享数。
    pub share_count: u64,
    /// 分 P 列表（「选集」Tab）。
    pub pages: Vec<VideoPage>,
    /// 当前播放的分 P 序号（从 1 开始；0 = 未知）。
    pub current_page: i32,
}

/// 由画质 id / 分辨率推一个展示文案。
fn quality_label(height: i32) -> String {
    match height {
        h if h >= 2160 => "4K 超清",
        h if h >= 1440 => "2K",
        h if h >= 1080 => "1080P 高清",
        h if h >= 720 => "720P 高清",
        h if h >= 480 => "480P 清晰",
        h if h > 0 => "360P 流畅",
        _ => "",
    }
    .to_string()
}

/// 单个画质的展示文案：优先 API 的 `new_description`，其次静态表，最后按分辨率推。
fn format_label(play: &PlayUrlData, id: i32, height: i32) -> String {
    if let Some(format) = play.support_formats.iter().find(|f| f.quality == id) {
        if !format.new_description.trim().is_empty() {
            return format.new_description.trim().to_string();
        }
        if !format.display_desc.trim().is_empty() {
            return format.display_desc.trim().to_string();
        }
    }
    if let Some(def) = quality_def(id) {
        return def.label.to_string();
    }
    quality_label(height)
}

/// 当前账号可选的清晰度列表（高 → 低，菜单从上到下）。
///
/// 规则（需求原文）：
/// 1. 大会员：**大会员 + 非大会员**允许播放的全部清晰度；
/// 2. 非大会员：只列非大会员允许播放的清晰度；
/// 3. 再用 playurl 的 `accept_quality` / `support_formats` 过滤 —— 视频本身
///    没有的档位（比如 480P 老片）不能出现在菜单里；
/// 4. 最后若一条都没匹配上（老接口 / 字段缺失），按 DASH 实际流兜底。
pub fn available_qualities(play: &PlayUrlData, is_vip: bool) -> Vec<PlaybackQuality> {
    let accepted = !play.accept_quality.is_empty();
    let has_formats = !play.support_formats.is_empty();
    let mut out = Vec::new();

    let vip_defs: &[QualityDef] = if is_vip { QUALITY_VIP_ONLY } else { &[] };
    let defs = QUALITY_NON_VIP.iter().chain(vip_defs.iter());

    // 只有拿到「账号允许的档位」或「视频支持的档位」时才按静态表交叉过滤；
    // 两个字段都缺（老接口）就直接按 DASH 实际流兜底，避免列出点了播不了的档。
    if accepted || has_formats {
        for def in defs {
            // 非大会员永远不列会员档；大会员只列视频 / 账号双方都允许的档。
            if def.vip_only && !is_vip {
                continue;
            }
            if accepted && !play.accept_quality.contains(&def.id) {
                continue;
            }
            if has_formats && !play.support_formats.iter().any(|f| f.quality == def.id) {
                continue;
            }
            let label = format_label(play, def.id, 0);
            out.push(PlaybackQuality {
                id: def.id,
                label,
                vip_only: def.vip_only,
            });
        }
    }

    // 老接口 / 字段缺失：按实际 DASH 流兜底，保证菜单至少能选当前档位。
    if out.is_empty() {
        if let Some(dash) = &play.dash {
            for stream in &dash.video {
                if stream.id <= 0 || out.iter().any(|q| q.id == stream.id) {
                    continue;
                }
                let vip_only = quality_def(stream.id).is_some_and(|def| def.vip_only);
                if vip_only && !is_vip {
                    continue;
                }
                out.push(PlaybackQuality {
                    id: stream.id,
                    label: format_label(play, stream.id, stream.height),
                    vip_only,
                });
            }
        }
    }

    // 高 → 低：和 B 站菜单一致，用户先看到最好的档位。
    out.sort_by(|a, b| b.id.cmp(&a.id));
    out.dedup_by_key(|q| q.id);
    out
}

/// 同一档位里挑一条：画质 id 高的优先，其次像素数，最后 AVC 优先（兼容性最好）。
fn best_stream<'a, I>(iter: I) -> Option<&'a DashStream>
where
    I: Iterator<Item = &'a DashStream>,
{
    iter.max_by_key(|v| (v.id, v.width.saturating_mul(v.height), v.codec_rank()))
}

/// 从 playurl 响应里选一条视频流。
///
/// `preferred` = 用户点选的 qn；该档不存在时自动退回最高可用档。
pub fn select_stream(play: &PlayUrlData, preferred: Option<i32>) -> Option<SelectedStream> {
    if let Some(dash) = &play.dash {
        let video = preferred
            .and_then(|qn| {
                best_stream(
                    dash.video
                        .iter()
                        .filter(|v| !v.play_url().is_empty() && v.id == qn),
                )
            })
            .or_else(|| {
                best_stream(dash.video.iter().filter(|v| !v.play_url().is_empty()))
            });

        if let Some(video) = video {
            let audio = dash
                .audio
                .iter()
                .filter(|a| !a.play_url().is_empty())
                .max_by_key(|a| a.id);
            return Some(SelectedStream {
                video_url: video.play_url().to_string(),
                audio_url: audio.map(|a| a.play_url().to_string()),
                width: video.width.max(0) as u32,
                height: video.height.max(0) as u32,
                quality_id: video.id,
                quality_label: format_label(play, video.id, video.height),
            });
        }
    }

    // 渐进式 durl：自带音轨，没有分档信息。
    play.durl
        .iter()
        .find(|d| !d.url.is_empty())
        .map(|part| {
            let id = preferred.unwrap_or(0);
            SelectedStream {
                video_url: part.url.clone(),
                audio_url: None,
                width: 0,
                height: 0,
                quality_id: id,
                quality_label: format_label(play, id, 0),
            }
        })
}

/// 从详情里收集信息面板需要的元数据（两条选流分支共用）。
fn meta_from_info(info: &VideoInfoData, duration_secs: u64, quality: String) -> PlaybackSource {
    let owner = info.owner.as_ref();
    let stat = info.stat.as_ref();
    let count = |f: fn(&crate::api_models::VideoStat) -> u64| stat.map(f).unwrap_or(0);
    PlaybackSource {
        video_url: String::new(),
        audio_url: None,
        title: info.title.clone(),
        desc: info.desc.trim().to_string(),
        pubdate: info.pubdate,
        category: info.tname.trim().to_string(),
        quality,
        quality_id: 0,
        qualities: Vec::new(),
        duration_secs,
        video_width: 0,
        video_height: 0,
        aid: info.aid,
        bvid: if info.bvid.is_empty() {
            String::new()
        } else {
            info.bvid.clone()
        },
        cid: info.cid.max(0),
        owner_mid: owner.map(|o| o.mid).unwrap_or(0),
        owner_name: owner.map(|o| o.name.trim().to_string()).unwrap_or_default(),
        owner_face: owner.map(|o| o.face.trim().to_string()).unwrap_or_default(),
        owner_sign: String::new(),
        view_count: count(|s| s.view),
        danmaku_count: count(|s| s.danmaku),
        reply_count: count(|s| s.reply),
        like_count: count(|s| s.like),
        coin_count: count(|s| s.coin),
        favorite_count: count(|s| s.favorite),
        share_count: count(|s| s.share),
        pages: info.pages.clone(),
        current_page: info
            .pages
            .iter()
            .find(|p| p.cid == info.cid)
            .map(|p| p.page)
            .unwrap_or(1),
    }
}

/// 从详情 + playurl 响应里选一条可播放的视频流。
///
/// 规则：
/// 1. 有 DASH：视频按 (id, 像素数, AVC 优先) 选最高，音频按 id 选最高；
/// 2. 没有 DASH：退回 `durl` 的第一段（自带音轨）；
/// 3. 都没有：返回错误，不静默播测试视频。
///
/// `is_vip` 决定画质菜单列哪些档位：大会员 = 大会员 + 非大会员，非大会员 = 仅后者。
pub fn select_playback(
    info: &VideoInfoData,
    play: &PlayUrlData,
    is_vip: bool,
) -> Result<PlaybackSource, String> {
    let duration_secs = if play.timelength > 0 {
        play.timelength / 1000
    } else {
        info.duration
    };

    let stream =
        select_stream(play, None).ok_or_else(|| "playurl 没有可用的视频流（可能需要登录或该视频受限）".to_string())?;
    let mut source = meta_from_info(info, duration_secs, stream.quality_label.clone());
    source.video_url = stream.video_url;
    source.audio_url = stream.audio_url;
    source.video_width = stream.width;
    source.video_height = stream.height;
    source.quality_id = stream.quality_id;
    source.quality = stream.quality_label;
    source.qualities = available_qualities(play, is_vip);
    Ok(source)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::api_models::{DashData, DurlItem, SupportFormat, VideoOwner, VideoPage, VideoStat};

    fn stream(id: i32, w: i32, h: i32, codecs: &str, url: &str) -> DashStream {
        DashStream {
            id,
            base_url: url.into(),
            fallback_url: String::new(),
            backup_urls: Vec::new(),
            codecs: codecs.into(),
            codec_id: 0,
            width: w,
            height: h,
        }
    }

    fn info() -> VideoInfoData {
        VideoInfoData {
            bvid: "BV1".into(),
            cid: 42,
            title: "测试视频".into(),
            duration: 100,
            pages: vec![VideoPage {
                cid: 42,
                page: 1,
                part: "P1".into(),
                duration: 100,
            }],
            ..Default::default()
        }
    }

    #[test]
    fn prefers_highest_dash_video_and_avc() {
        let play = PlayUrlData {
            timelength: 107_000,
            dash: Some(DashData {
                video: vec![
                    stream(32, 640, 360, "avc1.64001F", "v32-avc"),
                    stream(32, 640, 360, "hvc1", "v32-hevc"),
                    stream(80, 1920, 1080, "avc1.640028", "v80-avc"),
                ],
                audio: vec![
                    stream(30216, 0, 0, "mp4a", "a16"),
                    stream(30280, 0, 0, "mp4a", "a80"),
                ],
            }),
            ..Default::default()
        };
        let src = select_playback(&info(), &play, false).unwrap();
        assert_eq!(src.video_url, "v80-avc");
        assert_eq!(src.audio_url.as_deref(), Some("a80"));
        assert_eq!(src.duration_secs, 107);
        assert_eq!(src.video_width, 1920);
        assert_eq!(src.video_height, 1080);
        assert_eq!(src.title, "测试视频");
    }

    #[test]
    fn prefers_upos_backup_over_mcdn() {
        let play = PlayUrlData {
            dash: Some(DashData {
                video: vec![DashStream {
                    id: 80,
                    base_url: "https://edge.mcdn.bilivideo.cn/v.m4s".into(),
                    fallback_url: String::new(),
                    backup_urls: vec![
                        "https://b-x.edge.mountaintoys.cn/v.m4s".into(),
                        "https://upos-sz-mirrorcoso1.bilivideo.com/v.m4s".into(),
                    ],
                    codecs: "avc1".into(),
                    codec_id: 0,
                    width: 1920,
                    height: 1080,
                }],
                audio: vec![DashStream {
                    id: 30280,
                    base_url: "https://edge.mcdn.bilivideo.cn/a.m4s".into(),
                    fallback_url: String::new(),
                    backup_urls: vec!["https://upos-sz-mirrorcoso1.bilivideo.com/a.m4s".into()],
                    codecs: "mp4a".into(),
                    codec_id: 0,
                    width: 0,
                    height: 0,
                }],
            }),
            ..Default::default()
        };
        let src = select_playback(&info(), &play, false).unwrap();
        assert!(src.video_url.contains("upos"));
        assert!(src.audio_url.unwrap().contains("upos"));
    }

    #[test]
    fn falls_back_to_durl_when_no_dash() {
        let play = PlayUrlData {
            dash: None,
            durl: vec![DurlItem {
                url: "progressive.mp4".into(),
                length: 1000,
            }],
            ..Default::default()
        };
        let src = select_playback(&info(), &play, false).unwrap();
        assert_eq!(src.video_url, "progressive.mp4");
        assert_eq!(src.audio_url, None);
        assert_eq!(src.duration_secs, 100);
    }

    #[test]
    fn carries_panel_metadata_and_quality() {
        let mut info = info();
        info.owner = Some(VideoOwner {
            mid: 2333,
            name: "UP".into(),
            face: "https://i0.hdslb.com/f.jpg".into(),
            ..Default::default()
        });
        info.stat = Some(VideoStat {
            view: 1234,
            danmaku: 5,
            reply: 6,
            like: 7,
            coin: 8,
            favorite: 9,
            share: 10,
        });
        info.desc = "简介".into();
        info.pubdate = 42;
        info.tname = "单机游戏".into();
        let play = PlayUrlData {
            dash: Some(DashData {
                video: vec![stream(80, 1920, 1080, "avc1", "v")],
                audio: vec![],
            }),
            ..Default::default()
        };
        let src = select_playback(&info, &play, false).unwrap();
        assert_eq!(src.quality, "1080P 高清");
        assert_eq!(src.owner_mid, 2333);
        assert_eq!(src.owner_name, "UP");
        assert_eq!(src.owner_face, "https://i0.hdslb.com/f.jpg");
        assert_eq!(src.view_count, 1234);
        assert_eq!(src.danmaku_count, 5);
        assert_eq!(src.reply_count, 6);
        assert_eq!(src.like_count, 7);
        assert_eq!(src.coin_count, 8);
        assert_eq!(src.favorite_count, 9);
        assert_eq!(src.share_count, 10);
        assert_eq!(src.desc, "简介");
        assert_eq!(src.pubdate, 42);
        assert_eq!(src.category, "单机游戏");
        assert_eq!(src.current_page, 1);
    }

    #[test]
    fn empty_streams_are_an_error() {
        let play = PlayUrlData::default();
        assert!(select_playback(&info(), &play, false).is_err());
    }

    fn support(qn: i32, desc: &str, need_vip: bool) -> SupportFormat {
        SupportFormat {
            quality: qn,
            new_description: desc.into(),
            need_vip,
            ..Default::default()
        }
    }

    /// 非大会员菜单：只列非大会员档，并且和 accept_quality / support_formats 取交集。
    #[test]
    fn non_vip_quality_list_is_free_tiers_only() {
        let play = PlayUrlData {
            accept_quality: vec![16, 32, 64, 80, 120],
            support_formats: vec![
                support(16, "360P 流畅", false),
                support(32, "480P 清晰", false),
                support(64, "720P 高清", false),
                support(80, "1080P 高清", false),
                support(120, "4K 超清", true),
            ],
            ..Default::default()
        };
        let qualities = available_qualities(&play, false);
        let ids: Vec<i32> = qualities.iter().map(|q| q.id).collect();
        assert_eq!(ids, vec![80, 64, 32, 16], "非大会员不应看到 4K，且高→低排序");
        assert!(qualities.iter().all(|q| !q.vip_only));
    }

    /// 大会员菜单：非大会员档 + 大会员档，仍然只列视频真正支持的档位。
    #[test]
    fn vip_quality_list_adds_premium_tiers() {
        let play = PlayUrlData {
            accept_quality: vec![16, 32, 64, 80, 120, 127],
            support_formats: vec![
                support(16, "360P 流畅", false),
                support(32, "480P 清晰", false),
                support(64, "720P 高清", false),
                support(80, "1080P 高清", false),
                support(120, "4K 超清", true),
                support(127, "8K 超高清", true),
                // 视频没有 125 / 126：不应出现在菜单里
            ],
            ..Default::default()
        };
        let qualities = available_qualities(&play, true);
        let ids: Vec<i32> = qualities.iter().map(|q| q.id).collect();
        assert_eq!(ids, vec![127, 120, 80, 64, 32, 16]);
        assert!(qualities.iter().find(|q| q.id == 120).unwrap().vip_only);
    }

    /// 老接口没有 accept_quality / support_formats：按 DASH 实际流兜底。
    #[test]
    fn quality_list_falls_back_to_dash_streams() {
        let play = PlayUrlData {
            dash: Some(DashData {
                video: vec![
                    stream(80, 1920, 1080, "avc1", "v80"),
                    stream(32, 640, 360, "avc1", "v32"),
                    stream(120, 3840, 2160, "avc1", "v120"),
                ],
                audio: vec![],
            }),
            ..Default::default()
        };
        let ids: Vec<i32> = available_qualities(&play, false)
            .iter()
            .map(|q| q.id)
            .collect();
        assert_eq!(ids, vec![80, 32], "非大会员兜底列表也要过滤 4K");

        let ids: Vec<i32> = available_qualities(&play, true)
            .iter()
            .map(|q| q.id)
            .collect();
        assert_eq!(ids, vec![120, 80, 32]);
    }

    /// 点选某档清晰度后，选流必须命中该档，而不是继续选最高。
    #[test]
    fn select_stream_respects_requested_quality() {
        let play = PlayUrlData {
            support_formats: vec![
                support(80, "1080P 高清", false),
                support(64, "720P 高清", false),
            ],
            dash: Some(DashData {
                video: vec![
                    stream(80, 1920, 1080, "avc1", "v80"),
                    stream(64, 1280, 720, "avc1", "v64"),
                ],
                audio: vec![stream(30280, 0, 0, "mp4a", "a80")],
            }),
            ..Default::default()
        };
        let picked = select_stream(&play, Some(64)).unwrap();
        assert_eq!(picked.video_url, "v64");
        assert_eq!(picked.quality_id, 64);
        assert_eq!(picked.quality_label, "720P 高清");
        assert_eq!(picked.width, 1280);
        assert_eq!(picked.audio_url.as_deref(), Some("a80"));

        // 请求了不存在的档：退回最高可用档，不能播空。
        let fallback = select_stream(&play, Some(4)).unwrap();
        assert_eq!(fallback.video_url, "v80");
    }
}
