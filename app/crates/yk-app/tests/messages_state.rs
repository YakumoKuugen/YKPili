//! 消息中心页内状态的纯逻辑验收（不渲染）。
//!
//! 放在 integration test 里而不是 `src/` 内的 `#[test]`：
//! `yk-app` 的展开深度会触发 `#[test]` 宏的 recursion limit（与 `status.rs` 同一原因）。

use std::collections::HashSet;

use yk_app::messages_view::{MessageRow, MessageState};
use yk_core::messages::{Conversation, MessageChannel, MessageEntry, MessageTarget, MessageUser};

fn entry(id: i64, time: u64, last_view_at: u64, unread: bool) -> MessageEntry {
    MessageEntry {
        id,
        channel: MessageChannel::Reply,
        user: MessageUser {
            mid: id,
            nickname: format!("用户{id}"),
            avatar: String::new(),
        },
        action: "回复了你的评论".into(),
        summary: "摘要".into(),
        target_title: "视频".into(),
        uri: "https://www.bilibili.com/video/BV1xx411c7mD".into(),
        business: "comment".into(),
        business_id: 1,
        subject_id: 0,
        time,
        last_view_at,
        read_override: !unread,
        counts_reply: 1,
        is_up: false,
        likers: Vec::new(),
        like_count: 0,
        target: MessageTarget::Video {
            bvid: "BV1xx411c7mD".into(),
        },
    }
}

#[test]
fn unread_divider_sits_above_first_unread() {
    let mut state = MessageState::new();
    state.reply.entries = vec![
        entry(1, 10, 20, false),
        entry(2, 30, 20, true),
        entry(3, 40, 20, true),
    ];
    assert_eq!(
        state.inbox_rows(),
        vec![
            MessageRow::Entry(0),
            MessageRow::Divider,
            MessageRow::Entry(1),
            MessageRow::Entry(2),
            MessageRow::Footer,
        ]
    );
    assert_eq!(state.first_unread_row(), Some(2));
}

#[test]
fn only_unread_filters_and_drops_divider() {
    let mut state = MessageState::new();
    state.reply.entries = vec![entry(1, 10, 20, false), entry(2, 30, 20, true)];
    state.only_unread = true;
    assert_eq!(state.visible_indices(), vec![1]);
    assert_eq!(
        state.inbox_rows(),
        vec![MessageRow::Entry(1), MessageRow::Footer]
    );
}

#[test]
fn like_expansion_adds_one_row_per_liker() {
    let mut state = MessageState::new();
    let mut like = entry(1, 10, 0, true);
    like.channel = MessageChannel::Like;
    like.action = "赞了你的评论".into();
    like.likers = (0..3)
        .map(|index| MessageUser {
            mid: index,
            nickname: format!("赞{index}"),
            avatar: String::new(),
        })
        .collect();
    state.channel = MessageChannel::Like;
    state.like.entries = vec![like];
    state.expanded_like = Some(1);
    assert_eq!(
        state.inbox_rows(),
        vec![
            MessageRow::Divider,
            MessageRow::Entry(0),
            MessageRow::LikeDetail { entry: 0, liker: 0 },
            MessageRow::LikeDetail { entry: 0, liker: 1 },
            MessageRow::LikeDetail { entry: 0, liker: 2 },
            MessageRow::Footer,
        ]
    );
}

#[test]
fn switching_to_dm_clears_only_unread() {
    let mut state = MessageState::new();
    state.only_unread = true;
    state.switch_channel(MessageChannel::Dm);
    assert!(!state.only_unread);
    assert_eq!(state.channel, MessageChannel::Dm);
}

#[test]
fn whitelist_marks_relation_and_sums_only_mutual_unread() {
    let mut state = MessageState::new();
    state.dm.conversations = vec![
        Conversation {
            talker_id: 1,
            nickname: "A".into(),
            unread: 2,
            mutual: None,
            ..Default::default()
        },
        Conversation {
            talker_id: 2,
            nickname: "B".into(),
            unread: 3,
            mutual: None,
            ..Default::default()
        },
    ];
    state.whitelist = HashSet::from([1]);
    state.whitelist_loaded = true;
    state.apply_whitelist();
    assert_eq!(state.dm.conversations[0].mutual, Some(true));
    assert_eq!(state.dm.conversations[1].mutual, Some(false));
    assert_eq!(state.unread.dm, 2);
    assert_eq!(state.dm_unread_total(), 2);
}

#[test]
fn unread_total_drives_first_channel() {
    let mut state = MessageState::new();
    state.unread = yk_core::messages::UnreadCounts {
        reply: 0,
        mention: 0,
        like: 2,
        dm: 1,
    };
    assert_eq!(state.unread.first_unread_channel(), MessageChannel::Like);
    state.unread = Default::default();
    assert_eq!(state.unread.first_unread_channel(), MessageChannel::Reply);
}

#[test]
fn mention_unread_falls_back_to_newest_n() {
    let mut state = MessageState::new();
    state.channel = MessageChannel::Mention;
    state.mention.entries = vec![
        entry(1, 300, 0, true),
        entry(2, 200, 0, true),
        entry(3, 100, 0, true),
    ];
    state.unread = yk_core::messages::UnreadCounts {
        mention: 1,
        ..Default::default()
    };
    state.unread_loaded = true;
    state.reconcile_mention_unread();
    assert!(state.mention.entries[0].is_unread());
    assert!(!state.mention.entries[1].is_unread());
    assert!(!state.mention.entries[2].is_unread());

    // 本地点过的条目优先，不会被「最新 N 条」重新点亮。
    state.read_ids.insert(1);
    state.reconcile_mention_unread();
    assert!(!state.mention.entries[0].is_unread());
}

#[test]
fn mutual_filter_hides_non_mutual_and_resets_selection() {
    let mut state = MessageState::new();
    state.channel = MessageChannel::Dm;
    state.dm.conversations = vec![
        Conversation {
            talker_id: 1,
            nickname: "互关".into(),
            mutual: Some(true),
            unread: 1,
            ..Default::default()
        },
        Conversation {
            talker_id: 2,
            nickname: "已解除互关".into(),
            mutual: Some(false),
            unread: 0,
            ..Default::default()
        },
        Conversation {
            talker_id: 3,
            nickname: "关系未确认".into(),
            mutual: None,
            unread: 0,
            ..Default::default()
        },
    ];
    state.selected = Some(2);
    state.toggle_only_mutual();
    assert!(state.only_mutual);
    assert_eq!(state.visible_conversation_indices(), vec![0]);
    assert_eq!(
        state.selected,
        Some(1),
        "被筛掉的选中行要落回第一条可见会话"
    );
    assert_eq!(
        state.dm_rows(),
        vec![MessageRow::Conversation(0), MessageRow::Footer]
    );

    state.toggle_only_mutual();
    assert!(!state.only_mutual);
    assert_eq!(state.visible_conversation_indices(), vec![0, 1, 2]);
    assert_eq!(state.dm_rows().len(), 4);
}
