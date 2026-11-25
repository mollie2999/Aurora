use super::{BindingChoice, LayoutDocument, MAX_BYTES, store_layout};
use model::AppId;
use net::Connection;
use proto::*;
use std::sync::OnceLock;
use std::time::Duration;

const CONTROLLER_CONFIGS_APP: u32 = 241100;
/// `EPublishedFileInfoMatchingFileType::k_PFI_MatchingFileType_ControllerBindings` (request filter).
const QUERY_CONTROLLER_BINDINGS: u32 = 15;
/// `EWorkshopFileType::k_EWorkshopFileTypeControllerBinding` (response `PublishedFileDetails.file_type`).
const DETAIL_CONTROLLER_BINDING: u32 = 12;
const PAGE_SIZE: u32 = 30;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct LayoutRequest {
    pub account: u32,
    pub app_id: AppId,
    pub request_id: u64,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum QueryMode {
    /// Live Steam CM query: `appid = 241100`, `filetype = 15`, `required_kv_tags = [app = <game>]`.
    KvApp,
    /// Secondary fallback if a title ever uses `appid` as the kv-tag key.
    KvAppId,
}

impl QueryMode {
    const FIRST_PAGE_ORDER: [Self; 2] = [Self::KvApp, Self::KvAppId];

    fn kv_key(self) -> &'static str {
        match self {
            Self::KvApp => "app",
            Self::KvAppId => "appid",
        }
    }

    fn build_request(self, app_id: AppId, page: u32) -> CPublishedFileQueryFilesRequest {
        CPublishedFileQueryFilesRequest {
            query_type: Some(0), // RankedByVote
            page: Some(page.max(1)),
            numperpage: Some(PAGE_SIZE),
            // Steam indexes controller configs under consumer appid 241100, NOT creator_appid.
            appid: Some(CONTROLLER_CONFIGS_APP),
            filetype: Some(QUERY_CONTROLLER_BINDINGS),
            required_kv_tags: vec![c_published_file_query_files_request::KvTag {
                key: Some(self.kv_key().into()),
                value: Some(app_id.to_string()),
            }],
            return_details: Some(true),
            return_tags: Some(true),
            return_kv_tags: Some(true),
            return_vote_data: Some(true),
            return_short_description: Some(true),
            return_metadata: Some(true),
            return_playtime_stats: Some(30),
            strip_description_bbcode: Some(true),
            ..Default::default()
        }
    }
}

#[derive(Debug, Clone)]
pub struct CommunityCursor {
    page: u32,
    mode: QueryMode,
}

#[derive(Debug, Clone)]
pub struct CommunityLayout {
    pub id: u64,
    pub author: u64,
    pub title: String,
    pub description: String,
    pub controller_type: String,
    pub votes_up: u32,
    pub votes_down: u32,
    pub sessions: u64,
}

#[derive(Debug, Clone, Default)]
pub struct CommunityPage {
    pub items: Vec<CommunityLayout>,
    pub next: Option<CommunityCursor>,
}

fn explicit_app_tag(detail: &PublishedFileDetails) -> Option<AppId> {
    detail.kvtags.iter().find_map(|tag| {
        matches!(
            tag.key().to_ascii_lowercase().as_str(),
            "app" | "appid" | "app_id" | "gameid" | "game_id"
        )
        .then(|| tag.value().parse::<u32>().ok())
        .flatten()
    })
}

fn belongs_to(detail: &PublishedFileDetails, app_id: AppId, query_scoped_to_app: bool) -> bool {
    if detail.consumer_appid() == app_id {
        return true;
    }
    if let Some(tagged) = explicit_app_tag(detail) {
        return tagged == app_id;
    }
    if detail.tags.iter().any(|tag| tag.tag().parse::<u32>().ok() == Some(app_id)) {
        return true;
    }
    query_scoped_to_app && detail.consumer_appid() == CONTROLLER_CONFIGS_APP
}

fn controller_file(detail: &PublishedFileDetails) -> bool {
    let ft = detail.file_type();
    (ft == DETAIL_CONTROLLER_BINDING || ft == 0)
        && (detail.consumer_appid() == CONTROLLER_CONFIGS_APP || detail.creator_appid() == CONTROLLER_CONFIGS_APP)
        && !detail.banned()
        && detail.publishedfileid() != 0
}

fn layout_from_detail(d: PublishedFileDetails) -> CommunityLayout {
    let controller_type = d
        .kvtags
        .iter()
        .find(|tag| tag.key().eq_ignore_ascii_case("controller_type"))
        .map(|tag| tag.value())
        .or_else(|| d.tags.iter().map(|tag| tag.tag()).find(|tag| tag.starts_with("controller_")))
        .unwrap_or_default()
        .chars()
        .take(120)
        .collect();
    let desc = if !d.file_description().is_empty() {
        d.file_description()
    } else {
        d.short_description()
    };
    CommunityLayout {
        id: d.publishedfileid(),
        author: d.creator(),
        title: d.title().chars().take(256).collect(),
        description: desc.chars().take(2048).collect(),
        controller_type,
        votes_up: d.vote_data.as_ref().map_or(0, |v| v.votes_up()),
        votes_down: d.vote_data.as_ref().map_or(0, |v| v.votes_down()),
        sessions: d
            .playtime_stats
            .as_ref()
            .map_or_else(|| d.lifetime_playtime_sessions(), |v| v.num_sessions()),
    }
}

pub(crate) async fn query(conn: &Connection, app_id: AppId, cursor: Option<CommunityCursor>) -> Result<CommunityPage, String> {
    let (single_mode, page) = match cursor {
        Some(c) => (Some(c.mode), c.page.max(1)),
        None => (None, 1),
    };
    let modes: &[QueryMode] = match &single_mode {
        Some(mode) => std::slice::from_ref(mode),
        None => &QueryMode::FIRST_PAGE_ORDER,
    };
    let mut last_page = CommunityPage::default();

    for &mode in modes {
        let req = mode.build_request(app_id, page);
        let response = conn
            .call::<_, CPublishedFileQueryFilesResponse>("PublishedFile.QueryFiles#1", &req)
            .await
            .map_err(|err| err.to_string())?;
        let raw_count = response.publishedfiledetails.len();
        let total = response.total();
        let first_sample = response.publishedfiledetails.first().map(|d| {
            (
                d.publishedfileid(),
                d.file_type(),
                d.creator_appid(),
                d.consumer_appid(),
                !d.file_url().is_empty(),
                d.banned(),
            )
        });
        let items: Vec<CommunityLayout> = response
            .publishedfiledetails
            .into_iter()
            .filter(|d| controller_file(d) && !d.file_url().is_empty() && belongs_to(d, app_id, true))
            .take(PAGE_SIZE as usize)
            .map(layout_from_detail)
            .collect();
        let next = (raw_count > 0 && (page as u64) * (PAGE_SIZE as u64) < total as u64)
            .then_some(CommunityCursor { page: page + 1, mode });
        tracing::info!(
            app_id,
            ?mode,
            page,
            total,
            raw_count,
            kept = items.len(),
            ?first_sample,
            "Queried Steam community controller layouts"
        );
        if !items.is_empty() {
            return Ok(CommunityPage { items, next });
        }
        last_page = CommunityPage { items, next };
    }
    Ok(last_page)
}

fn trusted(url: &reqwest::Url) -> bool {
    url.scheme() == "https" && url.port_or_known_default() == Some(443) && url.username().is_empty()
        && url.password().is_none() && url.host_str().is_some_and(|host| {
            matches!(host, "steamusercontent-a.akamaihd.net" | "steamuserimages-a.akamaihd.net" | "steamcdn-a.akamaihd.net")
            || host.ends_with(".steamusercontent.com") || host.ends_with(".steamstatic.com")
            || host.ends_with(".steamcontent.com")
        })
}

fn http() -> Result<&'static reqwest::Client, String> {
    static HTTP: OnceLock<Result<reqwest::Client, reqwest::Error>> = OnceLock::new();
    HTTP.get_or_init(|| reqwest::Client::builder().https_only(true)
        .timeout(Duration::from_secs(20)).connect_timeout(Duration::from_secs(5))
        .redirect(reqwest::redirect::Policy::custom(|attempt| {
            if attempt.previous().len() > 5 || !trusted(attempt.url()) {
                attempt.error("untrusted Workshop redirect or redirect limit exceeded")
            } else {
                attempt.follow()
            }
        })).build()).as_ref().map_err(ToString::to_string)
}

pub(super) async fn download_text(conn: &Connection, id: u64, app_id: Option<AppId>) -> Result<String, String> {
    let req = CPublishedFileGetDetailsRequest {
        publishedfileids: vec![id],
        includetags: Some(true),
        includekvtags: Some(true),
        ..Default::default()
    };
    let details = conn.call::<_, CPublishedFileGetDetailsResponse>("PublishedFile.GetDetails#1", &req)
        .await.map_err(|err| err.to_string())?;
    let detail = details.publishedfiledetails.iter().find(|d| {
        d.publishedfileid() == id && d.result() == 1 && !d.banned()
            && app_id.is_none_or(|app| controller_file(d) && belongs_to(d, app, true))
    }).ok_or_else(|| {
        let sample = details.publishedfiledetails.first().map(|d| {
            (d.publishedfileid(), d.result(), d.file_type(), d.creator_appid(), d.consumer_appid(), d.banned())
        });
        tracing::warn!(id, ?app_id, ?sample, "Steam GetDetails did not match expected controller layout");
        "Steam did not return a matching controller layout".to_string()
    })?;
    let raw = detail.file_url();
    let url = reqwest::Url::parse(raw).map_err(|_| "invalid Workshop download URL")?;
    if raw.contains('\\') || raw.chars().any(char::is_control) || !trusted(&url) {
        return Err("untrusted Workshop download URL".into());
    }
    let mut response = http()?.get(url).send().await.and_then(|r| r.error_for_status())
        .map_err(|err| err.without_url().to_string())?;
    if !response.status().is_success() || response.content_length().is_some_and(|n| n > MAX_BYTES as u64) {
        return Err("Workshop download failed or exceeds 4 MiB".into());
    }
    let mut bytes = Vec::new();
    while let Some(chunk) = response.chunk().await.map_err(|err| err.without_url().to_string())? {
        if chunk.len() > MAX_BYTES - bytes.len() {
            return Err("Workshop layout exceeds 4 MiB".into());
        }
        bytes.extend_from_slice(&chunk);
    }
    let text = String::from_utf8(bytes).map_err(|_| "Workshop layout is not UTF-8")?;
    LayoutDocument::parse(&text).map_err(|err| err.to_string())?;
    Ok(text)
}

pub(crate) async fn download(conn: &Connection, key: LayoutRequest, id: u64) -> Result<BindingChoice, String> {
    let text = download_text(conn, id, Some(key.app_id)).await?;
    let doc = LayoutDocument::parse(&text).map_err(|err| err.to_string())?;
    store_layout(key.account, key.app_id, "community", &format!("{id}.vdf"), &doc)?;
    Ok(BindingChoice::Community(id))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn workshop_urls_are_https_on_steam_content_hosts() {
        for (url, expected) in [
            ("https://images.steamusercontent.com/ugc/1/file", true),
            ("https://steamusercontent-a.akamaihd.net/ugc/1/file", true),
            ("http://images.steamusercontent.com/ugc/1/file", false),
            ("https://images.steamusercontent.com.evil.example/", false),
            ("https://user@images.steamusercontent.com/", false),
            ("https://images.steamusercontent.com:444/", false),
        ] {
            assert_eq!(trusted(&reqwest::Url::parse(url).unwrap()), expected, "{url}");
        }
    }

    #[test]
    fn query_request_uses_241100_bucket_filetype_15_and_app_kv_tag() {
        let req = QueryMode::KvApp.build_request(553850, 1);
        assert_eq!(req.appid, Some(CONTROLLER_CONFIGS_APP));
        assert_eq!(req.creator_appid, None);
        assert_eq!(req.filetype, Some(QUERY_CONTROLLER_BINDINGS));
        assert_eq!(req.page, Some(1));
        assert_eq!(req.required_kv_tags.len(), 1);
        assert_eq!(req.required_kv_tags[0].key.as_deref(), Some("app"));
        assert_eq!(req.required_kv_tags[0].value.as_deref(), Some("553850"));
    }

    #[test]
    fn accepts_workshop_controller_binding_file_type_12_not_query_enum_15() {
        let mut detail = PublishedFileDetails {
            publishedfileid: Some(123456),
            creator_appid: Some(CONTROLLER_CONFIGS_APP),
            consumer_appid: Some(CONTROLLER_CONFIGS_APP),
            file_type: Some(DETAIL_CONTROLLER_BINDING),
            banned: Some(false),
            ..Default::default()
        };
        detail.kvtags.push(published_file_details::KvTag {
            key: Some("app".into()),
            value: Some("553850".into()),
        });
        assert!(controller_file(&detail));
        assert!(belongs_to(&detail, 553850, true));
        assert!(!belongs_to(&detail, 440, true));

        // Request enum 15 is not a valid EWorkshopFileType response value.
        detail.file_type = Some(QUERY_CONTROLLER_BINDINGS);
        assert!(!controller_file(&detail));

        // Omitted file_type (0) still passes if consumer_appid == 241100.
        detail.file_type = None;
        assert!(controller_file(&detail));

        // Banned or non-241100 files are rejected.
        detail.file_type = Some(DETAIL_CONTROLLER_BINDING);
        detail.banned = Some(true);
        assert!(!controller_file(&detail));
        detail.banned = Some(false);
        detail.creator_appid = Some(553850);
        detail.consumer_appid = Some(553850);
        assert!(!controller_file(&detail));
    }

    #[test]
    fn belongs_to_checks_kvtags_tags_and_scoped_creator_app() {
        let mut detail = PublishedFileDetails {
            publishedfileid: Some(1744110334),
            creator_appid: Some(CONTROLLER_CONFIGS_APP),
            consumer_appid: Some(CONTROLLER_CONFIGS_APP),
            file_type: Some(DETAIL_CONTROLLER_BINDING),
            ..Default::default()
        };
        // No appid tag: accepted only when Steam already scoped the query to the app.
        assert!(belongs_to(&detail, 553850, true));
        assert!(!belongs_to(&detail, 553850, false));

        // Explicit kvtag ("app" or "appid") for a different app must be rejected even when scoped.
        detail.kvtags.push(published_file_details::KvTag {
            key: Some("app".into()),
            value: Some("440".into()),
        });
        assert!(!belongs_to(&detail, 553850, true));
        assert!(belongs_to(&detail, 440, false));

        // Matching tag works when kvtag is absent.
        detail.kvtags.clear();
        detail.tags.push(published_file_details::Tag {
            tag: Some("553850".into()),
            ..Default::default()
        });
        assert!(belongs_to(&detail, 553850, false));
    }

    #[test]
    fn layout_falls_back_to_short_description() {
        let detail = PublishedFileDetails {
            publishedfileid: Some(42),
            title: Some("Gyro Aim".into()),
            short_description: Some("Flick stick + gyro".into()),
            ..Default::default()
        };
        let layout = layout_from_detail(detail);
        assert_eq!(layout.description, "Flick stick + gyro");
    }
}
