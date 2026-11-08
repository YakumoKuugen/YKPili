//! 播放动作控制器：卡片起播、进度拖动、弹幕、互动面板、关闭流程。
//!
//! 这里只处理「用户做了什么 → 改哪个状态 / 调哪个后端」，不产出元素。
//! 渲染在 `view.rs`，状态在 `model.rs`，三者共用同一个 `PlayerView`。

use super::*;
use crate::view::*;

impl PlayerView {

    /// 卡片点击时注入起始封面；必须在 `open_bvid` 之前调用。
    pub fn set_entrance(&mut self, source: CoverSource, cx: &mut Context<Self>) {
        eprintln!(
            "[player] 封面展开：{}（{:.0}x{:.0} → 浮窗）",
            source.title,
            source.bounds.size.width.as_f32(),
            source.bounds.size.height.as_f32()
        );
        self.entrance_source = Some(source);
        self.entrance_generation = ENTRANCE_SEQ.fetch_add(1, std::sync::atomic::Ordering::Relaxed);
        cx.notify();
    }

    /// 卡片点击入口：按 bvid 取详情 + playurl，回填后由下一次 render 启动 mpv。
    ///
    /// 请求在后台 executor 跑；用 `load_generation` 丢弃过期结果，
    /// 连续点两张卡时不会出现旧视频覆盖新视频。
    pub fn open_bvid(&mut self, bvid: String, cx: &mut Context<Self>) {
        self.open_bvid_at(bvid, None, cx);
    }

    /// 同 [`Self::open_bvid`]，但带一个「已知进度」（秒）。
    ///
    /// `seed` 来自「观看历史」页：那条记录本身就写着上次看到哪，
    /// 直接拿来当起播位置，不用等本地记录或再发一次请求。
    pub fn open_bvid_at(&mut self, bvid: String, seed: Option<f64>, cx: &mut Context<Self>) {
        // 换源前先把上一条的关闭上报补掉，否则连续点两个视频时
        // 第一个只留下打开时的 0s 那一拍。
        self.flush_history_report();
        self.save_resume_position();
        self.load_generation = self.load_generation.wrapping_add(1);
        let generation = self.load_generation;
        self.loading_source = true;
        self.model.clear_error();
        self.source = None;
        #[cfg(any(target_os = "macos", target_os = "windows"))]
        {
            self.worker = None;
            self.current = None;
        }
        #[cfg(target_os = "windows")]
        {
            self.needs_image_clear = true;
        }
        self.tried_worker = false;
        self.play_time = 0.0;
        self.model.state.position_secs = 0.0;
        self.model.state.playing = true;
        // 起播即藏控制栏：画面优先，鼠标一动再浮出来。
        self.model.controls_visible = false;
        self.model.controls_timer_generation = self.model.controls_timer_generation.wrapping_add(1);
        self.model.controls_timer_running = false;
        self.panel_hover_grace_until = None;
        self.panel_last_scroll = None;
        self.needs_focus = true;
        self.escape_hint = false;
        self.escape_hint_generation = self.escape_hint_generation.wrapping_add(1);
        self.finish_handled = false;
        // 续播上下文重置：起播位置要等取流成功（拿到时长）才能定。
        self.resume_seed = seed;
        self.start_position = 0.0;
        self.resume_key = None;
        self.last_resume_save = 0.0;
        // 信息面板数据跟着换源重置：旧视频的标签/评论不能落到新视频上。
        // 滚动位置也回到顶部，否则新视频一打开就停在旧视频的评论高度。
        self.tags.clear();
        self.comments = CommentState::Idle;
        self.comment_section.reset();
        // 弹幕跟着换源清空：旧视频的弹幕不能落在新视频的画面上。
        self.danmaku.clear();
        self.danmaku_all.clear();
        self.danmaku_sending = false;
        self.danmaku_state = DanmakuState::Idle;
        self.danmaku_generation = self.danmaku_generation.wrapping_add(1);
        self.lanes = LaneAllocator::new(1280.0, 220.0);
        self.panel_scroll.set_offset(point(px(0.), px(0.)));
        self.scrubbing = false;
        self.last_scrub_seek = None;
        self.seek_settle_until = None;
        self.meta_generation = self.meta_generation.wrapping_add(1);
        self.followed = false;
        self.follow_known = false;
        self.follow_busy = false;
        self.follow_generation = self.follow_generation.wrapping_add(1);
        self.owner_fans = None;
        // 互动状态跟着换源重置：上一个视频的点赞 / 投币 / 收藏不能带过来。
        self.interaction = yk_infra::InteractionState::default();
        self.interaction_busy = false;
        self.coin_panel_open = false;
        self.coin_also_like = false;
        self.fav_panel_open = false;
        self.share_panel_open = false;
        // 画质 / 倍速菜单跟着换源收起；未完成的换流结果直接作废。
        self.quality_popover_open = false;
        self.speed_popover_open = false;
        self.quality_switching = false;
        self.quality_generation = self.quality_generation.wrapping_add(1);
        self.control_popover_bounds.set(None);
        // 换源时把可能还在播的退场动画也断掉，别让上一支视频的卡飘在新视频上。
        self.overlay_closing_kind = None;
        self.overlay_generation = self.overlay_generation.wrapping_add(1);
        self.fav_folders.clear();
        self.fav_loading = false;
        self.fav_error = None;
        self.fav_list_expanded = false;
        self.interaction_note = None;
        self.interaction_note_closing = false;
        self.interaction_note_generation = self.interaction_note_generation.wrapping_add(1);
        // 本地没有记录、也没有历史页带来的进度时，才去服务端历史里找一次
        // （别的设备 / 别的客户端看过，本地不知道）。
        let need_remote_history =
            seed.is_none() && self.resume.resume_position(bvid.trim()).is_none();
        eprintln!("[player] 请求播放：{bvid}");
        cx.notify();

        cx.spawn(async move |this, cx| {
            let result = cx
                .background_executor()
                .spawn(async move {
                    let cookie = current_session_cookie();
                    let client = match yk_infra::api::client::ReqwestClient::new() {
                        Ok(client) => client,
                        Err(error) => return (Err(error), None),
                    };
                    // 服务端历史查询与取流并行：都是阻塞请求，放两条线程上跑。
                    // 查询走 channel 而不是 join —— 就算它卡住，超时后也直接
                    // 从头播，不能让历史查询拖住起播。
                    let (history_tx, history_rx) = std::sync::mpsc::channel::<Option<u64>>();
                    if need_remote_history && !cookie.trim().is_empty() {
                        let cookie = cookie.clone();
                        let bvid = bvid.clone();
                        std::thread::spawn(move || {
                            let progress = (|| {
                                let client = match yk_infra::api::client::ReqwestClient::new() {
                                    Ok(client) => client,
                                    Err(error) => {
                                        eprintln!("[resume] 历史查询客户端创建失败：{error}");
                                        return None;
                                    }
                                };
                                match yk_infra::api::client::fetch_history(
                                    &client, &cookie, None, "",
                                ) {
                                    Ok((entries, _)) => {
                                        let progress = yk_infra::api::client::find_history_progress(
                                            &entries, &bvid,
                                        );
                                        if let Some(progress) = progress {
                                            eprintln!(
                                                "[resume] 服务端历史命中：{bvid} @ {progress}s"
                                            );
                                        }
                                        progress
                                    }
                                    Err(error) => {
                                        eprintln!("[resume] 服务端历史查询失败：{error}");
                                        None
                                    }
                                }
                            })();
                            let _ = history_tx.send(progress);
                        });
                    }
                    let source = yk_infra::api::client::fetch_playback(&client, &bvid, &cookie);
                    // 历史查询**不能拖住起播**：它只决定「从第几秒开始」，
                    // 拿不到就从 0 / 本地续播记录开始。以前这里等 2 秒，
                    // 相当于每次开播都先站 2 秒——这是我们比别家慢的主因。
                    let remote_progress = history_rx
                        .recv_timeout(std::time::Duration::from_millis(250))
                        .ok()
                        .flatten();
                    (source, remote_progress)
                })
                .await;

            let _ = this.update(cx, |view, cx| {
                if view.load_generation != generation {
                    return;
                }
                view.loading_source = false;
                let (source, remote_progress) = result;
                match source {
                    Ok(src) => {
                        eprintln!(
                            "[player] 取流成功：{}（{}s，音轨 {}）",
                            src.title,
                            src.duration_secs,
                            if src.audio_url.is_some() {
                                "有"
                            } else {
                                "无"
                            }
                        );
                        view.model.state.duration_secs = src.duration_secs as f64;
                        view.model.state.playing = true;
                        // 续播：本地记录 + 历史页带来的进度 + 服务端历史，取更靠后的。
                        let start =
                            view.resolve_resume_position(&src, remote_progress.map(|p| p as f64));
                        view.start_position = start;
                        view.model.state.position_secs = start;
                        let (aid, bvid, cid) = (src.aid, src.bvid.clone(), src.cid);
                        let owner_mid = src.owner_mid;
                        view.source = Some(src);
                        view.begin_history_report(aid, bvid.clone(), cid, start);
                        // 标签 / 评论 / 弹幕不阻塞起播：源挂上之后再拉，回来一轮重绘。
                        view.spawn_video_meta(aid, bvid.clone(), cx);
                        view.spawn_danmaku(cid, aid, cx);
                        // 互动状态（已点赞 / 已投币 / 已收藏）也放后台：按钮高亮晚一拍无妨。
                        view.spawn_interaction_state(aid, bvid, cx);
                        // 关注状态与粉丝数：同样是「先出画面，按钮稍后点亮」。
                        view.spawn_follow_state(owner_mid, cx);
                    }
                    Err(e) => {
                        eprintln!("[player] 取流失败：{e}");
                        view.model.set_error(e);
                    }
                }
                cx.notify();
            });
        })
        .detach();
    }

    /// 拉信息面板的标签；评论交给共享的 `comment_section`。
    ///
    /// 起播不依赖这两个请求：取流成功、面板已有标题/UP 主之后再后台补。
    /// 失败只影响面板里的对应区块，不把整个播放器打成错误态。
    pub(crate) fn spawn_video_meta(&mut self, aid: i64, bvid: String, cx: &mut Context<Self>) {
        if aid <= 0 && bvid.trim().is_empty() {
            self.comments = CommentState::Ready;
            self.comment_section.list.is_end = true;
            return;
        }
        self.meta_generation = self.meta_generation.wrapping_add(1);
        let generation = self.meta_generation;
        self.comments = CommentState::Loading;
        // 评论目标：有 aid 才有评论区（动态 / 直播回放没有）。
        self.comment_section
            .set_target(
                (aid > 0).then(|| CommentTarget {
                    oid: aid.to_string(),
                    kind: 1,
                }),
                current_session_cookie(),
            );
        self.comment_section.load_first(cx);
        self.comments = CommentState::Ready;
        let cookie = current_session_cookie();
        cx.notify();
        cx.spawn(async move |this, cx| {
            let result = cx
                .background_executor()
                .spawn(async move {
                    if bvid.trim().is_empty() {
                        return Ok::<_, String>(Vec::new());
                    }
                    let client = yk_infra::api::client::ReqwestClient::new()
                        .map_err(|error| error.to_string())?;
                    Ok(match yk_infra::api::client::fetch_video_tags(&client, &bvid, &cookie) {
                        Ok(tags) => tags,
                        Err(error) => {
                            eprintln!("[player] 标签加载失败：{error}");
                            Vec::new()
                        }
                    })
                })
                .await;
            let _ = this.update(cx, |view, cx| {
                if view.meta_generation != generation {
                    return;
                }
                if let Ok(tags) = result {
                    if !tags.is_empty() {
                        view.tags = tags;
                    }
                }
                cx.notify();
            });
        })
        .detach();
    }

    /// 打开视频后拉一次互动状态：是否已点赞 / 已投币 / 已收藏。
    ///
    /// 未登录时直接跳过：这些接口都要登录态，返回的一定是「未互动」。
    /// 失败只影响按钮的高亮，不影响播放。
    pub(crate) fn spawn_interaction_state(&mut self, aid: i64, bvid: String, cx: &mut Context<Self>) {
        if bvid.trim().is_empty() {
            return;
        }
        let cookie = current_session_cookie();
        if cookie.trim().is_empty() {
            return;
        }
        self.interaction_generation = self.interaction_generation.wrapping_add(1);
        let generation = self.interaction_generation;
        cx.spawn(async move |this, cx| {
            let result = cx
                .background_executor()
                .spawn(async move {
                    let client = yk_infra::api::client::ReqwestClient::new()
                        .map_err(|error| error.to_string())?;
                    // 一个请求拿「赞 / 币 / 藏 / 关注」：
                    // 以前这三个状态是三个接口串行，等于每次开播多排两格队。
                    let (liked, coined, favoured, followed) =
                        yk_infra::interaction::fetch_archive_relation(
                            &client, &cookie, aid, &bvid,
                        )
                        .unwrap_or((false, false, false, None));
                    Ok::<_, String>((liked, coined, favoured, followed))
                })
                .await;
            let _ = this.update(cx, |view, cx| {
                if view.interaction_generation != generation {
                    return;
                }
                if let Ok((liked, coined, favoured, followed)) = result {
                    view.interaction.liked = liked;
                    view.interaction.coined = coined;
                    view.interaction.favoured = favoured;
                    // 关注态就靠这一次请求定：`attention` 回来了就是权威值。
                    if let Some(followed) = followed {
                        view.followed = followed;
                        view.follow_known = true;
                    }
                    cx.notify();
                }
            });
        })
        .detach();
    }

    /// 打开视频后读一次 UP 主关系：是否已关注 + 粉丝数。
    ///
    /// 两个请求都放后台、失败静默：
    /// - 关系状态要登录态，未登录直接跳过（按钮保持「关注」，点了会提示登录）；
    /// - 粉丝数是公开接口，未登录也能显示。
    pub(crate) fn spawn_follow_state(&mut self, owner_mid: i64, cx: &mut Context<Self>) {
        if owner_mid <= 0 {
            // 没有 uid 就无从查起：保持未知，点击时给出人话提示。
            return;
        }
        let cookie = current_session_cookie();
        self.follow_generation = self.follow_generation.wrapping_add(1);
        let generation = self.follow_generation;
        cx.spawn(async move |this, cx| {
            let result = cx
                .background_executor()
                .spawn(async move {
                    let client = yk_infra::api::client::ReqwestClient::new()
                        .map_err(|error| error.to_string())?;
                    // 关注状态不再单独发 `x/relation?fid`：同一次开播的
                    // `archive/relation` 已经把 `attention` 带回来了。
                    // 这里只补粉丝数（那是另一个接口的字段）。
                    let followed: Option<bool> = None;
                    let fans = match yk_infra::api::client::fetch_relation_stat(
                        &client, &cookie, owner_mid,
                    ) {
                        Ok((fans, _)) => {
                            eprintln!("[follow] 粉丝数：mid={owner_mid} fans={fans}");
                            Some(fans)
                        }
                        Err(error) => {
                            eprintln!("[follow] 粉丝数读取失败：{error}");
                            None
                        }
                    };
                    Ok::<_, String>((followed, fans))
                })
                .await;
            let _ = this.update(cx, |view, cx| {
                if view.follow_generation != generation {
                    return;
                }
                if let Ok((followed, fans)) = result {
                    if let Some(followed) = followed {
                        view.followed = followed;
                        view.follow_known = true;
                    }
                    if fans.is_some() {
                        view.owner_fans = fans;
                    }
                    cx.notify();
                }
            });
        })
        .detach();
    }

    /// 关注 / 取关当前 UP 主。
    ///
    /// 两种起点：
    /// - 已经读到过关系状态：按钮先乐观翻转，失败再翻回来；
    /// - 还没读到（比如刚打开视频就点）：先查一次关系，再写反状态，
    ///   期间按钮显示「处理中」，不猜当前状态。
    pub(crate) fn toggle_follow(&mut self, cx: &mut Context<Self>) {
        if self.follow_busy {
            return;
        }
        let Some(source) = self.source.clone() else {
            return;
        };
        if source.owner_mid <= 0 {
            self.show_interaction_note("没拿到 UP 主 uid，无法关注".into(), true, cx);
            return;
        }
        let Some(session) = current_session() else {
            self.show_interaction_note("请先登录后再关注".into(), true, cx);
            return;
        };
        let mid = source.owner_mid;
        let cookie = session.cookie_header();
        let csrf = session.bili_jct.clone();
        let known = self.follow_known;
        let mut next = !self.followed;
        self.follow_busy = true;
        if known {
            // 乐观更新：失败回滚（见下方 Err 分支）。
            self.followed = next;
            if let Some(fans) = self.owner_fans.as_mut() {
                *fans = if next {
                    fans.saturating_add(1)
                } else {
                    fans.saturating_sub(1)
                };
            }
        }
        self.follow_generation = self.follow_generation.wrapping_add(1);
        let generation = self.follow_generation;
        cx.notify();
        cx.spawn(async move |this, cx| {
            let result = cx
                .background_executor()
                .spawn(async move {
                    let client = yk_infra::api::client::ReqwestClient::new()
                        .map_err(|error| error.to_string())?;
                    if !known {
                        // 未知起点：先查真实状态，再决定写「关注」还是「取关」。
                        let followed =
                            yk_infra::api::client::fetch_followed(&client, &cookie, mid)?;
                        next = !followed;
                    }
                    yk_infra::interaction::follow_user(&client, &cookie, &csrf, mid, next)?;
                    Ok::<_, String>((known, next))
                })
                .await;
            let _ = this.update(cx, |view, cx| {
                if view.follow_generation != generation {
                    return;
                }
                view.follow_busy = false;
                match result {
                    Ok((known, next)) => {
                        if !known {
                            // 未知起点只在成功后才点亮；粉丝数跟着动一下。
                            view.followed = next;
                            view.follow_known = true;
                            if let Some(fans) = view.owner_fans.as_mut() {
                                *fans = if next {
                                    fans.saturating_add(1)
                                } else {
                                    fans.saturating_sub(1)
                                };
                            }
                        }
                        view.show_interaction_note(
                            if next { "已关注" } else { "已取消关注" }.into(),
                            false,
                            cx,
                        );
                    }
                    Err(error) => {
                        eprintln!("[interaction] 关注失败：{error}");
                        if known {
                            // 乐观更新回滚：按钮与粉丝数都退回去。
                            view.followed = !next;
                            if let Some(fans) = view.owner_fans.as_mut() {
                                *fans = if next {
                                    fans.saturating_sub(1)
                                } else {
                                    fans.saturating_add(1)
                                };
                            }
                        }
                        view.show_interaction_note(error, true, cx);
                    }
                }
                cx.notify();
            });
        })
        .detach();
    }

    /// 在窗口底部居中弹一条轻提示（toast）。
    ///
    /// 生命周期分三段：入场缩放 → 停留 `TOAST_VISIBLE_MS` → 退场缩放，
    /// 退场动画走完才把内容从 `interaction_note` 里摘掉。
    pub(crate) fn show_interaction_note(&mut self, text: String, is_error: bool, cx: &mut Context<Self>) {
        self.interaction_note = Some((text, is_error));
        self.interaction_note_closing = false;
        self.interaction_note_generation = self.interaction_note_generation.wrapping_add(1);
        let generation = self.interaction_note_generation;
        cx.notify();
        cx.spawn(async move |this, cx| {
            cx.background_executor()
                .timer(std::time::Duration::from_millis(TOAST_VISIBLE_MS))
                .await;
            let still_current = this
                .update(cx, |view, cx| {
                    if view.interaction_note_generation != generation {
                        return false;
                    }
                    // 先切到退场动画；内容还要再留 DURATION_FAST 让缩放收完。
                    view.interaction_note_closing = true;
                    cx.notify();
                    true
                })
                .unwrap_or(false);
            if !still_current {
                return;
            }
            // 动画的起点要等下一帧 request_layout 才建立，这里多留一两帧，
            // 避免缩放到一半就被摘掉（最后 10% 的透明度/尺寸跳变肉眼可见）。
            cx.background_executor()
                .timer(DURATION_FAST + std::time::Duration::from_millis(40))
                .await;
            let _ = this.update(cx, |view, cx| {
                if view.interaction_note_generation != generation {
                    return;
                }
                view.interaction_note = None;
                view.interaction_note_closing = false;
                cx.notify();
            });
        })
        .detach();
    }

    /// 点赞 / 取消点赞。
    ///
    /// 先本地翻转（乐观更新），服务端失败再回滚并提示。
    pub(crate) fn toggle_like(&mut self, cx: &mut Context<Self>) {
        if self.interaction_busy {
            return;
        }
        let Some(source) = self.source.clone() else {
            return;
        };
        let Some(session) = current_session() else {
            self.show_interaction_note("请先登录后再点赞".into(), true, cx);
            return;
        };
        if source.bvid.trim().is_empty() {
            return;
        }
        let next = !self.interaction.liked;
        self.interaction.liked = next;
        self.interaction_busy = true;
        if let Some(source) = self.source.as_mut() {
            source.like_count = if next {
                source.like_count.saturating_add(1)
            } else {
                source.like_count.saturating_sub(1)
            };
        }
        let bvid = source.bvid.clone();
        let cookie = session.cookie_header();
        let csrf = session.bili_jct.clone();
        cx.notify();
        cx.spawn(async move |this, cx| {
            let result = cx
                .background_executor()
                .spawn(async move {
                    let client = yk_infra::api::client::ReqwestClient::new()
                        .map_err(|error| error.to_string())?;
                    yk_infra::interaction::like_video(&client, &cookie, &csrf, &bvid, next)
                })
                .await;
            let _ = this.update(cx, |view, cx| {
                view.interaction_busy = false;
                match result {
                    Ok(()) => {
                        view.show_interaction_note(
                            if next { "已点赞" } else { "已取消点赞" }.into(),
                            false,
                            cx,
                        );
                    }
                    Err(error) => {
                        eprintln!("[interaction] 点赞失败：{error}");
                        // 乐观更新回滚：按钮状态和计数都退回去。
                        view.interaction.liked = !next;
                        if let Some(source) = view.source.as_mut() {
                            source.like_count = if next {
                                source.like_count.saturating_sub(1)
                            } else {
                                source.like_count.saturating_add(1)
                            };
                        }
                        view.show_interaction_note(error, true, cx);
                    }
                }
                cx.notify();
            });
        })
        .detach();
    }

    /// 展开 / 收起投币面板。
    pub(crate) fn toggle_coin_panel(&mut self, cx: &mut Context<Self>) {
        // 未登录时投不了币：直接提示，不展开一个点不动的面板。
        if !self.coin_panel_open && current_session().is_none() {
            self.show_interaction_note("请先登录后再投币".into(), true, cx);
            return;
        }
        if self.coin_panel_open {
            self.close_action_overlays(cx);
            return;
        }
        self.fav_panel_open = false;
        self.share_panel_open = false;
        self.coin_panel_open = true;
        self.note_overlay_opened(cx);
    }

    /// 投币（1 枚 / 2 枚，可同时点赞）。
    pub(crate) fn do_coin(&mut self, multiply: u8, also_like: bool, cx: &mut Context<Self>) {
        if self.interaction_busy {
            return;
        }
        let Some(source) = self.source.clone() else {
            return;
        };
        let Some(session) = current_session() else {
            self.show_interaction_note("请先登录后再投币".into(), true, cx);
            return;
        };
        if source.bvid.trim().is_empty() {
            return;
        }
        // 投币接口只认 av 号（bvid 会被服务端判成 -400 请求错误）。
        let aid = source.aid;
        // bvid 只用来拼视频页 Referer（风控会核对请求来源页）。
        let bvid = source.bvid.clone();
        if aid <= 0 {
            self.show_interaction_note("这个视频没有可投币的信息".into(), true, cx);
            return;
        }
        let coins = multiply.clamp(1, 2);
        self.interaction_busy = true;
        self.close_action_overlays(cx);
        let cookie = session.cookie_header();
        let csrf = session.bili_jct.clone();
        cx.notify();
        cx.spawn(async move |this, cx| {
            let result = cx
                .background_executor()
                .spawn(async move {
                    let client = yk_infra::api::client::ReqwestClient::new()
                        .map_err(|error| error.to_string())?;
                    yk_infra::interaction::coin_video(
                        &client, &cookie, &csrf, aid, &bvid, coins, also_like,
                    )
                })
                .await;
            let _ = this.update(cx, |view, cx| {
                view.interaction_busy = false;
                match result {
                    Ok(()) => {
                        view.interaction.coined = true;
                        if also_like {
                            view.interaction.liked = true;
                        }
                        if let Some(source) = view.source.as_mut() {
                            source.coin_count = source.coin_count.saturating_add(coins as u64);
                            if also_like {
                                source.like_count = source.like_count.saturating_add(1);
                            }
                        }
                        view.show_interaction_note(format!("已投 {coins} 枚硬币"), false, cx);
                    }
                    Err(error) => {
                        eprintln!("[interaction] 投币失败：{error}");
                        view.show_interaction_note(error, true, cx);
                    }
                }
                cx.notify();
            });
        })
        .detach();
    }

    /// 展开 / 收起收藏夹面板；展开时顺手把收藏夹列表拉回来。
    pub(crate) fn toggle_fav_panel(&mut self, cx: &mut Context<Self>) {
        // 未登录时列不出自己的收藏夹：直接提示。
        if !self.fav_panel_open && current_session().is_none() {
            self.show_interaction_note("请先登录后再收藏".into(), true, cx);
            return;
        }
        if self.fav_panel_open {
            self.close_action_overlays(cx);
            return;
        }
        self.coin_panel_open = false;
        self.share_panel_open = false;
        self.fav_panel_open = true;
        self.note_overlay_opened(cx);
        if self.fav_folders.is_empty() {
            self.load_fav_folders(cx);
        }
    }

    /// 拉自己的收藏夹列表。
    pub(crate) fn load_fav_folders(&mut self, cx: &mut Context<Self>) {
        let Some(session) = current_session() else {
            self.show_interaction_note("请先登录后再收藏".into(), true, cx);
            return;
        };
        self.fav_loading = true;
        self.fav_error = None;
        let cookie = session.cookie_header();
        let mid = session.dede_user_id;
        // 带上当前视频的 aid：夹子的 `fav_state` 才是「这个视频在不在里面」，
        // 否则面板上的勾选圈永远空着（看起来像没收藏）。
        let rid = self.source.as_ref().map(|source| source.aid).unwrap_or(0);
        cx.notify();
        cx.spawn(async move |this, cx| {
            let result = cx
                .background_executor()
                .spawn(async move {
                    let client = yk_infra::api::client::ReqwestClient::new()
                        .map_err(|error| error.to_string())?;
                    yk_infra::interaction::fetch_fav_folders(&client, &cookie, mid, rid)
                })
                .await;
            let _ = this.update(cx, |view, cx| {
                view.fav_loading = false;
                match result {
                    Ok(folders) => {
                        view.fav_folders = folders;
                        view.fav_error = None;
                    }
                    Err(error) => {
                        eprintln!("[interaction] 收藏夹加载失败：{error}");
                        // 面板内联显示失败态 + 重试：只弹 toast 的话，
                        // 面板里只剩一片空白，用户不知道是「没有收藏夹」还是「没加载出来」。
                        view.fav_error = Some(error.clone());
                        view.show_interaction_note(error, true, cx);
                    }
                }
                cx.notify();
            });
        })
        .detach();
    }

    /// 收藏到指定收藏夹 / 从收藏夹移除。
    pub(crate) fn do_fav(&mut self, folder_id: i64, add: bool, cx: &mut Context<Self>) {
        // 退场动画里那张卡只是残影：别再让它接业务操作。
        if self.interaction_busy || !self.fav_panel_open {
            return;
        }
        let Some(source) = self.source.clone() else {
            return;
        };
        let Some(session) = current_session() else {
            self.show_interaction_note("请先登录后再收藏".into(), true, cx);
            return;
        };
        if source.aid <= 0 {
            return;
        }
        self.interaction_busy = true;
        let aid = source.aid;
        let cookie = session.cookie_header();
        let csrf = session.bili_jct.clone();
        cx.notify();
        cx.spawn(async move |this, cx| {
            let result = cx
                .background_executor()
                .spawn(async move {
                    let client = yk_infra::api::client::ReqwestClient::new()
                        .map_err(|error| error.to_string())?;
                    yk_infra::interaction::fav_video(&client, &cookie, &csrf, aid, folder_id, add)
                })
                .await;
            let _ = this.update(cx, |view, cx| {
                view.interaction_busy = false;
                match result {
                    Ok(()) => {
                        // 收藏夹是多选：勾上 / 取消只改这一夹的状态。
                        // 先回写列表，否则面板上的勾选框永远停在旧状态
                        // （看起来像点了没反应）。
                        if let Some(folder) = view
                            .fav_folders
                            .iter_mut()
                            .find(|folder| folder.id == folder_id)
                        {
                            folder.fav_state = if add { 1 } else { 0 };
                        }
                        // 顶部的收藏数只在「一个夹都没有 → 至少收进一个」这一下 ±1；
                        // 收进第二个夹不该再加一次数。
                        let was_favoured = view.interaction.favoured;
                        let now_favoured =
                            view.fav_folders.iter().any(|folder| folder.contains_video());
                        view.interaction.favoured = now_favoured;
                        if was_favoured != now_favoured {
                            if let Some(source) = view.source.as_mut() {
                                source.favorite_count = if now_favoured {
                                    source.favorite_count.saturating_add(1)
                                } else {
                                    source.favorite_count.saturating_sub(1)
                                };
                            }
                        }
                        view.show_interaction_note(
                            if add { "已收藏" } else { "已取消收藏" }.into(),
                            false,
                            cx,
                        );
                    }
                    Err(error) => {
                        eprintln!("[interaction] 收藏失败：{error}");
                        view.show_interaction_note(error, true, cx);
                    }
                }
                cx.notify();
            });
        })
        .detach();
    }

    /// 收起互动区的所有弹层；返回「刚才确实有弹层开着」。
    ///
    /// Esc 和「面板收起」都走这里：弹层是浮在内容上的一次性操作，
    /// 关掉它不应该带动其它状态（不退出播放器、不重置互动）。
    pub(crate) fn close_action_overlays(&mut self, cx: &mut Context<Self>) -> bool {
        // 画质 / 倍速是控制栏里的轻量菜单：Esc 先关它，不进入「再按一次退出播放器」。
        if self.quality_popover_open || self.speed_popover_open {
            self.close_control_popovers(cx);
            return true;
        }
        let kind = if self.fav_panel_open {
            ActionOverlay::Fav
        } else if self.coin_panel_open {
            ActionOverlay::Coin
        } else if self.share_panel_open {
            ActionOverlay::Share
        } else {
            return false;
        };
        self.coin_panel_open = false;
        self.fav_panel_open = false;
        self.share_panel_open = false;
        // 留一两帧给退场动画：内容按 kind 重新画，动画走完再真正摘掉。
        self.overlay_closing_kind = Some(kind);
        self.overlay_generation = self.overlay_generation.wrapping_add(1);
        let generation = self.overlay_generation;
        cx.notify();
        cx.spawn(async move |this, cx| {
            // 动画起点要等下一帧 request_layout 才建立，多留两帧，
            // 免得淡到一半就被摘掉（最后 10% 的跳变肉眼可见）。
            cx.background_executor()
                .timer(DURATION_FAST + std::time::Duration::from_millis(40))
                .await;
            let _ = this.update(cx, |view, cx| {
                if view.overlay_generation != generation || view.overlay_closing_kind.is_none() {
                    return;
                }
                view.overlay_closing_kind = None;
                cx.notify();
            });
        })
        .detach();
        true
    }

    /// 弹层刚打开：清掉可能还在跑的退场动画，并起一段新的入场动画。
    pub(crate) fn note_overlay_opened(&mut self, cx: &mut Context<Self>) {
        self.overlay_closing_kind = None;
        self.overlay_generation = self.overlay_generation.wrapping_add(1);
        // 打开弹层时可能已经排了一个 220ms 的收起：取消掉，
        // 否则刚打开就被收走（面板一收弹层也跟着关）。
        if self.panel_collapse_pending {
            self.panel_collapse_pending = false;
            self.panel_collapse_generation = self.panel_collapse_generation.wrapping_add(1);
        }
        cx.notify();
    }

    /// 展开 / 收起分享面板。
    pub(crate) fn toggle_share_panel(&mut self, cx: &mut Context<Self>) {
        if self.share_panel_open {
            self.close_action_overlays(cx);
            return;
        }
        self.coin_panel_open = false;
        self.fav_panel_open = false;
        self.share_panel_open = true;
        self.note_overlay_opened(cx);
    }

    /// 分享到哔哩哔哩动态：转发视频，配文用标题。
    pub(crate) fn share_to_dynamic(&mut self, cx: &mut Context<Self>) {
        if self.interaction_busy {
            return;
        }
        let Some(source) = self.source.clone() else {
            return;
        };
        let Some(session) = current_session() else {
            self.show_interaction_note("请先登录后再分享".into(), true, cx);
            return;
        };
        if source.aid <= 0 {
            self.show_interaction_note("这个视频没有可转发的信息".into(), true, cx);
            return;
        }
        self.interaction_busy = true;
        self.close_action_overlays(cx);
        // 正文 = 标题 + 视频链接：B 站会把链接解析成视频卡片，
        // 解析不了也至少是一条能点进视频的动态。
        let content = format!("{} {}", source.title, video_page_url(&source.bvid));
        let cookie = session.cookie_header();
        let csrf = session.bili_jct.clone();
        cx.notify();
        cx.spawn(async move |this, cx| {
            let result = cx
                .background_executor()
                .spawn(async move {
                    let client = yk_infra::api::client::ReqwestClient::new()
                        .map_err(|error| error.to_string())?;
                    yk_infra::interaction::share_to_dynamic(&client, &cookie, &csrf, &content)
                })
                .await;
            let _ = this.update(cx, |view, cx| {
                view.interaction_busy = false;
                match result {
                    Ok(dynamic_id) => {
                        eprintln!("[interaction] 已转发到动态：{dynamic_id}");
                        if let Some(source) = view.source.as_mut() {
                            source.share_count = source.share_count.saturating_add(1);
                        }
                        view.show_interaction_note("已分享到哔哩哔哩动态".into(), false, cx);
                    }
                    Err(error) => {
                        eprintln!("[interaction] 分享到动态失败：{error}");
                        view.show_interaction_note(error, true, cx);
                    }
                }
                cx.notify();
            });
        })
        .detach();
    }

    /// 分享到 QQ：打开腾讯的 QQ 分享卡片页（默认浏览器）。
    ///
    /// QQ 桌面版注册的 `tencent://` / `mqqapi://` 只能跳到指定聊天对象，
    /// 没有「选好友分享」的入口；网页端的分享卡片才是腾讯官方给的那条路。
    pub(crate) fn share_to_qq(&mut self, cx: &mut Context<Self>) {
        let Some(source) = self.source.clone() else {
            return;
        };
        if source.bvid.trim().is_empty() {
            self.show_interaction_note("这个视频没有可分享的链接".into(), true, cx);
            return;
        }
        self.close_action_overlays(cx);
        let link = video_page_url(&source.bvid);
        let title = source.title.clone();
        let desc = source.desc.clone();
        let qq_url = qq_share_url(&link, &title, &desc);
        if open_in_browser(&qq_url) {
            if let Some(source) = self.source.as_mut() {
                source.share_count = source.share_count.saturating_add(1);
            }
            self.show_interaction_note("已打开 QQ 分享卡片".into(), false, cx);
        } else {
            self.show_interaction_note("打开 QQ 分享卡片失败".into(), true, cx);
        }
        cx.notify();
    }

    /// 复制视频链接：`链接 + 换行 + 标题`。
    ///
    /// 只贴一条裸链接，粘到聊天框里对方看不出是什么视频；带上标题一行，
    /// 既认得出内容，链接本身也还能点开（和 B 站网页版复制的形态一致）。
    pub(crate) fn copy_video_link(&mut self, cx: &mut Context<Self>) {
        let Some(source) = self.source.clone() else {
            return;
        };
        self.close_action_overlays(cx);
        let link = video_page_url(&source.bvid);
        let text = share_link_text(&link, &source.title);
        let with_title = text.len() > link.len();
        cx.write_to_clipboard(ClipboardItem::new_string(text));
        self.show_interaction_note(
            if with_title { "已复制链接和标题" } else { "链接已复制" }.into(),
            false,
            cx,
        );
    }

    /// 窗口可见性变化：不可见时让视频线程停帧，可见时恢复。
    ///
    /// **只管「看不见」，不管「失焦」**：失焦（点了别的应用）时窗口还在屏幕上，
    /// 视频必须照常按显示器的标准帧率播 —— 这正是 `inactive_frame_interval: None`
    /// 想保住的行为。最小化 / 被完全遮挡时画面没人看，解码 + NV12 转换
    /// 纯属烧电，这时才停。
    ///
    /// mpv 本身不暂停：音频继续播，进度继续走，恢复可见后接着出帧。
    pub(crate) fn set_window_visible(&mut self, visible: bool, cx: &mut Context<Self>) {
        if !self.visibility.set(visible) {
            return;
        }
        #[cfg(any(target_os = "macos", target_os = "windows"))]
        if let Some(worker) = self.worker.as_ref() {
            worker.set_render_paused(self.visibility.render_paused());
        }
        // 不可见时不必再排下一帧；可见时补一帧把画面拉回来。
        if visible {
            cx.notify();
        }
    }

    /// 拉当前分 P 的真实弹幕（`/x/v1/dm/list.so`，按 cid 区分）。
    ///
    /// 与标签 / 评论分开请求：弹幕和评论互不阻塞，谁先回来谁先渲染。
    /// 换源时 `danmaku_generation` 已经 +1，旧响应回来直接丢弃。
    /// 起播时先把弹幕状态复位，然后拉**第一段**（6 分钟）。
    ///
    /// 整包拉 `x/v1/dm/list.so` 是几 MB 的 deflate XML，等于每次开播都为
    /// 「可能只看了三十秒」付全片成本；改成按段取，看多少拉多少。
    pub(crate) fn spawn_danmaku(&mut self, cid: i64, aid: i64, cx: &mut Context<Self>) {
        self.danmaku_generation = self.danmaku_generation.wrapping_add(1);
        self.danmaku_source.clear();
        self.danmaku_all.clear();
        self.danmaku.clear();
        self.danmaku_cid = cid;
        self.danmaku_aid = aid;
        self.danmaku_next_segment = 1;
        self.danmaku_segment_loading = false;
        self.danmaku_retry_at = None;
        if cid <= 0 {
            self.danmaku_segment_done = true;
            self.danmaku_state = DanmakuState::Ready;
            cx.notify();
            return;
        }
        self.danmaku_segment_done = false;
        self.danmaku_state = DanmakuState::Loading;
        cx.notify();
        self.load_danmaku_segment(cx);
    }

    /// 拉下一段弹幕并追加。空段 = 到头了（第一段就空 = 这个视频没有弹幕）。
    pub(crate) fn load_danmaku_segment(&mut self, cx: &mut Context<Self>) {
        if self.danmaku_segment_loading || self.danmaku_segment_done || self.danmaku_cid <= 0 {
            return;
        }
        if let Some(at) = self.danmaku_retry_at {
            if at > std::time::Instant::now() {
                return;
            }
        }
        let index = self.danmaku_next_segment;
        let (cid, aid) = (self.danmaku_cid, self.danmaku_aid);
        let generation = self.danmaku_generation;
        let cookie = current_session_cookie();
        self.danmaku_segment_loading = true;
        cx.spawn(async move |this, cx| {
            let result = cx
                .background_executor()
                .spawn(async move {
                    let client = yk_infra::api::client::ReqwestClient::new()
                        .map_err(|error| error.to_string())?;
                    yk_infra::api::client::fetch_danmaku_segment(
                        &client, &cookie, cid, aid, index,
                    )
                })
                .await;
            let _ = this.update(cx, |view, cx| {
                if view.danmaku_generation != generation {
                    return;
                }
                view.danmaku_segment_loading = false;
                match result {
                    Ok(items) => {
                        view.danmaku_retry_at = None;
                        if items.is_empty() {
                            // 没有这一段 = 播到结尾了。
                            view.danmaku_segment_done = true;
                            view.danmaku_state = DanmakuState::Ready;
                        } else {
                            view.danmaku_source.extend(items);
                            view.danmaku_source.sort_by(|a, b| {
                                a.time.partial_cmp(&b.time).unwrap_or(std::cmp::Ordering::Equal)
                            });
                            view.danmaku_next_segment = index + 1;
                            view.danmaku_state = DanmakuState::Ready;
                            let all = view.danmaku_source.clone();
                            view.layout_danmaku(all);
                        }
                    }
                    Err(error) => {
                        // 失败不封顶：5 秒后再试，别在计数帧里死循环。
                        eprintln!("[danmaku] 第 {index} 段加载失败：{error}");
                        view.danmaku_retry_at =
                            Some(std::time::Instant::now() + std::time::Duration::from_secs(5));
                        if index == 1 {
                            view.danmaku_state = DanmakuState::Ready;
                        }
                    }
                }
                cx.notify();
            });
        })
        .detach();
    }

    /// 播到「已加载区间」末尾前 60 秒就预取下一段。
    pub(crate) fn ensure_danmaku_ahead(&mut self, cx: &mut Context<Self>) {
        if self.danmaku_segment_done || self.danmaku_cid <= 0 {
            return;
        }
        let loaded_until = self.danmaku_next_segment.saturating_sub(1) as f64
            * DANMAKU_SEGMENT_SECS;
        if self.play_time + DANMAKU_PREFETCH_SECS >= loaded_until {
            self.load_danmaku_segment(cx);
        }
    }

    pub(crate) fn install_danmaku(&mut self, set: DanmakuSet) {
        self.layout_danmaku(set.items().to_vec());
    }

    /// 按当前轨道数把弹幕重新分配到轨道上。
    ///
    /// 窗口高度变化时轨道数会变，重排一次就能把弹幕铺满新的播放区；
    /// 否则 14 条轨道在 700px 高的画面里只会占用上半屏。
    pub(crate) fn layout_danmaku(&mut self, items: Vec<Danmaku>) {
        let max_lanes = self.scroll_lanes.clamp(1, MAX_SCROLL_LANES);
        let mut lanes = LaneAllocator::new(1280.0, 220.0);
        let mut top_free = [f64::NEG_INFINITY; FIXED_LANES];
        let mut bottom_free = [f64::NEG_INFINITY; FIXED_LANES];
        let mut placed = Vec::with_capacity(items.len());
        let (mut scroll, mut top, mut bottom) = (0usize, 0usize, 0usize);

        for d in &items {
            let lane = match d.mode {
                DanmakuMode::Scroll => {
                    // 轮转分配：短弹幕（满屏「？」）不会全挤在第 0 条轨道。
                    let Some(lane) = lanes.assign_spread(d, max_lanes) else {
                        continue;
                    };
                    scroll += 1;
                    DanmakuLane::Scroll(lane)
                }
                DanmakuMode::Top => {
                    let Some(slot) = take_fixed_slot(&mut top_free, d.time) else {
                        continue;
                    };
                    top += 1;
                    DanmakuLane::Top(slot)
                }
                DanmakuMode::Bottom => {
                    let Some(slot) = take_fixed_slot(&mut bottom_free, d.time) else {
                        continue;
                    };
                    bottom += 1;
                    DanmakuLane::Bottom(slot)
                }
            };
            placed.push(PlacedDanmaku {
                data: d.clone(),
                lane,
            });
        }

        eprintln!(
            "[danmaku] 上屏 {} 条（滚动 {scroll} / 顶部 {top} / 底部 {bottom}，轨道 {max_lanes}），列表 {} 条",
            placed.len(),
            items.len()
        );
        self.lanes = lanes;
        self.danmaku = placed;
        self.danmaku_all = items;
    }

    /// 窗口高度变化后重排已有弹幕。
    pub(crate) fn reflow_danmaku(&mut self) {
        if self.danmaku_all.is_empty() {
            return;
        }
        let items = self.danmaku_all.clone();
        self.layout_danmaku(items);
    }

    /// 给用户刚发的一条滚动弹幕找一个当前空着的轨道。
    ///
    /// 预分配的 `LaneAllocator` 状态停在「整段视频排完」那一刻，
    /// 不能直接拿来给当前时刻的新弹幕分配；这里按「此刻每条轨道上
    /// 是否还有可见弹幕」现查，保证发出去的弹幕不会立刻叠字。
    pub(crate) fn pick_free_scroll_lane(&self, now: f64, area_width: f32) -> usize {
        for lane in 0..self.scroll_lanes.clamp(1, MAX_SCROLL_LANES) {
            let occupied = self.danmaku.iter().any(|placed| {
                matches!(placed.lane, DanmakuLane::Scroll(l) if l == lane)
                    && placed.data.time <= now + 0.2
                    && self.lanes.visible_at_in(&placed.data, now, area_width)
            });
            if !occupied {
                return lane;
            }
        }
        0
    }

    /// 面板展开 / 收起。
    ///
    /// 展开立刻生效；收起延迟 220ms，并先确认指针真的不在面板区域。
    /// 收起判断不信任 `on_hover(false)`：播放中每帧重绘会让它在指针没动时闪 false。
    pub(crate) fn set_panel_hover(&mut self, hovered: bool, window: &Window, cx: &mut Context<Self>) {
        if hovered {
            self.panel_collapse_pending = false;
            self.panel_collapse_generation = self.panel_collapse_generation.wrapping_add(1);
            self.panel_hover_grace_until = None;
            self.panel_hover_grace_pos = None;
            if !self.expanded {
                self.expanded = true;
                self.panel_generation = self.panel_generation.wrapping_add(1);
                cx.notify();
            }
            return;
        }
        // 已经收起就没什么可收的，避免鼠标在视频区移动时反复起定时器。
        if !self.expanded {
            return;
        }
        // 正在写评论：输入框还活着，不能把整个侧栏收走。
        if self.comment_section.focused {
            return;
        }
        // 刚在面板里滚动过：滚动期间 hover 会抖，先不收起。
        if self.panel_scrolled_recently() {
            return;
        }
        // 播放中每帧重绘都会重算 hover；指针实际还在面板里时，
        // 这次 false 是布局闪动，不是用户移出，直接忽略。
        // 用**当前**光标位置判断：`window.mouse_position()` 停在最后一次
        // 事件上，鼠标移出窗口后它还是旧位置，会把「真的移出去了」误判成
        // 「还在面板里」—— 面板就再也不会收起了。
        if self.pointer_in_panel_zone(cursor_position(window, cx), window) {
            return;
        }
        self.arm_panel_collapse(window, cx);
    }

    /// 互动弹层（投币 / 收藏夹 / 分享）现在是否开着。
    pub(crate) fn action_overlay_open(&self) -> bool {
        self.coin_panel_open || self.fav_panel_open || self.share_panel_open
    }

    /// 排一个 220ms 的收起定时器。调用方必须已经确认指针在 hover 区外。
    ///
    /// 不在这里再查 `window.mouse_position()`：watchdog 用的是系统级光标，
    /// 而 `Window::mouse_position()` 在快速移出窗口时可能仍停留在旧位置。
    pub(crate) fn arm_panel_collapse(&mut self, window: &Window, cx: &mut Context<Self>) {
        if !self.expanded {
            return;
        }
        // 弹层开着就把面板钉住：弹层是从互动行挂出来的浮层，
        // 指针为了看它 / 点它跑出面板边界，不该连带把面板和弹层一起收掉。
        // 弹层关掉后（外点 / Esc / 再点触发按钮）看门狗会重新判断，该收还是会收。
        if self.action_overlay_open() {
            return;
        }
        // 评论区输入框聚焦时，指针移到视频上也不能把侧栏收走。
        if self.comment_section.focused {
            return;
        }
        // 记下这一刻的光标位置：收起后用它判断「光标有没有动过」。
        self.panel_collapse_cursor = Some(cursor_position(window, cx));
        // watchdog 每 100ms 也会调到这里：已经有定时器在跑就不要再排一个，
        // 否则一直从现在起算 220ms，永远到不了点。
        if self.panel_collapse_pending {
            return;
        }
        self.panel_collapse_pending = true;
        self.panel_collapse_generation = self.panel_collapse_generation.wrapping_add(1);
        let generation = self.panel_collapse_generation;
        cx.spawn(async move |this, cx| {
            cx.background_executor()
                .timer(std::time::Duration::from_millis(220))
                .await;
            let _ = this.update(cx, |view, cx| {
                if view.panel_collapse_generation != generation {
                    // 已经被 set_panel_hover(true) 取消，或已有更新的
                    // 定时器接管；不要在这里清 pending，避免和新定时器打架。
                    return;
                }
                view.panel_collapse_pending = false;
                view.collapse_panel_now(cx);
            });
        })
        .detach();
    }

    /// 面板最近是否在滚动（含滚轮惯性尾部）。
    pub(crate) fn panel_scrolled_recently(&self) -> bool {
        self.panel_last_scroll
            .is_some_and(|at| at.elapsed() < std::time::Duration::from_millis(PANEL_SCROLL_HOLD_MS))
    }

    /// 立即收起面板（鼠标离开窗口 / 收起定时器到点）。
    pub(crate) fn collapse_panel_now(&mut self, cx: &mut Context<Self>) {
        if !self.expanded {
            return;
        }
        self.expanded = false;
        self.panel_collapse_pending = false;
        self.panel_generation = self.panel_generation.wrapping_add(1);
        // 弹层现在已经浮在面板之外：面板一收，它们就会悬在视频画面上。
        // 指针离开面板本来就该关掉弹层，三个一起收。
        self.close_action_overlays(cx);
        // 鼠标可能还停在面板左缘：400ms 内不响应，避免反复弹出。
        self.panel_hover_grace_until =
            Some(std::time::Instant::now() + std::time::Duration::from_millis(400));
        self.panel_hover_grace_pos = self.panel_collapse_cursor.take();
        cx.notify();
    }

    /// 按下进度条：记录拖动开始，并立刻跳到按下位置。
    pub(crate) fn begin_scrub(&mut self, position: Point<Pixels>, window: &Window, cx: &mut Context<Self>) {
        if self.model.state.duration_secs <= 0.0 {
            return;
        }
        self.note_activity(window, cx);
        self.scrubbing = true;
        self.seek_scrub_to(position, true, cx);
    }

    /// 拖动中：指针离开轨道也继续按轨道宽度换算。
    pub(crate) fn update_scrub(&mut self, position: Point<Pixels>, _window: &Window, cx: &mut Context<Self>) {
        if !self.scrubbing {
            return;
        }
        self.seek_scrub_to(position, false, cx);
    }

    /// 松开 / 指针离开窗口：结束拖动，并把最终位置补发给 mpv。
    pub(crate) fn end_scrub(&mut self, cx: &mut Context<Self>) {
        if !self.scrubbing {
            return;
        }
        self.scrubbing = false;
        self.last_scrub_seek = None;
        self.seek(self.model.state.position_secs, cx);
    }

    pub(crate) fn seek_scrub_to(&mut self, position: Point<Pixels>, force: bool, cx: &mut Context<Self>) {
        let Some(bounds) = self.progress_track_bounds.get() else {
            return;
        };
        let width = bounds.size.width.as_f32();
        if width <= 0.0 {
            return;
        }
        let fraction =
            ((position.x.as_f32() - bounds.origin.x.as_f32()) / width).clamp(0.0, 1.0) as f64;
        self.model.state.seek_to_fraction(fraction);
        self.play_time = self.model.state.position_secs;
        // 拖动中约 50ms 才向 mpv 发一次 seek；松开时 force 补最后一刀。
        let due = force
            || self
                .last_scrub_seek
                .is_none_or(|at| at.elapsed() >= std::time::Duration::from_millis(50));
        if due {
            #[cfg(any(target_os = "macos", target_os = "windows"))]
            if let Some(worker) = self.worker.as_ref() {
                worker.seek(self.model.state.position_secs);
            }
            self.last_scrub_seek = Some(std::time::Instant::now());
            self.seek_settle_until =
                Some(std::time::Instant::now() + std::time::Duration::from_millis(400));
        }
        cx.notify();
    }

    /// 用真实指针坐标决定面板展开 / 收起。
    ///
    /// `on_hover` 在播放中会因每帧重绘而抖动；鼠标移动是可靠的，
    /// 所以展开状态以这条路径为准，hover 回调只做兜底。
    pub(crate) fn update_panel_hover_from_pointer(
        &mut self,
        _event_position: Point<Pixels>,
        window: &Window,
        cx: &mut Context<Self>,
    ) {
        // 判定一律用**当前光标位置**，不用事件坐标：事件坐标是那一刻的
        // 位置，而面板展开 / 收起的真正依据是「光标现在在哪」。
        // 两套坐标混用时，面板会在「光标已移开」和「事件还在面板里」
        // 之间反复横跳 —— 表现就是侧栏反复弹出 / 收起。
        let position = cursor_position(window, cx);
        if self.pointer_in_panel_zone(position, window) {
            // 刚收起 / 刚点掉面板时给一小段冷却：光标**没动**就不弹回，
            // 动了就立刻响应（否则用户一动鼠标面板就弹回来，很烦）。
            if let Some(until) = self.panel_hover_grace_until {
                if until > std::time::Instant::now() {
                    let still = self.panel_hover_grace_pos.is_some_and(|at| {
                        (at.x - position.x).abs() < px(2.) && (at.y - position.y).abs() < px(2.)
                    });
                    if still {
                        return;
                    }
                }
            }
            self.set_panel_hover(true, window, cx);
        } else {
            // 真实鼠标已经移出：这次不受「滚动后 400ms 保持」影响，
            // 否则刚滚完就移到右侧边距会迟迟不收起。
            self.panel_last_scroll = None;
            self.set_panel_hover(false, window, cx);
        }
    }

    /// 用一个透明 canvas 注册全局鼠标移动监听。
    ///
    /// `on_mouse_move` 只有在元素的 hover 命中为真时才会回调；播放中每帧重绘
    /// 会让面板 / 播放器容器的 hover 偶发为 false，漏掉「鼠标真的移出面板」。
    /// 这里绕过 hover 命中，直接拿鼠标坐标驱动面板收起。
    pub(crate) fn pointer_tracker(&self, cx: &mut Context<Self>) -> AnyElement {
        let weak = cx.entity().downgrade();
        canvas(
            |_, _, _| (),
            move |_, _, window, cx| {
                let weak_move = weak.clone();
                window.on_mouse_event(move |event: &MouseMoveEvent, phase, window, cx| {
                    if phase != DispatchPhase::Bubble {
                        return;
                    }
                    let _ = weak_move.update(cx, |view, cx| {
                        if view.pointer_in_player(event.position, window) {
                            view.note_activity(window, cx);
                        }
                        view.update_scrub(event.position, window, cx);
                        view.update_panel_hover_from_pointer(event.position, window, cx);
                    });
                });
                let weak_down = weak.clone();
                window.on_mouse_event(move |event: &MouseDownEvent, phase, window, cx| {
                    if phase != DispatchPhase::Bubble || event.button != MouseButton::Left {
                        return;
                    }
                    let _ = weak_down.update(cx, |view, cx| {
                        // 音量弹出条：点它外面任何地方都收起；点音量图标本身
                        // 留给按钮的 on_click 做「切换」，否则会先关再开。
                        if view.volume_popover_open {
                            let inside_popover = view
                                .volume_popover_bounds
                                .get()
                                .is_some_and(|bounds| bounds.contains(&event.position));
                            let on_button = view
                                .volume_button_bounds
                                .get()
                                .is_some_and(|bounds| bounds.contains(&event.position));
                            if !inside_popover && !on_button {
                                view.close_volume_popover(cx);
                            }
                        }
                        // 画质 / 倍速菜单同理：点菜单外收起；点胶囊本身留给 on_click 切换。
                        if view.quality_popover_open || view.speed_popover_open {
                            let inside_popover = view
                                .control_popover_bounds
                                .get()
                                .is_some_and(|bounds| bounds.contains(&event.position));
                            let on_quality_button = view
                                .quality_button_bounds
                                .get()
                                .is_some_and(|bounds| bounds.contains(&event.position));
                            let on_speed_button = view
                                .speed_button_bounds
                                .get()
                                .is_some_and(|bounds| bounds.contains(&event.position));
                            if !inside_popover && !on_quality_button && !on_speed_button {
                                view.close_control_popovers(cx);
                            }
                        }
                        // 展开的面板盖在控制条右侧之上，点面板不能误触进度条。
                        let panel_covers =
                            view.expanded && view.pointer_in_panel_zone(event.position, window);
                        let on_track = view.model.controls_visible
                            && !panel_covers
                            && view
                                .progress_track_bounds
                                .get()
                                .is_some_and(|bounds| bounds.contains(&event.position));
                        if on_track {
                            view.begin_scrub(event.position, window, cx);
                            cx.stop_propagation();
                        }
                    });
                });
                let weak_up = weak.clone();
                window.on_mouse_event(move |_: &MouseUpEvent, phase, _window, cx| {
                    if phase != DispatchPhase::Bubble {
                        return;
                    }
                    let _ = weak_up.update(cx, |view, cx| view.end_scrub(cx));
                });
                // 指针直接离开窗口时没有 MouseMove / MouseUp；这里补一次收尾。
                window.on_mouse_event(move |event: &MouseExitEvent, phase, window, cx| {
                    if phase != DispatchPhase::Bubble {
                        return;
                    }
                    let position = event.position;
                    let _ = weak.update(cx, |view, cx| {
                        view.end_scrub(cx);
                        // 播放中每帧重绘会改变布局，GPUI 会因此偶发派发
                        // 播放器区域的 MouseExit（指针其实还在面板里）。
                        // 面板收起不在这里做：统一交给真实鼠标坐标 + 220ms 延迟，
                        // 否则视频每帧都可能把侧栏收掉再弹回来。
                        if !view.pointer_in_window(position, window) {
                            view.hide_controls(cx);
                        }
                    });
                });
            },
        )
        .absolute()
        .inset_0()
        .into_any_element()
    }

    /// 鼠标移动：位置真的变了才算活动。滚轮/重绘触发的重复 move 不应反复叫出控制栏。
    pub(crate) fn note_pointer_move(
        &mut self,
        position: Point<Pixels>,
        window: &Window,
        cx: &mut Context<Self>,
    ) {
        let dx = (position.x.as_f32() - self.last_pointer.x.as_f32()).abs();
        let dy = (position.y.as_f32() - self.last_pointer.y.as_f32()).abs();
        self.last_pointer = position;
        // 滚轮/重绘可能带出位置几乎不变的 move；2px 以内不算「移动」。
        if dx < 2.0 && dy < 2.0 {
            return;
        }
        self.note_activity(window, cx);
    }

    /// 鼠标 / 键盘活动：控制栏浮出，并重新开始 5 秒空闲计时。
    ///
    /// 只在窗口有焦点时响应：窗口在后台时鼠标掠过不应该把控制栏叫出来。
    pub(crate) fn note_activity(&mut self, window: &Window, cx: &mut Context<Self>) {
        if !window.is_window_active() {
            return;
        }
        self.last_activity = std::time::Instant::now();
        if !self.model.controls_visible {
            self.model.controls_visible = true;
            cx.notify();
        }
        if !self.model.controls_timer_running {
            self.arm_controls_timer(cx);
        }
    }

    /// 起一个 250ms 轮询的 5 秒空闲计时；可见后一直挂着，隐藏时退出。
    pub(crate) fn arm_controls_timer(&mut self, cx: &mut Context<Self>) {
        self.model.controls_timer_running = true;
        self.model.controls_timer_generation = self.model.controls_timer_generation.wrapping_add(1);
        let generation = self.model.controls_timer_generation;
        cx.spawn(async move |this, cx| {
            loop {
                cx.background_executor()
                    .timer(std::time::Duration::from_millis(250))
                    .await;
                let keep = this.update(cx, |view, cx| {
                    if view.model.controls_timer_generation != generation {
                        return false;
                    }
                    if !view.model.controls_visible {
                        return false;
                    }
                    // 正在输入弹幕 / 拖动进度条 / 调音量 / 开着画质或倍速菜单时不自动隐藏。
                    if view.input_focused
                        || view.scrubbing
                        || view.volume_popover_open
                        || view.quality_popover_open
                        || view.speed_popover_open
                    {
                        view.last_activity = std::time::Instant::now();
                        return true;
                    }
                    if view.last_activity.elapsed() >= std::time::Duration::from_secs(5) {
                        view.model.controls_visible = false;
                        view.model.controls_timer_running = false;
                        view.model.controls_timer_generation =
                            view.model.controls_timer_generation.wrapping_add(1);
                        cx.notify();
                        return false;
                    }
                    true
                });
                if !matches!(keep, Ok(true)) {
                    break;
                }
            }
        })
        .detach();
    }

    /// 立即藏起控制栏（播放开始 / 窗口失焦 / 鼠标离开时用）。
    pub(crate) fn hide_controls(&mut self, cx: &mut Context<Self>) {
        // 拖动中收控制条 / 窗口失焦：先把拖动收尾，避免 scrub 状态悬空。
        if self.scrubbing {
            self.end_scrub(cx);
        }
        // 音量弹出条挂在控制条里，控制条一收它也必须跟着消失。
        if self.volume_popover_open {
            self.volume_popover_open = false;
            self.volume_popover_bounds.set(None);
        }
        // 画质 / 倍速菜单同样挂在控制条里。
        self.quality_popover_open = false;
        self.speed_popover_open = false;
        self.control_popover_bounds.set(None);
        // 正在输入弹幕时不能把输入框从树里拿掉，等失焦后再走自动隐藏。
        if self.input_focused {
            return;
        }
        self.model.controls_timer_generation = self.model.controls_timer_generation.wrapping_add(1);
        self.model.controls_timer_running = false;
        if self.model.controls_visible {
            self.model.controls_visible = false;
            cx.notify();
        }
    }

    /// 算出这次的起播位置（秒）；0 表示从头播。
    ///
    /// 三个来源取更靠后的：
    /// - 本地续播记录（`bvid#cid`，退而求其次只用 `bvid`）；
    /// - 打开时带进来的历史进度（`resume_seed`）；
    /// - 服务端观看历史里的进度（`remote`，本地没记录时才会去查）。
    ///
    /// 规则见 [`yk_infra::resume::resolve_start_position`]：刚开始看 / 已看完
    /// 都从头播。
    pub(crate) fn resolve_resume_position(
        &mut self,
        src: &yk_domain::playback::PlaybackSource,
        remote: Option<f64>,
    ) -> f64 {
        let duration = src.duration_secs as f64;
        let key = yk_infra::resume::key(&src.bvid, src.cid);
        let local = self
            .resume
            .resume_position(&key)
            .or_else(|| self.resume.resume_position(&src.bvid));
        let seed = self.resume_seed;
        let start = yk_infra::resume::resolve_start_position(&[local, seed, remote], duration)
            .unwrap_or(0.0);
        self.resume_key = Some(key);
        if start > 0.0 {
            eprintln!(
                "[resume] 从 {:.1}s 起播：{}（本地 {:?} / 历史 {:?} / 服务端 {:?}，时长 {}s）",
                start, src.bvid, local, seed, remote, src.duration_secs
            );
        } else {
            eprintln!("[resume] 无可续播记录，从头播：{}", src.bvid);
        }
        start
    }

    /// 把当前播放位置写进本地续播记录。
    ///
    /// 只在明确的节点调用（关闭 / 换源 / 播放中定期落盘），
    /// 不在每帧写盘。
    pub(crate) fn save_resume_position(&mut self) {
        let Some(key) = self.resume_key.clone() else {
            return;
        };
        let position = self.play_time.max(0.0);
        let duration = self.model.state.duration_secs;
        self.resume.record(&key, position, duration);
        // 只按 bvid 存一份作为兜底（cid 未知时仍能续上）。
        let bvid = key.split('#').next().unwrap_or_default().to_string();
        if !bvid.is_empty() && bvid != key {
            self.resume.record(&bvid, position, duration);
        }
        if let Err(e) = self.resume.save() {
            eprintln!("[resume] 写盘失败：{e}");
        }
    }

    /// 打开视频时上报一次历史（进度 = 起播位置，状态「已观看」）。
    ///
    /// 不做周期心跳：只在打开 / 关闭两个时机各报一次。关闭时是否再报，
    /// 由 [`Self::flush_history_report`] 判断「打开是否已满 5 秒」。
    ///
    /// `start_position` 是这次实际的起播位置（续播时不是 0）：
    /// 服务端进度直接写成它，避免「续播中」的历史条目被重置成 0%。
    pub(crate) fn begin_history_report(&mut self, aid: i64, bvid: String, cid: i64, start_position: f64) {
        let Some(session) = current_session() else {
            self.history_report = None;
            return;
        };
        if aid <= 0 || cid <= 0 || bvid.trim().is_empty() {
            self.history_report = None;
            eprintln!("[history-report] 缺少 aid/cid/bvid，跳过：aid={aid} cid={cid} bvid={bvid}");
            return;
        }
        let report = HistoryReport {
            aid,
            bvid: bvid.clone(),
            cid,
            cookie: session.cookie_header(),
            csrf: session.bili_jct.clone(),
            opened_at: std::time::Instant::now(),
        };
        eprintln!(
            "[history-report] 打开上报：{bvid}（aid={aid} cid={cid} progress={:.0}s）",
            start_position
        );
        dispatch_history_report(&report, start_position.max(0.0) as u64);
        self.history_report = Some(report);
    }

    /// 关闭 / 换源时补报一次；打开不足 5 秒则只保留打开时的那一次。
    pub(crate) fn flush_history_report(&mut self) {
        let Some(mut report) = self.history_report.take() else {
            return;
        };
        if report.opened_at.elapsed() < std::time::Duration::from_secs(5) {
            eprintln!(
                "[history-report] 打开不足 5s，跳过关闭上报：{}（{:.1}s）",
                report.bvid,
                report.opened_at.elapsed().as_secs_f32()
            );
            return;
        }
        let position = self.play_time.max(0.0) as u64;
        eprintln!("[history-report] 关闭上报：{} @ {position}s", report.bvid);
        dispatch_history_report(&report, position);
    }

    /// AppRoot 注入：关闭时把自己的 `playing` 置空。
    /// 测试入口：读侧栏（信息面板）当前是否展开。
    pub fn panel_expanded(&self) -> bool {
        self.expanded
    }

    /// 测试用：当前播放状态 (playing, position_secs, volume, muted)。
    pub fn test_playback_state(&self) -> (bool, f64, f64, bool) {
        (
            self.model.state.playing,
            self.model.state.position_secs,
            self.model.state.volume,
            self.model.state.muted,
        )
    }

    /// 测试用：音量弹出条是否展开。
    pub fn test_volume_popover_open(&self) -> bool {
        self.volume_popover_open
    }

    /// 测试用：切换音量弹出条（等价于点控制栏音量图标）。
    pub fn test_toggle_volume_popover(&mut self, cx: &mut Context<Self>) {
        self.toggle_volume_popover(cx);
    }

    /// 测试入口：互动行的实际几何（窗口坐标）。
    /// 三个互动弹层按它定位，`None` 表示这一帧还没 prepaint。
    #[doc(hidden)]
    pub fn test_action_row_bounds(&self) -> Option<Bounds<Pixels>> {
        self.action_row_bounds.get()
    }

    /// 测试入口：直接打开收藏夹弹层（跳过登录检查，也不拉接口）。
    #[doc(hidden)]
    pub fn test_open_fav_overlay(&mut self, cx: &mut Context<Self>) {
        self.coin_panel_open = false;
        self.share_panel_open = false;
        self.fav_panel_open = true;
        self.note_overlay_opened(cx);
    }

    /// 测试入口：读三个互动面板的展开状态（投币 / 收藏 / 分享）。
    pub fn action_panels_open(&self) -> (bool, bool, bool) {
        (
            self.coin_panel_open,
            self.fav_panel_open,
            self.share_panel_open,
        )
    }

    /// 测试 / 截图入口：直接灌一批评论（不走网络），评论 Tab 走真实渲染路径。
    ///
    /// `is_end = false` 时列表底部显示「正在加载更多评论…」之外的正常态，
    /// 可以配合 [`Self::set_test_scroll`] 目检窗口化渲染。
    #[doc(hidden)]
    pub fn with_comments(
        mut self,
        items: Vec<DynamicComment>,
        total: u64,
        is_end: bool,
    ) -> Self {
        self.comment_section.list.items = items;
        self.comment_section.list.total = total;
        self.comment_section.list.is_end = is_end;
        self.comments = CommentState::Ready;
        self.tab = 0;
        self
    }

    /// 测试 / 截图入口：把信息面板滚到指定位置（负数向下滚）。
    #[doc(hidden)]
    pub fn set_test_scroll(&mut self, y: f32) {
        self.panel_scroll.set_offset(point(px(0.), px(y)));
    }

    /// 测试入口：信息面板当前的滚动偏移（负数向下滚）。
    #[doc(hidden)]
    pub fn test_scroll_offset(&self) -> f32 {
        self.panel_scroll.offset().y.as_f32()
    }

    /// 测试入口：最近一帧实际创建了几个评论行（窗口化渲染验收）。
    #[doc(hidden)]
    pub fn rendered_comment_rows(&self) -> usize {
        self.comment_section.rendered_rows
    }

    /// 测试入口：评论输入条是否展开 / 输入框是否拿到焦点。
    #[doc(hidden)]
    pub fn test_composer_state(&self) -> (bool, bool) {
        (
            self.comment_section.expanded,
            self.comment_section.focused,
        )
    }

    /// 测试 / 截图入口：把侧栏切到展开态（400 宽）。
    pub fn with_expanded(mut self, expanded: bool) -> Self {
        self.expanded = expanded;
        self
    }

    /// 测试 / 截图入口：预先展开倍速菜单。
    pub fn with_speed_menu(mut self) -> Self {
        self.force_controls_visible = true;
        self.speed_popover_open = true;
        self
    }

    /// 测试 / 截图入口：塞一份「大会员 + 非大会员」画质清单并展开菜单。
    pub fn with_quality_menu(mut self) -> Self {
        use yk_domain::playback::{PlaybackQuality, PlaybackSource};
        self.force_controls_visible = true;
        self.source = Some(PlaybackSource {
            title: "测试视频".into(),
            quality: "1080P 高清".into(),
            quality_id: 80,
            duration_secs: 754,
            qualities: vec![
                PlaybackQuality { id: 127, label: "8K 超高清".into(), vip_only: true },
                PlaybackQuality { id: 120, label: "4K 超清".into(), vip_only: true },
                PlaybackQuality { id: 116, label: "1080P 60帧".into(), vip_only: true },
                PlaybackQuality { id: 80, label: "1080P 高清".into(), vip_only: false },
                PlaybackQuality { id: 64, label: "720P 高清".into(), vip_only: false },
                PlaybackQuality { id: 32, label: "480P 清晰".into(), vip_only: false },
                PlaybackQuality { id: 16, label: "360P 流畅".into(), vip_only: false },
            ],
            ..Default::default()
        });
        // 截图不启动解码线程：空 URL 交给 mpv 只会刷错误日志。
        self.tried_worker = true;
        self.quality_popover_open = true;
        self
    }

    /// 测试 / 截图入口：直接灌一批弹幕（不走网络请求），并把播放位置拨到
    /// `at` 秒。弹幕排布与绘制和真实数据走完全同一条路径，用来目检
    /// 「弹幕层是不是跟播放层对齐」。
    #[doc(hidden)]
    pub fn with_danmaku(mut self, set: DanmakuSet, at: f64) -> Self {
        self.model.state.position_secs = at;
        self.play_time = at;
        self.install_danmaku(set);
        self
    }

    /// 测试 / 截图入口：把互动区的展开面板预先打开。
    ///
    /// 分享面板是这次新增的面板，截图验收时用它检查排版；
    /// 传 `("", false, false)` 就全部收起。
    pub fn with_open_panels(mut self, share: bool, coin: bool, fav: bool) -> Self {
        self.share_panel_open = share;
        self.coin_panel_open = coin;
        self.fav_panel_open = fav;
        if fav {
            // 截图用的夹具按线上真实形态铺：十几夹、几十条、两条已收藏，
            // 这样折叠 / 展开、勾选态、长标题截断都能一次看到。
            let fixtures: [(&str, u64, i64); 14] = [
                ("默认收藏夹", 257, 1),
                ("硬件", 2, 1),
                ("硬件科普", 1, 0),
                ("身体护理", 1, 0),
                ("音乐", 14, 0),
                ("NAS", 3, 0),
                ("锻炼", 3, 0),
                ("Swift", 1, 0),
                ("tauri", 1, 0),
                ("AI", 12, 0),
                ("原神", 15, 0),
                ("弹射", 1, 0),
                ("环", 1, 0),
                ("书", 4, 0),
            ];
            self.fav_folders = fixtures
                .iter()
                .enumerate()
                .map(|(index, (title, count, state))| yk_infra::FavFolder {
                    id: index as i64 + 1,
                    title: (*title).into(),
                    media_count: *count,
                    attr: if index == 0 { 1 } else { 0 },
                    fav_state: *state,
                })
                .collect();
        }
        self
    }

    pub fn set_image_handler(
        &mut self,
        handler: impl Fn(&mut App, &mut Window, Vec<String>, usize) + 'static,
    ) {
        self.image_handler = Some(std::rc::Rc::new(handler));
    }

    pub fn set_close_handler(&mut self, handler: impl Fn(&mut App, bool) + 'static) {
        self.close_handler = Some(Box::new(handler));
    }

    /// 注入「点头像打开 UP 主浮窗」的回调。播放器不认识 AppRoot，
    /// 只把 mid / 昵称 / 头像交出去，开哪一层由根节点决定。
    pub fn set_up_profile_handler(
        &mut self,
        handler: impl Fn(&mut App, &mut Window, i64, String, String, Bounds<Pixels>) + 'static,
    ) {
        self.up_profile_handler = Some(Box::new(handler));
    }

    pub(crate) fn escape_hint_overlay(&self, t: &YkTheme, cx: &mut Context<Self>) -> Option<AnyElement> {
        if !self.escape_hint {
            return None;
        }
        Some(
            div()
                .id("player-esc-hint")
                .absolute()
                // 顶栏右侧：在浮窗/视频之外，不遮画面。
                .top(px((TOP_BAR_HEIGHT - 32.) / 2.))
                .right(px(SPACE_4))
                .occlude()
                .h(px(32.))
                .px(px(SPACE_2))
                .flex()
                .items_center()
                .gap(px(SPACE_2))
                .rounded(px(INTERACTIVE_RADIUS))
                .bg(t.surfaces.surface_elevated_strong)
                .border_1()
                .border_color(t.surfaces.border_popover)
                .shadow(t.shadows.shadow_2.clone())
                .child(
                    div()
                        .w(px(18.))
                        .h(px(18.))
                        .flex()
                        .items_center()
                        .justify_center()
                        .rounded(px(RADIUS_SM))
                        .bg(t.palette.status_error)
                        .text_size(px(FONT_SIZE_MICRO))
                        .text_color(rgb(0xFFFFFF))
                        .child("×"),
                )
                .child(
                    div()
                        .text_size(px(FONT_SIZE_CAPTION))
                        .text_color(t.surfaces.text_secondary)
                        .child("再次按 Esc 关闭"),
                )
                .child(
                    div()
                        .px(px(4.))
                        .h(px(18.))
                        .flex()
                        .items_center()
                        .rounded(px(RADIUS_SM))
                        .border_1()
                        .border_color(t.surfaces.border_strong)
                        .text_size(px(FONT_SIZE_MICRO))
                        .text_color(t.surfaces.text_tertiary)
                        .child("Esc"),
                )
                .on_click(cx.listener(|this, _: &ClickEvent, window, cx| {
                    cx.stop_propagation();
                    this.request_close(window, cx);
                }))
                .into_any_element(),
        )
    }

    /// 第一次 Esc：显示提示，5 秒后自动收回；期间再按 Esc 才关闭。
    pub(crate) fn arm_escape_hint(&mut self, cx: &mut Context<Self>) {
        self.escape_hint = true;
        let generation = self.escape_hint_generation.wrapping_add(1);
        self.escape_hint_generation = generation;
        cx.notify();
        cx.spawn(async move |this, cx| {
            cx.background_executor()
                .timer(std::time::Duration::from_secs(5))
                .await;
            let _ = this.update(cx, |view, cx| {
                if view.escape_hint_generation == generation && view.escape_hint {
                    view.escape_hint = false;
                    cx.notify();
                }
            });
        })
        .detach();
    }

    pub(crate) fn request_close(&mut self, _window: &mut Window, cx: &mut Context<Self>) {
        self.begin_close(cx);
    }

    /// 关闭状态机本体；不依赖 Window，方便关闭请求从 Esc/遮罩/按钮统一进入。
    pub(crate) fn begin_close(&mut self, cx: &mut Context<Self>) {
        if self.closing {
            return;
        }
        // 关闭前补最后一拍；打开不足 5s 的会在 flush 里被冷却拦掉。
        self.flush_history_report();
        // 关闭时把当前位置存进本地续播记录，下次打开从这里继续。
        self.save_resume_position();
        self.escape_hint = false;
        self.escape_hint_generation = self.escape_hint_generation.wrapping_add(1);

        // 先恢复页面布局：关闭起点是“打开前”的封面坐标，分类条必须回来，
        // 否则网格还压在上移后的位置，缩回终点会对不上卡片。
        if let Some(handler) = self.close_handler.as_ref() {
            handler(cx, false);
        }

        // 没有封面快照（YK_BVID/YK_VIDEO 程序化打开）就直接收掉。
        if self.entrance_source.is_none() {
            if let Some(handler) = self.close_handler.as_ref() {
                handler(cx, true);
            }
            return;
        }

        self.closing = true;
        self.closing_generation = EXIT_SEQ.fetch_add(1, std::sync::atomic::Ordering::Relaxed);
        let generation = self.closing_generation;
        cx.notify();
        cx.spawn(async move |this, cx| {
            cx.background_executor()
                .timer(std::time::Duration::from_millis(PLAYER_EXIT_MS + 30))
                .await;
            let _ = this.update(cx, |view, cx| {
                if !view.closing || view.closing_generation != generation {
                    return;
                }
                view.closing = false;
                if let Some(handler) = view.close_handler.as_ref() {
                    handler(cx, true);
                }
            });
        })
        .detach();
    }

    /// 播放到结尾：按设置决定是否自动收起播放器。
    ///
    /// 关闭走的是和 Esc / 点浮窗外侧同一条 [`Self::begin_close`] 路径，
    /// 所以退出动画、历史上报、续播落盘都不会漏。
    /// 设置项关闭时只停在最后一帧并叫出控制栏，交给用户自己关。
    pub(crate) fn on_playback_finished(&mut self, cx: &mut Context<Self>) {
        if self.finish_handled {
            return;
        }
        self.finish_handled = true;
        self.model.state.playing = false;
        // 位置对齐到结尾：续播记录据此判定「已看完」并清掉，下次从头播。
        if self.model.state.duration_secs > 0.0 {
            self.play_time = self.model.state.duration_secs;
            self.model.state.position_secs = self.play_time;
        }
        self.save_resume_position();
        if !yk_infra::settings::current().close_player_on_finish {
            eprintln!("[player] 播放结束：自动关闭已在设置里关闭，保留浮窗");
            self.model.controls_visible = true;
            self.last_activity = std::time::Instant::now();
            cx.notify();
            return;
        }
        eprintln!("[player] 播放结束：自动关闭播放器");
        self.begin_close(cx);
    }

    pub(crate) fn panel_width(&self) -> f32 {
        if self.expanded {
            INFO_PANEL_EXPANDED
        } else {
            INFO_PANEL_COLLAPSED
        }
    }

    /// 视频按 Contain 放进播放层后的实际宽度与左偏移。
    ///
    /// 播放层宽度 = 窗口 − 两侧 16 − 收起态侧栏 200；高度 = 窗口 − 顶栏下方 16
    /// − 底部 92。控制条跟着这段**实际画面**走，而不是铺满整个播放层。
    pub(crate) fn video_bar_geometry(&self, window: &Window) -> (f32, f32) {
        let viewport = window.viewport_size();
        let available_w = playback_layer_width(window, INFO_PANEL_COLLAPSED);
        let available_h =
            (viewport.height.as_f32() - (TOP_BAR_HEIGHT + SPACE_4) - PLAYER_BOTTOM_INSET).max(1.0);
        let (video_w, video_h) = self.video_pixel_size();
        if video_w <= 0.0 || video_h <= 0.0 {
            return (0.0, available_w);
        }
        let scale = (available_w / video_w).min(available_h / video_h);
        let width = (video_w * scale).min(available_w);
        ((available_w - width) / 2.0, width)
    }

    /// Windows：把最新一帧 BGRA 包成 GPUI 的 `RenderImage`。
    ///
    /// 两个必须做的收尾：
    /// 1. 每帧新建 `RenderImage` 会拿到新 `ImageId`，GPUI 的 DirectX 图集会为它
    ///    分配一张纹理；不显式 `drop_image` 旧图，显存会随播放时间线性上涨。
    /// 2. 同一帧号不重复建图：render 可能被 hover / 动画触发多次，
    ///    但视频帧（16ms 一条）并没有变。
    #[cfg(target_os = "windows")]
    pub(crate) fn sync_video_image(&mut self, window: &mut Window) {
        if self.needs_image_clear {
            self.needs_image_clear = false;
            if let Some(old) = self.current_image.take() {
                let _ = window.drop_image(old);
            }
            self.current_image_serial = None;
        }

        let Some(serial) = self.current.as_ref().map(|frame| frame.serial) else {
            return;
        };
        if self.current_image_serial == Some(serial) {
            return;
        }

        let frame = self.current.take().expect("上面刚确认过 current 存在");
        let Some(buffer) = image::RgbaImage::from_raw(frame.width, frame.height, frame.bgra) else {
            eprintln!(
                "[player] 视频帧尺寸不匹配：{}x{} / {} 字节",
                frame.width,
                frame.height,
                frame.width as usize * frame.height as usize * 4
            );
            return;
        };
        // 字节序已经是 BGRA（mpv 的 `bgra` 格式），而 `image::Frame` 只是
        // 一个「RgbaImage 容器」；GPUI 的 `paint_image` 原样上传，
        // 所以这里不需要再做通道交换。
        let image = std::sync::Arc::new(RenderImage::new(vec![image::Frame::new(buffer)]));
        if let Some(old) = self.current_image.replace(image) {
            let _ = window.drop_image(old);
        }
        self.current_image_serial = Some(serial);
    }

    /// 当前视频的像素尺寸：优先用已解码帧，其次用 playurl 返回的分辨率。
    pub(crate) fn video_pixel_size(&self) -> (f32, f32) {
        #[cfg(target_os = "macos")]
        if let Some(buffer) = self.current.as_ref() {
            let width = objc2_core_video::CVPixelBufferGetWidth(buffer) as f32;
            let height = objc2_core_video::CVPixelBufferGetHeight(buffer) as f32;
            if width > 0.0 && height > 0.0 {
                return (width, height);
            }
        }
        if let Some(source) = self.source.as_ref() {
            if source.video_width > 0 && source.video_height > 0 {
                return (source.video_width as f32, source.video_height as f32);
            }
        }
        (0.0, 0.0)
    }

    /// 指针是否还在右侧面板的交互区（面板 + 展开态骑边界按钮的 16px 外沿）。
    ///
    /// 收起态只认 200px 面板本身：视频右缘不再铺 88px 唤出热区，
    /// 鼠标在画面上移动不会把侧栏弹出来。
    /// GPUI 的 `on_hover(false)` 会在布局变化时重算；播放中每帧都有重绘，
    /// 面板内滚动 / 控制条显隐都可能让它在指针没动时闪一次 false。
    /// 收起前用真实鼠标坐标兜底，避免面板被这种假移出事件收掉。
    pub(crate) fn pointer_in_panel_zone(&self, position: Point<Pixels>, window: &Window) -> bool {
        let viewport = window.viewport_size();
        // 播放器窗口贴窗口右缘内缩 SPACE_4；面板再贴播放器右缘。
        // 右边界以内缩后的面板右缘为准：移出到窗口最右 16px 边距
        // 也算离开 hover 区，延迟 220ms 后收起面板。
        let right = viewport.width - px(SPACE_4);
        // 展开态给骑边界按钮留 16px；收起态就是面板本身。
        let width = if self.expanded {
            INFO_PANEL_EXPANDED + 16.0
        } else {
            INFO_PANEL_COLLAPSED
        };
        let top = px(TOP_BAR_HEIGHT + SPACE_4);
        let bottom = viewport.height - px(PLAYER_BOTTOM_INSET);
        position.x >= right - px(width)
            && position.x <= right + px(1.)
            && position.y >= top - px(1.)
            && position.y <= bottom + px(1.)
    }

    /// 指针是否还在窗口内（含四周 16px 边距）。
    pub(crate) fn pointer_in_window(&self, position: Point<Pixels>, window: &Window) -> bool {
        let viewport = window.viewport_size();
        position.x >= px(0.)
            && position.x <= viewport.width
            && position.y >= px(0.)
            && position.y <= viewport.height
    }

    /// 指针是否还在播放器窗口内（不是顶栏 / Dock / 页面）。
    pub(crate) fn pointer_in_player(&self, position: Point<Pixels>, window: &Window) -> bool {
        let viewport = window.viewport_size();
        let top = px(TOP_BAR_HEIGHT + SPACE_4);
        let bottom = viewport.height - px(PLAYER_BOTTOM_INSET);
        position.x >= px(SPACE_4) - px(1.)
            && position.x <= viewport.width - px(SPACE_4) + px(1.)
            && position.y >= top - px(1.)
            && position.y <= bottom + px(1.)
    }
}

/// 入场/退场共用一条平滑曲线：EaseStandard 起手，尾部约 0.9% 过冲后
/// 自然回到 1。单段曲线没有速度断点，缩放过程不会在 300ms 附近「顿一下」。
pub(crate) fn entrance_progress(raw: f32) -> f32 {
    ease_entrance()(raw.clamp(0.0, 1.0))
}

pub(crate) fn lerp_bounds(a: Bounds<Pixels>, b: Bounds<Pixels>, t: f32) -> Bounds<Pixels> {
    let mix = |a: Pixels, b: Pixels| px(a.as_f32() + (b.as_f32() - a.as_f32()) * t);
    Bounds {
        origin: point(mix(a.origin.x, b.origin.x), mix(a.origin.y, b.origin.y)),
        size: size(
            mix(a.size.width, b.size.width),
            mix(a.size.height, b.size.height),
        ),
    }
}

impl PlayerView {
    /// 播放器快捷键的统一入口。
    ///
    /// **必须同时挂在「直开」外壳和「封面入场」外壳上**：从卡片点开时，
    /// 顶层元素是 `PlayerEntrance` 而不是下面的普通 div，只在普通 div 上挂
    /// 监听器会让正常使用路径（卡片打开）完全收不到键。
    pub(crate) fn handle_player_key(
        &mut self,
        event: &KeyDownEvent,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        // 输入框抢键盘时，空格 / 方向键属于「打字 / 移动光标」，
        // 不能被播放器快捷键劫走。Enter / Esc 仍按原逻辑走。
        let typing = self.input_focused;
        // 注意不能直接用 `modifiers.modified()`：macOS 给方向键带上
        // `NSFunctionKeyMask`（function = true），那样左右 / 上下会被误判成
        // 「带修饰键」而全部失效。只挡真正会改变语义的四个修饰键。
        let m = event.keystroke.modifiers;
        let plain = !(m.control || m.alt || m.shift || m.platform);
        match event.keystroke.key.as_str() {
            // 空格：播放 / 暂停。
            "space" if plain && !typing => {
                self.toggle_play(window, cx);
                cx.stop_propagation();
            }
            // 左右：快退 / 快进 5 秒。
            "left" if plain && !typing => {
                self.seek_relative(-5.0, window, cx);
                cx.stop_propagation();
            }
            "right" if plain && !typing => {
                self.seek_relative(5.0, window, cx);
                cx.stop_propagation();
            }
            // 上下：音量 -/+ 5%。
            "up" if plain && !typing => {
                let volume = (self.model.state.volume + 0.05).clamp(0.0, 1.0);
                self.set_volume(volume, window, cx);
                cx.stop_propagation();
            }
            "down" if plain && !typing => {
                let volume = (self.model.state.volume - 0.05).clamp(0.0, 1.0);
                self.set_volume(volume, window, cx);
                cx.stop_propagation();
            }
            // 回车：分享面板开着时复制链接（面板上有对应的键位提示）。
            "enter" => {
                if self.handle_enter(cx) {
                    cx.stop_propagation();
                }
            }
            "escape" => {
                // 弹层开着时 Esc 只关弹层（APG：Esc 关闭并把焦点还给触发按钮），
                // 别一按就进「再按一次退出播放器」——用户想关的是面前这张卡。
                if self.close_action_overlays(cx) {
                    cx.stop_propagation();
                    return;
                }
                if self.escape_hint {
                    self.request_close(window, cx);
                } else {
                    self.arm_escape_hint(cx);
                }
                cx.stop_propagation();
            }
            _ => {}
        }
    }
}

/// 回车键位提示（分享面板「复制链接」行尾）。
///
/// 用组件库的 `Kbd`：macOS 上渲染成 `⏎`，别的平台渲染成 `Enter` ——
/// 平台自带的键位符号，比自己拼一个文本胶囊更贴手。
pub(crate) fn enter_key_hint() -> AnyElement {
    match Keystroke::parse("enter") {
        Ok(stroke) => Kbd::new(stroke).into_any_element(),
        // 解析不出来就不画提示：宁可没有，也不能画一个假的键位。
        Err(_) => div().into_any_element(),
    }
}

/// 入场/退场共用的封面海报：渐变兜底 + 标题垫底 + 真实缩略图。
pub(crate) fn cover_poster(theme: &YkTheme, source: &CoverSource) -> AnyElement {
    let mut poster = div().absolute().inset_0().bg(yk_components::cover::placeholder_gradient(
        &source.placeholder_color,
        theme.surfaces.fill_3,
    ));
    if !source.title.is_empty() {
        poster = poster.child(
            div()
                .absolute()
                .inset_0()
                .p(px(SPACE_3))
                .flex()
                .items_center()
                .justify_center()
                .text_size(px(16.))
                .line_height(px(20.))
                .font_weight(FontWeight::BOLD)
                .text_color(rgb(0xFFFFFF))
                .text_center()
                .line_clamp(3)
                .child(source.title.clone()),
        );
    }
    if !source.image_url.is_empty() {
        poster = poster.child(
            img(ImageSource::from(source.image_url.clone()))
                .absolute()
                .inset_0()
                .w_full()
                .h_full()
                .object_fit(ObjectFit::Cover),
        );
    }
    poster.into_any_element()
}

/// 从封面几何展开到播放器目标尺寸。
///
/// 过渡期只挂封面海报，到 `progress == 1` 才挂完整播放器内容：
/// 控制栏/信息面板不会被塞进 192×108 的盒子，也不会让 deferred 的
/// 面板唤回按钮从窗口边缘跑出去。
#[derive(gpui::IntoElement)]
pub(crate) struct PlayerEntrance {
    pub(crate) theme: YkTheme,
    pub(crate) source: CoverSource,
    pub(crate) content: Option<AnyElement>,
    pub(crate) focus_handle: FocusHandle,
    /// 全套播放器快捷键。用 `Rc` 是因为动画每帧会用 `..el` 重建结构体，
    /// 必须能克隆；`Box<dyn Fn>` 在这里不可 Clone。
    pub(crate) on_key: Rc<dyn Fn(&KeyDownEvent, &mut Window, &mut App)>,
    pub(crate) progress: f32,
}

impl RenderOnce for PlayerEntrance {
    fn render(self, _window: &mut Window, _cx: &mut App) -> impl IntoElement {
        let PlayerEntrance {
            theme,
            source,
            content,
            focus_handle,
            on_key,
            progress,
        } = self;
        let p = entrance_progress(progress);
        let root_w = source.root_size.width.as_f32().max(2.0 * SPACE_4 + 1.0);
        let root_h = source
            .root_size
            .height
            .as_f32()
            .max(TOP_BAR_HEIGHT + SPACE_4 + PLAYER_BOTTOM_INSET + 1.0);
        let top = TOP_BAR_HEIGHT + SPACE_4;
        let destination = Bounds {
            origin: point(px(SPACE_4), px(top)),
            size: size(
                px(root_w - 2.0 * SPACE_4),
                px((root_h - top - PLAYER_BOTTOM_INSET).max(1.0)),
            ),
        };
        let bounds = lerp_bounds(source.bounds, destination, p);

        let shell = div()
            .id("player-window")
            .absolute()
            .left(px(bounds.origin.x.as_f32()))
            .top(px(bounds.origin.y.as_f32()))
            .w(px(bounds.size.width.as_f32().max(1.0)))
            .h(px(bounds.size.height.as_f32().max(1.0)))
            .rounded(px(CARD_RADIUS))
            .overflow_hidden()
            .bg(rgb(0x0B0F14))
            .border_1()
            .border_color(theme.surfaces.border_popover)
            .shadow(theme.shadows.shadow_glass_modal.clone())
            .occlude()
            .key_context("Player")
            .track_focus(&focus_handle)
            .on_key_down(move |event, window, cx| on_key(event, window, cx));

        if progress >= 1.0 {
            return shell.child(content.expect("入场结束时必须挂上播放器内容"));
        }

        // 后段交叉淡化：海报淡出、交互界面淡入；前半段仍只挂海报，
        // 避免控制栏/侧栏被塞进 192×108 的盒子逐帧重排。
        let fade = ((p - ENTRANCE_CONTENT_FADE_START) / (1.0 - ENTRANCE_CONTENT_FADE_START))
            .clamp(0.0, 1.0);
        let fade = fade * fade * (3.0 - 2.0 * fade);

        let mut shell = shell.child(
            div()
                .absolute()
                .inset_0()
                .opacity(1.0 - fade)
                .child(cover_poster(&theme, &source)),
        );

        if fade > 0.0 {
            let content = content.expect("开始淡入后必须有播放器内容");
            shell = shell.child(
                div()
                    .absolute()
                    .left_0()
                    .top_0()
                    // 交互界面直接按最终尺寸排版，外形交给 shell 的
                    // overflow_hidden 裁剪：盒子变大时控件不跟着重排。
                    .w(px(destination.size.width.as_f32()))
                    .h(px(destination.size.height.as_f32()))
                    .opacity(fade)
                    .child(content),
            );
        }

        shell
    }
}

/// 关闭缩回：从当前播放器目标框缩回点击时的封面几何。
///
/// 过渡期只保留当前视频帧（没有帧时用封面海报），不挂控制栏/信息面板，
/// 避免窗口缩小时控件被重排得乱跳。
#[derive(gpui::IntoElement)]
pub(crate) struct PlayerExit {
    pub(crate) theme: YkTheme,
    pub(crate) source: CoverSource,
    pub(crate) visual: Option<AnyElement>,
    pub(crate) progress: f32,
}

impl RenderOnce for PlayerExit {
    fn render(self, _window: &mut Window, _cx: &mut App) -> impl IntoElement {
        let PlayerExit {
            theme,
            source,
            visual,
            progress,
        } = self;
        let p = entrance_progress(progress);
        let q = 1.0 - p;
        let root_w = source.root_size.width.as_f32().max(2.0 * SPACE_4 + 1.0);
        let root_h = source
            .root_size
            .height
            .as_f32()
            .max(TOP_BAR_HEIGHT + SPACE_4 + PLAYER_BOTTOM_INSET + 1.0);
        let top = TOP_BAR_HEIGHT + SPACE_4;
        let destination = Bounds {
            origin: point(px(SPACE_4), px(top)),
            size: size(
                px(root_w - 2.0 * SPACE_4),
                px((root_h - top - PLAYER_BOTTOM_INSET).max(1.0)),
            ),
        };
        let bounds = lerp_bounds(source.bounds, destination, q);
        let shell = div()
            .id("player-window")
            .absolute()
            .left(px(bounds.origin.x.as_f32()))
            .top(px(bounds.origin.y.as_f32()))
            .w(px(bounds.size.width.as_f32().max(1.0)))
            .h(px(bounds.size.height.as_f32().max(1.0)))
            .rounded(px(CARD_RADIUS))
            .overflow_hidden()
            .bg(rgb(0x0B0F14))
            .border_1()
            .border_color(theme.surfaces.border_popover)
            .shadow(theme.shadows.shadow_glass_modal.clone())
            .occlude();
        shell.children(visual)
    }
}

/// 当前登录会话；未登录/旧文件损坏返回 None。
pub(crate) fn current_session() -> Option<yk_domain::models::UserSession> {
    let path = yk_infra::session::session_path();
    yk_infra::session::load(&path).filter(|s| s.is_valid())
}

/// 登录会话的 Cookie 头；未登录/旧文件损坏返回空串（仍可取 480P）。
pub(crate) fn current_session_cookie() -> String {
    current_session()
        .map(|s| s.cookie_header())
        .unwrap_or_default()
}

/// 系统级鼠标位置（全局显示坐标，原点在主屏左上角）。
///
/// `Window::mouse_position()` 只是「最后一次鼠标事件」的坐标；指针快速移出
/// 窗口时可能收不到新事件，读到的还是旧位置。CoreGraphics 这里直接问系统
/// 当前光标在哪，即使窗口不是 key/指针已在窗口外也能拿到。
#[cfg(target_os = "macos")]
#[repr(C)]
#[derive(Clone, Copy)]
pub(crate) struct CGPoint {
    x: f64,
    y: f64,
}

#[cfg(target_os = "macos")]
#[repr(C)]
pub(crate) struct CGSize {
    width: f64,
    height: f64,
}

#[cfg(target_os = "macos")]
#[repr(C)]
pub(crate) struct CGRect {
    origin: CGPoint,
    size: CGSize,
}

#[cfg(target_os = "macos")]
pub(crate) fn system_mouse_position() -> Option<Point<Pixels>> {
    #[link(name = "CoreGraphics", kind = "framework")]
    extern "C" {
        fn CGEventCreate(source: *const std::ffi::c_void) -> *mut std::ffi::c_void;
        fn CGEventGetLocation(event: *mut std::ffi::c_void) -> CGPoint;
    }
    #[link(name = "CoreFoundation", kind = "framework")]
    extern "C" {
        fn CFRelease(value: *const std::ffi::c_void);
    }
    unsafe {
        let event = CGEventCreate(std::ptr::null());
        if event.is_null() {
            return None;
        }
        let location = CGEventGetLocation(event);
        CFRelease(event as *const std::ffi::c_void);
        Some(point(px(location.x as f32), px(location.y as f32)))
    }
}

/// 窗口所在显示器在 CoreGraphics 全局坐标里的左上角。
///
/// **这是多屏修复的关键**：GPUI 的 `Window::bounds()` 在 macOS 上是
/// 「相对当前屏幕」的坐标（窗口左上角减去屏幕左上角），而
/// `CGEventGetLocation` 给的是**跨屏全局坐标**（主屏左上角为 0）。
/// 窗口在主屏时两者恰好重合；拖到副屏后直接相减会把光标算到窗口外，
/// 侧栏就永远判定不到，表现为「换屏后侧栏无法展开」。
#[cfg(target_os = "macos")]
pub(crate) fn window_display_origin(window: &Window, cx: &App) -> Option<Point<Pixels>> {
    #[link(name = "CoreGraphics", kind = "framework")]
    extern "C" {
        fn CGDisplayBounds(display: u32) -> CGRect;
    }
    let display = window.display(cx)?;
    let raw: u64 = display.id().into();
    let bounds = unsafe { CGDisplayBounds(raw as u32) };
    Some(point(
        px(bounds.origin.x as f32),
        px(bounds.origin.y as f32),
    ))
}

#[cfg(not(target_os = "macos"))]
pub(crate) fn window_display_origin(_window: &Window, _cx: &App) -> Option<Point<Pixels>> {
    None
}

/// 非 macOS 没有 CoreGraphics 的全局光标查询；退回最后一次事件位置。
#[cfg(not(target_os = "macos"))]
pub(crate) fn system_mouse_position() -> Option<Point<Pixels>> {
    None
}

/// 测试钩子：把光标位置固定下来（`None` = 用真实光标）。
///
/// headless 测试里系统光标在别的窗口上，`cursor_position` 读到的
/// 位置和测试模拟的鼠标完全对不上，判定会跑偏。
pub(crate) static TEST_CURSOR: std::sync::Mutex<Option<(f32, f32)>> = std::sync::Mutex::new(None);

/// 测试入口：固定光标位置；传 `None` 恢复真实光标。
pub fn set_test_cursor(position: Option<Point<Pixels>>) {
    let mut guard = TEST_CURSOR.lock().unwrap();
    *guard = position.map(|p| (p.x.as_f32(), p.y.as_f32()));
}

/// 光标**当前**的位置，换算成窗口局部坐标。
///
/// 侧栏的展开 / 收起必须用这个，不能用 `Window::mouse_position()`：
/// 后者只是「最后一次鼠标事件」的位置 —— 鼠标移出窗口后没有新事件，
/// 它会一直停在旧位置。于是会出现「指针其实在窗口外，但判定说还在
/// 面板里」：面板被反复弹出 / 收起（鼠标在面板左半区跑动时尤其明显）。
///
/// 三套坐标要对齐：
/// - `CGEventGetLocation`：跨屏全局坐标（主屏左上角为 0）；
/// - `window.display(cx)` + `CGDisplayBounds`：窗口所在屏幕的全局左上角；
/// - `window.bounds().origin`：macOS 上**相对当前屏幕**的左上角。
///
/// 先减屏幕原点得到「屏幕内坐标」，再减窗口原点才是窗口局部坐标。
/// 只减窗口原点只在主屏成立；窗口拖到副屏后侧栏会彻底判定不到。
/// 三套全局坐标 → 窗口局部坐标。
///
/// `global` 是 CoreGraphics 的跨屏坐标；`display_origin` 是窗口所在屏幕的
/// 左上角；`window_origin` 是 GPUI 给的「屏幕内」窗口左上角。
pub(crate) fn cursor_local(
    global: Point<Pixels>,
    display_origin: Point<Pixels>,
    window_origin: Point<Pixels>,
) -> Point<Pixels> {
    point(
        global.x - display_origin.x - window_origin.x,
        global.y - display_origin.y - window_origin.y,
    )
}

pub(crate) fn cursor_position(window: &Window, cx: &App) -> Point<Pixels> {
    if let Some((x, y)) = *TEST_CURSOR.lock().unwrap() {
        return point(px(x), px(y));
    }
    system_mouse_position()
        .map(|global| {
            let display = window_display_origin(window, cx)
                .unwrap_or_else(|| point(px(0.), px(0.)));
            cursor_local(global, display, window.bounds().origin)
        })
        .unwrap_or_else(|| window.mouse_position())
}

/// 把一次历史上报快照丢到后台线程发送。
///
/// 不挂在 `cx.spawn` 上：关闭动画结束时 `PlayerView` 会被回收，
/// 挂在实体上的任务可能一起被取消；最后一次上报必须独立于实体生命周期。
pub(crate) fn dispatch_history_report(report: &HistoryReport, progress: u64) {
    let payload = yk_infra::api::client::HistoryProgress {
        aid: report.aid,
        cid: report.cid,
        progress,
    };
    let cookie = report.cookie.clone();
    let csrf = report.csrf.clone();
    eprintln!(
        "[history-report] 上报 {}：progress={}s",
        report.bvid, payload.progress
    );
    std::thread::spawn(move || {
        let client = match yk_infra::api::client::ReqwestClient::new() {
            Ok(client) => client,
            Err(error) => {
                eprintln!("[history-report] 创建客户端失败：{error}");
                return;
            }
        };
        if let Err(error) = yk_infra::api::client::report_history(&client, &cookie, &csrf, &payload)
        {
            eprintln!("[history-report] 上报失败：{error}");
        }
    });

}

impl PlayerView {
    /// 控制条播放 / 暂停按钮的唯一入口。
    /// 播放 / 暂停：控制栏与画面中央按钮共用。
    pub(crate) fn toggle_play(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        // 点播放 = 放弃输入焦点，恢复渲染循环。
        self.input_focused = false;
        self.model.state.toggle_play();
        #[cfg(any(target_os = "macos", target_os = "windows"))]
        if let Some(worker) = self.worker.as_ref() {
            worker.set_paused(!self.model.state.playing);
        }
        if self.model.state.playing {
            // 起播即藏：黑框不挡画面；鼠标一动会重新浮出。
            window.focus(&self.focus_handle, cx);
            self.hide_controls(cx);
        } else {
            // 暂停态把控制栏叫回来，方便继续操作。
            self.note_activity(window, cx);
        }
        window.refresh();
        cx.notify();
    }

    /// 跳到指定秒数。拖动松手、键盘快进、续播都走这里，避免多套 seek 逻辑。
    pub(crate) fn seek(&mut self, position_secs: f64, cx: &mut Context<Self>) {
        // 夹到 [0, duration]：键盘连按右方向键很容易冲过片尾，
        // 越过之后 mpv 会直接 EOF，看起来像「一按就关播」。
        let duration = self.model.state.duration_secs;
        let max = if duration > 0.0 {
            duration
        } else {
            f64::MAX
        };
        let target = position_secs.clamp(0.0, max);
        self.model.state.position_secs = target;
        #[cfg(any(target_os = "macos", target_os = "windows"))]
        if let Some(worker) = self.worker.as_ref() {
            worker.seek(target);
        }
        self.seek_settle_until =
            Some(std::time::Instant::now() + std::time::Duration::from_millis(400));
        cx.notify();
    }

    /// 相对当前进度快进 / 快退（方向键左右）。负数为后退。
    ///
    /// 键盘操作时把控制栏叫出来：否则用户只听到声音变化 / 看不到进度，
    /// 不知道当前跳到哪了。
    pub(crate) fn seek_relative(
        &mut self,
        delta_secs: f64,
        window: &Window,
        cx: &mut Context<Self>,
    ) {
        self.note_activity(window, cx);
        self.seek(self.model.state.position_secs + delta_secs, cx);
    }

    /// 设置音量（0..=1），并同步到解码端与弹出条滑杆（键盘上下键走这里）。
    pub fn set_volume(&mut self, volume: f64, window: &mut Window, cx: &mut Context<Self>) {
        let volume = volume.clamp(0.0, 1.0);
        self.set_volume_from_panel(volume, true, cx);
        // 调音量意味着用户在看控制条：把黑框叫回来，别让滑块在脸上消失。
        self.note_activity(window, cx);
        // 键盘调音量时回写滑杆；滑杆拖动不会走这里，避免反向抖动。
        let muted = self.model.state.muted;
        let panel = self.volume_panel.clone();
        panel.update(cx, |panel, cx| {
            panel.set_volume(volume, muted, window, cx);
        });
    }

    /// 音量落库（模型 + mpv）。
    ///
    /// `notify_player` 只在滑杆松开 / 键盘操作时为 true：拖动过程中整窗
    /// 不重绘，由[`VolumePanelView`]自己按帧重绘，避免播放器主体拖泥带水。
    pub(crate) fn set_volume_from_panel(
        &mut self,
        volume: f64,
        notify_player: bool,
        cx: &mut Context<Self>,
    ) {
        self.model.set_volume(volume);
        self.sync_audio_to_worker();
        if notify_player {
            cx.notify();
        }
    }

    /// 音量图标按钮：切换音量弹出条。
    pub(crate) fn toggle_volume_popover(&mut self, cx: &mut Context<Self>) {
        self.volume_popover_open = !self.volume_popover_open;
        if !self.volume_popover_open {
            self.volume_popover_bounds.set(None);
        }
        cx.notify();
    }

    /// 点弹出条外部时收起。
    pub(crate) fn close_volume_popover(&mut self, cx: &mut Context<Self>) {
        if self.volume_popover_open {
            self.volume_popover_open = false;
            self.volume_popover_bounds.set(None);
            cx.notify();
        }
    }

    /// 静音 / 取消静音（音量弹出条里的喇叭按钮）。
    ///
    /// 返回新的静音状态，供弹出条自己的图标 / 文案同步；这里不碰弹出条
    /// 实体，避免「面板更新中再更新面板」的重入。
    pub(crate) fn toggle_mute_from_panel(&mut self, cx: &mut Context<Self>) -> bool {
        self.model.state.toggle_mute();
        self.sync_audio_to_worker();
        cx.notify();
        self.model.state.muted
    }

    // ================= 画质 / 倍速菜单 =================

    /// 画质胶囊：展开 / 收起画质菜单（两个菜单互斥）。
    pub(crate) fn toggle_quality_popover(&mut self, cx: &mut Context<Self>) {
        if self.source.is_none() {
            return;
        }
        let open = !self.quality_popover_open;
        self.quality_popover_open = open && !self.quality_switching;
        self.speed_popover_open = false;
        if !self.quality_popover_open {
            self.control_popover_bounds.set(None);
        }
        cx.notify();
    }

    /// 倍速胶囊：展开 / 收起倍速菜单（两个菜单互斥）。
    pub(crate) fn toggle_speed_popover(&mut self, cx: &mut Context<Self>) {
        let open = !self.speed_popover_open;
        self.speed_popover_open = open;
        self.quality_popover_open = false;
        if !self.speed_popover_open {
            self.control_popover_bounds.set(None);
        }
        cx.notify();
    }

    /// 收起画质 / 倍速菜单（点外部、Esc、控制栏隐藏时用）。
    pub(crate) fn close_control_popovers(&mut self, cx: &mut Context<Self>) {
        if !self.quality_popover_open && !self.speed_popover_open {
            return;
        }
        self.quality_popover_open = false;
        self.speed_popover_open = false;
        self.control_popover_bounds.set(None);
        cx.notify();
    }

    /// 设置播放倍速（0.1x–10x），同步到 mpv 并重绘胶囊文案。
    pub(crate) fn set_rate(
        &mut self,
        rate: f64,
        window: &Window,
        cx: &mut Context<Self>,
    ) {
        self.model.state.set_rate(rate);
        self.sync_rate_to_worker();
        self.note_activity(window, cx);
        cx.notify();
    }

    /// 倍速微调 ±0.1（菜单里的 − / ＋ 按钮）。
    pub(crate) fn adjust_rate(
        &mut self,
        delta: f64,
        window: &Window,
        cx: &mut Context<Self>,
    ) {
        self.set_rate(self.model.state.rate + delta, window, cx);
    }

    /// 点选某档清晰度：按 qn 重新取流，原位换片并保留播放进度。
    pub(crate) fn select_quality(&mut self, qn: i32, cx: &mut Context<Self>) {
        if self.quality_switching {
            return;
        }
        let Some(source) = self.source.clone() else {
            return;
        };
        // 点的就是当前档：只收菜单，不发请求。
        if source.quality_id == qn {
            self.close_control_popovers(cx);
            return;
        }
        let Some(label) = source
            .qualities
            .iter()
            .find(|quality| quality.id == qn)
            .map(|quality| quality.label.clone())
        else {
            return;
        };
        let position = self.play_time.max(self.model.state.position_secs).max(0.0);
        let load_generation = self.load_generation;
        self.quality_generation = self.quality_generation.wrapping_add(1);
        let generation = self.quality_generation;
        self.quality_switching = true;
        self.quality_popover_open = false;
        self.control_popover_bounds.set(None);
        eprintln!("[quality] 切换到 {label}（qn={qn}），保留进度 {position:.1}s");
        cx.notify();

        let bvid = source.bvid.clone();
        let cid = source.cid;
        cx.spawn(async move |this, cx| {
            let result = cx
                .background_executor()
                .spawn(async move {
                    let cookie = current_session_cookie();
                    let client = yk_infra::api::client::ReqwestClient::new()
                        .map_err(|error| error.to_string())?;
                    yk_infra::api::client::fetch_stream_at(&client, &bvid, cid, qn, &cookie)
                })
                .await;
            let _ = this.update(cx, |view, cx| {
                // 换过视频 / 又点了别的档：旧结果直接丢。
                if view.load_generation != load_generation
                    || view.quality_generation != generation
                {
                    return;
                }
                view.quality_switching = false;
                match result {
                    Ok(stream) => {
                        // 先停旧 worker，下一次 render 用新源在原位置重建。
                        #[cfg(any(target_os = "macos", target_os = "windows"))]
                        {
                            view.worker = None;
                            view.current = None;
                        }
                        #[cfg(target_os = "windows")]
                        {
                            view.needs_image_clear = true;
                        }
                        view.tried_worker = false;
                        view.start_position = position;
                        view.play_time = position;
                        view.model.state.position_secs = position;
                        view.seek_settle_until = None;
                        let mut switched = label;
                        if let Some(source) = view.source.as_mut() {
                            source.video_url = stream.video_url;
                            source.audio_url = stream.audio_url;
                            source.video_width = stream.width;
                            source.video_height = stream.height;
                            source.quality_id = stream.quality_id;
                            if !stream.quality_label.is_empty() {
                                source.quality = stream.quality_label.clone();
                            }
                            switched = source.quality.clone();
                        }
                        view.show_interaction_note(
                            format!("已切换到 {switched}"),
                            false,
                            cx,
                        );
                    }
                    Err(error) => {
                        eprintln!("[quality] 切换失败：{error}");
                        view.show_interaction_note(
                            format!("切换清晰度失败：{error}"),
                            true,
                            cx,
                        );
                    }
                }
                cx.notify();
            });
        })
        .detach();
    }

    /// 把视图模型里的倍速下发给解码线程。没有 worker 时是空操作。
    fn sync_rate_to_worker(&self) {
        #[cfg(any(target_os = "macos", target_os = "windows"))]
        if let Some(worker) = self.worker.as_ref() {
            worker.set_rate(self.model.state.rate);
        }
    }

    /// 把视图模型里的音量 / 静音状态下发给解码线程。没有 worker 时是空操作。
    fn sync_audio_to_worker(&self) {
        #[cfg(any(target_os = "macos", target_os = "windows"))]
        if let Some(worker) = self.worker.as_ref() {
            worker.set_volume(self.model.state.volume);
            worker.set_muted(self.model.state.muted);
        }
    }
}

// ================= 弹幕发送 =================

impl PlayerView {
    /// 把输入框里的文本发到 B 站（`/x/v2/dm/post`），**成功之后才上屏**。
    ///
    /// 以前这里是「本地回显，不接服务端」：字打出去、屏幕上飞过去，服务端一条都没有，
    /// 重新进视频就消失。现在走真写接口：
    /// - 未登录 / 没有 `bili_jct` → 直接提示，不发请求，也不假装成功；
    /// - 失败（风控、等级不足、频率过快…）→ 文本留在输入框里，弹一句服务端的原话；
    /// - 成功 → 才插进上屏列表与「弹幕」Tab，并清空输入框。
    ///
    /// 时间取当前播放位置（毫秒），末尾夹到时长内 —— 超出时长服务端会回
    /// `36714 弹幕发送时间不合法`。
    pub(crate) fn send_danmaku(&mut self, _window: &mut Window, cx: &mut Context<Self>) {
        if self.danmaku_sending {
            return;
        }
        let Some(st) = self.danmaku_input.clone() else {
            return;
        };
        let text = st.read(cx).value().trim().to_string();
        if text.is_empty() {
            return;
        }
        let Some(session) = current_session() else {
            self.show_interaction_note("请先登录后再发弹幕".into(), true, cx);
            return;
        };
        if session.bili_jct.trim().is_empty() {
            self.show_interaction_note("登录会话已失效，请重新登录后再发弹幕".into(), true, cx);
            return;
        }
        let Some(source) = self.source.clone() else {
            self.show_interaction_note("视频还没加载完，稍后再发".into(), true, cx);
            return;
        };
        if source.cid <= 0 {
            self.show_interaction_note("当前视频没有弹幕区，无法发送".into(), true, cx);
            return;
        }
        let chars = text.chars().count();
        let max_chars = yk_infra::interaction::MAX_DANMAKU_CHARS;
        if chars > max_chars {
            self.show_interaction_note(
                format!("弹幕最长 {max_chars} 字，当前 {chars} 字"),
                true,
                cx,
            );
            return;
        }

        // 播放位置 → 毫秒；时长已知时夹到「时长 - 100ms」，避免末尾几帧被判时间不合法。
        let raw_ms = (self.play_time.max(0.0) * 1000.0).round() as u64;
        let progress_ms = if source.duration_secs > 0 {
            raw_ms.min(source.duration_secs.saturating_mul(1000).saturating_sub(100))
        } else {
            raw_ms
        };

        let (bvid, cid) = (source.bvid.clone(), source.cid);
        let cookie = session.cookie_header();
        let csrf = session.bili_jct.clone();
        self.danmaku_sending = true;
        cx.notify();
        cx.spawn(async move |this, cx| {
            let text_for_send = text.clone();
            let result = cx
                .background_executor()
                .spawn(async move {
                    let client = yk_infra::api::client::ReqwestClient::new()
                        .map_err(|error| error.to_string())?;
                    yk_infra::interaction::send_danmaku(
                        &client,
                        &cookie,
                        &csrf,
                        &bvid,
                        cid,
                        &text_for_send,
                        progress_ms,
                    )
                })
                .await;
            let _ = this.update_in(cx, move |view, window, cx| {
                view.danmaku_sending = false;
                match result {
                    Ok(()) => {
                        // 上屏时间用「此刻」而不是发送时：一次请求的往返里视频又走了几百毫秒，
                        // 用旧时间会让刚发的弹幕直接从屏幕中间冒出来。
                        let shown_at = view.play_time + 0.05;
                        let width = playback_layer_width(window, view.panel_width());
                        view.insert_sent_danmaku(shown_at, &text, width);
                        if let Some(st) = view.danmaku_input.clone() {
                            st.update(cx, |st, cx| st.set_value("", window, cx));
                        }
                        view.show_interaction_note("弹幕已发送".into(), false, cx);
                    }
                    Err(error) => {
                        // 失败不清输入框：字还在，用户改一改就能重发；
                        // 清掉反而会让人以为发出去了（这正是旧实现的毛病）。
                        eprintln!("[danmaku] 发送失败：{error}");
                        view.show_interaction_note(error, true, cx);
                    }
                }
                cx.notify();
            });
        })
        .detach();
    }

    /// 把刚发送成功的弹幕插进上屏列表与「弹幕」Tab 列表（都保持时间序）。
    pub(crate) fn insert_sent_danmaku(&mut self, time: f64, text: &str, area_width: f32) {
        let d = Danmaku {
            time,
            mode: DanmakuMode::Scroll,
            font_size: 25.0,
            color: (0xFF, 0xFF, 0xFF),
            width: Danmaku::estimate_width(text, 25.0),
            text: text.to_string(),
        };
        // 预分配器状态停在「整段视频排完」，这里按当前时刻找空轨道；
        // 再按时间插回两个列表，保证时间序（面板列表也按时间展示）。
        let lane = self.pick_free_scroll_lane(time, area_width);
        let placed = PlacedDanmaku {
            data: d.clone(),
            lane: DanmakuLane::Scroll(lane),
        };
        let at = self
            .danmaku
            .partition_point(|item| item.data.time <= d.time);
        self.danmaku.insert(at, placed);
        let at = self.danmaku_all.partition_point(|item| item.time <= d.time);
        self.danmaku_all.insert(at, d);
        eprintln!(
            "[danmaku] 已发送，轨道 {lane}，共 {} 条",
            self.danmaku_all.len()
        );
    }
}

