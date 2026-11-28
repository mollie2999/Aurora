//! Steam group chats (`ChatRoom` unified service): loading groups, channels,
//! members, history, invites, reactions, and moderation.

use crate::{Event, Sink};
use model::{
    AppId, ChatGroup, ChatGroupMember, ChatRole, ChatRoleActions, ChatRoomChannel, ChatRoomId, GroupBanInfo, GroupChatId, GroupInviteInfo,
    GroupInviteLink, GroupInviteRef, GroupMessage, GroupServerMessage, MessageReaction, Persona, PersonaState, SteamId, account_to_steam_id,
    steam_id_to_account,
};
use net::{Connection, emsg};
use proto::*;
use std::collections::{BTreeSet, HashSet};
use std::sync::{Arc, Mutex};

pub type ActiveGroups = Arc<Mutex<BTreeSet<GroupChatId>>>;

fn active_snapshot(active: &ActiveGroups, extra: Option<GroupChatId>) -> Vec<GroupChatId> {
    let mut guard = active.lock().unwrap();
    if let Some(id) = extra.filter(|id| *id != 0) {
        guard.insert(id);
    }
    guard.iter().copied().collect()
}

fn replace_active(active: &ActiveGroups, ids: &[GroupChatId]) {
    let mut guard = active.lock().unwrap();
    guard.clear();
    guard.extend(ids.iter().copied().filter(|id| *id != 0));
}

fn remove_active(active: &ActiveGroups, id: GroupChatId) {
    active.lock().unwrap().remove(&id);
}

const PERSONA_FLAGS: u32 = 1 | 2 | 4 | 8 | 16 | 64 | 128 | 256 | 512 | 4096;

fn is_trusted_steam_avatar_url(raw: &str) -> Option<String> {
    let trimmed = raw.trim();
    if trimmed.is_empty() || trimmed.contains('\\') || trimmed.chars().any(|c| c.is_control()) {
        return None;
    }
    let parsed = reqwest::Url::parse(trimmed).ok()?;
    if parsed.scheme() != "https" || !parsed.username().is_empty() || parsed.password().is_some() {
        return None;
    }
    let host = parsed.host_str()?.trim_end_matches('.').to_ascii_lowercase();
    let trusted = matches!(
        host.as_str(),
        "steamusercontent-a.akamaihd.net"
            | "steamuserimages-a.akamaihd.net"
            | "steamcdn-a.akamaihd.net"
            | "media.steampowered.com"
            | "avatars.akamai.steamstatic.com"
            | "avatars.fastly.steamstatic.com"
            | "avatars.steamstatic.com"
    ) || host.ends_with(".steamusercontent.com")
        || host.ends_with(".steamstatic.com");
    trusted.then(|| parsed.into())
}

/// Avatar URL for a group chat: validated Steam UGC upload, chaticon SHA, or associated app header.
pub fn avatar_url(avatar_sha: &[u8], ugc_url: &str, app_id: AppId) -> Option<String> {
    if let Some(url) = is_trusted_steam_avatar_url(ugc_url) {
        return Some(url);
    }
    if let Some(url) = model::art::group_chat_avatar(avatar_sha) {
        return Some(url);
    }
    (app_id != 0).then(|| model::art::header(app_id))
}

pub fn role_actions_from_proto(a: &CChatRoleActions) -> ChatRoleActions {
    ChatRoleActions {
        role_id: a.role_id(),
        can_create_rename_delete_channel: a.can_create_rename_delete_channel(),
        can_kick: a.can_kick(),
        can_ban: a.can_ban(),
        can_invite: a.can_invite(),
        can_change_tagline_avatar_name: a.can_change_tagline_avatar_name(),
        can_chat: a.can_chat(),
        can_view_history: a.can_view_history(),
        can_change_group_roles: a.can_change_group_roles(),
        can_change_user_roles: a.can_change_user_roles(),
        can_mention_all: a.can_mention_all(),
        can_set_watching_broadcast: a.can_set_watching_broadcast(),
    }
}

pub fn channel_from_proto(r: &CChatRoomState, user: Option<&CUserChatRoomState>) -> ChatRoomChannel {
    ChatRoomChannel {
        id: r.chat_id(),
        name: if r.chat_name().trim().is_empty() { "General".into() } else { r.chat_name().to_string() },
        voice_allowed: r.voice_allowed(),
        members_in_voice: r.members_in_voice.clone(),
        sort_order: r.sort_order(),
        time_last_message: r.time_last_message(),
        last_message: r.last_message().to_string(),
        accountid_last_message: r.accountid_last_message(),
        time_last_ack: user.map(|u| u.time_last_ack()).unwrap_or(0),
        time_first_unread: user.map(|u| u.time_first_unread()).unwrap_or(0),
        time_last_mention: user.map(|u| u.time_last_mention()).unwrap_or(0),
        unread_muted: user.is_some_and(|u| u.unread_indicator_muted()),
        unread: 0,
    }
}

pub fn member_from_proto(m: &CChatRoomMember) -> ChatGroupMember {
    ChatGroupMember {
        steam_id: account_to_steam_id(m.accountid()),
        account_id: m.accountid(),
        rank: m.rank().into(),
        state: m.state().into(),
        time_kick_expire: m.time_kick_expire(),
        role_ids: m.role_ids.clone(),
    }
}

pub fn group_from_summary(summary: &CChatRoomGetChatRoomGroupSummaryResponse, user: Option<&CUserChatRoomGroupState>) -> ChatGroup {
    let mut rooms: Vec<ChatRoomChannel> = summary
        .chat_rooms
        .iter()
        .map(|r| {
            let u = user.and_then(|us| us.user_chat_room_state.iter().find(|x| x.chat_id() == r.chat_id()));
            channel_from_proto(r, u)
        })
        .collect();
    rooms.sort_by_key(|r| (r.sort_order, r.id));

    ChatGroup {
        id: summary.chat_group_id(),
        name: summary.chat_group_name().to_string(),
        tagline: summary.chat_group_tagline().to_string(),
        avatar_url: avatar_url(summary.chat_group_avatar_sha(), summary.avatar_ugc_url(), summary.appid()),
        owner_id: (summary.accountid_owner() != 0).then(|| account_to_steam_id(summary.accountid_owner())).unwrap_or(0),
        clan_id: summary.clanid(),
        app_id: summary.appid(),
        default_chat_id: summary.default_chat_id(),
        rooms,
        members: Vec::new(),
        top_members: summary.top_members.iter().copied().map(account_to_steam_id).collect(),
        active_member_count: summary.active_member_count(),
        active_voice_member_count: summary.active_voice_member_count(),
        my_rank: summary.rank().into(),
        my_role_ids: summary.role_ids.clone(),
        roles: Vec::new(),
        role_actions: summary.role_actions.iter().map(role_actions_from_proto).collect(),
        default_role_id: summary.default_role_id(),
        time_joined: user.map(|u| u.time_joined()).unwrap_or(0),
        time_last_group_ack: user.map(|u| u.time_last_group_ack()).unwrap_or(0),
        unread_muted: user.is_some_and(|u| u.unread_indicator_muted()),
        desktop_notification_level: user.map(|u| i32::from(u.desktop_notification_level())).unwrap_or(0),
        full_loaded: false,
        virtualized_members: false,
    }
}

pub fn apply_header(group: &mut ChatGroup, h: &CChatRoomGroupHeaderState) {
    if h.chat_group_id() != 0 {
        group.id = h.chat_group_id();
    }
    if !h.chat_name().trim().is_empty() {
        group.name = h.chat_name().to_string();
    }
    if !h.tagline().trim().is_empty() {
        group.tagline = h.tagline().to_string();
    }
    if h.clanid() != 0 {
        group.clan_id = h.clanid();
    }
    if h.appid() != 0 {
        group.app_id = h.appid();
    }
    if h.accountid_owner() != 0 {
        group.owner_id = account_to_steam_id(h.accountid_owner());
    }
    if let Some(url) = avatar_url(h.avatar_sha(), h.avatar_ugc_url(), group.app_id) {
        group.avatar_url = Some(url);
    }
    if h.default_role_id() != 0 {
        group.default_role_id = h.default_role_id();
    }
    if !h.roles.is_empty() {
        group.roles = h
            .roles
            .iter()
            .map(|r| ChatRole { role_id: r.role_id(), name: r.name().to_string(), ordinal: r.ordinal() })
            .collect();
        group.roles.sort_by_key(|r| r.ordinal);
    }
    if !h.role_actions.is_empty() {
        group.role_actions = h.role_actions.iter().map(role_actions_from_proto).collect();
    }
}

pub fn apply_rooms(group: &mut ChatGroup, default_chat_id: ChatRoomId, rooms: &[CChatRoomState]) {
    let old = std::mem::take(&mut group.rooms);
    let mut next: Vec<ChatRoomChannel> = rooms
        .iter()
        .map(|r| {
            let mut ch = channel_from_proto(r, None);
            if let Some(prev) = old.iter().find(|p| p.id == ch.id) {
                if r.chat_name().trim().is_empty() && !prev.name.trim().is_empty() {
                    ch.name = prev.name.clone();
                }
                ch.time_last_ack = prev.time_last_ack;
                ch.time_first_unread = prev.time_first_unread;
                ch.time_last_mention = prev.time_last_mention;
                ch.unread_muted = prev.unread_muted;
                ch.unread = prev.unread;
            }
            ch
        })
        .collect();
    next.sort_by_key(|r| (r.sort_order, r.id));
    group.rooms = next;
    if default_chat_id != 0 {
        group.default_chat_id = default_chat_id;
    }
    group.active_voice_member_count = group.rooms.iter().map(|r| r.members_in_voice.len() as u32).sum();
}

pub fn apply_group_state(group: &mut ChatGroup, state: &CChatRoomGroupState, user: Option<&CUserChatRoomGroupState>, me: SteamId) {
    if let Some(h) = &state.header_state {
        apply_header(group, h);
    }
    apply_rooms(group, state.default_chat_id(), &state.chat_rooms);
    if !state.members.is_empty() {
        let joined: Vec<ChatGroupMember> = state
            .members
            .iter()
            .filter(|m| matches!(m.state(), EChatRoomJoinState::KEChatRoomJoinStateJoined | EChatRoomJoinState::KEChatRoomJoinStateDefault))
            .map(member_from_proto)
            .collect();
        group.active_member_count = group.active_member_count.max(joined.len() as u32);
        if let Some(mine) = joined.iter().find(|m| m.steam_id == me) {
            group.my_rank = mine.rank;
            group.my_role_ids = mine.role_ids.clone();
        }
        group.members = joined;
    }
    if let Some(u) = user {
        group.time_joined = u.time_joined();
        group.time_last_group_ack = u.time_last_group_ack();
        group.unread_muted = u.unread_indicator_muted();
        group.desktop_notification_level = u.desktop_notification_level().into();
        for us in &u.user_chat_room_state {
            if let Some(ch) = group.channel_mut(us.chat_id()) {
                ch.time_last_ack = us.time_last_ack();
                ch.time_first_unread = us.time_first_unread();
                ch.time_last_mention = us.time_last_mention();
                ch.unread_muted = us.unread_indicator_muted();
            }
        }
    }
    group.full_loaded = true;
}

pub fn message_from_history(group_id: GroupChatId, chat_id: ChatRoomId, m: &c_chat_room_get_message_history_response::ChatMessage) -> GroupMessage {
    let parsed = model::chat::parse_message(m.message());
    let server_msg = m
        .server_message
        .as_ref()
        .filter(|s| s.message() != EChatRoomServerMessage::KEChatRoomServerMsgInvalid)
        .map(|s| GroupServerMessage {
            kind: s.message().into(),
            string_param: s.string_param().to_string(),
            account_id_param: s.accountid_param(),
        });
    GroupMessage {
        group_id,
        chat_id,
        from: account_to_steam_id(m.sender()),
        text: parsed.text,
        time: m.server_timestamp() as u64,
        ordinal: m.ordinal(),
        invite: parsed.invite,
        group_invite: parsed.group_invite,
        deleted: m.deleted(),
        server_msg,
        reactions: m
            .reactions
            .iter()
            .map(|r| MessageReaction {
                reaction_type: r.reaction_type().into(),
                reaction: r.reaction().to_string(),
                count: r.num_reactors(),
                reacted: r.has_user_reacted(),
            })
            .collect(),
        mentioned_me: false,
        media: parsed.media,
    }
}

pub fn message_from_notification(me: SteamId, n: &CChatRoomIncomingChatMessageNotification) -> GroupMessage {
    let raw = if n.message().is_empty() { n.message_no_bbcode() } else { n.message() };
    let parsed = model::chat::parse_message(raw);
    let text = if parsed.text.is_empty() && parsed.invite.is_none() && parsed.group_invite.is_none() && parsed.media.is_empty() {
        n.message_no_bbcode().to_string()
    } else {
        parsed.text
    };
    let me_acct = steam_id_to_account(me);
    let mentioned_me = n.mentions.as_ref().is_some_and(|m| {
        m.mention_all() || m.mention_here() || m.mention_accountids.contains(&me_acct)
    });
    let server_msg = n
        .server_message
        .as_ref()
        .filter(|s| s.message() != EChatRoomServerMessage::KEChatRoomServerMsgInvalid)
        .map(|s| GroupServerMessage {
            kind: s.message().into(),
            string_param: s.string_param().to_string(),
            account_id_param: s.accountid_param(),
        });
    GroupMessage {
        group_id: n.chat_group_id(),
        chat_id: n.chat_id(),
        from: n.steamid_sender(),
        text,
        time: n.timestamp() as u64,
        ordinal: n.ordinal(),
        invite: parsed.invite,
        group_invite: parsed.group_invite,
        deleted: false,
        server_msg,
        reactions: Vec::new(),
        mentioned_me,
        media: parsed.media,
    }
}

pub fn persona_from_proto(f: &c_msg_client_persona_state::Friend) -> Persona {
    let id = f.friendid();
    let hex = |b: &[u8]| b.iter().map(|x| format!("{x:02x}")).collect::<String>();
    let mut game_app_id = f.game_played_app_id.filter(|g| *g != 0);
    if game_app_id.is_none() {
        if let Some(gid) = f.gameid.filter(|g| *g != 0) {
            let app = (gid & 0xFF_FFFF) as AppId;
            game_app_id = (app != 0 && (gid >> 56) == 0).then_some(app);
        }
    }
    Persona {
        steam_id: id,
        name: f.player_name.clone().unwrap_or_default(),
        state: f.persona_state.map(PersonaState::from_u32).unwrap_or_default(),
        game_app_id,
        game_name: f.game_name.clone().filter(|s| !s.is_empty()),
        rich_status: f.rich_presence.iter().find(|kv| kv.key() == "status").map(|kv| kv.value().trim().to_string()).filter(|s| !s.is_empty() && !s.starts_with('#')),
        connect: f.rich_presence.iter().find(|kv| kv.key() == "connect").map(|kv| kv.value().trim().to_string()).filter(|c| !c.is_empty()),
        server: match (f.game_server_ip.unwrap_or(0), f.game_server_port.unwrap_or(0)) {
            (ip, port) if ip != 0 && port != 0 => Some(format!("{}:{port}", std::net::Ipv4Addr::from(ip))),
            _ => None,
        },
        lobby: f.game_lobby_id.filter(|l| *l != 0),
        broadcasting: f.broadcast_id.is_some_and(|b| b != 0),
        nickname: None,
        avatar_hash: f.avatar_hash.as_deref().map(hex),
        last_logoff: f.last_logoff.map(|t| t as u64),
    }
}

pub fn request_personas(conn: &Connection, me: SteamId, ids: impl IntoIterator<Item = SteamId>) {
    let mut list: Vec<SteamId> = ids.into_iter().filter(|id| *id != 0 && *id != me).collect();
    list.sort_unstable();
    list.dedup();
    for chunk in list.chunks(100) {
        let _ = conn.send(
            emsg::CLIENT_REQUEST_FRIEND_DATA,
            &CMsgClientRequestFriendData {
                persona_state_requested: Some(PERSONA_FLAGS),
                friends: chunk.to_vec(),
            },
        );
    }
}

pub fn load_my_groups(conn: Connection, me: SteamId, active: ActiveGroups, sink: Sink) {
    tokio::spawn(async move {
        let r: Result<CChatRoomGetMyChatRoomGroupsResponse, _> = conn
            .call("ChatRoom.GetMyChatRoomGroups#1", &CChatRoomGetMyChatRoomGroupsRequest::default())
            .await;
        match r {
            Ok(r) => {
                let mut groups = Vec::with_capacity(r.chat_room_groups.len());
                let mut need_personas = HashSet::new();
                for pair in &r.chat_room_groups {
                    let Some(summary) = &pair.group_summary else { continue };
                    let g = group_from_summary(summary, pair.user_chat_group_state.as_ref());
                    need_personas.extend(g.top_members.iter().copied());
                    for ch in &g.rooms {
                        if ch.accountid_last_message != 0 {
                            need_personas.insert(account_to_steam_id(ch.accountid_last_message));
                        }
                    }
                    groups.push(g);
                }
                let ids: Vec<u64> = groups.iter().map(|g| g.id).collect();
                replace_active(&active, &ids);
                sink(Event::ChatGroups(groups));
                request_personas(&conn, me, need_personas);
                // Subscribe to live notifications across all joined groups.
                if !ids.is_empty() {
                    activate_groups(conn, me, ids, Vec::new(), sink).await;
                }
            }
            Err(e) => tracing::warn!("load chat groups: {e}"),
        }
    });
}

pub async fn activate_groups(conn: Connection, me: SteamId, all_ids: Vec<u64>, data_ids: Vec<u64>, sink: Sink) {
    let req = CChatRoomSetSessionActiveChatRoomGroupsRequest {
        chat_group_ids: all_ids,
        chat_groups_data_requested: data_ids,
        virtualize_members_threshold: Some(100),
    };
    match conn.call::<_, CChatRoomSetSessionActiveChatRoomGroupsResponse>("ChatRoom.SetSessionActiveChatRoomGroups#1", &req).await {
        Ok(resp) => {
            let virtualized: HashSet<u64> = resp.virtualize_members_chat_group_ids.iter().copied().collect();
            let mut need_personas = HashSet::new();
            for st in &resp.chat_states {
                let gid = st.header_state.as_ref().map(|h| h.chat_group_id()).unwrap_or(0);
                if gid == 0 {
                    continue;
                }
                let mut g = ChatGroup { id: gid, virtualized_members: virtualized.contains(&gid), ..Default::default() };
                apply_group_state(&mut g, st, None, me);
                need_personas.extend(g.members.iter().map(|m| m.steam_id));
                sink(Event::ChatGroupUpdated(g));
            }
            for gid in &resp.virtualize_members_chat_group_ids {
                // Request the first 100 members of virtualized groups (ranked/online first).
                // Steam expects `client_changenumber >= 1` for an initial view update.
                let _ = conn.notify_service(
                    "ChatRoom.UpdateMemberListView#1",
                    &CChatRoomUpdateMemberListViewNotification {
                        chat_group_id: Some(*gid),
                        view_id: Some(*gid),
                        start: Some(0),
                        end: Some(100),
                        client_changenumber: Some(1),
                        ..Default::default()
                    },
                );
            }
            request_personas(&conn, me, need_personas);
        }
        Err(e) => tracing::debug!("SetSessionActiveChatRoomGroups: {e}"),
    }
}

pub fn load_group_state(conn: Connection, me: SteamId, group_id: GroupChatId, active: ActiveGroups, sink: Sink) {
    tokio::spawn(async move {
        let all_active = active_snapshot(&active, Some(group_id));
        activate_groups(conn, me, all_active, vec![group_id], sink).await;
    });
}

pub fn load_history(conn: Connection, me: SteamId, group_id: GroupChatId, chat_id: ChatRoomId, before: Option<(u32, u32)>, sink: Sink) {
    tokio::spawn(async move {
        let req = CChatRoomGetMessageHistoryRequest {
            chat_group_id: Some(group_id),
            chat_id: Some(chat_id),
            last_time: before.map(|(t, _)| t),
            last_ordinal: before.map(|(_, o)| o),
            max_count: Some(50),
            ..Default::default()
        };
        match conn.call::<_, CChatRoomGetMessageHistoryResponse>("ChatRoom.GetMessageHistory#1", &req).await {
            Ok(r) => {
                let mut messages: Vec<GroupMessage> = r.messages.iter().map(|m| message_from_history(group_id, chat_id, m)).collect();
                messages.sort_by_key(|m| (m.time, m.ordinal));
                let senders: Vec<SteamId> = messages.iter().map(|m| m.from).collect();
                sink(Event::GroupHistory {
                    group_id,
                    chat_id,
                    prepend: before.is_some(),
                    more: r.more_available(),
                    messages,
                });
                request_personas(&conn, me, senders);
            }
            Err(e) => tracing::warn!("group chat history: {e}"),
        }
    });
}

pub fn send_message(conn: Connection, me: SteamId, group_id: GroupChatId, chat_id: ChatRoomId, text: String, sink: Sink) {
    tokio::spawn(async move {
        let req = CChatRoomSendChatMessageRequest {
            chat_group_id: Some(group_id),
            chat_id: Some(chat_id),
            message: Some(text.clone()),
            echo_to_sender: Some(true),
        };
        match conn.call::<_, CChatRoomSendChatMessageResponse>("ChatRoom.SendChatMessage#1", &req).await {
            Ok(r) => {
                let raw = if r.modified_message().is_empty() { &text } else { r.modified_message() };
                let parsed = model::chat::parse_message(raw);
                sink(Event::GroupMessage(GroupMessage {
                    group_id,
                    chat_id,
                    from: me,
                    text: parsed.text,
                    time: r.server_timestamp() as u64,
                    ordinal: r.ordinal(),
                    invite: parsed.invite,
                    group_invite: parsed.group_invite,
                    deleted: false,
                    server_msg: None,
                    reactions: Vec::new(),
                    mentioned_me: false,
                    media: parsed.media,
                }));
            }
            Err(e) => sink(Event::Error(format!("Message not sent: {e}"))),
        }
    });
}

pub fn create_group(conn: Connection, me: SteamId, active: ActiveGroups, name: Option<String>, invitees: Vec<SteamId>, sink: Sink) {
    tokio::spawn(async move {
        let req = CChatRoomCreateChatRoomGroupRequest {
            name: name.as_deref().map(str::trim).filter(|s| !s.is_empty()).map(str::to_string),
            steamid_invitees: invitees,
            ..Default::default()
        };
        match conn.call::<_, CChatRoomCreateChatRoomGroupResponse>("ChatRoom.CreateChatRoomGroup#1", &req).await {
            Ok(r) => {
                let gid = r.chat_group_id();
                let mut g = ChatGroup { id: gid, ..Default::default() };
                if let Some(st) = &r.state {
                    apply_group_state(&mut g, st, r.user_chat_state.as_ref(), me);
                }
                if let Some(save_name) = name.as_deref().map(str::trim).filter(|s| !s.is_empty()) {
                    let _ = conn
                        .call::<_, CChatRoomSaveChatRoomGroupResponse>(
                            "ChatRoom.SaveChatRoomGroup#1",
                            &CChatRoomSaveChatRoomGroupRequest { chat_group_id: Some(gid), name: Some(save_name.to_string()) },
                        )
                        .await;
                    g.name = save_name.to_string();
                }
                let ids: Vec<SteamId> = g.members.iter().map(|m| m.steam_id).collect();
                sink(Event::ChatGroupUpdated(g));
                sink(Event::GroupJoined(gid));
                request_personas(&conn, me, ids);
                load_my_groups(conn, me, active, sink);
            }
            Err(e) => sink(Event::Error(format!("Couldn't create group chat: {e}"))),
        }
    });
}

pub fn join_group(conn: Connection, me: SteamId, active: ActiveGroups, group_id: Option<GroupChatId>, chat_id: Option<ChatRoomId>, invite_code: Option<String>, sink: Sink) {
    tokio::spawn(async move {
        let mut gid = group_id.unwrap_or(0);
        let mut cid = chat_id;
        if gid == 0 {
            if let Some(code) = &invite_code {
                match conn
                    .call::<_, CChatRoomGetInviteLinkInfoResponse>(
                        "ChatRoom.GetInviteLinkInfo#1",
                        &CChatRoomGetInviteLinkInfoRequest { invite_code: Some(code.clone()) },
                    )
                    .await
                {
                    Ok(info) => {
                        gid = info.group_summary.as_ref().map(|s| s.chat_group_id()).unwrap_or(0);
                        if cid.is_none() && info.chat_id() != 0 {
                            cid = Some(info.chat_id());
                        }
                    }
                    Err(e) => {
                        sink(Event::Error(format!("Invalid invite link: {e}")));
                        return;
                    }
                }
            }
        }
        if gid == 0 {
            sink(Event::Error("Missing group ID for invite".into()));
            return;
        }
        let req = CChatRoomJoinChatRoomGroupRequest {
            chat_group_id: Some(gid),
            invite_code,
            chat_id: cid,
        };
        match conn.call::<_, CChatRoomJoinChatRoomGroupResponse>("ChatRoom.JoinChatRoomGroup#1", &req).await {
            Ok(r) => {
                let mut g = ChatGroup { id: gid, ..Default::default() };
                if let Some(st) = &r.state {
                    apply_group_state(&mut g, st, r.user_chat_state.as_ref(), me);
                }
                let ids: Vec<SteamId> = g.members.iter().map(|m| m.steam_id).collect();
                sink(Event::ChatGroupUpdated(g));
                sink(Event::GroupJoined(gid));
                request_personas(&conn, me, ids);
                load_my_groups(conn, me, active, sink);
            }
            Err(e) => sink(Event::Error(format!("Couldn't join group chat: {e}"))),
        }
    });
}

fn invite_info_from_summary(
    key: String,
    summary: Option<&CChatRoomGetChatRoomGroupSummaryResponse>,
    chat_id: ChatRoomId,
    banned: bool,
    kick_expires: u32,
    expires: u32,
    already_member: bool,
) -> GroupInviteInfo {
    let gid = summary.map(|s| s.chat_group_id()).unwrap_or(0);
    let raw_name = summary.map(|s| s.chat_group_name()).unwrap_or("");
    let name = if raw_name.trim().is_empty() { format!("Group #{gid}") } else { raw_name.to_string() };
    GroupInviteInfo {
        key,
        group_id: gid,
        chat_id,
        name,
        avatar_url: summary.and_then(|s| avatar_url(s.chat_group_avatar_sha(), s.avatar_ugc_url(), s.appid())),
        member_count: summary.map(|s| s.active_member_count()).unwrap_or(0),
        banned,
        kick_expires,
        expires,
        already_member,
    }
}

pub fn resolve_invite(conn: Connection, me: SteamId, invite: GroupInviteRef, sink: Sink) {
    tokio::spawn(async move {
        let key = invite.key();
        let result = match invite {
            GroupInviteRef::Link { code } => conn
                .call::<_, CChatRoomGetInviteLinkInfoResponse>(
                    "ChatRoom.GetInviteLinkInfo#1",
                    &CChatRoomGetInviteLinkInfoRequest { invite_code: Some(code) },
                )
                .await
                .map(|r| {
                    invite_info_from_summary(
                        key.clone(),
                        r.group_summary.as_ref(),
                        r.chat_id(),
                        r.banned(),
                        r.time_kick_expire(),
                        r.time_expires(),
                        r.user_chat_group_state.is_some(),
                    )
                })
                .map_err(|e| e.to_string()),
            GroupInviteRef::Direct { group_id, chat_id } => conn
                .call::<_, CChatRoomGetInviteInfoResponse>(
                    "ChatRoom.GetInviteInfo#1",
                    &CChatRoomGetInviteInfoRequest {
                        steamid_invitee: Some(me),
                        chat_group_id: Some(group_id),
                        chat_id: (chat_id != 0).then_some(chat_id),
                        invite_code: None,
                    },
                )
                .await
                .map(|r| {
                    invite_info_from_summary(
                        key.clone(),
                        r.group_summary.as_ref(),
                        chat_id,
                        r.banned(),
                        r.time_kick_expire(),
                        0,
                        false,
                    )
                })
                .map_err(|e| e.to_string()),
        };
        sink(Event::GroupInviteResolved { key, result });
    });
}

pub fn ack_message(conn: &Connection, group_id: GroupChatId, chat_id: ChatRoomId, timestamp: u32) {
    let _ = conn.notify_service(
        "ChatRoom.AckChatMessage#1",
        &CChatRoomAckChatMessageNotification {
            chat_group_id: Some(group_id),
            chat_id: Some(chat_id),
            timestamp: Some(timestamp),
        },
    );
}

pub fn invite_friend(conn: Connection, group_id: GroupChatId, chat_id: Option<ChatRoomId>, friend: SteamId, sink: Sink) {
    tokio::spawn(async move {
        let req = CChatRoomInviteFriendToChatRoomGroupRequest {
            chat_group_id: Some(group_id),
            steamid: Some(friend),
            chat_id,
            skip_friendsui_check: Some(true),
        };
        if let Err(e) = conn.call::<_, CChatRoomInviteFriendToChatRoomGroupResponse>("ChatRoom.InviteFriendToChatRoomGroup#1", &req).await {
            sink(Event::Error(format!("Couldn't invite friend to group: {e}")));
        }
    });
}

pub fn leave_group(conn: Connection, active: ActiveGroups, group_id: GroupChatId, sink: Sink) {
    tokio::spawn(async move {
        let req = CChatRoomLeaveChatRoomGroupRequest { chat_group_id: Some(group_id) };
        match conn.call::<_, CChatRoomLeaveChatRoomGroupResponse>("ChatRoom.LeaveChatRoomGroup#1", &req).await {
            Ok(_) => {
                remove_active(&active, group_id);
                sink(Event::ChatGroupRemoved(group_id));
            }
            Err(e) => sink(Event::Error(format!("Couldn't leave group chat: {e}"))),
        }
    });
}

pub fn save_group(conn: Connection, me: SteamId, active: ActiveGroups, group_id: GroupChatId, name: String, sink: Sink) {
    tokio::spawn(async move {
        let req = CChatRoomSaveChatRoomGroupRequest { chat_group_id: Some(group_id), name: Some(name) };
        match conn.call::<_, CChatRoomSaveChatRoomGroupResponse>("ChatRoom.SaveChatRoomGroup#1", &req).await {
            Ok(_) => load_group_state(conn, me, group_id, active, sink),
            Err(e) => sink(Event::Error(format!("Couldn't save group chat: {e}"))),
        }
    });
}

pub fn rename_group(conn: Connection, me: SteamId, active: ActiveGroups, group_id: GroupChatId, name: String, sink: Sink) {
    tokio::spawn(async move {
        let req = CChatRoomRenameChatRoomGroupRequest { chat_group_id: Some(group_id), name: Some(name) };
        match conn.call::<_, CChatRoomRenameChatRoomGroupResponse>("ChatRoom.RenameChatRoomGroup#1", &req).await {
            Ok(_) => load_group_state(conn, me, group_id, active, sink),
            Err(e) => sink(Event::Error(format!("Couldn't rename group chat: {e}"))),
        }
    });
}

pub fn set_tagline(conn: Connection, me: SteamId, active: ActiveGroups, group_id: GroupChatId, tagline: String, sink: Sink) {
    tokio::spawn(async move {
        let req = CChatRoomSetChatRoomGroupTaglineRequest { chat_group_id: Some(group_id), tagline: Some(tagline) };
        match conn.call::<_, CChatRoomSetChatRoomGroupTaglineResponse>("ChatRoom.SetChatRoomGroupTagline#1", &req).await {
            Ok(_) => load_group_state(conn, me, group_id, active, sink),
            Err(e) => sink(Event::Error(format!("Couldn't update group tagline: {e}"))),
        }
    });
}

pub fn create_channel(conn: Connection, me: SteamId, active: ActiveGroups, group_id: GroupChatId, name: String, allow_voice: bool, sink: Sink) {
    tokio::spawn(async move {
        let req = CChatRoomCreateChatRoomRequest { chat_group_id: Some(group_id), name: Some(name), allow_voice: Some(allow_voice) };
        match conn.call::<_, CChatRoomCreateChatRoomResponse>("ChatRoom.CreateChatRoom#1", &req).await {
            Ok(_) => load_group_state(conn, me, group_id, active, sink),
            Err(e) => sink(Event::Error(format!("Couldn't create channel: {e}"))),
        }
    });
}

pub fn rename_channel(conn: Connection, me: SteamId, active: ActiveGroups, group_id: GroupChatId, chat_id: ChatRoomId, name: String, sink: Sink) {
    tokio::spawn(async move {
        let req = CChatRoomRenameChatRoomRequest { chat_group_id: Some(group_id), chat_id: Some(chat_id), name: Some(name) };
        match conn.call::<_, CChatRoomRenameChatRoomResponse>("ChatRoom.RenameChatRoom#1", &req).await {
            Ok(_) => load_group_state(conn, me, group_id, active, sink),
            Err(e) => sink(Event::Error(format!("Couldn't rename channel: {e}"))),
        }
    });
}

pub fn delete_channel(conn: Connection, me: SteamId, active: ActiveGroups, group_id: GroupChatId, chat_id: ChatRoomId, sink: Sink) {
    tokio::spawn(async move {
        let req = CChatRoomDeleteChatRoomRequest { chat_group_id: Some(group_id), chat_id: Some(chat_id) };
        match conn.call::<_, CChatRoomDeleteChatRoomResponse>("ChatRoom.DeleteChatRoom#1", &req).await {
            Ok(_) => load_group_state(conn, me, group_id, active, sink),
            Err(e) => sink(Event::Error(format!("Couldn't delete channel: {e}"))),
        }
    });
}

pub fn create_invite_link(conn: Connection, group_id: GroupChatId, chat_id: Option<ChatRoomId>, seconds_valid: u32, sink: Sink) {
    tokio::spawn(async move {
        let req = CChatRoomCreateInviteLinkRequest {
            chat_group_id: Some(group_id),
            seconds_valid: Some(seconds_valid),
            chat_id,
        };
        match conn.call::<_, CChatRoomCreateInviteLinkResponse>("ChatRoom.CreateInviteLink#1", &req).await {
            Ok(_) => load_invite_links(conn, group_id, sink),
            Err(e) => sink(Event::Error(format!("Couldn't create invite link: {e}"))),
        }
    });
}

pub fn load_invite_links(conn: Connection, group_id: GroupChatId, sink: Sink) {
    tokio::spawn(async move {
        let req = CChatRoomGetInviteLinksForGroupRequest { chat_group_id: Some(group_id) };
        let result = conn
            .call::<_, CChatRoomGetInviteLinksForGroupResponse>("ChatRoom.GetInviteLinksForGroup#1", &req)
            .await
            .map(|r| {
                r.invite_links
                    .iter()
                    .map(|l| GroupInviteLink {
                        code: l.invite_code().to_string(),
                        creator: l.steamid_creator(),
                        expires: l.time_expires(),
                        chat_id: l.chat_id(),
                    })
                    .collect()
            })
            .map_err(|e| e.to_string());
        sink(Event::GroupInviteLinks { group_id, result });
    });
}

pub fn delete_invite_link(conn: Connection, group_id: GroupChatId, code: String, sink: Sink) {
    tokio::spawn(async move {
        let req = CChatRoomDeleteInviteLinkRequest { chat_group_id: Some(group_id), invite_code: Some(code) };
        match conn.call::<_, CChatRoomDeleteInviteLinkResponse>("ChatRoom.DeleteInviteLink#1", &req).await {
            Ok(_) => load_invite_links(conn, group_id, sink),
            Err(e) => sink(Event::Error(format!("Couldn't delete invite link: {e}"))),
        }
    });
}

pub fn kick_member(conn: Connection, group_id: GroupChatId, steam_id: SteamId, expiration_secs: i32, sink: Sink) {
    tokio::spawn(async move {
        let req = CChatRoomKickUserRequest {
            chat_group_id: Some(group_id),
            steamid: Some(steam_id),
            expiration: Some(expiration_secs),
        };
        if let Err(e) = conn.call::<_, CChatRoomKickUserResponse>("ChatRoom.KickUserFromGroup#1", &req).await {
            sink(Event::Error(format!("Couldn't kick member: {e}")));
        }
    });
}

pub fn ban_member(conn: Connection, group_id: GroupChatId, steam_id: SteamId, ban: bool, sink: Sink) {
    tokio::spawn(async move {
        let req = CChatRoomSetUserBanStateRequest { chat_group_id: Some(group_id), steamid: Some(steam_id), ban_state: Some(ban) };
        match conn.call::<_, CChatRoomSetUserBanStateResponse>("ChatRoom.SetUserBanState#1", &req).await {
            Ok(_) => load_ban_list(conn, group_id, sink),
            Err(e) => sink(Event::Error(format!("Couldn't update ban state: {e}"))),
        }
    });
}

pub fn load_ban_list(conn: Connection, group_id: GroupChatId, sink: Sink) {
    tokio::spawn(async move {
        let req = CChatRoomGetBanListRequest { chat_group_id: Some(group_id) };
        let result = conn
            .call::<_, CChatRoomGetBanListResponse>("ChatRoom.GetBanList#1", &req)
            .await
            .map(|r| {
                r.bans
                    .iter()
                    .map(|b| GroupBanInfo {
                        steam_id: account_to_steam_id(b.accountid()),
                        actor: account_to_steam_id(b.accountid_actor()),
                        time_banned: b.time_banned(),
                        reason: b.ban_reason().to_string(),
                    })
                    .collect()
            })
            .map_err(|e| e.to_string());
        sink(Event::GroupBanList { group_id, result });
    });
}

#[allow(clippy::too_many_arguments)]
pub fn update_reaction(
    conn: Connection,
    me: SteamId,
    group_id: GroupChatId,
    chat_id: ChatRoomId,
    timestamp: u32,
    ordinal: u32,
    reaction_type: i32,
    reaction: String,
    add: bool,
    sink: Sink,
) {
    tokio::spawn(async move {
        let req = CChatRoomUpdateMessageReactionRequest {
            chat_group_id: Some(group_id),
            chat_id: Some(chat_id),
            server_timestamp: Some(timestamp),
            ordinal: Some(ordinal),
            reaction_type: Some(reaction_type),
            reaction: Some(reaction.clone()),
            is_add: Some(add),
        };
        match conn.call::<_, CChatRoomUpdateMessageReactionResponse>("ChatRoom.UpdateMessageReaction#1", &req).await {
            Ok(_) => sink(Event::GroupMessageReaction {
                group_id,
                chat_id,
                timestamp,
                ordinal,
                reaction_type,
                reaction,
                reactor: me,
                is_add: add,
            }),
            Err(e) => sink(Event::Error(format!("Couldn't update reaction: {e}"))),
        }
    });
}

pub fn delete_messages(conn: Connection, group_id: GroupChatId, chat_id: ChatRoomId, messages: Vec<(u32, u32)>, sink: Sink) {
    tokio::spawn(async move {
        let req = CChatRoomDeleteChatMessagesRequest {
            chat_group_id: Some(group_id),
            chat_id: Some(chat_id),
            messages: messages
                .iter()
                .map(|&(t, o)| c_chat_room_delete_chat_messages_request::Message { server_timestamp: Some(t), ordinal: Some(o) })
                .collect(),
        };
        match conn.call::<_, CChatRoomDeleteChatMessagesResponse>("ChatRoom.DeleteChatMessages#1", &req).await {
            Ok(_) => sink(Event::GroupMessageDeleted { group_id, chat_id, messages }),
            Err(e) => sink(Event::Error(format!("Couldn't delete messages: {e}"))),
        }
    });
}

pub fn set_preferences(conn: Connection, me: SteamId, active: ActiveGroups, group_id: GroupChatId, unread_muted: bool, desktop_level: Option<i32>, sink: Sink) {
    tokio::spawn(async move {
        let req = CChatRoomSetUserChatGroupPreferencesRequest {
            chat_group_id: Some(group_id),
            chat_group_preferences: Some(c_chat_room_set_user_chat_group_preferences_request::ChatGroupPreferences {
                unread_indicator_muted: Some(unread_muted),
                desktop_notification_level: desktop_level,
                ..Default::default()
            }),
            ..Default::default()
        };
        if let Err(e) = conn.call::<_, CChatRoomSetUserChatGroupPreferencesResponse>("ChatRoom.SetUserChatGroupPreferences#1", &req).await {
            sink(Event::Error(format!("Couldn't save group notification settings: {e}")));
        } else {
            load_my_groups(conn, me, active, sink);
        }
    });
}

pub fn search_members(conn: Connection, group_id: GroupChatId, query: String, sink: Sink) {
    tokio::spawn(async move {
        let req = CChatRoomSearchMembersRequest {
            chat_group_id: Some(group_id),
            search_id: Some(1),
            search_text: Some(query),
            max_results: Some(50),
        };
        if let Ok(resp) = conn.call::<_, CChatRoomSearchMembersResponse>("ChatRoom.SearchMembers#1", &req).await {
            let mut results = Vec::new();
            for m in &resp.matching_members {
                if let Some(p) = &m.persona {
                    results.push(persona_from_proto(p));
                } else {
                    results.push(Persona { steam_id: account_to_steam_id(m.accountid() as u32), ..Default::default() });
                }
            }
            if !results.is_empty() {
                sink(Event::Personas(results.clone()));
            }
            sink(Event::GroupMemberSearch { group_id, results });
        }
    });
}

/// Handle incoming `ChatRoomClient.*` service notifications pushed by Steam CM.
pub fn handle_notification(conn: &Connection, me: SteamId, active: &ActiveGroups, name: &str, p: &net::Packet, sink: &Sink) -> net::Result<()> {
    if name.starts_with("ChatRoomClient.NotifyIncomingChatMessage") {
        let n: CChatRoomIncomingChatMessageNotification = p.body()?;
        let msg = message_from_notification(me, &n);
        let sender = msg.from;
        sink(Event::GroupMessage(msg));
        request_personas(conn, me, [sender]);
    } else if name.starts_with("ChatRoomClient.NotifyChatMessageModified") {
        let n: CChatRoomChatMessageModifiedNotification = p.body()?;
        let deleted: Vec<(u32, u32)> = n
            .messages
            .iter()
            .filter(|m| m.deleted())
            .map(|m| (m.server_timestamp(), m.ordinal()))
            .collect();
        if !deleted.is_empty() {
            sink(Event::GroupMessageDeleted {
                group_id: n.chat_group_id(),
                chat_id: n.chat_id(),
                messages: deleted,
            });
        }
    } else if name.starts_with("ChatRoomClient.NotifyMessageReaction") {
        let n: CChatRoomMessageReactionNotification = p.body()?;
        sink(Event::GroupMessageReaction {
            group_id: n.chat_group_id(),
            chat_id: n.chat_id(),
            timestamp: n.server_timestamp(),
            ordinal: n.ordinal(),
            reaction_type: n.reaction_type().into(),
            reaction: n.reaction().to_string(),
            reactor: n.reactor(),
            is_add: n.is_add(),
        });
    } else if name.starts_with("ChatRoomClient.NotifyAckChatMessageEcho") {
        let n: CChatRoomAckChatMessageNotification = p.body()?;
        sink(Event::GroupAck {
            group_id: n.chat_group_id(),
            chat_id: n.chat_id(),
            timestamp: n.timestamp(),
        });
    } else if name.starts_with("ChatRoomClient.NotifyMemberStateChange") {
        let n: CChatRoomMemberStateChangeNotification = p.body()?;
        if let Some(m) = &n.member {
            let member = member_from_proto(m);
            let id = member.steam_id;
            sink(Event::GroupMemberChanged {
                group_id: n.chat_group_id(),
                member,
                change: n.change().into(),
            });
            request_personas(conn, me, [id]);
        }
    } else if name.starts_with("ChatRoomClient.NotifyChatRoomHeaderStateChange") {
        let n: CChatRoomChatRoomHeaderStateNotification = p.body()?;
        if let Some(h) = &n.header_state {
            let mut g = ChatGroup { id: h.chat_group_id(), ..Default::default() };
            apply_header(&mut g, h);
            sink(Event::GroupHeaderUpdated(g));
        }
    } else if name.starts_with("ChatRoomClient.NotifyChatRoomGroupRoomsChange") {
        let n: CChatRoomChatRoomGroupRoomsChangeNotification = p.body()?;
        let mut g = ChatGroup::default();
        apply_rooms(&mut g, n.default_chat_id(), &n.chat_rooms);
        sink(Event::GroupRoomsChanged {
            group_id: n.chat_group_id(),
            default_chat_id: n.default_chat_id(),
            rooms: g.rooms,
        });
    } else if name.starts_with("ChatRoomClient.NotifyChatGroupUserStateChanged") {
        let n: ChatRoomClientNotifyChatGroupUserStateChangedNotification = p.body()?;
        let gid = n.chat_group_id();
        // EChatRoomMemberStateChange: 2 Parted, 3 Kicked, 10 Banned.
        if matches!(n.user_action() as i32, 2 | 3 | 10) {
            remove_active(active, gid);
            sink(Event::ChatGroupRemoved(gid));
        } else if let Some(summary) = &n.group_summary {
            let g = group_from_summary(summary, n.user_chat_group_state.as_ref());
            let ids = g.top_members.clone();
            sink(Event::ChatGroupUpdated(g));
            request_personas(conn, me, ids);
            // Activate the newly joined group for live messages.
            load_my_groups(conn.clone(), me, active.clone(), sink.clone());
        } else {
            load_my_groups(conn.clone(), me, active.clone(), sink.clone());
        }
    } else if name.starts_with("ChatRoomClient.NotifyChatRoomDisconnect") {
        load_my_groups(conn.clone(), me, active.clone(), sink.clone());
    } else if name.starts_with("ChatRoomClient.NotifyMemberListViewUpdated") {
        let n: CChatRoomClientMemberListViewUpdatedNotification = p.body()?;
        let gid = n.chat_group_id();
        let mut personas = Vec::new();
        let mut members = Vec::new();
        for p in &n.subscribed_personas {
            personas.push(persona_from_proto(p));
        }
        for entry in &n.members {
            let acct = entry.accountid();
            if let Some(p) = &entry.persona {
                personas.push(persona_from_proto(p));
            }
            // In `MemberListViewEntry`, `rank` is the sorted list index (0..N), not EChatRoomGroupRank.
            members.push(ChatGroupMember {
                steam_id: account_to_steam_id(acct),
                account_id: acct,
                rank: 20,
                state: 2,
                time_kick_expire: 0,
                role_ids: Vec::new(),
            });
        }
        if !personas.is_empty() {
            sink(Event::Personas(personas));
        }
        if !members.is_empty() {
            let total = n.member_summary.as_ref().map(|sum| (sum.ingame() + sum.online() + sum.offline()).max(0) as u32);
            sink(Event::GroupMembersUpdated { group_id: gid, members, total, virtualized: true });
        }
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn avatar_url_restricts_ugc_urls_to_trusted_steam_https_hosts() {
        let trusted = "https://steamusercontent-a.akamaihd.net/ugc/12345/abc/";
        assert_eq!(avatar_url(&[], trusted, 0).as_deref(), Some(trusted));
        // Arbitrary third-party URLs or HTTP URLs must be rejected and fall back.
        assert_eq!(avatar_url(&[], "https://tracker.evil.example/pixel.png", 0), None);
        assert_eq!(avatar_url(&[], "http://steamusercontent-a.akamaihd.net/ugc/1/", 0), None);
        assert_eq!(
            avatar_url(&[], "https://tracker.evil.example/pixel.png", 440),
            Some(model::art::header(440))
        );
    }
}
