mod album_info;
mod album_info_cache;
mod artist_client;
mod browser_link;
mod cards_view;
mod client;
mod collection_actions;
mod credential;
mod detail;
mod detail_client;
mod detail_view;
mod discover;
mod models;
mod normalize;
mod results_view;
mod rows_view;
mod skeleton;
mod soundcloud_metadata;
mod suggestions;
mod view;

pub(crate) const SEARCH_PLACEHOLDER: &str = "Search songs, artists, or albums...";

pub(crate) use album_info::{
    AlbumInfo, open_card_info_dialog_with_prefetch, open_local_playlist_info_dialog,
    soundcloud_album_info,
};
pub(crate) use album_info_cache::{AlbumInfoCacheHost, AlbumInfoCacheKey, AlbumInfoPrefetch};
pub(crate) use client::{SOUNDCLOUD_CLIENT_ID, SearchClient};
pub(crate) use collection_actions::collection_routable;
pub(crate) use credential::{
    DEEZER_USER_AGENT, DeezerArl, DeezerCookieJar, SoundCloudToken, merge_cookie_parts,
};
pub(crate) use detail::{
    DetailPage, DetailRoute, artist_section_shows_action_for_counts,
    collection_subtitle_is_visible, detail_metadata, format_release_date,
};
#[cfg(test)]
pub(crate) use models::ArtistPage;
pub(crate) use models::{
    Card, Provider, ResultType, Source, Track, TrackArtistCollector, TrackArtistRef,
};
pub(crate) use normalize::{format_number, release_date, soundcloud_service_url};
pub(crate) use soundcloud_metadata::artist_subtitle as soundcloud_artist_subtitle;
pub(crate) use suggestions::{SuggestionKind, SuggestionRow};
pub(crate) use view::SearchView;
