use std::{cell::Cell, time::Instant};

use gpui::prelude::*;

use crate::{motion::ResponsiveModeMotion, search::Provider};

use super::{
    ArtistHoverKey, CAROUSEL_ARROW_TRACK_GAP, CAROUSEL_ARROW_WIDTH, CAROUSEL_CARD_ROW_BOTTOM_GAP,
    CAROUSEL_CONTENT_BOTTOM_PADDING, CAROUSEL_CONTROL_HEIGHT, CAROUSEL_THUMB_HEIGHT,
    CAROUSEL_TRACK_INSET, CATEGORY_LABEL_FIT_BUFFER, CATEGORY_TAB_ICON_WIDTH, CardCarouselState,
    CarouselDirection, MAIN_CONTENT_INSET, MOBILE_MAX, MenuRoute, MenuRouteKind,
    NARROW_MAIN_CONTENT_INSET, TOOLBAR_STACK_MAX, TRACK_PROVIDER_COMPACT_COLUMN_WIDTH,
    TRACK_PROVIDER_FULL_COLUMN_WIDTH, TRACK_PROVIDER_LABEL_MIN_WIDTH, artist_hover,
    artist_hover_registry, artist_route_at, artist_text_and_ranges, card_carousel,
    card_carousel_display_count, card_carousel_has_overflow, card_grid_skeleton_count,
    card_row_has_more, card_row_metrics, carousel_arrow_target_from_pending, carousel_drag_offset,
    carousel_edge_intensity, carousel_edge_trigger, carousel_snap_offset, carousel_target_offset,
    category_tab_tooltip, category_tabs_icon_only, category_tabs_label_width,
    compact_desktop_viewport, compact_track_provider, effective_content_width, fitted_columns,
    horizontal_scroll_id, main_content_inset, narrow_content_viewport, row_action_cursor,
    set_artist_hover, shell_metrics, shell_metrics_for_viewport, should_consume_horizontal_scroll,
    show_search_result_count, track_artist_text_and_ranges, track_artwork_url,
    track_provider_column_endpoints, visible_preview_cards,
};

#[test]
fn consumes_horizontal_only_wheel() {
    assert!(should_consume_horizontal_scroll(10., 0.));
}

#[test]
fn artist_ranges_follow_combined_text_byte_offsets() {
    let routes = vec![
        MenuRoute {
            kind: MenuRouteKind::Artist,
            id: "1".into(),
            title: "A".into(),
        },
        MenuRoute {
            kind: MenuRouteKind::Artist,
            id: "2".into(),
            title: "Björk".into(),
        },
    ];
    let (text, ranges) = artist_text_and_ranges(&routes);

    assert_eq!(text, "A, Björk");
    assert_eq!(&text[ranges[0].clone()], "A");
    assert_eq!(&text[ranges[1].clone()], "Björk");
}

#[test]
fn soundcloud_single_artist_route_keeps_credited_display_text() {
    let routes = vec![MenuRoute {
        kind: MenuRouteKind::Artist,
        id: "55".into(),
        title: "TRVCY".into(),
    }];

    let visual_provider_badge: Option<Provider> = None;
    let (text, ranges) = track_artist_text_and_ranges("Skrillex", &routes, Provider::SoundCloud);

    assert!(visual_provider_badge.is_none());
    assert_eq!(text, "Skrillex");
    assert_eq!(&text[ranges[0].clone()], "Skrillex");
    assert_eq!(routes[0].id, "55");
    assert_eq!(routes[0].title, "TRVCY");
}

#[test]
fn deezer_single_route_uses_the_route_title_instead_of_the_full_credit() {
    let routes = vec![MenuRoute {
        kind: MenuRouteKind::Artist,
        id: "1".into(),
        title: "Primary".into(),
    }];

    let (text, ranges) = track_artist_text_and_ranges("Primary, Guest", &routes, Provider::Deezer);

    assert_eq!(text, "Primary");
    assert_eq!(&text[ranges[0].clone()], "Primary");
}

#[test]
fn artist_hover_selects_only_the_range_under_the_pointer() {
    let ranges = vec![0..5, 7..12];

    assert_eq!(artist_route_at(Some(2), &ranges), Some(0));
    assert_eq!(artist_route_at(Some(6), &ranges), None);
    assert_eq!(artist_route_at(Some(8), &ranges), Some(1));
    assert_eq!(artist_route_at(None, &ranges), None);
}

#[test]
fn artist_hover_registry_keeps_only_the_active_row() {
    let first = ArtistHoverKey {
        scope: "virtualized-list".into(),
        row: 1,
        text: "First Artist".into(),
    };
    let second = ArtistHoverKey {
        scope: "virtualized-list".into(),
        row: 2_499,
        text: "Last Artist".into(),
    };
    artist_hover_registry().lock().unwrap().clear();

    set_artist_hover(&first, Some(0));
    set_artist_hover(&second, Some(0));

    let hover = artist_hover_registry().lock().unwrap();
    assert_eq!(hover.len(), 1);
    assert!(!hover.contains_key(&first));
    drop(hover);
    assert_eq!(artist_hover(&second), Some(0));

    set_artist_hover(&second, None);
    assert_eq!(artist_hover(&second), None);
}

#[test]
fn track_artwork_uses_small_provider_thumbnails() {
    assert_eq!(
        track_artwork_url("https://e-cdns-images.dzcdn.net/images/cover/hash/500x500.jpg"),
        "https://e-cdns-images.dzcdn.net/images/cover/hash/120x120.jpg"
    );
    assert_eq!(
        track_artwork_url("https://i1.sndcdn.com/track-t500x500.jpg"),
        "https://i1.sndcdn.com/track-t120x120.jpg"
    );
    assert_eq!(
        track_artwork_url("https://i1.sndcdn.com/track-large.jpg"),
        "https://i1.sndcdn.com/track-t120x120.jpg"
    );
}

#[test]
fn track_artwork_leaves_unknown_variants_unchanged() {
    let unknown = "https://example.com/track-large.jpg";
    assert_eq!(track_artwork_url(unknown), unknown);
    assert_eq!(
        track_artwork_url("https://e-cdns-images.dzcdn.net/images/cover/hash/250x250.jpg"),
        "https://e-cdns-images.dzcdn.net/images/cover/hash/250x250.jpg"
    );
    assert_eq!(track_artwork_url("not-a-url"), "not-a-url");
}

#[test]
fn card_row_has_more_tolerates_a_two_pixel_endpoint() {
    assert!(card_row_has_more(0., 500.));
    assert!(card_row_has_more(-497., 500.));
    assert!(!card_row_has_more(-498., 500.));
    assert!(!card_row_has_more(-500., 500.));
}

#[test]
fn carousel_edge_trigger_keeps_half_a_card_of_lead_room() {
    // Full size rows start dissolving the darkening half a card before
    // the end instead of clinging to the final pixels.
    assert_eq!(carousel_edge_trigger(162., 500.), 81.);
    // Narrow cards scale the lead room with the card pitch.
    assert_eq!(carousel_edge_trigger(135., 500.), 67.5);
    // Rows that barely overflow scale the trigger down so the darkening
    // survives, but never below the shared overflow tolerance.
    assert_eq!(carousel_edge_trigger(162., 100.), 50.);
    assert_eq!(carousel_edge_trigger(162., 30.), 15.);
    assert_eq!(carousel_edge_trigger(162., 3.), 2.);
    assert_eq!(carousel_edge_trigger(162., 0.), 2.);
    // Degenerate pitches fall back to the overflow tolerance.
    assert_eq!(carousel_edge_trigger(0., 500.), 2.);
}

#[test]
fn carousel_edges_mirror_each_others_intensities() {
    let ramp = carousel_edge_trigger(162., 500.);

    // A fresh row has content ahead on the right and nothing behind.
    assert_eq!(carousel_edge_intensity(500., ramp), 1.);
    assert_eq!(carousel_edge_intensity(0., ramp), 0.);
    // One card in, both edges are saturated.
    assert_eq!(carousel_edge_intensity(338., ramp), 1.);
    assert_eq!(carousel_edge_intensity(162., ramp), 1.);
    // Inside either edge's lead room the darkening scales down.
    assert_eq!(carousel_edge_intensity(81., ramp), 1.);
    assert!((carousel_edge_intensity(80., ramp) - 80. / 81.).abs() < 1e-6);
    assert!((carousel_edge_intensity(40.5, ramp) - 0.5).abs() < 1e-6);
}

#[test]
fn carousel_edge_intensity_ramps_with_distance_and_caps() {
    // Transparent at the very edge, and behind it.
    assert_eq!(carousel_edge_intensity(0., 81.), 0.);
    assert_eq!(carousel_edge_intensity(-20., 81.), 0.);
    // Ramping up proportionally with the remaining travel.
    assert!((carousel_edge_intensity(20.25, 81.) - 0.25).abs() < 1e-6);
    assert!((carousel_edge_intensity(60., 81.) - 60. / 81.).abs() < 1e-6);
    // Saturating at the ramp length and staying capped beyond it.
    assert_eq!(carousel_edge_intensity(81., 81.), 1.);
    assert_eq!(carousel_edge_intensity(162., 81.), 1.);
    assert_eq!(carousel_edge_intensity(5000., 81.), 1.);
    // A degenerate ramp keeps the edge transparent instead of NaN-ing.
    assert_eq!(carousel_edge_intensity(20., 0.), 0.);
}

#[gpui::test]
fn carousel_edge_intensities_follow_the_rendered_scroll_bounds(cx: &mut gpui::TestAppContext) {
    cx.update(|cx| {
        gpui_component::init(cx);
        crate::theme::configure_component_theme(cx);
    });
    let cx = cx.add_empty_window();

    struct CarouselHost(CardCarouselState);
    impl gpui::Render for CarouselHost {
        fn render(
            &mut self,
            _: &mut gpui::Window,
            _: &mut gpui::Context<Self>,
        ) -> impl gpui::IntoElement {
            card_carousel(
                "edge-fade-wiring",
                self.0.clone(),
                150.,
                12.,
                true,
                gpui::div()
                    .flex()
                    .flex_none()
                    .gap(gpui::px(12.))
                    .children((0..8).map(|_| gpui::div().w(gpui::px(150.)).h(gpui::px(196.))))
                    .into_any_element(),
            )
        }
    }

    let state = CardCarouselState::new();
    let view = cx.update(|_, cx| cx.new(|_| CarouselHost(state.clone())));
    let draw = |cx: &mut gpui::VisualTestContext, view: &gpui::Entity<CarouselHost>| {
        cx.draw(
            gpui::point(gpui::px(0.), gpui::px(0.)),
            gpui::size(gpui::px(400.), gpui::px(300.)),
            {
                let view = view.clone();
                move |_, _| view.into_any_element()
            },
        );
    };

    // A fresh overflowing row saturates the right darkening and keeps
    // the left one transparent, straight from the live scroll bounds.
    draw(cx, &view);
    let extent = f32::from(state.scroll_handle.max_offset().x);
    assert!(extent > 800.);
    let ramp = carousel_edge_trigger(162., extent);
    assert_eq!(carousel_edge_intensity(0., ramp), 0.);
    assert_eq!(carousel_edge_intensity(extent, ramp), 1.);

    // Scrolling into the final lead room scales the right darkening
    // down and saturates the mirrored left one.
    let scrolled = extent - 20.;
    state
        .scroll_handle
        .set_offset(gpui::point(gpui::px(-scrolled), gpui::px(0.)));
    draw(cx, &view);
    assert_eq!(f32::from(state.scroll_handle.offset().x), -scrolled);
    assert_eq!(carousel_edge_intensity(scrolled, ramp), 1.);
    let right = carousel_edge_intensity(extent - scrolled, ramp);
    assert!((right - 20. / ramp).abs() < 1e-6);
    assert!(right > 0. && right < 1.);
}

#[test]
fn carousel_controls_and_spacing_only_exist_for_overflowing_rows() {
    assert!(!card_carousel_has_overflow(0, 150., 12., 474., 0.));
    assert!(!card_carousel_has_overflow(1, 150., 12., 474., 0.));
    assert!(!card_carousel_has_overflow(3, 150., 12., 474., 0.));
    assert!(card_carousel_has_overflow(4, 150., 12., 474., 0.));
    assert!(card_carousel_has_overflow(3, 150., 12., 474., 2.));
    assert!(card_carousel_has_overflow(64, 150., 12., 7680., 0.));
}

#[test]
fn carousel_card_window_is_fixed_across_viewport_sizes() {
    assert_eq!(card_carousel_display_count(4, 12), 4);
    assert_eq!(card_carousel_display_count(40, 12), 40);
    assert_eq!(card_carousel_display_count(100, 12), 64);
}

#[test]
fn consumes_horizontal_dominant_wheel() {
    assert!(should_consume_horizontal_scroll(-10., 4.));
}

#[test]
fn preserves_vertical_dominant_wheel() {
    assert!(!should_consume_horizontal_scroll(4., -10.));
}

#[test]
fn preserves_zero_wheel() {
    assert!(!should_consume_horizontal_scroll(0., 0.));
}

#[test]
fn disabled_row_actions_use_the_default_cursor() {
    assert!(matches!(row_action_cursor(true), gpui::CursorStyle::Arrow));
    assert!(matches!(
        row_action_cursor(false),
        gpui::CursorStyle::PointingHand
    ));
}

#[test]
fn vertical_wheel_is_not_considered_horizontal() {
    assert!(!should_consume_horizontal_scroll(0., 12.));
    assert!(!should_consume_horizontal_scroll(4., 10.));
}

#[test]
fn carousel_targets_one_card_pitch_and_clamps_to_edges() {
    assert_eq!(
        carousel_target_offset(0., 500., 162., CarouselDirection::Next),
        -162.
    );
    assert_eq!(
        carousel_target_offset(-162., 500., 162., CarouselDirection::Previous),
        0.
    );
    assert_eq!(
        carousel_target_offset(-40., 500., 162., CarouselDirection::Next),
        -162.
    );
    assert_eq!(
        carousel_target_offset(-200., 500., 162., CarouselDirection::Previous),
        -162.
    );
    assert_eq!(
        carousel_target_offset(-162., 500., 162., CarouselDirection::Next),
        -324.
    );
    assert_eq!(
        carousel_target_offset(-324., 500., 162., CarouselDirection::Next),
        -500.
    );
    assert_eq!(
        carousel_target_offset(-500., 500., 162., CarouselDirection::Previous),
        -324.
    );
    assert_eq!(
        carousel_target_offset(-450., 500., 162., CarouselDirection::Next),
        -500.
    );
    assert_eq!(
        carousel_target_offset(-40., 500., 162., CarouselDirection::Previous),
        0.
    );
}

#[test]
fn carousel_target_normalizes_invalid_bounds_and_pitch() {
    assert_eq!(
        carousel_target_offset(80., 20., -5., CarouselDirection::Next),
        0.
    );
    assert_eq!(
        carousel_target_offset(-80., -20., 5., CarouselDirection::Next),
        -20.
    );
}

#[test]
fn carousel_arrow_retargets_from_the_live_displayed_offset() {
    assert_eq!(
        carousel_arrow_target_from_pending(-75., None, 500., 162., CarouselDirection::Next),
        -162.
    );
    assert_eq!(
        carousel_arrow_target_from_pending(-75., Some(-162.), 500., 162., CarouselDirection::Next,),
        -324.
    );
    assert_eq!(
        carousel_arrow_target_from_pending(-200., None, 500., 162., CarouselDirection::Previous,),
        -162.
    );
}

#[test]
fn three_rapid_next_and_previous_clicks_accumulate_card_destinations() {
    let next_pending = Cell::new(None);
    for _ in 0..3 {
        let target = carousel_arrow_target_from_pending(
            0.,
            next_pending.get(),
            500.,
            162.,
            CarouselDirection::Next,
        );
        next_pending.set(Some(target));
    }
    assert_eq!(next_pending.get(), Some(-500.));

    let previous_pending = Cell::new(None);
    for _ in 0..3 {
        let target = carousel_arrow_target_from_pending(
            -500.,
            previous_pending.get(),
            500.,
            162.,
            CarouselDirection::Previous,
        );
        previous_pending.set(Some(target));
    }
    assert_eq!(previous_pending.get(), Some(0.));
}

#[test]
fn pending_arrow_targets_clamp_at_the_scroll_endpoints() {
    assert_eq!(
        carousel_arrow_target_from_pending(0., Some(-500.), 500., 162., CarouselDirection::Next,),
        -500.
    );
    assert_eq!(
        carousel_arrow_target_from_pending(
            -500.,
            Some(0.),
            500.,
            162.,
            CarouselDirection::Previous,
        ),
        0.
    );
}

#[test]
fn carousel_snap_prefers_nearest_regular_or_final_boundary() {
    assert_eq!(carousel_snap_offset(-450., 500., 162.), -486.);
    assert_eq!(carousel_snap_offset(-495., 500., 162.), -500.);
    assert_eq!(carousel_snap_offset(-200., 500., 162.), -162.);
}

#[test]
fn carousel_drag_offset_clamps_to_scroll_range() {
    assert_eq!(carousel_drag_offset(-450., -100., 500.), -500.);
    assert_eq!(carousel_drag_offset(-40., -100., 500.), -140.);
    assert_eq!(carousel_drag_offset(-40., 100., 500.), 0.);
}

#[test]
fn carousel_controls_match_native_track_metrics() {
    assert_eq!(CAROUSEL_ARROW_WIDTH, 10.);
    assert_eq!(CAROUSEL_ARROW_TRACK_GAP, 2.);
    assert_eq!(CAROUSEL_TRACK_INSET, 12.);
    assert_eq!(CAROUSEL_CONTROL_HEIGHT, 10.);
    assert_eq!(CAROUSEL_THUMB_HEIGHT, 6.);
    assert_eq!(crate::theme::SCROLLBAR_THUMB, 0x3f3f46);
}

#[test]
fn carousel_track_inset_keeps_thumb_endpoints_symmetric() {
    let width = 100.;
    let left = CAROUSEL_TRACK_INSET;
    let right = width - CAROUSEL_TRACK_INSET;

    assert_eq!(left, CAROUSEL_ARROW_WIDTH + CAROUSEL_ARROW_TRACK_GAP);
    assert_eq!(
        width - right,
        CAROUSEL_ARROW_WIDTH + CAROUSEL_ARROW_TRACK_GAP
    );
    assert_eq!(left, width - right);
}

#[test]
fn carousel_content_padding_preserves_the_native_card_row_gap() {
    assert_eq!(CAROUSEL_CARD_ROW_BOTTOM_GAP, 8.);
    assert_eq!(CAROUSEL_CONTENT_BOTTOM_PADDING, 18.);
    assert_eq!(
        CAROUSEL_CONTENT_BOTTOM_PADDING,
        CAROUSEL_CARD_ROW_BOTTOM_GAP + CAROUSEL_CONTROL_HEIGHT
    );
}

#[test]
fn horizontal_scroll_ids_are_stable_and_section_qualified() {
    assert_eq!(
        horizontal_scroll_id("search-card-preview-scroll", "Albums", 0),
        "search-card-preview-scroll-Albums-0"
    );
    assert_ne!(
        horizontal_scroll_id("search-card-preview-scroll", "Albums", 0),
        horizontal_scroll_id("search-card-preview-scroll", "Artists", 1)
    );
}

#[test]
fn track_provider_labels_follow_effective_content_width() {
    assert!(narrow_content_viewport(768.));
    assert!(!compact_desktop_viewport(768.));
    assert!(!compact_track_provider(768.));

    assert!(!narrow_content_viewport(769.));
    assert!(compact_desktop_viewport(769.));
    assert!(!compact_track_provider(769.));

    assert!(!narrow_content_viewport(1200.));
    assert!(compact_desktop_viewport(1200.));
    assert!(!compact_track_provider(1200.));

    assert!(!narrow_content_viewport(1201.));
    assert!(!compact_desktop_viewport(1201.));
    assert!(compact_track_provider(539.));
    assert!(compact_track_provider(TRACK_PROVIDER_LABEL_MIN_WIDTH - 0.1));
    assert!(!compact_track_provider(TRACK_PROVIDER_LABEL_MIN_WIDTH));
}

#[test]
fn track_provider_motion_preserves_full_and_narrow_endpoints() {
    let mut motion = ResponsiveModeMotion::default();
    let now = Instant::now();
    let full = motion.prepare(false, now, false);
    assert_eq!(
        track_provider_column_endpoints(full),
        (
            TRACK_PROVIDER_FULL_COLUMN_WIDTH,
            TRACK_PROVIDER_FULL_COLUMN_WIDTH
        )
    );

    let narrow = motion.prepare(true, now, false);
    assert_eq!(
        track_provider_column_endpoints(narrow),
        (
            TRACK_PROVIDER_FULL_COLUMN_WIDTH,
            TRACK_PROVIDER_COMPACT_COLUMN_WIDTH
        )
    );
    assert_eq!(motion.prepare(true, now, false), narrow);

    let settled_at = now + crate::motion::CONTENT_DURATION;
    let settled = motion.prepare(true, settled_at, false);
    assert_eq!(settled.from, 1.0);
    assert_eq!(settled.target, 1.0);

    let expanded = motion.prepare(false, settled_at, false);
    assert_eq!(
        track_provider_column_endpoints(expanded),
        (
            TRACK_PROVIDER_COMPACT_COLUMN_WIDTH,
            TRACK_PROVIDER_FULL_COLUMN_WIDTH
        )
    );
}

#[test]
fn responsive_contract_has_exact_boundary_behavior() {
    for width in [
        459., 460., 461., 559., 560., 561., 639., 640., 641., 767., 768., 769., 899., 900., 901.,
        1199., 1200., 1201., 1439., 1440., 1441.,
    ] {
        let metrics = shell_metrics_for_viewport(width, 620.);
        assert_eq!(metrics.narrow_content, width <= MOBILE_MAX);
        assert_eq!(metrics.compact_desktop, (769. ..=1200.).contains(&width));
        assert_eq!(metrics.compact_player, (769. ..=1440.).contains(&width));
        assert_eq!(metrics.toolbar_stacked, width <= TOOLBAR_STACK_MAX);
    }
}

#[test]
fn card_row_metrics_follow_the_frozen_breakpoint_sizes() {
    assert_eq!(card_row_metrics(false), (150., 12.));
    assert_eq!(card_row_metrics(true), (126., 9.));
}

#[test]
fn preview_visibility_has_one_card_minimum() {
    assert_eq!(visible_preview_cards(0., false), 1);
    assert_eq!(visible_preview_cards(150., false), 1);
}

#[test]
fn preview_visibility_accounts_for_card_gap() {
    assert_eq!(visible_preview_cards(312., false), 2);
    assert_eq!(visible_preview_cards(474., false), 3);
    assert_eq!(visible_preview_cards(261., true), 2);
}

#[test]
fn shell_metrics_use_compact_desktop_only() {
    let narrow = shell_metrics(768.);
    assert_eq!((narrow.sidebar_width, narrow.gutter), (0., 12.));
    assert_eq!(narrow.available_width, 744.);

    let compact_start = shell_metrics(769.);
    assert_eq!(
        (compact_start.sidebar_width, compact_start.gutter),
        (68., 16.)
    );
    assert_eq!(compact_start.available_width, 669.);

    let compact_end = shell_metrics(1200.);
    assert_eq!((compact_end.sidebar_width, compact_end.gutter), (68., 16.));
    assert_eq!(compact_end.available_width, 1100.);

    let desktop = shell_metrics(1201.);
    assert_eq!((desktop.sidebar_width, desktop.gutter), (240., 28.));
    assert_eq!(desktop.available_width, 905.);
}

#[test]
fn shell_metrics_never_returns_negative_content_width() {
    assert_eq!(shell_metrics(100.).available_width, 76.);
}

#[test]
fn effective_content_width_accounts_for_open_right_sidebar() {
    let metrics = shell_metrics(1200.);
    assert_eq!(effective_content_width(&metrics, false), 1076.);
    assert_eq!(effective_content_width(&metrics, true), 716.);

    let narrow = shell_metrics(768.);
    assert_eq!(
        effective_content_width(&narrow, true),
        narrow.available_width
    );
}

#[test]
fn fitted_columns_follow_the_effective_center_width() {
    let metrics = shell_metrics(1200.);
    assert_eq!(fitted_columns(effective_content_width(&metrics, false)), 6);
    assert_eq!(fitted_columns(effective_content_width(&metrics, true)), 4);
}

#[test]
fn card_grid_skeleton_count_grows_with_height_in_complete_rows() {
    let short = card_grid_skeleton_count(4, 740., 500.);
    let tall = card_grid_skeleton_count(4, 740., 1_200.);

    assert_eq!(short % 4, 0);
    assert_eq!(tall % 4, 0);
    assert!(tall > short);
}

#[test]
fn search_and_library_insets_match_the_original_breakpoint() {
    assert_eq!(
        main_content_inset(&shell_metrics(768.)),
        NARROW_MAIN_CONTENT_INSET
    );
    assert_eq!(main_content_inset(&shell_metrics(769.)), MAIN_CONTENT_INSET);
    assert_eq!(
        main_content_inset(&shell_metrics(1201.)),
        MAIN_CONTENT_INSET
    );
}

#[test]
fn category_tab_labels_switch_to_balanced_icons_with_fit_buffer() {
    let labels = ["All", "Tracks", "Albums", "Artists", "Playlists"];
    let label_width = category_tabs_label_width(&labels);
    assert!(!category_tabs_icon_only(
        label_width + CATEGORY_LABEL_FIT_BUFFER,
        &labels
    ));
    assert!(category_tabs_icon_only(label_width + 47., &labels));
    assert_eq!(CATEGORY_TAB_ICON_WIDTH, 34.);
    assert_eq!(super::category_tab_text_width("Albums"), 43.);
}

#[test]
fn category_tab_tooltips_are_only_for_icon_only_tabs() {
    assert_eq!(category_tab_tooltip(true, "Tracks"), Some("Tracks"));
    assert_eq!(category_tab_tooltip(false, "Tracks"), None);
}

#[test]
fn search_result_count_is_hidden_at_or_below_header_boundary() {
    assert!(!show_search_result_count(640.));
    assert!(!show_search_result_count(320.));
    assert!(show_search_result_count(641.));
}
