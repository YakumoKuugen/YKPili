//! 卡片点击后的播放源选择。
//!
//! B 站 `playurl` 现在主要返回 DASH：视频、音频是两条独立的 m4s 流，
//! 不能直接把视频流丢给 mpv 就期待有声音。这里把响应收敛成
//! 「一条视频 URL + 可选一条音频 URL」交给播放器。

use crate::api::models::{DashStream, PlayUrlData, VideoInfoData, VideoPage};

/// 一次播放所需的全部来源信息。
#[derive(Debug, Clone, PartialEq, Eq)]
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

/// 从详情里收集信息面板需要的元数据（两条选流分支共用）。
fn meta_from_info(info: &VideoInfoData, duration_secs: u64, quality: String) -> PlaybackSource {
    let owner = info.owner.as_ref();
    let stat = info.stat.as_ref();
    let count = |f: fn(&crate::api::models::VideoStat) -> u64| stat.map(f).unwrap_or(0);
    PlaybackSource {
        video_url: String::new(),
        audio_url: None,
        title: info.title.clone(),
        desc: info.desc.trim().to_string(),
        pubdate: info.pubdate,
        category: info.tname.trim().to_string(),
        quality,
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

impl DashStream {
    /// 用哪条 CDN 地址。
    ///
    /// **优先 upos 镜像**：B 站 playurl 的 `baseUrl` 常给 `*.mcdn.bilivideo.cn`
    /// 边缘节点，实测会出现 HEAD 404、GET 只返回几百 KB（远小于完整 m4s），
    /// ffmpeg 就会报 `Stream ends prematurely at 0, should be N`。
    /// `backupUrl` 里的 `upos-sz-mirror*` 返回完整文件，优先使用。
    fn play_url(&self) -> &str {
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
    fn codec_rank(&self) -> u8 {
        if self.codecs.starts_with("avc1") {
            2
        } else if self.codecs.starts_with("hvc1") || self.codecs.starts_with("hev1") {
            1
        } else {
            0
        }
    }
}

/// 从详情 + playurl 响应里选一条可播放的视频流。
///
/// 规则：
/// 1. 有 DASH：视频按 (id, 像素数, AVC 优先) 选最高，音频按 id 选最高；
/// 2. 没有 DASH：退回 `durl` 的第一段（自带音轨）；
/// 3. 都没有：返回错误，不静默播测试视频。
pub fn select_playback(info: &VideoInfoData, play: &PlayUrlData) -> Result<PlaybackSource, String> {
    let duration_secs = if play.timelength > 0 {
        play.timelength / 1000
    } else {
        info.duration
    };

    if let Some(dash) = &play.dash {
        let video = dash
            .video
            .iter()
            .filter(|v| !v.play_url().is_empty())
            .max_by_key(|v| (v.id, v.width.saturating_mul(v.height), v.codec_rank()));
        if let Some(video) = video {
            let audio = dash
                .audio
                .iter()
                .filter(|a| !a.play_url().is_empty())
                .max_by_key(|a| a.id);
            let mut source = meta_from_info(info, duration_secs, quality_label(video.height));
            source.video_url = video.play_url().to_string();
            source.video_width = video.width.max(0) as u32;
            source.video_height = video.height.max(0) as u32;
            source.audio_url = audio.map(|a| a.play_url().to_string());
            return Ok(source);
        }
    }

    if let Some(part) = play.durl.iter().find(|d| !d.url.is_empty()) {
        let mut source = meta_from_info(info, duration_secs, String::new());
        source.video_url = part.url.clone();
        return Ok(source);
    }

    Err("playurl 没有可用的视频流（可能需要登录或该视频受限）".into())
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::api::models::{DashData, DurlItem, VideoOwner, VideoPage, VideoStat};

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
        let src = select_playback(&info(), &play).unwrap();
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
        let src = select_playback(&info(), &play).unwrap();
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
        let src = select_playback(&info(), &play).unwrap();
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
        let src = select_playback(&info, &play).unwrap();
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
        assert!(select_playback(&info(), &play).is_err());
    }
}
