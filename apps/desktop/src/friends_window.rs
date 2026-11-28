//! The undocked "Friends & Chat" window, like Steam's. It keeps working while
//! the main window is hidden in the tray.

use crate::app::SteamApp;
use crate::builtin::group_chat::{self, ChatTarget, GroupUiState};
use egui::{Align, Layout, Margin, Ui};
use extensions::Action;
use i18n::t;
use model::{GroupChatId, PersonaState, SteamId, art};
use ui::{icons, widgets as w};

#[derive(Default)]
pub struct FriendsWindow {
    pub open: bool,
    pub chat: Option<ChatTarget>,
    pub group_ui: GroupUiState,
    input: String,
    pending: Vec<Action>,
}

struct AppPtr(*mut SteamApp);
// SAFETY: deferred viewport callbacks run on the UI thread in their own frame,
// never concurrently with `App::ui`, and `SteamApp` outlives all viewports.
unsafe impl Send for AppPtr {}
unsafe impl Sync for AppPtr {}
impl AppPtr {
    fn get(&self) -> *mut SteamApp {
        self.0
    }
}

pub fn show(app: &mut SteamApp, ctx: &egui::Context, actions: &mut Vec<Action>) {
    actions.append(&mut app.friends_window.pending);
    if !app.friends_window.open {
        return;
    }
    let width = match app.friends_window.chat {
        None => 340.0,
        Some(ChatTarget::Friend(_)) => 820.0,
        Some(ChatTarget::Group(_)) => if app.friends_window.group_ui.show_members { 960.0 } else { 820.0 },
    };
    // Our own title bar, like the main window (no Windows frame).
    let builder = egui::ViewportBuilder::default()
        .with_title(t!("Friends & Chat"))
        .with_inner_size([width, 720.0])
        .with_min_inner_size([300.0, 420.0])
        .with_decorations(false)
        .with_icon(std::sync::Arc::new(ui::brand::window_icon(64)));
    let viewport = egui::ViewportId::from_hash_of("friends-window");
    if !ctx.embed_viewports() {
        let ptr = AppPtr(app);
        ctx.show_viewport_deferred(viewport, builder, move |ui, _| {
            let app = unsafe { &mut *ptr.get() };
            let ctx = ui.ctx().clone();
            let mut local_actions = std::mem::take(&mut app.friends_window.pending);
            app.drain_events(&ctx, &mut local_actions);
            viewport_ui(app, ui, &mut local_actions);
            // Apply chat read marks directly so they don't wait for the main window.
            let mut deferred_for_root = Vec::new();
            for act in local_actions {
                match act {
                    Action::MarkRead(id) => {
                        app.state.unread.remove(&id);
                    }
                    Action::MarkGroupRead { group_id, chat_id } => {
                        if let Some(ts) = app.state.mark_group_read(group_id, chat_id) {
                            app.client.send(client::Command::AckGroupMessage { group_id, chat_id, timestamp: ts });
                        }
                    }
                    other => deferred_for_root.push(other),
                }
            }
            app.friends_window.pending = deferred_for_root;
            if !app.friends_window.pending.is_empty() || !app.friends_window.open {
                ctx.request_repaint_of(egui::ViewportId::ROOT);
            }
        });
    } else {
        ctx.show_viewport_immediate(viewport, builder, |ui, _| {
            viewport_ui(app, ui, actions);
        });
    }
}

fn viewport_ui(app: &mut SteamApp, ui: &mut Ui, actions: &mut Vec<Action>) {
    if ui.input(|i| i.viewport().close_requested()) {
        app.friends_window.open = false;
    }
    let t = app.prefs.theme.clone();
    // No title bar row, like Steam's friends window: the window buttons
    // sit in the header (top right), and the header drags the window.
    if let Some(target) = app.friends_window.chat {
        egui::Panel::left("fw-list")
            .exact_size(340.0)
            .resizable(false)
            .frame(egui::Frame::new().fill(t.chrome).inner_margin(Margin { left: 14, right: 10, top: 12, bottom: 8 }))
            .show(ui, |ui| list(app, ui, actions));
        egui::CentralPanel::default()
            .frame(egui::Frame::new().fill(t.bg).inner_margin(Margin { left: 16, right: 0, top: 0, bottom: 16 }))
            .show(ui, |ui| {
                // Window buttons on the top edge of the chat side.
                let top = egui::Rect::from_min_size(ui.max_rect().min, egui::vec2(ui.max_rect().width(), 30.0));
                crate::chrome::drag_area(ui, top, true);
                ui.allocate_ui_with_layout(top.size(), Layout::right_to_left(Align::Min), |ui| crate::chrome::window_buttons(ui, &t, 30.0, true));
                ui.add_space(4.0);
                egui::Frame::new().inner_margin(Margin { left: 0, right: 16, top: 0, bottom: 0 }).show(ui, |ui| match target {
                    ChatTarget::Friend(id) => chat(app, ui, actions, id),
                    ChatTarget::Group(gid) => group_panel(app, ui, actions, gid),
                });
            });
    } else {
        egui::CentralPanel::default()
            .frame(egui::Frame::new().fill(t.chrome).inner_margin(Margin { left: 14, right: 10, top: 12, bottom: 8 }))
            .show(ui, |ui| list(app, ui, actions));
    }
    crate::chrome::resize_edges(ui);
    let ctx = ui.ctx().clone();
    crate::chrome::border(&ctx, &t);
    crate::shell::friend_dialogs(app, &ctx);
    group_chat::modals(&ctx, &t, &app.state, &app.client, &mut app.friends_window.group_ui);
    crate::builtin::chat_render::show_lightbox(&ctx, &t);
}

fn list(app: &mut SteamApp, ui: &mut Ui, actions: &mut Vec<Action>) {
    let t = app.prefs.theme.clone();
    let me = app.state.me().cloned().unwrap_or_default();
    // The header drags the window (widgets on top of it still get their clicks).
    let header = egui::Rect::from_min_size(ui.max_rect().min - egui::vec2(14.0, 12.0), egui::vec2(ui.max_rect().width() + 24.0, 64.0));
    crate::chrome::drag_area(ui, header, true);
    // With no chat open, the window buttons live in this header's top right.
    if app.friends_window.chat.is_none() {
        let corner = egui::Rect::from_min_max(egui::pos2(header.right() - 3.0 * 46.0, header.top()), egui::pos2(header.right(), header.top() + 30.0));
        let mut c = ui.new_child(egui::UiBuilder::new().max_rect(corner).layout(Layout::right_to_left(Align::Min)));
        crate::chrome::window_buttons(&mut c, &t, 30.0, true);
    }
    ui.horizontal(|ui| {
        w::avatar(ui, &t, &art::avatar(me.avatar_hash.as_deref()), 40.0, Some(crate::shell::status_color(&t, &me)));
        ui.vertical(|ui| {
            ui.spacing_mut().item_spacing.y = 0.0;
            ui.add_space(2.0);
            ui.label(w::text_semibold(&app.state.persona_name, 15.0, t.text));
            egui::ComboBox::from_id_salt("fw-status")
                .selected_text(w::text(i18n::tr_dyn(me.state.label()), 12.5, crate::shell::status_color(&t, &me)))
                .width(140.0)
                .show_ui(ui, |ui| {
                    for s in PersonaState::SELECTABLE {
                        if ui.selectable_label(me.state == s, i18n::tr_dyn(s.label())).clicked() {
                            app.client.send(client::Command::SetPersonaState(s));
                        }
                    }
                });
        });
    });
    ui.add_space(10.0);
    let width = ui.available_width() - 4.0;
    w::search(ui, &t, &mut app.friend_filter, t!("Search friends & groups"), width);
    ui.add_space(8.0);
    let sel_group = match app.friends_window.chat {
        Some(ChatTarget::Group(gid)) => Some(gid),
        _ => None,
    };
    if let Some(gid) = group_chat::group_list_section(ui, &t, &app.state, &app.friend_filter, sel_group, &mut app.friends_window.group_ui, &app.client, actions) {
        app.friends_window.chat = Some(ChatTarget::Group(gid));
        app.friends_window.group_ui.chat_id = None;
    }
    ui.add_space(6.0);
    let friends = app.state.friend_list();
    let sel_friend = match app.friends_window.chat {
        Some(ChatTarget::Friend(id)) => id,
        _ => 0,
    };
    // The shared list pushes OpenChat when nothing is selected; capture clicks here instead.
    let mut local_actions = Vec::new();
    if let Some(id) = crate::shell::friend_list(app, ui, &t, &mut local_actions, &friends, Some(sel_friend)) {
        app.friends_window.chat = Some(ChatTarget::Friend(id));
        app.client.send(client::Command::LoadHistory { friend: id });
        actions.push(Action::MarkRead(id));
    }
    actions.extend(local_actions.into_iter().filter(|a| !matches!(a, Action::OpenChat(_))));
}

fn chat(app: &mut SteamApp, ui: &mut Ui, actions: &mut Vec<Action>, id: SteamId) {
    let t = app.prefs.theme.clone();
    let p = app.state.persona(id);
    if app.state.unread.contains_key(&id) {
        actions.push(Action::MarkRead(id));
    }
    ui.horizontal(|ui| {
        // The close button is placed first (right side), so long game
        // statuses get cut off instead of covering it.
        ui.with_layout(Layout::right_to_left(Align::Center), |ui| {
            if w::icon_button(ui, &t, icons::X, t!("Close"), false, 0).clicked() {
                app.friends_window.chat = None;
            }
            ui.with_layout(Layout::left_to_right(Align::Center), |ui| {
                w::avatar(ui, &t, &art::avatar(p.avatar_hash.as_deref()), 36.0, Some(crate::shell::status_color(&t, &p)));
                ui.vertical(|ui| {
                    ui.spacing_mut().item_spacing.y = 0.0;
                    ui.add(egui::Label::new(w::text_semibold(p.display_name(), 15.0, t.text)).truncate());
                    let status = crate::shell::status_text(&app.state, &p);
                    ui.add(egui::Label::new(w::text(&status, 12.5, crate::shell::status_color(&t, &p))).truncate()).on_hover_text(&status);
                });
            });
        });
    });
    ui.separator();
    let h = ui.available_height();
    actions.extend(crate::builtin::community::chat_body(ui, &t, &app.state, &app.client, id, &mut app.friends_window.input, h, false));
}

fn group_panel(app: &mut SteamApp, ui: &mut Ui, actions: &mut Vec<Action>, group_id: GroupChatId) {
    let t = app.prefs.theme.clone();
    let Some(g) = app.state.chat_groups.get(&group_id).cloned() else {
        app.friends_window.chat = None;
        return;
    };
    let name = app.state.group_display_name(&g);
    ui.horizontal(|ui| {
        ui.with_layout(Layout::right_to_left(Align::Center), |ui| {
            if w::icon_button(ui, &t, icons::X, t!("Close"), false, 0).clicked() {
                app.friends_window.chat = None;
            }
            group_chat::header_actions(ui, &t, &app.state, &app.client, &g, &mut app.friends_window.group_ui, false);
            ui.with_layout(Layout::left_to_right(Align::Center), |ui| {
                group_chat::group_avatar(ui, &t, &g, &name, 36.0);
                ui.vertical(|ui| {
                    ui.spacing_mut().item_spacing.y = 0.0;
                    ui.add(egui::Label::new(w::text_semibold(&name, 15.0, t.text)).truncate());
                    let sub = t!("{n} members", n = g.active_member_count.max(g.members.len() as u32));
                    ui.add(egui::Label::new(w::text(sub, 12.5, t.text_dim)).truncate());
                });
            });
        });
    });
    ui.separator();
    let h = ui.available_height();
    group_chat::group_view(ui, &t, &app.state, &app.client, group_id, &mut app.friends_window.group_ui, actions, false, h);
}
