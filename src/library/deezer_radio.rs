use super::{
    client::{DeezerSession, LibraryClient, confirm_true, valid_deezer_id},
    model::{
        Card, Category, FlowCatalog, FlowCatalogMembership, FlowCatalogOption, Track, value_string,
    },
    normalize,
};
use crate::search::{DeezerArl, Provider};
use crate::smart_mix_title::{generated_smart_mix_title, specific_smart_mix_title};
use serde_json::{Value, json};

pub(super) const DEEZER_SMART_TRACKLIST_OPERATION: &str = "deezer.pageSmartTracklist";

#[derive(Clone, Copy, Debug, Default, Eq, PartialEq)]
pub(crate) enum FlowMode {
    #[default]
    Default,
    Discovery,
}

impl FlowMode {
    pub(crate) const fn api_value(self) -> &'static str {
        match self {
            Self::Default => "default",
            Self::Discovery => "discovery",
        }
    }
}

/// An opaque tuner value returned by Deezer and supplied to the next Flow call.
#[derive(Clone, Debug, Eq, PartialEq)]
pub(crate) struct FlowTuner(String);

impl FlowTuner {
    pub(crate) fn initial(mode: FlowMode) -> Self {
        Self(mode.api_value().into())
    }

    pub(crate) fn as_str(&self) -> &str {
        &self.0
    }

    fn from_response(value: Option<&Value>, fallback: Self) -> Self {
        value
            .and_then(Value::as_str)
            .map(str::trim)
            .filter(|value| !value.is_empty())
            .map(|value| Self(value.to_owned()))
            .unwrap_or(fallback)
    }
}

#[derive(Clone, Debug, Default)]
pub(crate) struct DeezerRadioBatch {
    pub(crate) tracks: Vec<Track>,
    pub(crate) total: usize,
    pub(crate) clear_remaining_tracks: bool,
    pub(crate) next_flow_tuner: Option<FlowTuner>,
}

#[derive(Clone, Debug, Default)]
pub(crate) struct DeezerSmartTracklist {
    pub(crate) tracks: Vec<Track>,
    pub(crate) total: usize,
    pub(crate) title: String,
    pub(crate) resolved_smart_mix_title: Option<String>,
    pub(crate) subtitle: String,
    pub(crate) description: String,
    pub(crate) artwork: String,
}

#[derive(Clone, Debug, Default)]
pub(crate) struct DeezerRelatedArtists {
    pub(crate) artists: Vec<Card>,
    pub(crate) total: usize,
}

#[derive(Clone, Copy, Debug, Eq, Hash, PartialEq)]
pub(crate) enum DeezerFeedbackKind {
    Song,
    Artist,
}

impl DeezerFeedbackKind {
    const fn api_value(self) -> &'static str {
        match self {
            Self::Song => "song",
            Self::Artist => "artist",
        }
    }
}

impl LibraryClient {
    pub(crate) async fn load_flow_radio(
        &self,
        config_id: &str,
        tuner: FlowTuner,
        arl: DeezerArl,
        saved_user_id: Option<String>,
    ) -> Result<DeezerRadioBatch, String> {
        let config_id = valid_flow_config_id(config_id)?;
        let (session, user_id) = self.bootstrap(arl, saved_user_id).await?;
        self.load_flow_radio_with_session(&config_id, tuner, &user_id, session)
            .await
    }

    pub(crate) async fn load_smart_tracklist(
        &self,
        smarttracklist_id: &str,
        arl: DeezerArl,
        saved_user_id: Option<String>,
    ) -> Result<DeezerSmartTracklist, String> {
        let smarttracklist_id = valid_flow_config_id(smarttracklist_id)?;
        let session = self.bootstrap(arl, saved_user_id).await?.0;
        let results = self
            .gateway_call(
                DEEZER_SMART_TRACKLIST_OPERATION,
                smart_tracklist_request(&smarttracklist_id),
                &session.token,
                session.cookie,
            )
            .await?;
        parse_smart_tracklist(&results, &smarttracklist_id)
    }

    pub(crate) async fn load_track_mix(
        &self,
        track_id: &str,
        arl: DeezerArl,
        saved_user_id: Option<String>,
    ) -> Result<DeezerRadioBatch, String> {
        let track_id = valid_deezer_id(track_id)?;
        let session = self.bootstrap(arl, saved_user_id).await?.0;
        self.load_radio_operation(
            "song.getSearchTrackMix",
            track_mix_request(&track_id),
            session,
        )
        .await
    }

    pub(crate) async fn load_artist_mix(
        &self,
        artist_id: &str,
        arl: DeezerArl,
        saved_user_id: Option<String>,
    ) -> Result<DeezerRadioBatch, String> {
        let artist_id = valid_deezer_id(artist_id)?;
        let session = self.bootstrap(arl, saved_user_id).await?.0;
        self.load_radio_operation(
            "smart.getSmartRadio",
            artist_mix_request(&artist_id),
            session,
        )
        .await
    }

    pub(crate) async fn load_similar_artists(
        &self,
        artist_id: &str,
        arl: DeezerArl,
        saved_user_id: Option<String>,
    ) -> Result<DeezerRelatedArtists, String> {
        let artist_id = valid_deezer_id(artist_id)?;
        let session = self.bootstrap(arl, saved_user_id).await?.0;
        let results = self
            .gateway_call(
                "artist.getRelated",
                similar_artists_request(&artist_id),
                &session.token,
                session.cookie,
            )
            .await?;
        Ok(parse_related_artists(&results))
    }

    pub(crate) async fn add_negative_feedback(
        &self,
        kind: DeezerFeedbackKind,
        id: &str,
        arl: DeezerArl,
        saved_user_id: Option<String>,
    ) -> Result<(), String> {
        let id = valid_deezer_id(id)?;
        let session = self.bootstrap(arl, saved_user_id).await?.0;
        let result = self
            .gateway_call(
                "favorite_dislike.add",
                negative_feedback_request(kind, &id),
                &session.token,
                session.cookie,
            )
            .await?;
        confirm_true(result, "favorite_dislike.add")
    }

    pub(super) async fn load_flow_radio_with_session(
        &self,
        config_id: &str,
        tuner: FlowTuner,
        user_id: &str,
        session: DeezerSession,
    ) -> Result<DeezerRadioBatch, String> {
        let request_tuner = tuner.clone();
        let results = self
            .gateway_call(
                "radio.getUserRadio",
                flow_radio_request(user_id, config_id, &tuner),
                &session.token,
                session.cookie.clone(),
            )
            .await?;
        let mut batch = self.radio_batch(&results, &session).await?;
        batch.next_flow_tuner = Some(FlowTuner::from_response(
            results.get("tuner"),
            request_tuner,
        ));
        Ok(batch)
    }

    async fn load_radio_operation(
        &self,
        operation: &str,
        body: Value,
        session: DeezerSession,
    ) -> Result<DeezerRadioBatch, String> {
        let results = self
            .gateway_call(operation, body, &session.token, session.cookie.clone())
            .await?;
        self.radio_batch(&results, &session).await
    }

    async fn radio_batch(
        &self,
        results: &Value,
        session: &DeezerSession,
    ) -> Result<DeezerRadioBatch, String> {
        let items = result_items(results);
        let tracks = self.hydrate_deezer_tracks(items, session).await?;
        Ok(DeezerRadioBatch {
            total: result_total(results, tracks.len()),
            clear_remaining_tracks: bool_value(results.get("clearRemainingTracks")),
            tracks,
            next_flow_tuner: None,
        })
    }

    pub(super) async fn hydrate_deezer_tracks(
        &self,
        items: Vec<Value>,
        session: &DeezerSession,
    ) -> Result<Vec<Track>, String> {
        let ids = items
            .iter()
            .filter_map(|item| {
                let id = value_string(item.get("SNG_ID"));
                (!id.is_empty()).then_some(id)
            })
            .collect::<Vec<_>>();
        let hydrated = if ids.is_empty() {
            Vec::new()
        } else {
            self.gateway_call(
                "song.getListData",
                json!({ "sng_ids": ids }),
                &session.token,
                session.cookie.clone(),
            )
            .await?
            .get("data")
            .and_then(Value::as_array)
            .cloned()
            .unwrap_or_default()
        };
        Ok(merge_and_normalize_tracks(items, &hydrated))
    }
}

fn flow_radio_request(user_id: &str, config_id: &str, tuner: &FlowTuner) -> Value {
    json!({
        "user_id": user_id,
        "config_id": config_id,
        "tuner": tuner.as_str(),
    })
}

fn smart_tracklist_request(smarttracklist_id: &str) -> Value {
    json!({ "smarttracklist_id": smarttracklist_id, "lang": "us" })
}

fn track_mix_request(track_id: &str) -> Value {
    json!({ "sng_id": track_id, "start_with_input_track": false })
}

fn artist_mix_request(artist_id: &str) -> Value {
    json!({ "art_id": artist_id })
}

fn parse_smart_tracklist(
    results: &Value,
    requested_id: &str,
) -> Result<DeezerSmartTracklist, String> {
    let data = results
        .get("DATA")
        .and_then(Value::as_object)
        .ok_or_else(|| "Deezer smart tracklist response is missing DATA".to_string())?;
    let smart_id = value_string(data.get("SMARTTRACKLIST_ID"));
    let configuration_id = value_string(data.get("CONFIGURATION_ID"));
    if smart_id.trim().is_empty() && configuration_id.trim().is_empty() {
        return Err("Deezer smart tracklist response is missing its stable id".into());
    }
    let requested_id = requested_id.trim();
    if (!smart_id.trim().is_empty() && smart_id.trim() != requested_id)
        || (!configuration_id.trim().is_empty() && configuration_id.trim() != requested_id)
    {
        return Err("Deezer smart tracklist response returned a mismatched stable id".into());
    }

    let songs = results
        .get("SONGS")
        .and_then(Value::as_object)
        .ok_or_else(|| "Deezer smart tracklist response is missing SONGS".to_string())?;
    let items = songs
        .get("data")
        .and_then(Value::as_array)
        .ok_or_else(|| "Deezer smart tracklist response is missing its songs".to_string())?;
    if items.is_empty() {
        return Err("Deezer smart tracklist returned no songs".into());
    }

    let tracks = items
        .iter()
        .filter(|item| valid_deezer_id(&value_string(item.get("SNG_ID"))).is_ok())
        .map(normalize::track)
        .filter(|track| !track.id.trim().is_empty())
        .collect::<Vec<_>>();
    if tracks.is_empty() {
        return Err("Deezer smart tracklist returned no usable songs".into());
    }
    let total = songs
        .get("total")
        .and_then(|value| {
            value
                .as_u64()
                .or_else(|| value.as_str()?.parse::<u64>().ok())
        })
        .and_then(|value| usize::try_from(value).ok())
        .unwrap_or(tracks.len());

    let resolved_smart_mix_title = first_smart_tracklist_title(data);
    Ok(DeezerSmartTracklist {
        total,
        title: resolved_smart_mix_title.clone().unwrap_or_default(),
        resolved_smart_mix_title,
        subtitle: first_metadata_text(data, &["SUBTITLE"]),
        description: first_metadata_text(data, &["DESCRIPTION"]),
        artwork: smart_tracklist_artwork(data),
        tracks,
    })
}

fn first_smart_tracklist_title(data: &serde_json::Map<String, Value>) -> Option<String> {
    [
        data.get("COVER_COMPOSITION")
            .and_then(|value| value.get("TITLE")),
        data.get("COVER_COMPOSITION")
            .and_then(|value| value.get("title")),
    ]
    .into_iter()
    .map(|value| value_string(value))
    .find_map(|value| specific_smart_mix_title(&value).map(str::to_owned))
    .or_else(|| specific_smart_mix_title(&value_string(data.get("TITLE"))).map(str::to_owned))
    .or_else(|| generated_smart_mix_title(&value_string(data.get("AUTO_GENERATED_TITLE"))))
}

fn first_metadata_text(data: &serde_json::Map<String, Value>, keys: &[&str]) -> String {
    keys.iter()
        .map(|key| value_string(data.get(*key)))
        .find(|value| !value.trim().is_empty())
        .unwrap_or_default()
}

fn smart_tracklist_artwork(data: &serde_json::Map<String, Value>) -> String {
    [data.get("COVER"), data.get("PICTURES")]
        .into_iter()
        .find_map(smart_tracklist_image)
        .unwrap_or_default()
}

fn smart_tracklist_image(value: Option<&Value>) -> Option<String> {
    match value? {
        Value::Array(values) => values
            .iter()
            .find_map(|value| smart_tracklist_image(Some(value))),
        Value::Object(object) => {
            let hash = value_string(object.get("MD5").or_else(|| object.get("md5")));
            let kind = value_string(object.get("TYPE").or_else(|| object.get("type")));
            smart_tracklist_image_url(&hash, &kind)
        }
        _ => None,
    }
}

fn smart_tracklist_image_url(hash: &str, kind: &str) -> Option<String> {
    let hash = hash.trim();
    let kind = kind.trim().to_ascii_lowercase();
    if hash.is_empty()
        || !hash
            .bytes()
            .all(|byte| byte.is_ascii_alphanumeric() || byte == b'-')
        || !matches!(kind.as_str(), "artist" | "cover" | "playlist")
    {
        return None;
    }
    Some(format!(
        "https://e-cdns-images.dzcdn.net/images/{kind}/{hash}/500x500.jpg"
    ))
}

fn similar_artists_request(artist_id: &str) -> Value {
    json!({ "id": artist_id, "nb": 10_000, "start": 0 })
}

fn negative_feedback_request(kind: DeezerFeedbackKind, id: &str) -> Value {
    json!({ "ID": id, "TYPE": kind.api_value() })
}

pub(super) fn result_items(results: &Value) -> Vec<Value> {
    results
        .get("data")
        .and_then(Value::as_array)
        .cloned()
        .unwrap_or_default()
}

pub(super) fn result_total(results: &Value, fallback: usize) -> usize {
    results
        .get("total")
        .and_then(|value| {
            value
                .as_u64()
                .or_else(|| value.as_str()?.parse::<u64>().ok())
        })
        .map(|value| value as usize)
        .unwrap_or(fallback)
}

fn bool_value(value: Option<&Value>) -> bool {
    value.is_some_and(|value| {
        value.as_bool() == Some(true)
            || value.as_u64().is_some_and(|value| value != 0)
            || value.as_str().is_some_and(|value| value == "1")
    })
}

fn merge_and_normalize_tracks(items: Vec<Value>, hydrated: &[Value]) -> Vec<Track> {
    items
        .into_iter()
        .map(|mut item| {
            let id = value_string(item.get("SNG_ID"));
            if let Some(Value::Object(source)) = hydrated
                .iter()
                .find(|track| value_string(track.get("SNG_ID")) == id)
                && let Value::Object(target) = &mut item
            {
                target.extend(source.clone());
            }
            normalize::track(&item)
        })
        .collect()
}

fn parse_related_artists(results: &Value) -> DeezerRelatedArtists {
    let mut artists = result_items(results)
        .iter()
        .map(|value| normalize::card(Category::Artists, value))
        .collect::<Vec<_>>();
    for artist in &mut artists {
        artist.source = Provider::Deezer;
    }
    DeezerRelatedArtists {
        total: result_total(results, artists.len()),
        artists,
    }
}

pub(super) fn parse_flow_catalog(section: &Value) -> Option<FlowCatalog> {
    let filter = section.get("filter")?;
    let options = filter
        .get("options")
        .and_then(Value::as_array)
        .into_iter()
        .flatten()
        .filter_map(|option| {
            let id = value_string(option.get("id"));
            (!id.is_empty()).then(|| FlowCatalogOption {
                id,
                label: value_string(option.get("label")),
            })
        })
        .collect::<Vec<_>>();
    if options.is_empty() {
        return None;
    }
    let memberships = section
        .get("items")
        .and_then(Value::as_array)
        .into_iter()
        .flatten()
        .filter(|item| item.get("type").and_then(Value::as_str) == Some("flow"))
        .filter_map(|item| {
            let config_id = value_string(item.pointer("/data/id"));
            if config_id.is_empty() {
                return None;
            }
            let option_ids = item
                .get("filter_option_ids")
                .and_then(Value::as_array)
                .into_iter()
                .flatten()
                .filter_map(Value::as_str)
                .map(str::trim)
                .filter(|id| !id.is_empty())
                .map(str::to_owned)
                .collect();
            Some(FlowCatalogMembership {
                config_id,
                option_ids,
            })
        })
        .collect();
    Some(FlowCatalog {
        default_option_id: value_string(filter.get("default_option_id")),
        options,
        memberships,
    })
}

fn valid_flow_config_id(value: &str) -> Result<String, String> {
    let value = value.trim();
    if value.is_empty()
        || value.len() > 128
        || value.chars().any(|character| character.is_control())
    {
        return Err("Deezer Flow configuration is invalid".into());
    }
    Ok(value.to_owned())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn captured_radio_request_shapes_are_preserved() {
        assert_eq!(
            flow_radio_request("42", "CONFIG_1", &FlowTuner::initial(FlowMode::Discovery)),
            json!({ "user_id": "42", "config_id": "CONFIG_1", "tuner": "discovery" })
        );
        assert_eq!(
            smart_tracklist_request("inspired-by-3"),
            json!({ "smarttracklist_id": "inspired-by-3", "lang": "us" })
        );
        assert_eq!(
            DEEZER_SMART_TRACKLIST_OPERATION,
            "deezer.pageSmartTracklist"
        );
        assert_eq!(
            track_mix_request("100"),
            json!({ "sng_id": "100", "start_with_input_track": false })
        );
        assert_eq!(artist_mix_request("200"), json!({ "art_id": "200" }));
        assert_eq!(
            similar_artists_request("200"),
            json!({ "id": "200", "nb": 10000, "start": 0 })
        );
    }

    #[test]
    fn captured_feedback_shapes_are_preserved() {
        assert_eq!(
            negative_feedback_request(DeezerFeedbackKind::Song, "100"),
            json!({ "ID": "100", "TYPE": "song" })
        );
        assert_eq!(
            negative_feedback_request(DeezerFeedbackKind::Artist, "200"),
            json!({ "ID": "200", "TYPE": "artist" })
        );
    }

    #[test]
    fn flow_response_preserves_opaque_tuner_for_the_next_call() {
        let fallback = FlowTuner::initial(FlowMode::Default);
        let continued = FlowTuner::from_response(
            Some(&json!("server-defined-continuation")),
            fallback.clone(),
        );
        assert_eq!(continued.as_str(), "server-defined-continuation");
        assert_eq!(
            FlowTuner::from_response(Some(&json!("")), fallback.clone()),
            fallback
        );
    }

    #[test]
    fn captured_radio_metadata_is_parsed_without_guessing() {
        let results = json!({
            "data": [{ "SNG_ID": "100" }],
            "total": "80",
            "clearRemainingTracks": true,
            "tuner": "discovery"
        });
        assert_eq!(result_items(&results).len(), 1);
        assert_eq!(result_total(&results, 0), 80);
        assert!(bool_value(results.get("clearRemainingTracks")));
    }

    #[test]
    fn hydration_keeps_station_order_and_adds_full_metadata() {
        let items = vec![
            json!({ "SNG_ID": "2", "SNG_TITLE": "Second" }),
            json!({ "SNG_ID": "1", "SNG_TITLE": "First" }),
        ];
        let hydrated = vec![
            json!({ "SNG_ID": "1", "ART_ID": "10", "ART_NAME": "One" }),
            json!({ "SNG_ID": "2", "ART_ID": "20", "ART_NAME": "Two" }),
        ];
        let tracks = merge_and_normalize_tracks(items, &hydrated);
        assert_eq!(
            tracks
                .iter()
                .map(|track| track.id.as_str())
                .collect::<Vec<_>>(),
            ["2", "1"]
        );
        assert_eq!(tracks[0].artist, "Two");
        assert_eq!(tracks[1].artist, "One");
    }

    #[test]
    fn related_artist_results_use_deezer_cards_and_report_total() {
        let related = parse_related_artists(&json!({
            "data": [{ "ART_ID": "20", "ART_NAME": "Related", "ART_PICTURE": "hash" }],
            "total": 25
        }));
        assert_eq!(related.total, 25);
        assert_eq!(related.artists.len(), 1);
        assert_eq!(related.artists[0].id, "20");
        assert_eq!(related.artists[0].source, Provider::Deezer);
    }

    #[test]
    fn only_nonempty_bounded_flow_configuration_ids_are_accepted() {
        assert_eq!(valid_flow_config_id(" CONFIG_1 ").unwrap(), "CONFIG_1");
        assert!(valid_flow_config_id("").is_err());
        assert!(valid_flow_config_id("bad\nconfig").is_err());
        assert!(valid_flow_config_id(&"a".repeat(129)).is_err());
    }

    #[test]
    fn smart_tracklist_parser_preserves_order_and_authoritative_metadata() {
        let results = json!({
            "DATA": {
                "SMARTTRACKLIST_ID": "inspired-by-3",
                "CONFIGURATION_ID": "inspired-by-3",
                "ID": "dynamic-user-seed-date",
                "TITLE": "Riddim Dubstep",
                "AUTO_GENERATED_TITLE": "Riddim Dubstep - 9/9/26",
                "SUBTITLE": "Featuring Subtronics, Yookie",
                "DESCRIPTION": "Discover music similar to the artists you've been listening to lately.",
                "COVER": { "TYPE": "artist", "MD5": "artist-hash" }
            },
            "SONGS": {
                "data": [
                    {
                        "SNG_ID": "2",
                        "SNG_TITLE": "Second",
                        "ART_ID": "20",
                        "ART_NAME": "Two",
                        "ALB_PICTURE": "album-hash",
                        "ARTISTS": [{ "ART_ID": "20", "ART_NAME": "Two" }]
                    },
                    {
                        "SNG_ID": "1",
                        "SNG_TITLE": "First",
                        "ART_ID": "10",
                        "ART_NAME": "One",
                        "ARTISTS": [{ "ART_ID": "10", "ART_NAME": "One" }]
                    }
                ],
                "count": 2,
                "total": "50"
            },
            "CONTEXT_ID": "config_name=inspired-by-3"
        });

        let page = parse_smart_tracklist(&results, " inspired-by-3 ").unwrap();
        assert_eq!(
            page.tracks
                .iter()
                .map(|track| track.id.as_str())
                .collect::<Vec<_>>(),
            ["2", "1"]
        );
        assert_eq!(page.tracks[0].title, "Second");
        assert_eq!(page.tracks[0].artist, "Two");
        assert_eq!(page.total, 50);
        assert_eq!(page.title, "Riddim Dubstep");
        assert_eq!(page.subtitle, "Featuring Subtronics, Yookie");
        assert_eq!(
            page.description,
            "Discover music similar to the artists you've been listening to lately."
        );
        assert_eq!(
            page.artwork,
            "https://e-cdns-images.dzcdn.net/images/artist/artist-hash/500x500.jpg"
        );
    }

    #[test]
    fn smart_tracklist_parser_falls_back_to_auto_title_and_ignores_dynamic_data_id() {
        let results = json!({
            "DATA": {
                "SMARTTRACKLIST_ID": "inspired-by-3",
                "ID": "not-the-config-id",
                "TITLE": "",
                "AUTO_GENERATED_TITLE": "Generated title",
                "SUBTITLE": "Generated subtitle"
            },
            "SONGS": {
                "data": [{ "SNG_ID": "42", "SNG_TITLE": "Track" }],
                "total": 1
            }
        });

        let page = parse_smart_tracklist(&results, "inspired-by-3").unwrap();
        assert_eq!(page.title, "Generated title");
        assert_eq!(page.subtitle, "Generated subtitle");
        assert_eq!(page.tracks[0].id, "42");
    }

    #[test]
    fn smart_tracklist_parser_skips_generic_title_for_dated_auto_title() {
        let results = json!({
            "DATA": {
                "CONFIGURATION_ID": "inspired-by-3",
                "TITLE": "daily",
                "AUTO_GENERATED_TITLE": "Electro Dance - 9/9/26"
            },
            "SONGS": {
                "data": [{ "SNG_ID": "42", "SNG_TITLE": "Track" }],
                "total": 50
            }
        });

        let page = parse_smart_tracklist(&results, "inspired-by-3").unwrap();
        assert_eq!(page.title, "Electro Dance");
        assert_eq!(
            page.resolved_smart_mix_title.as_deref(),
            Some("Electro Dance")
        );
    }

    #[test]
    fn smart_tracklist_parser_prefers_composition_title_over_daily_metadata() {
        let results = json!({
            "DATA": {
                "CONFIGURATION_ID": "inspired-by-3",
                "TITLE": "daily",
                "AUTO_GENERATED_TITLE": "daily 1 - 9/9/26",
                "COVER_COMPOSITION": { "TITLE": "Riddim Dubstep" }
            },
            "SONGS": {
                "data": [{ "SNG_ID": "42", "SNG_TITLE": "Track" }],
                "total": 50
            }
        });

        let page = parse_smart_tracklist(&results, "inspired-by-3").unwrap();
        assert_eq!(page.title, "Riddim Dubstep");
        assert_eq!(
            page.resolved_smart_mix_title.as_deref(),
            Some("Riddim Dubstep")
        );
    }

    #[test]
    fn smart_tracklist_parser_rejects_missing_or_mismatched_identity_and_empty_songs() {
        let missing_id = json!({
            "DATA": { "ID": "dynamic" },
            "SONGS": { "data": [{ "SNG_ID": "1" }] }
        });
        assert!(
            parse_smart_tracklist(&missing_id, "inspired-by-3")
                .unwrap_err()
                .contains("stable id")
        );

        let mismatched_id = json!({
            "DATA": { "SMARTTRACKLIST_ID": "inspired-by-4" },
            "SONGS": { "data": [{ "SNG_ID": "1" }] }
        });
        assert!(
            parse_smart_tracklist(&mismatched_id, "inspired-by-3")
                .unwrap_err()
                .contains("mismatched")
        );

        let empty_songs = json!({
            "DATA": { "CONFIGURATION_ID": "inspired-by-3" },
            "SONGS": { "data": [] }
        });
        assert!(
            parse_smart_tracklist(&empty_songs, "inspired-by-3")
                .unwrap_err()
                .contains("no songs")
        );

        let unusable_songs = json!({
            "DATA": { "CONFIGURATION_ID": "inspired-by-3" },
            "SONGS": { "data": [{ "SNG_ID": "not-a-deezer-id" }] }
        });
        assert!(
            parse_smart_tracklist(&unusable_songs, "inspired-by-3")
                .unwrap_err()
                .contains("usable")
        );
        assert!(parse_smart_tracklist(&json!({}), "inspired-by-3").is_err());
    }

    #[test]
    fn flow_catalog_preserves_filter_options_and_per_card_membership() {
        let catalog = parse_flow_catalog(&json!({
            "filter": {
                "default_option_id": "flow_config_mood",
                "options": [
                    { "id": "flow_config_mood", "label": "Moods" },
                    { "id": "flow_config_genre", "label": "Genres" }
                ]
            },
            "items": [
                {
                    "type": "flow",
                    "data": { "id": "default" },
                    "filter_option_ids": ["flow_config_mood", "flow_config_genre"]
                },
                {
                    "type": "flow",
                    "data": { "id": "genre-rock" },
                    "filter_option_ids": ["flow_config_genre"]
                }
            ]
        }))
        .unwrap();
        assert_eq!(catalog.default_option_id, "flow_config_mood");
        assert_eq!(catalog.options[0].label, "Moods");
        assert_eq!(catalog.options[1].label, "Genres");
        assert_eq!(
            catalog.option_ids_for("default"),
            ["flow_config_mood", "flow_config_genre"]
        );
        assert_eq!(catalog.option_ids_for("genre-rock"), ["flow_config_genre"]);
    }

    #[test]
    fn flow_catalog_is_absent_when_server_options_are_missing() {
        assert!(parse_flow_catalog(&json!({ "items": [] })).is_none());
        assert!(parse_flow_catalog(&json!({ "filter": { "options": [] }, "items": [] })).is_none());
    }
}
