//! 卡片点击后的播放源选择。
//!
//! B 站 `playurl` 现在主要返回 DASH：视频、音频是两条独立的 m4s 流，
//! 不能直接把视频流丢给 mpv 就期待有声音。这里把响应收敛成
//! 「一条视频 URL + 可选一条音频 URL」交给播放器。

use crate::api::models::{DashStream, PlayUrlData, VideoInfoData};

/// 一次播放所需的全部来源信息。
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct PlaybackSource {
    /// 视频流地址。
    pub video_url: String,
    /// 音频流地址；DASH 才有，渐进式 durl 为 None。
    pub audio_url: Option<String>,
    /// 视频标题（详情接口返回）。
    pub title: String,
    /// 时长（秒）。
    pub duration_secs: u64,
    /// 视频 aid；观看历史心跳上报用。
    pub aid: i64,
    /// 视频 bvid；观看历史心跳上报用。
    pub bvid: String,
    /// 当前分 P 的 cid；观看历史心跳上报用。
    pub cid: i64,
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
            return Ok(PlaybackSource {
                video_url: video.play_url().to_string(),
                audio_url: audio.map(|a| a.play_url().to_string()),
                title: info.title.clone(),
                duration_secs,
                aid: info.aid,
                bvid: if info.bvid.is_empty() {
                    String::new()
                } else {
                    info.bvid.clone()
                },
                cid: info.cid.max(0),
            });
        }
    }

    if let Some(part) = play.durl.iter().find(|d| !d.url.is_empty()) {
        return Ok(PlaybackSource {
            video_url: part.url.clone(),
            audio_url: None,
            title: info.title.clone(),
            duration_secs,
            aid: info.aid,
            bvid: info.bvid.clone(),
            cid: info.cid.max(0),
        });
    }

    Err("playurl 没有可用的视频流（可能需要登录或该视频受限）".into())
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::api::models::{DashData, DurlItem, VideoPage};

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
    fn empty_streams_are_an_error() {
        let play = PlayUrlData::default();
        assert!(select_playback(&info(), &play).is_err());
    }
}
