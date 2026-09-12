use std::collections::HashMap;

use serde::{Deserialize, Serialize};

#[derive(Clone, Copy, Debug, Eq, Hash, PartialEq, Serialize, Deserialize)]
pub enum LyricsProvider {
    Musixmatch,
    Genius,
}

#[derive(Clone, Debug, Default, PartialEq, Serialize, Deserialize)]
pub struct LyricsTrack {
    pub stable_id: Option<String>,
    pub artist: String,
    pub title: String,
    pub album: Option<String>,
    pub duration: Option<u64>,
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct LyricLine {
    pub time: f64,
    pub text: String,
    pub index: usize,
}

#[derive(Clone, Debug, Default, Eq, PartialEq, Serialize, Deserialize)]
pub struct LyricAnnotation {
    pub id: String,
    pub fragment: String,
    #[serde(default)]
    pub ids: Vec<String>,
}

impl LyricAnnotation {
    pub fn range_id(&self) -> String {
        if self.ids.is_empty() {
            self.id.trim().to_owned()
        } else {
            self.ids.join(",")
        }
    }
}

pub fn group_lyric_annotations(items: Vec<LyricAnnotation>) -> Vec<LyricAnnotation> {
    let mut grouped = Vec::new();
    let mut index_by_fragment = HashMap::new();
    for mut item in items {
        item.id = item.id.trim().to_owned();
        item.fragment = item
            .fragment
            .replace("\r\n", "\n")
            .replace('\r', "\n")
            .trim()
            .to_owned();
        normalize_annotation_ids(&mut item);
        if (item.id.is_empty() && item.ids.is_empty()) || item.fragment.is_empty() {
            continue;
        }
        let key = normalize_genius_fragment(&item.fragment);
        if key.is_empty() {
            continue;
        }
        if let Some(&index) = index_by_fragment.get(&key) {
            merge_annotation_ids(&mut grouped[index], item);
        } else {
            index_by_fragment.insert(key, grouped.len());
            grouped.push(item);
        }
    }
    grouped
}

fn normalize_genius_fragment(fragment: &str) -> String {
    fragment
        .split_whitespace()
        .collect::<Vec<_>>()
        .join(" ")
        .to_ascii_lowercase()
}

fn normalize_annotation_ids(item: &mut LyricAnnotation) {
    let mut ids = Vec::new();
    append_annotation_ids(&mut ids, &item.id);
    for id in &item.ids {
        append_annotation_ids(&mut ids, id);
    }
    item.ids = ids;
}

fn merge_annotation_ids(target: &mut LyricAnnotation, source: LyricAnnotation) {
    append_annotation_ids(&mut target.ids, &source.id);
    for id in source.ids {
        append_annotation_ids(&mut target.ids, &id);
    }
}

pub(super) fn append_annotation_ids(ids: &mut Vec<String>, raw: &str) {
    for part in raw.split(',') {
        let part = part.trim();
        if part.is_empty() {
            continue;
        }
        if !ids.iter().any(|existing| existing == part) {
            ids.push(part.to_owned());
        }
    }
}

#[derive(Clone, Debug, Eq, PartialEq, Serialize, Deserialize)]
pub enum EmptyLyricsReason {
    NotFound,
    NoMatch,
    MatchedWithoutLyrics,
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub enum LyricsResponse {
    Synced {
        text: String,
    },
    Plain {
        text: String,
    },
    Genius {
        text: String,
        annotations: Vec<LyricAnnotation>,
        url: String,
    },
    Empty {
        provider: LyricsProvider,
        reason: EmptyLyricsReason,
    },
}

#[derive(Clone, Debug, Eq, Hash, PartialEq)]
pub struct LyricsCacheKey {
    pub provider: LyricsProvider,
    pub identity: String,
}

impl LyricsCacheKey {
    pub fn new(provider: LyricsProvider, track: &LyricsTrack) -> Self {
        let identity = track
            .stable_id
            .as_deref()
            .map(str::trim)
            .filter(|id| !id.is_empty())
            .map(|id| format!("id:{id}"))
            .unwrap_or_else(|| {
                format!(
                    "metadata:{}\u{1f}{}\u{1f}{}\u{1f}{}",
                    normalize(&track.artist),
                    normalize(&track.title),
                    normalize(track.album.as_deref().unwrap_or_default()),
                    track
                        .duration
                        .map_or(String::new(), |value| value.to_string())
                )
            });
        Self { provider, identity }
    }
}

fn normalize(value: &str) -> String {
    value
        .split_whitespace()
        .collect::<Vec<_>>()
        .join(" ")
        .to_lowercase()
}
