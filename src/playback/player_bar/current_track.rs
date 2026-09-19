use super::*;

pub(super) fn current_favorite_key(track: Option<&PlaybackTrack>) -> Option<FavoriteKey> {
    track.map(|track| {
        FavoriteKey::for_provider(
            search_provider_for_playback(track.provider),
            FavoriteKind::Track,
            track.id.clone(),
        )
    })
}

pub(super) fn search_provider_for_playback(provider: PlaybackProvider) -> crate::search::Provider {
    match provider {
        PlaybackProvider::Deezer => crate::search::Provider::Deezer,
        PlaybackProvider::SoundCloud => crate::search::Provider::SoundCloud,
    }
}

pub(super) fn current_artist_navigation(
    track: &PlaybackTrack,
    status: PlaybackStatus,
    openers: Option<&ProviderNavigationOpeners>,
) -> Option<TrackArtistNavigation> {
    if !matches!(
        status,
        PlaybackStatus::Playing | PlaybackStatus::Paused | PlaybackStatus::Ended
    ) {
        return None;
    }
    let provider = search_provider_for_playback(track.provider);
    let (_, opener) = openers?.for_provider(provider);
    let routes = artist_routes_for_track(provider, &track.artists);
    (!routes.is_empty()).then_some(TrackArtistNavigation {
        provider,
        routes,
        opener,
    })
}

pub(super) fn download_available(track: Option<&PlaybackTrack>) -> bool {
    track.is_some()
}

pub(super) fn update_last_quality_label(
    last_quality_label: &mut String,
    resolved_quality: Option<&str>,
) {
    if let Some(quality) = resolved_quality.filter(|quality| !quality.is_empty()) {
        *last_quality_label = quality.to_owned();
    }
}

pub(super) fn update_quality_label_for_generation(
    last_label: &mut String,
    last_generation: &mut Option<u64>,
    generation: u64,
    resolved_quality: Option<&str>,
) {
    let generation_changed = *last_generation != Some(generation);
    *last_generation = Some(generation);
    if generation_changed {
        last_label.clear();
    }
    update_last_quality_label(last_label, resolved_quality);
}

/// Remembers the last artwork that finished loading so switching tracks keeps
/// the previous cover on screen until the new one is ready.
pub(super) struct ArtworkHold {
    cache: AnyImageCache,
    last_ready: Rc<RefCell<Option<Arc<RenderImage>>>>,
}

impl ArtworkHold {
    pub(super) fn new(cache: Entity<ArtworkCache>) -> Self {
        Self {
            cache: cache.into(),
            last_ready: Rc::default(),
        }
    }

    /// Forget the remembered cover so a stale one cannot outlive its playback
    /// session.
    pub(super) fn clear(&self) {
        self.last_ready.borrow_mut().take();
    }

    /// Build the player bar artwork source: the current track's cover once the
    /// artwork cache has it, and the remembered previous cover while it is
    /// still loading.
    #[allow(clippy::arc_with_non_send_sync)]
    fn source(&self, url: String) -> ImageSource {
        // ImageSource::Custom requires Arc even though GPUI invokes the
        // closure only on its single-threaded application executor.
        let cache = self.cache.clone();
        let last_ready = self.last_ready.clone();
        let resource = artwork_resource(url);
        ImageSource::Custom(Arc::new(move |window: &mut Window, cx: &mut App| {
            resolve_artwork(
                cache.load(&resource, window, cx),
                &mut last_ready.borrow_mut(),
            )
        }))
    }
}

/// Classify an artwork string exactly like `img(url)` does, so the held
/// artwork resolves through the same cache path as a plain image element.
pub(super) fn artwork_resource(url: String) -> Resource {
    match ImageSource::from(url) {
        ImageSource::Resource(resource) => resource,
        _ => unreachable!("string image sources always resolve to a resource"),
    }
}

/// Choose what the artwork element paints and update the remembered cover: a
/// ready artwork becomes the new memory, a still-loading one falls back to the
/// previous cover instead of an empty frame, and errors pass through so failed
/// covers keep showing the placeholder.
pub(super) fn resolve_artwork(
    current: Option<Result<Arc<RenderImage>, ImageCacheError>>,
    remembered: &mut Option<Arc<RenderImage>>,
) -> Option<Result<Arc<RenderImage>, ImageCacheError>> {
    match current {
        Some(Ok(image)) => {
            *remembered = Some(image.clone());
            Some(Ok(image))
        }
        Some(Err(error)) => Some(Err(error)),
        None => remembered.clone().map(Ok),
    }
}

#[allow(clippy::too_many_arguments)]
pub(super) fn render_current(
    track: Option<&PlaybackTrack>,
    artwork_hold: &ArtworkHold,
    status: PlaybackStatus,
    loading_from_cache: bool,
    desktop_layout: Option<PlayerBarLayout>,
    compact: bool,
    narrow: bool,
    favorite: Option<AnyElement>,
    width_animation: Option<(f32, f32, u64)>,
    artist_navigation: Option<TrackArtistNavigation>,
    text_width_visual: Option<ScalarMotionVisual>,
    favorite_visual: Option<FadeMotionVisual>,
    favorite_interactive: Option<Rc<Cell<bool>>>,
    window: &mut Window,
) -> AnyElement {
    if track.is_none() {
        // No track means playback cleared or the player closed; forget the
        // remembered cover so a stale one cannot return with the next session.
        artwork_hold.clear();
    }
    let block_width = desktop_layout.map_or(380., |layout| layout.side_width);
    let artwork = if narrow { 44. } else { 60. };
    let title = current_track_title(track, status);
    let subtitle = current_subtitle(track, status, loading_from_cache);
    let rendered_artist =
        rendered_current_artist_text(track, status, subtitle, artist_navigation.as_ref());
    // Fixed side widths only on desktop layouts; narrow rows stretch, so there
    // is no cheap width to compare against for overflow tooltips there. The
    // compact heart lives outside this row and does not reserve text width.
    let text_width = (!narrow).then(|| {
        current_text_available_width(
            desktop_layout
                .unwrap_or_else(|| PlayerBarLayout::from_geometry((0., 0., 0., block_width))),
            compact,
            favorite.is_some(),
        )
    });
    let text_block_width = text_width_visual.map(|visual| visual.target);
    let artist_line = if matches!(
        status,
        PlaybackStatus::Playing | PlaybackStatus::Paused | PlaybackStatus::Ended
    ) {
        track
            .map(|track| {
                crate::music_ui::track_artist_line(
                    0,
                    "player-bar",
                    &track.artist,
                    artist_navigation,
                    false,
                )
            })
            .unwrap_or_else(|| div().child(subtitle.to_owned()).into_any_element())
    } else {
        div().child(subtitle.to_owned()).into_any_element()
    };
    let overflow_tooltip =
        |el: gpui::Stateful<gpui::Div>, text: &str, fits: Option<bool>| match fits {
            Some(false) => {
                let text = text.to_owned();
                el.app_tooltip(text)
            }
            _ => el,
        };
    let text_block = div()
        .min_w_0()
        .flex()
        .flex_col()
        .gap(px(2.))
        .child(overflow_tooltip(
            div()
                .id("player-title")
                .truncate()
                .text_size(px(14.))
                .font_weight(FontWeight::SEMIBOLD)
                .text_color(rgb(FOREGROUND))
                .child(title.to_owned()),
            title,
            text_width.map(|width| text_fits(window, title, 14., FontWeight::SEMIBOLD, px(width))),
        ))
        .child(overflow_tooltip(
            div()
                .id("player-artist")
                .truncate()
                .text_size(px(12.))
                .text_color(rgb(MUTED))
                .child(artist_line),
            rendered_artist.as_str(),
            text_width.map(|width| {
                text_fits(
                    window,
                    rendered_artist.as_str(),
                    12.,
                    FontWeight::NORMAL,
                    px(width),
                )
            }),
        ));
    let text_block = if let Some(width) = text_block_width {
        let target_width = width;
        let from_width = text_width_visual
            .filter(|visual| visual.active)
            .map(|visual| visual.from);
        let initial_width = from_width.unwrap_or(target_width);
        let text_block = text_block
            .w(px(initial_width))
            .min_w(px(initial_width))
            .flex_none();
        if let (Some(visual), Some(from_width)) =
            (text_width_visual.filter(|visual| visual.active), from_width)
        {
            text_block
                .with_animation(
                    ("player-current-text-layout", visual.epoch),
                    crate::motion::panel(),
                    move |this, delta| {
                        let width = crate::motion::lerp(from_width, target_width, delta);
                        this.w(px(width)).min_w(px(width))
                    },
                )
                .into_any_element()
        } else {
            text_block.into_any_element()
        }
    } else if narrow || compact {
        text_block.flex_1().into_any_element()
    } else {
        text_block.flex_initial().into_any_element()
    };
    let current = div()
        .relative()
        .when(narrow, |this| this.w_full())
        .when(!narrow, |this| this.w(px(block_width)))
        .min_w_0()
        .flex()
        .items_center()
        .gap(px(12.))
        .child(
            div()
                .size(px(artwork))
                .flex_none()
                .overflow_hidden()
                .rounded(px(6.))
                .bg(rgb(SURFACE))
                .when_some(
                    track.filter(|track| !track.artwork.is_empty()),
                    |this, track| {
                        this.child(
                            img(artwork_hold.source(track.artwork.clone()))
                                .id("player-bar-artwork")
                                .size_full()
                                .rounded(px(6.))
                                .object_fit(ObjectFit::Cover),
                        )
                    },
                ),
        )
        .child(text_block);
    let current = if narrow {
        if let Some(interactive) = favorite_interactive {
            interactive.set(true);
        }
        current.children(favorite)
    } else if let (Some(visual), Some(button)) = (favorite_visual, favorite) {
        let favorite = div()
            .id("player-favorite-layout")
            .absolute()
            .top(px(visual.target.top))
            .left(px(visual.target.left))
            .size(px(34.))
            .child(button);
        let favorite: AnyElement = if visual.active {
            let interactive =
                favorite_interactive.expect("favorite fade should have an interaction gate");
            favorite
                .top(px(visual.from.top))
                .left(px(visual.from.left))
                .opacity(visual.from_opacity)
                .with_animation(
                    ("player-favorite-layout", visual.epoch),
                    crate::motion::relocation_fade(),
                    move |this, delta| {
                        let delta = visual.started_at.map_or(delta, |started_at| {
                            fade_motion_progress(started_at, Instant::now())
                        });
                        interactive.set(delta >= 1.);
                        let geometry = fade_motion_geometry(visual.from, visual.target, delta);
                        this.left(px(geometry.left)).top(px(geometry.top)).opacity(
                            fade_motion_opacity(visual.from_opacity, visual.target_opacity, delta),
                        )
                    },
                )
                .into_any_element()
        } else {
            if let Some(interactive) = favorite_interactive {
                interactive.set(true);
            }
            favorite.into_any_element()
        };
        current.child(favorite)
    } else {
        current
    };
    if let Some((from_width, target_width, epoch)) = width_animation {
        current
            .with_animation(
                ("player-current-width", epoch),
                crate::motion::panel(),
                move |this, delta| {
                    let width = crate::motion::lerp(from_width, target_width, delta);
                    this.w(px(width)).min_w(px(width))
                },
            )
            .into_any_element()
    } else {
        current.into_any_element()
    }
}

pub(super) fn current_subtitle(
    track: Option<&PlaybackTrack>,
    status: PlaybackStatus,
    loading_from_cache: bool,
) -> &str {
    match status {
        PlaybackStatus::Loading if !loading_from_cache => "Loading audio...",
        PlaybackStatus::Loading => track.map_or("", |track| track.artist.as_str()),
        PlaybackStatus::Failed => "Playback failed",
        _ => track.map_or("", |track| track.artist.as_str()),
    }
}

pub(super) fn current_track_title(track: Option<&PlaybackTrack>, status: PlaybackStatus) -> &str {
    match (track, status) {
        (Some(track), _) => track.title.as_str(),
        // A pending source load has no track yet; say what is happening
        // instead of claiming nothing is playing.
        (None, PlaybackStatus::Loading) => "Loading...",
        (None, _) => "Nothing playing",
    }
}

pub(super) fn rendered_current_artist_text(
    track: Option<&PlaybackTrack>,
    status: PlaybackStatus,
    subtitle: &str,
    navigation: Option<&TrackArtistNavigation>,
) -> String {
    if !matches!(
        status,
        PlaybackStatus::Playing | PlaybackStatus::Paused | PlaybackStatus::Ended
    ) {
        return subtitle.to_owned();
    }
    let Some(track) = track else {
        return subtitle.to_owned();
    };
    let Some(navigation) = navigation else {
        return track.artist.clone();
    };
    rendered_artist_text(&track.artist, navigation.provider, &navigation.routes)
}

pub(super) fn rendered_artist_text(
    artist: &str,
    provider: crate::search::Provider,
    routes: &[crate::entity_navigation::MenuRoute],
) -> String {
    if provider == crate::search::Provider::SoundCloud && routes.len() == 1 {
        artist.to_owned()
    } else {
        routes
            .iter()
            .map(|route| route.title.as_str())
            .collect::<Vec<_>>()
            .join(", ")
    }
}

pub(super) fn current_text_available_width(
    layout: PlayerBarLayout,
    compact: bool,
    has_favorite: bool,
) -> f32 {
    let favorite_reservation = if !compact && has_favorite {
        34. + 12.
    } else {
        0.
    };
    (layout.side_width - 60. - 12. - favorite_reservation).max(0.)
}

pub(super) fn current_text_intrinsic_width(window: &Window, title: &str, artist: &str) -> f32 {
    text_measure_width(window, title, 14., FontWeight::SEMIBOLD).max(text_measure_width(
        window,
        artist,
        12.,
        FontWeight::NORMAL,
    ))
}

pub(super) fn current_text_width_for_layout(
    layout: PlayerBarLayout,
    compact: bool,
    has_favorite: bool,
    intrinsic_width: f32,
) -> f32 {
    let available = current_text_available_width(layout, compact, has_favorite);
    if compact {
        available
    } else {
        intrinsic_width.max(0.).min(available)
    }
}

pub(super) fn favorite_top(compact: bool) -> f32 {
    if compact { 2. } else { 13. }
}

pub(super) fn favorite_left_for_layout(
    layout: PlayerBarLayout,
    compact: bool,
    intrinsic_width: f32,
) -> f32 {
    let text_width = current_text_width_for_layout(layout, compact, true, intrinsic_width);
    favorite_left_for_text_width(layout, compact, text_width)
}

pub(super) fn favorite_left_for_text_width(
    layout: PlayerBarLayout,
    compact: bool,
    text_width: f32,
) -> f32 {
    if compact {
        (layout.side_width + layout.column_gap - 1.).max(0.)
    } else {
        60. + 12. + text_width.max(0.) + 12.
    }
}

pub(super) fn text_measure_width(
    window: &Window,
    text: &str,
    font_size: f32,
    weight: FontWeight,
) -> f32 {
    if text.is_empty() {
        return 0.;
    }
    let font = Font {
        family: ui_font_family().into(),
        weight,
        ..Font::default()
    };
    let run = TextRun {
        len: text.len(),
        font,
        color: Hsla::default(),
        background_color: None,
        underline: None,
        strikethrough: None,
    };
    window
        .text_system()
        .shape_line(text.into(), px(font_size), &[run], None)
        .width
        .into()
}

/// Cheap single-line text measurement against an available width, used to
/// attach tooltips only to overflowing titles and artists.
pub(super) fn text_fits(
    window: &Window,
    text: &str,
    font_size: f32,
    weight: FontWeight,
    available: Pixels,
) -> bool {
    if text.is_empty() {
        return true;
    }
    px(text_measure_width(window, text, font_size, weight)) <= available
}
