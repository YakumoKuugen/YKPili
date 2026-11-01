//! 评论区行的共用实现。
//!
//! 播放器右侧评论区与「正在关注」展开详情的评论区之前各写了一份：行高、
//! 字号、置顶徽标、右键复制、图片缩略图、文本选区很容易一边改了另一边忘了。
//! 这里把一整行抽成同一个组件，两处只传布局参数和动作回调。

use std::rc::Rc;

use gpui::assets::IconName;
use gpui::component::menu::{ContextMenuExt as _, PopupMenu, PopupMenuItem};
use gpui::component::Icon;
use gpui::prelude::*;
use gpui::TestSupportExt as _;
use gpui::*;
use yk_core::dynamic::DynamicComment;
use yk_core::format;
use yk_ui::status;
use yk_ui::theme::*;

/// 正文行高。播放器的窗口化列表用它算前缀和，详情页也按同一行高排版。
pub const COMMENT_TEXT_LINE_HEIGHT: f32 = 20.0;
/// 一条评论最多展示几张缩略图。
pub const COMMENT_MAX_THUMBS: usize = 4;
/// 缩略图边长。
pub const COMMENT_THUMB_SIZE: f32 = 72.0;
/// 缩略图条对行高的贡献：72 图 + 6 上间距 + 2 下间距。
pub const COMMENT_IMAGE_ROW_HEIGHT: f32 = 80.0;

/// 一行评论的布局与动作参数。
///
/// 播放器按「完整文本折行数」算好固定行高交给 `row_height`（窗口化列表要定高），
/// 详情页传 `None` 让正文自然撑高。两处共用同一套头像、置顶徽标、右键菜单、
/// 图片与时间行。
pub struct CommentRowOptions {
    /// 正文列宽度。必须给确定值，`SelectableText` 才会按这个宽度折行。
    pub text_width: f32,
    /// 固定行高；`None` 表示由内容撑开。
    pub row_height: Option<f32>,
    /// 行上下内边距。
    pub padding_y: f32,
    /// 正文节点 id；详情页的测试会按它定位评论。
    pub text_id: ElementId,
    /// 复制成功后的轻提示回调。
    pub copy_notice: Rc<dyn Fn(&mut App)>,
    /// 点评论图片的回调：整组 URL + 当前下标。
    pub on_image: Option<Rc<dyn Fn(Vec<String>, usize, &mut Window, &mut App)>>,
}

/// 渲染一条评论。调用方负责外面那层虚拟列表 / 滚动容器。
pub fn comment_row(t: &YkTheme, comment: &DynamicComment, opts: CommentRowOptions) -> Div {
    let s = &t.surfaces;
    let p = &t.palette;
    let text = comment.text.clone();
    let menu_text = text.clone();

    let mut row = div()
        .w_full()
        // 详情页的评论列表在滚动容器里；不锁住 shrink 的话，评论一多，
        // flex 会把每一行压回容器高度，文字就溢出到评论区外面。
        .flex_shrink_0()
        .flex()
        .items_start()
        .gap(px(SPACE_2))
        .py(px(opts.padding_y))
        .border_b_1()
        .border_color(s.border_subtle);
    if let Some(height) = opts.row_height {
        row = row.h(px(height));
    }

    let mut content = div()
        .w(px(opts.text_width))
        .flex_shrink_0()
        .flex()
        .flex_col()
        .gap(px(2.))
        .child(
            div()
                .h(px(16.))
                .flex()
                .items_center()
                .gap(px(4.))
                .when(comment.is_top, |el| el.child(status::pinned_badge(t)))
                .child(
                    div()
                        .min_w(px(0.))
                        .flex_1()
                        .text_size(px(FONT_SIZE_CAPTION))
                        .line_height(px(16.))
                        .text_color(s.text_tertiary)
                        .truncate()
                        .child(comment.author.clone()),
                ),
        );

    // 这里不能裁切：`SelectableText` 的拖选自动滚动会把最近一层裁切当成
    // 可视区，裁到正文高度后左右拖动会一直触发滚动。
    let text_box = div()
        .id(opts.text_id)
        .test_support()
        .flex_shrink_0()
        .child(
            gpui::base::SelectableText::new(("comment-select", comment.rpid.max(0) as usize), text)
                .text_style(gpui::TextStyleRefinement {
                    font_size: Some(px(FONT_SIZE_CONTROL).into()),
                    line_height: Some(px(COMMENT_TEXT_LINE_HEIGHT).into()),
                    color: Some(s.text_primary),
                    ..Default::default()
                }),
        )
        // 右键菜单：有选区复制选区，没有则复制整条评论。
        .context_menu(text_copy_menu(menu_text, opts.copy_notice));
    content = content.child(text_box);

    if !comment.pictures.is_empty() {
        let urls: Vec<String> = comment
            .pictures
            .iter()
            .map(|picture| picture.detail_url())
            .collect();
        let mut strip = div()
            .mt(px(6.))
            .h(px(COMMENT_THUMB_SIZE))
            .flex()
            .gap(px(6.))
            .overflow_hidden();
        for (index, picture) in comment.pictures.iter().take(COMMENT_MAX_THUMBS).enumerate() {
            let mut tile = div()
                .id(("comment-image", (comment.rpid.max(0) as usize) << 4 | index))
                .w(px(COMMENT_THUMB_SIZE))
                .h(px(COMMENT_THUMB_SIZE))
                .flex_shrink_0()
                .rounded(px(RADIUS_MD))
                .overflow_hidden()
                .bg(s.fill_3)
                .child(
                    img(ImageSource::from(picture.thumb_url(144)))
                        .w(px(COMMENT_THUMB_SIZE))
                        .h(px(COMMENT_THUMB_SIZE))
                        .object_fit(ObjectFit::Cover),
                );
            if let Some(on_image) = opts.on_image.as_ref() {
                let on_image = on_image.clone();
                let urls = urls.clone();
                tile = tile.cursor_pointer().on_click(move |_, window, cx| {
                    on_image(urls.clone(), index, window, cx);
                    cx.stop_propagation();
                });
            }
            strip = strip.child(tile);
        }
        content = content.child(strip);
    }

    content = content.child(
        div()
            .h(px(14.))
            .flex()
            .items_center()
            .gap(px(SPACE_3))
            .text_size(px(FONT_SIZE_MICRO))
            .line_height(px(14.))
            .text_color(s.text_quaternary)
            .child(comment.time.clone())
            .when(comment.like > 0, |el| {
                el.child(
                    div()
                        .flex()
                        .items_center()
                        .gap(px(2.))
                        .child(Icon::new(IconName::ThumbsUp).size(px(11.)))
                        .child(format::count(comment.like)),
                )
            }),
    );

    row.child(
        div()
            .w(px(28.))
            .h(px(28.))
            .flex_shrink_0()
            .rounded_full()
            .overflow_hidden()
            .bg(linear_gradient(
                135.,
                linear_color_stop(p.bili_blue, 0.0),
                linear_color_stop(p.bili_pink, 1.0),
            ))
            .when(!comment.face.is_empty(), |el| {
                el.child(
                    img(ImageSource::from(comment.face.clone()))
                        .w(px(28.))
                        .h(px(28.))
                        .object_fit(ObjectFit::Cover),
                )
            }),
    )
    .child(content)
}

/// 任意文本的右键复制菜单：有选区复制选区，没有则复制整段。
pub fn text_copy_menu(
    text: String,
    notice: Rc<dyn Fn(&mut App)>,
) -> impl Fn(PopupMenu, &mut Window, &mut Context<PopupMenu>) -> PopupMenu + 'static {
    move |menu, window, cx| {
        // 必须在菜单打开的这一拍就把选区取走：点菜单项属于「文本外点击」，
        // 会在 on_click 之前把窗口选区清空，事后再读就只能拿到空串。
        let selected = gpui::base::TextSelection::selected_text(window, cx);
        let value = if selected.trim().is_empty() {
            text.clone()
        } else {
            selected
        };
        let notice = notice.clone();
        menu.item(
            PopupMenuItem::new("复制文本")
                .icon(Icon::new(IconName::Copy))
                .on_click(move |_, _, cx| {
                    if value.trim().is_empty() {
                        return;
                    }
                    cx.write_to_clipboard(ClipboardItem::new_string(value.clone()));
                    notice(cx);
                }),
        )
    }
}
