//! Profile showcases, like Steam's profile page: which showcases a profile
//! has (`Player.GetProfileCustomization`), plus what their slots point at:
//! workshop files and screenshots, the favorite group, and achievements.

use model::{AppId, Showcase, ShowcaseAchievement, ShowcaseFile, ShowcaseGame, ShowcaseGroup, ShowcaseSlot, SteamId, showcase as kind};
use net::Connection;
use proto::*;
use std::collections::{BTreeSet, HashMap};

const CLAN_BASE: u64 = 103582791429521408;
/// Games whose achievements are looked up per profile.
const MAX_GAMES: usize = 8;

pub async fn load(conn: &Connection, steam_id: SteamId, language: &str) -> Result<Vec<Showcase>, String> {
    let r: CPlayerGetProfileCustomizationResponse = conn
        .call(
            "Player.GetProfileCustomization#1",
            &CPlayerGetProfileCustomizationRequest { steamid: Some(steam_id), include_inactive_customizations: Some(false), include_purchased_customizations: Some(false) },
        )
        .await
        .map_err(|e| e.to_string())?;
    let mut list: Vec<Showcase> = r
        .customizations
        .iter()
        .filter(|c| c.active.unwrap_or(true))
        .map(|c| Showcase {
            kind: c.customization_type.unwrap_or(0),
            style: c.customization_style.unwrap_or(0),
            slots: c
                .slots
                .iter()
                .map(|s| ShowcaseSlot {
                    app_id: s.appid(),
                    file: (s.publishedfileid() != 0).then(|| ShowcaseFile { id: s.publishedfileid(), ..Default::default() }),
                    asset_id: s.item_assetid(),
                    context_id: s.item_contextid(),
                    title: s.title().to_string(),
                    notes: s.notes().to_string(),
                    account_id: s.accountid(),
                    badge_id: s.badgeid(),
                    border_color: s.border_color(),
                    achievement: None,
                })
                .collect(),
            ..Default::default()
        })
        .filter(|s| s.kind != 0)
        .collect();

    // Files: screenshots, artwork, workshop items, guides.
    let ids: Vec<u64> = list.iter().flat_map(|s| s.slots.iter().filter_map(|x| x.file.as_ref().map(|f| f.id))).collect::<BTreeSet<_>>().into_iter().collect();
    if !ids.is_empty() {
        let req = CPublishedFileGetDetailsRequest { publishedfileids: ids, short_description: Some(true), ..Default::default() };
        match conn.call::<_, CPublishedFileGetDetailsResponse>("PublishedFile.GetDetails#1", &req).await {
            Ok(d) => {
                let found: HashMap<u64, &PublishedFileDetails> = d.publishedfiledetails.iter().filter(|f| f.result() == 1).map(|f| (f.publishedfileid(), f)).collect();
                for s in list.iter_mut() {
                    // Artwork and screenshots: the picture itself at its own
                    // shape (and still animated), fit to the showcase width
                    // like Steam's page; other files: their preview.
                    let full = matches!(s.kind, kind::ARTWORK | kind::SINGLE_ARTWORK | kind::SCREENSHOTS);
                    for slot in s.slots.iter_mut() {
                        let Some(file) = slot.file.as_mut() else { continue };
                        if let Some(d) = found.get(&file.id) {
                            file.title = d.title().to_string();
                            file.app_id = d.consumer_appid();
                            let fitted = full.then(|| d.file_url()).filter(|u| u.starts_with("https://")).map(fit_width);
                            file.image = fitted.or_else(|| [d.preview_url(), d.file_url()].into_iter().find(|u| u.starts_with("https://")).map(String::from));
                        }
                    }
                }
            }
            Err(e) => tracing::debug!("showcase files: {e}"),
        }
    }

    // Favorite group.
    for s in list.iter_mut().filter(|s| s.kind == kind::FAVORITE_GROUP) {
        if let Some(account) = s.slots.iter().map(|x| x.account_id).find(|a| *a != 0) {
            s.group = group(CLAN_BASE + account as u64).await;
        }
    }

    // Achievements: favorite game, achievement and completionist showcases.
    let mut apps: Vec<AppId> = Vec::new();
    for s in &list {
        if matches!(s.kind, kind::FAVORITE_GAME | kind::RARE_ACHIEVEMENTS | kind::ACHIEVEMENTS | kind::COMPLETIONIST) {
            for a in s.slots.iter().map(|x| x.app_id).filter(|a| *a != 0) {
                if !apps.contains(&a) && apps.len() < MAX_GAMES {
                    apps.push(a);
                }
            }
        }
    }
    let mut by_app: HashMap<AppId, (ShowcaseGame, HashMap<String, ShowcaseAchievement>)> = HashMap::new();
    for app in apps {
        if let Some(found) = achievements(conn, steam_id, app, language).await {
            by_app.insert(app, found);
        }
    }
    for s in &mut list {
        let mut games = Vec::new();
        for slot in &mut s.slots {
            let Some((game, named)) = by_app.get(&slot.app_id) else { continue };
            if !slot.title.is_empty() {
                slot.achievement = named.get(&slot.title).cloned();
            }
            if matches!(s.kind, kind::FAVORITE_GAME | kind::COMPLETIONIST) && !games.iter().any(|g: &ShowcaseGame| g.app_id == slot.app_id) {
                games.push(game.clone());
            }
        }
        s.games = games;
    }
    Ok(list)
}

/// A user-content picture resized by Steam to the showcase width, keeping
/// its shape and animation (what Steam's profile page asks for).
fn fit_width(url: &str) -> String {
    let base = url.split('?').next().unwrap_or(url);
    format!("{base}?imw=800&ima=fit&impolicy=Letterbox&letterbox=false")
}

/// A game's achievements for one profile: progress, and each achievement by
/// its API name.
async fn achievements(conn: &Connection, steam_id: SteamId, app_id: AppId, language: &str) -> Option<(ShowcaseGame, HashMap<String, ShowcaseAchievement>)> {
    let schema: CPlayerGetGameAchievementsResponse = conn
        .call("Player.GetGameAchievements#1", &CPlayerGetGameAchievementsRequest { appid: Some(app_id), language: Some(language.to_string()), hash_only: Some(false) })
        .await
        .ok()?;
    let theirs: Option<CPlayerGetUserAchievementsResponse> =
        conn.call("Player.GetUserAchievements#1", &CPlayerGetUserAchievementsRequest { steamid: Some(steam_id), appid: Some(app_id) }).await.ok();
    let unlocked: HashMap<u32, u64> = theirs.map(|m| m.achievements.iter().filter(|a| a.unlocked()).map(|a| (a.internal_key(), a.unlock_time() as u64)).collect()).unwrap_or_default();
    let image = |file: &str| if file.is_empty() { String::new() } else { format!("https://cdn.akamai.steamstatic.com/steamcommunity/public/images/apps/{app_id}/{file}") };
    let mut game = ShowcaseGame { app_id, ..Default::default() };
    let mut named = HashMap::new();
    let mut got: Vec<(f32, ShowcaseAchievement)> = Vec::new();
    for a in schema.achievements.iter().filter(|a| !a.archived()) {
        let percent: f32 = a.player_percent_unlocked().parse().unwrap_or(100.0);
        let has = unlocked.contains_key(&a.internal_key());
        let entry = ShowcaseAchievement {
            name: a.localized_name().to_string(),
            description: a.localized_desc().to_string(),
            icon: image(if has || a.icon_gray().is_empty() { a.icon() } else { a.icon_gray() }),
            percent,
        };
        game.total += 1;
        if has {
            game.unlocked += 1;
            if percent < 10.0 {
                game.rare += 1;
            }
            got.push((percent, entry.clone()));
        }
        named.insert(a.internal_name().to_string(), ShowcaseAchievement { icon: image(a.icon()), ..entry });
    }
    got.sort_by(|a, b| a.0.total_cmp(&b.0));
    game.recent = got.into_iter().take(6).map(|g| g.1).collect();
    (game.total > 0).then_some((game, named))
}

/// A Steam group's name, avatar and member counts (its public XML).
async fn group(gid: u64) -> Option<ShowcaseGroup> {
    let body = crate::http()
        .get(format!("https://steamcommunity.com/gid/{gid}/memberslistxml/?xml=1"))
        .send()
        .await
        .ok()?
        .text()
        .await
        .ok()?;
    parse_group(&body)
}

fn xml_field(body: &str, tag: &str) -> Option<String> {
    let open = format!("<{tag}>");
    let start = body.find(&open)? + open.len();
    let end = body[start..].find(&format!("</{tag}>"))? + start;
    let raw = body[start..end].trim();
    let raw = raw.strip_prefix("<![CDATA[").and_then(|r| r.strip_suffix("]]>")).unwrap_or(raw);
    Some(decode_entities(raw.trim()))
}

/// Steam HTML-escapes group names even inside CDATA ("yanna &lt;3").
fn decode_entities(s: &str) -> String {
    let mut out = s.to_string();
    for (from, to) in [("&lt;", "<"), ("&gt;", ">"), ("&quot;", "\""), ("&#39;", "'"), ("&#039;", "'"), ("&apos;", "'"), ("&nbsp;", " ")] {
        out = out.replace(from, to);
    }
    out.replace("&amp;", "&")
}

fn parse_group(body: &str) -> Option<ShowcaseGroup> {
    let num = |tag: &str| xml_field(body, tag).and_then(|v| v.replace(',', "").parse().ok()).unwrap_or(0);
    let name = xml_field(body, "groupName").filter(|n| !n.is_empty())?;
    let url = xml_field(body, "groupURL").map(|u| format!("https://steamcommunity.com/groups/{u}")).unwrap_or_default();
    Some(ShowcaseGroup {
        name,
        headline: xml_field(body, "headline").unwrap_or_default(),
        url,
        avatar: xml_field(body, "avatarFull").filter(|a| a.starts_with("https://")),
        members: num("memberCount"),
        in_game: num("membersInGame"),
        online: num("membersOnline"),
        in_chat: num("membersInChat"),
    })
}

#[cfg(test)]
mod tests {
    #[test]
    fn reads_group_xml() {
        let xml = r#"<?xml version="1.0" encoding="UTF-8" standalone="yes"?><memberList><groupID64>103582791429521408</groupID64><groupDetails><groupName><![CDATA[yanna <3]]></groupName><groupURL><![CDATA[yannaa]]></groupURL><avatarFull><![CDATA[https://avatars.example/x_full.jpg]]></avatarFull><memberCount>2</memberCount><membersInChat>0</membersInChat><membersInGame>0</membersInGame><membersOnline>2</membersOnline></groupDetails></memberList>"#;
        let g = super::parse_group(xml).unwrap();
        assert_eq!(g.name, "yanna <3");
        assert_eq!(g.url, "https://steamcommunity.com/groups/yannaa");
        assert_eq!(g.avatar.as_deref(), Some("https://avatars.example/x_full.jpg"));
        assert_eq!((g.members, g.in_game, g.online, g.in_chat), (2, 0, 2, 0));
        assert!(super::parse_group("<memberList></memberList>").is_none());
        // Steam escapes names inside CDATA too.
        let escaped = xml.replace("yanna <3", "yanna &lt;3 &amp; co");
        assert_eq!(super::parse_group(&escaped).unwrap().name, "yanna <3 & co");
    }
}
