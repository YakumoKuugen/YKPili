//! 播放生命周期：起播（渲染线程）与播放循环。
//!
//! 两件事都必须在 `Render::render` 里按帧检查、只做一次；拆出来是为了让
//! render 专注于「取状态 → 组装元素」，也让「起播时发生了什么」和
//! 「每帧发生了什么」各自可读。

use super::*;

impl PlayerView {
    /// 首帧准备：有播放源才启动渲染线程；换源时 `open_bvid` 会把
    /// `tried_worker` 复位。
    ///
    /// 没有播放源时只有 `YK_VIDEO` 验收入口会退回本地测试文件，
    /// 正常点卡片期间（`loading_source`）绝不先播测试视频。
    #[cfg(any(target_os = "macos", target_os = "windows"))]
    pub(super) fn ensure_worker(&mut self, cx: &mut Context<Self>) {
        if self.tried_worker {
            return;
        }
        let source = self
            .source
            .as_ref()
            .map(|s| (s.video_url.clone(), s.audio_url.clone()));
        let fallback = if source.is_none() && !self.loading_source && self.model.error().is_none() {
            std::env::var("YK_VIDEO").ok()
        } else {
            None
        };
        let Some((video, audio)) = source.or_else(|| fallback.map(|v| (v, None))) else {
            return;
        };
        self.tried_worker = true;
        // 起播位置（续播）：mpv 打开文件时直接定位过去。
        let worker = crate::video_worker::FrameWorker::spawn_with_audio(
            video,
            audio,
            1280,
            720,
            self.start_position,
        );
        if worker.is_none() {
            // 线程都没起来：这是明确的「无法播放」，必须落错误日志并给出界面提示。
            // （libmpv 初始化 / 打开文件失败在线程内部记录，见 video_worker。）
            yk_infra::error_log::error(
                "player",
                "视频渲染线程启动失败：无法创建解码线程（无法播放）",
            );
            self.model.set_error("视频渲染线程启动失败，无法播放".to_string());
        }
        self.worker = worker;
        // 取流期间用户可能已经按过暂停：新 worker 启动后补发一次状态。
        if let Some(w) = self.worker.as_ref() {
            w.set_paused(!self.model.state.playing);
            // 音量 / 静音跟着视图模型走：新 worker 否则会从 mpv 默认 100% 起播，
            // 用户上一部片调过的音量在新片里丢失。
            w.set_volume(self.model.state.volume);
            w.set_muted(self.model.state.muted);
            // 倍速跟着视图模型走：换视频 / 换清晰度后新的 mpv 实例默认 1.0x，
            // 用户上一段调过的 1.5x 不能被重置。
            w.set_rate(self.model.state.rate);
            // 窗口当前不可见（最小化 / 被完全遮挡）：新 worker 直接停帧，
            // 别先解码几帧再停。
            w.set_render_paused(self.visibility.render_paused());
        }
        // 渲染循环在同一帧、worker 创建之后启动；这里再 notify 一次，
        // 让首帧尽快重绘，而不是等下一次用户事件。
        if self.worker.is_some() {
            cx.notify();
        }
    }

    /// 播放循环：每 16ms 取一帧最新画面并重绘（≈60fps）。
    ///
    /// **必须在 worker 创建之后调用**：以前放在 render 开头，靠
    /// `cx.notify()` 等下一轮 render 才启动；窗口未激活 / 被遮挡时下一轮
    /// 可能永远不来，表现就是音频在播、画面不出，点一下播放器触发 render
    /// 后才出首帧。libmpv 已在自动播放，这里只负责「把当前帧搬到 UI 上」。
    #[cfg(any(target_os = "macos", target_os = "windows"))]
    pub(super) fn ensure_playback_loop(&mut self, cx: &mut Context<Self>) {
        if self.loop_started || self.worker.is_none() {
            return;
        }
        self.loop_started = true;
        cx.spawn(async move |this, cx| {
            let mut received = 0u64;
            let mut idle_ticks = 0u32;
            loop {
                // 轮询间隔：窗口可见时 16ms（≈60fps）；不可见时 100ms。
                //
                // 窗口不可见时视频线程本来就只报位置（100ms 一条），
                // UI 线程再按 16ms 醒就是纯空转 —— 每秒 60 次定时器唤醒，
                // 在电池供电的笔记本上是实打实的耗电。
                let tick = this
                    .update(cx, |view, _| {
                        if view.visibility.is_visible() {
                            16
                        } else {
                            100
                        }
                    })
                    .unwrap_or(16);
                cx.background_executor()
                    .timer(std::time::Duration::from_millis(tick))
                    .await;
                // UI 线程只做「取一帧 + 通知重绘」，不碰任何解码/转换。
                // 这就是输入与播放互不耦合的关键：即使正在打字，
                // 渲染线程照常出帧，UI 也只是轻量地更新一下。
                let alive = this.update(cx, |view, cx| {
                    let mut got = false;
                    let mut finished = false;
                    if let Some(w) = view.worker.as_ref() {
                        // 结束信号独立于「有没有新帧」读取：
                        // 播到结尾后渲染线程可能不再产出帧。
                        finished = w.finished();
                        match w.latest() {
                            Some(crate::video_worker::WorkerFrame::Frame(buf, t)) => {
                                view.current = Some(buf);
                                // 首帧到了：本次播放在正常出画面，清掉无帧告警。
                                view.first_frame_seen = true;
                                view.no_frame_logged = false;
                                // 拖动 / seek 稳定期内不要让旧帧的 time-pos 把滑块顶回去。
                                let settling = view
                                    .seek_settle_until
                                    .is_some_and(|until| until > std::time::Instant::now());
                                if !view.scrubbing && !settling {
                                    view.play_time = t;
                                    view.model.state.position_secs = t;
                                    view.ensure_danmaku_ahead(cx);
                                    // 播放中定期把进度落盘：进程被杀 / 崩溃时
                                    // 也不至于丢掉续播位置。
                                    if t - view.last_resume_save >= 30.0 {
                                        view.last_resume_save = t;
                                        view.save_resume_position();
                                    }
                                }
                                got = true;
                            }
                            // 窗口不可见时视频线程只报位置：进度与续播位置照常推进，
                            // 只是没有新画面。这里**不** notify：没人看画面，
                            // 每次位置更新都重绘整窗只是白烧 CPU。
                            Some(crate::video_worker::WorkerFrame::Position(t)) => {
                                view.play_time = t;
                                view.model.state.position_secs = t;
                                view.ensure_danmaku_ahead(cx);
                                if t - view.last_resume_save >= 30.0 {
                                    view.last_resume_save = t;
                                    view.save_resume_position();
                                }
                            }
                            None => {}
                        }
                    }
                    // 播完自动收起：开关关掉时只停在最后一帧，等用户自己关。
                    if finished && !view.finish_handled && !view.closing && !view.scrubbing {
                        view.on_playback_finished(cx);
                    }
                    if got {
                        received += 1;
                        idle_ticks = 0;
                        if received == 1 {
                            eprintln!("[player] 收到首帧");
                        }
                        cx.notify();
                    } else {
                        idle_ticks += 1;
                        if idle_ticks == 125 {
                            idle_ticks = 0;
                            eprintln!("[player] 连续 2s 没有新帧");
                            // 首帧一直没来 + 正在播放：大概率就是「无法播放」。
                            // 只记一次，避免每 2 秒刷一条日志。
                            if !view.first_frame_seen
                                && !view.no_frame_logged
                                && view.model.state.playing
                                && !view.loading_source
                                // 窗口不可见时本来就不出帧，不能当成播放失败。
                                && view.visibility.is_visible()
                            {
                                view.no_frame_logged = true;
                                yk_infra::error_log::error(
                                    "player",
                                    "连续 2 秒没有收到视频首帧：解码或片源可能不可用（无法播放）",
                                );
                            }
                        }
                    }
                    got
                });
                if alive.is_err() {
                    break;
                }
            }
        })
        .detach();
    }
}
