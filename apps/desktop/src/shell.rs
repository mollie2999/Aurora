//! Main window chrome: top navigation, friends drawer, status bar, toasts.

use ui::widgets::EditMenu;
use i18n::t;
use crate::app::SteamApp;
use client::Command;
use egui::{Align, Align2, Layout, Margin, Rect, Sense, Stroke, Ui, vec2};
use extensions::{Action, Env, Placement};
use model::{Persona, PersonaState, art};
use ui::{Theme, icons, widgets as w};

/// Header height: a 56 px bar holding 36 px controls.
const TOP_BAR: f32 = 56.0;
const CTRL: f32 = w::CONTROL_LG;

#[allow(dead_code)]
pub fn show(app: &mut SteamApp, ui: &mut Ui, actions: &mut Vec<Action>) {
    show_with_theme(app, ui, actions, None, None, None, None, None);
}

#[allow(clippy::too_many_arguments)]
pub fn show_with_theme(
    app: &mut SteamApp,
    ui: &mut Ui,
    actions: &mut Vec<Action>,
    resolved: Option<&themes::ResolvedTheme>,
    bg_tex: Option<&egui::TextureHandle>,
    logo_tex: Option<&egui::TextureHandle>,
    mut paint_bg: Option<&mut dyn FnMut(&egui::Painter, Rect)>,
    mut paint_chrome: Option<&mut dyn FnMut(&egui::Painter, Rect)>,
) {
    if let Some(res) = resolved {
        if !res.layout.shell.is_empty() {
            crate::theme_renderer::render_shell(app, ui, actions, res, bg_tex, logo_tex, paint_bg, paint_chrome);
            return;
        }
    }

    let t = app.prefs.theme.clone();
    let wr = crate::chrome::window_rounding(ui.ctx());
    let top_round = egui::CornerRadius { nw: wr.nw, ne: wr.ne, sw: 0, se: 0 };
    let bot_round = egui::CornerRadius { nw: 0, ne: 0, sw: wr.sw, se: wr.se };

    egui::Panel::top("topbar")
        .exact_size(TOP_BAR)
        .resizable(false)
        .frame(egui::Frame::new().fill(t.chrome).corner_radius(top_round).inner_margin(Margin { left: 10, right: 0, top: 0, bottom: 0 }))
        .show(ui, |ui| {
            if let Some(f) = paint_chrome.as_deref_mut() {
                f(ui.painter(), ui.max_rect());
            }
            top_bar(app, ui, &t, actions);
        });

    egui::Panel::bottom("statusbar")
        .exact_size(34.0)
        .resizable(false)
        .frame(egui::Frame::new().fill(t.chrome).corner_radius(bot_round).inner_margin(Margin::symmetric(16, 0)))
        .show(ui, |ui| status_bar(app, ui, &t));

    if app.prefs.friends_open && app.prefs.page != "community" {
        egui::Panel::right("friends")
            .exact_size(290.0)
            .resizable(false)
            .frame(egui::Frame::new().fill(t.chrome).inner_margin(Margin { left: 14, right: 10, top: 16, bottom: 8 }))
            .show(ui, |ui| friends_drawer(app, ui, &t, actions));
    }

    egui::CentralPanel::default().frame(egui::Frame::new().fill(t.bg)).show(ui, |ui| {
        let panel_rect = ui.max_rect();
        if let (Some(tex), Some(res)) = (bg_tex, resolved) {
            crate::theme_renderer::paint_background_texture(ui, panel_rect, tex, res.background_fit, res.background_opacity, res.background_tint);
        }
        if let Some(f) = paint_bg.as_deref_mut() {
            f(ui.painter(), panel_rect);
        }
        show_active_page(app, ui, &t, actions);
    });
}

pub(crate) fn show_active_page(app: &mut SteamApp, ui: &mut Ui, t: &Theme, actions: &mut Vec<Action>) {
    let page = app.prefs.page.clone();
    // Offline: pages that need Steam say so instead.
    if crate::offline_ui::page(app, ui, &page) {
        return;
    }
    if page == "downloads" {
        crate::offline_ui::downloads_notice(app, ui, t);
    }
    let env = Env {
        state: &app.state,
        local: &app.local,
        theme: &app.prefs.theme,
        zoom: app.prefs.zoom,
        client: &app.client,
        actions: &mut *actions,
    };
    if !app.registry.show_page(&page, ui, env) {
        w::empty(ui, t, icons::WARNING, "Page not found", &page);
    }
}

fn top_bar(app: &mut SteamApp, ui: &mut Ui, t: &Theme, actions: &mut Vec<Action>) {
    let rect = ui.max_rect();
    // Empty parts of the bar move the window (we draw our own title bar).
    crate::chrome::drag_area(ui, rect.expand2(vec2(10.0, 0.0)), true);
    ui.horizontal_centered(|ui| {
        ui.spacing_mut().item_spacing.x = 6.0;
        ui.add_space(4.0);
        app_menu(app, ui, t, actions);
        ui.add_space(6.0);

        let pages = app.registry.pages();
        let current = app.prefs.page.clone();
        for p in pages.iter().filter(|p| p.placement == Placement::Main) {
            let icon = (!p.icon.is_empty()).then_some(p.icon.as_str());
            let tab = w::nav_tab_icon(ui, t, icon, &i18n::tr_dyn(&p.title), current == p.id, TOP_BAR);
            if tab.clicked() {
                actions.push(Action::Navigate { page: p.id.clone(), arg: None });
            }
            let items = tab_menu_items(app, &p.id);
            if let Some((page, arg)) = hover_menu(ui, t, egui::Id::unique(("tab-menu", &p.id)), &tab, &items) {
                actions.push(Action::Navigate { page, arg });
            }
        }
        let plugin_pages: Vec<_> = pages.iter().filter(|p| p.placement == Placement::Apps).collect();
        if !plugin_pages.is_empty() {
            let active = plugin_pages.iter().any(|p| p.id == current);
            let resp = w::nav_tab_icon(ui, t, Some(icons::PLUGS), &format!("{} {}", t!("Plugins"), icons::CARET_DOWN), active, TOP_BAR);
            egui::Popup::menu(&resp).width(220.0).show(|ui| {
                for p in &plugin_pages {
                    let label = format!("{}   {}", p.icon, p.title);
                    if ui.add(egui::Button::selectable(current == p.id, w::text_medium(label, 14.0, t.text)).min_size(vec2(200.0, 34.0))).clicked() {
                        actions.push(Action::Navigate { page: p.id.clone(), arg: None });
                    }
                }
            });
        }

        ui.with_layout(Layout::right_to_left(Align::Center), |ui| {
            ui.spacing_mut().item_spacing.x = 6.0;
            crate::chrome::window_buttons(ui, t, TOP_BAR, true);
            ui.add_space(6.0);
            account_menu(app, ui, t, actions);
            crate::offline_ui::badge(app, ui, t);
            // Like Steam: no wallet button while it's empty.
            if let Some((balance, currency)) = app.state.wallet.filter(|(b, _)| *b > 0 || app.state.wallet_pending > 0) {
                wallet_button(ui, t, balance, currency, app.state.wallet_pending, actions);
            }
            let unread = app.state.total_unread();
            if w::icon_button_sized(ui, t, icons::MONITOR, t!("Big Picture mode"), false, 0, CTRL).clicked() {
                app.bigpicture.enter(ui.ctx());
            }
            notifications_button(app, ui, t, actions);
            let fr = w::icon_button_sized(ui, t, icons::USERS, t!("Friends"), app.prefs.friends_open, unread, CTRL);
            if fr.clicked() {
                app.prefs.friends_open = !app.prefs.friends_open;
            }
            friends_hover(app, ui, t, &fr, actions);
        });
    });
}

/// One entry of a header tab's hover menu: icon, label, page, page argument.
pub(crate) type MenuEntry = (&'static str, String, &'static str, Option<String>);

/// Shortcuts into a page, like Steam's menus under STORE / LIBRARY /
/// COMMUNITY / your name. `None` entries are separators.
pub(crate) fn tab_menu_items(app: &SteamApp, page: &str) -> Vec<Option<MenuEntry>> {
    let e = |icon, label: &str, page, arg: Option<String>| Some((icon, label.to_string(), page, arg));
    let me = app.state.steam_id;
    match page {
        "store" => vec![
            e(icons::SPARKLE, t!("Featured"), "store", None),
            e(icons::SLIDERS, t!("Browse"), "store", Some("browse".into())),
            None,
            e(icons::TAG, t!("Specials"), "store", Some("section:specials".into())),
            e(icons::FIRE, t!("Top sellers"), "store", Some("section:top_sellers".into())),
            e(icons::STAR, t!("New releases"), "store", Some("section:new_releases".into())),
            e(icons::CALENDAR, t!("Coming soon"), "store", Some("section:coming_soon".into())),
            None,
            e(icons::HEART, t!("Wishlist"), "store", Some("wishlist".into())),
            e(icons::CART, t!("Cart"), "store", Some("cart".into())),
        ],
        "library" => vec![
            e(icons::HOUSE, t!("Home"), "library", None),
            e(icons::DOWNLOAD, t!("Downloads"), "downloads", None),
            None,
            e(icons::RESTORE, t!("Restore game backup…"), "library", Some("restore".into())),
        ],
        crate::builtin::community_home::PAGE => vec![
            e(icons::HOUSE, t!("Home"), crate::builtin::community_home::PAGE, None),
            e(icons::LIGHTNING, t!("Friends activity"), crate::builtin::community_home::PAGE, Some("friends".into())),
            e(icons::GLOBE, t!("Explore"), crate::builtin::community_home::PAGE, Some("explore".into())),
            e(icons::NOTE, t!("Developers"), crate::builtin::community_home::PAGE, Some("developers".into())),
            e(icons::CHATS, t!("Discussions"), crate::builtin::community_home::PAGE, Some("discussions".into())),
            None,
            e(icons::USERS, t!("Friends"), "community", None),
            e(icons::CUBE, t!("Workshop"), "workshop", None),
            e(icons::BACKPACK, t!("Inventory"), "inventory", None),
        ],
        "profile" => vec![
            e(icons::USER_CIRCLE, t!("My profile"), "profile", None),
            e(icons::CONTROLLER, t!("Games"), "profile", Some(format!("{me}/games"))),
            e(icons::BACKPACK, t!("Inventory"), "inventory", None),
        ],
        _ => Vec::new(),
    }
}

/// A menu that opens while `anchor` is hovered (after a short delay, so
/// passing over the tab doesn't open it) and stays open while the pointer
/// is on the tab or the menu. Returns the entry clicked.
pub(crate) fn hover_menu(ui: &mut Ui, t: &Theme, id: egui::Id, anchor: &egui::Response, items: &[Option<MenuEntry>]) -> Option<(String, Option<String>)> {
    if items.is_empty() {
        return None;
    }
    let now = ui.input(|i| i.time);
    let pointer = ui.input(|i| i.pointer.hover_pos());
    let menu_rect: Option<Rect> = ui.data(|d| d.get_temp(id.with("rect")));
    let over_menu = pointer.zip(menu_rect).is_some_and(|(p, r)| r.expand(8.0).contains(p));
    // The tab's own pill plus the strip down to the menu, so moving onto
    // the menu never crosses a gap.
    let pill = Rect::from_center_size(anchor.rect.center(), vec2(anchor.rect.width(), w::CONTROL_LG));
    let bridge = Rect::from_min_max(pill.left_top(), egui::pos2(pill.right(), anchor.rect.bottom() + 6.0));
    let over_tab = anchor.hovered() || pointer.is_some_and(|p| bridge.contains(p)) || over_menu;
    let mut since: Option<f64> = ui.data(|d| d.get_temp(id));
    if over_tab {
        since.get_or_insert(now);
        ui.data_mut(|d| d.insert_temp(id.with("left"), now));
    } else {
        let left: f64 = ui.data(|d| d.get_temp(id.with("left"))).unwrap_or(0.0);
        if now - left > 0.2 {
            since = None;
        }
    }
    ui.data_mut(|d| match since {
        Some(s) => {
            d.insert_temp(id, s);
        }
        None => {
            d.remove::<f64>(id);
        }
    });
    let Some(start) = since else {
        ui.data_mut(|d| d.remove::<Rect>(id.with("rect")));
        return None;
    };
    if now - start < 0.25 {
        ui.ctx().request_repaint_after(std::time::Duration::from_millis(60));
        return None;
    }
    let mut picked = None;
    let pos = egui::pos2(pill.left(), pill.bottom() + 6.0);
    let area = egui::Area::new(id.with("area")).order(egui::Order::Foreground).fixed_pos(pos).show(ui.ctx(), |ui| {
        egui::Frame::new()
            .fill(t.chrome)
            .stroke(Stroke::new(1.0, t.border))
            .corner_radius(t.corner())
            .shadow(egui::Shadow { offset: [0, 10], blur: 28, spread: 0, color: egui::Color32::from_black_alpha(130) })
            .inner_margin(Margin::same(6))
            .show(ui, |ui| {
                ui.set_width(210.0);
                ui.spacing_mut().item_spacing.y = 2.0;
                for item in items {
                    match item {
                        None => {
                            ui.add_space(3.0);
                            let (r, _) = ui.allocate_exact_size(vec2(ui.available_width(), 1.0), Sense::hover());
                            ui.painter().hline(r.x_range().shrink(6.0), r.center().y, Stroke::new(1.0, t.border));
                            ui.add_space(3.0);
                        }
                        Some((icon, label, page, arg)) => {
                            if w::menu_item(ui, t, icon, label).clicked() {
                                picked = Some((page.to_string(), arg.clone()));
                            }
                        }
                    }
                }
            });
    });
    ui.data_mut(|d| d.insert_temp(id.with("rect"), area.response.rect));
    if picked.is_some() {
        // Close right away; it reopens only on a fresh hover.
        ui.data_mut(|d| {
            d.remove::<f64>(id);
            d.remove::<Rect>(id.with("rect"));
            d.insert_temp(id.with("left"), 0.0f64);
        });
    }
    ui.ctx().request_repaint_after(std::time::Duration::from_millis(150));
    picked
}

/// Hovering the friends button shows a quick friends list, like Steam's,
/// with a way into the undocked Friends & Chat window.
pub(crate) fn friends_hover(app: &mut SteamApp, ui: &mut Ui, t: &Theme, button: &egui::Response, actions: &mut Vec<Action>) {
    let id = egui::Id::unique("friends-hover");
    let now = ui.input(|i| i.time);
    let mut since: Option<f64> = ui.data(|d| d.get_temp(id));
    let popup_rect: Option<Rect> = ui.data(|d| d.get_temp(id.with("rect")));
    let pointer = ui.input(|i| i.pointer.hover_pos());
    let over_popup = pointer.zip(popup_rect).is_some_and(|(p, r)| r.expand(6.0).contains(p));
    let bridge = Rect::from_min_max(button.rect.left_top(), egui::pos2(button.rect.right(), button.rect.bottom() + 8.0));
    let over_button = button.hovered() || pointer.is_some_and(|p| bridge.contains(p));
    if over_button || over_popup {
        since.get_or_insert(now);
        ui.data_mut(|d| d.insert_temp(id.with("left"), now));
    } else if pointer.is_none() {
        since = None;
    } else {
        let left: f64 = ui.data(|d| d.get_temp(id.with("left"))).unwrap_or(0.0);
        if now - left > 0.25 {
            since = None;
        }
    }
    ui.data_mut(|d| {
        match since {
            Some(s) => {
                d.insert_temp(id, s);
            }
            None => d.remove::<f64>(id),
        }
    });
    // Short delay so passing over the button doesn't open it.
    let Some(start) = since else {
        ui.data_mut(|d| d.remove::<Rect>(id.with("rect")));
        return;
    };
    if now - start < 0.35 {
        ui.ctx().request_repaint_after(std::time::Duration::from_millis(100));
        return;
    }
    let pos = egui::pos2(button.rect.right() - 300.0, button.rect.bottom() + 6.0);
    let area = egui::Area::new(id.with("area")).order(egui::Order::Foreground).fixed_pos(pos).show(ui.ctx(), |ui| {
        egui::Frame::new()
            .fill(t.chrome)
            .stroke(Stroke::new(1.0, t.border))
            .corner_radius(t.corner())
            .shadow(egui::Shadow { offset: [0, 10], blur: 30, spread: 0, color: egui::Color32::from_black_alpha(140) })
            .inner_margin(Margin::same(10))
            .show(ui, |ui| {
                ui.set_width(300.0);
                let online: Vec<_> = app
                    .state
                    .friend_refs()
                    .into_iter()
                    .filter_map(|(_, p)| p.filter(|p| p.state != PersonaState::Offline).cloned())
                    .collect();
                ui.horizontal(|ui| {
                    ui.label(w::text_bold(t!("Friends"), 15.0, t.text));
                    ui.label(w::text(t!("{n} online", n = online.len()), 12.5, t.online));
                    ui.with_layout(Layout::right_to_left(Align::Center), |ui| {
                        if w::icon_button(ui, t, icons::EXTERNAL, t!("Open Friends & Chat"), false, 0).clicked() {
                            app.friends_window.open = true;
                        }
                    });
                });
                ui.add_space(4.0);
                egui::ScrollArea::vertical().max_height(420.0).show(ui, |ui| {
                    let groups: Vec<_> = app
                        .state
                        .group_list()
                        .into_iter()
                        .map(|g| {
                            (
                                g.id,
                                app.state.group_display_name(g),
                                g.avatar_url.clone(),
                                g.unread_count(),
                                g.default_text_channel_id(),
                                g.active_member_count.max(g.members.len() as u32),
                            )
                        })
                        .collect();
                    if online.is_empty() && groups.is_empty() {
                        ui.label(w::text(t!("No friends online"), 13.0, t.text_faint));
                    }
                    for p in &online {
                        let (r, resp) = ui.allocate_exact_size(vec2(ui.available_width(), 42.0), Sense::click());
                        if !ui.is_rect_visible(r) {
                            continue;
                        }
                        if resp.hovered() {
                            ui.painter().rect_filled(r, t.corner_small(), t.surface_hover);
                        }
                        let av = Rect::from_min_size(r.min + vec2(4.0, 5.0), vec2(32.0, 32.0));
                        if !w::paint_cover(ui, av, &art::avatar(p.avatar_hash.as_deref()), egui::CornerRadius::same(4)) {
                            w::skeleton(ui, t, av, egui::CornerRadius::same(4));
                        }
                        let color = status_color(t, p);
                        ui.painter().circle_filled(av.right_bottom() - vec2(1.0, 1.0), 4.5, color);
                        let unread = app.state.unread.get(&p.steam_id).copied().unwrap_or(0);
                        let text_right = if unread > 0 { r.right() - 26.0 } else { r.right() - 6.0 };
                        let text_rect = Rect::from_min_max(egui::pos2(av.right() + 10.0, r.top() + 3.0), egui::pos2(text_right, r.bottom() - 3.0));
                        let mut child = ui.new_child(egui::UiBuilder::new().max_rect(text_rect).layout(Layout::top_down(Align::Min)));
                        child.spacing_mut().item_spacing.y = 0.0;
                        child.add(egui::Label::new(w::text_semibold(p.display_name(), 13.0, if p.in_game() { t.in_game } else { t.text })).truncate());
                        let status = status_text(&app.state, p);
                        child.add(egui::Label::new(w::text(&status, 11.5, color)).truncate());
                        if unread > 0 {
                            let c = egui::pos2(r.right() - 12.0, r.center().y);
                            ui.painter().circle_filled(c, 8.0, t.accent);
                            ui.painter().text(c, Align2::CENTER_CENTER, unread.to_string(), w::font_bold(10.0), egui::Color32::WHITE);
                        }
                        if resp.on_hover_cursor(egui::CursorIcon::PointingHand).on_hover_text(t!("Send a message")).clicked() {
                            // Chat in the undocked window, like Steam.
                            app.friends_window.open = true;
                            app.friends_window.chat = Some(crate::builtin::group_chat::ChatTarget::Friend(p.steam_id));
                            app.client.send(Command::LoadHistory { friend: p.steam_id });
                            actions.push(Action::MarkRead(p.steam_id));
                        }
                    }
                    if !groups.is_empty() {
                        if !online.is_empty() {
                            ui.add_space(6.0);
                        }
                        ui.label(w::text_semibold(t!("Group chats"), 11.5, t.text_dim));
                        ui.add_space(2.0);
                        for (gid, name, avatar_url, unread, default_ch, count) in groups {
                            let (r, resp) = ui.allocate_exact_size(vec2(ui.available_width(), 40.0), Sense::click());
                            if resp.hovered() {
                                ui.painter().rect_filled(r, t.corner_small(), t.surface_hover);
                            }
                            let av = Rect::from_min_size(r.min + vec2(4.0, 4.0), vec2(32.0, 32.0));
                            let radius = egui::CornerRadius::same(7);
                            if !avatar_url.as_deref().is_some_and(|u| w::paint_cover(ui, av, u, radius)) {
                                ui.painter().rect_filled(av, radius, w::mix(t.raised, t.accent, 0.25));
                                let init: String = name.chars().filter(|c| c.is_alphanumeric()).take(2).collect();
                                ui.painter().text(av.center(), Align2::CENTER_CENTER, init.to_uppercase(), w::font_bold(12.0), t.text);
                            }
                            let text_right = if unread > 0 { r.right() - 26.0 } else { r.right() - 6.0 };
                            let text_rect = Rect::from_min_max(egui::pos2(av.right() + 10.0, r.top() + 3.0), egui::pos2(text_right, r.bottom() - 3.0));
                            let mut child = ui.new_child(egui::UiBuilder::new().max_rect(text_rect).layout(Layout::top_down(Align::Min)));
                            child.spacing_mut().item_spacing.y = 0.0;
                            child.add(egui::Label::new(w::text_semibold(&name, 13.0, t.text)).truncate());
                            child.add(egui::Label::new(w::text(t!("{n} members", n = count), 11.5, t.text_dim)).truncate());
                            if unread > 0 {
                                let c = egui::pos2(r.right() - 12.0, r.center().y);
                                ui.painter().circle_filled(c, 8.0, t.accent);
                                ui.painter().text(c, Align2::CENTER_CENTER, unread.min(99).to_string(), w::font_bold(10.0), egui::Color32::WHITE);
                            }
                            if resp.on_hover_cursor(egui::CursorIcon::PointingHand).on_hover_text(t!("Open group chat")).clicked() {
                                app.friends_window.open = true;
                                app.friends_window.chat = Some(crate::builtin::group_chat::ChatTarget::Group(gid));
                                app.friends_window.group_ui.chat_id = default_ch;
                                if let Some(cid) = default_ch {
                                    app.client.send(Command::LoadGroupHistory { group_id: gid, chat_id: cid, before: None });
                                    actions.push(Action::MarkGroupRead { group_id: gid, chat_id: cid });
                                }
                            }
                        }
                    }
                });
                ui.separator();
                if ui.add(egui::Button::new(w::text_medium(format!("{}   {}", icons::USERS, t!("All friends")), 13.0, t.accent)).frame(false)).clicked() {
                    actions.push(Action::Navigate { page: "community".into(), arg: None });
                }
            });
    });
    ui.data_mut(|d| d.insert_temp(id.with("rect"), area.response.rect));
    ui.ctx().request_repaint_after(std::time::Duration::from_millis(150));
}

/// Icon and plain-language text for a notification. Covers every type
/// Steam defines (ESteamNotificationType).
pub fn notification_text(state: &client::State, n: &model::Notification) -> (&'static str, String) {
    let who = || n.from.map(|id| state.persona(id).name).filter(|s| !s.is_empty()).unwrap_or_else(|| t!("Someone").to_string());
    let game_of = |id: Option<model::AppId>| id.filter(|a| *a != 0 && *a != 753).and_then(|a| state.app_name(a).map(String::from)).or_else(|| n.text.clone());
    let game = |id: Option<model::AppId>| game_of(id).unwrap_or_else(|| t!("a game").to_string());
    let s = |x: &str| x.to_string();
    let (icon, text) = match n.kind {
        1 => (icons::BELL, t!("Test notification").to_string()),
        2 => match game_of(n.app_id) {
            Some(g) if n.from.is_some() => (icons::GIFT, t!("{name} sent you {game} as a gift", name = who(), game = g)),
            Some(g) => (icons::GIFT, t!("You received {game} as a gift", game = g)),
            None if n.from.is_some() => (icons::GIFT, t!("{name} sent you a gift", name = who())),
            None => (icons::GIFT, s(t!("You received a gift"))),
        },
        3 if n.from == Some(state.steam_id) || n.from.is_none() => (icons::CHAT, s(t!("New comment on your profile"))),
        3 => (icons::CHAT, t!("{name} replied in a discussion you follow", name = who())),
        4 if n.app_id == Some(753) && n.source_app.is_some_and(|a| a != 0) => (icons::BACKPACK, t!("New item from {game}", game = game(n.source_app))),
        4 if n.app_id == Some(753) => (icons::BACKPACK, s(t!("New item in your Steam inventory"))),
        4 => (icons::BACKPACK, t!("New item in your {game} inventory", game = game(n.app_id))),
        5 => (icons::USER_PLUS, t!("{name} sent you a friend request", name = who())),
        6 => (icons::TAG, s(t!("A big Steam sale has started"))),
        7 => (icons::DOWNLOAD, t!("{game} is ready to preload", game = game(n.app_id))),
        8 => (icons::HEART, t!("{game} from your wishlist is on sale", game = game(n.app_id))),
        9 => (icons::SWAP, t!("{name} sent you a trade offer", name = who())),
        10 => (icons::BELL, n.text.clone().unwrap_or_else(|| s(t!("New Steam notification")))),
        11 => (icons::INFO, s(t!("Steam Support replied to your request"))),
        12 => (icons::CONTROLLER, t!("It's your turn in {game}", game = game(n.app_id))),
        13 => (icons::CHATS, t!("New message from {name}", name = who())),
        14 => (icons::SHIELD, s(t!("You have a message from Steam moderators"))),
        15 => (icons::USERS_THREE, t!("{name} asked to use a feature", name = who())),
        16 => (icons::USERS_THREE, t!("{name} invited you to their Steam Family", name = who())),
        17 => (icons::CART, t!("{name} asked you to buy {game}", name = who(), game = game(n.app_id))),
        18 => (icons::CLOCK, t!("{name} asked for more playtime", name = who())),
        19 => (icons::CART, t!("Your request to buy {game} was answered", game = game(n.app_id))),
        20 => (icons::USERS_THREE, s(t!("Your feature request was answered"))),
        21 => (icons::CLOCK, s(t!("Your playtime request was answered"))),
        22 => (icons::BOOKS, t!("{game} was added to your library", game = game(n.app_id))),
        23 => (icons::SEND, s(t!("Sent to your phone"))),
        24 => (icons::CAMERA, t!("Your {game} clip is ready", game = game(n.app_id))),
        25 => (icons::KEY, s(t!("Someone is signing in to your account. Check the Steam Mobile app."))),
        26 => (icons::SHIELD, s(t!("Something is waiting for you to confirm in the Steam Mobile app"))),
        27 => (icons::CALENDAR, t!("{game} has a new event", game = game(n.app_id))),
        28 => (icons::CONTROLLER, t!("You're invited to the {game} playtest", game = game(n.app_id))),
        29 => (icons::WARNING, s(t!("A trade was reversed"))),
        30 => (icons::SHIELD, s(t!("Steam took action on content you reported"))),
        _ => (icons::BELL, n.text.clone().unwrap_or_else(|| s(t!("New Steam notification")))),
    };
    // "{game} ..." can open with the lowercase fallback "a game".
    let mut chars = text.chars();
    let text = chars.next().map(|c| c.to_uppercase().chain(chars).collect()).unwrap_or(text);
    (icon, text)
}

#[derive(Debug)]
enum NotificationTarget {
    Action(Action),
    Web(String),
    None,
}

fn notification_steam_url(n: &model::Notification) -> Option<String> {
    let v: serde_json::Value = serde_json::from_str(&n.data).ok()?;
    let url = v.get("url").and_then(|u| u.as_str()).map(str::trim)?;
    if url.starts_with("https://steamcommunity.com/") {
        Some(url.to_string())
    } else {
        None
    }
}

fn notification_target(state: &client::State, n: &model::Notification) -> NotificationTarget {
    let nav = |page: &str, arg: Option<String>| NotificationTarget::Action(Action::Navigate { page: page.into(), arg });
    let web = |url: String| NotificationTarget::Web(url);
    let me = state.steam_id;
    let app = n.app_id.filter(|a| *a != 0 && *a != 753);
    match n.kind {
        2 => {
            if me != 0 {
                web(format!("https://steamcommunity.com/profiles/{me}/inventory/#pending_gifts"))
            } else {
                NotificationTarget::None
            }
        }
        3 => {
            if let Some(url) = notification_steam_url(n).filter(|u| u.contains("/discussions/") || u.contains("/app/")) {
                nav("thread", Some(url))
            } else if n.from.is_some_and(|id| id != me) {
                match app {
                    Some(a) => nav("discussions", Some(a.to_string())),
                    None => nav("profile", (me != 0).then(|| me.to_string())),
                }
            } else {
                nav("profile", (me != 0).then(|| me.to_string()))
            }
        }
        4 => nav("inventory", Some(format!("{me}/{}", n.app_id.filter(|a| *a != 0).unwrap_or(753)))),
        5 => match n.from {
            Some(id) if state.friends.get(&id) != Some(&model::Relationship::RequestRecipient) => nav("profile", Some(id.to_string())),
            _ => nav("community", Some("invites".into())),
        },
        6 => nav("store", None),
        7 | 12 | 22 | 24 => nav("library", app.map(|a| a.to_string())),
        8 => nav("store", Some(app.map_or_else(|| "wishlist".to_string(), |a| format!("app:{a}")))),
        9 | 29 => {
            if me != 0 {
                web(format!("https://steamcommunity.com/profiles/{me}/tradeoffers/"))
            } else {
                NotificationTarget::None
            }
        }
        11 => nav("support", n.ticket.as_ref().map(|t| format!("ticket:{t}"))),
        13 => match n.from {
            Some(id) => NotificationTarget::Action(Action::OpenChat(id)),
            None => nav("community", None),
        },
        15..=21 => nav("settings", Some("family".into())),
        25 | 26 => nav("settings", Some("security".into())),
        27 => match app {
            Some(a) => nav("hub", Some(format!("{a}/news"))),
            None => nav("store", None),
        },
        28 => nav("store", app.map(|a| format!("app:{a}"))),
        _ => NotificationTarget::None,
    }
}

/// Where clicking a notification takes you.
pub(crate) fn open_notification(state: &client::State, n: &model::Notification, actions: &mut Vec<Action>) {
    match notification_target(state, n) {
        NotificationTarget::Action(a) => actions.push(a),
        NotificationTarget::Web(url) => {
            crate::links::open_http(&url);
        }
        NotificationTarget::None => {}
    }
}

/// The picture for a notification: which game it's about or who sent it,
/// with the kind of notification as a small badge, like Steam's.
fn notification_picture(ui: &Ui, t: &Theme, state: &client::State, n: &model::Notification, icon: &str, rect: Rect) {
    notification_picture_scaled(ui, t, state, n, icon, rect, 1.0);
}

pub(crate) fn notification_picture_scaled(ui: &Ui, t: &Theme, state: &client::State, n: &model::Notification, icon: &str, rect: Rect, s: f32) {
    let radius = egui::CornerRadius::same((6.0 * s).round().clamp(4.0, 255.0) as u8);
    let game = match n.kind {
        4 if n.app_id == Some(753) => n.source_app,
        _ => n.app_id,
    }
    .filter(|a| *a != 0 && *a != 753);
    let person = n.from.filter(|id| *id != state.steam_id);
    // People first where the person is the point; the game otherwise.
    let person_first = matches!(n.kind, 3 | 5 | 9 | 13 | 15 | 16 | 18);
    let draw_person = |id: u64| w::paint_cover(ui, rect, &art::avatar(state.persona(id).avatar_hash.as_deref()), radius);
    // Portrait library art crops well to a square; fall back to the header.
    let draw_game = |app: model::AppId| w::paint_cover(ui, rect, &art::capsule(app), radius) || w::paint_cover(ui, rect, &art::header(app), radius);
    let drawn = match (person, game) {
        (Some(p), _) if person_first => draw_person(p),
        (_, Some(g)) => draw_game(g),
        (Some(p), None) => draw_person(p),
        (None, None) => false,
    };
    let p = ui.painter();
    if !drawn {
        p.rect_filled(rect, radius, t.raised);
        p.text(rect.center(), Align2::CENTER_CENTER, icon, w::font(20.0 * s), if n.read { t.text_dim } else { t.accent });
        return;
    }
    // Kind badge in the corner.
    let c = rect.right_bottom() - vec2(3.0 * s, 3.0 * s);
    p.circle_filled(c, 10.5 * s, t.chrome);
    p.circle_filled(c, 8.8 * s, if n.read { t.raised } else { t.accent });
    p.text(c, Align2::CENTER_CENTER, icon, w::font(10.5 * s), if n.read { t.text_dim } else { t.on_accent });
}

/// The bell: Steam notifications with an unread badge.
fn notifications_button(app: &SteamApp, ui: &mut Ui, t: &Theme, actions: &mut Vec<Action>) {
    let Some(data) = &app.state.notifications else { return };
    let waiting = data.pending_gifts + data.pending_friends + data.confirmations;
    let badge = (data.unread + waiting) as usize;
    let r = w::icon_button_sized(ui, t, icons::BELL, t!("Notifications"), false, badge, CTRL);
    notifications_popup(app, &r, t, data, actions);
}

pub(crate) fn notifications_popup(app: &SteamApp, r: &egui::Response, t: &Theme, data: &model::Notifications, actions: &mut Vec<Action>) {
    egui::Popup::from_toggle_button_response(r).close_behavior(egui::PopupCloseBehavior::CloseOnClickOutside).width(380.0).show(|ui| {
        ui.with_layout(Layout::top_down(Align::Min), |ui| {
            ui.horizontal(|ui| {
                ui.label(w::text_bold(t!("Notifications"), 16.0, t.text));
                ui.with_layout(Layout::right_to_left(Align::Center), |ui| {
                    if data.unread > 0 && ui.link(w::text(t!("Mark all as read"), 12.5, t.accent)).clicked() {
                        app.client.send(Command::MarkNotificationsRead { ids: vec![] });
                    }
                });
            });
            // Things waiting for you, like the summary rows in Steam's bell.
            let mut summary = Vec::new();
            if data.pending_gifts > 0 {
                summary.push((icons::GIFT, t!("{n} gifts waiting for you", n = data.pending_gifts), 2));
            }
            if data.pending_friends > 0 {
                summary.push((icons::USER_PLUS, t!("{n} friend requests", n = data.pending_friends), 5));
            }
            if data.confirmations > 0 {
                summary.push((icons::SHIELD, t!("{n} confirmations waiting in the Steam Mobile app", n = data.confirmations), 26));
            }
            for (icon, text, kind) in summary {
                if ui.add(egui::Button::new(w::text_semibold(format!("{icon}  {text}"), 13.0, t.accent)).frame(false)).clicked() {
                    let row = model::Notification { kind, ..Default::default() };
                    open_notification(&app.state, &row, actions);
                }
            }
            ui.separator();
            if data.items.is_empty() {
                ui.label(w::text(t!("You're all caught up."), 13.0, t.text_faint));
                return;
            }
            egui::ScrollArea::vertical().max_height(440.0).show(ui, |ui| {
                for n in data.items.iter().take(40) {
                    let (icon, text) = notification_text(&app.state, n);
                    // Up to two lines of text, then the time.
                    let mut job = egui::text::LayoutJob::simple(text.clone(), w::font_medium(13.0), if n.read { t.text_dim } else { t.text }, ui.available_width() - 86.0);
                    job.wrap.max_rows = 2;
                    job.wrap.overflow_character = Some('…');
                    let galley = ui.painter().layout_job(job);
                    let h = (galley.size().y + 34.0).max(58.0);
                    let (rect, resp) = ui.allocate_exact_size(vec2(ui.available_width(), h), Sense::click());
                    if resp.hovered() {
                        ui.painter().rect_filled(rect, t.corner_small(), t.surface_hover);
                    }
                    let color = if n.read { t.text_dim } else { t.text };
                    notification_picture(ui, t, &app.state, n, icon, Rect::from_min_size(rect.min + vec2(6.0, 7.0), vec2(44.0, 44.0)));
                    let p = ui.painter();
                    let text_h = galley.size().y;
                    p.galley(egui::pos2(rect.left() + 60.0, rect.top() + 9.0), galley, color);
                    p.text(egui::pos2(rect.left() + 60.0, rect.top() + 14.0 + text_h), Align2::LEFT_TOP, ui::ago(n.time), w::font(11.5), t.text_faint);
                    if !n.read {
                        p.circle_filled(egui::pos2(rect.right() - 12.0, rect.center().y), 4.0, t.accent);
                    }
                    if resp.on_hover_cursor(egui::CursorIcon::PointingHand).on_hover_text(&text).clicked() {
                        if !n.read {
                            app.client.send(Command::MarkNotificationsRead { ids: vec![n.id] });
                        }
                        open_notification(&app.state, n, actions);
                        egui::Popup::close_all(ui.ctx());
                    }
                }
            });
        });
    });
}

/// Wallet balance in the top bar; click for pending funds and wallet actions.
pub(crate) fn wallet_button(ui: &mut Ui, t: &Theme, balance: i64, currency: i32, pending: i64, actions: &mut Vec<Action>) {
    let mut label = model::format_money(balance, currency);
    if pending > 0 {
        label = format!("{label}  ·  +{}", model::format_money(pending, currency));
    }
    let r = ui.add(egui::Button::new(w::text_medium(label, 13.0, t.text)).fill(t.raised).stroke(Stroke::NONE).corner_radius(t.corner_small()).min_size(vec2(0.0, CTRL)));
    let r = r.on_hover_text(t!("Steam Wallet"));
    egui::Popup::menu(&r).show(|ui| {
        ui.set_width(300.0);
        // Menus justify their text; the notes read better left-aligned.
        ui.with_layout(Layout::top_down(Align::Min), |ui| {
        ui.label(w::caption_text(t, t!("Steam Wallet")));
        ui.label(w::text_bold(model::format_money(balance, currency), 24.0, t.text));
        ui.label(w::text(t!("Available to spend"), 12.5, t.text_dim));
        if pending > 0 {
            ui.add_space(8.0);
            ui.horizontal(|ui| {
                ui.label(w::text(icons::CLOCK, 14.0, t.away));
                ui.label(w::text_semibold(t!("Pending: {amount}", amount = model::format_money(pending, currency)), 13.5, t.away));
            });
            ui.add(egui::Label::new(w::text(t!("Money from recent Market sales or refunds that Steam hasn't released yet. It's added to your balance automatically."), 12.0, t.text_dim)).wrap());
        }
        ui.add_space(6.0);
        ui.add(egui::Label::new(w::text(t!("Wallet funds can be spent in the Store and on the Community Market. They can't be withdrawn."), 12.0, t.text_faint)).wrap());
        });
        ui.separator();
        let item = |ui: &mut Ui, icon: &str, label: &str| w::menu_item(ui, t, icon, label).clicked();
        if item(ui, icons::PLUS, t!("Add funds…")) {
            crate::links::open_http("https://store.steampowered.com/steamaccount/addfunds");
        }
        if item(ui, icons::TAG, t!("Redeem a Steam Wallet code…")) {
            crate::links::open_http("https://store.steampowered.com/account/redeemwalletcode");
        }
        if item(ui, icons::STOREFRONT, t!("My market listings")) {
            actions.push(Action::Navigate { page: "inventory".into(), arg: Some("listings".into()) });
        }
        if item(ui, icons::SCROLL, t!("Purchase history")) {
            actions.push(Action::Navigate { page: "settings".into(), arg: Some("history".into()) });
        }
    });
}

/// The logo is a menu, like Steam's "Steam" menu.
fn app_menu(app: &mut SteamApp, ui: &mut Ui, t: &Theme, actions: &mut Vec<Action>) {
    let galley = ui.painter().layout_no_wrap(i18n::BRAND.into(), w::font_bold(15.0), t.text);
    let (rect, resp) = ui.allocate_exact_size(vec2(24.0 + 8.0 + galley.size().x + 28.0, CTRL), Sense::click());
    let hover = ui.ctx().animate_bool_with_time(resp.id, resp.hovered(), 0.1);
    ui.painter().rect_filled(rect, t.corner_small(), w::mix(egui::Color32::TRANSPARENT, t.surface_hover, hover));
    let mut c = ui.new_child(egui::UiBuilder::new().max_rect(Rect::from_min_size(rect.min + vec2(6.0, (CTRL - 24.0) / 2.0), vec2(24.0, 24.0))));
    w::logo(&mut c, t, 24.0);
    ui.painter().galley(egui::pos2(rect.left() + 38.0, rect.center().y - galley.size().y / 2.0), galley, t.text);
    ui.painter().text(rect.right_center() - vec2(10.0, 0.0), Align2::CENTER_CENTER, icons::CARET_DOWN, w::font(12.0), t.text_faint);
    let resp = resp.on_hover_cursor(egui::CursorIcon::PointingHand);
    egui::Popup::menu(&resp).width(320.0).show(|ui| {
        let item = |ui: &mut Ui, icon: &str, label: &str| w::menu_item(ui, t, icon, label).clicked();
        if item(ui, icons::GEAR, t!("Settings")) {
            actions.push(Action::Navigate { page: "settings".into(), arg: None });
        }
        if item(ui, icons::DOWNLOAD, t!("Downloads")) {
            actions.push(Action::Navigate { page: "downloads".into(), arg: None });
        }
        if item(ui, icons::MONITOR, t!("Big Picture mode")) {
            app.bigpicture.enter(ui.ctx());
        }
        if item(ui, icons::INFO, t!("Steam Support")) {
            actions.push(Action::Navigate { page: "support".into(), arg: None });
        }
        ui.separator();
        if item(ui, icons::KEY, t!("Activate a product…")) {
            app.activate = Some(Default::default());
        }
        if item(ui, icons::TAG, t!("Redeem a Steam Wallet code…")) {
            crate::links::open_http("https://store.steampowered.com/account/redeemwalletcode");
        }
        if item(ui, icons::PLUGS, t!("Plugins")) {
            actions.push(Action::Navigate { page: "settings".into(), arg: Some("plugins".into()) });
        }
        // Like Steam's "Check for Steam Client Updates…".
        if item(ui, icons::REFRESH, t!("Check for Aurora updates…")) {
            crate::updater::check(ui.ctx().clone(), true);
        }
        ui.separator();
        if item(ui, icons::USERS, t!("Switch account…")) {
            app.client.send(Command::LogOut);
        }
        if item(ui, icons::POWER, t!("Exit")) {
            app.request_exit(ui.ctx());
        }
    });
}

/// "Activate a product" dialog (product keys).
pub fn activate_dialog(app: &mut SteamApp, ctx: &egui::Context) {
    let t = app.prefs.theme.clone();
    let Some(dlg) = &mut app.activate else { return };
    let mut close = false;
    ui::widgets::modal(ctx, egui::Id::unique("activate")).show(ctx, |ui| {
        ui.set_width(460.0);
        ui.label(w::text(icons::KEY, 28.0, t.accent));
        ui.label(w::text_bold(t!("Activate a product on Steam"), 20.0, t.text));
        ui.add_space(4.0);
        ui.label(w::text(t!("Enter a product key from a retail box, a bundle or a gift email."), 13.5, t.text_dim));
        ui.add_space(10.0);
        ui.add_enabled(
            !dlg.busy,
            egui::TextEdit::singleline(&mut dlg.key).hint_text("XXXXX-XXXXX-XXXXX").font(w::font_semibold(18.0)).desired_width(f32::INFINITY).margin(vec2(12.0, 10.0)),
        ).with_edit_menu();
        match &dlg.result {
            Some(Ok(items)) => {
                ui.label(w::text(format!("{}  {}", icons::CHECK, t!("Activated!")), 14.0, t.play));
                for i in items {
                    ui.label(w::text(format!("•  {i}"), 13.5, t.text));
                }
            }
            Some(Err(e)) => {
                ui.label(w::text(format!("{}  {}", icons::WARNING, i18n::tr_dyn(e)), 13.5, t.danger));
            }
            None => {}
        }
        ui.add_space(12.0);
        ui.horizontal(|ui| {
            let ready = dlg.key.trim().len() >= 5 && !dlg.busy;
            if ui.add_enabled_ui(ready, |ui| w::button(ui, &t, None, t!("Activate"), w::Kind::Primary, w::Size::Md)).inner.clicked() {
                dlg.busy = true;
                dlg.result = None;
                app.client.send(Command::ActivateKey { key: dlg.key.trim().to_string() });
            }
            if dlg.busy {
                ui.add(egui::Spinner::new().color(t.accent));
            }
            if w::button(ui, &t, None, t!("Close"), w::Kind::Secondary, w::Size::Md).clicked() {
                close = true;
            }
        });
    });
    if close {
        app.activate = None;
    }
}

fn account_menu(app: &mut SteamApp, ui: &mut Ui, t: &Theme, actions: &mut Vec<Action>) {
    let me = app.state.me().cloned().unwrap_or_default();
    let name = if app.state.persona_name.is_empty() { &app.state.account } else { &app.state.persona_name };
    let short: String = if name.chars().count() > 16 { name.chars().take(15).chain(std::iter::once('…')).collect() } else { name.clone() };
    let color = status_color(t, &me);
    // Painted as one fixed-size widget so it behaves inside right-to-left layouts.
    // NEVKO's account button: avatar and name in one filled 32 px control,
    // the name in the status color (blue online, green in game).
    let name_color = if me.in_game() { t.in_game } else if me.state == PersonaState::Offline { t.text } else { color };
    let name_g = ui.painter().layout_no_wrap(short, w::font_medium(13.5), name_color);
    let (rect, resp) = ui.allocate_exact_size(vec2(6.0 + 24.0 + 8.0 + name_g.size().x + 28.0, CTRL), Sense::click());
    let hover = ui.ctx().animate_bool_with_time(resp.id, resp.hovered(), 0.1);
    ui.painter().rect_filled(rect, t.corner_small(), w::mix(t.raised, t.surface_hover, hover));
    let mut child = ui.new_child(egui::UiBuilder::new().max_rect(Rect::from_min_size(rect.min + vec2(6.0, (CTRL - 24.0) / 2.0), vec2(24.0, 24.0))));
    // Square, rounded like the rest of the controls.
    w::avatar_rounded(&mut child, t, &art::avatar(me.avatar_hash.as_deref()), 24.0, Some(color), t.corner_small());
    let p = ui.painter();
    p.galley(egui::pos2(rect.left() + 38.0, rect.center().y - name_g.size().y / 2.0), name_g, name_color);
    p.text(rect.right_center() - vec2(12.0, 0.0), Align2::CENTER_CENTER, icons::CARET_DOWN, w::font(12.0), t.text_faint);
    let resp = resp.on_hover_cursor(egui::CursorIcon::PointingHand);
    account_menu_popup(app, &resp, t, &me, actions);
}

pub(crate) fn account_menu_popup(app: &mut SteamApp, resp: &egui::Response, t: &Theme, me: &Persona, actions: &mut Vec<Action>) {
    egui::Popup::menu(resp).width(250.0).show(|ui| {
        // Every row is a full-width menu row: icon column at 18 px, text at
        // 36 px, same height and hover, so nothing sits against the border.
        ui.spacing_mut().item_spacing.y = 2.0;
        crate::offline_ui::account_items(app, ui, t);
        ui.add_space(4.0);
        ui.horizontal(|ui| {
            ui.add_space(10.0);
            ui.label(w::caption_text(t, t!("Status")));
        });
        ui.add_space(2.0);
        for s in PersonaState::SELECTABLE {
            // A colored dot per status, like Steam's.
            let on = me.state == s;
            let (r, resp) = ui.allocate_exact_size(vec2(ui.available_width(), 32.0), Sense::click());
            if on || resp.hovered() {
                ui.painter().rect_filled(r, t.corner_small(), if on { w::active_fill(t) } else { t.surface_hover });
            }
            let probe = model::Persona { state: s, ..Default::default() };
            ui.painter().circle_filled(egui::pos2(r.left() + 18.0, r.center().y), 4.5, status_color(t, &probe));
            ui.painter().text(egui::pos2(r.left() + 36.0, r.center().y), Align2::LEFT_CENTER, i18n::tr_dyn(s.label()), w::font_medium(13.5), if on { t.text } else { w::mix(t.text_dim, t.text, 0.6) });
            if on {
                ui.painter().text(egui::pos2(r.right() - 14.0, r.center().y), Align2::RIGHT_CENTER, icons::CHECK, w::font(13.0), t.accent);
            }
            if resp.on_hover_cursor(egui::CursorIcon::PointingHand).clicked() {
                app.client.send(Command::SetPersonaState(s));
            }
        }
        ui.separator();
        if w::menu_item(ui, t, icons::USER_CIRCLE, t!("View profile")).clicked() {
            actions.push(Action::Navigate { page: "profile".into(), arg: None });
        }
        if w::menu_item(ui, t, icons::GEAR, t!("Settings")).clicked() {
            actions.push(Action::Navigate { page: "settings".into(), arg: None });
        }
        ui.separator();
        if w::menu_item(ui, t, icons::SIGN_OUT, t!("Sign out of account")).clicked() {
            app.client.send(Command::LogOut);
        }
        if w::menu_item_tinted(ui, t, icons::POWER, t!("Exit Aurora"), Some(t.danger)).clicked() {
            app.request_exit(ui.ctx());
        }
    });
}

fn status_bar(app: &mut SteamApp, ui: &mut Ui, t: &Theme) {
    let bar = ui.max_rect();
    // Left: offline note and the game being played.
    ui.horizontal_centered(|ui| {
        crate::offline_ui::status_note(app, ui, t);
        if let Some(id) = app.state.playing {
            let name = app.state.app_name(id).unwrap_or("").to_string();
            ui.label(w::text(format!("{}  {}", icons::CONTROLLER, t!("Playing {game}", game = name)), 12.5, t.in_game));
            if cfg!(windows) {
                ui.label(w::text(t!("· Shift+Tab opens the overlay"), 12.5, t.text_faint));
            }
        }
        ui.with_layout(Layout::right_to_left(Align::Center), |ui| {
            let label = format!("{}  {}", icons::CHATS, t!("Friends & Chat"));
            let r = ui.add(egui::Label::new(w::text_medium(label, 12.5, t.text_dim)).sense(Sense::click()));
            if r.on_hover_cursor(egui::CursorIcon::PointingHand).clicked() {
                app.friends_window.open = !app.friends_window.open;
            }
        });
    });
    downloads_control(app, ui, t, bar);
}

/// Center of the bottom bar, like Steam's "Manage downloads": the game
/// downloading and a thin progress line, nothing else. Numbers (percent,
/// queue) are in the tooltip and on the Downloads page.
pub(crate) fn downloads_control(app: &mut SteamApp, ui: &mut Ui, t: &Theme, bar: Rect) {
    let active = app.state.downloads.values().find(|d| !matches!(d.phase, model::DownloadPhase::Done | model::DownloadPhase::Failed)).cloned();
    let waiting = app.state.download_queue.iter().filter(|id| active.as_ref().is_none_or(|d| d.app_id != **id)).count();
    let progress = active.as_ref().map(|d| (d.disk_done as f64 / d.disk_total.max(1) as f64).clamp(0.0, 1.0) as f32);
    let label = match &active {
        Some(d) => app.state.app_name(d.app_id).unwrap_or("").to_string(),
        None => t!("Downloads").to_string(),
    };
    let paused = active.as_ref().is_some_and(|d| d.paused);
    let galley = ui.painter().layout_no_wrap(label, w::font_medium(12.5), egui::Color32::PLACEHOLDER);
    let width = (galley.size().x + 52.0).clamp(160.0, 340.0);
    let rect = Rect::from_center_size(bar.center(), vec2(width, 26.0));
    let resp = ui.interact(rect, ui.scope_id().with("downloads-control"), Sense::click());
    let hover = ui.ctx().animate_bool_with_time(resp.id, resp.hovered(), 0.1);
    let p = ui.painter();
    if hover > 0.0 {
        p.rect_filled(rect, t.corner_small(), t.raised.gamma_multiply(hover));
    }
    let busy = active.is_some();
    let icon_c = if busy && !paused { t.accent } else { t.text_dim };
    let text_c = if busy { w::mix(t.text_dim, t.text, 0.6 + 0.4 * hover) } else { w::mix(t.text_dim, t.text, hover) };
    // Icon and name centered together; long names are cut to fit.
    let text_w = galley.size().x.min(width - 44.0);
    let left = rect.center().x - (text_w + 22.0) / 2.0;
    let y = rect.center().y - if busy { 1.5 } else { 0.0 };
    p.text(egui::pos2(left + 7.0, y), Align2::CENTER_CENTER, if paused { "\u{E39E}" } else { icons::DOWNLOAD }, w::font(13.0), icon_c);
    let clip = Rect::from_min_max(egui::pos2(left + 22.0, rect.top()), egui::pos2(left + 22.0 + text_w, rect.bottom()));
    ui.painter_at(clip).galley(egui::pos2(left + 22.0, y - galley.size().y / 2.0), galley, text_c);
    if let Some(f) = progress {
        let track = Rect::from_min_size(egui::pos2(rect.left() + 10.0, rect.bottom() - 4.0), vec2(rect.width() - 20.0, 2.0));
        p.rect_filled(track, egui::CornerRadius::same(1), t.raised);
        let fill = Rect::from_min_size(track.min, vec2(track.width() * f, track.height()));
        p.rect_filled(fill, egui::CornerRadius::same(1), if paused { t.text_faint } else { t.accent });
    }
    let tip = match (&active, progress) {
        (Some(_), Some(f)) => {
            let pct = format!("{:.0}", f * 100.0);
            let state = if paused { t!("Paused at {pct}%", pct = pct) } else { t!("{pct}% downloaded", pct = pct) };
            if waiting > 0 { format!("{state} · {}", t!("+{n} queued", n = waiting)) } else { state }
        }
        _ if waiting > 0 => t!("{n} queued", n = waiting),
        _ => t!("Nothing in the queue").to_string(),
    };
    if resp.on_hover_text(tip).on_hover_cursor(egui::CursorIcon::PointingHand).clicked() {
        app.navigate("downloads".into(), None);
    }
}

pub fn status_color(t: &Theme, p: &Persona) -> egui::Color32 {
    if p.in_game() {
        return t.in_game;
    }
    match p.state {
        PersonaState::Offline | PersonaState::Invisible => t.text_faint,
        PersonaState::Busy => t.danger,
        PersonaState::Away | PersonaState::Snooze => t.away,
        _ => t.online,
    }
}

/// What a friend can be invited to while you play, like Steam's friend menu.
#[derive(Clone, Debug, PartialEq)]
pub enum Invite {
    /// The game's matchmaking lobby.
    Lobby(model::AppId, u64),
    /// The game itself, with its join string (rich presence `connect`).
    Game(model::AppId, String),
    /// Just start the game (it says nothing about how to join).
    Play(model::AppId),
}

/// The invites that make sense right now, best first, with their labels.
pub fn invite_options(state: &client::State) -> Vec<(Invite, String)> {
    let Some(app) = state.playing else { return Vec::new() };
    let mut out = Vec::new();
    if let Some(lobby) = state.game_lobby {
        out.push((Invite::Lobby(app, lobby), t!("Invite to lobby").to_string()));
    }
    if let Some(connect) = state.game_connect.clone() {
        out.push((Invite::Game(app, connect), t!("Invite to game").to_string()));
    }
    if out.is_empty() {
        let game = state.app_name(app).unwrap_or_default().to_string();
        out.push((Invite::Play(app), t!("Invite to play {game}", game = game).to_string()));
    }
    out
}

/// Send an invite. Lobby invites are Steam's own (every client shows them).
/// The others are a `steam://run` link with the join arguments: Steam
/// strips invite markup that doesn't come from its own client, and any Steam
/// client opens the link. Ours shows it as an invite card.
pub fn send_invite(app: &mut SteamApp, to: &Persona, invite: Invite, actions: &mut Vec<Action>) {
    let friend = to.steam_id;
    let game = |id| app.state.app_name(id).unwrap_or_default().to_string();
    match invite {
        Invite::Lobby(app_id, lobby) => app.client.send(Command::InviteToLobby { app_id, lobby, friend }),
        Invite::Game(app_id, connect) => {
            let link = model::GameInvite { app_id, connect }.link();
            let text = t!("Join me in {game}! {link}", game = game(app_id), link = link).to_string();
            app.client.send(Command::SendMessage { to: friend, text });
        }
        Invite::Play(app_id) => {
            let link = model::GameInvite { app_id, connect: String::new() }.link();
            let text = t!("Play {game} with me! {link}", game = game(app_id), link = link).to_string();
            app.client.send(Command::SendMessage { to: friend, text });
        }
    }
    let offline = to.state == PersonaState::Offline;
    actions.push(Action::Toast(if offline { t!("Invite sent. {name} will see it when they come online.", name = to.name) } else { t!("Invite sent to {name}", name = to.name) }));
}

/// Where a game opens: your library if you own it, else its store page.
pub fn game_page(state: &client::State, app: model::AppId) -> Action {
    let owned = state.apps.as_ref().is_some_and(|a| a.contains_key(&app));
    if owned {
        Action::Navigate { page: "library".into(), arg: Some(app.to_string()) }
    } else {
        Action::Navigate { page: "store".into(), arg: Some(format!("app:{app}")) }
    }
}

pub fn status_text(state: &client::State, p: &Persona) -> String {
    let game = p.game_app_id.and_then(|id| state.app_name(id).map(String::from)).or_else(|| p.game_name.clone());
    if p.in_game() {
        let mut line = t!("Playing {game}", game = game.unwrap_or_else(|| t!("a game").to_string()));
        if let Some(rich) = &p.rich_status {
            line = format!("{line} · {rich}");
        }
        return line;
    }
    match (p.state, p.last_logoff) {
        (PersonaState::Offline, Some(t)) if t > 0 => t!("Last online {when}", when = ui::ago(t)),
        (s, _) => i18n::tr_dyn(s.label()),
    }
}

pub(crate) fn friends_drawer(app: &mut SteamApp, ui: &mut Ui, t: &Theme, actions: &mut Vec<Action>) {
    let list = app.state.friend_list();
    let online = list.iter().filter(|p| p.state != PersonaState::Offline).count();
    ui.horizontal(|ui| {
        ui.label(w::text_bold(t!("Friends"), 17.0, t.text));
        ui.label(w::text(t!("{n} online", n = online), 13.0, t.text_dim));
        ui.with_layout(Layout::right_to_left(Align::Center), |ui| {
            if w::icon_button(ui, t, icons::X, t!("Close"), false, 0).clicked() {
                app.prefs.friends_open = false;
            }
        });
    });
    ui.add_space(6.0);
    let width = ui.available_width() - 4.0;
    w::search(ui, t, &mut app.friend_filter, t!("Search friends"), width);
    ui.add_space(10.0);
    crate::offline_ui::friends_notice(app, ui, t);
    friend_list(app, ui, t, actions, &list, None);
}

/// Grouped friend list, shared by the drawer and the Community page.
/// Returns the friend that was clicked.
pub fn friend_list(app: &mut SteamApp, ui: &mut Ui, t: &Theme, actions: &mut Vec<Action>, list: &[Persona], selected: Option<u64>) -> Option<u64> {
    let filter = app.friend_filter.to_lowercase();
    let mut favorites = Vec::new();
    let mut in_game = Vec::new();
    let mut online = Vec::new();
    let mut offline = Vec::new();
    for p in list {
        if !filter.is_empty()
            && !model::contains_ignore_case(&p.name, &filter)
            && !p.nickname.as_ref().is_some_and(|n| model::contains_ignore_case(n, &filter))
        {
            continue;
        }
        if app.prefs.favorite_friends.contains(&p.steam_id) {
            favorites.push(p);
            continue;
        }
        if p.state == PersonaState::Offline {
            offline.push(p);
        } else if !p.in_game() {
            online.push(p);
        }
        if p.in_game() {
            in_game.push(p);
        }
    }
    let mut clicked = None;

    egui::ScrollArea::vertical().id_salt("friend-list").auto_shrink([false, false]).show(ui, |ui| {
        if list.is_empty() {
            ui.label(w::text(t!("Loading friends…"), 13.0, t.text_faint));
        }
        for (title, group) in [(t!("Favorites"), &favorites), (t!("In game"), &in_game), (t!("Online"), &online), (t!("Offline"), &offline)] {
            if group.is_empty() {
                continue;
            }
            ui.add_space(4.0);
            w::caption(ui, t, &format!("{title} — {}", group.len()));
            ui.add_space(2.0);
            for p in group.iter() {
                if friend_row(app, ui, t, p, actions, selected == Some(p.steam_id)) {
                    clicked = Some(p.steam_id);
                }
            }
            ui.add_space(8.0);
        }
    });
    if let Some(id) = clicked {
        if selected.is_none() {
            actions.push(Action::OpenChat(id));
        }
    }
    clicked
}

fn friend_row(app: &mut SteamApp, ui: &mut Ui, t: &Theme, p: &Persona, actions: &mut Vec<Action>, selected: bool) -> bool {
    let (rect, resp) = ui.allocate_exact_size(vec2(ui.available_width(), 48.0), Sense::click());
    if !ui.is_rect_visible(rect) && !resp.context_menu_opened() {
        return false;
    }
    let offline = p.state == PersonaState::Offline;
    let color = status_color(t, p);
    let name = p.display_name();
    let status = if app.state.is_typing(p.steam_id) { t!("typing…").to_string() } else { status_text(&app.state, p) };
    let unread = app.state.unread.get(&p.steam_id).copied().unwrap_or(0);

    // The whole row lights up wherever the pointer is in it (also over the
    // game link inside it), and clearly enough to see on the dark list.
    let pointer_in = ui.rect_contains_pointer(rect) || resp.context_menu_opened();
    let hover = ui.ctx().animate_bool_with_time(resp.id, pointer_in, 0.1);
    if selected {
        ui.painter().rect_filled(rect, t.corner_small(), t.accent.gamma_multiply(0.18));
    } else if hover > 0.0 {
        ui.painter().rect_filled(rect, t.corner_small(), w::mix(egui::Color32::TRANSPARENT, t.surface_hover, hover));
    }
    // Leave room on the right for the unread badge, so text never runs under it.
    let inner = rect.shrink2(vec2(6.0, 6.0));
    let inner = if unread > 0 { Rect::from_min_max(inner.min, egui::pos2(inner.right() - 26.0, inner.bottom())) } else { inner };
    let mut child = ui.new_child(egui::UiBuilder::new().max_rect(inner).layout(Layout::left_to_right(Align::Center)));
    w::avatar(&mut child, t, &art::avatar(p.avatar_hash.as_deref()), 36.0, Some(color));
    child.add_space(4.0);
    child.vertical(|ui| {
        ui.spacing_mut().item_spacing.y = 1.0;
        ui.add_space(1.0);
        let nc = if offline { t.text_faint } else if p.in_game() { t.in_game } else { t.text };
        ui.add(egui::Label::new(w::text_medium(&name, 14.0, nc)).truncate());
        // The game is a link to its page.
        let r = ui.add(egui::Label::new(w::text(&status, 12.0, if offline { t.text_faint } else if p.in_game() { t.in_game } else { t.text_dim })).truncate().sense(Sense::click()));
        if let Some(target) = p.game_app_id.filter(|_| p.in_game()) {
            // The game is a link of its own: underlined while the pointer is
            // on it, so it's clear a click opens the game, not the chat.
            if r.hovered() {
                ui.painter().hline(r.rect.x_range(), r.rect.bottom() - 1.0, Stroke::new(1.0, t.in_game));
            }
            let r = r.on_hover_cursor(egui::CursorIcon::PointingHand).on_hover_text(t!("Open the game's page"));
            if r.clicked() {
                actions.push(game_page(&app.state, target));
            }
        }
    });
    if unread > 0 {
        let c = rect.right_center() - vec2(16.0, 0.0);
        ui.painter().circle_filled(c, 9.0, t.accent);
        ui.painter().text(c, Align2::CENTER_CENTER, unread.min(99).to_string(), w::font_bold(10.0), t.on_accent);
    }

    let persona = p;
    let vp = ui.ctx().viewport_id();
    resp.context_menu(|ui| {
        ui.label(w::text_semibold(&name, 14.0, t.text));
        if persona.nickname.is_some() && !persona.name.is_empty() {
            ui.label(w::text(&persona.name, 12.0, t.text_dim));
        }
        ui.separator();
        if ui.button(format!("{}  {}", icons::CHAT, t!("Send a message"))).clicked() {
            actions.push(Action::OpenChat(persona.steam_id));
            ui.close();
        }
        // In a game: join them, see their server, watch their broadcast, like Steam.
        if let Some(game) = persona.game_app_id.filter(|_| persona.in_game()) {
            let game_name = app.state.app_name(game).map(String::from).unwrap_or_else(|| t!("the game").to_string());
            ui.separator();
            if let Some(args) = persona.join_args() {
                if ui.button(format!("{}  {}", icons::PLAY, t!("Join game"))).on_hover_text(&args).clicked() {
                    match app.local.games.iter().find(|g| g.app_id == game) {
                        Some(g) => {
                            app.client.send(Command::Launch { app_id: game, install_dir: g.install_dir.clone(), args: client::LaunchArgs::connect(args) });
                            actions.push(Action::Toast(t!("Joining {name} in {game}…", name = persona.display_name(), game = game_name)));
                        }
                        None => actions.push(Action::Toast(t!("Install {game} to join {name}.", game = game_name, name = persona.display_name()))),
                    }
                    ui.close();
                }
            }
            if let Some(addr) = persona.server.clone() {
                if ui.button(format!("{}  {}", icons::INFO, t!("View server info"))).clicked() {
                    app.client.send(Command::QueryServer { addr: addr.clone() });
                    app.friend_dialog = Some((vp, FriendDialog::Server { name: persona.display_name(), game, addr }));
                    ui.close();
                }
            }
            if persona.broadcasting && ui.button(format!("{}  {}", icons::EYE, t!("Watch game"))).on_hover_text(t!("Opens their Steam broadcast")).clicked() {
                crate::links::open_http(&format!("https://steamcommunity.com/broadcast/watch/{}", persona.steam_id));
                ui.close();
            }
            if ui.button(format!("{}  {}", icons::CONTROLLER, t!("View game info"))).clicked() {
                actions.push(game_page(&app.state, game));
                ui.close();
            }
        }
        // While you're in a game: invite them to it, its lobby or its match.
        for (invite, label) in invite_options(&app.state) {
            if ui.button(format!("{}  {}", icons::USER_PLUS, label)).clicked() {
                send_invite(app, persona, invite, actions);
                ui.close();
            }
        }
        ui.separator();
        if ui.button(format!("{}  {}", icons::USER_CIRCLE, t!("View profile"))).clicked() {
            actions.push(Action::Navigate { page: "profile".into(), arg: Some(persona.steam_id.to_string()) });
            ui.close();
        }
        if ui.button(format!("{}  {}", icons::BACKPACK, t!("Inventory"))).clicked() {
            actions.push(Action::Navigate { page: "inventory".into(), arg: Some(persona.steam_id.to_string()) });
            ui.close();
        }
        let invitable_groups: Vec<_> = app
            .state
            .group_list()
            .into_iter()
            .filter(|g| g.effective_permissions(model::steam_id_to_account(app.state.steam_id)).can_invite)
            .map(|g| (g.id, app.state.group_display_name(g), g.default_text_channel_id()))
            .collect();
        ui.menu_button(format!("{}  {}", icons::USER_PLUS, t!("Invite to group chat")), |ui| {
            for (gid, gname, cid) in &invitable_groups {
                if ui.button(gname).clicked() {
                    app.client.send(Command::InviteToChatGroup {
                        group_id: *gid,
                        chat_id: *cid,
                        friend: persona.steam_id,
                    });
                    actions.push(Action::Toast(t!("Invited {name} to {group}", name = persona.display_name(), group = gname.clone())));
                    ui.close();
                }
            }
            if !invitable_groups.is_empty() {
                ui.separator();
            }
            if ui.button(format!("{}  {}", icons::PLUS, t!("New group chat with {name}", name = persona.display_name()))).clicked() {
                app.client.send(Command::CreateChatGroup {
                    name: None,
                    invitees: vec![persona.steam_id],
                });
                ui.close();
            }
        });
        ui.menu_button(format!("{}  {}", icons::GEAR, t!("Manage")), |ui| {
            if ui.button(t!("Add nickname…")).clicked() {
                app.friend_dialog = Some((vp, FriendDialog::Nickname { id: persona.steam_id, text: persona.nickname.clone().unwrap_or_default() }));
                ui.close();
            }
            if persona.nickname.is_some() && ui.button(t!("Remove nickname")).clicked() {
                app.client.send(Command::SetNickname { steam_id: persona.steam_id, nickname: None });
                ui.close();
            }
            let is_fav = app.prefs.favorite_friends.contains(&persona.steam_id);
            if ui.button(if is_fav { t!("Remove from Favorites") } else { t!("Add to Favorites") }).clicked() {
                if is_fav {
                    app.prefs.favorite_friends.retain(|f| *f != persona.steam_id);
                } else {
                    app.prefs.favorite_friends.push(persona.steam_id);
                }
                ui.close();
            }
            if ui.button(t!("Copy SteamID")).clicked() {
                ui.ctx().copy_text(persona.steam_id.to_string());
                ui.close();
            }
            ui.separator();
            if ui.button(w::text(t!("Remove friend…"), 13.5, t.danger)).clicked() {
                app.friend_dialog = Some((vp, FriendDialog::Remove { id: persona.steam_id, name: persona.display_name() }));
                ui.close();
            }
            if ui.button(w::text(t!("Block all communication…"), 13.5, t.danger)).clicked() {
                app.friend_dialog = Some((vp, FriendDialog::Block { id: persona.steam_id, name: persona.display_name() }));
                ui.close();
            }
        });
        let env = Env {
            state: &app.state,
            local: &app.local,
            theme: &app.prefs.theme,
            zoom: app.prefs.zoom,
            client: &app.client,
            actions: &mut *actions,
        };
        app.registry.friend_menu(&persona, ui, env);
    });
    resp.on_hover_cursor(egui::CursorIcon::PointingHand).clicked()
}

pub fn toasts(app: &mut SteamApp, ctx: &egui::Context, keyboard_inset: f32) {
    let now = ctx.input(|i| i.time);
    app.toasts.retain(|t| t.until > now);
    if app.toasts.is_empty() {
        return;
    }
    let t = &app.prefs.theme;
    egui::Area::new(egui::Id::unique("toasts"))
        .anchor(Align2::RIGHT_BOTTOM, vec2(-20.0, -48.0_f32.max(keyboard_inset + 12.0)))
        .order(egui::Order::Foreground)
        .show(ctx, |ui| {
            for toast in app.toasts.iter().rev().take(4) {
                let card = egui::Frame::new()
                    .fill(t.raised)
                    .stroke(Stroke::new(1.0, t.border))
                    .corner_radius(t.corner())
                    .shadow(egui::Shadow { offset: [0, 8], blur: 24, spread: 0, color: egui::Color32::from_black_alpha(120) })
                    .inner_margin(Margin { left: 22, right: 16, top: 12, bottom: 12 })
                    .show(ui, |ui| {
                        ui.set_width(320.0);
                        ui.label(w::text_semibold(&toast.title, 14.0, t.text));
                        if !toast.body.is_empty() {
                            ui.add(egui::Label::new(w::text(&toast.body, 13.0, t.text_dim)).wrap());
                        }
                    });
                // Accent bar inside the card, sized to it once the text is laid out.
                let r = card.response.rect;
                let bar = Rect::from_min_max(egui::pos2(r.left() + 9.0, r.top() + 10.0), egui::pos2(r.left() + 12.0, r.bottom() - 10.0));
                ui.painter().rect_filled(bar, egui::CornerRadius::same(2), t.accent);
                ui.add_space(8.0);
            }
        });
    ctx.request_repaint_after(std::time::Duration::from_millis(200));
}

/// Dialogs opened from a friend's menu.
#[derive(Clone)]
pub enum FriendDialog {
    Nickname { id: model::SteamId, text: String },
    Server { name: String, game: model::AppId, addr: String },
    Remove { id: model::SteamId, name: String },
    Block { id: model::SteamId, name: String },
}

/// Draw the open friend dialog, in the window it was opened from.
pub fn friend_dialogs(app: &mut SteamApp, ctx: &egui::Context) {
    let Some((vp, dialog)) = app.friend_dialog.clone() else { return };
    if vp != ctx.viewport_id() {
        return;
    }
    let t = app.prefs.theme.clone();
    let mut close = false;
    let mut keep = dialog.clone();
    // Fits narrow windows too (the Friends window can be ~300 px wide).
    let width = (ctx.content_rect().width() - 64.0).clamp(220.0, 400.0);
    ui::widgets::modal(ctx, egui::Id::unique("friend-dialog")).show(ctx, |ui| {
        ui.set_width(width);
        match &mut keep {
            FriendDialog::Nickname { id, text } => {
                ui.label(w::text_bold(t!("Add a nickname"), 17.0, t.text));
                ui.label(w::text(t!("Only you see it. Steam keeps it on all your devices."), 12.5, t.text_dim));
                ui.add_space(8.0);
                let r = ui.add(egui::TextEdit::singleline(text).desired_width(f32::INFINITY).hint_text(t!("Nickname"))).with_edit_menu();
                w::autofocus(&r, true);
                ui.add_space(10.0);
                ui.horizontal(|ui| {
                    let save = w::button(ui, &t, None, t!("Save"), w::Kind::Primary, w::Size::Sm).clicked() || ui.input(|i| i.key_pressed(egui::Key::Enter));
                    if save {
                        let nick = text.trim().to_string();
                        app.client.send(Command::SetNickname { steam_id: *id, nickname: (!nick.is_empty()).then_some(nick) });
                        close = true;
                    }
                    if w::button(ui, &t, None, t!("Cancel"), w::Kind::Ghost, w::Size::Sm).clicked() {
                        close = true;
                    }
                });
            }
            FriendDialog::Server { name, game, addr } => {
                let game_name = app.state.app_name(*game).unwrap_or_default().to_string();
                ui.add(egui::Label::new(w::text_bold(t!("{name}'s server", name = name), 17.0, t.text)).wrap());
                ui.label(w::text(&game_name, 12.5, t.text_dim));
                ui.add_space(8.0);
                match app.state.servers.get(addr.as_str()) {
                    None => {
                        ui.label(w::text(t!("Asking the server…"), 13.0, t.text_dim));
                    }
                    Some(Err(e)) => {
                        ui.label(w::text(e, 13.0, t.text_dim));
                    }
                    Some(Ok(s)) => {
                        egui::Grid::new("server-info").num_columns(2).spacing(vec2(14.0, 6.0)).show(ui, |ui| {
                            for (k, v) in [
                                (t!("Name"), s.name.clone()),
                                (t!("Map"), s.map.clone()),
                                (t!("Players"), format!("{} / {}{}", s.players, s.max_players, if s.bots > 0 { format!(" (+{} bots)", s.bots) } else { String::new() })),
                                (t!("Ping"), format!("{} ms", s.ping_ms)),
                                (t!("Anti-cheat"), if s.vac { "VAC".to_string() } else { "—".to_string() }),
                                (t!("Password"), if s.password { t!("Yes").to_string() } else { t!("No").to_string() }),
                            ] {
                                ui.label(w::caption_text(&t, k));
                                ui.add(egui::Label::new(w::text(v, 13.0, t.text)).truncate());
                                ui.end_row();
                            }
                        });
                    }
                }
                ui.add_space(6.0);
                ui.horizontal(|ui| {
                    ui.label(w::text(addr.as_str(), 12.5, t.text_dim));
                    if ui.small_button(t!("Copy")).clicked() {
                        ui.ctx().copy_text(addr.clone());
                    }
                });
                ui.add_space(10.0);
                ui.horizontal(|ui| {
                    if let Some(g) = app.local.games.iter().find(|g| g.app_id == *game) {
                        if w::button(ui, &t, Some(icons::PLAY), t!("Join"), w::Kind::Primary, w::Size::Sm).clicked() {
                            app.client.send(Command::Launch { app_id: *game, install_dir: g.install_dir.clone(), args: client::LaunchArgs::connect(format!("+connect {addr}")) });
                            close = true;
                        }
                    }
                    if w::button(ui, &t, Some(icons::REFRESH), t!("Refresh"), w::Kind::Secondary, w::Size::Sm).clicked() {
                        app.client.send(Command::QueryServer { addr: addr.clone() });
                    }
                    if w::button(ui, &t, None, t!("Close"), w::Kind::Ghost, w::Size::Sm).clicked() {
                        close = true;
                    }
                });
            }
            FriendDialog::Remove { id, name } => {
                ui.add(egui::Label::new(w::text_bold(t!("Remove {name} from your friends?", name = name), 17.0, t.text)).wrap());
                ui.add_space(12.0);
                ui.horizontal(|ui| {
                    if w::button(ui, &t, None, t!("Remove friend"), w::Kind::Danger, w::Size::Sm).clicked() {
                        app.client.send(Command::RemoveFriend { steam_id: *id });
                        close = true;
                    }
                    if w::button(ui, &t, None, t!("Cancel"), w::Kind::Ghost, w::Size::Sm).clicked() {
                        close = true;
                    }
                });
            }
            FriendDialog::Block { id, name } => {
                ui.add(egui::Label::new(w::text_bold(t!("Block all communication with {name}?", name = name), 17.0, t.text)).wrap());
                ui.add(egui::Label::new(w::text(t!("They'll be removed from your friends and can't message you or invite you."), 12.5, t.text_dim)).wrap());
                ui.add_space(12.0);
                ui.horizontal(|ui| {
                    if w::button(ui, &t, None, t!("Block"), w::Kind::Danger, w::Size::Sm).clicked() {
                        app.client.send(Command::BlockFriend { steam_id: *id, block: true });
                        close = true;
                    }
                    if w::button(ui, &t, None, t!("Cancel"), w::Kind::Ghost, w::Size::Sm).clicked() {
                        close = true;
                    }
                });
            }
        }
    });
    if close {
        app.friend_dialog = None;
    } else if let Some((_, d)) = &mut app.friend_dialog {
        *d = keep;
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn test_state(me: u64) -> client::State {
        let mut s = client::State::default();
        s.steam_id = me;
        s
    }

    fn assert_nav(target: NotificationTarget, expected_page: &str, expected_arg: Option<&str>) {
        match target {
            NotificationTarget::Action(Action::Navigate { page, arg }) => {
                assert_eq!(page, expected_page);
                assert_eq!(arg.as_deref(), expected_arg);
            }
            other => panic!("expected Navigate({expected_page:?}, {expected_arg:?}), got {other:?}"),
        }
    }

    #[test]
    fn family_notifications_route_natively_to_settings_family() {
        let state = test_state(76561197960265729);
        for kind in 15..=21 {
            let n = model::Notification { kind, ..Default::default() };
            assert_nav(notification_target(&state, &n), "settings", Some("family"));
            let mut actions = Vec::new();
            open_notification(&state, &n, &mut actions);
            assert_eq!(actions.len(), 1);
            match &actions[0] {
                Action::Navigate { page, arg } => {
                    assert_eq!(page, "settings");
                    assert_eq!(arg.as_deref(), Some("family"));
                }
                other => panic!("expected Navigate, got {other:?}"),
            }
        }
    }

    #[test]
    fn native_notifications_route_to_expected_pages_and_dialogs() {
        let me = 76561197960265729;
        let friend = 76561197960265828;
        let mut state = test_state(me);
        state.friends.insert(friend, model::Relationship::Friend);
        let requester = 76561197960265829;
        state.friends.insert(requester, model::Relationship::RequestRecipient);

        // Kind 3: profile comment vs discussion reply vs thread URL.
        assert_nav(
            notification_target(&state, &model::Notification { kind: 3, from: Some(me), ..Default::default() }),
            "profile",
            Some(&me.to_string()),
        );
        assert_nav(
            notification_target(&state, &model::Notification { kind: 3, from: Some(friend), app_id: Some(570), ..Default::default() }),
            "discussions",
            Some("570"),
        );
        assert_nav(
            notification_target(
                &state,
                &model::Notification {
                    kind: 3,
                    from: Some(friend),
                    data: r#"{"url":"https://steamcommunity.com/app/570/discussions/0/123/"}"#.into(),
                    ..Default::default()
                },
            ),
            "thread",
            Some("https://steamcommunity.com/app/570/discussions/0/123/"),
        );

        // Kind 4: inventory item (handles None and Some(0) cleanly).
        assert_nav(
            notification_target(&state, &model::Notification { kind: 4, app_id: Some(730), ..Default::default() }),
            "inventory",
            Some(&format!("{me}/730")),
        );
        assert_nav(
            notification_target(&state, &model::Notification { kind: 4, app_id: Some(0), ..Default::default() }),
            "inventory",
            Some(&format!("{me}/753")),
        );

        // Kind 5: friend request summary or pending invite -> community/invites; accepted friend -> profile.
        assert_nav(
            notification_target(&state, &model::Notification { kind: 5, ..Default::default() }),
            "community",
            Some("invites"),
        );
        assert_nav(
            notification_target(&state, &model::Notification { kind: 5, from: Some(requester), ..Default::default() }),
            "community",
            Some("invites"),
        );
        assert_nav(
            notification_target(&state, &model::Notification { kind: 5, from: Some(friend), ..Default::default() }),
            "profile",
            Some(&friend.to_string()),
        );

        // Kind 6: store sale.
        assert_nav(notification_target(&state, &model::Notification { kind: 6, ..Default::default() }), "store", None);

        // Kind 7, 12, 22, 24: library game page.
        for kind in [7, 12, 22, 24] {
            assert_nav(
                notification_target(&state, &model::Notification { kind, app_id: Some(1145360), ..Default::default() }),
                "library",
                Some("1145360"),
            );
        }

        // Kind 8: wishlist sale (specific app or wishlist page).
        assert_nav(
            notification_target(&state, &model::Notification { kind: 8, app_id: Some(1145360), ..Default::default() }),
            "store",
            Some("app:1145360"),
        );
        assert_nav(
            notification_target(&state, &model::Notification { kind: 8, ..Default::default() }),
            "store",
            Some("wishlist"),
        );

        // Kind 11: support ticket.
        assert_nav(
            notification_target(&state, &model::Notification { kind: 11, ticket: Some("HTDEMO".into()), ..Default::default() }),
            "support",
            Some("ticket:HTDEMO"),
        );

        // Kind 13: chat message (specific friend opens chat; missing sender opens community).
        match notification_target(&state, &model::Notification { kind: 13, from: Some(friend), ..Default::default() }) {
            NotificationTarget::Action(Action::OpenChat(id)) => assert_eq!(id, friend),
            other => panic!("expected OpenChat, got {other:?}"),
        }
        assert_nav(notification_target(&state, &model::Notification { kind: 13, ..Default::default() }), "community", None);

        // Kind 25, 26: Steam Guard sign-in / mobile confirmation alerts -> settings/security.
        for kind in [25, 26] {
            assert_nav(
                notification_target(&state, &model::Notification { kind, ..Default::default() }),
                "settings",
                Some("security"),
            );
        }

        // Kind 27: game event -> hub news (or store when app missing).
        assert_nav(
            notification_target(&state, &model::Notification { kind: 27, app_id: Some(570), ..Default::default() }),
            "hub",
            Some("570/news"),
        );
        assert_nav(notification_target(&state, &model::Notification { kind: 27, ..Default::default() }), "store", None);

        // Kind 28: playtest invite -> store app page.
        assert_nav(
            notification_target(&state, &model::Notification { kind: 28, app_id: Some(1145360), ..Default::default() }),
            "store",
            Some("app:1145360"),
        );
    }

    #[test]
    fn web_fallback_and_noop_notifications() {
        let me = 76561197960265729;
        let state = test_state(me);

        match notification_target(&state, &model::Notification { kind: 2, ..Default::default() }) {
            NotificationTarget::Web(url) => assert_eq!(url, format!("https://steamcommunity.com/profiles/{me}/inventory/#pending_gifts")),
            other => panic!("expected Web, got {other:?}"),
        }
        for kind in [9, 29] {
            match notification_target(&state, &model::Notification { kind, ..Default::default() }) {
                NotificationTarget::Web(url) => assert_eq!(url, format!("https://steamcommunity.com/profiles/{me}/tradeoffers/")),
                other => panic!("expected Web, got {other:?}"),
            }
        }
        for kind in [1, 10, 14, 23, 30, 999] {
            assert!(matches!(
                notification_target(&state, &model::Notification { kind, ..Default::default() }),
                NotificationTarget::None
            ));
        }
    }
}
