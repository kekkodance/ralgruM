use futures::StreamExt;
use reqwest::{Url, header};
use serde_json::{Value, json};

use super::model::{DiscoverAction, DiscoverItem, DiscoverSection};
use crate::search::client::DeezerSession;
use crate::search::credential::DeezerArl;
use crate::search::models::{Card, Provider, ResultType};
use crate::search::{SearchClient, normalize, release_date};
use crate::smart_mix_title::{generated_smart_mix_title, specific_smart_mix_title};

const GATEWAY: &str = "https://www.deezer.com/ajax/gw-light.php";
const APP_VERSION: &str = "2.5";
const DISCOVER_LANGUAGE: &str = "en";
pub(crate) const MAX_SMART_MIX_ENRICHMENT_IDS: usize = 8;
const SMART_MIX_ENRICHMENT_CONCURRENCY: usize = 2;
const MAX_SMART_MIX_ID_CHARS: usize = 128;

pub(crate) async fn load(
    client: &SearchClient,
    arl: DeezerArl,
) -> Result<Vec<DiscoverSection>, String> {
    let value = load_page(client, arl, "home").await?;
    parse_home(&value)
}

pub(crate) async fn load_channel(
    client: &SearchClient,
    arl: DeezerArl,
    slug: &str,
) -> Result<(String, Vec<DiscoverSection>), String> {
    let slug = valid_channel_slug(slug).ok_or_else(|| "Invalid Deezer channel".to_owned())?;
    let value = load_page(client, arl, &format!("channels/{slug}")).await?;
    parse_channel(&value, &slug)
}

async fn load_page(client: &SearchClient, arl: DeezerArl, page: &str) -> Result<Value, String> {
    let session = client
        .deezer_session(Some(arl))
        .await
        .map_err(|error| error.message)?;
    let cookie = session_cookie_header(&session)?;
    let url = page_request_url(&session.check_form, page).map_err(str::to_owned)?;
    let response = client
        .http()
        .get(url)
        .header(header::COOKIE, cookie)
        .send()
        .await
        .map_err(|_| "Deezer Discover request failed".to_owned())?;
    crate::search::client::deezer_json(response)
        .await
        .map_err(|error| error.message)
}

pub(crate) async fn enrich_smart_mix_titles(
    client: &SearchClient,
    arl: DeezerArl,
    ids: Vec<String>,
) -> Vec<(String, String)> {
    let ids = ids
        .into_iter()
        .filter_map(|id| valid_smart_mix_id(&id))
        .take(MAX_SMART_MIX_ENRICHMENT_IDS)
        .collect::<Vec<_>>();
    if ids.is_empty() {
        return Vec::new();
    }
    let Ok(session) = client.deezer_session(Some(arl)).await else {
        return Vec::new();
    };
    futures::stream::iter(ids.into_iter().map(|id| {
        let client = client.clone();
        let session = session.clone();
        async move { fetch_smart_mix_title(&client, &session, &id).await }
    }))
    .buffer_unordered(SMART_MIX_ENRICHMENT_CONCURRENCY)
    .filter_map(|result| async move { result })
    .collect()
    .await
}

async fn fetch_smart_mix_title(
    client: &SearchClient,
    session: &DeezerSession,
    requested_id: &str,
) -> Option<(String, String)> {
    let requested_id = valid_smart_mix_id(requested_id)?;
    let cookie = session_cookie_header(session).ok()?;
    let url = smart_tracklist_request_url(&session.check_form).ok()?;
    let response = client
        .http()
        .post(url)
        .header(header::COOKIE, cookie)
        .header(header::CONTENT_TYPE, "application/json; charset=UTF-8")
        .json(&smart_tracklist_request(&requested_id))
        .send()
        .await
        .ok()?;
    let value = crate::search::client::deezer_json(response).await.ok()?;
    let results = value.get("results")?;
    let data = results.get("DATA")?.as_object()?;
    if !smart_tracklist_response_matches(data, &requested_id) {
        return None;
    }
    let title = smart_tracklist_response_title(data)?;
    Some((requested_id, title))
}

fn session_cookie_header(session: &DeezerSession) -> Result<header::HeaderValue, String> {
    // Discover stays authenticated-only, so the session always carries an arl.
    let arl = session
        .arl
        .as_ref()
        .ok_or_else(|| "Deezer account required".to_owned())?;
    let mut cookie = arl.cookie_header().map_err(|error| error.message)?;
    if !session.cookies.is_empty() {
        let arl = cookie
            .to_str()
            .map_err(|_| "The saved Deezer session is invalid".to_owned())?;
        cookie = header::HeaderValue::from_str(&format!("{arl}; {}", session.cookies))
            .map_err(|_| "Deezer returned an invalid session".to_owned())?;
        cookie.set_sensitive(true);
    }
    Ok(cookie)
}

#[cfg(test)]
pub(crate) fn home_request_url(check_form: &str) -> Result<Url, &'static str> {
    page_request_url(check_form, "home")
}

#[cfg(test)]
pub(crate) fn channel_request_url(check_form: &str, slug: &str) -> Result<Url, &'static str> {
    let slug = valid_channel_slug(slug).ok_or("Invalid Deezer channel slug")?;
    page_request_url(check_form, &format!("channels/{slug}"))
}

fn page_request_url(check_form: &str, page: &str) -> Result<Url, &'static str> {
    let mut url = Url::parse(GATEWAY).map_err(|_| "Invalid Deezer Discover endpoint")?;
    let gateway_input = serde_json::to_string(&gateway_input_for_page(page))
        .map_err(|_| "Invalid Discover request")?;
    url.query_pairs_mut()
        .append_pair("method", "page.get")
        .append_pair("input", "3")
        .append_pair("api_version", "1.0")
        .append_pair("api_token", check_form)
        .append_pair("gateway_input", &gateway_input);
    Ok(url)
}

fn smart_tracklist_request_url(check_form: &str) -> Result<Url, &'static str> {
    let mut url = Url::parse(GATEWAY).map_err(|_| "Invalid Deezer smart tracklist endpoint")?;
    url.query_pairs_mut()
        .append_pair("method", "deezer.pageSmartTracklist")
        .append_pair("input", "3")
        .append_pair("api_version", "1.0")
        .append_pair("api_token", check_form);
    Ok(url)
}

fn smart_tracklist_request(smarttracklist_id: &str) -> Value {
    json!({ "smarttracklist_id": smarttracklist_id, "lang": "us" })
}

fn gateway_input_for_page(page: &str) -> Value {
    json!({
        "PAGE": page,
        "VERSION": APP_VERSION,
        "SUPPORT": {
            "event-card": ["live-event"],
            "grid-preview-one": ["album", "artist", "artistLineUp", "channel", "livestream", "flow", "playlist", "radio", "show", "smarttracklist", "track", "user", "video-link", "external-link"],
            "grid-preview-two": ["album", "artist", "artistLineUp", "channel", "livestream", "flow", "playlist", "radio", "show", "smarttracklist", "track", "user", "video-link", "external-link"],
            "grid": ["album", "artist", "artistLineUp", "channel", "livestream", "flow", "playlist", "radio", "show", "smarttracklist", "track", "user", "video-link", "external-link"],
            "horizontal-grid": ["album", "artist", "artistLineUp", "channel", "livestream", "flow", "playlist", "radio", "show", "smarttracklist", "track", "user", "video-link", "external-link"],
            "horizontal-list": ["track", "song"],
            "large-card": ["album", "external-link", "playlist", "show", "smarttracklist", "video-link"],
            "list": ["episode"],
            "message": ["call_onboarding"],
            "mini-banner": ["external-link"],
            "slideshow": ["album", "artist", "channel", "external-link", "flow", "livestream", "playlist", "show", "smarttracklist", "user", "video-link"],
            "small-horizontal-grid": ["flow"],
            "long-card-horizontal-grid": ["album", "artist", "artistLineUp", "channel", "livestream", "flow", "playlist", "radio", "show", "smarttracklist", "track", "user", "video-link", "external-link"],
            "filterable-grid": ["flow"]
        },
        "LANG": DISCOVER_LANGUAGE,
        "OPTIONS": ["deeplink_newsandentertainment", "deeplink_subscribeoffer"]
    })
}

pub(crate) fn parse_home(value: &Value) -> Result<Vec<DiscoverSection>, String> {
    parse_sections(value, "", true)
}

pub(crate) fn parse_channel(
    value: &Value,
    slug: &str,
) -> Result<(String, Vec<DiscoverSection>), String> {
    let slug = valid_channel_slug(slug).ok_or_else(|| "Invalid Deezer channel".to_owned())?;
    let title = string(value.pointer("/results/title"));
    if title.trim().is_empty() {
        return Err("Deezer returned an invalid channel response".to_owned());
    }
    let sections = parse_sections(value, &format!("channel:{slug}"), false)?;
    if sections.is_empty() {
        return Err("Deezer channel has no playable sections".to_owned());
    }
    Ok((title, sections))
}

fn parse_sections(
    value: &Value,
    page_identity: &str,
    exclude_home_sections: bool,
) -> Result<Vec<DiscoverSection>, String> {
    let sections = value
        .pointer("/results/sections")
        .and_then(Value::as_array)
        .ok_or_else(|| "Deezer returned an invalid Discover response".to_owned())?;
    let mut parsed = Vec::new();
    for (index, section) in sections.iter().enumerate() {
        let title = string(section.get("title"));
        if exclude_home_sections && excluded_section(&title) {
            continue;
        }
        let Some(items) = section.get("items").and_then(Value::as_array) else {
            continue;
        };
        let items = items.iter().filter_map(parse_item).collect::<Vec<_>>();
        if title.trim().is_empty() || items.is_empty() {
            continue;
        }
        let id = if page_identity.is_empty() {
            format!("deezer:{index}:{}", stable_part(&title))
        } else {
            format!("deezer:{page_identity}:{index}:{}", stable_part(&title))
        };
        parsed.push(DiscoverSection {
            id,
            provider: Provider::Deezer,
            title,
            subtitle: string(section.get("subtitle")),
            items,
        });
    }
    Ok(parsed)
}

pub(crate) fn excluded_section(title: &str) -> bool {
    matches!(
        title.trim().to_ascii_lowercase().as_str(),
        "continue streaming" | "recently you've been loving..." | "recently played"
    )
}

fn parse_item(item: &Value) -> Option<DiscoverItem> {
    let item_type = string(item.get("type")).to_ascii_lowercase();
    let data = item.get("data").unwrap_or(&Value::Null);
    match item_type.as_str() {
        "album" => parse_openable_card(item, data, ResultType::Albums),
        "artist" => parse_openable_card(item, data, ResultType::Artists),
        "playlist" => parse_openable_card(item, data, ResultType::Playlists),
        "flow" | "track" | "smarttracklist" | "channel" => {
            parse_presentation_card(item, data, &item_type)
        }
        _ => None,
    }
}

fn parse_openable_card(item: &Value, data: &Value, kind: ResultType) -> Option<DiscoverItem> {
    let mut card = normalize::normalize_card(Provider::Deezer, kind, data);
    let id = first_string(&[
        id_for_kind(data, kind),
        string(item.get("id")),
        string(data.get("id")),
    ])?;
    if !valid_deezer_id(&id) {
        return None;
    }
    card.id = id;
    if let Some(title) = nonempty(string(item.get("title"))) {
        card.title = title;
    }
    if let Some(subtitle) = nonempty(string(item.get("subtitle"))) {
        card.subtitle = subtitle;
    }
    let artwork = wrapper_artwork(item, data);
    if !artwork.is_empty() {
        card.artwork = artwork;
    }
    if kind == ResultType::Albums {
        card.release_date = release_date(data);
        card.badge = year(&card.release_date);
    }
    Some(DiscoverItem {
        card,
        action: DiscoverAction::OpenDetail,
    })
}

fn parse_presentation_card(item: &Value, data: &Value, item_type: &str) -> Option<DiscoverItem> {
    let id = (if item_type == "smarttracklist" {
        first_smart_mix_id(item, data)
    } else if item_type == "track" {
        first_string(&[
            string(data.get("SNG_ID")),
            string(data.get("id")),
            string(item.get("id")),
            string(data.get("SMARTTRACKLIST_ID")),
        ])
    } else {
        first_string(&[
            string(item.get("id")),
            string(data.get("id")),
            string(data.get("SNG_ID")),
            string(data.get("SMARTTRACKLIST_ID")),
        ])
    })?;
    let fallback_title = match item_type {
        "flow" => "Flow",
        "track" => "Track",
        "smarttracklist" => "Mix",
        "channel" => "Genre",
        _ => "Discover",
    };
    let title = if item_type == "smarttracklist" {
        smarttracklist_title(item, data)
    } else {
        first_string(&[
            string(item.get("title")),
            string(data.get("title")),
            string(data.get("TITLE")),
            string(data.get("SNG_TITLE")),
            string(data.get("ART_NAME")),
        ])
    }
    .unwrap_or_else(|| fallback_title.to_owned());
    let subtitle = first_string(&[
        string(item.get("subtitle")),
        string(data.get("subtitle")),
        string(data.get("SUBTITLE")),
        string(data.get("ART_NAME")),
    ])
    .unwrap_or_default();
    // Flow and smart-mix items mirror their title in the subtitle field; a
    // mirrored subtitle is not information, so drop it for those cards. The
    // smart-mix display title can differ from the raw feed title, so both
    // forms are checked.
    let raw_title = string(item.get("title"));
    let mirrored_subtitle = subtitle.trim().eq_ignore_ascii_case(title.trim())
        || (!raw_title.trim().is_empty() && subtitle.trim().eq_ignore_ascii_case(raw_title.trim()));
    let subtitle = if matches!(item_type, "flow" | "smarttracklist") && mirrored_subtitle {
        String::new()
    } else {
        subtitle
    };
    let action = match item_type {
        "track" if valid_deezer_id(&id) => DiscoverAction::PlayDeezerTrack(id.clone()),
        "flow" => DiscoverAction::PlayDeezerFlow { smart_mix: false },
        "smarttracklist" => DiscoverAction::PlayDeezerFlow { smart_mix: true },
        "channel" => channel_slug(item, data)
            .map(DiscoverAction::OpenDeezerChannel)
            .unwrap_or(DiscoverAction::None),
        _ => DiscoverAction::None,
    };
    let card = Card {
        kind: ResultType::All,
        id,
        title: title.clone(),
        subtitle,
        artwork: wrapper_artwork(item, data),
        source: Provider::Deezer,
        ..Card::default()
    };
    Some(DiscoverItem { card, action })
}

fn channel_slug(item: &Value, data: &Value) -> Option<String> {
    valid_channel_slug(&string(data.get("slug"))).or_else(|| {
        let target = string(item.get("target"));
        let path = target.split(['?', '#']).next().unwrap_or_default().trim();
        let slug = path.strip_prefix("/channels/")?;
        valid_channel_slug(slug)
    })
}

pub(crate) fn valid_channel_slug(value: &str) -> Option<String> {
    let value = value.trim();
    if value.is_empty()
        || value.len() > 64
        || value.starts_with('-')
        || value.ends_with('-')
        || !value
            .bytes()
            .all(|byte| byte.is_ascii_lowercase() || byte.is_ascii_digit() || byte == b'-')
    {
        return None;
    }
    Some(value.to_owned())
}

fn smarttracklist_title(item: &Value, data: &Value) -> Option<String> {
    let candidates = [
        string(item.get("title")),
        string(data.get("TITLE")),
        string(data.get("title")),
        string(item.pointer("/cover_composition/title")),
        string(item.pointer("/cover_composition/TITLE")),
        string(data.pointer("/COVER_COMPOSITION/TITLE")),
        string(data.pointer("/COVER_COMPOSITION/title")),
        string(item.get("cover_title")),
    ];
    candidates
        .iter()
        .find_map(|title| specific_smart_mix_title(title).map(str::to_owned))
        .or_else(|| {
            [
                string(data.get("AUTO_GENERATED_TITLE")),
                string(item.get("auto_generated_title")),
            ]
            .iter()
            .find_map(|title| generated_smart_mix_title(title))
        })
}

fn first_smart_mix_id(item: &Value, data: &Value) -> Option<String> {
    [
        string(data.get("SMARTTRACKLIST_ID")),
        string(data.get("CONFIGURATION_ID")),
        string(item.get("id")),
        string(data.get("id")),
    ]
    .iter()
    .find_map(|value| valid_smart_mix_id(value))
}

pub(crate) fn valid_smart_mix_id(value: &str) -> Option<String> {
    let value = value.trim();
    if value.is_empty()
        || value.chars().count() > MAX_SMART_MIX_ID_CHARS
        || value.chars().any(char::is_control)
    {
        None
    } else {
        Some(value.to_owned())
    }
}

fn smart_tracklist_response_matches(
    data: &serde_json::Map<String, Value>,
    requested_id: &str,
) -> bool {
    let requested_id = requested_id.trim();
    if requested_id.is_empty() {
        return false;
    }
    let stable_ids = [
        string(data.get("SMARTTRACKLIST_ID")),
        string(data.get("CONFIGURATION_ID")),
    ]
    .iter()
    .filter(|value| !value.trim().is_empty())
    .map(|value| valid_smart_mix_id(value))
    .collect::<Vec<_>>();
    !stable_ids.is_empty()
        && stable_ids
            .iter()
            .all(|value| value.as_deref() == Some(requested_id))
}

fn smart_tracklist_response_title(data: &serde_json::Map<String, Value>) -> Option<String> {
    [
        data.get("COVER_COMPOSITION")
            .and_then(|value| value.get("TITLE")),
        data.get("COVER_COMPOSITION")
            .and_then(|value| value.get("title")),
        data.get("TITLE"),
        data.get("title"),
    ]
    .into_iter()
    .map(|value| string(value))
    .find_map(|value| specific_smart_mix_title(&value).map(str::to_owned))
    .or_else(|| {
        [
            data.get("AUTO_GENERATED_TITLE"),
            data.get("auto_generated_title"),
        ]
        .into_iter()
        .map(|value| string(value))
        .find_map(|value| generated_smart_mix_title(&value))
    })
}

fn id_for_kind(value: &Value, kind: ResultType) -> String {
    match kind {
        ResultType::Albums => string(value.get("ALB_ID")),
        ResultType::Artists => string(value.get("ART_ID")),
        ResultType::Playlists => string(value.get("PLAYLIST_ID")),
        ResultType::All | ResultType::Tracks => String::new(),
    }
}

fn wrapper_artwork(item: &Value, data: &Value) -> String {
    [
        item.get("cover"),
        item.get("pictures"),
        item.get("image_linked_item"),
        data.get("PICTURE_XL"),
        data.get("PICTURE_BIG"),
        data.get("PICTURE_MEDIUM"),
        data.get("PICTURE"),
        data.get("ALB_PICTURE"),
        data.get("ART_PICTURE"),
        data.get("PLAYLIST_PICTURE"),
    ]
    .into_iter()
    .find_map(image_value)
    .unwrap_or_default()
}

fn image_value(value: Option<&Value>) -> Option<String> {
    let value = value?;
    match value {
        Value::Array(items) => items.iter().find_map(|item| image_value(Some(item))),
        Value::Object(object) => {
            if let Some(url) = object.get("url").and_then(Value::as_str) {
                return nonempty(normalize::safe_artwork(url));
            }
            let hash = object
                .get("md5")
                .or_else(|| object.get("MD5"))
                .and_then(Value::as_str)?;
            let kind = object
                .get("type")
                .or_else(|| object.get("TYPE"))
                .and_then(Value::as_str)
                .unwrap_or("cover");
            deezer_image(hash, kind)
        }
        Value::String(value) => {
            if value.contains('/') {
                nonempty(normalize::safe_artwork(value))
            } else {
                deezer_image(value, "cover")
            }
        }
        _ => None,
    }
}

fn deezer_image(hash: &str, kind: &str) -> Option<String> {
    let hash = hash.trim();
    if hash.is_empty()
        || !hash
            .bytes()
            .all(|byte| byte.is_ascii_alphanumeric() || byte == b'-')
    {
        return None;
    }
    let mut kind = match kind.to_ascii_lowercase().as_str() {
        "cover" | "playlist" | "artist" => kind.to_ascii_lowercase(),
        _ => return None,
    };
    if kind == "playlist" && hash.contains('-') {
        kind = "cover".to_owned();
    }
    let url = format!("https://e-cdns-images.dzcdn.net/images/{kind}/{hash}/500x500.jpg");
    nonempty(normalize::safe_artwork(&url))
}

fn year(value: &str) -> String {
    value
        .get(..4)
        .filter(|year| year.bytes().all(|byte| byte.is_ascii_digit()))
        .unwrap_or_default()
        .to_owned()
}

fn valid_deezer_id(value: &str) -> bool {
    !value.trim().is_empty() && value.bytes().all(|byte| byte.is_ascii_digit())
}

fn stable_part(value: &str) -> String {
    value
        .trim()
        .chars()
        .map(|character| {
            if character.is_ascii_alphanumeric() {
                character.to_ascii_lowercase()
            } else {
                '-'
            }
        })
        .collect()
}

fn string(value: Option<&Value>) -> String {
    value
        .and_then(|value| match value {
            Value::String(value) => Some(value.clone()),
            Value::Number(value) => Some(value.to_string()),
            _ => None,
        })
        .unwrap_or_default()
}

fn first_string(values: &[String]) -> Option<String> {
    values.iter().find_map(|value| nonempty(value.clone()))
}

fn nonempty(value: String) -> Option<String> {
    (!value.trim().is_empty()).then_some(value)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn flow_items_drop_a_subtitle_that_mirrors_the_title() {
        let mirrored = json!({
            "type": "flow",
            "title": "Dance",
            "subtitle": "Dance",
            "data": { "id": "genre-danceedm" }
        });
        let item = parse_presentation_card(&mirrored, &mirrored["data"], "flow").unwrap();
        assert_eq!(item.card.title, "Dance");
        assert_eq!(item.card.subtitle, "");
        assert!(matches!(
            item.action,
            DiscoverAction::PlayDeezerFlow { smart_mix: false }
        ));

        let smart_mix = json!({
            "type": "smarttracklist",
            "title": "Daily Mix",
            "subtitle": "Daily Mix",
            "data": { "SMARTTRACKLIST_ID": "mix-1" }
        });
        let item =
            parse_presentation_card(&smart_mix, &smart_mix["data"], "smarttracklist").unwrap();
        assert_eq!(item.card.subtitle, "");

        // A genuinely different subtitle, such as the artist behind a smart
        // mix, is still information and stays visible.
        let artist_subtitle = json!({
            "type": "smarttracklist",
            "title": "Daily Mix 1",
            "subtitle": "Justice",
            "data": { "SMARTTRACKLIST_ID": "mix-2" }
        });
        let item =
            parse_presentation_card(&artist_subtitle, &artist_subtitle["data"], "smarttracklist")
                .unwrap();
        assert_eq!(item.card.subtitle, "Justice");
    }
    #[test]
    fn request_matches_captured_get_contract_without_exposing_token() {
        let url = home_request_url("check-form-sentinel").unwrap();
        assert_eq!(url.scheme(), "https");
        assert_eq!(url.host_str(), Some("www.deezer.com"));
        let query = url.query_pairs().collect::<Vec<_>>();
        assert!(query.contains(&("method".into(), "page.get".into())));
        assert!(query.contains(&("input".into(), "3".into())));
        assert!(query.contains(&("api_version".into(), "1.0".into())));
        assert!(query.contains(&("api_token".into(), "check-form-sentinel".into())));
        let gateway = query
            .iter()
            .find(|(key, _)| key == "gateway_input")
            .map(|(_, value)| serde_json::from_str::<Value>(value).unwrap())
            .unwrap();
        assert_eq!(gateway["PAGE"], "home");
        assert_eq!(gateway["VERSION"], APP_VERSION);
        assert_eq!(gateway["LANG"], DISCOVER_LANGUAGE);
        assert_eq!(DISCOVER_LANGUAGE, "en");
        assert_eq!(gateway["OPTIONS"][0], "deeplink_newsandentertainment");
        assert_eq!(gateway["OPTIONS"][1], "deeplink_subscribeoffer");
        assert!(
            gateway["SUPPORT"]["horizontal-grid"]
                .as_array()
                .unwrap()
                .iter()
                .any(|item| item == "album")
        );
        let channel = channel_request_url("check-form-sentinel", "dance").unwrap();
        let channel_gateway = channel
            .query_pairs()
            .find(|(key, _)| key == "gateway_input")
            .map(|(_, value)| serde_json::from_str::<Value>(&value).unwrap())
            .unwrap();
        assert_eq!(channel_gateway["PAGE"], "channels/dance");
        assert!(channel_request_url("check-form-sentinel", "Dance").is_err());
    }

    #[test]
    fn parser_preserves_order_and_excludes_recent_sections() {
        let value = json!({
            "results": { "sections": [
                { "title": "Continue streaming", "items": [{"type":"artist","id":"1"}] },
                { "title": "Useful", "items": [
                    { "type": "album", "id": "wrapper-id", "title": "Wrapper title", "subtitle": "Wrapper artist", "pictures": [{"md5":"abc123","type":"cover"}], "data": {"ALB_ID":"42","ALB_TITLE":"Raw title","ART_NAME":"Raw artist","ORIGINAL_RELEASE_DATE":"2025-04-06","ALB_PICTURE":"raw"} },
                    { "type": "flow", "id": "flow-1", "title": "Flow title", "data": {"id":"flow-1","title":"Raw flow"} },
                    { "type": "smarttracklist", "id": "mix-1", "title": "My top August", "data": {"ID":"mix-1","TITLE":"Daily"} },
                    { "type": "channel", "id": "dance", "title": "Dance", "data": {"id":"dance","title":"Raw dance"} },
                    { "type": "track", "id": "track-1", "title": "Track title", "subtitle": "Artist", "data": {"SNG_ID":"99","SNG_TITLE":"Raw track","ART_NAME":"Raw artist"} },
                    { "type": "playlist", "id": "playlist-wrapper", "title": "Playlist", "data": {"PLAYLIST_ID":"77","TITLE":"Raw playlist","NB_SONG":3,"PLAYLIST_PICTURE":"pl"} }
                ] },
                { "title": "Recently you've been loving...", "items": [{"type":"album","id":"9","data":{"ALB_ID":"9"}}] }
            ] }
        });
        let sections = parse_home(&value).unwrap();
        assert_eq!(sections.len(), 1);
        assert_eq!(sections[0].title, "Useful");
        assert_eq!(sections[0].items.len(), 6);
        assert_eq!(sections[0].items[0].card.title, "Wrapper title");
        assert_eq!(sections[0].items[0].card.subtitle, "Wrapper artist");
        assert_eq!(sections[0].items[0].card.badge, "2025");
        assert_eq!(sections[0].subtitle, "");
        assert_eq!(sections[0].items[0].action, DiscoverAction::OpenDetail);
        assert_eq!(
            sections[0].items[1].action,
            DiscoverAction::PlayDeezerFlow { smart_mix: false }
        );
        assert_eq!(
            sections[0].items[2].action,
            DiscoverAction::PlayDeezerFlow { smart_mix: true }
        );
        assert_eq!(sections[0].items[2].card.title, "My top August");
        assert_eq!(sections[0].items[3].action, DiscoverAction::None);
        assert_eq!(
            sections[0].items[4].action,
            DiscoverAction::PlayDeezerTrack("99".into())
        );
        assert_eq!(sections[0].items[5].action, DiscoverAction::OpenDetail);
    }

    #[test]
    fn malformed_collection_ids_are_rejected_but_flow_ids_remain_actionable() {
        let value = json!({
            "results": { "sections": [{
                "title": "Useful",
                "items": [
                    {"type":"album","id":"bad","data":{"ALB_ID":"not-numeric"}},
                    {"type":"flow","id":"flow-id","title":"Flow","data":{"id":"flow-id"}}
                ]
            }] }
        });
        let sections = parse_home(&value).unwrap();
        assert_eq!(sections[0].items.len(), 1);
        assert_eq!(
            sections[0].items[0].action,
            DiscoverAction::PlayDeezerFlow { smart_mix: false }
        );
    }

    #[test]
    fn smarttracklist_generic_daily_title_falls_through_to_composition() {
        let value = json!({
            "results": { "sections": [{
                "title": "Made for you",
                "items": [{
                    "type": "smarttracklist",
                    "id": "inspired-by-1",
                    "title": "daily",
                    "cover_title": "ELECTRO DANCE",
                    "cover_composition": { "title": "Electro Dance" },
                    "data": {
                        "SMARTTRACKLIST_ID": "inspired-by-1",
                        "TITLE": "daily",
                        "COVER_COMPOSITION": { "TITLE": "Electro Dance" }
                    }
                }]
            }] }
        });

        let sections = parse_home(&value).unwrap();

        assert_eq!(sections[0].items[0].card.title, "Electro Dance");
    }

    #[test]
    fn parser_preserves_captured_inspired_by_titles() {
        let value = json!({
            "results": { "sections": [{
                "title": "Made for you",
                "items": [
                    {
                        "type": "smarttracklist",
                        "id": "inspired-by-1",
                        "title": "Pop Rap",
                        "cover_title": "POP RAP",
                        "data": {
                            "SMARTTRACKLIST_ID": "inspired-by-1",
                            "CONFIGURATION_ID": "inspired-by-1",
                            "TITLE": "Pop Rap"
                        }
                    },
                    {
                        "type": "smarttracklist",
                        "id": "inspired-by-2",
                        "title": "Electro Dance",
                        "cover_title": "ELECTRO DANCE",
                        "data": {
                            "SMARTTRACKLIST_ID": "inspired-by-2",
                            "CONFIGURATION_ID": "inspired-by-2",
                            "TITLE": "Electro Dance"
                        }
                    }
                ]
            }] }
        });

        let sections = parse_home(&value).unwrap();
        let titles = sections[0]
            .items
            .iter()
            .map(|item| item.card.title.as_str())
            .collect::<Vec<_>>();

        assert_eq!(titles, ["Pop Rap", "Electro Dance"]);
    }

    #[test]
    fn smarttracklist_ordinary_title_beats_uppercase_cover_titles() {
        let item = json!({
            "type": "smarttracklist",
            "id": "wrapper-id",
            "title": "New releases",
            "cover_title": "NUOVE USCITE",
            "cover_composition": { "title": "NUOVE USCITE" },
            "data": {
                "CONFIGURATION_ID": "new-releases",
                "TITLE": "daily"
            }
        });

        let parsed = parse_item(&item).unwrap();

        assert_eq!(parsed.card.id, "new-releases");
        assert_eq!(parsed.card.title, "New releases");
    }

    #[test]
    fn smarttracklist_id_uses_configuration_when_smart_id_is_missing() {
        let item = json!({
            "type": "smarttracklist",
            "id": "wrapper-id",
            "title": "Electro Dance",
            "data": {
                "CONFIGURATION_ID": "inspired-by-4",
                "TITLE": "Electro Dance"
            }
        });

        assert_eq!(parse_item(&item).unwrap().card.id, "inspired-by-4");
    }

    #[test]
    fn smarttracklist_ids_reject_empty_control_and_overlong_values() {
        assert!(valid_smart_mix_id("").is_none());
        assert!(valid_smart_mix_id("bad\nvalue").is_none());
        assert!(valid_smart_mix_id(&"a".repeat(129)).is_none());
        assert_eq!(
            valid_smart_mix_id(" inspired-by-1 "),
            Some("inspired-by-1".into())
        );
    }

    #[test]
    fn smarttracklist_enrichment_request_and_response_preserve_gateway_contract() {
        let url = smart_tracklist_request_url("check-form-sentinel").unwrap();
        let query = url.query_pairs().collect::<Vec<_>>();
        assert!(query.contains(&("method".into(), "deezer.pageSmartTracklist".into())));
        assert_eq!(
            smart_tracklist_request("inspired-by-3"),
            json!({ "smarttracklist_id": "inspired-by-3", "lang": "us" })
        );
        let data = json!({
            "SMARTTRACKLIST_ID": "inspired-by-3",
            "CONFIGURATION_ID": "inspired-by-3",
            "TITLE": "daily",
            "AUTO_GENERATED_TITLE": "daily 1 - 9/9/26",
            "COVER_COMPOSITION": {"TITLE": "Riddim Dubstep"}
        });
        let data = data.as_object().unwrap();
        assert!(smart_tracklist_response_matches(data, "inspired-by-3"));
        assert_eq!(
            smart_tracklist_response_title(data).as_deref(),
            Some("Riddim Dubstep")
        );
    }

    #[test]
    fn smarttracklist_generic_candidates_do_not_hide_a_later_specific_title() {
        let value = json!({
            "results": { "sections": [{
                "title": "Made for you",
                "items": [{
                    "type": "smarttracklist",
                    "id": "inspired-by-1",
                    "title": "Mix",
                    "cover_title": "Flow",
                    "data": {
                        "SMARTTRACKLIST_ID": "inspired-by-1",
                        "TITLE": "daily",
                        "AUTO_GENERATED_TITLE": "Electro Dance - 9/8/26"
                    }
                }]
            }] }
        });

        let sections = parse_home(&value).unwrap();

        assert_eq!(sections[0].items[0].card.title, "Electro Dance");
    }

    #[test]
    fn smarttracklist_keeps_the_provider_config_id() {
        let item = json!({
            "type": "smarttracklist",
            "id": "wrapper-id",
            "title": "Daily",
            "data": {
                "SMARTTRACKLIST_ID": "monthly-top",
                "TITLE": "My top August"
            }
        });
        let parsed = parse_item(&item).unwrap();
        assert_eq!(parsed.card.id, "monthly-top");
        assert_eq!(
            parsed.action,
            DiscoverAction::PlayDeezerFlow { smart_mix: true }
        );
    }

    #[test]
    fn smarttracklist_generated_title_is_a_safe_fallback_without_its_date() {
        assert_eq!(
            generated_smart_mix_title("Electro Dance - 9/8/26").as_deref(),
            Some("Electro Dance")
        );
        assert_eq!(
            generated_smart_mix_title("R&B Vibes - 08.09.2026").as_deref(),
            Some("R&B Vibes")
        );
        assert_eq!(
            generated_smart_mix_title("Release Radar - Deluxe").as_deref(),
            Some("Release Radar - Deluxe")
        );
        assert!(generated_smart_mix_title("Daily - 9/8/26").is_none());
        assert!(generated_smart_mix_title("Daily 1 - 08/09/26").is_none());
        assert_eq!(specific_smart_mix_title("Daily Drive"), Some("Daily Drive"));
        assert!(specific_smart_mix_title(" Daily 12 ").is_none());
    }

    #[test]
    fn smarttracklist_placeholder_fields_fall_back_to_neutral_mix() {
        let item = json!({
            "type": "smarttracklist",
            "id": "inspired-by-1",
            "title": "Daily 1",
            "cover_title": " daily ",
            "cover_composition": {"title": "Daily 2"},
            "auto_generated_title": "Daily 1 - 08/09/26",
            "data": {
                "SMARTTRACKLIST_ID": "inspired-by-1",
                "TITLE": "DAILY 3",
                "AUTO_GENERATED_TITLE": "daily 1 - 08/09/26"
            }
        });
        let parsed = parse_item(&item).unwrap();
        assert_eq!(parsed.card.title, "Mix");
    }

    #[test]
    fn parser_keeps_deezer_section_subtitles() {
        let value = json!({
            "results": { "sections": [{
                "title": "Flow",
                "subtitle": "Personalized recommendations",
                "items": [{
                    "type": "flow",
                    "id": "flow-id",
                    "title": "Flow",
                    "data": {"id": "flow-id"}
                }]
            }] }
        });
        let sections = parse_home(&value).unwrap();
        assert_eq!(sections[0].subtitle, "Personalized recommendations");
    }

    #[test]
    fn channel_parser_uses_the_nested_page_title_and_openable_cards() {
        let value = json!({
            "results": {
                "title": "Dance & EDM",
                "sections": [{
                    "title": "Playlists",
                    "items": [{
                        "type": "playlist",
                        "id": "wrapper",
                        "title": "Dance essentials",
                        "target": "/playlist/77",
                        "data": {
                            "PLAYLIST_ID": "77",
                            "TITLE": "Dance essentials",
                            "NB_SONG": 12
                        }
                    }, {
                        "type": "album",
                        "id": "88",
                        "title": "Dance album",
                        "data": {"ALB_ID": "88", "ALB_TITLE": "Dance album"}
                    }]
                }]
            }
        });
        let (title, sections) = parse_channel(&value, "dance").unwrap();
        assert_eq!(title, "Dance & EDM");
        assert_eq!(sections.len(), 1);
        assert_eq!(sections[0].items.len(), 2);
        assert!(
            sections[0]
                .items
                .iter()
                .all(|item| item.action == DiscoverAction::OpenDetail)
        );
    }

    #[test]
    fn channel_items_use_a_validated_data_slug_or_target_fallback() {
        let data_slug = json!({
            "type": "channel",
            "id": "uuid",
            "title": "Dance",
            "data": {"id": "uuid", "slug": "dance"}
        });
        let target_slug = json!({
            "type": "channel",
            "id": "uuid",
            "title": "Dance",
            "target": "/channels/dance",
            "data": {"id": "uuid"}
        });
        let malformed = json!({
            "type": "channel",
            "id": "uuid",
            "title": "Dance",
            "target": "/channels/Dance",
            "data": {"id": "uuid", "slug": "bad/slug"}
        });
        assert_eq!(
            parse_item(&data_slug).unwrap().action,
            DiscoverAction::OpenDeezerChannel("dance".into())
        );
        assert_eq!(
            parse_item(&target_slug).unwrap().action,
            DiscoverAction::OpenDeezerChannel("dance".into())
        );
        assert_eq!(parse_item(&malformed).unwrap().action, DiscoverAction::None);
        assert!(valid_channel_slug("-dance").is_none());
        assert!(valid_channel_slug("dance-").is_none());
        assert!(valid_channel_slug("dance/edm").is_none());
        assert!(valid_channel_slug("dance").is_some());
    }

    #[test]
    fn composite_playlist_hashes_use_the_cover_cdn_kind() {
        assert_eq!(
            deezer_image("abc123-def456", "playlist"),
            Some("https://e-cdns-images.dzcdn.net/images/cover/abc123-def456/500x500.jpg".into())
        );
    }

    #[test]
    fn composite_wrapper_cover_wins_over_the_first_picture() {
        let item = json!({
            "cover": {"type": "artist", "md5": "composite-cover-hash"},
            "pictures": {"type": "cover", "md5": "first-picture-hash"}
        });
        assert_eq!(
            wrapper_artwork(&item, &Value::Null),
            "https://e-cdns-images.dzcdn.net/images/artist/composite-cover-hash/500x500.jpg"
        );
    }
}
