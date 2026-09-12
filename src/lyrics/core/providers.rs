use super::models::{append_annotation_ids, group_lyric_annotations};
use super::{EmptyLyricsReason, LyricAnnotation, LyricsProvider, LyricsResponse};
use serde_json::Value;

pub fn extract_musixmatch_lyrics(value: &Value) -> Vec<LyricsResponse> {
    let subtitles = value.pointer("/message/body/macro_calls/track.subtitles.get/message/body/subtitle_list/0/subtitle/subtitle_body").and_then(Value::as_str);
    let plain = value
        .pointer("/message/body/macro_calls/track.lyrics.get/message/body/lyrics/lyrics_body")
        .and_then(Value::as_str);
    let mut result = Vec::new();
    if let Some(text) = subtitles.filter(|text| !text.is_empty()) {
        result.push(LyricsResponse::Synced { text: text.into() });
    }
    if let Some(text) = plain.filter(|text| !text.is_empty()) {
        result.push(LyricsResponse::Plain { text: text.into() });
    }
    if result.is_empty() {
        result.push(LyricsResponse::Empty {
            provider: LyricsProvider::Musixmatch,
            reason: EmptyLyricsReason::NotFound,
        });
    }
    result
}

#[cfg(test)]
pub fn extract_genius_hits(value: &Value) -> Vec<Value> {
    value
        .pointer("/response/sections/0/hits")
        .or_else(|| value.pointer("/response/hits"))
        .and_then(Value::as_array)
        .cloned()
        .unwrap_or_default()
}

pub fn extract_genius_lyrics(value: &Value) -> Option<String> {
    value
        .pointer("/response/song/lyrics/plain")
        .and_then(Value::as_str)
        .map(str::trim)
        .filter(|text| !text.is_empty())
        .map(str::to_owned)
}

pub fn extract_genius_lyrics_result(value: &Value, url: &str) -> LyricsResponse {
    let text = value
        .get("lyrics")
        .and_then(Value::as_str)
        .map(str::trim)
        .filter(|text| !text.is_empty());
    match text {
        Some(text) => LyricsResponse::Genius {
            text: text.to_owned(),
            annotations: value
                .get("annotations")
                .and_then(Value::as_array)
                .map(|annotations| {
                    group_lyric_annotations(
                        annotations
                            .iter()
                            .filter_map(annotation_from_value)
                            .collect(),
                    )
                })
                .unwrap_or_default(),
            url: url.trim().to_owned(),
        },
        None => LyricsResponse::Empty {
            provider: LyricsProvider::Genius,
            reason: EmptyLyricsReason::MatchedWithoutLyrics,
        },
    }
}

pub fn extract_genius_referents(value: &Value) -> Vec<LyricAnnotation> {
    group_lyric_annotations(
        value
            .pointer("/response/referents")
            .and_then(Value::as_array)
            .into_iter()
            .flatten()
            .filter(|referent| {
                referent.get("is_description").and_then(Value::as_bool) != Some(true)
                    && referent
                        .get("annotations")
                        .and_then(Value::as_array)
                        .is_some_and(|items| !items.is_empty())
            })
            .filter_map(annotation_from_value)
            .collect(),
    )
}

fn annotation_from_value(value: &Value) -> Option<LyricAnnotation> {
    let id = parse_genius_id_token(value.get("id")?)?;
    let fragment = value.get("fragment")?.as_str()?.trim().to_owned();
    if fragment.is_empty() {
        return None;
    }
    let mut ids = Vec::new();
    append_annotation_ids(&mut ids, &id);
    if let Some(items) = value.get("ids").and_then(Value::as_array) {
        for item in items {
            if let Some(parsed) = parse_genius_id_token(item) {
                append_annotation_ids(&mut ids, &parsed);
            }
        }
    }
    if let Some(annotations) = value.get("annotations").and_then(Value::as_array) {
        for annotation in annotations {
            if let Some(referent_id) = annotation
                .get("referent_id")
                .and_then(parse_genius_id_token)
            {
                append_annotation_ids(&mut ids, &referent_id);
            }
        }
    }
    let id = ids.first().cloned().unwrap_or(id);
    Some(LyricAnnotation { id, fragment, ids })
}

fn parse_genius_id_token(value: &Value) -> Option<String> {
    match value {
        Value::Number(number) if number.as_u64().is_some() => Some(number.to_string()),
        Value::String(id) => {
            let trimmed = id.trim();
            if trimmed.is_empty() {
                return None;
            }
            let valid = trimmed.split(',').all(|part| {
                let part = part.trim();
                !part.is_empty() && part.bytes().all(|byte| byte.is_ascii_digit())
            });
            valid.then(|| trimmed.to_owned())
        }
        _ => None,
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;
    #[test]
    fn extracts_musixmatch_paths_and_empty() {
        let value = json!({
            "message": {
                "body": {
                    "macro_calls": {
                        "track.subtitles.get": {"message": {"body": {"subtitle_list": [{"subtitle": {"subtitle_body": "[00:01.00] hi"}}]}}},
                        "track.lyrics.get": {"message": {"body": {"lyrics": {"lyrics_body": "plain"}}}}
                    }
                }
            }
        });
        assert_eq!(extract_musixmatch_lyrics(&value).len(), 2);
        assert!(matches!(
            extract_musixmatch_lyrics(&json!({}))[0],
            LyricsResponse::Empty { .. }
        ));
    }
    #[test]
    fn extracts_both_genius_hit_shapes_and_valid_referents() {
        let value = json!({"response":{"hits":[{"result":{"id":1}}],"song":{"lyrics":{"plain":" lyrics "}},"referents":[{"id":42,"fragment":"line","annotations":[{}]},{"id":43,"fragment":"description","is_description":true,"annotations":[{}]}]}});
        assert_eq!(extract_genius_hits(&value).len(), 1);
        assert_eq!(extract_genius_lyrics(&value).as_deref(), Some("lyrics"));
        assert_eq!(
            extract_genius_referents(&value),
            vec![LyricAnnotation {
                id: "42".into(),
                fragment: "line".into(),
                ids: vec!["42".into()],
            }]
        );
    }

    #[test]
    fn genius_sections_hits_take_precedence_and_preserve_order() {
        let value = json!({"response":{"sections":[{"hits":[{"result":{"id":2}},{"result":{"id":1}}]}],"hits":[{"result":{"id":3}}]}});
        let hits = extract_genius_hits(&value);
        assert_eq!(
            hits[0].pointer("/result/id").and_then(Value::as_u64),
            Some(2)
        );
        assert_eq!(
            hits[1].pointer("/result/id").and_then(Value::as_u64),
            Some(1)
        );
    }

    #[test]
    fn genius_missing_or_malformed_shapes_are_empty() {
        assert!(extract_genius_hits(&json!({"response":{"hits":{}}})).is_empty());
        assert_eq!(
            extract_genius_lyrics(&json!({"response":{"song":{"lyrics":{"plain":"  "}}}})),
            None
        );
        assert!(extract_genius_referents(&json!({"response":{"referents":"bad"}})).is_empty());
    }

    #[test]
    fn genius_command_result_extracts_annotations_and_empty_reason() {
        let result = extract_genius_lyrics_result(
            &json!({"lyrics":" text ","annotations":[{"id":"12","fragment":" text "},{"id":"bad","fragment":"ignored"}]}),
            " https://genius.com/song ",
        );
        assert_eq!(
            result,
            LyricsResponse::Genius {
                text: "text".into(),
                annotations: vec![LyricAnnotation {
                    id: "12".into(),
                    fragment: "text".into(),
                    ids: vec!["12".into()],
                }],
                url: "https://genius.com/song".into()
            }
        );
        assert!(matches!(
            extract_genius_lyrics_result(&json!({}), ""),
            LyricsResponse::Empty {
                reason: EmptyLyricsReason::MatchedWithoutLyrics,
                ..
            }
        ));
    }

    #[test]
    fn genius_referents_with_the_same_fragment_are_grouped() {
        let value = json!({
            "response": {
                "referents": [
                    {
                        "id": 8506070,
                        "fragment": "[Produced by Diplo & Skrillex]",
                        "annotations": [{ "id": 1001, "referent_id": 7274811 }]
                    },
                    {
                        "id": 8505909,
                        "fragment": " [Produced by Diplo & Skrillex] ",
                        "annotations": [{ "id": 1002, "referent_id": 7274811 }]
                    },
                    {
                        "id": 99,
                        "fragment": "other line",
                        "annotations": [{}]
                    }
                ]
            }
        });
        let extracted = extract_genius_referents(&value);
        assert_eq!(
            extracted,
            vec![
                LyricAnnotation {
                    id: "8506070".into(),
                    fragment: "[Produced by Diplo & Skrillex]".into(),
                    ids: vec!["8506070".into(), "7274811".into(), "8505909".into()],
                },
                LyricAnnotation {
                    id: "99".into(),
                    fragment: "other line".into(),
                    ids: vec!["99".into()],
                },
            ]
        );
    }
}
