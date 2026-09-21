use std::{sync::Arc, time::Duration};

use gpui::{
    AnyElement, Context, Entity, FontWeight, IntoElement, KeyDownEvent, ListAlignment, ListState,
    Render, SharedString, Task, WeakEntity, Window, div, prelude::*, px, rgb,
};
use gpui_component::scroll::{Scrollbar, ScrollbarShow};
use tokio::runtime::Runtime;

use crate::{
    assets::{LocalIcon, local_icon},
    browser_scroll::{
        BrowserScrollState, BrowserScrollTarget, FixedListScrollHandle, browser_scroll_surface,
    },
    context_menu::{self, track_menu},
    downloads::DownloadModel,
    entity_navigation::{ProviderNavigationOpeners, artist_routes_for_track},
    library::{FavoriteKey, FavoriteKind, FavoriteState, LibraryView},
    music_ui::{
        TrackArtistNavigation, TrackRowDisplay, row_download_button, row_more_button,
        track_row_with_action,
    },
    playback::{AudioCache, PlaybackContext, PlaybackModel, PlaybackProvider, PlaybackTrack},
    playing_indicator::{PlayingSnapshot, QueuePlayingSnapshot},
    search::Provider,
    settings::AccountState,
    theme::{FOREGROUND, MUTED},
};

const CACHE_REFRESH_INTERVAL: Duration = Duration::from_millis(500);
const CACHE_CONTENT_BOTTOM_PADDING_PX: f32 = 16.;
pub(crate) const CACHE_ICON: LocalIcon = LocalIcon::HardDrive;

pub(crate) struct CacheView {
    cache: AudioCache,
    runtime: Arc<Runtime>,
    playback: Entity<PlaybackModel>,
    library: WeakEntity<LibraryView>,
    navigation_openers: ProviderNavigationOpeners,
    downloads: Entity<DownloadModel>,
    account: Entity<AccountState>,
    favorites: Entity<FavoriteState>,
    tracks: Arc<Vec<PlaybackTrack>>,
    playing: PlayingSnapshot,
    loading: bool,
    load_generation: u64,
    loaded_revision: Option<u64>,
    list_state: ListState,
    browser_scroll: BrowserScrollState,
    _watcher: Task<()>,
}

impl CacheView {
    pub(crate) fn new(
        cache: AudioCache,
        runtime: Arc<Runtime>,
        playback: Entity<PlaybackModel>,
        library: &Entity<LibraryView>,
        navigation_openers: ProviderNavigationOpeners,
        downloads: Entity<DownloadModel>,
        account: Entity<AccountState>,
        favorites: Entity<FavoriteState>,
        cx: &mut Context<Self>,
    ) -> Self {
        let playing = PlayingSnapshot::from_playback(&playback.read(cx).state);
        cx.observe(&playback, |this, playback, cx| {
            let playing = PlayingSnapshot::from_playback(&playback.read(cx).state);
            if playing != this.playing {
                this.playing = playing;
                cx.notify();
            }
        })
        .detach();

        let executor = cx.background_executor().clone();
        let watcher = cx.spawn(async move |this, cx| {
            loop {
                executor.timer(CACHE_REFRESH_INTERVAL).await;
                if this
                    .update(cx, |this, cx| this.refresh_if_needed(cx))
                    .is_err()
                {
                    break;
                }
            }
        });

        let mut view = Self {
            cache,
            runtime,
            playback,
            library: library.downgrade(),
            navigation_openers,
            downloads,
            account,
            favorites,
            tracks: Arc::new(Vec::new()),
            playing,
            loading: false,
            load_generation: 0,
            loaded_revision: None,
            list_state: ListState::new(
                0,
                ListAlignment::Top,
                crate::library::virtualization::overdraw(),
            )
            .with_uniform_item_height(crate::library::virtualization::row_height()),
            browser_scroll: BrowserScrollState::new(),
            _watcher: watcher,
        };
        view.refresh(cx);
        view
    }

    fn refresh_if_needed(&mut self, cx: &mut Context<Self>) {
        if !self.loading && self.loaded_revision != Some(self.cache.revision()) {
            self.refresh(cx);
        }
    }

    fn refresh(&mut self, cx: &mut Context<Self>) {
        self.load_generation = self.load_generation.wrapping_add(1);
        let generation = self.load_generation;
        let revision = self.cache.revision();
        self.loading = true;
        let cache = self.cache.clone();
        let task = self
            .runtime
            .spawn(async move { cache.cached_tracks().await });
        cx.spawn(async move |this, cx| {
            let Ok(tracks) = task.await else {
                let _ = this.update(cx, |this, cx| {
                    if this.load_generation == generation {
                        this.loading = false;
                        cx.notify();
                    }
                });
                return;
            };
            let _ = this.update(cx, |this, cx| {
                if this.load_generation != generation {
                    return;
                }
                if this.tracks.len() != tracks.len() {
                    this.list_state.reset_with_uniform_height(
                        tracks.len(),
                        crate::library::virtualization::row_height(),
                    );
                }
                this.tracks = Arc::new(tracks);
                this.loading = false;
                // This result describes the revision at which its read began.
                // A newer cache mutation must trigger another refresh instead
                // of being mistaken for part of this snapshot.
                this.loaded_revision = Some(revision);
                cx.notify();
            });
        })
        .detach();
    }
}

impl Render for CacheView {
    fn render(&mut self, window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        self.refresh_if_needed(cx);
        let metrics = crate::music_ui::shell_metrics(f32::from(window.viewport_size().width));
        let narrow =
            crate::music_ui::narrow_content_viewport(f32::from(window.viewport_size().width));
        let tracks = self.tracks.clone();
        let host = cx.entity();
        let count = tracks.len();
        let loading = self.loading;
        let gutter = crate::music_ui::main_content_inset(&metrics);
        let playing = self.playing.for_queue(&tracks);
        let fixed_scroll = FixedListScrollHandle::new(
            self.list_state.clone(),
            count,
            crate::library::virtualization::row_height(),
        );
        let list = gpui::list(self.list_state.clone(), move |index, _window, app| {
            let row = render_cache_row(
                index,
                tracks.clone(),
                narrow,
                host.read(app),
                &host,
                playing,
                app,
            );
            div()
                .w_full()
                .h(crate::library::virtualization::row_height())
                .child(row)
                .into_any_element()
        });
        let body = div()
            .id("cache-page-content")
            .size_full()
            .min_h_0()
            .px(px(gutter))
            .pb(px(CACHE_CONTENT_BOTTOM_PADDING_PX))
            .when(count == 0, |this| this.child(empty_cache_state(loading)))
            .when(count > 0, |this| {
                this.child(list.w_full().h_full().min_h_0())
            });
        let scroll_viewport = div()
            .id("cache-page-scroll-viewport")
            .relative()
            .flex_1()
            .min_h_0()
            .child(body)
            .child(
                div()
                    .absolute()
                    .inset_0()
                    .child(Scrollbar::vertical(&fixed_scroll).scrollbar_show(ScrollbarShow::Hover)),
            )
            .into_any_element();
        let scroll = browser_scroll_surface(
            "cache-page-scroll",
            scroll_viewport,
            BrowserScrollTarget::FixedList(fixed_scroll),
            self.browser_scroll.clone(),
        );

        div()
            .id("cache-page")
            .size_full()
            .flex()
            .flex_col()
            .child(cache_header(count, gutter))
            .child(scroll)
    }
}

fn cache_header(count: usize, gutter: f32) -> AnyElement {
    div()
        .id("cache-page-header")
        .flex_none()
        .w_full()
        .px(px(gutter))
        .pt(px(24.))
        .pb(px(12.))
        .child(
            div()
                .flex()
                .items_center()
                .justify_between()
                .gap(px(12.))
                .child(
                    div()
                        .flex()
                        .flex_col()
                        .gap(px(3.))
                        .child(
                            div()
                                .text_size(px(20.))
                                .font_weight(FontWeight(650.))
                                .text_color(rgb(FOREGROUND))
                                .child("Cache"),
                        )
                        .child(
                            div()
                                .text_size(px(13.))
                                .text_color(rgb(MUTED))
                                .child("Tracks you've played before, cached for offline playback."),
                        ),
                )
                .child(
                    div()
                        .flex_none()
                        .text_size(px(13.))
                        .text_color(rgb(MUTED))
                        .child(format!(
                            "{count} {}",
                            if count == 1 { "track" } else { "tracks" }
                        )),
                ),
        )
        .into_any_element()
}

fn render_cache_row(
    index: usize,
    tracks: Arc<Vec<PlaybackTrack>>,
    narrow: bool,
    view: &CacheView,
    host: &Entity<CacheView>,
    playing: QueuePlayingSnapshot,
    cx: &gpui::App,
) -> AnyElement {
    let Some(track) = tracks.get(index) else {
        return div().into_any_element();
    };
    let blocked = view.playing.blocks(track);
    let click_playback = view.playback.clone();
    let key_playback = view.playback.clone();
    let click_tracks = tracks.clone();
    let key_tracks = tracks.clone();
    let source = provider(track.provider);
    let (open_album, open_artist) = view.navigation_openers.for_provider(source);
    let artist_navigation = {
        let routes = artist_routes_for_track(source, &track.artists);
        (!routes.is_empty()).then(|| TrackArtistNavigation {
            provider: source,
            routes,
            opener: open_artist.clone(),
        })
    };
    let trailing_actions = div()
        .flex()
        .gap(px(4.))
        .child(download_button(
            index,
            track,
            &view.downloads,
            &view.account,
        ))
        .child(
            context_menu::track_menu_button(
                row_more_button(SharedString::from(format!(
                    "cache-track-more-{index}-{}-{}",
                    source.label(),
                    track.id
                ))),
                context_menu::queue_track_entity(track),
                view.playback.clone(),
                view.downloads.clone(),
                view.account.clone(),
                host.clone(),
                open_album.clone(),
                open_artist.clone(),
                false,
                None,
            )
            .into_any_element(),
        )
        .into_any_element();
    let row = track_row_with_action(
        index,
        "cache",
        &track.title,
        &track.artist,
        artist_navigation,
        &track.artwork,
        track.duration.as_secs(),
        TrackRowDisplay {
            provider: Some(source),
            narrow,
            provider_icon_only: false,
        },
        crate::music_ui::TrackRowLabels {
            explicit: track.explicit,
            ai_generated: track.ai_generated,
        },
        playing.row(index),
        None,
        blocked,
        None,
        None,
        Some(trailing_actions),
        cx,
    )
    .id(("cache-playback-row", index))
    .focusable()
    .tab_stop(!blocked)
    .role(gpui::Role::Button)
    .focus_visible(|style| style.border_1().border_color(rgb(crate::theme::PRIMARY)))
    .when(!blocked, |this| this.cursor_pointer())
    .on_click(move |_, _, cx| {
        let Some(selected) = click_tracks.get(index) else {
            return;
        };
        click_playback.update(cx, |playback, cx| {
            if !playback.state.content_blocked(selected) {
                playback.replace_queue((*click_tracks).clone(), index, cx);
                playback.set_context(PlaybackContext::None, cx);
            }
        });
    })
    .on_key_down(move |event: &KeyDownEvent, window, cx| {
        if !crate::tab_keyboard::is_activation_key(event.keystroke.key.as_str()) {
            return;
        }
        window.prevent_default();
        let Some(selected) = key_tracks.get(index) else {
            return;
        };
        key_playback.update(cx, |playback, cx| {
            if !playback.state.content_blocked(selected) {
                playback.replace_queue((*key_tracks).clone(), index, cx);
                playback.set_context(PlaybackContext::None, cx);
            }
        });
    });

    track_menu(
        row,
        context_menu::queue_track_entity(track),
        view.playback.clone(),
        view.downloads.clone(),
        view.account.clone(),
        host.clone(),
        open_album,
        open_artist,
        false,
        None,
    )
    .into_any_element()
}

fn download_button(
    index: usize,
    track: &PlaybackTrack,
    downloads: &Entity<DownloadModel>,
    account: &Entity<AccountState>,
) -> AnyElement {
    context_menu::track_download_button_above(
        row_download_button(SharedString::from(format!(
            "cache-track-download-{index}-{}-{}",
            provider(track.provider).label(),
            track.id
        ))),
        track.clone(),
        downloads.clone(),
        account.clone(),
    )
    .into_any_element()
}

impl crate::entity_navigation::TrackMenuHost for CacheView {
    fn favorites_entity(&self) -> Entity<FavoriteState> {
        self.favorites.clone()
    }

    fn local_track_saved(&self, track: &PlaybackTrack, cx: &gpui::App) -> bool {
        self.library.upgrade().is_some_and(|library| {
            crate::entity_navigation::TrackMenuHost::local_track_saved(library.read(cx), track, cx)
        })
    }

    fn set_local_track_saved(&mut self, track: PlaybackTrack, saved: bool, cx: &mut Context<Self>) {
        if let Some(library) = self.library.upgrade() {
            library.update(cx, |library, cx| {
                crate::entity_navigation::TrackMenuHost::set_local_track_saved(
                    library, track, saved, cx,
                )
            });
        }
    }

    fn can_remove_from_cache(&self, _provider: Provider, track_id: &str) -> bool {
        !track_id.trim().is_empty()
    }

    fn remove_from_cache(&mut self, provider: Provider, track_id: String, cx: &mut Context<Self>) {
        let cache = self.cache.clone();
        let cache_provider = match provider {
            Provider::Deezer => PlaybackProvider::Deezer,
            Provider::SoundCloud => PlaybackProvider::SoundCloud,
        };
        let task = self
            .runtime
            .spawn(async move { cache.remove_track(cache_provider, &track_id).await });
        cx.spawn(async move |this, cx| {
            let result = task
                .await
                .unwrap_or_else(|_| Err("Cache removal request failed".into()));
            let _ = this.update(cx, |_, cx| {
                match result {
                    Ok(true) => crate::toast::push_global(
                        cx,
                        crate::toast::ToastKind::Success,
                        "Removed from cache",
                        None,
                    ),
                    Ok(false) => crate::toast::push_global(
                        cx,
                        crate::toast::ToastKind::Error,
                        "Could not remove from cache",
                        Some("The cached track is no longer in the cache catalog.".into()),
                    ),
                    Err(error) => crate::toast::push_global(
                        cx,
                        crate::toast::ToastKind::Error,
                        "Could not remove from cache",
                        Some(error.into()),
                    ),
                }
                cx.notify();
            });
        })
        .detach();
    }

    fn resolve_favorite_state(&mut self, key: FavoriteKey, cx: &mut Context<Self>) {
        if let Some(library) = self.library.upgrade() {
            library.update(cx, |library, cx| {
                library.resolve_favorite_state(key, cx);
            });
        }
    }

    fn open_playlist_picker(
        &mut self,
        track_id: String,
        provider: Provider,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        if let Some(library) = self.library.upgrade() {
            library.update(cx, |library, cx| {
                library.open_add_picker(vec![track_id], "cache".into(), provider, window, cx)
            });
        }
    }

    fn open_local_playlist_picker(
        &mut self,
        track: PlaybackTrack,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        if let Some(library) = self.library.upgrade() {
            library.update(cx, |library, cx| {
                library.open_local_playlist_picker(track, window, cx);
            });
        }
    }

    fn preload_playlist_catalog(&mut self, provider: Provider, cx: &mut Context<Self>) {
        if let Some(library) = self.library.upgrade() {
            library.update(cx, |library, cx| match provider {
                Provider::Deezer => library.ensure_playlist_catalog(cx),
                Provider::SoundCloud => library.ensure_soundcloud_playlist_catalog(cx),
            });
        }
    }

    fn toggle_favorite_state(
        &mut self,
        provider: Provider,
        track_id: String,
        known_favorite: bool,
        cx: &mut Context<Self>,
    ) {
        if let Some(library) = self.library.upgrade() {
            library.update(cx, |library, cx| {
                library.toggle_favorite(
                    FavoriteKey::for_provider(provider, FavoriteKind::Track, track_id),
                    known_favorite,
                    cx,
                );
            });
        }
    }

    fn start_deezer_track_mix(&mut self, track_id: String, cx: &mut Context<Self>) {
        if let Some(library) = self.library.upgrade() {
            library.update(cx, |library, cx| {
                library.start_deezer_track_mix(track_id, cx)
            });
        }
    }

    fn start_deezer_artist_mix(&mut self, artist_id: String, cx: &mut Context<Self>) {
        if let Some(library) = self.library.upgrade() {
            library.update(cx, |library, cx| {
                library.start_deezer_artist_mix(artist_id, cx)
            });
        }
    }

    fn start_soundcloud_artist_station(&mut self, artist_id: String, cx: &mut Context<Self>) {
        if let Some(library) = self.library.upgrade() {
            library.update(cx, |library, cx| {
                library.start_soundcloud_artist_station(artist_id, cx)
            });
        }
    }

    fn start_soundcloud_track_station(&mut self, track: PlaybackTrack, cx: &mut Context<Self>) {
        if let Some(library) = self.library.upgrade() {
            library.update(cx, |library, cx| {
                library.start_soundcloud_track_station(track, cx)
            });
        }
    }

    fn add_negative_feedback(
        &mut self,
        kind: crate::library::DeezerFeedbackKind,
        id: String,
        cx: &mut Context<Self>,
    ) {
        if let Some(library) = self.library.upgrade() {
            library.update(cx, |library, cx| {
                library.add_negative_feedback(kind, id, cx)
            });
        }
    }

    fn open_similar_artists(
        &mut self,
        artist_id: String,
        title: String,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        if let Some(library) = self.library.upgrade() {
            library.update(cx, |library, cx| {
                library.open_similar_artists(artist_id, title, window, cx)
            });
        }
    }

    fn toggle_artist_favorite(
        &mut self,
        provider: Provider,
        artist_id: String,
        known_favorite: bool,
        cx: &mut Context<Self>,
    ) {
        if let Some(library) = self.library.upgrade() {
            library.update(cx, |library, cx| {
                library.toggle_favorite(
                    FavoriteKey::for_provider(provider, FavoriteKind::Artist, artist_id),
                    known_favorite,
                    cx,
                );
            });
        }
    }

    fn open_track_info(
        &mut self,
        provider: Provider,
        track_id: String,
        seed: crate::library::TrackInfo,
        deezer_arl: Option<crate::search::DeezerArl>,
        soundcloud_token: Option<crate::search::SoundCloudToken>,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        if let Some(library) = self.library.upgrade() {
            library.update(cx, |library, cx| {
                crate::entity_navigation::TrackMenuHost::open_track_info(
                    library,
                    provider,
                    track_id,
                    seed,
                    deezer_arl,
                    soundcloud_token,
                    window,
                    cx,
                );
            });
        }
    }

    fn preload_track_info(
        &mut self,
        provider: Provider,
        track_id: String,
        deezer_arl: Option<crate::search::DeezerArl>,
        soundcloud_token: Option<crate::search::SoundCloudToken>,
        _cx: &mut Context<Self>,
    ) {
        crate::library::preload_track_info(
            provider,
            track_id,
            deezer_arl,
            soundcloud_token,
            &self.runtime,
        );
    }
}

fn provider(provider: PlaybackProvider) -> Provider {
    match provider {
        PlaybackProvider::Deezer => Provider::Deezer,
        PlaybackProvider::SoundCloud => Provider::SoundCloud,
    }
}

fn empty_cache_state(loading: bool) -> impl IntoElement {
    let (title, description) = if loading {
        ("Loading cache", "Checking complete cached tracks.")
    } else {
        (
            "No cached tracks yet",
            "Fully cached tracks will appear here after they are played or prefetched.",
        )
    };
    div()
        .w_full()
        .py(px(60.))
        .flex()
        .flex_col()
        .items_center()
        .gap(px(7.))
        .child(local_icon(CACHE_ICON, 0x71717a).size(px(40.)))
        .child(
            div()
                .text_size(px(16.))
                .font_weight(FontWeight::SEMIBOLD)
                .text_color(rgb(FOREGROUND))
                .child(title),
        )
        .child(
            div()
                .text_size(px(13.))
                .text_color(rgb(MUTED))
                .child(description),
        )
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn cache_rows_keep_their_provider_badge() {
        assert_eq!(provider(PlaybackProvider::Deezer), Provider::Deezer);
        assert_eq!(provider(PlaybackProvider::SoundCloud), Provider::SoundCloud);
    }

    #[test]
    fn cache_icon_is_the_font_awesome_solid_hard_drive() {
        assert_eq!(
            CACHE_ICON.path(),
            "ralgrum/icons/fontawesome-free-7.3.1/solid/hard-drive.svg"
        );
    }
}
