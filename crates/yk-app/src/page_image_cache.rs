//! 页面图片缓存：只保留「可见 + 一行缓冲」的卡片图。
//!
//! GPUI 默认的 `img()` 会把解码结果留在资源缓存里，**不会因为卡片滚出视口就释放**；
//! 而解码后的 BGRA 帧同时占 CPU 和 GPU 图集两份内存。这里用 GPUI 提供的
//! `ImageCache` 扩展点做一层页面级缓存：
//!
//! - 卡片按滚动位置决定是否挂 `img()`；
//! - 页面每帧把实际挂出来的 URL 写入 `ImageResidency::current`；
//! - 下一帧开始时，缓存里不在 keep 集合中的图片会被 `drop_image` 同时移出
//!   CPU 资源和窗口图集。
//!
//! 这样滚动时驻留量只与「可视卡片 + 一行缓冲」有关，而不是整条信息流。

use gpui::{
    App, AppContext, Entity, ImageCache, ImageCacheError, ImageCacheItem, RenderImage, Resource,
    Window,
};
use std::collections::{HashMap, HashSet};
use std::sync::Arc;

/// 页面图片缓存实体。
pub struct PageImageCache {
    items: HashMap<Resource, ImageCacheItem>,
}

impl PageImageCache {
    pub fn new(cx: &mut App) -> Entity<Self> {
        let cache = cx.new(|_| Self {
            items: HashMap::new(),
        });
        // 缓存实体被释放时把图集条目也带走；否则窗口重建后旧纹理可能一直留在
        // 图集里（`ImageCacheItem` 自身只负责取消加载，不会 drop_image）。
        cx.observe_release(&cache, |cache: &mut Self, cx| {
            for (_, item) in std::mem::take(&mut cache.items) {
                if let Some(Ok(image)) = item.get() {
                    cx.drop_image(image, None);
                }
            }
        })
        .detach();
        cache
    }

    /// 丢掉不在 `keep` 里的图片。
    ///
    /// `keep` 里是卡片实际挂出的 URL；`drop_image` 会同时移除 CPU 资源和
    /// 当前窗口的图集条目，避免只删资源缓存、GPU 纹理仍然驻留。
    pub fn prune(&mut self, keep: &HashSet<String>, window: &mut Window, cx: &mut App) {
        let evicted: Vec<Resource> = self
            .items
            .keys()
            .filter(|resource| match resource {
                Resource::Uri(uri) => !keep.contains(&uri.to_string()),
                // 卡片图只走 Uri；其它资源不归这里管。
                _ => false,
            })
            .cloned()
            .collect();

        for resource in evicted {
            if let Some(item) = self.items.remove(&resource) {
                if let Some(Ok(image)) = item.get() {
                    cx.drop_image(image, Some(window));
                }
            }
        }
    }
}

impl ImageCache for PageImageCache {
    fn load(
        &mut self,
        resource: &Resource,
        window: &mut Window,
        cx: &mut App,
    ) -> Option<Result<Arc<RenderImage>, ImageCacheError>> {
        self.items
            .entry(resource.clone())
            .or_insert_with(|| ImageCacheItem::new(resource, cx))
            .use_image(window)
    }
}
