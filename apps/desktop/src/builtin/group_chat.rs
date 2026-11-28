//! Shared Group Chat UI: group list section, channel bar, message feed with
//! reactions/mentions/invites, member sidebar, and group management dialogs.
//! Reused across Community, the undocked Friends & Chat window, and the overlay.

use super::community::{Sender, format_time, game_invite_card};
use client::{Command, State};
use egui::{Align, Align2, Color32, CornerRadius, Key, Layout, Margin, Rect, Sense, Stroke, Ui, pos2, vec2};
use extensions::Action;
use i18n::t;
use model::{
    ChatGroup, ChatGroupMember, ChatRoomId, GroupChatId, GroupInviteRef, GroupMessage, GroupServerMessage, Persona, PersonaState, SteamId, art,
    account_to_steam_id, steam_id_to_account,
};
use std::collections::{HashMap, HashSet};
use ui::widgets::EditMenu;
use ui::{Theme, icons, widgets as w};

const QUICK_REACTIONS: &[&str] = &["👍", "❤️", "😂", "😮", "🎉", "🔥"];

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ChatTarget {
    Friend(SteamId),
    Group(GroupChatId),
}

#[derive(Clone)]
pub enum GroupModal {
    CreateGroup { name: String, filter: String, selected: HashSet<SteamId> },
    JoinByLink { input: String },
    InviteFriends { group_id: GroupChatId, chat_id: Option<ChatRoomId>, filter: String },
    GroupSettings { group_id: GroupChatId, name: String, tagline: String, unread_muted: bool, links_requested: bool, bans_requested: bool },
    CreateChannel { group_id: GroupChatId, name: String, allow_voice: bool },
    RenameChannel { group_id: GroupChatId, chat_id: ChatRoomId, name: String },
    ConfirmLeave { group_id: GroupChatId, name: String },
    ConfirmKick { group_id: GroupChatId, steam_id: SteamId, name: String },
    ConfirmBan { group_id: GroupChatId, steam_id: SteamId, name: String },
}

pub struct GroupUiState {
    pub chat_id: Option<ChatRoomId>,
    pub input: String,
    pub show_members: bool,
    pub member_filter: String,
    pub loaded_groups: HashSet<GroupChatId>,
    pub loaded_channels: HashMap<(GroupChatId, ChatRoomId), std::time::Instant>,
    pub modal: Option<GroupModal>,
    last_search: HashMap<GroupChatId, String>,
}

impl Default for GroupUiState {
    fn default() -> Self {
        Self {
            chat_id: None,
            input: String::new(),
            show_members: true,
            member_filter: String::new(),
            loaded_groups: HashSet::new(),
            loaded_channels: HashMap::new(),
            modal: None,
            last_search: HashMap::new(),
        }
    }
}

impl GroupUiState {
    pub fn open_create(&mut self) {
        self.modal = Some(GroupModal::CreateGroup { name: String::new(), filter: String::new(), selected: HashSet::new() });
    }

    pub fn open_join_link(&mut self) {
        self.modal = Some(GroupModal::JoinByLink { input: String::new() });
    }

    pub fn open_settings(&mut self, g: &ChatGroup) {
        self.modal = Some(GroupModal::GroupSettings {
            group_id: g.id,
            name: g.name.clone(),
            tagline: g.tagline.clone(),
            unread_muted: g.unread_muted,
            links_requested: false,
            bans_requested: false,
        });
    }
}

/// Group avatar or colored initials square.
pub fn group_avatar(ui: &mut Ui, t: &Theme, g: &ChatGroup, name: &str, size: f32) {
    let (rect, _) = ui.allocate_exact_size(egui::Vec2::splat(size), Sense::hover());
    let radius = CornerRadius::same((size * 0.22).round() as u8);
    if let Some(url) = &g.avatar_url {
        if w::paint_cover(ui, rect, url, radius) {
            return;
        }
    }
    ui.painter().rect_filled(rect, radius, w::mix(t.raised, t.accent, 0.25));
    let initials: String = name
        .split_whitespace()
        .filter_map(|w| w.chars().next())
        .take(2)
        .collect::<String>()
        .to_uppercase();
    let label = if initials.is_empty() { icons::USERS_THREE.to_string() } else { initials };
    ui.painter().text(rect.center(), Align2::CENTER_CENTER, label, w::font_bold((size * 0.4).max(11.0)), t.accent);
}

/// Group chats list section shared across Community, Friends & Chat window, and the overlay.
/// Returns the clicked group ID (or `None`).
#[allow(clippy::too_many_arguments)]
pub fn group_list_section(
    ui: &mut Ui,
    t: &Theme,
    state: &State,
    filter: &str,
    selected: Option<GroupChatId>,
    ui_state: &mut GroupUiState,
    client: &impl Sender,
    actions: &mut Vec<Action>,
) -> Option<GroupChatId> {
    let q = filter.trim().to_lowercase();
    let groups: Vec<(&ChatGroup, String)> = state
        .group_list()
        .into_iter()
        .map(|g| (g, state.group_display_name(g)))
        .filter(|(_, name)| q.is_empty() || name.to_lowercase().contains(&q))
        .collect();

    ui.horizontal(|ui| {
        w::caption(ui, t, &format!("{} · {}", t!("Group chats"), groups.len()));
        ui.with_layout(Layout::right_to_left(Align::Center), |ui| {
            if w::icon_button_sized(ui, t, icons::PLUS, t!("Create group chat"), false, 0, 24.0).clicked() {
                ui_state.open_create();
            }
            if w::icon_button_sized(ui, t, icons::LINK, t!("Join group by link"), false, 0, 24.0).clicked() {
                ui_state.open_join_link();
            }
        });
    });
    ui.add_space(2.0);
    if groups.is_empty() {
        ui.label(w::text(t!("No group chats yet"), 12.0, t.text_faint));
        return None;
    }

    let mut clicked = None;
    egui::ScrollArea::vertical().id_salt(ui.next_auto_id()).max_height(200.0).auto_shrink([false, true]).show(ui, |ui| {
        for (g, name) in groups {
            let is_selected = selected == Some(g.id);
            let unread = g.unread_count();
            let (rect, resp) = ui.allocate_exact_size(vec2(ui.available_width(), 48.0), Sense::click());
            let pointer_in = ui.rect_contains_pointer(rect) || resp.context_menu_opened();
            let hover = ui.ctx().animate_bool_with_time(resp.id, pointer_in, 0.1);
            if is_selected {
                ui.painter().rect_filled(rect, t.corner_small(), t.accent.gamma_multiply(0.18));
            } else if hover > 0.0 {
                ui.painter().rect_filled(rect, t.corner_small(), w::mix(Color32::TRANSPARENT, t.surface_hover, hover));
            }
            let inner = rect.shrink2(vec2(6.0, 6.0));
            let inner = if unread > 0 { Rect::from_min_max(inner.min, pos2(inner.right() - 26.0, inner.bottom())) } else { inner };
            let mut child = ui.new_child(egui::UiBuilder::new().max_rect(inner).layout(Layout::left_to_right(Align::Center)));
            group_avatar(&mut child, t, g, &name, 36.0);
            child.add_space(4.0);
            child.vertical(|ui| {
                ui.spacing_mut().item_spacing.y = 1.0;
                ui.add(egui::Label::new(w::text_medium(&name, 14.0, t.text)).truncate());
                let sub = if g.active_voice_member_count > 0 {
                    format!("{} {} · {}", icons::SPEAKER_HIGH, t!("{n} in voice", n = g.active_voice_member_count), t!("{n} members", n = g.active_member_count.max(g.members.len() as u32)))
                } else {
                    t!("{n} members", n = g.active_member_count.max(g.members.len() as u32))
                };
                ui.add(egui::Label::new(w::text(sub, 12.0, if g.active_voice_member_count > 0 { t.online } else { t.text_dim })).truncate());
            });
            if unread > 0 {
                let c = rect.right_center() - vec2(16.0, 0.0);
                ui.painter().circle_filled(c, 9.0, t.accent);
                ui.painter().text(c, Align2::CENTER_CENTER, unread.min(99).to_string(), w::font_bold(10.0), t.on_accent);
            }

            let g_clone = g.clone();
            let name_clone = name.clone();
            resp.context_menu(|ui| {
                if ui.button(format!("{}  {}", icons::CHAT, t!("Open group chat"))).clicked() {
                    actions.push(Action::OpenGroupChat { group_id: g_clone.id, chat_id: None });
                    ui.close();
                }
                if g_clone.effective_permissions(steam_id_to_account(state.steam_id)).can_invite {
                    if ui.button(format!("{}  {}", icons::USER_PLUS, t!("Invite friends…"))).clicked() {
                        ui_state.modal = Some(GroupModal::InviteFriends { group_id: g_clone.id, chat_id: None, filter: String::new() });
                        ui.close();
                    }
                }
                let mute_label = if g_clone.unread_muted { t!("Unmute notifications") } else { t!("Mute notifications") };
                if ui.button(format!("{}  {mute_label}", icons::BELL)).clicked() {
                    client.send(Command::SetGroupNotificationPrefs {
                        group_id: g_clone.id,
                        unread_muted: !g_clone.unread_muted,
                        desktop_level: Some(g_clone.desktop_notification_level),
                    });
                    ui.close();
                }
                if ui.button(format!("{}  {}", icons::GEAR, t!("Group settings…"))).clicked() {
                    ui_state.open_settings(&g_clone);
                    ui.close();
                }
                ui.separator();
                if ui.button(w::text(t!("Leave group chat…"), 13.5, t.danger)).clicked() {
                    ui_state.modal = Some(GroupModal::ConfirmLeave { group_id: g_clone.id, name: name_clone.clone() });
                    ui.close();
                }
            });
            if resp.on_hover_cursor(egui::CursorIcon::PointingHand).clicked() {
                clicked = Some(g.id);
            }
        }
    });
    clicked
}

/// Header action buttons for an open group chat (Save ad-hoc, Invite, Members toggle, Settings menu).
pub fn header_actions(ui: &mut Ui, t: &Theme, state: &State, client: &impl Sender, g: &ChatGroup, ui_state: &mut GroupUiState, compact: bool) {
    let perms = g.effective_permissions(steam_id_to_account(state.steam_id));
    let name = state.group_display_name(g);
    let r = w::icon_button(ui, t, icons::GEAR, t!("Group settings"), false, 0);
    egui::Popup::menu(&r).show(|ui| {
        if w::menu_item(ui, t, icons::GEAR, t!("Group settings & links…")).clicked() {
            ui_state.open_settings(g);
        }
        let mute_label = if g.unread_muted { t!("Unmute notifications") } else { t!("Mute notifications") };
        if w::menu_item(ui, t, icons::BELL, mute_label).clicked() {
            client.send(Command::SetGroupNotificationPrefs {
                group_id: g.id,
                unread_muted: !g.unread_muted,
                desktop_level: Some(g.desktop_notification_level),
            });
        }
        if perms.can_create_rename_delete_channel {
            if w::menu_item(ui, t, icons::PLUS, t!("New channel…")).clicked() {
                ui_state.modal = Some(GroupModal::CreateChannel { group_id: g.id, name: String::new(), allow_voice: false });
            }
        }
        w::menu_separator(ui, t);
        if w::menu_item_tinted(ui, t, icons::SIGN_OUT, t!("Leave group chat…"), Some(t.danger)).clicked() {
            ui_state.modal = Some(GroupModal::ConfirmLeave { group_id: g.id, name: name.clone() });
        }
    });
    if !compact && w::icon_button(ui, t, icons::USERS, t!("Toggle member list"), ui_state.show_members, 0).clicked() {
        ui_state.show_members = !ui_state.show_members;
    }
    if perms.can_invite && w::icon_button(ui, t, icons::USER_PLUS, t!("Invite friends"), false, 0).clicked() {
        ui_state.modal = Some(GroupModal::InviteFriends { group_id: g.id, chat_id: ui_state.chat_id, filter: String::new() });
    }
    if !g.is_saved() && w::button(ui, t, None, t!("Save group"), w::Kind::Secondary, w::Size::Sm).clicked() {
        ui_state.open_settings(g);
    }
}

/// Full group chat body: channel bar, message feed with reactions/invites/mentions,
/// composer, and optional right-hand member list.
#[allow(clippy::too_many_arguments)]
pub fn group_view(
    ui: &mut Ui,
    t: &Theme,
    state: &State,
    client: &impl Sender,
    group_id: GroupChatId,
    ui_state: &mut GroupUiState,
    actions: &mut Vec<Action>,
    compact: bool,
    height: f32,
) {
    let Some(g) = state.chat_groups.get(&group_id).cloned() else {
        w::empty(ui, t, icons::USERS_THREE, t!("Group chat closed"), t!("Pick a friend or group chat from the list."));
        return;
    };
    if ui_state.loaded_groups.insert(group_id) || (!g.full_loaded && g.members.is_empty()) {
        client.send(Command::OpenChatGroup { group_id });
    }
    let active_chat_id = ui_state
        .chat_id
        .filter(|cid| g.rooms.iter().any(|r| r.id == *cid && !r.voice_allowed))
        .or_else(|| g.default_text_channel_id())
        .unwrap_or(g.default_chat_id);
    ui_state.chat_id = Some(active_chat_id);

    let channel = (group_id, active_chat_id);
    if active_chat_id != 0
        && (ui_state.loaded_channels.get(&channel).is_none_or(|at| {
            state.group_messages.get(&channel).is_none() && at.elapsed() > std::time::Duration::from_secs(15)
        }))
    {
        ui_state.loaded_channels.insert(channel, std::time::Instant::now());
        client.send(Command::LoadGroupHistory { group_id, chat_id: active_chat_id, before: None });
    }
    if g.channel(active_chat_id).is_some_and(|ch| ch.has_unread()) {
        actions.push(Action::MarkGroupRead { group_id, chat_id: active_chat_id });
    }

    let perms = g.effective_permissions(steam_id_to_account(state.steam_id));
    let total_h = height.max(80.0);
    let top_y = ui.cursor().top();
    // Channel strip.
    channel_bar(ui, t, &g, active_chat_id, perms.can_create_rename_delete_channel, ui_state, client);
    ui.add_space(4.0);
    ui.separator();
    ui.add_space(4.0);

    let used_h = (ui.cursor().top() - top_y).max(0.0);
    let body_h = (total_h - used_h).floor().max(60.0);
    let show_members = !compact && ui_state.show_members && ui.available_width() > 480.0;
    let member_w = if show_members { 200.0 } else { 0.0 };
    let feed_w = (ui.available_width() - if show_members { member_w + 12.0 } else { 0.0 }).max(160.0);

    ui.allocate_ui_with_layout(vec2(ui.available_width(), body_h), Layout::left_to_right(Align::Min), |ui| {
        ui.set_min_height(body_h);
        ui.set_max_height(body_h);
        ui.allocate_ui_with_layout(vec2(feed_w, body_h), Layout::top_down(Align::Min), |ui| {
            ui.set_min_height(body_h);
            ui.set_max_height(body_h);
            message_feed(ui, t, state, client, &g, active_chat_id, &perms, ui_state, actions, body_h);
        });
        if show_members {
            ui.add_space(6.0);
            ui.separator();
            ui.add_space(6.0);
            ui.allocate_ui_with_layout(vec2(member_w - 8.0, body_h), Layout::top_down(Align::Min), |ui| {
                ui.set_min_height(body_h);
                ui.set_max_height(body_h);
                member_list(ui, t, state, client, &g, &perms, ui_state, actions, body_h);
            });
        }
    });
}

fn channel_bar(
    ui: &mut Ui,
    t: &Theme,
    g: &ChatGroup,
    active_chat_id: ChatRoomId,
    can_manage: bool,
    ui_state: &mut GroupUiState,
    client: &impl Sender,
) -> f32 {
    let mut rooms = g.rooms.clone();
    rooms.sort_by_key(|r| (r.voice_allowed, r.sort_order, r.id));
    let r = egui::ScrollArea::horizontal().id_salt(("gc-ch", g.id)).show(ui, |ui| {
        ui.horizontal(|ui| {
            ui.spacing_mut().item_spacing.x = 6.0;
            for ch in &rooms {
                let ch_name = if ch.name.trim().is_empty() { "General" } else { ch.name.as_str() };
                if !ch.voice_allowed {
                    let selected = ch.id == active_chat_id;
                    let unread = ch.unread_badge();
                    let label = if unread > 0 { format!("# {ch_name} ({unread})") } else { format!("# {ch_name}") };
                    let resp = w::chip(ui, t, &label, selected);
                    if resp.clicked() {
                        ui_state.chat_id = Some(ch.id);
                    }
                    if can_manage {
                        let gid = g.id;
                        let cid = ch.id;
                        let cname = ch_name.to_string();
                        resp.context_menu(|ui| {
                            if ui.button(t!("Rename channel…")).clicked() {
                                ui_state.modal = Some(GroupModal::RenameChannel { group_id: gid, chat_id: cid, name: cname.clone() });
                                ui.close();
                            }
                            if rooms.iter().filter(|r| !r.voice_allowed).count() > 1 && ui.button(w::text(t!("Delete channel"), 13.5, t.danger)).clicked() {
                                client.send(Command::DeleteChatChannel { group_id: gid, chat_id: cid });
                                ui.close();
                            }
                        });
                    }
                } else {
                    let n = ch.members_in_voice.len();
                    let label = if n > 0 { format!("{} {} ({n})", icons::SPEAKER_HIGH, ch_name) } else { format!("{} {}", icons::SPEAKER_HIGH, ch_name) };
                    let resp = w::tinted_chip(ui, t, &label, (n > 0).then_some(t.online), false);
                    let tip = if n > 0 {
                        t!("{n} in voice channel · Voice chat coming next", n = n)
                    } else {
                        t!("Voice channel · Voice chat coming next").to_string()
                    };
                    resp.on_hover_text(tip);
                }
            }
            if can_manage {
                if w::icon_button_sized(ui, t, icons::PLUS, t!("New channel"), false, 0, 28.0).clicked() {
                    ui_state.modal = Some(GroupModal::CreateChannel { group_id: g.id, name: String::new(), allow_voice: false });
                }
            }
        });
    });
    r.inner_rect.height()
}

#[allow(clippy::too_many_arguments)]
fn message_feed(
    ui: &mut Ui,
    t: &Theme,
    state: &State,
    client: &impl Sender,
    g: &ChatGroup,
    chat_id: ChatRoomId,
    perms: &model::ChatRoleActions,
    ui_state: &mut GroupUiState,
    actions: &mut Vec<Action>,
    height: f32,
) {
    let empty = Vec::new();
    let msgs = state.group_messages.get(&(g.id, chat_id)).unwrap_or(&empty);
    let composer_id = ui.scope_id().with(("gc-composer", g.id, chat_id));
    let composer_h = ui.data(|d| d.get_temp::<f32>(composer_id)).unwrap_or(46.0);
    let gap = 8.0;
    let spacing = ui.spacing().item_spacing.y;
    let list_h = (height - composer_h - gap - spacing).floor().max(40.0);

    ui.allocate_ui_with_layout(vec2(ui.available_width(), list_h), Layout::top_down(Align::Min), |ui| {
        ui.set_min_height(list_h);
        ui.set_max_height(list_h);
        egui::ScrollArea::vertical()
            .id_salt(("gc-msgs", g.id, chat_id))
            .max_height(list_h)
            .min_scrolled_height(list_h)
            .auto_shrink([false, false])
            .stick_to_bottom(true)
            .show(ui, |ui| {
                if state.group_more_history.get(&(g.id, chat_id)) == Some(&true) {
                    if let Some(oldest) = msgs.first() {
                        ui.horizontal(|ui| {
                            if ui.link(w::text(t!("Load older messages"), 12.5, t.accent)).clicked() {
                                client.send(Command::LoadGroupHistory {
                                    group_id: g.id,
                                    chat_id,
                                    before: Some((oldest.time as u32, oldest.ordinal)),
                                });
                            }
                        });
                        ui.add_space(4.0);
                    }
                }
                if msgs.is_empty() {
                    w::empty(ui, t, icons::CHAT, t!("No messages yet"), t!("Start the conversation!"));
                }
                let mut last: Option<(SteamId, u64)> = None;
                for m in msgs {
                    if let Some(srv) = &m.server_msg {
                        ui.add_space(4.0);
                        ui.label(w::text(format!("{} · {}", server_message_text(state, m.from, srv), format_time(m.time)), 12.0, t.text_faint).italics());
                        last = None;
                        continue;
                    }
                    if m.deleted {
                        ui.add_space(4.0);
                        ui.label(w::text(t!("(message deleted)"), 12.5, t.text_faint).italics());
                        last = None;
                        continue;
                    }
                    if m.text.is_empty() && m.media.is_empty() && m.invite.is_none() && m.group_invite.is_none() {
                        continue;
                    }
                    let new_group = last.is_none_or(|(from, time)| from != m.from || m.time.saturating_sub(time) > 300);
                    if new_group {
                        ui.add_space(10.0);
                        let p = state.persona(m.from);
                        ui.horizontal_top(|ui| {
                            w::avatar(ui, t, &art::avatar(p.avatar_hash.as_deref()), 34.0, None);
                            ui.vertical(|ui| {
                                ui.spacing_mut().item_spacing.y = 2.0;
                                ui.horizontal(|ui| {
                                    let me = m.from == state.steam_id;
                                    let name = if p.name.is_empty() {
                                        if me { t!("You").to_string() } else { m.from.to_string() }
                                    } else {
                                        p.display_name()
                                    };
                                    let color = if me {
                                        t.accent
                                    } else if g.owner_id == m.from {
                                        t.away
                                    } else {
                                        t.text
                                    };
                                    ui.label(w::text_semibold(name, 14.0, color));
                                    ui.label(w::text(format_time(m.time), 11.5, t.text_faint));
                                });
                                group_message_body(ui, t, state, client, g, chat_id, perms, m, ui_state, actions);
                            });
                        });
                    } else {
                        ui.horizontal_top(|ui| {
                            ui.add_space(42.0);
                            ui.vertical(|ui| group_message_body(ui, t, state, client, g, chat_id, perms, m, ui_state, actions));
                        });
                    }
                    last = Some((m.from, m.time));
                }
            });
    });

    ui.add_space(gap);
    let r = egui::Frame::new()
        .fill(t.raised)
        .stroke(Stroke::new(1.0, t.border))
        .corner_radius(t.corner())
        .inner_margin(Margin::symmetric(12, 8))
        .show(ui, |ui| {
            // Inline @mention suggestions while typing "@..."
            mention_suggestions(ui, t, state, g, perms.can_mention_all, &mut ui_state.input);
            let ch_name = g
                .channel(chat_id)
                .map(|c| c.name.trim())
                .filter(|s| !s.is_empty())
                .unwrap_or("General");
            let hint = if perms.can_chat {
                t!("Message #{channel}", channel = ch_name)
            } else {
                t!("You don't have permission to chat here").to_string()
            };
            ui.horizontal(|ui| {
                ui.add_enabled_ui(perms.can_chat, |ui| {
                    let edit = ui
                        .add(
                            egui::TextEdit::singleline(&mut ui_state.input)
                                .hint_text(w::text(hint, 14.0, t.text_faint))
                                .font(w::font(14.5))
                                .frame(egui::Frame::NONE)
                                .desired_width(ui.available_width() - 48.0),
                        )
                        .with_edit_menu();
                    let send = edit.lost_focus() && ui.input(|i| i.key_pressed(Key::Enter));
                    let clicked = w::icon_button(ui, t, icons::SEND, t!("Send"), false, 0).clicked();
                    if (send || clicked) && !ui_state.input.trim().is_empty() {
                        let member_names: Vec<(u32, String)> = g
                            .members
                            .iter()
                            .filter_map(|m| {
                                let p = state.persona(m.steam_id);
                                (!p.name.is_empty()).then_some((m.account_id, p.name))
                            })
                            .collect();
                        let refs: Vec<(u32, &str)> = member_names.iter().map(|(id, n)| (*id, n.as_str())).collect();
                        let encoded = model::chat::encode_mentions(ui_state.input.trim(), &refs);
                        client.send(Command::SendGroupMessage { group_id: g.id, chat_id, text: encoded });
                        ui_state.input.clear();
                        edit.request_focus();
                    }
                });
            });
        });
    let measured = r.response.rect.height();
    if (measured - composer_h).abs() > 0.5 {
        ui.data_mut(|d| d.insert_temp(composer_id, measured));
        ui.ctx().request_repaint();
    }
}

fn mention_suggestions(ui: &mut Ui, t: &Theme, state: &State, g: &ChatGroup, can_mention_all: bool, input: &mut String) {
    let Some(token) = input.rsplit(|c: char| c.is_whitespace()).next().and_then(|w| w.strip_prefix('@')) else {
        return;
    };
    let token_len = token.len();
    let q = token.to_lowercase();
    let mut candidates: Vec<(String, Option<String>)> = Vec::new();
    if can_mention_all {
        for kw in ["all", "here"] {
            if q.is_empty() || kw.starts_with(&q) {
                candidates.push((kw.to_string(), None));
            }
        }
    }
    for m in &g.members {
        if m.steam_id == state.steam_id {
            continue;
        }
        let p = state.persona(m.steam_id);
        if !p.name.is_empty() && (q.is_empty() || p.name.to_lowercase().contains(&q)) {
            candidates.push((p.name, p.avatar_hash));
        }
        if candidates.len() >= 5 {
            break;
        }
    }
    if candidates.is_empty() {
        return;
    }
    ui.horizontal_wrapped(|ui| {
        ui.spacing_mut().item_spacing = vec2(4.0, 4.0);
        for (name, _) in candidates {
            if w::chip(ui, t, &format!("@{name}"), false).clicked() {
                let keep = input.len().saturating_sub(token_len + 1);
                input.truncate(keep);
                input.push('@');
                input.push_str(&name);
                input.push(' ');
            }
        }
    });
    ui.add_space(4.0);
}

#[allow(clippy::too_many_arguments)]
fn group_message_body(
    ui: &mut Ui,
    t: &Theme,
    state: &State,
    client: &impl Sender,
    g: &ChatGroup,
    chat_id: ChatRoomId,
    perms: &model::ChatRoleActions,
    m: &GroupMessage,
    _ui_state: &mut GroupUiState,
    actions: &mut Vec<Action>,
) {
    let can_delete = m.from == state.steam_id || perms.can_kick;
    let populate_menu = |ui: &mut Ui| {
        ui.horizontal(|ui| {
            for emoji in QUICK_REACTIONS {
                if ui.small_button(*emoji).clicked() {
                    let already = m.reactions.iter().any(|r| r.reaction == *emoji && r.reacted);
                    client.send(Command::UpdateGroupReaction {
                        group_id: g.id,
                        chat_id,
                        timestamp: m.time as u32,
                        ordinal: m.ordinal,
                        reaction_type: 1,
                        reaction: (*emoji).to_string(),
                        add: !already,
                    });
                    ui.close();
                }
            }
        });
        super::chat_render::context_menu_items(ui, t, &m.text, &m.media);
        if can_delete && m.ordinal > 0 && ui.button(w::text(format!("{}  {}", icons::TRASH, t!("Delete message")), 13.5, t.danger)).clicked() {
            client.send(Command::DeleteGroupMessages { group_id: g.id, chat_id, messages: vec![(m.time as u32, m.ordinal)] });
            ui.close();
        }
    };
    let mut text_resp = None;
    let resp = egui::Frame::new()
        .fill(if m.mentioned_me { t.accent.gamma_multiply(0.10) } else { Color32::TRANSPARENT })
        .corner_radius(t.corner_small())
        .inner_margin(if m.mentioned_me { Margin::symmetric(6, 2) } else { Margin::ZERO })
        .show(ui, |ui| {
            if !m.text.is_empty() {
                text_resp = Some(super::chat_render::render_message_text(ui, t, &m.text, &m.media, t.text));
            }
            super::chat_render::render_media(ui, t, &m.text, &m.media);
            if let Some(inv) = &m.invite {
                let mut join = None;
                game_invite_card(ui, t, state, inv, m.from == state.steam_id, &mut join);
                if let Some(a) = join {
                    actions.push(a);
                }
            }
            if let Some(ginv) = &m.group_invite {
                let mut act = None;
                invite_card(ui, t, state, client, ginv, m.from == state.steam_id, &mut act);
                if let Some(a) = act {
                    actions.push(a);
                }
            }
            if !m.reactions.is_empty() {
                ui.add_space(2.0);
                ui.horizontal_wrapped(|ui| {
                    ui.spacing_mut().item_spacing = vec2(4.0, 4.0);
                    for rx in &m.reactions {
                        let label = format!("{} {}", rx.reaction, rx.count);
                        if w::chip(ui, t, &label, rx.reacted).clicked() {
                            client.send(Command::UpdateGroupReaction {
                                group_id: g.id,
                                chat_id,
                                timestamp: m.time as u32,
                                ordinal: m.ordinal,
                                reaction_type: rx.reaction_type,
                                reaction: rx.reaction.clone(),
                                add: !rx.reacted,
                            });
                        }
                    }
                });
            }
        })
        .response;

    if let Some(tr) = text_resp {
        tr.context_menu(populate_menu);
    }
    resp.context_menu(populate_menu);
}

/// Inline group chat invite card shared by 1:1 chat and group chat messages.
pub fn invite_card(
    ui: &mut Ui,
    t: &Theme,
    state: &State,
    client: &impl Sender,
    inv: &GroupInviteRef,
    mine: bool,
    out_action: &mut Option<Action>,
) {
    let key = inv.key();
    let ask_id = egui::Id::unique(("gc-inv-asked", &key));
    let asked = ui.ctx().data(|d| d.get_temp::<bool>(ask_id)).unwrap_or(false);
    if !state.group_invite_previews.contains_key(&key) && !asked {
        ui.ctx().data_mut(|d| d.insert_temp(ask_id, true));
        client.send(Command::ResolveGroupInvite { invite: inv.clone() });
    }
    let preview = state.group_invite_previews.get(&key);
    let width = ui.available_width().min(360.0);
    egui::Frame::new()
        .fill(t.raised)
        .stroke(Stroke::new(1.0, t.border))
        .corner_radius(t.corner())
        .inner_margin(Margin::same(10))
        .show(ui, |ui| {
            ui.set_width((width - 20.0).max(0.0));
            let head = if mine { t!("YOU SENT A GROUP INVITE") } else { t!("GROUP CHAT INVITE") };
            ui.label(w::text_bold(head, 10.5, t.text_faint));
            match preview {
                None => {
                    ui.label(w::text(t!("Loading invite…"), 13.0, t.text_dim));
                }
                Some(Err(e)) => {
                    ui.label(w::text(e, 13.0, t.text_dim));
                }
                Some(Ok(info)) => {
                    ui.horizontal(|ui| {
                        let dummy = ChatGroup { avatar_url: info.avatar_url.clone(), id: info.group_id, ..Default::default() };
                        group_avatar(ui, t, &dummy, &info.name, 40.0);
                        ui.vertical(|ui| {
                            ui.spacing_mut().item_spacing.y = 2.0;
                            ui.add(egui::Label::new(w::text_bold(&info.name, 14.0, t.text)).truncate());
                            ui.label(w::text(t!("{n} members", n = info.member_count), 12.0, t.text_dim));
                            let in_group = state.chat_groups.contains_key(&info.group_id);
                            if info.banned {
                                ui.label(w::text(t!("You are banned from this group"), 12.0, t.danger));
                            } else if in_group {
                                if w::button(ui, t, Some(icons::CHAT), t!("Open group"), w::Kind::Secondary, w::Size::Sm).clicked() {
                                    *out_action = Some(Action::OpenGroupChat { group_id: info.group_id, chat_id: (info.chat_id != 0).then_some(info.chat_id) });
                                }
                            } else if w::button(ui, t, Some(icons::USER_PLUS), t!("Join group"), w::Kind::Primary, w::Size::Sm).clicked() {
                                let code = match inv {
                                    GroupInviteRef::Link { code } => Some(code.clone()),
                                    GroupInviteRef::Direct { .. } => None,
                                };
                                *out_action = Some(Action::JoinGroupInvite {
                                    group_id: (info.group_id != 0).then_some(info.group_id),
                                    chat_id: (info.chat_id != 0).then_some(info.chat_id),
                                    code,
                                });
                            }
                        });
                    });
                }
            }
        });
}

#[allow(clippy::too_many_arguments)]
fn member_list(
    ui: &mut Ui,
    t: &Theme,
    state: &State,
    client: &impl Sender,
    g: &ChatGroup,
    perms: &model::ChatRoleActions,
    ui_state: &mut GroupUiState,
    actions: &mut Vec<Action>,
    height: f32,
) {
    w::search_sized(ui, t, &mut ui_state.member_filter, t!("Search members"), ui.available_width(), 28.0);
    ui.add_space(6.0);
    let q = ui_state.member_filter.trim().to_lowercase();
    if g.virtualized_members && !q.is_empty() && ui_state.last_search.get(&g.id).map(|s| s.as_str()) != Some(&q) {
        ui_state.last_search.insert(g.id, q.clone());
        client.send(Command::SearchGroupMembers { group_id: g.id, query: q.clone() });
    }
    let scroll_h = (height - 40.0).max(40.0);
    egui::ScrollArea::vertical().id_salt(("gc-members", g.id)).max_height(scroll_h).auto_shrink([false, false]).show(ui, |ui| {
        if g.virtualized_members && !q.is_empty() {
            let results = state.group_member_search.get(&g.id).cloned().unwrap_or_default();
            for p in results {
                member_row(ui, t, state, g, None, &p, perms, ui_state, actions);
            }
            return;
        }
        let mut entries: Vec<(&ChatGroupMember, Persona)> = g
            .members
            .iter()
            .map(|m| (m, state.persona(m.steam_id)))
            .filter(|(m, p)| {
                q.is_empty()
                    || p.display_name().to_lowercase().contains(&q)
                    || m.steam_id.to_string().contains(&q)
            })
            .collect();
        entries.sort_by_cached_key(|(m, p)| (std::cmp::Reverse(m.rank), p.state == PersonaState::Offline, !p.in_game(), p.display_name().to_lowercase()));

        let staff: Vec<_> = entries.iter().filter(|(m, _)| m.rank >= 30).cloned().collect();
        let in_game: Vec<_> = entries.iter().filter(|(m, p)| m.rank < 30 && p.in_game()).cloned().collect();
        let online: Vec<_> = entries.iter().filter(|(m, p)| m.rank < 30 && !p.in_game() && p.state != PersonaState::Offline).cloned().collect();
        let offline: Vec<_> = entries.iter().filter(|(m, p)| m.rank < 30 && p.state == PersonaState::Offline).cloned().collect();

        for (title, list) in [(t!("Staff"), &staff), (t!("In game"), &in_game), (t!("Online"), &online), (t!("Offline"), &offline)] {
            if list.is_empty() {
                continue;
            }
            w::caption(ui, t, &format!("{title} · {}", list.len()));
            for (m, p) in list {
                member_row(ui, t, state, g, Some(m), &p, perms, ui_state, actions);
            }
            ui.add_space(6.0);
        }
        if g.virtualized_members {
            ui.label(w::text(t!("Showing {n} members · search for more", n = g.members.len()), 11.5, t.text_faint));
        }
    });
}

#[allow(clippy::too_many_arguments)]
fn member_row(
    ui: &mut Ui,
    t: &Theme,
    state: &State,
    g: &ChatGroup,
    m: Option<&ChatGroupMember>,
    p: &Persona,
    perms: &model::ChatRoleActions,
    ui_state: &mut GroupUiState,
    actions: &mut Vec<Action>,
) {
    let (rect, resp) = ui.allocate_exact_size(vec2(ui.available_width(), 34.0), Sense::click());
    if resp.hovered() || resp.context_menu_opened() {
        ui.painter().rect_filled(rect, t.corner_small(), t.surface_hover);
    }
    let mut c = ui.new_child(egui::UiBuilder::new().max_rect(rect.shrink2(vec2(4.0, 3.0))).layout(Layout::left_to_right(Align::Center)));
    w::avatar(&mut c, t, &art::avatar(p.avatar_hash.as_deref()), 26.0, Some(crate::shell::status_color(t, p)));
    let name = if p.name.is_empty() { p.steam_id.to_string() } else { p.display_name() };
    let rank = m.map(|m| m.rank).unwrap_or(20);
    let badge = if rank >= 50 { Some(icons::CROWN) } else if rank >= 30 { Some(icons::SHIELD) } else { None };
    let color = if p.in_game() {
        t.in_game
    } else if p.state == PersonaState::Offline {
        t.text_dim
    } else {
        t.text
    };
    let display = match badge {
        Some(b) => format!("{b} {name}"),
        None => name.clone(),
    };
    c.add(egui::Label::new(w::text_medium(display, 13.0, color)).truncate());

    let sid = p.steam_id;
    let is_me = sid == state.steam_id;
    let can_mod = !is_me && g.my_rank > rank;
    if resp.clicked() && !is_me {
        actions.push(Action::OpenChat(sid));
    }
    resp.context_menu(|ui| {
        if !is_me && ui.button(format!("{}  {}", icons::CHAT, t!("Send a message"))).clicked() {
            actions.push(Action::OpenChat(sid));
            ui.close();
        }
        if ui.button(format!("{}  {}", icons::USER_CIRCLE, t!("View profile"))).clicked() {
            actions.push(Action::Navigate { page: "profile".into(), arg: Some(sid.to_string()) });
            ui.close();
        }
        if can_mod && perms.can_kick {
            ui.separator();
            if ui.button(w::text(t!("Kick from group…"), 13.5, t.danger)).clicked() {
                ui_state.modal = Some(GroupModal::ConfirmKick { group_id: g.id, steam_id: sid, name: name.clone() });
                ui.close();
            }
        }
        if can_mod && perms.can_ban {
            if ui.button(w::text(t!("Ban from group…"), 13.5, t.danger)).clicked() {
                ui_state.modal = Some(GroupModal::ConfirmBan { group_id: g.id, steam_id: sid, name: name.clone() });
                ui.close();
            }
        }
    });
}

fn server_message_text(state: &State, sender: SteamId, srv: &GroupServerMessage) -> String {
    let who = || {
        let p = state.persona(sender);
        if p.name.is_empty() { sender.to_string() } else { p.display_name() }
    };
    let target = || {
        if srv.account_id_param != 0 {
            let sid = account_to_steam_id(srv.account_id_param);
            let p = state.persona(sid);
            if !p.name.is_empty() {
                return p.display_name();
            }
        }
        srv.string_param.clone()
    };
    match srv.kind {
        1 => t!("{name} renamed the channel to {target}", name = who(), target = srv.string_param),
        2 | 12 => t!("{name} joined the chat", name = who()),
        3 | 13 => t!("{name} left the chat", name = who()),
        4 => t!("{name} was kicked from the chat", name = target()),
        5 | 14 => t!("{name} invited {target}", name = who(), target = target()),
        8 => t!("Invite to {name} was dismissed", name = target()),
        9 => t!("{name} updated the group tagline", name = who()),
        10 => t!("{name} updated the group avatar", name = who()),
        _ => t!("{name} updated the chat", name = who()),
    }
}

/// Extract an invite code from `https://s.team/chat/<code>`, `steamcommunity.com/chat/invite/<code>`, or a raw code.
pub fn parse_invite_code(raw: &str) -> Option<String> {
    let s = raw.trim().trim_end_matches('/');
    if s.is_empty() {
        return None;
    }
    let code = s
        .rsplit_once("/chat/invite/")
        .or_else(|| s.rsplit_once("/chat/"))
        .map(|(_, c)| c)
        .unwrap_or(s);
    let code = code.split(['?', '#']).next().unwrap_or(code).trim();
    (!code.is_empty() && code.chars().all(|c| c.is_ascii_alphanumeric() || c == '-' || c == '_')).then(|| code.to_string())
}

/// Group chat modals (Create, Join by link, Invite friends, Settings/Links/Bans, Channels, Confirmations).
pub fn modals(ctx: &egui::Context, t: &Theme, state: &State, client: &impl Sender, ui_state: &mut GroupUiState) {
    let Some(mut modal) = ui_state.modal.clone() else { return };
    let mut close = false;
    let width = (ctx.content_rect().width() - 64.0).clamp(260.0, 460.0);

    ui::widgets::modal(ctx, egui::Id::unique("group-chat-modal")).show(ctx, |ui| {
        ui.set_width(width);
        match &mut modal {
            GroupModal::CreateGroup { name, filter, selected } => {
                ui.label(w::text_bold(t!("Create group chat"), 18.0, t.text));
                ui.add_space(6.0);
                ui.label(w::text(t!("Group name (optional)"), 12.5, t.text_dim));
                ui.add(egui::TextEdit::singleline(name).desired_width(f32::INFINITY).hint_text(t!("Leave blank for an ad-hoc group"))).with_edit_menu();
                ui.add_space(8.0);
                w::search(ui, t, filter, t!("Search friends to invite"), ui.available_width());
                ui.add_space(6.0);
                let q = filter.trim().to_lowercase();
                egui::ScrollArea::vertical().max_height(220.0).show(ui, |ui| {
                    for f in state.friend_list() {
                        if !q.is_empty() && !f.display_name().to_lowercase().contains(&q) {
                            continue;
                        }
                        let mut checked = selected.contains(&f.steam_id);
                        if w::check(ui, t, &mut checked, f.display_name()).changed() {
                            if checked {
                                selected.insert(f.steam_id);
                            } else {
                                selected.remove(&f.steam_id);
                            }
                        }
                    }
                });
                ui.add_space(10.0);
                ui.horizontal(|ui| {
                    let ready = !selected.is_empty() || !name.trim().is_empty();
                    if ui.add_enabled_ui(ready, |ui| w::button(ui, t, Some(icons::PLUS), t!("Create"), w::Kind::Primary, w::Size::Sm)).inner.clicked() {
                        client.send(Command::CreateChatGroup {
                            name: (!name.trim().is_empty()).then(|| name.trim().to_string()),
                            invitees: selected.iter().copied().collect(),
                        });
                        close = true;
                    }
                    if w::button(ui, t, None, t!("Cancel"), w::Kind::Ghost, w::Size::Sm).clicked() {
                        close = true;
                    }
                });
            }
            GroupModal::JoinByLink { input } => {
                ui.label(w::text_bold(t!("Join group chat"), 18.0, t.text));
                ui.label(w::text(t!("Paste an invite link (s.team/chat/…) or invite code."), 12.5, t.text_dim));
                ui.add_space(8.0);
                let r = ui.add(egui::TextEdit::singleline(input).desired_width(f32::INFINITY).hint_text("https://s.team/chat/…")).with_edit_menu();
                w::autofocus(&r, true);
                ui.add_space(10.0);
                let code = parse_invite_code(input);
                ui.horizontal(|ui| {
                    let submit = ui.input(|i| i.key_pressed(Key::Enter));
                    if ui.add_enabled_ui(code.is_some(), |ui| w::button(ui, t, Some(icons::USER_PLUS), t!("Join"), w::Kind::Primary, w::Size::Sm)).inner.clicked() || (submit && code.is_some()) {
                        client.send(Command::JoinChatGroup { group_id: None, chat_id: None, invite_code: code });
                        close = true;
                    }
                    if w::button(ui, t, None, t!("Cancel"), w::Kind::Ghost, w::Size::Sm).clicked() {
                        close = true;
                    }
                });
            }
            GroupModal::InviteFriends { group_id, chat_id, filter } => {
                ui.label(w::text_bold(t!("Invite friends to group"), 18.0, t.text));
                ui.add_space(6.0);
                w::search(ui, t, filter, t!("Search friends"), ui.available_width());
                ui.add_space(6.0);
                let q = filter.trim().to_lowercase();
                let members: HashSet<SteamId> = state
                    .chat_groups
                    .get(group_id)
                    .map(|g| g.members.iter().map(|m| m.steam_id).collect())
                    .unwrap_or_default();
                egui::ScrollArea::vertical().max_height(240.0).show(ui, |ui| {
                    for f in state.friend_list() {
                        if members.contains(&f.steam_id) {
                            continue;
                        }
                        if !q.is_empty() && !f.display_name().to_lowercase().contains(&q) {
                            continue;
                        }
                        ui.horizontal(|ui| {
                            w::avatar(ui, t, &art::avatar(f.avatar_hash.as_deref()), 26.0, None);
                            ui.label(w::text_medium(f.display_name(), 13.5, t.text));
                            ui.with_layout(Layout::right_to_left(Align::Center), |ui| {
                                if w::button(ui, t, None, t!("Invite"), w::Kind::Secondary, w::Size::Sm).clicked() {
                                    client.send(Command::InviteToChatGroup { group_id: *group_id, chat_id: *chat_id, friend: f.steam_id });
                                }
                            });
                        });
                    }
                });
                ui.add_space(8.0);
                if w::button(ui, t, None, t!("Done"), w::Kind::Ghost, w::Size::Sm).clicked() {
                    close = true;
                }
            }
            GroupModal::GroupSettings { group_id, name, tagline, unread_muted, links_requested, bans_requested } => {
                let Some(g) = state.chat_groups.get(group_id).cloned() else {
                    close = true;
                    return;
                };
                let perms = g.effective_permissions(steam_id_to_account(state.steam_id));
                if perms.can_invite && !*links_requested {
                    *links_requested = true;
                    client.send(Command::LoadGroupInviteLinks { group_id: *group_id });
                }
                if perms.can_ban && !*bans_requested {
                    *bans_requested = true;
                    client.send(Command::LoadGroupBanList { group_id: *group_id });
                }
                ui.label(w::text_bold(t!("Group settings"), 18.0, t.text));
                ui.add_space(6.0);
                if perms.can_change_tagline_avatar_name || !g.is_saved() {
                    ui.label(w::text(t!("Group name"), 12.5, t.text_dim));
                    ui.add(egui::TextEdit::singleline(name).desired_width(f32::INFINITY)).with_edit_menu();
                    ui.add_space(6.0);
                    ui.label(w::text(t!("Tagline"), 12.5, t.text_dim));
                    ui.add(egui::TextEdit::singleline(tagline).desired_width(f32::INFINITY)).with_edit_menu();
                    ui.add_space(6.0);
                }
                w::check(ui, t, unread_muted, t!("Mute unread notifications"));
                ui.add_space(8.0);
                ui.horizontal(|ui| {
                    if w::button(ui, t, None, t!("Save"), w::Kind::Primary, w::Size::Sm).clicked() {
                        let trimmed = name.trim();
                        if !trimmed.is_empty() && trimmed != g.name {
                            if !g.is_saved() {
                                client.send(Command::SaveChatGroup { group_id: *group_id, name: trimmed.to_string() });
                            } else if perms.can_change_tagline_avatar_name {
                                client.send(Command::RenameChatGroup { group_id: *group_id, name: trimmed.to_string() });
                            }
                        }
                        if perms.can_change_tagline_avatar_name && tagline.trim() != g.tagline {
                            client.send(Command::SetChatGroupTagline { group_id: *group_id, tagline: tagline.trim().to_string() });
                        }
                        if *unread_muted != g.unread_muted {
                            client.send(Command::SetGroupNotificationPrefs {
                                group_id: *group_id,
                                unread_muted: *unread_muted,
                                desktop_level: Some(g.desktop_notification_level),
                            });
                        }
                        close = true;
                    }
                    if w::button(ui, t, None, t!("Close"), w::Kind::Ghost, w::Size::Sm).clicked() {
                        close = true;
                    }
                });
                if perms.can_invite {
                    ui.separator();
                    ui.horizontal(|ui| {
                        ui.label(w::caption_text(t, t!("Invite links")));
                        ui.with_layout(Layout::right_to_left(Align::Center), |ui| {
                            if w::button(ui, t, Some(icons::PLUS), t!("Create link"), w::Kind::Secondary, w::Size::Sm).clicked() {
                                client.send(Command::CreateGroupInviteLink { group_id: *group_id, chat_id: None, seconds_valid: 86_400 });
                            }
                        });
                    });
                    match state.group_invite_links.get(group_id) {
                        None => {
                            ui.label(w::text(t!("Loading…"), 12.0, t.text_faint));
                        }
                        Some(Err(e)) => {
                            ui.label(w::text(e, 12.0, t.text_dim));
                        }
                        Some(Ok(links)) if links.is_empty() => {
                            ui.label(w::text(t!("No active invite links."), 12.0, t.text_faint));
                        }
                        Some(Ok(links)) => {
                            for l in links.iter().take(6) {
                                let url = GroupInviteRef::link_url(&l.code);
                                ui.horizontal(|ui| {
                                    ui.add(egui::Label::new(w::text(&url, 12.5, t.text)).truncate());
                                    if w::icon_button_sized(ui, t, icons::COPY, t!("Copy link"), false, 0, 24.0).clicked() {
                                        ui.ctx().copy_text(url.clone());
                                    }
                                    if w::icon_button_sized(ui, t, icons::TRASH, t!("Delete link"), false, 0, 24.0).clicked() {
                                        client.send(Command::DeleteGroupInviteLink { group_id: *group_id, code: l.code.clone() });
                                    }
                                });
                            }
                        }
                    }
                }
                if perms.can_ban {
                    if let Some(Ok(bans)) = state.group_bans.get(group_id).filter(|r| !matches!(r, Ok(v) if v.is_empty())) {
                        ui.separator();
                        ui.label(w::caption_text(t, t!("Banned users")));
                        for b in bans.iter().take(6) {
                            let bp = state.persona(b.steam_id);
                            let label = if bp.name.is_empty() { b.steam_id.to_string() } else { bp.display_name() };
                            ui.horizontal(|ui| {
                                ui.label(w::text(label, 12.5, t.text));
                                ui.with_layout(Layout::right_to_left(Align::Center), |ui| {
                                    if w::button(ui, t, None, t!("Unban"), w::Kind::Secondary, w::Size::Sm).clicked() {
                                        client.send(Command::BanGroupMember { group_id: *group_id, steam_id: b.steam_id, ban: false });
                                    }
                                });
                            });
                        }
                    }
                }
            }
            GroupModal::CreateChannel { group_id, name, allow_voice } => {
                ui.label(w::text_bold(t!("Create channel"), 18.0, t.text));
                ui.add_space(8.0);
                let r = ui.add(egui::TextEdit::singleline(name).desired_width(f32::INFINITY).hint_text(t!("Channel name"))).with_edit_menu();
                w::autofocus(&r, true);
                ui.add_space(6.0);
                w::check(ui, t, allow_voice, t!("Voice channel"));
                ui.add_space(10.0);
                ui.horizontal(|ui| {
                    let ok = !name.trim().is_empty();
                    if ui.add_enabled_ui(ok, |ui| w::button(ui, t, None, t!("Create"), w::Kind::Primary, w::Size::Sm)).inner.clicked() {
                        client.send(Command::CreateChatChannel { group_id: *group_id, name: name.trim().to_string(), allow_voice: *allow_voice });
                        close = true;
                    }
                    if w::button(ui, t, None, t!("Cancel"), w::Kind::Ghost, w::Size::Sm).clicked() {
                        close = true;
                    }
                });
            }
            GroupModal::RenameChannel { group_id, chat_id, name } => {
                ui.label(w::text_bold(t!("Rename channel"), 18.0, t.text));
                ui.add_space(8.0);
                let r = ui.add(egui::TextEdit::singleline(name).desired_width(f32::INFINITY)).with_edit_menu();
                w::autofocus(&r, true);
                ui.add_space(10.0);
                ui.horizontal(|ui| {
                    let ok = !name.trim().is_empty();
                    if ui.add_enabled_ui(ok, |ui| w::button(ui, t, None, t!("Save"), w::Kind::Primary, w::Size::Sm)).inner.clicked() {
                        client.send(Command::RenameChatChannel { group_id: *group_id, chat_id: *chat_id, name: name.trim().to_string() });
                        close = true;
                    }
                    if w::button(ui, t, None, t!("Cancel"), w::Kind::Ghost, w::Size::Sm).clicked() {
                        close = true;
                    }
                });
            }
            GroupModal::ConfirmLeave { group_id, name } => {
                ui.add(egui::Label::new(w::text_bold(t!("Leave {name}?", name = name), 17.0, t.text)).wrap());
                ui.add_space(10.0);
                ui.horizontal(|ui| {
                    if w::button(ui, t, None, t!("Leave group"), w::Kind::Danger, w::Size::Sm).clicked() {
                        client.send(Command::LeaveChatGroup { group_id: *group_id });
                        close = true;
                    }
                    if w::button(ui, t, None, t!("Cancel"), w::Kind::Ghost, w::Size::Sm).clicked() {
                        close = true;
                    }
                });
            }
            GroupModal::ConfirmKick { group_id, steam_id, name } => {
                ui.add(egui::Label::new(w::text_bold(t!("Kick {name} from this group?", name = name), 17.0, t.text)).wrap());
                ui.add_space(10.0);
                ui.horizontal(|ui| {
                    if w::button(ui, t, None, t!("Kick"), w::Kind::Danger, w::Size::Sm).clicked() {
                        client.send(Command::KickGroupMember { group_id: *group_id, steam_id: *steam_id, expiration_secs: 300 });
                        close = true;
                    }
                    if w::button(ui, t, None, t!("Cancel"), w::Kind::Ghost, w::Size::Sm).clicked() {
                        close = true;
                    }
                });
            }
            GroupModal::ConfirmBan { group_id, steam_id, name } => {
                ui.add(egui::Label::new(w::text_bold(t!("Ban {name} from this group?", name = name), 17.0, t.text)).wrap());
                ui.add_space(10.0);
                ui.horizontal(|ui| {
                    if w::button(ui, t, None, t!("Ban"), w::Kind::Danger, w::Size::Sm).clicked() {
                        client.send(Command::BanGroupMember { group_id: *group_id, steam_id: *steam_id, ban: true });
                        close = true;
                    }
                    if w::button(ui, t, None, t!("Cancel"), w::Kind::Ghost, w::Size::Sm).clicked() {
                        close = true;
                    }
                });
            }
        }
    });
    if close {
        ui_state.modal = None;
    } else {
        ui_state.modal = Some(modal);
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    struct NoSend;
    impl Sender for NoSend {
        fn send(&self, _: Command) {}
    }

    #[test]
    fn parses_invite_links() {
        assert_eq!(parse_invite_code("https://s.team/chat/AbCd1234"), Some("AbCd1234".into()));
        assert_eq!(parse_invite_code("https://steamcommunity.com/chat/invite/XyZ_99/"), Some("XyZ_99".into()));
        assert_eq!(parse_invite_code("AbCd1234"), Some("AbCd1234".into()));
        assert_eq!(parse_invite_code("   "), None);
    }

    #[test]
    fn group_view_keeps_window_size() {
        let ctx = egui::Context::default();
        let t = Theme::default();
        let mut state = State::default();
        let me = 76561197960265729;
        let friend = 76561197960265730;
        state.steam_id = me;
        state.apply(&client::Event::ChatGroups(vec![ChatGroup {
            id: 42,
            name: "Co-op Squad".into(),
            default_chat_id: 1,
            full_loaded: true,
            rooms: vec![
                model::ChatRoomChannel { id: 1, name: "general".into(), ..Default::default() },
                model::ChatRoomChannel { id: 2, name: "Voice".into(), voice_allowed: true, ..Default::default() },
            ],
            ..Default::default()
        }]));
        for i in 0..25 {
            state.apply(&client::Event::GroupMessage(GroupMessage {
                group_id: 42,
                chat_id: 1,
                from: friend,
                text: format!("group line {i}"),
                time: 1_700_000_000 + i * 600,
                ordinal: i as u32 + 1,
                invite: None,
                group_invite: None,
                deleted: false,
                server_msg: None,
                reactions: Vec::new(),
                mentioned_me: false,
                media: Vec::new(),
            }));
        }
        let mut ui_state = GroupUiState::default();
        let mut heights = Vec::new();
        for frame in 0..30 {
            let raw = egui::RawInput { screen_rect: Some(egui::Rect::from_min_size(egui::Pos2::ZERO, vec2(1920.0, 1080.0))), time: Some(frame as f64 / 60.0), ..Default::default() };
            let mut out = ctx.run_ui(raw, |ui| {
                let r = egui::Window::new("group")
                    .title_bar(false)
                    .frame(egui::Frame::new().inner_margin(Margin::same(14)))
                    .default_pos(pos2(100.0, 100.0))
                    .default_size(vec2(520.0, 520.0))
                    .min_size(vec2(300.0, 200.0))
                    .resizable(true)
                    .show(ui.ctx(), |ui| {
                        let mut actions = Vec::new();
                        let h = ui.available_height();
                        group_view(ui, &t, &state, &NoSend, 42, &mut ui_state, &mut actions, false, h);
                    });
                heights.push(r.map(|r| r.response.rect.height()).unwrap_or(0.0));
            });
            out.textures_delta.clear();
        }
        let last = heights[heights.len() - 1];
        assert!((last - heights[5]).abs() < 0.5, "group window grew: {:?}", &heights[..]);
    }
}
