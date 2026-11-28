//! Signed-in steamcommunity.com session and the Community Market.
//!
//! The web session is made the same way the official client makes it: a
//! short-lived access token generated from your saved sign-in, sent as the
//! `steamLoginSecure` cookie. It lives in memory only. Listing an item still
//! needs your confirmation in the Steam Mobile app, exactly like on Steam.

use model::{AppId, SteamId};
use net::Connection;
use proto::*;
use serde_json::Value;

use std::sync::Arc;
use std::time::{Duration, Instant};

pub struct Web {
    pub http: reqwest::Client,
    pub me: SteamId,
    pub session_id: String,
}

impl Web {
    /// A signed-out session, for public pages.
    pub fn anonymous() -> Web {
        Web { http: crate::http(), me: 0, session_id: String::new() }
    }
}

pub type WebCache = Arc<tokio::sync::Mutex<Option<(Arc<Web>, Instant)>>>;

const COMMUNITY: &str = "https://steamcommunity.com";
const STEAM_WEB_HOSTS: [&str; 3] = ["steamcommunity.com", "store.steampowered.com", "help.steampowered.com"];

/// The signed-in web session, made on first use and renewed every 20 minutes.
pub async fn web(conn: &Connection, me: SteamId, refresh: &str, cache: &WebCache) -> Result<Arc<Web>, String> {
    let mut slot = cache.lock().await;
    if let Some((w, at)) = slot.as_ref() {
        if at.elapsed() < Duration::from_secs(20 * 60) {
            return Ok(w.clone());
        }
    }
    let req = CAuthenticationAccessTokenGenerateForAppRequest { refresh_token: Some(refresh.to_string()), steamid: Some(me), ..Default::default() };
    let r: CAuthenticationAccessTokenGenerateForAppResponse =
        conn.call("Authentication.GenerateAccessTokenForApp#1", &req).await.map_err(|e| e.to_string())?;
    if r.access_token().is_empty() {
        return Err("Steam didn't give a web sign-in".into());
    }
    let session_id = new_session_id();
    let jar = build_cookie_jar(me, r.access_token(), &session_id);
    let http = build_web_http(jar)?;
    let w = Arc::new(Web { http, me, session_id });
    *slot = Some((w.clone(), Instant::now()));
    Ok(w)
}

fn build_cookie_jar(me: SteamId, access_token: &str, session_id: &str) -> Arc<reqwest::cookie::Jar> {
    let jar = Arc::new(reqwest::cookie::Jar::default());
    for host in STEAM_WEB_HOSTS.into_iter().chain([crate::chat_attachments::HOST]) {
        let url: reqwest::Url = format!("https://{host}").parse().unwrap();
        // Omit Domain= so cookies stay host-only and cannot be sent to subdomains.
        jar.add_cookie_str(&format!("steamLoginSecure={me}%7C%7C{access_token}; Path=/; Secure; HttpOnly"), &url);
        jar.add_cookie_str(&format!("sessionid={session_id}; Path=/; Secure"), &url);
    }
    jar
}

fn build_web_http(jar: Arc<reqwest::cookie::Jar>) -> Result<reqwest::Client, String> {
    let redirect = reqwest::redirect::Policy::custom(|attempt| {
        if attempt.previous().iter().any(|url| url.host_str() == Some(crate::chat_attachments::HOST)) {
            return attempt.error("Steam Chat redirected the request");
        }
        if attempt.previous().len() >= 10 {
            return attempt.error("too many redirects");
        }
        if check_steam_url_parts(attempt.url(), &STEAM_WEB_HOSTS).is_ok() {
            attempt.follow()
        } else {
            attempt.error("redirect leaves allowed Steam origins")
        }
    });
    reqwest::Client::builder()
        .cookie_provider(jar)
        .redirect(redirect)
        .timeout(Duration::from_secs(20))
        .build()
        .map_err(|e| e.to_string())
}

fn decode_percent_segment(seg: &str) -> Result<String, String> {
    let bytes = seg.as_bytes();
    let mut out = Vec::with_capacity(bytes.len());
    let mut i = 0;
    while i < bytes.len() {
        if bytes[i] == b'%' {
            let hex = bytes.get(i + 1..i + 3).and_then(|s| std::str::from_utf8(s).ok()).ok_or_else(|| "invalid percent-encoding in URL path".to_string())?;
            let b = u8::from_str_radix(hex, 16).map_err(|_| "invalid percent-encoding in URL path".to_string())?;
            out.push(b);
            i += 3;
        } else {
            out.push(bytes[i]);
            i += 1;
        }
    }
    if out.iter().any(|&b| b == b'/' || b == b'\\' || b < 0x20 || b == 0x7f) {
        return Err("forbidden character in URL path".into());
    }
    String::from_utf8(out).map_err(|_| "non-UTF-8 URL path".into())
}

fn check_segment(seg: &str) -> Result<(), String> {
    if seg == "." || seg == ".." {
        return Err("path traversal in URL".into());
    }
    let decoded = decode_percent_segment(seg)?;
    if decoded == "." || decoded == ".." {
        return Err("encoded path traversal in URL".into());
    }
    Ok(())
}

fn check_steam_url_parts(url: &reqwest::Url, allowed_hosts: &[&str]) -> Result<(), String> {
    if url.scheme() != "https" {
        return Err("Steam URL must use https".into());
    }
    let Some(host) = url.host_str() else {
        return Err("Steam URL missing host".into());
    };
    if !allowed_hosts.contains(&host) {
        return Err("not a Steam URL".into());
    }
    if !url.username().is_empty() || url.password().is_some() || url.port().is_some() {
        return Err("unexpected authority components in Steam URL".into());
    }
    if let Some(segments) = url.path_segments() {
        for seg in segments {
            check_segment(seg)?;
        }
    }
    Ok(())
}

pub(crate) fn validate_steam_url_hosts(raw: &str, allowed_hosts: &[&str]) -> Result<reqwest::Url, String> {
    if raw.bytes().any(|b| b < 0x20 || b == 0x7f) || raw.contains('\\') {
        return Err("forbidden character in URL".into());
    }
    let url = reqwest::Url::parse(raw).map_err(|e| e.to_string())?;
    check_steam_url_parts(&url, allowed_hosts)?;
    // Check raw pre-normalization segments so literal `/../` is rejected before Url::parse collapses it.
    if let Some((_, after_scheme)) = raw.split_once("://") {
        let before_q = after_scheme.split(['?', '#']).next().unwrap_or("");
        if let Some((_, raw_path)) = before_q.split_once('/') {
            for seg in raw_path.split('/') {
                check_segment(seg)?;
            }
        }
    }
    Ok(url)
}

pub(crate) fn validate_steam_url(raw: &str, expected_host: &str) -> Result<reqwest::Url, String> {
    validate_steam_url_hosts(raw, &[expected_host])
}

fn new_session_id() -> String {
    auth::random_hex(12)
}

/// Every game with items in an account's inventory, read from the
/// `g_rgAppContextData` list that Steam's inventory page is built from.
pub async fn inventory_apps(web: Option<&Web>, steam_id: SteamId) -> (Result<Vec<model::InventoryApp>, String>, Option<model::fees::Rules>) {
    let url = format!("{COMMUNITY}/profiles/{steam_id}/inventory/");
    let resp = match web {
        Some(w) => w.http.get(&url).send().await,
        None => crate::http().get(&url).send().await,
    };
    let body = match resp {
        Ok(r) => r.text().await.map_err(|e| e.to_string()),
        Err(e) => Err(e.to_string()),
    };
    match body {
        Ok(b) => (parse_app_contexts(&b), wallet_rules(&b)),
        Err(e) => (Err(e), None),
    }
}

/// Fee rules for the signed-in wallet, from a page's `g_rgWalletInfo`.
pub fn wallet_rules(body: &str) -> Option<model::fees::Rules> {
    let at = body.find("g_rgWalletInfo")?;
    let rest = &body[at..];
    let start = rest.find('{')?;
    let v: Value = serde_json::Deserializer::from_str(&rest[start..]).into_iter::<Value>().next()?.ok()?;
    let f = |k: &str| v.get(k).and_then(|x| x.as_f64().or_else(|| x.as_str()?.parse().ok()));
    let d = model::fees::Rules::default();
    Some(model::fees::Rules {
        minimum: f("wallet_fee_minimum").map(|x| x as u64).unwrap_or(d.minimum).max(1),
        percent: f("wallet_fee_percent").unwrap_or(d.percent),
        base: f("wallet_fee_base").map(|x| x as u64).unwrap_or(d.base),
        game_default: f("wallet_publisher_fee_percent_default").unwrap_or(d.game_default),
    })
}

pub fn parse_app_contexts(body: &str) -> Result<Vec<model::InventoryApp>, String> {
    let Some(at) = body.find("g_rgAppContextData") else {
        return Err(if body.contains("profile_private_info") || body.contains("private_profile") { "private".into() } else { "unavailable".into() });
    };
    let rest = &body[at..];
    let start = rest.find('{').ok_or("unavailable")?;
    let value = serde_json::Deserializer::from_str(&rest[start..]).into_iter::<Value>().next().ok_or("unavailable")?.map_err(|e| e.to_string())?;
    let mut apps: Vec<model::InventoryApp> = value
        .as_object()
        .map(|o| o.values().collect::<Vec<_>>())
        .unwrap_or_default()
        .into_iter()
        .filter_map(|a| {
            let app_id = num(a.get("appid")?)? as AppId;
            let mut contexts: Vec<(u64, String, u32)> = a
                .get("rgContexts")
                .and_then(|c| c.as_object())
                .map(|c| {
                    c.values()
                        .filter_map(|x| {
                            let id = num(x.get("id")?)?;
                            Some((id, str_of(x, "name"), x.get("asset_count").and_then(num).unwrap_or(0) as u32))
                        })
                        .collect()
                })
                .unwrap_or_default();
            contexts.sort_by_key(|c| c.0);
            Some(model::InventoryApp { app_id, name: str_of(a, "name"), icon: Some(str_of(a, "icon")).filter(|s| !s.is_empty()), contexts })
        })
        .collect();
    let count = |a: &model::InventoryApp| a.contexts.iter().map(|c| c.2 as u64).sum::<u64>();
    apps.sort_by(|a, b| count(b).cmp(&count(a)).then_with(|| a.name.cmp(&b.name)));
    Ok(apps)
}

fn num(v: &Value) -> Option<u64> {
    v.as_u64().or_else(|| v.as_str()?.replace(',', "").parse().ok())
}

fn str_of(v: &Value, key: &str) -> String {
    v.get(key).and_then(|n| n.as_str()).unwrap_or_default().to_string()
}

pub(crate) fn encode(s: &str) -> String {
    s.bytes().map(|b| if b.is_ascii_alphanumeric() || b"-_.~".contains(&b) { (b as char).to_string() } else { format!("%{b:02X}") }).collect()
}

/// Market listing page of an item.
pub fn listing_url(app: AppId, hash_name: &str) -> String {
    format!("{COMMUNITY}/market/listings/{app}/{}", encode(hash_name))
}

/// What an item's market page tells us: order book, price history, fee rules.
#[derive(Default)]
struct Page {
    name_id: Option<u64>,
    rules: Option<model::fees::Rules>,
    book: Option<Value>,
    history: Option<Value>,
}

async fn listing_page(http: &reqwest::Client, app: AppId, hash_name: &str) -> Page {
    let Ok(r) = http.get(listing_url(app, hash_name)).send().await else { return Page::default() };
    if !r.status().is_success() {
        return Page::default();
    }
    let body = r.text().await.unwrap_or_default();
    Page {
        // Older market pages load the order book with a separate request.
        name_id: body.find("Market_LoadOrderSpread(").and_then(|at| {
            let rest = &body[at + "Market_LoadOrderSpread(".len()..];
            rest.trim_start().chars().take_while(|c| c.is_ascii_digit()).collect::<String>().parse().ok()
        }),
        rules: wallet_rules(&body),
        // Current market pages embed their data as an escaped JSON string.
        book: embedded(&body, r#"{\"amtMaxBuyOrder\""#),
        history: embedded(&body, r#"{\"ecurrency\":"#).filter(|v| v.get("prices").is_some()),
    }
}

/// Parse the JSON object starting at `marker` inside a JS string literal.
fn embedded(body: &str, marker: &str) -> Option<Value> {
    let at = body.find(marker)?;
    let mut text = String::new();
    let mut chars = body[at..].chars();
    while let Some(c) = chars.next() {
        if c == '\\' {
            match chars.next()? {
                'n' => text.push('\n'),
                't' => text.push('\t'),
                'u' => {
                    let hex: String = chars.by_ref().take(4).collect();
                    text.push(char::from_u32(u32::from_str_radix(&hex, 16).ok()?).unwrap_or('?'));
                }
                other => text.push(other),
            }
        } else if c == '"' {
            break; // end of the JS string
        } else {
            text.push(c);
        }
    }
    serde_json::Deserializer::from_str(&text).into_iter::<Value>().next()?.ok()
}

/// [price, count, price, count, ...] -> pairs, best first as Steam sends them.
fn ladder(v: Option<&Value>) -> Vec<(u64, u64)> {
    let nums: Vec<u64> = v.and_then(|x| x.as_array()).map(|a| a.iter().filter_map(num).collect()).unwrap_or_default();
    nums.chunks(2).filter(|c| c.len() == 2).map(|c| (c[0], c[1])).collect()
}

pub async fn price(web: Option<&Web>, app: AppId, hash_name: &str, currency: i32, country: &str, full: bool) -> Result<model::MarketPrice, String> {
    let fallback = crate::http();
    let http = web.map(|w| &w.http).unwrap_or(&fallback);
    let currency = if currency > 0 { currency } else { 1 };
    let mut out = model::MarketPrice { currency, ..Default::default() };
    let r = http
        .get(format!("{COMMUNITY}/market/priceoverview/"))
        .query(&[("appid", app.to_string()), ("currency", currency.to_string()), ("market_hash_name", hash_name.to_string())])
        .send()
        .await
        .map_err(|e| e.to_string())?;
    if r.status().as_u16() == 429 {
        return Err("Steam is limiting price requests. Try again in a minute.".into());
    }
    let v: Value = r.json().await.unwrap_or(Value::Null);
    if v.get("success").and_then(|s| s.as_bool()) == Some(true) {
        out.lowest = v.get("lowest_price").and_then(|s| s.as_str()).map(String::from);
        out.median = v.get("median_price").and_then(|s| s.as_str()).map(String::from);
        out.volume = v.get("volume").and_then(num);
    }
    if !full {
        return Ok(out);
    }
    let page = listing_page(http, app, hash_name).await;
    out.fee_rules = page.rules;
    if let Some(book) = &page.book {
        let book_currency = book.get("eCurrency").and_then(num).map(|c| c as i32);
        // Only use the embedded book if it's in your wallet currency.
        if book_currency.is_none_or(|c| c == currency) {
            out.highest_buy = book.get("amtMaxBuyOrder").and_then(num);
            out.lowest_sell = book.get("amtMinSellOrder").and_then(num);
            out.buy_count = book.get("cBuyOrders").and_then(num);
            out.sell_count = book.get("cSellOrders").and_then(num);
            out.buy_ladder = ladder(book.get("rgCompactBuyOrders")).into_iter().take(8).collect();
            out.sell_ladder = ladder(book.get("rgCompactSellOrders")).into_iter().take(8).collect();
        }
    }
    if let Some(h) = &page.history {
        let same = h.get("ecurrency").and_then(num).map(|c| c as i32).is_none_or(|c| c == currency);
        if same {
            out.history = h
                .get("prices")
                .and_then(|p| p.as_array())
                .map(|p| {
                    p.iter()
                        .filter_map(|x| Some((num(x.get("time")?)?, x.get("price_median")?.as_f64()?, x.get("purchases").and_then(num).unwrap_or(0) as u32)))
                        .collect()
                })
                .unwrap_or_default();
        }
    }
    if out.lowest_sell.is_none() {
        if let Some(id) = page.name_id {
            let h: Value = http
                .get(format!("{COMMUNITY}/market/itemordershistogram"))
                .query(&[
                    ("country", country.to_uppercase()),
                    ("language", "english".into()),
                    ("currency", currency.to_string()),
                    ("item_nameid", id.to_string()),
                    ("two_factor", "0".into()),
                ])
                .send()
                .await
                .map_err(|e| e.to_string())?
                .json()
                .await
                .unwrap_or(Value::Null);
            out.highest_buy = h.get("highest_buy_order").and_then(num);
            out.lowest_sell = h.get("lowest_sell_order").and_then(num);
            out.buy_count = h.get("buy_order_count").and_then(num);
            out.sell_count = h.get("sell_order_count").and_then(num);
        }
    }
    if out.lowest.is_none() && out.lowest_sell.is_none() && out.volume.is_none() {
        return Err("Nobody is selling this item right now".into());
    }
    Ok(out)
}

/// List an item. Returns whether Steam wants a mobile/email confirmation.
/// Send a trade offer: `give` from your inventory, `take` from theirs.
/// `token` is the partner's trade URL token (needed when you aren't friends).
pub async fn send_trade_offer(web: &Web, partner: SteamId, token: Option<&str>, message: &str, give: &[model::TradeAsset], take: &[model::TradeAsset]) -> Result<model::TradeOfferSent, String> {
    let account = partner & 0xFFFF_FFFF;
    let assets = |list: &[model::TradeAsset]| -> Vec<Value> {
        list.iter()
            .map(|a| serde_json::json!({ "appid": a.app_id, "contextid": a.context_id.to_string(), "amount": a.amount.max(1), "assetid": a.asset_id.to_string() }))
            .collect()
    };
    let offer = serde_json::json!({
        "newversion": true,
        "version": give.len() + take.len() + 1,
        "me": { "assets": assets(give), "currency": [], "ready": false },
        "them": { "assets": assets(take), "currency": [], "ready": false },
    });
    let params = match token {
        Some(t) => serde_json::json!({ "trade_offer_access_token": t }),
        None => serde_json::json!({}),
    };
    let referer = match token {
        Some(t) => format!("{COMMUNITY}/tradeoffer/new/?partner={account}&token={}", encode(t)),
        None => format!("{COMMUNITY}/tradeoffer/new/?partner={account}"),
    };
    let resp = web
        .http
        .post(format!("{COMMUNITY}/tradeoffer/new/send"))
        .header("Referer", referer)
        .form(&[
            ("sessionid", web.session_id.clone()),
            ("serverid", "1".to_string()),
            ("partner", partner.to_string()),
            ("tradeoffermessage", message.to_string()),
            ("json_tradeoffer", offer.to_string()),
            ("captcha", String::new()),
            ("trade_offer_create_params", params.to_string()),
        ])
        .send()
        .await
        .map_err(|e| e.to_string())?;
    let v: Value = resp.json().await.map_err(|e| e.to_string())?;
    trade_result(&v)
}

/// Steam's answer to a trade offer: its id, or `strError`.
fn trade_result(v: &Value) -> Result<model::TradeOfferSent, String> {
    match v.get("tradeofferid").and_then(num) {
        Some(id) if id > 0 => Ok(model::TradeOfferSent {
            id,
            needs_confirmation: v.get("needs_mobile_confirmation").and_then(|b| b.as_bool()).unwrap_or(false) || v.get("needs_email_confirmation").and_then(|b| b.as_bool()).unwrap_or(false),
        }),
        _ => Err(Some(str_of(v, "strError")).filter(|m| !m.is_empty()).unwrap_or_else(|| "Steam didn't accept the trade offer".into())),
    }
}

pub async fn sell(web: &Web, app: AppId, context: u64, asset: u64, amount: u64, receive: u64) -> Result<bool, String> {
    let v: Value = web
        .http
        .post(format!("{COMMUNITY}/market/sellitem/"))
        .header("Referer", format!("{COMMUNITY}/profiles/{}/inventory/", web.me))
        .form(&[
            ("sessionid", web.session_id.clone()),
            ("appid", app.to_string()),
            ("contextid", context.to_string()),
            ("assetid", asset.to_string()),
            ("amount", amount.max(1).to_string()),
            ("price", receive.to_string()),
        ])
        .send()
        .await
        .map_err(|e| e.to_string())?
        .json()
        .await
        .map_err(|e| e.to_string())?;
    if v.get("success").and_then(|s| s.as_bool()) == Some(true) {
        Ok(v.get("requires_confirmation").and_then(num).unwrap_or(0) == 1 || v.get("needs_mobile_confirmation").and_then(|b| b.as_bool()).unwrap_or(false))
    } else {
        Err(Some(str_of(&v, "message")).filter(|m| !m.is_empty()).unwrap_or_else(|| "Steam didn't accept the listing".into()))
    }
}

pub async fn cancel_listing(web: &Web, listing: u64) -> Result<(), String> {
    let r = web
        .http
        .post(format!("{COMMUNITY}/market/removelisting/{listing}"))
        .header("Referer", format!("{COMMUNITY}/market/"))
        .form(&[("sessionid", web.session_id.clone())])
        .send()
        .await
        .map_err(|e| e.to_string())?;
    if r.status().is_success() { Ok(()) } else { Err(format!("Steam answered {}", r.status())) }
}

pub async fn cancel_buy_order(web: &Web, order: u64) -> Result<(), String> {
    let v: Value = web
        .http
        .post(format!("{COMMUNITY}/market/cancelbuyorder/"))
        .header("Referer", format!("{COMMUNITY}/market/"))
        .form(&[("sessionid", web.session_id.clone()), ("buy_orderid", order.to_string())])
        .send()
        .await
        .map_err(|e| e.to_string())?
        .json()
        .await
        .map_err(|e| e.to_string())?;
    if num(v.get("success").unwrap_or(&Value::Null)) == Some(1) { Ok(()) } else { Err("Steam didn't cancel the order".into()) }
}

/// Your active listings, ones waiting for confirmation, and buy orders.
pub async fn my_listings(web: &Web) -> Result<model::MyListings, String> {
    let mut out = model::MyListings::default();
    let mut start = 0u64;
    loop {
        let v: Value = web
            .http
            .get(format!("{COMMUNITY}/market/mylistings"))
            .query(&[("norender", "1".to_string()), ("start", start.to_string()), ("count", "100".to_string())])
            .send()
            .await
            .map_err(|e| e.to_string())?
            .json()
            .await
            .map_err(|_| "Couldn't read your listings. Is the market available on your account?".to_string())?;
        if v.get("success").and_then(|s| s.as_bool()) != Some(true) {
            return Err("Steam didn't return your listings".into());
        }
        let page = |key: &str, status: model::ListingStatus| -> Vec<model::MyListing> {
            v.get(key).and_then(|l| l.as_array()).map(|l| l.iter().filter_map(|x| listing(x, status)).collect()).unwrap_or_default()
        };
        let active = page("listings", model::ListingStatus::Active);
        let n = active.len() as u64;
        out.listings.extend(active);
        if start == 0 {
            out.listings.extend(page("listings_to_confirm", model::ListingStatus::NeedsConfirmation));
            out.listings.extend(page("listings_on_hold", model::ListingStatus::OnHold));
            out.buy_orders = v.get("buy_orders").and_then(|l| l.as_array()).map(|l| l.iter().filter_map(buy_order).collect()).unwrap_or_default();
        }
        let total = v.get("total_count").and_then(num).unwrap_or(0);
        start += 100;
        if n < 100 || start >= total || start >= 2000 {
            break;
        }
    }
    Ok(out)
}

fn listing(x: &Value, status: model::ListingStatus) -> Option<model::MyListing> {
    let a = x.get("asset")?;
    if tracing::enabled!(tracing::Level::DEBUG) {
        let keys = ["price", "original_price", "fee", "steam_fee", "publisher_fee", "publisher_fee_percent", "converted_price", "converted_fee", "currencyid", "converted_currencyid", "original_amount_listed"];
        tracing::debug!("listing fields: {:?}", keys.iter().map(|k| (*k, x.get(*k))).collect::<Vec<_>>());
    }
    let receive = x.get("original_price").or_else(|| x.get("price")).and_then(num).unwrap_or(0);
    let fee = x.get("fee").and_then(num).unwrap_or(0);
    Some(model::MyListing {
        listing_id: num(x.get("listingid")?)?,
        app_id: a.get("appid").and_then(num).unwrap_or(0) as AppId,
        name: str_of(a, "name"),
        game: str_of(a, "type"),
        icon: str_of(a, "icon_url"),
        name_color: Some(str_of(a, "name_color")).filter(|c| !c.is_empty()),
        you_receive: receive,
        buyer_pays: receive + fee,
        currency: x.get("currencyid").and_then(num).map(|c| (c as i32) - 2000).unwrap_or(1),
        amount: a.get("amount").and_then(num).unwrap_or(1).max(1),
        created: x.get("time_created").and_then(num).unwrap_or(0),
        status,
        market_hash_name: str_of(a, "market_hash_name"),
    })
}

fn buy_order(x: &Value) -> Option<model::BuyOrder> {
    let d = x.get("description");
    Some(model::BuyOrder {
        order_id: num(x.get("buy_orderid")?)?,
        app_id: x.get("appid").and_then(num).unwrap_or(0) as AppId,
        name: d.map(|d| str_of(d, "name")).filter(|n| !n.is_empty()).unwrap_or_else(|| str_of(x, "hash_name")),
        icon: d.map(|d| str_of(d, "icon_url")).unwrap_or_default(),
        price: x.get("price").and_then(num).unwrap_or(0),
        currency: x.get("wallet_currency").and_then(num).unwrap_or(1) as i32,
        quantity: x.get("quantity").and_then(num).unwrap_or(0),
        remaining: x.get("quantity_remaining").and_then(num).unwrap_or(0),
        market_hash_name: str_of(x, "hash_name"),
    })
}

#[cfg(test)]
mod tests {
    #[test]
    fn reads_trade_offer_results() {
        let ok = super::trade_result(&serde_json::json!({"tradeofferid": "123456", "needs_mobile_confirmation": true})).unwrap();
        assert_eq!((ok.id, ok.needs_confirmation), (123456, true));
        assert_eq!(super::trade_result(&serde_json::json!({"strError": "You cannot trade with this user (15)"})), Err("You cannot trade with this user (15)".to_string()));
    }

    #[test]
    fn reads_embedded_order_book() {
        let page = r#"<script>x = JSON.parse("{\"queries\":[{\"state\":{\"data\":{\"amtMaxBuyOrder\":null,\"amtMinSellOrder\":3,\"eCurrency\":1,\"cBuyOrders\":0,\"cSellOrders\":51830,\"rgCompactBuyOrders\":[],\"rgCompactSellOrders\":[3,11059,4,25755]},\"x\":1}]}");</script>"#;
        let book = super::embedded(page, r#"{\"amtMaxBuyOrder\""#).unwrap();
        assert_eq!(book["cSellOrders"], 51830);
        assert_eq!(super::ladder(book.get("rgCompactSellOrders")), vec![(3, 11059), (4, 25755)]);
    }

    #[test]
    fn reads_app_contexts() {
        let page = r#"<script>var g_rgAppContextData = {"753":{"appid":753,"name":"Steam","icon":"https://x/753.jpg","asset_count":12,"rgContexts":{"6":{"asset_count":12,"id":"6","name":"Community"}}},"730":{"appid":730,"name":"Counter-Strike 2","asset_count":40,"rgContexts":{"2":{"asset_count":40,"id":"2","name":"Backpack"}}}};
var g_strInventoryLoadURL = "x";</script>"#;
        let apps = super::parse_app_contexts(page).unwrap();
        assert_eq!(apps.len(), 2);
        assert_eq!(apps[0].app_id, 730);
        assert_eq!(apps[0].contexts, vec![(2, "Backpack".to_string(), 40)]);
        assert_eq!(apps[1].contexts[0].0, 6);
    }

    #[tokio::test]
    async fn cookies_are_host_only_and_redirects_stay_on_steam() {
        use reqwest::cookie::CookieStore;
        use std::io::{Read, Write};
        use std::net::TcpListener;

        let jar = super::build_cookie_jar(76561198000000001, "tok", "deadbeef");
        for host in super::STEAM_WEB_HOSTS.into_iter().chain([crate::chat_attachments::HOST]) {
            let root: reqwest::Url = format!("https://{host}/market/").parse().unwrap();
            let sub: reqwest::Url = format!("https://evil.{host}/market/").parse().unwrap();
            assert!(jar.cookies(&root).is_some(), "expected cookie on {host}");
            assert!(jar.cookies(&sub).is_none(), "cookie must be host-only, leaked to {sub}");
        }

        for bad in [
            "https://steamcommunity.com.evil.tld/discussions/",
            "https://user@steamcommunity.com/discussions/",
            "http://steamcommunity.com/discussions/",
            "https://steamcommunity.com/app/440/../secret",
            "https://steamcommunity.com/app/440/%2e%2e/secret",
            "https://steamcommunity.com/app/440/%2E./secret",
        ] {
            assert!(super::validate_steam_url(bad, "steamcommunity.com").is_err(), "must reject {bad}");
        }
        assert!(super::validate_steam_url("https://steamcommunity.com/app/440/discussions/", "steamcommunity.com").is_ok());

        // Off-origin redirect from a local server must be blocked by Web.http redirect policy.
        let listener = TcpListener::bind("127.0.0.1:0").unwrap();
        let addr = listener.local_addr().unwrap();
        let srv = std::thread::spawn(move || {
            let (mut sock, _) = listener.accept().unwrap();
            let mut buf = [0u8; 1024];
            let _ = sock.read(&mut buf);
            let _ = sock.write_all(
                b"HTTP/1.1 302 Found\r\nLocation: https://steamcommunity.com.evil.tld/steal\r\nContent-Length: 0\r\nConnection: close\r\n\r\n",
            );
        });
        let http = super::build_web_http(jar).unwrap();
        let err = http.get(format!("http://{addr}/start")).send().await.unwrap_err();
        assert!(err.is_redirect(), "expected redirect policy error, got {err}");
        srv.join().unwrap();
    }
}

/// Fetch a signed-in Steam web page (for features without an API).
pub async fn page(web: &Web, url: &str) -> Result<String, String> {
    let url = validate_steam_url_hosts(url, &STEAM_WEB_HOSTS)?;
    let r = web.http.get(url).send().await.map_err(|e| e.to_string())?;
    let status = r.status();
    let body = r.text().await.map_err(|e| e.to_string())?;
    if !status.is_success() {
        return Err(format!("Steam answered {status}"));
    }
    Ok(body)
}
