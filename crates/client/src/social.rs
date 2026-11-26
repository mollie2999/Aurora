//! Profile comments and badges from steamcommunity.com (no API for these).

use crate::market::Web;
use model::SteamId;
use serde_json::Value;

const COMMUNITY: &str = "https://steamcommunity.com";
const BASE_ID: u64 = 76561197960265728;

fn http(web: Option<&Web>) -> reqwest::Client {
    web.map(|w| w.http.clone()).unwrap_or_default()
}

/// A page of comments, newest first. `Err` carries Steam's reason, e.g.
/// "This profile is private." or comments being turned off.
pub async fn comments(web: Option<&Web>, steam_id: SteamId, start: u32) -> Result<model::Comments, String> {
    let v: Value = http(web)
        .get(format!("{COMMUNITY}/comment/Profile/render/{steam_id}/-1/"))
        .query(&[("start", start.to_string()), ("count", "20".to_string())])
        .send()
        .await
        .map_err(|e| e.to_string())?
        .json()
        .await
        .map_err(|_| "Comments aren't available".to_string())?;
    page(&v)
}

fn page(v: &Value) -> Result<model::Comments, String> {
    if v.get("success").and_then(|s| s.as_bool()) != Some(true) {
        return Err(v.get("error").and_then(|e| e.as_str()).unwrap_or("Comments aren't available").to_string());
    }
    let html = v.get("comments_html").and_then(|h| h.as_str()).unwrap_or_default();
    let total = v.get("total_count").and_then(|n| n.as_u64().or_else(|| n.as_str()?.parse().ok())).unwrap_or(0) as u32;
    Ok(model::Comments { items: parse_comments(html), total })
}

fn between<'a>(s: &'a str, start: &str, end: &str) -> Option<&'a str> {
    let a = s.find(start)? + start.len();
    let b = s[a..].find(end)? + a;
    Some(&s[a..b])
}

pub fn parse_comments(html: &str) -> Vec<model::Comment> {
    html.split("class=\"commentthread_comment ")
        .skip(1)
        .filter_map(|block| {
            let id = between(block, "id=\"comment_", "\"")?.parse().ok()?;
            // The author's link carries their 32-bit account id.
            let author_block = &block[block.find("commentthread_author_link")?..];
            let account: u64 = between(author_block, "data-miniprofile=\"", "\"")?.parse().ok()?;
            let name = between(author_block, "<bdi>", "</bdi>").unwrap_or_default().trim().to_string();
            // Avatar: first image inside the avatar link (after any frame).
            let avatar = block.find("<a href=").and_then(|i| {
                let rest = &block[i..];
                let url_at = ["srcset=\"", "src=\""].iter().filter_map(|k| rest.find(k).map(|p| p + k.len())).min()?;
                let url: String = rest[url_at..].chars().take_while(|c| *c != '"' && *c != ' ').collect();
                url.starts_with("http").then_some(url)
            });
            let time = between(block, "data-timestamp=\"", "\"").and_then(|t| t.parse().ok()).unwrap_or(0);
            let text_start = block.find("class=\"commentthread_comment_text\"")?;
            let text = between(&block[text_start..], ">", "</div>").unwrap_or_default().trim().to_string();
            Some(model::Comment { id, author: account + BASE_ID, name: unescape(&name), avatar, time, html: text })
        })
        .collect()
}

fn unescape(s: &str) -> String {
    s.replace("&amp;", "&").replace("&quot;", "\"").replace("&#39;", "'").replace("&lt;", "<").replace("&gt;", ">")
}

/// Leave a comment. Needs the signed-in web session.
pub async fn post_comment(web: &Web, steam_id: SteamId, text: &str) -> Result<model::Comments, String> {
    let v: Value = web
        .http
        .post(format!("{COMMUNITY}/comment/Profile/post/{steam_id}/-1/"))
        .header("Referer", format!("{COMMUNITY}/profiles/{steam_id}/"))
        .form(&[("comment", text), ("count", "20"), ("sessionid", web.session_id.as_str()), ("feature2", "-1")])
        .send()
        .await
        .map_err(|e| e.to_string())?
        .json()
        .await
        .map_err(|e| e.to_string())?;
    page(&v)
}

/// Block (or unblock) all communication with someone, like the profile's
/// "Block all communication".
pub async fn block_user(web: &Web, steam_id: SteamId, block: bool) -> Result<bool, String> {
    let resp = web
        .http
        .post(format!("{COMMUNITY}/actions/BlockUserAjax"))
        .header("Referer", format!("{COMMUNITY}/profiles/{steam_id}/"))
        .form(&[("sessionid", web.session_id.clone()), ("steamid", steam_id.to_string()), ("block", if block { "1" } else { "0" }.to_string())])
        .send()
        .await
        .map_err(|e| e.to_string())?;
    if resp.status().is_success() { Ok(block) } else { Err(format!("Steam said {}", resp.status())) }
}

/// Recent co-play players from Steam Community (`/profiles/{id}/friends/coplay`).
pub async fn recent_players(web: Option<&Web>, steam_id: SteamId) -> Result<Vec<model::RecentPlayer>, String> {
    let resp = http(web)
        .get(format!("{COMMUNITY}/profiles/{steam_id}/friends/coplay"))
        .send()
        .await
        .map_err(|e| e.to_string())?;
    if !resp.status().is_success() {
        return Err(format!("Steam said {}", resp.status()));
    }
    let html = resp.text().await.map_err(|e| e.to_string())?;
    Ok(parse_recent_players(&html))
}

fn strip_tags(s: &str) -> String {
    let mut out = String::with_capacity(s.len());
    let mut in_tag = false;
    for c in s.chars() {
        match c {
            '<' => in_tag = true,
            '>' => in_tag = false,
            _ if !in_tag => out.push(c),
            _ => {}
        }
    }
    unescape(out.trim())
}

pub fn parse_recent_players(html: &str) -> Vec<model::RecentPlayer> {
    let mut out = Vec::new();
    let mut seen = std::collections::HashSet::new();
    for block in html.split("data-steamid=\"").skip(1) {
        let Some(id_str) = block.split('"').next() else { continue };
        let Ok(steam_id) = id_str.trim().parse::<u64>() else { continue };
        if !seen.insert(steam_id) {
            continue;
        }
        let end = block.find("class=\"friend_block_v2").unwrap_or(block.len().min(2048));
        let chunk = &block[..end];
        let avatar = between(chunk, "class=\"playerAvatar", ">")
            .and_then(|_| between(chunk, "<img", ">"))
            .and_then(|img| between(img, "src=\"", "\""))
            .filter(|u| u.starts_with("http"))
            .unwrap_or_default()
            .to_string();
        let name = between(chunk, "friend_block_content\">", "<br")
            .or_else(|| between(chunk, "friend_block_content\">", "</div>"))
            .map(strip_tags)
            .filter(|s| !s.is_empty())
            .unwrap_or_else(|| steam_id.to_string());
        let played_at = between(chunk, "friend_small_text\">", "</span>")
            .or_else(|| between(chunk, "friend_small_text\">", "</div>"))
            .map(strip_tags)
            .unwrap_or_default();
        let app_id = between(chunk, "/app/", "/")
            .or_else(|| between(chunk, "app/", "\""))
            .and_then(|d| d.trim().parse::<u32>().ok());
        out.push(model::RecentPlayer {
            steam_id,
            name,
            avatar,
            app_id,
            game_name: String::new(),
            played_at,
        });
    }
    out
}

/// `Community.DeleteCommentFromThread#1`, the comment service Steam's own
/// web pages use to delete comments now (the old form endpoint below fails
/// with a generic error). Defined here instead of vendoring
/// steammessages_community.proto with all its imports.
#[derive(Clone, PartialEq, prost::Message)]
struct DeleteCommentFromThreadRequest {
    #[prost(fixed64, optional, tag = "1")]
    steamid: Option<u64>,
    /// ECommentThreadType.
    #[prost(int32, optional, tag = "2")]
    comment_thread_type: Option<i32>,
    #[prost(fixed64, optional, tag = "3")]
    gidfeature: Option<u64>,
    #[prost(fixed64, optional, tag = "4")]
    gidfeature2: Option<u64>,
    #[prost(fixed64, optional, tag = "5")]
    gidcomment: Option<u64>,
    #[prost(bool, optional, tag = "6")]
    undelete: Option<bool>,
}

#[derive(Clone, PartialEq, prost::Message)]
struct DeleteCommentFromThreadResponse {}

/// k_ECommentThreadTypeProfile (enums.proto).
const THREAD_PROFILE: i32 = 10;
/// A profile thread has no feature ids; the web pages send "-1", which the
/// service takes as the largest u64.
const NO_FEATURE: u64 = u64::MAX;

/// Delete a profile comment through Steam's comment service.
pub async fn delete_comment_service(conn: &net::Connection, steam_id: SteamId, comment_id: u64) -> Result<(), String> {
    let req = DeleteCommentFromThreadRequest {
        steamid: Some(steam_id),
        comment_thread_type: Some(THREAD_PROFILE),
        gidfeature: Some(NO_FEATURE),
        gidfeature2: Some(NO_FEATURE),
        gidcomment: Some(comment_id),
        undelete: Some(false),
    };
    let _: DeleteCommentFromThreadResponse = conn.call("Community.DeleteCommentFromThread#1", &req).await.map_err(|e| format!("{e} ({e:?})"))?;
    Ok(())
}

/// Delete a comment. Steam allows it on your own profile and for your own comments.
pub async fn delete_comment(web: &Web, steam_id: SteamId, comment_id: u64) -> Result<model::Comments, String> {
    let v: Value = web
        .http
        .post(format!("{COMMUNITY}/comment/Profile/delete/{steam_id}/-1/"))
        .header("Referer", format!("{COMMUNITY}/profiles/{steam_id}/"))
        .form(&[("gidcomment", comment_id.to_string()), ("start", "0".into()), ("count", "20".into()), ("sessionid", web.session_id.clone()), ("feature2", "-1".into())])
        .send()
        .await
        .map_err(|e| e.to_string())?
        .json()
        .await
        .map_err(|e| e.to_string())?;
    page(&v)
}

/// Level, XP and badges from the profile's badges page.
pub async fn badges(web: Option<&Web>, steam_id: SteamId) -> Result<model::Badges, String> {
    let body = http(web).get(format!("{COMMUNITY}/profiles/{steam_id}/badges/")).send().await.map_err(|e| e.to_string())?.text().await.map_err(|e| e.to_string())?;
    if !body.contains("badge_row") && body.contains("profile_private_info") {
        return Err("private".into());
    }
    Ok(parse_badges(&body))
}

fn clean(s: &str) -> String {
    let mut out = String::new();
    let mut tag = false;
    for c in s.chars() {
        match c {
            '<' => tag = true,
            '>' => tag = false,
            _ if !tag => out.push(c),
            _ => {}
        }
    }
    unescape(&out).replace("&nbsp;", " ").split_whitespace().collect::<Vec<_>>().join(" ")
}

pub fn parse_badges(body: &str) -> model::Badges {
    let level = between(body, "friendPlayerLevelNum\">", "<").and_then(|l| l.trim().parse().ok()).unwrap_or(0);
    let xp = between(body, "profile_xp_block_xp\">", "<").map(clean).unwrap_or_default();
    let next = between(body, "profile_xp_block_remaining\">", "<").map(clean).unwrap_or_default();
    let progress = between(body, "profile_xp_block_remaining_bar_progress\" style=\"width: ", "%").and_then(|p| p.trim().parse::<f32>().ok()).unwrap_or(0.0) / 100.0;
    let items = body
        .split("class=\"badge_row ")
        .skip(1)
        .filter_map(|row| {
            let url = between(row, "badge_row_overlay\" href=\"", "\"").unwrap_or_default().to_string();
            let icon = between(row, "data-delayed-image=\"", "\"").or_else(|| between(row, "class=\"badge_info_image\">", "</div>").and_then(|b| between(b, "src=\"", "\""))).map(String::from).filter(|u| !u.contains("trans.gif"));
            let info = between(row, "class=\"badge_info_description\">", "<div style=\"clear")?;
            let title = between(info, "class=\"badge_info_title\">", "</div>").map(clean).unwrap_or_default();
            let unlocked = between(info, "class=\"badge_info_unlocked\">", "</div>").map(clean).unwrap_or_default();
            // The line between title and unlock date: level / XP.
            let after_title = info.split("badge_info_title").nth(1).unwrap_or_default();
            let detail = between(after_title, "<div>", "</div>").map(clean).unwrap_or_default();
            (!title.is_empty()).then_some(model::Badge { title, icon, detail, unlocked, url })
        })
        .collect();
    model::Badges { level, xp, next, progress, items }
}

/// The game's card collection behind a badge (`foil`: the foil set).
pub async fn card_set(web: Option<&Web>, steam_id: SteamId, app: model::AppId, foil: bool) -> Result<model::CardSet, String> {
    let mut req = http(web).get(format!("{COMMUNITY}/profiles/{steam_id}/gamecards/{app}/"));
    if foil {
        req = req.query(&[("border", "1")]);
    }
    let body = req.send().await.map_err(|e| e.to_string())?.text().await.map_err(|e| e.to_string())?;
    if !body.contains("badge_card_set_card") && body.contains("profile_private_info") {
        return Err("private".into());
    }
    let set = parse_card_set(&body);
    if set.cards.is_empty() {
        return Err("This game has no trading cards".into());
    }
    Ok(set)
}

pub fn parse_card_set(body: &str) -> model::CardSet {
    let title = between(body, "class=\"badge_title\">", "</div>").map(clean).unwrap_or_default();
    let playtime = between(body, "class=\"badge_title_stats_playtime\">", "</div>").map(clean).unwrap_or_default();
    let drops = between(body, "class=\"badge_title_stats_drops\">", "</div>").map(clean).unwrap_or_default();
    let badge = between(body, "class=\"badge_info_description\">", "<div style=\"clear").and_then(|info| {
        let name = between(info, "class=\"badge_info_title\">", "</div>").map(clean).filter(|n| !n.is_empty())?;
        let after = info.split("badge_info_title").nth(1).unwrap_or_default();
        let level = between(after, "<div>", "</div>").map(clean).unwrap_or_default();
        let unlocked = between(info, "class=\"badge_info_unlocked\">", "</div>").map(clean).unwrap_or_default();
        let icon = between(body, "class=\"badge_info_image\">", "</div>").and_then(|b| between(b, "src=\"", "\"")).map(String::from);
        Some(model::CardBadge { name, icon, level, unlocked })
    });
    let cards = body
        .split("class=\"badge_card_set_card ")
        .skip(1)
        .filter_map(|c| {
            let owned = c.starts_with("owned");
            let image = between(c, "class=\"gamecard\" src=\"", "\"")?.to_string();
            let title = between(c, "badge_card_set_title ellipsis\">", "<div style")?;
            let count = match between(title, "class=\"badge_card_set_text_qty\">(", ")") {
                Some(n) => n.trim().parse().unwrap_or(1),
                None => u32::from(owned),
            };
            let name = clean(title.rsplit("</div>").next().unwrap_or(title));
            let series = c.split("badge_card_set_title").nth(1).and_then(|r| between(r, "class=\"badge_card_set_text ellipsis\">", "</div>")).map(clean).unwrap_or_default();
            Some(model::Card { name, image, count, series })
        })
        .collect();
    model::CardSet { title, badge, playtime, drops, cards }
}

/// One page (1-based, 10 a page) of a profile's reviews.
pub async fn reviews(web: Option<&Web>, steam_id: SteamId, page: u32) -> Result<model::UserReviews, String> {
    let body = http(web)
        .get(format!("{COMMUNITY}/profiles/{steam_id}/recommended/"))
        .query(&[("p", page.max(1).to_string())])
        .send()
        .await
        .map_err(|e| e.to_string())?
        .text()
        .await
        .map_err(|e| e.to_string())?;
    if !body.contains("review_box") && body.contains("profile_private_info") {
        return Err("private".into());
    }
    Ok(parse_reviews(&body))
}

pub fn parse_reviews(body: &str) -> model::UserReviews {
    let items = body
        .split("class=\"review_box\"")
        .skip(1)
        .filter_map(|b| {
            let content = b.split("class=\"review_box_content\"").nth(1)?;
            let app_id = between(content, "steamcommunity.com/app/", "\"").and_then(|a| a.trim_end_matches('/').parse().ok())?;
            let capsule = between(content, "class=\"game_capsule\"src=\"", "\"").or_else(|| between(content, "class=\"game_capsule\" src=\"", "\"")).map(String::from);
            let vote = between(content, "class=\"vote_header\"", "class=\"content")?;
            let recommended = vote.contains("icon_thumbsUp");
            let url = between(vote, "class=\"title\"><a href=\"", "\"").unwrap_or_default().to_string();
            let hours = between(vote, "class=\"hours\">", "</div>").map(clean).unwrap_or_default();
            let text = between(content, "class=\"content", "</div>").and_then(|c| c.split_once('>')).map(|(_, t)| clean(&t.replace("<br>", "\n"))).unwrap_or_default();
            let posted = between(content, "class=\"posted\">", "</div>").map(clean).unwrap_or_default();
            let helpful = between(b, "class=\"header\">", "<div").map(|h| clean(&h.replace("<br>", " · "))).unwrap_or_default();
            Some(model::UserReview { app_id, capsule, recommended, hours, posted, text, helpful, url })
        })
        .collect();
    // "Showing 1-10 of 23 entries"
    let total = between(body, "class=\"workshopBrowsePagingInfo\">", "</div>")
        .and_then(|s| clean(s).split_whitespace().rev().nth(1).map(|n| n.replace([',', '.'], "")))
        .and_then(|n| n.parse().ok())
        .unwrap_or(0);
    model::UserReviews { items, total }
}

/// The groups a profile belongs to.
pub async fn groups(web: Option<&Web>, steam_id: SteamId) -> Result<Vec<model::UserGroup>, String> {
    let body = http(web).get(format!("{COMMUNITY}/profiles/{steam_id}/groups/")).send().await.map_err(|e| e.to_string())?.text().await.map_err(|e| e.to_string())?;
    if !body.contains("group_block") && body.contains("profile_private_info") {
        return Err("private".into());
    }
    Ok(parse_groups(&body))
}

pub fn parse_groups(body: &str) -> Vec<model::UserGroup> {
    body.split("class=\"group_block invite_row\"")
        .skip(1)
        .filter_map(|b| {
            let title = between(b, "class=\"linkTitle\" href=\"", "</a>")?;
            let (url, name) = title.split_once("\">")?;
            let avatar = between(b, "class=\"avatarMedium\">", "</div>").and_then(|a| between(a, "src=\"", "\"")).map(|u| u.replace("_medium.", "_full."));
            let members = between(b, "groupMemberStat linkStandard\" href=\"", "</a>").and_then(|m| m.split_once("\">")).map(|(_, m)| clean(m)).unwrap_or_default();
            let online = between(b, "groupMemberStat membersOnline\">", "</span>").map(clean).unwrap_or_default();
            Some(model::UserGroup { name: clean(name), url: url.to_string(), avatar, members, online })
        })
        .collect()
}

/// A game's Workshop tag groups, as its Workshop page declares them.
pub async fn workshop_tags(app: model::AppId, language: &str) -> Result<Vec<model::WorkshopTagCategory>, String> {
    let body = crate::http()
        .get(format!("{COMMUNITY}/workshop/browse/"))
        .query(&[("appid", app.to_string()), ("l", language.to_string())])
        .send()
        .await
        .map_err(|e| e.to_string())?
        .text()
        .await
        .map_err(|e| e.to_string())?;
    parse_workshop_tags(&body)
}

pub fn parse_workshop_tags(body: &str) -> Result<Vec<model::WorkshopTagCategory>, String> {
    // The page embeds its data as an escaped JSON string.
    let marker = r#"\"declaredTags\":"#;
    let at = body.find(marker).ok_or("This game's Workshop has no tags")? + marker.len();
    let mut text = String::new();
    let mut chars = body[at..].chars();
    while let Some(c) = chars.next() {
        match c {
            '\\' => match chars.next() {
                Some('u') => {
                    let hex: String = chars.by_ref().take(4).collect();
                    text.push(u32::from_str_radix(&hex, 16).ok().and_then(char::from_u32).unwrap_or('?'));
                }
                Some(other) => text.push(other),
                None => break,
            },
            '"' => break,
            _ => text.push(c),
        }
    }
    let v: Value = serde_json::Deserializer::from_str(&text).into_iter::<Value>().next().ok_or("bad tag data")?.map_err(|e| e.to_string())?;
    // Regular items first; games that only sell items use the store's groups.
    let groups = ["readytouse_tags", "mtx_tags", "collection_tags"]
        .iter()
        .filter_map(|k| v.get(*k)?.as_array())
        .find(|a| !a.is_empty())
        .cloned()
        .unwrap_or_default();
    Ok(groups
        .iter()
        .filter_map(|g| {
            let tags: Vec<(String, String)> = g
                .get("tags")?
                .as_array()?
                .iter()
                .filter(|t| !t.get("admin_only").and_then(|a| a.as_bool()).unwrap_or(false))
                .filter_map(|t| {
                    let name = t.get("name")?.as_str()?.to_string();
                    let shown = t.get("display_name").and_then(|d| d.as_str()).filter(|d| !d.is_empty()).unwrap_or(&name).to_string();
                    Some((name, shown))
                })
                .collect();
            let name = g.get("name").and_then(|n| n.as_str()).unwrap_or_default().trim_start_matches('#').to_string();
            (!tags.is_empty()).then_some(model::WorkshopTagCategory { name, single: g.get("htmlelement").and_then(|h| h.as_str()) == Some("select"), tags })
        })
        .collect())
}

/// Library collections from Steam's synced settings ("user-collections.*").
pub fn collections<'a>(entries: impl Iterator<Item = (&'a str, bool, &'a str)>) -> Vec<model::SteamCollection> {
    let mut out: Vec<model::SteamCollection> = entries
        .filter(|(key, deleted, _)| key.starts_with("user-collections.") && !deleted)
        .filter_map(|(_, _, value)| {
            let v: Value = serde_json::from_str(value).ok()?;
            let id = v.get("id")?.as_str()?.to_string();
            let added: Vec<u32> = v.get("added").and_then(|a| a.as_array()).map(|a| a.iter().filter_map(|x| x.as_u64().map(|n| n as u32)).collect()).unwrap_or_default();
            let removed: Vec<u32> = v.get("removed").and_then(|a| a.as_array()).map(|a| a.iter().filter_map(|x| x.as_u64().map(|n| n as u32)).collect()).unwrap_or_default();
            Some(model::SteamCollection {
                name: v.get("name").and_then(|n| n.as_str()).unwrap_or_default().to_string(),
                games: added.into_iter().filter(|a| !removed.contains(a)).collect(),
                dynamic: v.get("filterSpec").is_some(),
                id,
            })
        })
        .collect();
    out.sort_by(|a, b| a.name.to_lowercase().cmp(&b.name.to_lowercase()));
    out
}

#[cfg(test)]
mod tests {
    #[test]
    fn reads_steam_collections() {
        let entries = [
            ("user-collections.favorite", false, r#"{"id":"favorite","name":"Favorites","added":[570,620,730],"removed":[730]}"#),
            ("user-collections.uc-1", true, r#"{"id":"uc-1","name":"Gone","added":[1]}"#),
            ("showcases.1", false, "{}"),
        ];
        let c = super::collections(entries.into_iter());
        assert_eq!(c.len(), 1);
        assert_eq!(c[0].id, "favorite");
        assert_eq!(c[0].games, vec![570, 620]);
    }

    #[test]
    fn reads_workshop_tag_groups() {
        let page = r#"x = "{\"declaredTags\":{\"guide_tags\":[],\"readytouse_tags\":[{\"name\":\"Content Type\",\"htmlelement\":\"select\",\"tags\":[{\"id\":\"1\",\"name\":\"Addon\",\"display_name\":\"Addon\",\"admin_only\":false}]},{\"name\":\"Addon Type\",\"htmlelement\":\"checkbox\",\"tags\":[{\"id\":\"2\",\"name\":\"Weapon\",\"display_name\":\"Weapon\",\"admin_only\":false},{\"id\":\"3\",\"name\":\"Secret\",\"display_name\":\"Secret\",\"admin_only\":true}]}]}}";"#;
        let cats = super::parse_workshop_tags(page).unwrap();
        assert_eq!(cats.len(), 2);
        assert!(cats[0].single);
        assert_eq!(cats[1].tags, vec![("Weapon".to_string(), "Weapon".to_string())]);
    }

    #[test]
    fn reads_card_sets() {
        let page = r#"<div class="badge_title_stats_playtime">
        &nbsp;29 hrs on record </div><div class="badge_title_stats_drops"><span class="progress_info_bold">No card drops remaining</span></div></div></div>
        <div class="badge_title">
        The Witcher 2 Badge	</div></div><div class="badge_info"><div class="badge_info_image"><img src="https://x/badge.png" class="badge_icon"></div><div class="badge_info_description"><div class="badge_info_title">Sellsword</div><div>
        Level 1, 100 XP	</div><div class="badge_info_unlocked">Unlocked 21 Jun, 2014 @ 5:20am</div></div><div style="clear: inline-start;"></div>
        <div class="badge_card_set_cards"><div class="badge_card_set_card owned"><div class="game_card_ctn "><img class="gamecard" src="https://x/a.png"></div><div class="badge_card_set_text badge_card_set_title ellipsis"><div class="badge_card_set_text_qty">(5)</div>
        The Ancient Dead	<div style="clear: inline-end"></div></div><div class="badge_card_set_text ellipsis">
        1 of 2, Series 1	</div></div>
        <div class="badge_card_set_card owned"><div class="game_card_ctn "><img class="gamecard" src="https://x/b.png"></div><div class="badge_card_set_text badge_card_set_title ellipsis">
        Solo	<div style="clear: inline-end"></div></div><div class="badge_card_set_text ellipsis">2 of 3, Series 1</div></div>
        <div class="badge_card_set_card unowned"><div class="game_card_ctn "><div class="game_card_unowned_border"></div><img class="gamecard" src="https://x/c.png"></div><div class="badge_card_set_text badge_card_set_title ellipsis">
        Iorweth	<div style="clear: inline-end"></div></div><div class="badge_card_set_text ellipsis">3 of 3, Series 1</div></div></div>"#;
        let s = super::parse_card_set(page);
        assert_eq!(s.title, "The Witcher 2 Badge");
        assert_eq!((s.playtime.as_str(), s.drops.as_str()), ("29 hrs on record", "No card drops remaining"));
        let b = s.badge.unwrap();
        assert_eq!((b.name.as_str(), b.level.as_str(), b.icon.as_deref()), ("Sellsword", "Level 1, 100 XP", Some("https://x/badge.png")));
        let names: Vec<(&str, u32, &str)> = s.cards.iter().map(|c| (c.name.as_str(), c.count, c.series.as_str())).collect();
        assert_eq!(names, vec![("The Ancient Dead", 5, "1 of 2, Series 1"), ("Solo", 1, "2 of 3, Series 1"), ("Iorweth", 0, "3 of 3, Series 1")]);
        assert_eq!(s.cards[2].image, "https://x/c.png");
    }

    #[test]
    fn reads_reviews_and_groups() {
        let r = r#"<div class="review_box"><div class="header">63 people found this review helpful<br>14 people found this review funny<div class="review_award_ctn"></div></div><div class="review_box_content"><div class="leftcol"><a href="https://steamcommunity.com/app/365360" class="game_capsule_ctn "><img class="game_capsule"src="https://cdn.example/capsule_184x69.jpg"></a></div><div class="rightcol"><div class="vote_header"><div class="thumb"><a href="u"><img src="https://x/icon_thumbsUp.png"></a></div><div class="title"><a href="https://steamcommunity.com/id/x/recommended/365360/">Recommended</a></div><div class="hours">
        1,083.3 hrs on record	(223.8 hrs at review time)	</div></div><div class="content ">The best &amp; all.<br>Really.	</div><div class="posted">
        Posted 24 March, 2017.	</div></div></div><div class="workshopBrowsePagingInfo">
        Showing 1-1 of 1,203 entries </div>"#;
        let v = super::parse_reviews(r);
        assert_eq!(v.total, 1203);
        assert_eq!(v.items.len(), 1);
        let i = &v.items[0];
        assert_eq!((i.app_id, i.recommended), (365360, true));
        assert_eq!(i.capsule.as_deref(), Some("https://cdn.example/capsule_184x69.jpg"));
        assert_eq!(i.hours, "1,083.3 hrs on record (223.8 hrs at review time)");
        assert_eq!(i.text, "The best & all. Really.");
        assert_eq!(i.posted, "Posted 24 March, 2017.");
        assert_eq!(i.helpful, "63 people found this review helpful · 14 people found this review funny");
        assert_eq!(i.url, "https://steamcommunity.com/id/x/recommended/365360/");
        let g = r#"<div class="group_block invite_row" data-search="dota"><div class="avatarMedium"><a href="https://steamcommunity.com/groups/DotA"><img src="https://avatars.example/ad58_medium.jpg" /></a></div><div class="ellipsis groupTitle"><a class="linkTitle" href="https://steamcommunity.com/groups/DotA">Dota 2 Internal</a></div><div class="memberRow"><a class="groupMemberStat linkStandard" href="https://steamcommunity.com/groups/DotA/members">14 Members</a><span class="groupMemberStat membersOnline">4 Online</span></div></div>"#;
        let groups = super::parse_groups(g);
        assert_eq!(groups.len(), 1);
        assert_eq!(groups[0].name, "Dota 2 Internal");
        assert_eq!(groups[0].url, "https://steamcommunity.com/groups/DotA");
        assert_eq!(groups[0].avatar.as_deref(), Some("https://avatars.example/ad58_full.jpg"));
        assert_eq!((groups[0].members.as_str(), groups[0].online.as_str()), ("14 Members", "4 Online"));
    }

    #[test]
    fn reads_comments_and_badges() {
        let c = r#"<div class="commentthread_comment responsive_body_text   " id="comment_123"><div class="commentthread_comment_content"><div class="commentthread_comment_author"><div class="commentthread_comment_avatar playerAvatar"><a href="https://steamcommunity.com/profiles/76561197960265730" data-miniprofile="2"><img src="https://avatars.example/a.jpg"></a></div><div class="author_name_group"><a class="hoverunderline commentthread_author_link" href="x" data-miniprofile="2"><bdi>Tester</bdi></a><div class="commentthread_comment_timestamp" data-timestamp="1788630100">Sep 5</div></div></div><div class="commentthread_comment_text" id="comment_content_123"> hello &amp; welcome </div></div></div>"#;
        let list = super::parse_comments(c);
        assert_eq!(list.len(), 1);
        assert_eq!(list[0].author, 76561197960265730);
        assert_eq!(list[0].name, "Tester");
        assert_eq!(list[0].time, 1788630100);
        assert_eq!(list[0].avatar.as_deref(), Some("https://avatars.example/a.jpg"));
        assert_eq!(list[0].html, "hello &amp; welcome");

        let b = r#"<span class="friendPlayerLevelNum">18</span><span class="profile_xp_block_xp">XP 2,715</span><div class="profile_xp_block_remaining">85 XP to reach Level 19</div><div class="profile_xp_block_remaining_bar_progress" style="width: 58%"></div>
        <div class="badge_row is_link"><a class="badge_row_overlay" href="https://x/badges/13"></a><div class="badge_info_image"><img src="trans.gif" data-delayed-image="https://x/13.png" class="badge_icon"></div><div class="badge_info_description"><div class="badge_info_title">Power Player</div><div> 340 XP </div><div class="badge_info_unlocked"> Unlocked Oct 14 @ 10:06am </div></div><div style="clear: both;"></div></div>"#;
        let badges = super::parse_badges(b);
        assert_eq!(badges.level, 18);
        assert_eq!(badges.next, "85 XP to reach Level 19");
        assert!((badges.progress - 0.58).abs() < 0.01);
        assert_eq!(badges.items.len(), 1);
        assert_eq!(badges.items[0].title, "Power Player");
        assert_eq!(badges.items[0].detail, "340 XP");
        assert_eq!(badges.items[0].icon.as_deref(), Some("https://x/13.png"));
    }

    #[test]
    fn reads_recent_players() {
        let html = r#"<div class="friend_block_v2" data-steamid="76561197960287930"><div class="playerAvatar offline"><img src="https://avatars.example/p1.jpg"></div><div class="friend_block_content">SquadMate<br><span class="friend_small_text">Team Fortress 2 - Yesterday</span></div></div>"#;
        let list = super::parse_recent_players(html);
        assert_eq!(list.len(), 1);
        assert_eq!(list[0].steam_id, 76561197960287930);
        assert_eq!(list[0].name, "SquadMate");
        assert_eq!(list[0].avatar, "https://avatars.example/p1.jpg");
        assert_eq!(list[0].played_at, "Team Fortress 2 - Yesterday");
    }
}
