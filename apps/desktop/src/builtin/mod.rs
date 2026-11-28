//! Built-in extensions. Every page in the app lives here and uses the same
//! public API as third-party extensions. To add your own, write a type that
//! implements `extensions::Extension` and register it below. For scripts that
//! users install themselves, see the `plugins` crate.

pub mod community;
pub mod community_home;
mod chat_image_picker;
pub mod chat_render;
pub mod group_chat;
pub(crate) mod downloads;
pub mod hub;
pub mod inventory;
pub mod library;
mod plugins;
mod screenshots;
mod servers;
mod support;
pub(crate) mod profile;
mod reviews;
pub mod settings;
mod settings_account;
mod settings_controller;
mod storage_ui;
pub mod store;
mod workshop;

use extensions::Registry;

pub fn registry(
    rt: tokio::runtime::Handle,
    ctx: egui::Context,
    flags: settings::SharedFlags,
    themes: crate::app::SharedThemeManager,
    library_layout: crate::app::SharedLibraryLayout,
) -> Registry {
    let mut r = Registry::default();
    r.register(store::Store::new(rt.clone(), ctx.clone()));
    r.register(library::Library::new(rt.clone(), ctx.clone(), flags.clone(), library_layout));
    r.register(community_home::CommunityHome::new(rt.clone(), ctx.clone()));
    r.register(community::Community::default());
    r.register(profile::Profile::new(flags.clone()));
    r.register(downloads::Downloads::new(flags.clone()));
    r.register(workshop::Workshop::default());
    r.register(hub::Hub::default());
    r.register(inventory::Inventory::default());
    r.register(screenshots::Screenshots::new(rt.clone(), ctx.clone()));
    r.register(servers::Servers::default());
    r.register(support::Support::default());
    r.register(plugins::Plugins::new());
    r.register(settings::Settings::new(flags, themes));
    r
}

/// One row of the library: owned and/or installed.
pub struct Entry<'a> {
    pub app_id: model::AppId,
    pub name: &'a str,
    pub installed: Option<&'a model::InstalledGame>,
    pub owned: Option<&'a model::OwnedApp>,
}

/// Merge owned apps (from Steam) with games found on disk.
pub fn entries<'a>(state: &'a client::State, local: &'a local::LocalState) -> Vec<Entry<'a>> {
    let mut installed: std::collections::HashMap<_, _> = local.games.iter().map(|g| (g.app_id, g)).collect();
    let mut out = Vec::with_capacity(state.apps.as_ref().map_or(0, |apps| apps.len()) + installed.len());
    if let Some(apps) = &state.apps {
        for a in apps.values().filter(|a| !a.is_dlc()) {
            out.push(Entry { app_id: a.app_id, name: &a.name, installed: installed.remove(&a.app_id), owned: Some(a) });
        }
    }
    for g in &local.games {
        if installed.remove(&g.app_id).is_some() {
            out.push(Entry { app_id: g.app_id, name: &g.name, installed: Some(g), owned: None });
        }
    }
    out
}

/// The games whose page data (store details, news…) is kept in memory: the
/// most recently shown ones. Older games' data is dropped; it comes back
/// from the disk cache (`store::disk`) when the game is shown again.
pub(crate) struct RecentApps {
    order: std::collections::VecDeque<model::AppId>,
    cap: usize,
}

impl Default for RecentApps {
    fn default() -> Self {
        Self::new(16)
    }
}

impl RecentApps {
    /// `cap` must be above the number of games one screen shows at once.
    pub fn new(cap: usize) -> Self {
        Self { order: Default::default(), cap }
    }

    /// Mark `id` as just shown; returns the game to forget, if any.
    pub fn touch(&mut self, id: model::AppId) -> Option<model::AppId> {
        if self.order.back() == Some(&id) {
            return None;
        }
        self.order.retain(|x| *x != id);
        self.order.push_back(id);
        if self.order.len() > self.cap { self.order.pop_front() } else { None }
    }

    pub fn clear(&mut self) {
        self.order.clear();
    }
}
