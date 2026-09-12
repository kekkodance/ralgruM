// Browser integration entry point for ralgrum://open links (v1).
// Parsing and pending link stash live here so main.rs stays a thin
// composition root. Tests sit with the module they verify.
use std::{
    fs,
    path::{Path, PathBuf},
    time::{SystemTime, UNIX_EPOCH},
};

use serde::Deserialize;

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub(crate) enum BrowserProvider {
    Deezer,
    SoundCloud,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub(crate) enum BrowserKind {
    Track,
    Album,
    Playlist,
    Artist,
}
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub(crate) enum BrowserAction {
    Play,
    Open,
}
#[derive(Clone, Debug, Eq, PartialEq)]
pub(crate) struct BrowserEntity {
    pub(crate) provider: BrowserProvider,
    pub(crate) kind: BrowserKind,
    pub(crate) id: String,
    pub(crate) url: String,
    pub(crate) action: BrowserAction,
    pub(crate) title: String,
}
/// File the startup path writes when launched through the browser extension.
/// The main view consumes and deletes it to route the entity. Kept as JSON
/// so future fields stay backward compatible with older readers.
pub(crate) const PENDING_FILE_NAME: &str = "browser-link-pending.json";
const PENDING_DIRECTORY_NAME: &str = "browser-link-queue";
const LEGACY_PENDING_DIRECTORY_NAME: &str = "browser-links-pending";

#[derive(Debug, Deserialize)]
struct PendingBrowserLink {
    raw_url: String,
}
fn decode_component(input: &str) -> String {
    // Percent escapes encode UTF-8 bytes, so decode into bytes first. Casting
    // every escaped byte directly to char corrupts non-ASCII titles and URLs.
    let input_bytes = input.as_bytes();
    let mut decoded = Vec::with_capacity(input.len());
    let mut i = 0;
    while i < input_bytes.len() {
        if input_bytes[i] == b'%' && i + 2 < input_bytes.len() {
            let hi = (input_bytes[i + 1] as char).to_digit(16);
            let lo = (input_bytes[i + 2] as char).to_digit(16);
            if let (Some(hi), Some(lo)) = (hi, lo) {
                decoded.push(((hi << 4) | lo) as u8);
                i += 3;
                continue;
            }
        }
        if input_bytes[i] == b'+' {
            decoded.push(b' ');
        } else {
            decoded.push(input_bytes[i]);
        }
        i += 1;
    }
    String::from_utf8_lossy(&decoded).into_owned()
}

fn truncate_utf8(value: &mut String, max_bytes: usize) {
    if value.len() <= max_bytes {
        return;
    }
    let mut boundary = max_bytes;
    while boundary > 0 && !value.is_char_boundary(boundary) {
        boundary -= 1;
    }
    value.truncate(boundary);
}
fn valid_deezer_id(id: &str) -> bool {
    if id.is_empty() || id.len() > 32 {
        return false;
    }
    let mut chars = id.chars();
    match chars.next() {
        Some(first) if first.is_ascii_digit() && first != '0' => {}
        _ => return false,
    }
    id.bytes().all(|b| b.is_ascii_digit())
}
fn parse_provider(value: &str) -> Option<BrowserProvider> {
    match value.trim().to_ascii_lowercase().as_str() {
        "deezer" => Some(BrowserProvider::Deezer),
        "soundcloud" => Some(BrowserProvider::SoundCloud),
        _ => None,
    }
}
fn parse_kind(value: &str) -> Option<BrowserKind> {
    match value.trim().to_ascii_lowercase().as_str() {
        "track" => Some(BrowserKind::Track),
        "album" => Some(BrowserKind::Album),
        "playlist" => Some(BrowserKind::Playlist),
        "artist" => Some(BrowserKind::Artist),
        _ => None,
    }
}
fn parse_action(value: Option<&str>, kind: BrowserKind) -> BrowserAction {
    match value.map(|v| v.trim().to_ascii_lowercase()) {
        Some(action) if action == "play" => BrowserAction::Play,
        Some(action) if action == "open" => BrowserAction::Open,
        _ => {
            if kind == BrowserKind::Track {
                BrowserAction::Play
            } else {
                BrowserAction::Open
            }
        }
    }
}
/// Share shortlinks such as link.deezer.com/s/<code> carry no numeric id.
/// The code is opaque, so the app follows the redirect natively (web pages
/// cannot, CORS forbids reading cross origin redirect targets) and parses
/// the canonical page it lands on.
pub(crate) fn is_deezer_short_url(raw: &str) -> bool {
    let Ok(url) = url::Url::parse(raw) else {
        return false;
    };
    if url.scheme() != "https" || !url.username().is_empty() || url.password().is_some() {
        return false;
    }
    let Some(host) = url.host_str().map(str::to_ascii_lowercase) else {
        return false;
    };
    if host != "link.deezer.com"
        && !host.ends_with(".link.deezer.com")
        && host != "deezer.page.link"
        && !host.ends_with(".deezer.page.link")
    {
        return false;
    }
    url.path_segments()
        .is_some_and(|mut segments| segments.any(|segment| !segment.trim().is_empty()))
}

/// SoundCloud share shortlinks (on.soundcloud.com/<code>) likewise need a
/// native redirect follow before the api-v2 resolve call.
pub(crate) fn is_soundcloud_short_url(raw: &str) -> bool {
    let Ok(url) = url::Url::parse(raw) else {
        return false;
    };
    if url.scheme() != "https" || !url.username().is_empty() || url.password().is_some() {
        return false;
    }
    let Some(host) = url.host_str().map(str::to_ascii_lowercase) else {
        return false;
    };
    if host != "on.soundcloud.com" && !host.ends_with(".on.soundcloud.com") {
        return false;
    }
    url.path_segments()
        .is_some_and(|mut segments| segments.any(|segment| !segment.trim().is_empty()))
}

pub(crate) const fn deezer_entity_slug(kind: BrowserKind) -> &'static str {
    match kind {
        BrowserKind::Track => "track",
        BrowserKind::Album => "album",
        BrowserKind::Playlist => "playlist",
        BrowserKind::Artist => "artist",
    }
}

fn deezer_kind_id_from_path(path: &str) -> Option<(BrowserKind, String)> {
    let segments = path
        .split('/')
        .filter(|segment| !segment.trim().is_empty())
        .collect::<Vec<_>>();
    // A leading locale segment (for example /it/track/11234004) is skipped
    // naturally because only kind names match below.
    segments.windows(2).find_map(|pair| {
        let kind = parse_kind(pair[0])?;
        if valid_deezer_id(pair[1]) {
            Some((kind, pair[1].to_owned()))
        } else {
            None
        }
    })
}

/// Parses a canonical Deezer page URL into its entity kind and numeric id.
/// Also understands the link.deezer.com gateway, which embeds the canonical
/// page in its dest/awf/gwf/iwf query params. Only Deezer hosts are accepted
/// so an open redirect can never point the lookup at another site.
pub(crate) fn parse_deezer_canonical_entity(raw: &str) -> Option<(BrowserKind, String)> {
    let url = url::Url::parse(raw).ok()?;
    if url.scheme() != "https" || !is_deezer_page_host(url.host_str()?) {
        return None;
    }
    if let Some(found) = deezer_kind_id_from_path(url.path()) {
        return Some(found);
    }
    for key in ["dest", "awf", "gwf", "iwf"] {
        let nested = url
            .query_pairs()
            .find(|(name, _)| name == key)
            .map(|(_, value)| value.into_owned());
        if let Some(nested) = nested
            && let Ok(nested) = url::Url::parse(&nested)
            && nested.scheme() == "https"
            && let Some(host) = nested.host_str()
            && is_deezer_page_host(host)
            && let Some(found) = deezer_kind_id_from_path(nested.path())
        {
            return Some(found);
        }
    }
    None
}

fn is_deezer_page_host(host: &str) -> bool {
    let host = host.to_ascii_lowercase();
    host == "deezer.com"
        || host.ends_with(".deezer.com")
        || host == "link.deezer.com"
        || host.ends_with(".link.deezer.com")
        || host == "deezer.page.link"
        || host.ends_with(".deezer.page.link")
}

fn valid_provider_url(provider: BrowserProvider, kind: BrowserKind, id: &str, raw: &str) -> bool {
    let Ok(url) = url::Url::parse(raw) else {
        return false;
    };
    if url.scheme() != "https" || !url.username().is_empty() || url.password().is_some() {
        return false;
    }
    let Some(host) = url.host_str().map(str::to_ascii_lowercase) else {
        return false;
    };
    match provider {
        BrowserProvider::Deezer => {
            if is_deezer_short_url(raw) {
                // Shortlinks resolve to the canonical page natively later.
                return true;
            }
            if host != "deezer.com" && !host.ends_with(".deezer.com") {
                return false;
            }
            let wanted_kind = match kind {
                BrowserKind::Track => "track",
                BrowserKind::Album => "album",
                BrowserKind::Playlist => "playlist",
                BrowserKind::Artist => "artist",
            };
            let segments = url
                .path_segments()
                .into_iter()
                .flatten()
                .collect::<Vec<_>>();
            segments
                .windows(2)
                .any(|pair| pair[0].eq_ignore_ascii_case(wanted_kind) && pair[1] == id)
        }
        BrowserProvider::SoundCloud => {
            (host == "soundcloud.com" || host.ends_with(".soundcloud.com"))
                && url
                    .path_segments()
                    .is_some_and(|mut segments| segments.any(|segment| !segment.trim().is_empty()))
        }
    }
}

/// Parses a ralgrum://open URL into a typed entity. Returns None for
/// anything outside the v1 allowlist so callers can ignore it safely.
pub(crate) fn parse_ralgrum_url(input: &str) -> Option<BrowserEntity> {
    let input = input.trim();
    if input.len() > 2048 {
        return None;
    }
    let lower = input.to_ascii_lowercase();
    if !lower.starts_with("ralgrum://open") && !lower.starts_with("ralgrum:open") {
        return None;
    }
    let query = input.splitn(2, '?').nth(1).unwrap_or("");
    let mut provider = None;
    let mut kind = None;
    let mut id = String::new();
    let mut url = String::new();
    let mut action_raw: Option<String> = None;
    let mut title = String::new();
    for pair in query.split('&') {
        if pair.is_empty() {
            continue;
        }
        let mut parts = pair.splitn(2, '=');
        let key = parts.next().unwrap_or("").trim().to_ascii_lowercase();
        let value = decode_component(parts.next().unwrap_or(""));
        match key.as_str() {
            "provider" => provider = parse_provider(&value),
            "type" => kind = parse_kind(&value),
            "id" => id = value.trim().to_owned(),
            "url" => url = value.trim().to_owned(),
            "action" => action_raw = Some(value),
            "title" => {
                let mut short = value.trim().to_owned();
                truncate_utf8(&mut short, 200);
                title = short;
            }
            _ => {}
        }
    }
    let provider = provider?;
    let kind = kind?;
    if provider == BrowserProvider::Deezer {
        let short = is_deezer_short_url(&url);
        if short && !id.is_empty() {
            // Shortlinks carry an opaque code in the URL, never a numeric id.
            return None;
        }
        if !short && !valid_deezer_id(&id) {
            return None;
        }
    } else if !id.is_empty() {
        // SoundCloud is URL addressed, ignore stray ids.
        id.clear();
    }
    if !valid_provider_url(provider, kind, &id, &url) {
        return None;
    }
    let action = parse_action(action_raw.as_deref(), kind);
    Some(BrowserEntity {
        provider,
        kind,
        id,
        url,
        action,
        title,
    })
}

/// Finds the first ralgrum:// arg in a CLI arg list (skips argv[0]).
/// Accepts a bare protocol URL or a --open-url= wrapper for shells that
/// need an explicit flag form.
pub(crate) fn find_browser_link_arg(args: &[String]) -> Option<String> {
    args.iter()
        .skip(1)
        .find(|arg| {
            let low = arg.to_ascii_lowercase();
            low.starts_with("ralgrum://")
                || low.starts_with("ralgrum:")
                || arg.starts_with("--open-url=")
        })
        .map(|arg| {
            arg.strip_prefix("--open-url=")
                .unwrap_or(arg)
                .trim_matches('"')
                .to_owned()
        })
}
fn pending_directory() -> Option<PathBuf> {
    crate::paths::config_dir().map(|directory| directory.join(PENDING_DIRECTORY_NAME))
}

fn pending_directories() -> Option<[PathBuf; 2]> {
    crate::paths::config_dir().map(|directory| {
        [
            directory.join(PENDING_DIRECTORY_NAME),
            directory.join(LEGACY_PENDING_DIRECTORY_NAME),
        ]
    })
}

fn queue_entry_name() -> String {
    let timestamp = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|duration| duration.as_nanos())
        .unwrap_or_default();
    format!(
        "{timestamp}-{}-{}.link",
        std::process::id(),
        uuid::Uuid::new_v4().simple()
    )
}

/// Validates and atomically queues one browser link. Each launch gets its own
/// file so a burst of extension clicks cannot overwrite an earlier action.
pub(crate) fn enqueue_pending_link(raw: &str) -> Option<(BrowserEntity, PathBuf)> {
    let entity = parse_ralgrum_url(raw)?;
    let directory = pending_directory()?;
    fs::create_dir_all(&directory).ok()?;
    let final_path = directory.join(queue_entry_name());
    let temporary_path = directory.join(format!(".{}.tmp", uuid::Uuid::new_v4()));
    let write_result = (|| {
        let mut file = fs::OpenOptions::new()
            .create_new(true)
            .write(true)
            .open(&temporary_path)
            .ok()?;
        std::io::Write::write_all(&mut file, raw.as_bytes()).ok()?;
        file.sync_all().ok()?;
        fs::rename(&temporary_path, &final_path).ok()
    })();
    if write_result.is_none() {
        let _ = fs::remove_file(&temporary_path);
        return None;
    }
    Some((entity, final_path))
}

fn consume_pending_file(path: &Path) -> Option<BrowserEntity> {
    let bytes = fs::read(path).ok();
    let _ = fs::remove_file(path);
    parse_pending_bytes(&bytes?)
}

fn parse_pending_bytes(bytes: &[u8]) -> Option<BrowserEntity> {
    if let Ok(raw) = std::str::from_utf8(bytes)
        && let Some(entity) = parse_ralgrum_url(raw)
    {
        return Some(entity);
    }
    let pending: PendingBrowserLink = serde_json::from_slice(bytes).ok()?;
    parse_ralgrum_url(&pending.raw_url)
}

/// Drains all queued links in filename order and removes malformed entries.
/// The old single pending file is consumed once for upgrade compatibility.
pub(crate) fn drain_pending_links() -> Vec<BrowserEntity> {
    let mut paths = pending_directories()
        .into_iter()
        .flatten()
        .filter_map(|directory| fs::read_dir(directory).ok())
        .flatten()
        .filter_map(Result::ok)
        .map(|entry| entry.path())
        .filter(|path| {
            path.extension()
                .is_some_and(|extension| extension == "link" || extension == "json")
        })
        .collect::<Vec<_>>();
    paths.sort();
    if let Some(legacy) = crate::paths::config_dir().map(|dir| dir.join(PENDING_FILE_NAME))
        && legacy.exists()
    {
        paths.insert(0, legacy);
    }
    paths
        .iter()
        .filter_map(|path| {
            if !fs::symlink_metadata(path)
                .map(|metadata| metadata.file_type().is_file())
                .unwrap_or(false)
            {
                let _ = fs::remove_file(path);
                return None;
            }
            consume_pending_file(path)
        })
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;
    use tempfile::tempdir;
    #[test]
    fn parses_deezer_track_with_play() {
        let entity = parse_ralgrum_url("ralgrum://open?provider=deezer&type=track&id=3135556&action=play&url=https%3A%2F%2Fwww.deezer.com%2Ftrack%2F3135556").unwrap();
        assert_eq!(entity.provider, BrowserProvider::Deezer);
        assert_eq!(entity.kind, BrowserKind::Track);
        assert_eq!(entity.id, "3135556");
        assert_eq!(entity.action, BrowserAction::Play);
    }

    #[test]
    fn percent_decoding_preserves_utf8_and_form_spaces() {
        let entity = parse_ralgrum_url("ralgrum://open?provider=deezer&type=track&id=1&title=Za%C5%BC%C3%B3%C5%82%C4%87+g%C4%99%C5%9Bl%C4%85&url=https%3A%2F%2Fwww.deezer.com%2Ftrack%2F1").unwrap();
        assert_eq!(entity.title, "Zażółć gęślą");
    }

    #[test]
    fn title_limit_stops_at_a_utf8_boundary() {
        let title = "é".repeat(101);
        let encoded = title
            .as_bytes()
            .iter()
            .map(|byte| format!("%{byte:02X}"))
            .collect::<String>();
        let raw = format!(
            "ralgrum://open?provider=deezer&type=track&id=1&title={encoded}&url=https%3A%2F%2Fwww.deezer.com%2Ftrack%2F1"
        );
        let entity = parse_ralgrum_url(&raw).unwrap();
        assert_eq!(entity.title.len(), 200);
        assert_eq!(entity.title.chars().count(), 100);
    }
    #[test]
    fn album_defaults_to_open_without_action() {
        let entity = parse_ralgrum_url("ralgrum://open?provider=deezer&type=album&id=302127&url=https%3A%2F%2Fwww.deezer.com%2Falbum%2F302127").unwrap();
        assert_eq!(entity.kind, BrowserKind::Album);
        assert_eq!(entity.action, BrowserAction::Open);
    }
    #[test]
    fn parses_soundcloud_track_from_permalink() {
        let entity = parse_ralgrum_url("ralgrum://open?provider=soundcloud&type=track&action=play&url=https%3A%2F%2Fsoundcloud.com%2Fartist%2Fslug").unwrap();
        assert_eq!(entity.provider, BrowserProvider::SoundCloud);
        assert!(entity.id.is_empty());
    }
    #[test]
    fn rejects_bad_provider_bad_id_and_http_url() {
        assert!(
            parse_ralgrum_url(
                "ralgrum://open?provider=other&type=track&id=1&url=https%3A%2F%2Fx.com"
            )
            .is_none()
        );
        assert!(parse_ralgrum_url("ralgrum://open?provider=deezer&type=track&id=abc&url=https%3A%2F%2Fwww.deezer.com%2Ftrack%2Fabc").is_none());
        assert!(
            parse_ralgrum_url(
                "ralgrum://open?provider=deezer&type=track&id=1&url=http%3A%2F%2Fevil.com"
            )
            .is_none()
        );
        assert!(parse_ralgrum_url("https://www.deezer.com/track/1").is_none());
        assert!(parse_ralgrum_url("ralgrum://open?provider=deezer&type=track&id=1&url=https%3A%2F%2Fexample.com%2Ftrack%2F1").is_none());
        assert!(parse_ralgrum_url("ralgrum://open?provider=deezer&type=track&id=1&url=https%3A%2F%2Fwww.deezer.com%2Ftrack%2F2").is_none());
        assert!(parse_ralgrum_url("ralgrum://open?provider=soundcloud&type=track&url=https%3A%2F%2Fexample.com%2Fartist%2Fsong").is_none());
    }
    #[test]
    fn finds_cli_arg_forms() {
        let args = vec![
            "ralgruM".to_owned(),
            "ralgrum://open?provider=deezer&type=track&id=1&url=https%3A%2F%2Fwww.deezer.com%2Ftrack%2F1".to_owned(),
        ];
        assert!(find_browser_link_arg(&args).is_some());
        let with_flag = vec![
            "ralgruM".to_owned(),
            "--open-url=ralgrum://open?provider=deezer&type=track&id=1&url=https%3A%2F%2Fwww.deezer.com%2Ftrack%2F1"
                .to_owned(),
        ];
        assert!(find_browser_link_arg(&with_flag).is_some());
        let bare: Vec<String> = vec!["ralgruM".to_owned()];
        assert!(find_browser_link_arg(&bare).is_none());
    }

    #[test]
    fn pending_entries_accept_raw_urls_and_legacy_json_only_after_revalidation() {
        let raw = "ralgrum://open?provider=deezer&type=track&id=1&url=https%3A%2F%2Fwww.deezer.com%2Ftrack%2F1";
        assert_eq!(parse_pending_bytes(raw.as_bytes()).unwrap().id, "1");
        let legacy = format!(r#"{{"raw_url":"{raw}","received_at":1}}"#);
        assert_eq!(parse_pending_bytes(legacy.as_bytes()).unwrap().id, "1");
        assert!(parse_pending_bytes(br#"{"raw_url":"https://evil"}"#).is_none());
    }

    #[test]
    fn queue_entry_names_are_app_generated_and_unique() {
        let first = queue_entry_name();
        let second = queue_entry_name();
        assert_ne!(first, second);
        assert!(first.ends_with(".link"));
        assert!(!first.contains(".."));
    }

    #[test]
    fn consuming_a_queue_entry_removes_it_even_when_invalid() {
        let directory = tempdir().unwrap();
        let valid = directory.path().join("valid.link");
        fs::write(
            &valid,
            "ralgrum://open?provider=deezer&type=album&id=42&url=https%3A%2F%2Fwww.deezer.com%2Falbum%2F42",
        )
        .unwrap();
        assert_eq!(consume_pending_file(&valid).unwrap().id, "42");
        assert!(!valid.exists());

        let invalid = directory.path().join("invalid.link");
        fs::write(&invalid, "not a ralgrum link").unwrap();
        assert!(consume_pending_file(&invalid).is_none());
        assert!(!invalid.exists());
    }

    #[test]
    fn parses_deezer_short_link_without_id() {
        let entity = parse_ralgrum_url("ralgrum://open?provider=deezer&type=track&action=open&url=https%3A%2F%2Flink.deezer.com%2Fs%2F34mrzDee5J0nOnvPzHHPH").unwrap();
        assert_eq!(entity.provider, BrowserProvider::Deezer);
        assert!(entity.id.is_empty());
    }

    #[test]
    fn rejects_deezer_short_link_with_stray_id_or_plain_http() {
        assert!(
            parse_ralgrum_url(
                "ralgrum://open?provider=deezer&type=track&id=1&url=https%3A%2F%2Flink.deezer.com%2Fs%2Fabc"
            )
            .is_none()
        );
        assert!(
            parse_ralgrum_url(
                "ralgrum://open?provider=deezer&type=track&url=http%3A%2F%2Flink.deezer.com%2Fs%2Fabc"
            )
            .is_none()
        );
        assert!(
            parse_ralgrum_url(
                "ralgrum://open?provider=deezer&type=track&url=https%3A%2F%2Flink.deezer.com%2F"
            )
            .is_none()
        );
    }

    #[test]
    fn parses_on_soundcloud_short_link() {
        let entity = parse_ralgrum_url(
            "ralgrum://open?provider=soundcloud&type=track&action=open&url=https%3A%2F%2Fon.soundcloud.com%2FAbC123",
        )
        .unwrap();
        assert_eq!(entity.provider, BrowserProvider::SoundCloud);
        assert!(entity.id.is_empty());
        assert!(is_soundcloud_short_url(&entity.url));
    }

    #[test]
    fn canonical_deezer_entity_supports_locales_queries_and_gateway_params() {
        let (kind, id) =
            parse_deezer_canonical_entity("https://www.deezer.com/track/11234004").unwrap();
        assert_eq!(kind, BrowserKind::Track);
        assert_eq!(id, "11234004");

        let (kind, id) = parse_deezer_canonical_entity(
            "https://www.deezer.com/it/track/11234004?host=1&utm_source=x",
        )
        .unwrap();
        assert_eq!(kind, BrowserKind::Track);
        assert_eq!(id, "11234004");

        let gateway = "https://link.deezer.com/?awf=https%3A%2F%2Fwww.deezer.com%2Ftrack%2F11234004&dest=https%3A%2F%2Fwww.deezer.com%2Falbum%2F302127";
        let (kind, id) = parse_deezer_canonical_entity(gateway).unwrap();
        assert_eq!(kind, BrowserKind::Album);
        assert_eq!(id, "302127");

        assert!(parse_deezer_canonical_entity("https://www.deezer.com/search?q=test").is_none());
        assert!(parse_deezer_canonical_entity("https://example.com/track/1").is_none());
        assert!(parse_deezer_canonical_entity("http://www.deezer.com/track/1").is_none());
        assert!(
            parse_deezer_canonical_entity(
                "https://link.deezer.com/?dest=https%3A%2F%2Fexample.com%2Ftrack%2F1"
            )
            .is_none()
        );
    }
}
