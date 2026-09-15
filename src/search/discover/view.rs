use std::rc::Rc;

use gpui::prelude::*;
use gpui::{AnyElement, Entity, FontWeight, KeyDownEvent, px, rgb};

use crate::{
    app_button::primary_button,
    assets::{LocalIcon, local_icon},
    browser_scroll::{BrowserScrollTarget, browser_scroll_surface},
    collection_detail::{
        DETAIL_CARD_CAROUSEL_INSET_PX, DETAIL_CARD_GRID_ROW_HEIGHT_EXTRA_PX,
        collection_card_carousel_content, collection_card_frame,
    },
    music_ui::{self, CardCarouselState},
    theme::{BORDER, DEEZER, FOREGROUND, MUTED, SOUNDCLOUD, SURFACE, SURFACE_RAISED},
};

use super::{DiscoverChannelStatus, DiscoverItem, DiscoverSection, DiscoverStatus};
use crate::search::{
    SearchView, cards_view,
    models::{Provider, Source},
};

pub(super) const DISCOVER_CARD_PREVIEW_LIMIT: usize = 24;
const DISCOVER_SECTION_GAP_PX: f32 = 22.;
const DISCOVER_SECTION_TITLE_HEIGHT_PX: f32 = 20.;
const DISCOVER_SECTION_SUBTITLE_HEIGHT_PX: f32 = 16.;
const DISCOVER_SECTION_TEXT_GAP_PX: f32 = 2.;
const DISCOVER_SECTION_HEADING_GAP_PX: f32 = 10.;
const DISCOVER_SECTION_CAROUSEL_INSET_PX: f32 = DETAIL_CARD_CAROUSEL_INSET_PX;
const DISCOVER_SECTION_CAROUSEL_RESERVE_PX: f32 = music_ui::CAROUSEL_CONTENT_BOTTOM_PADDING;
const DISCOVER_PROVIDER_ICON_OPTICAL_OFFSET_PX: f32 = 0.;
const DISCOVER_CHANNEL_HEADING_HEIGHT_PX: f32 = 42.;
const DISCOVER_CHANNEL_HEADING_TITLE_LINE_HEIGHT_PX: f32 = 30.;
const DISCOVER_CHANNEL_HEADING_OPTICAL_Y_OFFSET_PX: f32 = -1.;
const DISCOVER_LOADING_OVERDRAW_ROWS: usize = 2;
const DISCOVER_FEED_OVERDRAW_ROWS: f32 = 2.;

#[derive(Clone)]
enum DiscoverFeedEntry {
    Section {
        section: DiscoverSection,
        carousel: CardCarouselState,
    },
    Loading {
        provider: Provider,
        index: usize,
        single_line: bool,
        carousel: CardCarouselState,
    },
    AccountRequired {
        provider: Provider,
    },
    ChannelLoading {
        provider: Provider,
        index: usize,
        carousel: CardCarouselState,
    },
    Status {
        provider: Provider,
        title: String,
        description: String,
        retry: bool,
    },
    ChannelStatus {
        title: String,
        description: String,
        retry: bool,
    },
}

#[derive(Clone, Copy, Debug, PartialEq)]
struct DiscoverSectionGeometry {
    card_width: f32,
    card_row_height: f32,
    heading_height: f32,
    heading_gap: f32,
    carousel_reserve: f32,
    row_height: f32,
}

/// The full two-line heading slot: title line, text gap, and subtitle line.
/// Subtitled sections and loading skeletons measure it in full; sections
/// without a subtitle measure only the title line, so their rows sit
/// exactly the subtitle delta higher in the feed instead of carrying dead
/// space at the bottom of a pinned slot.
fn section_heading_height() -> f32 {
    DISCOVER_SECTION_TITLE_HEIGHT_PX
        + DISCOVER_SECTION_TEXT_GAP_PX
        + DISCOVER_SECTION_SUBTITLE_HEIGHT_PX
}

fn section_geometry(narrow: bool) -> DiscoverSectionGeometry {
    let (card_width, _) = music_ui::card_row_metrics(narrow);
    let card_row_height = card_width + DETAIL_CARD_GRID_ROW_HEIGHT_EXTRA_PX;
    let heading_height = section_heading_height();
    let visual_height = heading_height
        + DISCOVER_SECTION_HEADING_GAP_PX
        + DISCOVER_SECTION_CAROUSEL_INSET_PX
        + card_row_height
        + DISCOVER_SECTION_CAROUSEL_RESERVE_PX;
    DiscoverSectionGeometry {
        card_width,
        card_row_height,
        heading_height,
        heading_gap: DISCOVER_SECTION_HEADING_GAP_PX,
        carousel_reserve: DISCOVER_SECTION_CAROUSEL_RESERVE_PX,
        row_height: visual_height + DISCOVER_SECTION_GAP_PX,
    }
}

fn loading_row_count(available_height: f32, narrow: bool) -> usize {
    let estimate = section_geometry(narrow).row_height.max(1.);
    let visible_rows = (available_height.max(estimate) / estimate).ceil() as usize;
    visible_rows.max(1) + DISCOVER_LOADING_OVERDRAW_ROWS
}

pub(crate) fn render(
    view: &SearchView,
    host: &Entity<SearchView>,
    source: Source,
    available_width: f32,
    available_height: f32,
    narrow: bool,
    _cx: &mut gpui::Context<SearchView>,
) -> AnyElement {
    if view.discover.channel_open() {
        let channel = view.discover.channel().clone();
        let mut entries = Vec::new();
        let mut content_identity = format!(
            "channel:{}:{}:{:?}",
            channel.slug, channel.title, channel.status
        );
        match channel.status {
            DiscoverChannelStatus::Ready => {
                append_sections(view, &channel.sections, &mut entries, &mut content_identity);
            }
            DiscoverChannelStatus::Loading => {
                entries.extend(
                    (0..loading_row_count(available_height, narrow)).map(|index| {
                        let scroll_id =
                            format!("discover-channel-loading:{}:{index}", channel.slug);
                        DiscoverFeedEntry::ChannelLoading {
                            provider: Provider::Deezer,
                            index,
                            carousel: view.card_scroll_handle(&scroll_id),
                        }
                    }),
                );
            }
            DiscoverChannelStatus::Failed(error) => {
                entries.push(DiscoverFeedEntry::ChannelStatus {
                    title: channel.title.clone(),
                    description: error,
                    retry: true,
                });
            }
            DiscoverChannelStatus::Closed => unreachable!("closed channel is not rendered"),
        }
        let feed = render_feed(
            view,
            host,
            available_width,
            available_height,
            narrow,
            entries,
            content_identity,
            format!("discover-feed:{source:?}:channel:{}", channel.slug),
        );
        return gpui::div()
            .size_full()
            .flex()
            .flex_col()
            .min_h_0()
            .child(channel_heading(&channel.title))
            .child(feed)
            .into_any_element();
    }

    let mut entries = Vec::new();
    let mut content_identity = format!("source:{source:?}");
    let defer_soundcloud_loading = should_defer_all_soundcloud_loading(
        source,
        &view.discover.provider(Provider::Deezer).status,
        &view.discover.provider(Provider::SoundCloud).status,
    );
    for provider in source.providers() {
        append_provider_entries(
            view,
            *provider,
            defer_soundcloud_loading,
            &mut entries,
            &mut content_identity,
        );
    }

    append_loading_overdraw(
        view,
        source,
        available_height,
        narrow,
        defer_soundcloud_loading,
        &mut entries,
        &mut content_identity,
    );

    render_feed(
        view,
        host,
        available_width,
        available_height,
        narrow,
        entries,
        content_identity,
        format!("discover-feed:{source:?}:home"),
    )
}

fn render_feed(
    view: &SearchView,
    host: &Entity<SearchView>,
    available_width: f32,
    _available_height: f32,
    narrow: bool,
    entries: Vec<DiscoverFeedEntry>,
    content_identity: String,
    cache_identity: String,
) -> AnyElement {
    // Rows measure their natural heights: the heading renders its subtitle
    // line only when present, cards render their caption rows only when
    // present, and the section gap is the same bottom padding on every
    // row, so every gap between sections is consistent by construction.
    // The list measures all rows in its first prepaint (measure_all), so
    // the scrollbar is exact without pinning rows to a uniform height.
    let overdraw = px(section_geometry(narrow).row_height * DISCOVER_FEED_OVERDRAW_ROWS);
    let item_count = entries.len();
    let (state, browser_scroll) =
        view.discover_feed_state(&cache_identity, &content_identity, item_count, overdraw);
    let entries = Rc::new(entries);
    let host = host.clone();
    let account = view.account.clone();
    let list = gpui::list(state.clone(), move |index, _window, _app| {
        render_entry(&entries[index], &host, &account, available_width, narrow)
    });
    // Every row is measured before paint, so the raw list state is exact
    // for the scrollbar and the wheel math; no fixed-height adapter is
    // needed.
    let content = gpui::div()
        .size_full()
        .flex_1()
        .min_h_0()
        .relative()
        .child(list.w_full().h_full().min_h_0())
        .child(crate::library::virtualization::library_vertical_scrollbar(
            &state, narrow,
        ))
        .into_any_element();
    browser_scroll_surface(
        format!("discover-feed-scroll-{cache_identity}"),
        content,
        BrowserScrollTarget::List(state),
        browser_scroll,
    )
}

fn append_loading_overdraw(
    view: &SearchView,
    source: Source,
    available_height: f32,
    narrow: bool,
    defer_soundcloud_loading: bool,
    entries: &mut Vec<DiscoverFeedEntry>,
    content_identity: &mut String,
) {
    let loading_providers: Vec<_> = source
        .providers()
        .iter()
        .copied()
        .filter(|provider| {
            matches!(
                view.discover.provider(*provider).status,
                DiscoverStatus::Idle | DiscoverStatus::Loading
            ) && !(defer_soundcloud_loading && *provider == Provider::SoundCloud)
        })
        .collect();
    if loading_providers.is_empty() {
        return;
    }
    let target = loading_row_count(available_height, narrow);
    let current = entries
        .iter()
        .filter(|entry| matches!(entry, DiscoverFeedEntry::Loading { .. }))
        .count();
    for index in current..target {
        let provider = loading_providers[index % loading_providers.len()];
        let ordinal = loading_occurrence_ordinal(entries, provider);
        let single_line = view.discover.skeleton_single_line(provider, ordinal);
        let scroll_id = format!("discover-loading-carousel-{provider:?}-{index}");
        entries.push(DiscoverFeedEntry::Loading {
            provider,
            index,
            single_line,
            carousel: view.card_scroll_handle(&scroll_id),
        });
        content_identity.push_str(&format!("|loading:{provider:?}:{index}:{single_line}"));
    }
}

/// How many loading stand-ins already exist for the provider. Each loading
/// row stands in for the section at that position in the provider's feed,
/// so the count is the position ordinal the next row represents.
fn loading_occurrence_ordinal(entries: &[DiscoverFeedEntry], provider: Provider) -> usize {
    entries
        .iter()
        .filter(|entry| {
            matches!(
                entry,
                DiscoverFeedEntry::Loading {
                    provider: candidate,
                    ..
                } if *candidate == provider
            )
        })
        .count()
}

fn append_provider_entries(
    view: &SearchView,
    provider: Provider,
    defer_soundcloud_loading: bool,
    entries: &mut Vec<DiscoverFeedEntry>,
    content_identity: &mut String,
) {
    let state = view.discover.provider(provider);
    if defer_soundcloud_loading && provider == Provider::SoundCloud {
        content_identity.push_str("|SoundCloud:loading-deferred");
        return;
    }
    match &state.status {
        DiscoverStatus::Ready if state.sections.is_empty() => {
            content_identity.push_str(&format!("|{provider:?}:empty"));
            entries.push(DiscoverFeedEntry::Status {
                provider,
                title: "No recommendations".into(),
                description: "This provider did not return any Discover sections.".into(),
                retry: false,
            });
        }
        DiscoverStatus::Ready => {
            append_sections(view, &state.sections, entries, content_identity);
        }
        DiscoverStatus::Loading | DiscoverStatus::Idle => {
            let ordinal = loading_occurrence_ordinal(entries, provider);
            let single_line = view.discover.skeleton_single_line(provider, ordinal);
            content_identity.push_str(&format!("|{provider:?}:loading:{single_line}"));
            let scroll_id = format!("discover-loading-carousel-{provider:?}-{ordinal}");
            entries.push(DiscoverFeedEntry::Loading {
                provider,
                index: ordinal,
                single_line,
                carousel: view.card_scroll_handle(&scroll_id),
            });
        }
        DiscoverStatus::AccountRequired => {
            content_identity.push_str(&format!("|{provider:?}:account"));
            entries.push(DiscoverFeedEntry::AccountRequired { provider });
        }
        DiscoverStatus::Failed(error) => {
            content_identity.push_str(&format!("|{provider:?}:failed:{error}"));
            entries.push(DiscoverFeedEntry::Status {
                provider,
                title: error.clone(),
                description: "Try again to reload this provider.".into(),
                retry: true,
            });
        }
    }
}

fn should_defer_all_soundcloud_loading(
    _source: Source,
    _deezer_status: &DiscoverStatus,
    _soundcloud_status: &DiscoverStatus,
) -> bool {
    // Providers load independently. Keep both loading surfaces visible so
    // whichever provider completes first can publish useful content.
    false
}

fn append_sections(
    view: &SearchView,
    sections: &[DiscoverSection],
    entries: &mut Vec<DiscoverFeedEntry>,
    content_identity: &mut String,
) {
    for section in sections {
        let scroll_id = format!("discover-card-carousel-{}", section.id);
        let carousel = view.card_scroll_handle(&scroll_id);
        content_identity.push_str(&format!(
            "|section:{}:{}:{}",
            section.id, section.title, section.subtitle,
        ));
        for item in section.items.iter().take(DISCOVER_CARD_PREVIEW_LIMIT) {
            content_identity.push('|');
            content_identity.push_str(&discover_item_content_identity(item));
        }
        entries.push(DiscoverFeedEntry::Section {
            section: section.clone(),
            carousel,
        });
    }
}

fn discover_item_content_identity(item: &DiscoverItem) -> String {
    format!(
        "{}:title={:?}:subtitle={:?}:badge={:?}:release_date={:?}:action={:?}",
        cards_view::stable_card_identity(&item.card),
        item.card.title,
        item.card.subtitle,
        item.card.badge,
        item.card.release_date,
        item.action,
    )
}

fn render_entry(
    entry: &DiscoverFeedEntry,
    host: &Entity<SearchView>,
    account: &Entity<crate::settings::AccountState>,
    available_width: f32,
    narrow: bool,
) -> AnyElement {
    let content = match entry {
        DiscoverFeedEntry::Section { section, carousel } => render_section(
            host,
            account,
            section,
            carousel.clone(),
            available_width,
            narrow,
        ),
        DiscoverFeedEntry::Loading {
            provider,
            index,
            single_line,
            carousel,
        } => render_loading(
            *provider,
            *index,
            *single_line,
            narrow,
            available_width,
            carousel.clone(),
        ),
        DiscoverFeedEntry::AccountRequired { provider } => render_account_required(*provider, host),
        DiscoverFeedEntry::ChannelLoading {
            provider,
            index,
            carousel,
        } => render_channel_loading(*provider, narrow, *index, available_width, carousel.clone()),
        DiscoverFeedEntry::Status {
            provider,
            title,
            description,
            retry,
        } => render_status(*provider, title, description, *retry, host, false),
        DiscoverFeedEntry::ChannelStatus {
            title,
            description,
            retry,
        } => render_status(Provider::Deezer, title, description, *retry, host, true),
    };
    // Rows size naturally inside the measured list: each entry renders
    // exactly the height its content needs, so no shape difference can
    // leave dead space inside the row.
    gpui::div()
        .w_full()
        .flex_none()
        .child(content)
        .into_any_element()
}

fn render_account_required(provider: Provider, host: &Entity<SearchView>) -> AnyElement {
    let (title, description) = crate::empty_state::discover_account_required_copy(provider.label());
    let click_host = host.clone();
    let action = primary_button(
        format!("discover-account-settings-{provider:?}"),
        Some(LocalIcon::Settings),
        "Open account settings",
        move |_, window, app| {
            click_host.update(app, |view, cx| {
                view.open_account_settings(window, cx);
            });
        },
    )
    .into_any_element();
    gpui::div()
        .w_full()
        .child(crate::empty_state::render(
            LocalIcon::UserLock,
            title,
            description,
            Some(action),
        ))
        .into_any_element()
}

fn render_section(
    host: &Entity<SearchView>,
    account: &Entity<crate::settings::AccountState>,
    section: &DiscoverSection,
    carousel: CardCarouselState,
    available_width: f32,
    narrow: bool,
) -> AnyElement {
    let geometry = section_geometry(narrow);
    let card_width = geometry.card_width;
    let (_, row_gap) = music_ui::card_row_metrics(narrow);
    let scroll_id = format!("discover-card-carousel-{}", section.id);
    let cards = section
        .items
        .iter()
        .take(DISCOVER_CARD_PREVIEW_LIMIT)
        .enumerate()
        .map(|(item_index, item)| {
            cards_view::render_discover_card(host, item, &section.id, item_index, narrow, account)
        });
    let controls_available = music_ui::card_carousel_has_overflow(
        section.items.len().min(DISCOVER_CARD_PREVIEW_LIMIT),
        card_width,
        row_gap,
        available_width,
        2.,
    );
    gpui::div()
        .w_full()
        .flex()
        .flex_col()
        .gap(px(geometry.heading_gap))
        .child(section_heading(section))
        .child(discover_card_carousel(
            scroll_id,
            carousel,
            card_width,
            row_gap,
            controls_available,
            collection_card_carousel_content(row_gap, true, cards).into_any_element(),
        ))
        .pb(px(DISCOVER_SECTION_GAP_PX))
        .into_any_element()
}

fn section_heading(section: &DiscoverSection) -> AnyElement {
    // The subtitle line is rendered only when the section has one, so a
    // single line heading sits the normal heading gap above its carousel
    // and the row measures exactly as tall as its content. The heading row
    // is top aligned and the icon box matches the title line box, so the
    // icon rides the title line in single and double line headings alike.
    let has_subtitle = !section.subtitle.trim().is_empty();
    gpui::div()
        .flex()
        .items_start()
        .gap(px(8.))
        .child(
            gpui::div()
                .h(px(DISCOVER_SECTION_TITLE_HEIGHT_PX))
                .flex_none()
                .flex()
                .items_center()
                .relative()
                .top(px(DISCOVER_PROVIDER_ICON_OPTICAL_OFFSET_PX))
                .child(provider_icon(section.provider)),
        )
        .child(
            gpui::div()
                .min_w_0()
                .flex()
                .flex_col()
                .gap(px(DISCOVER_SECTION_TEXT_GAP_PX))
                .child(
                    gpui::div()
                        .min_w_0()
                        .truncate()
                        .text_size(px(15.))
                        .line_height(px(DISCOVER_SECTION_TITLE_HEIGHT_PX))
                        .font_weight(FontWeight::SEMIBOLD)
                        .text_color(rgb(FOREGROUND))
                        .child(section.title.clone()),
                )
                .when(has_subtitle, |this| {
                    this.child(
                        gpui::div()
                            .min_w_0()
                            .truncate()
                            .h(px(DISCOVER_SECTION_SUBTITLE_HEIGHT_PX))
                            .text_size(px(12.))
                            .line_height(px(DISCOVER_SECTION_SUBTITLE_HEIGHT_PX))
                            .text_color(rgb(MUTED))
                            .child(section.subtitle.clone()),
                    )
                }),
        )
        .into_any_element()
}

fn provider_icon(provider: Provider) -> AnyElement {
    match provider {
        Provider::Deezer => local_icon(LocalIcon::Deezer, DEEZER).size(px(14.)),
        Provider::SoundCloud => local_icon(LocalIcon::SoundCloud, SOUNDCLOUD)
            .w(px(16.))
            .h(px(12.)),
    }
    .into_any_element()
}

fn render_loading(
    provider: Provider,
    index: usize,
    single_line: bool,
    narrow: bool,
    available_width: f32,
    carousel: CardCarouselState,
) -> AnyElement {
    let geometry = section_geometry(narrow);
    let card_width = geometry.card_width;
    let (_, row_gap) = music_ui::card_row_metrics(narrow);
    // The recorded card shape decides the stand-in's line count: single
    // line rows drop the subtitle row so the swap to real cards keeps
    // the row height stable.
    let single_line_cards = single_line;
    let cards = (0..DISCOVER_CARD_PREVIEW_LIMIT).map(|card_index| {
        let body = gpui::div()
            .id(format!(
                "discover-loading-{provider:?}-{index}-{card_index}"
            ))
            .min_w_0()
            .flex()
            .flex_col()
            .gap(px(7.))
            .child(
                gpui::div()
                    .w_full()
                    .aspect_square()
                    .rounded(px(if narrow { 6. } else { 8. }))
                    .bg(rgb(SURFACE_RAISED)),
            )
            .child(
                gpui::div()
                    .min_w_0()
                    .w_full()
                    .px(px(2.))
                    .pt(px(2.))
                    .pb(px(3.))
                    .child(
                        gpui::div()
                            .when(single_line_cards, |this| this.mx_auto())
                            .w(px(88.))
                            .h(px(music_ui::COLLECTION_CARD_TITLE_LINE_HEIGHT_PX))
                            .rounded(px(8.))
                            .bg(rgb(SURFACE_RAISED)),
                    )
                    .when(!single_line_cards, |this| {
                        this.child(
                            gpui::div()
                                .mt(px(2.))
                                .w(px(70.))
                                .h(px(music_ui::COLLECTION_CARD_SUBTITLE_ROW_HEIGHT_PX))
                                .rounded(px(8.))
                                .bg(rgb(SURFACE_RAISED)),
                        )
                    }),
            );
        collection_card_frame(true, narrow, body.into_any_element()).into_any_element()
    });
    let controls_available = music_ui::card_carousel_has_overflow(
        DISCOVER_CARD_PREVIEW_LIMIT,
        card_width,
        row_gap,
        available_width,
        2.,
    );
    let scroll_id = format!("discover-loading-carousel-{provider:?}-{index}");
    gpui::div()
        .w_full()
        .flex()
        .flex_col()
        .gap(px(geometry.heading_gap))
        .child(skeleton_heading(provider))
        .child(discover_card_carousel(
            scroll_id,
            carousel,
            card_width,
            row_gap,
            controls_available,
            collection_card_carousel_content(row_gap, true, cards).into_any_element(),
        ))
        .pb(px(DISCOVER_SECTION_GAP_PX))
        .into_any_element()
}

fn render_channel_loading(
    provider: Provider,
    narrow: bool,
    index: usize,
    available_width: f32,
    carousel: CardCarouselState,
) -> AnyElement {
    let geometry = section_geometry(narrow);
    let card_width = geometry.card_width;
    let (_, row_gap) = music_ui::card_row_metrics(narrow);
    const SKELETON_CARD_COUNT: usize = DISCOVER_CARD_PREVIEW_LIMIT;
    let cards = (0..SKELETON_CARD_COUNT).map(|card_index| {
        let body = gpui::div()
            .id(format!(
                "discover-channel-loading-{provider:?}-{index}-{card_index}"
            ))
            .min_w_0()
            .flex()
            .flex_col()
            .gap(px(7.))
            .child(
                gpui::div()
                    .w_full()
                    .aspect_square()
                    .rounded(px(if narrow { 6. } else { 8. }))
                    .bg(rgb(SURFACE_RAISED)),
            )
            .child(
                gpui::div()
                    .min_w_0()
                    .w_full()
                    .px(px(2.))
                    .pt(px(2.))
                    .pb(px(3.))
                    .child(
                        gpui::div()
                            .w(px(88.))
                            .h(px(music_ui::COLLECTION_CARD_TITLE_LINE_HEIGHT_PX))
                            .rounded(px(8.))
                            .bg(rgb(SURFACE_RAISED)),
                    )
                    .child(
                        gpui::div()
                            .mt(px(2.))
                            .h(px(music_ui::COLLECTION_CARD_SUBTITLE_ROW_HEIGHT_PX))
                            .flex()
                            .items_center()
                            .child(
                                gpui::div()
                                    .w(px(70.))
                                    .h(px(11.))
                                    .rounded(px(5.5))
                                    .bg(rgb(SURFACE_RAISED)),
                            ),
                    ),
            );
        collection_card_frame(true, narrow, body.into_any_element()).into_any_element()
    });
    let controls_available = music_ui::card_carousel_has_overflow(
        SKELETON_CARD_COUNT,
        card_width,
        row_gap,
        available_width,
        2.,
    );
    let carousel_id = format!("discover-channel-loading-{provider:?}-{index}");
    gpui::div()
        .w_full()
        .flex()
        .flex_col()
        .gap(px(geometry.heading_gap))
        .child(skeleton_heading(provider))
        .child(discover_card_carousel(
            carousel_id,
            carousel,
            card_width,
            row_gap,
            controls_available,
            collection_card_carousel_content(row_gap, true, cards).into_any_element(),
        ))
        .pb(px(DISCOVER_SECTION_GAP_PX))
        .into_any_element()
}

fn skeleton_heading(provider: Provider) -> AnyElement {
    // Loading rows cannot know whether the section they are standing in
    // for will have a subtitle, so both text slots stay reserved and
    // skeleton headings measure the full two-line slot. The icon box
    // matches the title line box and the row is top aligned, mirroring
    // section_heading.
    gpui::div()
        .flex()
        .items_start()
        .gap(px(8.))
        .child(
            gpui::div()
                .h(px(DISCOVER_SECTION_TITLE_HEIGHT_PX))
                .flex_none()
                .flex()
                .items_center()
                .relative()
                .top(px(DISCOVER_PROVIDER_ICON_OPTICAL_OFFSET_PX))
                .child(provider_icon(provider)),
        )
        .child(
            gpui::div()
                .min_w_0()
                .flex()
                .flex_col()
                .gap(px(DISCOVER_SECTION_TEXT_GAP_PX))
                .child(
                    gpui::div()
                        .h(px(DISCOVER_SECTION_TITLE_HEIGHT_PX))
                        .flex()
                        .items_center()
                        .child(
                            gpui::div()
                                .w(px(104.))
                                .h(px(15.))
                                .rounded(px(7.5))
                                .bg(rgb(SURFACE_RAISED)),
                        ),
                )
                .child(
                    gpui::div()
                        .h(px(DISCOVER_SECTION_SUBTITLE_HEIGHT_PX))
                        .flex()
                        .items_center()
                        .child(
                            gpui::div()
                                .w(px(78.))
                                .h(px(12.))
                                .rounded(px(6.))
                                .bg(rgb(SURFACE_RAISED)),
                        ),
                ),
        )
        .into_any_element()
}

fn discover_card_carousel(
    scroll_id: String,
    carousel: CardCarouselState,
    card_width: f32,
    row_gap: f32,
    controls_available: bool,
    content: AnyElement,
) -> AnyElement {
    let carousel = music_ui::card_carousel(
        scroll_id.clone(),
        carousel,
        card_width,
        row_gap,
        controls_available,
        content,
    );
    music_ui::horizontal_scroll_boundary(
        format!("{scroll_id}-boundary"),
        gpui::div()
            .w_full()
            .min_w_0()
            .when(!controls_available, |this| {
                this.pb(px(music_ui::CAROUSEL_CONTENT_BOTTOM_PADDING))
            })
            .child(carousel)
            .into_any_element(),
    )
    .into_any_element()
}

fn render_status(
    provider: Provider,
    title: &str,
    description: &str,
    retry: bool,
    host: &Entity<SearchView>,
    retry_channel: bool,
) -> AnyElement {
    let retry_button = gpui::div()
        .id(format!("discover-retry-{provider:?}"))
        .when(retry, |this| {
            let click_host = host.clone();
            let key_host = host.clone();
            this.focusable()
                .tab_stop(true)
                .role(gpui::Role::Button)
                .aria_label(format!("Retry {} Discover", provider.label()))
                .cursor_pointer()
                .px(px(10.))
                .py(px(6.))
                .rounded(px(6.))
                .border_1()
                .border_color(rgb(BORDER))
                .bg(rgb(SURFACE))
                .text_size(px(12.))
                .text_color(rgb(FOREGROUND))
                .hover(|style| style.bg(rgb(SURFACE_RAISED)))
                .on_click(move |_, _, app| {
                    click_host.update(app, |view, cx| {
                        if retry_channel {
                            view.retry_discover_channel(cx);
                        } else {
                            view.retry_discover(provider, cx);
                        }
                    });
                })
                .on_key_down(move |event: &KeyDownEvent, window, app| {
                    if crate::tab_keyboard::is_activation_key(event.keystroke.key.as_str()) {
                        window.prevent_default();
                        key_host.update(app, |view, cx| {
                            if retry_channel {
                                view.retry_discover_channel(cx);
                            } else {
                                view.retry_discover(provider, cx);
                            }
                        });
                    }
                })
                .child("Try again")
        });
    gpui::div()
        .w_full()
        .flex()
        .flex_col()
        .gap(px(DISCOVER_SECTION_HEADING_GAP_PX))
        .when(!retry_channel, |this| {
            this.child(static_heading(provider, title, None))
        })
        .child(
            gpui::div()
                .flex()
                .items_center()
                .justify_between()
                .gap(px(12.))
                .text_size(px(12.))
                .text_color(rgb(MUTED))
                .child(
                    gpui::div()
                        .min_w_0()
                        .flex_1()
                        .truncate()
                        .child(description.to_owned()),
                )
                .when(retry, |this| this.child(retry_button)),
        )
        .into_any_element()
}

fn static_heading(provider: Provider, title: &str, subtitle: Option<&str>) -> AnyElement {
    gpui::div()
        .flex()
        .items_center()
        .gap(px(8.))
        .child(provider_icon(provider))
        .child(
            gpui::div()
                .min_w_0()
                .flex()
                .flex_col()
                .gap(px(DISCOVER_SECTION_TEXT_GAP_PX))
                .child(
                    gpui::div()
                        .min_w_0()
                        .truncate()
                        .text_size(px(15.))
                        .font_weight(FontWeight::SEMIBOLD)
                        .text_color(rgb(FOREGROUND))
                        .child(title.to_owned()),
                )
                .when_some(subtitle, |this, subtitle| {
                    this.child(
                        gpui::div()
                            .min_w_0()
                            .truncate()
                            .text_size(px(12.))
                            .text_color(rgb(MUTED))
                            .child(subtitle.to_owned()),
                    )
                }),
        )
        .into_any_element()
}

fn channel_heading(title: &str) -> AnyElement {
    gpui::div()
        .flex_none()
        .h(px(DISCOVER_CHANNEL_HEADING_HEIGHT_PX))
        .flex()
        .items_center()
        .child(
            gpui::div()
                .relative()
                .top(px(DISCOVER_CHANNEL_HEADING_OPTICAL_Y_OFFSET_PX))
                .min_w_0()
                .truncate()
                .text_size(px(24.))
                .line_height(px(DISCOVER_CHANNEL_HEADING_TITLE_LINE_HEIGHT_PX))
                .font_weight(FontWeight::SEMIBOLD)
                .text_color(rgb(FOREGROUND))
                .child(title.to_owned()),
        )
        .into_any_element()
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::music_ui;
    use crate::search::models::{Card, ResultType};

    #[test]
    fn discover_item_identity_includes_height_affecting_card_metadata() {
        let mut item = DiscoverItem {
            card: Card {
                kind: ResultType::Albums,
                id: "album-1".into(),
                title: "First title".into(),
                subtitle: "First subtitle".into(),
                badge: "2026".into(),
                release_date: "2026-01-01".into(),
                source: Provider::Deezer,
                ..Card::default()
            },
            action: super::super::DiscoverAction::OpenDetail,
        };
        let initial = discover_item_content_identity(&item);

        item.card.title = "Second title".into();
        assert_ne!(initial, discover_item_content_identity(&item));
        item.card.title = "First title".into();
        item.card.subtitle = "Second subtitle".into();
        assert_ne!(initial, discover_item_content_identity(&item));
    }

    #[test]
    fn section_geometry_is_derived_from_card_metrics_for_both_layouts() {
        let desktop = section_geometry(false);
        let narrow = section_geometry(true);

        assert_eq!(desktop.card_width, 150.);
        assert_eq!(narrow.card_width, 126.);
        assert_eq!(
            desktop.card_row_height,
            150. + DETAIL_CARD_GRID_ROW_HEIGHT_EXTRA_PX
        );
        assert_eq!(
            narrow.card_row_height,
            126. + DETAIL_CARD_GRID_ROW_HEIGHT_EXTRA_PX
        );
        assert_eq!(desktop.heading_height, 38.);
        assert_eq!(narrow.heading_height, 38.);
        assert_eq!(desktop.row_height, 286.);
        assert_eq!(narrow.row_height, 262.);
        assert_eq!(desktop.heading_gap, narrow.heading_gap);
        assert_eq!(
            desktop.carousel_reserve,
            music_ui::CAROUSEL_CONTENT_BOTTOM_PADDING
        );
    }

    #[test]
    fn section_rows_measure_natural_heights_with_the_exact_subtitle_delta() {
        // Rows measure their natural height: a subtitle adds exactly the
        // subtitle line and the text gap, and nothing else in the row
        // changes, so one-line sections sit 18px higher in the feed instead
        // of leaving dead space at the bottom of a pinned row. Skeleton
        // rows reserve both slots, so their natural height is the two-line
        // shape the geometry describes.
        let subtitle_delta = DISCOVER_SECTION_SUBTITLE_HEIGHT_PX + DISCOVER_SECTION_TEXT_GAP_PX;
        assert_eq!(subtitle_delta, 18.);
        for narrow in [false, true] {
            let geometry = section_geometry(narrow);
            let two_line = section_heading_height()
                + geometry.heading_gap
                + DISCOVER_SECTION_CAROUSEL_INSET_PX
                + geometry.card_row_height
                + geometry.carousel_reserve
                + DISCOVER_SECTION_GAP_PX;
            let one_line = DISCOVER_SECTION_TITLE_HEIGHT_PX
                + geometry.heading_gap
                + DISCOVER_SECTION_CAROUSEL_INSET_PX
                + geometry.card_row_height
                + geometry.carousel_reserve
                + DISCOVER_SECTION_GAP_PX;
            assert_eq!(geometry.row_height, two_line);
            assert_eq!(two_line - one_line, subtitle_delta);
        }
        assert_eq!(section_heading_height(), 20. + 2. + 16.);
        assert_eq!(section_geometry(false).card_row_height, 196.);
        assert_eq!(loading_row_count(0., false), 3);
        assert!(loading_row_count(700., false) >= 4);
    }

    // Measure a repeated row through a real measured-all list, the same
    // path the feed uses: the extent reveals the exact per-row height.
    fn measured_row_height(
        cx: &mut gpui::VisualTestContext,
        width: f32,
        rows: usize,
        build: Rc<dyn Fn(usize) -> AnyElement>,
    ) -> f32 {
        struct Rows(gpui::ListState, Rc<dyn Fn(usize) -> AnyElement>);
        impl gpui::Render for Rows {
            fn render(
                &mut self,
                _: &mut gpui::Window,
                _: &mut gpui::Context<Self>,
            ) -> impl gpui::IntoElement {
                let state = self.0.clone();
                let build = self.1.clone();
                gpui::div().size_full().child(
                    gpui::list(state, move |ix, _, _| build(ix))
                        .w_full()
                        .h_full(),
                )
            }
        }
        let state = gpui::ListState::new(rows, gpui::ListAlignment::Top, px(0.)).measure_all();
        let view = cx.update(|_, cx| cx.new(|_| Rows(state.clone(), build)));
        cx.draw(
            gpui::point(px(0.), px(0.)),
            gpui::size(px(width), px(100.)),
            {
                let view = view.clone();
                move |_, _| view.into_any_element()
            },
        );
        (f32::from(state.max_offset_for_scrollbar().y) + 100.) / rows as f32
    }

    #[gpui::test]
    fn section_headings_measure_their_natural_heights(cx: &mut gpui::TestAppContext) {
        fn section_with_subtitle(subtitle: &str) -> DiscoverSection {
            DiscoverSection {
                id: "section-1".into(),
                provider: Provider::Deezer,
                title: "Your top genres".into(),
                subtitle: subtitle.into(),
                items: Vec::new(),
            }
        }

        let cx = cx.add_empty_window();
        let subtitled_section = section_with_subtitle("Based on your listening");
        let plain_section = section_with_subtitle("   ");
        let subtitled = measured_row_height(cx, 400., 10, {
            let section = subtitled_section.clone();
            Rc::new(move |_: usize| section_heading(&section).into_any_element())
        });
        let plain = measured_row_height(cx, 400., 10, {
            let section = plain_section.clone();
            Rc::new(move |_: usize| section_heading(&section).into_any_element())
        });
        let skeleton = measured_row_height(
            cx,
            400.,
            10,
            Rc::new(|_: usize| skeleton_heading(Provider::Deezer).into_any_element()),
        );

        let subtitle_delta = DISCOVER_SECTION_SUBTITLE_HEIGHT_PX + DISCOVER_SECTION_TEXT_GAP_PX;
        assert_eq!(subtitled, section_heading_height());
        assert_eq!(plain, DISCOVER_SECTION_TITLE_HEIGHT_PX);
        assert_eq!(subtitled - plain, subtitle_delta);
        // Skeleton headings reserve both text slots, so loading rows keep
        // the two-line height until the real sections replace them.
        assert_eq!(skeleton, subtitled);
    }

    #[gpui::test]
    fn discover_rows_keep_their_natural_height_across_carousel_control_states(
        cx: &mut gpui::TestAppContext,
    ) {
        // The horizontal carousel scrollbar and its arrows are overlays, and
        // the control lane reserve is unconditional: with controls the
        // scroll viewport carries the bottom padding, without them the
        // carousel wrapper does. A resize that flips the controls on or off
        // therefore cannot change any row's measured height or shift the
        // feed while the list re-measures at the new width.
        let (card_width, row_gap) = music_ui::card_row_metrics(false);
        assert!(music_ui::card_carousel_has_overflow(
            DISCOVER_CARD_PREVIEW_LIMIT,
            card_width,
            row_gap,
            400.,
            2.,
        ));
        assert!(!music_ui::card_carousel_has_overflow(
            DISCOVER_CARD_PREVIEW_LIMIT,
            card_width,
            row_gap,
            4000.,
            2.,
        ));

        let carousel = CardCarouselState::new();
        cx.update(|cx| {
            gpui_component::init(cx);
            crate::theme::configure_component_theme(cx);
        });
        let cx = cx.add_empty_window();
        let with_controls = measured_row_height(cx, 400., 3, {
            let carousel = carousel.clone();
            Rc::new(move |_: usize| {
                render_loading(Provider::Deezer, 2, false, false, 400., carousel.clone())
                    .into_any_element()
            })
        });
        let without_controls = measured_row_height(cx, 4000., 3, {
            let carousel = carousel.clone();
            Rc::new(move |_: usize| {
                render_loading(Provider::Deezer, 2, false, false, 4000., carousel.clone())
                    .into_any_element()
            })
        });

        // Both control states measure the same natural skeleton row: the
        // full two-line geometry the estimate is derived from.
        assert_eq!(with_controls, section_geometry(false).row_height);
        assert_eq!(without_controls, section_geometry(false).row_height);
        assert_eq!(with_controls, 286.);
    }

    #[gpui::test]
    fn loading_rows_measure_their_recorded_card_line_count(cx: &mut gpui::TestAppContext) {
        // A loading stand-in renders the card line count recorded for the
        // section position it stands in for, for either provider and either
        // layout: the single-line row drops exactly the subtitle chin (its
        // top margin plus the subtitle row) from the card body, while the
        // default two-line row keeps the full geometry.
        let subtitle_row = music_ui::COLLECTION_CARD_SUBTITLE_ROW_HEIGHT_PX + 2.;
        cx.update(|cx| {
            gpui_component::init(cx);
            crate::theme::configure_component_theme(cx);
        });
        let cx = cx.add_empty_window();
        let carousel = CardCarouselState::new();
        for narrow in [false, true] {
            for provider in [Provider::Deezer, Provider::SoundCloud] {
                let single_line = measured_row_height(cx, 4000., 3, {
                    let carousel = carousel.clone();
                    Rc::new(move |_: usize| {
                        render_loading(provider, 0, true, narrow, 4000., carousel.clone())
                            .into_any_element()
                    })
                });
                let two_line = measured_row_height(cx, 4000., 3, {
                    let carousel = carousel.clone();
                    Rc::new(move |_: usize| {
                        render_loading(provider, 0, false, narrow, 4000., carousel.clone())
                            .into_any_element()
                    })
                });
                assert_eq!(two_line, section_geometry(narrow).row_height);
                assert_eq!(
                    single_line,
                    section_geometry(narrow).row_height - subtitle_row
                );
            }
        }
    }

    #[test]
    fn loading_occurrence_ordinals_count_per_provider() {
        let carousel = CardCarouselState::new();
        let section = DiscoverFeedEntry::Section {
            section: DiscoverSection {
                id: "section".into(),
                provider: Provider::Deezer,
                title: "Made for you".into(),
                subtitle: String::new(),
                items: Vec::new(),
            },
            carousel: carousel.clone(),
        };
        let entries = vec![
            section,
            DiscoverFeedEntry::Loading {
                provider: Provider::Deezer,
                index: 0,
                single_line: true,
                carousel: carousel.clone(),
            },
            DiscoverFeedEntry::Loading {
                provider: Provider::SoundCloud,
                index: 1,
                single_line: false,
                carousel: carousel.clone(),
            },
            DiscoverFeedEntry::Loading {
                provider: Provider::Deezer,
                index: 2,
                single_line: false,
                carousel: carousel.clone(),
            },
            DiscoverFeedEntry::ChannelLoading {
                provider: Provider::Deezer,
                index: 0,
                carousel: carousel.clone(),
            },
        ];
        // Only the provider's own loading stand-ins count: sections and
        // channel skeletons never advance a provider's ordinal.
        assert_eq!(loading_occurrence_ordinal(&entries, Provider::Deezer), 2);
        assert_eq!(
            loading_occurrence_ordinal(&entries, Provider::SoundCloud),
            1
        );
        assert_eq!(loading_occurrence_ordinal(&[], Provider::Deezer), 0);
    }

    #[test]
    fn all_mode_initial_loading_keeps_both_providers_visible() {
        assert!(!should_defer_all_soundcloud_loading(
            Source::All,
            &DiscoverStatus::Loading,
            &DiscoverStatus::Loading,
        ));
        assert!(!should_defer_all_soundcloud_loading(
            Source::All,
            &DiscoverStatus::Idle,
            &DiscoverStatus::Loading,
        ));
        assert!(!should_defer_all_soundcloud_loading(
            Source::All,
            &DiscoverStatus::Ready,
            &DiscoverStatus::Loading,
        ));
        assert!(!should_defer_all_soundcloud_loading(
            Source::All,
            &DiscoverStatus::Loading,
            &DiscoverStatus::Ready,
        ));
        assert!(!should_defer_all_soundcloud_loading(
            Source::Deezer,
            &DiscoverStatus::Loading,
            &DiscoverStatus::Loading,
        ));
        assert!(!should_defer_all_soundcloud_loading(
            Source::SoundCloud,
            &DiscoverStatus::Loading,
            &DiscoverStatus::Loading,
        ));
        assert!(loading_row_count(700., false) >= 4);
    }
}
