//! Store: rendered natively from Steam's store JSON (no web view).
//! Payment is the only step that leaves the app: Steam only accepts it on its website.
//! Search suggestions, redesigned Store bar, Discovery Queue walkthrough,
//! microtrailers on hover, and wishlist reordering adapted from upstream work by `madonchik123`.

use ui::widgets::EditMenu;
use i18n::t;
use egui::text::{LayoutJob, TextFormat};
use egui::{Align, Align2, Color32, CornerRadius, Layout, Margin, Rect, Sense, Stroke, Ui, pos2, vec2};
use extensions::{Ctx, Extension, Manifest, PageSpec};
use model::AppId;
use std::collections::{HashMap, HashSet};
use std::future::Future;
use std::sync::{Arc, Mutex};
use store::browse::BrowseInfo;
use store::{Details, Item, Section, StoreClient, html::Block};
use ui::{Theme, icons, widgets as w};

#[path = "store_more.rs"]
mod more;
#[path = "store_discovery.rs"]
mod discovery;
#[path = "store_suggest.rs"]
mod suggest;
#[path = "store_micro.rs"]
mod micro;
#[path = "store_points.rs"]
mod points;

enum Load<T> {
    Loading,
    Ready(T),
    Failed(String),
}
type Shared<T> = Arc<Mutex<Load<T>>>;
type StoreFront = (Vec<Item>, Vec<Section>);

struct Players {
    asked: std::time::Instant,
    slot: Shared<Option<u32>>,
    last: Option<u32>,
}

#[derive(Clone, PartialEq)]
enum View {
    Home,
    Section(String),
    Search(String),
    /// Everything by a developer or publisher: ("developer" | "publisher", name).
    Creator(String, String),
    App(AppId),
    Browse,
    Discovery(usize),
    Points,
    Wishlist,
    Cart,
}

pub struct Store {
    /// A game's reviews (read, or write your own).
    reviews: Option<super::reviews::ReviewWindow>,
    /// The personal picks were asked for (once per sign-in).
    recs_asked: bool,
    /// Personal shelves built from the picks: (picks they came from, shelves).
    recs: Option<(model::StoreRecommendations, Shared<Arc<[Section]>>)>,
    /// A view opened inside the store, to record in Back/Forward history.
    nav_out: Option<Option<String>>,
    /// Set while the app navigates us (Back, Forward, links): don't record again.
    in_nav: bool,
    /// A free game being added to the account.
    free_pending: Option<AppId>,
    /// Text in the Browse page's tag search box.
    tag_query: String,
    rt: tokio::runtime::Handle,
    egui: egui::Context,
    api: StoreClient,
    front: Option<Shared<Arc<StoreFront>>>,
    details: HashMap<AppId, Shared<Arc<Details>>>,
    browse: HashMap<AppId, Shared<Arc<BrowseInfo>>>,
    /// Games whose details, info, players and price history are kept.
    recent: super::RecentApps,
    /// People playing each game right now, refreshed every minute.
    players: HashMap<AppId, Players>,
    tags: Option<Shared<HashMap<u32, String>>>,
    /// When the tag names were last asked for (to retry after failure/empty answer).
    tags_asked: Option<std::time::Instant>,
    /// (country, language) the caches were loaded with.
    locale: (String, String),
    /// Content descriptors per app, for the mature content filter.
    ratings: Arc<Mutex<HashMap<AppId, Vec<u32>>>>,
    ratings_pending: Vec<AppId>,
    ratings_busy: Arc<Mutex<bool>>,
    /// Descriptor ids the user chose to hide.
    hidden: std::collections::BTreeSet<u32>,
    /// Mature pages the user chose to view this session.
    allowed: HashSet<AppId>,
    /// Text in the Store subnav search bar.
    bar_query: String,
    /// Search-as-you-type game suggestions under the Store search bar.
    suggest: suggest::Suggest,
    /// Active microtrailer hovering on a store card.
    micro: micro::Micro,
    /// Points Shop catalog state.
    points: points::Points,
    /// Discovery Queue walkthrough state.
    discovery: Option<Result<Vec<AppId>, String>>,
    discovery_wait: bool,
    discovery_late: bool,
    discovery_asked: Option<std::time::Instant>,
    /// Local demo toggles so `--demo` interactions stay responsive without Steam.
    demo_followed: HashMap<AppId, bool>,
    demo_ignored: HashMap<AppId, bool>,
    demo_wish_removed: HashSet<AppId>,
    demo_wish_added: Vec<(AppId, u64)>,
    search: Option<(String, Shared<Arc<[Item]>>)>,
    view: View,
    hero: usize,
    hero_changed: f64,
    shelf_page: HashMap<String, usize>,
    shot: usize,
    /// The strip item last scrolled into view, so a new pick gets centered once.
    shot_centered: Option<usize>,
    /// Games on your account (library + installed), for "In library" marks.
    owned: HashSet<AppId>,
    /// The trailer playing in the media area: (source URL, player).
    trailer: Option<(String, video::Player)>,
    /// Frame counters for freeing trailer/microtrailer players when leaving Store.
    frames: u64,
    drawn_frame: u64,
    filters: store::query::Filters,
    browse_page: u32,
    query: Option<(store::query::Filters, u32, Shared<Arc<(Vec<store::query::QueryItem>, u64)>>)>,
    hide_owned: bool,
    wish: more::WishView,
    wish_items: Option<(Vec<AppId>, Shared<Arc<[store::query::QueryItem]>>)>,
    cart_names: Option<(Vec<u32>, Shared<Vec<(u32, String, String, AppId)>>)>,
    /// Steam's "Added to your cart!" popup, for one game.
    cart_popup: Option<more::CartPopup>,
    gift: Option<more::Gift>,
    history: HashMap<AppId, Shared<store::itad::PriceHistory>>,
    itad_key: String,
    price_log: std::collections::BTreeMap<AppId, Vec<(u64, u64)>>,
    price_log_dirty: bool,
    lists_requested: bool,
    /// Big Picture's search, kept apart from the store page's own.
    tv_search: Option<(String, Shared<Vec<Item>>)>,
}

#[derive(serde::Serialize, serde::Deserialize, Default)]
#[serde(default)]
struct Saved {
    hidden: Vec<u32>,
    itad_key: String,
    price_log: std::collections::BTreeMap<AppId, Vec<(u64, u64)>>,
}

impl Store {
    pub fn new(rt: tokio::runtime::Handle, egui: egui::Context) -> Self {
        Self {
            reviews: None,
            recs_asked: false,
            recs: None,
            rt,
            egui,
            api: StoreClient::default(),
            front: None,
            details: HashMap::new(),
            browse: HashMap::new(),
            recent: super::RecentApps::new(24),
            players: HashMap::new(),
            tags: None,
            tags_asked: None,
            locale: ("us".into(), "english".into()),
            ratings: Default::default(),
            ratings_pending: Vec::new(),
            ratings_busy: Default::default(),
            hidden: [3, 4].into_iter().collect(),
            allowed: Default::default(),
            bar_query: String::new(),
            suggest: Default::default(),
            micro: Default::default(),
            points: Default::default(),
            discovery: None,
            discovery_wait: false,
            discovery_late: false,
            discovery_asked: None,
            demo_followed: HashMap::new(),
            demo_ignored: HashMap::new(),
            demo_wish_removed: HashSet::new(),
            demo_wish_added: Vec::new(),
            filters: Default::default(),
            browse_page: 0,
            query: None,
            hide_owned: false,
            wish: Default::default(),
            wish_items: None,
            cart_names: None,
            cart_popup: None,
            gift: None,
            history: HashMap::new(),
            nav_out: None,
            in_nav: false,
            free_pending: None,
            tag_query: String::new(),
            itad_key: String::new(),
            price_log: Default::default(),
            price_log_dirty: false,
            lists_requested: false,
            tv_search: None,
            search: None,
            view: View::Home,
            hero: 0,
            hero_changed: 0.0,
            shelf_page: HashMap::new(),
            shot: 0,
            shot_centered: None,
            owned: Default::default(),
            trailer: None,
            frames: 0,
            drawn_frame: 0,
        }
    }

    fn fetch<T: Send + 'static>(&self, fut: impl Future<Output = store::Result<T>> + Send + 'static) -> Shared<T> {
        let slot = Arc::new(Mutex::new(Load::Loading));
        let (out, ctx) = (slot.clone(), self.egui.clone());
        self.rt.spawn(async move {
            let r = fut.await;
            *out.lock().unwrap() = match r {
                Ok(v) => Load::Ready(v),
                Err(e) => Load::Failed(e.to_string()),
            };
            ctx.request_repaint();
        });
        slot
    }

    /// Keep `id`'s page data; forget the game shown longest ago.
    fn touch(&mut self, id: AppId) {
        if let Some(old) = self.recent.touch(id) {
            self.details.remove(&old);
            self.browse.remove(&old);
            self.players.remove(&old);
            self.history.remove(&old);
        }
    }

    fn details(&mut self, id: AppId) -> Option<Arc<Details>> {
        self.details_result(id).and_then(Result::ok)
    }

    pub(crate) fn details_result(&mut self, id: AppId) -> Option<Result<Arc<Details>, String>> {
        self.touch(id);
        if !self.details.contains_key(&id) {
            let api = self.api.clone();
            let slot = self.fetch(async move { api.details(id).await.map(Arc::new) });
            self.details.insert(id, slot);
        }
        match &*self.details[&id].lock().unwrap() {
            Load::Ready(d) => Some(Ok(d.clone())),
            Load::Failed(error) => Some(Err(error.clone())),
            Load::Loading => None,
        }
    }

    pub(crate) fn retry_details(&mut self, id: AppId) {
        let api = self.api.clone();
        let slot = self.fetch(async move { api.refresh_details(id).await.map(Arc::new) });
        self.details.insert(id, slot);
    }

    fn browse_info(&mut self, id: AppId) -> Option<Arc<BrowseInfo>> {
        self.touch(id);
        if !self.browse.contains_key(&id) {
            let api = self.api.clone();
            let slot = self.fetch(async move { api.browse(&[id]).await.map(|mut v| Arc::new(v.pop().unwrap_or_default())) });
            self.browse.insert(id, slot);
        }
        match &*self.browse[&id].lock().unwrap() {
            Load::Ready(b) => Some(b.clone()),
            _ => None,
        }
    }

    /// People in the game right now (Steam's count). Asked again every
    /// minute while the page is open; the last number stays meanwhile.
    fn current_players(&mut self, id: AppId) -> Option<u32> {
        const EVERY: std::time::Duration = std::time::Duration::from_secs(60);
        if self.players.get(&id).is_none_or(|p| p.asked.elapsed() >= EVERY) {
            let last = self.players.get(&id).and_then(|p| p.last);
            let api = self.api.clone();
            let slot = self.fetch(async move { api.current_players(id).await });
            self.players.insert(id, Players { asked: std::time::Instant::now(), slot, last });
        }
        self.egui.request_repaint_after(EVERY);
        let p = self.players.get_mut(&id)?;
        let ready = match &*p.slot.lock().unwrap() {
            Load::Ready(n) => Some(*n),
            _ => None,
        };
        if let Some(n) = ready {
            p.last = n;
        }
        p.last
    }

    /// Every Steam tag whose name contains `query` (case-insensitive), by name.
    fn find_tags(&mut self, query: &str) -> Vec<(u32, String)> {
        self.tag_name(0);
        let q = query.trim().to_lowercase();
        let Some(slot) = &self.tags else { return Vec::new() };
        let Load::Ready(all) = &*slot.lock().unwrap() else { return Vec::new() };
        let mut found: Vec<(u32, String)> = all.iter().filter(|(_, n)| n.to_lowercase().contains(&q)).map(|(id, n)| (*id, n.clone())).collect();
        // Names that start with the query first, then alphabetical.
        found.sort_by_key(|(_, n)| (!n.to_lowercase().starts_with(&q), n.to_lowercase()));
        found
    }

    fn tag_name(&mut self, id: u32) -> Option<String> {
        self.tag_names_ready();
        match &*self.tags.as_ref()?.lock().unwrap() {
            Load::Ready(m) => m.get(&id).cloned(),
            _ => None,
        }
    }

    /// Steam's tag names, requested once and retried after 10s on failure or empty answer.
    fn tag_names_ready(&mut self) -> bool {
        const RETRY: std::time::Duration = std::time::Duration::from_secs(10);
        let state = self.tags.as_ref().map(|s| match &*s.lock().unwrap() {
            Load::Ready(m) if !m.is_empty() => Some(true),
            Load::Loading => None,
            _ => Some(false),
        });
        let retry = matches!(state, Some(Some(false))) && self.tags_asked.is_none_or(|a| a.elapsed() >= RETRY);
        if self.tags.is_none() || retry {
            let api = self.api.clone();
            self.tags = Some(self.fetch(async move { api.tag_names().await }));
            self.tags_asked = Some(std::time::Instant::now());
        } else if matches!(state, Some(Some(false))) {
            self.egui.request_repaint_after(RETRY);
        }
        matches!(state, Some(Some(true)))
    }

    /// Follow the wallet's country (prices) and the interface language.
    /// `false` while signed in but Steam hasn't said the country yet.
    fn sync_locale(&mut self, ctx: &Ctx) -> bool {
        let wallet = ctx.state.wallet.map(|w| w.1);
        let Some(country) = model::store_country(wallet, &ctx.state.country) else {
            if ctx.state.steam_id != 0 {
                return false;
            }
            return self.sync_locale_to("us".into());
        };
        self.sync_locale_to(country)
    }

    fn sync_locale_to(&mut self, country: String) -> bool {
        let language = i18n::language().steam_name();
        if (self.locale.0.as_str(), self.locale.1.as_str()) != (country.as_str(), language) {
            self.api = self.api.with_locale(&country, language);
            self.locale = (country, language.to_string());
            self.front = None;
            self.details.clear();
            self.browse.clear();
            self.recent.clear();
            // Browse results carry prices too: load them again in the new currency.
            self.query = None;
            self.tv_search = None;
            self.tags = None;
            self.ratings.lock().unwrap().clear();
        }
        true
    }

    /// `Some(true)` = fine to show, `Some(false)` = filtered out by the
    /// user's content preferences, `None` = not known yet (show a placeholder).
    fn safe(&mut self, id: AppId) -> Option<bool> {
        if let Some(ids) = self.ratings.lock().unwrap().get(&id) {
            return Some(!ids.iter().any(|d| self.hidden.contains(d)));
        }
        if !self.ratings_pending.contains(&id) {
            self.ratings_pending.push(id);
        }
        None
    }

    /// Fetch content descriptors for everything that asked this frame.
    fn flush_ratings(&mut self) {
        if self.ratings_pending.is_empty() || *self.ratings_busy.lock().unwrap() {
            return;
        }
        *self.ratings_busy.lock().unwrap() = true;
        let ids: Vec<AppId> = self.ratings_pending.drain(..).take(100).collect();
        let (api, out, busy, ctx) = (self.api.clone(), self.ratings.clone(), self.ratings_busy.clone(), self.egui.clone());
        self.rt.spawn(async move {
            match api.browse(&ids).await {
                Ok(items) => {
                    let mut m = out.lock().unwrap();
                    for id in &ids {
                        m.insert(*id, Vec::new());
                    }
                    for i in items {
                        m.insert(i.app_id, i.content_descriptors);
                    }
                }
                Err(e) => tracing::warn!("content ratings: {e}"),
            }
            *busy.lock().unwrap() = false;
            ctx.request_repaint();
        });
    }

    fn open(&mut self, view: View) {
        self.trailer = None;
        self.micro = Default::default();
        self.suggest.open = false;
        if let View::App(_) | View::Discovery(_) = view {
            self.shot = 0;
            self.shot_centered = None;
        }
        // Opened from inside the store: put it in the app's Back history,
        // so Back returns to the previous store page.
        if !self.in_nav && view != self.view {
            self.nav_out = Some(view_arg(&view));
        }
        self.view = view;
    }

    pub(super) fn start_search(&mut self, q: String) {
        let api = self.api.clone();
        let qq = q.clone();
        let slot = self.fetch(async move { api.search(&qq).await.map(Arc::from) });
        self.search = Some((q.clone(), slot));
        self.open(View::Search(q));
    }

    /// This view's address on Steam's store (for "Copy page URL").
    fn page_url(&self, me: model::SteamId) -> String {
        const STORE: &str = "https://store.steampowered.com";
        let app = |id: AppId| format!("{STORE}/app/{id}/");
        match &self.view {
            View::Home => format!("{STORE}/"),
            View::Section(id) => match id.as_str() {
                "specials" => format!("{STORE}/specials"),
                "top_sellers" => format!("{STORE}/charts/topselling"),
                "new_releases" => format!("{STORE}/explore/new/"),
                "coming_soon" => format!("{STORE}/explore/upcoming/"),
                _ => format!("{STORE}/"),
            },
            View::Search(q) => crate::links::with_query(&format!("{STORE}/search/"), &[("term", q)]),
            View::Creator(kind, name) => crate::links::with_query(&format!("{STORE}/search/"), &[(if kind == "publisher" { "publisher" } else { "developer" }, name)]),
            View::App(id) => app(*id),
            View::Browse => format!("{STORE}/search/"),
            View::Discovery(pos) => self.discovery.as_ref().and_then(|r| r.as_ref().ok()).and_then(|q| q.get(*pos).copied()).map(app).unwrap_or_else(|| format!("{STORE}/explore/")),
            View::Points => format!("{STORE}/points/shop"),
            View::Wishlist => format!("{STORE}/wishlist/profiles/{me}/"),
            View::Cart => format!("{STORE}/cart/"),
        }
    }

    pub(super) fn wishlist_entries(&self, ctx: &Ctx) -> Option<Vec<(AppId, u64)>> {
        let base = ctx.state.wishlist.as_ref()?;
        if ctx.state.account != "demo" {
            return Some(base.clone());
        }
        let mut list: Vec<(AppId, u64)> = base.iter().filter(|(id, _)| !self.demo_wish_removed.contains(id)).copied().collect();
        for &(id, added) in &self.demo_wish_added {
            if !list.iter().any(|(x, _)| *x == id) {
                list.push((id, added));
            }
        }
        Some(list)
    }

    pub(super) fn is_wishlisted(&self, ctx: &Ctx, id: AppId) -> bool {
        self.wishlist_entries(ctx).is_some_and(|l| l.iter().any(|(a, _)| *a == id))
    }

    pub(super) fn toggle_wishlist(&mut self, ctx: &mut Ctx, id: AppId, wished: bool) {
        if ctx.state.account == "demo" {
            if wished {
                self.demo_wish_removed.insert(id);
                self.demo_wish_added.retain(|(x, _)| *x != id);
            } else {
                self.demo_wish_removed.remove(&id);
                if !self.demo_wish_added.iter().any(|(x, _)| *x == id) {
                    let now = std::time::SystemTime::now().duration_since(std::time::UNIX_EPOCH).map(|d| d.as_secs()).unwrap_or(0);
                    self.demo_wish_added.push((id, now));
                }
            }
            self.wish.pending = None;
            return;
        }
        ctx.send(if wished { client::Command::WishlistRemove { app_id: id } } else { client::Command::WishlistAdd { app_id: id } });
    }

    /// A developer's or publisher's games, like their store page.
    fn open_creator(&mut self, kind: &str, name: &str) {
        let (api, k, n) = (self.api.clone(), kind.to_string(), name.to_string());
        let slot = self.fetch(async move { api.creator(&k, &n).await.map(Arc::from) });
        self.search = Some((format!("{kind}:{name}"), slot));
        self.open(View::Creator(kind.to_string(), name.to_string()));
    }

    /// Browse everything with one tag (clicked on a game page).
    fn open_tag(&mut self, tag: u32) {
        self.filters = store::query::Filters { tags: vec![tag], ..Default::default() };
        self.browse_page = 0;
        self.open(View::Browse);
    }

    /// A tag id from its (localized) name, e.g. a genre on a game page.
    fn tag_by_name(&mut self, name: &str) -> Option<u32> {
        self.tag_name(0);
        match &*self.tags.as_ref()?.lock().unwrap() {
            Load::Ready(m) => m.iter().find(|(_, n)| n.eq_ignore_ascii_case(name)).map(|(id, _)| *id),
            _ => None,
        }
    }
}

/// What Big Picture reads from the store: the same data, cache and
/// content filter as the store page.
impl Store {
    /// The front page (featured games and sections) in the account's currency.
    pub(crate) fn front_for(&mut self, ctx: &Ctx) -> Option<Result<Arc<StoreFront>, String>> {
        if !self.sync_locale(ctx) {
            return None;
        }
        if self.front.is_none() {
            let api = self.api.clone();
            self.front = Some(self.fetch(async move { api.front().await.map(Arc::new) }));
        }
        self.flush_ratings();
        self.front_data()
    }

    /// Search results for `query` (asked once per new query).
    pub(crate) fn search_for(&mut self, ctx: &Ctx, query: &str) -> Option<Result<Vec<Item>, String>> {
        if !self.sync_locale(ctx) {
            return None;
        }
        if self.tv_search.as_ref().is_none_or(|(q, _)| q != query) {
            let (api, q) = (self.api.clone(), query.to_string());
            let slot = self.fetch(async move { api.search(&q).await });
            self.tv_search = Some((query.to_string(), slot));
        }
        let (_, slot) = self.tv_search.as_ref()?;
        match &*slot.lock().unwrap() {
            Load::Loading => None,
            Load::Ready(v) => Some(Ok(v.clone())),
            Load::Failed(e) => Some(Err(e.clone())),
        }
    }

    /// `None` while locale or content descriptors are still loading.
    pub(crate) fn big_picture_access(&mut self, ctx: &Ctx, id: AppId) -> Option<Result<bool, String>> {
        if !self.sync_locale(ctx) {
            return None;
        }
        if self.allowed.contains(&id) || self.hidden.is_empty() {
            return Some(Ok(true));
        }
        if let Some(classified) = self.shown(id) {
            return Some(Ok(classified));
        }
        self.details_result(id).map(|result| result.map(|details| {
            !details.content_descriptors.iter().any(|descriptor| self.hidden.contains(descriptor))
        }))
    }

    pub(crate) fn allow_big_picture_page(&mut self, id: AppId) {
        self.allowed.insert(id);
    }

    pub(crate) fn details_for(&mut self, ctx: &Ctx, id: AppId) -> Option<Arc<Details>> {
        if !matches!(self.big_picture_access(ctx, id), Some(Ok(true))) {
            return None;
        }
        self.details(id)
    }

    /// `Some(true)` = fine to show, `Some(false)` = hidden by the mature
    /// content settings, `None` = not known yet.
    pub(crate) fn shown(&mut self, id: AppId) -> Option<bool> {
        let r = self.safe(id);
        self.flush_ratings();
        r
    }
}

impl Extension for Store {
    fn manifest(&self) -> Manifest {
        Manifest { id: "store", name: "Store", author: "Aurora", description: "The Steam store, rendered natively", core: true, ..Manifest::DEFAULT }
    }

    fn pages(&self) -> Vec<PageSpec> {
        vec![PageSpec::new("store", "Store", icons::STORE).main().order(10)]
    }

    fn on_event(&mut self, event: &client::Event, ctx: &mut Ctx) {
        if let Some(r) = &mut self.reviews {
            r.on_event(event, ctx);
        }
        if let client::Event::Wishlist(_) = event {
            self.wish.pending = None;
        }
        if let client::Event::DiscoveryQueue(r) = event {
            self.discovery_wait = false;
            self.discovery_late = false;
            self.discovery_asked = None;
            self.discovery = Some(r.clone());
        }
        if let client::Event::LoggedIn { .. } = event {
            // A new account: its own picks and discovery queue.
            self.recs_asked = false;
            self.recs = None;
            self.discovery = None;
            self.discovery_wait = false;
            self.discovery_late = false;
            self.discovery_asked = None;
            self.demo_followed.clear();
            self.demo_ignored.clear();
            self.demo_wish_removed.clear();
            self.demo_wish_added.clear();
        }
        if let client::Event::FreeLicense { app_id, result } = event {
            if self.free_pending == Some(*app_id) {
                self.free_pending = None;
            }
            match result {
                Ok(()) => ctx.toast(t!("Added to your library. You can install it from there.")),
                Err(e) => ctx.toast(i18n::tr_dyn(e).to_string()),
            }
        }
    }

    fn on_navigate(&mut self, _page: &str, arg: Option<&str>) {
        self.in_nav = true;
        self.nav_out = None;
        match arg {
            Some(a) if a.starts_with("section:") => self.open(View::Section(a.trim_start_matches("section:").to_string())),
            Some(a) if a.starts_with("search:") => self.start_search(a.trim_start_matches("search:").to_string()),
            Some(a) if a.starts_with("developer:") || a.starts_with("publisher:") => {
                let (kind, name) = a.split_once(':').unwrap_or_default();
                self.open_creator(kind, name);
            }
            Some(a) if a.starts_with("tag:") => {
                if let Ok(tag) = a.trim_start_matches("tag:").parse() {
                    self.open_tag(tag);
                }
            }
            Some("browse") => self.open(View::Browse),
            Some("discovery") => self.open(View::Discovery(0)),
            Some(a) if a.starts_with("discovery/") => {
                let pos = a.trim_start_matches("discovery/").parse().unwrap_or(0);
                self.open(View::Discovery(pos));
            }
            Some("points") => self.open(View::Points),
            Some(a) if a.starts_with("points:") => {
                if let Ok(id) = a.trim_start_matches("points:").parse() {
                    self.open_points_game(id);
                } else {
                    self.open(View::Points);
                }
            }
            Some("wishlist") => self.open(View::Wishlist),
            Some("cart") => self.open(View::Cart),
            Some(a) => {
                if let Ok(id) = a.trim_start_matches("app:").parse() {
                    self.open(View::App(id));
                }
            }
            None => self.open(View::Home),
        }
        self.in_nav = false;
    }

    fn tick(&mut self, _ctx: &mut Ctx) {
        self.frames += 1;
        if self.frames.saturating_sub(self.drawn_frame) > 2 {
            self.trailer = None;
            self.micro = Default::default();
        }
    }

    fn page(&mut self, _page: &str, ui: &mut Ui, ctx: &mut Ctx) {
        self.drawn_frame = self.frames;
        if let Some(arg) = self.nav_out.take() {
            ctx.navigate("store", arg);
        }
        let t = ctx.theme.clone();
        self.personal(ctx);
        // Refresh the owned set only when the library changes size.
        let count = ctx.state.apps.as_ref().map(|a| a.len()).unwrap_or(0) + ctx.local.games.len();
        if count != self.owned.len() {
            self.owned = ctx.state.apps.as_ref().map(|a| a.keys().copied().collect()).unwrap_or_default();
            self.owned.extend(ctx.local.games.iter().map(|g| g.app_id));
        }
        if !self.sync_locale(ctx) {
            // Don't load prices in the wrong currency while sign-in finishes.
            let (r, _) = ui.allocate_exact_size(ui.available_size(), Sense::hover());
            w::skeleton(ui, &t, r.shrink(28.0), t.corner());
            return;
        }
        if !self.lists_requested && ctx.state.steam_id != 0 {
            self.lists_requested = true;
            ctx.send(client::Command::LoadWishlist);
            ctx.send(client::Command::LoadCart);
        }
        if self.front.is_none() {
            let api = self.api.clone();
            self.front = Some(self.fetch(async move { api.front().await.map(Arc::new) }));
        }
        let url = self.page_url(ctx.state.steam_id);
        crate::links::page_url_menu(ui, &url);
        egui::ScrollArea::vertical().id_salt(("store", view_key(&self.view))).auto_shrink([false, false]).show(ui, |ui| {
            // Paint game background art from the very top of the page so the
            // Store sub-bar and Discovery Queue bar merge seamlessly into it.
            let bg_app = match &self.view {
                View::App(id) => Some(*id),
                View::Discovery(pos) => self.discovery.as_ref().and_then(|r| r.as_ref().ok()).and_then(|q| q.get(*pos).copied()),
                _ => None,
            };
            if let Some(id) = bg_app {
                let bg_url = self.details.get(&id).and_then(|s| match &*s.lock().unwrap() {
                    Load::Ready(d) => Some(d.background.clone()),
                    _ => None,
                });
                if let Some(url) = bg_url {
                    let full = ui.max_rect();
                    let bg = Rect::from_min_size(full.min, vec2(full.width(), 580.0));
                    if w::paint_cover(ui, bg, &url, CornerRadius::ZERO) {
                        ui.painter().rect_filled(bg, CornerRadius::ZERO, t.bg.gamma_multiply(0.55));
                    }
                    w::fade_down(ui, bg, t.bg, 0.0);
                }
            }
            ui.add_space(6.0);
            w::centered(ui, 1200.0, 28.0, |ui| {
                self.subnav(ui, &t, ctx);
            });
            ui.add_space(4.0);
            match self.view.clone() {
                View::App(id) if !self.allowed.contains(&id) && self.details_mature(id) => self.mature_gate(ui, &t, id),
                View::App(id) => self.app_page(ui, &t, ctx, id),
                View::Discovery(pos) => self.discovery_page(ui, &t, ctx, pos),
                view => {
                    ui.add_space(8.0);
                    w::centered(ui, 1200.0, 28.0, |ui| {
                        match view {
                            View::Home => self.home(ui, &t),
                            View::Section(id) => self.section(ui, &t, &id),
                            View::Search(q) => self.search_results(ui, &t, &t!("Results for “{q}”", q = q)),
                            View::Creator(kind, name) => {
                                let title = if kind == "publisher" { t!("Published by {name}", name = name) } else { t!("Developed by {name}", name = name) };
                                self.search_results(ui, &t, &title)
                            }
                            View::Browse => self.browse_page(ui, &t, ctx),
                            View::Points => self.points_page(ui, &t, ctx),
                            View::Wishlist => self.wishlist_page(ui, &t, ctx),
                            View::Cart => self.cart_page(ui, &t, ctx),
                            View::App(_) | View::Discovery(_) => {}
                        }
                        ui.add_space(40.0);
                    });
                }
            }
        });
        if let Some(r) = &mut self.reviews {
            if !r.show(ui, &t, ctx) {
                self.reviews = None;
            }
        }
        self.flush_ratings();
        self.micro_end_frame();
        self.gift_dialog(ui, &t, ctx);
        self.cart_popup_ui(ui, &t, ctx);
    }

    fn settings(&mut self, ui: &mut Ui, ctx: &mut Ctx) -> bool {
        let t = ctx.theme.clone();
        ui.label(w::text_semibold(t!("Mature content"), 15.0, t.text));
        ui.label(w::text(t!("Choose which kinds of content the store shows. Hidden games don't appear in lists, and opening one asks first."), 12.5, t.text_dim));
        ui.add_space(8.0);
        for (id, label) in DESCRIPTORS {
            let mut show = !self.hidden.contains(id);
            if w::check(ui, &t, &mut show, w::text(i18n::tr_dyn(label), 14.0, t.text)).changed() {
                if show {
                    self.hidden.remove(id);
                } else {
                    self.hidden.insert(*id);
                }
            }
        }
        ui.add_space(14.0);
        ui.label(w::text_semibold(t!("Price history"), 15.0, t.text));
        ui.add(egui::Label::new(w::text(t!("Paste a free IsThereAnyDeal API key to see each game's price history and all-time low on its store page. Get one at isthereanydeal.com/apps/my."), 12.5, t.text_dim)).wrap());
        ui.horizontal(|ui| {
            let r = ui.add(egui::TextEdit::singleline(&mut self.itad_key).password(true).hint_text(t!("API key")).desired_width(320.0).margin(vec2(10.0, 8.0))).with_edit_menu();
            if r.changed() {
                self.history.clear();
            }
            if ui.link(w::text(t!("Get a key"), 12.5, t.accent)).clicked() {
                crate::links::open_http("https://isthereanydeal.com/apps/my/");
            }
        });
        true
    }

    fn as_any(&mut self) -> Option<&mut dyn std::any::Any> {
        Some(self)
    }

    fn save(&self) -> Option<String> {
        serde_json::to_string(&Saved { hidden: self.hidden.iter().copied().collect(), itad_key: self.itad_key.clone(), price_log: self.price_log.clone() }).ok()
    }

    fn load(&mut self, data: &str) {
        if let Ok(saved) = serde_json::from_str::<Saved>(data) {
            self.hidden = saved.hidden.into_iter().collect();
            self.itad_key = saved.itad_key;
            self.price_log = saved.price_log;
        } else if let Ok(h) = serde_json::from_str(data) {
            // Older format: just the hidden descriptors.
            self.hidden = h;
        }
    }
}

fn view_key(v: &View) -> String {
    match v {
        View::Home => "home".into(),
        View::Section(s) => format!("s:{s}"),
        View::Search(q) => format!("q:{q}"),
        View::Creator(k, n) => format!("c:{k}:{n}"),
        View::App(id) => format!("a:{id}"),
        View::Browse => "browse".into(),
        View::Discovery(pos) => format!("d:{pos}"),
        View::Points => "points".into(),
        View::Wishlist => "wishlist".into(),
        View::Cart => "cart".into(),
    }
}

/// Padding that clamps i8 margins for very wide windows.
fn pad_margin(ui: &Ui, max: f32) -> f32 {
    ((ui.available_width() - max) / 2.0).max(28.0)
}

impl Store {
    fn front_data(&self) -> Option<Result<Arc<StoreFront>, String>> {
        match &*self.front.as_ref()?.lock().unwrap() {
            Load::Loading => None,
            Load::Ready(d) => Some(Ok(d.clone())),
            Load::Failed(e) => Some(Err(e.clone())),
        }
    }

    /// Single-line Store bar: Home, Browse dropdown (filters + lists), Discovery queue,
    /// search-as-you-type search box, and Wishlist & Cart on the right.
    fn subnav(&mut self, ui: &mut Ui, t: &Theme, ctx: &mut Ctx) {
        let width = ui.available_width();
        let (bar, _) = ui.allocate_exact_size(vec2(width, 36.0), Sense::hover());
        let mut row = ui.new_child(egui::UiBuilder::new().max_rect(bar).layout(Layout::left_to_right(Align::Center)));
        let ui = &mut row;
        ui.spacing_mut().item_spacing.x = 6.0;

        if w::nav_tab_icon(ui, t, Some(icons::SPARKLE), t!("Featured"), self.view == View::Home, 34.0).clicked() {
            self.open(View::Home);
        }
        let in_browse = matches!(self.view, View::Browse | View::Section(_));
        let browse_btn = w::nav_tab_icon(ui, t, Some(icons::SLIDERS), &format!("{}  {}", t!("Browse|store"), icons::CARET_DOWN), in_browse, 34.0);
        egui::Popup::from_toggle_button_response(&browse_btn).close_behavior(egui::PopupCloseBehavior::CloseOnClickOutside).width(230.0).show(|ui| {
            w::menu_spacing(ui);
            if w::menu_item(ui, t, icons::SLIDERS, t!("All games, with filters")).clicked() {
                self.open(View::Browse);
                ui.close();
            }
            let front = self.front_data().and_then(Result::ok);
            for s in front.as_deref().into_iter().flat_map(|(_, sections)| sections) {
                let icon = match s.id.as_str() {
                    "specials" => icons::TAG,
                    "top_sellers" => icons::FIRE,
                    "new_releases" => icons::STAR,
                    _ => icons::CALENDAR,
                };
                if w::menu_item(ui, t, icon, &i18n::tr_dyn(&s.title)).clicked() {
                    self.open(View::Section(s.id.clone()));
                    ui.close();
                }
            }
        });
        if ctx.state.steam_id != 0
            && w::nav_tab_icon(ui, t, Some(icons::STACK), t!("Discovery queue"), matches!(self.view, View::Discovery(_)), 34.0).clicked()
        {
            self.open(View::Discovery(0));
        }
        if w::nav_tab_icon(ui, t, Some(icons::GIFT), t!("Points Shop"), self.view == View::Points, 34.0).clicked() {
            self.open(View::Points);
        }
        if let Some((q, _)) = &self.search {
            let q = q.clone();
            let (label, view) = match q.split_once(':') {
                Some((kind, name)) if kind == "developer" || kind == "publisher" => (name.to_string(), View::Creator(kind.to_string(), name.to_string())),
                _ => (format!("“{q}”"), View::Search(q.clone())),
            };
            if ui.available_width() > 540.0 && w::nav_tab_icon(ui, t, Some(icons::SEARCH), &label, self.view == view, 34.0).clicked() {
                self.open(view);
            }
        }

        let cart = ctx.state.cart.as_ref().map(|c| c.len()).unwrap_or(0);
        let wish = self.wishlist_entries(ctx).map(|w| w.len()).unwrap_or(0);
        let cart_label = format!("{} ({cart})", t!("Cart"));
        let wish_label = format!("{} ({wish})", t!("Wishlist"));
        let tab_w = |lbl: &str| ui.painter().layout_no_wrap(lbl.to_string(), w::font_medium(13.0), t.text).size().x + 48.0;
        let right_reserve = tab_w(&cart_label) + if ctx.state.steam_id != 0 { tab_w(&wish_label) + 6.0 } else { 0.0 } + 16.0;
        let search_w = (ui.available_width() - right_reserve).clamp(150.0, 320.0);
        if ui.available_width() > right_reserve + 90.0 {
            ui.add_space(4.0);
            self.search_box(ui, t, ctx, search_w);
        }

        ui.with_layout(Layout::right_to_left(Align::Center), |ui| {
            ui.spacing_mut().item_spacing.x = 6.0;
            if w::nav_tab_icon(ui, t, Some(icons::CART), &cart_label, self.view == View::Cart, 34.0).clicked() {
                self.open(View::Cart);
            }
            if ctx.state.steam_id != 0
                && w::nav_tab_icon(ui, t, Some(icons::HEART), &wish_label, self.view == View::Wishlist, 34.0).clicked()
            {
                self.open(View::Wishlist);
            }
        });
    }

    fn home(&mut self, ui: &mut Ui, t: &Theme) {
        let Some(data) = self.front_data() else {
            let w_ = ui.available_width();
            let (r, _) = ui.allocate_exact_size(vec2(w_, 380.0), Sense::hover());
            w::skeleton(ui, t, r, t.corner());
            return;
        };
        let front = match data {
            Ok(d) => d,
            Err(e) => {
                w::empty(ui, t, icons::WARNING, t!("The store didn't load"), &e);
                if w::button(ui, t, Some(icons::REFRESH), t!("Try again"), w::Kind::Secondary, w::Size::Md).clicked() {
                    self.front = None;
                }
                return;
            }
        };
        // Hide items the user filtered out; wait for ratings before showing art.
        let (spotlight, sections) = front.as_ref();
        let spotlight: Vec<_> = spotlight.iter().filter(|i| self.safe(i.app_id) == Some(true)).collect();
        let mut sections: Vec<_> = sections
            .iter()
            .map(|s| {
                let items: Vec<_> = s.items.iter().filter(|i| self.safe(i.app_id) != Some(false)).collect();
                (s, items)
            })
            .collect();
        // Your picks first (Steam's discovery queues, friends, favourite tags),
        // without games you own; the featured lists follow.
        let recommendations = self.personal_sections();
        let personal: Vec<_> = recommendations
            .as_deref()
            .unwrap_or_default()
            .iter()
            .map(|s| {
                let items: Vec<_> = s.items.iter().filter(|i| !self.owned.contains(&i.app_id) && self.safe(i.app_id) != Some(false)).collect();
                (s, items)
            })
            .filter(|(_, items)| items.len() >= 3)
            .collect();
        let spotlight = match personal.first() {
            Some((_, items)) => items.iter().copied().filter(|i| self.safe(i.app_id) == Some(true)).take(10).collect(),
            None => spotlight,
        };
        for (_, items) in &mut sections {
            items.retain(|i| !self.owned.contains(&i.app_id));
        }
        if spotlight.is_empty() && !self.ratings_pending.is_empty() {
            let w_ = ui.available_width();
            let (r, _) = ui.allocate_exact_size(vec2(w_, 400.0), Sense::hover());
            w::skeleton(ui, t, r, t.corner());
            ui.add_space(32.0);
        }
        if !spotlight.is_empty() {
            if let Some((s, _)) = personal.first() {
                ui.label(w::text_bold(&s.title, 20.0, t.text));
                ui.add_space(8.0);
            }
            self.carousel(ui, t, &spotlight);
            ui.add_space(32.0);
        }
        for (s, items) in personal.iter().skip(1).chain(sections.iter()) {
            self.shelf(ui, t, s, items);
            ui.add_space(28.0);
        }
    }

    /// Ask for the personal picks once signed in, and turn them into
    /// shelves of store cards when they arrive.
    fn personal(&mut self, ctx: &mut Ctx) {
        if !self.recs_asked && ctx.state.online() {
            self.recs_asked = true;
            ctx.send(client::Command::LoadStoreRecommendations { language: i18n::language().steam_name().to_string() });
        }
        let Some(recs) = ctx.state.store_recs.clone() else { return };
        if self.recs.as_ref().is_some_and(|(r, _)| *r == recs) {
            return;
        }
        let api = self.api.clone();
        let picks = recs.clone();
        let titles: Vec<(String, String)> = [
            ("ml", t!("Recommended for you")),
            ("friends", t!("Popular with your friends")),
            ("on_sale", t!("Recommended specials")),
            ("free", t!("Free games you might like")),
            ("coming_soon", t!("Upcoming games you might like")),
        ]
        .into_iter()
        .map(|(id, title)| (id.to_string(), title.to_string()))
        .collect();
        let tag_title = t!("Top picks in {tag}", tag = "{tag}").to_string();
        let shelves = self.fetch(async move {
            // Shelf id → app ids, in the order Steam gave them.
            let mut lists: Vec<(String, Vec<AppId>)> = picks.queues.clone();
            let mut friends = picks.friends.clone();
            friends.sort_by_key(|(_, n)| std::cmp::Reverse(*n));
            lists.push(("friends".into(), friends.into_iter().map(|(a, _)| a).collect()));
            let ids: Vec<AppId> = lists.iter().flat_map(|(_, l)| l.iter().copied()).collect::<std::collections::BTreeSet<_>>().into_iter().collect();
            let cards: HashMap<AppId, Item> = api.items(&ids).await?.into_iter().map(|q| (q.app_id, query_card(q))).collect();
            let mut out = Vec::new();
            for (id, title) in &titles {
                let Some((_, list)) = lists.iter().find(|(l, _)| l == id) else { continue };
                let items: Vec<Item> = list.iter().filter_map(|a| cards.get(a).cloned()).collect();
                out.push(Section { id: format!("you-{id}"), title: title.clone(), items });
            }
            // Your two favourite tags, best-selling first.
            for (tag, name) in picks.tags.iter().take(2) {
                let f = store::query::Filters { tags: vec![*tag], sort: store::query::Sort::TopSellers, ..Default::default() };
                if let Ok((items, _)) = api.query(&f, 0, 24).await {
                    out.push(Section { id: format!("you-tag-{tag}"), title: tag_title.replace("{tag}", name), items: items.into_iter().map(query_card).collect() });
                }
            }
            Ok(Arc::from(out))
        });
        self.recs = Some((recs, shelves));
    }

    fn personal_sections(&self) -> Option<Arc<[Section]>> {
        match &*self.recs.as_ref()?.1.lock().unwrap() {
            Load::Ready(s) => Some(s.clone()),
            _ => None,
        }
    }

    fn carousel(&mut self, ui: &mut Ui, t: &Theme, items: &[&Item]) {
        let now = ui.input(|i| i.time);
        let n = items.len();
        self.hero %= n;
        let width = ui.available_width();
        let (rect, resp) = ui.allocate_exact_size(vec2(width, 400.0), Sense::click());
        if !resp.hovered() && n > 1 {
            let elapsed = now - self.hero_changed;
            if elapsed >= 8.0 {
                self.hero = (self.hero + 1) % n;
                self.hero_changed = now;
            }
            let remaining = (8.0 - (now - self.hero_changed)).max(0.1);
            ui.ctx().request_repaint_after(std::time::Duration::from_secs_f64(remaining));
        }
        let item = items[self.hero];

        let p = ui.painter();
        p.rect_filled(rect, t.corner(), t.surface);
        let img = Rect::from_min_size(rect.min, vec2(rect.width() * 0.64, rect.height()));
        let r_img = CornerRadius { nw: t.radius, sw: t.radius, ne: 0, se: 0 };
        w::image_or_skeleton(ui, t, img, item.large.as_deref().unwrap_or(&item.header), r_img);
        let owned = self.owned.contains(&item.app_id);
        if owned {
            in_library_badge(ui, t, img);
        }

        // Right panel: title, screenshots, price.
        let side = Rect::from_min_max(pos2(img.right(), rect.top()), rect.max).shrink2(vec2(22.0, 22.0));
        // Everything above the price row, so nothing can run into it.
        let bottom = Rect::from_min_max(pos2(side.left(), side.bottom() - 30.0), side.max);
        let upper = Rect::from_min_max(side.min, pos2(side.right(), bottom.top() - 12.0));
        let mut c = ui.new_child(egui::UiBuilder::new().max_rect(upper).layout(Layout::top_down(Align::Min)));
        c.set_clip_rect(upper.expand(1.0).intersect(ui.clip_rect()));
        c.add(egui::Label::new(w::text_bold(&item.name, 22.0, t.text)).truncate());
        c.add_space(8.0);
        if let Some(d) = self.details(item.app_id) {
            // Screenshots: two rows if they fit above the tags, else one.
            let tags_h = 30.0;
            let room = (upper.bottom() - c.cursor().top() - tags_h - 10.0).max(0.0);
            let mut tw = (side.width() - 8.0) / 2.0;
            let rows = if 2.0 * tw * 0.5625 + 8.0 <= room { 2 } else { 1 };
            if rows == 1 && tw * 0.5625 > room {
                tw = room / 0.5625;
            }
            for row in d.screenshots.chunks(2).take(rows) {
                c.horizontal(|ui| {
                    ui.spacing_mut().item_spacing.x = 8.0;
                    for s in row {
                        let (r, _) = ui.allocate_exact_size(vec2(tw, tw * 0.5625), Sense::hover());
                        w::image_or_skeleton(ui, t, r, &s.thumb, t.corner_small());
                    }
                });
            }
            c.add_space(10.0);
            // One row of genre pills: stop at the first one that doesn't fit.
            c.horizontal(|ui| {
                ui.spacing_mut().item_spacing.x = 6.0;
                for g in d.genres.iter().take(4) {
                    let need = ui.painter().layout_no_wrap(g.clone(), w::font(12.0), t.text_dim).size().x + 22.0;
                    if need > ui.available_width() {
                        break;
                    }
                    w::pill(ui, g, t.text_dim, t.raised);
                }
            });
        } else {
            let tw = side.width();
            let (r, _) = c.allocate_exact_size(vec2(tw, (tw * 0.56).min(upper.height() - 40.0).max(0.0)), Sense::hover());
            w::skeleton(&c, t, r, t.corner_small());
        }
        // Price on the right first; the label gets what's left and shortens itself.
        let mut b = ui.new_child(egui::UiBuilder::new().max_rect(bottom).layout(Layout::right_to_left(Align::Center)));
        if owned {
            b.label(w::text_semibold(t!("In library"), 14.0, t.play));
        } else {
            w::price_tag(&mut b, t, &item.price.label(), item.price.original_label().as_deref(), item.price.discount);
        }
        b.add_space(10.0);
        b.with_layout(Layout::left_to_right(Align::Center), |ui| {
            let label = if item.price.discount > 0 { t!("Special offer") } else { t!("Now available") };
            ui.add(egui::Label::new(w::text_medium(label, 13.0, t.text_dim)).truncate());
        });

        // Arrows and dots.
        for (dir, x) in [(-1i32, rect.left() - 18.0), (1, rect.right() + 18.0)] {
            let r = Rect::from_center_size(pos2(x, rect.center().y), vec2(32.0, 60.0));
            let a = ui.interact(r, egui::Id::unique(("hero-arrow", dir)), Sense::click());
            let col = if a.hovered() { t.text } else { t.text_faint };
            ui.painter().text(r.center(), Align2::CENTER_CENTER, if dir < 0 { icons::BACK } else { icons::CARET_RIGHT }, w::font(26.0), col);
            if a.clicked() {
                self.hero = (self.hero as i32 + dir).rem_euclid(n as i32) as usize;
                self.hero_changed = now;
            }
        }
        let dots_w = n as f32 * 18.0;
        for i in 0..n {
            let c = pos2(rect.center().x - dots_w / 2.0 + i as f32 * 18.0 + 9.0, rect.bottom() + 16.0);
            let r = Rect::from_center_size(c, vec2(14.0, 14.0));
            let d = ui.interact(r, egui::Id::unique(("hero-dot", i)), Sense::click());
            let on = i == self.hero;
            ui.painter().rect_filled(Rect::from_center_size(c, vec2(if on { 18.0 } else { 8.0 }, 4.0)), CornerRadius::same(2), if on { t.accent } else { t.border });
            if d.clicked() {
                self.hero = i;
                self.hero_changed = now;
            }
        }
        ui.add_space(14.0);
        if resp.on_hover_cursor(egui::CursorIcon::PointingHand).clicked() {
            self.open(View::App(item.app_id));
        }
    }

    fn shelf(&mut self, ui: &mut Ui, t: &Theme, s: &Section, items: &[&Item]) {
        let card_w = 272.0;
        let gap = 16.0;
        let per = (((ui.available_width() + gap) / (card_w + gap)).floor() as usize).max(1);
        let pages = items.len().div_ceil(per);
        let page = self.shelf_page.get(&s.id).copied().unwrap_or(0).min(pages.saturating_sub(1));
        let mut see_all = false;
        ui.horizontal(|ui| {
            ui.label(w::text_bold(&s.title, 20.0, t.text));
            ui.with_layout(Layout::right_to_left(Align::Center), |ui| {
                ui.add_enabled_ui(page + 1 < pages, |ui| {
                    if w::icon_button(ui, t, icons::CARET_RIGHT, t!("Next"), false, 0).clicked() {
                        self.shelf_page.insert(s.id.clone(), page + 1);
                    }
                });
                ui.add_enabled_ui(page > 0, |ui| {
                    if w::icon_button(ui, t, icons::BACK, t!("Previous"), false, 0).clicked() {
                        self.shelf_page.insert(s.id.clone(), page - 1);
                    }
                });
                ui.add_space(8.0);
                see_all = w::button(ui, t, None, t!("See all"), w::Kind::Ghost, w::Size::Sm).clicked();
            });
        });
        ui.add_space(8.0);
        let actual_w = (ui.available_width() - gap * (per as f32 - 1.0)) / per as f32;
        ui.horizontal(|ui| {
            ui.spacing_mut().item_spacing.x = gap;
            for item in items.iter().copied().skip(page * per).take(per) {
                if self.safe(item.app_id).is_none() {
                    let h = actual_w * 215.0 / 460.0 + 64.0;
                    let (r, _) = ui.allocate_exact_size(vec2(actual_w, h), Sense::hover());
                    w::skeleton(ui, t, r, t.corner());
                    continue;
                }
                let owned = self.owned.contains(&item.app_id);
                if self.store_card(ui, t, item, actual_w, owned).clicked() {
                    self.open(View::App(item.app_id));
                }
            }
        });
        if see_all {
            self.open(View::Section(s.id.clone()));
        }
    }

    fn section(&mut self, ui: &mut Ui, t: &Theme, id: &str) {
        let Some(Ok(front)) = self.front_data() else {
            w::loading(ui, t, t!("Loading…"));
            return;
        };
        let Some(s) = front.1.iter().find(|s| s.id == id) else { return };
        let items: Vec<_> = s.items.iter().filter(|i| self.safe(i.app_id) == Some(true)).collect();
        w::page_title(ui, t, &i18n::tr_dyn(&s.title), Some(&t!("{n} games", n = items.len())));
        self.grid(ui, t, &items);
    }

    fn grid(&mut self, ui: &mut Ui, t: &Theme, items: &[&Item]) {
        let gap = 16.0;
        let per = (((ui.available_width() + gap) / (272.0 + gap)).floor() as usize).max(1);
        let cw = (ui.available_width() - gap * (per as f32 - 1.0)) / per as f32;
        for row in items.chunks(per) {
            ui.horizontal(|ui| {
                ui.spacing_mut().item_spacing.x = gap;
                for &item in row {
                    let owned = self.owned.contains(&item.app_id);
                    if self.store_card(ui, t, item, cw, owned).clicked() {
                        self.open(View::App(item.app_id));
                    }
                }
            });
            ui.add_space(gap);
        }
    }

    fn search_results(&mut self, ui: &mut Ui, t: &Theme, title: &str) {
        w::page_title(ui, t, title, None);
        let Some((_, slot)) = &self.search else { return };
        let items = match &*slot.lock().unwrap() {
            Load::Loading => None,
            Load::Ready(v) => Some(Ok(v.clone())),
            Load::Failed(e) => Some(Err(e.clone())),
        };
        match items {
            None => w::loading(ui, t, t!("Searching…")),
            Some(Err(e)) => w::empty(ui, t, icons::WARNING, t!("Search failed"), &e),
            Some(Ok(v)) if v.is_empty() => w::empty(ui, t, icons::SEARCH, t!("No results"), t!("Try a different name.")),
            Some(Ok(v)) => {
                let v: Vec<_> = v.iter().filter(|i| self.safe(i.app_id) == Some(true)).collect();
                if v.is_empty() && !self.ratings_pending.is_empty() {
                    w::loading(ui, t, t!("Searching…"));
                }
                for item in &v {
                    let (rect, resp) = ui.allocate_exact_size(vec2(ui.available_width(), 84.0), Sense::click());
                    let hover = ui.ctx().animate_bool_with_time(resp.id, resp.hovered(), 0.1);
                    ui.painter().rect_filled(rect, t.corner(), w::mix(t.surface, t.surface_hover, hover));
                    let img = Rect::from_min_size(rect.min + vec2(10.0, 10.0), vec2(137.0, 64.0));
                    w::image_or_skeleton(ui, t, img, &item.header, t.corner_small());
                    let mut c = ui.new_child(egui::UiBuilder::new().max_rect(Rect::from_min_max(pos2(img.right() + 16.0, rect.top()), rect.max - vec2(16.0, 0.0))).layout(Layout::left_to_right(Align::Center)));
                    c.vertical(|ui| {
                        ui.add_space(18.0);
                        ui.label(w::text_semibold(&item.name, 16.0, t.text));
                        if item.windows {
                            ui.label(w::text(icons::WINDOWS, 13.0, t.text_faint));
                        }
                    });
                    c.with_layout(Layout::right_to_left(Align::Center), |ui| {
                        if self.owned.contains(&item.app_id) {
                            w::pill(ui, t!("In library"), t.play, t.play.gamma_multiply(0.15));
                        } else {
                            w::price_tag(ui, t, &item.price.label(), item.price.original_label().as_deref(), item.price.discount);
                        }
                    });
                    if resp.on_hover_cursor(egui::CursorIcon::PointingHand).clicked() {
                        self.open(View::App(item.app_id));
                    }
                    ui.add_space(8.0);
                }
            }
        }
    }

    fn app_page(&mut self, ui: &mut Ui, t: &Theme, ctx: &mut Ctx, id: AppId) {
        let max = 1200.0;
        let pad = pad_margin(ui, max);
        self.touch(id);
        let state = self.details.get(&id).map(|s| match &*s.lock().unwrap() {
            Load::Loading => None,
            Load::Ready(d) => Some(Ok(d.clone())),
            Load::Failed(e) => Some(Err(e.clone())),
        });
        let d = match state.flatten() {
            Some(Ok(d)) => d,
            Some(Err(e)) => {
                egui::Frame::new().inner_margin(Margin::same(32)).show(ui, |ui| {
                    if w::button(ui, t, Some(icons::ARROW_LEFT), t!("Store"), w::Kind::Ghost, w::Size::Sm).clicked() {
                        self.open(View::Home);
                    }
                    w::empty(ui, t, icons::WARNING, t!("This game isn't available"), &e);
                });
                return;
            }
            None => {
                self.details(id);
                w::loading(ui, t, t!("Loading game…"));
                return;
            }
        };

        ui.add_space(6.0);
        w::centered(ui, max, pad.min(28.0), |ui| {
            ui.horizontal(|ui| {
                if w::button(ui, t, Some(icons::ARROW_LEFT), t!("Store"), w::Kind::Ghost, w::Size::Sm).clicked() {
                    self.open(View::Home);
                }
                if let Some(g) = d.genres.first() {
                    ui.label(w::text(icons::CARET_RIGHT, 13.0, t.text_dim));
                    if link(ui, t, g, 13.0, t.text_dim).clicked() {
                        if let Some(tag) = self.tag_by_name(g) {
                            self.open_tag(tag);
                        }
                    }
                }
            });
            ui.add_space(6.0);
            ui.label(w::text_bold(&d.name, 34.0, Color32::WHITE));
            ui.add_space(14.0);

            let total = ui.available_width();
            let right_w = (total * 0.34).clamp(300.0, 420.0);
            let left_w = total - right_w - 24.0;
            ui.horizontal_top(|ui| {
                ui.spacing_mut().item_spacing.x = 24.0;
                ui.vertical(|ui| {
                    ui.set_width(left_w);
                    let trailers = self.browse_info(id).map(|b| b.trailers.clone()).unwrap_or_default();
                    self.media(ui, t, &d, &trailers, left_w);
                });
                ui.vertical(|ui| {
                    ui.set_width(right_w);
                    let (r, _) = ui.allocate_exact_size(vec2(right_w, right_w * 215.0 / 460.0), Sense::hover());
                    w::image_or_skeleton(ui, t, r, &d.header, t.corner_small());
                    ui.add_space(8.0);
                    ui.add(egui::Label::new(w::text(&d.short_description, 14.0, t.text_dim)).wrap());
                    ui.add_space(10.0);
                    let info = self.browse_info(id);
                    egui::Grid::new("app-facts").num_columns(2).spacing(vec2(14.0, 6.0)).show(ui, |ui| {
                        if let Some(r) = info.as_ref().and_then(|i| i.reviews.clone()) {
                            ui.label(w::caption_text(&t, t!("REVIEWS")));
                            ui.horizontal(|ui| {
                                ui.label(w::text_semibold(i18n::tr_dyn(&r.label), 13.5, review_color(t, r.score)));
                                ui.label(w::text(t!("({percent}% of {count})", percent = r.percent_positive, count = thousands(r.count)), 12.5, t.text_faint));
                            });
                            ui.end_row();
                        } else if let Some(r) = d.recommendations {
                            fact(ui, t, t!("REVIEWS"), &format!("{} {}", icons::THUMBS_UP, thousands(r)));
                        }
                        if let Some(m) = d.metacritic {
                            ui.label(w::caption_text(&t, t!("METACRITIC")));
                            let col = if m >= 75 { Color32::from_rgb(0x66, 0xCC, 0x33) } else if m >= 50 { t.away } else { t.danger };
                            w::pill(ui, &m.to_string(), Color32::BLACK, col);
                            ui.end_row();
                        }
                        if let Some(n) = self.current_players(id) {
                            ui.label(w::caption_text(&t, t!("PLAYING NOW")));
                            ui.horizontal(|ui| {
                                ui.spacing_mut().item_spacing.x = 6.0;
                                let (dot, _) = ui.allocate_exact_size(vec2(8.0, 8.0), Sense::hover());
                                ui.painter().circle_filled(dot.center(), 4.0, t.play);
                                ui.label(w::text_semibold(thousands(n as u64), 13.5, t.text));
                            })
                            .response
                            .on_hover_text(t!("People playing on Steam right now. Updates every minute."));
                            ui.end_row();
                        }
                        fact(ui, t, t!("Release"), &d.release_date);
                        for (label, kind, names) in [(t!("Developer"), "developer", &d.developers), (t!("Publisher"), "publisher", &d.publishers)] {
                            if names.is_empty() {
                                continue;
                            }
                            ui.label(w::caption_text(t, label));
                            ui.horizontal_wrapped(|ui| {
                                ui.spacing_mut().item_spacing.x = 0.0;
                                for (i, name) in names.iter().enumerate() {
                                    if i > 0 {
                                        ui.label(w::text(", ", 13.0, t.text_dim));
                                    }
                                    if link(ui, t, name, 13.0, t.accent).on_hover_text(t!("All games by {name}", name = name)).clicked() {
                                        self.open_creator(kind, name);
                                    }
                                }
                            });
                            ui.end_row();
                        }
                    });
                    ui.add_space(10.0);
                    ui.horizontal_wrapped(|ui| {
                        ui.spacing_mut().item_spacing = vec2(6.0, 6.0);
                        // Popular tags (or the genres before tags load); each opens Browse with it.
                        let tags: Vec<(Option<u32>, String)> = info.as_ref().map(|i| i.tags.iter().take(8).filter_map(|id| Some((Some(*id), self.tag_name(*id)?))).collect()).unwrap_or_default();
                        let list: Vec<(Option<u32>, String)> = if tags.is_empty() { d.genres.iter().map(|g| (None, g.clone())).collect() } else { tags };
                        for (id, name) in &list {
                            if tag_chip(ui, t, name).clicked() {
                                if let Some(tag) = id.or_else(|| self.tag_by_name(name)) {
                                    self.open_tag(tag);
                                }
                            }
                        }
                    });
                });
            });
            ui.add_space(28.0);

            // Purchase box.
            let owned = ctx.state.apps.as_ref().is_some_and(|a| a.contains_key(&id)) || ctx.local.games.iter().any(|g| g.app_id == id);
            w::card(t).show(ui, |ui| {
                ui.set_width(ui.available_width());
                ui.horizontal(|ui| {
                    ui.vertical(|ui| {
                        let title = if d.is_free { t!("Play {name}", name = d.name) } else { t!("Buy {name}", name = d.name) };
                        ui.label(w::text_bold(title, 18.0, t.text));
                        let (win, mac, lin) = d.platforms;
                        let mut plats = String::new();
                        for (on, i) in [(win, icons::WINDOWS), (mac, icons::APPLE), (lin, icons::LINUX)] {
                            if on {
                                plats.push_str(i);
                                plats.push_str("  ");
                            }
                        }
                        ui.label(w::text(plats, 15.0, t.text_dim));
                    });
                    ui.with_layout(Layout::right_to_left(Align::Center), |ui| {
                        if owned {
                            if w::button(ui, t, Some(icons::BOOKS), t!("In your library"), w::Kind::Play, w::Size::Lg).clicked() {
                                ctx.navigate("library", Some(id.to_string()));
                            }
                            // Like Steam: you can still buy copies, as gifts.
                            if let Some(pkg) = d.package_id.filter(|_| !d.is_free) {
                                ui.add_space(8.0);
                                let price = d.price.as_ref().map(|p| (p.label(), p.original_label(), p.discount));
                                self.purchase_actions(ui, t, ctx, id, Some(pkg), &d.name, price.clone(), true);
                                ui.add_space(8.0);
                                if let Some((label, original, discount)) = &price {
                                    w::price_tag(ui, t, label, original.as_deref(), *discount);
                                }
                            }
                        } else if d.is_free {
                            // Like Steam's "Play Game": the free license goes on the account.
                            let pending = self.free_pending == Some(id);
                            let label = if pending { t!("Adding…") } else { t!("Add to library") };
                            let r = ui.add_enabled_ui(!pending, |ui| w::button(ui, t, Some(icons::PLUS), label, w::Kind::Play, w::Size::Lg)).inner;
                            if r.on_hover_text(t!("Free to play: adds it to your account, then you can install it")).clicked() {
                                self.free_pending = Some(id);
                                ctx.send(client::Command::RequestFreeLicense { app_id: id });
                            }
                        } else if d.coming_soon {
                            w::pill(ui, t!("Coming soon"), t.text, t.raised);
                            ui.add_space(8.0);
                            self.purchase_actions(ui, t, ctx, id, None, &d.name, None, false);
                        } else {
                            let price = d.price.as_ref().map(|p| (p.label(), p.original_label(), p.discount));
                            self.purchase_actions(ui, t, ctx, id, d.package_id, &d.name, price, false);
                            ui.add_space(8.0);
                            if let Some(p) = &d.price {
                                if let Some(c) = p.final_ {
                                    self.log_price(id, c);
                                }
                            }
                            match &d.price {
                                Some(p) => w::price_tag(ui, t, &p.label(), p.original_label().as_deref(), p.discount),
                                None => {
                                    ui.label(w::text_semibold(d.price_text.clone().unwrap_or_default(), 15.0, t.text));
                                }
                            }
                        }
                    });
                });
            });
            ui.add_space(12.0);
            self.reviews_card(ui, t, id, &d.name, owned);
            ui.add_space(28.0);

            ui.horizontal_top(|ui| {
                ui.spacing_mut().item_spacing.x = 24.0;
                ui.vertical(|ui| {
                    ui.set_width(left_w);
                    if !d.is_free {
                        self.price_history(ui, t, id);
                    }
                    w::section(ui, t, t!("About this game"), None);
                    blocks(ui, t, &d.about, left_w);
                    ui.add_space(24.0);
                    w::section(ui, t, t!("System requirements"), None);
                    ui.columns(2, |cols| {
                        cols[0].label(w::text_semibold(t!("Minimum"), 14.0, t.text_dim));
                        blocks(&mut cols[0], t, &d.requirements_min, left_w / 2.0);
                        cols[1].label(w::text_semibold(t!("Recommended"), 14.0, t.text_dim));
                        blocks(&mut cols[1], t, &d.requirements_rec, left_w / 2.0);
                    });
                });
                ui.vertical(|ui| {
                    ui.set_width(right_w);
                    self.relevance(ui, t, ctx, id, &d);
                    ui.add_space(12.0);
                    if !d.categories.is_empty() {
                        w::card(t).show(ui, |ui| {
                            ui.set_width(ui.available_width());
                            ui.label(w::text_semibold(t!("Features"), 14.0, t.text));
                            ui.add_space(4.0);
                            for c in &d.categories {
                                ui.horizontal(|ui| {
                                    ui.label(w::text(feature_icon(c), 15.0, t.accent));
                                    ui.label(w::text(c, 13.5, t.text_dim));
                                });
                            }
                        });
                        ui.add_space(12.0);
                    }
                    w::card(t).show(ui, |ui| {
                        ui.set_width(ui.available_width());
                        ui.label(w::text_semibold(t!("Languages"), 14.0, t.text));
                        ui.add(egui::Label::new(w::text(d.languages.replace("*languages with full audio support", ""), 12.5, t.text_dim)).wrap());
                        if let Some(site) = &d.website {
                            ui.add_space(8.0);
                            if ui.link(w::text(format!("{}  {}", icons::GLOBE, t!("Official website")), 13.0, t.accent)).clicked() {
                                crate::links::open_http(site);
                            }
                        }
                    });
                });
            });
        });
    }

    fn media(&mut self, ui: &mut Ui, t: &Theme, d: &Details, trailers: &[store::browse::Trailer], width: f32) {
        // Trailers come first in the strip, like on Steam. `self.shot` counts
        // trailers first, then screenshots.
        let n_tr = trailers.len();
        let total = n_tr + d.screenshots.len();
        if total == 0 {
            let (r, _) = ui.allocate_exact_size(vec2(width, width * 0.5625), Sense::hover());
            w::image_or_skeleton(ui, t, r, &d.header, t.corner());
            return;
        }
        self.shot %= total;
        let (main, resp) = ui.allocate_exact_size(vec2(width, width * 0.5625), Sense::click());
        if self.shot >= n_tr {
            // Leaving the trailers stops playback.
            self.trailer = None;
        }
        if self.shot < n_tr {
            let tr = &trailers[self.shot];
            let source = tr.hls.clone().map(video::Source::Hls).or_else(|| tr.micro_mp4.clone().map(video::Source::Mp4));
            if let Some(src) = source {
                self.drawn_frame = self.frames;
                if self.trailer.as_ref().is_none_or(|(url, _)| url != src.url()) {
                    // A picked trailer plays right away (muted until the user
                    // turns sound on once), like Steam; full screen carries over.
                    let fullscreen = self.trailer.as_ref().is_some_and(|(_, p)| p.fullscreen);
                    let url = src.url().to_string();
                    let mut p = video::Player::new(&self.rt, ui.ctx(), src, video::widget::options(ui.ctx()));
                    p.fullscreen = fullscreen;
                    self.trailer = Some((url, p));
                }
                let (_, p) = self.trailer.as_mut().expect("just set");
                if video::widget::show(ui, t, main, p, &tr.thumbnail, &tr.name).ended {
                    // Like Steam: on to the next trailer or screenshot.
                    self.step_shot(1, total);
                }
            } else {
                w::image_or_skeleton(ui, t, main, &tr.thumbnail, t.corner());
                ui.painter().text(main.left_bottom() + vec2(16.0, -16.0), Align2::LEFT_BOTTOM, &tr.name, w::font_semibold(15.0), Color32::WHITE);
            }
        } else {
            let s = &d.screenshots[self.shot - n_tr];
            if !w::paint_cover(ui, main, &s.full, t.corner()) {
                w::image_or_skeleton(ui, t, main, &s.thumb, t.corner());
            }
        }
        // Previous / next on the sides while the pointer is over it, like Steam.
        if total > 1 && ui.rect_contains_pointer(main) {
            for (dir, icon, x) in [(-1i32, icons::CARET_LEFT, main.left() + 30.0), (1, icons::CARET_RIGHT, main.right() - 30.0)] {
                let r = Rect::from_center_size(pos2(x, main.center().y), vec2(44.0, 64.0));
                let b = ui.interact(r, ui.scope_id().with(("media-arrow", dir)), Sense::click());
                ui.painter().rect_filled(r, CornerRadius::same(6), Color32::from_black_alpha(if b.hovered() { 200 } else { 130 }));
                ui.painter().text(r.center(), Align2::CENTER_CENTER, icon, w::font(24.0), Color32::WHITE);
                if b.on_hover_cursor(egui::CursorIcon::PointingHand).clicked() {
                    self.step_shot(dir, total);
                }
            }
        } else if resp.clicked() && self.shot >= n_tr {
            self.step_shot(1, total);
        }
        ui.add_space(8.0);
        self.strip(ui, t, d, trailers, width);
    }

    /// Move the highlight by one, wrapping around.
    fn step_shot(&mut self, dir: i32, total: usize) {
        self.shot = (self.shot as i32 + dir).rem_euclid(total as i32) as usize;
    }

    /// The thumbnail strip: arrows at both ends, and the picked thumbnail
    /// scrolls to the middle, like Steam's.
    fn strip(&mut self, ui: &mut Ui, t: &Theme, d: &Details, trailers: &[store::browse::Trailer], width: f32) {
        let n_tr = trailers.len();
        let total = n_tr + d.screenshots.len();
        let arrow_w = 28.0;
        let center_now = self.shot_centered != Some(self.shot);
        let mut pick = None;
        ui.horizontal(|ui| {
            ui.spacing_mut().item_spacing.x = 6.0;
            let arrow = |ui: &mut Ui, icon: &str| -> bool {
                let (r, b) = ui.allocate_exact_size(vec2(arrow_w, 72.0), Sense::click());
                let hover = ui.ctx().animate_bool_with_time(b.id, b.hovered(), 0.1);
                ui.painter().rect_filled(r, t.corner_small(), w::mix(t.raised, t.surface_hover, hover));
                ui.painter().text(r.center(), Align2::CENTER_CENTER, icon, w::font(18.0), w::mix(t.text_dim, t.text, hover));
                b.on_hover_cursor(egui::CursorIcon::PointingHand).clicked()
            };
            if arrow(ui, icons::CARET_LEFT) && total > 0 {
                pick = Some((self.shot + total - 1) % total);
            }
            let inner = (width - 2.0 * (arrow_w + 6.0)).max(100.0);
            ui.allocate_ui(vec2(inner, 80.0), |ui| {
                egui::ScrollArea::horizontal().id_salt("shots").max_width(inner).scroll_bar_visibility(egui::scroll_area::ScrollBarVisibility::AlwaysHidden).show(ui, |ui| {
                    ui.horizontal(|ui| {
                        ui.spacing_mut().item_spacing.x = 8.0;
                        for i in 0..total {
                            let (r, resp) = ui.allocate_exact_size(vec2(128.0, 72.0), Sense::click());
                            if i < n_tr {
                                let tr = &trailers[i];
                                w::image_or_skeleton(ui, t, r, &tr.thumbnail, t.corner_small());
                                ui.painter().circle_filled(r.center(), 14.0, Color32::from_black_alpha(150));
                                ui.painter().text(r.center() + vec2(1.5, 0.0), Align2::CENTER_CENTER, icons::PLAY, w::font(14.0), Color32::WHITE);
                            } else {
                                w::image_or_skeleton(ui, t, r, &d.screenshots[i - n_tr].thumb, t.corner_small());
                            }
                            if i == self.shot {
                                ui.painter().rect_stroke(r, t.corner_small(), Stroke::new(2.0, Color32::WHITE), egui::StrokeKind::Inside);
                                if center_now {
                                    resp.scroll_to_me(Some(Align::Center));
                                }
                            } else if !resp.hovered() {
                                ui.painter().rect_filled(r, t.corner_small(), Color32::from_black_alpha(90));
                            }
                            let resp = if i < n_tr { resp.on_hover_text(&trailers[i].name) } else { resp };
                            if resp.on_hover_cursor(egui::CursorIcon::PointingHand).clicked() {
                                pick = Some(i);
                            }
                        }
                    });
                });
            });
            if arrow(ui, icons::CARET_RIGHT) && total > 0 {
                pick = Some((self.shot + 1) % total);
            }
        });
        if center_now {
            self.shot_centered = Some(self.shot);
        }
        if let Some(i) = pick {
            self.shot = i;
        }
    }
}

/// Text that underlines on hover, like Steam's store links.
fn link(ui: &mut Ui, t: &Theme, text: &str, size: f32, color: Color32) -> egui::Response {
    let r = ui.add(egui::Label::new(w::text(text, size, color)).sense(Sense::click()).selectable(false));
    if r.hovered() {
        ui.painter().hline(r.rect.x_range(), r.rect.bottom() - 1.0, Stroke::new(1.0, t.text));
    }
    r.on_hover_cursor(egui::CursorIcon::PointingHand)
}

/// A tag on a game page: a pill you can click.
fn tag_chip(ui: &mut Ui, t: &Theme, name: &str) -> egui::Response {
    let galley = ui.painter().layout_no_wrap(name.to_string(), w::font(12.5), t.text_dim);
    let (r, resp) = ui.allocate_exact_size(galley.size() + vec2(20.0, 10.0), Sense::click());
    let hover = ui.ctx().animate_bool_with_time(resp.id, resp.hovered(), 0.1);
    ui.painter().rect_filled(r, CornerRadius::same(4), w::mix(t.raised, t.accent.gamma_multiply(0.35), hover));
    let color = w::mix(t.text_dim, t.text, hover);
    ui.painter().galley(r.center() - galley.size() / 2.0, galley, color);
    resp.on_hover_cursor(egui::CursorIcon::PointingHand)
}

fn fact(ui: &mut Ui, t: &Theme, label: &str, value: &str) {
    ui.label(w::caption_text(t, label));
    ui.add(egui::Label::new(w::text(value, 13.0, t.text)).truncate());
    ui.end_row();
}

pub(crate) fn thousands(n: u64) -> String {
    let s = n.to_string();
    let mut out = String::new();
    for (i, c) in s.chars().enumerate() {
        if i > 0 && (s.len() - i) % 3 == 0 {
            out.push(',');
        }
        out.push(c);
    }
    out
}

impl Store {
    /// Store game card with hover microtrailer playback, keeping `"In library"`
    /// painted above any playing preview clip.
    pub(super) fn store_card(&mut self, ui: &mut Ui, t: &Theme, item: &Item, width: f32, owned: bool) -> egui::Response {
        let (r, img) = card(ui, t, item, width, owned);
        self.micro(ui, item.app_id, img, CornerRadius { nw: t.radius, ne: t.radius, sw: 0, se: 0 }, r.hovered());
        if owned && ui.is_rect_visible(img) {
            in_library_badge(ui, t, img);
        }
        r
    }
}

/// One store card: header art, name, price. Returns the card response and art rect.
fn card(ui: &mut Ui, t: &Theme, item: &Item, width: f32, owned: bool) -> (egui::Response, Rect) {
    let h = width * 215.0 / 460.0;
    let (rect, resp) = ui.allocate_exact_size(vec2(width, h + 64.0), Sense::click());
    if !ui.is_rect_visible(rect) {
        return (resp, Rect::from_min_size(rect.min, vec2(width, h)));
    }
    let hover = ui.ctx().animate_bool_with_time(resp.id, resp.hovered(), 0.15);
    let lift = rect.translate(vec2(0.0, -3.0 * hover));
    let p = ui.painter();
    if hover > 0.0 {
        p.rect_filled(lift.translate(vec2(0.0, 10.0)), t.corner(), Color32::from_black_alpha((80.0 * hover) as u8));
    }
    p.rect_filled(lift, t.corner(), w::mix(t.surface, t.surface_hover, hover));
    let img = Rect::from_min_size(lift.min, vec2(width, h));
    w::image_or_skeleton(ui, t, img, &item.header, CornerRadius { nw: t.radius, ne: t.radius, sw: 0, se: 0 });
    let body = Rect::from_min_max(pos2(lift.left() + 12.0, img.bottom() + 8.0), lift.max - vec2(12.0, 8.0));
    let mut c = ui.new_child(egui::UiBuilder::new().max_rect(body).layout(Layout::top_down(Align::Min)));
    c.spacing_mut().item_spacing.y = 2.0;
    c.add(egui::Label::new(w::text_medium(&item.name, 14.0, t.text)).truncate());
    c.with_layout(Layout::right_to_left(Align::Center), |ui| {
        // Like Steam: games you own say so instead of showing a price.
        if owned {
            ui.label(w::text_semibold(t!("In library"), 13.0, t.play));
        } else {
            w::price_tag(ui, t, &item.price.label(), item.price.original_label().as_deref(), item.price.discount);
        }
        ui.with_layout(Layout::left_to_right(Align::Center), |ui| {
            if item.windows {
                ui.label(w::text(icons::WINDOWS, 13.0, t.text_faint));
            }
        });
    });
    (resp.on_hover_cursor(egui::CursorIcon::PointingHand), img)
}

/// "In library" ribbon on a store picture's top-left corner, like Steam's.
fn in_library_badge(ui: &Ui, t: &Theme, img: Rect) {
    let text = format!("{}  {}", icons::CHECK, t!("In library"));
    let galley = ui.painter().layout_no_wrap(text, w::font_semibold(11.5), Color32::WHITE);
    let r = Rect::from_min_size(img.min + vec2(8.0, 8.0), galley.size() + vec2(14.0, 8.0));
    ui.painter().rect_filled(r, CornerRadius::same(4), t.play.gamma_multiply(0.92));
    ui.painter().galley(r.min + vec2(7.0, 4.0), galley, Color32::WHITE);
}

/// Lay out store HTML / Steam BBCode blocks.
pub fn blocks(ui: &mut Ui, t: &Theme, blocks: &[Block], width: f32) {
    let job = |runs: &[(String, bool)], color: Color32| {
        let mut job = LayoutJob::default();
        for (i, (text, bold)) in runs.iter().enumerate() {
            let sep = if i > 0 { " " } else { "" };
            let font = if *bold { w::font_semibold(14.0) } else { w::font(14.0) };
            job.append(&format!("{sep}{text}"), 0.0, TextFormat { font_id: font, color, line_height: Some(21.0), ..Default::default() });
        }
        job.wrap.max_width = width;
        job
    };
    for b in blocks {
        match b {
            Block::Heading(s) => {
                ui.add_space(12.0);
                ui.label(w::text_bold(s.to_uppercase(), 13.5, t.text));
                ui.add_space(2.0);
            }
            Block::Paragraph(runs) => {
                ui.add(egui::Label::new(job(runs, t.text_dim)).wrap());
                ui.add_space(4.0);
            }
            Block::Bullet(runs) => {
                ui.horizontal_top(|ui| {
                    ui.label(w::text("•", 14.0, t.text_faint));
                    ui.add(egui::Label::new(job(runs, t.text_dim)).wrap());
                });
            }
            Block::Image(url) => {
                ui.add_space(6.0);
                let max_w = width.min(860.0);
                let sized = crate::sized_images::inline_url(url, max_w, 1200.0, ui.ctx().pixels_per_point());
                ui.add(egui::Image::new(sized).max_width(max_w).corner_radius(t.corner_small()));
                ui.add_space(6.0);
            }
            Block::Video(id) => {
                let Some(id) = crate::links::valid_youtube_id(id) else { continue };
                ui.add_space(6.0);
                let w_ = width.min(640.0);
                let (r, resp) = ui.allocate_exact_size(vec2(w_, w_ * 0.5625), Sense::click());
                w::image_or_skeleton(ui, t, r, &format!("https://img.youtube.com/vi/{id}/hqdefault.jpg"), t.corner_small());
                ui.painter().rect_filled(r, t.corner_small(), Color32::from_black_alpha(60));
                ui.painter().circle_filled(r.center(), 30.0, Color32::from_rgb(0xE5, 0x2D, 0x27));
                ui.painter().text(r.center() + vec2(2.0, 0.0), Align2::CENTER_CENTER, icons::PLAY, w::font(26.0), Color32::WHITE);
                if resp.on_hover_cursor(egui::CursorIcon::PointingHand).on_hover_text(t!("Watch on YouTube")).clicked() {
                    crate::links::open_youtube(id);
                }
                ui.add_space(6.0);
            }
        }
    }
}

impl Store {
    /// "Is this game relevant to you?" — what Steam users check before buying.
    fn relevance(&mut self, ui: &mut Ui, t: &Theme, ctx: &mut Ctx, id: AppId, d: &Details) {
        let info = self.browse_info(id);
        let local = ctx.local.games.iter().find(|g| g.app_id == id);
        let owned = ctx.state.apps.as_ref().is_some_and(|a| a.contains_key(&id)) || local.is_some();
        let top_seller = matches!(self.front_data(), Some(Ok(front)) if front.1.iter().any(|s| s.id == "top_sellers" && s.items.iter().any(|i| i.app_id == id)));
        let playing = ctx.state.friends_playing(id);

        w::card(t).show(ui, |ui| {
            ui.set_width(ui.available_width());
            ui.label(w::text_semibold(t!("Is this game relevant to you?"), 15.0, t.text));
            ui.add_space(6.0);
            let row = |ui: &mut Ui, icon: &str, color: Color32, text: String| {
                ui.horizontal_wrapped(|ui| {
                    ui.label(w::text(icon, 15.0, color));
                    ui.label(w::text(text, 13.5, t.text_dim));
                });
            };
            if let Some(m) = local.and_then(|g| g.playtime_minutes).filter(|m| *m > 0) {
                row(ui, icons::INFO, t.away, t!("You've played {time}", time = ui::playtime(m)));
            } else if owned {
                row(ui, icons::CHECK, t.play, t!("It's already in your library").to_string());
            }
            if let Some(r) = info.as_ref().and_then(|i| i.reviews.as_ref()) {
                let (icon, color) = if r.score >= 7 { (icons::CHECK, t.play) } else if r.score <= 4 { (icons::X, t.danger) } else { (icons::INFO, t.away) };
                row(ui, icon, color, t!("User reviews: {label}", label = i18n::tr_dyn(&r.label)));
            }
            if top_seller {
                row(ui, icons::CHECK, t.play, t!("In the Top Sellers").to_string());
            }
            if let Some(m) = d.metacritic.filter(|m| *m >= 75) {
                row(ui, icons::CHECK, t.play, t!("Metacritic score: {score}", score = m));
            }
            if !playing.is_empty() {
                row(ui, icons::USERS, t.in_game, t!("Friends playing right now: {n}", n = playing.len()));
                ui.horizontal_wrapped(|ui| {
                    for p in playing.iter().take(8) {
                        w::avatar(ui, t, &model::art::avatar(p.avatar_hash.as_deref()), 30.0, Some(t.in_game)).on_hover_text(&p.name);
                    }
                });
            }
            if d.coming_soon {
                row(ui, icons::CALENDAR, t.accent, t!("Not released yet").to_string());
            }
        });
    }
}

impl Store {
    /// Reviews, right under the purchase box like on Steam: the score, and
    /// big buttons to read what players say or write your own.
    fn reviews_card(&mut self, ui: &mut Ui, t: &Theme, id: AppId, name: &str, owned: bool) {
        let info = self.browse_info(id);
        let tint = info.as_ref().and_then(|i| i.reviews.as_ref()).map(|r| review_color(t, r.score)).unwrap_or(t.accent);
        egui::Frame::new()
            .fill(w::mix(t.surface, tint, 0.10))
            .stroke(egui::Stroke::new(1.0, w::mix(t.border, tint, 0.45)))
            .corner_radius(t.corner())
            .inner_margin(Margin::symmetric(20, 16))
            .show(ui, |ui| {
                ui.set_width(ui.available_width());
                ui.horizontal(|ui| {
                    ui.label(w::text(icons::CHATS, 26.0, tint));
                    ui.add_space(6.0);
                    ui.vertical(|ui| {
                        ui.label(w::text_bold(t!("Player reviews"), 17.0, t.text));
                        match info.as_ref().and_then(|i| i.reviews.clone()) {
                            Some(r) => {
                                ui.horizontal(|ui| {
                                    ui.label(w::text_semibold(i18n::tr_dyn(&r.label), 14.5, tint));
                                    ui.label(w::text(t!("({percent}% of {count})", percent = r.percent_positive, count = thousands(r.count)), 13.0, t.text_dim));
                                });
                            }
                            None => {
                                ui.label(w::text(t!("See what players think of {name}.", name = name), 13.0, t.text_dim));
                            }
                        }
                    });
                    ui.with_layout(Layout::right_to_left(Align::Center), |ui| {
                        if owned && w::button(ui, t, Some(icons::NOTE), t!("Write a review"), w::Kind::Play, w::Size::Lg).clicked() {
                            self.reviews = Some(super::reviews::ReviewWindow::new(id, name, owned, true));
                        }
                        if w::button(ui, t, Some(icons::THUMBS_UP), t!("Read reviews"), w::Kind::Primary, w::Size::Lg).clicked() {
                            self.reviews = Some(super::reviews::ReviewWindow::new(id, name, owned, false));
                        }
                    });
                });
                ui.add_space(10.0);
                ui.horizontal_wrapped(|ui| {
                    ui.spacing_mut().item_spacing = vec2(6.0, 6.0);
                    ui.label(w::text(t!("Browse by:"), 12.5, t.text_dim));
                    let presets = [
                        (t!("Most helpful"), model::ReviewFilter::default()),
                        (t!("Most recent"), model::ReviewFilter { order: model::ReviewOrder::Recent, ..Default::default() }),
                        (t!("Funniest"), model::ReviewFilter { order: model::ReviewOrder::Funny, ..Default::default() }),
                        (t!("Positive|review"), model::ReviewFilter { kind: model::ReviewKind::Positive, ..Default::default() }),
                        (t!("Negative|review"), model::ReviewFilter { kind: model::ReviewKind::Negative, ..Default::default() }),
                    ];
                    for (label, filter) in presets {
                        if w::chip(ui, t, label, false).clicked() {
                            self.reviews = Some(super::reviews::ReviewWindow::with_filter(id, name, owned, false, filter));
                        }
                    }
                });
            });
    }
}

fn review_color(t: &Theme, score: u32) -> Color32 {
    match score {
        7..=9 => Color32::from_rgb(0x66, 0xC0, 0xF4),
        5..=6 => Color32::from_rgb(0xB9, 0xA0, 0x74),
        _ => t.danger,
    }
}

/// Icon for a store "feature" category (Co-op, Trading Cards, ...).
fn feature_icon(name: &str) -> &'static str {
    let n = name.to_lowercase();
    if n.contains("single") || n.contains("одиноч") {
        icons::USER
    } else if n.contains("multi") || n.contains("co-op") || n.contains("pvp") || n.contains("мульти") || n.contains("кооп") {
        icons::USERS
    } else if n.contains("achiev") || n.contains("достиж") {
        icons::TROPHY
    } else if n.contains("card") || n.contains("карточ") {
        icons::TAG
    } else if n.contains("cloud") || n.contains("облак") {
        icons::DOWNLOAD
    } else if n.contains("controller") || n.contains("контрол") {
        icons::CONTROLLER
    } else if n.contains("workshop") || n.contains("мастерск") {
        icons::SLIDERS
    } else if n.contains("anti-cheat") || n.contains("vac") || n.contains("античит") {
        icons::SHIELD
    } else if n.contains("purchase") || n.contains("покуп") {
        icons::CART
    } else {
        icons::CHECK
    }
}

const DESCRIPTORS: &[(u32, &str)] = &[
    (1, "Some nudity or sexual content"),
    (2, "Frequent violence or gore"),
    (3, "Adult-only sexual content"),
    (4, "Frequent nudity or sexual content"),
    (5, "General mature content"),
];

impl Store {
    /// True if the app's page should be gated by the user's content settings.
    fn details_mature(&mut self, id: AppId) -> bool {
        match self.safe(id) {
            Some(ok) => !ok,
            // Until we know, use the details call if it has already answered.
            None => self.details(id).is_some_and(|d| d.content_descriptors.iter().any(|x| self.hidden.contains(x))),
        }
    }

    /// Steam-style mature content notice instead of the page.
    fn mature_gate(&mut self, ui: &mut Ui, t: &Theme, id: AppId) {
        let name = self.details(id).map(|d| d.name.clone()).unwrap_or_default();
        let notes = self.details(id).and_then(|d| d.content_notes.clone());
        ui.add_space(80.0);
        w::centered(ui, 560.0, 32.0, |ui| {
            w::card(t).show(ui, |ui| {
                ui.set_width(ui.available_width());
                ui.label(w::text(icons::WARNING, 30.0, t.away));
                ui.label(w::text_bold(&name, 22.0, t.text));
                ui.add_space(4.0);
                ui.add(egui::Label::new(w::text(t!("This game contains content you chose to hide in your store preferences."), 14.0, t.text_dim)).wrap());
                if let Some(n) = notes {
                    ui.add_space(6.0);
                    ui.add(egui::Label::new(w::text(t!("Developer's description: {notes}", notes = n), 12.5, t.text_faint)).wrap());
                }
                ui.add_space(14.0);
                ui.horizontal(|ui| {
                    if w::button(ui, t, Some(icons::ARROW_LEFT), t!("Back to the store"), w::Kind::Primary, w::Size::Md).clicked() {
                        self.open(View::Home);
                    }
                    if w::button(ui, t, None, t!("View page"), w::Kind::Secondary, w::Size::Md).clicked() {
                        self.allowed.insert(id);
                    }
                });
            });
        });
    }
}

/// The store link for a view, as `on_navigate` reads it.
fn view_arg(view: &View) -> Option<String> {
    match view {
        View::Home => None,
        View::Section(id) => Some(format!("section:{id}")),
        View::Search(q) => Some(format!("search:{q}")),
        View::Creator(kind, name) => Some(format!("{kind}:{name}")),
        View::App(id) => Some(format!("app:{id}")),
        View::Browse => Some("browse".into()),
        View::Discovery(0) => Some("discovery".into()),
        View::Discovery(pos) => Some(format!("discovery/{pos}")),
        View::Points => Some("points".into()),
        View::Wishlist => Some("wishlist".into()),
        View::Cart => Some("cart".into()),
    }
}

/// A store card from a store query result.
pub(super) fn query_card(q: store::query::QueryItem) -> Item {
    Item { app_id: q.app_id, name: q.name, header: q.header, large: q.capsule, price: q.price, windows: q.platforms.0 }
}

#[cfg(test)]
mod tests {
    use super::{View, view_arg};

    #[test]
    fn encodes_discovery_routes() {
        assert_eq!(view_arg(&View::Discovery(0)).as_deref(), Some("discovery"));
        assert_eq!(view_arg(&View::Discovery(4)).as_deref(), Some("discovery/4"));
        assert_eq!(view_arg(&View::Browse).as_deref(), Some("browse"));
    }
}

/// For Big Picture: trailers, review summaries and a trailer player.
impl Store {
    pub(crate) fn trailers_for(&mut self, id: AppId) -> Vec<store::browse::Trailer> {
        self.browse_info(id).map(|b| b.trailers.clone()).unwrap_or_default()
    }

    pub(crate) fn ratings_for(&mut self, id: AppId) -> Option<(u32, u64)> {
        self.browse_info(id).and_then(|b| b.reviews.as_ref().map(|r| (r.percent_positive, r.count)))
    }

    /// Play trailer `i` of a game in `rect` (started if it isn't the one
    /// playing). True when it ended.
    pub(crate) fn play_trailer(&mut self, ui: &mut Ui, t: &Theme, rect: Rect, id: AppId, i: usize) -> bool {
        let trailers = self.trailers_for(id);
        let Some(tr) = trailers.get(i) else { return false };
        let Some(src) = tr.hls.clone().map(video::Source::Hls).or_else(|| tr.micro_mp4.clone().map(video::Source::Mp4)) else {
            w::image_or_skeleton(ui, t, rect, &tr.thumbnail, t.corner());
            return false;
        };
        // The registry's idle cleanup also runs while Big Picture draws this player.
        self.drawn_frame = self.frames;
        if self.trailer.as_ref().is_none_or(|(url, _)| url != src.url()) {
            let url = src.url().to_string();
            let mut p = video::Player::new(&self.rt, ui.ctx(), src, video::widget::options(ui.ctx()));
            p.play();
            self.trailer = Some((url, p));
        }
        let (_, p) = self.trailer.as_mut().expect("just set");
        p.fullscreen = false;
        video::widget::show(ui, t, rect, p, &tr.thumbnail, &tr.name).ended
    }

    /// Pause/play, or move by `seconds`, the trailer playing.
    pub(crate) fn trailer_control(&mut self, toggle: bool, seconds: f64) {
        if let Some((_, p)) = self.trailer.as_mut() {
            if toggle {
                p.toggle();
            }
            if seconds != 0.0 {
                let to = (p.position() + seconds).clamp(0.0, p.duration().max(0.0));
                p.seek(to);
            }
        }
    }

    pub(crate) fn stop_trailer(&mut self) {
        self.trailer = None;
    }
}
