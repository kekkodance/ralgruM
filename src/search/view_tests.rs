use gpui::px;

use super::*;

fn favorite_track(provider: Provider, id: &str, favorite: Option<bool>) -> Track {
    Track {
        id: id.into(),
        source: provider,
        favorite,
        ..Track::default()
    }
}

#[test]
fn collection_favorites_support_captured_deezer_and_soundcloud_entities() {
    assert_eq!(
        favorite_kind(Provider::Deezer, ResultType::Albums, "42"),
        Some(FavoriteKind::Album)
    );
    assert_eq!(
        favorite_kind(Provider::SoundCloud, ResultType::Albums, "42"),
        Some(FavoriteKind::Album)
    );
    assert_eq!(
        favorite_kind(Provider::SoundCloud, ResultType::Artists, "42"),
        Some(FavoriteKind::Artist)
    );
    assert_eq!(
        favorite_kind(Provider::SoundCloud, ResultType::Playlists, "42"),
        Some(FavoriteKind::Playlist)
    );
    assert!(favorite_kind(Provider::Deezer, ResultType::Albums, "").is_none());
    assert!(favorite_kind(Provider::Deezer, ResultType::Tracks, "42").is_none());
    assert!(favorite_kind(Provider::SoundCloud, ResultType::Tracks, "42").is_none());
}

#[test]
fn detail_favorite_seeds_extract_artist_and_track_membership() {
    let result = Ok(super::super::detail::DetailPage {
        route: super::super::detail::DetailRoute {
            provider: Provider::Deezer,
            kind: ResultType::Artists,
            id: "artist-1".into(),
            ..super::super::detail::DetailRoute {
                provider: Provider::Deezer,
                kind: ResultType::Artists,
                id: String::new(),
                title: String::new(),
                subtitle: String::new(),
                artwork: String::new(),
                release_date: String::new(),
                service_url: String::new(),
            }
        },
        tracks: vec![favorite_track(Provider::Deezer, "page-track", Some(true))],
        total: None,
        raw_loaded_count: 0,
        normalized_count: 0,
        authoritative_total: None,
        artist: Some(super::super::models::ArtistPage {
            favorite: Some(true),
            popular_tracks: vec![favorite_track(
                Provider::Deezer,
                "popular-track",
                Some(false),
            )],
            ..super::super::models::ArtistPage::default()
        }),
        description: String::new(),
        album_info: None,
    });

    let seeds = detail_favorite_seeds(&result);

    assert!(seeds.contains(&(
        FavoriteKey::for_provider(Provider::Deezer, FavoriteKind::Artist, "artist-1".into()),
        true
    )));
    assert!(seeds.contains(&(
        FavoriteKey::for_provider(Provider::Deezer, FavoriteKind::Track, "page-track".into()),
        true
    )));
    assert!(seeds.contains(&(
        FavoriteKey::for_provider(
            Provider::Deezer,
            FavoriteKind::Track,
            "popular-track".into()
        ),
        false
    )));
    assert_eq!(seeds.len(), 3);
}

#[test]
fn detail_favorite_seeds_ignore_errors_unknown_values_and_empty_ids() {
    let error = Err(super::super::models::ProviderError::new("failed"));
    assert!(detail_favorite_seeds(&error).is_empty());

    let result = Ok(super::super::detail::DetailPage {
        route: super::super::detail::DetailRoute {
            provider: Provider::SoundCloud,
            kind: ResultType::Albums,
            id: "42".into(),
            title: String::new(),
            subtitle: String::new(),
            artwork: String::new(),
            release_date: String::new(),
            service_url: String::new(),
        },
        tracks: vec![favorite_track(Provider::Deezer, "", None)],
        total: None,
        raw_loaded_count: 0,
        normalized_count: 0,
        authoritative_total: None,
        artist: None,
        description: String::new(),
        album_info: None,
    });
    assert!(detail_favorite_seeds(&result).is_empty());
}

#[test]
fn discover_credential_gate_covers_each_provider_and_all() {
    assert!(discover_provider_has_credentials(
        Provider::Deezer,
        true,
        false
    ));
    assert!(!discover_provider_has_credentials(
        Provider::Deezer,
        false,
        true
    ));
    assert!(discover_provider_has_credentials(
        Provider::SoundCloud,
        false,
        true
    ));
    assert!(!discover_provider_has_credentials(
        Provider::SoundCloud,
        true,
        false
    ));

    assert_eq!(
        Source::All
            .providers()
            .iter()
            .map(|provider| discover_provider_has_credentials(*provider, true, true))
            .collect::<Vec<_>>(),
        vec![true, true]
    );
    assert_eq!(
        Source::All
            .providers()
            .iter()
            .map(|provider| discover_provider_has_credentials(*provider, false, false))
            .collect::<Vec<_>>(),
        vec![false, false]
    );
}

#[test]
fn all_search_missing_accounts_reports_only_soundcloud() {
    assert!(all_search_missing_accounts(Source::All, true).is_empty());
    assert_eq!(
        all_search_missing_accounts(Source::All, false),
        vec![Provider::SoundCloud]
    );
    assert!(all_search_missing_accounts(Source::Deezer, false).is_empty());
    assert!(all_search_missing_accounts(Source::SoundCloud, false).is_empty());
}

#[test]
fn forward_detail_keeps_previous_list_offset_and_new_state_starts_at_top() {
    let previous = ListState::new(8, ListAlignment::Top, px(8.));
    previous.scroll_to(gpui::ListOffset {
        item_ix: 4,
        offset_in_item: px(3.),
    });
    let current = ListState::new(8, ListAlignment::Top, px(8.));

    let previous_offset = previous.logical_scroll_top();
    assert_eq!(previous_offset.item_ix, 4);
    assert_eq!(previous_offset.offset_in_item, px(3.));
    let current_offset = current.logical_scroll_top();
    assert_eq!(current_offset.item_ix, 0);
    assert_eq!(current_offset.offset_in_item, px(0.));
}

#[test]
fn detail_back_preserves_the_dedicated_card_grid_offset() {
    let card = Card {
        kind: ResultType::Albums,
        id: "42".into(),
        source: Provider::Deezer,
        ..Card::default()
    };
    let search_offset = point(px(0.), px(-320.));
    let mut navigation = DetailNavigation::default();

    assert!(
        navigation
            .open_with_scroll(&card, true, search_offset)
            .is_some()
    );
    assert_eq!(
        navigation.back_with_scroll(point(px(0.), px(0.))),
        search_offset
    );
}

#[test]
fn channel_detail_back_restores_channel_identity_and_scroll_before_closing_channel() {
    let mut discover = DiscoverState::new("scope".into());
    let (generation, account_scope) = discover
        .start_channel_with_title("dance".into(), "Dance & EDM".into())
        .unwrap();
    assert!(discover.complete_channel(
        generation,
        &account_scope,
        "dance",
        Ok(("Dance & EDM".into(), Vec::new())),
    ));

    let card = Card {
        kind: ResultType::Albums,
        id: "42".into(),
        source: Provider::Deezer,
        ..Card::default()
    };
    let channel_scroll = point(px(0.), px(-240.));
    let mut navigation = DetailNavigation::default();
    assert!(
        navigation
            .open_with_scroll(&card, true, channel_scroll)
            .is_some()
    );
    assert!(!should_close_discover_channel_before_detail(
        true,
        discover.channel_open()
    ));
    assert_eq!(
        search_navigation_back_target(true, discover.channel_open()),
        SearchNavigationBackTarget::Detail
    );
    assert_eq!(
        navigation.back_with_scroll(point(px(0.), px(0.))),
        channel_scroll
    );
    assert!(discover.channel_open());
    assert_eq!(discover.channel().slug, "dance");

    assert_eq!(
        search_navigation_back_target(false, discover.channel_open()),
        SearchNavigationBackTarget::DiscoverChannel
    );
    assert!(discover.close_channel());
    assert!(!discover.channel_open());
}

#[test]
fn submitted_search_results_expose_a_root_navigation_target_only_at_the_root() {
    assert!(search_results_root_visible(
        "ambient",
        &ResultState::Results,
        false,
        false
    ));
    assert!(search_results_root_visible(
        "ambient",
        &ResultState::Loading,
        false,
        false
    ));
    assert!(search_results_root_visible(
        "ambient",
        &ResultState::Empty,
        false,
        false
    ));
    assert!(!search_results_root_visible(
        "ambient",
        &ResultState::Initial,
        false,
        false
    ));
    assert!(!search_results_root_visible(
        "",
        &ResultState::Results,
        false,
        false
    ));
    assert!(!search_results_root_visible(
        "ambient",
        &ResultState::Results,
        true,
        false
    ));
    assert!(!search_results_root_visible(
        "ambient",
        &ResultState::Results,
        false,
        true
    ));
}

#[test]
fn discover_feed_content_change_resets_the_fresh_feed_to_the_top() {
    let state = ListState::new(8, ListAlignment::Top, px(8.)).measure_all();
    state.scroll_to(gpui::ListOffset {
        item_ix: 4,
        offset_in_item: px(3.),
    });
    let mut cache = DiscoverFeedCache {
        state: state.clone(),
        browser_scroll: BrowserScrollState::new(),
        content_identity: "feed-a".to_owned(),
        item_count: 8,
    };

    // Same content: the cache leaves the measured state and scroll alone.
    update_discover_feed_cache(&mut cache, "feed-a", 8);
    assert_eq!(state.logical_scroll_top().item_ix, 4);
    assert_eq!(f32::from(state.logical_scroll_top().offset_in_item), 3.);

    // A content change resets to the top with the new item count, and
    // the reset re-arms the full measure pass for the next prepaint.
    update_discover_feed_cache(&mut cache, "feed-b", 12);
    assert_eq!(state.item_count(), 12);
    assert_eq!(cache.item_count, 12);
    assert_eq!(state.logical_scroll_top().item_ix, 0);
    assert_eq!(f32::from(state.logical_scroll_top().offset_in_item), 0.);
}

#[test]
fn discover_feed_scroll_offset_uses_the_list_state_pixel_position() {
    let state = ListState::new(8, ListAlignment::Top, px(8.)).with_uniform_item_height(px(100.));
    state.scroll_to(gpui::ListOffset {
        item_ix: 3,
        offset_in_item: px(7.),
    });

    assert_eq!(
        state.scroll_px_offset_for_scrollbar(),
        point(px(0.), px(-307.))
    );
}

#[gpui::test]
fn discover_feed_measured_rows_keep_extent_and_scroll_across_resizes(
    cx: &mut gpui::TestAppContext,
) {
    use gpui::prelude::*;

    // Mixed natural row heights that also change with the width, like a
    // feed where one-line genre sections sit between two-line sections
    // and rows shrink at the narrow breakpoint.
    const TALL_ROWS: [f32; 10] = [50., 32., 50., 32., 50., 32., 50., 32., 50., 32.];
    const SHORT_ROWS: [f32; 10] = [40., 26., 40., 26., 40., 26., 40., 26., 40., 26.];
    struct FeedRows(ListState, [f32; 10]);
    impl gpui::Render for FeedRows {
        fn render(
            &mut self,
            _: &mut gpui::Window,
            _: &mut gpui::Context<Self>,
        ) -> impl gpui::IntoElement {
            let state = self.0.clone();
            let row_heights = self.1;
            gpui::div().size_full().child(
                gpui::list(state, move |ix, _, _| {
                    gpui::div()
                        .h(px(row_heights[ix]))
                        .w_full()
                        .into_any_element()
                })
                .w_full()
                .h_full(),
            )
        }
    }

    let state = ListState::new(10, ListAlignment::Top, px(0.)).measure_all();
    let mut cache = DiscoverFeedCache {
        state: state.clone(),
        browser_scroll: BrowserScrollState::new(),
        content_identity: "feed-a".to_owned(),
        item_count: 10,
    };
    let cx = cx.add_empty_window();
    let view = cx.update(|_, cx| cx.new(|_| FeedRows(state.clone(), TALL_ROWS)));
    let draw = |cx: &mut gpui::VisualTestContext, width: f32, row_heights: [f32; 10]| {
        cx.update(|_, cx| {
            view.update(cx, |rows, _| rows.1 = row_heights);
        });
        cx.draw(
            gpui::point(px(0.), px(0.)),
            gpui::size(px(width), px(200.)),
            {
                let view = view.clone();
                move |_, _| view.into_any_element()
            },
        );
    };

    // Every row is measured in the first prepaint, so the extent is
    // exact from the first frame even with mixed heights.
    let tall_total: f32 = TALL_ROWS.iter().sum();
    draw(cx, 100., TALL_ROWS);
    assert_eq!(
        f32::from(state.max_offset_for_scrollbar().y),
        tall_total - 200.
    );
    assert_eq!(f32::from(state.scroll_px_offset_for_scrollbar().y), 0.);

    let tall_top_of_item_2: f32 = TALL_ROWS.iter().take(2).sum();
    state.scroll_to(gpui::ListOffset {
        item_ix: 2,
        offset_in_item: px(0.),
    });
    draw(cx, 100., TALL_ROWS);
    assert_eq!(
        f32::from(state.scroll_px_offset_for_scrollbar().y),
        -tall_top_of_item_2
    );

    // A width change wipes every measured height and re-arms the
    // measure pass within the same prepaint, so the extent is exact at
    // the new widths and heights immediately, with no collapsed
    // scrollbar frame in between. The item-anchored scroll position
    // survives and re-resolves against the new heights.
    let short_total: f32 = SHORT_ROWS.iter().sum();
    let short_top_of_item_2: f32 = SHORT_ROWS.iter().take(2).sum();
    draw(cx, 200., SHORT_ROWS);
    assert_eq!(
        f32::from(state.max_offset_for_scrollbar().y),
        short_total - 200.
    );
    assert_eq!(
        f32::from(state.scroll_px_offset_for_scrollbar().y),
        -short_top_of_item_2
    );
    assert_eq!(state.logical_scroll_top().item_ix, 2);
    assert_eq!(f32::from(state.logical_scroll_top().offset_in_item), 0.);

    // A content change resets the feed to the top and the next
    // prepaint re-measures every row, so scrolling keeps working.
    update_discover_feed_cache(&mut cache, "feed-b", 10);
    draw(cx, 200., SHORT_ROWS);
    assert_eq!(
        f32::from(state.max_offset_for_scrollbar().y),
        short_total - 200.
    );
    assert_eq!(f32::from(state.scroll_px_offset_for_scrollbar().y), 0.);
    assert_eq!(state.logical_scroll_top().item_ix, 0);
    state.scroll_by(px(100.));
    draw(cx, 200., SHORT_ROWS);
    assert_eq!(f32::from(state.scroll_px_offset_for_scrollbar().y), -100.);
}

#[test]
fn card_cache_identity_includes_account_source_type_query_and_section() {
    let first = search_card_grid_cache_key(
        "account-a",
        Source::Deezer,
        ResultType::Albums,
        "one",
        "Albums",
    );
    assert_eq!(
        first,
        search_card_grid_cache_key(
            "account-a",
            Source::Deezer,
            ResultType::Albums,
            "one",
            "Albums"
        )
    );
    for (scope, source, result_type, query, section) in [
        (
            "account-b",
            Source::Deezer,
            ResultType::Albums,
            "one",
            "Albums",
        ),
        (
            "account-a",
            Source::SoundCloud,
            ResultType::Albums,
            "one",
            "Albums",
        ),
        (
            "account-a",
            Source::Deezer,
            ResultType::Artists,
            "one",
            "Albums",
        ),
        (
            "account-a",
            Source::Deezer,
            ResultType::Albums,
            "two",
            "Albums",
        ),
        (
            "account-a",
            Source::Deezer,
            ResultType::Albums,
            "one",
            "Artists",
        ),
    ] {
        assert_ne!(
            first,
            search_card_grid_cache_key(scope, source, result_type, query, section)
        );
    }
}

#[test]
fn card_cache_invalidates_content_and_layout_signatures() {
    let layout = CardGridLayout::new(4, 800., 194., 260.);
    let cache = CardGridCache {
        state: ListState::new(3, ListAlignment::Top, px(520.)),
        browser_scroll: BrowserScrollState::new(),
        content_signature: 11,
        layout,
        row_count: 3,
        card_count: 12,
    };
    assert!(!card_grid_cache_needs_reset(&cache, 11, 12, 3, layout));
    assert!(card_grid_cache_needs_reset(&cache, 12, 12, 3, layout));
    assert!(card_grid_cache_needs_reset(
        &cache,
        11,
        12,
        3,
        CardGridLayout::new(5, 800., 154., 220.)
    ));
}

#[cfg(test)]
mod playback_selection_tests {}

#[cfg(test)]
mod playlist_update_tests {}
