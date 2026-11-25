//! The Steam client "engine". It runs on a tokio runtime and talks to the rest
//! of the app through two plain message types:
//!
//! - [`Command`]: what the UI (or an extension) asks for.
//! - [`Event`]: what happened. Every event goes to one [`Sink`] callback; the
//!   desktop app fans it out to the UI state and to all extensions.
//!
//! [`State`] is a ready-made reducer that folds events into a snapshot the UI
//! can render, so extensions see exactly the same data as built-in pages.

pub mod download;
pub mod feed;
pub mod workshop;
pub mod hub;
pub mod read_cache;
pub mod ingame;
pub mod ipc;
mod launch;
#[cfg(not(windows))]
pub mod shaders;
mod installscript;
pub mod market;
pub mod profile_edit;
pub mod social;
pub mod cloud;
pub mod reviews;
pub mod discovery;
pub mod store_recs;
pub mod wishlist;
pub mod collections;
pub mod regions;
pub mod a2s;
pub mod game_ugc;
pub mod game_inventory;
pub mod game_servers;
mod input_config;
pub mod account;
pub mod chat_groups;
mod notify;
mod pads;
pub(crate) mod vpad;

/// One HTTP client for plain requests (one connection pool and TLS setup,
/// instead of a new client per request).
pub(crate) fn http() -> reqwest::Client {
    static HTTP: std::sync::LazyLock<reqwest::Client> = std::sync::LazyLock::new(reqwest::Client::new);
    HTTP.clone()
}
pub mod offline;
pub mod support;
mod pics;
mod session;
pub mod state;
pub mod store;

pub use auth::Guard;
pub use input_config::{
    ActionSetSummary, BindingChoice, BindingOption, BindingOrigin, BuiltInTemplate, CommunityCursor, CommunityLayout,
    CommunityPage, Control, InputConfigReport, LayoutDocument, LayoutId, LayoutRequest, Output, TemplateChoice,
    TemplateFileName, app_binding, app_bindings, binding_document, controller_key_name,
    inspect_local as inspect_input_config_local,
    managed_bindings_dir, managed_options, parse_vdf_summary, player_config_dir, read_layout, save_custom_layout,
    set_app_binding, set_app_bindings,
};
pub use pads::{
    ConnectedPadSummary, ConnectionKind, ControllerFamily, ControllerSettings, DetectedController, DetectionSnapshot,
    FamilyToggles, LauncherPadFrame, LauncherPadSample, LiveInput, PadCalibration, SteamInputDecision, TouchpadFinger,
    app_controller, connected_summary as connected_pad_summary, effective_controller_override, poll_ui as poll_controllers,
    refresh_ui as refresh_controllers, resolve_steam_input, set_app_controller, set_app_controllers,
    set_settings as set_controller_settings, settings as controller_settings, subscribe_launcher_pads,
    trigger_test_rumble, virtual_pad_available, virtual_pad_backend, virtual_pad_hint,
};
pub use launch::{LaunchArgs, LaunchMode, STEAM_DRM, acknowledge_anti_cheat, app_overlay_enabled, mode as launch_mode, mode_cached as launch_mode_cached, mode_peek as launch_mode_peek, overlay_enabled, proton_builds, proton_override, app_proton, app_protons, default_proton, native_build, NATIVE, DEFAULT_PROTON, safe_connect_args, set_app_overlay, set_app_proton, set_app_protons, set_anti_cheat_acknowledged, set_overlay_disabled_apps, set_overlay_enabled, set_overlay_hotkeys, set_prefer_official as launch_through_steam, set_proton_override, set_steam_setting, steam_setting, steam_settings_locked, SteamSetting, apply_steam_prefs, gameplay_downloads, set_gameplay_downloads, spawn_non_steam};
pub use state::State;
pub use store::SavedAccount;

use model::{
    AppId, ChatGroup, ChatGroupMember, ChatMessage, ChatRoomChannel, ChatRoomId, GroupBanInfo, GroupChatId, GroupInviteInfo, GroupInviteLink,
    GroupInviteRef, GroupMessage, OwnedApp, Persona, PersonaState, ProfileInfo, Relationship, SteamId,
};
use std::path::PathBuf;
use std::sync::Arc;
use tokio::sync::mpsc;

#[derive(Clone)]
pub enum Command {
    /// Log in with account name and password.
    LoginPassword { account: String, password: String },
    /// Log in by scanning a QR code with the Steam Mobile app.
    LoginQr,
    /// Log in with a refresh token saved from an earlier login.
    LoginSaved { account: String },
    /// Start a saved account in offline mode, from what was cached at its
    /// last sign-in (no network needed).
    StartOffline { account: String },
    /// Leave offline mode / reconnect now.
    GoOnline,
    /// Disconnect from Steam and keep going in offline mode.
    GoOffline,
    /// Answer a Steam Guard prompt.
    SubmitCode(String),
    /// Give up on the login in progress.
    CancelLogin,
    /// Remove a saved login from this machine.
    ForgetAccount { account: String },
    SetPersonaState(PersonaState),
    /// Start an installed game from its folder.
    Launch { app_id: AppId, install_dir: PathBuf, args: LaunchArgs },
    /// Start a game without the Steam Cloud check (after a conflict, "play anyway").
    LaunchWithoutCloud { app_id: AppId, install_dir: PathBuf, args: LaunchArgs },
    /// Stop compiling a game's shaders and start it now.
    SkipShaderCompile { app_id: AppId },
    /// Where Steam keeps `userdata`, and games with Steam Cloud turned off.
    CloudSettings { steam_root: Option<PathBuf>, disabled: Vec<AppId> },
    /// Sync a game's saves now. Conflicts listed in `keep_local` are uploaded,
    /// in `keep_cloud` downloaded. `then_launch` starts the game afterwards.
    CloudSync { app_id: AppId, install_dir: PathBuf, mode: cloud::Mode, keep_local: Vec<String>, keep_cloud: Vec<String>, then_launch: Option<LaunchArgs> },
    /// Delete files from a game's Steam Cloud (`CloudFile::raw` names),
    /// then report the cloud again.
    CloudDelete { app_id: AppId, install_dir: PathBuf, names: Vec<String> },
    /// Close a running game (and the processes it started).
    StopGame { app_id: AppId },
    /// For developers: let a game started by hand (a debugger, a test probe)
    /// use this session's Steam features. Answered with
    /// [`Event::GameAttached`]; nothing is reported to Steam as played.
    AttachGame { app_id: AppId, install_dir: PathBuf },
    /// The player accepted an invite (or "Join game") for a game that's
    /// running: hand it to the game like Steam does (GameLobbyJoinRequested_t
    /// for "+connect_lobby <id>", else GameRichPresenceJoinRequested_t).
    /// Start a game that isn't running with `connect` as its arguments instead.
    JoinRunningGame { app_id: AppId, friend: SteamId, connect: String },
    SendMessage { to: SteamId, text: String },
    /// Invite a friend into the running game's lobby: lets them in (private
    /// lobbies) and sends a game invite that joins it.
    InviteToLobby { app_id: AppId, lobby: u64, friend: SteamId },
    /// Send a friend request, or accept one you received.
    AddFriend { steam_id: SteamId },
    /// Remove a friend, or ignore a friend request.
    RemoveFriend { steam_id: SteamId },
    /// Set (or clear, with `None`) your nickname for someone.
    SetNickname { steam_id: SteamId, nickname: Option<String> },
    /// Block (or unblock) all communication with someone.
    BlockFriend { steam_id: SteamId, block: bool },
    /// Ask a game server about itself ("ip:port").
    QueryServer { addr: String },
    /// Tell a friend you're typing.
    Typing { to: SteamId },
    /// Fetch recent chat history with a friend.
    LoadHistory { friend: SteamId },
    /// Load all group chats you belong to.
    LoadChatGroups,
    /// Mark a group chat as active in this session and fetch its full state (channels, members, roles).
    OpenChatGroup { group_id: GroupChatId },
    /// Fetch message history for a group chat channel (`before`: `(server_timestamp, ordinal)` for older messages).
    LoadGroupHistory { group_id: GroupChatId, chat_id: ChatRoomId, before: Option<(u32, u32)> },
    SendGroupMessage { group_id: GroupChatId, chat_id: ChatRoomId, text: String },
    AckGroupMessage { group_id: GroupChatId, chat_id: ChatRoomId, timestamp: u32 },
    /// Create a new group chat with optional initial name and invited friends.
    CreateChatGroup { name: Option<String>, invitees: Vec<SteamId> },
    InviteToChatGroup { group_id: GroupChatId, chat_id: Option<ChatRoomId>, friend: SteamId },
    JoinChatGroup { group_id: Option<GroupChatId>, chat_id: Option<ChatRoomId>, invite_code: Option<String> },
    LeaveChatGroup { group_id: GroupChatId },
    SaveChatGroup { group_id: GroupChatId, name: String },
    RenameChatGroup { group_id: GroupChatId, name: String },
    SetChatGroupTagline { group_id: GroupChatId, tagline: String },
    CreateChatChannel { group_id: GroupChatId, name: String, allow_voice: bool },
    RenameChatChannel { group_id: GroupChatId, chat_id: ChatRoomId, name: String },
    DeleteChatChannel { group_id: GroupChatId, chat_id: ChatRoomId },
    CreateGroupInviteLink { group_id: GroupChatId, chat_id: Option<ChatRoomId>, seconds_valid: u32 },
    LoadGroupInviteLinks { group_id: GroupChatId },
    DeleteGroupInviteLink { group_id: GroupChatId, code: String },
    ResolveGroupInvite { invite: GroupInviteRef },
    KickGroupMember { group_id: GroupChatId, steam_id: SteamId, expiration_secs: i32 },
    BanGroupMember { group_id: GroupChatId, steam_id: SteamId, ban: bool },
    LoadGroupBanList { group_id: GroupChatId },
    UpdateGroupReaction { group_id: GroupChatId, chat_id: ChatRoomId, timestamp: u32, ordinal: u32, reaction_type: i32, reaction: String, add: bool },
    DeleteGroupMessages { group_id: GroupChatId, chat_id: ChatRoomId, messages: Vec<(u32, u32)> },
    SetGroupNotificationPrefs { group_id: GroupChatId, unread_muted: bool, desktop_level: Option<i32> },
    SearchGroupMembers { group_id: GroupChatId, query: String },
    /// Fetch someone's profile background, frame and bio.
    LoadProfile { steam_id: SteamId },
    /// Fetch one game's inventory for an account (753/6 is the Steam
    /// inventory: cards, backgrounds, emoticons). `language` is Steam's name.
    LoadInventory { steam_id: SteamId, app_id: AppId, context_id: u64, language: String },
    /// Send a trade offer (`token` from the partner's trade URL, for non-friends).
    SendTradeOffer { partner: SteamId, token: Option<String>, message: String, give: Vec<model::TradeAsset>, take: Vec<model::TradeAsset> },
    /// Find every game that has items in an account's inventory.
    LoadInventoryApps { steam_id: SteamId },
    /// Community Market price of an item in your wallet currency. `full` also
    /// reads the order book (highest buy order, cheapest listing in cents).
    MarketPrice { app_id: AppId, hash_name: String, currency: i32, country: String, full: bool },
    /// List an item on the Community Market. `receive` is what you get, in cents.
    /// Steam then asks for confirmation in the Steam Mobile app.
    SellItem { app_id: AppId, context_id: u64, asset_id: u64, amount: u64, receive: u64 },
    LoadMyListings,
    /// Your profile's editable details, privacy, owned items, themes, and featured badge.
    LoadProfileEdit { language: String },
    SaveProfileInfo(Box<model::ProfileEdit>),
    SaveProfilePrivacy(model::ProfilePrivacy),
    /// Upload a picture file as your avatar.
    UploadAvatar { path: std::path::PathBuf },
    /// Equip a profile item (0 background, 1 mini profile background, 2 avatar
    /// frame, 3 animated avatar); `id` 0 takes it off.
    EquipProfileItem { kind: u8, id: u64 },
    /// Steam's "Full Screen" vs "Original Size" flag for an equipped background.
    SetBackgroundFullScreen { id: u64, full: bool },
    /// Feature a community badge on your profile (0 takes it off).
    SetFavoriteBadge { badge: u32 },
    SetProfileTheme(String),
    /// Countries, a country's states, or a state's cities.
    LoadLocations { country: Option<String>, state: Option<String> },
    /// Your library collections from Steam (Favorites, Hidden, your own).
    LoadSteamCollections,
    /// Write library collections to Steam: `save` are created or replaced
    /// (by Steam id), `delete` removed.
    SaveSteamCollections { save: Vec<(String, collections::Entry)>, delete: Vec<String> },
    /// Comments on a profile (newest first), from `start`.
    LoadComments { steam_id: SteamId, start: u32 },
    /// Leave a comment on a profile.
    PostComment { steam_id: SteamId, text: String },
    /// Delete a comment (on your profile, or one you wrote).
    DeleteComment { steam_id: SteamId, comment_id: u64 },
    LoadBadges { steam_id: SteamId },
    /// A page (1-based) of a profile's reviews.
    LoadReviews { steam_id: SteamId, page: u32 },
    /// A game's reviews (Steam language name or "all"). `cursor`: the next
    /// page after the ones loaded (`AppReviews::cursor`); `None` for the first.
    LoadAppReviews { app_id: AppId, language: String, cursor: Option<String>, filter: model::ReviewFilter },
    /// Your store discovery queue (`rebuild` skips the current one and asks Steam for a fresh dozen).
    LoadDiscoveryQueue { country: String, rebuild: bool },
    /// Load your Steam Points Shop balance.
    LoadPointsBalance,
    /// Mark a game in your discovery queue as seen ("Next in queue").
    SkipDiscoveryQueueItem { app_id: AppId },
    /// "Not interested" (or undo it) on a store game.
    IgnoreApp { app_id: AppId, ignore: bool },
    /// Follow or unfollow a game's store hub.
    FollowApp { app_id: AppId, follow: bool },
    /// Your own review of a game, if any.
    LoadMyReview { app_id: AppId },
    /// Write or replace your review of a game.
    PostReview { app_id: AppId, text: String, recommended: bool, public: bool, allow_comments: bool, language: String },
    DeleteReview { app_id: AppId },
    /// The store's personal picks (discovery queues, friends, tags).
    LoadStoreRecommendations { language: String },
    /// The groups a profile belongs to.
    LoadGroups { steam_id: SteamId },
    /// A game's trading-card collection on a profile (`foil`: the foil set).
    LoadCardSet { steam_id: SteamId, app_id: AppId, foil: bool },
    LoadNotifications,
    /// Mark notifications as read (empty = all).
    MarkNotificationsRead { ids: Vec<u64> },
    /// Your Steam Support requests.
    LoadSupportTickets,
    /// The messages in one support request.
    LoadSupportTicket { id: String, url: String },
    /// Steam Guard, authenticator, phone, remembered devices (read only).
    LoadAccountSecurity,
    /// Purchase history: the first page, or the next after `cursor`.
    LoadPurchases { language: String, cursor: Option<String> },
    /// Licenses and product key activations.
    LoadLicenses { language: String },
    /// Your Steam Family and its shared library.
    LoadFamily { language: String },
    /// Create a new Steam Family with you as its first adult member.
    CreateFamily { name: String, language: String },
    /// Rename your Steam Family (adult role required).
    RenameFamily { family_id: u64, name: String, language: String },
    /// Invite a Steam user to your family.
    InviteToFamily { family_id: u64, steam_id: SteamId, role: model::FamilyRole, language: String },
    /// Cancel a pending outgoing invite, or decline an incoming invite (`steam_id == me`).
    CancelFamilyInvite { family_id: u64, steam_id: SteamId, language: String },
    /// Resend a pending family invitation.
    ResendFamilyInvite { family_id: u64, steam_id: SteamId },
    /// Accept a pending invitation and join a Steam Family.
    JoinFamily { family_id: u64, language: String },
    /// Remove a member from the family, or leave the family (`steam_id == me`).
    RemoveFromFamily { family_id: u64, steam_id: SteamId, language: String },
    /// Delete a Steam Family (adult role required).
    DeleteFamily { family_id: u64, language: String },
    CancelListing { listing_id: u64 },
    CancelBuyOrder { order_id: u64 },
    /// Fetch the games on someone's profile with their playtime.
    LoadPlayerGames { steam_id: SteamId, language: String },
    /// Your achievement progress (unlocked / total) for these games.
    LoadAchievementProgress { app_ids: Vec<AppId> },
    /// A game's achievements with which ones you've unlocked.
    LoadAchievements { app_id: AppId, language: String },
    /// Redeem a product key (CD key).
    ActivateKey { key: String },
    /// Install or update a game (queued; one download runs at a time).
    Install(download::InstallRequest),
    PauseDownload { app_id: AppId },
    ResumeDownload { app_id: AppId },
    CancelDownload { app_id: AppId },
    /// Put the queued downloads in this order (games left out keep their
    /// place after the ones listed). The running download isn't touched.
    ReorderDownloads(Vec<AppId>),
    /// Whether downloads may resume by themselves on startup ("Automatic
    /// updates"). Sent at launch and whenever the setting changes.
    SetAutoDownloads(bool),
    /// Measure every download region from this PC.
    ProbeRegions,
    /// Download from this Steam region (`None` = automatic), or prefer one
    /// content host (a partner CDN such as Akamai).
    DownloadRegion { cell_id: Option<u32>, host: Option<String> },
    /// Speed limit (KB/s, 0 = none) and parallel chunk downloads.
    DownloadLimits { limit_kbps: u64, concurrency: u32 },
    /// Unsubscribe from a workshop item (Steam removes it from the game).
    UnsubscribeWorkshop { app_id: AppId, file_id: u64 },
    /// Browse a game's Workshop. `query_type` is Steam's
    /// EPublishedFileQueryType (0 top rated, 1 newest, 3 trending, 9 most subscribed, 12 text search).
    WorkshopQuery { request_id: u64, app_id: AppId, query_type: u32, page: u32, search: String, days: u32, tags: Vec<String> },
    WorkshopSubscribe { app_id: AppId, file_id: u64 },
    /// Check an installed game's files against Steam's manifests, shown on
    /// the Downloads page like Steam's "Verify integrity of game files".
    VerifyGame { game: model::InstalledGame, steam_root: PathBuf },
    CancelVerify { app_id: AppId },
    /// Move an installed game (and its manifest / workshop content) to another library folder.
    MoveGame { game: model::InstalledGame, dest_library: PathBuf },
    CancelMove { app_id: AppId },
    /// Back up an installed game's manifest and files into `dest_dir`.
    BackupGame { game: model::InstalledGame, dest_dir: PathBuf },
    CancelBackup { app_id: AppId },
    /// Restore a game backup (Aurora or standard Steam `.acf` backup) into `dest_library`.
    RestoreGame { backup_dir: PathBuf, dest_library: PathBuf },
    CancelRestore,
    /// Add a free-to-play game to the account, like Steam's "Play Game" /
    /// "Add to Library" on a free game's store page.
    RequestFreeLicense { app_id: AppId },
    /// Download every subscribed Workshop item of an installed game that
    /// is missing or outdated (also done before each launch, like Steam).
    SyncWorkshop { app_id: AppId },
    /// Inspect Steam Input controller configurations (local + Workshop + official) for a game.
    InspectInputConfig { key: LayoutRequest, install_dir: PathBuf },
    QueryControllerLayouts { key: LayoutRequest, cursor: Option<CommunityCursor> },
    DownloadControllerLayout { key: LayoutRequest, id: u64 },
    /// The game's own Workshop tag groups (for filters).
    LoadWorkshopTags { app_id: AppId, language: String },
    LoadWishlist,
    WishlistAdd { app_id: AppId },
    WishlistRemove { app_id: AppId },
    /// Put wishlisted `app_id` at `to` (0-based in "Your rank" order).
    WishlistMove { app_id: AppId, to: usize },
    LoadCart,
    /// Add a package to the cart, optionally as a gift for a friend.
    /// Add a copy to the cart: for this account, or as a gift (optionally
    /// already for a friend).
    CartAdd { package_id: u32, gift: bool, gift_to: Option<SteamId> },
    /// Switch a cart line between "For my account" and "This is a gift"
    /// (optionally for a friend), like the dropdown in Steam's cart.
    CartModify { line_item_id: u64, gift: bool, gift_to: Option<SteamId> },
    CartRemove { line_item_id: u64 },
    /// Community pages: Workshop items, guides, discussions, hub, achievements.
    Hub(hub::Request),
    LogOut,
}

impl std::fmt::Debug for Command {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Command::LoginPassword { account, .. } => f.debug_struct("LoginPassword").field("account", account).field("password", &"[REDACTED]").finish(),
            Command::LoginQr => f.write_str("LoginQr"),
            Command::LoginSaved { account } => f.debug_struct("LoginSaved").field("account", account).finish(),
            Command::StartOffline { account } => f.debug_struct("StartOffline").field("account", account).finish(),
            Command::GoOnline => f.write_str("GoOnline"),
            Command::GoOffline => f.write_str("GoOffline"),
            Command::SubmitCode(_) => f.debug_tuple("SubmitCode").field(&"[REDACTED]").finish(),
            Command::CancelLogin => f.write_str("CancelLogin"),
            Command::ForgetAccount { account } => f.debug_struct("ForgetAccount").field("account", account).finish(),
            Command::SetPersonaState(s) => f.debug_tuple("SetPersonaState").field(s).finish(),
            Command::Launch { app_id, install_dir, args } => f.debug_struct("Launch").field("app_id", app_id).field("install_dir", install_dir).field("args", args).finish(),
            Command::LaunchWithoutCloud { app_id, install_dir, args } => f.debug_struct("LaunchWithoutCloud").field("app_id", app_id).field("install_dir", install_dir).field("args", args).finish(),
            Command::SkipShaderCompile { app_id } => f.debug_struct("SkipShaderCompile").field("app_id", app_id).finish(),
            Command::CloudSettings { steam_root, disabled } => f.debug_struct("CloudSettings").field("steam_root", steam_root).field("disabled", disabled).finish(),
            Command::CloudSync { app_id, install_dir, mode, keep_local, keep_cloud, then_launch } => f.debug_struct("CloudSync").field("app_id", app_id).field("install_dir", install_dir).field("mode", mode).field("keep_local", keep_local).field("keep_cloud", keep_cloud).field("then_launch", then_launch).finish(),
            Command::CloudDelete { app_id, install_dir, names } => f.debug_struct("CloudDelete").field("app_id", app_id).field("install_dir", install_dir).field("names", names).finish(),
            Command::StopGame { app_id } => f.debug_struct("StopGame").field("app_id", app_id).finish(),
            Command::AttachGame { app_id, install_dir } => f.debug_struct("AttachGame").field("app_id", app_id).field("install_dir", install_dir).finish(),
            Command::JoinRunningGame { app_id, friend, connect } => f.debug_struct("JoinRunningGame").field("app_id", app_id).field("friend", friend).field("connect", connect).finish(),
            Command::SendMessage { to, text } => f.debug_struct("SendMessage").field("to", to).field("text", text).finish(),
            Command::InviteToLobby { app_id, lobby, friend } => f.debug_struct("InviteToLobby").field("app_id", app_id).field("lobby", lobby).field("friend", friend).finish(),
            Command::AddFriend { steam_id } => f.debug_struct("AddFriend").field("steam_id", steam_id).finish(),
            Command::RemoveFriend { steam_id } => f.debug_struct("RemoveFriend").field("steam_id", steam_id).finish(),
            Command::SetNickname { steam_id, nickname } => f.debug_struct("SetNickname").field("steam_id", steam_id).field("nickname", nickname).finish(),
            Command::BlockFriend { steam_id, block } => f.debug_struct("BlockFriend").field("steam_id", steam_id).field("block", block).finish(),
            Command::QueryServer { addr } => f.debug_struct("QueryServer").field("addr", addr).finish(),
            Command::Typing { to } => f.debug_struct("Typing").field("to", to).finish(),
            Command::LoadHistory { friend } => f.debug_struct("LoadHistory").field("friend", friend).finish(),
            Command::LoadChatGroups => f.write_str("LoadChatGroups"),
            Command::OpenChatGroup { group_id } => f.debug_struct("OpenChatGroup").field("group_id", group_id).finish(),
            Command::LoadGroupHistory { group_id, chat_id, before } => f.debug_struct("LoadGroupHistory").field("group_id", group_id).field("chat_id", chat_id).field("before", before).finish(),
            Command::SendGroupMessage { group_id, chat_id, text } => f.debug_struct("SendGroupMessage").field("group_id", group_id).field("chat_id", chat_id).field("text", text).finish(),
            Command::AckGroupMessage { group_id, chat_id, timestamp } => f.debug_struct("AckGroupMessage").field("group_id", group_id).field("chat_id", chat_id).field("timestamp", timestamp).finish(),
            Command::CreateChatGroup { name, invitees } => f.debug_struct("CreateChatGroup").field("name", name).field("invitees", invitees).finish(),
            Command::InviteToChatGroup { group_id, chat_id, friend } => f.debug_struct("InviteToChatGroup").field("group_id", group_id).field("chat_id", chat_id).field("friend", friend).finish(),
            Command::JoinChatGroup { group_id, chat_id, invite_code } => f.debug_struct("JoinChatGroup").field("group_id", group_id).field("chat_id", chat_id).field("invite_code", invite_code).finish(),
            Command::LeaveChatGroup { group_id } => f.debug_struct("LeaveChatGroup").field("group_id", group_id).finish(),
            Command::SaveChatGroup { group_id, name } => f.debug_struct("SaveChatGroup").field("group_id", group_id).field("name", name).finish(),
            Command::RenameChatGroup { group_id, name } => f.debug_struct("RenameChatGroup").field("group_id", group_id).field("name", name).finish(),
            Command::SetChatGroupTagline { group_id, tagline } => f.debug_struct("SetChatGroupTagline").field("group_id", group_id).field("tagline", tagline).finish(),
            Command::CreateChatChannel { group_id, name, allow_voice } => f.debug_struct("CreateChatChannel").field("group_id", group_id).field("name", name).field("allow_voice", allow_voice).finish(),
            Command::RenameChatChannel { group_id, chat_id, name } => f.debug_struct("RenameChatChannel").field("group_id", group_id).field("chat_id", chat_id).field("name", name).finish(),
            Command::DeleteChatChannel { group_id, chat_id } => f.debug_struct("DeleteChatChannel").field("group_id", group_id).field("chat_id", chat_id).finish(),
            Command::CreateGroupInviteLink { group_id, chat_id, seconds_valid } => f.debug_struct("CreateGroupInviteLink").field("group_id", group_id).field("chat_id", chat_id).field("seconds_valid", seconds_valid).finish(),
            Command::LoadGroupInviteLinks { group_id } => f.debug_struct("LoadGroupInviteLinks").field("group_id", group_id).finish(),
            Command::DeleteGroupInviteLink { group_id, code } => f.debug_struct("DeleteGroupInviteLink").field("group_id", group_id).field("code", code).finish(),
            Command::ResolveGroupInvite { invite } => f.debug_struct("ResolveGroupInvite").field("invite", invite).finish(),
            Command::KickGroupMember { group_id, steam_id, expiration_secs } => f.debug_struct("KickGroupMember").field("group_id", group_id).field("steam_id", steam_id).field("expiration_secs", expiration_secs).finish(),
            Command::BanGroupMember { group_id, steam_id, ban } => f.debug_struct("BanGroupMember").field("group_id", group_id).field("steam_id", steam_id).field("ban", ban).finish(),
            Command::LoadGroupBanList { group_id } => f.debug_struct("LoadGroupBanList").field("group_id", group_id).finish(),
            Command::UpdateGroupReaction { group_id, chat_id, timestamp, ordinal, reaction_type, reaction, add } => f.debug_struct("UpdateGroupReaction").field("group_id", group_id).field("chat_id", chat_id).field("timestamp", timestamp).field("ordinal", ordinal).field("reaction_type", reaction_type).field("reaction", reaction).field("add", add).finish(),
            Command::DeleteGroupMessages { group_id, chat_id, messages } => f.debug_struct("DeleteGroupMessages").field("group_id", group_id).field("chat_id", chat_id).field("messages", messages).finish(),
            Command::SetGroupNotificationPrefs { group_id, unread_muted, desktop_level } => f.debug_struct("SetGroupNotificationPrefs").field("group_id", group_id).field("unread_muted", unread_muted).field("desktop_level", desktop_level).finish(),
            Command::SearchGroupMembers { group_id, query } => f.debug_struct("SearchGroupMembers").field("group_id", group_id).field("query", query).finish(),
            Command::LoadProfile { steam_id } => f.debug_struct("LoadProfile").field("steam_id", steam_id).finish(),
            Command::LoadInventory { steam_id, app_id, context_id, language } => f.debug_struct("LoadInventory").field("steam_id", steam_id).field("app_id", app_id).field("context_id", context_id).field("language", language).finish(),
            Command::SendTradeOffer { partner, token, message, give, take } => f.debug_struct("SendTradeOffer").field("partner", partner).field("token", &token.as_ref().map(|_| "[REDACTED]")).field("message", message).field("give", give).field("take", take).finish(),
            Command::LoadInventoryApps { steam_id } => f.debug_struct("LoadInventoryApps").field("steam_id", steam_id).finish(),
            Command::MarketPrice { app_id, hash_name, currency, country, full } => f.debug_struct("MarketPrice").field("app_id", app_id).field("hash_name", hash_name).field("currency", currency).field("country", country).field("full", full).finish(),
            Command::SellItem { app_id, context_id, asset_id, amount, receive } => f.debug_struct("SellItem").field("app_id", app_id).field("context_id", context_id).field("asset_id", asset_id).field("amount", amount).field("receive", receive).finish(),
            Command::LoadMyListings => f.write_str("LoadMyListings"),
            Command::LoadProfileEdit { language } => f.debug_struct("LoadProfileEdit").field("language", language).finish(),
            Command::SaveProfileInfo(_) => f.write_str("SaveProfileInfo"),
            Command::SaveProfilePrivacy(p) => f.debug_tuple("SaveProfilePrivacy").field(p).finish(),
            Command::UploadAvatar { path } => f.debug_struct("UploadAvatar").field("path", path).finish(),
            Command::EquipProfileItem { kind, id } => f.debug_struct("EquipProfileItem").field("kind", kind).field("id", id).finish(),
            Command::SetBackgroundFullScreen { id, full } => f.debug_struct("SetBackgroundFullScreen").field("id", id).field("full", full).finish(),
            Command::SetFavoriteBadge { badge } => f.debug_struct("SetFavoriteBadge").field("badge", badge).finish(),
            Command::SetProfileTheme(t) => f.debug_tuple("SetProfileTheme").field(t).finish(),
            Command::LoadLocations { country, state } => f.debug_struct("LoadLocations").field("country", country).field("state", state).finish(),
            Command::LoadSteamCollections => f.write_str("LoadSteamCollections"),
            Command::SaveSteamCollections { save, delete } => f.debug_struct("SaveSteamCollections").field("save", save).field("delete", delete).finish(),
            Command::LoadComments { steam_id, start } => f.debug_struct("LoadComments").field("steam_id", steam_id).field("start", start).finish(),
            Command::PostComment { steam_id, text } => f.debug_struct("PostComment").field("steam_id", steam_id).field("text", text).finish(),
            Command::DeleteComment { steam_id, comment_id } => f.debug_struct("DeleteComment").field("steam_id", steam_id).field("comment_id", comment_id).finish(),
            Command::LoadBadges { steam_id } => f.debug_struct("LoadBadges").field("steam_id", steam_id).finish(),
            Command::LoadReviews { steam_id, page } => f.debug_struct("LoadReviews").field("steam_id", steam_id).field("page", page).finish(),
            Command::LoadAppReviews { app_id, language, cursor, filter } => {
                f.debug_struct("LoadAppReviews").field("app_id", app_id).field("language", language).field("cursor", cursor).field("filter", filter).finish()
            }
            Command::LoadDiscoveryQueue { country, rebuild } => f.debug_struct("LoadDiscoveryQueue").field("country", country).field("rebuild", rebuild).finish(),
            Command::LoadPointsBalance => write!(f, "LoadPointsBalance"),
            Command::SkipDiscoveryQueueItem { app_id } => f.debug_struct("SkipDiscoveryQueueItem").field("app_id", app_id).finish(),
            Command::IgnoreApp { app_id, ignore } => f.debug_struct("IgnoreApp").field("app_id", app_id).field("ignore", ignore).finish(),
            Command::FollowApp { app_id, follow } => f.debug_struct("FollowApp").field("app_id", app_id).field("follow", follow).finish(),
            Command::LoadMyReview { app_id } => f.debug_struct("LoadMyReview").field("app_id", app_id).finish(),
            Command::PostReview { app_id, text, recommended, public, allow_comments, language } => f.debug_struct("PostReview").field("app_id", app_id).field("text", text).field("recommended", recommended).field("public", public).field("allow_comments", allow_comments).field("language", language).finish(),
            Command::DeleteReview { app_id } => f.debug_struct("DeleteReview").field("app_id", app_id).finish(),
            Command::LoadStoreRecommendations { language } => f.debug_struct("LoadStoreRecommendations").field("language", language).finish(),
            Command::LoadGroups { steam_id } => f.debug_struct("LoadGroups").field("steam_id", steam_id).finish(),
            Command::LoadCardSet { steam_id, app_id, foil } => f.debug_struct("LoadCardSet").field("steam_id", steam_id).field("app_id", app_id).field("foil", foil).finish(),
            Command::LoadNotifications => f.write_str("LoadNotifications"),
            Command::MarkNotificationsRead { ids } => f.debug_struct("MarkNotificationsRead").field("ids", ids).finish(),
            Command::LoadSupportTickets => f.write_str("LoadSupportTickets"),
            Command::LoadSupportTicket { id, url } => f.debug_struct("LoadSupportTicket").field("id", id).field("url", url).finish(),
            Command::LoadAccountSecurity => f.write_str("LoadAccountSecurity"),
            Command::LoadPurchases { language, cursor } => f.debug_struct("LoadPurchases").field("language", language).field("cursor", cursor).finish(),
            Command::LoadLicenses { language } => f.debug_struct("LoadLicenses").field("language", language).finish(),
            Command::LoadFamily { language } => f.debug_struct("LoadFamily").field("language", language).finish(),
            Command::CreateFamily { name, language } => f.debug_struct("CreateFamily").field("name", name).field("language", language).finish(),
            Command::RenameFamily { family_id, name, language } => f.debug_struct("RenameFamily").field("family_id", family_id).field("name", name).field("language", language).finish(),
            Command::InviteToFamily { family_id, steam_id, role, language } => {
                f.debug_struct("InviteToFamily").field("family_id", family_id).field("steam_id", steam_id).field("role", role).field("language", language).finish()
            }
            Command::CancelFamilyInvite { family_id, steam_id, language } => {
                f.debug_struct("CancelFamilyInvite").field("family_id", family_id).field("steam_id", steam_id).field("language", language).finish()
            }
            Command::ResendFamilyInvite { family_id, steam_id } => f.debug_struct("ResendFamilyInvite").field("family_id", family_id).field("steam_id", steam_id).finish(),
            Command::JoinFamily { family_id, language } => f.debug_struct("JoinFamily").field("family_id", family_id).field("language", language).finish(),
            Command::RemoveFromFamily { family_id, steam_id, language } => {
                f.debug_struct("RemoveFromFamily").field("family_id", family_id).field("steam_id", steam_id).field("language", language).finish()
            }
            Command::DeleteFamily { family_id, language } => f.debug_struct("DeleteFamily").field("family_id", family_id).field("language", language).finish(),
            Command::CancelListing { listing_id } => f.debug_struct("CancelListing").field("listing_id", listing_id).finish(),
            Command::CancelBuyOrder { order_id } => f.debug_struct("CancelBuyOrder").field("order_id", order_id).finish(),
            Command::LoadPlayerGames { steam_id, language } => f.debug_struct("LoadPlayerGames").field("steam_id", steam_id).field("language", language).finish(),
            Command::LoadAchievementProgress { app_ids } => f.debug_struct("LoadAchievementProgress").field("app_ids", app_ids).finish(),
            Command::LoadAchievements { app_id, language } => f.debug_struct("LoadAchievements").field("app_id", app_id).field("language", language).finish(),
            Command::ActivateKey { .. } => f.debug_struct("ActivateKey").field("key", &"[REDACTED]").finish(),
            Command::Install(req) => f.debug_tuple("Install").field(req).finish(),
            Command::PauseDownload { app_id } => f.debug_struct("PauseDownload").field("app_id", app_id).finish(),
            Command::ResumeDownload { app_id } => f.debug_struct("ResumeDownload").field("app_id", app_id).finish(),
            Command::CancelDownload { app_id } => f.debug_struct("CancelDownload").field("app_id", app_id).finish(),
            Command::ReorderDownloads(ids) => f.debug_tuple("ReorderDownloads").field(ids).finish(),
            Command::SetAutoDownloads(on) => f.debug_tuple("SetAutoDownloads").field(on).finish(),
            Command::ProbeRegions => f.write_str("ProbeRegions"),
            Command::DownloadRegion { cell_id, host } => f.debug_struct("DownloadRegion").field("cell_id", cell_id).field("host", host).finish(),
            Command::DownloadLimits { limit_kbps, concurrency } => f.debug_struct("DownloadLimits").field("limit_kbps", limit_kbps).field("concurrency", concurrency).finish(),
            Command::UnsubscribeWorkshop { app_id, file_id } => f.debug_struct("UnsubscribeWorkshop").field("app_id", app_id).field("file_id", file_id).finish(),
            Command::WorkshopQuery { request_id, app_id, query_type, page, search, days, tags } => f.debug_struct("WorkshopQuery").field("request_id", request_id).field("app_id", app_id).field("query_type", query_type).field("page", page).field("search", search).field("days", days).field("tags", tags).finish(),
            Command::WorkshopSubscribe { app_id, file_id } => f.debug_struct("WorkshopSubscribe").field("app_id", app_id).field("file_id", file_id).finish(),
            Command::VerifyGame { game, steam_root } => f.debug_struct("VerifyGame").field("game", game).field("steam_root", steam_root).finish(),
            Command::CancelVerify { app_id } => f.debug_struct("CancelVerify").field("app_id", app_id).finish(),
            Command::MoveGame { game, dest_library } => f.debug_struct("MoveGame").field("app_id", &game.app_id).field("dest_library", dest_library).finish(),
            Command::CancelMove { app_id } => f.debug_struct("CancelMove").field("app_id", app_id).finish(),
            Command::BackupGame { game, dest_dir } => f.debug_struct("BackupGame").field("app_id", &game.app_id).field("dest_dir", dest_dir).finish(),
            Command::CancelBackup { app_id } => f.debug_struct("CancelBackup").field("app_id", app_id).finish(),
            Command::RestoreGame { backup_dir, dest_library } => f.debug_struct("RestoreGame").field("backup_dir", backup_dir).field("dest_library", dest_library).finish(),
            Command::CancelRestore => f.write_str("CancelRestore"),
            Command::RequestFreeLicense { app_id } => f.debug_struct("RequestFreeLicense").field("app_id", app_id).finish(),
            Command::SyncWorkshop { app_id } => f.debug_struct("SyncWorkshop").field("app_id", app_id).finish(),
            Command::InspectInputConfig { key, .. } => f.debug_tuple("InspectInputConfig").field(key).finish(),
            Command::QueryControllerLayouts { key, .. } => f.debug_tuple("QueryControllerLayouts").field(key).finish(),
            Command::DownloadControllerLayout { key, id } => f.debug_tuple("DownloadControllerLayout").field(key).field(id).finish(),
            Command::LoadWorkshopTags { app_id, language } => f.debug_struct("LoadWorkshopTags").field("app_id", app_id).field("language", language).finish(),
            Command::LoadWishlist => f.write_str("LoadWishlist"),
            Command::WishlistAdd { app_id } => f.debug_struct("WishlistAdd").field("app_id", app_id).finish(),
            Command::WishlistRemove { app_id } => f.debug_struct("WishlistRemove").field("app_id", app_id).finish(),
            Command::WishlistMove { app_id, to } => f.debug_struct("WishlistMove").field("app_id", app_id).field("to", to).finish(),
            Command::LoadCart => f.write_str("LoadCart"),
            Command::CartAdd { package_id, gift, gift_to } => f.debug_struct("CartAdd").field("package_id", package_id).field("gift", gift).field("gift_to", gift_to).finish(),
            Command::CartModify { line_item_id, gift, gift_to } => f.debug_struct("CartModify").field("line_item_id", line_item_id).field("gift", gift).field("gift_to", gift_to).finish(),
            Command::CartRemove { line_item_id } => f.debug_struct("CartRemove").field("line_item_id", line_item_id).finish(),
            Command::Hub(hub::Request::PostStatus { app_id, .. }) => f.debug_struct("HubPostStatus").field("app_id", app_id).finish_non_exhaustive(),
            Command::Hub(req) => f.debug_tuple("Hub").field(req).finish(),
            Command::LogOut => f.write_str("LogOut"),
        }
    }
}

#[derive(Debug, Clone)]
pub enum Event {
    Connecting,
    /// Show this URL as a QR code (it may be replaced by a newer one).
    QrChallenge { url: String },
    /// Steam Guard needs the user. `guards` lists allowed options, preferred first.
    GuardRequired { guards: Vec<Guard> },
    GuardCodeRejected(String),
    LoginFailed(String),
    /// A saved login couldn't reach Steam (no network, servers down). The
    /// login itself is fine; offline mode is possible if `offline_ready`.
    ConnectionError { account: String, message: String, offline_ready: bool },
    /// The connection to Steam changed (online, offline mode, reconnecting).
    Link(Link),
    /// Play sessions recorded while not connected: (game, seconds in all,
    /// seconds not on Steam yet). Replaces the earlier list.
    OfflinePlaytime(Vec<(AppId, u64, u64)>),
    /// Games whose Steam Cloud upload waits for a connection (full list).
    CloudPending(Vec<AppId>),
    /// A game started offline without its Steam Cloud download.
    CloudSkipped { app_id: AppId },
    LoggedIn { steam_id: SteamId, account: String },
    AccountInfo { persona_name: String, country: String },
    /// Number of licenses; owned apps follow in [`Event::Apps`].
    Licenses { count: usize },
    Apps(Vec<OwnedApp>),
    /// Full friends list (replaces any earlier one).
    Friends(Vec<(SteamId, Relationship)>),
    /// Personas whose details changed (friends and yourself).
    Personas(Vec<Persona>),
    /// Names of games outside your library (e.g. what friends are playing).
    AppNames(Vec<(AppId, String)>),
    /// A game has been asked to launch and is preparing (workshop/cloud/host startup).
    GameLaunching { app_id: AppId },
    GameStarted { app_id: AppId },
    /// The `STEAMINRUST_IPC` value for a game started by hand ([`Command::AttachGame`]).
    GameAttached { app_id: AppId, env: String },
    /// A game asked for Steam's overlay (see `ipc::protocol::Request::Overlay`
    /// for `dialog` and `target`): Aurora's overlay should open there.
    GameOverlay { app_id: AppId, dialog: String, target: String },
    /// Launch paused before a detected anti-cheat game uses the headless host.
    AntiCheatWarning { app_id: AppId, install_dir: PathBuf, args: LaunchArgs, skip_cloud: bool },
    /// How a game is being started.
    LaunchMode { app_id: AppId, mode: LaunchMode },
    /// A game's shaders are compiling before it starts (`done` of `total`
    /// pipelines; `total` is 0 until Fossilize has counted them).
    ShaderCompile { app_id: AppId, done: u32, total: u32 },
    /// Shader compiling ended (finished or skipped); the game starts next.
    ShaderCompileDone { app_id: AppId },
    /// The game's install script is running (first launch): the step's
    /// name, or `None` once it's done and the game starts next.
    InstallScript { app_id: AppId, step: Option<String> },
    /// Steam Cloud is syncing a game's saves (before launch or after exit).
    CloudSyncing { app_id: AppId },
    /// Result of a Steam Cloud sync. `held_launch` is set when the game
    /// wasn't started because of conflicts (its launch arguments).
    CloudReport { report: model::CloudReport, held_launch: Option<LaunchArgs> },
    GameStopped { app_id: AppId },
    /// A game was stopped because its Steam DRM refused Aurora's
    /// steamclient; Play it again (it now starts on Valve's engine).
    RelaunchForSteamDrm { app_id: AppId },
    /// The running game entered (`Some`) or left a matchmaking lobby, so
    /// friends can be invited to it.
    GameLobby { app_id: AppId, lobby: Option<u64> },
    /// The running game's join string (rich presence `connect`), so friends
    /// can be invited to the game itself.
    GameConnect { app_id: AppId, connect: Option<String> },
    /// A chat message, incoming or sent by you (`from == your SteamID`).
    Message(ChatMessage),
    Typing { from: SteamId },
    History { friend: SteamId, messages: Vec<ChatMessage> },
    ChatGroups(Vec<ChatGroup>),
    ChatGroupUpdated(ChatGroup),
    GroupHeaderUpdated(ChatGroup),
    GroupMembersUpdated { group_id: GroupChatId, members: Vec<ChatGroupMember>, total: Option<u32>, virtualized: bool },
    ChatGroupRemoved(GroupChatId),
    GroupJoined(GroupChatId),
    GroupHistory { group_id: GroupChatId, chat_id: ChatRoomId, prepend: bool, more: bool, messages: Vec<GroupMessage> },
    GroupMessage(GroupMessage),
    GroupMessageDeleted { group_id: GroupChatId, chat_id: ChatRoomId, messages: Vec<(u32, u32)> },
    GroupMessageReaction { group_id: GroupChatId, chat_id: ChatRoomId, timestamp: u32, ordinal: u32, reaction_type: i32, reaction: String, reactor: SteamId, is_add: bool },
    GroupAck { group_id: GroupChatId, chat_id: ChatRoomId, timestamp: u32 },
    GroupMemberChanged { group_id: GroupChatId, member: ChatGroupMember, change: i32 },
    GroupRoomsChanged { group_id: GroupChatId, default_chat_id: ChatRoomId, rooms: Vec<ChatRoomChannel> },
    GroupInviteResolved { key: String, result: Result<GroupInviteInfo, String> },
    GroupInviteLinks { group_id: GroupChatId, result: Result<Vec<GroupInviteLink>, String> },
    GroupBanList { group_id: GroupChatId, result: Result<Vec<GroupBanInfo>, String> },
    GroupMemberSearch { group_id: GroupChatId, results: Vec<Persona> },
    /// Your Steam level.
    Level(u32),
    /// Steam Wallet balance in cents, with Steam's currency code.
    /// `pending` is money Steam hasn't released yet (recent Market sales, refunds).
    Wallet { balance: i64, currency: i32, pending: i64 },
    Profile(ProfileInfo),
    /// Your account's email address (shown in Settings → Account).
    Email { address: String, validated: bool },
    Inventory(model::Inventory),
    MarketPrice { app_id: AppId, hash_name: String, result: Result<model::MarketPrice, String> },
    /// Result of [`Command::SellItem`]: `Ok(true)` = needs confirming on your phone.
    ItemListed { asset_id: u64, result: Result<bool, String> },
    MyListings(Result<model::MyListings, String>),
    ProfileEdit(Result<model::ProfileEdit, String>),
    ProfileSaved { what: model::ProfileSave, result: Result<(), String> },
    /// (code, name) for [`Command::LoadLocations`].
    Locations { country: Option<String>, state: Option<String>, result: Result<Vec<(String, String)>, String> },
    SteamCollections(Result<Vec<model::SteamCollection>, String>),
    /// A `SaveSteamCollections` finished (the same lists, so the library
    /// knows what Steam now has).
    SteamCollectionsSaved { save: Vec<(String, collections::Entry)>, delete: Vec<String>, result: Result<(), String> },
    /// Comments on a profile, or why they can't be shown (private, turned off).
    Comments { steam_id: SteamId, result: Result<model::Comments, String> },
    CommentPosted { steam_id: SteamId, result: Result<(), String> },
    ServerInfo { addr: String, result: Result<crate::a2s::ServerInfo, String> },
    Blocked { steam_id: SteamId, result: Result<bool, String> },
    Badges { steam_id: SteamId, result: Result<model::Badges, String> },
    Reviews { steam_id: SteamId, page: u32, result: Result<model::UserReviews, String> },
    /// A page of a game's reviews (`more`: added after the loaded ones).
    AppReviews { app_id: AppId, filter: model::ReviewFilter, more: bool, result: Result<model::AppReviews, String> },
    /// Your store discovery queue (app ids, next to see first).
    DiscoveryQueue(Result<Vec<AppId>, String>),
    /// Your Steam Points Shop balance.
    PointsBalance(Result<u64, String>),
    /// "Not interested" was saved (or reverted if `result.is_err()`).
    AppIgnored { app_id: AppId, ignored: bool, result: Result<(), String> },
    /// Games you follow on Steam.
    FollowedApps(Vec<AppId>),
    /// Follow/unfollow finished (`result` holds the new followed state on `Ok`).
    AppFollowed { app_id: AppId, result: Result<bool, String> },
    MyReview { app_id: AppId, result: Result<Option<model::UserReview>, String> },
    /// A review was posted (`deleted`: removed) or failed.
    ReviewSaved { app_id: AppId, deleted: bool, result: Result<(), String> },
    StoreRecommendations(model::StoreRecommendations),
    TradeOfferSent { partner: SteamId, result: Result<model::TradeOfferSent, String> },
    Groups { steam_id: SteamId, result: Result<Vec<model::UserGroup>, String> },
    CardSet { steam_id: SteamId, app_id: AppId, foil: bool, result: Result<model::CardSet, String> },
    /// The full notification list (replaces the old one).
    Notifications(model::Notifications),
    /// New notifications just arrived (for a toast).
    NotificationsArrived(Vec<model::Notification>),
    SupportTickets(Result<Vec<model::SupportTicket>, String>),
    SupportTicket { id: String, result: Result<Vec<model::SupportMessage>, String> },
    AccountSecurity(Result<model::AccountSecurity, String>),
    /// A page of purchase history; `more` is true when it continues the list.
    Purchases { more: bool, result: Result<model::PurchasePage, String> },
    LicenseList(Result<Vec<model::License>, String>),
    Family(Result<model::FamilyStatus, String>),
    /// Outcome of a Steam Family management action.
    FamilyAction(Result<model::FamilyActionDone, String>),
    /// Community Market fee rules for your wallet currency.
    FeeRules(model::fees::Rules),
    ListingCancelled { listing_id: u64, result: Result<(), String> },
    BuyOrderCancelled { order_id: u64, result: Result<(), String> },
    /// Games with inventories for an account, or why it's unknown ("private").
    InventoryApps { steam_id: SteamId, result: Result<Vec<model::InventoryApp>, String> },
    /// Games on a profile, or why they can't be shown ("private").
    PlayerGames { steam_id: SteamId, result: Result<Vec<model::PlayerGame>, String> },
    /// Your achievement progress: (game, unlocked, total).
    AchievementProgress(Vec<(AppId, u32, u32)>),
    Achievements { app_id: AppId, result: Result<Vec<model::Achievement>, String> },
    /// Result of [`Command::ActivateKey`]: `Ok(items)` or `Err(reason)`.
    KeyActivated(Result<Vec<String>, String>),
    Download(model::DownloadStatus),
    /// Download regions, fastest first.
    Regions(Vec<model::DownloadRegion>),
    DownloadQueue(Vec<AppId>),
    DownloadFinished { app_id: AppId, result: Result<String, String> },
    WorkshopUnsubscribed { file_id: u64, result: Result<(), String> },
    WorkshopResults { request_id: u64, total: u64, items: Vec<model::WorkshopEntry> },
    WorkshopSubscribed { file_id: u64, result: Result<(), String> },
    /// A verification ended (its row leaves the Downloads page).
    VerifyFinished { app_id: AppId, result: Result<local::manage::VerifyReport, String> },
    MoveProgress { app_id: AppId, done_bytes: u64, total_bytes: u64 },
    MoveFinished { app_id: AppId, result: Result<local::manage::MoveReport, String> },
    BackupProgress { app_id: AppId, done_bytes: u64, total_bytes: u64 },
    BackupFinished { app_id: AppId, result: Result<local::manage::BackupReport, String> },
    RestoreProgress { app_id: AppId, name: String, done_bytes: u64, total_bytes: u64 },
    RestoreFinished { result: Result<local::manage::RestoreReport, String> },
    /// Answer to [`Command::RequestFreeLicense`].
    FreeLicense { app_id: AppId, result: Result<(), String> },
    /// Workshop items were downloaded (or failed) for a game.
    WorkshopSync { app_id: AppId, result: Result<crate::workshop::Report, String> },
    /// Controller bindings / action sets inspected for a game.
    InputConfigInspected { key: LayoutRequest, report: InputConfigReport },
    ControllerLayouts { key: LayoutRequest, result: Result<CommunityPage, String> },
    ControllerLayoutDownloaded { key: LayoutRequest, result: Result<BindingChoice, String> },
    ControllerLayoutRoute { app_id: AppId, enabled: bool },
    ControllerLayoutApplied { app_id: AppId, source: String },
    WorkshopTags { app_id: AppId, result: Result<Vec<model::WorkshopTagCategory>, String> },
    /// (app id, date added)
    Wishlist(Vec<(AppId, u64)>),
    /// A wishlist add/remove is in flight.
    WishlistChanging { app_id: AppId, add: bool },
    Cart(Vec<model::CartLine>),
    /// A store action failed (cart, wishlist).
    StoreError(String),
    /// A message the client doesn't handle yet; useful for extension authors.
    Unhandled { emsg: u32, name: String },
    LoggedOut { reason: String },
    Error(String),
    /// Answer to a [`Command::Hub`] request.
    Hub(hub::Response),
}

/// How the client is connected to Steam right now.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum Link {
    #[default]
    Online,
    /// Offline mode: everything from the local cache, no connection attempts.
    Offline,
    /// The connection dropped; trying again (`attempt` from 1, next try in
    /// `retry_in` seconds, 0 = trying now).
    Reconnecting { attempt: u32, retry_in: u32 },
}

pub type Sink = Arc<dyn Fn(Event) + Send + Sync>;

/// Handle used to send commands to the running engine. Cheap to clone.
#[derive(Clone)]
pub struct Client {
    tx: mpsc::UnboundedSender<Command>,
}

impl Client {
    /// Start the engine on `rt`. `sink` is called for every event, from a
    /// background thread.
    pub fn spawn(rt: &tokio::runtime::Handle, sink: Sink) -> Self {
        let (tx, rx) = mpsc::unbounded_channel();
        rt.spawn(session::run(rx, sink));
        Self { tx }
    }

    /// Create a client whose commands go to a custom channel (e.g. demo mode or tests).
    pub fn from_sender(tx: mpsc::UnboundedSender<Command>) -> Self {
        Self { tx }
    }

    pub fn send(&self, cmd: Command) {
        let _ = self.tx.send(cmd);
    }
}

#[derive(Debug, thiserror::Error)]
pub enum Error {
    #[error(transparent)]
    Net(#[from] net::Error),
    #[error(transparent)]
    Auth(#[from] auth::Error),
    #[error("{0}")]
    Other(String),
}

// Setters publish their new value before advancing this revision. The desktop
// reads the revision before copying preferences, so concurrent writes get
// picked up again on its next pass.
static PREFERENCES_REVISION: std::sync::atomic::AtomicU64 = std::sync::atomic::AtomicU64::new(0);

pub fn preferences_revision() -> u64 {
    PREFERENCES_REVISION.load(std::sync::atomic::Ordering::Acquire)
}

fn preferences_changed() {
    PREFERENCES_REVISION.fetch_add(1, std::sync::atomic::Ordering::AcqRel);
}
