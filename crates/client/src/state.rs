//! A snapshot of the session built purely from [`Event`]s.

use crate::{Event, Guard};
use model::{
    AppId, ChatGroup, ChatMessage, ChatRoomId, GroupBanInfo, GroupChatId, GroupInviteInfo, GroupInviteLink, GroupMessage, OwnedApp, Persona,
    PersonaState, ProfileInfo, Relationship, SteamId, steam_id_to_account,
};
use std::collections::{BTreeMap, HashMap};

#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub enum Phase {
    #[default]
    LoggedOut,
    Connecting,
    /// Waiting for a QR code scan.
    Qr(String),
    Guard(Vec<Guard>),
    LoggedIn,
}

#[derive(Debug, Clone, Default)]
pub struct State {
    pub phase: Phase,
    pub steam_id: SteamId,
    pub account: String,
    pub persona_name: String,
    pub country: String,
    pub level: Option<u32>,
    /// Wallet balance (cents) and currency code.
    pub wallet: Option<(i64, i32)>,
    /// Wallet money not released yet (recent Market sales, refunds), in cents.
    pub wallet_pending: i64,
    pub profiles: crate::read_cache::ReadCache<SteamId, ProfileInfo, 64>,
    /// Loaded inventories by (account, app, context).
    pub inventories: crate::read_cache::ReadCache<(SteamId, AppId, u64), model::Inventory, 32>,
    /// Market prices by (app, market hash name).
    pub market_prices: crate::read_cache::ReadCache<(AppId, String), Result<model::MarketPrice, String>, 128>,
    pub my_listings: Option<Result<model::MyListings, String>>,
    pub profile_edit: Option<Result<model::ProfileEdit, String>>,
    /// Location lists by (country, state).
    pub locations: HashMap<(Option<String>, Option<String>), Vec<(String, String)>>,
    /// Names of games not in your library.
    pub app_names: HashMap<AppId, String>,
    pub email: Option<(String, bool)>,
    /// Last Steam Cloud result per game, and games syncing right now.
    pub cloud: HashMap<AppId, model::CloudReport>,
    pub cloud_syncing: std::collections::HashSet<AppId>,
    /// Games whose shaders are compiling before launch: (done, total).
    pub shader_compiling: HashMap<AppId, (u32, u32)>,
    /// Games whose install script is running before they start: the step.
    pub install_script: HashMap<AppId, String>,
    pub comments: crate::read_cache::ReadCache<SteamId, Result<model::Comments, String>, 64>,
    pub badges: crate::read_cache::ReadCache<SteamId, Result<model::Badges, String>, 64>,
    /// Profile reviews by (profile, page).
    pub reviews: crate::read_cache::ReadCache<(SteamId, u32), Result<model::UserReviews, String>, 64>,
    /// Reviews of games by filter (summary and pages loaded so far).
    pub app_reviews: crate::read_cache::ReadCache<(AppId, model::ReviewFilter), Result<model::AppReviews, String>, 64>,
    /// Games you marked "Not interested" in the store.
    pub ignored_apps: std::collections::HashSet<AppId>,
    /// Your Steam Points Shop balance, when loaded.
    pub points_balance: Option<u64>,
    /// Games you follow in the store.
    pub followed_apps: std::collections::HashSet<AppId>,
    /// Your own review of each game looked up (`None`: none written).
    pub my_reviews: HashMap<AppId, Result<Option<model::UserReview>, String>>,
    /// The store's personal picks for this account (after sign-in).
    pub store_recs: Option<model::StoreRecommendations>,
    pub groups: HashMap<SteamId, Result<Vec<model::UserGroup>, String>>,
    /// Card collections by (profile, game, foil).
    pub card_sets: HashMap<(SteamId, AppId, bool), Result<model::CardSet, String>>,
    pub notifications: Option<model::Notifications>,
    pub support_tickets: Option<Result<Vec<model::SupportTicket>, String>>,
    pub account_security: Option<Result<model::AccountSecurity, String>>,
    /// Purchase history loaded so far (pages appended).
    pub purchases: Option<Result<model::PurchasePage, String>>,
    pub licenses: Option<Result<Vec<model::License>, String>>,
    pub family: Option<Result<model::FamilyStatus, String>>,
    /// Sequence number and outcome of the most recent family management action.
    pub family_action: Option<(u64, Result<model::FamilyActionDone, String>)>,
    pub support_messages: HashMap<String, Result<Vec<model::SupportMessage>, String>>,
    /// Market fee rules for your wallet currency (USD-style until Steam says).
    pub fee_rules: model::fees::Rules,
    /// Games with inventories, by account.
    pub inventory_apps: HashMap<SteamId, Result<Vec<model::InventoryApp>, String>>,
    /// Games on profiles (yours and friends'), or why they're hidden.
    pub player_games: crate::read_cache::ReadCache<SteamId, Result<Vec<model::PlayerGame>, String>, 32>,
    /// Your achievements per game: (unlocked, total).
    pub achievements: HashMap<AppId, (u32, u32)>,
    /// Game server details asked for with "View server info".
    pub servers: HashMap<String, Result<crate::a2s::ServerInfo, String>>,
    /// A game's achievement list, once loaded.
    pub achievement_list: crate::read_cache::ReadCache<AppId, Result<Vec<model::Achievement>, String>, 64>,
    pub downloads: HashMap<AppId, model::DownloadStatus>,
    pub download_queue: Vec<AppId>,
    pub wishlist: Option<Vec<(AppId, u64)>>,
    pub cart: Option<Vec<model::CartLine>>,
    pub license_count: usize,
    /// `None` until PICS has answered.
    pub apps: Option<BTreeMap<AppId, OwnedApp>>,
    pub friends: HashMap<SteamId, Relationship>,
    pub personas: HashMap<SteamId, Persona>,
    /// Conversations by friend, oldest message first.
    pub chats: crate::read_cache::ReadCache<SteamId, Vec<ChatMessage>, 128>,
    /// Unread message counts by friend.
    pub unread: HashMap<SteamId, usize>,
    /// When each friend last sent a typing notice (unix seconds).
    pub typing: HashMap<SteamId, u64>,
    /// Joined Steam group chats keyed by `chat_group_id`.
    pub chat_groups: BTreeMap<GroupChatId, ChatGroup>,
    /// Group chat channel histories by `(chat_group_id, chat_id)`.
    pub group_messages: crate::read_cache::ReadCache<(GroupChatId, ChatRoomId), Vec<GroupMessage>, 128>,
    /// Whether older history pages exist for `(chat_group_id, chat_id)`.
    pub group_more_history: HashMap<(GroupChatId, ChatRoomId), bool>,
    /// Resolved group invite previews keyed by `GroupInviteRef::key()`.
    pub group_invite_previews: HashMap<String, Result<GroupInviteInfo, String>>,
    /// Active invite links per group.
    pub group_invite_links: HashMap<GroupChatId, Result<Vec<GroupInviteLink>, String>>,
    /// Ban lists per group.
    pub group_bans: HashMap<GroupChatId, Result<Vec<GroupBanInfo>, String>>,
    /// Member search results for large/virtualized groups.
    pub group_member_search: HashMap<GroupChatId, Vec<Persona>>,
    pub launching: Option<AppId>,
    pub playing: Option<AppId>,
    /// The running game's lobby, if it's in one ("Invite to lobby").
    pub game_lobby: Option<u64>,
    /// The running game's join string, rich presence `connect` ("Invite to game").
    pub game_connect: Option<String>,
    /// How each game was last started (Aurora / official Steam / no Steam).
    pub launch_modes: HashMap<AppId, crate::LaunchMode>,
    pub last_error: Option<String>,
    pub guard_error: Option<String>,
    /// Community pages loaded so far (Workshop items, guides, discussions).
    pub hub: crate::hub::Cache,
    /// Online, offline mode, or reconnecting.
    pub link: crate::Link,
    /// Play time recorded while not connected: game -> (seconds in all,
    /// seconds not on Steam yet).
    pub offline_playtime: HashMap<AppId, (u64, u64)>,
    /// Games whose Steam Cloud upload waits for a connection.
    pub cloud_pending: std::collections::HashSet<AppId>,
    /// Games currently being moved between library folders.
    pub moving: HashMap<AppId, TransferProgress>,
    /// Games currently being backed up.
    pub backing_up: HashMap<AppId, TransferProgress>,
    /// Backup currently being restored into a library folder.
    pub restoring: Option<RestoreProgress>,
}

/// Progress of an in-flight move or backup operation.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct TransferProgress {
    pub done_bytes: u64,
    pub total_bytes: u64,
}

impl TransferProgress {
    pub fn fraction(&self) -> f32 {
        (self.done_bytes as f32 / self.total_bytes.max(1) as f32).clamp(0.0, 1.0)
    }
}

/// Progress of an in-flight backup restore operation.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct RestoreProgress {
    pub app_id: AppId,
    pub name: String,
    pub done_bytes: u64,
    pub total_bytes: u64,
}

impl RestoreProgress {
    pub fn fraction(&self) -> f32 {
        (self.done_bytes as f32 / self.total_bytes.max(1) as f32).clamp(0.0, 1.0)
    }
}

fn now() -> u64 {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|d| d.as_secs())
        .unwrap_or(0)
}

impl State {
    pub fn apply(&mut self, e: &Event) {
        match e {
            Event::Connecting => {
                self.phase = Phase::Connecting;
                self.last_error = None;
            }
            Event::QrChallenge { url } => self.phase = Phase::Qr(url.clone()),
            Event::GuardRequired { guards } => {
                self.phase = Phase::Guard(guards.clone());
                self.guard_error = None;
            }
            Event::GuardCodeRejected(msg) => self.guard_error = Some(msg.clone()),
            Event::LoginFailed(msg) => {
                *self = State { last_error: Some(msg.clone()), ..Default::default() };
            }
            Event::LoggedIn { steam_id, account } => {
                self.phase = Phase::LoggedIn;
                self.steam_id = *steam_id;
                self.account = account.clone();
            }
            Event::AccountInfo { persona_name, country } => {
                self.persona_name = persona_name.clone();
                self.country = country.clone();
            }
            Event::Level(l) => self.level = Some(*l),
            Event::Wallet { balance, currency, pending } => {
                self.wallet = Some((*balance, *currency));
                self.wallet_pending = *pending;
            }
            Event::Profile(p) => {
                self.profiles.insert(p.steam_id, p.clone());
            }
            Event::Inventory(inv) => {
                self.inventories.insert((inv.steam_id, inv.app_id, inv.context_id), inv.clone());
            }
            Event::MarketPrice { app_id, hash_name, result } => {
                // Keep order-book numbers from an earlier full load.
                let mut result = result.clone();
                if let (Ok(new), Some(Ok(old))) = (&mut result, self.market_prices.get(&(*app_id, hash_name.clone()))) {
                    new.lowest_sell = new.lowest_sell.or(old.lowest_sell);
                    new.highest_buy = new.highest_buy.or(old.highest_buy);
                    new.sell_count = new.sell_count.or(old.sell_count);
                    new.buy_count = new.buy_count.or(old.buy_count);
                }
                if let Ok(Some(r)) = result.as_ref().map(|p| p.fee_rules) {
                    self.fee_rules = r;
                }
                self.market_prices.insert((*app_id, hash_name.clone()), result);
            }
            Event::MyListings(r) => self.my_listings = Some(r.clone()),
            Event::ProfileEdit(r) => self.profile_edit = Some(r.clone()),
            Event::Locations { country, state, result: Ok(l) } => {
                self.locations.insert((country.clone(), state.clone()), l.clone());
            }
            Event::Locations { .. } | Event::ProfileSaved { .. } => {}
            Event::Comments { steam_id, result } => {
                self.comments.insert(*steam_id, result.clone());
            }
            Event::Badges { steam_id, result } => {
                self.badges.insert(*steam_id, result.clone());
            }
            Event::Reviews { steam_id, page, result } => {
                self.reviews.insert((*steam_id, *page), result.clone());
            }
            Event::AppReviews { app_id, filter, more, result } => match (result, self.app_reviews.get_mut(&(*app_id, *filter))) {
                (Ok(page), Some(Ok(have))) if *more => {
                    have.items.extend(page.items.iter().cloned());
                    have.cursor = page.cursor.clone();
                }
                _ => {
                    self.app_reviews.insert((*app_id, *filter), result.clone());
                }
            },
            Event::MyReview { app_id, result } => {
                self.my_reviews.insert(*app_id, result.clone());
            }
            Event::ReviewSaved { app_id, result: Ok(()), .. } => {
                self.app_reviews.retain(|(id, _), _| id != app_id);
            }
            Event::ReviewSaved { .. } => {}
            Event::DiscoveryQueue(_) => {}
            Event::PointsBalance(Ok(pts)) => {
                self.points_balance = Some(*pts);
            }
            Event::PointsBalance(Err(_)) => {}
            Event::AppIgnored { app_id, ignored, result } => {
                if result.is_ok() && *ignored {
                    self.ignored_apps.insert(*app_id);
                } else if result.is_ok() {
                    self.ignored_apps.remove(app_id);
                }
            }
            Event::FollowedApps(ids) => {
                self.followed_apps = ids.iter().copied().collect();
            }
            Event::AppFollowed { app_id, result: Ok(followed) } => {
                if *followed {
                    self.followed_apps.insert(*app_id);
                } else {
                    self.followed_apps.remove(app_id);
                }
            }
            Event::AppFollowed { .. } | Event::WishlistChanging { .. } => {}
            Event::StoreRecommendations(r) => self.store_recs = Some(r.clone()),
            Event::TradeOfferSent { .. } => {}
            Event::Groups { steam_id, result } => {
                self.groups.insert(*steam_id, result.clone());
            }
            Event::CardSet { steam_id, app_id, foil, result } => {
                self.card_sets.insert((*steam_id, *app_id, *foil), result.clone());
            }
            Event::CommentPosted { .. } => {}
            Event::ServerInfo { addr, result } => {
                self.servers.insert(addr.clone(), result.clone());
            }
            Event::Blocked { steam_id, result: Ok(true) } => {
                self.friends.remove(steam_id);
            }
            Event::Blocked { .. } => {}
            Event::WorkshopTags { .. } => {}
            Event::RelaunchForSteamDrm { .. } => {}
            Event::AppNames(list) => self.app_names.extend(list.iter().cloned()),
            Event::Email { address, validated } => self.email = Some((address.clone(), *validated)),
            Event::Regions(_) => {}
            Event::SteamCollections(_) | Event::SteamCollectionsSaved { .. } => {}
            Event::ShaderCompile { app_id, done, total } => {
                self.shader_compiling.insert(*app_id, (*done, *total));
            }
            Event::ShaderCompileDone { app_id } => {
                self.shader_compiling.remove(app_id);
            }
            Event::InstallScript { app_id, step } => match step {
                Some(s) => {
                    self.install_script.insert(*app_id, s.clone());
                }
                None => {
                    self.install_script.remove(app_id);
                }
            },
            Event::CloudSyncing { app_id } => {
                self.cloud_syncing.insert(*app_id);
            }
            Event::CloudReport { report, held_launch } => {
                self.cloud_syncing.remove(&report.app_id);
                if held_launch.is_some() && self.launching == Some(report.app_id) {
                    self.launching = None;
                }
                self.cloud.insert(report.app_id, report.clone());
            }
            Event::Notifications(n) => {
                let mut notifications = n.clone();
                notifications.items.sort_by_key(|n| std::cmp::Reverse(n.time));
                self.notifications = Some(notifications);
            }
            Event::NotificationsArrived(_) => {}
            Event::SupportTickets(r) => self.support_tickets = Some(r.clone()),
            Event::AccountSecurity(r) => self.account_security = Some(r.clone()),
            Event::Purchases { more, result } => match (more, &mut self.purchases, result) {
                (true, Some(Ok(page)), Ok(next)) => {
                    page.rows.extend(next.rows.iter().cloned());
                    page.more = next.more.clone();
                }
                _ => self.purchases = Some(result.clone()),
            },
            Event::LicenseList(r) => self.licenses = Some(r.clone()),
            Event::Family(r) => self.family = Some(r.clone()),
            Event::FamilyAction(r) => {
                let seq = self.family_action.as_ref().map_or(1, |(s, _)| s.saturating_add(1));
                self.family_action = Some((seq, r.clone()));
            }
            Event::SupportTicket { id, result } => {
                self.support_messages.insert(id.clone(), result.clone());
            }
            Event::FeeRules(r) => self.fee_rules = *r,
            Event::ListingCancelled { listing_id, result: Ok(()) } => {
                if let Some(Ok(l)) = &mut self.my_listings {
                    l.listings.retain(|x| x.listing_id != *listing_id);
                }
            }
            Event::BuyOrderCancelled { order_id, result: Ok(()) } => {
                if let Some(Ok(l)) = &mut self.my_listings {
                    l.buy_orders.retain(|x| x.order_id != *order_id);
                }
            }
            Event::ItemListed { asset_id, result: Ok(_) } => {
                // The item leaves the inventory once listed.
                for inv in self.inventories.values_mut() {
                    inv.items.retain(|i| i.asset_id != *asset_id);
                }
            }
            Event::ItemListed { .. } | Event::ListingCancelled { .. } | Event::BuyOrderCancelled { .. } => {}
            Event::InventoryApps { steam_id, result } => {
                self.inventory_apps.insert(*steam_id, result.clone());
            }
            Event::PlayerGames { steam_id, result } => {
                self.player_games.insert(*steam_id, result.clone());
            }
            Event::Achievements { app_id, result } => {
                if let Ok(list) = result {
                    let unlocked = list.iter().filter(|a| a.unlocked).count() as u32;
                    self.achievements.insert(*app_id, (unlocked, list.len() as u32));
                }
                self.achievement_list.insert(*app_id, result.clone());
            }
            Event::AchievementProgress(list) => {
                for (app, unlocked, total) in list {
                    self.achievements.insert(*app, (*unlocked, *total));
                }
            }
            Event::Licenses { count } => self.license_count = *count,
            Event::Apps(apps) => {
                for a in apps.iter() {
                    for path in &a.art {
                        model::art::remember(a.app_id, path);
                    }
                }
                self.apps = Some(apps.iter().map(|a| (a.app_id, a.clone())).collect());
            }
            Event::Friends(list) => self.friends = list.iter().copied().collect(),
            Event::Personas(list) => {
                for p in list {
                    self.personas.insert(p.steam_id, p.clone());
                }
            }
            Event::Message(m) => {
                let chat = self.chats.get_or_default(m.friend);
                // The server echoes our own messages; skip exact duplicates.
                if !chat.iter().rev().take(5).any(|c| c.from == m.from && c.text == m.text && c.invite == m.invite && c.time.abs_diff(m.time) < 5) {
                    chat.push(m.clone());
                }
                if m.from != self.steam_id {
                    *self.unread.entry(m.friend).or_default() += 1;
                    self.typing.remove(&m.friend);
                }
            }
            Event::Typing { from } => {
                self.typing.insert(*from, now());
            }
            Event::History { friend, messages } => {
                let chat = self.chats.get_or_default(*friend);
                let newest_loaded = messages.last().map(|m| m.time).unwrap_or(0);
                let mut merged = messages.clone();
                merged.extend(chat.iter().filter(|m| m.time > newest_loaded).cloned());
                *chat = merged;
            }
            Event::ChatGroups(list) => {
                let mut next = BTreeMap::new();
                for g in list {
                    let mut merged = g.clone();
                    if let Some(prev) = self.chat_groups.get(&g.id) {
                        merge_group(&mut merged, prev, true);
                    }
                    next.insert(merged.id, merged);
                }
                self.chat_groups = next;
            }
            Event::ChatGroupUpdated(g) => {
                let entry = self.chat_groups.entry(g.id).or_default();
                let prev = entry.clone();
                let mut merged = g.clone();
                merge_group(&mut merged, &prev, false);
                *entry = merged;
            }
            Event::GroupHeaderUpdated(g) => {
                let entry = self.chat_groups.entry(g.id).or_insert_with(|| ChatGroup { id: g.id, ..Default::default() });
                entry.name = g.name.clone();
                entry.tagline = g.tagline.clone();
                entry.avatar_url = g.avatar_url.clone();
                if g.owner_id != 0 {
                    entry.owner_id = g.owner_id;
                }
                entry.clan_id = g.clan_id;
                entry.app_id = g.app_id;
                if g.default_role_id != 0 {
                    entry.default_role_id = g.default_role_id;
                }
                if !g.roles.is_empty() {
                    entry.roles = g.roles.clone();
                }
                if !g.role_actions.is_empty() {
                    entry.role_actions = g.role_actions.clone();
                }
            }
            Event::GroupMembersUpdated { group_id, members, total, virtualized } => {
                let entry = self.chat_groups.entry(*group_id).or_insert_with(|| ChatGroup { id: *group_id, ..Default::default() });
                if *virtualized {
                    // Virtualized view updates can arrive as incremental deltas; merge by account_id
                    // instead of replacing the whole list with a 1-member delta.
                    for m in members {
                        if let Some(existing) = entry.members.iter_mut().find(|e| e.account_id == m.account_id) {
                            existing.state = m.state;
                        } else {
                            entry.members.push(m.clone());
                        }
                    }
                    if entry.members.len() > 200 {
                        entry.members.truncate(200);
                    }
                } else {
                    entry.members = members.clone();
                }
                entry.virtualized_members = *virtualized;
                if let Some(n) = total {
                    entry.active_member_count = *n;
                }
            }
            Event::ChatGroupRemoved(id) => {
                self.chat_groups.remove(id);
                self.group_invite_links.remove(id);
                self.group_bans.remove(id);
                self.group_member_search.remove(id);
                self.group_messages.retain(|(gid, _), _| gid != id);
                self.group_more_history.retain(|(gid, _), _| gid != id);
            }
            Event::GroupJoined(_) => {}
            Event::GroupHistory { group_id, chat_id, prepend, more, messages } => {
                self.group_more_history.insert((*group_id, *chat_id), *more);
                let chat = self.group_messages.get_or_default((*group_id, *chat_id));
                let mut merged = if *prepend {
                    let mut all = messages.clone();
                    for existing in chat.iter() {
                        if !all.iter().any(|m| same_group_msg(m, existing)) {
                            all.push(existing.clone());
                        }
                    }
                    all
                } else {
                    let newest_loaded = messages.last().map(|m| (m.time, m.ordinal)).unwrap_or((0, 0));
                    let mut all = messages.clone();
                    for existing in chat.iter() {
                        if (existing.time, existing.ordinal) > newest_loaded && !all.iter().any(|m| same_group_msg(m, existing)) {
                            all.push(existing.clone());
                        }
                    }
                    all
                };
                merged.sort_by_key(|m| (m.time, m.ordinal));
                *chat = merged;
            }
            Event::GroupMessage(m) => {
                let chat = self.group_messages.get_or_default((m.group_id, m.chat_id));
                let is_new = !chat.iter().rev().take(10).any(|c| same_group_msg(c, m));
                if is_new {
                    chat.push(m.clone());
                    chat.sort_by_key(|x| (x.time, x.ordinal));
                }
                if let Some(g) = self.chat_groups.get_mut(&m.group_id) {
                    let group_muted = g.unread_muted;
                    if let Some(ch) = g.channel_mut(m.chat_id) {
                        ch.time_last_message = ch.time_last_message.max(m.time as u32);
                        ch.accountid_last_message = steam_id_to_account(m.from);
                        if !m.text.is_empty() {
                            ch.last_message = m.text.clone();
                        }
                        if m.from == self.steam_id {
                            ch.time_last_ack = ch.time_last_ack.max(m.time as u32);
                            ch.unread = 0;
                        } else if is_new && m.server_msg.is_none() && !group_muted && !ch.unread_muted {
                            ch.unread += 1;
                        }
                    }
                }
            }
            Event::GroupMessageDeleted { group_id, chat_id, messages } => {
                if let Some(list) = self.group_messages.get_mut(&(*group_id, *chat_id)) {
                    for msg in list {
                        if messages.iter().any(|&(t, o)| msg.time as u32 == t && msg.ordinal == o) {
                            msg.deleted = true;
                        }
                    }
                }
            }
            Event::GroupMessageReaction { group_id, chat_id, timestamp, ordinal, reaction_type, reaction, reactor, is_add } => {
                if let Some(list) = self.group_messages.get_mut(&(*group_id, *chat_id)) {
                    if let Some(msg) = list.iter_mut().find(|m| m.time as u32 == *timestamp && m.ordinal == *ordinal) {
                        let mine = *reactor == self.steam_id;
                        if let Some(r) = msg.reactions.iter_mut().find(|r| r.reaction_type == *reaction_type && r.reaction == *reaction) {
                            if *is_add {
                                if !mine || !r.reacted {
                                    r.count = r.count.saturating_add(1);
                                }
                                if mine {
                                    r.reacted = true;
                                }
                            } else {
                                if !mine || r.reacted {
                                    r.count = r.count.saturating_sub(1);
                                }
                                if mine {
                                    r.reacted = false;
                                }
                            }
                            msg.reactions.retain(|r| r.count > 0);
                        } else if *is_add {
                            msg.reactions.push(model::MessageReaction {
                                reaction_type: *reaction_type,
                                reaction: reaction.clone(),
                                count: 1,
                                reacted: mine,
                            });
                        }
                    }
                }
            }
            Event::GroupAck { group_id, chat_id, timestamp } => {
                if let Some(ch) = self.chat_groups.get_mut(group_id).and_then(|g| g.channel_mut(*chat_id)) {
                    ch.time_last_ack = ch.time_last_ack.max(*timestamp);
                    ch.unread = 0;
                }
            }
            Event::GroupMemberChanged { group_id, member, change } => {
                if let Some(g) = self.chat_groups.get_mut(group_id) {
                    // 2 = Parted, 3 = Kicked, 10 = Banned
                    if matches!(change, 2 | 3 | 10) {
                        if member.steam_id == self.steam_id {
                            self.chat_groups.remove(group_id);
                        } else {
                            let before = g.members.len();
                            g.members.retain(|m| m.steam_id != member.steam_id);
                            if g.members.len() < before {
                                g.active_member_count = g.active_member_count.saturating_sub(1);
                            }
                        }
                    } else {
                        if let Some(existing) = g.members.iter_mut().find(|m| m.steam_id == member.steam_id) {
                            *existing = member.clone();
                        } else {
                            g.members.push(member.clone());
                            g.active_member_count = g.active_member_count.max(g.members.len() as u32);
                        }
                        if member.steam_id == self.steam_id {
                            g.my_rank = member.rank;
                            g.my_role_ids = member.role_ids.clone();
                        }
                    }
                }
            }
            Event::GroupRoomsChanged { group_id, default_chat_id, rooms } => {
                if let Some(g) = self.chat_groups.get_mut(group_id) {
                    let old = std::mem::take(&mut g.rooms);
                    g.rooms = rooms
                        .iter()
                        .map(|r| {
                            let mut ch = r.clone();
                            if let Some(prev) = old.iter().find(|p| p.id == ch.id) {
                                ch.time_last_ack = prev.time_last_ack;
                                ch.time_first_unread = prev.time_first_unread;
                                ch.time_last_mention = prev.time_last_mention;
                                ch.unread_muted = prev.unread_muted;
                                ch.unread = prev.unread;
                            }
                            ch
                        })
                        .collect();
                    g.sort_rooms();
                    if *default_chat_id != 0 {
                        g.default_chat_id = *default_chat_id;
                    }
                    g.active_voice_member_count = g.rooms.iter().map(|r| r.members_in_voice.len() as u32).sum();
                }
            }
            Event::GroupInviteResolved { key, result } => {
                self.group_invite_previews.insert(key.clone(), result.clone());
            }
            Event::GroupInviteLinks { group_id, result } => {
                self.group_invite_links.insert(*group_id, result.clone());
            }
            Event::GroupBanList { group_id, result } => {
                self.group_bans.insert(*group_id, result.clone());
            }
            Event::GroupMemberSearch { group_id, results } => {
                self.group_member_search.insert(*group_id, results.clone());
            }
            // A developer attaching a game by hand: nothing to show.
            Event::GameAttached { .. } | Event::GameOverlay { .. } => {}
            Event::GameLaunching { app_id } => {
                self.launching = Some(*app_id);
            }
            Event::GameStarted { app_id } => {
                if self.launching == Some(*app_id) {
                    self.launching = None;
                }
                self.playing = Some(*app_id);
                self.game_lobby = None;
                self.game_connect = None;
                // Show it on our own status right away, like Steam.
                let me = self.steam_id;
                self.personas.entry(me).or_insert_with(|| Persona { steam_id: me, ..Default::default() }).game_app_id = Some(*app_id);
            }
            Event::GameStopped { app_id } => {
                if self.launching == Some(*app_id) {
                    self.launching = None;
                }
                if self.playing == Some(*app_id) {
                    self.playing = None;
                    self.game_lobby = None;
                    self.game_connect = None;
                    if let Some(p) = self.personas.get_mut(&self.steam_id) {
                        p.game_app_id = None;
                        p.game_name = None;
                    }
                }
            }
            Event::AntiCheatWarning { app_id, .. } => {
                if self.launching == Some(*app_id) {
                    self.launching = None;
                }
            }
            Event::GameLobby { app_id, lobby } if self.playing == Some(*app_id) => self.game_lobby = *lobby,
            Event::GameConnect { app_id, connect } if self.playing == Some(*app_id) => self.game_connect = connect.clone(),
            Event::GameLobby { .. } | Event::GameConnect { .. } => {}
            Event::LoggedOut { reason } => {
                *self = State { last_error: Some(reason.clone()), ..Default::default() };
            }
            Event::Error(msg) => {
                if self.playing.is_none() {
                    self.launching = None;
                }
                self.last_error = Some(msg.clone());
            }
            Event::StoreError(_) => {
                if self.playing.is_none() {
                    self.launching = None;
                }
            }
            Event::KeyActivated(_) | Event::WorkshopUnsubscribed { .. } => {}
            Event::WorkshopResults { .. } | Event::WorkshopSubscribed { .. } | Event::WorkshopSync { .. } | Event::FreeLicense { .. } => {}
            Event::Download(d) => {
                self.downloads.insert(d.app_id, d.clone());
            }
            Event::DownloadQueue(q) => self.download_queue = q.clone(),
            Event::VerifyFinished { app_id, .. } => {
                if self.downloads.get(app_id).is_some_and(|d| d.verifying()) {
                    self.downloads.remove(app_id);
                }
            }
            Event::MoveProgress { app_id, done_bytes, total_bytes } => {
                self.moving.insert(*app_id, TransferProgress { done_bytes: *done_bytes, total_bytes: *total_bytes });
            }
            Event::MoveFinished { app_id, .. } => {
                self.moving.remove(app_id);
            }
            Event::BackupProgress { app_id, done_bytes, total_bytes } => {
                self.backing_up.insert(*app_id, TransferProgress { done_bytes: *done_bytes, total_bytes: *total_bytes });
            }
            Event::BackupFinished { app_id, .. } => {
                self.backing_up.remove(app_id);
            }
            Event::RestoreProgress { app_id, name, done_bytes, total_bytes } => {
                self.restoring = Some(RestoreProgress {
                    app_id: *app_id,
                    name: name.clone(),
                    done_bytes: *done_bytes,
                    total_bytes: *total_bytes,
                });
            }
            Event::RestoreFinished { .. } => {
                self.restoring = None;
            }
            Event::DownloadFinished { app_id, result } => {
                if let Some(d) = self.downloads.get_mut(app_id) {
                    match result {
                        Ok(_) => d.phase = model::DownloadPhase::Done,
                        Err(e) => {
                            d.phase = model::DownloadPhase::Failed;
                            d.error = Some(e.clone());
                        }
                    }
                }
            }
            Event::Wishlist(w) => self.wishlist = Some(w.clone()),
            Event::Cart(c) => self.cart = Some(c.clone()),
            Event::Unhandled { .. } => {}
            Event::LaunchMode { app_id, mode } => {
                self.launch_modes.insert(*app_id, mode.clone());
            }
            Event::Hub(r) => self.hub.apply(r),
            Event::ConnectionError { message, .. } => {
                *self = State { last_error: Some(message.clone()), ..Default::default() };
            }
            Event::Link(l) => self.link = *l,
            Event::OfflinePlaytime(list) => self.offline_playtime = list.iter().map(|&(a, total, pending)| (a, (total, pending))).collect(),
            Event::CloudPending(list) => self.cloud_pending = list.iter().copied().collect(),
            Event::CloudSkipped { .. } => {}
            Event::InputConfigInspected { .. }
            | Event::ControllerLayouts { .. }
            | Event::ControllerLayoutDownloaded { .. }
            | Event::ControllerLayoutRoute { .. }
            | Event::ControllerLayoutApplied { .. } => {}
        }
    }

    /// Connected to Steam (not offline mode, not reconnecting).
    pub fn online(&self) -> bool {
        self.link == crate::Link::Online
    }

    pub fn me(&self) -> Option<&Persona> {
        self.personas.get(&self.steam_id)
    }

    pub fn persona(&self, id: SteamId) -> Persona {
        self.personas.get(&id).cloned().unwrap_or_else(|| Persona { steam_id: id, ..Default::default() })
    }

    /// Accepted friends with their persona (if known yet).
    pub fn friend_list(&self) -> Vec<Persona> {
        let mut out: Vec<Persona> = self
            .friends
            .iter()
            .filter(|(_, r)| **r == Relationship::Friend)
            .map(|(id, _)| self.persona(*id))
            .collect();
        out.sort_by(|a, b| model::cmp_ignore_case(&a.name, &b.name));
        out
    }

    /// Accepted friends in display order, including IDs whose persona has not arrived.
    pub fn friend_refs(&self) -> Vec<(SteamId, Option<&Persona>)> {
        let mut friends: Vec<_> = self.friends.iter()
            .filter(|(_, relationship)| **relationship == Relationship::Friend)
            .map(|(&id, _)| (id, self.personas.get(&id)))
            .collect();
        friends.sort_by(|(_, a), (_, b)| {
            model::cmp_ignore_case(a.map_or("", |p| p.name.as_str()), b.map_or("", |p| p.name.as_str()))
        });
        friends
    }

    pub fn friend_count(&self) -> usize {
        self.friends.values().filter(|r| **r == Relationship::Friend).count()
    }

    pub fn online_friend_count(&self) -> usize {
        self.friends
            .iter()
            .filter(|(id, relationship)| {
                **relationship == Relationship::Friend
                    && self.personas.get(*id).is_some_and(|p| p.state != PersonaState::Offline)
            })
            .count()
    }

    pub fn friends_playing(&self, app_id: AppId) -> Vec<&Persona> {
        let mut out: Vec<_> = self
            .friends
            .iter()
            .filter(|(_, relationship)| **relationship == Relationship::Friend)
            .filter_map(|(id, _)| self.personas.get(id))
            .filter(|p| p.game_app_id == Some(app_id))
            .collect();
        out.sort_by(|a, b| model::cmp_ignore_case(&a.name, &b.name));
        out
    }

    pub fn pending_requests(&self) -> usize {
        self.friends.values().filter(|r| **r == Relationship::RequestRecipient).count()
    }

    pub fn total_unread(&self) -> usize {
        self.unread.values().sum::<usize>() + self.chat_groups.values().map(ChatGroup::unread_count).sum::<usize>()
    }

    pub fn group_display_name(&self, g: &ChatGroup) -> String {
        g.display_name(|id| self.personas.get(&id).map(|p| p.display_name()), self.steam_id)
    }

    /// Joined group chats, unread first, then saved groups, then most recently active.
    pub fn group_list(&self) -> Vec<&ChatGroup> {
        let mut out: Vec<&ChatGroup> = self.chat_groups.values().collect();
        out.sort_by_key(|g| {
            let latest = g.rooms.iter().map(|r| r.time_last_message).max().unwrap_or(0);
            (g.unread_count() == 0, !g.is_saved(), std::cmp::Reverse(latest), self.group_display_name(g).to_lowercase())
        });
        out
    }

    /// Clear unread state for a channel and return the timestamp to ack on Steam.
    pub fn mark_group_read(&mut self, group_id: GroupChatId, chat_id: ChatRoomId) -> Option<u32> {
        let msg_ts = self.group_messages.get(&(group_id, chat_id)).and_then(|v| v.last()).map(|m| m.time as u32).unwrap_or(0);
        let ch = self.chat_groups.get_mut(&group_id)?.channel_mut(chat_id)?;
        let ts = ch.time_last_message.max(msg_ts);
        let had_unread = ch.unread > 0 || (ts > ch.time_last_ack && ts > 0);
        ch.unread = 0;
        if ts > ch.time_last_ack {
            ch.time_last_ack = ts;
        }
        (had_unread && ts > 0).then_some(ts)
    }

    /// True if the friend sent a typing notice in the last 8 seconds.
    pub fn is_typing(&self, id: SteamId) -> bool {
        self.typing.get(&id).is_some_and(|t| now().saturating_sub(*t) < 8)
    }

    pub fn app_name(&self, id: AppId) -> Option<&str> {
        self.apps.as_ref().and_then(|a| a.get(&id)).map(|a| a.name.as_str()).or_else(|| self.app_names.get(&id).map(|s| s.as_str()))
    }
}

fn same_group_msg(a: &GroupMessage, b: &GroupMessage) -> bool {
    if a.ordinal != 0 && b.ordinal != 0 {
        a.time == b.time && a.ordinal == b.ordinal
    } else {
        a.from == b.from && a.text == b.text && a.invite == b.invite && a.group_invite == b.group_invite && a.time.abs_diff(b.time) < 5
    }
}

fn merge_group(dst: &mut ChatGroup, prev: &ChatGroup, summary_only: bool) {
    if dst.name.is_empty() && summary_only {
        dst.name = prev.name.clone();
    }
    if dst.tagline.is_empty() && !prev.tagline.is_empty() && summary_only {
        dst.tagline = prev.tagline.clone();
    }
    if dst.avatar_url.is_none() {
        dst.avatar_url = prev.avatar_url.clone();
    }
    if dst.owner_id == 0 {
        dst.owner_id = prev.owner_id;
    }
    if dst.clan_id == 0 {
        dst.clan_id = prev.clan_id;
    }
    if dst.app_id == 0 {
        dst.app_id = prev.app_id;
    }
    if dst.default_chat_id == 0 {
        dst.default_chat_id = prev.default_chat_id;
    }
    if dst.top_members.is_empty() {
        dst.top_members = prev.top_members.clone();
    }
    if dst.members.is_empty() {
        dst.members = prev.members.clone();
    }
    if dst.roles.is_empty() {
        dst.roles = prev.roles.clone();
    }
    if dst.role_actions.is_empty() {
        dst.role_actions = prev.role_actions.clone();
    }
    if dst.default_role_id == 0 {
        dst.default_role_id = prev.default_role_id;
    }
    if dst.my_rank == 0 {
        dst.my_rank = prev.my_rank;
    }
    if dst.my_role_ids.is_empty() {
        dst.my_role_ids = prev.my_role_ids.clone();
    }
    if dst.time_joined == 0 {
        dst.time_joined = prev.time_joined;
    }
    if dst.time_last_group_ack == 0 {
        dst.time_last_group_ack = prev.time_last_group_ack;
    }
    if !summary_only && prev.unread_muted {
        dst.unread_muted = prev.unread_muted;
    }
    if !summary_only && dst.desktop_notification_level == 0 {
        dst.desktop_notification_level = prev.desktop_notification_level;
    }
    if dst.active_member_count == 0 {
        dst.active_member_count = prev.active_member_count;
    }
    dst.full_loaded |= prev.full_loaded;
    dst.virtualized_members |= prev.virtualized_members;
    if dst.rooms.is_empty() {
        dst.rooms = prev.rooms.clone();
    } else {
        for ch in &mut dst.rooms {
            if let Some(old) = prev.rooms.iter().find(|r| r.id == ch.id) {
                if (ch.name.trim().is_empty() || ch.name == "General") && !old.name.trim().is_empty() {
                    ch.name = old.name.clone();
                }
                ch.unread = old.unread;
                if ch.time_last_ack == 0 {
                    ch.time_last_ack = old.time_last_ack;
                }
                if ch.time_first_unread == 0 {
                    ch.time_first_unread = old.time_first_unread;
                }
                if ch.time_last_mention == 0 {
                    ch.time_last_mention = old.time_last_mention;
                }
                if !summary_only {
                    ch.unread_muted = old.unread_muted;
                }
            }
        }
    }
}

#[cfg(test)]
mod notification_tests {
    use super::*;
    use model::{Notification, Notifications};

    #[test]
    fn stores_notifications_newest_first_with_stable_ties() {
        let mut state = State::default();
        let items = [(1, 10), (2, 20), (3, 20), (4, 5)]
            .map(|(id, time)| Notification { id, time, ..Default::default() })
            .to_vec();

        state.apply(&Event::Notifications(Notifications { items, ..Default::default() }));

        let ids: Vec<_> = state.notifications.unwrap().items.iter().map(|n| n.id).collect();
        assert_eq!(ids, [2, 3, 1, 4]);
    }
}

#[cfg(test)]
mod group_tests {
    use super::*;
    use model::{ChatGroup, ChatRoomChannel, GroupMessage};

    #[test]
    fn group_unread_ack_reactions_and_deletes() {
        let mut s = State::default();
        let me = 76561197960265729;
        let friend = 76561197960265730;
        s.apply(&Event::LoggedIn { steam_id: me, account: "me".into() });
        s.apply(&Event::ChatGroups(vec![ChatGroup {
            id: 100,
            name: "Squad".into(),
            default_chat_id: 1,
            rooms: vec![ChatRoomChannel { id: 1, name: "general".into(), time_last_ack: 10, ..Default::default() }],
            ..Default::default()
        }]));

        let msg = GroupMessage {
            group_id: 100,
            chat_id: 1,
            from: friend,
            text: "hi squad".into(),
            time: 50,
            ordinal: 1,
            invite: None,
            group_invite: None,
            deleted: false,
            server_msg: None,
            reactions: Vec::new(),
            mentioned_me: false,
        };
        s.apply(&Event::GroupMessage(msg.clone()));
        // Duplicate delivery is ignored.
        s.apply(&Event::GroupMessage(msg));
        assert_eq!(s.group_messages.get(&(100, 1)).unwrap().len(), 1);
        assert_eq!(s.total_unread(), 1);

        // Reaction add / remove toggles count & reacted state.
        s.apply(&Event::GroupMessageReaction {
            group_id: 100,
            chat_id: 1,
            timestamp: 50,
            ordinal: 1,
            reaction_type: 1,
            reaction: ":thumbsup:".into(),
            reactor: me,
            is_add: true,
        });
        let r = &s.group_messages.get(&(100, 1)).unwrap()[0].reactions[0];
        assert_eq!((r.count, r.reacted), (1, true));

        // Ack clears unread.
        assert_eq!(s.mark_group_read(100, 1), Some(50));
        assert_eq!(s.total_unread(), 0);

        // Delete marks message deleted.
        s.apply(&Event::GroupMessageDeleted { group_id: 100, chat_id: 1, messages: vec![(50, 1)] });
        assert!(s.group_messages.get(&(100, 1)).unwrap()[0].deleted);
    }

    #[test]
    fn tracks_storage_move_backup_and_restore_progress() {
        let mut s = State::default();
        s.apply(&Event::MoveProgress { app_id: 620, done_bytes: 50, total_bytes: 100 });
        assert_eq!(s.moving.get(&620).unwrap().fraction(), 0.5);
        s.apply(&Event::MoveFinished { app_id: 620, result: Err("cancelled".into()) });
        assert!(s.moving.is_empty());

        s.apply(&Event::BackupProgress { app_id: 440, done_bytes: 25, total_bytes: 100 });
        assert_eq!(s.backing_up.get(&440).unwrap().fraction(), 0.25);
        s.apply(&Event::BackupFinished { app_id: 440, result: Err("cancelled".into()) });
        assert!(s.backing_up.is_empty());

        s.apply(&Event::RestoreProgress { app_id: 730, name: "CS2".into(), done_bytes: 80, total_bytes: 100 });
        let r = s.restoring.as_ref().unwrap();
        assert_eq!(r.app_id, 730);
        assert_eq!(r.name, "CS2");
        assert_eq!(r.fraction(), 0.8);
        s.apply(&Event::RestoreFinished { result: Err("cancelled".into()) });
        assert!(s.restoring.is_none());
    }

    #[test]
    fn tracks_game_launching_and_playing_transitions() {
        let mut s = State::default();
        s.apply(&Event::GameLaunching { app_id: 730 });
        assert_eq!(s.launching, Some(730));
        assert_eq!(s.playing, None);

        s.apply(&Event::GameStarted { app_id: 730 });
        assert_eq!(s.launching, None);
        assert_eq!(s.playing, Some(730));

        // Unrelated GameStopped does not clear active playing app.
        s.apply(&Event::GameStopped { app_id: 440 });
        assert_eq!(s.playing, Some(730));

        s.apply(&Event::GameStopped { app_id: 730 });
        assert_eq!(s.playing, None);

        // Launch failure clears launching state.
        s.apply(&Event::GameLaunching { app_id: 570 });
        s.apply(&Event::Error("failed".into()));
        assert_eq!(s.launching, None);

        // Anti-cheat warning pauses launch and clears launching state.
        s.apply(&Event::GameLaunching { app_id: 570 });
        s.apply(&Event::AntiCheatWarning {
            app_id: 570,
            install_dir: std::path::PathBuf::new(),
            args: crate::LaunchArgs::default(),
            skip_cloud: false,
        });
        assert_eq!(s.launching, None);
    }
}
