//! 观看历史的 `HistoryHost` 组装点实现。
//!
//! 动作与渲染都在 `yk-library`；这里只提供 session / cookie 与播放器 hook。

use gpui::*;
use yk_library::history::{HistoryHost, HistoryState};
use yk_player::CoverSource;

use crate::AppRoot;

impl HistoryHost for AppRoot {
    fn history_session(&self) -> Option<&yk_domain::models::UserSession> {
        self.session.as_ref()
    }

    fn history_cookie(&self) -> String {
        self.cookie()
    }

    fn history(&self) -> &HistoryState {
        &self.history
    }

    fn history_mut(&mut self) -> &mut HistoryState {
        &mut self.history
    }





    fn history_open_player(
        &mut self,
        bvid: String,
        title: String,
        progress_secs: u64,
        entrance: Option<CoverSource>,
        cx: &mut Context<Self>,
    ) {
        eprintln!("[history] 进入播放浮窗：{title}（{bvid}）");
        self.ensure_player(cx);
        if let (Some(source), Some(player)) = (entrance, self.playing.clone()) {
            let _ = player.update(cx, |view, cx| view.set_entrance(source, cx));
        }
        let seed = if progress_secs > 0 {
            Some(progress_secs as f64)
        } else {
            None
        };
        if let Some(player) = self.playing.clone() {
            let _ = player.update(cx, |view, cx| view.open_bvid_at(bvid, seed, cx));
        }
        cx.notify();
    }

    fn history_go_home(&mut self, cx: &mut Context<Self>) {
        self.page = None;
        self.scroll_content_to_top();
        cx.notify();
    }

}
