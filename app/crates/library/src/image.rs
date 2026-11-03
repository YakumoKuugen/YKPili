//! 本帧实际加载的卡片图 URL 集合。
//!
//! 虚拟列表只渲染可视区 + 一行，卡片在 `render` 时把用到的 URL 标进来；
//! 页面图片缓存在帧末按这个集合驱逐不再可见的图。属于「视图测量」，
//! 因此放在 library 而不是 core / infra。

use std::collections::HashSet;

/// 当前帧的图片驻留集合。
#[derive(Default)]
pub struct ImageResidency {
    pub current: HashSet<String>,
}

impl ImageResidency {
    pub fn new() -> Self {
        Self::default()
    }

    pub fn mark(&mut self, url: &str) {
        if !url.is_empty() {
            self.current.insert(url.to_string());
        }
    }
}
