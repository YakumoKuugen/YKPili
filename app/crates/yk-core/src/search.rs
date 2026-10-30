//! 搜索：联想词、热搜、结果解析与本地搜索历史。
//!
//! 界面侧（`yk-app/src/search_view.rs`）只消费这里已经**清洗过**的数据：
//! 标题里的 `<em class="keyword">` 高亮标签、`//i0.hdslb.com/...` 这种
//! 协议相对地址、`"11:7"` 这种时长字符串都不该漏到视图层。
//!
//! 接口选择：
//! - 联想：`s.search.bilibili.com/main/suggest`（公开，不需要 WBI）；
//! - 热搜：`s.search.bilibili.com/main/hotword`（公开）；
//! - 结果：`api.bilibili.com/x/web-interface/search/all/v2`（公开，一页拿到
//!   视频 / 番剧 / 影视 / UP 主 / 直播 / 专栏所有分组）。
//!
//! 为什么结果页用 `all/v2` 而不是 `search/type`：桌面端未登录请求
//! `search/type` 会返回风控 voucher（`data` 里只有 `v_voucher`），
//! 而 `all/v2` 未登录也能返回完整分组；一次请求就能支撑综合页的
//! 「影视 + 用户 + 视频」三段，分类 tab 只是本地过滤，切 tab 不再发请求。

use std::collections::HashSet;
use std::fs;
use std::io;
use std::path::{Path, PathBuf};
use std::time::{SystemTime, UNIX_EPOCH};

use serde::{Deserialize, Serialize};
use serde_json::Value;

use crate::api::bilibili::{self, HttpClient};
use crate::format;

/// 结果页分类 tab。
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum SearchTab {
    #[default]
    All,
    Video,
    Bangumi,
    Film,
    User,
    Live,
    Article,
}

impl SearchTab {
    pub const ALL: [Self; 7] = [
        Self::All,
        Self::Video,
        Self::Bangumi,
        Self::Film,
        Self::User,
        Self::Live,
        Self::Article,
    ];

    pub fn label(self) -> &'static str {
        match self {
            Self::All => "综合",
            Self::Video => "视频",
            Self::Bangumi => "番剧",
            Self::Film => "影视",
            Self::User => "用户",
            Self::Live => "直播",
            Self::Article => "专栏",
        }
    }
}

/// 结果排序。全部在本地对已加载结果排序 —— `all/v2` 不认 `order` 参数，
/// 桌面端未登录也拿不到 `search/type` 的排序结果，先保证交互可用。
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum SearchSort {
    #[default]
    Total,
    Play,
    Newest,
    Danmaku,
    Favorite,
}

impl SearchSort {
    pub const ALL: [Self; 5] = [
        Self::Total,
        Self::Play,
        Self::Newest,
        Self::Danmaku,
        Self::Favorite,
    ];

    pub fn label(self) -> &'static str {
        match self {
            Self::Total => "综合排序",
            Self::Play => "最多播放",
            Self::Newest => "最新发布",
            Self::Danmaku => "最多弹幕",
            Self::Favorite => "最多收藏",
        }
    }
}

/// 联想词里的一段文字；`highlighted = true` 的部分在面板里用主题色。
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct HighlightSpan {
    pub text: String,
    pub highlighted: bool,
}

/// 一条联想词。
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct SuggestItem {
    /// 点击 / 回车时真正提交的关键词（接口的 `value`）。
    pub value: String,
    /// 带高亮分段的展示文本（接口 `name` 去标签之后）。
    pub spans: Vec<HighlightSpan>,
}

impl SuggestItem {
    pub fn plain(&self) -> String {
        self.spans.iter().map(|s| s.text.as_str()).collect()
    }
}

/// 一条热搜。
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Hotword {
    pub keyword: String,
    pub pos: u32,
    /// 4=新 5=热 7=直播中 11=话题。
    pub word_type: u32,
}

impl Hotword {
    /// 角标文案；没有角标返回 `None`。
    pub fn tag(&self) -> Option<&'static str> {
        match self.word_type {
            4 => Some("新"),
            5 => Some("热"),
            7 => Some("直播"),
            11 => Some("话题"),
            _ => None,
        }
    }
}

/// 一条视频搜索结果（也用作 UP 主卡片里的小视频条目）。
#[derive(Debug, Clone, Default)]
pub struct SearchVideo {
    pub bvid: String,
    pub aid: u64,
    pub title: String,
    pub description: String,
    pub author: String,
    pub mid: i64,
    /// UP 主头像（搜索接口的 `upic`）。
    pub up_face: String,
    pub cover_url: String,
    pub view_count: u64,
    pub danmaku_count: u64,
    pub favorite_count: u64,
    pub duration_secs: u64,
    pub pubdate: u64,
    pub typename: String,
    pub is_live: bool,
}

impl SearchVideo {
    /// 转成首页卡片模型，直接复用首页 `card()` 的几何、hover 与点击播放。
    pub fn to_cover_item(&self, index: usize) -> crate::models::CoverItem {
        crate::models::CoverItem {
            index,
            title: self.title.clone(),
            placeholder_color: crate::feed::PLACEHOLDER_PALETTE
                [index % crate::feed::PLACEHOLDER_PALETTE.len()]
            .to_string(),
            image_url: self.cover_url.clone(),
            bvid: self.bvid.clone(),
            cid: 0,
            up_name: self.author.clone(),
            up_face: self.up_face.clone(),
            view_count: self.view_count,
            danmaku_count: self.danmaku_count,
            duration_secs: self.duration_secs,
            pubdate: self.pubdate,
            is_live: self.is_live,
            progress_secs: 0,
        }
    }
}

/// 一个 UP 主搜索结果。
#[derive(Debug, Clone, Default)]
pub struct SearchUser {
    pub mid: i64,
    pub name: String,
    pub sign: String,
    pub avatar_url: String,
    pub fans: u64,
    pub videos: u64,
    pub level: u32,
    /// 认证文案（如「bilibili 知名UP主」）；没有认证为空。
    pub verify: String,
    /// 接口顺手返回的最近投稿（最多 3 条）。
    pub previews: Vec<SearchVideo>,
}

/// 一集 / 一支 PV。
#[derive(Debug, Clone, Default)]
pub struct SearchEpisode {
    pub id: u64,
    pub index_title: String,
    pub long_title: String,
    pub cover_url: String,
}

/// 番剧 / 影视搜索结果。
#[derive(Debug, Clone, Default)]
pub struct SearchMedia {
    pub media_id: i64,
    pub season_id: i64,
    pub title: String,
    pub original_title: String,
    pub cover_url: String,
    pub areas: String,
    pub styles: String,
    pub index_show: String,
    pub description: String,
    pub season_type_name: String,
    pub score: Option<f64>,
    pub score_count: u64,
    pub episodes: Vec<SearchEpisode>,
    /// `true` = 电影（`media_ft`），`false` = 番剧（`media_bangumi`）。
    pub is_film: bool,
}

impl SearchMedia {
    /// 卡片 / 详情区第二行的元信息：「日本 · 动作/奇幻 · 全 6 集」。
    pub fn meta_line(&self) -> String {
        let mut parts = Vec::new();
        if !self.areas.trim().is_empty() {
            parts.push(self.areas.trim().to_string());
        }
        if !self.styles.trim().is_empty() {
            parts.push(self.styles.trim().to_string());
        }
        if !self.index_show.trim().is_empty() {
            parts.push(self.index_show.trim().to_string());
        }
        parts.join(" · ")
    }

    pub fn episode_count_label(&self) -> String {
        if self.episodes.is_empty() {
            "全 1 集".to_string()
        } else {
            format!("全 {} 集", self.episodes.len())
        }
    }
}

/// 一个直播间搜索结果。
#[derive(Debug, Clone, Default)]
pub struct SearchLive {
    pub roomid: i64,
    pub title: String,
    pub uname: String,
    pub cover_url: String,
    pub online: u64,
    pub area_name: String,
}

/// 一篇专栏搜索结果。
#[derive(Debug, Clone, Default)]
pub struct SearchArticle {
    pub id: i64,
    pub title: String,
    pub description: String,
    pub author: String,
    pub view: u64,
    pub like: u64,
    pub cover_url: String,
}

/// 一页搜索的合并结果。
#[derive(Debug, Clone, Default)]
pub struct SearchResults {
    pub videos: Vec<SearchVideo>,
    pub users: Vec<SearchUser>,
    pub bangumi: Vec<SearchMedia>,
    pub films: Vec<SearchMedia>,
    pub live: Vec<SearchLive>,
    pub articles: Vec<SearchArticle>,
}

impl SearchResults {
    pub fn is_empty(&self) -> bool {
        self.videos.is_empty()
            && self.users.is_empty()
            && self.bangumi.is_empty()
            && self.films.is_empty()
            && self.live.is_empty()
            && self.articles.is_empty()
    }

    /// 当前 tab 是否有内容（综合 tab 看全部）。
    pub fn is_empty_for(&self, tab: SearchTab) -> bool {
        match tab {
            SearchTab::All => self.is_empty(),
            SearchTab::Video => self.videos.is_empty(),
            SearchTab::Bangumi => self.bangumi.is_empty(),
            SearchTab::Film => self.films.is_empty(),
            SearchTab::User => self.users.is_empty(),
            SearchTab::Live => self.live.is_empty(),
            SearchTab::Article => self.articles.is_empty(),
        }
    }

    /// 把另一页并进来，按稳定 id 去重；返回新增条数。
    pub fn merge(&mut self, other: SearchResults) -> usize {
        let mut added = 0usize;

        let mut bvids: HashSet<String> = self
            .videos
            .iter()
            .map(|v| video_key(v))
            .filter(|k| !k.is_empty())
            .collect();
        for video in other.videos {
            let key = video_key(&video);
            if key.is_empty() || bvids.insert(key) {
                self.videos.push(video);
                added += 1;
            }
        }

        let mut mids: HashSet<i64> = self.users.iter().map(|u| u.mid).collect();
        for user in other.users {
            if user.mid == 0 || mids.insert(user.mid) {
                self.users.push(user);
                added += 1;
            }
        }

        let mut media_ids: HashSet<i64> = self
            .bangumi
            .iter()
            .chain(self.films.iter())
            .map(|m| m.media_id)
            .filter(|id| *id != 0)
            .collect();
        for media in other.bangumi {
            let key = media.media_id;
            if key == 0 || media_ids.insert(key) {
                self.bangumi.push(media);
                added += 1;
            }
        }
        for media in other.films {
            let key = media.media_id;
            if key == 0 || media_ids.insert(key) {
                self.films.push(media);
                added += 1;
            }
        }

        let mut roomids: HashSet<i64> = self.live.iter().map(|l| l.roomid).collect();
        for live in other.live {
            if live.roomid == 0 || roomids.insert(live.roomid) {
                self.live.push(live);
                added += 1;
            }
        }

        let mut article_ids: HashSet<i64> = self.articles.iter().map(|a| a.id).collect();
        for article in other.articles {
            if article.id == 0 || article_ids.insert(article.id) {
                self.articles.push(article);
                added += 1;
            }
        }

        added
    }

    /// 综合排序保持接口顺序；其余排序只作用于视频分组。
    pub fn sort_videos(&mut self, sort: SearchSort) {
        match sort {
            SearchSort::Total => {}
            SearchSort::Play => self.videos.sort_by(|a, b| b.view_count.cmp(&a.view_count)),
            SearchSort::Newest => self.videos.sort_by(|a, b| b.pubdate.cmp(&a.pubdate)),
            SearchSort::Danmaku => self
                .videos
                .sort_by(|a, b| b.danmaku_count.cmp(&a.danmaku_count)),
            SearchSort::Favorite => self
                .videos
                .sort_by(|a, b| b.favorite_count.cmp(&a.favorite_count)),
        }
    }
}

fn video_key(video: &SearchVideo) -> String {
    if !video.bvid.trim().is_empty() {
        video.bvid.trim().to_string()
    } else if video.aid > 0 {
        format!("av{}", video.aid)
    } else {
        String::new()
    }
}

/// 一页搜索结果 + 分页信息。
#[derive(Debug, Clone, Default)]
pub struct SearchPage {
    pub results: SearchResults,
    pub page: u32,
    pub num_pages: u32,
    pub num_results: u64,
}

// ================= 请求 =================

/// 结果请求（公开接口，不需要 WBI）。
pub fn search_all(keyword: &str, page: u32, cookie: &str) -> bilibili::Request {
    let term =
        percent_encoding::utf8_percent_encode(keyword.trim(), percent_encoding::NON_ALPHANUMERIC);
    bilibili::Request {
        url: format!(
            "{}/x/web-interface/search/all/v2?keyword={term}&page={}&page_size=20",
            bilibili::API_BASE,
            page.max(1)
        ),
        cookie: cookie.to_string(),
        name: "search_all",
    }
}

/// 联想词请求。
pub fn search_suggest(term: &str) -> bilibili::Request {
    let term =
        percent_encoding::utf8_percent_encode(term.trim(), percent_encoding::NON_ALPHANUMERIC);
    bilibili::Request {
        url: format!(
            "https://s.search.bilibili.com/main/suggest?term={term}&main_ver=v1&highlight="
        ),
        cookie: String::new(),
        name: "search_suggest",
    }
}

/// 热搜请求。
pub fn search_hotword() -> bilibili::Request {
    bilibili::Request {
        url: "https://s.search.bilibili.com/main/hotword".to_string(),
        cookie: String::new(),
        name: "search_hotword",
    }
}

// ================= 拉取 =================

/// 拉一页搜索结果。
pub fn fetch_search_all<C>(
    client: &C,
    keyword: &str,
    page: u32,
    cookie: &str,
) -> Result<SearchPage, String>
where
    C: HttpClient<Error = String>,
{
    if keyword.trim().is_empty() {
        return Err("搜索关键词不能为空".into());
    }
    let body = client.get(&search_all(keyword, page, cookie))?;
    parse_search_page(&body)
}

/// 拉联想词。失败时返回错误，由 UI 保留历史 / 热搜并显示「联想不可用」。
pub fn fetch_suggest<C>(client: &C, term: &str) -> Result<Vec<SuggestItem>, String>
where
    C: HttpClient<Error = String>,
{
    if term.trim().is_empty() {
        return Ok(Vec::new());
    }
    let body = client.get(&search_suggest(term))?;
    parse_suggest(&body)
}

/// 拉热搜。
pub fn fetch_hotwords<C>(client: &C) -> Result<Vec<Hotword>, String>
where
    C: HttpClient<Error = String>,
{
    let body = client.get(&search_hotword())?;
    parse_hotwords(&body)
}

// ================= 解析 =================

/// 解析 `all/v2` 响应。
pub fn parse_search_page(body: &str) -> Result<SearchPage, String> {
    let root: Value = serde_json::from_str(body).map_err(|e| format!("搜索响应解析失败：{e}"))?;
    let code = root.get("code").and_then(Value::as_i64).unwrap_or(-1);
    if code != 0 {
        let message = str_of(&root, "message");
        return Err(format!(
            "搜索接口返回 code={code}{}",
            if message.is_empty() {
                String::new()
            } else {
                format!("（{message}）")
            }
        ));
    }
    let data = root
        .get("data")
        .ok_or_else(|| "搜索响应缺少 data".to_string())?;
    let mut page = SearchPage {
        page: num(data, "page").max(1) as u32,
        num_pages: num(data, "numPages") as u32,
        num_results: num(data, "numResults"),
        ..Default::default()
    };
    let groups = data
        .get("result")
        .and_then(Value::as_array)
        .cloned()
        .unwrap_or_default();

    for group in groups {
        let result_type = str_of(&group, "result_type");
        let items = group
            .get("data")
            .and_then(Value::as_array)
            .cloned()
            .unwrap_or_default();
        match result_type.as_str() {
            "video" | "live_room" => {
                for item in items {
                    // 视频分组里会混进「直播中」条目，按 type 拆开。
                    if str_of(&item, "type") == "live_room" {
                        page.results.live.push(parse_live(&item));
                    } else {
                        page.results.videos.push(parse_video(&item));
                    }
                }
            }
            "media_bangumi" => {
                for item in items {
                    page.results.bangumi.push(parse_media(&item, false));
                }
            }
            "media_ft" => {
                for item in items {
                    page.results.films.push(parse_media(&item, true));
                }
            }
            "bili_user" => {
                for item in items {
                    page.results.users.push(parse_user(&item));
                }
            }
            "article" => {
                for item in items {
                    page.results.articles.push(parse_article(&item));
                }
            }
            _ => {}
        }
    }

    if page.num_pages == 0 && !page.results.is_empty() {
        page.num_pages = page.page;
    }
    Ok(page)
}

/// 解析联想词响应。
pub fn parse_suggest(body: &str) -> Result<Vec<SuggestItem>, String> {
    let root: Value = serde_json::from_str(body).map_err(|e| format!("联想词解析失败：{e}"))?;
    let code = root.get("code").and_then(Value::as_i64).unwrap_or(0);
    if code != 0 {
        return Err(format!("联想词接口返回 code={code}"));
    }
    let tags = root
        .get("result")
        .and_then(|r| r.get("tag"))
        .and_then(Value::as_array)
        .cloned()
        .unwrap_or_default();
    let mut out = Vec::with_capacity(tags.len());
    for tag in tags.iter().take(10) {
        let value = str_of(tag, "value");
        let name = str_of(tag, "name");
        if value.trim().is_empty() && name.trim().is_empty() {
            continue;
        }
        let mut spans = parse_highlight_spans(&name);
        let mut plain: String = spans.iter().map(|s| s.text.as_str()).collect();
        // 极少数响应只有 value 没有 name：直接把 value 当展示文本。
        if plain.trim().is_empty() {
            plain = value.clone();
            spans = vec![HighlightSpan {
                text: plain.clone(),
                highlighted: false,
            }];
        }
        out.push(SuggestItem {
            value: if value.trim().is_empty() {
                plain
            } else {
                value
            },
            spans,
        });
    }
    Ok(out)
}

/// 解析热搜响应。
pub fn parse_hotwords(body: &str) -> Result<Vec<Hotword>, String> {
    let root: Value = serde_json::from_str(body).map_err(|e| format!("热搜解析失败：{e}"))?;
    let code = root.get("code").and_then(Value::as_i64).unwrap_or(0);
    if code != 0 {
        return Err(format!("热搜接口返回 code={code}"));
    }
    let list = root
        .get("list")
        .and_then(Value::as_array)
        .cloned()
        .unwrap_or_default();
    Ok(list
        .iter()
        .take(10)
        .enumerate()
        .map(|(i, item)| {
            let pos = num(item, "pos") as u32;
            Hotword {
                keyword: str_of(item, "keyword"),
                pos: if pos == 0 { i as u32 + 1 } else { pos },
                word_type: num(item, "word_type") as u32,
            }
        })
        .filter(|h| !h.keyword.trim().is_empty())
        .collect())
}

/// 把 `<em class="suggest_high_light">test</em>flight` 切成
/// `[("test", true), ("flight", false)]`。其他标签一律丢弃。
pub fn parse_highlight_spans(input: &str) -> Vec<HighlightSpan> {
    let mut spans: Vec<HighlightSpan> = Vec::new();
    let mut buf = String::new();
    let mut highlighted = false;
    let mut rest = input;

    while !rest.is_empty() {
        if let Some(start) = rest.find('<') {
            if start > 0 {
                buf.push_str(&rest[..start]);
            }
            let after = &rest[start..];
            let Some(end) = after.find('>') else {
                // 不完整的标签：按普通文本处理，避免吞掉内容。
                buf.push_str(after);
                break;
            };
            let tag = &after[..=end];
            let closing = tag.starts_with("</");
            let is_em = tag.contains("em") || tag.contains("EM");
            if is_em {
                push_span(&mut spans, &mut buf, highlighted);
                highlighted = !closing;
            }
            rest = &after[end + 1..];
        } else {
            buf.push_str(rest);
            rest = "";
        }
    }
    push_span(&mut spans, &mut buf, highlighted);

    if spans.is_empty() {
        spans.push(HighlightSpan {
            text: String::new(),
            highlighted: false,
        });
    }
    spans
}

fn push_span(spans: &mut Vec<HighlightSpan>, buf: &mut String, highlighted: bool) {
    if buf.is_empty() {
        return;
    }
    let text = format::html_decode(buf);
    buf.clear();
    if let Some(last) = spans.last_mut() {
        if last.highlighted == highlighted {
            last.text.push_str(&text);
            return;
        }
    }
    spans.push(HighlightSpan { text, highlighted });
}

fn parse_video(item: &Value) -> SearchVideo {
    // UP 主头像在搜索结果里是 `upic`（`pic` 是封面）。少数条目只给 `uface`，
    // 两个都接，避免卡片署名行退回渐变占位。
    let up_face = {
        let upic = image_url(&str_of(item, "upic"));
        if upic.is_empty() {
            image_url(&str_of(item, "uface"))
        } else {
            upic
        }
    };
    SearchVideo {
        bvid: str_of(item, "bvid"),
        aid: num(item, "aid"),
        title: clean_text(&str_of(item, "title")),
        description: clean_text(&str_of(item, "description")),
        author: clean_text(&str_of(item, "author")),
        mid: num(item, "mid") as i64,
        up_face,
        cover_url: image_url(&str_of(item, "pic")),
        view_count: num(item, "play"),
        danmaku_count: num(item, "video_review").max(num(item, "danmaku")),
        favorite_count: num(item, "favorites"),
        duration_secs: parse_duration(&str_of(item, "duration")),
        pubdate: num(item, "pubdate"),
        typename: clean_text(&str_of(item, "typename")),
        is_live: false,
    }
}

fn parse_user(item: &Value) -> SearchUser {
    let verify = {
        let direct = str_of(item, "verify_info");
        if direct.trim().is_empty() {
            item.get("official_verify")
                .map(|v| str_of(v, "desc"))
                .unwrap_or_default()
        } else {
            direct
        }
    };
    let previews = item
        .get("res")
        .and_then(Value::as_array)
        .map(|items| items.iter().take(3).map(parse_video).collect())
        .unwrap_or_default();
    SearchUser {
        mid: num(item, "mid") as i64,
        name: clean_text(&str_of(item, "uname")),
        sign: clean_text(&str_of(item, "usign")),
        avatar_url: image_url(&str_of(item, "upic")),
        fans: num(item, "fans"),
        videos: num(item, "videos"),
        level: num(item, "level") as u32,
        verify: clean_text(&verify),
        previews,
    }
}

fn parse_media(item: &Value, is_film: bool) -> SearchMedia {
    let episodes = item
        .get("eps")
        .and_then(Value::as_array)
        .map(|eps| {
            eps.iter()
                .map(|ep| SearchEpisode {
                    id: num(ep, "id"),
                    index_title: clean_text(&str_of(ep, "index_title")),
                    long_title: clean_text(&str_of(ep, "long_title")),
                    cover_url: image_url(&str_of(ep, "cover")),
                })
                .collect::<Vec<_>>()
        })
        .unwrap_or_default();
    let cover = {
        let own = image_url(&str_of(item, "cover"));
        if own.is_empty() {
            episodes
                .first()
                .map(|ep| ep.cover_url.clone())
                .unwrap_or_default()
        } else {
            own
        }
    };
    let (score, score_count) = item
        .get("media_score")
        .map(|m| (m.get("score").and_then(Value::as_f64), num(m, "user_count")))
        .unwrap_or((None, 0));
    SearchMedia {
        media_id: num(item, "media_id") as i64,
        season_id: num(item, "season_id") as i64,
        title: clean_text(&str_of(item, "title")),
        original_title: clean_text(&str_of(item, "org_title")),
        cover_url: cover,
        areas: clean_text(&str_of(item, "areas")),
        styles: clean_text(&str_of(item, "styles")),
        index_show: clean_text(&str_of(item, "index_show")),
        description: clean_text(&str_of(item, "desc")),
        season_type_name: clean_text(&str_of(item, "season_type_name")),
        score: score.filter(|s| *s > 0.0),
        score_count,
        episodes,
        is_film,
    }
}

fn parse_live(item: &Value) -> SearchLive {
    SearchLive {
        roomid: num(item, "roomid") as i64,
        title: clean_text(&str_of(item, "title")),
        uname: clean_text(&str_of(item, "uname")),
        cover_url: image_url(&str_of(item, "cover")),
        online: num(item, "online"),
        area_name: clean_text(&str_of(item, "area_name")),
    }
}

fn parse_article(item: &Value) -> SearchArticle {
    let mut cover = str_of(item, "image_urls");
    if cover.trim().is_empty() {
        cover = str_of(item, "pic");
    }
    SearchArticle {
        id: num(item, "id") as i64,
        title: clean_text(&str_of(item, "title")),
        description: clean_text(&str_of(item, "desc")),
        author: clean_text(&str_of(item, "author")),
        view: num(item, "view"),
        like: num(item, "like"),
        cover_url: image_url(&cover),
    }
}

/// 去标签 + 解 HTML 实体。
pub fn clean_text(input: &str) -> String {
    let mut out = String::with_capacity(input.len());
    let mut rest = input;
    while let Some(start) = rest.find('<') {
        out.push_str(&rest[..start]);
        let after = &rest[start..];
        match after.find('>') {
            Some(end) => rest = &after[end + 1..],
            None => {
                rest = "";
                break;
            }
        }
    }
    out.push_str(rest);
    format::html_decode(&out).trim().to_string()
}

/// 图床地址归一化：`//i0.hdslb.com/x.jpg` → `https://i0.hdslb.com/x.jpg`。
pub fn image_url(url: &str) -> String {
    let url = url.trim();
    if url.is_empty() {
        return String::new();
    }
    if let Some(rest) = url.strip_prefix("//") {
        return format!("https://{rest}");
    }
    if let Some(rest) = url.strip_prefix("http://") {
        if rest.contains("hdslb.com") {
            return format!("https://{rest}");
        }
    }
    url.to_string()
}

/// `"11:7"` / `"1:02:03"` → 秒。
pub fn parse_duration(text: &str) -> u64 {
    let mut total = 0u64;
    let mut parts = 0usize;
    for part in text.trim().split(':') {
        let Ok(value) = part.trim().parse::<u64>() else {
            return 0;
        };
        total = total.saturating_mul(60).saturating_add(value);
        parts += 1;
    }
    if parts == 0 {
        0
    } else {
        total
    }
}

fn str_of(value: &Value, key: &str) -> String {
    value
        .get(key)
        .map(|v| match v {
            Value::String(s) => s.clone(),
            Value::Null => String::new(),
            other => other.to_string(),
        })
        .unwrap_or_default()
}

fn num(value: &Value, key: &str) -> u64 {
    num_value(value.get(key))
}

fn num_value(value: Option<&Value>) -> u64 {
    match value {
        Some(Value::Number(n)) => n.as_u64().unwrap_or(0),
        Some(Value::String(s)) => s.trim().parse::<u64>().unwrap_or(0),
        _ => 0,
    }
}

// ================= 本地搜索历史 =================

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct SearchHistoryEntry {
    pub term: String,
    pub at: u64,
}

#[derive(Debug, Clone, Default, Serialize, Deserialize)]
struct SearchHistoryFile {
    #[serde(default)]
    entries: Vec<SearchHistoryEntry>,
}

/// 本地搜索历史：最多 8 条，按时间倒序；每次改动立即落盘。
#[derive(Debug, Clone)]
pub struct SearchHistory {
    path: PathBuf,
    entries: Vec<SearchHistoryEntry>,
}

/// 历史上限（设计文档 12.1.1）。
pub const HISTORY_LIMIT: usize = 8;

impl SearchHistory {
    /// 默认路径：macOS 写 `~/Library/Application Support/YKPili/search_history.json`，
    /// 测试 / 验收可用 `YK_SEARCH_HISTORY_PATH` 覆盖。
    pub fn default_path() -> PathBuf {
        if let Ok(path) = std::env::var("YK_SEARCH_HISTORY_PATH") {
            if !path.trim().is_empty() {
                return PathBuf::from(path);
            }
        }
        #[cfg(target_os = "macos")]
        {
            if let Ok(home) = std::env::var("HOME") {
                return PathBuf::from(home)
                    .join("Library/Application Support/YKPili/search_history.json");
            }
        }
        std::env::temp_dir()
            .join("YKPili")
            .join("search_history.json")
    }

    pub fn load_default() -> Self {
        Self::load(Self::default_path())
    }

    pub fn load(path: impl Into<PathBuf>) -> Self {
        let path = path.into();
        let file = fs::read_to_string(&path)
            .ok()
            .and_then(|s| serde_json::from_str::<SearchHistoryFile>(&s).ok())
            .unwrap_or_default();
        let mut history = Self {
            path,
            entries: file.entries,
        };
        history.entries.truncate(HISTORY_LIMIT);
        history
    }

    pub fn entries(&self) -> &[SearchHistoryEntry] {
        &self.entries
    }

    pub fn terms(&self) -> Vec<String> {
        self.entries.iter().map(|e| e.term.clone()).collect()
    }

    /// 记一条；已存在时提到最前，超出上限按时间淘汰。
    pub fn push(&mut self, term: &str) -> bool {
        let term = term.trim();
        if term.is_empty() {
            return false;
        }
        let before = self.entries.clone();
        self.entries.retain(|e| e.term != term);
        self.entries.insert(
            0,
            SearchHistoryEntry {
                term: term.to_string(),
                at: now_secs(),
            },
        );
        self.entries.truncate(HISTORY_LIMIT);
        if self.entries == before {
            return false;
        }
        let _ = self.save();
        true
    }

    pub fn remove(&mut self, term: &str) -> bool {
        let before = self.entries.len();
        self.entries.retain(|e| e.term != term);
        let changed = self.entries.len() != before;
        if changed {
            let _ = self.save();
        }
        changed
    }

    pub fn clear(&mut self) -> bool {
        if self.entries.is_empty() {
            return false;
        }
        self.entries.clear();
        let _ = self.save();
        true
    }

    pub fn save(&self) -> io::Result<()> {
        if let Some(parent) = self.path.parent() {
            fs::create_dir_all(parent)?;
        }
        let file = SearchHistoryFile {
            entries: self.entries.clone(),
        };
        let json = serde_json::to_string_pretty(&file)
            .map_err(|e| io::Error::new(io::ErrorKind::Other, e))?;
        fs::write(&self.path, json)
    }

    pub fn path(&self) -> &Path {
        &self.path
    }
}

fn now_secs() -> u64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|d| d.as_secs())
        .unwrap_or(0)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn requests_encode_keyword_and_keep_cookie() {
        let req = search_all("原神 test", 2, "SESSDATA=x");
        assert!(req.url.contains("/x/web-interface/search/all/v2"));
        assert!(req.url.contains("page=2"));
        assert!(req.url.contains("%E5%8E%9F%E7%A5%9E"));
        assert!(!req.url.contains("原神"));
        assert_eq!(req.cookie, "SESSDATA=x");
        assert_eq!(req.name, "search_all");
    }

    #[test]
    fn suggest_request_does_not_send_cookie() {
        let req = search_suggest("test");
        assert!(req.url.contains("term=test"));
        assert!(req.url.contains("main_ver=v1"));
        assert!(req.cookie.is_empty());
        assert_eq!(req.name, "search_suggest");
    }

    #[test]
    fn highlight_spans_keep_em_ranges() {
        let spans =
            parse_highlight_spans(r#"<em class="suggest_high_light">test</em>flight邀请码"#);
        assert_eq!(spans.len(), 2);
        assert_eq!(spans[0].text, "test");
        assert!(spans[0].highlighted);
        assert_eq!(spans[1].text, "flight邀请码");
        assert!(!spans[1].highlighted);
    }

    #[test]
    fn highlight_spans_survive_multiple_em() {
        let spans = parse_highlight_spans("<em>原</em>神<em>攻略</em>");
        assert_eq!(
            spans,
            vec![
                HighlightSpan {
                    text: "原".into(),
                    highlighted: true
                },
                HighlightSpan {
                    text: "神".into(),
                    highlighted: false
                },
                HighlightSpan {
                    text: "攻略".into(),
                    highlighted: true
                }
            ]
        );
    }

    #[test]
    fn duration_parsing() {
        assert_eq!(parse_duration("11:7"), 667);
        assert_eq!(parse_duration("1:02:03"), 3723);
        assert_eq!(parse_duration(""), 0);
        assert_eq!(parse_duration("bad"), 0);
    }

    #[test]
    fn image_urls_are_normalized() {
        assert_eq!(
            image_url("//i0.hdslb.com/a.jpg"),
            "https://i0.hdslb.com/a.jpg"
        );
        assert_eq!(
            image_url("http://i0.hdslb.com/a.jpg"),
            "https://i0.hdslb.com/a.jpg"
        );
        assert_eq!(image_url("https://x/y.png"), "https://x/y.png");
    }

    #[test]
    fn clean_text_strips_em_and_entities() {
        assert_eq!(
            clean_text(r#"《<em class="keyword">原神</em>》&amp; 攻略"#),
            "《原神》& 攻略"
        );
    }

    #[test]
    fn search_page_parses_all_groups() {
        let body = r#"{
          "code": 0,
          "data": {
            "page": 1, "numPages": 3, "numResults": 58,
            "result": [
              {"result_type":"video","data":[
                {"type":"video","bvid":"BV1xx","aid":1,"title":"<em>原神</em>攻略",
                 "author":"UP","mid":9,"pic":"//i0.hdslb.com/a.jpg",
                 "upic":"//i0.hdslb.com/face.jpg","play":"12000",
                 "video_review":30,"favorites":5,"duration":"11:7","pubdate":1790000000,
                 "typename":"游戏","description":"desc"},
                {"type":"live_room","roomid":555,"title":"直播","uname":"主播",
                 "cover":"//i0.hdslb.com/l.jpg","online":88}
              ]},
              {"result_type":"media_ft","data":[
                {"media_id":2,"season_id":3,"title":"电影","cover":"//i0.hdslb.com/m.jpg",
                 "areas":"日本","styles":"动画","index_show":"全 1 集",
                 "button_text":"立即观看","media_score":{"score":9.6,"user_count":20},
                 "eps":[{"id":7,"index_title":"PV1","long_title":"预告","cover":"//i0.hdslb.com/e.jpg"}]}
              ]},
              {"result_type":"media_bangumi","data":[
                {"media_id":4,"season_id":5,"title":"番剧","cover":"//i0.hdslb.com/b.jpg",
                 "areas":"日本","styles":"奇幻","index_show":"全 12 集","eps":[]}
              ]},
              {"result_type":"bili_user","data":[
                {"mid":77,"uname":"原神","usign":"官方","upic":"//i0.hdslb.com/u.jpg",
                 "fans":100,"videos":2,"level":6,"verify_info":"bilibili 知名UP主",
                 "res":[{"bvid":"BV2yy","aid":2,"title":"投稿","pic":"//i0.hdslb.com/p.jpg",
                         "play":3,"duration":"01:00","pubdate":1790000000}]}
              ]},
              {"result_type":"article","data":[
                {"id":8,"title":"专栏","desc":"摘要","author":"作者","view":10,"like":2}
              ]}
            ]
          }
        }"#;
        let page = parse_search_page(body).unwrap();
        assert_eq!(page.page, 1);
        assert_eq!(page.num_pages, 3);
        assert_eq!(page.num_results, 58);
        assert_eq!(page.results.videos.len(), 1);
        assert_eq!(page.results.videos[0].title, "原神攻略");
        assert_eq!(page.results.videos[0].duration_secs, 667);
        // UP 主头像走 `upic`（`pic` 是封面），并且要带进首页卡片模型。
        assert_eq!(
            page.results.videos[0].up_face,
            "https://i0.hdslb.com/face.jpg"
        );
        assert_eq!(
            page.results.videos[0].to_cover_item(0).up_face,
            "https://i0.hdslb.com/face.jpg"
        );
        assert_eq!(page.results.live.len(), 1);
        assert_eq!(page.results.films.len(), 1);
        assert_eq!(page.results.films[0].score, Some(9.6));
        assert_eq!(page.results.films[0].episodes.len(), 1);
        assert_eq!(page.results.bangumi.len(), 1);
        assert_eq!(page.results.users.len(), 1);
        assert_eq!(page.results.users[0].previews.len(), 1);
        assert_eq!(page.results.articles.len(), 1);
        assert!(!page.results.is_empty());
    }

    #[test]
    fn video_avatar_falls_back_to_uface() {
        // 线上少数视频条目只给 `uface` 不给 `upic`；不能因此退回渐变占位。
        let body = r#"{
          "code": 0,
          "data": {
            "result": [
              {"result_type":"video","data":[
                {"type":"video","bvid":"BV1zz","title":"无 upic","author":"UP","mid":1,
                 "pic":"//i0.hdslb.com/c.jpg","uface":"//i1.hdslb.com/fallback.jpg",
                 "play":1,"duration":"01:00","pubdate":1790000000}
              ]}
            ]
          }
        }"#;
        let page = parse_search_page(body).unwrap();
        assert_eq!(
            page.results.videos[0].up_face,
            "https://i1.hdslb.com/fallback.jpg"
        );
        assert_eq!(
            page.results.videos[0].to_cover_item(0).up_face,
            "https://i1.hdslb.com/fallback.jpg"
        );
    }

    #[test]
    fn merge_dedupes_and_reports_added() {
        let mut a = SearchResults {
            videos: vec![SearchVideo {
                bvid: "BV1".into(),
                ..Default::default()
            }],
            users: vec![SearchUser {
                mid: 1,
                ..Default::default()
            }],
            ..Default::default()
        };
        let other = SearchResults {
            videos: vec![
                SearchVideo {
                    bvid: "BV1".into(),
                    ..Default::default()
                },
                SearchVideo {
                    bvid: "BV2".into(),
                    ..Default::default()
                },
            ],
            users: vec![
                SearchUser {
                    mid: 1,
                    ..Default::default()
                },
                SearchUser {
                    mid: 2,
                    ..Default::default()
                },
            ],
            ..Default::default()
        };
        assert_eq!(a.merge(other), 2);
        assert_eq!(a.videos.len(), 2);
        assert_eq!(a.users.len(), 2);
    }

    #[test]
    fn sorting_videos_by_metric() {
        let mut results = SearchResults {
            videos: vec![
                SearchVideo {
                    bvid: "a".into(),
                    view_count: 10,
                    pubdate: 3,
                    ..Default::default()
                },
                SearchVideo {
                    bvid: "b".into(),
                    view_count: 30,
                    pubdate: 1,
                    ..Default::default()
                },
                SearchVideo {
                    bvid: "c".into(),
                    view_count: 20,
                    pubdate: 2,
                    ..Default::default()
                },
            ],
            ..Default::default()
        };
        results.sort_videos(SearchSort::Play);
        assert_eq!(
            results
                .videos
                .iter()
                .map(|v| v.bvid.as_str())
                .collect::<Vec<_>>(),
            vec!["b", "c", "a"]
        );
        results.sort_videos(SearchSort::Newest);
        assert_eq!(
            results
                .videos
                .iter()
                .map(|v| v.bvid.as_str())
                .collect::<Vec<_>>(),
            vec!["a", "c", "b"]
        );
    }

    #[test]
    fn suggest_parser_caps_ten() {
        let tags = (0..15)
            .map(|i| format!(r#"{{"value":"v{i}","name":"<em>v</em>{i}"}}"#))
            .collect::<Vec<_>>()
            .join(",");
        let body = format!(r#"{{"code":0,"result":{{"tag":[{tags}]}}}}"#);
        let items = parse_suggest(&body).unwrap();
        assert_eq!(items.len(), 10);
        assert_eq!(items[0].value, "v0");
        assert_eq!(items[0].plain(), "v0");
    }

    #[test]
    fn suggest_parser_falls_back_to_value_without_name() {
        let body = r#"{"code":0,"result":{"tag":[{"value":"原神","name":""}]}}"#;
        let items = parse_suggest(body).unwrap();
        assert_eq!(items.len(), 1);
        assert_eq!(items[0].value, "原神");
        assert_eq!(items[0].plain(), "原神");
        assert_eq!(
            items[0].spans,
            vec![HighlightSpan {
                text: "原神".into(),
                highlighted: false
            }]
        );
    }

    #[test]
    fn hotword_parser_tags_words() {
        let body = r#"{"code":0,"list":[
            {"keyword":"甲","pos":1,"word_type":4},
            {"keyword":"乙","pos":2,"word_type":7}
        ]}"#;
        let list = parse_hotwords(body).unwrap();
        assert_eq!(list.len(), 2);
        assert_eq!(list[0].tag(), Some("新"));
        assert_eq!(list[1].tag(), Some("直播"));
    }

    #[test]
    fn history_dedupes_truncates_and_persists() {
        let path = std::env::temp_dir().join(format!(
            "ykpili-search-history-test-{}-{}.json",
            std::process::id(),
            now_secs()
        ));
        let _ = fs::remove_file(&path);
        let mut history = SearchHistory::load(&path);
        for i in 0..10 {
            history.push(&format!("词{i}"));
        }
        assert_eq!(history.entries().len(), HISTORY_LIMIT);
        assert_eq!(history.entries()[0].term, "词9");
        history.push("词8");
        assert_eq!(history.entries()[0].term, "词8");
        assert_eq!(history.entries().len(), HISTORY_LIMIT);
        history.remove("词8");
        assert!(!history.terms().contains(&"词8".to_string()));
        let reloaded = SearchHistory::load(&path);
        assert_eq!(reloaded.entries().len(), HISTORY_LIMIT - 1);
        history.clear();
        assert!(SearchHistory::load(&path).entries().is_empty());
        let _ = fs::remove_file(&path);
    }
}
