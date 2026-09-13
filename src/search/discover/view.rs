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

const DISCOVER_CARD_PREVIEW_LIMIT: usize = 24;
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

#[derive(Clone)]
enum DiscoverFeedEntry {
    Section {
        section: DiscoverSection,
        carousel: CardCarouselState,
    },
    Loading {
        provider: Provider,
        index: usize,
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

/// Every section heading reserves the subtitle line, empty when absent, so
/// subtitled and plain sections measure identical heading heights.
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
    // Rows are pinned to the unified section geometry so the uniform height
    // hint matches what every row actually measures. Headings and card
    // captions reserve their subtitle lines, so section and loading rows fill
    // the pinned height exactly and the scrollbar never corrects mid-scroll.
    let row_height = section_geometry(narrow).row_height;
    let (state, browser_scroll) = view.discover_feed_state(
        &cache_identity,
        &content_identity,
        entries.len(),
        row_height,
    );
    let entries = Rc::new(entries);
    let host = host.clone();
    let account = view.account.clone();
    let list = gpui::list(state.clone(), move |index, _window, _app| {
        render_entry(
            &entries[index],
            &host,
            &account,
            available_width,
            narrow,
            row_height,
        )
    });
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
        let scroll_id = format!("discover-loading-carousel-{provider:?}-{index}");
        entries.push(DiscoverFeedEntry::Loading {
            provider,
            index,
            carousel: view.card_scroll_handle(&scroll_id),
        });
        content_identity.push_str(&format!("|loading:{provider:?}:{index}"));
    }
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
            content_identity.push_str(&format!("|{provider:?}:loading"));
            let scroll_id = format!("discover-loading-carousel-{provider:?}-0");
            entries.push(DiscoverFeedEntry::Loading {
                provider,
                index: 0,
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
    row_height: f32,
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
            carousel,
        } => render_loading(*provider, *index, narrow, available_width, carousel.clone()),
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
    let pins_height = matches!(
        entry,
        DiscoverFeedEntry::Section { .. }
            | DiscoverFeedEntry::Loading { .. }
            | DiscoverFeedEntry::ChannelLoading { .. }
    );
    gpui::div()
        .w_full()
        .flex_none()
        .when(pins_height, |this| this.h(px(row_height)))
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
    // The subtitle line is always reserved, empty when absent, so every
    // section heading measures the same height inside its pinned row. The
    // heading row is top aligned and the icon box matches the title line
    // box, so the icon rides the title line in single and double line
    // headings alike.
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
                .child(
                    gpui::div()
                        .min_w_0()
                        .truncate()
                        .h(px(DISCOVER_SECTION_SUBTITLE_HEIGHT_PX))
                        .text_size(px(12.))
                        .line_height(px(DISCOVER_SECTION_SUBTITLE_HEIGHT_PX))
                        .text_color(rgb(MUTED))
                        .when(has_subtitle, |this| this.child(section.subtitle.clone())),
                ),
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
    narrow: bool,
    available_width: f32,
    carousel: CardCarouselState,
) -> AnyElement {
    let geometry = section_geometry(narrow);
    let card_width = geometry.card_width;
    let (_, row_gap) = music_ui::card_row_metrics(narrow);
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
                            .w(px(88.))
                            .h(px(music_ui::COLLECTION_CARD_TITLE_LINE_HEIGHT_PX))
                            .rounded(px(8.))
                            .bg(rgb(SURFACE_RAISED)),
                    )
                    .child(
                        gpui::div()
                            .mt(px(2.))
                            .w(px(70.))
                            .h(px(music_ui::COLLECTION_CARD_SUBTITLE_ROW_HEIGHT_PX))
                            .rounded(px(8.))
                            .bg(rgb(SURFACE_RAISED)),
                    ),
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
    // Both text slots are reserved for every provider so skeleton headings
    // measure exactly the section heading height. The icon box matches the
    // title line box and the row is top aligned, mirroring section_heading.
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
    fn discover_feed_uses_a_virtualized_list_and_discover_card_renderer() {
        let implementation = include_str!("view.rs")
            .split_once("#[cfg(test)]")
            .map_or(include_str!("view.rs"), |(production, _)| production);
        assert!(implementation.contains("gpui::list(state.clone()"));
        assert!(implementation.contains(".when(pins_height, |this| this.h(px(row_height)))"));
        assert!(implementation.contains("BrowserScrollTarget::List(state)"));
        assert!(implementation.contains("render_discover_card"));
    }

    #[test]
    fn discover_feed_identity_ignores_layout_affordances() {
        let implementation = include_str!("view.rs");
        let sections = implementation
            .split_once("fn append_sections")
            .and_then(|(_, rest)| rest.split_once("fn render_entry"))
            .expect("discover section identity source");
        assert!(sections.0.contains("section:{}:{}:{}"));
        assert!(!sections.0.contains("controls_available"));
        assert!(!sections.0.contains("controls="));
    }

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
    fn section_logos_use_the_baseline_optical_offset() {
        let implementation = include_str!("view.rs");
        assert!(
            implementation.contains("const DISCOVER_PROVIDER_ICON_OPTICAL_OFFSET_PX: f32 = 0.")
        );
        assert!(implementation.contains("top(px(DISCOVER_PROVIDER_ICON_OPTICAL_OFFSET_PX))"));
        assert!(implementation.contains("fn skeleton_heading(provider: Provider)"));
    }

    #[test]
    fn loading_headings_are_skeletons_without_visible_loading_copy() {
        let implementation = include_str!("view.rs")
            .split_once("#[cfg(test)]")
            .map_or(include_str!("view.rs"), |(production, _)| production);
        assert!(!implementation.contains("Loading home"));
        assert!(!implementation.contains("Personalized picks"));
        let loading = implementation
            .split("fn render_loading")
            .nth(1)
            .and_then(|source| source.split("fn render_channel_loading").next())
            .expect("loading renderer source");
        assert!(loading.contains("skeleton_heading(provider)"));
        assert!(!loading.contains("static_heading("));
    }

    #[test]
    fn account_required_entries_use_shared_empty_state_and_settings_action() {
        assert_eq!(
            crate::empty_state::account_required_copy(Provider::Deezer.label()).0,
            "Deezer account required"
        );
        assert_eq!(
            crate::empty_state::account_required_copy(Provider::SoundCloud.label()).0,
            "SoundCloud account required"
        );

        let production = include_str!("view.rs")
            .split_once("#[cfg(test)]")
            .map_or(include_str!("view.rs"), |(production, _)| production);
        assert!(production.contains("AccountRequired {\n        provider: Provider,"));
        assert!(production.contains("DiscoverFeedEntry::AccountRequired { provider }"));
        assert!(production.contains("crate::empty_state::render("));
        assert!(production.contains("primary_button("));
        assert!(production.contains("\"Open account settings\""));
        assert!(production.contains("view.open_account_settings(window, cx)"));
        assert!(!production.contains("title: \"Account required\""));

        let search_view = include_str!("../view.rs");
        assert!(search_view.contains("pub(crate) fn open_account_settings("));
        assert!(search_view.contains("library.open_settings(window, cx)"));
    }

    #[test]
    fn loading_headings_reserve_the_subtitle_line_for_every_provider() {
        // Skeleton headings mirror the real heading geometry for both
        // providers so loading rows measure the pinned row height exactly.
        assert_eq!(section_heading_height(), 20. + 2. + 16.);
        let production = include_str!("view.rs")
            .split_once("#[cfg(test)]")
            .map_or(include_str!("view.rs"), |(production, _)| production);
        assert!(!production.contains("skeleton_heading_has_subtitle"));
        let heading = production
            .split("fn skeleton_heading")
            .nth(1)
            .and_then(|source| source.split("fn discover_card_carousel").next())
            .expect("skeleton heading source");
        assert!(heading.contains(".h(px(DISCOVER_SECTION_TITLE_HEIGHT_PX))"));
        assert!(heading.contains(".h(px(DISCOVER_SECTION_SUBTITLE_HEIGHT_PX))"));
        assert!(heading.contains(".items_start()"));
        assert!(heading.contains("top(px(DISCOVER_PROVIDER_ICON_OPTICAL_OFFSET_PX))"));
        assert!(!heading.contains(".h(px(18.))"));
    }

    #[test]
    fn section_headings_reserve_the_subtitle_line() {
        let production = include_str!("view.rs")
            .split_once("#[cfg(test)]")
            .map_or(include_str!("view.rs"), |(production, _)| production);
        let heading = production
            .split("fn section_heading(section: &DiscoverSection)")
            .nth(1)
            .and_then(|source| source.split("fn provider_icon").next())
            .expect("section heading source");
        assert!(heading.contains(".line_height(px(DISCOVER_SECTION_TITLE_HEIGHT_PX))"));
        assert!(heading.contains(".h(px(DISCOVER_SECTION_SUBTITLE_HEIGHT_PX))"));
        assert!(heading.contains(".line_height(px(DISCOVER_SECTION_SUBTITLE_HEIGHT_PX))"));
        assert!(!heading.contains(".when(!section.subtitle.trim().is_empty()"));
        assert!(heading.contains(".items_start()"));
        assert!(heading.contains(".h(px(DISCOVER_SECTION_TITLE_HEIGHT_PX))"));
        assert!(!heading.contains(".h(px(18.))"));
    }

    #[test]
    fn channel_heading_is_text_only_and_compact() {
        let implementation = include_str!("view.rs");
        assert!(implementation.contains("const DISCOVER_CHANNEL_HEADING_HEIGHT_PX: f32 = 42."));
        assert!(
            implementation
                .contains("const DISCOVER_CHANNEL_HEADING_TITLE_LINE_HEIGHT_PX: f32 = 30.")
        );
        assert!(implementation.contains(".text_size(px(24.))"));
        let heading = implementation
            .split("fn channel_heading")
            .nth(1)
            .and_then(|source| source.split("#[cfg(test)]").next())
            .expect("channel heading source");
        assert!(!heading.contains("provider_icon"));
        assert!(!heading.contains("h(px(54.))"));
    }

    #[test]
    fn channel_heading_has_symmetric_optical_padding() {
        assert_eq!(DISCOVER_CHANNEL_HEADING_HEIGHT_PX, 42.);
        assert_eq!(DISCOVER_CHANNEL_HEADING_TITLE_LINE_HEIGHT_PX, 30.);
        assert_eq!(DISCOVER_CHANNEL_HEADING_OPTICAL_Y_OFFSET_PX, -1.);
        let top_padding = (DISCOVER_CHANNEL_HEADING_HEIGHT_PX
            - DISCOVER_CHANNEL_HEADING_TITLE_LINE_HEIGHT_PX)
            / 2.;
        let bottom_padding = DISCOVER_CHANNEL_HEADING_HEIGHT_PX
            - DISCOVER_CHANNEL_HEADING_TITLE_LINE_HEIGHT_PX
            - top_padding;
        assert_eq!(top_padding, 6.);
        assert_eq!(top_padding, bottom_padding);

        let implementation = include_str!("view.rs");
        let production = implementation
            .split_once("#[cfg(test)]")
            .map_or(implementation, |(production, _)| production);
        let heading = implementation
            .split("fn channel_heading")
            .nth(1)
            .and_then(|source| source.split("#[cfg(test)]").next())
            .expect("channel heading source");
        assert!(heading.contains(".items_center()"));
        assert!(heading.contains(
            ".relative()\n                .top(px(DISCOVER_CHANNEL_HEADING_OPTICAL_Y_OFFSET_PX))"
        ));
        assert!(
            heading.contains(".line_height(px(DISCOVER_CHANNEL_HEADING_TITLE_LINE_HEIGHT_PX))")
        );
        assert!(!heading.contains(".items_start()"));
        assert!(!heading.contains(".pt("));
        assert!(!heading.contains(".pb("));
        assert!(
            production
                .contains(".child(channel_heading(&channel.title))\n            .child(feed)")
        );
    }

    #[test]
    fn channel_loading_uses_the_persistent_carousel_path() {
        let implementation = include_str!("view.rs");
        let loading = implementation
            .split("fn render_channel_loading")
            .nth(1)
            .and_then(|source| source.split("fn render_status").next())
            .expect("channel loading source");
        assert!(loading.contains("card_carousel_has_overflow"));
        assert!(loading.contains("horizontal_scroll_boundary"));
        assert!(loading.contains("card_carousel("));
        assert!(loading.contains("collection_card_carousel_content"));
        assert!(loading.contains("SKELETON_CARD_COUNT: usize = DISCOVER_CARD_PREVIEW_LIMIT"));
        assert!(implementation.contains("carousel: view.card_scroll_handle(&scroll_id)"));
    }

    #[test]
    fn all_discover_section_states_share_heading_and_carousel_reserve() {
        let implementation = include_str!("view.rs")
            .split_once("#[cfg(test)]")
            .map_or(include_str!("view.rs"), |(production, _)| production);
        assert!(implementation.contains("fn section_heading_height()"));
        assert!(implementation.contains("fn section_geometry(narrow: bool)"));
        assert!(implementation.contains("DETAIL_CARD_GRID_ROW_HEIGHT_EXTRA_PX"));
        assert!(implementation.contains("DISCOVER_SECTION_HEADING_GAP_PX: f32 = 10."));
        assert!(implementation.contains("DISCOVER_SECTION_CAROUSEL_INSET_PX"));
        assert!(implementation.contains("fn discover_card_carousel("));
        assert!(implementation.contains("CAROUSEL_CONTENT_BOTTOM_PADDING"));
        assert!(implementation.contains(".when(!controls_available"));
        assert!(implementation.contains("section_heading(section)"));
        assert!(implementation.contains("static_heading("));
        assert!(implementation.contains("fn skeleton_heading(provider: Provider)"));
        assert!(implementation.contains(".pb(px(DISCOVER_SECTION_GAP_PX))"));
        assert!(!implementation.contains("metadata_compensation"));
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
    fn section_rows_measure_the_pinned_row_height_exactly() {
        // The pinned row height is the sum of the rendered content parts:
        // the heading (title line, text gap, reserved subtitle slot), the
        // heading gap, the carousel inset, the card row, the carousel
        // reserve, and the section gap padding.
        for narrow in [false, true] {
            let geometry = section_geometry(narrow);
            let content_height = section_heading_height()
                + geometry.heading_gap
                + DISCOVER_SECTION_CAROUSEL_INSET_PX
                + geometry.card_row_height
                + geometry.carousel_reserve
                + DISCOVER_SECTION_GAP_PX;
            assert_eq!(geometry.row_height, content_height);
        }
        assert_eq!(section_heading_height(), 20. + 2. + 16.);
        assert_eq!(section_geometry(false).card_row_height, 196.);
        assert_eq!(loading_row_count(0., false), 3);
        assert!(loading_row_count(700., false) >= 4);
    }

    #[test]
    fn loading_card_captions_reserve_the_subtitle_line_for_every_provider() {
        let production = include_str!("view.rs")
            .split_once("#[cfg(test)]")
            .map_or(include_str!("view.rs"), |(production, _)| production);
        let loading = production
            .split("fn render_loading")
            .nth(1)
            .and_then(|source| source.split("fn render_channel_loading").next())
            .expect("loading renderer source");
        let channel_loading = production
            .split("fn render_channel_loading")
            .nth(1)
            .and_then(|source| source.split("fn skeleton_heading").next())
            .expect("channel loading renderer source");
        for renderer in [loading, channel_loading] {
            assert!(renderer.contains("COLLECTION_CARD_TITLE_LINE_HEIGHT_PX"));
            assert!(renderer.contains("COLLECTION_CARD_SUBTITLE_ROW_HEIGHT_PX"));
            assert!(!renderer.contains("skeleton_heading_has_subtitle"));
        }
    }

    #[test]
    fn loading_overdraw_has_unique_indexed_rows() {
        let implementation = include_str!("view.rs");
        assert!(implementation.contains("DISCOVER_LOADING_OVERDRAW_ROWS"));
        assert!(implementation.contains("discover-loading-carousel-{provider:?}-{index}"));
        assert!(implementation.contains("discover-loading-{provider:?}-{index}-{card_index}"));
        let results = include_str!("../results_view.rs");
        assert!(results.contains(
            "available_width,\n                available_height,\n                narrow"
        ));
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
