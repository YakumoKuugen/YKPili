//! 关注流 / 动态列表的视图状态。
//!
//! 只是渲染用的只读快照：数据与加载逻辑仍在组装点。

use yk_infra::dynamic::DynamicItem;

pub struct FeedState<'a> {
    pub items: &'a [DynamicItem],
    pub loading: bool,
    pub error: Option<&'a str>,
    /// 是否已经成功拉过一页；用来区分「空列表」与「还没开始」。
    pub loaded: bool,
    pub has_more: bool,
    pub loading_more: bool,
    pub signed_in: bool,
}
