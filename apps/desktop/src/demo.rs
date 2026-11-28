//! `--demo`: fills the UI with sample data (plus your real installed games)
//! without signing in. Handy for UI work and for testing extensions.

use client::Event;
use model::{
    ChatGroup, ChatGroupMember, ChatMessage, ChatRole, ChatRoleActions, ChatRoomChannel, GroupInviteInfo, GroupInviteLink, GroupInviteRef, GroupMessage,
    GroupServerMessage, MessageReaction, OwnedApp, Persona, PersonaState, Relationship, steam_id_to_account,
};

pub fn events(local: &local::LocalState) -> Vec<Event> {
    const BASE: u64 = 76561197960265728;
    let me = BASE + 1;
    let app = |id, name: &str| OwnedApp { app_id: id, name: name.into(), kind: "game".into(), launch: vec![], icon: None, parent: None, ..Default::default() };
    let mut apps: Vec<OwnedApp> = local.games.iter().map(|g| app(g.app_id, &g.name)).collect();
    for (id, name) in [(440, "Team Fortress 2"), (570, "Dota 2"), (620, "Portal 2"), (730, "Counter-Strike 2"), (1145360, "Hades"), (413150, "Stardew Valley"), (292030, "The Witcher 3: Wild Hunt"), (1203220, "永劫无间 NARAKA"), (1938090, "コール オブ デューティ")] {
        if !apps.iter().any(|a| a.app_id == id) {
            apps.push(app(id, name));
        }
    }
    // Windows and Linux builds, so the install dialog offers the choice
    // (`--page=library --arg=install:413150`).
    if let Some(a) = apps.iter_mut().find(|a| a.app_id == 413150) {
        let depot = |id, os: &str, size| model::DepotInfo { id, os: os.into(), manifests: vec![("public".into(), 1, size)], ..Default::default() };
        a.depots = vec![depot(413151, "windows", 640_000_000), depot(413153, "linux", 655_000_000)];
    }

    let friends = [
        ("Ferris", PersonaState::Online, Some(620)),
        ("Nova", PersonaState::Online, Some(570)),
        ("Pixel", PersonaState::Away, None),
        ("Quartz", PersonaState::Online, None),
        ("Juniper", PersonaState::Offline, None),
        ("Marlow", PersonaState::Offline, None),
    ];
    let personas: Vec<Persona> = std::iter::once(Persona { steam_id: me, name: "You".into(), state: PersonaState::Online, ..Default::default() })
        .chain(friends.iter().enumerate().map(|(i, (name, state, game))| Persona {
            steam_id: BASE + 100 + i as u64,
            name: (*name).into(),
            state: *state,
            game_app_id: *game,
            last_logoff: Some(1_700_000_000),
            // Nova is on a game server: shows "Join game" and "View server info".
            server: (i == 1).then(|| "127.0.0.1:27015".to_string()),
            rich_status: (i == 1).then(|| "Ranked · Dire 21 – 17 Radiant".to_string()),
            nickname: (i == 2).then(|| "Pix".to_string()),
            ..Default::default()
        }))
        .collect();

    let now = std::time::SystemTime::now().duration_since(std::time::UNIX_EPOCH).map(|d| d.as_secs()).unwrap_or(0);
    let ferris = BASE + 100;
    let chat = [
        (ferris, "hey, portal 2 co-op tonight?", 600),
        (me, "sure, after dinner", 540),
        (ferris, "nice. I'll send an invite around 8", 500),
    ];
    let mut events = vec![
        Event::LoggedIn { steam_id: me, account: "demo".into() },
        Event::AccountInfo { persona_name: "You".into(), country: "US".into() },
        Event::Level(27),
        Event::Licenses { count: apps.len() },
        Event::Apps(apps),
        Event::Friends((0..friends.len()).map(|i| (BASE + 100 + i as u64, Relationship::Friend)).collect()),
        Event::Personas(personas),
    ];
    let mut history: Vec<ChatMessage> = chat
        .iter()
        .map(|(from, text, ago)| {
            let parsed = model::chat::parse_message(text);
            ChatMessage {
                friend: ferris,
                from: *from,
                text: parsed.text,
                time: now - ago,
                invite: parsed.invite,
                group_invite: parsed.group_invite,
                media: parsed.media,
            }
        })
        .collect();
    history.push(ChatMessage {
        friend: ferris,
        from: ferris,
        text: "hop into our weekend squad group:".into(),
        time: now - 420,
        invite: None,
        group_invite: Some(GroupInviteRef::Link { code: "DEMO99".into() }),
        media: Vec::new(),
    });
    events.push(Event::History { friend: ferris, messages: history });
    let summary = "Collector of indie gems and long co-op nights.\n\nCurrently: replaying Hades on the hardest heat, slowly finishing Stardew Valley's community center, and trying every Portal 2 workshop map my friends send me.\n\nAdd me if you want to play co-op, trade cards or just chat about games. I'm usually online in the evenings.";
    events.push(Event::Profile(model::ProfileInfo {
        steam_id: me,
        background: Some(model::art::hero(1145360)),
        real_name: "Demo Player".into(),
        summary: summary.into(),
        location: "Seattle, Washington, United States".into(),
        member_since: Some(now - 9 * 31_536_000),
        background_full: true,
        mini_background: Some(model::art::hero(1145360)),
        theme: "Midnight".into(),
        ..Default::default()
    }));
    events.push(Event::Wallet { balance: 1234, currency: 1, pending: 250 });
    events.push(Event::Message(ChatMessage {
        friend: BASE + 101,
        from: BASE + 101,
        text: "gg! one more?".into(),
        time: now - 30,
        invite: None,
        group_invite: None,
        media: Vec::new(),
    }));
    events.extend(group_chats(me, now));
    events.extend(inventory(me, now));
    events.extend(crate::builtin::hub::demo::events());
    events.extend(crate::builtin::community_home::demo_events());
    events
}

fn group_chats(me: u64, now: u64) -> Vec<Event> {
    const BASE: u64 = 76561197960265728;
    let ferris = BASE + 100;
    let nova = BASE + 101;
    let pixel = BASE + 102;
    let quartz = BASE + 103;
    let juniper = BASE + 104;

    let gid1 = 900_001;
    let gid2 = 900_002;
    let my_acc = steam_id_to_account(me);
    let members = vec![
        ChatGroupMember { steam_id: me, account_id: my_acc, rank: 50, state: 2, time_kick_expire: 0, role_ids: vec![1] },
        ChatGroupMember { steam_id: ferris, account_id: steam_id_to_account(ferris), rank: 40, state: 2, time_kick_expire: 0, role_ids: vec![2] },
        ChatGroupMember { steam_id: nova, account_id: steam_id_to_account(nova), rank: 30, state: 2, time_kick_expire: 0, role_ids: vec![3] },
        ChatGroupMember { steam_id: pixel, account_id: steam_id_to_account(pixel), rank: 20, state: 2, time_kick_expire: 0, role_ids: vec![4] },
        ChatGroupMember { steam_id: quartz, account_id: steam_id_to_account(quartz), rank: 20, state: 2, time_kick_expire: 0, role_ids: vec![4] },
        ChatGroupMember { steam_id: juniper, account_id: steam_id_to_account(juniper), rank: 20, state: 2, time_kick_expire: 0, role_ids: vec![4] },
    ];
    let squad = ChatGroup {
        id: gid1,
        name: "Aurora Co-op Squad".into(),
        tagline: "Weekend co-op nights & speedruns".into(),
        avatar_url: Some(model::art::hero(620)),
        owner_id: me,
        default_chat_id: 1,
        rooms: vec![
            ChatRoomChannel {
                id: 1,
                name: "General".into(),
                sort_order: 1,
                time_last_message: (now - 90) as u32,
                time_last_ack: (now - 90) as u32,
                ..Default::default()
            },
            ChatRoomChannel {
                id: 2,
                name: "LFG & Invites".into(),
                sort_order: 2,
                time_last_message: (now - 45) as u32,
                time_last_ack: (now - 300) as u32,
                unread: 1,
                ..Default::default()
            },
            ChatRoomChannel {
                id: 3,
                name: "Lobby Voice".into(),
                voice_allowed: true,
                members_in_voice: vec![steam_id_to_account(ferris), steam_id_to_account(quartz)],
                sort_order: 3,
                ..Default::default()
            },
        ],
        members,
        top_members: vec![me, ferris, nova, pixel, quartz],
        active_member_count: 6,
        active_voice_member_count: 2,
        my_rank: 50,
        my_role_ids: vec![1],
        roles: vec![
            ChatRole { role_id: 1, name: "Owner".into(), ordinal: 1 },
            ChatRole { role_id: 2, name: "Officer".into(), ordinal: 2 },
            ChatRole { role_id: 3, name: "Moderator".into(), ordinal: 3 },
            ChatRole { role_id: 4, name: "Member".into(), ordinal: 4 },
        ],
        role_actions: vec![ChatRoleActions { role_id: 1, ..ChatRoleActions::all() }],
        default_role_id: 4,
        time_joined: (now - 86_400) as u32,
        full_loaded: true,
        ..Default::default()
    };

    let party = ChatGroup {
        id: gid2,
        default_chat_id: 1,
        rooms: vec![ChatRoomChannel {
            id: 1,
            name: "Chat".into(),
            sort_order: 1,
            time_last_message: (now - 240) as u32,
            time_last_ack: (now - 240) as u32,
            ..Default::default()
        }],
        members: vec![
            ChatGroupMember { steam_id: me, account_id: my_acc, rank: 50, state: 2, ..Default::default() },
            ChatGroupMember { steam_id: ferris, account_id: steam_id_to_account(ferris), rank: 20, state: 2, ..Default::default() },
            ChatGroupMember { steam_id: nova, account_id: steam_id_to_account(nova), rank: 20, state: 2, ..Default::default() },
        ],
        top_members: vec![ferris, nova],
        active_member_count: 3,
        my_rank: 50,
        full_loaded: true,
        ..Default::default()
    };

    vec![
        Event::ChatGroups(vec![squad, party]),
        Event::GroupHistory {
            group_id: gid1,
            chat_id: 1,
            prepend: false,
            more: false,
            messages: vec![
                GroupMessage {
                    group_id: gid1,
                    chat_id: 1,
                    from: quartz,
                    text: String::new(),
                    time: now - 900,
                    ordinal: 1,
                    invite: None,
                    group_invite: None,
                    deleted: false,
                    server_msg: Some(GroupServerMessage { kind: 4, string_param: String::new(), account_id_param: steam_id_to_account(quartz) }),
                    reactions: vec![],
                    mentioned_me: false,
                    media: vec![],
                },
                GroupMessage {
                    group_id: gid1,
                    chat_id: 1,
                    from: ferris,
                    text: "Ready for Portal 2 custom chambers tonight? @You bring snacks!".into(),
                    time: now - 320,
                    ordinal: 1,
                    invite: None,
                    group_invite: None,
                    deleted: false,
                    server_msg: None,
                    reactions: vec![
                        MessageReaction { reaction_type: 1, reaction: "👍".into(), count: 3, reacted: true },
                        MessageReaction { reaction_type: 1, reaction: "🎉".into(), count: 2, reacted: false },
                    ],
                    mentioned_me: true,
                    media: vec![],
                },
                GroupMessage {
                    group_id: gid1,
                    chat_id: 1,
                    from: me,
                    text: "On my way once shaders finish compiling!".into(),
                    time: now - 90,
                    ordinal: 1,
                    invite: None,
                    group_invite: None,
                    deleted: false,
                    server_msg: None,
                    reactions: vec![MessageReaction { reaction_type: 1, reaction: "🔥".into(), count: 1, reacted: false }],
                    mentioned_me: false,
                    media: vec![],
                },
            ],
        },
        Event::GroupHistory {
            group_id: gid1,
            chat_id: 2,
            prepend: false,
            more: false,
            messages: vec![GroupMessage {
                group_id: gid1,
                chat_id: 2,
                from: nova,
                text: "Need 1 more for ranked queue — lobby open!".into(),
                time: now - 45,
                ordinal: 1,
                invite: None,
                group_invite: None,
                deleted: false,
                server_msg: None,
                reactions: vec![],
                mentioned_me: false,
                media: vec![],
            }],
        },
        Event::GroupInviteLinks {
            group_id: gid1,
            result: Ok(vec![GroupInviteLink {
                code: "AURO99".into(),
                creator: me,
                expires: (now + 86_400) as u32,
                chat_id: 1,
            }]),
        },
        Event::GroupInviteResolved {
            key: "link:DEMO99".into(),
            result: Ok(GroupInviteInfo {
                key: "link:DEMO99".into(),
                group_id: 900_999,
                chat_id: 1,
                name: "Weekend Indie Club".into(),
                avatar_url: Some(model::art::hero(1145360)),
                member_count: 14,
                banned: false,
                kick_expires: 0,
                expires: 0,
                already_member: false,
            }),
        },
    ]
}

/// Sample inventory, market and support data (public Dota 2 items).
fn inventory(me: u64, now: u64) -> Vec<Event> {
    use model::{InventoryApp, InventoryItem, MarketPrice, MyListing, MyListings, SupportMessage, SupportTicket};
    const ITEMS: &[(&str, &str, &str, &str, u64, &str, &str)] = &[
        ("Arms of Desolation", "Immortal arms", "Immortal", "Arms", 1138, "Shadow Fiend", "i0CoZ81Ui0m-9KwlBY1L_18myuGuq1wfhWSaZgMttydbPaERSR0Wqmu7LAocGIyi3kajH-Xcws6uIkSR7FN78Mj1-FT0fhn0jZmv_DBW_M2qbbdibvHHDTLJlLZ3s7NvFyjqzE1z5znVm9uucHiWO1coCMF1RbRZsRW6l9H5d7S1tUgGsHE"),
        ("Hydrakan Latch", "Immortal Latch", "Immortal", "Weapon", 289, "Dragon Knight", "i0CoZ81Ui0m-9KwlBY1L_18myuGuq1wfhWSaZgMttydbPaERSR0Wqmu7LAocGIyi3kajH-XYwtiqenOO4UR0_4b61VXmVR750Zvl7ioU5-uqeq9sLs-fD3OTydF7v-VsQ2fll0wi6mrdnNipJC6ebg4nDZYkEeQKtBPsl9W1YuK2sQLc2NhAmCX2kGoXufdXeDr7"),
        ("The Strings of Suradan", "Immortal Wearable", "Immortal", "Weapon", 563, "Dark Willow", "i0CoZ81Ui0m-9KwlBY1L_18myuGuq1wfhWSaZgMttydbPaERSR0Wqmu7LAocGIyi3kajH_7bzM62PHWcql56-4PLuAm1EyL4k5vv7zZa4825baV9L_7cBmifxdEk4LM7cCC-mBI1pzyJpZj8IDvIMBhzDsciFuZY5ES-xoXhMenqtVTfiItEyy6viS4b6Cpv4roEBaF086GBkUifZmzryOWo"),
        ("Frosty the Sew-Man", "Immortal Bundle", "Immortal", "Bundle", 2627, "Pudge", "i0CoZ81Ui0m-9KwlBY1L_18myuGuq1wfhWSaZgMttydbPaERSR0Wqmu7LAocGIm52k-5XvHr0MmzMH6Z9hlz5ojn_lDxVA6jzsSzwjZU9uK7bKNobvPGCmSWxe0h5bI_Fny1lkUm42XVnIqgdX3CaAEnXpV4F7UOuhS7k9z5d7S1xv_hlhM"),
        ("Lumpo and Rupertus", "Legendary Bundle", "Legendary", "Bundle", 2729, "Ogre Magi", "i0CoZ81Ui0m-9KwlBY1L_18myuGuq1wfhWSaZgMttydbPaERSR0Wqmu7LAocGJGzyF-5RPPZ_NmtOm-o4Ftl4J66s1jiF0iizMLi_nsPt_esafA4c_PLCDbHw-sutbY9TH7gw08k622GysHpLyzhnw2XIg"),
        ("Jingle Fist Bundle", "Immortal Bundle", "Immortal", "Bundle", 2664, "Monkey King", "i0CoZ81Ui0m-9KwlBY1L_18myuGuq1wfhWSaZgMttydbPaERSR0Wqmu7LAocGIm52k-5XvHr0MmzMH6Z9hlz5ojn_lDxVA6jzsSzwjZO_PngP_w9dPWSDT7HxO8itrg-GS_lkUR_6jmDmdn7I3-UPQR1XJJzQu9YsQ74zIMcHg3psw"),
        ("Crownfall Treasure I", "Mythical Treasure chest", "Mythical", "Treasure Chest", 175, "", "i0CoZ81Ui0m-9KwlBY1L_18myuGuq1wfhWSaZgMttydbPaERSR0Wqmu7LAocGJG51EejH_XGzN2vM3qb6Wlm95Xx71jySiLljJPh7jdJ6rz7O6FsePbCCzKRwrpz6edvFi23wk0i6zvTzd76dXrEZgUpCZR0QOILrFDmxRCfiKj7"),
        ("Dead Reckoning Chest", "Rare Treasure chest", "Rare", "Treasure Chest", 30, "", "i0CoZ81Ui0m-9KwlBY1L_18myuGuq1wfhWSaZgMttydbPaERSR0Wqmu7LAocGJG51EejH-LGxsuyIGmS2llzy4Px613YUxjylZnu9CxcoaX8PadodKSQXT6UmLsi4uM5TXjlkR9ytm3Xmdz6di_CPAIjW5t5TO4U8k7vczduM-k"),
        ("Frostivus 2023 Treasure Chest", "Rare Treasure chest", "Rare", "Treasure Chest", 19, "", "i0CoZ81Ui0m-9KwlBY1L_18myuGuq1wfhWSaZgMttydbPaERSR0Wqmu7LAocGJG51EejH_DGzNm1PG2C9ml2_ILn_hfjEU2oncXjq3JZv6P4afZudKXAVjDIl-gm5uJrSnDhzU5z4mmBztmsbzvJOaPTTigK"),
        ("Hallowed Chest of the Diretide", "Rare Treasure chest", "Rare", "Treasure Chest", 9, "", "i0CoZ81Ui0m-9KwlBY1L_18myuGuq1wfhWSaZgMttydbPaERSR0Wqmu7LAocGJG51EejH-LGxsuyIGmS2lV98ZTg1V3uUxjll5Llr3Jk7OCvfKEjJvTDXzKSkL915bA8HC2ylk535WTcn9irJ3qWP1B0CZR5FuII50W7ksqnab2Wq9Ge1A"),
    ];
    let rarity_color = |r: &str| match r {
        "Immortal" => "E4AE39",
        "Legendary" => "D32CE6",
        "Mythical" => "8847FF",
        _ => "4B69FF",
    };
    let mut items = Vec::new();
    let mut prices = Vec::new();
    for (i, (name, kind, rarity, slot, cents, hero, icon)) in ITEMS.iter().enumerate() {
        let mut tags = vec![("Rarity".to_string(), rarity.to_string(), Some(rarity_color(rarity).to_string())), ("Type".to_string(), slot.to_string(), None)];
        if !hero.is_empty() {
            tags.push(("Hero".to_string(), hero.to_string(), None));
        }
        let copies = if slot.contains("Chest") { 3 } else { 1 };
        for c in 0..copies {
            items.push(InventoryItem {
                asset_id: 9000 + (i * 10 + c) as u64,
                class_id: 100 + i as u64,
                amount: 1,
                name: name.to_string(),
                kind: kind.to_string(),
                name_color: Some("D2D2D2".into()),
                icon: icon.to_string(),
                tradable: i != 2,
                marketable: i != 2,
                market_hash_name: name.to_string(),
                description: vec![("Used By: ".to_string() + if hero.is_empty() { "All heroes" } else { hero }, None)],
                tags: tags.clone(),
                context_id: 2,
                owner_notes: if i == 2 { vec!["Tradable/Marketable After Nov 14, 2026 (8:00:00) GMT".into()] } else { vec![] },
                trade_hold_days: 7,
                market_hold_days: 7,
                market_fee: Some(0.1),
                ..Default::default()
            });
        }
        // A wavy 60-day price history around the current price.
        let history = (0..60u64)
            .map(|d| {
                let f = 1.0 + 0.18 * ((d as f64) / 6.0).sin() + 0.002 * d as f64;
                (now - (60 - d) * 86_400, *cents as f64 / 100.0 * f, 5 + (d % 9) as u32)
            })
            .collect();
        prices.push(Event::MarketPrice {
            app_id: 570,
            hash_name: name.to_string(),
            result: Ok(MarketPrice {
                lowest: Some(model::format_money(*cents as i64, 1)),
                median: Some(model::format_money((*cents as f64 * 0.97) as i64, 1)),
                volume: Some(40 + i as u64 * 7),
                lowest_sell: Some(*cents),
                highest_buy: Some((*cents as f64 * 0.86) as u64),
                sell_count: Some(380 + i as u64 * 51),
                buy_count: Some(1200 + i as u64 * 97),
                fee_rules: None,
                currency: 1,
                sell_ladder: (0..5).map(|k| (cents + k * 2, 3 + k * 4)).collect(),
                buy_ladder: (0..5).map(|k| (((*cents as f64 * 0.86) as u64).saturating_sub(k * 2).max(1), 10 + k * 7)).collect(),
                history,
            }),
        });
    }
    let mut out = vec![
        Event::InventoryApps {
            steam_id: me,
            result: Ok(vec![
                InventoryApp { app_id: 570, name: "Dota 2".into(), icon: None, contexts: vec![(2, "Backpack".into(), items.len() as u32)] },
                InventoryApp { app_id: 753, name: "Steam".into(), icon: None, contexts: vec![(6, "Community".into(), 0)] },
            ]),
        },
        Event::Inventory(model::Inventory { steam_id: me, app_id: 570, context_id: 2, total: items.len() as u32, items, error: None }),
        Event::Inventory(model::Inventory { steam_id: me, app_id: 753, context_id: 6, ..Default::default() }),
        Event::MyListings(Ok(MyListings {
            listings: vec![
                MyListing { listing_id: 1, app_id: 570, name: "Hydrakan Latch".into(), game: "Dota 2".into(), icon: ITEMS[1].6.into(), name_color: Some("D2D2D2".into()), buyer_pays: 299, you_receive: 260, currency: 1, amount: 1, created: now - 7200, status: model::ListingStatus::Active, market_hash_name: "Hydrakan Latch".into() },
                MyListing { listing_id: 2, app_id: 570, name: "Dead Reckoning Chest".into(), game: "Dota 2".into(), icon: ITEMS[7].6.into(), name_color: Some("D2D2D2".into()), buyer_pays: 32, you_receive: 28, currency: 1, amount: 1, created: now - 600, status: model::ListingStatus::NeedsConfirmation, market_hash_name: "Dead Reckoning Chest".into() },
            ],
            buy_orders: vec![],
        })),
        Event::SupportTickets(Ok(vec![
            SupportTicket { id: "HT-DEMO-0001".into(), title: "Refund Request - Accidentally purchased twice".into(), area: "Dota 2".into(), icon: None, status: "Closed".into(), open: false, updated: "Oct 7".into(), url: "https://help.steampowered.com/en/wizard/HelpRequests".into() },
            SupportTicket { id: "HT-DEMO-0002".into(), title: "My game won't start after the update".into(), area: "Portal 2".into(), icon: None, status: "Waiting for you".into(), open: true, updated: "Nov 2".into(), url: "https://help.steampowered.com/en/wizard/HelpRequests".into() },
        ])),
        Event::SupportTicket {
            id: "HT-DEMO-0002".into(),
            result: Ok(vec![
                SupportMessage { from_steam: false, header: "Message from you on Nov 1 @ 9:12pm".into(), html: "Since the last update the game closes right after the logo.".into() },
                SupportMessage { from_steam: true, header: "Message from Steam Support on Nov 2 @ 10:03am".into(), html: "<b>Thanks for reaching out.</b><br>Please verify the game's files, then tell us if it still happens.<ul><li>Library → Portal 2 → Manage → Verify game files</li></ul>".into() },
            ]),
        },
        // A download in progress, to show install states in the library.
        Event::Download(model::DownloadStatus {
            app_id: 1145360,
            phase: model::DownloadPhase::Downloading,
            downloaded: 6_100_000_000,
            download_total: 13_400_000_000,
            disk_done: 6_100_000_000,
            disk_total: 13_400_000_000,
            speed: 3_200_000,
            install_dir: Some(std::path::PathBuf::from(r"C:\Games\SteamLibrary\steamapps\common\Hades")),
            ..Default::default()
        }),
        Event::PlayerGames {
            steam_id: me,
            result: Ok(vec![model::PlayerGame { app_id: 1145360, name: "Hades".into(), playtime_minutes: 2_710, playtime_2weeks: 185, last_played: Some(now - 3600), icon: None }]),
        },
        Event::Achievements {
            app_id: 1145360,
            result: Ok([
                ("Escaped Tartarus", "Clear Tartarus for the first time.", true, now - 3600, 92.1),
                ("Is There No Escape?", "Escape the Underworld for the first time.", true, now - 86_400 * 4, 64.3),
                ("Friends Forever", "Gain the trust of every one of Zagreus's friends.", false, 0, 18.4),
                ("Secret", "", false, 0, 7.9),
            ]
            .into_iter()
            .map(|(name, desc, unlocked, time, pct)| model::Achievement {
                name: name.into(),
                description: desc.into(),
                hidden: desc.is_empty(),
                unlocked,
                unlock_time: time,
                global_percent: pct,
                ..Default::default()
            })
            .collect()),
        },
        Event::Notifications(model::Notifications {
            items: vec![
                model::Notification { id: 1, kind: 4, time: now - 900, read: false, app_id: Some(753), source_app: Some(570), ..Default::default() },
                model::Notification { id: 2, kind: 8, time: now - 3600 * 5, read: false, app_id: Some(1145360), ..Default::default() },
                model::Notification { id: 3, kind: 11, time: now - 86_400, read: true, ticket: Some("HTDEMO0002".into()), ..Default::default() },
                model::Notification { id: 4, kind: 5, time: now - 86_400 * 3, read: true, from: Some(76561197960265728 + 103), ..Default::default() },
                model::Notification { id: 6, kind: 2, time: now - 600, read: false, app_id: Some(413150), from: Some(76561197960265728 + 100), ..Default::default() },
                model::Notification { id: 5, kind: 3, time: now - 86_400 * 6, read: true, from: Some(me), ..Default::default() },
                model::Notification { id: 7, kind: 28, time: now - 86_400 * 8, read: true, app_id: Some(1145360), ..Default::default() },
            ],
            unread: 3,
            pending_gifts: 1,
            pending_friends: 0,
            confirmations: 1,
        }),
    ];
    out.extend(prices);
    // Human Fall Flat's Workshop tag groups, for the Workshop filters.
    let cat = |name: &str, single: bool, tags: &[&str]| model::WorkshopTagCategory { name: name.into(), single, tags: tags.iter().map(|t| (t.to_string(), t.to_string())).collect() };
    out.push(Event::WorkshopTags {
        app_id: 477160,
        result: Ok(vec![
            cat("", false, &["Human's Finest", "Levels", "Lobbies", "Model", "Aug's Selection", "Monthly Throwback", "New Selection"]),
            cat("Theme", false, &["Mansion", "Train", "Carry", "Mountain", "Demolition", "Castle", "Water", "Power Plant", "Aztec", "Dark", "Steam", "Ice", "Christmas", "Model - Animal", "Model - Anime / Manga", "Model - Cartoon / TV", "Model - Christmas", "Model - Comic Book", "Model - Film", "Model - Horror", "Model - Human", "Model - Sci-fi", "Model - Sport", "Model - Video Game", "Model - Other"]),
            cat("Type", true, &["Puzzle", "Platforming", "Story", "Misc", "Sandbox", "Hide and Seek"]),
            cat("Level Type", true, &["Multiplayer", "Singleplayer", "Speedrun"]),
            cat("Recommended Players", true, &["1 Player", "2 Players", "3 Players", "4 Players", "5 Players", "6 Players", "7 Players", "8 Players"]),
            cat("Difficulty", true, &["Beginner", "Intermediate", "Expert"]),
            cat("Language", true, &["English", "Spanish", "French", "German", "Russian", "Italian", "Chinese", "Japanese", "Korean", "Polish", "Portuguese", "Vietnamese"]),
        ]),
    });
    // Your badges, reviews and groups (`--page=profile --arg=<id>/reviews`).
    let me = 76561197960265729;
    out.push(Event::ProfileEdit(Ok(demo_profile_edit())));
    out.push(Event::Locations { country: None, state: None, result: Ok(demo_locations(None, None)) });
    out.push(Event::Locations { country: Some("US".into()), state: None, result: Ok(demo_locations(Some("US"), None)) });
    out.push(Event::Locations { country: Some("US".into()), state: Some("WA".into()), result: Ok(demo_locations(Some("US"), Some("WA"))) });
    let badge = |title: &str, detail: &str, unlocked: &str| model::Badge { title: title.into(), detail: detail.into(), unlocked: unlocked.into(), ..Default::default() };
    out.push(Event::Badges {
        steam_id: me,
        result: Ok(model::Badges {
            level: 18,
            xp: "1,915 XP".into(),
            next: "85 XP to reach Level 19".into(),
            progress: 0.6,
            items: vec![
                model::Badge { url: format!("https://steamcommunity.com/profiles/{me}/gamecards/413150/"), ..badge("Stardew Valley Badge", "Level 2, 200 XP", "Unlocked 9 Mar, 2023") },
                model::Badge { url: format!("https://steamcommunity.com/profiles/{me}/badges/1"), ..badge("Years of Service", "300 XP", "Unlocked 3 Feb, 2019") },
                model::Badge { url: format!("https://steamcommunity.com/profiles/{me}/badges/2"), ..badge("Community Leader", "100 XP", "Unlocked 14 Jun, 2021") },
                model::Badge { url: format!("https://steamcommunity.com/profiles/{me}/badges/13"), ..badge("Game Collector", "250 XP", "Unlocked 2 Jan, 2024") },
            ],
        }),
    });
    // Its card collection (`--page=profile --arg=<id>/cards/413150`).
    let card = |name: &str, n: u32, count: u32| model::Card { name: name.into(), count, series: format!("{n} of 6, Series 1"), image: model::art::header(413150) };
    out.push(Event::CardSet {
        steam_id: me,
        app_id: 413150,
        foil: false,
        result: Ok(model::CardSet {
            title: "Stardew Valley Badge".into(),
            badge: Some(model::CardBadge { name: "Farmhand".into(), icon: None, level: "Level 2, 200 XP".into(), unlocked: "Unlocked 9 Mar, 2023".into() }),
            playtime: "212 hrs on record".into(),
            drops: "No card drops remaining".into(),
            cards: vec![card("Abigail", 1, 2), card("Linus", 2, 0), card("Shane", 3, 1), card("Krobus", 4, 0), card("Pierre's Shop", 5, 3), card("Spring Festival", 6, 1)],
        }),
    });
    let review = |app_id, recommended, hours: &str, text: &str| model::UserReview {
        app_id,
        recommended,
        hours: hours.into(),
        posted: "Posted 12 October, 2025.".into(),
        text: text.into(),
        helpful: "12 people found this review helpful".into(),
        ..Default::default()
    };
    out.push(Event::Reviews {
        steam_id: me,
        page: 1,
        result: Ok(model::UserReviews {
            items: vec![
                review(413150, true, "212.4 hrs on record", "The coziest farm sim there is. Fishing still beats me."),
                review(1145360, true, "88.1 hrs on record (40.2 hrs at review time)", "Every run feels different, and the music is incredible."),
                review(570, false, "1,204.9 hrs on record", "One more game, they said."),
            ],
            total: 3,
        }),
    });
    let group = |name: &str, members: &str, online: &str| model::UserGroup { name: name.into(), url: "https://steamcommunity.com/groups/".into(), members: members.into(), online: online.into(), avatar: None };
    out.push(Event::Groups { steam_id: me, result: Ok(vec![group("Aurora Testers", "1,204 Members", "88 Online"), group("Cozy Farmers", "52,310 Members", "3,021 Online")]) });

    // Demo store state: wishlist, empty cart, discovery queue, and offline game reviews.
    out.push(Event::Wishlist(vec![
        (1245620, now - 86_400 * 60),
        (268910, now - 86_400 * 30),
        (646570, now - 86_400 * 14),
        (367520, now - 86_400 * 7),
    ]));
    out.push(Event::Cart(Vec::new()));
    out.push(Event::DiscoveryQueue(Ok(vec![1145360, 413150, 367520, 646570, 268910, 477160])));
    let sample_reviews = model::AppReviews {
        label: "Overwhelmingly Positive".into(),
        positive: 98,
        negative: 2,
        total: 100,
        cursor: None,
        items: vec![
            model::AppReview {
                id: "demo-1".into(),
                author: 76561197960265729 + 100,
                minutes_played: 5_400,
                minutes_at_review: 1_800,
                recommended: true,
                text: "Crisp combat, reactive writing, and runs effortlessly on every setup I've tried.".into(),
                created: now - 86_400 * 2,
                helpful: 142,
                funny: 12,
                early_access: false,
                free: false,
            },
            model::AppReview {
                id: "demo-2".into(),
                author: 76561197960265729 + 101,
                minutes_played: 12_200,
                minutes_at_review: 9_000,
                recommended: true,
                text: "One more run turned into 200 hours. Dialogue reactivity stays fresh well past the credits.".into(),
                created: now - 86_400 * 12,
                helpful: 89,
                funny: 64,
                early_access: false,
                free: false,
            },
            model::AppReview {
                id: "demo-3".into(),
                author: 76561197960265729 + 102,
                minutes_played: 420,
                minutes_at_review: 360,
                recommended: false,
                text: "Love the art and music, though later difficulty spikes require more grinding than I expected.".into(),
                created: now - 86_400 * 20,
                helpful: 27,
                funny: 4,
                early_access: false,
                free: false,
            },
        ],
    };
    for id in [1145360, 413150, 367520, 646570, 268910, 477160, 1245620, 570] {
        out.push(Event::AppReviews { app_id: id, filter: model::ReviewFilter::default(), more: false, result: Ok(sample_reviews.clone()) });
    }
    out
}

fn demo_profile_edit() -> model::ProfileEdit {
    let item = |base_id: u64, i: usize, app: u32, name: &str, game: &str, image: String, large: String| model::ProfileItemChoice {
        id: base_id + i as u64,
        name: name.into(),
        image,
        large,
        app_id: app,
        app_name: game.into(),
        ..Default::default()
    };
    model::ProfileEdit {
        persona_name: "You".into(),
        real_name: "Demo Player".into(),
        custom_url: "demo-player".into(),
        summary: "Collector of indie gems and long co-op nights.\n\nCurrently: replaying Hades on the hardest heat, slowly finishing Stardew Valley's community center, and trying every Portal 2 workshop map my friends send me.\n\nAdd me if you want to play co-op, trade cards or just chat about games. I'm usually online in the evenings.".into(),
        country: "US".into(),
        state: "WA".into(),
        city: "47692".into(),
        location_text: "Seattle, Washington, United States".into(),
        privacy: model::ProfilePrivacy { profile: 3, inventory: 3, gifts: 1, games: 3, playtime: 1, friends: 2, comments: 1 },
        themes: ["Summer", "Midnight", "Steel", "Cosmic", "DarkMode"]
            .iter()
            .map(|n| (n.to_string(), format!("#ProfileTheme_{n}")))
            .collect(),
        theme: "Midnight".into(),
        backgrounds: [
            (1145360, "Hades Underworld", "Hades"),
            (413150, "Pelican Town", "Stardew Valley"),
            (620, "Aperture Labs", "Portal 2"),
            (440, "Mann Co. Workshop", "Team Fortress 2"),
            (570, "Dire Ancient", "Dota 2"),
        ]
        .iter()
        .enumerate()
        .map(|(i, (app, name, game))| item(9100, i, *app, name, game, model::art::header(*app), model::art::hero(*app)))
        .collect(),
        mini_backgrounds: [
            (1145360, "Elysium Fields", "Hades"),
            (413150, "Starry Farm", "Stardew Valley"),
            (730, "Dust II Dawn", "Counter-Strike 2"),
        ]
        .iter()
        .enumerate()
        .map(|(i, (app, name, game))| item(9200, i, *app, name, game, model::art::header(*app), model::art::hero(*app)))
        .collect(),
        frames: [
            (1145360, "Underworld Flame", "Hades"),
            (620, "Aperture Portal", "Portal 2"),
            (570, "Aegis Crest", "Dota 2"),
        ]
        .iter()
        .enumerate()
        .map(|(i, (app, name, game))| item(9400, i, *app, name, game, model::art::capsule(*app), String::new()))
        .collect(),
        animated_avatars: [
            (413150, "Junimo Hop", "Stardew Valley"),
            (1145360, "Zagreus", "Hades"),
            (620, "Turret Blink", "Portal 2"),
            (440, "Heavy Grin", "Team Fortress 2"),
            (570, "Roshan Roar", "Dota 2"),
        ]
        .iter()
        .enumerate()
        .map(|(i, (app, name, game))| item(9300, i, *app, name, game, model::art::capsule(*app), String::new()))
        .collect(),
        equipped: [9100, 9200, 0, 0],
        background_full: true,
        favorite_badge: 2,
        ..Default::default()
    }
}

fn demo_locations(country: Option<&str>, state: Option<&str>) -> Vec<(String, String)> {
    match (country, state) {
        (None, None) => vec![
            ("US".into(), "United States".into()),
            ("CA".into(), "Canada".into()),
            ("GB".into(), "United Kingdom".into()),
            ("FR".into(), "France".into()),
            ("DE".into(), "Germany".into()),
            ("ES".into(), "Spain".into()),
            ("JP".into(), "Japan".into()),
            ("KZ".into(), "Kazakhstan".into()),
        ],
        (Some("US"), None) => vec![("CA".into(), "California".into()), ("NY".into(), "New York".into()), ("WA".into(), "Washington".into())],
        (Some("CA"), None) => vec![("BC".into(), "British Columbia".into()), ("ON".into(), "Ontario".into())],
        (Some("KZ"), None) => vec![("02".into(), "Almaty".into()), ("01".into(), "Astana".into())],
        (Some("US"), Some("WA")) => vec![("47692".into(), "Seattle".into()), ("47634".into(), "Bellevue".into())],
        (Some("US"), Some("CA")) => vec![("4515".into(), "Los Angeles".into()), ("4801".into(), "San Francisco".into())],
        (Some("US"), Some("NY")) => vec![("31400".into(), "New York".into())],
        (Some("CA"), Some("BC")) => vec![("8432".into(), "Vancouver".into())],
        (Some("CA"), Some("ON")) => vec![("8390".into(), "Toronto".into())],
        (Some("KZ"), Some("02")) => vec![("1234".into(), "Almaty".into())],
        _ => Vec::new(),
    }
}

fn format_demo_location(country: &str, state: &str, city: &str) -> String {
    let country_name = demo_locations(None, None).into_iter().find(|(c, _)| c == country).map(|(_, n)| n).unwrap_or_else(|| country.to_string());
    let state_name = if country.is_empty() || state.is_empty() {
        String::new()
    } else {
        demo_locations(Some(country), None).into_iter().find(|(c, _)| c == state).map(|(_, n)| n).unwrap_or_else(|| state.to_string())
    };
    let city_name = if country.is_empty() || state.is_empty() || city.is_empty() {
        String::new()
    } else {
        demo_locations(Some(country), Some(state)).into_iter().find(|(c, _)| c == city).map(|(_, n)| n).unwrap_or_else(|| city.to_string())
    };
    [city_name, state_name, country_name].into_iter().filter(|s| !s.is_empty()).collect::<Vec<_>>().join(", ")
}

/// Start an offline interactive `--demo` session that seeds demo state and
/// handles profile-edit commands in memory without touching Steam.
pub fn spawn(rt: &tokio::runtime::Handle, local: &local::LocalState, sink: client::Sink) -> client::Client {
    use client::Command;
    let initial = events(local);
    let mut demo_profile = initial
        .iter()
        .find_map(|e| match e {
            Event::Profile(p) => Some(p.clone()),
            _ => None,
        })
        .unwrap_or_default();
    let mut demo_edit = demo_profile_edit();
    for e in initial {
        sink(e);
    }
    let (tx, mut rx) = tokio::sync::mpsc::unbounded_channel();
    rt.spawn(async move {
        let me = demo_profile.steam_id;
        let mut ordinal = 0;
        let now = || std::time::SystemTime::now().duration_since(std::time::UNIX_EPOCH).unwrap_or_default().as_secs();
        while let Some(cmd) = rx.recv().await {
            match cmd {
                Command::SendMessage { to, text } => {
                    let parsed = model::chat::parse_message(&text);
                    sink(Event::Message(ChatMessage {
                        friend: to,
                        from: me,
                        text: parsed.text,
                        time: now(),
                        invite: parsed.invite,
                        group_invite: parsed.group_invite,
                        media: parsed.media,
                    }));
                }
                Command::SendChatAttachment { .. } => {
                    sink(Event::ChatAttachmentFailed("Attachments require a Steam connection.".into()));
                }
                Command::SendGroupMessage { group_id, chat_id, text } => {
                    ordinal += 1;
                    let parsed = model::chat::parse_message(&text);
                    sink(Event::GroupMessage(GroupMessage {
                        group_id,
                        chat_id,
                        from: me,
                        text: parsed.text,
                        time: now(),
                        ordinal,
                        invite: parsed.invite,
                        group_invite: parsed.group_invite,
                        deleted: false,
                        server_msg: None,
                        reactions: Vec::new(),
                        mentioned_me: false,
                        media: parsed.media,
                    }));
                }
                Command::LoadProfileEdit { .. } => sink(Event::ProfileEdit(Ok(demo_edit.clone()))),
                Command::LoadProfile { steam_id } if steam_id == me => sink(Event::Profile(demo_profile.clone())),
                Command::LoadLocations { country, state } => {
                    let list = demo_locations(country.as_deref(), state.as_deref());
                    sink(Event::Locations { country, state, result: Ok(list) });
                }
                Command::SaveProfileInfo(e) => {
                    let name_changed = demo_edit.persona_name != e.persona_name;
                    demo_edit.persona_name = e.persona_name;
                    demo_edit.real_name = e.real_name;
                    demo_edit.custom_url = e.custom_url;
                    demo_edit.country = e.country;
                    demo_edit.state = e.state;
                    demo_edit.city = e.city;
                    demo_edit.summary = e.summary;
                    demo_edit.hide_awards = e.hide_awards;
                    demo_edit.location_text = format_demo_location(&demo_edit.country, &demo_edit.state, &demo_edit.city);

                    demo_profile.real_name = demo_edit.real_name.clone();
                    demo_profile.summary = demo_edit.summary.clone();
                    demo_profile.location = demo_edit.location_text.clone();
                    if name_changed {
                        sink(Event::AccountInfo { persona_name: demo_edit.persona_name.clone(), country: "US".into() });
                    }
                    sink(Event::Profile(demo_profile.clone()));
                    sink(Event::ProfileEdit(Ok(demo_edit.clone())));
                    sink(Event::ProfileSaved { what: model::ProfileSave::Info, result: Ok(()) });
                }
                Command::SaveProfilePrivacy(p) => {
                    demo_edit.privacy = p;
                    sink(Event::ProfileEdit(Ok(demo_edit.clone())));
                    sink(Event::ProfileSaved { what: model::ProfileSave::Privacy, result: Ok(()) });
                }
                Command::UploadAvatar { path } => {
                    let result = client::profile_edit::read_avatar_file(&path).map(|_| ());
                    sink(Event::ProfileSaved { what: model::ProfileSave::Avatar, result });
                }
                Command::EquipProfileItem { kind, id } => {
                    if let Some(slot) = demo_edit.equipped.get_mut(kind as usize) {
                        *slot = id;
                    }
                    let find_url = |list: &[model::ProfileItemChoice], large: bool| {
                        list.iter()
                            .find(|i| i.id == id)
                            .map(|i| if large && !i.large.is_empty() { i.large.clone() } else { i.image.clone() })
                    };
                    match kind {
                        0 => demo_profile.background = find_url(&demo_edit.backgrounds, true),
                        1 => demo_profile.mini_background = find_url(&demo_edit.mini_backgrounds, true),
                        2 => demo_profile.avatar_frame = find_url(&demo_edit.frames, false),
                        3 => demo_profile.animated_avatar = find_url(&demo_edit.animated_avatars, false),
                        _ => {}
                    }
                    sink(Event::Profile(demo_profile.clone()));
                    sink(Event::ProfileEdit(Ok(demo_edit.clone())));
                    sink(Event::ProfileSaved { what: model::ProfileSave::Item, result: Ok(()) });
                }
                Command::SetBackgroundFullScreen { id, full } => {
                    if demo_edit.equipped[0] == id {
                        demo_edit.background_full = full;
                        demo_profile.background_full = full;
                    }
                    sink(Event::Profile(demo_profile.clone()));
                    sink(Event::ProfileEdit(Ok(demo_edit.clone())));
                    sink(Event::ProfileSaved { what: model::ProfileSave::BackgroundFlags, result: Ok(()) });
                }
                Command::SetFavoriteBadge { badge } => {
                    demo_edit.favorite_badge = badge;
                    sink(Event::ProfileEdit(Ok(demo_edit.clone())));
                    sink(Event::ProfileSaved { what: model::ProfileSave::Badge, result: Ok(()) });
                }
                Command::SetProfileTheme(theme) => {
                    demo_edit.theme = theme.clone();
                    demo_profile.theme = theme;
                    sink(Event::Profile(demo_profile.clone()));
                    sink(Event::ProfileEdit(Ok(demo_edit.clone())));
                    sink(Event::ProfileSaved { what: model::ProfileSave::Theme, result: Ok(()) });
                }
                _ => {}
            }
        }
    });
    client::Client::from_sender(tx)
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::time::Duration;

    #[test]
    fn demo_spawn_handles_profile_edit_commands() {
        let rt = tokio::runtime::Builder::new_current_thread().enable_time().build().unwrap();
        rt.block_on(async {
            let (tx, mut rx) = tokio::sync::mpsc::unbounded_channel();
            let sink: client::Sink = std::sync::Arc::new(move |e| {
                let _ = tx.send(e);
            });
            let client = spawn(rt.handle(), &local::LocalState::default(), sink);

            async fn recv_until(
                rx: &mut tokio::sync::mpsc::UnboundedReceiver<Event>,
                expected: model::ProfileSave,
            ) -> Vec<Event> {
                let mut out = Vec::new();
                tokio::time::timeout(Duration::from_secs(2), async {
                    while let Some(ev) = rx.recv().await {
                        let done = matches!(&ev, Event::ProfileSaved { what, result: Ok(()) } if *what == expected);
                        out.push(ev);
                        if done {
                            break;
                        }
                    }
                })
                .await
                .expect("timed out waiting for demo ProfileSaved event");
                out
            }

            // Drain initial seed events before testing command round-trips.
            let mut initial_has_profile = false;
            while let Ok(ev) = rx.try_recv() {
                if matches!(ev, Event::Profile(_)) {
                    initial_has_profile = true;
                }
            }
            assert!(initial_has_profile);

            client.send(client::Command::LoadProfileEdit { language: "english".into() });
            let ev = tokio::time::timeout(Duration::from_secs(2), rx.recv()).await.unwrap().unwrap();
            let mut edit = match ev {
                Event::ProfileEdit(Ok(e)) => e,
                other => panic!("expected ProfileEdit, got {other:?}"),
            };
            assert_eq!(edit.persona_name, "You");
            assert_eq!(edit.theme, "Midnight");

            client.send(client::Command::LoadLocations { country: Some("US".into()), state: Some("WA".into()) });
            let ev = tokio::time::timeout(Duration::from_secs(2), rx.recv()).await.unwrap().unwrap();
            match ev {
                Event::Locations { country, state, result: Ok(list) } => {
                    assert_eq!(country.as_deref(), Some("US"));
                    assert_eq!(state.as_deref(), Some("WA"));
                    assert!(list.iter().any(|(c, n)| c == "47692" && n == "Seattle"));
                }
                other => panic!("expected Locations, got {other:?}"),
            }

            edit.persona_name = "AuroraTester".into();
            edit.real_name = "Alice".into();
            edit.summary = "Updated bio".into();
            edit.country = "CA".into();
            edit.state = "BC".into();
            edit.city = "8432".into();
            client.send(client::Command::SaveProfileInfo(Box::new(edit.clone())));
            let info_events = recv_until(&mut rx, model::ProfileSave::Info).await;
            assert!(info_events.iter().any(|e| matches!(e, Event::AccountInfo { persona_name, .. } if persona_name == "AuroraTester")));
            assert!(info_events.iter().any(|e| matches!(e, Event::Profile(p) if p.real_name == "Alice" && p.summary == "Updated bio" && p.location == "Vancouver, British Columbia, Canada")));
            assert!(info_events.iter().any(|e| matches!(e, Event::ProfileEdit(Ok(pe)) if pe.persona_name == "AuroraTester" && pe.location_text == "Vancouver, British Columbia, Canada")));

            let new_privacy = model::ProfilePrivacy { profile: 2, inventory: 2, gifts: 0, games: 2, playtime: 0, friends: 1, comments: 2 };
            client.send(client::Command::SaveProfilePrivacy(new_privacy.clone()));
            let priv_events = recv_until(&mut rx, model::ProfileSave::Privacy).await;
            assert!(priv_events.iter().any(|e| matches!(e, Event::ProfileEdit(Ok(pe)) if pe.privacy == new_privacy)));

            client.send(client::Command::EquipProfileItem { kind: 0, id: 9101 });
            let item_events = recv_until(&mut rx, model::ProfileSave::Item).await;
            assert!(item_events.iter().any(|e| matches!(e, Event::ProfileEdit(Ok(pe)) if pe.equipped[0] == 9101)));
            assert!(item_events.iter().any(|e| matches!(e, Event::Profile(p) if p.background.as_deref().is_some_and(|u| u.contains("413150")))));

            client.send(client::Command::SetBackgroundFullScreen { id: 9101, full: false });
            let bg_events = recv_until(&mut rx, model::ProfileSave::BackgroundFlags).await;
            assert!(bg_events.iter().any(|e| matches!(e, Event::Profile(p) if !p.background_full)));
            assert!(bg_events.iter().any(|e| matches!(e, Event::ProfileEdit(Ok(pe)) if !pe.background_full)));

            client.send(client::Command::SetFavoriteBadge { badge: 13 });
            let badge_events = recv_until(&mut rx, model::ProfileSave::Badge).await;
            assert!(badge_events.iter().any(|e| matches!(e, Event::ProfileEdit(Ok(pe)) if pe.favorite_badge == 13)));

            client.send(client::Command::SetProfileTheme("Cosmic".into()));
            let theme_events = recv_until(&mut rx, model::ProfileSave::Theme).await;
            assert!(theme_events.iter().any(|e| matches!(e, Event::Profile(p) if p.theme == "Cosmic")));
            assert!(theme_events.iter().any(|e| matches!(e, Event::ProfileEdit(Ok(pe)) if pe.theme == "Cosmic")));
        });
    }
}
