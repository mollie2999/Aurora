//! The right column and Recent Activity as Steam's profile page has them:
//! Level, featured badge, Edit Profile, status, 4 badges, the count links,
//! groups and the top 6 friends by level; recent games with achievements.

use super::{Link, Profile, Section, level_badge, panel};
use client::Command;
use egui::{Align, Align2, Color32, CornerRadius, Layout, Rect, Sense, Stroke, Ui, pos2, vec2};
use extensions::Ctx;
use i18n::t;
use model::{PageFriend, Persona, PersonaState, ProfilePage, RecentGame, SteamId};
use ui::{Theme, icons, widgets as w};

/// Steam's status colors by persona class.
fn status_color(t: &Theme, status: &str) -> Color32 {
    match status {
        "in-game" => t.in_game,
        "online" => t.online,
        _ => t.text_dim,
    }
}

impl Profile {
    /// The name chevron's popup: "This user has also played as", and on your
    /// own profile "Clear Previous Aliases".
    pub(super) fn aliases_menu(&mut self, ui: &mut Ui, t: &Theme, ctx: &mut Ctx, id: SteamId, mine: bool) {
        if !ctx.state.aliases.contains_key(&id) && self.aliases_requested.insert(id) {
            ctx.send(Command::LoadAliases { steam_id: id });
        }
        ui.set_min_width(260.0);
        ui.label(w::text(t!("This user has also played as:"), 12.5, t.text_dim));
        ui.add_space(4.0);
        match ctx.state.aliases.get(&id) {
            None => {
                ui.add(egui::Spinner::new().size(14.0).color(t.accent));
            }
            Some(Err(e)) => {
                ui.label(w::text(i18n::tr_dyn(e), 12.5, t.text_faint));
            }
            Some(Ok(list)) if list.is_empty() => {
                ui.label(w::text(t!("No previous names."), 12.5, t.text_faint));
            }
            Some(Ok(list)) => {
                egui::ScrollArea::vertical().max_height(300.0).show(ui, |ui| {
                    for a in list {
                        ui.horizontal(|ui| {
                            ui.label(w::text(&a.name, 13.5, t.text));
                            ui.with_layout(Layout::right_to_left(Align::Center), |ui| {
                                ui.label(w::text(&a.changed, 11.5, t.text_faint));
                            });
                        });
                    }
                });
            }
        }
        if mine {
            ui.separator();
            if w::button(ui, t, Some(icons::TRASH), t!("Clear Previous Aliases"), w::Kind::Ghost, w::Size::Sm).clicked() {
                self.confirm_clear_aliases = true;
                ui.close();
            }
        }
    }

    /// Clearing the name history can't be undone, so it's confirmed first.
    pub(super) fn clear_aliases_dialog(&mut self, ui: &mut Ui, t: &Theme, ctx: &mut Ctx) {
        if !self.confirm_clear_aliases {
            return;
        }
        let mut done = None;
        if ui.input(|i| i.key_pressed(egui::Key::Escape)) {
            done = Some(false);
        }
        w::modal(ui.ctx(), egui::Id::unique("clear-aliases")).show(ui.ctx(), |ui| {
            ui.set_width(420.0);
            ui.label(w::text_bold(t!("Clear Previous Aliases?"), 18.0, t.text));
            ui.add_space(6.0);
            ui.add(egui::Label::new(w::text(t!("Your previous names will no longer show on your profile. This can't be undone."), 13.5, t.text_dim)).wrap());
            ui.add_space(12.0);
            ui.horizontal(|ui| {
                if w::button(ui, t, Some(icons::TRASH), t!("Clear"), w::Kind::Danger, w::Size::Md).clicked() {
                    done = Some(true);
                }
                if w::button(ui, t, None, t!("Cancel"), w::Kind::Ghost, w::Size::Md).clicked() {
                    done = Some(false);
                }
            });
        });
        match done {
            Some(true) => {
                ctx.send(Command::ClearAliases);
                self.confirm_clear_aliases = false;
            }
            Some(false) => self.confirm_clear_aliases = false,
            None => {}
        }
    }

    /// Ask once for Steam's profile page data; `None` until it arrives.
    pub(super) fn page_data(&mut self, ctx: &mut Ctx, id: SteamId) -> Option<Result<ProfilePage, String>> {
        if !ctx.state.profile_pages.contains_key(&id) && self.page_requested.insert(id) {
            ctx.send(Command::LoadProfilePage { steam_id: id, language: i18n::language().steam_name().into() });
        }
        ctx.state.profile_pages.get(&id).cloned()
    }

    /// The header's right side, as on Steam: Level, the featured badge, the
    /// profile actions (Edit Profile and Copy SteamID side by side), then
    /// "Member since" below them.
    #[allow(clippy::too_many_arguments)]
    pub(super) fn header_side(&mut self, ui: &mut Ui, t: &Theme, ctx: &mut Ctx, id: SteamId, mine: bool, member_since: Option<u64>) {
        ui.spacing_mut().item_spacing.y = 6.0;
        let page = match self.page_data(ctx, id) {
            Some(Ok(p)) => Some(p),
            _ => None,
        };
        // Level.
        let level = page.as_ref().and_then(|p| p.level).or(if mine { ctx.state.level } else { None });
        if let Some(level) = level {
            let r = ui.horizontal(|ui| {
                ui.label(w::text(t!("Level"), 24.0, t.text));
                level_badge(ui, t, level);
            });
            if r.response.interact(Sense::click()).on_hover_cursor(egui::CursorIcon::PointingHand).clicked() {
                ctx.navigate("profile", Some(format!("{id}/{}", Section::Badges.slug())));
            }
        }
        // Featured badge.
        if let Some(b) = page.as_ref().and_then(|p| p.featured_badge.as_ref()) {
            ui.add_space(4.0);
            ui.horizontal(|ui| {
                let (r, resp) = ui.allocate_exact_size(vec2(54.0, 54.0), Sense::click());
                if !w::paint_contain(ui, r, &b.icon, Align2::CENTER_CENTER) {
                    w::skeleton(ui, t, r, CornerRadius::same(4));
                }
                ui.vertical(|ui| {
                    ui.spacing_mut().item_spacing.y = 1.0;
                    ui.label(w::text_semibold(&b.name, 14.0, t.text));
                    ui.label(w::text(&b.detail, 12.5, t.text_dim));
                });
                if resp.on_hover_cursor(egui::CursorIcon::PointingHand).clicked() {
                    super::open_badge(ctx, id, &model::Badge { title: b.name.clone(), icon: Some(b.icon.clone()), detail: b.detail.clone(), unlocked: String::new(), url: b.url.clone() });
                }
            });
        } else if mine && page.is_some() {
            ui.add(egui::Label::new(w::text(t!("You can feature one of your Badges here."), 12.5, t.text_dim)).wrap());
            if ui.link(w::text(t!("Select one from your edit profile page."), 12.5, t.text)).clicked() {
                ctx.navigate("profile", Some("edit/badge".into()));
            }
        }
        // Actions: Edit Profile (or message / Steam link) with Copy SteamID
        // to its right.
        ui.add_space(8.0);
        ui.horizontal_wrapped(|ui| {
            if mine {
                if w::button(ui, t, Some(icons::NOTE), t!("Edit Profile"), w::Kind::Secondary, w::Size::Sm).clicked() {
                    ctx.navigate("profile", Some("edit".into()));
                }
            } else {
                if w::button(ui, t, Some(icons::CHAT), t!("Send a message"), w::Kind::Primary, w::Size::Sm).clicked() {
                    ctx.act(extensions::Action::OpenChat(id));
                }
                if w::button(ui, t, Some(icons::EXTERNAL), t!("View on Steam"), w::Kind::Secondary, w::Size::Sm).clicked() {
                    crate::links::open_http(&format!("https://steamcommunity.com/profiles/{id}"));
                }
            }
            if w::button(ui, t, Some(icons::COPY), t!("Copy SteamID"), w::Kind::Ghost, w::Size::Sm).clicked() {
                ui.ctx().copy_text(id.to_string());
                ctx.toast(t!("SteamID copied"));
            }
        });
        if let Some(since) = member_since {
            ui.add_space(2.0);
            ui.label(w::text(t!("Member since {date}", date = ui::date(since)), 12.5, t.text_faint)).on_hover_text(ui::ago(since));
        }
    }

    /// Steam's right column. Falls back to the plain link list when the page
    /// can't be read (private profiles, Steam errors).
    #[allow(clippy::too_many_arguments)]
    pub(super) fn right_column(&mut self, ui: &mut Ui, t: &Theme, ctx: &mut Ctx, id: SteamId, mine: bool, open: bool, persona: &Persona) {
        let page = match self.page_data(ctx, id) {
            Some(Ok(p)) => p,
            Some(Err(_)) => {
                self.sidebar(ui, t, ctx, id, mine, open);
                return;
            }
            None => {
                panel(ui, t, |ui| {
                    for h in [40.0, 70.0, 220.0] {
                        let (r, _) = ui.allocate_exact_size(vec2(ui.available_width(), h), Sense::hover());
                        w::skeleton(ui, t, r.shrink2(vec2(0.0, 4.0)), t.corner_small());
                    }
                });
                return;
            }
        };
        panel(ui, t, |ui| {
            ui.spacing_mut().item_spacing.y = 6.0;
            // Level, featured badge and Edit Profile sit in the header, like
            // Steam; the column starts with the status.
            status_block(ui, t, ctx, &page, persona);
            if !open {
                return;
            }
            // Badges: the four Steam shows (newest first).
            if let Some(c) = page.counts.iter().find(|c| c.key == "badges") {
                ui.add_space(14.0);
                count_header(ui, t, ctx, id, mine, c);
                ui.horizontal(|ui| {
                    ui.spacing_mut().item_spacing.x = 10.0;
                    for b in page.badges.iter().take(4) {
                        let (r, resp) = ui.allocate_exact_size(vec2(54.0, 54.0), Sense::click());
                        if !w::paint_contain(ui, r, &b.icon, Align2::CENTER_CENTER) {
                            w::skeleton(ui, t, r, CornerRadius::same(4));
                        }
                        let tip = if b.detail.is_empty() { b.name.clone() } else { format!("{}\n{}", b.name, b.detail) };
                        if resp.on_hover_text(tip).on_hover_cursor(egui::CursorIcon::PointingHand).clicked() {
                            super::open_badge(ctx, id, &model::Badge { title: b.name.clone(), icon: Some(b.icon.clone()), detail: b.detail.clone(), unlocked: String::new(), url: b.url.clone() });
                        }
                    }
                });
            }
            // The other count links.
            ui.add_space(10.0);
            for c in page.counts.iter().filter(|c| !matches!(c.key.as_str(), "badges" | "groups" | "friends")) {
                count_header(ui, t, ctx, id, mine, c);
            }
            // Groups.
            if let Some(c) = page.counts.iter().find(|c| c.key == "groups") {
                ui.add_space(14.0);
                count_header(ui, t, ctx, id, mine, c);
                in_common(ui, t, ctx, &page.groups_in_common);
                for g in &page.groups {
                    let resp = ui
                        .horizontal(|ui| {
                            // Steam's group rows: a 64 px avatar, bold name.
                            let (r, _) = ui.allocate_exact_size(vec2(64.0, 64.0), Sense::hover());
                            match &g.avatar {
                                Some(u) => w::image_or_skeleton(ui, t, r, u, CornerRadius::same(2)),
                                None => w::placeholder(ui, t, r, "", CornerRadius::same(2)),
                            }
                            ui.add_space(8.0);
                            ui.vertical(|ui| {
                                ui.add_space(12.0);
                                ui.spacing_mut().item_spacing.y = 2.0;
                                ui.label(w::text_semibold(&g.name, 14.5, t.text));
                                ui.label(w::text(&g.members, 14.0, t.text_dim));
                            });
                        })
                        .response
                        .interact(Sense::click());
                    if resp.on_hover_cursor(egui::CursorIcon::PointingHand).clicked() && !g.url.is_empty() {
                        crate::links::open_http(&g.url);
                    }
                }
            }
            // Friends: Steam's top 6, highest level first.
            if let Some(c) = page.counts.iter().find(|c| c.key == "friends") {
                ui.add_space(14.0);
                count_header(ui, t, ctx, id, mine, c);
                if !page.friends_in_common.is_empty() {
                    let html = &page.friends_in_common;
                    let html = if let Some(start) = html.find("<a ") {
                        if let Some(end) = html[start..].find('>') {
                            format!("{}<a href=\"aurora:mutual-friends\">{}", &html[..start], &html[start + end + 1..])
                        } else { html.clone() }
                    } else { html.clone() };
                    if ui::steam_text::show(ui, t, &html, ui::steam_text::Mode::Markup, 12.5, t.text_dim).is_some() {
                        ui.ctx().data_mut(|d| d.insert_temp(egui::Id::new(("mutual-friends", id)), true));
                    }
                    ui.add_space(4.0);
                }
                let mut friends = page.friends.clone();
                friends.sort_by_key(|f| std::cmp::Reverse(f.level));
                for f in friends.iter().take(6) {
                    friend_row(ui, t, ctx, f);
                }
            }
        });
        let open_id = egui::Id::new(("mutual-friends", id));
        if ui.ctx().data(|d| d.get_temp::<bool>(open_id).unwrap_or(false)) {
            let mut close = false;
            let modal = w::modal(ui.ctx(), egui::Id::new(("mutual-friends-modal", id))).show(ui.ctx(), |ui| {
                ui.set_width(ui.available_width().min(820.0));
                ui.horizontal(|ui| {
                    ui.label(w::text_bold(t!("Friends in Common"), 20.0, t.text));
                    ui.with_layout(Layout::right_to_left(Align::Center), |ui| {
                        if w::icon_button(ui, t, icons::X, t!("Close"), false, 0).clicked() { close = true; }
                    });
                });
                ui.add_space(12.0);
                match &page.mutual_friends {
                    Some(Ok(friends)) if !friends.is_empty() => {
                        egui::ScrollArea::vertical().max_height(400.0).show(ui, |ui| {
                            let cols = if ui.available_width() >= 600.0 { 3 } else if ui.available_width() >= 400.0 { 2 } else { 1 };
                            for row in friends.chunks(cols) {
                                ui.columns(cols, |columns| {
                                    for (ui, friend) in columns.iter_mut().zip(row) {
                                        friend_row(ui, t, ctx, friend);
                                    }
                                });
                            }
                        });
                    }
                    Some(Ok(_)) => { ui.label(w::text(t!("No friends in common found."), 13.0, t.text_dim)); }
                    Some(Err(e)) => { ui.label(w::text(e, 13.0, t.text_dim)); }
                    None => { ui.label(w::text(t!("Could not load friends in common."), 13.0, t.text_dim)); }
                }
                ui.add_space(12.0);
                ui.with_layout(Layout::right_to_left(Align::Center), |ui| {
                    if w::button(ui, t, None, t!("OK"), w::Kind::Secondary, w::Size::Sm).clicked() { close = true; }
                });
            });
            if close || modal.should_close() { ui.ctx().data_mut(|d| d.insert_temp(open_id, false)); }
        }
    }

    /// Steam's Recent Activity: each game with hours, last played,
    /// achievement progress and screenshots, then the quick links.
    pub(super) fn recent_activity(&mut self, ui: &mut Ui, t: &Theme, ctx: &mut Ctx, id: SteamId) {
        let page = match self.page_data(ctx, id) {
            Some(Ok(p)) => p,
            Some(Err(_)) => return self.recent_games(ui, t, ctx, id),
            None => return,
        };
        if page.recent.is_empty() {
            return;
        }
        panel(ui, t, |ui| {
            let themed = super::profile_theme(ui);
            // Reserved first so the header bar is painted under the title row.
            let bar_slot = ui.painter().add(egui::Shape::Noop);
            let head = ui.horizontal(|ui| {
                ui.add_space(if themed.is_some() { 10.0 } else { 0.0 });
                ui.label(w::text(t!("Recent Activity"), 18.0, if themed.is_some() { Color32::WHITE } else { t.text }));
                ui.with_layout(Layout::right_to_left(Align::Center), |ui| {
                    ui.add_space(if themed.is_some() { 10.0 } else { 0.0 });
                    ui.label(w::text(&page.recent_playtime, 16.0, if themed.is_some() { Color32::WHITE } else { t.text }));
                });
            });
            // Steam's header bar on themed profiles, behind the row just drawn.
            if let Some(theme) = themed {
                let bar = head.response.rect.expand2(vec2(0.0, 6.0));
                let bar = egui::Rect::from_x_y_ranges(ui.max_rect().x_range(), bar.y_range());
                let mut mesh = egui::Mesh::default();
                for (p, c) in [(bar.left_top(), theme.showcase_header_left), (bar.right_top(), theme.showcase_header), (bar.right_bottom(), theme.showcase_header), (bar.left_bottom(), theme.showcase_header_left)] {
                    mesh.colored_vertex(p, c);
                }
                mesh.add_triangle(0, 1, 2);
                mesh.add_triangle(0, 2, 3);
                ui.painter().set(bar_slot, egui::Shape::mesh(mesh));
            }
            ui.add_space(10.0);
            for (i, g) in page.recent.iter().enumerate() {
                if i > 0 {
                    ui.add_space(22.0);
                }
                recent_game(ui, t, ctx, id, g);
            }
            ui.add_space(16.0);
            ui.with_layout(Layout::right_to_left(Align::Center), |ui| {
                ui.spacing_mut().item_spacing.x = 8.0;
                let link = |ui: &mut Ui, label: &str| ui.add(egui::Label::new(w::text(label, 13.0, t.text)).sense(Sense::click())).on_hover_cursor(egui::CursorIcon::PointingHand).clicked();
                if link(ui, t!("Reviews")) {
                    ctx.navigate("profile", Some(format!("{id}/{}", Section::Reviews.slug())));
                }
                ui.label(w::text("|", 13.0, t.text_faint));
                if link(ui, t!("Wishlist")) {
                    crate::links::open_http(&format!("https://store.steampowered.com/wishlist/profiles/{id}/"));
                }
                ui.label(w::text("|", 13.0, t.text_faint));
                if link(ui, t!("All Recently Played")) {
                    ctx.navigate("profile", Some(format!("{id}/{}", Section::Games.slug())));
                }
                ui.label(w::text(t!("View"), 13.0, t.text_faint));
            });
        });
        // With the 8 px item spacing: Steam's 12 px between panels.
        ui.add_space(4.0);
    }
}

/// "Currently Online" / "Currently In-Game" + game / "Currently Offline" +
/// last online, like Steam. Live status for people we see, else the page's.
fn status_block(ui: &mut Ui, t: &Theme, ctx: &mut Ctx, page: &ProfilePage, persona: &Persona) {
    let live = persona.state != PersonaState::Offline || persona.last_logoff.is_some_and(|l| l > 0);
    let (class, header, detail) = if live && persona.in_game() {
        let game = persona.game_name.clone().or_else(|| persona.game_app_id.and_then(|g| ctx.state.app_name(g).map(String::from))).unwrap_or_default();
        ("in-game", t!("Currently In-Game").to_string(), game)
    } else if live && persona.state != PersonaState::Offline {
        ("online", t!("Currently Online").to_string(), String::new())
    } else if live {
        let when = persona.last_logoff.filter(|l| *l > 0).map(|l| t!("Last Online {when}", when = ui::ago(l))).unwrap_or_default();
        ("offline", t!("Currently Offline").to_string(), when)
    } else {
        (page.status.as_str(), page.status_header.clone(), page.status_detail.clone())
    };
    if header.is_empty() {
        return;
    }
    let color = status_color(t, class);
    ui.label(w::text(header, 20.0, color));
    if !detail.is_empty() {
        let r = ui.add(egui::Label::new(w::text(&detail, 15.0, color)).sense(Sense::click()));
        if let Some(game) = persona.game_app_id.filter(|_| class == "in-game") {
            if r.on_hover_cursor(egui::CursorIcon::PointingHand).clicked() {
                ctx.act(crate::shell::game_page(ctx.state, game));
            }
        }
    }
    // Steam's green "Join Game", when the game can be joined.
    if class == "in-game" {
        let join = join_from_link(&page.join_link, persona.game_app_id).or_else(|| Some((persona.game_app_id?, persona.join_args()?)));
        if let Some((app_id, connect)) = join {
            ui.add_space(4.0);
            if join_button(ui, t!("Join Game")).on_hover_text(&connect).clicked() {
                ctx.act(extensions::Action::JoinGame { app_id, connect });
            }
        }
    }
}

/// The game and join arguments in a Steam join link:
/// `steam://joinlobby/<app>/<lobby>/<owner>` or `steam://connect/<server>`.
fn join_from_link(link: &str, app: Option<model::AppId>) -> Option<(model::AppId, String)> {
    let target = link.strip_prefix("steam://")?;
    if let Some(server) = target.strip_prefix("connect/") {
        let app = app?;
        return valid_server_address(server).then(|| (app, format!("+connect {server}")));
    }
    let mut parts = target.split('/').filter(|part| !part.is_empty());
    if parts.next()? != "joinlobby" {
        return None;
    }
    let game = parts.next()?.parse().ok()?;
    let lobby = parts.next()?.parse::<u64>().ok()?;
    Some((game, format!("+connect_lobby {lobby}")))
}

fn valid_server_address(server: &str) -> bool {
    use std::net::{Ipv4Addr, Ipv6Addr};

    if server.is_empty() || server.len() > 255 || server.starts_with(['-', '+']) {
        return false;
    }
    if server.parse::<Ipv6Addr>().is_ok() {
        return true;
    }
    if let Some(bracketed) = server.strip_prefix('[') {
        let Some((address, suffix)) = bracketed.split_once(']') else { return false };
        return address.parse::<Ipv6Addr>().is_ok() && (suffix.is_empty() || suffix.strip_prefix(':').is_some_and(valid_server_port));
    }

    let host = match server.split_once(':') {
        Some((host, port)) if valid_server_port(port) => host,
        Some(_) => return false,
        None => server,
    };
    if host.parse::<Ipv4Addr>().is_ok() {
        return true;
    }

    let host = host.strip_suffix('.').unwrap_or(host);
    // An all-numeric dotted address must parse as IPv4, not as a hostname.
    host.len() <= 253
        && !host.bytes().all(|b| b.is_ascii_digit() || b == b'.')
        && host.split('.').all(|label| {
            (1..=63).contains(&label.len())
                && !label.starts_with('-')
                && !label.ends_with('-')
                && label.bytes().all(|b| b.is_ascii_alphanumeric() || b == b'-')
        })
}

fn valid_server_port(port: &str) -> bool {
    port.bytes().all(|b| b.is_ascii_digit()) && port.parse::<u16>().is_ok_and(|n| n != 0)
}

/// Steam's small green button (`btn_green_white_innerfade btn_small_thin`).
fn join_button(ui: &mut Ui, label: &str) -> egui::Response {
    let galley = ui.painter().layout_no_wrap(label.to_string(), w::font(13.0), Color32::PLACEHOLDER);
    let (r, resp) = ui.allocate_exact_size(vec2(galley.size().x + 14.0, 24.0), Sense::click());
    let hover = resp.hovered();
    let (top, bottom) = if hover { (Color32::from_rgb(0x8e, 0xd6, 0x29), Color32::from_rgb(0x6a, 0xa6, 0x21)) } else { (Color32::from_rgb(0x75, 0xb0, 0x22), Color32::from_rgb(0x58, 0x8a, 0x1b)) };
    w::gradient(ui.painter(), r, top, bottom, false);
    let fg = if hover { Color32::WHITE } else { Color32::from_rgb(0xd2, 0xef, 0xa9) };
    ui.painter().galley(r.center() - galley.size() / 2.0, galley, fg);
    resp.on_hover_cursor(egui::CursorIcon::PointingHand)
}

/// Steam's "You have 1 group in common" / "… friends in common" line, its
/// count a link (opens in Aurora when it can).
fn in_common(ui: &mut Ui, t: &Theme, ctx: &mut Ctx, html: &str) {
    if html.is_empty() {
        return;
    }
    let url = ui::steam_text::show(ui, t, html, ui::steam_text::Mode::Markup, 12.5, t.text_dim);
    super::showcases::follow(ctx, url);
    ui.add_space(4.0);
}

/// "Games 124" as Steam writes it: the label, then the count large and dim.
fn count_header(ui: &mut Ui, t: &Theme, ctx: &mut Ctx, id: SteamId, mine: bool, c: &model::PageCount) {
    let resp = ui
        .horizontal(|ui| {
            ui.label(w::text(&c.label, 14.5, t.text));
            if let Some(n) = c.count {
                ui.label(w::text(n.to_string(), 22.0, t.text_dim));
            }
        })
        .response
        .interact(Sense::click());
    let files = |kind: &str| Link::Page("user_files", Some(format!("{id}/{kind}")));
    let link = match c.key.as_str() {
        "badges" => Link::Section(Section::Badges),
        "games" => Link::Section(Section::Games),
        "reviews" => Link::Section(Section::Reviews),
        "groups" => Link::Section(Section::Groups),
        "inventory" => Link::Page("inventory", Some(id.to_string())),
        "screenshots" => files("screenshots"),
        "artwork" => files("artwork"),
        "videos" => files("videos"),
        "workshop" => files("workshop"),
        "guides" => files("guides"),
        "friends" if mine => Link::Page("community", None),
        _ => Link::Web(c.url.clone()),
    };
    if resp.on_hover_cursor(egui::CursorIcon::PointingHand).clicked() {
        match link {
            Link::Page(page, arg) => ctx.navigate(page, arg),
            Link::Section(s) => ctx.navigate("profile", Some(format!("{id}/{}", s.slug()))),
            Link::Web(url) => {
                crate::links::open_http(&url);
            }
        }
    }
}

fn friend_row(ui: &mut Ui, t: &Theme, ctx: &mut Ctx, f: &PageFriend) {
    let (r, resp) = ui.allocate_exact_size(vec2(ui.available_width(), 48.0), Sense::click());
    if resp.hovered() {
        ui.painter().rect_filled(r, t.corner_small(), t.surface_hover.gamma_multiply(0.6));
    }
    let color = status_color(t, &f.status);
    let av = Rect::from_min_size(r.min + vec2(2.0, 6.0), vec2(36.0, 36.0));
    match &f.avatar {
        Some(u) => {
            if !w::paint_cover(ui, av, u, CornerRadius::same(2)) {
                w::skeleton(ui, t, av, CornerRadius::same(2));
            }
        }
        None => w::placeholder(ui, t, av, "", CornerRadius::same(2)),
    }
    ui.painter().rect_stroke(av, CornerRadius::same(2), Stroke::new(1.5, color), egui::StrokeKind::Outside);
    let x = av.right() + 10.0;
    let max_w = (r.right() - 48.0 - x).max(40.0);
    let galley = |s: &str, size: f32, c: Color32| {
        let mut job = egui::text::LayoutJob::simple_singleline(s.to_string(), w::font(size), c);
        job.wrap = egui::text::TextWrapping::truncate_at_width(max_w);
        ui.painter().layout_job(job)
    };
    let name = galley(&f.name, 13.0, if f.status == "offline" { t.text_dim } else { color });
    let mut y = r.top() + 5.0;
    ui.painter().galley(pos2(x, y), name, t.text);
    for line in f.lines.iter().take(2) {
        y += 14.0;
        ui.painter().galley(pos2(x, y), galley(line, 11.5, if f.status == "offline" { t.text_faint } else { color }), t.text);
    }
    // Level on the right, like Steam.
    let lr = Rect::from_center_size(pos2(r.right() - 20.0, r.center().y), vec2(34.0, 34.0));
    let mut child = ui.new_child(egui::UiBuilder::new().max_rect(lr).layout(Layout::left_to_right(Align::Center)));
    if f.level > 0 { level_badge(&mut child, t, f.level); }
    if resp.on_hover_cursor(egui::CursorIcon::PointingHand).clicked() {
        ctx.navigate("profile", Some(f.steam_id.to_string()));
    }
}

fn recent_game(ui: &mut Ui, t: &Theme, ctx: &mut Ctx, id: SteamId, g: &RecentGame) {
    ui.horizontal_top(|ui| {
        let (r, resp) = ui.allocate_exact_size(vec2(184.0, 69.0), Sense::click());
        let capsule = g.capsule.clone().unwrap_or_else(|| model::art::header(g.app_id));
        w::image_or_skeleton(ui, t, r, &capsule, CornerRadius::ZERO);
        let open = resp.on_hover_cursor(egui::CursorIcon::PointingHand).clicked();
        ui.add_space(8.0);
        let open_name = ui.add(egui::Label::new(w::text(&g.name, 15.0, t.text)).sense(Sense::click())).on_hover_cursor(egui::CursorIcon::PointingHand).clicked();
        ui.with_layout(Layout::top_down(Align::Max), |ui| {
            ui.add_space(18.0);
            ui.spacing_mut().item_spacing.y = 2.0;
            for line in &g.details {
                ui.label(w::text(line, 12.5, t.text_dim));
            }
        });
        if open || open_name {
            ctx.act(crate::shell::game_page(ctx.state, g.app_id));
        }
    });
    if let Some(badge) = &g.badge {
        // Steam's layout with a game badge: the badge box on the left, the
        // achievements (summary, bar, icons) stacked on its right.
        ui.add_space(10.0);
        ui.horizontal_top(|ui| {
            ui.add_space(8.0);
            game_badge(ui, t, ctx, id, badge);
            ui.add_space(14.0);
            if !g.achievements.is_empty() {
                ui.vertical(|ui| {
                    ui.spacing_mut().item_spacing.y = 4.0;
                    let bar_w = ui.available_width().clamp(160.0, 320.0);
                    ui.horizontal(|ui| {
                        ui.label(w::text(icons::TROPHY, 22.0, t.text_faint));
                        ui.vertical(|ui| {
                            ui.set_width(bar_w - 30.0);
                            ui.horizontal(|ui| {
                                let r = ui.add(egui::Label::new(w::text(&g.achievements, 13.0, t.text)).sense(Sense::click()));
                                if r.on_hover_cursor(egui::CursorIcon::PointingHand).clicked() {
                                    ctx.navigate("achievements", Some(g.app_id.to_string()));
                                }
                                if !g.rare.is_empty() {
                                    ui.with_layout(Layout::right_to_left(Align::Center), |ui| {
                                        ui.label(w::text(&g.rare, 12.5, t.away));
                                    });
                                }
                            });
                            w::progress(ui, t, g.progress, bar_w - 30.0);
                        });
                    });
                    ui.add_space(4.0);
                    ui.horizontal(|ui| {
                        ui.spacing_mut().item_spacing.x = 8.0;
                        achievement_icons(ui, t, ctx, g);
                    });
                });
            }
        });
    } else if !g.achievements.is_empty() {
        ui.add_space(6.0);
        ui.horizontal(|ui| {
            ui.add_space(8.0);
            ui.label(w::text(icons::TROPHY, 22.0, t.text_faint));
            let icons_w = (g.icons.len() as f32 + if g.more.is_empty() { 0.0 } else { 1.0 }) * 40.0;
            let bar_w = (ui.available_width() - icons_w - 24.0).clamp(160.0, 320.0);
            ui.vertical(|ui| {
                ui.set_width(bar_w);
                ui.spacing_mut().item_spacing.y = 4.0;
                ui.horizontal(|ui| {
                    let r = ui.add(egui::Label::new(w::text(&g.achievements, 13.0, t.text)).sense(Sense::click()));
                    if r.on_hover_cursor(egui::CursorIcon::PointingHand).clicked() {
                        ctx.navigate("achievements", Some(g.app_id.to_string()));
                    }
                    if !g.rare.is_empty() {
                        ui.with_layout(Layout::right_to_left(Align::Center), |ui| {
                            ui.label(w::text(&g.rare, 12.5, t.away));
                        });
                    }
                });
                w::progress(ui, t, g.progress, bar_w);
            });
            ui.with_layout(Layout::right_to_left(Align::Center), |ui| {
                ui.spacing_mut().item_spacing.x = 8.0;
                // Right to left: "+N" first, then the icons reversed.
                if !g.more.is_empty() {
                    more_box(ui, t, ctx, g);
                }
                for (name, icon) in g.icons.iter().rev() {
                    achievement_icon(ui, t, ctx, g, name, icon);
                }
            });
        });
    }
    // "Screenshots 12  ·  Review 1", each only when there is one.
    if !g.screenshots.is_empty() || !g.review.is_empty() {
        ui.add_space(4.0);
        let (r, _) = ui.allocate_exact_size(vec2(ui.available_width(), 9.0), Sense::hover());
        ui.painter().hline(r.left() + 8.0..=r.right() - 8.0, r.center().y, Stroke::new(1.0, t.border));
        ui.horizontal(|ui| {
            ui.add_space(8.0);
            if !g.screenshots.is_empty() {
                ui.label(w::text(icons::CAMERA, 14.0, t.text_dim));
                let r = ui.add(egui::Label::new(w::text(&g.screenshots, 13.0, t.text)).sense(Sense::click()));
                if r.on_hover_cursor(egui::CursorIcon::PointingHand).clicked() {
                    ctx.navigate("user_files", Some(format!("{id}/screenshots")));
                }
                ui.add_space(16.0);
            }
            if !g.review.is_empty() {
                ui.label(w::text(icons::THUMBS_UP, 14.0, t.text_dim));
                let r = ui.add(egui::Label::new(w::text(&g.review, 13.0, t.text)).sense(Sense::click()));
                if r.on_hover_cursor(egui::CursorIcon::PointingHand).clicked() {
                    ctx.navigate("profile", Some(format!("{id}/{}", Section::Reviews.slug())));
                }
            }
        });
    }
}

/// Steam's bordered badge box in Recent Activity: icon, name and XP.
fn game_badge(ui: &mut Ui, t: &Theme, ctx: &mut Ctx, id: SteamId, b: &model::PageBadge) {
    let size = vec2(260.0, 78.0);
    let (r, resp) = ui.allocate_exact_size(size, Sense::click());
    ui.painter().rect_stroke(r, CornerRadius::same(2), Stroke::new(1.0, t.text_faint.gamma_multiply(0.6)), egui::StrokeKind::Inside);
    let icon = Rect::from_center_size(pos2(r.left() + 36.0, r.center().y), vec2(48.0, 48.0));
    if !w::paint_contain(ui, icon, &b.icon, Align2::CENTER_CENTER) {
        w::skeleton(ui, t, icon, CornerRadius::same(4));
    }
    let x = icon.right() + 16.0;
    let max_w = r.right() - x - 8.0;
    let galley = |s: &str, size: f32, c: Color32| {
        let mut job = egui::text::LayoutJob::simple_singleline(s.to_string(), w::font(size), c);
        job.wrap = egui::text::TextWrapping::truncate_at_width(max_w);
        ui.painter().layout_job(job)
    };
    let name = galley(&b.name, 14.0, t.text);
    let detail = galley(&b.detail, 13.0, t.text_dim);
    let top = r.center().y - (name.size().y + detail.size().y + 2.0) / 2.0;
    ui.painter().galley(pos2(x, top), name.clone(), t.text);
    ui.painter().galley(pos2(x, top + name.size().y + 2.0), detail, t.text);
    let tip = if b.detail.is_empty() { b.name.clone() } else { format!("{}\n{}", b.name, b.detail) };
    if resp.on_hover_text(tip).on_hover_cursor(egui::CursorIcon::PointingHand).clicked() {
        super::open_badge(ctx, id, &model::Badge { title: b.name.clone(), icon: Some(b.icon.clone()), detail: b.detail.clone(), unlocked: String::new(), url: b.url.clone() });
    }
}

/// The recent game's achievement icons, left to right, then "+N".
fn achievement_icons(ui: &mut Ui, t: &Theme, ctx: &mut Ctx, g: &RecentGame) {
    for (name, icon) in &g.icons {
        achievement_icon(ui, t, ctx, g, name, icon);
    }
    if !g.more.is_empty() {
        more_box(ui, t, ctx, g);
    }
}

fn achievement_icon(ui: &mut Ui, t: &Theme, ctx: &mut Ctx, g: &RecentGame, name: &str, icon: &str) {
    let (r, resp) = ui.allocate_exact_size(vec2(32.0, 32.0), Sense::click());
    if !w::paint_cover(ui, r, icon, CornerRadius::same(2)) {
        w::skeleton(ui, t, r, CornerRadius::same(2));
    }
    if resp.on_hover_text(name).on_hover_cursor(egui::CursorIcon::PointingHand).clicked() {
        ctx.navigate("achievements", Some(g.app_id.to_string()));
    }
}

fn more_box(ui: &mut Ui, t: &Theme, ctx: &mut Ctx, g: &RecentGame) {
    let (r, resp) = ui.allocate_exact_size(vec2(32.0, 32.0), Sense::click());
    ui.painter().rect_filled(r, CornerRadius::same(2), t.raised);
    ui.painter().text(r.center(), Align2::CENTER_CENTER, &g.more, w::font_semibold(12.0), t.text);
    if resp.on_hover_cursor(egui::CursorIcon::PointingHand).clicked() {
        ctx.navigate("achievements", Some(g.app_id.to_string()));
    }
}

#[cfg(test)]
mod tests {
    use super::join_from_link;

    #[test]
    fn connect_links_reject_invalid_server_addresses() {
        for server in [
            "", "1.2.3.4 -exec evil", "+exec", "-exec", "host;quit", "host\nquit", "host\"quit", "host'quit", "host:65536", "host:0", "host:", "bad..host", "256.2.3.4", "user@host", "host?foo", "host#foo",
            "/host", "host/password", &"a".repeat(256),
        ] {
            assert_eq!(join_from_link(&format!("steam://connect/{server}"), Some(440)), None, "{server:?}");
        }
    }

    #[test]
    fn connect_links_accept_supported_server_addresses() {
        for server in ["1.2.3.4", "1.2.3.4:27015", "game-server.example.org:27015", "localhost", "::1", "[2001:db8::1]:27015"] {
            assert_eq!(join_from_link(&format!("steam://connect/{server}"), Some(440)), Some((440, format!("+connect {server}"))));
        }
        assert_eq!(join_from_link("steam://joinlobby/440/1234/5678", None), Some((440, "+connect_lobby 1234".into())));
    }
}
