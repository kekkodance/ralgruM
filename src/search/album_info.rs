use std::sync::Arc;

use gpui::{
    AnimationExt as _, Context, FontWeight, IntoElement, KeyDownEvent, Render, Window, div,
    prelude::*, px, rgb,
};
use gpui_component::WindowExt;
use serde_json::Value;
use tokio::runtime::Runtime;

use super::{
    Card, DeezerArl, Provider, SearchClient, SoundCloudToken,
    album_info_cache::AlbumInfoCacheHost,
    detail::{DetailPage, DetailRoute, format_release_date},
    models::ResultType,
    normalize::release_date as normalized_release_date,
};
use crate::{
    dialog_layout::{DialogCloseMotion, DialogCloseTarget, request_dialog_close},
    music_ui::ghost_close_button_with_icon_size,
    theme::{BORDER, FOREGROUND, MUTED},
};

/// Metadata surfaced by the "About this album" popover. Album rows follow
/// the track Info tag set for every field Deezer exposes at album level.
/// Track-only tags (ISRC, BPM, gain, per-track credits) stay off this
/// dialog. Playlists keep the original author/created/modified layout.
#[derive(Clone, Debug, Default, Eq, PartialEq)]
pub(crate) struct AlbumInfo {
    pub(crate) title: String,
    pub(crate) subtitle: String,
    pub(crate) artists: String,
    pub(crate) album_artist: String,
    pub(crate) album_type: String,
    pub(crate) label: String,
    pub(crate) genres: Vec<String>,
    pub(crate) explicit: Option<bool>,
    pub(crate) copyright: String,
    pub(crate) barcode: String,
    pub(crate) duration_seconds: Option<u64>,
    pub(crate) disc_count: Option<u64>,
    pub(crate) release_date: String,
    pub(crate) modified_date: String,
    pub(crate) description: String,
    pub(crate) track_count: Option<u64>,
}

impl AlbumInfo {
    pub(crate) fn has_content(&self) -> bool {
        !self.title.is_empty()
            || !self.subtitle.is_empty()
            || !self.artists.is_empty()
            || !self.album_artist.is_empty()
            || !self.album_type.is_empty()
            || !self.label.is_empty()
            || !self.genres.is_empty()
            || self.explicit.is_some()
            || !self.copyright.is_empty()
            || !self.barcode.is_empty()
            || self.duration_seconds.is_some()
            || self.disc_count.is_some()
            || !self.release_date.is_empty()
            || !self.modified_date.is_empty()
            || !self.description.is_empty()
            || self.track_count.is_some()
    }

    fn rows(&self, route: &DetailRoute) -> Vec<(&'static str, String)> {
        if route.kind == ResultType::Playlists {
            return self.playlist_rows(route);
        }
        self.album_rows(route)
    }

    fn playlist_rows(&self, route: &DetailRoute) -> Vec<(&'static str, String)> {
        let mut rows = Vec::new();
        let author = route.subtitle.trim();
        if !author.is_empty() {
            rows.push(("Author", author.to_owned()));
        }
        if !self.release_date.is_empty() {
            rows.push(("Created", format_release_date(&self.release_date)));
        }
        if !self.modified_date.is_empty() {
            rows.push(("Modified", format_release_date(&self.modified_date)));
        }
        if let Some(tracks) = self.track_count {
            rows.push(("Tracks", tracks.to_string()));
        }
        if let Some(duration) = self.duration_seconds.filter(|seconds| *seconds > 0) {
            rows.push(("Duration", format_duration(duration)));
        }
        rows
    }

    fn album_rows(&self, route: &DetailRoute) -> Vec<(&'static str, String)> {
        let mut rows = Vec::new();
        let artists = if self.artists.trim().is_empty() {
            route.subtitle.trim()
        } else {
            self.artists.trim()
        };
        if !artists.is_empty() {
            rows.push(("Artists", artists.to_owned()));
        }
        let title = if self.title.trim().is_empty() {
            route.title.trim()
        } else {
            self.title.trim()
        };
        if !title.is_empty() {
            rows.push(("Title", title.to_owned()));
        }
        push_text(&mut rows, "Subtitle", &self.subtitle);
        if !self.release_date.is_empty() {
            rows.push(("Release Date", format_release_date(&self.release_date)));
        }
        if !self.album_artist.trim().is_empty() && self.album_artist.trim() != artists {
            rows.push(("Album artist", self.album_artist.clone()));
        }
        push_text(&mut rows, "Album type", &self.album_type);
        if !self.genres.is_empty() {
            rows.push(("Genre", self.genres.join(", ")));
        }
        if let Some(explicit) = self.explicit {
            rows.push(("Explicit", if explicit { "Yes" } else { "No" }.to_owned()));
        }
        push_text(&mut rows, "Record Label", &self.label);
        push_text(&mut rows, "Copyright", &self.copyright);
        push_text(&mut rows, "Barcode", &self.barcode);
        if let Some(tracks) = self.track_count.filter(|count| *count > 0) {
            rows.push(("Total tracks", tracks.to_string()));
        }
        if let Some(discs) = self.disc_count.filter(|count| *count > 0) {
            rows.push(("Total discs", discs.to_string()));
        }
        if let Some(duration) = self.duration_seconds.filter(|seconds| *seconds > 0) {
            rows.push(("Duration", format_duration(duration)));
        }
        rows
    }
}

fn push_text(rows: &mut Vec<(&'static str, String)>, label: &'static str, value: &str) {
    if !value.trim().is_empty() {
        rows.push((label, value.to_owned()));
    }
}

const INFO_DIALOG_WIDTH: f32 = 440.;
const INFO_DIALOG_BODY_PADDING: f32 = 18.;
const INFO_DIALOG_LABEL_WIDTH: f32 = 96.;
const INFO_DIALOG_DESCRIPTION_LINE_HEIGHT: f32 = 18.;

pub(crate) fn format_duration(total_seconds: u64) -> String {
    let hours = total_seconds / 3600;
    let minutes = (total_seconds % 3600) / 60;
    let seconds = total_seconds % 60;
    if hours > 0 {
        format!("{hours}h {minutes}m")
    } else {
        format!("{minutes}m {seconds:02}s")
    }
}

fn payload_text(value: Option<&Value>) -> String {
    value
        .and_then(Value::as_str)
        .map(str::trim)
        .filter(|text| !text.is_empty())
        .unwrap_or_default()
        .to_owned()
}

fn payload_count(value: Option<&Value>) -> Option<u64> {
    value
        .and_then(Value::as_u64)
        .or_else(|| {
            value
                .and_then(Value::as_str)
                .and_then(|text| text.parse().ok())
        })
        .filter(|count| *count > 0)
}

fn capitalize_label(value: &str) -> String {
    let mut chars = value.chars();
    match chars.next() {
        Some(first) => first.to_uppercase().collect::<String>() + chars.as_str(),
        None => String::new(),
    }
}

fn album_type_label(album: &Value) -> String {
    let record = payload_text(
        album
            .get("record_type")
            .or_else(|| album.get("RECORD_TYPE")),
    );
    if !record.is_empty() {
        return match record.to_ascii_lowercase().as_str() {
            "compile" => "Compilation".to_owned(),
            other => capitalize_label(other),
        };
    }
    let type_value = album.get("TYPE").or_else(|| album.get("type"));
    let code = type_value.and_then(Value::as_u64).or_else(|| {
        type_value
            .and_then(Value::as_str)
            .and_then(|text| text.parse().ok())
    });
    match code {
        Some(0) => "Album".to_owned(),
        Some(1) => "Single".to_owned(),
        Some(2) => "Compilation".to_owned(),
        Some(3) => "EP".to_owned(),
        _ => {
            let type_text = payload_text(type_value);
            match type_text.to_ascii_lowercase().as_str() {
                "album" | "single" | "ep" => capitalize_label(&type_text),
                "compile" | "compilation" => "Compilation".to_owned(),
                _ => String::new(),
            }
        }
    }
}

fn album_explicit(album: &Value) -> Option<bool> {
    let value = album
        .get("explicit_lyrics")
        .or_else(|| album.get("EXPLICIT_LYRICS"));
    if let Some(explicit) = value.and_then(Value::as_bool) {
        return Some(explicit);
    }
    if let Some(flag) = value.and_then(Value::as_u64) {
        return Some(flag != 0);
    }
    let flag = payload_text(value);
    match flag.as_str() {
        "1" | "true" => return Some(true),
        "0" | "false" => return Some(false),
        _ => {}
    }
    let status = album
        .pointer("/EXPLICIT_ALBUM_CONTENT/EXPLICIT_LYRICS_STATUS")
        .and_then(Value::as_u64)
        .or_else(|| {
            album
                .pointer("/EXPLICIT_ALBUM_CONTENT/EXPLICIT_LYRICS_STATUS")
                .and_then(Value::as_str)
                .and_then(|text| text.parse().ok())
        });
    match status {
        Some(1) | Some(4) => Some(true),
        Some(0) => Some(false),
        _ => None,
    }
}

fn contributor_names(values: Option<&Value>) -> String {
    values
        .and_then(Value::as_array)
        .map(|items| {
            items
                .iter()
                .filter_map(|item| {
                    let name = payload_text(item.get("ART_NAME").or_else(|| item.get("name")));
                    (!name.is_empty()).then_some(name)
                })
                .collect::<Vec<_>>()
                .join(", ")
        })
        .unwrap_or_default()
}

/// Parses a Deezer gw-light album.getData response into popover metadata.
/// REST-style lowercase keys are accepted too so a public album payload
/// still fills the same rows.
pub(crate) fn parse_deezer_album_info(album: &Value) -> AlbumInfo {
    let genres = album
        .pointer("/genres/data")
        .or_else(|| album.pointer("/GENRES/data"))
        .and_then(Value::as_array)
        .map(|entries| {
            entries
                .iter()
                .map(|entry| payload_text(entry.get("name").or(entry.get("NAME"))))
                .filter(|genre| !genre.is_empty())
                .collect()
        })
        .unwrap_or_default();
    let mut artists = contributor_names(album.get("ARTISTS").or_else(|| album.get("contributors")));
    let album_artist = payload_text(
        album
            .get("ART_NAME")
            .or_else(|| album.pointer("/artist/name")),
    );
    if artists.is_empty() {
        artists = album_artist.clone();
    }
    AlbumInfo {
        title: payload_text(album.get("ALB_TITLE").or_else(|| album.get("title"))),
        subtitle: payload_text(
            album
                .get("ALB_SUBTITLE")
                .or_else(|| album.get("subtitle"))
                .or_else(|| album.get("VERSION"))
                .or_else(|| album.get("version")),
        ),
        artists,
        album_artist,
        album_type: album_type_label(album),
        label: payload_text(
            album
                .get("label")
                .or_else(|| album.get("LABEL"))
                .or_else(|| album.get("LABEL_NAME")),
        ),
        genres,
        explicit: album_explicit(album),
        copyright: payload_text(
            album
                .get("COPYRIGHT")
                .or_else(|| album.get("copyright"))
                .or_else(|| album.get("PRODUCER_LINE")),
        ),
        barcode: payload_text(album.get("UPC").or_else(|| album.get("upc"))),
        duration_seconds: payload_count(album.get("duration").or_else(|| album.get("DURATION"))),
        disc_count: payload_count(album.get("nb_disk").or_else(|| album.get("NB_DISK"))),
        release_date: normalized_release_date(album),
        modified_date: String::new(),
        description: String::new(),
        track_count: payload_count(
            album
                .get("NUMBER_TRACK")
                .or_else(|| album.get("nb_tracks"))
                .or_else(|| album.get("NB_TRACK")),
        ),
    }
}

/// Builds album popover metadata from a SoundCloud playlist payload.
pub(crate) fn soundcloud_album_info(playlist: &Value, description: &str) -> AlbumInfo {
    fn text(value: Option<&Value>) -> String {
        value
            .and_then(Value::as_str)
            .map(str::trim)
            .filter(|text| !text.is_empty())
            .unwrap_or_default()
            .to_owned()
    }
    fn count(value: Option<&Value>) -> Option<u64> {
        value
            .and_then(Value::as_u64)
            .or_else(|| {
                value
                    .and_then(Value::as_str)
                    .and_then(|text| text.parse().ok())
            })
            .filter(|count| *count > 0)
    }
    fn duration_seconds(playlist: &Value) -> Option<u64> {
        let milliseconds_to_seconds = |value: Option<&Value>| {
            count(value)
                .and_then(|milliseconds| milliseconds.checked_div(1_000))
                .filter(|seconds| *seconds > 0)
        };
        milliseconds_to_seconds(playlist.get("duration"))
            .or_else(|| milliseconds_to_seconds(playlist.get("full_duration")))
            .or_else(|| milliseconds_to_seconds(playlist.get("fullDuration")))
    }
    AlbumInfo {
        title: text(playlist.get("title")),
        artists: text(
            playlist
                .pointer("/user/username")
                .or_else(|| playlist.pointer("/user/full_name")),
        ),
        genres: text(playlist.get("genre"))
            .split('/')
            .map(str::trim)
            .filter(|genre| !genre.is_empty())
            .map(str::to_owned)
            .collect(),
        duration_seconds: duration_seconds(playlist),
        release_date: text(
            playlist
                .get("release_date")
                .or_else(|| playlist.get("display_date"))
                .or_else(|| playlist.get("created_at")),
        ),
        description: description.to_owned(),
        track_count: count(playlist.get("track_count")),
        ..AlbumInfo::default()
    }
}

/// Parses a Deezer gw-light deezer.pagePlaylist response into popover metadata.
pub(crate) fn parse_deezer_playlist_info(playlist: &Value) -> AlbumInfo {
    fn text(value: Option<&Value>) -> String {
        value
            .and_then(Value::as_str)
            .map(str::trim)
            .filter(|text| !text.is_empty())
            .unwrap_or_default()
            .to_owned()
    }
    fn count(value: Option<&Value>) -> Option<u64> {
        value.and_then(Value::as_u64).or_else(|| {
            value
                .and_then(Value::as_str)
                .and_then(|text| text.parse().ok())
        })
    }
    let data = playlist
        .pointer("/results/DATA")
        .or_else(|| playlist.pointer("/DATA"))
        .unwrap_or(playlist);
    let description = text(data.get("DESCRIPTION").or_else(|| data.get("description")));
    let release_date = text(
        data.get("DATE_ADD")
            .or_else(|| data.get("DATE_CREATE"))
            .or_else(|| data.get("date_add")),
    );
    let track_count = count(data.get("NB_SONG").or_else(|| data.get("nb_song")));
    let duration_seconds = count(data.get("DURATION").or_else(|| data.get("duration")));
    let modified_date = text(
        data.get("DATE_MOD")
            .or_else(|| data.get("date_mod"))
            .or_else(|| data.get("last_modified")),
    );
    AlbumInfo {
        title: text(data.get("TITLE").or_else(|| data.get("title"))),
        artists: text(
            data.get("PARENT_USERNAME")
                .or_else(|| data.pointer("/PARENT_USER/name"))
                .or_else(|| data.get("author")),
        ),
        duration_seconds,
        release_date,
        modified_date,
        description,
        track_count,
        ..AlbumInfo::default()
    }
}

pub(crate) fn album_info_for_page(page: &DetailPage) -> Option<AlbumInfo> {
    if !matches!(page.route.kind, ResultType::Albums | ResultType::Playlists) {
        return None;
    }
    let info = page.album_info.as_ref()?;
    if !info.has_content() {
        return None;
    }
    Some(info.clone())
}

/// Opens the Info dialog, using prefetched about data when available so the
/// dialog renders fully populated. Falls back to the existing sparse then
/// fetch flow when prefetch has not completed. Fresh fetches are stored into
/// the caller about cache so later dialogs and prefetches avoid duplicate
/// network requests.
pub(crate) fn open_card_info_dialog_with_prefetch<T: AlbumInfoCacheHost + 'static>(
    card: Card,
    runtime: Arc<Runtime>,
    client: Option<SearchClient>,
    deezer_arl: Option<DeezerArl>,
    soundcloud_token: Option<SoundCloudToken>,
    prefetched: Option<(DetailRoute, AlbumInfo)>,
    window: &mut Window,
    cx: &mut Context<T>,
) {
    open_card_info_dialog_inner(
        card,
        runtime,
        client,
        deezer_arl,
        soundcloud_token,
        prefetched,
        window,
        cx,
    );
}

pub(crate) fn open_local_playlist_info_dialog<T: 'static>(
    title: String,
    description: String,
    track_count: usize,
    duration_seconds: u64,
    window: &mut Window,
    cx: &mut Context<T>,
) {
    let route = DetailRoute {
        provider: Provider::Deezer,
        kind: ResultType::Playlists,
        id: "local".into(),
        title: title.clone(),
        subtitle: String::new(),
        artwork: String::new(),
        release_date: String::new(),
    };
    let info = AlbumInfo {
        title,
        description,
        track_count: Some(track_count as u64),
        duration_seconds: Some(duration_seconds),
        ..AlbumInfo::default()
    };
    let _ = open_album_info_dialog_entity(route, info, window, cx);
}

#[allow(clippy::too_many_arguments)]
fn open_card_info_dialog_inner<T: AlbumInfoCacheHost + 'static>(
    card: Card,
    runtime: Arc<Runtime>,
    client: Option<SearchClient>,
    deezer_arl: Option<DeezerArl>,
    soundcloud_token: Option<SoundCloudToken>,
    prefetched: Option<(DetailRoute, AlbumInfo)>,
    window: &mut Window,
    cx: &mut Context<T>,
) {
    let Some(route) = DetailRoute::from_card(&card) else {
        return;
    };
    if let Some((prefetched_route, prefetched_info)) = prefetched {
        if prefetched_route.provider == route.provider
            && prefetched_route.kind == route.kind
            && prefetched_route.id == route.id
            && prefetched_info.has_content()
        {
            let _ = open_album_info_dialog_entity(prefetched_route, prefetched_info, window, cx);
            return;
        }
    }
    // Seed the dialog with what the card already knows (artist and release
    // date, plus the playlist track count from the card badge). Rows for
    // fields that are still unknown stay hidden until the detail fetch below
    // resolves, so both entry points render the same filtered row set.
    let initial_info = AlbumInfo {
        title: card.title.clone(),
        artists: card.subtitle.clone(),
        release_date: card.release_date.clone(),
        track_count: (card.kind == ResultType::Playlists)
            .then(|| card.badge.trim().parse::<u64>().ok())
            .flatten(),
        ..AlbumInfo::default()
    };
    let Some(dialog) = open_album_info_dialog_entity(route.clone(), initial_info, window, cx)
    else {
        return;
    };
    let Some(client) = client else {
        return;
    };
    let cache_generation = cx.entity().read(cx).album_info_generation();
    let task =
        runtime.spawn(async move { client.detail(route, deezer_arl, soundcloud_token).await });
    cx.spawn_in(window, async move |this, cx| {
        let Ok(Ok(page)) = task.await else {
            return;
        };
        let Some(info) = album_info_for_page(&page) else {
            return;
        };
        let route = page.route.clone();
        let page_for_cache = page;
        let _ = this.update_in(cx, |view, window, cx| {
            if view.album_info_generation() != cache_generation {
                return;
            }
            view.store_album_info_page(&page_for_cache);
            if window.has_active_dialog(cx) {
                // Swap the content in place without a fade or height
                // animation so the dialog stays visually stable.
                dialog.update(cx, |dialog, cx| {
                    dialog.route = route;
                    dialog.info = info;
                    cx.notify();
                });
            }
        });
    })
    .detach();
}

fn open_album_info_dialog_entity<T: 'static>(
    route: DetailRoute,
    info: AlbumInfo,
    window: &mut Window,
    cx: &mut Context<T>,
) -> Option<gpui::Entity<AlbumInfoDialog>> {
    if window.has_active_dialog(cx) {
        return None;
    }
    let dialog = cx.new(|_| AlbumInfoDialog {
        route,
        info,
        close_motion: DialogCloseMotion::default(),
    });
    let dialog_content = dialog.clone();
    let viewport_height = f32::from(window.viewport_size().height);
    let centered_top = crate::dialog_layout::centered_margin_top(viewport_height, 320.);
    window.open_dialog(cx, move |dialog_view, _, cx| {
        let closing = dialog_content.read(cx).close_motion.closing();
        let close_epoch = dialog_content.read(cx).close_motion.epoch();
        let dialog_handle = dialog_content.clone();
        dialog_view
            .w(px(INFO_DIALOG_WIDTH))
            .max_w(px(560.))
            .margin_top(centered_top)
            .p_0()
            .gap_0()
            .bg(gpui::rgba(0x00000000))
            .border_0()
            .rounded(px(8.))
            .close_button(false)
            .overlay(true)
            .overlay_closable(true)
            .keyboard(false)
            .on_cancel(move |_, window, cx| {
                dialog_handle.update(cx, |this, cx| request_dialog_close(this, window, cx));
                false
            })
            .closing(closing, close_epoch)
            .child(dialog_content.clone())
    });
    Some(dialog)
}

struct AlbumInfoDialog {
    route: DetailRoute,
    info: AlbumInfo,
    close_motion: DialogCloseMotion,
}

impl DialogCloseTarget for AlbumInfoDialog {
    fn dialog_close_motion(&mut self) -> &mut DialogCloseMotion {
        &mut self.close_motion
    }
}

impl Render for AlbumInfoDialog {
    fn render(&mut self, _window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let rows = self.info.rows(&self.route);
        let closing = self.close_motion.closing();
        let close_epoch = self.close_motion.epoch();
        let has_description = !self.info.description.trim().is_empty();
        let dialog_entity = cx.entity();

        let data = div()
            .flex()
            .flex_col()
            .gap(px(12.))
            .when(!rows.is_empty(), |this| {
                this.child(
                    div()
                        .flex()
                        .flex_col()
                        .gap(px(7.))
                        .children(rows.into_iter().map(|(label, value)| {
                            div()
                                .flex()
                                .gap(px(12.))
                                .child(
                                    div()
                                        .w(px(INFO_DIALOG_LABEL_WIDTH))
                                        .flex_none()
                                        .text_size(px(12.))
                                        .text_color(rgb(MUTED))
                                        .child(label),
                                )
                                .child(div().min_w_0().text_size(px(12.5)).child(value))
                        })),
                )
            })
            .when(has_description, |this| {
                this.child(
                    div()
                        .flex()
                        .flex_col()
                        .gap(px(6.))
                        .child(
                            div()
                                .text_size(px(12.))
                                .font_weight(FontWeight::MEDIUM)
                                .child("Description"),
                        )
                        .child(
                            div()
                                .min_h(px(INFO_DIALOG_DESCRIPTION_LINE_HEIGHT))
                                .text_size(px(12.5))
                                .line_height(px(INFO_DIALOG_DESCRIPTION_LINE_HEIGHT))
                                .text_color(rgb(MUTED))
                                .child(self.info.description.clone()),
                        ),
                )
            });
        let body = div()
            .id("album-info-body")
            .flex()
            .flex_col()
            .p(px(INFO_DIALOG_BODY_PADDING))
            .child(data);

        let dialog = div()
            .id("album-info-dialog")
            .on_key_down(cx.listener(|this, event: &KeyDownEvent, window, cx| {
                if event.keystroke.key.as_str() != "escape" {
                    return;
                }
                window.prevent_default();
                request_dialog_close(this, window, cx);
                cx.stop_propagation();
            }))
            .overflow_hidden()
            .rounded(px(8.))
            .border_1()
            .border_color(rgb(BORDER))
            .bg(rgb(crate::theme::BACKGROUND))
            .text_color(rgb(FOREGROUND))
            .child(
                div()
                    .flex()
                    .items_center()
                    .justify_between()
                    .pl(px(18.))
                    .pr(px(14.))
                    .py(px(14.))
                    .border_b_1()
                    .border_color(rgb(BORDER))
                    .child(div().font_weight(FontWeight::SEMIBOLD).child(
                        if self.route.kind == ResultType::Playlists {
                            "About this playlist"
                        } else {
                            "About this album"
                        },
                    ))
                    .child(ghost_close_button_with_icon_size(
                        "album-info-close",
                        10.,
                        move |_, window, cx| {
                            dialog_entity
                                .update(cx, |this, cx| request_dialog_close(this, window, cx));
                        },
                    )),
            )
            .child(body);

        if closing {
            dialog
                .with_animation(
                    ("album-info-dialog-close", close_epoch),
                    crate::motion::dialog_close(),
                    |this, delta| this.opacity(crate::motion::lerp(1.0, 0.0, delta)),
                )
                .into_any_element()
        } else {
            dialog.into_any_element()
        }
    }
}

#[cfg(test)]
mod tests {
    use super::super::models::Provider;
    use super::*;
    use serde_json::json;

    #[test]
    fn deezer_album_data_maps_every_supported_field() {
        let info = parse_deezer_album_info(&json!({
            "ALB_TITLE": "Discovery",
            "ALB_SUBTITLE": "Remastered",
            "ART_NAME": "Daft Punk",
            "ARTISTS": [{"ART_NAME": "Daft Punk"}, {"ART_NAME": "Guest"}],
            "TYPE": "0",
            "LABEL": "Some Label",
            "COPYRIGHT": "2001 Daft Life",
            "UPC": "724384960650",
            "NUMBER_TRACK": "14",
            "NB_DISK": "2",
            "DURATION": 3725,
            "EXPLICIT_LYRICS": "0",
            "GENRES": {"data": [{"name": "Pop"}, {"name": "Rock"}]},
            "DIGITAL_RELEASE_DATE": "2019-05-10"
        }));
        assert_eq!(info.title, "Discovery");
        assert_eq!(info.subtitle, "Remastered");
        assert_eq!(info.artists, "Daft Punk, Guest");
        assert_eq!(info.album_artist, "Daft Punk");
        assert_eq!(info.album_type, "Album");
        assert_eq!(info.label, "Some Label");
        assert_eq!(info.copyright, "2001 Daft Life");
        assert_eq!(info.barcode, "724384960650");
        assert_eq!(info.track_count, Some(14));
        assert_eq!(info.disc_count, Some(2));
        assert_eq!(info.duration_seconds, Some(3725));
        assert_eq!(info.explicit, Some(false));
        assert_eq!(info.genres, vec!["Pop".to_owned(), "Rock".to_owned()]);
        assert_eq!(info.release_date, "2019-05-10");
        assert!(info.has_content());
    }

    #[test]
    fn rest_album_payload_fills_the_same_album_rows() {
        let info = parse_deezer_album_info(&json!({
            "title": "Discovery",
            "upc": "724384960650",
            "label": "Daft Life Ltd./ADA France",
            "nb_tracks": 14,
            "duration": 3662,
            "release_date": "2001-03-07",
            "record_type": "album",
            "explicit_lyrics": false,
            "contributors": [{"name": "Daft Punk"}],
            "artist": {"name": "Daft Punk"},
            "genres": {"data": [{"name": "Electro"}]},
        }));
        assert_eq!(info.title, "Discovery");
        assert_eq!(info.artists, "Daft Punk");
        assert_eq!(info.album_type, "Album");
        assert_eq!(info.barcode, "724384960650");
        assert_eq!(info.track_count, Some(14));
        assert_eq!(info.explicit, Some(false));
        assert_eq!(info.genres, vec!["Electro".to_owned()]);
    }

    #[test]
    fn deezer_playlist_data_maps_description_created_modified_and_tracks() {
        let info = parse_deezer_playlist_info(&json!({
            "results": {
                "DATA": {
                    "TITLE": "Rock Hits",
                    "DESCRIPTION": "The best rock tracks",
                    "DATE_ADD": "2024-03-01 10:00:00",
                    "DATE_MOD": "2024-03-05 12:00:00",
                    "NB_SONG": 42,
                    "DURATION": 7200
                }
            }
        }));
        assert_eq!(info.description, "The best rock tracks");
        assert_eq!(info.release_date, "2024-03-01 10:00:00");
        assert_eq!(info.modified_date, "2024-03-05 12:00:00");
        assert_eq!(info.track_count, Some(42));
        assert_eq!(info.duration_seconds, Some(7200));

        let route = DetailRoute {
            provider: Provider::Deezer,
            kind: ResultType::Playlists,
            id: "123".into(),
            title: "Rock Hits".into(),
            subtitle: "Curator".into(),
            artwork: String::new(),
            release_date: String::new(),
        };
        let rows = info.rows(&route);
        assert_eq!(rows[0], ("Author", "Curator".into()));
        assert_eq!(
            rows[1],
            ("Created", format_release_date(&info.release_date))
        );
        assert_eq!(
            rows[2],
            ("Modified", format_release_date(&info.modified_date))
        );
        assert_eq!(rows[3], ("Tracks", "42".into()));
        assert_eq!(rows[4], ("Duration", "2h 0m".into()));
    }

    #[test]
    fn card_info_hides_unavailable_rows_until_details_load() {
        let route = DetailRoute {
            provider: Provider::Deezer,
            kind: ResultType::Playlists,
            id: "123".into(),
            title: "Rock Hits".into(),
            subtitle: "Curator".into(),
            artwork: String::new(),
            release_date: String::new(),
        };
        // The right-click entry point seeds the dialog with card data only.
        // Rows without backing data stay hidden instead of rendering empty.
        assert_eq!(
            AlbumInfo::default().rows(&route),
            vec![("Author", "Curator".into())]
        );
    }

    #[test]
    fn card_info_rows_match_across_entry_points_once_details_load() {
        let route = DetailRoute {
            provider: Provider::Deezer,
            kind: ResultType::Albums,
            id: "42".into(),
            title: "Album".into(),
            subtitle: "Artist".into(),
            artwork: String::new(),
            release_date: "2019-05-10".into(),
        };
        let info = AlbumInfo {
            title: "Album".into(),
            artists: "Artist".into(),
            album_type: "Album".into(),
            label: "Some Label".into(),
            genres: vec!["Pop".into(), "Rock".into()],
            explicit: Some(false),
            copyright: "2019 Label".into(),
            barcode: "123456".into(),
            duration_seconds: Some(3725),
            disc_count: Some(2),
            track_count: Some(10),
            release_date: "2019-05-10".into(),
            ..AlbumInfo::default()
        };
        // Both the heading button and the right-click menu render this same
        // filtered row set, so populated fields appear in both dialogs.
        let rows = info.rows(&route);
        assert!(rows.contains(&("Artists", "Artist".into())));
        assert!(rows.contains(&("Title", "Album".into())));
        assert!(rows.contains(&("Release Date", "May 10, 2019".into())));
        assert!(rows.contains(&("Album type", "Album".into())));
        assert!(rows.contains(&("Genre", "Pop, Rock".into())));
        assert!(rows.contains(&("Explicit", "No".into())));
        assert!(rows.contains(&("Record Label", "Some Label".into())));
        assert!(rows.contains(&("Copyright", "2019 Label".into())));
        assert!(rows.contains(&("Barcode", "123456".into())));
        assert!(rows.contains(&("Total tracks", "10".into())));
        assert!(rows.contains(&("Total discs", "2".into())));
        assert!(rows.contains(&("Duration", "1h 2m".into())));

        let sparse = AlbumInfo {
            release_date: "2019-05-10".into(),
            ..AlbumInfo::default()
        };
        let sparse_rows = sparse.rows(&route);
        assert_eq!(
            sparse_rows,
            vec![
                ("Artists", "Artist".into()),
                ("Title", "Album".into()),
                ("Release Date", "May 10, 2019".into()),
            ]
        );
    }

    #[test]
    fn deezer_playlist_data_accepts_the_unwrapped_gateway_result() {
        let info = parse_deezer_playlist_info(&json!({
            "DATA": {
                "DESCRIPTION": "The unwrapped playlist",
                "DATE_ADD": "2024-04-01",
                "DATE_MOD": "2024-04-02",
                "NB_SONG": "7",
                "DURATION": "900"
            }
        }));

        assert_eq!(info.description, "The unwrapped playlist");
        assert_eq!(info.release_date, "2024-04-01");
        assert_eq!(info.modified_date, "2024-04-02");
        assert_eq!(info.track_count, Some(7));
        assert_eq!(info.duration_seconds, Some(900));
        assert!(info.has_content());
    }

    #[test]
    fn deezer_release_date_matches_the_album_heading_for_all_sources() {
        let payloads = [
            (
                json!({
                    "ALB_ID": "42",
                    "ALB_TITLE": "Album",
                    "ART_NAME": "Artist",
                    "PHYSICAL_RELEASE_DATE": "2026-06-05",
                    "DIGITAL_RELEASE_DATE": "2025-01-01",
                    "ORIGINAL_RELEASE_DATE": "2024-01-01"
                }),
                "2026-06-05",
            ),
            (
                json!({
                    "ALB_ID": "42",
                    "ALB_TITLE": "Album",
                    "ART_NAME": "Artist",
                    "ORIGINAL_RELEASE_DATE": "2026-06-05"
                }),
                "2026-06-05",
            ),
            (
                json!({
                    "ALB_ID": "42",
                    "ALB_TITLE": "Album",
                    "ART_NAME": "Artist",
                    "original_release_date": "2026-06-05"
                }),
                "2026-06-05",
            ),
            (
                json!({
                    "ALB_ID": "42",
                    "ALB_TITLE": "Album",
                    "ART_NAME": "Artist",
                    "originalReleaseDate": "2026-06-05"
                }),
                "2026-06-05",
            ),
            (
                json!({
                    "ALB_ID": "42",
                    "ALB_TITLE": "Album",
                    "ART_NAME": "Artist",
                    "release_date": "2026-06-05",
                    "display_date": "2025-01-01"
                }),
                "2026-06-05",
            ),
        ];

        for (payload, expected_raw) in payloads {
            let card = super::super::normalize::normalize_card(
                super::super::models::Provider::Deezer,
                ResultType::Albums,
                &payload,
            );
            let route = DetailRoute::from_card(&card).expect("representative album card");
            let info = parse_deezer_album_info(&payload);
            let released = info
                .rows(&route)
                .into_iter()
                .find(|(label, _)| *label == "Release Date")
                .map(|(_, value)| value);

            assert_eq!(route.release_date, expected_raw);
            assert_eq!(info.release_date, expected_raw);
            assert_eq!(
                super::super::detail::detail_metadata(&route),
                "Artist • Jun 5, 2026"
            );
            assert_eq!(released.as_deref(), Some("Jun 5, 2026"));
        }
    }

    #[test]
    fn empty_album_data_has_no_content() {
        assert!(!parse_deezer_album_info(&json!({})).has_content());
    }

    #[test]
    fn soundcloud_genre_slashes_split_into_genres() {
        let info =
            soundcloud_album_info(&json!({"genre": "Hip Hop / Rap", "duration": 600_000}), "");
        assert_eq!(info.genres, vec!["Hip Hop".to_owned(), "Rap".to_owned()]);
        assert_eq!(info.duration_seconds, Some(600));
    }

    #[test]
    fn soundcloud_duration_uses_full_duration_as_milliseconds_fallback() {
        let info = soundcloud_album_info(&json!({"full_duration": "125000"}), "");
        assert_eq!(info.duration_seconds, Some(125));

        let short_duration =
            soundcloud_album_info(&json!({"duration": 600, "full_duration": 125000}), "");
        assert_eq!(short_duration.duration_seconds, Some(125));

        let camel_case = soundcloud_album_info(&json!({"fullDuration": 3725000}), "");
        assert_eq!(camel_case.duration_seconds, Some(3725));
    }

    #[test]
    fn album_rows_format_release_dates_like_the_detail_heading() {
        let route = DetailRoute {
            provider: super::super::models::Provider::SoundCloud,
            kind: ResultType::Albums,
            id: "album-id".to_owned(),
            title: "Album".to_owned(),
            subtitle: "Artist".to_owned(),
            artwork: String::new(),
            release_date: String::new(),
        };

        for (raw_date, expected) in [
            ("2026-06-05T00:00:00Z", "Jun 5, 2026"),
            ("2026-06-05", "Jun 5, 2026"),
            ("2026-06", "2026-06"),
            ("not-a-date", "not-a-date"),
        ] {
            let info = AlbumInfo {
                release_date: raw_date.to_owned(),
                ..AlbumInfo::default()
            };
            let released = info
                .rows(&route)
                .into_iter()
                .find(|(label, _)| *label == "Release Date")
                .map(|(_, value)| value);
            assert_eq!(released.as_deref(), Some(expected));
        }
    }

    #[test]
    fn durations_render_as_hours_then_minutes() {
        assert_eq!(format_duration(3725), "1h 2m");
        assert_eq!(format_duration(125), "2m 05s");
    }

    #[test]
    fn info_dialog_registers_escape_and_overlay_dismiss_handlers() {
        let source = include_str!("album_info.rs");
        let production = &source[..source.find("#[cfg(test)]").unwrap()];
        assert!(production.contains("overlay_closable(true)"));
        assert!(production.contains("on_cancel"));
        assert!(production.contains("\"escape\""));
    }
}
