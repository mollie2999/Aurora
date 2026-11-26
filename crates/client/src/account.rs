//! Your account: security details and Steam Family through Steam's
//! services (what Steam's own client calls), and purchase history and
//! licenses from the store pages Steam's client shows for them (they have
//! no service). Sensitive account changes stay on Steam's site; Steam
//! Family management actions run through `FamilyGroups`.

use crate::market::Web;
use model::{AccountSecurity, Family, FamilyInvite, FamilyMember, FamilyRole, FamilyStatus, GuardDevice, License, Purchase, PurchasePage, SharedApp, SharedDlc, SteamId};
use net::Connection;
use proto::webui::*;
use proto::*;

const STORE: &str = "https://store.steampowered.com";

/// Steam Guard, the mobile authenticator, phone and credential change dates.
pub async fn security(conn: &Connection) -> Result<AccountSecurity, String> {
    let guard: CCredentialsGetSteamGuardDetailsResponse =
        conn.call("Credentials.GetSteamGuardDetails#1", &CCredentialsGetSteamGuardDetailsRequest::default()).await.map_err(|e| e.to_string())?;
    let changes: Option<CCredentialsLastCredentialChangeTimeResponse> = conn
        .call("Credentials.GetCredentialChangeTimeDetails#1", &CCredentialsLastCredentialChangeTimeRequest { user_changes_only: Some(false) })
        .await
        .map_err(|e| tracing::debug!("credential changes: {e}"))
        .ok();
    let time = |t: Option<u32>| t.filter(|t| *t > 0).map(u64::from);
    Ok(AccountSecurity {
        steam_guard: guard.is_steamguard_enabled(),
        steam_guard_since: time(guard.timestamp_steamguard_enabled),
        authenticator: guard.is_twofactor_enabled(),
        authenticator_since: time(guard.timestamp_twofactor_enabled),
        phone_verified: guard.is_phone_verified(),
        password_changed: changes.as_ref().and_then(|c| time(c.timestamp_last_password_change)),
        email_changed: changes.as_ref().and_then(|c| time(c.timestamp_last_email_change)),
        password_reset: changes.as_ref().and_then(|c| time(c.timestamp_last_password_reset)),
        devices: guard
            .session_data
            .iter()
            .map(|s| GuardDevice {
                name: s.machine_name_userchosen().to_string(),
                since: time(s.timestamp_machine_steamguard_enabled),
                ip: s.public_ip_address().to_string(),
            })
            .collect(),
    })
}

/// Your Steam Family, its members and the games shared in it.
pub async fn family(conn: &Connection, me: SteamId, language: &str) -> Result<FamilyStatus, String> {
    let r: CFamilyGroupsGetFamilyGroupForUserResponse = conn
        .call("FamilyGroups.GetFamilyGroupForUser#1", &CFamilyGroupsGetFamilyGroupForUserRequest { steamid: Some(me), include_family_group_response: Some(true) })
        .await
        .map_err(|e| e.to_string())?;
    let mut status = FamilyStatus {
        family: None,
        role: FamilyRole::from_steam(r.role() as i32),
        cooldown: r.cooldown_seconds_remaining(),
        invites: r
            .pending_group_invites
            .iter()
            .map(|i| FamilyInvite {
                family_id: i.family_groupid(),
                inviter: i.inviter_steamid(),
                role: FamilyRole::from_steam(i.role()),
                awaiting_2fa: i.awaiting_2fa(),
            })
            .collect(),
    };
    if r.is_not_member_of_any_group() || r.family_groupid() == 0 {
        return Ok(status);
    }
    let id = r.family_groupid();
    let g = match r.family_group {
        Some(g) => g,
        None => conn
            .call::<_, CFamilyGroupsGetFamilyGroupResponse>("FamilyGroups.GetFamilyGroup#1", &CFamilyGroupsGetFamilyGroupRequest { family_groupid: Some(id), send_running_apps: Some(false) })
            .await
            .map_err(|e| e.to_string())?,
    };
    let shared = conn
        .call::<_, CFamilyGroupsGetSharedLibraryAppsResponse>(
            "FamilyGroups.GetSharedLibraryApps#1",
            &CFamilyGroupsGetSharedLibraryAppsRequest {
                family_groupid: Some(id),
                include_own: Some(true),
                include_excluded: Some(true),
                language: Some(language.to_string()),
                max_apps: None,
                include_non_games: Some(true),
                steamid: Some(me),
            },
        )
        .await;
    let (shared, shared_available) = match shared {
        Ok(response) => (response.apps, true),
        Err(e) => {
            tracing::warn!("family library: {e}");
            (Vec::new(), false)
        }
    };
    let (dlc, games): (Vec<_>, Vec<_>) = shared.into_iter().partition(|a| a.app_type() == APP_TYPE_DLC);
    let mut shared: Vec<SharedApp> = games
        .into_iter()
        .map(|a| SharedApp { app_id: a.appid(), name: a.name().to_string(), owners: a.owner_steamids.clone(), excluded: a.exclude_reason() != 0 })
        .collect();
    shared.sort_by_key(|a| a.name.to_lowercase());
    let (shared_dlc, shared_dlc_complete) = if shared_available {
        shared_dlc(conn, dlc, me).await
    } else {
        (Vec::new(), false)
    };
    let preferred = preferred_lenders(conn, id).await;
    status.family = Some(Family {
        id,
        name: g.name().to_string(),
        members: g
            .members
            .iter()
            .map(|m| FamilyMember { steam_id: m.steamid(), role: FamilyRole::from_steam(m.role()), joined: m.time_joined().into(), cooldown: m.cooldown_seconds_remaining() })
            .collect(),
        invites: g.pending_invites.iter().map(|i| (i.steamid(), FamilyRole::from_steam(i.role()))).collect(),
        free_spots: g.free_spots(),
        country: g.country().to_string(),
        slot_cooldown: g.slot_cooldown_remaining_seconds(),
        shared,
        shared_available,
        shared_dlc,
        shared_dlc_complete,
        preferred,
    });
    Ok(status)
}

/// Steam's `EProtoAppType` value for DLC.
const APP_TYPE_DLC: i32 = 32;

/// Attach each family DLC to its game through PICS public info.
async fn shared_dlc(
    conn: &Connection,
    mut dlc: Vec<CFamilyGroupsGetSharedLibraryAppsResponseSharedApp>,
    me: SteamId,
) -> (Vec<SharedDlc>, bool) {
    // An excluded DLC can be used with your own copy, but cannot be borrowed.
    for entry in &mut dlc {
        if entry.exclude_reason() != 0 {
            entry.owner_steamids.retain(|owner| *owner == me);
        }
    }
    dlc.retain(|entry| !entry.owner_steamids.is_empty());
    if dlc.is_empty() {
        return (Vec::new(), true);
    }
    let ids: Vec<u32> = dlc.iter().map(|d| d.appid()).collect();
    let parents = match crate::pics::dlc_parents(conn, &ids).await {
        Ok(parents) => parents,
        Err(e) => {
            tracing::warn!("family dlc parents: {e}");
            return (Vec::new(), false);
        }
    };
    let parents: std::collections::HashMap<_, _> =
        parents.into_iter().map(|(id, parent, name)| (id, (parent, name))).collect();
    let expected = dlc.len();
    let entries: Vec<_> = dlc
        .into_iter()
        .filter_map(|d| {
            let (parent, pics_name) = parents.get(&d.appid())?;
            let name = if d.name().is_empty() { pics_name.clone() } else { d.name().to_string() };
            Some(SharedDlc { app_id: d.appid(), parent: *parent, name, owners: d.owner_steamids })
        })
        .collect();
    let complete = entries.len() == expected;
    (entries, complete)
}

/// Your preferred copy per game, as `(app, lender)`.
async fn preferred_lenders(conn: &Connection, family_id: u64) -> Vec<(u32, SteamId)> {
    match conn
        .call::<_, CFamilyGroupsGetPreferredLendersResponse>("FamilyGroups.GetPreferredLenders#1", &CFamilyGroupsGetPreferredLendersRequest { family_groupid: Some(family_id) })
        .await
    {
        Ok(r) => r.members.iter().flat_map(|m| m.preferred_appids.iter().map(move |a| (*a, m.steamid()))).collect(),
        Err(e) => {
            tracing::warn!("preferred lenders: {e}");
            Vec::new()
        }
    }
}

/// Choose whose copy of `app_id` you play (Steam's "Change Preferred Copy").
pub async fn set_preferred_lender(conn: &Connection, family_id: u64, app_id: u32, lender: SteamId) -> Result<(), String> {
    let _: CFamilyGroupsSetPreferredLenderResponse = conn
        .call(
            "FamilyGroups.SetPreferredLender#1",
            &CFamilyGroupsSetPreferredLenderRequest { family_groupid: Some(family_id), appid: Some(app_id), lender_steamid: Some(lender) },
        )
        .await
        .map_err(|e| e.to_string())?;
    Ok(())
}

/// Validate and trim a Steam Family name before sending it to Steam.
pub fn validate_family_name(name: &str) -> Result<String, String> {
    let trimmed = name.trim();
    if trimmed.is_empty() {
        return Err("Family name cannot be empty".into());
    }
    if trimmed.chars().count() > 64 {
        return Err("Family name is too long (64 characters max)".into());
    }
    Ok(trimmed.to_string())
}

/// Create a new Steam Family with this account as its first adult member.
pub async fn create_family(conn: &Connection, me: SteamId, name: &str) -> Result<u64, String> {
    let name = validate_family_name(name)?;
    let r: CFamilyGroupsCreateFamilyGroupResponse = conn
        .call(
            "FamilyGroups.CreateFamilyGroup#1",
            &CFamilyGroupsCreateFamilyGroupRequest { name: Some(name), steamid: Some(me) },
        )
        .await
        .map_err(|e| e.to_string())?;
    Ok(r.family_groupid())
}

/// Rename an existing Steam Family (adult role required).
pub async fn rename_family(conn: &Connection, family_id: u64, name: &str) -> Result<(), String> {
    let name = validate_family_name(name)?;
    let _: CFamilyGroupsModifyFamilyGroupDetailsResponse = conn
        .call(
            "FamilyGroups.ModifyFamilyGroupDetails#1",
            &CFamilyGroupsModifyFamilyGroupDetailsRequest { family_groupid: Some(family_id), name: Some(name) },
        )
        .await
        .map_err(|e| e.to_string())?;
    Ok(())
}

/// Invite a Steam user to the family with the given role.
/// Returns `Ok(true)` if Steam requires 2FA / mobile confirmation.
pub async fn invite_to_family(conn: &Connection, family_id: u64, receiver: SteamId, role: FamilyRole) -> Result<bool, String> {
    if receiver == 0 {
        return Err("Invalid Steam ID".into());
    }
    let role_id = role.to_steam();
    if role_id == 0 {
        return Err("Invalid family role".into());
    }
    let r: CFamilyGroupsInviteToFamilyGroupResponse = conn
        .call(
            "FamilyGroups.InviteToFamilyGroup#1",
            &CFamilyGroupsInviteToFamilyGroupRequest {
                family_groupid: Some(family_id),
                receiver_steamid: Some(receiver),
                receiver_role: Some(role_id),
            },
        )
        .await
        .map_err(|e| e.to_string())?;
    Ok(r.two_factor_method() != 0)
}

/// Cancel a pending outgoing invite (as an adult) or decline an incoming invite (with `steam_id == me`).
pub async fn cancel_family_invite(conn: &Connection, family_id: u64, steam_id: SteamId) -> Result<(), String> {
    let _: CFamilyGroupsCancelFamilyGroupInviteResponse = conn
        .call(
            "FamilyGroups.CancelFamilyGroupInvite#1",
            &CFamilyGroupsCancelFamilyGroupInviteRequest { family_groupid: Some(family_id), steamid_to_cancel: Some(steam_id) },
        )
        .await
        .map_err(|e| e.to_string())?;
    Ok(())
}

/// Resend an existing pending invitation to a Steam user.
pub async fn resend_family_invite(conn: &Connection, family_id: u64, steam_id: SteamId) -> Result<(), String> {
    let _: CFamilyGroupsResendInvitationToFamilyGroupResponse = conn
        .call(
            "FamilyGroups.ResendInvitationToFamilyGroup#1",
            &CFamilyGroupsResendInvitationToFamilyGroupRequest { family_groupid: Some(family_id), steamid: Some(steam_id) },
        )
        .await
        .map_err(|e| e.to_string())?;
    Ok(())
}

/// Accept a pending invitation and join the Steam Family.
/// Returns `Ok(true)` if Steam requires 2FA / mobile confirmation.
pub async fn join_family(conn: &Connection, family_id: u64) -> Result<bool, String> {
    let r: CFamilyGroupsJoinFamilyGroupResponse = conn
        .call(
            "FamilyGroups.JoinFamilyGroup#1",
            &CFamilyGroupsJoinFamilyGroupRequest { family_groupid: Some(family_id), nonce: None },
        )
        .await
        .map_err(|e| e.to_string())?;
    Ok(r.two_factor_method() != 0)
}

/// Remove a member from the family (or leave the family when `steam_id == me`).
pub async fn remove_from_family(conn: &Connection, family_id: u64, steam_id: SteamId) -> Result<(), String> {
    let _: CFamilyGroupsRemoveFromFamilyGroupResponse = conn
        .call(
            "FamilyGroups.RemoveFromFamilyGroup#1",
            &CFamilyGroupsRemoveFromFamilyGroupRequest { family_groupid: Some(family_id), steamid_to_remove: Some(steam_id) },
        )
        .await
        .map_err(|e| e.to_string())?;
    Ok(())
}

/// Delete a Steam Family (adult role required).
pub async fn delete_family(conn: &Connection, family_id: u64) -> Result<(), String> {
    let _: CFamilyGroupsDeleteFamilyGroupResponse = conn
        .call(
            "FamilyGroups.DeleteFamilyGroup#1",
            &CFamilyGroupsDeleteFamilyGroupRequest { family_groupid: Some(family_id) },
        )
        .await
        .map_err(|e| e.to_string())?;
    Ok(())
}

/// A signed-in store page (`query` already has its `?`).
async fn store_page(web: &Web, path: &str, query: &str) -> Result<String, String> {
    let url = store_page_url(path, query)?;
    let body = crate::market::page(web, &url).await?;
    if body.contains("g_steamID = false;") {
        return Err("Steam didn't accept the web sign-in".into());
    }
    Ok(body)
}

fn store_page_url(path: &str, query: &str) -> Result<String, String> {
    if !path.starts_with('/') || path.contains('?') || path.contains('#') {
        return Err("invalid Steam store path".into());
    }
    if !query.is_empty() && (!query.starts_with('?') || query.contains('#')) {
        return Err("invalid Steam store query".into());
    }
    let parsed = crate::market::validate_steam_url(&format!("{STORE}{path}{query}"), "store.steampowered.com")?;
    Ok(parsed.into())
}

fn history_more_url(language: &str) -> String {
    format!("{STORE}/account/AjaxLoadMoreHistory/?l={}", crate::market::encode(language))
}

/// The inside of the first `<td>` whose class starts with `class`.
fn cell<'a>(row: &'a str, class: &str) -> Option<&'a str> {
    let at = row.find(&format!("class=\"{class}"))?;
    let open = row[at..].find('>')? + at + 1;
    let close = row[open..].find("</td>")? + open;
    Some(&row[open..close])
}

/// Visible text: tags removed, entities decoded (™, ®, numeric ones too).
fn text(html: &str) -> String {
    let s = crate::support::clean(html).replace("&trade;", "™").replace("&reg;", "®").replace("&copy;", "©");
    if !s.contains("&#") {
        return s;
    }
    let mut out = String::new();
    let mut rest = s.as_str();
    while let Some(i) = rest.find("&#") {
        out.push_str(&rest[..i]);
        let after = &rest[i + 2..];
        let end = after.find(';').filter(|e| *e <= 7);
        let code = end.and_then(|e| if let Some(h) = after[..e].strip_prefix(['x', 'X']) { u32::from_str_radix(h, 16).ok() } else { after[..e].parse().ok() });
        match (end, code.and_then(char::from_u32)) {
            (Some(e), Some(c)) => {
                out.push(c);
                rest = &after[e + 1..];
            }
            _ => {
                out.push_str("&#");
                rest = after;
            }
        }
    }
    out.push_str(rest);
    out
}

/// The text of each `<div>` inside `html`, in order.
fn div_texts(html: &str) -> Vec<(bool, String)> {
    html.split("<div")
        .skip(1)
        .filter_map(|d| {
            let payment = d.starts_with(" class=\"wth_payment");
            let open = d.find('>')? + 1;
            let text = text(&d[open..d.find("</div>").unwrap_or(d.len())]);
            (!text.is_empty()).then_some((payment, text))
        })
        .collect()
}

/// Rows of the purchase history table (the page or a "load more" chunk).
pub fn parse_purchases(html: &str) -> Vec<Purchase> {
    html.split("class=\"wallet_table_row")
        .skip(1)
        .map(|row| {
            let row = &row[..row.find("</tr>").unwrap_or(row.len())];
            let field = |class: &str| cell(row, class).map(text).unwrap_or_default();
            let kind = cell(row, "wht_type").map(div_texts).unwrap_or_default();
            // Items are divs, or plain text (Market, conversions, wallet credit).
            let items = match cell(row, "wht_items") {
                Some(c) if c.contains("<div") => div_texts(c).into_iter().map(|(_, t)| t).collect(),
                Some(c) => Some(text(c)).filter(|t| !t.is_empty()).into_iter().collect(),
                None => Vec::new(),
            };
            Purchase {
                date: field("wht_date"),
                items,
                kind: kind.iter().find(|(p, _)| !p).map(|(_, t)| t.clone()).unwrap_or_default(),
                payment: kind.iter().find(|(p, _)| *p).map(|(_, t)| t.clone()).unwrap_or_default(),
                total: field("wht_total"),
                wallet_change: field("wht_wallet_change"),
                wallet_balance: field("wht_wallet_balance"),
                refunded: row.contains("wht_refunded") || row.contains("wht_item_refunded"),
            }
        })
        .collect()
}

/// `var g_historyCursor = {...};` from the history page (None when it ends).
fn history_cursor(body: &str) -> Option<String> {
    let start = body.find("var g_historyCursor = ")? + "var g_historyCursor = ".len();
    let end = body[start..].find(";\n").or_else(|| body[start..].find(';'))? + start;
    let json = body[start..end].trim();
    (json.starts_with('{')).then(|| json.to_string())
}

/// The first page of your purchase history, or the next one after `cursor`
/// (what Steam's "Load more transactions" button asks for).
pub async fn purchases(web: &Web, language: &str, cursor: Option<String>) -> Result<PurchasePage, String> {
    let Some(cursor) = cursor else {
        let query = format!("?l={}", crate::market::encode(language));
        let body = store_page(web, "/account/history/", &query).await?;
        return Ok(PurchasePage { rows: parse_purchases(&body), more: history_cursor(&body) });
    };
    let fields: serde_json::Map<String, serde_json::Value> = serde_json::from_str(&cursor).map_err(|e| e.to_string())?;
    let mut form: Vec<(String, String)> = fields
        .iter()
        .map(|(k, v)| (format!("cursor[{k}]"), v.as_str().map(str::to_string).unwrap_or_else(|| v.to_string())))
        .collect();
    form.push(("sessionid".into(), web.session_id.clone()));
    let r: serde_json::Value = web
        .http
        .post(history_more_url(language))
        .form(&form)
        .send()
        .await
        .map_err(|e| e.to_string())?
        .json()
        .await
        .map_err(|e| e.to_string())?;
    let rows = parse_purchases(r["html"].as_str().unwrap_or_default());
    let more = r.get("cursor").filter(|c| c.is_object()).map(|c| c.to_string());
    Ok(PurchasePage { rows, more })
}

/// Rows of the licenses table, and the link to the next page.
pub fn parse_licenses(body: &str) -> (Vec<License>, Option<String>) {
    let table = &body[body.find("class=\"account_table\"").unwrap_or(0)..];
    let rows = table
        .split("<tr")
        .skip(2)
        .filter_map(|row| {
            let row = &row[..row.find("</tr>").unwrap_or(row.len())];
            let mut cells = row.split("<td").skip(1).map(|c| {
                let open = c.find('>').map(|i| i + 1).unwrap_or(0);
                &c[open..c.find("</td>").unwrap_or(c.len())]
            });
            let (date, item, method) = (cells.next()?, cells.next()?, cells.next()?);
            // A "Remove" link (free licenses) sits before the name.
            let package_id = item.split("RemoveFreeLicense(").nth(1).and_then(|s| s.split(',').next()).and_then(|s| s.trim().parse().ok());
            let name = item.rsplit("</div>").next().unwrap_or(item);
            Some(License { date: text(date), name: text(name), method: text(method), package_id })
        })
        .collect();
    let next = body
        .split("class=\"license_paginator_next\" href=\"")
        .nth(1)
        .and_then(|s| s.split('"').next())
        .filter(|h| h.starts_with('?') && !h.contains('#'))
        .map(|h| h.replace("&amp;", "&"));
    (rows, next)
}

/// Every license on your account (Steam shows them 100 at a time).
pub async fn licenses(web: &Web, language: &str) -> Result<Vec<License>, String> {
    let mut all = Vec::new();
    let lang = crate::market::encode(language);
    let mut query = format!("?l={lang}");
    for _ in 0..50 {
        let body = store_page(web, "/account/licenses/", &query).await?;
        let (rows, next) = parse_licenses(&body);
        all.extend(rows);
        match next {
            Some(next) => query = format!("{next}&l={lang}"),
            None => break,
        }
    }
    Ok(all)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn purchase_rows() {
        let html = r#"<tr class="wallet_table_row wallet_table_row_amt_change" onclick="x">
		<td class="wht_date">14 Oct, 2026</td>
		<td data-tooltip-text="Click" class="wht_items "">
			<div style="clear: both">PlateUp!</div></td>
		<td class="wht_type "><div>Purchase</div><div class="wth_payment">Wallet</div></td>
		<td class="wht_base_price"></td>
		<td class="wht_total ">520₸</td>
		<td class="wht_wallet_change wallet_column" data-tooltip-html="&lt;div&gt;">-520₸</td>
		<td class="wht_wallet_balance wallet_column" data-tooltip-html="&lt;b&gt;"> 522,58₸</td>
		</tr>
		<tr class="wallet_table_row ">
		<td class="wht_date">7 Oct, 2026</td>
		<td class="wht_items wht_item_refunded"><div>Dota 2</div><div class="wth_payment">2 Trove of Terror</div></td>
		<td class="wht_type wht_refunded"><div>Refund</div><div class="wth_payment">Wallet Credit</div></td>
		<td class="wht_total wht_refunded">1 300₸</td></tr>
		<script>var g_historyCursor = {"wallet_txnid":"764","timestamp_newest":1718964004,"balance":"29178","currency":37};
"#;
        let rows = parse_purchases(html);
        assert_eq!(rows.len(), 2);
        assert_eq!(rows[0].date, "14 Oct, 2026");
        assert_eq!(rows[0].items, vec!["PlateUp!"]);
        assert_eq!((rows[0].kind.as_str(), rows[0].payment.as_str()), ("Purchase", "Wallet"));
        assert_eq!((rows[0].total.as_str(), rows[0].wallet_change.as_str(), rows[0].wallet_balance.as_str()), ("520₸", "-520₸", "522,58₸"));
        assert!(!rows[0].refunded && rows[1].refunded);
        assert_eq!(rows[1].items, vec!["Dota 2", "2 Trove of Terror"]);
        let market = parse_purchases(r#"<tr class="wallet_table_row "><td class="wht_items "">
            Steam Community Market  </td><td class="wht_type "><div>Market Transaction</div></td></tr>"#);
        assert_eq!(market[0].items, vec!["Steam Community Market"]);
        assert_eq!(text("NieR&trade; &#8212; &#x41;"), "NieR™ — A");
        assert_eq!(history_cursor(html).as_deref(), Some(r#"{"wallet_txnid":"764","timestamp_newest":1718964004,"balance":"29178","currency":37}"#));
    }

    #[test]
    fn license_rows() {
        let html = r#"<a class="license_paginator_next" href="?continuationToken=17:37&offset=100">Next</a>
		<table class="account_table"><tr><th class="license_date_col">Date</th><th>Item</th><th>Acquisition Method</th></tr>
		<tr role="button"><td  class="license_date_col">14 Oct, 2026</td><td>
			PlateUp!</td><td class="license_acquisition_col">Steam Store</td></tr>
		<tr role="button"><td  class="license_date_col">9 Oct, 2026</td><td>
			<div class="free_license_remove_link"><a href="javascript:RemoveFreeLicense( 1335331, 'TXk' );">Remove</a></div>
			My Auto Clicker</td><td class="license_acquisition_col">Complimentary</td></tr></table>"#;
        let (rows, next) = parse_licenses(html);
        assert_eq!(rows.len(), 2);
        assert_eq!((rows[0].name.as_str(), rows[0].method.as_str(), rows[0].package_id), ("PlateUp!", "Steam Store", None));
        assert_eq!((rows[1].name.as_str(), rows[1].package_id), ("My Auto Clicker", Some(1335331)));
        assert_eq!(next.as_deref(), Some("?continuationToken=17:37&offset=100"));
    }

    #[test]
    fn store_urls_reject_traversal_and_encode_language() {
        assert!(store_page_url("/account/%2e%2e/admin/", "?l=english").is_err());
        assert!(store_page_url(".evil.tld/account/", "?l=english").is_err());
        let hist = history_more_url("english&evil=1");
        assert!(!hist.contains("&evil=1"), "language must be percent-encoded: {hist}");
    }

    #[test]
    fn family_name_validation_and_role_roundtrip() {
        assert_eq!(validate_family_name("  My Family  ").unwrap(), "My Family");
        assert!(validate_family_name("").is_err());
        assert!(validate_family_name("   ").is_err());
        assert!(validate_family_name(&"a".repeat(65)).is_err());
        assert_eq!(validate_family_name(&"a".repeat(64)).unwrap().len(), 64);

        assert_eq!(FamilyRole::from_steam(FamilyRole::Adult.to_steam()), FamilyRole::Adult);
        assert_eq!(FamilyRole::from_steam(FamilyRole::Child.to_steam()), FamilyRole::Child);
        assert_eq!(FamilyRole::Unknown.to_steam(), 0);
    }
}
