//! Profiles: yours and your friends'. Shows the equipped profile background,
//! avatar frame, level, bio, and (for you) stats and game history.

use ui::widgets::EditMenu;
use client::Command;
use egui::{Align, Align2, Color32, CornerRadius, Layout, Margin, Rect, Sense, Stroke, Ui, pos2, vec2};
use extensions::{Ctx, Extension, Manifest, PageSpec};
use i18n::t;
use model::{Relationship, SteamId, Visibility, art};
use std::collections::{HashMap, HashSet};
use ui::{Theme, icons, widgets as w};

#[path = "profile_classic.rs"]
mod classic;
#[path = "profile_pages.rs"]
mod pages;
#[path = "profile_edit.rs"]
pub(crate) mod edit;
#[path = "profile_themes.rs"]
pub(crate) mod themes;
#[path = "profile_showcases.rs"]
mod showcases;
#[path = "profile_sidebar.rs"]
mod sidebar;

/// What the profile page shows: the profile, or one of its lists.
#[derive(Clone, Copy, PartialEq, Eq, Hash, Default, Debug)]
enum Section {
    #[default]
    Profile,
    Games,
    Badges,
    Reviews,
    Groups,
    CommonGroups,
    /// A game's card collection (the game and foil set in `Profile::cards`).
    Cards,
    /// Editing your own profile.
    Edit,
}

impl Section {
    fn slug(self) -> &'static str {
        match self {
            Section::Profile => "",
            Section::Games => "games",
            Section::Badges => "badges",
            Section::Reviews => "reviews",
            Section::Groups => "groups",
            Section::CommonGroups => "groupscommon",
            Section::Cards => "cards",
            Section::Edit => "edit",
        }
    }

    fn from_slug(s: Option<&str>) -> Section {
        [Section::Games, Section::Badges, Section::Reviews, Section::Groups, Section::CommonGroups, Section::Cards].into_iter().find(|x| Some(x.slug()) == s).unwrap_or_default()
    }
}

#[derive(Clone, Copy, PartialEq, Eq, Default)]
enum GameSort {
    #[default]
    Recent,
    Playtime,
    Name,
}

#[derive(Default)]
pub struct Profile {
    flags: super::settings::SharedFlags,
    /// `None` = your own profile.
    viewing: Option<SteamId>,
    /// A custom profile URL (steamcommunity.com/id/<name>) waiting for its
    /// SteamID, and the ones asked for.
    vanity: Option<String>,
    vanity_requested: HashSet<String>,
    requested: HashSet<SteamId>,
    games_requested: HashSet<SteamId>,
    /// The profile, or one of its lists (games, badges, reviews, groups).
    section: Section,
    /// Reviews page shown (1-based), and the review pages and group lists asked for.
    review_page: u32,
    lists_requested: HashSet<(SteamId, Section, u32)>,
    /// The card collection shown: game and foil set.
    cards: (model::AppId, bool),
    games_search: String,
    games_sort: GameSort,
    showcase_requested: HashSet<SteamId>,
    classic_inventory_requested: HashSet<SteamId>,
    /// Last request for an item showcase inventory: (profile, app, context).
    /// Failed inventory RPCs need a retry; keeping only a set left the tiles
    /// shimmering forever after one transient Steam failure.
    showcase_inventories: HashMap<(SteamId, model::AppId, u64), std::time::Instant>,
    /// Steam profile pages and name histories asked for.
    page_requested: HashSet<SteamId>,
    aliases_requested: HashSet<SteamId>,
    /// "Clear Previous Aliases" waiting for confirmation.
    confirm_clear_aliases: bool,
    social_requested: HashSet<SteamId>,
    comment_draft: String,
    /// Which page of comments shows (first comment's index), per profile.
    comment_start: std::collections::HashMap<SteamId, u32>,
    /// Editing your profile ("Edit profile" pages).
    edit: edit::EditState,
}

impl Profile {
    pub fn new(flags: super::settings::SharedFlags) -> Self {
        Self { flags, ..Self::default() }
    }
}

impl Extension for Profile {
    fn manifest(&self) -> Manifest {
        Manifest { id: "profile", name: "Profile", author: "Aurora", description: "Profiles", core: true, ..Manifest::DEFAULT }
    }

    fn pages(&self) -> Vec<PageSpec> {
        vec![PageSpec::new("profile", "Profile", icons::USER_CIRCLE).main().order(40)]
    }

    fn on_event(&mut self, event: &client::Event, ctx: &mut Ctx) {
        match event {
            client::Event::Profile(p) => { self.requested.remove(&p.steam_id); }
            client::Event::PlayerGames { steam_id, .. } => { self.games_requested.remove(steam_id); }
            client::Event::Reviews { steam_id, page, .. } => { self.lists_requested.remove(&(*steam_id, Section::Reviews, *page)); }
            _ => {}
        }
        self.edit_event(event, ctx);
        if let client::Event::AliasesCleared(result) = event {
            match result {
                Ok(()) => ctx.toast(t!("Name history cleared")),
                Err(e) => ctx.toast(t!("Couldn't clear the name history: {error}", error = i18n::tr_dyn(e))),
            }
        }
        if let client::Event::CommentPosted { result, .. } = event {
            match result {
                Ok(()) => {
                    self.comment_draft.clear();
                    ctx.toast(t!("Comment posted"));
                }
                Err(e) => ctx.toast(t!("Couldn't post the comment: {error}", error = i18n::tr_dyn(e))),
            }
        }
    }

    fn on_navigate(&mut self, _page: &str, arg: Option<&str>) {
        // "edit" or "edit/<section>": your profile's settings.
        if let Some(rest) = arg.and_then(|a| a.strip_prefix("edit")) {
            if rest.is_empty() || rest.starts_with('/') {
                self.viewing = None;
                self.section = Section::Edit;
                self.edit.section = edit::Section::from_arg(rest.trim_start_matches('/'));
                return;
            }
        }
        // "<id>/<part>", or just "<part>" ("games", "badges"…) for your own.
        // "id:<name>/<part>": a custom profile URL, resolved before showing.
        let mut parts = arg.unwrap_or_default().split('/').peekable();
        self.vanity = parts.peek().and_then(|a| a.strip_prefix("id:")).filter(|v| !v.is_empty()).map(|v| v.to_ascii_lowercase());
        self.viewing = parts.peek().and_then(|a| a.parse().ok());
        if self.viewing.is_some() || self.vanity.is_some() || parts.peek() == Some(&"") {
            parts.next();
        }
        self.section = Section::from_slug(parts.next());
        self.review_page = 1;
        if self.section == Section::Cards {
            self.cards = (parts.next().and_then(|a| a.parse().ok()).unwrap_or(0), parts.next() == Some("foil"));
        }
        self.games_search.clear();
    }

    fn page(&mut self, _page: &str, ui: &mut Ui, ctx: &mut Ctx) {
        let t = ctx.theme.clone();
        let steam_profile_ui = self.flags.lock().unwrap().steam_profile_ui;
        ui.ctx().data_mut(|d| d.remove::<ProfileStyle>(profile_style_id()));
        // A custom profile URL: find its SteamID first.
        if let Some(vanity) = self.vanity.clone() {
            match ctx.state.vanity_ids.get(&vanity) {
                Some(Ok(steam_id)) => {
                    self.viewing = Some(*steam_id);
                    self.vanity = None;
                }
                Some(Err(e)) => {
                    let e = e.clone();
                    ui.add_space(60.0);
                    ui.vertical_centered(|ui| {
                        ui.label(w::text_bold(t!("Couldn't open this profile"), 18.0, t.text));
                        ui.label(w::text(i18n::tr_dyn(&e), 13.5, t.text_dim));
                        if w::button(ui, &t, Some(icons::EXTERNAL), t!("View on Steam"), w::Kind::Secondary, w::Size::Sm).clicked() {
                            crate::links::open_http(&format!("https://steamcommunity.com/id/{vanity}/"));
                        }
                    });
                    return;
                }
                None => {
                    if self.vanity_requested.insert(vanity.clone()) {
                        ctx.send(Command::ResolveProfileUrl { vanity });
                    }
                    ui.add_space(80.0);
                    ui.vertical_centered(|ui| ui.add(egui::Spinner::new().size(28.0).color(t.accent)));
                    return;
                }
            }
        }
        let id = self.viewing.filter(|id| *id != ctx.state.steam_id).unwrap_or(ctx.state.steam_id);
        let mine = id == ctx.state.steam_id;
        if id != 0 && !ctx.state.profiles.contains_key(&id) && self.requested.insert(id) {
            ctx.send(Command::LoadProfile { steam_id: id });
        }
        let friend = ctx.state.friends.get(&id) == Some(&Relationship::Friend);
        let visibility = ctx.state.profiles.get(&id).map(|i| i.visibility).unwrap_or_default();
        // Whether Steam will show us this profile's games, inventory and friends.
        let open = mine
            || match visibility {
                Visibility::Public | Visibility::Unknown => true,
                Visibility::FriendsOnly => friend,
                Visibility::Private => false,
            };
        if id != 0 && open && !ctx.state.player_games.contains_key(&id) && self.games_requested.insert(id) {
            ctx.send(Command::LoadPlayerGames { steam_id: id, language: i18n::language().steam_name().into() });
        }
        // Right-click on empty space: "Copy page URL" (this profile on Steam).
        let path = match self.section {
            Section::Games => "games/?tab=all",
            Section::Badges => "badges/",
            Section::Reviews => "recommended/",
            Section::Groups => "groups/",
            Section::CommonGroups => "groupscommon/",
            Section::Cards => "badges/",
            Section::Edit => "edit/info",
            Section::Profile => "",
        };
        crate::links::page_url_menu(ui, &format!("https://steamcommunity.com/profiles/{id}/{path}"));

        if !steam_profile_ui {
            self.classic_page(ui, &t, ctx, id, open);
            return;
        }
        let persona = ctx.state.persona(id);
        let info = ctx.state.profiles.get(&id).cloned();
        let display_name = if mine && !ctx.state.persona_name.is_empty() { ctx.state.persona_name.clone() } else { persona.name.clone() };

        // The profile's own Steam theme or game profile colors its panels,
        // like on Steam. Profiles without one keep Aurora's look.
        let steam_theme = match ctx.state.profile_pages.get(&id).and_then(|r| r.as_ref().ok()) {
            Some(p) if p.theme == "GameProfile" => themes::from_vars(&p.theme_vars),
            Some(p) if !p.theme.is_empty() => themes::get(&p.theme),
            _ => info.as_ref().map(|i| i.theme.as_str()).filter(|t| !t.is_empty() && *t != "Default").and_then(themes::get),
        };
        ui.ctx().data_mut(|d| d.insert_temp(profile_style_id(), ProfileStyle::Steam(steam_theme)));

        // Full-page background, like a Steam profile.
        let full = ui.max_rect();
        let bg_url = info.as_ref().and_then(|i| i.background.clone());
        // Steam's page is black around and below a profile background,
        // whatever Aurora theme is in use.
        if bg_url.is_some() && self.section == Section::Profile {
            ui.painter().rect_filled(full, CornerRadius::ZERO, Color32::BLACK);
        }
        if bg_url.is_none() {
            w::gradient(ui.painter(), Rect::from_min_size(full.min, vec2(full.width(), 340.0)), t.accent.gamma_multiply(0.30), t.accent_2.gamma_multiply(0.18), true);
            w::fade_down(ui, Rect::from_min_size(full.min, vec2(full.width(), 340.0)), t.bg, 0.0);
        }

        let is_game_profile = matches!(ctx.state.profile_pages.get(&id), Some(Ok(p)) if p.theme == "GameProfile");
        let bg_full = info.as_ref().is_some_and(|i| i.background_full);

        egui::ScrollArea::vertical().auto_shrink([false, false]).show(ui, |ui| {
            // As on Steam, the background belongs to the page: its own size,
            // centered at the top ("Full Screen": as wide as the window), and
            // it scrolls away with the content.
            let page_top = ui.max_rect().top();
            let bg_rect = bg_url.as_deref().and_then(|url| w::paint_page_background(ui, full.center().x, page_top, url, bg_full.then_some(full.width())));
            // The page column and its parts, painted once their size is known.
            let column_slot = ui.painter().add(egui::Shape::Noop);
            let content_slot = ui.painter().add(egui::Shape::Noop);
            let tint_slot = ui.painter().add(egui::Shape::Noop);
            w::centered(ui, PAGE_W, 0.0, |ui| {
                match self.section {
                    Section::Profile => {}
                    Section::Games => {
                        ui.add_space(40.0);
                        return self.games_page(ui, &t, ctx, id, &display_name, mine);
                    }
                    Section::Edit => {
                        ui.add_space(40.0);
                        return self.edit_page(ui, &t, ctx);
                    }
                    other => {
                        ui.add_space(40.0);
                        return self.list_page(ui, &t, ctx, id, &display_name, mine, other);
                    }
                }
                // Steam's grid: a 976 px column; the header holds the avatar,
                // the name column at 228 px and the Level block at 676 px;
                // below, a 652 px and a 288 px column with 12 px gutters.
                let col = Rect::from_min_size(ui.cursor().min, vec2(ui.available_width(), 0.0));
                // The columns get the height the scroll area offers, like any
                // page content; infinite room would push centered rows (and
                // everything after them, like the comments) out of sight.
                let col_h = ui.available_height().max(1.0);
                let scale_w = col.width();
                let right_w = RIGHT_W.min(scale_w * 0.32);
                let left_w = (scale_w - right_w - GUTTER * 3.0).max(200.0);
                let right_x = col.left() + GUTTER * 2.0 + left_w;
                let header_side_w = right_w;
                let header_top = col.top();
                // ---- header ----
                let avatar_box = Rect::from_min_size(pos2(col.left() + 24.0, header_top + 24.0), vec2(166.0, 166.0));
                {
                    // Steam: a 166 px box, the picture inset 2 px (162 px) on
                    // the status color's border, and the frame drawn over the
                    // picture at 1.22× its size, centered.
                    let img = avatar_box.shrink(2.0);
                    let still = art::avatar_full(persona.avatar_hash.as_deref());
                    let avatar_url = info.as_ref().and_then(|i| i.animated_avatar.as_deref()).unwrap_or(&still);
                    let frame = info.as_ref().and_then(|i| i.avatar_frame.as_deref());
                    if frame.is_none() {
                        ui.painter().rect_filled(avatar_box, CornerRadius::ZERO, avatar_border(&t, &persona));
                    }
                    // Animated avatars and frames play, capped for performance.
                    if !w::paint_cover_animated(ui, img, avatar_url, CornerRadius::ZERO, AVATAR_FPS) {
                        w::skeleton(ui, &t, img, CornerRadius::ZERO);
                    }
                    if let Some(frame) = frame {
                        w::paint_cover_animated(ui, Rect::from_center_size(img.center(), img.size() * 1.22), frame, CornerRadius::ZERO, AVATAR_FPS);
                    }
                }
                let name_x = col.left() + 228.0;
                let name_w = (right_x - GUTTER - name_x).max(160.0);
                let mut name_ui = ui.new_child(egui::UiBuilder::new().max_rect(Rect::from_min_size(pos2(name_x, header_top + 28.0), vec2(name_w, col_h))).layout(Layout::top_down(Align::Min)));
                {
                    let ui = &mut name_ui;
                    ui.set_width(name_w);
                    ui.spacing_mut().item_spacing = vec2(8.0, 4.0);
                    // Rows as tall as their text, like Steam's lines: the
                    // theme's 32 px button height would pad the location and
                    // summary rows and push the summary down.
                    ui.spacing_mut().interact_size.y = 0.0;
                    {
                                let name = if mine && !ctx.state.persona_name.is_empty() { ctx.state.persona_name.clone() } else { persona.name.clone() };
                                // Steam's `profile_header_centered_persona`: the
                                // name and the real name / location line, with
                                // 18 px under it before the summary (the same
                                // when there's no location line).
                                let persona_block = ui.vertical(|ui| {
                                ui.horizontal(|ui| {
                                    ui.label(w::text(name, 30.0, Color32::WHITE));
                                    // Steam's chevron: the names this profile used before.
                                    let chevron = ui.add(egui::Label::new(w::text(icons::CARET_DOWN, 16.0, t.text_dim)).sense(Sense::click()));
                                    let chevron = chevron.on_hover_cursor(egui::CursorIcon::PointingHand).on_hover_text(t!("Previous names"));
                                    egui::Popup::menu(&chevron).width(280.0).show(|ui| self.aliases_menu(ui, &t, ctx, id, mine));
                                });
                                if let Some(i) = &info {
                                    // Real name, then the country flag and location, like Steam.
                                    let flag = match ctx.state.profile_pages.get(&id) {
                                        Some(Ok(p)) => p.country_flag.clone(),
                                        _ => String::new(),
                                    };
                                    if !i.real_name.is_empty() || !i.location.is_empty() {
                                        ui.horizontal(|ui| {
                                            ui.spacing_mut().item_spacing.x = 6.0;
                                            if !i.real_name.is_empty() {
                                                ui.label(w::text(&i.real_name, 14.0, t.text_dim));
                                            }
                                            if !i.location.is_empty() {
                                                if !i.real_name.is_empty() {
                                                    ui.add_space(4.0);
                                                }
                                                if !flag.is_empty() {
                                                    let (fr, _) = ui.allocate_exact_size(vec2(16.0, 11.0), Sense::hover());
                                                    w::paint_contain(ui, fr, &flag, Align2::CENTER_CENTER);
                                                }
                                                ui.label(w::text(&i.location, 14.0, t.text_dim));
                                            }
                                        });
                                    }
                                } else {
                                    let (r, _) = ui.allocate_exact_size(vec2(220.0, 14.0), Sense::hover());
                                    w::skeleton(ui, &t, r, CornerRadius::same(4));
                                }
                                }).response.rect;
                                let gap = persona_block.bottom() + PERSONA_PADDING_BOTTOM - ui.cursor().top();
                                ui.add_space(gap.max(0.0));
                                // Leading line breaks go, but not full-width
                                // spaces: people indent with them, and Steam
                                // keeps that indent.
                                match info.as_ref().map(|i| i.summary.trim_end().trim_start_matches(|c: char| c.is_ascii_whitespace()).to_string()) {
                                    Some(s) if !s.trim().is_empty() => {
                                        // Steam's BBCode, emoticons and emoji, like the real profile.
                                        summary(ui, &t, ctx, id, &s);
                                    }
                                    Some(_) => {
                                        ui.label(w::text(t!("No profile summary yet."), 13.5, t.text_faint));
                                    }
                                    None => {
                                        for wdt in [380.0, 300.0] {
                                            let (r, _) = ui.allocate_exact_size(vec2(wdt, 12.0), Sense::hover());
                                            w::skeleton(ui, &t, r, CornerRadius::same(4));
                                        }
                                    }
                                }
                    }
                }
                // Level, featured badge, actions and Member since, at the
                // right column's x like Steam.
                let mut side_ui = ui.new_child(egui::UiBuilder::new().max_rect(Rect::from_min_size(pos2(right_x, header_top + 28.0), vec2(header_side_w, col_h))).layout(Layout::top_down(Align::Min)));
                side_ui.set_width(header_side_w);
                let since = info.as_ref().and_then(|i| i.member_since);
                self.header_side(&mut side_ui, &t, ctx, id, mine, since);
                // Steam's header is at least 230 px tall; taller when its text runs longer.
                let header_bottom = (header_top + HEADER_H).max(name_ui.min_rect().bottom() + 24.0).max(side_ui.min_rect().bottom() + 24.0);
                ui.allocate_rect(Rect::from_min_max(col.min, pos2(col.right(), header_bottom)), Sense::hover());

                // ---- the two columns ----
                let content_top = header_bottom;
                let mut left_ui = ui.new_child(egui::UiBuilder::new().max_rect(Rect::from_min_size(pos2(col.left() + GUTTER, content_top + GUTTER), vec2(left_w, col_h))).layout(Layout::top_down(Align::Min)));
                left_ui.set_width(left_w);
                {
                    let ui = &mut left_ui;
                        // The in-game status is only in the right column, as on Steam.
                        if open {
                            // Like Steam: showcases, recent activity, comments.
                            self.showcases(ui, &t, ctx, id);
                            self.recent_activity(ui, &t, ctx, id);
                            self.comments(ui, &t, ctx, id, mine);
                        } else {
                            panel(ui, &t, |ui| {
                                ui.horizontal(|ui| {
                                    ui.label(w::text(icons::LOCK_SIMPLE, 26.0, t.text_dim));
                                    ui.add_space(6.0);
                                    ui.vertical(|ui| {
                                        let title = if visibility == Visibility::FriendsOnly { t!("This profile is visible to friends only") } else { t!("This profile is private") };
                                        ui.label(w::text_bold(title, 16.0, t.text));
                                        ui.label(w::text(t!("Games, inventory and friends are hidden by its owner."), 13.0, t.text_dim));
                                    });
                                });
                            });
                        }
                }
                // Steam's right column is its own box with its content 10 px
                // in, 12 px away from the left column.
                let right_inner = right_w - PANEL_PAD * 2.0;
                let mut right_ui = ui.new_child(egui::UiBuilder::new().max_rect(Rect::from_min_size(pos2(right_x + PANEL_PAD, content_top + GUTTER), vec2(right_inner, col_h))).layout(Layout::top_down(Align::Min)));
                right_ui.set_width(right_inner);
                self.right_column(&mut right_ui, &t, ctx, id, mine, open, &persona);
                let content_bottom = left_ui.min_rect().bottom().max(right_ui.min_rect().bottom()) + GUTTER;
                ui.allocate_rect(Rect::from_min_max(pos2(col.left(), content_top), pos2(col.right(), content_bottom)), Sense::hover());

                // ---- the page column behind it all, as Steam themes paint it ----
                let column = Rect::from_min_max(pos2(col.left(), page_top), pos2(col.right(), content_bottom));
                match profile_theme(ui) {
                    // Game profiles: a see-through header, and each column
                    // frosted on its own with a gap between, like Steam.
                    Some(theme) if is_game_profile => {
                        let boxes = [
                            Rect::from_min_max(pos2(col.left() + GUTTER, content_top), pos2(right_x - GUTTER, content_bottom)),
                            Rect::from_min_max(pos2(right_x, content_top), pos2(right_x + right_w, content_bottom)),
                        ];
                        if let (Some(url), Some(bg)) = (bg_url.as_deref(), bg_rect) {
                            let shapes: Vec<egui::Shape> = boxes.iter().filter_map(|b| w::blurred_background_shape(ui, bg, *b, url)).collect();
                            ui.painter().set(content_slot, egui::Shape::Vec(shapes));
                        }
                        ui.painter().set(tint_slot, egui::Shape::Vec(boxes.iter().map(|b| egui::Shape::rect_filled(*b, CornerRadius::ZERO, theme.frost_tint())).collect()));
                    }
                    // Other themes: the whole column tinted, the panels on top.
                    Some(theme) => {
                        ui.painter().set(column_slot, egui::Shape::rect_filled(column, CornerRadius::ZERO, theme.column_fill()));
                    }
                    // Steam's default look.
                    None => {
                        ui.painter().set(column_slot, egui::Shape::rect_filled(column, CornerRadius::ZERO, Color32::from_black_alpha(110)));
                    }
                }
            });
            ui.add_space(40.0);
        });
        if mine {
            self.clear_aliases_dialog(ui, &t, ctx);
        }
    }
}

/// Where a profile link goes.
enum Link {
    Page(&'static str, Option<String>),
    /// One of this profile's lists.
    Section(Section),
    Web(String),
}

impl Profile {
    /// Right column: level, links with counts, and friends.
    fn sidebar(&mut self, ui: &mut Ui, t: &Theme, ctx: &mut Ctx, id: SteamId, mine: bool, open: bool) {
        if !open {
            return;
        }
        let owned = ctx.state.apps.as_ref().map(|a| a.values().filter(|a| !a.is_dlc()).count());
        let games = match ctx.state.player_games.get(&id) {
            Some(Ok(g)) if mine && g.is_empty() => owned.map(|n| n.to_string()).unwrap_or_default(),
            Some(Ok(g)) => g.len().to_string(),
            Some(Err(_)) => "—".into(),
            None if mine => owned.map(|n| n.to_string()).unwrap_or_default(),
            None => String::new(),
        };
        let items: u64 = match ctx.state.inventory_apps.get(&id) {
            Some(Ok(apps)) => apps.iter().flat_map(|a| a.contexts.iter()).map(|c| c.2 as u64).sum(),
            _ => ctx.state.inventories.iter().filter(|(k, _)| k.0 == id).map(|(_, v)| v.total as u64).sum(),
        };
        let web = |path: &str| Link::Web(format!("https://steamcommunity.com/profiles/{id}/{path}"));
        let friends_link = if mine { Link::Page("community", None) } else { web("friends/") };
        let friends_count = if mine { ctx.state.friend_count().to_string() } else { String::new() };
        // Counts once those lists have loaded.
        let reviews = match ctx.state.reviews.get(&(id, 1)) {
            Some(Ok(r)) if r.total > 0 => r.total.to_string(),
            _ => String::new(),
        };
        let groups = match ctx.state.groups.get(&id) {
            Some(Ok(g)) if !g.is_empty() => g.len().to_string(),
            _ => String::new(),
        };
        let links: Vec<(&str, &str, String, Link)> = vec![
            (icons::BOOKS, t!("Games"), games, Link::Section(Section::Games)),
            (icons::BACKPACK, t!("Inventory"), if items > 0 { items.to_string() } else { String::new() }, Link::Page("inventory", Some(id.to_string()))),
            (icons::USERS, t!("Friends"), friends_count, friends_link),
            (icons::MEDAL, t!("Badges"), String::new(), Link::Section(Section::Badges)),
            (icons::CAMERA, t!("Screenshots"), String::new(), Link::Page("user_files", Some(format!("{id}/screenshots")))),
            (icons::CUBE, t!("Workshop items"), String::new(), Link::Page("user_files", Some(format!("{id}/workshop")))),
            (icons::BOOKS, t!("Guides"), String::new(), Link::Page("user_files", Some(format!("{id}/guides")))),
            (icons::THUMBS_UP, t!("Reviews"), reviews, Link::Section(Section::Reviews)),
            (icons::USERS_THREE, t!("Groups"), groups, Link::Section(Section::Groups)),
        ];
        panel(ui, t, |ui| {
            for (icon, label, count, link) in links {
                let external = matches!(link, Link::Web(_));
                let (r, resp) = ui.allocate_exact_size(vec2(ui.available_width(), 38.0), Sense::click());
                let hover = ui.ctx().animate_bool_with_time(resp.id, resp.hovered(), 0.1);
                let p = ui.painter();
                p.rect_filled(r, t.corner_small(), w::mix(Color32::TRANSPARENT, t.surface_hover, hover));
                p.text(pos2(r.left() + 14.0, r.center().y), Align2::CENTER_CENTER, icon, w::font(16.0), t.accent);
                p.text(pos2(r.left() + 34.0, r.center().y), Align2::LEFT_CENTER, label, w::font_medium(14.0), w::mix(t.text_dim, t.text, hover));
                if !count.is_empty() {
                    p.text(pos2(r.right() - 10.0, r.center().y), Align2::RIGHT_CENTER, &count, w::font_bold(14.0), t.text);
                } else if external {
                    p.text(pos2(r.right() - 10.0, r.center().y), Align2::RIGHT_CENTER, icons::EXTERNAL, w::font(12.0), t.text_faint);
                }
                let resp = resp.on_hover_cursor(egui::CursorIcon::PointingHand);
                let resp = if external { resp.on_hover_text(t!("Opens in your browser")) } else { resp };
                if resp.clicked() {
                    match link {
                        Link::Page(page, arg) => ctx.navigate(page, arg),
                        Link::Section(s) => ctx.navigate("profile", Some(format!("{id}/{}", s.slug()))),
                        Link::Web(url) => {
                            crate::links::open_http(&url);
                        }
                    }
                }
            }
        });
        if mine {
            let friends = ctx.state.friend_list();
            if !friends.is_empty() {
                ui.add_space(12.0);
                panel(ui, t, |ui| {
                    let online = friends.iter().filter(|p| p.state != model::PersonaState::Offline).count();
                    ui.horizontal(|ui| {
                        ui.label(w::text_bold(t!("Friends"), 16.0, t.text));
                        ui.label(w::text(t!("{n} online", n = online), 12.5, t.online));
                    });
                    ui.add_space(6.0);
                    for p in friends.iter().take(8) {
                        let (r, resp) = ui.allocate_exact_size(vec2(ui.available_width(), 40.0), Sense::click());
                        if resp.hovered() {
                            ui.painter().rect_filled(r, t.corner_small(), t.surface_hover);
                        }
                        let av = Rect::from_min_size(r.min + vec2(4.0, 4.0), vec2(32.0, 32.0));
                        if !w::paint_cover(ui, av, &art::avatar(p.avatar_hash.as_deref()), CornerRadius::same(4)) {
                            w::skeleton(ui, t, av, CornerRadius::same(4));
                        }
                        let color = crate::shell::status_color(t, p);
                        ui.painter().text(pos2(av.right() + 10.0, r.top() + 13.0), Align2::LEFT_CENTER, &p.name, w::font_semibold(13.0), t.text);
                        ui.painter().text(pos2(av.right() + 10.0, r.top() + 29.0), Align2::LEFT_CENTER, crate::shell::status_text(ctx.state, p), w::font(11.5), color);
                        if resp.on_hover_cursor(egui::CursorIcon::PointingHand).clicked() {
                            ctx.navigate("profile", Some(p.steam_id.to_string()));
                        }
                    }
                    if friends.len() > 8 && ui.link(w::text(t!("See all {n} friends", n = friends.len()), 12.5, t.accent)).clicked() {
                        ctx.navigate("community", None);
                    }
                });
            }
        }
    }

    fn request_social(&mut self, ctx: &mut Ctx, id: SteamId) {
        let have_badges = ctx.state.badges.contains_key(&id);
        let have_comments = ctx.state.comments.contains_key(&id);
        if have_badges && have_comments {
            self.social_requested.remove(&id);
        } else if self.social_requested.insert(id) {
            if !have_badges {
                ctx.send(Command::LoadBadges { steam_id: id });
            }
            if !have_comments {
                ctx.send(Command::LoadComments { steam_id: id, start: 0 });
            }
        }
        // For the review and group counts in the sidebar.
        if !ctx.state.reviews.contains_key(&(id, 1)) && self.lists_requested.insert((id, Section::Reviews, 1)) {
            ctx.send(Command::LoadReviews { steam_id: id, page: 1 });
        }
        if self.lists_requested.insert((id, Section::Groups, 0)) {
            ctx.send(Command::LoadGroups { steam_id: id });
        }
    }

    /// Profile comments, with posting. Respects profiles that hide or turn
    /// off comments (Steam answers with the reason, which is shown).
    fn comments(&mut self, ui: &mut Ui, t: &Theme, ctx: &mut Ctx, id: SteamId, mine: bool) {
        self.request_social(ctx, id);
        panel(ui, t, |ui| {
            let total = match ctx.state.comments.get(&id) {
                Some(Ok(c)) => c.total,
                _ => 0,
            };
            ui.horizontal(|ui| {
                ui.label(w::text_bold(t!("Comments"), 17.0, t.text));
                if total > 0 {
                    ui.label(w::text(total.to_string(), 14.0, t.text_dim));
                }
                ui.with_layout(Layout::right_to_left(Align::Center), |ui| {
                    if w::icon_button(ui, t, icons::REFRESH, t!("Refresh"), false, 0).clicked() {
                        self.comment_start.remove(&id);
                        ctx.send(Command::LoadComments { steam_id: id, start: 0 });
                    }
                });
            });
            ui.add_space(6.0);
            let data = match ctx.state.comments.get(&id) {
                None => {
                    for _ in 0..2 {
                        let (r, _) = ui.allocate_exact_size(vec2(ui.available_width(), 52.0), Sense::hover());
                        w::skeleton(ui, t, r.shrink2(vec2(0.0, 4.0)), t.corner_small());
                    }
                    return;
                }
                Some(Err(e)) => {
                    let why = if e.to_lowercase().contains("private") {
                        t!("This profile's comments are private.").to_string()
                    } else {
                        t!("Comments aren't available on this profile: {reason}", reason = i18n::tr_dyn(e))
                    };
                    ui.horizontal(|ui| {
                        ui.label(w::text(icons::LOCK_SIMPLE, 16.0, t.text_faint));
                        ui.label(w::text(why, 13.0, t.text_dim));
                    });
                    return;
                }
                Some(Ok(c)) => c.clone(),
            };
            // Composer.
            let hint = if mine { t!("Write a comment on your profile…") } else { t!("Add a comment…") };
            ui.add(egui::TextEdit::multiline(&mut self.comment_draft).hint_text(hint).desired_rows(2).desired_width(f32::INFINITY).margin(vec2(10.0, 8.0))).with_edit_menu();
            ui.add_space(4.0);
            ui.horizontal(|ui| {
                let text = self.comment_draft.trim().to_string();
                ui.add_enabled_ui(!text.is_empty() && text.chars().count() <= 1000, |ui| {
                    if w::button(ui, t, Some(icons::SEND), t!("Post comment"), w::Kind::Primary, w::Size::Sm).clicked() {
                        ctx.send(Command::PostComment { steam_id: id, text });
                    }
                });
                ui.label(w::text(format!("{}/1000", self.comment_draft.chars().count()), 11.5, t.text_faint));
            });
            ui.add_space(8.0);
            if data.items.is_empty() {
                ui.label(w::text(t!("No comments yet."), 13.0, t.text_faint));
            }
            // At most a page, even if Steam sends more (e.g. after posting).
            for c in data.items.iter().take(model::COMMENTS_PER_PAGE as usize) {
                ui.separator();
                ui.horizontal_top(|ui| {
                    let (av, resp) = ui.allocate_exact_size(vec2(36.0, 36.0), Sense::click());
                    match &c.avatar {
                        Some(u) => {
                            if !w::paint_cover(ui, av, u, CornerRadius::same(4)) {
                                w::skeleton(ui, t, av, CornerRadius::same(4));
                            }
                        }
                        None => w::placeholder(ui, t, av, "", CornerRadius::same(4)),
                    }
                    if resp.on_hover_cursor(egui::CursorIcon::PointingHand).clicked() {
                        ctx.navigate("profile", Some(c.author.to_string()));
                    }
                    ui.vertical(|ui| {
                        ui.spacing_mut().item_spacing.y = 2.0;
                        ui.horizontal(|ui| {
                            if ui.add(egui::Label::new(w::text_semibold(&c.name, 13.5, t.accent)).sense(Sense::click())).on_hover_cursor(egui::CursorIcon::PointingHand).clicked() {
                                ctx.navigate("profile", Some(c.author.to_string()));
                            }
                            if c.time > 0 {
                                ui.label(w::text(ui::ago(c.time), 11.5, t.text_faint));
                            }
                            // Steam lets you delete any comment on your profile, and your own anywhere.
                            if mine || c.author == ctx.state.steam_id {
                                ui.with_layout(Layout::right_to_left(Align::Center), |ui| {
                                    if w::icon_button(ui, t, icons::TRASH, t!("Delete comment"), false, 0).clicked() {
                                        ctx.send(Command::DeleteComment { steam_id: id, comment_id: c.id });
                                    }
                                });
                            }
                        });
                        match profile_style(ui) {
                            ProfileStyle::Classic => {
                                w::markup_label(ui, c.html.trim(), 13.5, t.text);
                            }
                            ProfileStyle::Steam(_) => {
                                let url = ui::steam_text::show(ui, t, c.html.trim(), ui::steam_text::Mode::Markup, 13.5, t.text);
                                showcases::follow(ctx, url);
                            }
                        }
                    });
                });
            }
            // Older and newer pages, 6 at a time.
            let per = model::COMMENTS_PER_PAGE;
            let start = self.comment_start.get(&id).copied().unwrap_or(0);
            if data.total as usize > data.items.len() {
                ui.add_space(4.0);
                ui.horizontal(|ui| {
                    let mut go = None;
                    ui.add_enabled_ui(start > 0, |ui| {
                        if w::button(ui, t, Some(icons::CARET_LEFT), t!("Newer"), w::Kind::Ghost, w::Size::Sm).clicked() {
                            go = Some(start.saturating_sub(per));
                        }
                    });
                    let page = start / per + 1;
                    let pages = data.total.div_ceil(per).max(1);
                    ui.label(w::text(t!("Page {page} of {pages}", page = page, pages = pages), 12.5, t.text_dim));
                    ui.add_enabled_ui(start + per < data.total, |ui| {
                        if w::button(ui, t, Some(icons::CARET_RIGHT), t!("Older"), w::Kind::Ghost, w::Size::Sm).clicked() {
                            go = Some(start + per);
                        }
                    });
                    if let Some(s) = go {
                        self.comment_start.insert(id, s);
                        ctx.send(Command::LoadComments { steam_id: id, start: s });
                    }
                });
            }
        });
    }

    /// Recently played games on someone else's profile, like Steam's.
    fn recent_games(&mut self, ui: &mut Ui, t: &Theme, ctx: &mut Ctx, id: SteamId) {
        let Some(result) = ctx.state.player_games.get(&id).cloned() else {
            panel(ui, t, |ui| {
                ui.label(w::text_bold(t!("Recent activity"), 17.0, t.text));
                for _ in 0..3 {
                    let (r, _) = ui.allocate_exact_size(vec2(ui.available_width(), 56.0), Sense::hover());
                    w::skeleton(ui, t, r.shrink2(vec2(0.0, 4.0)), t.corner_small());
                }
            });
            return;
        };
        let Ok(mut games) = result else { return };
        // Your own profile: fall back to this PC's play history.
        if games.is_empty() && id == ctx.state.steam_id {
            games = ctx
                .local
                .games
                .iter()
                .map(|g| model::PlayerGame { app_id: g.app_id, name: g.name.clone(), playtime_minutes: g.playtime_minutes.unwrap_or(0), playtime_2weeks: 0, last_played: g.last_played, icon: None })
                .collect();
        }
        let total_hours: u64 = games.iter().map(|g| g.playtime_minutes).sum::<u64>() / 60;
        let recent_minutes: u64 = games.iter().map(|g| g.playtime_2weeks).sum();
        games.sort_by_key(|g| std::cmp::Reverse((g.playtime_2weeks, g.last_played)));
        panel(ui, t, |ui| {
            ui.horizontal(|ui| {
                ui.label(w::text_bold(t!("Recent activity"), 17.0, t.text));
                ui.with_layout(Layout::right_to_left(Align::Center), |ui| {
                    ui.label(w::text(t!("{hours} hours past 2 weeks · {total} hours total", hours = format!("{:.1}", recent_minutes as f64 / 60.0), total = total_hours), 12.5, t.text_dim));
                });
            });
            ui.add_space(6.0);
            let shown: Vec<_> = games.iter().filter(|g| g.last_played.is_some() || g.playtime_2weeks > 0).take(4).collect();
            if shown.is_empty() {
                ui.label(w::text(t!("No recent games."), 13.5, t.text_faint));
            }
            for g in shown {
                game_row(ui, t, ctx, g);
            }
            ui.add_space(6.0);
            if ui.link(w::text(t!("See all {n} games", n = games.len()), 13.0, t.accent)).clicked() {
                ctx.navigate("profile", Some(format!("{id}/games")));
            }
        });
    }

    /// Full games list for a profile.
    fn games_page(&mut self, ui: &mut Ui, t: &Theme, ctx: &mut Ctx, id: SteamId, name: &str, mine: bool) {
        ui.horizontal(|ui| {
            if w::button(ui, t, Some(icons::ARROW_LEFT), t!("Back to profile"), w::Kind::Ghost, w::Size::Sm).clicked() {
                ctx.navigate("profile", Some(id.to_string()));
            }
        });
        ui.add_space(8.0);
        panel(ui, t, |ui| {
            let title = if mine { t!("Your games").to_string() } else { t!("{name}'s games", name = name) };
            ui.label(w::text_bold(title, 24.0, t.text));
            let games = match ctx.state.player_games.get(&id).cloned() {
                None => {
                    for _ in 0..8 {
                        let (r, _) = ui.allocate_exact_size(vec2(ui.available_width(), 56.0), Sense::hover());
                        w::skeleton(ui, t, r.shrink2(vec2(0.0, 4.0)), t.corner_small());
                    }
                    return;
                }
                Some(Err(e)) => {
                    let body = if e == "private" { t!("Its owner has chosen not to show their games.").to_string() } else { e };
                    w::empty(ui, t, icons::LOCK_SIMPLE, t!("These games are hidden"), &body);
                    return;
                }
                Some(Ok(g)) => g,
            };
            let hours: u64 = games.iter().map(|g| g.playtime_minutes).sum::<u64>() / 60;
            let shared = if mine { 0 } else { games.iter().filter(|g| ctx.state.apps.as_ref().is_some_and(|a| a.contains_key(&g.app_id))).count() };
            let mut sub = t!("{n} games · {hours} hours on record", n = games.len(), hours = hours);
            if !mine && shared > 0 {
                sub = format!("{sub} · {}", t!("{n} in common with you", n = shared));
            }
            ui.label(w::text(sub, 13.5, t.text_dim));
            ui.add_space(12.0);
            ui.horizontal(|ui| {
                w::search(ui, t, &mut self.games_search, &t!("Search games"), 280.0);
                ui.add_space(8.0);
                w::segmented(ui, t, &mut self.games_sort, &[(GameSort::Recent, &t!("Recently played")), (GameSort::Playtime, &t!("Playtime")), (GameSort::Name, &t!("Name"))]);
            });
            ui.add_space(10.0);
            let q = self.games_search.trim().to_lowercase();
            let mut list: Vec<_> = games.iter().filter(|g| q.is_empty() || g.name.to_lowercase().contains(&q)).collect();
            match self.games_sort {
                GameSort::Recent => list.sort_by_key(|g| std::cmp::Reverse(g.last_played.unwrap_or(0))),
                GameSort::Playtime => list.sort_by_key(|g| std::cmp::Reverse(g.playtime_minutes)),
                GameSort::Name => list.sort_by(|a, b| model::cmp_ignore_case(&a.name, &b.name)),
            }
            if list.is_empty() {
                ui.label(w::text(t!("No games match your search."), 13.5, t.text_faint));
            }
            let row_h = 62.0;
            egui::ScrollArea::vertical().id_salt(("profile-games", id)).max_height(ui.ctx().content_rect().height() - 260.0).auto_shrink([false, true]).show_rows(ui, row_h, list.len(), |ui, range| {
                for g in &list[range] {
                    game_row(ui, t, ctx, g);
                }
            });
        });
    }
}

/// One game with playtime; opens it in your library if you own it, else the store.
fn game_row(ui: &mut Ui, t: &Theme, ctx: &mut Ctx, g: &model::PlayerGame) {
    let (row, resp) = ui.allocate_exact_size(vec2(ui.available_width(), 62.0), Sense::click());
    let hover = ui.ctx().animate_bool_with_time(resp.id, resp.hovered(), 0.1);
    ui.painter().rect_filled(row, t.corner_small(), w::mix(Color32::TRANSPARENT, t.surface_hover, hover));
    let img = Rect::from_min_size(row.min + vec2(6.0, 8.0), vec2(98.0, 46.0));
    w::image_or_skeleton(ui, t, img, &art::header(g.app_id), t.corner_small());
    let owned = ctx.state.apps.as_ref().is_some_and(|a| a.contains_key(&g.app_id));
    let p = ui.painter();
    p.text(pos2(img.right() + 12.0, row.top() + 20.0), Align2::LEFT_CENTER, &g.name, w::font_semibold(14.0), t.text);
    let mut sub = t!("{time} on record", time = ui::playtime(g.playtime_minutes));
    if g.playtime_2weeks > 0 {
        sub = format!("{sub} · {}", t!("{time} past 2 weeks", time = ui::playtime(g.playtime_2weeks)));
    }
    if let Some(when) = g.last_played {
        sub = format!("{sub} · {}", t!("last played {when}", when = ui::ago(when)));
    }
    p.text(pos2(img.right() + 12.0, row.top() + 42.0), Align2::LEFT_CENTER, sub, w::font(12.0), t.text_dim);
    if owned {
        p.text(pos2(row.right() - 12.0, row.center().y), Align2::RIGHT_CENTER, format!("{}  {}", icons::CHECK, t!("In your library")), w::font_semibold(12.0), t.online);
    }
    if resp.on_hover_cursor(egui::CursorIcon::PointingHand).clicked() {
        if owned {
            ctx.navigate("library", Some(g.app_id.to_string()));
        } else {
            ctx.navigate("store", Some(format!("app:{}", g.app_id)));
        }
    }
}

/// Steam's profile grid, in CSS pixels (= points): the page column, the
/// gutters, the right column and the header's minimum height. The left
/// column is what's left: 976 − 3 × 12 − 288 = 652.
const PAGE_W: f32 = 976.0;
const GUTTER: f32 = 12.0;
const RIGHT_W: f32 = 288.0;
const HEADER_H: f32 = 230.0;
/// Space under the name and location before the summary (Steam's
/// `.profile_header_centered_persona { padding-bottom: 18px }`).
const PERSONA_PADDING_BOTTOM: f32 = 18.0;

/// The border behind a profile picture without a frame, in Steam's status
/// colors (dimmed, as its gradient border reads).
fn avatar_border(t: &Theme, p: &model::Persona) -> Color32 {
    crate::shell::status_color(t, p).gamma_multiply(0.55)
}

/// Frame-rate cap for animated avatars and avatar frames.
const AVATAR_FPS: f32 = 24.0;

/// Steam's summary (profilev2.css + InitProfileSummary): 13 px text on
/// 18 px lines; shown whole when it's at most 76 px tall, otherwise cut to
/// 57 px with "View more info".
const SUMMARY_TEXT: f32 = 13.0;
const SUMMARY_FITS: f32 = 76.0;
const SUMMARY_CLIPPED: f32 = 57.0;

/// The profile summary, cut to Steam's height. When it runs longer,
/// "View more info" opens the whole of it in an "Info" dialog, like Steam.
fn summary(ui: &mut Ui, t: &Theme, ctx: &mut Ctx, id: SteamId, text: &str) {
    let open_id = egui::Id::unique(("aurora-profile-summary-open", id));
    let top = ui.cursor().min;
    let width = ui.available_width();
    // Steam's 76 px / 57 px on its fixed 18 px lines are 4 and 3 lines. Our
    // rows vary with the font and inline pictures, so lines are counted:
    // the text's own lines, or (when it wraps) last frame's height in rows.
    let height_id = egui::Id::unique(("aurora-profile-summary-height", id));
    let measured = ui.ctx().data(|d| d.get_temp::<f32>(height_id)).unwrap_or(0.0);
    let lines = ui::steam_text::line_count(&ui::steam_text::parse(text, ui::steam_text::Mode::Markup)).max(1);
    let row = ui.text_style_height(&egui::TextStyle::Body).max(SUMMARY_TEXT * 1.35) + 2.0;
    let rows = lines.max((measured / row).floor() as usize);
    let long = rows > (SUMMARY_FITS / 18.0) as usize;
    let per_line = if measured > 0.0 { measured / rows as f32 } else { row };
    let shown_max = if long { per_line * (SUMMARY_CLIPPED / 18.0).round() + 1.0 } else { f32::INFINITY };
    let mut child = ui.new_child(egui::UiBuilder::new().max_rect(Rect::from_min_size(top, vec2(width, ui.available_height().max(1.0)))).layout(Layout::top_down(Align::Min)));
    child.set_clip_rect(child.clip_rect().intersect(Rect::from_min_size(top, vec2(width, shown_max))));
    let url = ui::steam_text::show(&mut child, t, text, ui::steam_text::Mode::Markup, SUMMARY_TEXT, t.text);
    let full = child.min_rect().height();
    ui.ctx().data_mut(|d| d.insert_temp(height_id, full));
    ui.allocate_rect(Rect::from_min_size(top, vec2(width, full.min(shown_max))), Sense::hover());
    showcases::follow(ctx, url);
    if !long {
        return;
    }
    ui.add_space(6.0);
    let r = ui.add(egui::Label::new(w::text(t!("View more info"), 13.0, t.text_dim)).sense(Sense::click())).on_hover_cursor(egui::CursorIcon::PointingHand);
    if r.clicked() {
        ui.ctx().data_mut(|d| d.insert_temp(open_id, true));
    }
    if !ui.ctx().data(|d| d.get_temp::<bool>(open_id)).unwrap_or(false) {
        return;
    }
    let mut close = ui.input(|i| i.key_pressed(egui::Key::Escape));
    let mut follow = None;
    let modal = w::modal(ui.ctx(), egui::Id::unique(("aurora-profile-summary-modal", id))).show(ui.ctx(), |ui| {
        ui.set_width(620.0);
        ui.horizontal(|ui| {
            ui.label(w::text_bold(t!("Info"), 20.0, t.text));
            ui.with_layout(Layout::right_to_left(Align::Center), |ui| {
                if w::icon_button(ui, t, icons::X, t!("Close"), false, 0).clicked() {
                    close = true;
                }
            });
        });
        ui.add_space(10.0);
        egui::ScrollArea::vertical().max_height(480.0).auto_shrink([false, true]).show(ui, |ui| {
            follow = ui::steam_text::show(ui, t, text, ui::steam_text::Mode::Markup, 15.0, t.text);
        });
    });
    if modal.should_close() {
        close = true;
    }
    if follow.is_some() {
        close = true;
        showcases::follow(ctx, follow);
    }
    if close {
        ui.ctx().data_mut(|d| d.remove::<bool>(open_id));
    }
}

/// The layout and Steam theme used while drawing the profile.
#[derive(Clone, Copy, Default)]
enum ProfileStyle {
    #[default]
    Classic,
    Steam(Option<themes::SteamTheme>),
}

fn profile_style_id() -> egui::Id {
    egui::Id::unique("aurora-profile-style")
}

fn profile_style(ui: &Ui) -> ProfileStyle {
    ui.ctx().data(|d| d.get_temp(profile_style_id()).unwrap_or_default())
}

/// The shown profile's Steam theme, if it has one.
pub(super) fn profile_theme(ui: &Ui) -> Option<themes::SteamTheme> {
    match profile_style(ui) {
        ProfileStyle::Classic => None,
        ProfileStyle::Steam(theme) => theme,
    }
}

/// A section title: on a themed profile, on Steam's showcase header bar.
pub(super) fn section_title(ui: &mut Ui, t: &Theme, title: &str) {
    match profile_theme(ui) {
        Some(theme) => {
            // Steam's bar runs edge to edge across the panel's top, over its
            // padding; it's always the panel's first item.
            let (r, _) = ui.allocate_exact_size(vec2(ui.available_width(), 30.0), Sense::hover());
            let bar = Rect::from_min_max(pos2(r.left() - PANEL_PAD, r.top() - PANEL_PAD), pos2(r.right() + PANEL_PAD, r.bottom()));
            w::gradient(ui.painter(), bar, theme.showcase_header_left, theme.showcase_header, true);
            ui.painter().text(pos2(bar.left() + 10.0, bar.center().y), Align2::LEFT_CENTER, title, w::font(16.0), Color32::WHITE);
            ui.add_space(PANEL_PAD);
        }
        None => {
            ui.label(w::text_bold(title, 17.0, t.text));
        }
    }
}

/// Translucent panel so the profile background shows through.
pub(super) fn panel(ui: &mut Ui, t: &Theme, add: impl FnOnce(&mut Ui)) {
    // A themed profile's panels take its background color (a game profile
    // may make them see-through, as Steam does).
    // Steam's panels: square, 10 px padding, 12 px apart.
    let style = profile_style(ui);
    let (fill, stroke, radius) = match style {
        ProfileStyle::Steam(Some(theme)) => (theme.panel_fill(), Stroke::NONE, CornerRadius::ZERO),
        _ => (t.chrome.gamma_multiply(0.85), Stroke::new(1.0, t.border), t.corner()),
    };
    let padding = match style {
        ProfileStyle::Classic => 18,
        ProfileStyle::Steam(_) => PANEL_PAD as i8,
    };
    egui::Frame::new()
        .fill(fill)
        .stroke(stroke)
        .corner_radius(radius)
        .inner_margin(Margin::same(padding))
        .show(ui, |ui| {
            ui.set_width(ui.available_width());
            add(ui);
        });
}

/// Padding inside profile panels, as on Steam.
const PANEL_PAD: f32 = 10.0;

/// Steam-style level circle; color steps every 10 levels.
fn level_badge(ui: &mut Ui, t: &Theme, level: u32) {
    let colors = [0x9B9B9B, 0xC02942, 0xD95B43, 0xFECC23, 0x467A3C, 0x4E8DDB, 0x7652C9, 0xC252C9, 0x542437, 0x997C52];
    let c = colors[(level as usize / 10) % colors.len()];
    let color = Color32::from_rgb((c >> 16) as u8, (c >> 8) as u8, c as u8);
    let (r, _) = ui.allocate_exact_size(vec2(38.0, 38.0), Sense::hover());
    ui.painter().circle_stroke(r.center(), 16.0, Stroke::new(2.5, color));
    ui.painter().text(r.center(), Align2::CENTER_CENTER, level.to_string(), w::font_bold(13.5), t.text);
}

/// The game and foil flag of a badge crafted from a card collection.
pub(crate) fn card_set_of(url: &str) -> Option<(model::AppId, bool)> {
    let rest = url.split("/gamecards/").nth(1)?;
    let app = rest.split(|c: char| !c.is_ascii_digit()).next()?.parse().ok()?;
    Some((app, rest.contains("border=1")))
}

/// A badge from a card collection opens that collection here; others
/// open their page on Steam.
fn open_badge(ctx: &mut Ctx, id: SteamId, badge: &model::Badge) {
    match card_set_of(&badge.url) {
        Some((app, foil)) => ctx.navigate("profile", Some(format!("{id}/cards/{app}{}", if foil { "/foil" } else { "" }))),
        None if !badge.url.is_empty() => {
            crate::links::open_http(&badge.url);
        }
        None => {}
    }
}

/// A badge picture with its name and details on hover; true when clicked.
fn badge_icon(ui: &mut Ui, t: &Theme, badge: &model::Badge, size: f32) -> bool {
    let (r, resp) = ui.allocate_exact_size(vec2(size, size), Sense::click());
    match &badge.icon {
        Some(u) => {
            if !w::paint_contain(ui, r, u, Align2::CENTER_CENTER) {
                w::skeleton(ui, t, r, CornerRadius::same(6));
            }
        }
        None => w::placeholder(ui, t, r, "", CornerRadius::same(6)),
    }
    let tip = [badge.title.as_str(), badge.detail.as_str(), badge.unlocked.as_str()].into_iter().filter(|s| !s.is_empty()).collect::<Vec<_>>().join("\n");
    resp.on_hover_text(tip).on_hover_cursor(egui::CursorIcon::PointingHand).clicked()
}

#[cfg(test)]
mod tests {
    use extensions::Extension;

    #[test]
    fn only_steam_layout_requests_steam_profile_page_and_showcases() {
        let flags = super::super::settings::SharedFlags::default();
        let mut profile = super::Profile::new(flags.clone());
        let egui = egui::Context::default();
        ui::install_fonts(&egui);
        let (tx, mut rx) = tokio::sync::mpsc::unbounded_channel();
        let client = client::Client::from_sender(tx);
        let state = client::State { steam_id: 76561198000000000, ..Default::default() };
        let local = local::LocalState::default();
        let theme = ui::Theme::default();
        let mut registry = extensions::Registry::default();
        let mut actions = Vec::new();

        for (steam_style, profile_arg) in [(false, None), (true, None), (false, Some("76561198000000001"))] {
            flags.lock().unwrap().steam_profile_ui = steam_style;
            profile.on_navigate("profile", profile_arg);
            let mut ctx = extensions::Ctx::new(&state, &local, &theme, 1.0, &client, &mut registry, &mut actions);
            let mut output = egui.run_ui(egui::RawInput::default(), |ui| profile.page("profile", ui, &mut ctx));
            output.textures_delta.clear();
            let mut page = false;
            let mut showcases = false;
            while let Ok(command) = rx.try_recv() {
                match command {
                    client::Command::LoadProfilePage { .. } => page = true,
                    client::Command::LoadShowcases { .. } => showcases = true,
                    _ => {}
                }
            }
            assert_eq!((page, showcases), (steam_style, steam_style));
        }
    }

    #[test]
    fn badges_from_card_collections() {
        assert_eq!(super::card_set_of("https://steamcommunity.com/id/x/gamecards/570/"), Some((570, false)));
        assert_eq!(super::card_set_of("https://steamcommunity.com/profiles/1/gamecards/20920/?border=1"), Some((20920, true)));
        assert_eq!(super::card_set_of("https://steamcommunity.com/id/x/badges/13"), None);
    }

    #[test]
    fn navigates_to_edit_and_own_sections() {
        let mut p = super::Profile::default();
        p.on_navigate("profile", Some("edit"));
        assert!(p.viewing.is_none() && p.section == super::Section::Edit && p.edit.section == super::edit::Section::General);
        p.on_navigate("profile", Some("edit/privacy"));
        assert!(p.viewing.is_none() && p.section == super::Section::Edit && p.edit.section == super::edit::Section::Privacy);
        p.on_navigate("profile", Some("edit/theme"));
        assert!(p.viewing.is_none() && p.section == super::Section::Edit && p.edit.section == super::edit::Section::Theme);
        p.on_navigate("profile", Some("badges"));
        assert!(p.viewing.is_none() && p.section == super::Section::Badges);
        p.on_navigate("profile", Some("76561197960265729/games"));
        assert!(p.viewing == Some(76561197960265729) && p.section == super::Section::Games);
        p.on_navigate("profile", None);
        assert!(p.viewing.is_none() && p.section == super::Section::Profile);
    }
}
