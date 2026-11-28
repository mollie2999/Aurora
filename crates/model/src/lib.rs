//! Plain data types shared by every crate. No I/O lives here, so extensions
//! can depend on it without pulling in networking.

use serde::{Deserialize, Serialize};
use std::path::PathBuf;

pub type AppId = u32;
pub type SteamId = u64;
pub type GroupChatId = u64;
pub type ChatRoomId = u64;

/// Convert a 32-bit Steam account ID into a full 64-bit individual SteamID
/// (universe 1 = Public, type 1 = Individual, instance 1 = Desktop).
pub const fn account_to_steam_id(account_id: u32) -> SteamId {
    76561197960265728 + account_id as u64
}

/// Extract the 32-bit account ID from a 64-bit SteamID.
pub const fn steam_id_to_account(steam_id: SteamId) -> u32 {
    (steam_id & 0xFFFF_FFFF) as u32
}

/// A game installed in one of the local Steam library folders.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct InstalledGame {
    pub app_id: AppId,
    pub name: String,
    pub install_dir: PathBuf,
    pub library: PathBuf,
    pub size_on_disk: u64,
    /// Unix seconds.
    pub last_updated: Option<u64>,
    /// Bytes downloaded by the last update (from Steam's appmanifest).
    #[serde(default)]
    pub last_update_bytes: Option<u64>,
    /// Unix seconds, from the account's localconfig.
    pub last_played: Option<u64>,
    /// Total minutes played, from the account's localconfig.
    pub playtime_minutes: Option<u64>,
    pub state: InstallState,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub enum InstallState {
    Installed,
    UpdateRequired,
    Downloading,
    Unknown,
}

impl InstallState {
    /// Decode the `StateFlags` bitfield from an app manifest.
    pub fn from_flags(flags: u64) -> Self {
        const FULLY_INSTALLED: u64 = 4;
        const UPDATE_REQUIRED: u64 = 2;
        const DOWNLOADING: u64 = 1 << 20;
        if flags & DOWNLOADING != 0 {
            Self::Downloading
        } else if flags & UPDATE_REQUIRED != 0 {
            Self::UpdateRequired
        } else if flags & FULLY_INSTALLED != 0 {
            Self::Installed
        } else {
            Self::Unknown
        }
    }

    pub fn label(self) -> &'static str {
        match self {
            Self::Installed => "Installed",
            Self::UpdateRequired => "Update required",
            Self::Downloading => "Downloading",
            Self::Unknown => "Unknown state",
        }
    }
}

/// A Steam account that has logged in on this machine (from loginusers.vdf).
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct LocalAccount {
    pub steam_id: SteamId,
    pub account_name: String,
    pub persona_name: String,
    pub most_recent: bool,
    pub last_login: Option<u64>,
}

/// 32-bit account id used for `userdata/<id>` folders.
pub fn account_id(steam_id: SteamId) -> u32 {
    (steam_id & 0xFFFF_FFFF) as u32
}

/// An app the logged-in account owns, resolved through PICS.
#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub struct OwnedApp {
    pub app_id: AppId,
    pub name: String,
    pub kind: String,
    pub launch: Vec<LaunchOption>,
    /// Icon hash for [`art::icon`].
    #[serde(default)]
    pub icon: Option<String>,
    /// For DLC: the game it belongs to.
    #[serde(default)]
    pub parent: Option<AppId>,
    /// Folder name under `steamapps/common`.
    #[serde(default)]
    pub install_dir: String,
    #[serde(default)]
    pub depots: Vec<DepotInfo>,
    #[serde(default)]
    pub branches: Vec<Branch>,
    /// Steam Cloud settings, when the game uses it.
    #[serde(default)]
    pub cloud: Option<CloudConfig>,
    /// Has store art (a store page). Steam leaves DLC without it, like
    /// Dota 2's "Vulkan support", off the game's library page.
    #[serde(default)]
    pub store_assets: bool,
    /// Art paths from app info in Steam's newer layout, e.g.
    /// "<hash>/header.jpg" (see [`art::remember`]).
    #[serde(default)]
    pub art: Vec<String>,
    /// Steam's feature categories (1 multi-player, 2 single-player, 9 co-op,
    /// 22 achievements, 23 cloud, 28 full controller support, 30 workshop…).
    #[serde(default)]
    pub categories: Vec<u32>,
    /// Steam genre ids (1 Action, 2 Strategy, 3 RPG, 23 Indie…).
    #[serde(default)]
    pub genres: Vec<u32>,
    /// Steam Deck: 0 unknown, 1 unsupported, 2 playable, 3 verified.
    #[serde(default)]
    pub deck: u8,
    /// The install script Steam runs on first launch, relative to the
    /// install folder (app info `extended/installscript`), when declared.
    #[serde(default)]
    pub install_script: Option<String>,
}

/// A game's Steam Cloud setup, from its app info (`ufs`).
#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub struct CloudConfig {
    /// Bytes allowed in the cloud.
    pub quota: u64,
    pub max_files: u32,
    /// Auto-Cloud: folders the game saves into by itself.
    pub save_files: Vec<SaveRule>,
}

/// One Auto-Cloud rule: files matching `pattern` under `root`/`path`.
#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub struct SaveRule {
    /// Steam's root name, e.g. "WinMyDocuments" or "gameinstall".
    pub root: String,
    pub path: String,
    pub pattern: String,
    pub recursive: bool,
}

/// What a Steam Cloud sync did, or found.
#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub struct CloudReport {
    pub app_id: AppId,
    pub downloaded: u32,
    pub uploaded: u32,
    pub up_to_date: u32,
    /// Files changed both here and in the cloud since the last sync.
    pub conflicts: Vec<CloudConflict>,
    /// Files that couldn't be synced, with the reason.
    pub errors: Vec<(String, String)>,
    pub cloud_files: u32,
    pub cloud_bytes: u64,
    pub quota_bytes: u64,
    /// Unix seconds.
    pub finished: u64,
    /// Every file in the cloud, for the file list in Properties.
    #[serde(default)]
    pub files: Vec<CloudFile>,
}

/// One file in a game's Steam Cloud.
#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub struct CloudFile {
    /// The name Steam stores it under (what a delete needs).
    pub raw: String,
    /// The same name, cleaned up for showing.
    pub name: String,
    pub size: u64,
    /// Unix seconds.
    pub time: u64,
}

#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub struct CloudConflict {
    /// The file's cloud name, e.g. "%WinMyDocuments%My Games/X/save1.sav".
    pub name: String,
    pub local_time: u64,
    pub cloud_time: u64,
    pub local_size: u64,
    pub cloud_size: u64,
}

/// One depot (content package) of an app, from PICS.
#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub struct DepotInfo {
    pub id: u32,
    /// "windows", "macos", "linux" or empty for all.
    pub os: String,
    /// "64", "32" or empty.
    pub arch: String,
    /// Empty = all languages.
    pub language: String,
    pub low_violence: bool,
    pub dlc_app: Option<AppId>,
    /// Shared depot owned by another app (e.g. redistributables).
    pub shared_install: bool,
    /// (branch, manifest gid, size in bytes)
    pub manifests: Vec<(String, u64, u64)>,
}

impl DepotInfo {
    pub fn manifest(&self, branch: &str) -> Option<(u64, u64)> {
        self.manifests.iter().find(|m| m.0 == branch).or_else(|| self.manifests.iter().find(|m| m.0 == "public")).map(|m| (m.1, m.2))
    }
}

/// A version branch (Steam's "Betas").
#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub struct Branch {
    pub name: String,
    pub description: String,
    pub build_id: u64,
    /// Unix seconds.
    pub updated: u64,
    pub password: bool,
}

/// A Steam Workshop item, as listed in the Workshop browser.
#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub struct WorkshopEntry {
    pub id: u64,
    pub title: String,
    pub preview: Option<String>,
    pub description: String,
    pub size: u64,
    pub subscriptions: u64,
    pub favorited: u64,
    /// Unix seconds.
    pub updated: u64,
    pub votes_up: u32,
    pub votes_down: u32,
    pub tags: Vec<String>,
}

/// A game's own Workshop tag group, e.g. "Addon Type" or "Heroes".
#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub struct WorkshopTagCategory {
    pub name: String,
    /// Steam shows it as a dropdown: pick one.
    pub single: bool,
    /// (tag sent to Steam, name to show)
    pub tags: Vec<(String, String)>,
}

/// One line in the shopping cart.
#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub struct CartLine {
    pub line_item_id: u64,
    pub package_id: u32,
    /// Formatted price as Steam shows it.
    pub price: String,
    /// Friend's account id when bought as a gift.
    pub gift_to: Option<u32>,
    /// A gift (Steam's "This is a gift"), with or without a recipient yet.
    #[serde(default)]
    pub is_gift: bool,
}

/// Live state of one download, as shown on the Downloads page.
#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub struct DownloadStatus {
    pub app_id: AppId,
    pub phase: DownloadPhase,
    pub downloaded: u64,
    pub download_total: u64,
    pub disk_done: u64,
    pub disk_total: u64,
    /// Bytes per second over the last few seconds.
    pub speed: u64,
    pub paused: bool,
    pub error: Option<String>,
    /// Bytes saved by reusing unchanged chunks.
    pub reused: u64,
    /// Not errors, but worth showing (e.g. skipped content).
    #[serde(default)]
    pub notes: Vec<String>,
    /// Where the game is going (exists as soon as files start arriving).
    #[serde(default)]
    pub install_dir: Option<PathBuf>,
    /// Bytes per second landing on disk (downloaded, reused or copied).
    #[serde(default)]
    pub disk_speed: u64,
}

impl DownloadStatus {
    /// (done, total) for the progress bar, like Steam: the bytes this
    /// download fetches (an update shows only the update, e.g. 0 / 6 GB).
    /// While existing files are checked or verified, the check's progress.
    pub fn shown(&self) -> (u64, u64) {
        if self.phase == DownloadPhase::Verifying || self.download_total == 0 {
            (self.disk_done.min(self.disk_total), self.disk_total)
        } else {
            (self.downloaded.min(self.download_total), self.download_total)
        }
    }

    pub fn fraction(&self) -> f32 {
        let (done, total) = self.shown();
        (done as f32 / total.max(1) as f32).clamp(0.0, 1.0)
    }

    pub fn verifying(&self) -> bool {
        self.phase == DownloadPhase::Verifying
    }

    /// What it's doing right now, in words (an English key for translation):
    /// downloading, writing to disk, checking files…, like Steam's status.
    pub fn activity(&self) -> &'static str {
        match self.phase {
            DownloadPhase::Done => "Done",
            DownloadPhase::Failed => "Failed",
            // Paused shows as paused whatever step it stopped at.
            _ if self.paused => "Paused",
            DownloadPhase::Queued | DownloadPhase::Preparing => "Preparing…",
            DownloadPhase::Checking => "Checking existing files…",
            DownloadPhase::Verifying => "Verifying files…",
            DownloadPhase::Finishing => "Finishing…",
            // More than ~50 KB/s over the network: downloading. Otherwise
            // data still landing on disk (reused chunks, buffered writes).
            _ if self.speed > 50_000 => "Downloading",
            _ if self.disk_speed > 0 => "Writing to disk",
            _ => "Waiting for content servers…",
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Default, Serialize, Deserialize)]
pub enum DownloadPhase {
    #[default]
    Queued,
    Preparing,
    Checking,
    Downloading,
    Finishing,
    /// Checking an installed game's files (Verify integrity).
    Verifying,
    Done,
    Failed,
}

impl OwnedApp {
    pub fn is_dlc(&self) -> bool {
        self.kind == "dlc"
    }
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct LaunchOption {
    pub executable: String,
    pub arguments: String,
    pub working_dir: String,
    pub description: String,
    /// Empty means any OS.
    pub os_list: String,
}

impl LaunchOption {
    pub fn runs_on_this_os(&self) -> bool {
        let os = if cfg!(windows) {
            "windows"
        } else if cfg!(target_os = "macos") {
            "macos"
        } else {
            "linux"
        };
        self.os_list.is_empty() || self.os_list.split(',').any(|o| o.trim() == os)
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Default, Serialize, Deserialize)]
pub enum PersonaState {
    #[default]
    Offline,
    Online,
    Busy,
    Away,
    Snooze,
    LookingToTrade,
    LookingToPlay,
    Invisible,
}

impl PersonaState {
    pub fn from_u32(v: u32) -> Self {
        match v {
            1 => Self::Online,
            2 => Self::Busy,
            3 => Self::Away,
            4 => Self::Snooze,
            5 => Self::LookingToTrade,
            6 => Self::LookingToPlay,
            7 => Self::Invisible,
            _ => Self::Offline,
        }
    }
    pub fn as_u32(self) -> u32 {
        self as u32
    }
    pub fn label(self) -> &'static str {
        match self {
            Self::Offline => "Offline",
            Self::Online => "Online",
            Self::Busy => "Busy",
            Self::Away => "Away",
            Self::Snooze => "Snooze",
            Self::LookingToTrade => "Looking to trade",
            Self::LookingToPlay => "Looking to play",
            Self::Invisible => "Invisible",
        }
    }
    /// States a user can pick for themselves.
    pub const SELECTABLE: [PersonaState; 4] =
        [Self::Online, Self::Away, Self::Busy, Self::Invisible];
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub enum Relationship {
    None,
    Blocked,
    RequestRecipient,
    Friend,
    RequestInitiator,
    Ignored,
    Other(u32),
}

impl Relationship {
    pub fn from_u32(v: u32) -> Self {
        match v {
            0 => Self::None,
            1 => Self::Blocked,
            2 => Self::RequestRecipient,
            3 => Self::Friend,
            4 => Self::RequestInitiator,
            5 => Self::Ignored,
            o => Self::Other(o),
        }
    }
}

/// Everything known about one user's public presence.
#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub struct Persona {
    pub steam_id: SteamId,
    pub name: String,
    pub state: PersonaState,
    pub game_app_id: Option<AppId>,
    pub game_name: Option<String>,
    /// The game's own status line, when it's plain text (e.g. "In a match").
    #[serde(default)]
    pub rich_status: Option<String>,
    /// Hex avatar hash; see [`art::avatar`].
    pub avatar_hash: Option<String>,
    pub last_logoff: Option<u64>,
    /// The game server they're on ("ip:port"), when the game reports one.
    #[serde(default)]
    pub server: Option<String>,
    /// The game lobby they're in.
    #[serde(default)]
    pub lobby: Option<u64>,
    /// How to join them: the game's own launch arguments (rich presence "connect").
    #[serde(default)]
    pub connect: Option<String>,
    /// They're broadcasting their game on Steam.
    #[serde(default)]
    pub broadcasting: bool,
    /// Your nickname for them (Steam syncs these).
    #[serde(default)]
    pub nickname: Option<String>,
}

impl Persona {
    /// Your nickname for them, else their name, else their SteamID.
    pub fn display_name(&self) -> String {
        match (&self.nickname, self.name.is_empty()) {
            (Some(n), _) if !n.is_empty() => n.clone(),
            (_, false) => self.name.clone(),
            _ => self.steam_id.to_string(),
        }
    }

    /// Launch arguments that join them in their game, if Steam says how.
    pub fn join_args(&self) -> Option<String> {
        if let Some(tokens) = self.connect.as_deref().and_then(|c| safe_connect_tokens(c).ok()).filter(|t| !t.is_empty()) {
            return Some(tokens.join(" "));
        }
        if let Some(s) = self.server.as_deref().map(str::trim).filter(|s| is_safe_host_port(s)) {
            return Some(format!("+connect {s}"));
        }
        self.lobby.map(|l| format!("+connect_lobby {l}"))
    }

    pub fn in_game(&self) -> bool {
        self.game_app_id.is_some() || self.game_name.is_some()
    }
}

/// Safe `host` / `host:port` / `[ipv6]:port` token for `+connect`.
pub fn is_safe_host_port(s: &str) -> bool {
    !s.is_empty()
        && s.len() <= 255
        && !s.starts_with(['-', '+'])
        && s.bytes().all(|b| b.is_ascii_alphanumeric() || matches!(b, b'.' | b'_' | b':' | b'[' | b']' | b'-'))
}

fn is_u64_digits(s: &str) -> bool {
    !s.is_empty() && s.bytes().all(|b| b.is_ascii_digit()) && s.parse::<u64>().is_ok()
}

fn is_safe_connect_password(s: &str) -> bool {
    !s.is_empty()
        && s.len() <= 128
        && !s.starts_with(['-', '+'])
        && !s.contains("%command%")
        && !s.chars().any(|c| c.is_control() || c.is_whitespace() || matches!(c, '"' | '\'' | ';' | '\\'))
}

/// Validate an untrusted remote join/invite `connect` string into argv tokens.
/// Empty or whitespace-only strings return `Ok(Vec::new())`.
pub fn safe_connect_tokens(connect: &str) -> Result<Vec<String>, &'static str> {
    if connect.chars().any(char::is_control) {
        return Err("unsafe control characters in connect arguments");
    }
    let trimmed = connect.trim();
    if trimmed.is_empty() {
        return Ok(Vec::new());
    }
    let tokens: Vec<&str> = trimmed.split_whitespace().collect();
    match tokens.as_slice() {
        ["+connect_lobby", id] if is_u64_digits(id) => Ok(vec!["+connect_lobby".into(), (*id).into()]),
        ["+invite", id] if is_u64_digits(id) => Ok(vec!["+invite".into(), (*id).into()]),
        ["+connect", addr] if is_safe_host_port(addr) => Ok(vec!["+connect".into(), (*addr).into()]),
        ["+connect", addr, "+password", pw] if is_safe_host_port(addr) && is_safe_connect_password(pw) => {
            Ok(vec!["+connect".into(), (*addr).into(), "+password".into(), (*pw).into()])
        }
        _ => Err("unsafe connect arguments"),
    }
}

/// Public profile details and equipped cosmetics.
#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub struct ProfileInfo {
    pub steam_id: SteamId,
    /// Full URL of the profile background image.
    pub background: Option<String>,
    /// Full URL of the avatar frame (PNG).
    pub avatar_frame: Option<String>,
    pub real_name: String,
    pub summary: String,
    pub location: String,
    /// Unix seconds.
    pub member_since: Option<u64>,
    /// Who can see this profile's games, inventory and friends.
    pub visibility: Visibility,
    /// VAC bans on record (from the public profile).
    #[serde(default)]
    pub vac_banned: bool,
    /// Whether the background fills the window (Steam's "Full Screen")
    /// rather than showing at its own size.
    #[serde(default)]
    pub background_full: bool,
    /// An animated avatar from the Points Shop.
    #[serde(default)]
    pub animated_avatar: Option<String>,
    /// The mini profile's background image.
    #[serde(default)]
    pub mini_background: Option<String>,
    /// The profile's Steam theme ("Summer", "DarkMode"…; "" for the default).
    #[serde(default)]
    pub theme: String,
}

/// Steam community visibility of a profile.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default, Serialize, Deserialize)]
pub enum Visibility {
    /// Not loaded yet.
    #[default]
    Unknown,
    Private,
    FriendsOnly,
    Public,
}

/// One item (or stack) in a Steam inventory.
#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub struct InventoryItem {
    pub asset_id: u64,
    pub class_id: u64,
    pub amount: u64,
    pub name: String,
    /// The item's type line, e.g. "Mil-Spec Grade Rifle" or "Trading Card".
    pub kind: String,
    /// Hex colors without '#', as Steam sends them.
    pub name_color: Option<String>,
    pub background: Option<String>,
    /// Image hash; see [`art::econ`].
    pub icon: String,
    pub icon_large: Option<String>,
    pub tradable: bool,
    pub marketable: bool,
    pub market_hash_name: String,
    /// Description lines (may contain simple HTML) and their color.
    pub description: Vec<(String, Option<String>)>,
    /// (category, tag, color)
    pub tags: Vec<(String, String, Option<String>)>,
    /// Links Steam attaches, e.g. "Inspect in Game..." (name, url).
    pub actions: Vec<(String, String)>,
    pub context_id: u64,
    /// Notes only the owner sees, e.g. "Tradable After Nov 14, 2026".
    pub owner_notes: Vec<String>,
    /// Days of trade / market hold after getting the item (0 = none).
    pub trade_hold_days: u32,
    pub market_hold_days: u32,
    /// Interchangeable items (cards, cases) that the market sells by count.
    pub commodity: bool,
    /// The game's own market fee (e.g. 0.10); Steam's is separate.
    pub market_fee: Option<f64>,
    /// A trading card: its game, and whether it's foil (for its card set).
    #[serde(default)]
    pub card_set: Option<(AppId, bool)>,
}

/// What an item sells for on the Community Market, in the wallet currency.
#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub struct MarketPrice {
    /// Cheapest listing, as Steam formats it.
    pub lowest: Option<String>,
    pub median: Option<String>,
    /// Sold in the last 24 hours.
    pub volume: Option<u64>,
    /// In cents (what the buyer pays). From the order book, when loaded.
    pub lowest_sell: Option<u64>,
    pub highest_buy: Option<u64>,
    pub sell_count: Option<u64>,
    pub buy_count: Option<u64>,
    /// Fee rules for your wallet currency, when Steam sent them.
    pub fee_rules: Option<fees::Rules>,
    /// Steam currency code the cent values are in.
    pub currency: i32,
    /// Cheapest listings and highest buy orders: (price in cents, count).
    pub sell_ladder: Vec<(u64, u64)>,
    pub buy_ladder: Vec<(u64, u64)>,
    /// Daily median sale price: (unix time, price in currency units, sold).
    pub history: Vec<(u64, f64, u32)>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Default, Serialize, Deserialize)]
pub enum ListingStatus {
    #[default]
    Active,
    /// Waiting for confirmation in the Steam Mobile app (or email).
    NeedsConfirmation,
    OnHold,
}

/// One of your Community Market listings.
#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub struct MyListing {
    pub listing_id: u64,
    pub app_id: AppId,
    pub name: String,
    pub game: String,
    pub icon: String,
    pub name_color: Option<String>,
    /// Cents the buyer pays and you receive.
    pub buyer_pays: u64,
    pub you_receive: u64,
    pub currency: i32,
    pub amount: u64,
    /// Unix seconds.
    pub created: u64,
    pub status: ListingStatus,
    pub market_hash_name: String,
}

/// One of your buy orders.
#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub struct BuyOrder {
    pub order_id: u64,
    pub app_id: AppId,
    pub name: String,
    pub icon: String,
    pub price: u64,
    pub currency: i32,
    pub quantity: u64,
    pub remaining: u64,
    pub market_hash_name: String,
}

/// A Steam notification (the bell): gifts, items, comments, trade offers...
#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub struct Notification {
    pub id: u64,
    /// Steam's ESteamNotificationType (2 gift, 3 comment, 4 item, 5 friend
    /// invite, 8 wishlist, 9 trade offer, 11 support reply, ...).
    pub kind: i32,
    /// Unix seconds.
    pub time: u64,
    pub read: bool,
    /// Details Steam attached, when present.
    pub app_id: Option<AppId>,
    pub from: Option<SteamId>,
    pub text: Option<String>,
    /// New items: where the item is, and the game that dropped it.
    pub context_id: Option<u64>,
    pub asset_id: Option<u64>,
    pub source_app: Option<AppId>,
    /// Support replies: the request's reference code.
    pub ticket: Option<String>,
    /// The raw JSON Steam sent, for plugins and future types.
    pub data: String,
}

#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub struct Notifications {
    pub items: Vec<Notification>,
    pub unread: u32,
    pub pending_gifts: u32,
    pub pending_friends: u32,
    /// Things waiting for confirmation in the Steam Mobile app.
    pub confirmations: u32,
}

/// Who can see a part of your profile: 1 only you, 2 friends, 3 everyone.
/// Comments: 0 friends, 1 everyone, 2 nobody.
/// Adapted from upstream SteamInRust PR #74 by madonchik123.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default, Serialize, Deserialize)]
pub struct ProfilePrivacy {
    pub profile: u8,
    pub inventory: u8,
    pub gifts: u8,
    pub games: u8,
    pub playtime: u8,
    pub friends: u8,
    pub comments: u8,
}

/// A profile item you own (background, frame...).
/// Adapted from upstream SteamInRust PR #74 / #101 by madonchik123.
#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize)]
pub struct ProfileItemChoice {
    pub id: u64,
    pub name: String,
    pub image: String,
    pub app_id: AppId,
    /// The full-size picture (backgrounds), when there's one.
    #[serde(default)]
    pub large: String,
    /// Its video (MP4) for animated items.
    #[serde(default)]
    pub movie: String,
    /// The game (or Steam collection) it comes from.
    #[serde(default)]
    pub app_name: String,
}

/// Your profile as Steam's Edit Profile pages show it.
/// Adapted from upstream SteamInRust PR #74 / #101 by madonchik123.
#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize)]
pub struct ProfileEdit {
    pub persona_name: String,
    pub real_name: String,
    pub custom_url: String,
    pub summary: String,
    /// Location codes as Steam stores them (country "KZ", state, city id).
    pub country: String,
    pub state: String,
    pub city: String,
    /// "City, State, Country" as Steam names them.
    pub location_text: String,
    pub privacy: ProfilePrivacy,
    pub hide_awards: bool,
    pub theme: String,
    /// (theme id, title)
    pub themes: Vec<(String, String)>,
    pub backgrounds: Vec<ProfileItemChoice>,
    pub mini_backgrounds: Vec<ProfileItemChoice>,
    pub frames: Vec<ProfileItemChoice>,
    pub animated_avatars: Vec<ProfileItemChoice>,
    /// Equipped item ids: background, mini profile background, frame,
    /// animated avatar (0 = none).
    pub equipped: [u64; 4],
    /// The equipped background fills the window (Steam's "Full Screen").
    #[serde(default)]
    pub background_full: bool,
    /// The featured badge (a community badge's id; 0 = none).
    #[serde(default)]
    pub favorite_badge: u32,
}

/// Which part of the profile a save was for.
/// Adapted from upstream SteamInRust PR #74 / #101 by madonchik123.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub enum ProfileSave {
    Info,
    Privacy,
    Avatar,
    Item,
    BackgroundFlags,
    Theme,
    Badge,
}

/// Profile comments shown per page.
pub const COMMENTS_PER_PAGE: u32 = 6;

/// What Steam's profile page shows besides the showcases: the right column
/// (level, featured badge, status, badges, counts, groups, top friends) and
/// recent activity. Texts are in the language the page was asked in.
#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub struct ProfilePage {
    pub level: Option<u32>,
    pub featured_badge: Option<PageBadge>,
    /// "online", "in-game" or "offline".
    pub status: String,
    /// e.g. "Currently Offline".
    pub status_header: String,
    /// e.g. the game, or "Last Online 5 days ago".
    pub status_detail: String,
    /// The few badges Steam shows (newest first).
    pub badges: Vec<PageBadge>,
    /// The count links ("Badges 32", "Games 124"…), in Steam's order.
    pub counts: Vec<PageCount>,
    pub groups: Vec<PageGroup>,
    /// Steam's top friends (by level).
    pub friends: Vec<PageFriend>,
    /// e.g. "47.8 hours past 2 weeks".
    pub recent_playtime: String,
    pub recent: Vec<RecentGame>,
    /// The profile theme as the page's body class has it, without "Theme":
    /// "Summer", "Cosmic", "GameProfile"… ("" = Steam's default).
    #[serde(default)]
    pub theme: String,
    /// A game profile's own colors (`--gradient-right` → value), from the
    /// page's `body.GameProfileTheme { … }` rule.
    #[serde(default)]
    pub theme_vars: Vec<(String, String)>,
    /// The country flag next to the location (Steam's `countryflags/xx.gif`).
    #[serde(default)]
    pub country_flag: String,
    /// Steam's "You have <a>1 group</a> in common" (HTML, in the page's
    /// language) under Groups, and the same for friends; "" when none.
    #[serde(default)]
    pub groups_in_common: String,
    #[serde(default)]
    pub common_groups: Option<Result<Vec<UserGroup>, String>>,
    #[serde(default)]
    pub friends_in_common: String,
    #[serde(default)]
    pub mutual_friends: Option<Result<Vec<PageFriend>, String>>,
    /// The "Join Game" link under an in-game status (e.g.
    /// `steam://joinlobby/381210/<lobby>/<owner>`); "" when there's none.
    #[serde(default)]
    pub join_link: String,
    /// Completionist showcases as the page draws them, in page order.
    #[serde(default)]
    pub completionist: Vec<PageCompletionist>,
    /// Workshop showcases as the page draws them, in page order.
    #[serde(default)]
    pub workshop: Vec<PageWorkshop>,
    #[serde(default)]
    pub awards: Vec<PageAwards>,
    #[serde(default)]
    pub collectors: Vec<PageCollector>,
    #[serde(default)]
    pub guides: Vec<PageGuide>,
    #[serde(default)]
    pub showcase_items: Vec<(u64, String)>,
}

#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub struct PageGuide {
    pub id: u64,
    pub image: String,
    pub title: String,
    pub creator: String,
    pub app: String,
    pub app_icon: String,
    pub rating_icon: String,
    pub ratings: String,
    pub description: String,
}

#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub struct PageCollector {
    pub stats: Vec<PageStat>,
    pub games: Vec<AppId>,
}

#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub struct PageAwards {
    pub items: Vec<(String, String, String)>,
    pub stats: Vec<PageStat>,
}

/// Steam's Completionist Showcase: game headers with their achievement
/// counts, and the stats row under them.
#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize)]
pub struct PageCompletionist {
    pub games: Vec<PageCompletedGame>,
    /// ("10", "Completed Games"), ("38%", "Avg. Game Completion Rate")…
    pub stats: Vec<PageStat>,
}

#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize)]
pub struct PageCompletedGame {
    pub app_id: AppId,
    /// The game's header picture (460×215).
    pub image: String,
    /// e.g. "49 Achievements".
    pub achievements: String,
    /// e.g. "10 RARE"; "" when none.
    pub rare: String,
    /// The profile's achievements page for this game.
    pub url: String,
}

#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize)]
pub struct PageStat {
    pub value: String,
    pub label: String,
    /// Where the stat links to; "" when it doesn't.
    pub url: String,
    pub tooltip: String,
}

/// Steam's Workshop Showcase: the owner's workshop header, their newest
/// items and the stats row.
#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize)]
pub struct PageWorkshop {
    /// e.g. "Another Duck's Workshop".
    pub title: String,
    pub url: String,
    pub avatar: Option<String>,
    pub items: Vec<PageWorkshopItem>,
    pub stats: Vec<PageStat>,
}

#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize)]
pub struct PageWorkshopItem {
    pub id: u64,
    pub app_id: AppId,
    /// The item's preview at its own shape.
    pub image: String,
}

#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize)]
pub struct PageBadge {
    pub name: String,
    /// e.g. "Level 3 Necesse Badge" or "100 XP".
    pub detail: String,
    pub icon: String,
    pub url: String,
}

#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize)]
pub struct PageCount {
    /// Which list: "badges", "games", "inventory", "screenshots", "videos",
    /// "workshop", "reviews", "guides", "artwork", "groups", "friends"…
    pub key: String,
    pub label: String,
    pub count: Option<u32>,
    pub url: String,
}

#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize)]
pub struct PageGroup {
    pub name: String,
    pub url: String,
    pub avatar: Option<String>,
    /// e.g. "2 Members".
    pub members: String,
}

#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize)]
pub struct PageFriend {
    pub steam_id: SteamId,
    pub name: String,
    pub avatar: Option<String>,
    /// "online", "in-game" or "offline".
    pub status: String,
    /// e.g. ["In-Game", "Counter-Strike 2"] or ["Last Online 6 days ago"].
    pub lines: Vec<String>,
    pub level: u32,
}

/// A game in a profile's Recent Activity.
#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize)]
pub struct RecentGame {
    pub app_id: AppId,
    pub name: String,
    pub capsule: Option<String>,
    /// e.g. ["1,234 hrs on record", "last played on 25 Nov"].
    pub details: Vec<String>,
    /// e.g. "203 / 318 Achievements" ("" for games without them).
    pub achievements: String,
    /// e.g. "93 Rare".
    pub rare: String,
    /// Unlocked share, 0..1.
    pub progress: f32,
    /// (name, icon)
    pub icons: Vec<(String, String)>,
    /// e.g. "+198".
    pub more: String,
    /// e.g. "Screenshots 5".
    pub screenshots: String,
    /// The game's badge this profile earned, if any (e.g. "Clawing", "200 XP").
    #[serde(default)]
    pub badge: Option<PageBadge>,
    /// e.g. "Review 1", if this profile reviewed the game.
    #[serde(default)]
    pub review: String,
    #[serde(default)]
    pub review_url: String,
}

/// A name a profile used before, and when it changed (Steam's wording).
#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize)]
pub struct Alias {
    pub name: String,
    pub changed: String,
}

/// A showcase on a Steam profile, in the order its owner arranged them.
#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub struct Showcase {
    /// Steam's `EProfileCustomizationType`; see [`showcase`].
    pub kind: i32,
    /// Steam's `EProfileCustomizationStyle` (layout variant).
    pub style: i32,
    pub slots: Vec<ShowcaseSlot>,
    /// The Favorite Group showcase's group.
    pub group: Option<ShowcaseGroup>,
    /// Achievement progress for the games this showcase shows.
    pub games: Vec<ShowcaseGame>,
}

/// One filled slot of a showcase.
#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub struct ShowcaseSlot {
    pub app_id: AppId,
    /// A screenshot, artwork, workshop item or guide.
    pub file: Option<ShowcaseFile>,
    /// An inventory item: asset and context id (in `app_id`'s inventory).
    pub asset_id: u64,
    pub context_id: u64,
    /// Custom text box title, or an achievement's API name.
    pub title: String,
    /// Custom text box body (BBCode).
    pub notes: String,
    pub account_id: u32,
    pub badge_id: u32,
    pub border_color: u32,
    /// The achievement this slot features (achievement showcases).
    pub achievement: Option<ShowcaseAchievement>,
}

#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub struct ShowcaseFile {
    pub id: u64,
    pub title: String,
    /// Picture to show (preview, or the screenshot itself).
    pub image: Option<String>,
    pub app_id: AppId,
}

#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub struct ShowcaseAchievement {
    pub name: String,
    pub description: String,
    pub icon: String,
    /// Share of players who have it, 0-100.
    pub percent: f32,
}

#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub struct ShowcaseGroup {
    pub name: String,
    #[serde(default)]
    pub headline: String,
    pub url: String,
    pub avatar: Option<String>,
    pub members: u32,
    pub in_game: u32,
    pub online: u32,
    pub in_chat: u32,
}

/// A game's achievements as a profile shows them.
#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub struct ShowcaseGame {
    pub app_id: AppId,
    pub unlocked: u32,
    pub total: u32,
    /// Unlocked achievements fewer than 10% of players have.
    pub rare: u32,
    /// Unlocked achievements, rarest first.
    pub recent: Vec<ShowcaseAchievement>,
}

/// Steam's `EProfileCustomizationType` values.
pub mod showcase {
    pub const RARE_ACHIEVEMENTS: i32 = 1;
    pub const GAME_COLLECTOR: i32 = 2;
    pub const ITEMS: i32 = 3;
    pub const TRADE: i32 = 4;
    pub const BADGES: i32 = 5;
    pub const FAVORITE_GAME: i32 = 6;
    pub const SCREENSHOTS: i32 = 7;
    pub const CUSTOM_TEXT: i32 = 8;
    pub const FAVORITE_GROUP: i32 = 9;
    pub const REVIEW: i32 = 10;
    pub const WORKSHOP: i32 = 11;
    pub const MY_WORKSHOP: i32 = 12;
    pub const ARTWORK: i32 = 13;
    pub const VIDEO: i32 = 14;
    pub const GUIDES: i32 = 15;
    pub const MY_GUIDES: i32 = 16;
    pub const ACHIEVEMENTS: i32 = 17;
    pub const GREENLIGHT: i32 = 18;
    pub const MY_GREENLIGHT: i32 = 19;
    pub const SALIEN: i32 = 20;
    pub const REACTIONS: i32 = 21;
    pub const SINGLE_ARTWORK: i32 = 22;
    pub const COMPLETIONIST: i32 = 23;
    pub const REPLAY: i32 = 24;
}

/// A comment on a profile.
#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub struct Comment {
    pub id: u64,
    pub author: SteamId,
    pub name: String,
    pub avatar: Option<String>,
    /// Unix seconds.
    pub time: u64,
    /// The comment as Steam's HTML.
    pub html: String,
}

#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub struct Comments {
    pub items: Vec<Comment>,
    pub total: u32,
}

/// A review on a profile (its Reviews page).
#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize)]
pub struct UserReview {
    pub app_id: AppId,
    /// The game's small capsule image.
    pub capsule: Option<String>,
    pub recommended: bool,
    /// e.g. "1,083.3 hrs on record (223.8 hrs at review time)".
    pub hours: String,
    /// e.g. "Posted 24 March, 2017."
    pub posted: String,
    pub text: String,
    /// e.g. "63 people found this review helpful".
    pub helpful: String,
    pub url: String,
}

/// One player's review of a game, from Steam's review feed.
#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize)]
pub struct AppReview {
    pub id: String,
    pub author: SteamId,
    pub minutes_played: u64,
    pub minutes_at_review: u64,
    pub recommended: bool,
    pub text: String,
    /// Unix seconds.
    pub created: u64,
    pub helpful: u32,
    pub funny: u32,
    pub early_access: bool,
    /// The player got the game for free.
    pub free: bool,
}

/// A game's reviews: the summary and one or more pages of reviews.
#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize)]
pub struct AppReviews {
    /// e.g. "Very Positive".
    pub label: String,
    pub positive: u32,
    pub negative: u32,
    pub total: u32,
    pub items: Vec<AppReview>,
    /// Where the next page starts; `None` at the end.
    pub cursor: Option<String>,
}

/// Which review polarity to request from Steam's review feed.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, Hash, Serialize, Deserialize)]
pub enum ReviewKind {
    #[default]
    All,
    Positive,
    Negative,
}

/// Sort order for Steam's review feed.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, Hash, Serialize, Deserialize)]
pub enum ReviewOrder {
    #[default]
    Helpful,
    Recent,
    Funny,
}

/// Time window used when sorting by [`ReviewOrder::Helpful`].
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, Hash, Serialize, Deserialize)]
pub enum ReviewWindow {
    AllTime,
    Day,
    Week,
    #[default]
    Month,
    ThreeMonths,
    SixMonths,
    Year,
}

/// Filters for [`AppReviews`] fetched via `IUserReviewsService/GetAppReviews`.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
pub struct ReviewFilter {
    pub kind: ReviewKind,
    pub order: ReviewOrder,
    pub window: ReviewWindow,
    pub all_languages: bool,
}

impl Default for ReviewFilter {
    fn default() -> Self {
        Self { kind: ReviewKind::All, order: ReviewOrder::Helpful, window: ReviewWindow::Month, all_languages: false }
    }
}

/// The store's picks for you, from Steam's personal sources.
#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize)]
pub struct StoreRecommendations {
    /// Discovery queues by shelf id ("ml", "on_sale", "free", "coming_soon").
    pub queues: Vec<(String, Vec<AppId>)>,
    /// Games your friends play a lot lately: (game, how many friends).
    pub friends: Vec<(AppId, u32)>,
    /// Your favourite tags, best first: (tag id, name).
    pub tags: Vec<(u32, String)>,
}

/// One page (10) of a profile's reviews.
#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize)]
pub struct UserReviews {
    pub items: Vec<UserReview>,
    /// All reviews on the profile.
    pub total: u32,
}

/// A Steam group a profile belongs to.
#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize)]
pub struct UserGroup {
    pub name: String,
    pub url: String,
    pub avatar: Option<String>,
    /// e.g. "14 Members".
    pub members: String,
    /// e.g. "4 Online".
    pub online: String,
}

/// A game's trading-card collection on a profile (its badge's page).
#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize)]
pub struct CardSet {
    /// e.g. "The Witcher 2 Badge".
    pub title: String,
    /// The badge reached so far (`None` before the first craft).
    pub badge: Option<CardBadge>,
    /// e.g. "29 hrs on record".
    pub playtime: String,
    /// e.g. "No card drops remaining" (only on your own profile).
    pub drops: String,
    pub cards: Vec<Card>,
}

#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize)]
pub struct CardBadge {
    pub name: String,
    pub icon: Option<String>,
    /// e.g. "Level 1, 100 XP".
    pub level: String,
    pub unlocked: String,
}

/// One card of a set.
#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize)]
pub struct Card {
    pub name: String,
    pub image: String,
    /// How many the profile has (0 = missing).
    pub count: u32,
    /// e.g. "1 of 8, Series 1".
    pub series: String,
}

/// A badge on a profile.
#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub struct Badge {
    pub title: String,
    pub icon: Option<String>,
    /// e.g. "340 XP" or "Level 2, 200 XP".
    pub detail: String,
    /// e.g. "Unlocked Oct 14 @ 10:06am".
    pub unlocked: String,
    pub url: String,
}

#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub struct Badges {
    pub level: u32,
    pub xp: String,
    /// e.g. "85 XP to reach Level 19".
    pub next: String,
    /// 0..1 progress to the next level.
    pub progress: f32,
    pub items: Vec<Badge>,
}

/// A library collection from the official Steam client (synced by Steam).
#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub struct SteamCollection {
    /// "favorite", "hidden", or Steam's id for one you made ("uc-…").
    pub id: String,
    pub name: String,
    pub games: Vec<AppId>,
    /// Built from filters in Steam (its games aren't listed).
    pub dynamic: bool,
}

/// A Steam download region with the download speed (and connection time)
/// measured from this PC.
#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub struct DownloadRegion {
    /// 0 for a partner CDN (pick it by `host`).
    pub cell_id: u32,
    pub name: String,
    pub code: String,
    pub host: String,
    /// Server load, 0-100.
    pub load: u32,
    /// Best TCP connect time; `None` if unreachable.
    pub ping_ms: Option<u32>,
    /// Measured download speed in KB/s (one real game chunk); 0 if that
    /// download failed, `None` when it wasn't measured.
    #[serde(default)]
    pub speed_kbps: Option<u32>,
}

impl DownloadRegion {
    /// Download speed, 0 slow .. 3 fastest: measured when possible, else
    /// guessed from ping and load.
    pub fn speed_tier(&self) -> u8 {
        if let Some(kbps) = self.speed_kbps {
            return match kbps {
                k if k >= 30_000 => 3,
                k if k >= 10_000 => 2,
                k if k >= 3_000 => 1,
                _ => 0,
            };
        }
        let base: u8 = match self.ping_ms {
            Some(p) if p < 30 => 3,
            Some(p) if p < 80 => 2,
            Some(p) if p < 160 => 1,
            _ => 0,
        };
        if self.load > 85 { base.saturating_sub(1) } else { base }
    }
}

/// One of your Steam Support requests.
#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub struct SupportTicket {
    pub id: String,
    pub title: String,
    /// What it's about, usually a game's name.
    pub area: String,
    pub icon: Option<String>,
    pub status: String,
    pub open: bool,
    /// As Steam shows it, e.g. "Oct 7".
    pub updated: String,
    pub url: String,
}

/// One message in a support request.
#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub struct SupportMessage {
    pub from_steam: bool,
    /// "Message from Steam Support on Oct 9 @ 2:43pm | 4 weeks ago"
    pub header: String,
    /// The message body as Steam's HTML.
    pub html: String,
}

/// Your account's security details (Steam's "Account details" page).
#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub struct AccountSecurity {
    pub steam_guard: bool,
    /// Unix time Steam Guard was turned on.
    pub steam_guard_since: Option<u64>,
    /// The Steam Mobile authenticator.
    pub authenticator: bool,
    pub authenticator_since: Option<u64>,
    pub phone_verified: bool,
    pub password_changed: Option<u64>,
    pub email_changed: Option<u64>,
    pub password_reset: Option<u64>,
    /// Computers that Steam Guard remembers for this account.
    pub devices: Vec<GuardDevice>,
}

#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub struct GuardDevice {
    pub name: String,
    /// Unix time it was authorized.
    pub since: Option<u64>,
    pub ip: String,
}

/// One row of your purchase history (store purchases, gifts, wallet and
/// Market transactions).
#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub struct Purchase {
    /// As Steam writes it ("8 Nov, 2026").
    pub date: String,
    pub items: Vec<String>,
    /// "Purchase", "Gift Purchase", "Refund", "Market Transaction", ...
    pub kind: String,
    /// Payment method under the type, when shown.
    pub payment: String,
    pub total: String,
    pub wallet_change: String,
    pub wallet_balance: String,
    /// Refunded or otherwise struck through.
    pub refunded: bool,
}

/// A page of purchase history; `more` continues it.
#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub struct PurchasePage {
    pub rows: Vec<Purchase>,
    /// Opaque cursor for the next page (None at the end).
    pub more: Option<String>,
}

/// A license on your account (Steam's "Licenses and product key activations").
#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub struct License {
    pub date: String,
    pub name: String,
    /// "Steam Store", "Retail", "Complimentary", "Gift/Guest Pass", ...
    pub method: String,
    /// Package id, when Steam shows a remove link for it.
    pub package_id: Option<u32>,
}

/// Your Steam Family.
#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub struct Family {
    pub id: u64,
    pub name: String,
    pub members: Vec<FamilyMember>,
    /// People invited but not in yet: (steam id, role).
    pub invites: Vec<(SteamId, FamilyRole)>,
    pub free_spots: u32,
    pub country: String,
    /// Seconds until a new member can join after someone left.
    pub slot_cooldown: u32,
    /// Games shared in the family, with who owns them.
    pub shared: Vec<SharedApp>,
    /// Whether Steam returned its shared library successfully.
    #[serde(default)]
    pub shared_available: bool,
    /// DLC owned in the family, with the game each belongs to.
    #[serde(default)]
    pub shared_dlc: Vec<SharedDlc>,
    /// Whether the DLC list includes all entries returned by Steam.
    #[serde(default)]
    pub shared_dlc_complete: bool,
    /// Your preferred copy per game: (app, lender).
    #[serde(default)]
    pub preferred: Vec<(AppId, SteamId)>,
}

impl Family {
    fn shareable_app(&self, app_id: AppId) -> Option<&SharedApp> {
        self.shared.iter().find(|a| a.app_id == app_id && !a.excluded)
    }

    /// The copy Steam has saved as preferred, if it is still shareable.
    pub fn preferred_copy(&self, app_id: AppId) -> Option<SteamId> {
        let app = self.shareable_app(app_id)?;
        self.preferred
            .iter()
            .find(|(a, owner)| *a == app_id && app.owners.contains(owner))
            .map(|(_, owner)| *owner)
    }

    /// The saved preference, or the sole owner when there is no choice.
    pub fn active_copy(&self, app_id: AppId) -> Option<SteamId> {
        self.preferred_copy(app_id).or_else(|| {
            let [owner] = self.shareable_app(app_id)?.owners.as_slice() else { return None };
            Some(*owner)
        })
    }

    /// Record an accepted change without reloading unrelated family metadata.
    pub fn set_preferred_copy(&mut self, app_id: AppId, owner: SteamId) {
        self.preferred.retain(|(id, _)| *id != app_id);
        self.preferred.push((app_id, owner));
    }

    /// Every selectable copy of the game in the family, yours first.
    pub fn copies(&self, app_id: AppId, me: SteamId) -> Vec<SteamId> {
        let Some(app) = self.shareable_app(app_id) else { return Vec::new() };
        let mut v = app.owners.clone();
        v.sort_by_key(|o| *o != me);
        v
    }

    /// DLC for `app_id` that `owner` has, sorted by name.
    pub fn dlc_of(&self, app_id: AppId, owner: SteamId) -> Vec<&SharedDlc> {
        let mut v: Vec<&SharedDlc> = self.shared_dlc.iter().filter(|d| d.parent == app_id && d.owners.contains(&owner)).collect();
        v.sort_by_key(|d| d.name.to_lowercase());
        v
    }
}

/// A DLC owned by one or more family members.
#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub struct SharedDlc {
    pub app_id: AppId,
    pub parent: AppId,
    pub name: String,
    pub owners: Vec<SteamId>,
}

#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, Serialize, Deserialize)]
pub enum FamilyRole {
    Adult,
    Child,
    #[default]
    Unknown,
}

impl FamilyRole {
    /// Steam's `EFamilyGroupRole`: 1 adult, 2 child.
    pub fn from_steam(v: i32) -> Self {
        match v {
            1 => FamilyRole::Adult,
            2 => FamilyRole::Child,
            _ => FamilyRole::Unknown,
        }
    }

    pub fn to_steam(self) -> i32 {
        match self {
            FamilyRole::Adult => 1,
            FamilyRole::Child => 2,
            FamilyRole::Unknown => 0,
        }
    }
}

#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub struct FamilyMember {
    pub steam_id: SteamId,
    pub role: FamilyRole,
    pub joined: u64,
    /// Seconds until they can join another family after leaving.
    pub cooldown: u32,
}

#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub struct SharedApp {
    pub app_id: AppId,
    pub name: String,
    pub owners: Vec<SteamId>,
    /// Can't be shared (Steam's exclude reason is not 0).
    pub excluded: bool,
}

/// An incoming invitation to join another user's Steam Family.
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct FamilyInvite {
    pub family_id: u64,
    pub inviter: SteamId,
    pub role: FamilyRole,
    pub awaiting_2fa: bool,
}

/// Outcome of a Steam Family management action.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub enum FamilyActionDone {
    Created,
    Renamed,
    Invited { needs_2fa: bool },
    InviteCancelled,
    InviteResent,
    Joined { needs_2fa: bool },
    Removed,
    Left,
    Deleted,
}

/// Whether you're in a family, and which.
#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub struct FamilyStatus {
    pub family: Option<Family>,
    /// Your own role in it.
    pub role: FamilyRole,
    /// Seconds before you can join a family again.
    pub cooldown: u32,
    /// Families that invited you.
    pub invites: Vec<FamilyInvite>,
}

#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub struct MyListings {
    pub listings: Vec<MyListing>,
    pub buy_orders: Vec<BuyOrder>,
}

/// Community Market fee math, matching Steam's own: a Steam fee (5%) plus the
/// game's fee (usually 10%), each rounded down and at least the wallet
/// currency's minimum fee (1 cent in USD, more in some currencies).
pub mod fees {
    use serde::{Deserialize, Serialize};

    pub const DEFAULT_GAME: f64 = 0.10;

    /// Steam's fee rules for one wallet currency (`g_rgWalletInfo`).
    #[derive(Debug, Clone, Copy, PartialEq, Serialize, Deserialize)]
    pub struct Rules {
        pub minimum: u64,
        pub percent: f64,
        pub base: u64,
        pub game_default: f64,
    }

    impl Default for Rules {
        fn default() -> Self {
            Self { minimum: 1, percent: 0.05, base: 0, game_default: DEFAULT_GAME }
        }
    }

    /// (Steam fee, game fee) when you receive `receive`.
    pub fn split(receive: u64, game_fee: f64, r: &Rules) -> (u64, u64) {
        let steam = (receive as f64 * r.percent).floor().max(r.minimum as f64) as u64 + r.base;
        let game = if game_fee > 0.0 { (receive as f64 * game_fee).floor().max(r.minimum as f64) as u64 } else { 0 };
        (steam, game)
    }

    /// What the buyer pays when you ask to receive `receive`.
    pub fn buyer_pays(receive: u64, game_fee: f64, r: &Rules) -> u64 {
        if receive == 0 {
            return 0;
        }
        let (a, b) = split(receive, game_fee, r);
        receive + a + b
    }

    /// The most you receive while the buyer pays at most `pays`.
    pub fn you_receive(pays: u64, game_fee: f64, r: &Rules) -> u64 {
        if pays <= buyer_pays(1, game_fee, r) {
            return if pays == buyer_pays(1, game_fee, r) { 1 } else { 0 };
        }
        let mut x = ((pays.saturating_sub(r.base)) as f64 / (1.0 + r.percent + game_fee)).floor() as u64;
        while x > 0 && buyer_pays(x, game_fee, r) > pays {
            x -= 1;
        }
        while buyer_pays(x + 1, game_fee, r) <= pays {
            x += 1;
        }
        x
    }

    #[cfg(test)]
    mod tests {
        use super::*;
        #[test]
        fn matches_steam_usd() {
            let r = Rules::default();
            assert_eq!(buyer_pays(1, DEFAULT_GAME, &r), 3);
            assert_eq!(you_receive(3, DEFAULT_GAME, &r), 1);
            assert_eq!(buyer_pays(100, DEFAULT_GAME, &r), 115);
            assert_eq!(you_receive(115, DEFAULT_GAME, &r), 100);
            for pays in 3..5000 {
                assert!(buyer_pays(you_receive(pays, DEFAULT_GAME, &r), DEFAULT_GAME, &r) <= pays);
            }
        }
        #[test]
        fn matches_steam_with_a_currency_minimum() {
            // Real tenge listings: receive 5.00 -> buyer pays 13.00; 21.00 -> 29.00.
            let r = Rules { minimum: 400, ..Rules::default() };
            assert_eq!(buyer_pays(500, 0.1, &r), 1300);
            assert_eq!(buyer_pays(2100, 0.1, &r), 2900);
            assert_eq!(you_receive(2900, 0.1, &r), 2100);
        }
    }
}

/// "$1,234.56", "1 234,56€", "¥ 120" -> cents (for currencies with cents).
pub fn parse_money(s: &str) -> Option<u64> {
    let digits: String = s.chars().filter(|c| c.is_ascii_digit() || *c == '.' || *c == ',').collect();
    let digits = digits.trim_matches(|c| c == '.' || c == ',');
    if digits.is_empty() {
        return None;
    }
    // A separator followed by exactly two digits at the end is the decimal point.
    let bytes = digits.as_bytes();
    let n = bytes.len();
    let has_cents = n > 3 && (bytes[n - 3] == b'.' || bytes[n - 3] == b',');
    let whole: String = digits.chars().filter(|c| c.is_ascii_digit()).collect();
    let v: u64 = whole.parse().ok()?;
    Some(if has_cents { v } else { v * 100 })
}

/// A game that has an inventory on someone's account, as Steam lists it.
#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub struct InventoryApp {
    pub app_id: AppId,
    pub name: String,
    pub icon: Option<String>,
    /// Sections of the inventory: (context id, name, item count).
    pub contexts: Vec<(u64, String, u32)>,
}

/// One game's inventory for one account.
#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub struct Inventory {
    pub steam_id: SteamId,
    pub app_id: AppId,
    pub context_id: u64,
    pub items: Vec<InventoryItem>,
    pub total: u32,
    /// Set when the inventory couldn't be read ("private" for hidden ones).
    pub error: Option<String>,
}

/// An item in a trade offer.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct TradeAsset {
    pub app_id: AppId,
    pub context_id: u64,
    pub asset_id: u64,
    pub amount: u64,
}

/// A trade offer Steam accepted.
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct TradeOfferSent {
    pub id: u64,
    /// Steam waits for you to confirm it in the Steam Mobile app (or email).
    pub needs_confirmation: bool,
}

/// A trade URL (`…/tradeoffer/new/?partner=<account>&token=<token>`):
/// the partner's SteamID and the token that lets non-friends send offers.
pub fn parse_trade_url(url: &str) -> Option<(SteamId, Option<String>)> {
    let query = url.trim().split_once('?')?.1;
    let mut partner = None;
    let mut token = None;
    for pair in query.split('&') {
        match pair.split_once('=') {
            Some(("partner", v)) => partner = v.parse::<u64>().ok(),
            Some(("token", v)) if !v.is_empty() => token = Some(v.to_string()),
            _ => {}
        }
    }
    let account = partner?;
    let steam_id = if account > u32::MAX as u64 { account } else { 76561197960265728 + account };
    Some((steam_id, token))
}

/// One of a game's achievements, with whether you have it.
#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub struct Achievement {
    pub name: String,
    pub description: String,
    pub icon: String,
    pub icon_gray: String,
    pub hidden: bool,
    pub unlocked: bool,
    /// Unix seconds, 0 if locked.
    pub unlock_time: u64,
    /// Share of players who have it, 0..100.
    pub global_percent: f32,
}

/// A game on someone's profile, with their playtime.
#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub struct PlayerGame {
    pub app_id: AppId,
    pub name: String,
    pub playtime_minutes: u64,
    pub playtime_2weeks: u64,
    pub last_played: Option<u64>,
    pub icon: Option<String>,
}

/// Steam currency code -> (ISO code, symbol, symbol goes after the number).
pub fn currency(code: i32) -> (&'static str, &'static str, bool) {
    match code {
        1 => ("USD", "$", false),
        2 => ("GBP", "£", false),
        3 => ("EUR", "€", true),
        4 => ("CHF", "CHF ", false),
        5 => ("RUB", " ₽", true),
        6 => ("PLN", "zł", true),
        7 => ("BRL", "R$ ", false),
        8 => ("JPY", "¥ ", false),
        17 => ("TRY", "₺", false),
        18 => ("UAH", "₴", true),
        20 => ("CAD", "CDN$ ", false),
        21 => ("AUD", "A$ ", false),
        23 => ("CNY", "¥ ", false),
        24 => ("INR", "₹ ", false),
        37 => ("KZT", "₸", true),
        _ => ("", "", false),
    }
}

/// A country whose store prices are in this Steam currency (lowercase
/// two-letter code), so the store can price like your wallet.
pub fn currency_country(code: i32) -> Option<&'static str> {
    Some(match code {
        1 => "us",
        2 => "gb",
        3 => "de",
        4 => "ch",
        5 => "ru",
        6 => "pl",
        7 => "br",
        8 => "jp",
        9 => "no",
        10 => "id",
        11 => "my",
        12 => "ph",
        13 => "sg",
        14 => "th",
        15 => "vn",
        16 => "kr",
        17 => "tr",
        18 => "ua",
        19 => "mx",
        20 => "ca",
        21 => "au",
        22 => "nz",
        23 => "cn",
        24 => "in",
        25 => "cl",
        26 => "pe",
        27 => "co",
        28 => "za",
        29 => "hk",
        30 => "tw",
        31 => "sa",
        32 => "ae",
        33 => "se",
        34 => "ar",
        35 => "il",
        36 => "by",
        37 => "kz",
        38 => "kw",
        39 => "qa",
        40 => "cr",
        41 => "uy",
        42 => "bg",
        43 => "hr",
        44 => "cz",
        45 => "dk",
        46 => "hu",
        47 => "ro",
        _ => return None,
    })
}

/// The `cc` to load the store with: your wallet's country, like Steam's own
/// client, so a VPN doesn't change the currency. The IP country (from
/// sign-in) is only a fallback for accounts without a wallet. `None` = not
/// known yet.
pub fn store_country(wallet_currency: Option<i32>, ip_country: &str) -> Option<String> {
    let wallet = wallet_currency.and_then(currency_country).map(str::to_string);
    wallet.or_else(|| Some(ip_country.to_lowercase()).filter(|c| !c.is_empty()))
}

#[cfg(test)]
mod store_country_tests {
    use super::*;

    #[test]
    fn wallet_currency_picks_the_country() {
        assert_eq!(currency_country(37), Some("kz"));
        assert_eq!(currency_country(5), Some("ru"));
        assert_eq!(currency_country(1), Some("us"));
        assert_eq!(currency_country(18), Some("ua"));
        assert_eq!(currency_country(3), Some("de"));
        assert_eq!(currency_country(0), None);
        assert_eq!(currency_country(999), None);
    }

    #[test]
    fn every_known_currency_has_a_country() {
        for code in 1..=47 {
            if !currency(code).0.is_empty() {
                assert!(currency_country(code).is_some(), "currency {code}");
            }
        }
    }

    #[test]
    fn wallet_beats_the_vpn_country() {
        // KZT wallet, signed in through a Dutch VPN.
        assert_eq!(store_country(Some(37), "NL").as_deref(), Some("kz"));
        // No wallet: the IP country.
        assert_eq!(store_country(None, "NL").as_deref(), Some("nl"));
        // Unknown currency code: the IP country.
        assert_eq!(store_country(Some(0), "DE").as_deref(), Some("de"));
        // Nothing yet: wait.
        assert_eq!(store_country(None, ""), None);
    }
}

pub fn format_money(cents: i64, code: i32) -> String {
    let (iso, _, _) = currency(code);
    format_price(cents, iso)
}

/// A price the way Steam's store writes it in that currency: "$19.99",
/// "19,99€", "4 480₸", "1 999 ₽", "R$ 19,99", "¥ 1,980"…
pub fn format_price(cents: i64, iso: &str) -> String {
    // (before, after, decimal mark, thousands mark, whole numbers only)
    let (pre, post, dec, th, whole) = match iso {
        "USD" | "" => ("$", "", '.', ",", false),
        "EUR" => ("", "€", ',', " ", false),
        "GBP" => ("£", "", '.', ",", false),
        "RUB" => ("", " ₽", ',', " ", true),
        "KZT" => ("", "₸", ',', " ", true),
        "UAH" => ("", "₴", ',', " ", true),
        "PLN" => ("", "zł", ',', " ", false),
        "BRL" => ("R$ ", "", ',', ".", false),
        "TRY" => ("₺", "", ',', ".", false),
        "JPY" => ("¥ ", "", '.', ",", true),
        "CNY" => ("¥ ", "", '.', ",", false),
        "KRW" => ("₩ ", "", '.', ",", true),
        "INR" => ("₹ ", "", '.', ",", true),
        "CAD" => ("CDN$ ", "", '.', ",", false),
        "AUD" => ("A$ ", "", '.', ",", false),
        "NZD" => ("NZ$ ", "", '.', ",", false),
        "CHF" => ("CHF ", "", '.', "'", false),
        "NOK" => ("", " kr", ',', " ", false),
        "MXN" => ("Mex$ ", "", '.', ",", false),
        "IDR" => ("Rp ", "", ',', " ", true),
        "VND" => ("", "₫", ',', ".", true),
        "SGD" => ("S$", "", '.', ",", false),
        "HKD" => ("HK$ ", "", '.', ",", false),
        "ILS" => ("₪", "", '.', ",", false),
        "ZAR" => ("R ", "", '.', " ", false),
        other => return format!("{:.2} {other}", cents as f64 / 100.0),
    };
    let negative = cents < 0;
    let cents = cents.unsigned_abs();
    let (units, minor) = (cents / 100, cents % 100);
    // Group the whole part in threes.
    let digits = units.to_string();
    let mut grouped = String::new();
    for (i, ch) in digits.chars().enumerate() {
        if i > 0 && (digits.len() - i) % 3 == 0 {
            grouped.push_str(th);
        }
        grouped.push(ch);
    }
    // Whole-number currencies still show kopecks/tiyn if a price has them.
    let frac = if whole && minor == 0 { String::new() } else { format!("{dec}{minor:02}") };
    format!("{}{pre}{grouped}{frac}{post}", if negative { "-" } else { "" })
}

#[cfg(test)]
mod family_tests {
    use super::*;

    fn family() -> Family {
        let dlc = |app_id, name: &str, owners: Vec<SteamId>| SharedDlc { app_id, parent: 10, name: name.into(), owners };
        Family {
            shared: vec![SharedApp { app_id: 10, name: "Game".into(), owners: vec![1, 2, 3], excluded: false }],
            shared_dlc: vec![dlc(12, "b pack", vec![2]), dlc(11, "A pack", vec![2, 3]), dlc(13, "Other", vec![1])],
            ..Default::default()
        }
    }

    #[test]
    fn saved_copy_must_still_be_owned_and_shareable() {
        let mut f = family();
        f.preferred = vec![(10, 7), (10, 3)];
        assert_eq!(f.preferred_copy(10), Some(3));
        assert_eq!(f.active_copy(10), Some(3));

        f.set_preferred_copy(10, 2);
        assert_eq!(f.preferred, [(10, 2)]);
        assert_eq!(f.preferred_copy(10), Some(2));

        f.shared[0].owners = vec![3];
        assert_eq!(f.preferred_copy(10), None);
        assert_eq!(f.active_copy(10), Some(3));
        assert_eq!(f.active_copy(99), None);
    }

    #[test]
    fn several_owners_do_not_imply_an_active_or_preferred_copy() {
        let f = family();
        assert_eq!(f.active_copy(10), None);
        assert_eq!(f.preferred_copy(10), None);
    }

    #[test]
    fn excluded_games_offer_no_shared_copies() {
        let mut f = family();
        f.shared[0].excluded = true;
        f.preferred = vec![(10, 2)];
        assert_eq!(f.preferred_copy(10), None);
        assert_eq!(f.active_copy(10), None);
        assert!(f.copies(10, 2).is_empty());
    }

    #[test]
    fn copies_list_yours_first() {
        let f = family();
        assert_eq!(f.copies(10, 2), [2, 1, 3]);
        assert_eq!(f.copies(10, 9), [1, 2, 3]);
        assert!(f.copies(99, 2).is_empty());
    }

    #[test]
    fn dlc_of_filters_by_owner_and_sorts_by_name() {
        let f = family();
        let names: Vec<&str> = f.dlc_of(10, 2).iter().map(|d| d.name.as_str()).collect();
        assert_eq!(names, ["A pack", "b pack"]);
        assert!(f.dlc_of(10, 4).is_empty());
    }
}

#[cfg(test)]
mod price_tests {
    use super::format_price;

    #[test]
    fn prices_look_like_steams() {
        assert_eq!(format_price(448_000, "KZT"), "4 480₸");
        assert_eq!(format_price(640_000, "KZT"), "6 400₸");
        assert_eq!(format_price(1999, "USD"), "$19.99");
        assert_eq!(format_price(1999, "EUR"), "19,99€");
        assert_eq!(format_price(199_900, "RUB"), "1 999 ₽");
        assert_eq!(format_price(1_234_567, "USD"), "$12,345.67");
        assert_eq!(format_price(199_850, "KZT"), "1 998,50₸");
        assert_eq!(format_price(198_000, "JPY"), "¥ 1,980");
    }
}

/// Media item extracted from chat BBCode (`[img]`, `[url]`) or message URLs.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub enum ChatMediaKind {
    Image,
    Video,
    Link,
}

/// Extracted link or media preview attached to a chat message.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct ChatMedia {
    pub kind: ChatMediaKind,
    pub url: String,
    #[serde(default)]
    pub label: Option<String>,
    /// True for trusted Steam CDN / YouTube thumbnail hosts that can auto-load.
    #[serde(default)]
    pub trusted: bool,
}

/// One chat message in a 1:1 conversation.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ChatMessage {
    /// The other person in the conversation.
    pub friend: SteamId,
    /// Who wrote it (the friend, or you).
    pub from: SteamId,
    pub text: String,
    /// Unix seconds.
    pub time: u64,
    /// A game invite (Steam's `[gameinvite]`, or an older Aurora
    /// "steam://run" link), shown as a card with a Join button.
    #[serde(default)]
    pub invite: Option<GameInvite>,
    /// A group chat invite (`[invite]`, `[inviteurl]`, or `s.team/chat/…`),
    /// shown as a card with a Join button.
    #[serde(default)]
    pub group_invite: Option<GroupInviteRef>,
    /// Embedded media/link previews parsed from BBCode and URLs.
    #[serde(default)]
    pub media: Vec<ChatMedia>,
}

/// Reference to a group chat invitation embedded in a message.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub enum GroupInviteRef {
    Direct { group_id: GroupChatId, chat_id: ChatRoomId },
    Link { code: String },
}

impl GroupInviteRef {
    pub fn key(&self) -> String {
        match self {
            Self::Direct { group_id, chat_id } => format!("direct:{group_id}:{chat_id}"),
            Self::Link { code } => format!("link:{code}"),
        }
    }

    pub fn link_url(code: &str) -> String {
        format!("https://s.team/chat/{code}")
    }
}

/// Summary information about a group chat invite fetched from Steam.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct GroupInviteInfo {
    pub key: String,
    pub group_id: GroupChatId,
    pub chat_id: ChatRoomId,
    pub name: String,
    pub avatar_url: Option<String>,
    pub member_count: u32,
    pub banned: bool,
    pub kick_expires: u32,
    pub expires: u32,
    pub already_member: bool,
}

/// A role within a group chat.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct ChatRole {
    pub role_id: u64,
    pub name: String,
    pub ordinal: u32,
}

/// Permissions granted to a group chat role.
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct ChatRoleActions {
    pub role_id: u64,
    pub can_create_rename_delete_channel: bool,
    pub can_kick: bool,
    pub can_ban: bool,
    pub can_invite: bool,
    pub can_change_tagline_avatar_name: bool,
    pub can_chat: bool,
    pub can_view_history: bool,
    pub can_change_group_roles: bool,
    pub can_change_user_roles: bool,
    pub can_mention_all: bool,
    pub can_set_watching_broadcast: bool,
}

impl ChatRoleActions {
    pub fn all() -> Self {
        Self {
            role_id: 0,
            can_create_rename_delete_channel: true,
            can_kick: true,
            can_ban: true,
            can_invite: true,
            can_change_tagline_avatar_name: true,
            can_chat: true,
            can_view_history: true,
            can_change_group_roles: true,
            can_change_user_roles: true,
            can_mention_all: true,
            can_set_watching_broadcast: true,
        }
    }

    pub fn merge(&mut self, other: &Self) {
        self.can_create_rename_delete_channel |= other.can_create_rename_delete_channel;
        self.can_kick |= other.can_kick;
        self.can_ban |= other.can_ban;
        self.can_invite |= other.can_invite;
        self.can_change_tagline_avatar_name |= other.can_change_tagline_avatar_name;
        self.can_chat |= other.can_chat;
        self.can_view_history |= other.can_view_history;
        self.can_change_group_roles |= other.can_change_group_roles;
        self.can_change_user_roles |= other.can_change_user_roles;
        self.can_mention_all |= other.can_mention_all;
        self.can_set_watching_broadcast |= other.can_set_watching_broadcast;
    }
}

/// One text or voice channel inside a group chat.
#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub struct ChatRoomChannel {
    pub id: ChatRoomId,
    pub name: String,
    pub voice_allowed: bool,
    pub members_in_voice: Vec<u32>,
    pub sort_order: u32,
    pub time_last_message: u32,
    pub last_message: String,
    pub accountid_last_message: u32,
    pub time_last_ack: u32,
    pub time_first_unread: u32,
    pub time_last_mention: u32,
    pub unread_muted: bool,
    /// Locally tracked unread messages arriving while this channel isn't open.
    pub unread: usize,
}

impl ChatRoomChannel {
    pub fn has_unread(&self) -> bool {
        !self.unread_muted && (self.unread > 0 || (self.time_last_message > self.time_last_ack && self.time_last_message > 0))
    }

    pub fn unread_badge(&self) -> usize {
        if self.unread_muted {
            0
        } else if self.unread > 0 {
            self.unread
        } else if self.time_last_message > self.time_last_ack && self.time_last_message > 0 {
            1
        } else {
            0
        }
    }
}

/// One member inside a group chat.
#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub struct ChatGroupMember {
    pub steam_id: SteamId,
    pub account_id: u32,
    /// `EChatRoomGroupRank`: 50 Owner, 40 Officer, 30 Moderator, 20 Member, 15 Guest, 10 Viewer.
    pub rank: i32,
    /// `EChatRoomJoinState`: 2 Joined, 1 None.
    pub state: i32,
    pub time_kick_expire: u32,
    pub role_ids: Vec<u64>,
}

/// A Steam group chat (ad-hoc multi-friend chat, saved group, or Steam clan chat).
#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub struct ChatGroup {
    pub id: GroupChatId,
    pub name: String,
    pub tagline: String,
    pub avatar_url: Option<String>,
    pub owner_id: SteamId,
    pub clan_id: u32,
    pub app_id: AppId,
    pub default_chat_id: ChatRoomId,
    pub rooms: Vec<ChatRoomChannel>,
    pub members: Vec<ChatGroupMember>,
    pub top_members: Vec<SteamId>,
    pub active_member_count: u32,
    pub active_voice_member_count: u32,
    pub my_rank: i32,
    pub my_role_ids: Vec<u64>,
    pub roles: Vec<ChatRole>,
    pub role_actions: Vec<ChatRoleActions>,
    pub default_role_id: u64,
    pub time_joined: u32,
    pub time_last_group_ack: u32,
    pub unread_muted: bool,
    pub desktop_notification_level: i32,
    pub full_loaded: bool,
    pub virtualized_members: bool,
}

impl ChatGroup {
    pub fn is_saved(&self) -> bool {
        !self.name.trim().is_empty() || self.clan_id != 0
    }

    pub fn sort_rooms(&mut self) {
        self.rooms.sort_by_key(|r| (r.sort_order, r.id));
    }

    pub fn text_channels(&self) -> impl Iterator<Item = &ChatRoomChannel> {
        self.rooms.iter().filter(|r| !r.voice_allowed)
    }

    pub fn voice_channels(&self) -> impl Iterator<Item = &ChatRoomChannel> {
        self.rooms.iter().filter(|r| r.voice_allowed)
    }

    pub fn channel(&self, chat_id: ChatRoomId) -> Option<&ChatRoomChannel> {
        self.rooms.iter().find(|r| r.id == chat_id)
    }

    pub fn channel_mut(&mut self, chat_id: ChatRoomId) -> Option<&mut ChatRoomChannel> {
        self.rooms.iter_mut().find(|r| r.id == chat_id)
    }

    pub fn default_text_channel_id(&self) -> Option<ChatRoomId> {
        if self.rooms.iter().any(|r| r.id == self.default_chat_id && !r.voice_allowed) {
            return Some(self.default_chat_id);
        }
        self.rooms
            .iter()
            .find(|r| !r.voice_allowed)
            .or_else(|| self.rooms.first())
            .map(|r| r.id)
    }

    pub fn unread_count(&self) -> usize {
        if self.unread_muted {
            return 0;
        }
        self.rooms.iter().map(ChatRoomChannel::unread_badge).sum()
    }

    pub fn effective_permissions(&self, me_account: u32) -> ChatRoleActions {
        if self.my_rank >= 50 || (me_account != 0 && self.owner_id == account_to_steam_id(me_account)) {
            return ChatRoleActions::all();
        }
        let mut perms = ChatRoleActions::default();
        let mut matched = false;
        let ids: Vec<u64> = if !self.my_role_ids.is_empty() {
            self.my_role_ids.clone()
        } else if let Some(m) = self.members.iter().find(|m| m.account_id == me_account) {
            m.role_ids.clone()
        } else {
            Vec::new()
        };
        for act in &self.role_actions {
            if ids.contains(&act.role_id) || (ids.is_empty() && act.role_id == self.default_role_id) {
                perms.merge(act);
                matched = true;
            }
        }
        if !matched && self.role_actions.is_empty() {
            // Summary before full role actions load: allow chatting & inviting for ordinary members.
            perms.can_chat = self.my_rank >= 15 || self.my_rank == 0;
            perms.can_view_history = true;
            perms.can_invite = self.my_rank >= 20 || !self.is_saved();
            perms.can_create_rename_delete_channel = self.my_rank >= 40;
            perms.can_change_tagline_avatar_name = self.my_rank >= 40;
            perms.can_kick = self.my_rank >= 30;
            perms.can_ban = self.my_rank >= 40;
            perms.can_mention_all = self.my_rank >= 30;
        }
        perms
    }

    pub fn display_name(&self, resolve: impl Fn(SteamId) -> Option<String>, me: SteamId) -> String {
        if !self.name.trim().is_empty() {
            return self.name.clone();
        }
        let mut seen = Vec::new();
        for id in self.top_members.iter().copied().chain(self.members.iter().map(|m| m.steam_id)) {
            if id == 0 || id == me || seen.contains(&id) {
                continue;
            }
            seen.push(id);
        }
        let names: Vec<String> = seen
            .iter()
            .take(3)
            .filter_map(|id| resolve(*id).filter(|s| !s.trim().is_empty()))
            .collect();
        if names.is_empty() {
            return format!("Group #{}", self.id);
        }
        let extra = self.active_member_count.saturating_sub(1 + names.len() as u32);
        if extra > 0 && names.len() >= 2 {
            format!("{} + {extra}", names[..2].join(", "))
        } else {
            names.join(", ")
        }
    }
}

/// System/server notice recorded in group chat history (`EChatRoomServerMessage`).
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct GroupServerMessage {
    pub kind: i32,
    pub string_param: String,
    pub account_id_param: u32,
}

/// One emoji/sticker reaction summary on a group chat message.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct MessageReaction {
    pub reaction_type: i32,
    pub reaction: String,
    pub count: u32,
    pub reacted: bool,
}

/// One message in a group chat channel.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct GroupMessage {
    pub group_id: GroupChatId,
    pub chat_id: ChatRoomId,
    pub from: SteamId,
    pub text: String,
    pub time: u64,
    pub ordinal: u32,
    #[serde(default)]
    pub invite: Option<GameInvite>,
    #[serde(default)]
    pub group_invite: Option<GroupInviteRef>,
    #[serde(default)]
    pub deleted: bool,
    #[serde(default)]
    pub server_msg: Option<GroupServerMessage>,
    #[serde(default)]
    pub reactions: Vec<MessageReaction>,
    #[serde(default)]
    pub mentioned_me: bool,
    #[serde(default)]
    pub media: Vec<ChatMedia>,
}

/// An active invite link for a group chat.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct GroupInviteLink {
    pub code: String,
    pub creator: SteamId,
    pub expires: u32,
    pub chat_id: ChatRoomId,
}

/// A banned user in a group chat.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct GroupBanInfo {
    pub steam_id: SteamId,
    pub actor: SteamId,
    pub time_banned: u32,
    pub reason: String,
}

/// "Come play with me": which game, and how to join the inviter.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct GameInvite {
    pub app_id: AppId,
    /// Launch arguments that join the inviter (`+connect_lobby 123`,
    /// `+invite 456 …`); empty to just start the game.
    pub connect: String,
}

impl GameInvite {
    /// A link that starts the game with the join arguments, for invites in
    /// chat. (Steam strips `[gameinvite]` markup that doesn't come from its
    /// own client, so we can't send that; lobby invites go through the
    /// lobby itself and Steam writes their chat message.) Any Steam client
    /// opens it; ours shows it as an invite card.
    pub fn link(&self) -> String {
        if self.connect.is_empty() {
            return format!("steam://run/{}", self.app_id);
        }
        let mut args = String::new();
        for b in self.connect.bytes() {
            match b {
                b'A'..=b'Z' | b'a'..=b'z' | b'0'..=b'9' | b'-' | b'_' | b'.' | b'+' | b':' => args.push(b as char),
                _ => args.push_str(&format!("%{b:02X}")),
            }
        }
        format!("steam://run/{}//{args}", self.app_id)
    }
}

/// Steam chat markup ("BBCode") to plain text plus any embedded game or
/// group-chat invite.
pub mod chat {
    use super::{ChatMedia, ChatMediaKind, GameInvite, GroupInviteRef};

    const MAX_CHAT_MEDIA: usize = 8;

    #[derive(Debug, Clone, PartialEq, Eq)]
    pub struct ParsedChat {
        pub text: String,
        pub invite: Option<GameInvite>,
        pub group_invite: Option<GroupInviteRef>,
        pub media: Vec<ChatMedia>,
    }

    const TRUSTED_MEDIA_HOSTS: &[&str] = &[
        "steamusercontent.com",
        "steamusercontent-a.akamaihd.net",
        "steamuserimages-a.akamaihd.net",
        "steamstatic.com",
        "steamcdn-a.akamaihd.net",
        "steamcommunity.com",
        "media.steampowered.com",
        "i.ytimg.com",
        "img.youtube.com",
    ];

    /// Return true when `host` is an allowed Steam CDN or YouTube thumbnail host.
    pub fn is_trusted_media_host(host: &str) -> bool {
        let host = host.trim().trim_end_matches('.').to_ascii_lowercase();
        if host.is_empty() {
            return false;
        }
        TRUSTED_MEDIA_HOSTS.iter().any(|allowed| {
            host == *allowed
                || (host.len() > allowed.len()
                    && host.ends_with(allowed)
                    && host.as_bytes()[host.len() - allowed.len() - 1] == b'.')
        })
    }

    fn http_rest(raw: &str) -> Option<&str> {
        let s = raw.trim();
        if s.chars().any(char::is_control) {
            return None;
        }
        if s.get(..8).is_some_and(|p| p.eq_ignore_ascii_case("https://")) {
            Some(&s[8..])
        } else if s.get(..7).is_some_and(|p| p.eq_ignore_ascii_case("http://")) {
            Some(&s[7..])
        } else {
            None
        }
    }

    /// Extract the host portion of an `http://` or `https://` URL.
    pub fn url_host(raw: &str) -> Option<&str> {
        let rest = http_rest(raw)?;
        let authority = rest.split(['/', '?', '#']).next()?;
        if authority.is_empty() || authority.contains('\\') || authority.chars().any(char::is_whitespace) {
            return None;
        }
        let host_port = authority.rsplit_once('@').map(|(_, h)| h).unwrap_or(authority);
        if host_port.is_empty() {
            return None;
        }
        let host = if let Some(bracketed) = host_port.strip_prefix('[') {
            bracketed.split(']').next()?
        } else {
            host_port.split(':').next()?
        };
        (!host.is_empty()).then_some(host)
    }

    fn is_safe_http_url(raw: &str) -> bool {
        url_host(raw).is_some()
    }

    fn has_image_extension(url: &str) -> bool {
        let path = url.split(['?', '#']).next().unwrap_or(url);
        let lower = path.to_ascii_lowercase();
        [".png", ".jpg", ".jpeg", ".gif", ".webp"]
            .iter()
            .any(|ext| lower.ends_with(ext))
    }

    /// Steam screenshots and uploaded chat images use a UGC ID and hash in place of an extension.
    fn is_steam_ugc_image(url: &str, host: &str) -> bool {
        let host = host.trim_end_matches('.').to_ascii_lowercase();
        if !matches!(host.as_str(), "images.steamusercontent.com" | "steamusercontent-a.akamaihd.net" | "steamuserimages-a.akamaihd.net") {
            return false;
        }
        let Some((_, path)) = http_rest(url).and_then(|rest| rest.split(['?', '#']).next()?.split_once('/')) else {
            return false;
        };
        let mut segments = path.trim_end_matches('/').split('/');
        matches!(
            (segments.next(), segments.next(), segments.next(), segments.next()),
            (Some("ugc"), Some(id), Some(hash), None)
                if !id.is_empty() && id.bytes().all(|b| b.is_ascii_digit())
                    && !hash.is_empty() && hash.bytes().all(|b| b.is_ascii_hexdigit())
        )
    }

    fn valid_yt_id(id: &str) -> bool {
        id.len() == 11 && id.bytes().all(|b| b.is_ascii_alphanumeric() || b == b'-' || b == b'_')
    }

    /// Extract a validated 11-character YouTube video ID from common watch/short/embed URLs.
    pub fn youtube_id_from_url(url: &str) -> Option<String> {
        let host = url_host(url)?.to_ascii_lowercase();
        let host = host.strip_prefix("www.").or_else(|| host.strip_prefix("m.")).unwrap_or(&host);
        let rest = http_rest(url)?;
        let path_and_query = match rest.find('/') {
            Some(i) => &rest[i..],
            None => "",
        };
        let without_frag = path_and_query.split('#').next().unwrap_or("");
        let (path, query) = without_frag.split_once('?').unwrap_or((without_frag, ""));
        if host == "youtu.be" {
            let seg = path.trim_start_matches('/').split('/').next().unwrap_or("");
            return valid_yt_id(seg).then(|| seg.to_string());
        }
        if host == "youtube.com" || host == "youtube-nocookie.com" {
            let segs: Vec<&str> = path.trim_matches('/').split('/').filter(|s| !s.is_empty()).collect();
            if segs.first() == Some(&"watch") || segs.is_empty() {
                for pair in query.split('&') {
                    if let Some(v) = pair.strip_prefix("v=") {
                        if valid_yt_id(v) {
                            return Some(v.to_string());
                        }
                    }
                }
            }
            if let [kind, id, ..] = segs.as_slice() {
                if matches!(*kind, "shorts" | "embed" | "live" | "v") && valid_yt_id(id) {
                    return Some((*id).to_string());
                }
            }
        }
        None
    }

    fn trim_trailing_url_punct(raw: &str) -> &str {
        let mut s = raw;
        while let Some(last) = s.chars().last() {
            let strip = match last {
                '.' | ',' | '!' | '?' | ';' | ':' | '"' | '\'' | '>' => true,
                ')' => s.chars().filter(|&c| c == ')').count() > s.chars().filter(|&c| c == '(').count(),
                ']' => s.chars().filter(|&c| c == ']').count() > s.chars().filter(|&c| c == '[').count(),
                _ => false,
            };
            if strip {
                s = &s[..s.len() - last.len_utf8()];
            } else {
                break;
            }
        }
        s
    }

    /// Find all `http://` and `https://` URLs with their byte spans in `text`.
    pub fn find_http_urls(text: &str) -> Vec<(std::ops::Range<usize>, String)> {
        let mut out = Vec::new();
        let bytes = text.as_bytes();
        let mut idx = 0;
        while idx < text.len() {
            let slice = &text[idx..];
            let next_rel = slice
                .find("https://")
                .into_iter()
                .chain(slice.find("http://"))
                .min();
            let Some(rel) = next_rel else { break };
            let start = idx + rel;
            if start > 0 && bytes[start - 1].is_ascii_alphanumeric() {
                idx = start + 4;
                continue;
            }
            let rest = &text[start..];
            let end_rel = rest
                .char_indices()
                .find(|&(_, ch)| ch.is_whitespace() || matches!(ch, '<' | '"'))
                .map(|(i, _)| i)
                .unwrap_or(rest.len());
            let candidate = trim_trailing_url_punct(&rest[..end_rel]);
            if is_safe_http_url(candidate) {
                let end = start + candidate.len();
                out.push((start..end, candidate.to_string()));
                idx = end.max(start + 1);
            } else {
                idx = start + 7.min(rest.len());
            }
        }
        out
    }

    /// Extract validated `http(s)://` URL strings from plain message text.
    pub fn extract_http_urls(text: &str) -> Vec<String> {
        find_http_urls(text).into_iter().map(|(_, u)| u).collect()
    }

    fn push_media_classified(media: &mut Vec<ChatMedia>, url: &str, label: Option<String>, force_image: bool) {
        if media.len() >= MAX_CHAT_MEDIA {
            return;
        }
        let trimmed = url.trim();
        let Some(host) = url_host(trimmed) else { return };
        if media.iter().any(|m| m.url == trimmed) {
            return;
        }
        let (kind, trusted) = if force_image {
            (ChatMediaKind::Image, is_trusted_media_host(host))
        } else if youtube_id_from_url(trimmed).is_some() {
            (ChatMediaKind::Video, true)
        } else if has_image_extension(trimmed) || is_steam_ugc_image(trimmed, host) {
            (ChatMediaKind::Image, is_trusted_media_host(host))
        } else {
            (ChatMediaKind::Link, is_trusted_media_host(host))
        };
        media.push(ChatMedia {
            kind,
            url: trimmed.to_string(),
            label: label.filter(|l| !l.trim().is_empty() && l.trim() != trimmed),
            trusted,
        });
    }

    /// `name="value"` (or `name=value`) from inside a tag.
    fn attr(tag: &str, name: &str) -> Option<String> {
        let mut from = 0;
        while let Some(i) = tag[from..].find(name).map(|i| i + from) {
            from = i + name.len();
            let starts_word = tag[..i].chars().last().is_none_or(|c| c.is_whitespace());
            let Some(v) = tag[from..].strip_prefix('=') else { continue };
            if !starts_word {
                continue;
            }
            return Some(match v.strip_prefix('"') {
                Some(q) => q.split('"').next().unwrap_or("").to_string(),
                None => v.split(char::is_whitespace).next().unwrap_or("").to_string(),
            });
        }
        None
    }

    fn invite_from(tag: &str) -> Option<GameInvite> {
        let app_id = attr(tag, "appid")?.parse().ok()?;
        let mut connect = attr(tag, "connect").unwrap_or_default();
        if connect.is_empty() {
            if let Some(lobby) = attr(tag, "lobbyid").filter(|l| !l.is_empty() && l != "0") {
                connect = format!("+connect_lobby {lobby}");
            }
        }
        Some(GameInvite { app_id, connect })
    }

    fn group_invite_from(tag: &str) -> Option<GroupInviteRef> {
        if let Some(code) = attr(tag, "invitecode").filter(|c| !c.is_empty()) {
            return Some(GroupInviteRef::Link { code });
        }
        let group_id = attr(tag, "chatgroupid")?.parse().ok()?;
        let chat_id = attr(tag, "chatid").and_then(|c| c.parse().ok()).unwrap_or(0);
        Some(GroupInviteRef::Direct { group_id, chat_id })
    }

    fn link_group_invite(text: &str) -> Option<GroupInviteRef> {
        for prefix in ["https://s.team/chat/", "http://s.team/chat/", "https://steamcommunity.com/chat/invite/", "http://steamcommunity.com/chat/invite/"] {
            if let Some(at) = text.find(prefix) {
                let code: String = text[at + prefix.len()..]
                    .chars()
                    .take_while(|c| c.is_ascii_alphanumeric() || *c == '-' || *c == '_')
                    .collect();
                if !code.is_empty() {
                    return Some(GroupInviteRef::Link { code });
                }
            }
        }
        None
    }

    /// Parse Steam chat markup into display text plus optional invites and media/link previews.
    pub fn parse_message(raw: &str) -> ParsedChat {
        let mut out = String::new();
        let mut invite = None;
        let mut group_invite = None;
        let mut media = Vec::new();
        let mut open_url: Option<(Option<String>, usize)> = None;
        let mut open_img: Option<(Option<String>, String)> = None;
        let mut rest = raw;
        while let Some(c) = rest.chars().next() {
            // Steam escapes a literal "[" as "\[".
            if let Some(after) = rest.strip_prefix("\\[").or_else(|| rest.strip_prefix("\\\\")) {
                let ch = rest.as_bytes()[1] as char;
                if let Some((_, buf)) = &mut open_img {
                    buf.push(ch);
                } else {
                    out.push(ch);
                }
                rest = after;
                continue;
            }
            if c == '[' {
                if let Some(end) = rest.find(']') {
                    let tag = &rest[1..end];
                    let closing = tag.starts_with('/');
                    let name = tag.trim_start_matches('/').split(|c: char| c.is_whitespace() || c == '=').next().unwrap_or("");
                    if !name.is_empty() && name.chars().all(|c| c.is_ascii_alphanumeric() || c == '_') {
                        let lower_name = name.to_ascii_lowercase();
                        match lower_name.as_str() {
                            "gameinvite" | "lobbyinvite" if !closing => invite = invite.or_else(|| invite_from(tag)),
                            "invite" | "inviteurl" if !closing => group_invite = group_invite.or_else(|| group_invite_from(tag)),
                            "emoticon" => out.push(':'),
                            "sticker" if !closing => out.push_str(&format!("[{}]", attr(tag, "type").unwrap_or_else(|| "sticker".into()))),
                            "img" if !closing => {
                                let src = tag
                                    .strip_prefix(name)
                                    .and_then(|t| t.trim_start().strip_prefix('='))
                                    .map(|v| v.trim().trim_matches('"').to_string())
                                    .filter(|v| !v.is_empty())
                                    .or_else(|| attr(tag, "src"));
                                open_img = Some((src, String::new()));
                            }
                            "img" if closing => {
                                if let Some((src, inner)) = open_img.take() {
                                    let url = src.filter(|s| !s.is_empty()).unwrap_or(inner);
                                    push_media_classified(&mut media, &url, None, true);
                                }
                            }
                            "url" if !closing => {
                                let href = tag
                                    .strip_prefix(name)
                                    .and_then(|t| t.trim_start().strip_prefix('='))
                                    .map(|v| v.trim().trim_matches('"').to_string())
                                    .filter(|v| !v.is_empty())
                                    .or_else(|| attr(tag, "href"));
                                open_url = Some((href, out.len()));
                            }
                            "url" if closing => {
                                if let Some((href_opt, start)) = open_url.take() {
                                    let label = out[start..].trim().to_string();
                                    let href = href_opt.unwrap_or_else(|| label.clone());
                                    if label.is_empty() && !href.is_empty() && is_safe_http_url(&href) {
                                        out.push_str(&href);
                                    }
                                    push_media_classified(&mut media, &href, Some(label), false);
                                }
                            }
                            _ => {}
                        }
                        rest = &rest[end + 1..];
                        continue;
                    }
                }
            }
            if let Some((_, buf)) = &mut open_img {
                buf.push(c);
            } else {
                out.push(c);
            }
            rest = &rest[c.len_utf8()..];
        }
        if let Some((src, inner)) = open_img.take() {
            let url = src.filter(|s| !s.is_empty()).unwrap_or(inner);
            push_media_classified(&mut media, &url, None, true);
        }
        if let Some((href_opt, start)) = open_url.take() {
            let label = out[start..].trim().to_string();
            let href = href_opt.unwrap_or_else(|| label.clone());
            push_media_classified(&mut media, &href, Some(label), false);
        }
        let text = out.trim().to_string();
        // Aurora's invites: "Play X with me! steam://run/123//+args".
        if invite.is_none() {
            invite = link_invite(&text);
        }
        if group_invite.is_none() {
            group_invite = link_group_invite(&text);
        }
        for url in extract_http_urls(&text) {
            if group_invite.is_some()
                && (url.contains("s.team/chat/") || url.contains("steamcommunity.com/chat/invite/"))
            {
                continue;
            }
            push_media_classified(&mut media, &url, None, false);
        }
        ParsedChat { text, invite, group_invite, media }
    }

    /// Plain text for display, and the game invite in it.
    pub fn parse(raw: &str) -> (String, Option<GameInvite>) {
        let parsed = parse_message(raw);
        (parsed.text, parsed.invite)
    }

    /// Convert `@all`, `@here`, and `@name` tokens into Steam `[mention=...]` BBCode.
    pub fn encode_mentions(raw: &str, members: &[(u32, &str)]) -> String {
        let mut sorted: Vec<(u32, &str)> = members.iter().copied().filter(|(_, n)| !n.trim().is_empty()).collect();
        sorted.sort_by_key(|(_, n)| std::cmp::Reverse(n.len()));
        let mut out = String::with_capacity(raw.len());
        let mut rest = raw;
        while let Some(at) = rest.find('@') {
            out.push_str(&rest[..at]);
            let after = &rest[at + 1..];
            let preceded_ok = rest[..at].chars().last().is_none_or(|c| c.is_whitespace());
            if !preceded_ok {
                out.push('@');
                rest = after;
                continue;
            }
            let kw = ["all", "here"].into_iter().find(|k| {
                after.len() >= k.len()
                    && after[..k.len()].eq_ignore_ascii_case(k)
                    && after[k.len()..].chars().next().is_none_or(|c| !c.is_alphanumeric() && c != '_')
            });
            if let Some(k) = kw {
                out.push_str(&format!("[mention={k}]@{k}[/mention]"));
                rest = &after[k.len()..];
                continue;
            }
            let matched = sorted.iter().find(|(_, name)| {
                after.len() >= name.len()
                    && after[..name.len()].eq_ignore_ascii_case(name)
                    && after[name.len()..].chars().next().is_none_or(|c| c.is_whitespace() || ",.!?;:)]}".contains(c))
            });
            if let Some((acct, name)) = matched {
                let exact = &after[..name.len()];
                out.push_str(&format!("[mention={acct}]@{exact}[/mention]"));
                rest = &after[name.len()..];
            } else {
                out.push('@');
                rest = after;
            }
        }
        out.push_str(rest);
        out
    }

    /// The invite in a `steam://run/<app>[//<args>]` link, if any.
    fn link_invite(text: &str) -> Option<GameInvite> {
        let at = text.find("steam://run/")?;
        let rest = &text[at + 12..];
        let digits: String = rest.chars().take_while(|c| c.is_ascii_digit()).collect();
        let app_id = digits.parse().ok()?;
        let args = rest[digits.len()..].strip_prefix("//").unwrap_or("");
        let args = args.split(char::is_whitespace).next().unwrap_or("");
        // Percent-decoded launch arguments.
        let mut bytes = Vec::new();
        let raw = args.as_bytes();
        let mut i = 0;
        while i < raw.len() {
            match (raw[i], raw.get(i + 1..i + 3).and_then(|h| std::str::from_utf8(h).ok()).and_then(|h| u8::from_str_radix(h, 16).ok())) {
                (b'%', Some(b)) => {
                    bytes.push(b);
                    i += 3;
                }
                (b, _) => {
                    bytes.push(b);
                    i += 1;
                }
            }
        }
        Some(GameInvite { app_id, connect: String::from_utf8_lossy(&bytes).into_owned() })
    }

    #[cfg(test)]
    mod tests {
        use super::*;

        #[test]
        fn steam_invites() {
            let (text, inv) = parse(r#"[gameinvite  appid="570" connect="+invite 30095787509687845 -launchsource2"][/gameinvite]"#);
            assert_eq!(text, "");
            assert_eq!(inv, Some(GameInvite { app_id: 570, connect: "+invite 30095787509687845 -launchsource2".into() }));
            let (_, inv) = parse(r#"[lobbyinvite appid="477160" lobbyid="109775241000000001"][/lobbyinvite]"#);
            assert_eq!(inv.unwrap().connect, "+connect_lobby 109775241000000001");
        }

        #[test]
        fn group_invites_and_mentions() {
            let p = parse_message(r#"[invite chatgroupid="12345" chatid="67890"][/invite]"#);
            assert_eq!(p.text, "");
            assert_eq!(p.group_invite, Some(GroupInviteRef::Direct { group_id: 12345, chat_id: 67890 }));

            let p2 = parse_message(r#"[inviteurl invitecode="AbCdEf12"][/inviteurl]"#);
            assert_eq!(p2.group_invite, Some(GroupInviteRef::Link { code: "AbCdEf12".into() }));

            let p3 = parse_message("join us at https://s.team/chat/XyZ987 tonight!");
            assert_eq!(p3.group_invite, Some(GroupInviteRef::Link { code: "XyZ987".into() }));

            let p4 = parse_message("hey [mention=42]@Ferris[/mention] check [mention=all]@all[/mention]");
            assert_eq!(p4.text, "hey @Ferris check @all");

            let encoded = encode_mentions("hi @Ferris and @all! email@test.com", &[(42, "Ferris")]);
            assert_eq!(encoded, "hi [mention=42]@Ferris[/mention] and [mention=all]@all[/mention]! email@test.com");
        }

        #[test]
        fn links() {
            let me = GameInvite { app_id: 10, connect: "+connect_lobby 1234 -x \"a b\"".into() };
            assert_eq!(me.link(), "steam://run/10//+connect_lobby%201234%20-x%20%22a%20b%22");
            assert_eq!(parse(&format!("Join me! {}", me.link())).1, Some(me));
            let plain = GameInvite { app_id: 477160, connect: String::new() };
            assert_eq!(plain.link(), "steam://run/477160");
            assert_eq!(parse("Play it with me! steam://run/477160 now").1, Some(plain));
        }

        #[test]
        fn plain_text() {
            assert_eq!(parse("hi [emoticon]steamhappy[/emoticon] there").0, "hi :steamhappy: there");
            assert_eq!(parse(r"a \[b] c").0, "a [b] c");
            assert_eq!(parse("[url=https://x.y]link[/url]").0, "link");
            assert_eq!(parse("1 [ 2").0, "1 [ 2");
            assert_eq!(parse("no tags").1, None);
            let (text, inv) = parse("Сыграем в Human Fall Flat? steam://run/477160");
            assert_eq!(text, "Сыграем в Human Fall Flat? steam://run/477160");
            assert_eq!(inv, Some(GameInvite { app_id: 477160, connect: String::new() }));
        }

        #[test]
        fn steam_screenshot_links_and_direct_gifs() {
            let path = "/ugc/12345678901234567890/ABCDEF0123456789ABCDEF0123456789ABCDEF0/";
            for url in [
                format!("https://images.steamusercontent.com{path}"),
                format!("https://steamuserimages-a.akamaihd.net{path}?imw=600"),
            ] {
                let parsed = parse_message(&format!("look at {url}"));
                assert_eq!(parsed.media[0].kind, ChatMediaKind::Image);
                assert_eq!(parsed.media[0].url, url);
                assert!(parsed.media[0].trusted);
            }
            let lookalike = parse_message(&format!("https://images.steamusercontent.com.example{path}"));
            assert_eq!(lookalike.media[0].kind, ChatMediaKind::Link);
            assert!(!lookalike.media[0].trusted);

            let gif = parse_message("https://example.com/clip.GIF?v=1");
            assert_eq!(gif.media[0].kind, ChatMediaKind::Image);
            assert!(!gif.media[0].trusted);
        }

        #[test]
        fn media_links_images_and_youtube() {
            let p_img = parse_message("[img]https://images.steamusercontent.com/ugc/1234/shot.png[/img]");
            assert_eq!(p_img.text, "");
            assert_eq!(p_img.media.len(), 1);
            assert_eq!(p_img.media[0].kind, ChatMediaKind::Image);
            assert!(p_img.media[0].trusted);

            let p_ext = parse_message("look https://example.com/pic.jpg.");
            assert_eq!(p_ext.media.len(), 1);
            assert_eq!(p_ext.media[0].url, "https://example.com/pic.jpg");
            assert_eq!(p_ext.media[0].kind, ChatMediaKind::Image);
            assert!(!p_ext.media[0].trusted);

            let p_evil_suffix = parse_message("https://evil-steamstatic.com/x.png");
            assert!(!p_evil_suffix.media[0].trusted);

            let p_url = parse_message("[url=https://store.steampowered.com/app/620]Portal 2[/url]");
            assert_eq!(p_url.text, "Portal 2");
            assert_eq!(p_url.media[0].kind, ChatMediaKind::Link);
            assert_eq!(p_url.media[0].url, "https://store.steampowered.com/app/620");
            assert_eq!(p_url.media[0].label.as_deref(), Some("Portal 2"));

            let p_yt = parse_message("watch https://youtu.be/dQw4w9WgXcQ!");
            assert_eq!(p_yt.media[0].kind, ChatMediaKind::Video);
            assert_eq!(p_yt.media[0].url, "https://youtu.be/dQw4w9WgXcQ");
            assert_eq!(youtube_id_from_url(&p_yt.media[0].url).as_deref(), Some("dQw4w9WgXcQ"));

            // Bad schemes ignored
            let p_bad = parse_message("[img]file:///etc/passwd[/img] [url=javascript:alert(1)]bad[/url]");
            assert!(p_bad.media.is_empty());

            // Deduplicates and caps at MAX_CHAT_MEDIA
            let mut raw = String::from("https://example.com/1 https://example.com/1 ");
            for i in 2..15 {
                raw.push_str(&format!("https://example.com/{i} "));
            }
            let p_cap = parse_message(&raw);
            assert_eq!(p_cap.media.len(), MAX_CHAT_MEDIA);
            assert_eq!(p_cap.media[0].url, "https://example.com/1");
        }
    }
}

/// Image URLs on Valve's public CDN.
pub mod art {
    use super::AppId;
    const CDN: &str = "https://cdn.akamai.steamstatic.com/steam/apps";
    const DEFAULT_AVATAR: &str = "fef49e7fa7e1997310d705b2a6158ff8dc1cdfeb";

    /// Art paths from app info, for apps whose art moved to Steam's newer
    /// store asset folders (`<hash>/header.jpg`, `<hash>/library_600x900.jpg`…).
    /// Newer games and DLC only have it there; the old fixed address is a 404.
    static PATHS: std::sync::RwLock<Option<std::collections::HashMap<(AppId, String), String>>> = std::sync::RwLock::new(None);
    const ASSETS: &str = "https://shared.akamai.steamstatic.com/store_item_assets/steam/apps";

    /// Remember an art path from an app's info, like "<hash>/header.jpg".
    pub fn remember(id: AppId, path: &str) {
        let Some((_, file)) = path.rsplit_once('/') else { return };
        if let Ok(mut map) = PATHS.write() {
            map.get_or_insert_with(Default::default).insert((id, file.to_string()), path.to_string());
        }
    }

    /// The newer address when app info gave one, else the classic one.
    fn url(id: AppId, file: &str) -> String {
        let known = PATHS.read().ok().and_then(|m| m.as_ref()?.get(&(id, file.to_string())).cloned());
        match known {
            Some(path) => format!("{ASSETS}/{id}/{path}"),
            None => format!("{CDN}/{id}/{file}"),
        }
    }

    pub fn header(id: AppId) -> String {
        url(id, "header.jpg")
    }
    pub fn capsule(id: AppId) -> String {
        url(id, "library_600x900.jpg")
    }
    pub fn hero(id: AppId) -> String {
        url(id, "library_hero.jpg")
    }
    pub fn logo(id: AppId) -> String {
        url(id, "logo.png")
    }
    pub fn icon(id: AppId, hash: &str) -> String {
        format!("https://cdn.akamai.steamstatic.com/steamcommunity/public/images/apps/{id}/{hash}.jpg")
    }
    pub fn avatar_full(hash: Option<&str>) -> String {
        avatar(hash).replace("_medium.jpg", "_full.jpg")
    }
    /// Community item images (profile backgrounds, frames).
    pub fn community_item(path: &str) -> String {
        format!("https://cdn.akamai.steamstatic.com/steamcommunity/public/images/{path}")
    }
    /// Group chat icon from its SHA bytes.
    pub fn group_chat_avatar(sha: &[u8]) -> Option<String> {
        if sha.len() < 3 || sha.iter().all(|b| *b == 0) {
            return None;
        }
        let hex: String = sha.iter().map(|b| format!("{b:02x}")).collect();
        Some(format!(
            "https://shared.fastly.steamstatic.com/community_assets/images/chaticons/{}/{}/{}/{}_64.jpg",
            &hex[0..2],
            &hex[2..4],
            &hex[4..6],
            hex
        ))
    }
    /// Inventory item image, at most `size` × `size` with its own shape.
    /// (The "…fx…f" form pads it into a transparent square, which made
    /// items look small; this one doesn't.)
    pub fn econ(icon: &str, size: u32) -> String {
        format!("https://community.akamai.steamstatic.com/economy/image/{icon}/{size}x{size}")
    }
    pub fn avatar(hash: Option<&str>) -> String {
        let h = hash
            .filter(|h| !h.is_empty() && !h.chars().all(|c| c == '0'))
            .unwrap_or(DEFAULT_AVATAR);
        format!("https://avatars.steamstatic.com/{h}_medium.jpg")
    }
}

#[cfg(test)]
mod trade_url_tests {
    #[test]
    fn reads_trade_urls() {
        let (id, token) = super::parse_trade_url("https://steamcommunity.com/tradeoffer/new/?partner=12345&token=AbC-12").unwrap();
        assert_eq!(id, 76561197960265728 + 12345);
        assert_eq!(token.as_deref(), Some("AbC-12"));
        assert_eq!(super::parse_trade_url("https://steamcommunity.com/tradeoffer/new/?partner=76561197960278073"), Some((76561197960278073, None)));
        assert_eq!(super::parse_trade_url("https://steamcommunity.com/id/someone"), None);
    }

    #[test]
    fn persona_join_args_hardens_server_and_connect() {
        use super::Persona;
        let ok_srv = Persona { server: Some("192.168.1.5:27015".into()), ..Default::default() };
        assert_eq!(ok_srv.join_args().as_deref(), Some("+connect 192.168.1.5:27015"));
        let ok_v6 = Persona { server: Some("[2001:db8::1]:27015".into()), ..Default::default() };
        assert_eq!(ok_v6.join_args().as_deref(), Some("+connect [2001:db8::1]:27015"));

        for bad in [
            "1.2.3.4:27015 --gpu-launcher=/bin/sh",
            "-mono-assembly-path=/tmp/x",
            "+exec evil.cfg",
            "LD_PRELOAD=/tmp/x.so %command%",
            "1.2.3.4:27015\n+quit",
            "",
        ] {
            let p = Persona { server: Some(bad.into()), ..Default::default() };
            assert_eq!(p.join_args(), None, "unsafe server must be rejected: {bad:?}");
        }

        for bad_connect in [
            "LD_PRELOAD=/tmp/x.so /bin/sh %command%",
            "%command%",
            "--gpu-launcher=/bin/sh",
            "+exec autoexec.cfg",
        ] {
            let p = Persona { connect: Some(bad_connect.into()), ..Default::default() };
            assert_eq!(p.join_args(), None, "unsafe connect must be rejected: {bad_connect:?}");
        }

        let fallback = Persona {
            connect: Some("%command%".into()),
            server: Some("-bad".into()),
            lobby: Some(42),
            ..Default::default()
        };
        assert_eq!(fallback.join_args().as_deref(), Some("+connect_lobby 42"));
    }
}

/// Entry in Aurora's Game Servers browser (master server, Favorites, History, LAN, Friends).
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct GameServerEntry {
    pub addr: String,
    pub ip: u32,
    pub game_port: u16,
    pub query_port: u16,
    pub steam_id: u64,
    pub app_id: u32,
    pub name: String,
    pub map: String,
    pub game_dir: String,
    pub description: String,
    pub players: u32,
    pub max_players: u32,
    pub bots: u32,
    pub password: bool,
    pub secure: bool,
    pub ping_ms: u32,
    pub tags: String,
}

/// Player recently played with (`View -> Players` / `/friends/coplay`).
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct RecentPlayer {
    pub steam_id: SteamId,
    pub name: String,
    #[serde(default)]
    pub avatar: String,
    #[serde(default)]
    pub app_id: Option<AppId>,
    #[serde(default)]
    pub game_name: String,
    #[serde(default)]
    pub played_at: String,
}

/// Compare names as lowercase text, with no allocation for ASCII names.
pub fn cmp_ignore_case(a: &str, b: &str) -> std::cmp::Ordering {
    if a.is_ascii() && b.is_ascii() {
        a.bytes().map(|c| c.to_ascii_lowercase())
            .cmp(b.bytes().map(|c| c.to_ascii_lowercase()))
    } else {
        a.to_lowercase().cmp(&b.to_lowercase())
    }
}

/// Search for an already-lowercased filter, preserving Unicode lowercasing.
pub fn contains_ignore_case(text: &str, lowercase_query: &str) -> bool {
    if lowercase_query.is_empty() {
        true
    } else if text.is_ascii() {
        text.as_bytes().windows(lowercase_query.len()).any(|w| w.eq_ignore_ascii_case(lowercase_query.as_bytes()))
    } else {
        text.to_lowercase().contains(lowercase_query)
    }
}
