//! M5 页面扩展：热门 / 分区 / 排行 / 直播 / 我的 / 设置，外加搜索。
//!
//! 设计原则（对齐第 05 节与第 12 节）：
//! - **每页四态**：加载中 / 有内容 / 空 / 错误。四态不是「以后补」，是页面的一部分。
//! - 页面只负责内容区，顶栏、分类条、Dock 属于外壳（`AppRoot`），不在这里重复。
//! - 颜色一律走 `YkTheme`，不写字面量。

use gpui::prelude::*;
use gpui::*;
use yk_ui::status;
use yk_ui::theme::*;

/// 页面标识。也充当最小路由。
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum PageKind {
    Hot,
    Category,
    Rank,
    Live,
    /// 关注动态：底部 Dock 的「正在关注」入口。
    Following,
    /// 观看历史：底部 Dock 的「历史」动作。
    History,
    /// 消息中心：回复 / @ / 赞 / 私信四通道。
    Messages,
    /// 收藏夹：接口未接入，先保留 Dock 入口与选中态。
    Favorites,
    Mine,
    Settings,
    Search,
}

impl PageKind {
    pub fn title(self) -> &'static str {
        match self {
            Self::Hot => "热门",
            Self::Category => "分区",
            Self::Rank => "视频排行榜",
            Self::Live => "直播",
            Self::Following => "正在关注",
            Self::History => "观看历史",
            Self::Messages => "消息中心",
            Self::Favorites => "我的收藏",
            Self::Mine => "我的",
            Self::Settings => "设置",
            Self::Search => "搜索结果",
        }
    }
}

/// 页面四态。
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum PageState {
    Loading,
    Ready,
    Empty,
    Error,
}

pub fn render(
    theme: &YkTheme,
    page: PageKind,
    state: PageState,
    signed_in: bool,
) -> impl IntoElement {
    let s = &theme.surfaces;
    let mut col = div()
        .flex()
        .flex_col()
        .gap(px(SPACE_4))
        .w_full()
        .child(header(theme, page))
        .child(match state {
            PageState::Loading => status::skeleton(theme, 6).into_any_element(),
            PageState::Empty => status::empty(
                theme,
                &format!("这里还没有{}内容", page.title()),
                "换个分类或稍后再试",
            )
            .into_any_element(),
            PageState::Error => {
                status::error(theme, "加载失败", "检查网络后重试").into_any_element()
            }
            PageState::Ready => body(theme, page, signed_in).into_any_element(),
        });
    // 内容区背景用页面底色，避免与顶栏/分类条同色糊成一片
    col = col.bg(s.bg_base);
    col
}

fn header(theme: &YkTheme, page: PageKind) -> impl IntoElement {
    let s = &theme.surfaces;
    div()
        .flex()
        .items_center()
        .justify_between()
        .w_full()
        .child(
            div()
                .text_size(px(FONT_SIZE_HEADING))
                .font_weight(FontWeight::SEMIBOLD)
                .text_color(s.text_primary)
                .child(page.title()),
        )
}

// ================= 四态 =================

// ================= 各页面正文 =================

fn body(theme: &YkTheme, page: PageKind, signed_in: bool) -> AnyElement {
    match page {
        PageKind::Hot => hot(theme).into_any_element(),
        PageKind::Category => category(theme).into_any_element(),
        PageKind::Rank => rank(theme).into_any_element(),
        PageKind::Live => live(theme).into_any_element(),
        PageKind::Following => following(theme, signed_in),
        // 历史页是独立的两栏视图，由 AppRoot 在 content_area 里特判，
        // 不会走到这里；保留分支只为让 match 穷尽。
        PageKind::History => {
            status::empty(theme, "观看历史", "由历史页独立渲染").into_any_element()
        }
        // 消息页自带分栏与虚拟列表，由 AppRoot 在 content_area 里特判。
        PageKind::Messages => {
            status::empty(theme, "消息中心", "由消息页独立渲染").into_any_element()
        }
        PageKind::Favorites => status::empty(
            theme,
            "收藏夹功能尚未接入",
            "收藏接口接入后，收藏的视频会显示在这里",
        )
        .into_any_element(),
        PageKind::Mine => mine(theme).into_any_element(),
        PageKind::Settings => settings(theme).into_any_element(),
        PageKind::Search => search(theme).into_any_element(),
    }
}

const HOT_TITLES: [&str; 8] = [
    "生肉烹饪师和他的猫：一顿饭的成本到底有多少",
    "一碗甜子的深夜食堂 · 第 12 期",
    "班长captain 的极限运动合集",
    "用 Rust 从零写一个视频播放器",
    "当我把整个 B 站的热门都下载下来",
    "三分钟看懂 GPUI 的渲染管线",
    "这个夏天最值得去的十个地方",
    "为什么你的代码总是越写越乱",
];

fn hot(theme: &YkTheme) -> impl IntoElement {
    let s = &theme.surfaces;
    let p = &theme.palette;
    let mut list = div().flex().flex_col().gap(px(SPACE_2)).w_full();
    for (i, t) in HOT_TITLES.iter().enumerate() {
        list = list.child(
            div()
                .flex()
                .items_center()
                .gap(px(SPACE_3))
                .h(px(64.))
                .px(px(SPACE_3))
                .rounded(px(CARD_RADIUS))
                .bg(s.surface_content)
                .border_1()
                .border_color(s.border_subtle)
                .child(
                    div()
                        .w(px(24.))
                        .text_size(px(FONT_SIZE_BODY))
                        .font_weight(FontWeight::BOLD)
                        .text_color(if i < 3 {
                            p.status_error
                        } else {
                            s.text_quaternary
                        })
                        .child(format!("{}", i + 1)),
                )
                .child(
                    div()
                        .flex_1()
                        .text_size(px(FONT_SIZE_CONTROL))
                        .text_color(s.text_primary)
                        .line_clamp(1)
                        .child(*t),
                )
                .child(
                    div()
                        .text_size(px(FONT_SIZE_MICRO))
                        .text_color(s.text_tertiary)
                        .child(yk_core::format::count(120_000 - i as u64 * 9_000)),
                ),
        );
    }
    list
}

const CATEGORIES: [(&str, &str); 8] = [
    ("动画", "番剧 · 国创 · 鬼畜"),
    ("音乐", "原创 · 翻唱 · VOCALOID"),
    ("游戏", "单机 · 网游 · 手游"),
    ("知识", "科学 · 社科 · 技能"),
    ("生活", "日常 · 美食 · 旅行"),
    ("影视", "电影 · 剧集 · 纪录片"),
    ("科技", "数码 · 计算机 · 机械"),
    ("舞蹈", "宅舞 · 街舞 · 明星"),
];

fn category(theme: &YkTheme) -> impl IntoElement {
    let s = &theme.surfaces;
    let mut grid = div().flex().flex_wrap().gap(px(SPACE_3)).w_full();
    for (name, desc) in CATEGORIES {
        grid = grid.child(
            div()
                .flex()
                .flex_col()
                .gap(px(SPACE_1))
                .w(px(200.))
                .p(px(SPACE_4))
                .rounded(px(CARD_RADIUS))
                .bg(s.surface_content)
                .border_1()
                .border_color(s.border_subtle)
                .child(
                    div()
                        .text_size(px(FONT_SIZE_BODY))
                        .font_weight(FontWeight::MEDIUM)
                        .text_color(s.text_primary)
                        .child(name),
                )
                .child(
                    div()
                        .text_size(px(FONT_SIZE_MICRO))
                        .text_color(s.text_tertiary)
                        .child(desc),
                ),
        );
    }
    grid
}

fn rank(theme: &YkTheme) -> impl IntoElement {
    // 排行接口（全站榜 / 分区榜）尚未接入；这里不放占位榜单，
    // 避免用户把假数据当成真实排行。
    status::empty(
        theme,
        "排行榜功能尚未接入",
        "排行接口接入后，全站榜与分区榜会显示在这里",
    )
}

fn chip(theme: &YkTheme, label: &str, active: bool) -> impl IntoElement {
    let s = &theme.surfaces;
    let p = &theme.palette;
    div()
        .px(px(SPACE_3))
        .h(px(28.))
        .flex()
        .items_center()
        .rounded(px(RADIUS_FULL))
        .bg(if active { p.theme_primary_20 } else { s.fill_1 })
        .text_size(px(FONT_SIZE_CONTROL))
        .text_color(if active {
            p.theme_primary
        } else {
            s.text_secondary
        })
        .child(label.to_string())
}

fn live(theme: &YkTheme) -> impl IntoElement {
    let s = &theme.surfaces;
    let p = &theme.palette;
    let mut grid = div().flex().flex_wrap().gap(px(SPACE_3)).w_full();
    for i in 0..8 {
        grid = grid.child(
            div()
                .flex()
                .flex_col()
                .gap(px(SPACE_1))
                .w(px(COVER_CELL_WIDTH))
                .rounded(px(CARD_RADIUS))
                .overflow_hidden()
                .bg(s.surface_content)
                .border_1()
                .border_color(s.border_subtle)
                .child(
                    div()
                        .relative()
                        .w_full()
                        .h(px(COVER_ITEM_HEIGHT))
                        .bg(s.fill_3)
                        .child(
                            div()
                                .absolute()
                                .top(px(4.))
                                .left(px(4.))
                                .px(px(4.))
                                .rounded(px(RADIUS_SM))
                                .bg(p.status_error)
                                .text_size(px(FONT_SIZE_MICRO))
                                .text_color(rgb(0xFFFFFF))
                                .child("直播中"),
                        ),
                )
                .child(
                    div()
                        .p(px(SPACE_2))
                        .text_size(px(FONT_SIZE_CONTROL))
                        .text_color(s.text_primary)
                        .line_clamp(1)
                        .child(format!("直播间 · 第 {} 场", i + 1)),
                ),
        );
    }
    grid
}

const FOLLOWING_UPDATES: [(&str, &str, &str, &str); 5] = [
    (
        "生",
        "生肉烹饪师",
        "发布了新视频：一顿饭的成本到底有多少",
        "12 分钟前",
    ),
    (
        "甜",
        "一碗甜子",
        "发布了新视频：深夜食堂 · 第 12 期",
        "1 小时前",
    ),
    (
        "班",
        "班长captain",
        "发布了 3 张图片 · 极限运动合集",
        "3 小时前",
    ),
    (
        "明",
        "技术宅小明",
        "发布了新视频：用 Rust 从零写一个播放器",
        "昨天",
    ),
    (
        "猫",
        "猫猫观察日记",
        "转发了动态：这个夏天最值得去的十个地方",
        "昨天",
    ),
];

/// 关注动态：未登录只给登录引导，登录后展示关注 UP 主的更新流。
fn following(theme: &YkTheme, signed_in: bool) -> AnyElement {
    let s = &theme.surfaces;
    let p = &theme.palette;

    if !signed_in {
        return status::empty(
            theme,
            "登录后查看关注动态",
            "关注 UP 主后，TA 的新视频和动态会出现在这里",
        )
        .into_any_element();
    }

    let mut list = div().flex().flex_col().gap(px(SPACE_2)).w_full();
    for (avatar, up, text, time) in FOLLOWING_UPDATES {
        list = list.child(
            div()
                .flex()
                .items_center()
                .gap(px(SPACE_3))
                .p(px(SPACE_3))
                .rounded(px(CARD_RADIUS))
                .bg(s.surface_content)
                .border_1()
                .border_color(s.border_subtle)
                .child(
                    div()
                        .w(px(40.))
                        .h(px(40.))
                        .rounded_full()
                        .bg(p.theme_primary_20)
                        .flex()
                        .items_center()
                        .justify_center()
                        .text_size(px(FONT_SIZE_BODY))
                        .font_weight(FontWeight::SEMIBOLD)
                        .text_color(p.theme_primary)
                        .child(avatar.to_string()),
                )
                .child(
                    div()
                        .flex_1()
                        .min_w_0()
                        .flex()
                        .flex_col()
                        .gap(px(2.))
                        .child(
                            div()
                                .text_size(px(FONT_SIZE_CONTROL))
                                .font_weight(FontWeight::MEDIUM)
                                .text_color(s.text_primary)
                                .child(up.to_string()),
                        )
                        .child(
                            div()
                                .text_size(px(FONT_SIZE_CAPTION))
                                .text_color(s.text_tertiary)
                                .line_clamp(1)
                                .child(text.to_string()),
                        ),
                )
                .child(
                    div()
                        .text_size(px(FONT_SIZE_MICRO))
                        .text_color(s.text_quaternary)
                        .child(time.to_string()),
                ),
        );
    }

    div()
        .flex()
        .flex_col()
        .gap(px(SPACE_3))
        .w_full()
        .child(
            div()
                .flex()
                .items_center()
                .gap(px(SPACE_2))
                .child(chip(theme, "全部动态", true))
                .child(chip(theme, "新视频", false))
                .child(chip(theme, "图文动态", false))
                .child(div().flex_1())
                .child(
                    div()
                        .text_size(px(FONT_SIZE_MICRO))
                        .text_color(s.text_tertiary)
                        .child("按发布时间排序"),
                ),
        )
        .child(list)
        .into_any_element()
}

fn mine(theme: &YkTheme) -> impl IntoElement {
    let s = &theme.surfaces;
    let p = &theme.palette;
    div()
        .flex()
        .flex_col()
        .gap(px(SPACE_4))
        .w_full()
        .child(
            div()
                .flex()
                .items_center()
                .gap(px(SPACE_4))
                .p(px(SPACE_4))
                .rounded(px(CARD_RADIUS))
                .bg(s.surface_content)
                .border_1()
                .border_color(s.border_subtle)
                .child(
                    div()
                        .w(px(64.))
                        .h(px(64.))
                        .rounded(px(32.))
                        .bg(p.theme_primary_20)
                        .border_1()
                        .border_color(s.border_popover),
                )
                .child(
                    div()
                        .flex()
                        .flex_col()
                        .gap(px(SPACE_1))
                        .child(
                            div()
                                .text_size(px(FONT_SIZE_BODY))
                                .font_weight(FontWeight::SEMIBOLD)
                                .text_color(s.text_primary)
                                .child("未登录"),
                        )
                        .child(
                            div()
                                .text_size(px(FONT_SIZE_CAPTION))
                                .text_color(s.text_tertiary)
                                .child("登录后同步历史、收藏与关注"),
                        ),
                ),
        )
        .child(div().flex().gap(px(SPACE_3)).children(
            ["观看历史", "我的收藏", "关注动态", "稍后再看"].map(|label| {
                div()
                    .flex_1()
                    .h(px(88.))
                    .flex()
                    .items_center()
                    .justify_center()
                    .rounded(px(CARD_RADIUS))
                    .bg(s.surface_content)
                    .border_1()
                    .border_color(s.border_subtle)
                    .text_size(px(FONT_SIZE_CONTROL))
                    .text_color(s.text_secondary)
                    .child(label)
            }),
        ))
}

const SETTINGS_GROUPS: [(&str, [&str; 3]); 3] = [
    ("播放", ["默认清晰度", "自动连播", "记忆播放进度"]),
    ("外观", ["主题跟随系统", "卡片密度", "右侧信息面板默认展开"]),
    ("其它", ["开机自启", "检查更新", "清除缓存"]),
];

fn settings(theme: &YkTheme) -> impl IntoElement {
    let s = &theme.surfaces;
    let mut col = div().flex().flex_col().gap(px(SPACE_4)).w_full();
    for (group, rows) in SETTINGS_GROUPS {
        let mut card = div()
            .flex()
            .flex_col()
            .w_full()
            .rounded(px(CARD_RADIUS))
            .overflow_hidden()
            .bg(s.surface_content)
            .border_1()
            .border_color(s.border_subtle)
            .child(
                div()
                    .px(px(SPACE_4))
                    .py(px(SPACE_2))
                    .text_size(px(FONT_SIZE_CAPTION))
                    .text_color(s.text_tertiary)
                    .child(group),
            );
        for row in rows {
            card = card.child(
                div()
                    .flex()
                    .items_center()
                    .justify_between()
                    .px(px(SPACE_4))
                    .h(px(48.))
                    .border_t_1()
                    .border_color(s.border_subtle)
                    .child(
                        div()
                            .text_size(px(FONT_SIZE_CONTROL))
                            .text_color(s.text_primary)
                            .child(row),
                    )
                    .child(
                        div()
                            .text_size(px(FONT_SIZE_MICRO))
                            .text_color(s.text_quaternary)
                            .child("›"),
                    ),
            );
        }
        col = col.child(card);
    }
    col
}

const SEARCH_HISTORY: [&str; 5] = [
    "GPUI 布局性能",
    "Rust 视频播放",
    "B 站 API",
    "弹幕渲染",
    "材质设计",
];

fn search(theme: &YkTheme) -> impl IntoElement {
    let s = &theme.surfaces;
    div()
        .flex()
        .flex_col()
        .gap(px(SPACE_4))
        .w_full()
        .child(
            div()
                .flex()
                .items_center()
                .h(px(CONTROL_HEIGHT))
                .px(px(SPACE_3))
                .rounded(px(INTERACTIVE_RADIUS))
                .bg(s.fill_1)
                .border_1()
                .border_color(s.border_subtle)
                .text_size(px(FONT_SIZE_CONTROL))
                .text_color(s.text_tertiary)
                .child("搜索视频、番剧、UP 主…"),
        )
        .child(
            div()
                .flex()
                .flex_col()
                .gap(px(SPACE_2))
                .child(
                    div()
                        .text_size(px(FONT_SIZE_CAPTION))
                        .text_color(s.text_tertiary)
                        .child("搜索历史"),
                )
                .child(
                    div()
                        .flex()
                        .flex_wrap()
                        .gap(px(SPACE_2))
                        .children(SEARCH_HISTORY.map(|h| chip(theme, h, false))),
                ),
        )
}
