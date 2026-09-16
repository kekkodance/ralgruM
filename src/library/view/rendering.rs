use super::*;

pub(super) fn settings_category_for_service(_: Service) -> SettingsCategory {
    SettingsCategory::Providers
}

pub(super) fn track_favorites_available_for_route(route: &Route) -> bool {
    !route.is_local_route()
        && matches!(
            route.source,
            crate::search::Provider::Deezer | crate::search::Provider::SoundCloud
        )
}

pub(super) fn local_tracks_page(tracks: &[crate::library::local_store::LocalTrack]) -> Page {
    let tracks = tracks.iter().map(Track::from).collect::<Vec<_>>();
    Page {
        title: "Local Tracks".into(),
        description: "Tracks saved to your local library from Deezer and SoundCloud.".into(),
        platform: Some(Service::Local),
        count_noun: "track".into(),
        total: tracks.len(),
        raw_loaded_count: tracks.len(),
        normalized_count: tracks.len(),
        authoritative_total: Some(tracks.len()),
        tracks,
        empty_title: "No local tracks yet".into(),
        empty_description: "Tracks you save to your local library will appear here. Saving a track keeps a reference, not an audio file.".into(),
        ..Page::default()
    }
}

fn playback_provider(track: &crate::playback::PlaybackTrack) -> Provider {
    match track.provider {
        crate::playback::PlaybackProvider::Deezer => Provider::Deezer,
        crate::playback::PlaybackProvider::SoundCloud => Provider::SoundCloud,
    }
}

impl ResizeSettledTarget for LibraryView {
    fn commit_resize(&mut self, request: crate::music_ui::ResizeRequest) -> bool {
        self.card_columns.commit(request)
    }
}

pub(super) fn library_content_scroll(
    content: gpui::AnyElement,
    contained: bool,
    scroll: &ScrollHandle,
    browser_scroll: BrowserScrollState,
) -> gpui::AnyElement {
    if contained {
        return div()
            .id("library-content")
            .flex_1()
            .min_h_0()
            .overflow_hidden()
            .child(content)
            .into_any_element();
    }
    let scroll_content = div()
        .id("library-content-scroll")
        .flex_1()
        .min_h_0()
        .track_scroll(scroll)
        .overflow_y_scroll()
        .vertical_scrollbar(scroll)
        .child(content)
        .into_any_element();
    browser_scroll_surface(
        "library-content",
        scroll_content,
        BrowserScrollTarget::Handle(scroll.clone()),
        browser_scroll,
    )
}

pub(super) fn library_content_transition_identity(state: &LibraryState) -> String {
    if matches!(state.status, crate::library::state::Status::AccountRequired) {
        return format!(
            "library-content-body:{}:account-required",
            state.service.label()
        );
    }

    let route = state.route();
    if state.routes.len() == 1
        && matches!(
            state.category,
            Category::Albums | Category::Artists | Category::Playlists
        )
    {
        return format!("library-content-body:{}:root-cards", state.service.label());
    }

    format!(
        "library-content-body:{}:{}:{}:{}",
        state.service.label(),
        state.category.label(),
        route.action,
        route.id
    )
}

impl Render for LibraryView {
    fn render(&mut self, window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let now = Instant::now();
        let metrics = crate::music_ui::shell_metrics(f32::from(window.viewport_size().width));
        let right_sidebar_open =
            self.playback.read(cx).state.right_sidebar != crate::playback::RightSidebar::Closed;
        let available_width = effective_content_width(&metrics, right_sidebar_open);
        self.card_available_width = available_width;
        let available_height = f32::from(window.viewport_size().height);
        if let Some(request) = self.card_columns.observe_width(available_width) {
            crate::music_ui::schedule_resize(cx, window, request);
        }
        let columns = self.card_columns.columns();
        self.card_grid_motion
            .prepare(columns, available_width, now, cx.reduce_motion());
        let narrow = metrics.narrow_content;
        let small_page_heading = f32::from(window.viewport_size().width)
            <= crate::library::content_view::PAGE_HEADING_SMALL_MAX_VIEWPORT;
        let gutter = crate::music_ui::main_content_inset(&metrics);
        let categories = self.state.service.categories();
        let selected_category_index = categories
            .iter()
            .position(|category| *category == self.state.category)
            .unwrap_or_default();
        let category_visual = self.category_motion.prepare(
            selected_category_index,
            categories.len(),
            now,
            cx.reduce_motion(),
        );
        let category_labels = self
            .state
            .service
            .categories()
            .iter()
            .map(|category| category.label())
            .collect::<Vec<_>>();
        let use_category_tab_icons = category_tabs_icon_only(available_width, &category_labels);
        let category_responsive =
            self.category_tabs_responsive
                .prepare(use_category_tab_icons, now, cx.reduce_motion());
        let detail_open = self.detail_open();
        div()
            .size_full()
            .flex()
            .flex_col()
            .gap(px(14.))
            .child(
                div()
                    .px(px(gutter))
                    .flex()
                    .flex_col()
                    .gap(px(10.))
                    .child(crate::library::content_view::render_main_header(
                        self,
                        small_page_heading,
                        cx,
                    ))
                    .when(!detail_open, |this| {
                        this.child(
                            div()
                                .id("library-category-tabs")
                                .role(gpui::Role::TabList)
                                .aria_label("Library category")
                                .w_full()
                                .h(px(38.))
                                .flex()
                                .gap(px(2.))
                                .p(px(3.))
                                .rounded(px(8.))
                                .border_1()
                                .border_color(rgb(BORDER))
                                .bg(rgb(SURFACE_RAISED))
                                .child(
                                    div()
                                        .relative()
                                        .flex()
                                        .flex_1()
                                        .min_w_0()
                                        .items_center()
                                        .when(!use_category_tab_icons, |this| {
                                            this.child(crate::motion::segmented_selector_indicator(
                                                category_visual,
                                            ))
                                        })
                                        .children(categories.iter().copied().enumerate().map(
                                            |(index, category)| {
                                                self.tab(category, index, category_responsive, cx)
                                            },
                                        )),
                                ),
                        )
                    }),
            )
            .child({
                let similar_open = self.similar_artists.is_open();
                let similar_virtualized =
                    self.similar_artists.current.as_ref().is_some_and(|entry| {
                        matches!(
                            &entry.state,
                            crate::library::deezer_similar::SimilarArtistsState::Results(cards)
                                if !cards.is_empty()
                        )
                    });
                let virtualized = if similar_open {
                    similar_virtualized
                } else {
                    crate::library::content_view::uses_virtualized_scroll(self, cx)
                };
                let loading = !similar_open
                    && matches!(self.state.status, crate::library::state::Status::Loading);
                let contained = virtualized || loading;
                let body = if similar_open {
                    crate::library::deezer_similar::render(
                        self,
                        &cx.entity(),
                        &self.similar_artists,
                        columns,
                        narrow,
                        cx,
                    )
                } else {
                    crate::library::content_view::render_content(
                        self,
                        columns,
                        available_width,
                        available_height,
                        narrow,
                        cx,
                    )
                };
                let content = div()
                    .w_full()
                    .px(px(gutter))
                    .when(contained, |this| {
                        this.size_full().flex().flex_col().min_h_0()
                    })
                    // Breathing room above the player bar. Padding lives on
                    // the inner content so it only shows at the tail for div
                    // scrolls. Virtualized lists own their scroll state, so
                    // they must not get a persistent outer gap.
                    .when(!contained, |this| {
                        this.pb(px(LIBRARY_CONTENT_BOTTOM_PADDING_PX))
                    })
                    .child(body);
                let content = if similar_open {
                    content.into_any_element()
                } else {
                    let content_identity = library_content_transition_identity(&self.state);
                    content
                        .relative()
                        .with_animation(
                            content_identity,
                            crate::motion::quick_content(),
                            |this, delta| {
                                this.opacity(crate::motion::lerp(0.0, 1.0, delta))
                                    .top(px(crate::motion::lerp(4.0, 0.0, delta)))
                            },
                        )
                        .into_any_element()
                };
                library_content_scroll(
                    content,
                    contained,
                    &self.scroll,
                    self.browser_scroll.clone(),
                )
            })
    }
}

pub(super) fn reset_list_state_to_top(state: &ListState) {
    state.scroll_to(ListOffset::default());
}

impl crate::entity_navigation::TrackMenuHost for LibraryView {
    fn favorites_entity(&self) -> Entity<FavoriteState> {
        self.favorites.clone()
    }

    fn local_track_saved(&self, track: &crate::playback::PlaybackTrack, _cx: &gpui::App) -> bool {
        let provider = playback_provider(track);
        self.local_store.as_ref().is_ok_and(|store| {
            store
                .tracks()
                .iter()
                .any(|saved| saved.provider == provider && saved.id == track.id)
        })
    }

    fn set_local_track_saved(
        &mut self,
        track: crate::playback::PlaybackTrack,
        saved: bool,
        cx: &mut Context<Self>,
    ) {
        let title = track.title.clone();
        let result = self.enqueue_local_library_mutation(
            LocalLibraryMutation::SetSaved {
                track: Box::new(crate::library::local_store::LocalTrack::from(&track)),
                saved,
            },
            Box::new(move |result, _, cx| match result {
                Ok(LocalLibraryMutationOutcome::Saved)
                | Ok(LocalLibraryMutationOutcome::Removed) => {
                    crate::toast::push_global(
                        cx,
                        crate::toast::ToastKind::Success,
                        if saved {
                            "Saved to local"
                        } else {
                            "Removed from local"
                        },
                        Some(title.into()),
                    );
                }
                Err(error) => crate::toast::push_global(
                    cx,
                    crate::toast::ToastKind::Error,
                    "Local library could not be updated",
                    Some(error.to_string().into()),
                ),
                Ok(_) => {}
            }),
            cx,
        );
        if let Err(error) = result {
            crate::toast::push_global(
                cx,
                crate::toast::ToastKind::Error,
                "Local library could not be updated",
                Some(error.to_string().into()),
            );
        }
    }

    fn resolve_favorite_state(&mut self, key: FavoriteKey, cx: &mut Context<Self>) {
        LibraryView::resolve_favorite_state(self, key, cx);
    }

    fn open_playlist_picker(
        &mut self,
        track_id: String,
        provider: crate::search::Provider,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        let scope = self.add_status_scope();
        self.open_add_picker(vec![track_id], scope, provider, window, cx)
    }

    fn open_local_playlist_picker(
        &mut self,
        track: crate::playback::PlaybackTrack,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        LibraryView::open_local_playlist_picker(self, track, window, cx);
    }

    fn preload_playlist_catalog(
        &mut self,
        provider: crate::search::Provider,
        cx: &mut Context<Self>,
    ) {
        match provider {
            crate::search::Provider::Deezer => {
                self.ensure_playlist_catalog(cx);
            }
            crate::search::Provider::SoundCloud => {
                self.ensure_soundcloud_playlist_catalog(cx);
            }
        }
    }

    fn toggle_favorite_state(
        &mut self,
        provider: crate::search::Provider,
        track_id: String,
        known_favorite: bool,
        cx: &mut Context<Self>,
    ) {
        use crate::library::favorite_state::{FavoriteKey, FavoriteKind};
        self.toggle_favorite(
            FavoriteKey::for_provider(provider, FavoriteKind::Track, track_id),
            known_favorite,
            cx,
        )
    }

    fn start_deezer_track_mix(&mut self, track_id: String, cx: &mut Context<Self>) {
        self.start_deezer_track_mix(track_id, cx);
    }

    fn start_deezer_artist_mix(&mut self, artist_id: String, cx: &mut Context<Self>) {
        self.start_deezer_artist_mix(artist_id, cx);
    }

    fn start_soundcloud_artist_station(&mut self, artist_id: String, cx: &mut Context<Self>) {
        self.start_soundcloud_artist_station(artist_id, cx);
    }

    fn start_soundcloud_track_station(
        &mut self,
        track: crate::playback::PlaybackTrack,
        cx: &mut Context<Self>,
    ) {
        self.start_soundcloud_track_station(track, cx);
    }

    fn add_negative_feedback(
        &mut self,
        kind: crate::library::DeezerFeedbackKind,
        id: String,
        cx: &mut Context<Self>,
    ) {
        self.add_negative_feedback(kind, id, cx);
    }

    fn toggle_artist_favorite(
        &mut self,
        provider: crate::search::Provider,
        artist_id: String,
        known_favorite: bool,
        cx: &mut Context<Self>,
    ) {
        self.toggle_favorite(
            crate::library::FavoriteKey::for_provider(
                provider,
                crate::library::FavoriteKind::Artist,
                artist_id,
            ),
            known_favorite,
            cx,
        );
    }

    fn open_similar_artists(
        &mut self,
        artist_id: String,
        title: String,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        self.open_similar_artists(artist_id, title, window, cx);
    }

    fn open_track_info(
        &mut self,
        provider: crate::search::Provider,
        track_id: String,
        seed: crate::library::TrackInfo,
        deezer_arl: Option<crate::search::DeezerArl>,
        soundcloud_token: Option<crate::search::SoundCloudToken>,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        let runtime = self.runtime.clone();
        crate::library::open_track_info_dialog(
            provider,
            track_id,
            seed,
            deezer_arl,
            soundcloud_token,
            runtime,
            window,
            cx,
        );
    }

    fn preload_track_info(
        &mut self,
        provider: crate::search::Provider,
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
