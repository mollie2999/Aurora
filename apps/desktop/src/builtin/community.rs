//! Community: friends, 1:1 chat, and group chats.

use super::group_chat::{self, ChatTarget, GroupUiState};
use ui::widgets::EditMenu;
use i18n::t;
use client::{Client, Command, Event, State};
use egui::{Align, Align2, CornerRadius, Key, Layout, Margin, Rect, Sense, Stroke, Ui, pos2, vec2};
use extensions::{Action, Ctx, Extension, Manifest, PageSpec};
use model::{GroupChatId, PersonaState, SteamId, art};
use std::collections::HashSet;
use ui::{Theme, icons, widgets as w};

#[derive(Default)]
pub struct Community {
    selected: Option<ChatTarget>,
    filter: String,
    input: String,
    loaded: HashSet<SteamId>,
    group_ui: GroupUiState,
    /// Friends page: which section, its search, the "add a friend" box.
    tab: FriendsTab,
    search: String,
    add: String,
    remove_confirm: Option<SteamId>,
    players_loaded: bool,
}

#[derive(Clone, Copy, PartialEq, Eq, Default)]
enum FriendsTab {
    #[default]
    Friends,
    Groups,
    Players,
    Add,
    Invites,
    Blocked,
}

impl Extension for Community {
    fn manifest(&self) -> Manifest {
        Manifest {
            id: "community",
            name: "Community",
            author: "Aurora",
            description: "Friends and chat",
            core: true,
            ..Manifest::DEFAULT
        }
    }

    fn pages(&self) -> Vec<PageSpec> {
        vec![PageSpec::new("community", "Friends", icons::USERS).hidden().order(35)]
    }

    fn on_navigate(&mut self, _page: &str, arg: Option<&str>) {
        if let Some(arg) = arg {
            if arg == "invites" {
                self.selected = None;
                self.tab = FriendsTab::Invites;
                return;
            }
            if arg == "players" {
                self.selected = None;
                self.tab = FriendsTab::Players;
                self.players_loaded = false;
                return;
            }
            if let Some(rest) = arg.strip_prefix("group:") {
                let (gid_s, cid_s) = rest.split_once(':').unwrap_or((rest, ""));
                if let Ok(gid) = gid_s.parse::<GroupChatId>() {
                    self.selected = Some(ChatTarget::Group(gid));
                    self.group_ui.chat_id = cid_s.parse().ok();
                    return;
                }
            }
            if let Ok(id) = arg.parse::<SteamId>() {
                self.selected = Some(ChatTarget::Friend(id));
            }
        }
    }

    fn on_event(&mut self, event: &Event, _ctx: &mut Ctx) {
        match event {
            Event::GroupJoined(gid) => {
                self.selected = Some(ChatTarget::Group(*gid));
                self.group_ui.chat_id = None;
            }
            Event::ChatGroupRemoved(gid) if self.selected == Some(ChatTarget::Group(*gid)) => {
                self.selected = None;
            }
            _ => {}
        }
    }

    fn page(&mut self, _page: &str, ui: &mut Ui, ctx: &mut Ctx) {
        let t = ctx.theme.clone();
        if let Some(ChatTarget::Friend(id)) = self.selected {
            if self.loaded.insert(id) {
                ctx.send(Command::LoadHistory { friend: id });
            }
            if ctx.state.unread.contains_key(&id) {
                ctx.act(Action::MarkRead(id));
            }
        }

        let mut actions = Vec::new();
        egui::Panel::left("community-friends")
            .exact_size(300.0)
            .resizable(false)
            .frame(egui::Frame::new().fill(t.chrome).inner_margin(Margin { left: 16, right: 10, top: 18, bottom: 10 }))
            .show(ui, |ui| {
                ui.horizontal(|ui| {
                    ui.spacing_mut().item_spacing.x = 8.0;
                    ui.label(w::text(icons::USERS, 20.0, t.text_dim));
                    ui.label(w::text_bold(t!("Friends"), 20.0, t.text));
                });
                ui.add_space(8.0);
                let width = ui.available_width() - 4.0;
                w::search(ui, &t, &mut self.filter, t!("Search friends & groups"), width);
                ui.add_space(8.0);
                let sel_group = match self.selected {
                    Some(ChatTarget::Group(gid)) => Some(gid),
                    _ => None,
                };
                let sender = client_of(ctx);
                if let Some(gid) = group_chat::group_list_section(ui, &t, ctx.state, &self.filter, sel_group, &mut self.group_ui, &sender, &mut actions) {
                    self.selected = Some(ChatTarget::Group(gid));
                    self.group_ui.chat_id = None;
                }
                ui.add_space(6.0);
                w::caption(ui, &t, &format!("{} · {}", t!("Friends"), ctx.state.friend_count()));
                let sel_friend = match self.selected {
                    Some(ChatTarget::Friend(id)) => Some(id),
                    _ => None,
                };
                if let Some(id) = friend_list(ui, &t, ctx.state, &self.filter, sel_friend) {
                    self.selected = Some(ChatTarget::Friend(id));
                }
            });

        egui::CentralPanel::default().frame(egui::Frame::new().fill(t.bg)).show(ui, |ui| match self.selected {
            Some(ChatTarget::Friend(id)) => self.conversation(ui, &t, ctx, id),
            Some(ChatTarget::Group(gid)) => self.group_conversation(ui, &t, ctx, gid, &mut actions),
            None => self.overview(ui, &t, ctx),
        });
        for a in actions {
            ctx.act(a);
        }
        group_chat::modals(ui.ctx(), &t, ctx.state, &client_of(ctx), &mut self.group_ui);
    }
}

impl Community {
    fn group_conversation(&mut self, ui: &mut Ui, t: &Theme, ctx: &mut Ctx, group_id: GroupChatId, actions: &mut Vec<Action>) {
        let Some(g) = ctx.state.chat_groups.get(&group_id).cloned() else {
            self.selected = None;
            return;
        };
        let name = ctx.state.group_display_name(&g);
        let sender = client_of(ctx);
        egui::Frame::new().fill(t.chrome).inner_margin(Margin::symmetric(24, 14)).show(ui, |ui| {
            ui.set_width(ui.available_width());
            ui.horizontal(|ui| {
                group_chat::group_avatar(ui, t, &g, &name, 44.0);
                ui.vertical(|ui| {
                    ui.spacing_mut().item_spacing.y = 1.0;
                    ui.add_space(3.0);
                    ui.label(w::text_bold(&name, 18.0, t.text));
                    let sub = if !g.tagline.is_empty() {
                        format!("{} · {}", g.tagline, t!("{n} members", n = g.active_member_count.max(g.members.len() as u32)))
                    } else {
                        t!("{n} members", n = g.active_member_count.max(g.members.len() as u32))
                    };
                    ui.label(w::text(sub, 13.0, t.text_dim));
                });
                ui.with_layout(Layout::right_to_left(Align::Center), |ui| {
                    if w::icon_button(ui, t, icons::X, t!("Close conversation"), false, 0).clicked() {
                        self.selected = None;
                    }
                    group_chat::header_actions(ui, t, ctx.state, &sender, &g, &mut self.group_ui, false);
                });
            });
        });
        let rect = ui.max_rect();
        ui.painter().hline(rect.x_range(), ui.cursor().top(), Stroke::new(1.0, t.border));
        egui::Frame::new().inner_margin(Margin::symmetric(24, 12)).show(ui, |ui| {
            let h = ui.available_height();
            group_chat::group_view(ui, t, ctx.state, &sender, group_id, &mut self.group_ui, actions, false, h);
        });
    }

    fn conversation(&mut self, ui: &mut Ui, t: &Theme, ctx: &mut Ctx, id: SteamId) {
        let p = ctx.state.persona(id);
        egui::Frame::new().fill(t.chrome).inner_margin(Margin::symmetric(24, 14)).show(ui, |ui| {
            ui.set_width(ui.available_width());
            ui.horizontal(|ui| {
                w::avatar(ui, t, &art::avatar(p.avatar_hash.as_deref()), 44.0, Some(crate::shell::status_color(t, &p)));
                ui.vertical(|ui| {
                    ui.spacing_mut().item_spacing.y = 1.0;
                    ui.add_space(3.0);
                    ui.label(w::text_bold(if p.name.is_empty() { id.to_string() } else { p.name.clone() }, 18.0, t.text));
                    let r = ui.add(egui::Label::new(w::text(crate::shell::status_text(ctx.state, &p), 13.0, crate::shell::status_color(t, &p))).sense(Sense::click()));
                    if let Some(game) = p.game_app_id.filter(|_| p.in_game()) {
                        if r.on_hover_cursor(egui::CursorIcon::PointingHand).clicked() {
                            ctx.act(crate::shell::game_page(ctx.state, game));
                        }
                    }
                });
                ui.with_layout(Layout::right_to_left(Align::Center), |ui| {
                    if w::icon_button(ui, t, icons::COPY, t!("Copy SteamID"), false, 0).clicked() {
                        ui.ctx().copy_text(id.to_string());
                        ctx.toast(t!("SteamID copied"));
                    }
                    if w::icon_button(ui, t, icons::X, t!("Close conversation"), false, 0).clicked() {
                        self.selected = None;
                    }
                });
            });
        });
        let rect = ui.max_rect();
        ui.painter().hline(rect.x_range(), ui.cursor().top(), Stroke::new(1.0, t.border));
        egui::Frame::new().inner_margin(Margin::symmetric(24, 12)).show(ui, |ui| {
            let h = ui.available_height();
            if let Some(a) = chat_body(ui, t, ctx.state, &client_of(ctx), id, &mut self.input, h) {
                ctx.act(a);
            }
        });
    }
}

/// Extensions reach the client through `Ctx::send`; chat_body takes a
/// `Client` so the overlay can share it. This adapter wraps `Ctx`.
fn client_of<'a, 'b>(ctx: &'a Ctx<'b>) -> CtxSender<'a, 'b> {
    CtxSender(ctx)
}

pub struct CtxSender<'a, 'b>(&'a Ctx<'b>);

pub trait Sender {
    fn send(&self, cmd: Command);
}
impl Sender for CtxSender<'_, '_> {
    fn send(&self, cmd: Command) {
        self.0.send(cmd);
    }
}
impl Sender for Client {
    fn send(&self, cmd: Command) {
        Client::send(self, cmd);
    }
}

/// Message list plus composer. Used by Community, the Friends window and
/// the overlay. Returns a game invite the user chose to join.
pub fn chat_body(ui: &mut Ui, t: &Theme, state: &State, client: &impl Sender, friend: SteamId, input: &mut String, height: f32) -> Option<Action> {
    let empty = Vec::new();
    let msgs = state.chats.get(&friend).unwrap_or(&empty);
    let mut join = None;
    // The messages get exactly what the composer (measured last frame)
    // leaves, so the whole thing never needs more than `height`. Inside a
    // resizable window, needing even a little more made it grow every frame.
    let composer_id = ui.scope_id().with(("chat-composer", friend));
    let composer = ui.data(|d| d.get_temp::<f32>(composer_id)).unwrap_or(46.0);
    let gap = 8.0;
    let spacing = ui.spacing().item_spacing.y;
    let list = (height - composer - gap - spacing).floor().max(40.0);
    egui::ScrollArea::vertical()
        .id_salt(("chat", friend))
        .max_height(list)
        .min_scrolled_height(list)
        .auto_shrink([false, false])
        .stick_to_bottom(true)
        .show(ui, |ui| {
            if msgs.is_empty() {
                w::empty(ui, t, icons::CHAT, t!("No messages yet"), t!("Say hi!"));
            }
            let mut last: Option<(SteamId, u64)> = None;
            // Nothing to show (e.g. markup we don't draw): skip it.
            for m in msgs.iter().filter(|m| !m.text.is_empty() || !m.media.is_empty() || m.invite.is_some() || m.group_invite.is_some()) {
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
                                let name = if p.name.is_empty() { t!("You").to_string() } else { p.name.clone() };
                                ui.label(w::text_semibold(name, 14.0, if me { t.accent } else { t.text }));
                                ui.label(w::text(format_time(m.time), 11.5, t.text_faint));
                            });
                            message(ui, t, state, client, m, &mut join);
                        });
                    });
                } else {
                    ui.horizontal_top(|ui| {
                        ui.add_space(42.0);
                        ui.vertical(|ui| message(ui, t, state, client, m, &mut join));
                    });
                }
                last = Some((m.from, m.time));
            }
            if state.is_typing(friend) {
                ui.add_space(6.0);
                ui.label(w::text(t!("{name} is typing…", name = state.persona(friend).name), 12.5, t.text_faint).italics());
                ui.ctx().request_repaint_after(std::time::Duration::from_secs(1));
            }
        });
    ui.add_space(gap);
    let r = egui::Frame::new()
        .fill(t.raised)
        .stroke(Stroke::new(1.0, t.border))
        .corner_radius(t.corner())
        .inner_margin(Margin::symmetric(12, 8))
        .show(ui, |ui| {
            ui.horizontal(|ui| {
                let before = input.len();
                let r = ui.add(
                    egui::TextEdit::singleline(input)
                        .hint_text(w::text(t!("Message {name}", name = state.persona(friend).name), 14.0, t.text_faint))
                        .font(w::font(14.5))
                        .frame(egui::Frame::NONE)
                        .desired_width(ui.available_width() - 48.0),
                ).with_edit_menu();
                if input.len() > before && before == 0 {
                    client.send(Command::Typing { to: friend });
                }
                let send = r.lost_focus() && ui.input(|i| i.key_pressed(Key::Enter));
                let clicked = w::icon_button(ui, t, icons::SEND, t!("Send"), false, 0).clicked();
                if (send || clicked) && !input.trim().is_empty() {
                    client.send(Command::SendMessage { to: friend, text: input.trim().to_string() });
                    input.clear();
                    r.request_focus();
                }
            });
        });
    let measured = r.response.rect.height();
    if (measured - composer).abs() > 0.5 {
        ui.data_mut(|d| d.insert_temp(composer_id, measured));
        ui.ctx().request_repaint();
    }
    join
}

/// One message: its text, media previews, plus any game invite or group chat invite card.
fn message(ui: &mut Ui, t: &Theme, state: &State, client: &impl Sender, m: &model::ChatMessage, join: &mut Option<Action>) {
    if !m.text.is_empty() && !(m.invite.is_some() && m.text.contains("steam://run/")) {
        let resp = super::chat_render::render_message_text(ui, t, &m.text, &m.media, t.text);
        resp.context_menu(|ui| super::chat_render::context_menu_items(ui, t, &m.text, &m.media));
    }
    super::chat_render::render_media(ui, t, &m.text, &m.media);
    if m.text.is_empty() && !m.media.is_empty() {
        let (_, filler) = ui.allocate_exact_size(vec2(1.0, 1.0), Sense::click());
        filler.context_menu(|ui| super::chat_render::context_menu_items(ui, t, &m.text, &m.media));
    }
    if let Some(inv) = &m.invite {
        game_invite_card(ui, t, state, inv, m.from == state.steam_id, join);
    }
    if let Some(ginv) = &m.group_invite {
        group_chat::invite_card(ui, t, state, client, ginv, m.from == state.steam_id, join);
    }
}

pub(crate) fn game_invite_card(ui: &mut Ui, t: &Theme, state: &State, inv: &model::GameInvite, mine: bool, join: &mut Option<Action>) {
    let name = state.app_name(inv.app_id).map(String::from).unwrap_or_else(|| t!("App {id}", id = inv.app_id).to_string());
    let width = ui.available_width().min(380.0);
    egui::Frame::new()
        .fill(t.raised)
        .stroke(Stroke::new(1.0, t.border))
        .corner_radius(t.corner())
        .inner_margin(Margin::same(8))
        .show(ui, |ui| {
            ui.set_width((width - 16.0).max(0.0));
            ui.horizontal(|ui| {
                let (r, _) = ui.allocate_exact_size(vec2(107.0, 50.0), Sense::hover());
                w::image_or_title(ui, t, r, &art::header(inv.app_id), &name, t.corner_small());
                ui.add_space(4.0);
                ui.vertical(|ui| {
                    ui.spacing_mut().item_spacing.y = 2.0;
                    let head = if mine { t!("YOU SENT A GAME INVITE") } else { t!("INVITED YOU TO PLAY") };
                    ui.label(w::text_bold(head, 10.5, t.text_faint));
                    ui.add(egui::Label::new(w::text_bold(&name, 14.0, t.text)).truncate());
                    if !mine {
                        let label = if inv.connect.is_empty() { t!("Play") } else { t!("Join game") };
                        if w::button(ui, t, Some(icons::PLAY), label, w::Kind::Primary, w::Size::Sm).clicked() {
                            *join = Some(Action::JoinGame { app_id: inv.app_id, connect: inv.connect.clone() });
                        }
                    }
                });
            });
        });
}

pub(crate) fn format_time(unix: u64) -> String {
    if unix == 0 {
        return String::new();
    }
    ui::ago(unix)
}

/// Compact friend list for the Community sidebar.
fn friend_list(ui: &mut Ui, t: &Theme, state: &State, filter: &str, selected: Option<SteamId>) -> Option<SteamId> {
    let f = filter.to_lowercase();
    let mut list = state.friend_list();
    list.retain(|p| f.is_empty() || p.name.to_lowercase().contains(&f));
    // Unread first, then in game, online, offline.
    list.sort_by_key(|p| {
        let unread = state.unread.get(&p.steam_id).copied().unwrap_or(0);
        (unread == 0, !p.in_game(), p.state == PersonaState::Offline, p.name.to_lowercase())
    });
    let mut clicked = None;
    egui::ScrollArea::vertical().id_salt("community-list").auto_shrink([false, false]).show(ui, |ui| {
        if list.is_empty() {
            ui.label(w::text(t!("No friends to show"), 13.0, t.text_faint));
        }
        for p in &list {
            let (rect, resp) = ui.allocate_exact_size(vec2(ui.available_width(), 50.0), Sense::click());
            let hover = ui.ctx().animate_bool_with_time(resp.id, resp.hovered(), 0.1);
            if selected == Some(p.steam_id) {
                ui.painter().rect_filled(rect, t.corner_small(), t.surface_hover);
            } else if hover > 0.0 {
                ui.painter().rect_filled(rect, t.corner_small(), w::mix(egui::Color32::TRANSPARENT, t.surface, hover));
            }
            let mut c = ui.new_child(egui::UiBuilder::new().max_rect(rect.shrink2(vec2(6.0, 6.0))).layout(Layout::left_to_right(Align::Center)));
            w::avatar(&mut c, t, &art::avatar(p.avatar_hash.as_deref()), 36.0, Some(crate::shell::status_color(t, p)));
            c.vertical(|ui| {
                ui.spacing_mut().item_spacing.y = 1.0;
                ui.add_space(2.0);
                let off = p.state == PersonaState::Offline;
                ui.add(egui::Label::new(w::text_medium(&p.name, 14.0, if off { t.text_faint } else { t.text })).truncate());
                let status = if state.is_typing(p.steam_id) {
                    "typing…".to_string()
                } else if let Some(m) = state.chats.get(&p.steam_id).and_then(|c| c.last()) {
                    super::chat_render::toast_preview(&m.text, &m.media)
                } else {
                    crate::shell::status_text(state, p)
                };
                ui.add(egui::Label::new(w::text(status, 12.0, t.text_dim)).truncate());
            });
            if let Some(n) = state.unread.get(&p.steam_id).filter(|n| **n > 0) {
                let cc = rect.right_center() - vec2(16.0, 0.0);
                ui.painter().circle_filled(cc, 9.0, t.accent);
                ui.painter().text(cc, egui::Align2::CENTER_CENTER, n.min(&99).to_string(), w::font_bold(10.0), t.on_accent);
            }
            if resp.on_hover_cursor(egui::CursorIcon::PointingHand).clicked() {
                clicked = Some(p.steam_id);
            }
        }
    });
    clicked
}

impl Community {
    /// The friends page, laid out like Steam's: a menu on the left and the
    /// friends in a grid, grouped by in game / online / offline.
    fn overview(&mut self, ui: &mut Ui, t: &Theme, ctx: &mut Ctx) {
        let state = ctx.state;
        let count = |rel: model::Relationship| state.friends.values().filter(|r| **r == rel).count();
        let (friends_n, invites_n) = (count(model::Relationship::Friend), count(model::Relationship::RequestRecipient));
        let blocked_n = state.friends.values().filter(|r| matches!(r, model::Relationship::Blocked | model::Relationship::Ignored)).count();
        egui::Frame::new().inner_margin(Margin::same(28)).show(ui, |ui| {
            ui.horizontal_top(|ui| {
                // Left menu.
                ui.allocate_ui_with_layout(vec2(220.0, ui.available_height()), Layout::top_down(Align::Min), |ui| {
                    ui.set_width(220.0);
                    ui.label(w::caption_text(&t, &format!("{}  {}", icons::USERS, w::sentence_case(t!("FRIENDS")))));
                    ui.add_space(4.0);
                    let players_n = {
                        let mut seen = HashSet::new();
                        for (id, _, _) in &state.session_coplay {
                            seen.insert(*id);
                        }
                        if let Some(Ok(list)) = &state.recent_players {
                            for p in list {
                                seen.insert(p.steam_id);
                            }
                        }
                        seen.len()
                    };
                    let items = [
                        (FriendsTab::Friends, icons::USER, t!("Your friends"), friends_n),
                        (FriendsTab::Groups, icons::USERS_THREE, t!("Group chats"), state.chat_groups.len()),
                        (FriendsTab::Players, icons::CLOCK, t!("Players"), players_n),
                        (FriendsTab::Add, icons::USER_PLUS, t!("Add a friend"), 0),
                        (FriendsTab::Invites, icons::BELL, t!("Invites"), invites_n),
                        (FriendsTab::Blocked, icons::LOCK_SIMPLE, t!("Blocked"), blocked_n),
                    ];
                    for (tab, icon, label, n) in items {
                        let (r, resp) = ui.allocate_exact_size(vec2(220.0, 34.0), Sense::click());
                        let on = self.tab == tab;
                        let hover = resp.hovered();
                        if on || hover {
                            ui.painter().rect_filled(r, t.corner_small(), if on { w::active_fill(t) } else { t.raised });
                        }
                        let c = if on { t.accent } else if hover { t.text } else { t.text_dim };
                        ui.painter().text(pos2(r.left() + 18.0, r.center().y), Align2::CENTER_CENTER, icon, w::font(16.0), c);
                        ui.painter().text(pos2(r.left() + 40.0, r.center().y), Align2::LEFT_CENTER, label, w::font_medium(13.5), c);
                        if n > 0 {
                            let highlight = tab == FriendsTab::Invites;
                            let txt = n.to_string();
                            let g = ui.painter().layout_no_wrap(txt, w::font_bold(11.0), if highlight { t.on_accent } else { t.text_faint });
                            let pill = Rect::from_center_size(pos2(r.right() - 10.0 - g.size().x / 2.0 - 4.0, r.center().y), g.size() + vec2(10.0, 4.0));
                            if highlight {
                                ui.painter().rect_filled(pill, CornerRadius::same(8), t.accent);
                            }
                            ui.painter().galley(pill.center() - g.size() / 2.0, g, t.text);
                        }
                        if resp.on_hover_cursor(egui::CursorIcon::PointingHand).clicked() {
                            self.tab = tab;
                        }
                    }
                    ui.add_space(14.0);
                    if w::button(ui, t, Some(icons::LIGHTNING), t!("Friends activity"), w::Kind::Secondary, w::Size::Sm).clicked() {
                        ctx.navigate(super::community_home::PAGE, None);
                    }
                });
                ui.add_space(24.0);
                // Main area.
                ui.vertical(|ui| {
                    egui::ScrollArea::vertical().id_salt("friends-page").auto_shrink([false, false]).show(ui, |ui| match self.tab {
                        FriendsTab::Friends => self.friends_grid(ui, t, ctx, friends_n),
                        FriendsTab::Groups => self.groups_grid(ui, t, ctx),
                        FriendsTab::Players => self.recent_players_view(ui, t, ctx),
                        FriendsTab::Add => self.add_friend(ui, t, ctx),
                        FriendsTab::Invites => self.invites(ui, t, ctx),
                        FriendsTab::Blocked => self.blocked(ui, t, ctx),
                    });
                });
            });
        });
        self.remove_dialog(ui, t, ctx);
    }

    fn groups_grid(&mut self, ui: &mut Ui, t: &Theme, ctx: &mut Ctx) {
        let groups = ctx.state.group_list();
        ui.horizontal(|ui| {
            ui.label(w::text_bold(format!("{}  {}", icons::USERS_THREE, w::sentence_case(t!("GROUP CHATS"))), 15.0, t.text));
            ui.label(w::text_bold(groups.len().to_string(), 13.0, t.accent));
            ui.with_layout(Layout::right_to_left(Align::Center), |ui| {
                if w::button(ui, t, Some(icons::PLUS), t!("Create group chat"), w::Kind::Primary, w::Size::Sm).clicked() {
                    self.group_ui.open_create();
                }
                if w::button(ui, t, Some(icons::LINK), t!("Join by invite link"), w::Kind::Secondary, w::Size::Sm).clicked() {
                    self.group_ui.open_join_link();
                }
            });
        });
        ui.add_space(12.0);
        if groups.is_empty() {
            w::empty(ui, t, icons::USERS_THREE, t!("No group chats yet"), t!("Start a group chat with friends or join via an invite link."));
            return;
        }
        for g in groups {
            let name = ctx.state.group_display_name(g);
            let unread = g.unread_count();
            egui::Frame::new().fill(t.raised).corner_radius(t.corner_small()).inner_margin(Margin::symmetric(14, 10)).show(ui, |ui| {
                ui.set_width(ui.available_width().min(720.0));
                ui.horizontal(|ui| {
                    group_chat::group_avatar(ui, t, g, &name, 42.0);
                    ui.add_space(4.0);
                    ui.vertical(|ui| {
                        ui.spacing_mut().item_spacing.y = 2.0;
                        ui.label(w::text_semibold(&name, 14.5, t.text));
                        let members = t!("{n} members", n = g.active_member_count.max(g.members.len() as u32));
                        let channels = t!("{n} channels", n = g.rooms.len());
                        let sub = if g.tagline.is_empty() { format!("{members} · {channels}") } else { format!("{} · {members}", g.tagline) };
                        ui.label(w::text(sub, 12.5, t.text_dim));
                    });
                    ui.with_layout(Layout::right_to_left(Align::Center), |ui| {
                        if w::button(ui, t, Some(icons::CHAT), t!("Open"), w::Kind::Primary, w::Size::Sm).clicked() {
                            self.selected = Some(ChatTarget::Group(g.id));
                            self.group_ui.chat_id = None;
                        }
                        if unread > 0 {
                            w::pill(ui, &t!("{n} unread", n = unread), t.on_accent, t.accent);
                        }
                    });
                });
            });
            ui.add_space(8.0);
        }
    }

    fn friends_grid(&mut self, ui: &mut Ui, t: &Theme, ctx: &mut Ctx, total: usize) {
        ui.horizontal(|ui| {
            ui.label(w::text_bold(format!("{}  {}", icons::USER, w::sentence_case(t!("YOUR FRIENDS"))), 15.0, t.text));
            ui.label(w::text_bold(total.to_string(), 13.0, t.accent));
        });
        ui.add_space(8.0);
        w::search(ui, t, &mut self.search, &t!("Search friends by name or game"), 420.0);
        ui.add_space(14.0);
        let q = self.search.trim().to_lowercase();
        let all: Vec<model::Persona> = ctx
            .state
            .friend_list()
            .into_iter()
            .filter(|p| q.is_empty() || p.name.to_lowercase().contains(&q) || p.game_name.as_deref().unwrap_or_default().to_lowercase().contains(&q))
            .collect();
        let in_game: Vec<_> = all.iter().filter(|p| p.in_game()).cloned().collect();
        let online: Vec<_> = all.iter().filter(|p| !p.in_game() && p.state != PersonaState::Offline).cloned().collect();
        let offline: Vec<_> = all.iter().filter(|p| !p.in_game() && p.state == PersonaState::Offline).cloned().collect();
        if all.is_empty() {
            ui.label(w::text(t!("No friends match."), 13.5, t.text_faint));
        }
        for (title, list) in [(t!("IN GAME"), in_game), (t!("ONLINE"), online), (t!("OFFLINE"), offline)] {
            if list.is_empty() {
                continue;
            }
            ui.label(w::caption_text(t, &format!("{title} · {}", list.len())));
            ui.add_space(8.0);
            // Cards fill the row edge to edge, like the library's shelves.
            let gap = 10.0;
            let avail = ui.available_width();
            let cols = (((avail + gap) / (280.0 + gap)).floor() as usize).max(1);
            let card = vec2(((avail - gap * (cols - 1) as f32) / cols as f32).floor(), 56.0);
            for row in list.chunks(cols) {
                ui.horizontal(|ui| {
                    ui.spacing_mut().item_spacing.x = gap;
                    for p in row {
                        self.friend_card(ui, t, ctx, p, card);
                    }
                });
                ui.add_space(gap);
            }
            ui.add_space(10.0);
        }
    }

    fn friend_card(&mut self, ui: &mut Ui, t: &Theme, ctx: &mut Ctx, p: &model::Persona, size: egui::Vec2) {
        let (r, resp) = ui.allocate_exact_size(size, Sense::click());
        let hover = ui.ctx().animate_bool_with_time(resp.id, resp.hovered(), 0.1);
        let color = crate::shell::status_color(t, p);
        let offline = p.state == PersonaState::Offline;
        // NEVKO card: one flat fill, the avatar inset with a status dot (no
        // colored stripe down the side).
        let fill = w::mix(t.raised, t.surface_hover, hover);
        ui.painter().rect_filled(r, t.corner_small(), fill);
        let side = size.y - 16.0;
        let av = Rect::from_min_size(r.min + vec2(8.0, 8.0), vec2(side, side));
        let ar = CornerRadius::same(6);
        if !w::paint_cover(ui, av, &art::avatar(p.avatar_hash.as_deref()), ar) {
            w::skeleton(ui, t, av, ar);
        }
        if offline {
            ui.painter().rect_filled(av, ar, egui::Color32::from_black_alpha(110));
        }
        let dot = av.right_bottom() - vec2(1.0, 1.0);
        ui.painter().circle_filled(dot, 6.0, fill);
        ui.painter().circle_filled(dot, 4.0, color);
        let x = av.right() + 12.0;
        let max_w = r.right() - x - 8.0;
        let fit = |text: String, font: egui::FontId, c: egui::Color32| {
            let mut job = egui::text::LayoutJob::simple_singleline(text, font, c);
            job.wrap = egui::text::TextWrapping::truncate_at_width(max_w);
            ui.painter().layout_job(job)
        };
        let name_color = if p.in_game() { t.in_game } else if offline { t.text_dim } else { t.accent };
        let name = fit(p.name.clone(), w::font_semibold(14.0), name_color);
        let status = fit(crate::shell::status_text(ctx.state, p), w::font(12.0), if offline { t.text_faint } else { color });
        ui.painter().galley(pos2(x, r.top() + 10.0), name, name_color);
        ui.painter().galley(pos2(x, r.top() + 30.0), status, color);
        let resp = resp.on_hover_cursor(egui::CursorIcon::PointingHand);
        if resp.clicked() {
            ctx.act(Action::OpenChat(p.steam_id));
        }
        resp.context_menu(|ui| {
            if ui.button(format!("{}   {}", icons::CHAT, t!("Send a message"))).clicked() {
                ctx.act(Action::OpenChat(p.steam_id));
                ui.close();
            }
            if ui.button(format!("{}   {}", icons::USER_CIRCLE, t!("View profile"))).clicked() {
                ctx.navigate("profile", Some(p.steam_id.to_string()));
                ui.close();
            }
            if ui.button(format!("{}   {}", icons::BACKPACK, t!("Inventory"))).clicked() {
                ctx.navigate("inventory", Some(p.steam_id.to_string()));
                ui.close();
            }
            ui.separator();
            if ui.button(format!("{}   {}", icons::X, t!("Remove friend…"))).clicked() {
                self.remove_confirm = Some(p.steam_id);
                ui.close();
            }
        });
    }

    fn remove_dialog(&mut self, ui: &mut Ui, t: &Theme, ctx: &mut Ctx) {
        let Some(id) = self.remove_confirm else { return };
        let name = ctx.state.persona(id).name;
        let mut done = false;
        ui::widgets::modal(ui.ctx(), egui::Id::unique(("remove-friend", id))).show(ui.ctx(), |ui| {
            ui.set_width(360.0);
            ui.label(w::text_bold(t!("Remove {name} from friends?", name = name), 17.0, t.text));
            ui.add_space(10.0);
            ui.horizontal(|ui| {
                if w::button(ui, t, Some(icons::X), t!("Remove friend"), w::Kind::Danger, w::Size::Md).clicked() {
                    ctx.send(client::Command::RemoveFriend { steam_id: id });
                    done = true;
                }
                if w::button(ui, t, None, t!("Cancel"), w::Kind::Ghost, w::Size::Md).clicked() {
                    done = true;
                }
            });
        });
        if done {
            self.remove_confirm = None;
        }
    }

    fn add_friend(&mut self, ui: &mut Ui, t: &Theme, ctx: &mut Ctx) {
        ui.label(w::text_bold(t!("Add a friend"), 20.0, t.text));
        ui.add_space(10.0);
        let code = (ctx.state.steam_id & 0xFFFF_FFFF).to_string();
        w::panel(t).show(ui, |ui| {
            ui.set_width(ui.available_width().min(620.0));
            ui.label(w::caption_text(&t, t!("YOUR FRIEND CODE")));
            ui.horizontal(|ui| {
                ui.label(w::text_bold(&code, 24.0, t.text));
                if w::button(ui, t, Some(icons::COPY), t!("Copy"), w::Kind::Secondary, w::Size::Sm).clicked() {
                    ui.ctx().copy_text(code.clone());
                    ctx.toast(t!("Friend code copied"));
                }
            });
            ui.label(w::text(t!("Share it with a friend so they can add you."), 12.5, t.text_dim));
        });
        ui.add_space(12.0);
        w::panel(t).show(ui, |ui| {
            ui.set_width(ui.available_width().min(620.0));
            ui.label(w::caption_text(&t, t!("ADD BY FRIEND CODE OR PROFILE LINK")));
            ui.add_space(4.0);
            let target = parse_friend(&self.add);
            ui.horizontal(|ui| {
                ui.add(egui::TextEdit::singleline(&mut self.add).hint_text(t!("e.g. 836957442 or steamcommunity.com/profiles/…")).desired_width(380.0).margin(vec2(10.0, 8.0))).with_edit_menu();
                ui.add_enabled_ui(target.is_some(), |ui| {
                    if w::button(ui, t, Some(icons::USER_PLUS), t!("Send invite"), w::Kind::Primary, w::Size::Md).clicked() {
                        if let Some(id) = target {
                            ctx.send(client::Command::AddFriend { steam_id: id });
                            ctx.toast(t!("Friend invite sent"));
                            self.add.clear();
                        }
                    }
                });
            });
            if !self.add.trim().is_empty() && target.is_none() {
                ui.label(w::text(t!("Enter a friend code (numbers) or a link like steamcommunity.com/profiles/7656…"), 12.0, t.away));
            }
        });
    }

    fn invites(&mut self, ui: &mut Ui, t: &Theme, ctx: &mut Ctx) {
        let pick = |rel: model::Relationship| -> Vec<model::Persona> { ctx.state.friends.iter().filter(|(_, r)| **r == rel).map(|(id, _)| ctx.state.persona(*id)).collect() };
        let received = pick(model::Relationship::RequestRecipient);
        let sent = pick(model::Relationship::RequestInitiator);
        ui.label(w::text_bold(t!("Invites"), 20.0, t.text));
        ui.add_space(10.0);
        if received.is_empty() && sent.is_empty() {
            ui.label(w::text(t!("No pending invites."), 13.5, t.text_faint));
        }
        for (title, list, incoming) in [(t!("RECEIVED"), received, true), (t!("SENT"), sent, false)] {
            if list.is_empty() {
                continue;
            }
            ui.label(w::text_bold(format!("{title}  ·  {}", list.len()), 11.5, t.text_faint));
            ui.add_space(6.0);
            for p in &list {
                egui::Frame::new().fill(t.surface).corner_radius(CornerRadius::same(4)).inner_margin(Margin::symmetric(10, 8)).show(ui, |ui| {
                    ui.set_width(ui.available_width().min(720.0));
                    ui.horizontal(|ui| {
                        w::avatar(ui, t, &art::avatar(p.avatar_hash.as_deref()), 36.0, None);
                        let name = if p.name.is_empty() { p.steam_id.to_string() } else { p.name.clone() };
                        ui.label(w::text_semibold(name, 14.0, t.text));
                        ui.with_layout(Layout::right_to_left(Align::Center), |ui| {
                            if incoming {
                                if w::button(ui, t, Some(icons::X), t!("Ignore"), w::Kind::Ghost, w::Size::Sm).clicked() {
                                    ctx.send(client::Command::RemoveFriend { steam_id: p.steam_id });
                                }
                                if w::button(ui, t, Some(icons::CHECK), t!("Accept"), w::Kind::Primary, w::Size::Sm).clicked() {
                                    ctx.send(client::Command::AddFriend { steam_id: p.steam_id });
                                }
                            } else if w::button(ui, t, Some(icons::X), t!("Cancel invite"), w::Kind::Ghost, w::Size::Sm).clicked() {
                                ctx.send(client::Command::RemoveFriend { steam_id: p.steam_id });
                            }
                            if w::button(ui, t, Some(icons::USER_CIRCLE), t!("Profile"), w::Kind::Secondary, w::Size::Sm).clicked() {
                                ctx.navigate("profile", Some(p.steam_id.to_string()));
                            }
                        });
                    });
                });
                ui.add_space(6.0);
            }
            ui.add_space(10.0);
        }
    }

    fn blocked(&mut self, ui: &mut Ui, t: &Theme, ctx: &mut Ctx) {
        ui.label(w::text_bold(t!("Blocked"), 20.0, t.text));
        ui.label(w::text(t!("They can't message you or send invites."), 13.0, t.text_dim));
        ui.add_space(10.0);
        let list: Vec<model::Persona> = ctx.state.friends.iter().filter(|(_, r)| matches!(r, model::Relationship::Blocked | model::Relationship::Ignored)).map(|(id, _)| ctx.state.persona(*id)).collect();
        if list.is_empty() {
            ui.label(w::text(t!("You haven't blocked anyone."), 13.5, t.text_faint));
        }
        for p in &list {
            ui.horizontal(|ui| {
                w::avatar(ui, t, &art::avatar(p.avatar_hash.as_deref()), 32.0, None);
                let name = if p.name.is_empty() { p.steam_id.to_string() } else { p.name.clone() };
                ui.label(w::text_semibold(name, 13.5, t.text));
                if ui.link(w::text(t!("Profile"), 12.5, t.accent)).clicked() {
                    ctx.navigate("profile", Some(p.steam_id.to_string()));
                }
            });
            ui.add_space(4.0);
        }
    }

    fn recent_players_view(&mut self, ui: &mut Ui, t: &Theme, ctx: &mut Ctx) {
        if !self.players_loaded && ctx.state.online() {
            self.players_loaded = true;
            ctx.send(Command::LoadRecentPlayers);
        }
        ui.horizontal(|ui| {
            ui.label(w::text_bold(t!("Players"), 20.0, t.text));
            ui.with_layout(Layout::right_to_left(Align::Center), |ui| {
                if w::button(ui, t, Some(icons::REFRESH), t!("Refresh"), w::Kind::Secondary, w::Size::Sm).clicked() && ctx.state.online() {
                    ctx.send(Command::LoadRecentPlayers);
                }
            });
        });
        ui.label(w::text(t!("People you've recently played multiplayer games with."), 13.0, t.text_dim));
        ui.add_space(10.0);

        let mut rows: Vec<model::RecentPlayer> = Vec::new();
        let mut seen = HashSet::new();
        for (steam_id, app_id, ts) in &ctx.state.session_coplay {
            if !seen.insert(*steam_id) {
                continue;
            }
            let p = ctx.state.persona(*steam_id);
            let game_name = ctx.state.app_name(*app_id).map(String::from).unwrap_or_else(|| format!("App {app_id}"));
            rows.push(model::RecentPlayer {
                steam_id: *steam_id,
                name: if p.name.is_empty() { steam_id.to_string() } else { p.name },
                avatar: art::avatar(p.avatar_hash.as_deref()),
                app_id: Some(*app_id),
                game_name,
                played_at: if *ts > 0 { ui::date(*ts) } else { t!("Current session").to_string() },
            });
        }
        if let Some(Ok(list)) = &ctx.state.recent_players {
            for r in list {
                if seen.insert(r.steam_id) {
                    rows.push(r.clone());
                }
            }
        }

        if let Some(Err(e)) = &ctx.state.recent_players
            && rows.is_empty()
        {
            ui.label(w::text(i18n::tr_dyn(e), 13.5, t.danger));
            return;
        }
        if rows.is_empty() {
            let msg = if ctx.state.recent_players.is_none() && ctx.state.online() {
                t!("Loading recent players…")
            } else {
                t!("No recent co-op or multiplayer players recorded yet.")
            };
            ui.label(w::text(msg, 13.5, t.text_faint));
            return;
        }

        for r in &rows {
            let is_friend = ctx.state.friends.get(&r.steam_id).is_some_and(|rel| *rel == model::Relationship::Friend);
            let p = ctx.state.persona(r.steam_id);
            let avatar = p.avatar_hash.as_deref().map(|h| art::avatar(Some(h))).unwrap_or_else(|| r.avatar.clone());
            let display = if !p.name.is_empty() { &p.name } else if !r.name.is_empty() { &r.name } else { &r.steam_id.to_string() };
            egui::Frame::new()
                .fill(t.raised)
                .corner_radius(t.corner_small())
                .inner_margin(Margin::symmetric(12, 8))
                .show(ui, |ui| {
                    ui.set_width(ui.available_width());
                    ui.horizontal(|ui| {
                        w::avatar(ui, t, &avatar, 36.0, None);
                        ui.add_space(6.0);
                        ui.vertical(|ui| {
                            ui.label(w::text_semibold(display, 13.5, t.text));
                            let mut sub = Vec::new();
                            if !r.game_name.is_empty() {
                                sub.push(r.game_name.clone());
                            }
                            if !r.played_at.is_empty() {
                                sub.push(r.played_at.clone());
                            }
                            if !sub.is_empty() {
                                ui.label(w::text(sub.join(" · "), 12.0, t.text_dim));
                            }
                        });
                        ui.with_layout(Layout::right_to_left(Align::Center), |ui| {
                            if w::button(ui, t, Some(icons::USER_CIRCLE), t!("Profile"), w::Kind::Ghost, w::Size::Sm).clicked() {
                                ctx.navigate("profile", Some(r.steam_id.to_string()));
                            }
                            if is_friend {
                                if w::button(ui, t, Some(icons::CHAT), t!("Message"), w::Kind::Secondary, w::Size::Sm).clicked() {
                                    ctx.act(Action::OpenChat(r.steam_id));
                                }
                            } else if w::button(ui, t, Some(icons::USER_PLUS), t!("Add friend"), w::Kind::Secondary, w::Size::Sm).clicked() {
                                ctx.send(Command::AddFriend { steam_id: r.steam_id });
                            }
                        });
                    });
                });
            ui.add_space(6.0);
        }
    }
}

/// A friend code (account id), SteamID64, or profile link -> SteamID64.
fn parse_friend(input: &str) -> Option<SteamId> {
    let s = input.trim().trim_end_matches('/');
    let s = s.rsplit_once("/profiles/").map(|x| x.1).unwrap_or(s);
    let n: u64 = s.trim_end_matches('/').parse().ok()?;
    match n {
        1..=0xFFFF_FFFF => Some(n + 76561197960265728),
        76561197960265729..=76561202255233023 => Some(n),
        _ => None,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    struct NoSend;
    impl Sender for NoSend {
        fn send(&self, _: Command) {}
    }

    /// The chat inside a resizable window (like the overlay's) must keep the
    /// window's size; it used to grow a little every frame.
    #[test]
    fn chat_keeps_window_size() {
        let ctx = egui::Context::default();
        let t = Theme::default();
        let mut state = State::default();
        let friend: SteamId = 76561197960287930;
        state.chats.insert(friend, (0..30).map(|i| model::ChatMessage { friend, from: friend, text: format!("message {i}"), time: 1_700_000_000 + i * 600, invite: (i % 7 == 0).then(|| model::GameInvite { app_id: 477160, connect: "+connect_lobby 1".into() }), group_invite: None, media: Vec::new() }).collect());
        let mut input = String::new();
        let mut heights = Vec::new();
        for frame in 0..40 {
            let raw = egui::RawInput { screen_rect: Some(egui::Rect::from_min_size(egui::Pos2::ZERO, vec2(1920.0, 1080.0))), time: Some(frame as f64 / 60.0), ..Default::default() };
            let mut out = ctx.run_ui(raw, |ui| {
                let r = egui::Window::new("chat")
                    .title_bar(false)
                    .frame(egui::Frame::new().inner_margin(Margin::same(14)))
                    .default_pos(pos2(100.0, 100.0))
                    .default_size(vec2(440.0, 520.0))
                    .min_size(vec2(260.0, 160.0))
                    .resizable(true)
                    .show(ui.ctx(), |ui| {
                        ui.horizontal(|ui| ui.label("title"));
                        ui.separator();
                        ui.horizontal(|ui| ui.label("status"));
                        ui.add_space(6.0);
                        let h = ui.available_height();
                        let _ = chat_body(ui, &t, &state, &NoSend, friend, &mut input, h);
                    });
                heights.push(r.map(|r| r.response.rect.height()).unwrap_or(0.0));
            });
            out.textures_delta.clear();
        }
        let last = heights[heights.len() - 1];
        assert!((last - heights[5]).abs() < 0.5, "window grew: {:?}", &heights[..]);
    }
}
