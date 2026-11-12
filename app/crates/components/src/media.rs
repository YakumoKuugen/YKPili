//! 通用远程图片：统一 `img()` 的裁剪、尺寸与空 URL 处理。
//!
//! 各页面不再各写一遍 `img(ImageSource::from(...)).object_fit(...)`。

use gpui::prelude::*;
use gpui::*;
use gpui::{img, px, AnyElement, App, Hsla, ImageSource, ObjectFit, Window};

/// 铺满父容器的封面图；`url` 为空时返回 `None`，由调用方保留占位层。
pub fn cover_image(url: &str) -> Option<AnyElement> {
    if url.trim().is_empty() {
        return None;
    }
    Some(
        img(ImageSource::from(url.to_string()))
            .absolute()
            .top_0()
            .left_0()
            .w_full()
            .h_full()
            .object_fit(ObjectFit::Cover)
            .into_any_element(),
    )
}

/// 固定尺寸的方形缩略图（头像 / 列表缩略图）。
pub fn square_image(url: &str, size: f32) -> Option<AnyElement> {
    if url.trim().is_empty() {
        return None;
    }
    Some(
        img(ImageSource::from(url.to_string()))
            .absolute()
            .top(px(0.))
            .left(px(0.))
            .w(px(size))
            .h(px(size))
            .object_fit(ObjectFit::Cover)
            .into_any_element(),
    )
}

/// 固定尺寸的图片（海报 / 封面格），不做绝对定位。
pub fn sized_image(url: &str, width: f32, height: f32) -> Option<AnyElement> {
    if url.trim().is_empty() {
        return None;
    }
    Some(
        img(ImageSource::from(url.to_string()))
            .w(px(width))
            .h(px(height))
            .object_fit(ObjectFit::Cover)
            .into_any_element(),
    )
}

/// 可点击的图片预览格：评论图片条、UP 主投稿、动态图文共用。
pub fn media_tile(
    id: impl Into<gpui::ElementId>,
    background: Hsla,
    url: String,
    width: f32,
    height: f32,
    radius: f32,
    on_click: Option<std::rc::Rc<dyn Fn(&mut Window, &mut App)>>,
) -> AnyElement {
    let tile = div()
        .id(id)
        .w(px(width))
        .h(px(height))
        .flex_shrink_0()
        .rounded(px(radius))
        .overflow_hidden()
        .bg(background)
        .child(
            img(ImageSource::from(url))
                .w(px(width))
                .h(px(height))
                .object_fit(ObjectFit::Cover),
        );
    match on_click {
        Some(on_click) => tile
            .cursor_pointer()
            .on_click(move |_, window, cx| {
                on_click(window, cx);
                cx.stop_propagation();
            })
            .into_any_element(),
        None => tile.into_any_element(),
    }
}

/// 撑满父宽度、固定高度的图片（横向列表缩略图 / 通栏封面）。
pub fn fill_image(url: &str, height: f32) -> Option<AnyElement> {
    if url.trim().is_empty() {
        return None;
    }
    Some(
        img(ImageSource::from(url.to_string()))
            .w_full()
            .h(px(height))
            .object_fit(ObjectFit::Cover)
            .into_any_element(),
    )
}
