//! Settings: account, appearance, plugins, extensions, overlay, library folders, about.

use i18n::t;
use client::Command;
use egui::{Align, Layout, Margin, Sense, Ui, vec2};
use extensions::{Action, Ctx, Extension, Manifest, PageSpec};
use ui::{Theme, icons, widgets as w};

#[derive(Clone, Copy, PartialEq, Eq, Default)]
enum Section {
    #[default]
    Account,
    Friends,
    Family,
    Security,
    Notifications,
    Interface,
    Appearance,
    Store,
    Library,
    Downloads,
    Folders,
    Cloud,
    Overlay,
    Controller,
    Accessibility,
    Plugins,
    About,
}

/// Steam's settings list, in Steam's order; `None` draws a divider.
const SECTIONS: [Option<(Section, &str, &str)>; 20] = [
    Some((Section::Account, icons::USER_CIRCLE, "Account")),
    Some((Section::Friends, icons::USERS, "Friends & Chat")),
    Some((Section::Family, icons::USERS_THREE, "Family")),
    Some((Section::Security, icons::SHIELD, "Security")),
    None,
    Some((Section::Notifications, icons::BELL, "Notifications")),
    Some((Section::Interface, icons::MONITOR, "Interface")),
    Some((Section::Appearance, icons::PALETTE, "Appearance")),
    Some((Section::Store, icons::STORE, "Store")),
    Some((Section::Library, icons::BOOKS, "Library")),
    Some((Section::Downloads, icons::DOWNLOAD, "Downloads")),
    Some((Section::Folders, icons::DRIVES, "Storage")),
    Some((Section::Cloud, icons::CLOUD_CHECK, "Cloud")),
    Some((Section::Overlay, icons::CONTROLLER, "In game")),
    Some((Section::Controller, icons::CONTROLLER, "Controller")),
    Some((Section::Accessibility, icons::EYE, "Accessibility")),
    None,
    Some((Section::Plugins, icons::PLUGS, "Plugins")),
    Some((Section::About, icons::INFO, "About")),
    None,
];

/// App-level options the Settings page edits (owned by the app, shared here).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default, serde::Serialize, serde::Deserialize)]
pub enum FpsCorner {
    #[default]
    Off,
    TopLeft,
    TopRight,
    BottomLeft,
    BottomRight,
}

impl FpsCorner {
    pub const ALL: [FpsCorner; 5] = [FpsCorner::Off, FpsCorner::TopLeft, FpsCorner::TopRight, FpsCorner::BottomLeft, FpsCorner::BottomRight];

    pub fn label(self) -> &'static str {
        match self {
            FpsCorner::Off => t!("Off"),
            FpsCorner::TopLeft => t!("Top-left"),
            FpsCorner::TopRight => t!("Top-right"),
            FpsCorner::BottomLeft => t!("Bottom-left"),
            FpsCorner::BottomRight => t!("Bottom-right"),
        }
    }
}

#[derive(Debug, Clone, Copy, Default)]
pub struct AppFlags {
    pub close_to_tray: bool,
    pub ask_account: bool,
    /// Master switch: keep games updated automatically (like Steam's).
    pub auto_updates: bool,
    /// Steam Cloud for every game that supports it.
    pub cloud: bool,
    /// Pop-ups for new chat messages / Steam notifications.
    pub toast_messages: bool,
    pub toast_notifications: bool,
    /// Multiple of egui's scroll distance per wheel notch / touchpad move.
    pub scroll_speed: f32,
    /// Glide each mouse-wheel notch over a few frames (off: jump at once).
    pub smooth_scroll: bool,
    /// Draw the main window at most 60 times a second.
    pub cap_fps: bool,
    /// The one-time Steam-DRM launch notice has been shown and accepted.
    pub drm_host_ack: bool,
    /// Enable the in-game overlay.
    pub overlay_enabled: bool,
    /// Where to draw the in-game FPS counter.
    pub fps_counter: FpsCorner,
    /// High-contrast green color for the in-game FPS counter.
    pub fps_high_contrast: bool,
    /// Show the "What's new" news shelf on the library home.
    pub show_whats_new: bool,
    pub steam_profile_ui: bool,
}

pub type SharedFlags = std::sync::Arc<std::sync::Mutex<AppFlags>>;

pub struct Settings {
    section: Section,
    /// Account details, purchase history, licenses, family.
    acct: super::settings_account::AccountPages,
    controller: super::settings_controller::ControllerPage,
    flags: SharedFlags,
    themes: crate::app::SharedThemeManager,
    confirm_uninstall: Option<String>,
    option_hex_bufs: std::collections::HashMap<String, String>,
    /// Cached "are we the steam:// handler?" (registry lookups are slow).
    handler: Option<bool>,
    restore: Option<super::storage_ui::StorageDialog>,
}

impl Settings {
    pub fn new(flags: SharedFlags, themes: crate::app::SharedThemeManager) -> Self {
        Self {
            section: Section::default(),
            acct: Default::default(),
            controller: Default::default(),
            flags,
            themes,
            confirm_uninstall: None,
            option_hex_bufs: Default::default(),
            handler: None,
            restore: None,
        }
    }
}

impl Extension for Settings {
    fn manifest(&self) -> Manifest {
        Manifest { id: "settings", name: "Settings", author: "Aurora", description: "Appearance, plugins, account", core: true, ..Manifest::DEFAULT }
    }

    fn pages(&self) -> Vec<PageSpec> {
        vec![PageSpec::new("settings", "Settings", icons::GEAR).hidden().order(1000)]
    }

    fn on_navigate(&mut self, _page: &str, arg: Option<&str>) {
        use super::settings_account::AccountView;
        match arg {
            Some("plugins") => self.section = Section::Plugins,
            Some("family") => self.section = Section::Family,
            Some("security") => self.section = Section::Security,
            Some(a) if a == "controller" || a.starts_with("controller:") => {
                self.section = Section::Controller;
                self.controller.open_arg(a);
            }
            Some(a @ ("history" | "licenses" | "account-details")) => {
                self.section = Section::Account;
                self.acct.open(match a {
                    "history" => AccountView::Purchases,
                    "licenses" => AccountView::Licenses,
                    _ => AccountView::Details,
                });
            }
            _ => {}
        }
    }

    fn on_event(&mut self, event: &client::Event, _ctx: &mut Ctx) {
        self.controller.on_event(event);
    }

    fn page(&mut self, _page: &str, ui: &mut Ui, ctx: &mut Ctx) {
        let t = ctx.theme.clone();
        egui::Panel::left("settings-nav")
            .exact_size(240.0)
            .resizable(false)
            .frame(egui::Frame::new().fill(t.chrome).inner_margin(Margin { left: 14, right: 10, top: 22, bottom: 10 }))
            .show(ui, |ui| {
                ui.label(w::text_bold(t!("Settings"), 18.0, t.text));
                ui.add_space(12.0);
                egui::ScrollArea::vertical().show(ui, |ui| {
                    for entry in SECTIONS {
                        let Some((s, icon, label)) = entry else {
                            ui.add_space(4.0);
                            let (r, _) = ui.allocate_exact_size(vec2(ui.available_width(), 1.0), Sense::hover());
                            ui.painter().hline(r.x_range(), r.center().y, egui::Stroke::new(1.0, t.border));
                            ui.add_space(4.0);
                            continue;
                        };
                        let on = self.section == s;
                        let (r, resp) = ui.allocate_exact_size(vec2(ui.available_width(), 34.0), Sense::click());
                        if on || resp.hovered() {
                            ui.painter().rect_filled(r, t.corner_small(), if on { w::active_fill(&t) } else { t.raised });
                        }
                        let c = if on { t.accent } else if resp.hovered() { t.text } else { t.text_dim };
                        ui.painter().text(egui::pos2(r.left() + 18.0, r.center().y), egui::Align2::CENTER_CENTER, icon, w::font(16.0), c);
                        ui.painter().text(egui::pos2(r.left() + 40.0, r.center().y), egui::Align2::LEFT_CENTER, i18n::tr_dyn(label), w::font_medium(13.5), c);
                        if resp.on_hover_cursor(egui::CursorIcon::PointingHand).clicked() {
                            self.section = s;
                            self.acct.open(Default::default());
                        }
                    }
                });
            });
        egui::CentralPanel::default().frame(egui::Frame::new().fill(t.bg)).show(ui, |ui| {
            egui::ScrollArea::vertical().auto_shrink([false, false]).show(ui, |ui| {
                ui.add_space(28.0);
                w::centered(ui, 820.0, 32.0, |ui| match self.section {
                    Section::Account if self.acct.view != Default::default() => self.acct.show(ui, &t, ctx),
                    Section::Account => account(ui, &t, ctx, &mut self.acct),
                    Section::Friends => friends_chat(ui, &t, ctx),
                    Section::Family => self.acct.family(ui, &t, ctx),
                    Section::Security => security(ui, &t, ctx, &mut self.acct),
                    Section::Notifications => notifications(ui, &t, &self.flags),
                    Section::Library => library(ui, &t, ctx, &self.flags),
                    Section::Downloads => downloads(ui, &t, ctx, &self.flags),
                    Section::Cloud => cloud(ui, &t, &self.flags),
                    Section::Accessibility => accessibility(ui, &t, ctx, &self.flags),
                    Section::Appearance => appearance(ui, &t, ctx, &self.themes, &mut self.confirm_uninstall, &mut self.option_hex_bufs),
                    Section::Plugins => {
                        w::page_title(ui, &t, t!("Plugins"), Some(t!("Scripts you install yourself. Each one runs in a sandbox and only gets the permissions you approve.")));
                        ctx.settings_for("plugins", ui);
                    }
                    Section::Interface => interface(ui, &t, ctx, &self.flags, &mut self.handler),
                    Section::Store => {
                        w::page_title(ui, &t, t!("Store"), None);
                        w::card(&t).show(ui, |ui| {
                            ui.set_width(ui.available_width());
                            ctx.settings_for("store", ui);
                        });
                    }
                    Section::Overlay => overlay(ui, &t, ctx, &self.flags),
                    Section::Controller => self.controller.show(ui, &t, ctx),
                    Section::Folders => folders(ui, &t, ctx, &mut self.restore),
                    Section::About => about(ui, &t),
                });
                ui.add_space(40.0);
            });
        });
    }
}

/// A row like Steam's settings: label, value, and a button on the right.
pub(super) fn info_row(ui: &mut Ui, t: &Theme, label: &str, value: &str, button: Option<&str>) -> bool {
    let mut clicked = false;
    egui::Frame::new().fill(t.surface).corner_radius(t.corner_small()).inner_margin(Margin::symmetric(14, 12)).show(ui, |ui| {
        ui.set_width(ui.available_width());
        ui.horizontal(|ui| {
            ui.label(w::text_medium(label, 14.0, t.text));
            ui.add_space(8.0);
            ui.label(w::text(value, 13.0, t.text_dim));
            if let Some(b) = button {
                ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                    clicked = w::button(ui, t, None, b, w::Kind::Secondary, w::Size::Sm).clicked();
                });
            }
        });
    });
    ui.add_space(6.0);
    clicked
}

/// A big square shortcut tile (icon, title, line under it).
fn tile(ui: &mut Ui, t: &Theme, icon: &str, title: &str, sub: &str, width: f32) -> bool {
    let (r, resp) = ui.allocate_exact_size(vec2(width, 132.0), Sense::click());
    let hover = ui.ctx().animate_bool_with_time(resp.id, resp.hovered(), 0.1);
    ui.painter().rect_filled(r, t.corner_small(), w::mix(t.surface, t.surface_hover, hover));
    ui.painter().text(egui::pos2(r.center().x, r.top() + 42.0), egui::Align2::CENTER_CENTER, icon, w::font(34.0), t.text);
    ui.painter().text(egui::pos2(r.center().x, r.top() + 84.0), egui::Align2::CENTER_CENTER, title, w::font_semibold(14.5), t.text);
    let g = ui.painter().layout(sub.to_string(), w::font(12.0), t.text_dim, width - 20.0);
    ui.painter().galley(egui::pos2(r.center().x - g.size().x / 2.0, r.top() + 98.0), g, t.text_dim);
    resp.on_hover_cursor(egui::CursorIcon::PointingHand).clicked()
}

pub(super) fn web(url: &str) {
    crate::links::open_http(url);
}

/// Like Steam's Account page.
fn account(ui: &mut Ui, t: &Theme, ctx: &mut Ctx, pages: &mut super::settings_account::AccountPages) {
    use super::settings_account::AccountView;
    w::page_title(ui, t, t!("Account"), None);
    let me = ctx.state.persona(ctx.state.steam_id);
    let name = if ctx.state.persona_name.is_empty() { me.name.clone() } else { ctx.state.persona_name.clone() };
    ui.horizontal(|ui| {
        w::avatar(ui, t, &model::art::avatar_full(me.avatar_hash.as_deref()), 60.0, None);
        ui.vertical(|ui| {
            ui.add_space(8.0);
            ui.label(w::text_semibold(&name, 16.0, t.accent));
            ui.label(w::text(i18n::tr_dyn(me.state.label()), 13.0, crate::shell::status_color(t, &me)));
        });
        ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
            if w::button(ui, t, None, t!("View profile"), w::Kind::Secondary, w::Size::Md).clicked() {
                ctx.navigate("profile", None);
            }
        });
    });
    ui.add_space(14.0);
    let width = (ui.available_width() - 16.0) / 3.0;
    let balance = ctx.state.wallet.map(|(b, c)| model::format_money(b, c)).unwrap_or_else(|| "—".into());
    ui.horizontal(|ui| {
        ui.spacing_mut().item_spacing.x = 8.0;
        if tile(ui, t, icons::COINS, t!("Add funds"), &t!("Current balance: {amount}", amount = balance), width) {
            web("https://store.steampowered.com/steamaccount/addfunds");
        }
        if tile(ui, t, icons::EYE_SLASH, t!("Privacy settings"), t!("Profile, games, inventory and more"), width) {
            ctx.navigate("profile", Some("edit/privacy".into()));
        }
        if tile(ui, t, icons::NOTE, t!("Account details"), t!("Contact, Steam Guard, password, devices"), width) {
            pages.open(AccountView::Details);
        }
    });
    ui.add_space(10.0);
    if info_row(ui, t, t!("Purchase history"), t!("Purchases, gifts, refunds, wallet and Market"), Some(t!("View"))) {
        pages.open(AccountView::Purchases);
    }
    if info_row(ui, t, t!("Licenses and product key activations"), t!("Everything on your account and how you got it"), Some(t!("View"))) {
        pages.open(AccountView::Licenses);
    }
    let code = (ctx.state.steam_id & 0xFFFF_FFFF).to_string();
    if info_row(ui, t, t!("Your friend code"), &code, Some(t!("Add friends"))) {
        ctx.navigate("community", None);
    }
    let email = ctx.state.email.as_ref().map(|(e, ok)| if *ok { e.clone() } else { format!("{e} ({})", t!("not verified")) }).unwrap_or_else(|| "—".into());
    if info_row(ui, t, t!("Email address"), &email, Some(t!("Change email address"))) {
        web("https://help.steampowered.com/wizard/HelpChangeEmail");
    }
    let vac = match ctx.state.profiles.get(&ctx.state.steam_id) {
        Some(p) if p.vac_banned => t!("VAC bans on record").to_string(),
        Some(_) => t!("No VAC bans").to_string(),
        None => "…".into(),
    };
    if info_row(ui, t, t!("VAC status"), &vac, Some(t!("Learn more"))) {
        web("https://help.steampowered.com/faqs/view/647C-5CC1-7EA9-3C29");
    }
    ui.add_space(8.0);
    ui.horizontal(|ui| {
        ui.label(w::text(format!("{} · {} · {}", ctx.state.account, ctx.state.steam_id, ctx.state.country), 12.0, t.text_faint));
        ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
            if w::button(ui, t, Some(icons::SIGN_OUT), t!("Sign out"), w::Kind::Danger, w::Size::Sm).clicked() {
                ctx.send(Command::LogOut);
            }
        });
    });
}

fn friends_chat(ui: &mut Ui, t: &Theme, ctx: &mut Ctx) {
    w::page_title(ui, t, t!("Friends & Chat"), None);
    let code = (ctx.state.steam_id & 0xFFFF_FFFF).to_string();
    if info_row(ui, t, t!("Your friend code"), &code, Some(t!("Copy"))) {
        ui.ctx().copy_text(code);
        ctx.toast(t!("Friend code copied"));
    }
    if info_row(ui, t, t!("Friends"), &ctx.state.friend_count().to_string(), Some(t!("Open friends list"))) {
        ctx.navigate("community", None);
    }
    if info_row(ui, t, t!("Who can see your status and send invites"), "", Some(t!("Privacy settings"))) {
        ctx.navigate("profile", Some("edit/privacy".into()));
    }
}


fn security(ui: &mut Ui, t: &Theme, ctx: &mut Ctx, pages: &mut super::settings_account::AccountPages) {
    w::page_title(ui, t, t!("Security"), None);
    pages.ask_security(ctx);
    let (guard, password) = match &ctx.state.account_security {
        Some(Ok(s)) => (
            if s.authenticator {
                t!("Steam Guard Mobile Authenticator is on").to_string()
            } else if s.steam_guard {
                t!("Codes by email are on").to_string()
            } else {
                t!("Off: your account isn't protected").to_string()
            },
            match (s.password_changed, s.password_reset) {
                (Some(c), _) => t!("Last changed: {date}", date = ui::date(c)).to_string(),
                (None, Some(r)) => t!("Last reset: {date}", date = ui::date(r)).to_string(),
                _ => String::new(),
            },
        ),
        _ => (t!("Protects your account with codes from the Steam Mobile app").to_string(), String::new()),
    };
    if info_row(ui, t, t!("Steam Guard"), &guard, Some(t!("Manage Steam Guard"))) {
        web("https://store.steampowered.com/twofactor/manage");
    }
    if info_row(ui, t, t!("Authorized devices"), t!("See and remove devices signed in to your account"), Some(t!("Manage devices"))) {
        web("https://store.steampowered.com/account/authorizeddevices");
    }
    if info_row(ui, t, t!("Password"), &password, Some(t!("Change password"))) {
        web("https://help.steampowered.com/wizard/HelpChangePassword");
    }
    if info_row(ui, t, t!("This PC"), t!("Saved sign-in for this account is stored on this PC"), Some(t!("Sign out"))) {
        ctx.send(Command::LogOut);
    }
}

pub(crate) fn switch(ui: &mut Ui, t: &Theme, title: &str, body: &str, value: &mut bool) {
    egui::Frame::new().fill(t.surface).corner_radius(t.corner_small()).inner_margin(Margin::symmetric(14, 12)).show(ui, |ui| {
        ui.set_width(ui.available_width());
        ui.horizontal(|ui| {
            ui.vertical(|ui| {
                ui.label(w::text_medium(title, 14.0, t.text));
                if !body.is_empty() {
                    ui.add(egui::Label::new(w::text(body, 12.5, t.text_dim)).wrap());
                }
            });
            ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                ui.add(toggle(value));
            });
        });
    });
    ui.add_space(6.0);
}

fn notifications(ui: &mut Ui, t: &Theme, flags: &SharedFlags) {
    w::page_title(ui, t, t!("Notifications"), None);
    let mut f = *flags.lock().unwrap();
    switch(ui, t, t!("New chat messages"), t!("Show a pop-up when a friend messages you."), &mut f.toast_messages);
    switch(ui, t, t!("Steam notifications"), t!("Show a pop-up for gifts, items, friend requests, sales and more."), &mut f.toast_notifications);
    *flags.lock().unwrap() = f;
}

fn library(ui: &mut Ui, t: &Theme, ctx: &mut Ctx, flags: &SharedFlags) {
    w::page_title(ui, t, t!("Library"), None);
    let mut steam_profile_ui = flags.lock().unwrap().steam_profile_ui;
    switch(
        ui,
        t,
        t!("Steam-style profile page"),
        t!("Use the new Steam-style profile layout instead of Aurora's classic profile view."),
        &mut steam_profile_ui,
    );
    flags.lock().unwrap().steam_profile_ui = steam_profile_ui;
    ctx.settings_for("library", ui);
    if info_row(ui, t, t!("Add a non-Steam game"), "", Some(t!("Open library"))) {
        ctx.navigate("library", None);
    }
    let n = ctx.local.libraries.len();
    if info_row(ui, t, t!("Library folders"), &t!("{n} folders", n = n), Some(t!("Add library folder…"))) {
        add_library_folder(ctx);
    }
    #[cfg(not(windows))]
    proton_version(ui, t);
}

/// Linux: pick which Proton Aurora runs Windows games with.
#[cfg(not(windows))]
fn proton_version(ui: &mut Ui, t: &Theme) {
    let builds = client::proton_builds();
    if builds.is_empty() {
        return;
    }
    ui.add_space(12.0);
    w::card(t).show(ui, |ui| {
        ui.set_width(ui.available_width());
        ui.label(w::text_semibold(t!("Proton version"), 15.0, t.text));
        ui.add(egui::Label::new(w::text(t!("Which Proton runs your Windows games (including Steam-DRM games). Automatic follows Steam's own choice per game."), 12.5, t.text_dim)).wrap());
        ui.add_space(6.0);
        let current = client::proton_override();
        ui.horizontal_wrapped(|ui| {
            if w::chip(ui, t, t!("Automatic"), current.is_none()).clicked() {
                client::set_proton_override(None);
            }
            for (name, display) in &builds {
                if w::chip(ui, t, display, current.as_deref() == Some(name.as_str())).clicked() {
                    client::set_proton_override(Some(name.clone()));
                }
            }
        });
    });
}

fn downloads(ui: &mut Ui, t: &Theme, ctx: &mut Ctx, flags: &SharedFlags) {
    w::page_title(ui, t, t!("Downloads"), None);
    let mut f = *flags.lock().unwrap();
    switch(ui, t, t!("Automatic updates"), t!("Keep installed games up to date. Turn off to update only when you choose (or when you launch a game)."), &mut f.auto_updates);
    *flags.lock().unwrap() = f;
    // Region, bandwidth limit, schedule… (kept by the Downloads component).
    ctx.settings_for("downloads", ui);
    steam_download_settings(ui, t);
}

/// Steam's own download and shader switches (kept in Steam's config, so the
/// official client and the windowless host follow them too), plus
/// compiling shaders before a game starts.
fn steam_download_settings(ui: &mut Ui, t: &Theme) {
    use client::SteamSetting as S;
    ui.add_space(18.0);
    ui.label(w::text_semibold(t!("While playing"), 15.0, t.text));
    ui.add_space(6.0);
    let locked = client::steam_settings_locked();
    let steam_switch = |ui: &mut Ui, s: S, title: &str, body: &str| {
        let Some(was) = client::steam_setting(s) else { return };
        let mut on = was;
        ui.add_enabled_ui(locked.is_none(), |ui| switch(ui, t, title, body, &mut on));
        if on != was {
            if let Err(e) = client::set_steam_setting(s, on) {
                tracing::warn!("Steam setting {s:?}: {e}");
            }
        }
    };
    // Aurora's choice, put into Steam's settings before every game.
    let mut dl = client::gameplay_downloads();
    switch(
        ui,
        t,
        t!("Allow downloads during gameplay"),
        t!("Let Steam update other games and download shaders while you play. Off uses less memory while a game runs. Aurora sets this in Steam before every game."),
        &mut dl,
    );
    if dl != client::gameplay_downloads() {
        client::set_gameplay_downloads(dl);
        client::apply_steam_prefs();
    }
    ui.add_space(12.0);
    ui.label(w::text_semibold(t!("Shader pre-caching"), 15.0, t.text));
    ui.add_space(6.0);
    steam_switch(
        ui,
        S::ShaderPreCaching,
        t!("Enable shader pre-caching"),
        t!("Download shaders already compiled for your GPU and driver, so games stutter less the first time you play."),
    );
    // Linux: Aurora compiles before launch instead, and turns Steam's
    // background processing off before every game while it does.
    #[cfg(not(windows))]
    {
        let mut on = client::shaders::precompile();
        switch(
            ui,
            t,
            t!("Compile shaders before a game starts"),
            t!("Like Steam’s “Processing Vulkan shaders”: shaders not compiled yet are compiled when you press Play (you can skip it). While this is on, Steam's own background processing of Vulkan shaders is turned off before every game, so it doesn't do the same work while you play."),
            &mut on,
        );
        if on != client::shaders::precompile() {
            client::shaders::set_precompile(on);
            client::apply_steam_prefs();
        }
        if !on {
            steam_switch(ui, S::ShaderBackgroundProcessing, t!("Allow background processing of Vulkan shaders"), t!("Steam compiles downloaded shaders while it's open and idle."));
        }
    }
    #[cfg(windows)]
    steam_switch(ui, S::ShaderBackgroundProcessing, t!("Allow background processing of Vulkan shaders"), t!("Steam compiles downloaded shaders while it's open and idle."));
    if let Some(why) = locked {
        ui.add(egui::Label::new(w::text(i18n::tr_dyn(why), 12.5, t.text_dim)).wrap());
    }
}

fn cloud(ui: &mut Ui, t: &Theme, flags: &SharedFlags) {
    w::page_title(ui, t, t!("Cloud"), Some(t!("Steam Cloud keeps your game data the same on every PC.")));
    let mut f = *flags.lock().unwrap();
    switch(ui, t, t!("Enable Steam Cloud for games that support it"), t!("Each game can also be turned off in its Properties."), &mut f.cloud);
    *flags.lock().unwrap() = f;
}

fn accessibility(ui: &mut Ui, t: &Theme, ctx: &mut Ctx, flags: &SharedFlags) {
    w::page_title(ui, t, t!("Accessibility"), None);
    let row = |ui: &mut Ui, title: &str, body: &str, control: &mut dyn FnMut(&mut Ui)| {
        egui::Frame::new().fill(t.surface).corner_radius(t.corner_small()).inner_margin(Margin::symmetric(14, 12)).show(ui, |ui| {
            ui.set_width(ui.available_width());
            ui.horizontal(|ui| {
                ui.vertical(|ui| {
                    ui.label(w::text_medium(title, 14.0, t.text));
                    ui.label(w::text(body, 12.5, t.text_dim));
                });
                ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| control(ui));
            });
        });
    };
    row(ui, t!("Interface size"), t!("Makes text and everything else bigger or smaller."), &mut |ui| zoom_slider(ui, ctx));
    ui.add_space(8.0);
    let mut f = *flags.lock().unwrap();
    row(ui, t!("Scroll speed"), t!("How far the mouse wheel and touchpad scroll."), &mut |ui| {
        ui.add(egui::Slider::new(&mut f.scroll_speed, 0.5..=5.0).step_by(0.25).custom_formatter(|v, _| format!("{v:.2}×")));
    });
    ui.add_space(8.0);
    row(ui, t!("Smooth scrolling"), t!("Glide each mouse wheel step over a few frames. Off: the page jumps the whole step at once."), &mut |ui| {
        ui.add(toggle(&mut f.smooth_scroll));
    });
    ui.add_space(8.0);
    row(ui, t!("Limit to 60 fps"), t!("Draw the window at most 60 times a second, even on faster monitors. Uses less CPU and GPU while scrolling or animating."), &mut |ui| {
        ui.add(toggle(&mut f.cap_fps));
    });
    let mut stored = flags.lock().unwrap();
    stored.scroll_speed = f.scroll_speed;
    stored.smooth_scroll = f.smooth_scroll;
    stored.cap_fps = f.cap_fps;
}

/// The interface size slider. Zooming rescales the slider itself, so a
/// live zoom while dragging moves the handle away from the pointer and the
/// value runs off to an end. While dragging it shows a draft value and
/// applies it on release; a click or the arrow keys apply at once.
fn zoom_slider(ui: &mut Ui, ctx: &mut Ctx) {
    let id = egui::Id::unique("zoom-slider-draft");
    let mut zoom: f32 = ui.data(|d| d.get_temp(id)).unwrap_or(ctx.zoom);
    let r = ui.add(egui::Slider::new(&mut zoom, 0.8..=1.6).step_by(0.05).custom_formatter(|v, _| format!("{:.0}%", v * 100.0)));
    if r.dragged() {
        ui.data_mut(|d| d.insert_temp(id, zoom));
    } else {
        ui.data_mut(|d| d.remove::<f32>(id));
        if r.drag_stopped() || (r.changed() && (zoom - ctx.zoom).abs() > f32::EPSILON) {
            ctx.act(Action::SetZoom(zoom));
        }
    }
}

fn appearance(
    ui: &mut Ui,
    t: &Theme,
    ctx: &mut Ctx,
    themes_state: &crate::app::SharedThemeManager,
    confirm_uninstall: &mut Option<String>,
    option_hex_bufs: &mut std::collections::HashMap<String, String>,
) {
    use themes::{OptionKind, OptionValue};

    w::page_title(
        ui,
        t,
        t!("Appearance"),
        Some("Install and manage declarative theme packages (.zip / .aurora-theme)."),
    );

    let (themes_dir, mut hot_reload, last_error, active_id, script_error, missing_bundled) = {
        let st = themes_state.lock().unwrap();
        let script_err = st.manager.resolved.as_ref().and_then(|r| {
            r.script_error
                .clone()
                .or_else(|| r.script.as_ref().and_then(|s| s.error().map(str::to_string)))
        });
        (
            st.manager.themes_dir.clone(),
            st.hot_reload,
            st.last_error.clone(),
            st.manager.active_id.clone(),
            script_err,
            st.manager.missing_bundled_themes(),
        )
    };

    // Top action bar
    w::card(t).show(ui, |ui| {
        ui.set_width(ui.available_width());
        ui.horizontal_wrapped(|ui| {
            ui.spacing_mut().item_spacing = vec2(8.0, 8.0);
            if w::button(ui, t, Some(icons::PACKAGE), "Install Theme…", w::Kind::Primary, w::Size::Md).clicked() {
                if let Some(zip_path) = rfd::FileDialog::new()
                    .add_filter("Aurora Theme", &["zip", "aurora-theme"])
                    .pick_file()
                {
                    match themes::install_zip(&zip_path, &themes_dir) {
                        Ok(meta) => {
                            {
                                let mut st = themes_state.lock().unwrap();
                                st.manager.note_theme_installed(&meta.id);
                                st.manager.scan();
                            }
                            ctx.act(Action::ActivateTheme(Some(meta.id.clone())));
                            ctx.toast(format!("Installed and activated \"{}\"", meta.name));
                        }
                        Err(e) => ctx.toast(format!("Install failed: {e}")),
                    }
                }
            }
            if w::button(ui, t, Some(icons::FOLDER), "Open Themes Folder", w::Kind::Secondary, w::Size::Md).clicked() {
                let _ = std::fs::create_dir_all(&themes_dir);
                local::open_folder(&themes_dir);
            }
            if w::button(ui, t, Some(icons::REFRESH), "Reload", w::Kind::Ghost, w::Size::Md).clicked() {
                ctx.act(Action::ReloadThemes);
                ctx.toast("Themes reloaded");
            }
            if !missing_bundled.is_empty()
                && w::button(ui, t, Some(icons::DOWNLOAD), "Restore Built-in Themes", w::Kind::Ghost, w::Size::Md).clicked()
            {
                let res = {
                    let mut st = themes_state.lock().unwrap();
                    st.manager.restore_bundled_themes()
                };
                match res {
                    Ok(restored) => {
                        ctx.act(Action::ReloadThemes);
                        if !restored.is_empty() {
                            ctx.toast(format!("Restored {} built-in theme(s)", restored.len()));
                        }
                    }
                    Err(e) => ctx.toast(format!("Restore failed: {e}")),
                }
            }
            ui.add_space(6.0);
            if ui.add(toggle(&mut hot_reload)).changed() {
                let mut st = themes_state.lock().unwrap();
                st.hot_reload = hot_reload;
            }
            ui.label(w::text_medium("Hot-Reload", 13.0, t.text_dim));
        });
    });

    if let Some(err) = last_error {
        ui.add_space(10.0);
        theme_warning_banner(ui, t, "Theme error", &err);
    }
    if let Some(err) = script_error {
        ui.add_space(10.0);
        theme_warning_banner(ui, t, "theme.rhai script error", &err);
    }

    ui.add_space(16.0);

    // Default (Aurora) card
    let default_active = active_id.is_none();
    egui::Frame::new()
        .fill(t.surface)
        .corner_radius(t.corner())
        .inner_margin(Margin::same(16))
        .stroke(if default_active {
            egui::Stroke::new(2.0, t.accent)
        } else {
            egui::Stroke::new(1.0, t.border)
        })
        .show(ui, |ui| {
            ui.set_width(ui.available_width());
            ui.horizontal(|ui| {
                ui.vertical(|ui| {
                    ui.horizontal(|ui| {
                        ui.label(w::text_semibold("Default (Aurora)", 15.0, t.text));
                        if default_active {
                            ui.label(w::text_semibold(format!("{} Active", icons::CHECK), 12.0, t.play));
                        }
                    });
                    ui.label(w::text("Aurora's built-in default interface and layout.", 12.5, t.text_dim));
                });
                ui.with_layout(Layout::right_to_left(Align::Center), |ui| {
                    if !default_active && w::button(ui, t, Some(icons::CHECK), "Activate", w::Kind::Secondary, w::Size::Sm).clicked() {
                        ctx.act(Action::ActivateTheme(None));
                    }
                });
            });
        });

    ui.add_space(16.0);

    // Installed Themes list
    let (installed, broken) = {
        let st = themes_state.lock().unwrap();
        (st.manager.installed.clone(), st.manager.broken.clone())
    };

    ui.label(w::text_semibold(format!("Installed Themes ({})", installed.len()), 15.0, t.text));
    ui.add_space(8.0);

    if installed.is_empty() {
        w::empty(
            ui,
            t,
            icons::PALETTE,
            "No themes installed yet",
            "Click \"Install Theme…\" above to install a .zip or .aurora-theme package.",
        );
    } else {
        let mut uninstall_id: Option<String> = None;
        for item in &installed {
            let id = &item.manifest.meta.id;
            let is_active = active_id.as_deref() == Some(id.as_str());
            egui::Frame::new()
                .fill(t.surface)
                .corner_radius(t.corner())
                .inner_margin(Margin::same(16))
                .stroke(if is_active {
                    egui::Stroke::new(2.0, t.accent)
                } else {
                    egui::Stroke::new(1.0, t.border)
                })
                .show(ui, |ui| {
                    ui.set_width(ui.available_width());
                    ui.horizontal(|ui| {
                        ui.vertical(|ui| {
                            ui.horizontal(|ui| {
                                ui.label(w::text_semibold(&item.manifest.meta.name, 15.0, t.text));
                                if !item.manifest.meta.version.is_empty() {
                                    ui.label(w::text(format!("v{}", item.manifest.meta.version), 11.5, t.text_faint));
                                }
                                if themes::is_bundled_theme(id) {
                                    ui.label(w::text("built-in", 11.5, t.text_faint));
                                }
                                if item.has_layout {
                                    ui.label(w::text("layout", 11.5, t.text_faint));
                                }
                                if item.has_script {
                                    ui.label(w::text(format!("{} rhai", icons::SPARKLE), 11.5, t.accent));
                                }
                                if is_active {
                                    ui.label(w::text_semibold(format!("{} Active", icons::CHECK), 12.0, t.play));
                                }
                            });
                            if !item.manifest.meta.author.is_empty() {
                                ui.label(w::text(format!("by {}", item.manifest.meta.author), 12.0, t.text_dim));
                            }
                            if !item.manifest.meta.description.is_empty() {
                                ui.add_space(3.0);
                                ui.add(egui::Label::new(w::text(&item.manifest.meta.description, 12.5, t.text_dim)).wrap());
                            }
                        });
                        ui.with_layout(Layout::right_to_left(Align::Center), |ui| {
                            ui.spacing_mut().item_spacing.x = 6.0;
                            if confirm_uninstall.as_deref() == Some(id.as_str()) {
                                if w::button(ui, t, Some(icons::TRASH), "Confirm", w::Kind::Danger, w::Size::Sm).clicked() {
                                    uninstall_id = Some(id.clone());
                                    *confirm_uninstall = None;
                                }
                                if w::button(ui, t, None, "Cancel", w::Kind::Ghost, w::Size::Sm).clicked() {
                                    *confirm_uninstall = None;
                                }
                            } else {
                                if w::button(ui, t, Some(icons::TRASH), "Uninstall", w::Kind::Ghost, w::Size::Sm).clicked() {
                                    *confirm_uninstall = Some(id.clone());
                                }
                                if is_active {
                                    if w::button(ui, t, None, "Deactivate", w::Kind::Secondary, w::Size::Sm).clicked() {
                                        ctx.act(Action::ActivateTheme(None));
                                    }
                                } else if w::button(ui, t, Some(icons::CHECK), "Activate", w::Kind::Primary, w::Size::Sm).clicked() {
                                    ctx.act(Action::ActivateTheme(Some(id.clone())));
                                }
                            }
                        });
                    });
                });
            ui.add_space(8.0);
        }

        if let Some(id) = uninstall_id {
            if active_id.as_deref() == Some(id.as_str()) {
                ctx.act(Action::ActivateTheme(None));
            }
            let res = {
                let mut st = themes_state.lock().unwrap();
                st.manager.uninstall(&id)
            };
            match res {
                Ok(()) => {
                    ctx.act(Action::ReloadThemes);
                    ctx.toast(format!("Uninstalled theme \"{id}\""));
                }
                Err(e) => ctx.toast(format!("Uninstall failed: {e}")),
            }
        }
    }

    // Broken themes warning card
    if !broken.is_empty() {
        ui.add_space(12.0);
        egui::Frame::new()
            .fill(t.surface)
            .corner_radius(t.corner_small())
            .inner_margin(Margin::same(14))
            .stroke(egui::Stroke::new(1.0, t.away))
            .show(ui, |ui| {
                ui.set_width(ui.available_width());
                ui.label(w::text_semibold(format!("{}  Broken Themes ({})", icons::WARNING, broken.len()), 14.0, t.away));
                ui.add_space(6.0);
                for (path, err) in &broken {
                    ui.label(w::text_medium(path.display().to_string(), 12.5, t.text));
                    ui.add(egui::Label::new(w::text(err, 12.0, t.text_dim)).wrap());
                    ui.add_space(4.0);
                }
            });
    }

    // Active theme options panel
    let active_options = {
        let st = themes_state.lock().unwrap();
        st.manager.active_manifest().and_then(|m| {
            if m.options.is_empty() {
                None
            } else {
                let id = m.meta.id.clone();
                let name = m.meta.name.clone();
                let values = st.manager.load_option_values(&id);
                let opts: Vec<(String, themes::ThemeOption)> = m.options.iter().map(|(k, v)| (k.clone(), v.clone())).collect();
                Some((id, name, values, opts))
            }
        })
    };

    if let Some((theme_id, theme_name, values, options)) = active_options {
        ui.add_space(16.0);
        ui.horizontal(|ui| {
            ui.label(w::text_semibold(format!("{theme_name} Options"), 15.0, t.text));
            ui.with_layout(Layout::right_to_left(Align::Center), |ui| {
                if w::button(ui, t, Some(icons::REFRESH), "Reset Options", w::Kind::Ghost, w::Size::Sm).clicked() {
                    let res = {
                        let mut st = themes_state.lock().unwrap();
                        let r = st.manager.reset_options(&theme_id);
                        if r.is_ok() {
                            st.revision = st.revision.wrapping_add(1);
                            st.dirty = true;
                        }
                        r
                    };
                    option_hex_bufs.clear();
                    match res {
                        Ok(()) => ctx.toast("Reset theme options to defaults"),
                        Err(e) => ctx.toast(format!("Reset failed: {e}")),
                    }
                }
            });
        });
        ui.add_space(6.0);
        w::card(t).show(ui, |ui| {
            ui.set_width(ui.available_width());
            for (idx, (key, opt)) in options.iter().enumerate() {
                if idx > 0 {
                    ui.add_space(10.0);
                }
                let current = values.get(key).unwrap_or(&opt.default);
                ui.horizontal(|ui| {
                    ui.vertical(|ui| {
                        ui.label(w::text_medium(&opt.label, 14.0, t.text));
                        if !opt.description.is_empty() {
                            ui.add(egui::Label::new(w::text(&opt.description, 12.0, t.text_dim)).wrap());
                        }
                    });
                    ui.with_layout(Layout::right_to_left(Align::Center), |ui| match opt.kind {
                        OptionKind::Bool => {
                            let mut b = current.as_bool().unwrap_or(false);
                            if ui.add(toggle(&mut b)).changed() {
                                ctx.act(Action::SetThemeOption {
                                    theme_id: theme_id.clone(),
                                    key: key.clone(),
                                    value: b.to_string(),
                                });
                            }
                        }
                        OptionKind::Select => {
                            let selected = current.as_str().unwrap_or("");
                            ui.horizontal_wrapped(|ui| {
                                for choice in opt.choices.iter().rev() {
                                    if w::chip(ui, t, &choice.label, choice.value == selected).clicked() {
                                        ctx.act(Action::SetThemeOption {
                                            theme_id: theme_id.clone(),
                                            key: key.clone(),
                                            value: choice.value.clone(),
                                        });
                                    }
                                }
                            });
                        }
                        OptionKind::Color => {
                            let mut col = current
                                .as_str()
                                .and_then(|s| themes::parse_hex_color(s).ok())
                                .unwrap_or(t.accent);
                            let buf = option_hex_bufs
                                .entry(key.clone())
                                .or_insert_with(|| themes::color_to_hex(col));
                            let resp = ui.add(
                                egui::TextEdit::singleline(buf)
                                    .font(egui::FontId::monospace(12.5))
                                    .desired_width(86.0),
                            );
                            if resp.changed() {
                                if let Ok(parsed) = themes::parse_hex_color(buf) {
                                    ctx.act(Action::SetThemeOption {
                                        theme_id: theme_id.clone(),
                                        key: key.clone(),
                                        value: themes::color_to_hex(parsed),
                                    });
                                }
                            } else if !resp.has_focus() {
                                let canonical = themes::color_to_hex(col);
                                if *buf != canonical {
                                    *buf = canonical;
                                }
                            }
                            if ui.color_edit_button_srgba(&mut col).changed() {
                                let hex = themes::color_to_hex(col);
                                option_hex_bufs.insert(key.clone(), hex.clone());
                                ctx.act(Action::SetThemeOption {
                                    theme_id: theme_id.clone(),
                                    key: key.clone(),
                                    value: hex,
                                });
                            }
                        }
                        OptionKind::Slider => {
                            let min = opt.min.filter(|v| v.is_finite()).unwrap_or(0.0);
                            let max = opt.max.filter(|v| v.is_finite()).unwrap_or(100.0).max(min);
                            let raw_val = current.as_f32().filter(|v| v.is_finite()).unwrap_or(min);
                            let mut val = raw_val.clamp(min, max);
                            let mut slider = egui::Slider::new(&mut val, min..=max);
                            if let Some(step) = opt.step.filter(|s| s.is_finite() && *s > 0.0) {
                                slider = slider.step_by(step as f64);
                            }
                            if ui.add(slider).changed() {
                                ctx.act(Action::SetThemeOption {
                                    theme_id: theme_id.clone(),
                                    key: key.clone(),
                                    value: OptionValue::Number(val as f64).to_string_value(),
                                });
                            }
                        }
                    });
                });
            }
        });
    }
}

fn theme_warning_banner(ui: &mut Ui, t: &Theme, title: &str, body: &str) {
    egui::Frame::new()
        .fill(t.surface)
        .corner_radius(t.corner_small())
        .inner_margin(Margin::same(12))
        .stroke(egui::Stroke::new(1.2, t.danger))
        .show(ui, |ui| {
            ui.set_width(ui.available_width());
            ui.horizontal_top(|ui| {
                ui.label(w::text(icons::WARNING, 16.0, t.danger));
                ui.vertical(|ui| {
                    ui.label(w::text_semibold(title, 13.5, t.danger));
                    ui.add(egui::Label::new(w::text(body, 12.5, t.text_dim)).wrap());
                });
            });
        });
}

fn interface(ui: &mut Ui, t: &Theme, ctx: &mut Ctx, flags: &SharedFlags, handler: &mut Option<bool>) {
    w::page_title(ui, t, t!("Interface"), None);
    let mut f = *flags.lock().unwrap();
    w::card(t).show(ui, |ui| {
        ui.set_width(ui.available_width());
        let row = |ui: &mut Ui, title: &str, body: &str, value: &mut bool| {
            ui.horizontal(|ui| {
                ui.vertical(|ui| {
                    ui.label(w::text_semibold(title, 14.5, t.text));
                    ui.label(w::text(body, 12.5, t.text_dim));
                });
                ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                    ui.add(toggle(value));
                });
            });
            ui.add_space(8.0);
        };
        row(ui, t!("Close to tray"), t!("Keep Aurora running in the system tray when you close the window."), &mut f.close_to_tray);
        row(ui, t!("Ask which account to use"), t!("Show the account picker at startup instead of signing in automatically."), &mut f.ask_account);
    });
    *flags.lock().unwrap() = f;
    ui.add_space(12.0);
    let mut on = *handler.get_or_insert_with(crate::links::is_default_handler);
    w::card(t).show(ui, |ui| {
        ui.set_width(ui.available_width());
        ui.horizontal(|ui| {
            ui.vertical(|ui| {
                ui.label(w::text_semibold(t!("Open steam:// links with Aurora"), 14.5, t.text));
                ui.add(egui::Label::new(w::text(t!("Store buttons, desktop shortcuts and links like steam://rungameid/570 will open here. Turning it off gives them back to the official Steam client."), 12.5, t.text_dim)).wrap());
            });
            ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                if ui.add(toggle(&mut on)).changed() {
                    let r = if on { crate::links::register() } else { crate::links::unregister() };
                    match r {
                        Ok(()) => ctx.toast(if on { t!("steam:// links now open in Aurora") } else { t!("steam:// links handed back") }),
                        Err(e) => ctx.toast(e),
                    }
                    *handler = None;
                }
            });
        });
    });
    ui.add_space(12.0);
    w::card(t).show(ui, |ui| {
        ui.set_width(ui.available_width());
        ui.label(w::text_semibold(t!("Language"), 15.0, t.text));
        ui.add_space(6.0);
        ui.horizontal(|ui| {
            for lang in i18n::Lang::ALL {
                if w::chip(ui, t, lang.native_name(), i18n::language() == lang).clicked() {
                    ctx.act(Action::SetLanguage(lang.code().into()));
                }
            }
        });
    });
}

fn overlay(ui: &mut Ui, t: &Theme, ctx: &mut Ctx, flags: &SharedFlags) {
    w::page_title(ui, t, t!("In-game overlay"), None);
    let mut f = flags.lock().unwrap();
    w::card(t).show(ui, |ui| {
        ui.set_width(ui.available_width());
        ui.label(w::text_semibold(t!("Shift+Tab"), 15.0, t.text));
        ui.add(egui::Label::new(w::text(
            t!("Press Shift+Tab (or the controller Guide button) while playing to open the overlay: friends, chat, achievements, guides, discussions, screenshots and per-game notes. Press F12 to take a screenshot."),
            13.5,
            t.text_dim,
        )).wrap());
        ui.add_space(10.0);
        ui.checkbox(&mut f.overlay_enabled, t!("Enable the in-game overlay"));
        ui.add_space(10.0);
        if w::button(ui, t, Some(icons::EYE), t!("Preview overlay"), w::Kind::Secondary, w::Size::Md).clicked() {
            ctx.act(Action::ToggleOverlay);
        }
    });
    ui.add_space(14.0);
    w::card(t).show(ui, |ui| {
        ui.set_width(ui.available_width());
        ui.label(w::text_semibold(t!("In-game FPS counter"), 15.0, t.text));
        ui.add_space(6.0);
        ui.horizontal_wrapped(|ui| {
            for corner in FpsCorner::ALL {
                if w::chip(ui, t, corner.label(), f.fps_counter == corner).clicked() {
                    f.fps_counter = corner;
                }
            }
        });
        if f.fps_counter != FpsCorner::Off {
            ui.add_space(8.0);
            ui.checkbox(&mut f.fps_high_contrast, t!("High contrast color"));
        }
    });
}

/// Pick a folder and remember it as a library folder (games installed there show up).
fn add_library_folder(ctx: &mut Ctx) {
    if let Some(p) = rfd::FileDialog::new().pick_folder() {
        match local::add_library(&p) {
            Ok(()) => ctx.act(extensions::Action::ReloadLocal),
            Err(e) => ctx.toast(format!("{}: {e}", p.display())),
        }
    }
}

fn folders(ui: &mut Ui, t: &Theme, ctx: &mut Ctx, restore: &mut Option<super::storage_ui::StorageDialog>) {
    // Saved pictures (store and library art), like Steam's web browser cache.
    w::card(t).show(ui, |ui| {
        ui.set_width(ui.available_width());
        ui.horizontal(|ui| {
            ui.label(w::text(icons::IMAGE, 22.0, t.accent));
            ui.vertical(|ui| {
                ui.label(w::text_semibold(t!("Picture cache"), 15.0, t.text));
                let size = ui.ctx().memory(|m| m.data.get_temp::<u64>(egui::Id::unique("image-cache-size")));
                let size = size.unwrap_or_else(|| {
                    let s = crate::image_cache::size();
                    ui.ctx().memory_mut(|m| m.data.insert_temp(egui::Id::unique("image-cache-size"), s));
                    s
                });
                ui.label(w::text(t!("{size} of store and library pictures, kept so pages open instantly and work offline.", size = ui::bytes(size)), 12.5, t.text_dim));
            });
            ui.with_layout(Layout::right_to_left(Align::Center), |ui| {
                if w::button(ui, t, Some(icons::TRASH), t!("Clear"), w::Kind::Secondary, w::Size::Sm).clicked() {
                    crate::image_cache::clear(ui.ctx());
                    ui.ctx().memory_mut(|m| m.data.insert_temp(egui::Id::unique("image-cache-size"), 0u64));
                }
            });
        });
    });
    ui.add_space(14.0);
    w::page_title(ui, t, t!("Library folders"), Some(t!("Folders where games are installed: your Steam setup's, and any you add here.")));
    ui.horizontal_wrapped(|ui| {
        ui.spacing_mut().item_spacing = vec2(8.0, 8.0);
        if w::button(ui, t, Some(icons::FOLDER), t!("Add library folder…"), w::Kind::Secondary, w::Size::Sm).clicked() {
            add_library_folder(ctx);
        }
        if w::button(ui, t, Some(icons::RESTORE), t!("Restore game backup…"), w::Kind::Secondary, w::Size::Sm).clicked() {
            *restore = Some(super::storage_ui::StorageDialog::Restore(super::storage_ui::RestoreDialog::new(ctx)));
        }
    });
    if let Some(r) = &ctx.state.restoring {
        ui.add_space(8.0);
        super::storage_ui::restore_progress_card(ui, t, ctx, r);
        ui.ctx().request_repaint_after(std::time::Duration::from_millis(100));
    }
    if let Some(mut dlg) = restore.take()
        && dlg.show(ui, t, ctx, "settings")
    {
        *restore = Some(dlg);
    }
    ui.add_space(8.0);
    if ctx.local.libraries.is_empty() {
        w::empty(ui, t, icons::DRIVES, t!("No library folders found"), t!("Add a folder to install games into."));
    }
    for lib in &ctx.local.libraries {
        let games: Vec<_> = ctx.local.games.iter().filter(|g| &g.library == lib).collect();
        let size: u64 = games.iter().map(|g| g.size_on_disk).sum();
        w::card(t).show(ui, |ui| {
            ui.set_width(ui.available_width());
            ui.horizontal(|ui| {
                ui.label(w::text(icons::DRIVES, 24.0, t.accent));
                ui.vertical(|ui| {
                    ui.label(w::text_semibold(lib.display().to_string(), 14.5, t.text));
                    ui.label(w::text(t!("{n} games · {size}", n = games.len(), size = ui::bytes(size)), 13.0, t.text_dim));
                });
                ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                    if w::button(ui, t, Some(icons::FOLDER), t!("Open"), w::Kind::Secondary, w::Size::Sm).clicked() {
                        local::open_folder(lib);
                    }
                });
            });
        });
        ui.add_space(8.0);
    }
}

fn about(ui: &mut Ui, t: &Theme) {
    w::page_title(ui, t, t!("About"), None);
    crate::updater::settings_ui(ui, t);
    ui.add_space(12.0);
    w::card(t).show(ui, |ui| {
        ui.set_width(ui.available_width());
        ui.horizontal(|ui| {
            w::logo(ui, t, 40.0);
            ui.vertical(|ui| {
                ui.label(w::text_bold(format!("Aurora {}", env!("CARGO_PKG_VERSION")), 18.0, t.text));
                ui.label(w::text(t!("A native Steam client written in Rust."), 13.5, t.text_dim));
            });
        });
        ui.add_space(10.0);
        ui.label(w::text(t!("Not affiliated with Valve. Steam is a trademark of Valve Corporation."), 12.5, t.text_faint));
        ui.label(w::text(t!("Fonts: Geist and Inter (OFL). Icons: Phosphor (MIT). Protocol definitions: SteamTracking/Protobufs."), 12.5, t.text_faint));
        let dir = client::store::config_dir();
        ui.add_space(10.0);
        ui.horizontal(|ui| {
            ui.label(w::text(t!("Data folder: {path}", path = dir.display()), 12.5, t.text_dim));
            if w::button(ui, t, Some(icons::FOLDER), t!("Open"), w::Kind::Ghost, w::Size::Sm).clicked() {
                let _ = std::fs::create_dir_all(&dir);
                local::open_folder(&dir);
            }
        });
    });
}

/// iOS-style switch.
pub fn toggle(on: &mut bool) -> impl egui::Widget + '_ {
    move |ui: &mut Ui| {
        let size = vec2(40.0, 22.0);
        let (rect, mut resp) = ui.allocate_exact_size(size, Sense::click());
        if resp.clicked() {
            *on = !*on;
            resp.mark_changed();
        }
        let k = ui.ctx().animate_bool_with_time(resp.id, *on, 0.15);
        let v = ui.visuals();
        let accent = v.selection.stroke.color;
        let off = v.widgets.inactive.bg_fill;
        let fill = w::mix(off, accent, k);
        let fill = if ui.is_enabled() { fill } else { fill.gamma_multiply(0.5) };
        ui.painter().rect_filled(rect, egui::CornerRadius::same(11), fill);
        let x = egui::lerp(rect.left() + 11.0..=rect.right() - 11.0, k);
        ui.painter().circle_filled(egui::pos2(x, rect.center().y), 8.0, egui::Color32::WHITE);
        resp.on_hover_cursor(egui::CursorIcon::PointingHand)
    }
}
