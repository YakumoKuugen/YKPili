//! 评论区：播放器信息面板与动态详情共用**同一份实现**。
//!
//! 之前两边各写一套（分页、底部状态、回到顶部、输入条），行为逐渐分叉。
//! 现在统一成 [`Section`]：
//! - 数据与分页（游标 / 去重 / 到底 / 失败重试）在 [`CommentList`]；
//! - 列表是同一套窗口化渲染（[`CommentRows`] 前缀和，只建可见行）；
//! - 「回到顶部」、浮动输入条（点圆按钮展开）、拖选 / 右键复制都是同一份代码；
//! - 宿主只需要实现 [`CommentHost`]（把 `Section` 借出来），再把
//!   [`Section::body`] 放进自己的滚动容器、[`Section::composer`] /
//!   [`Section::back_to_top`] 挂在容器右下角。

use std::cell::Cell;
use std::rc::Rc;
use std::time::Duration;

use gpui::component::input::{Input, InputEvent, InputState};
use gpui::prelude::*;
use gpui::TestSupportExt as _;
// 不要 `use gpui::*`：它会带进 `gpui::test` 宏，和 `#[test]` 属性同名，
// 测试模块里会展开成递归（recursion limit reached while expanding `#[test]`）。
use gpui::{
    canvas, div, px, rgb, Animation, AnimationExt, AnyElement, App, ClickEvent, Context, Div,
    DispatchPhase, ElementId, Focusable, FontWeight, Hsla, Interpolate, IntoElement, MouseButton,
    MouseDownEvent, ParentElement, Point, Pixels, ScrollHandle, Styled, Subscription, Window,
};
use yk_infra::api::client::{
    fetch_comment_page, post_comment, CommentPage, ReqwestClient, COMMENT_PAGE_SIZE,
};
use yk_infra::dynamic::{CommentTarget, DynamicComment};
use yk_theme::theme::*;

use crate::comment_view::{CommentRowOptions, COMMENT_IMAGE_ROW_HEIGHT, COMMENT_TEXT_LINE_HEIGHT};

/// 每页条数：一次只发 1 个请求，剩下的滚到底再取。
pub const PAGE_SIZE: u32 = COMMENT_PAGE_SIZE;
/// 距底部多近开始补页。
pub const LOAD_MORE_GAP: f32 = 240.0;
/// 评论行里除正文之外的高度：上下 padding 16 + 作者行 16 + 两个 2px 间距。
pub const ROW_CHROME: f32 = 50.0;
/// 窗口化渲染上下各多建的高度。
pub const OVERSCAN: f32 = 1200.0;
/// 两次评论请求之间的最小间隔：连续触发会被频控成 `-352`。
const REQUEST_INTERVAL: Duration = Duration::from_millis(1000);

/// 浮动输入条收起态直径。
const COMPOSER_FAB: f32 = 40.0;
/// 输入条高度（展开 / 收起同高）。
const COMPOSER_HEIGHT: f32 = 40.0;
/// 被「回到顶部」顶起的距离：按钮 34 + 间距 8。
const COMPOSER_LIFT: f32 = 34.0 + SPACE_2;
/// 展开 / 收起动画时长。
const TOGGLE_MS: u64 = 180;
/// 顶起 / 落回动画时长。
const LIFT_MS: u64 = 200;

/// 滚过这个距离才浮出「回到顶部」。
pub const BACK_TO_TOP_THRESHOLD: f32 = 24.0;
/// 已经浮出后，滚回到这个距离以内才收回（两档阈值做迟滞）。
pub const BACK_TO_TOP_HIDE_THRESHOLD: f32 = 8.0;

/// 评论区宿主：只是把 `Section` 借出来，其余全在共享实现里。
pub trait CommentHost: Sized + 'static {
    fn comments(&mut self) -> &mut Section;
    /// 发送成功 / 失败时的轻提示（两边各用自己的 toast）。
    fn comment_notice(&mut self, text: String, error: bool, cx: &mut Context<Self>);
    /// 收起输入条后把焦点还给页面根（默认什么都不做）。
    fn comment_focus_root(&mut self, _window: &mut Window, _cx: &mut Context<Self>) {}
}

// ================= 数据与分页 =================

/// 评论列表的共享状态：游标翻页 / 去重 / 到底判断。
#[derive(Debug, Clone, Default)]
pub struct CommentList {
    pub items: Vec<DynamicComment>,
    /// 下一页游标（`cursor.next`）。
    pub next: u64,
    /// 服务端说「真的没有更多了」。
    pub is_end: bool,
    /// 正在拉下一页。
    pub loading_more: bool,
    /// 下一页失败原因：非空时底部显示「重试」，不自动重试。
    pub load_error: Option<String>,
    /// 评论总数（`cursor.all_count`）。
    pub total: u64,
}

impl CommentList {
    /// 清空（换视频 / 重新打开详情）。
    pub fn reset(&mut self) {
        *self = Self::default();
    }

    /// 第一页回来：整表替换。`available = false` 表示这个目标没有评论区。
    pub fn apply_first(&mut self, page: CommentPage, available: bool) {
        self.items = page.items;
        self.next = page.next;
        self.is_end = page.is_end || !available;
        self.total = page.total;
        self.loading_more = false;
        self.load_error = None;
    }

    /// 下一页回来：追加去重；游标没前进且没有新条目 = 服务端在重复同一页，收手。
    pub fn apply_more(&mut self, page: CommentPage) {
        let cursor = self.next;
        let added = yk_infra::dynamic::merge_comments(&mut self.items, page.items);
        let stalled = added == 0 && page.next == cursor;
        self.next = page.next;
        self.is_end = page.is_end || stalled;
        if page.total > 0 {
            self.total = page.total;
        }
        self.loading_more = false;
        self.load_error = None;
    }

    /// 拉下一页失败：保留已有内容，底部给「重试」。
    pub fn fail_more(&mut self, error: String) {
        self.loading_more = false;
        self.load_error = Some(error);
    }

    /// 发评论成功：插到列表顶部并让总数 +1。
    pub fn insert_sent(&mut self, comment: DynamicComment) {
        self.items.retain(|item| item.rpid != comment.rpid);
        self.items.insert(0, comment);
        self.total = self.total.saturating_add(1);
    }

    /// 现在该不该补下一页。
    pub fn needs_more(&self, offset_y: f32, max_offset_y: f32, viewport_h: f32) -> bool {
        if self.is_end || self.loading_more || self.load_error.is_some() {
            return false;
        }
        let near_bottom = max_offset_y <= -1.0 && offset_y - max_offset_y <= LOAD_MORE_GAP;
        let short = viewport_h > 0.0 && max_offset_y > -1.0;
        near_bottom || short
    }
}

// ================= 行高与窗口化 =================

/// 每条评论的固定行高 + 前缀和，只建可见行。
#[derive(Default)]
pub struct CommentRows {
    heights: Vec<f32>,
    offsets: Vec<f32>,
    width: f32,
}

impl CommentRows {
    pub fn total(&self) -> f32 {
        *self.offsets.last().unwrap_or(&0.0)
    }

    /// 保证缓存覆盖 `items`（正文宽度变了就整表重算）。
    pub fn ensure(
        &mut self,
        items: &[DynamicComment],
        width: f32,
        mut lines_of: impl FnMut(&str) -> usize,
    ) {
        if (self.width - width).abs() > 0.5 {
            self.width = width;
            self.heights.clear();
            self.offsets.clear();
        }
        if self.heights.len() > items.len() {
            self.heights.truncate(items.len());
            self.offsets.truncate(self.heights.len() + 1);
        }
        if self.offsets.is_empty() {
            self.offsets.push(0.0);
        }
        for comment in &items[self.heights.len()..] {
            let lines = lines_of(&comment.text).max(1);
            let image_extra = if comment.pictures.is_empty() {
                0.0
            } else {
                COMMENT_IMAGE_ROW_HEIGHT
            };
            let height = ROW_CHROME + COMMENT_TEXT_LINE_HEIGHT * lines as f32 + image_extra;
            let top = *self.offsets.last().unwrap_or(&0.0);
            self.heights.push(height);
            self.offsets.push(top + height);
        }
    }

    /// `y`（相对评论列表顶部）处的第一条可见行。
    pub fn first_row_at(&self, y: f32) -> usize {
        let rows = self.heights.len();
        let mut lo = 0usize;
        let mut hi = rows;
        while lo < hi {
            let mid = (lo + hi) / 2;
            if self.offsets[mid + 1] <= y {
                lo = mid + 1;
            } else {
                hi = mid;
            }
        }
        lo.min(rows)
    }

    pub fn offset_of(&self, index: usize) -> f32 {
        self.offsets.get(index).copied().unwrap_or(0.0)
    }

    pub fn height_of(&self, index: usize) -> f32 {
        self.heights.get(index).copied().unwrap_or(ROW_CHROME)
    }
}

/// 评论正文可用宽度：容器宽 − 左右 padding 32 − 头像 28 − 间距 8。
pub fn text_width(container_width: f32) -> f32 {
    container_width - SPACE_4 * 2.0 - 28.0 - SPACE_2
}

/// 评论正文在 `width` 内折成的完整行数（与渲染同字号、同宽度）。
pub fn wrapped_lines(window: &mut Window, text: &str, width: f32) -> usize {
    if text.trim().is_empty() || width <= 0.0 {
        return 1;
    }
    let run = gpui::TextRun {
        len: text.len(),
        // 必须用窗口当前字体（主题里的 PingFang / 雅黑）；默认字体与渲染
        // 字体不一致时折行数会差一截，定高行就会压字或留白。
        font: window.text_style().font(),
        ..Default::default()
    };
    match window.text_system().shape_text(
        text.to_string().into(),
        px(FONT_SIZE_CONTROL),
        &[run],
        Some(px(width)),
        None,
    ) {
        // `shape_text` 返回的是段落（按 `\n` 切），折行数要看每段的
        // `wrap_boundaries`：段内折一次就多一行。
        Ok(paragraphs) => paragraphs
            .iter()
            .map(|line| line.wrap_boundaries.len() + 1)
            .sum::<usize>()
            .max(1),
        Err(_) => 1,
    }
}

// ================= 共享评论区 =================

/// 评论区完整的共享状态：列表 + 输入条 + 回到顶部。
pub struct Section {
    pub list: CommentList,
    pub rows: CommentRows,
    /// 最近一帧实际创建的行数（验收指标）。
    pub rendered_rows: usize,
    pub input: Option<gpui::Entity<InputState>>,
    subscription: Option<Subscription>,
    pub sending: bool,
    pub focused: bool,
    /// 输入条是否展开成输入框。
    pub expanded: bool,
    toggle_generation: u64,
    toggle_animating: bool,
    /// 输入条是否被「回到顶部」顶起。
    pub lifted: bool,
    lift_generation: u64,
    lift_animating: bool,
    /// 发送成功后的回弹代次。
    pub pulse: u64,
    /// 刚发送的评论 rpid（列表里做进场动画）。
    pub sent_rpid: Option<i64>,
    /// 输入条在窗口坐标里的范围，点外部收起时用。
    pub bounds: Rc<Cell<Option<gpui::Bounds<Pixels>>>>,
    /// 「回到顶部」是否浮出（迟滞状态）。
    pub back_to_top: bool,
    /// 评论目标与 Cookie：读 / 写都在这里用。
    pub target: Option<CommentTarget>,
    pub cookie: String,
    /// 上一次评论请求（节流）。
    last_request: Option<std::time::Instant>,
}

impl Default for Section {
    fn default() -> Self {
        Self {
            list: CommentList::default(),
            rows: CommentRows::default(),
            rendered_rows: 0,
            input: None,
            subscription: None,
            sending: false,
            focused: false,
            expanded: false,
            toggle_generation: 0,
            toggle_animating: false,
            lifted: false,
            lift_generation: 0,
            lift_animating: false,
            pulse: 0,
            sent_rpid: None,
            bounds: Rc::new(Cell::new(None)),
            back_to_top: false,
            target: None,
            cookie: String::new(),
            last_request: None,
        }
    }
}

impl Section {
    pub fn reset(&mut self) {
        *self = Self::default();
    }

    /// 换目标（换视频 / 打开另一条动态）。
    pub fn set_target(&mut self, target: Option<CommentTarget>, cookie: String) {
        self.target = target;
        self.cookie = cookie;
    }

    /// 第一页：清列表、发请求。
    pub fn load_first<T: CommentHost>(&mut self, cx: &mut Context<T>) {
        let Some(target) = self.target.clone() else {
            self.list.reset();
            self.list.is_end = true;
            return;
        };
        self.list.reset();
        self.rows = CommentRows::default();
        self.rendered_rows = 0;
        let cookie = self.cookie.clone();
        let wait = self.wait_time();
        self.note_request(wait);
        cx.notify();
        cx.spawn(async move |this, cx| {
            let result = cx
                .background_executor()
                .spawn(async move {
                    if !wait.is_zero() {
                        std::thread::sleep(wait);
                    }
                    let client = ReqwestClient::new().map_err(|e| e.to_string())?;
                    fetch_comment_page(&client, &cookie, &target, 0, PAGE_SIZE)
                })
                .await;
            let _ = this.update(cx, |view, cx| {
                match result {
                    Ok(page) => view.comments().list.apply_first(page, true),
                    Err(error) => {
                        eprintln!("[comments] 首批加载失败：{error}");
                        let section = view.comments();
                        section.list.load_error = Some(error);
                        section.list.is_end = true;
                    }
                }
                cx.notify();
            });
        })
        .detach();
    }

    /// 下一页（滚到底 / 点「加载更多」）。
    pub fn load_more<T: CommentHost>(&mut self, cx: &mut Context<T>) {
        let Some(target) = self.target.clone() else {
            self.list.is_end = true;
            return;
        };
        if self.list.loading_more || self.list.is_end {
            return;
        }
        let cursor = self.list.next;
        let cookie = self.cookie.clone();
        let wait = self.wait_time();
        self.list.loading_more = true;
        self.list.load_error = None;
        self.note_request(wait);
        cx.notify();
        cx.spawn(async move |this, cx| {
            let result = cx
                .background_executor()
                .spawn(async move {
                    if !wait.is_zero() {
                        std::thread::sleep(wait);
                    }
                    let client = ReqwestClient::new().map_err(|e| e.to_string())?;
                    fetch_comment_page(&client, &cookie, &target, cursor, PAGE_SIZE)
                })
                .await;
            let _ = this.update(cx, |view, cx| {
                match result {
                    Ok(page) => view.comments().list.apply_more(page),
                    Err(error) => {
                        eprintln!("[comments] 加载更多失败：{error}");
                        view.comments().list.fail_more(error);
                    }
                }
                cx.notify();
            });
        })
        .detach();
    }

    /// 滚到列表底部附近就补下一页（宿主在滚动事件 / 每帧调用）。
    pub fn maybe_load_more<T: CommentHost>(&mut self, scroll: &ScrollHandle, cx: &mut Context<T>) {
        if self.list.needs_more(
            scroll.offset().y.as_f32(),
            scroll.max_offset().y.as_f32(),
            scroll.bounds().size.height.as_f32(),
        ) {
            self.load_more(cx);
        }
    }

    fn wait_time(&mut self) -> Duration {
        let now = std::time::Instant::now();
        match self.last_request {
            Some(last) => (last + REQUEST_INTERVAL).saturating_duration_since(now),
            None => Duration::ZERO,
        }
    }

    fn note_request(&mut self, wait: Duration) {
        self.last_request = Some(std::time::Instant::now() + wait);
    }

    /// 输入框（懒创建）。Enter 直接发送。
    pub fn ensure_input<T: CommentHost>(
        &mut self,
        window: &mut Window,
        cx: &mut Context<T>,
    ) -> gpui::Entity<InputState> {
        if let Some(state) = self.input.as_ref() {
            return state.clone();
        }
        let state = cx.new(|cx| InputState::new(window, cx).placeholder("发一条友善的评论"));
        self.subscription = Some(cx.subscribe(
            &state,
            |view: &mut T, _state, event: &InputEvent, cx| match event {
                InputEvent::PressEnter {
                    secondary: false,
                    shift: false,
                } => view.comments().submit(cx),
                InputEvent::Focus => {
                    let section = view.comments();
                    section.focused = true;
                    section.expanded = true;
                    cx.notify();
                }
                InputEvent::Blur => {
                    view.comments().focused = false;
                    cx.notify();
                }
                _ => {}
            },
        ));
        self.input = Some(state.clone());
        state
    }

    /// 发送评论：成功清空输入框、插到列表顶部、收起输入条。
    pub fn submit<T: CommentHost>(&mut self, cx: &mut Context<T>) {
        if self.sending {
            return;
        }
        let Some(input) = self.input.clone() else {
            return;
        };
        let text = input.read(cx).value().trim().to_string();
        if text.is_empty() {
            return;
        }
        let Some(target) = self.target.clone() else {
            return;
        };
        let Some(session) = current_session() else {
            return;
        };
        if session.bili_jct.trim().is_empty() {
            return;
        }
        let cookie = session.cookie_header();
        let csrf = session.bili_jct.clone();
        let author = if session.nickname.trim().is_empty() {
            "我".to_string()
        } else {
            session.nickname.clone()
        };
        let face = session.face_url.clone();
        let sent_text = text.clone();
        let send_text = sent_text.clone();
        self.sending = true;
        cx.notify();
        cx.spawn(async move |this, cx| {
            let result = cx
                .background_executor()
                .spawn(async move {
                    let client = ReqwestClient::new().map_err(|e| e.to_string())?;
                    post_comment(&client, &cookie, &csrf, target.kind, &target.oid, &send_text)
                })
                .await;
            let _ = this.update_in(cx, |view, window, cx| {
                let section = view.comments();
                section.sending = false;
                match result {
                    Ok(rpid) => {
                        if let Some(input) = section.input.clone() {
                            input.update(cx, |state, cx| state.set_value("", window, cx));
                        }
                        let rpid = rpid.unwrap_or_else(|| -(section.list.items.len() as i64 + 1));
                        section.sent_rpid = Some(rpid);
                        section.pulse = section.pulse.wrapping_add(1);
                        section.list.insert_sent(DynamicComment {
                            rpid,
                            author,
                            face,
                            text: sent_text,
                            time: "刚刚".to_string(),
                            ..Default::default()
                        });
                        view.comment_notice("评论已发送".into(), false, cx);
                        view.comments().collapse(cx);
                    }
                    Err(error) => {
                        eprintln!("[comments] 发送失败：{error}");
                        view.comment_notice(format!("发送失败：{error}"), true, cx);
                    }
                }
                cx.notify();
            });
        })
        .detach();
    }

    /// 收起输入条。
    pub fn collapse<T: CommentHost>(&mut self, cx: &mut Context<T>) {
        if !self.expanded {
            return;
        }
        self.expanded = false;
        self.focused = false;
        self.bump_toggle(cx);
        cx.notify();
    }

    /// 点输入条外面的位置：不在输入条上就收起并返回 true。
    pub fn collapse_if_outside<T: CommentHost>(
        &mut self,
        position: Point<Pixels>,
        cx: &mut Context<T>,
    ) -> bool {
        if !self.expanded {
            return false;
        }
        if self
            .bounds
            .get()
            .is_some_and(|bounds| bounds.contains(&position))
        {
            return false;
        }
        self.expanded = false;
        self.focused = false;
        self.bump_toggle(cx);
        cx.notify();
        true
    }

    fn bump_toggle<T: CommentHost>(&mut self, cx: &mut Context<T>) {
        self.toggle_generation = self.toggle_generation.wrapping_add(1);
        self.toggle_animating = true;
        let generation = self.toggle_generation;
        cx.spawn(async move |this, cx| {
            cx.background_executor()
                .timer(Duration::from_millis(TOGGLE_MS + 80))
                .await;
            let _ = this.update(cx, |view, cx| {
                let section = view.comments();
                if section.toggle_generation == generation {
                    section.toggle_animating = false;
                    cx.notify();
                }
            });
        })
        .detach();
    }

    fn bump_lift<T: CommentHost>(&mut self, lifted: bool, cx: &mut Context<T>) {
        self.lifted = lifted;
        self.lift_generation = self.lift_generation.wrapping_add(1);
        self.lift_animating = true;
        let generation = self.lift_generation;
        cx.spawn(async move |this, cx| {
            cx.background_executor()
                .timer(Duration::from_millis(LIFT_MS + 80))
                .await;
            let _ = this.update(cx, |view, cx| {
                let section = view.comments();
                if section.lift_generation == generation {
                    section.lift_animating = false;
                    cx.notify();
                }
            });
        })
        .detach();
    }

    /// 用滚动位置同步「回到顶部是否浮出」。
    ///
    /// 输入条要不要让位、按钮画不画，都只看这一个状态；宿主不再各自算，
    /// 否则一处少让 46px、一处多让一次，就会叠在一起。
    pub fn sync_scroll(&mut self, scroll: &ScrollHandle) {
        self.back_to_top = back_to_top_visible(scroll.offset().y.as_f32(), self.back_to_top);
    }

    /// 「回到顶部」按钮（收起时返回 0 尺寸占位，保证子节点数量稳定）。
    ///
    /// 位置固定右下角；输入条被顶起后自然让出这一格。
    pub fn back_to_top<T: CommentHost>(
        &mut self,
        t: &YkTheme,
        scroll: &ScrollHandle,
        cx: &mut Context<T>,
    ) -> AnyElement {
        if !self.back_to_top {
            return div()
                .absolute()
                .right(px(SPACE_4))
                .bottom(px(SPACE_4))
                .w(px(0.))
                .h(px(0.))
                .into_any_element();
        }
        let handle = scroll.clone();
        back_to_top_button(
            t,
            "comment-back-to-top",
            0.0,
            cx.listener(move |_: &mut T, _: &ClickEvent, _window, cx| {
                handle.set_offset(gpui::point(px(0.), px(0.)));
                cx.notify();
            }),
        )
    }

    /// 列表本体：窗口化行 + 底部四态。宿主把它放进自己的滚动容器。
    pub fn body<T: CommentHost>(
        &mut self,
        t: &YkTheme,
        window: &mut Window,
        scroll: &ScrollHandle,
        text_width: f32,
        id_prefix: &'static str,
        copy_notice: Rc<dyn Fn(&mut App)>,
        on_image: Option<Rc<dyn Fn(Vec<String>, usize, &mut Window, &mut App)>>,
        cx: &mut Context<T>,
    ) -> AnyElement {
        {
            let items = &self.list.items;
            let rows = &mut self.rows;
            rows.ensure(items, text_width, |text| {
                wrapped_lines(window, text, text_width)
            });
        }
        let total_height = self.rows.total();
        let viewport = scroll.bounds().size.height.as_f32();
        let scrolled = -scroll.offset().y.as_f32();
        let lo = (scrolled - OVERSCAN).max(0.0);
        let hi = scrolled + viewport + OVERSCAN;
        let first = self.rows.first_row_at(lo);

        let mut stack = div().relative().w_full().h(px(total_height));
        let mut index = first;
        while index < self.list.items.len() && self.rows.offset_of(index) < hi {
            let top = self.rows.offset_of(index);
            let height = self.rows.height_of(index);
            let comment = &self.list.items[index];
            let sent_animation = (self.sent_rpid == Some(comment.rpid)).then_some(self.pulse);
            let row = crate::comment_view::comment_row(
                t,
                comment,
                CommentRowOptions {
                    text_width,
                    row_height: Some(height),
                    padding_y: SPACE_2,
                    text_id: ElementId::Name(format!("{id_prefix}-text-{}", comment.rpid).into()),
                    copy_notice: copy_notice.clone(),
                    on_image: on_image.clone(),
                },
            );
            let row = match sent_animation {
                Some(generation) => row
                    .with_animation(
                        format!("comment-sent-{generation}-{}", comment.rpid),
                        Animation::new(Duration::from_millis(360)).with_easing(ease_entrance()),
                        |el, delta| el.opacity(delta).mt(px(16.0 * (1.0 - delta))),
                    )
                    .into_any_element(),
                None => row.into_any_element(),
            };
            stack = stack.child(
                div()
                    .absolute()
                    .left_0()
                    .right_0()
                    .top(px(top))
                    .h(px(height))
                    .child(row),
            );
            index += 1;
        }
        self.rendered_rows = index.saturating_sub(first);

        let footer = footer(
            t,
            &self.list,
            id_prefix,
            cx.listener(|view: &mut T, _: &ClickEvent, _window, cx| {
                view.comments().list.load_error = None;
                view.comments().load_more(cx);
            }),
        );
        div()
            .w_full()
            .flex()
            .flex_col()
            // 拖选文字时选区只在鼠标事件里更新；没有这一层 notify，
            // 高亮要等到松手才出现。放组件里，两边自动一致。
            .on_mouse_move(cx.listener(|_: &mut T, _: &gpui::MouseMoveEvent, _, cx| {
                cx.notify();
            }))
            .child(stack)
            .child(footer)
            .into_any_element()
    }

    /// 浮动输入条（右下角圆按钮，点开成输入框；被「回到顶部」顶起）。
    pub fn composer<T: CommentHost>(
        &mut self,
        t: &YkTheme,
        window: &mut Window,
        cx: &mut Context<T>,
        width: f32,
    ) -> AnyElement {
        let lifted = self.back_to_top;
        let state = self.ensure_input(window, cx);
        let s = &t.surfaces;
        let p = &t.palette;
        let handle = state.read(cx).focus_handle(cx);
        let sending = self.sending;
        let expanded = self.expanded;

        if self.lifted != lifted {
            self.bump_lift(lifted, cx);
        }
        let target_width = if expanded {
            width - SPACE_4 * 2.0
        } else {
            COMPOSER_FAB
        };
        let fab_bg = p.theme_primary;
        let bar_bg = s.surface_elevated_strong;

        let body = if expanded {
            div()
                .w_full()
                .h_full()
                .flex()
                .items_center()
                .gap(px(SPACE_2))
                .child(
                    div()
                        .flex_1()
                        .min_w(px(0.))
                        .h_full()
                        .flex()
                        .items_center()
                        .key_context("Input")
                        .track_focus(&handle)
                        .on_mouse_down(
                            MouseButton::Left,
                            cx.listener({
                                let handle = handle.clone();
                                move |view: &mut T, _: &MouseDownEvent, window, cx| {
                                    if handle.is_focused(window) {
                                        return;
                                    }
                                    window.focus(&handle, cx);
                                    view.comments().focused = true;
                                    cx.notify();
                                }
                            }),
                        )
                        .child(
                            Input::new(&state)
                                .appearance(false)
                                .bordered(false)
                                .w_full()
                                .h_full(),
                        ),
                )
                .child(
                    div()
                        .id("comment-send")
                        .test_support()
                        .h(px(28.))
                        .px(px(SPACE_3))
                        .flex_shrink_0()
                        .flex()
                        .items_center()
                        .justify_center()
                        .rounded(px(RADIUS_MD))
                        .bg(p.theme_primary)
                        .text_size(px(FONT_SIZE_CAPTION))
                        .font_weight(FontWeight::SEMIBOLD)
                        .text_color(rgb(0xFFFFFF))
                        .cursor_pointer()
                        .hover(|style| style.opacity(0.9))
                        .when(sending, |el| el.opacity(0.55))
                        .child(if sending { "发送中…" } else { "发送" })
                        .on_click(cx.listener(|view: &mut T, _: &ClickEvent, _window, cx| {
                            view.comments().submit(cx);
                        })),
                )
                .into_any_element()
        } else {
            div()
                .id("comment-fab")
                .test_support()
                .w(px(COMPOSER_FAB))
                .h_full()
                .flex()
                .items_center()
                .justify_center()
                .cursor_pointer()
                .child(
                    gpui::component::Icon::new(gpui::assets::IconName::SquarePen)
                        .size(px(ICON_SIZE_SM + 2.))
                        .text_color(rgb(0xFFFFFF)),
                )
                .on_click(cx.listener({
                    let state = state.clone();
                    move |view: &mut T, _: &ClickEvent, window, cx| {
                        let section = view.comments();
                        section.expanded = true;
                        section.focused = true;
                        section.bump_toggle(cx);
                        let handle = state.read(cx).focus_handle(cx);
                        window.focus(&handle, cx);
                        cx.notify();
                    }
                }))
                .into_any_element()
        };

        let shell = div()
            .id("comment-composer")
            .test_support()
            .occlude()
            .w(px(target_width))
            .h(px(COMPOSER_HEIGHT))
            .flex()
            .items_center()
            .overflow_hidden()
            .border_1()
            .border_color(s.border_popover)
            .shadow(t.shadows.shadow_2.clone())
            .bg(if expanded { bar_bg } else { fab_bg })
            .rounded(px(if expanded {
                RADIUS_LG
            } else {
                COMPOSER_FAB / 2.0
            }))
            .when(expanded, |el| el.px(px(SPACE_3)))
            .when(!expanded, |el| {
                el.justify_center().hover(|style| style.opacity(0.9))
            })
            .child(body)
            .child({
                let cell = self.bounds.clone();
                canvas(
                    move |bounds, _, _| cell.set(Some(bounds)),
                    |_, _, _, _| {},
                )
                .absolute()
                .inset_0()
            })
            .child({
                // 窗口级监听只能在 paint 阶段注册（`canvas` 的 paint 回调）。
                // 开着的这段时间里，点输入条以外的任何地方都收起 —— 组件自带，
                // 宿主不用各写一遍（详情页之前就是漏了这段，打开后关不掉）。
                let weak = cx.entity().downgrade();
                let expanded = self.expanded;
                canvas(
                    |_, _, _| {},
                    move |_, _, window, cx| {
                        if !expanded {
                            return;
                        }
                        let weak = weak.clone();
                        window.on_mouse_event(
                            move |event: &MouseDownEvent, phase, window, cx| {
                                if phase != DispatchPhase::Bubble
                                    || event.button != MouseButton::Left
                                {
                                    return;
                                }
                                let _ = weak.update(cx, |view, cx| {
                                    let section = view.comments();
                                    if !section.expanded {
                                        return;
                                    }
                                    let inside = section
                                        .bounds
                                        .get()
                                        .is_some_and(|bounds| {
                                            bounds.contains(&event.position)
                                        });
                                    if inside {
                                        return;
                                    }
                                    section.collapse(cx);
                                    view.comment_focus_root(window, cx);
                                });
                            },
                        );
                    },
                )
                .absolute()
                .inset_0()
            });

        let generation = self.toggle_generation;
        let shell = if generation > 0 && self.toggle_animating {
            let from_width = if expanded {
                COMPOSER_FAB
            } else {
                width - SPACE_4 * 2.0
            };
            shell
                .with_animation(
                    format!("comment-toggle-{generation}"),
                    Animation::new(Duration::from_millis(TOGGLE_MS)).with_easing(ease_standard()),
                    move |el, delta| {
                        let phase = if expanded { delta } else { 1.0 - delta };
                        let w = from_width + (target_width - from_width) * delta;
                        let radius =
                            COMPOSER_FAB / 2.0 + (RADIUS_LG - COMPOSER_FAB / 2.0) * phase;
                        el.w(px(w))
                            .rounded(px(radius))
                            .bg(Hsla::interpolate(fab_bg, bar_bg, phase))
                    },
                )
                .into_any_element()
        } else {
            shell.into_any_element()
        };

        let composer = if self.pulse > 0 {
            div()
                .child(shell)
                .with_animation(
                    format!("comment-pulse-{}", self.pulse),
                    Animation::new(Duration::from_millis(320)).with_easing(ease_entrance()),
                    |el, delta| el.mt(px(-8.0 * (1.0 - delta))).opacity(0.65 + 0.35 * delta),
                )
                .into_any_element()
        } else {
            shell
        };

        let lift_target = if lifted { COMPOSER_LIFT } else { 0.0 };
        let lift_from = if lifted { 0.0 } else { COMPOSER_LIFT };
        let lift_generation = self.lift_generation;
        let lift = div()
            .id("comment-composer-lift")
            .absolute()
            .left(px(SPACE_4))
            .right(px(SPACE_4))
            .bottom(px(SPACE_4 + lift_target))
            .h(px(COMPOSER_HEIGHT))
            .flex()
            .justify_end()
            .child(composer);
        if lift_generation > 0 && self.lift_animating {
            lift.with_animation(
                format!("comment-lift-{lift_generation}"),
                Animation::new(Duration::from_millis(LIFT_MS)).with_easing(ease_standard()),
                move |el, delta| {
                    el.bottom(px(SPACE_4 + lift_from + (lift_target - lift_from) * delta))
                },
            )
            .into_any_element()
        } else {
            lift.into_any_element()
        }
    }
}

/// 当前登录会话；未登录 / 旧文件损坏返回 None。
pub fn current_session() -> Option<yk_domain::models::UserSession> {
    let path = yk_infra::session::session_path();
    yk_infra::session::load(&path).filter(|session| session.is_valid())
}

// ================= 组件 =================

/// 列表底部的四态：加载中 / 失败 + 重试 / 没有更多 / 加载更多。
pub fn footer(
    t: &YkTheme,
    list: &CommentList,
    id_prefix: &'static str,
    on_more: impl Fn(&ClickEvent, &mut Window, &mut App) + 'static,
) -> AnyElement {
    let s = &t.surfaces;
    if list.loading_more {
        return line(t, "正在加载更多评论…").into_any_element();
    }
    if let Some(error) = list.load_error.clone() {
        return div()
            .flex()
            .flex_col()
            .items_start()
            .gap(px(SPACE_2))
            .py(px(SPACE_3))
            .child(
                div()
                    .text_size(px(FONT_SIZE_MICRO))
                    .text_color(s.text_tertiary)
                    .child(format!("加载更多失败：{error}")),
            )
            .child(
                div()
                    .id(ElementId::Name(format!("{id_prefix}-retry").into()))
                    .h(px(28.))
                    .px(px(SPACE_3))
                    .flex()
                    .items_center()
                    .rounded_full()
                    .bg(s.fill_1)
                    .text_size(px(FONT_SIZE_MICRO))
                    .text_color(s.text_secondary)
                    .cursor_pointer()
                    .hover(|style| style.bg(s.fill_2))
                    .child("重试")
                    .on_click(on_more),
            )
            .into_any_element();
    }
    if list.is_end {
        return line(t, "没有更多评论").into_any_element();
    }
    div()
        .py(px(SPACE_3))
        .child(
            div()
                .id(ElementId::Name(format!("{id_prefix}-more").into()))
                .h(px(32.))
                .px(px(SPACE_4))
                .flex()
                .items_center()
                .justify_center()
                .rounded_full()
                .border_1()
                .border_color(s.border_subtle)
                .bg(s.fill_1)
                .text_size(px(FONT_SIZE_MICRO))
                .text_color(s.text_secondary)
                .cursor_pointer()
                .hover(|style| style.bg(s.fill_2))
                .child("加载更多评论")
                .on_click(on_more),
        )
        .into_any_element()
}

fn line(t: &YkTheme, text: &str) -> Div {
    div()
        .py(px(SPACE_4))
        .text_size(px(FONT_SIZE_MICRO))
        .text_color(t.surfaces.text_quaternary)
        .child(text.to_string())
}

/// 「回到顶部」是否应该浮出（两档阈值迟滞）。
pub fn back_to_top_visible(scroll_y: f32, currently_shown: bool) -> bool {
    let threshold = if currently_shown {
        BACK_TO_TOP_HIDE_THRESHOLD
    } else {
        BACK_TO_TOP_THRESHOLD
    };
    scroll_y < -threshold
}

/// 右下角「回到顶部」按钮。
pub fn back_to_top_button(
    t: &YkTheme,
    id: &'static str,
    bottom_offset: f32,
    on_click: impl Fn(&ClickEvent, &mut Window, &mut App) + 'static,
) -> AnyElement {
    let s = &t.surfaces;
    let mut shadow = t.shadows.shadow_2.clone();
    shadow.extend(t.shadows.edge_glow_1.clone());
    div()
        .id(id)
        .test_support()
        .absolute()
        .right(px(SPACE_4))
        .bottom(px(SPACE_4 + bottom_offset))
        .w(px(34.))
        .h(px(34.))
        .flex()
        .items_center()
        .justify_center()
        .rounded_full()
        .bg(s.surface_elevated_solid)
        .border_1()
        .border_color(s.border_popover)
        .shadow(shadow)
        .cursor_pointer()
        .hover(|style| style.bg(s.surface_elevated_solid_hover))
        .text_size(px(18.))
        .font_weight(FontWeight::MEDIUM)
        .text_color(s.text_secondary)
        .child("↑")
        .on_click(on_click)
        .into_any_element()
}

#[cfg(test)]
mod tests {
    use super::*;

    fn comment(rpid: i64) -> DynamicComment {
        DynamicComment {
            rpid,
            ..Default::default()
        }
    }

    fn page(items: Vec<DynamicComment>, next: u64, is_end: bool) -> CommentPage {
        CommentPage {
            items,
            next,
            is_end,
            total: 0,
        }
    }

    #[test]
    fn apply_first_replaces_and_flags_unavailable() {
        let mut list = CommentList::default();
        list.apply_first(page(vec![comment(1), comment(2)], 2, false), true);
        assert_eq!(list.items.len(), 2);
        assert_eq!(list.next, 2);
        assert!(!list.is_end);

        list.apply_first(page(vec![], 0, false), false);
        assert!(list.is_end);
    }

    #[test]
    fn apply_more_dedupes_and_stops_on_stalled_cursor() {
        let mut list = CommentList::default();
        list.apply_first(page(vec![comment(1), comment(2)], 1, false), true);
        list.apply_more(page(vec![comment(2), comment(3)], 2, false));
        assert_eq!(
            list.items.iter().map(|c| c.rpid).collect::<Vec<_>>(),
            vec![1, 2, 3]
        );
        assert_eq!(list.next, 2);

        list.apply_more(page(vec![comment(3)], 2, false));
        assert!(list.is_end);
    }

    #[test]
    fn fail_more_keeps_items_and_blocks_auto_load() {
        let mut list = CommentList::default();
        list.apply_first(page(vec![comment(1)], 1, false), true);
        list.loading_more = true;
        list.fail_more("boom".into());
        assert_eq!(list.items.len(), 1);
        assert!(!list.loading_more);
        assert_eq!(list.load_error.as_deref(), Some("boom"));
        assert!(!list.needs_more(0.0, -10_000.0, 600.0));
    }

    #[test]
    fn needs_more_near_bottom_or_short_content() {
        let mut list = CommentList::default();
        list.apply_first(page(vec![comment(1)], 1, false), true);
        assert!(!list.needs_more(0.0, -10_000.0, 650.0));
        assert!(list.needs_more(-9_800.0, -10_000.0, 650.0));
        assert!(list.needs_more(0.0, 0.0, 650.0));
        list.is_end = true;
        assert!(!list.needs_more(-9_800.0, -10_000.0, 650.0));
    }

    #[test]
    fn insert_sent_prepends_and_counts() {
        let mut list = CommentList::default();
        list.apply_first(page(vec![comment(1)], 1, false), true);
        list.total = 10;
        list.insert_sent(comment(99));
        assert_eq!(list.items.first().unwrap().rpid, 99);
        assert_eq!(list.total, 11);
        list.insert_sent(comment(99));
        assert_eq!(list.items.iter().filter(|c| c.rpid == 99).count(), 1);
    }

    #[test]
    fn back_to_top_uses_hysteresis() {
        assert!(BACK_TO_TOP_HIDE_THRESHOLD < BACK_TO_TOP_THRESHOLD);
        assert!(!back_to_top_visible(0.0, false));
        assert!(!back_to_top_visible(-(BACK_TO_TOP_THRESHOLD - 1.0), false));
        assert!(back_to_top_visible(-(BACK_TO_TOP_THRESHOLD + 1.0), false));
        assert!(back_to_top_visible(-(BACK_TO_TOP_THRESHOLD - 1.0), true));
        assert!(back_to_top_visible(
            -(BACK_TO_TOP_HIDE_THRESHOLD + 1.0),
            true
        ));
        assert!(!back_to_top_visible(
            -(BACK_TO_TOP_HIDE_THRESHOLD - 1.0),
            true
        ));
    }
}
