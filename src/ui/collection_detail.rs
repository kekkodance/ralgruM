use std::path::PathBuf;

use gpui::{AnyElement, Div, FontWeight, ObjectFit, div, img, prelude::*, px, rgb};

use crate::{
    assets::{LocalIcon, local_icon},
    library::Service,
    music_ui::{CardKind, CollectionCardTitleAlignment},
    search::{Provider, ResultType},
    theme::{BORDER, DEEZER, MUTED, SOUNDCLOUD, SURFACE_RAISED},
};

pub(crate) const DETAIL_HEADER_ARTWORK_SIZE_PX: f32 = 76.;
pub(crate) const DETAIL_HEADER_ARTWORK_RADIUS_PX: f32 = 6.;
pub(crate) const DETAIL_HEADER_GAP_PX: f32 = 16.;
pub(crate) const DETAIL_HEADER_TITLE_GAP_PX: f32 = 10.;
pub(crate) const DETAIL_HEADER_STACK_GAP_PX: f32 = 4.;
pub(crate) const DETAIL_PROVIDER_GAP_PX: f32 = 8.;
pub(crate) const DETAIL_CONTEXT_OPTICAL_OFFSET_PX: f32 = 2.;
pub(crate) const DETAIL_PROVIDER_RIGHT_MARGIN_PX: f32 = 14.;
pub(crate) const DETAIL_SECTION_GAP_PX: f32 = 18.;
pub(crate) const DETAIL_SECTION_HEADER_GAP_PX: f32 = 12.;
pub(crate) const DETAIL_SECTION_ACTION_GAP_PX: f32 = 9.;
pub(crate) const DETAIL_SECTION_CONTENT_GAP_PX: f32 = 9.;
pub(crate) const DETAIL_SECTION_TITLE_SIZE_PX: f32 = 15.;
pub(crate) const DETAIL_CARD_GAP_PX: f32 = 7.;
pub(crate) const DETAIL_CARD_CAROUSEL_INSET_PX: f32 = 2.;
pub(crate) const DETAIL_CARD_GRID_GAP_PX: f32 = 12.;
pub(crate) const DETAIL_CARD_GRID_ROW_HEIGHT_EXTRA_PX: f32 = 46.;

pub(crate) fn collection_card_carousel_content(
    row_gap: f32,
    inset: bool,
    cards: impl IntoIterator<Item = AnyElement>,
) -> Div {
    div()
        .flex()
        .flex_none()
        .gap(px(row_gap))
        .when(inset, |this| {
            this.px(px(DETAIL_CARD_CAROUSEL_INSET_PX))
                .pt(px(DETAIL_CARD_CAROUSEL_INSET_PX))
        })
        .children(cards)
}

/// The shared visual shell used by collection cards in Search and Library.
/// Surface-specific navigation and context menus are deliberately applied by
/// the caller around this shell.
pub(crate) fn collection_card_frame(carousel: bool, narrow: bool, content: AnyElement) -> Div {
    div()
        .min_w_0()
        .when(carousel, |this| {
            let (card_width, _) = crate::music_ui::card_row_metrics(narrow);
            this.w(px(card_width)).flex_none()
        })
        .p(px(if narrow { 6. } else { 8. }))
        .rounded(px(if narrow { 8. } else { 12. }))
        .border_1()
        .border_color(rgb(BORDER))
        .bg(rgb(crate::theme::SURFACE))
        .hover(|style| {
            style
                .border_color(rgb(0x3f3f46))
                .bg(rgb(0x17171a))
                .shadow_sm()
        })
        .flex()
        .flex_col()
        .gap(px(DETAIL_CARD_GAP_PX))
        .child(content)
}

/// Shared card body data for provider collection cards. Keeping provider and
/// badge presentation here prevents Search and Library from drifting apart.
pub(crate) fn collection_card_content(
    title: &str,
    subtitle: &str,
    artwork: &str,
    kind: CardKind,
    provider: Provider,
    badge: &str,
    is_private: Option<bool>,
    id: &str,
) -> AnyElement {
    collection_card_content_with_presentation(
        title,
        subtitle,
        artwork,
        kind,
        provider,
        badge,
        is_private,
        id,
        CollectionCardPresentation::default(),
    )
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub(crate) struct CollectionCardPresentation {
    pub(crate) show_provider: bool,
    pub(crate) show_badge: bool,
    pub(crate) show_privacy: bool,
    pub(crate) title_alignment: CollectionCardTitleAlignment,
}

impl CollectionCardPresentation {
    pub(crate) const fn discover() -> Self {
        Self {
            show_provider: false,
            show_badge: false,
            show_privacy: false,
            title_alignment: CollectionCardTitleAlignment::Left,
        }
    }
}

impl Default for CollectionCardPresentation {
    fn default() -> Self {
        Self {
            show_provider: true,
            show_badge: true,
            show_privacy: true,
            title_alignment: CollectionCardTitleAlignment::KindDefault,
        }
    }
}

pub(crate) fn collection_card_content_with_presentation(
    title: &str,
    subtitle: &str,
    artwork: &str,
    kind: CardKind,
    provider: Provider,
    badge: &str,
    is_private: Option<bool>,
    id: &str,
    presentation: CollectionCardPresentation,
) -> AnyElement {
    let badge = (!badge.trim().is_empty()).then_some(badge);
    let effective_privacy = presentation.show_privacy.then_some(is_private).flatten();
    let content = crate::music_ui::collection_card_with_title_alignment(
        title,
        subtitle,
        artwork,
        kind,
        presentation.show_provider.then_some(provider),
        presentation.show_badge.then_some(badge).flatten(),
        effective_privacy,
        id,
        presentation.title_alignment,
    );
    content
}

pub(crate) struct ProviderHeaderSpec {
    pub(crate) artwork: String,
    pub(crate) title: String,
    pub(crate) metadata: String,
    pub(crate) description: String,
    pub(crate) provider: Provider,
    pub(crate) kind: ResultType,
    pub(crate) total: Option<usize>,
    pub(crate) actions: Vec<AnyElement>,
    pub(crate) body_fills: bool,
}

pub(crate) struct LocalPlaylistHeaderSpec {
    pub(crate) artwork: Option<PathBuf>,
    pub(crate) title: String,
    pub(crate) description: String,
    pub(crate) actions: Vec<AnyElement>,
}

enum HeaderArtwork {
    Remote(String),
    Local(Option<PathBuf>),
}

enum HeaderContext {
    Provider(Provider),
    Local,
}

pub(crate) struct ArtistHeaderSpec {
    pub(crate) artwork: String,
    pub(crate) title: String,
    pub(crate) subtitle: String,
    pub(crate) provider: Provider,
    pub(crate) actions: Vec<AnyElement>,
}

fn provider_header_total(kind: ResultType, total: Option<usize>) -> Option<usize> {
    match kind {
        ResultType::Albums | ResultType::Playlists => None,
        ResultType::All | ResultType::Tracks | ResultType::Artists => total,
    }
}

pub(crate) fn render_provider_detail(spec: ProviderHeaderSpec, body: AnyElement) -> AnyElement {
    div()
        .w_full()
        .flex()
        .flex_col()
        .when(spec.body_fills, |this| this.flex_1().min_h_0())
        .gap(px(12.))
        .child(render_provider_header(spec))
        .child(body)
        .into_any_element()
}

pub(crate) fn render_provider_header(spec: ProviderHeaderSpec) -> AnyElement {
    let ProviderHeaderSpec {
        artwork: artwork_url,
        title,
        metadata,
        description,
        provider,
        kind,
        total,
        actions,
        body_fills: _,
    } = spec;
    let total = provider_header_total(kind, total);
    let header = render_collection_header(
        HeaderArtwork::Remote(artwork_url),
        title,
        metadata,
        HeaderContext::Provider(provider),
        total,
        actions,
    );
    div()
        .w_full()
        .flex()
        .flex_col()
        .gap(px(12.))
        .child(header)
        .when_some(
            detail_description(kind, &description),
            |this, description| {
                this.child(
                    div()
                        .text_size(px(12.5))
                        .line_height(px(18.))
                        .text_color(rgb(MUTED))
                        .child(description),
                )
            },
        )
        .into_any_element()
}

pub(crate) fn render_local_playlist_header(spec: LocalPlaylistHeaderSpec) -> AnyElement {
    render_collection_header(
        HeaderArtwork::Local(spec.artwork),
        spec.title,
        spec.description,
        HeaderContext::Local,
        None,
        spec.actions,
    )
}

fn render_collection_header(
    artwork_source: HeaderArtwork,
    title: String,
    metadata: String,
    context: HeaderContext,
    total: Option<usize>,
    actions: Vec<AnyElement>,
) -> AnyElement {
    let mut title_block = div()
        .flex_1()
        .min_w_0()
        .flex()
        .items_center()
        .gap(px(DETAIL_HEADER_TITLE_GAP_PX))
        .child(
            div()
                .flex_1()
                .min_w_0()
                .flex()
                .flex_col()
                .gap(px(DETAIL_HEADER_STACK_GAP_PX))
                .child(
                    div()
                        .min_w_0()
                        .truncate()
                        .text_size(px(18.))
                        .line_height(px(22.5))
                        .font_weight(FontWeight::SEMIBOLD)
                        .child(title),
                )
                .when(!metadata.trim().is_empty(), |this| {
                    this.child(
                        div()
                            .truncate()
                            .text_size(px(11.5))
                            .text_color(rgb(MUTED))
                            .child(metadata),
                    )
                }),
        );
    if !actions.is_empty() {
        title_block = title_block.children(actions);
    }
    div()
        .w_full()
        .flex()
        .items_center()
        .gap(px(DETAIL_HEADER_GAP_PX))
        .child(header_artwork(
            artwork_source,
            px(DETAIL_HEADER_ARTWORK_SIZE_PX),
            px(DETAIL_HEADER_ARTWORK_RADIUS_PX),
        ))
        .child(title_block)
        .child(
            div()
                .ml_auto()
                .mr(px(DETAIL_PROVIDER_RIGHT_MARGIN_PX))
                .flex_none()
                .flex()
                .items_center()
                .gap(px(DETAIL_PROVIDER_GAP_PX))
                .child(match context {
                    HeaderContext::Provider(provider) => provider_context(provider),
                    HeaderContext::Local => local_context(),
                })
                .when_some(total, |this, total| {
                    this.child(
                        div()
                            .mt(px(DETAIL_CONTEXT_OPTICAL_OFFSET_PX))
                            .whitespace_nowrap()
                            .text_size(px(11.5))
                            .text_color(rgb(MUTED))
                            .child(format!(
                                "{total} {}",
                                if total == 1 { "track" } else { "tracks" }
                            )),
                    )
                }),
        )
        .into_any_element()
}

pub(crate) fn render_artist_header(spec: ArtistHeaderSpec) -> AnyElement {
    let ArtistHeaderSpec {
        artwork: artwork_url,
        title,
        subtitle,
        provider,
        actions,
    } = spec;
    let mut title_block = div()
        .flex()
        .items_center()
        .gap(px(DETAIL_HEADER_TITLE_GAP_PX))
        .child(
            div()
                .flex()
                .flex_col()
                .gap(px(DETAIL_HEADER_STACK_GAP_PX))
                .child(
                    div()
                        .min_w_0()
                        .truncate()
                        .text_size(px(18.))
                        .line_height(px(22.5))
                        .font_weight(FontWeight::SEMIBOLD)
                        .child(title),
                )
                .when(!subtitle.trim().is_empty(), |this| {
                    this.child(
                        div()
                            .whitespace_nowrap()
                            .text_size(px(11.5))
                            .text_color(rgb(MUTED))
                            .child(subtitle),
                    )
                }),
        );
    if !actions.is_empty() {
        title_block = title_block.children(actions);
    }
    div()
        .w_full()
        .flex()
        .items_center()
        .justify_between()
        .child(
            div()
                .flex()
                .items_center()
                .gap(px(DETAIL_HEADER_GAP_PX))
                .child(artwork(
                    &artwork_url,
                    px(DETAIL_HEADER_ARTWORK_SIZE_PX),
                    px(DETAIL_HEADER_ARTWORK_RADIUS_PX),
                ))
                .child(title_block),
        )
        .child(
            div()
                .mr(px(DETAIL_PROVIDER_RIGHT_MARGIN_PX))
                .child(provider_context(provider)),
        )
        .into_any_element()
}

pub(crate) fn render_artist_section(
    title: &str,
    total: usize,
    loaded_count: usize,
    expanded: bool,
    hide_action: bool,
    action: Option<AnyElement>,
    body: AnyElement,
) -> AnyElement {
    div()
        .w_full()
        .flex()
        .flex_col()
        .gap(px(DETAIL_SECTION_CONTENT_GAP_PX))
        .child(render_artist_section_header(
            title,
            total,
            loaded_count,
            expanded,
            hide_action,
            action,
        ))
        .child(body)
        .into_any_element()
}

pub(crate) fn render_collection_empty(is_playlist: bool) -> AnyElement {
    crate::empty_state::render(
        LocalIcon::CompactDisc,
        if is_playlist {
            "This playlist is empty"
        } else {
            "Nothing here"
        },
        if is_playlist {
            "Add tracks to this playlist to get started."
        } else {
            "This result does not contain any items."
        },
        None,
    )
}

pub(crate) fn detail_description(kind: ResultType, description: &str) -> Option<String> {
    if description.trim().is_empty() || matches!(kind, ResultType::Albums | ResultType::Playlists) {
        None
    } else {
        Some(description.to_owned())
    }
}

pub(crate) fn render_artist_section_header(
    title: &str,
    total: usize,
    _loaded_count: usize,
    _expanded: bool,
    hide_action: bool,
    action: Option<AnyElement>,
) -> AnyElement {
    div()
        .w_full()
        .flex()
        .items_center()
        .justify_between()
        .gap(px(DETAIL_SECTION_HEADER_GAP_PX))
        .child(
            div()
                .min_w_0()
                .truncate()
                .text_size(px(DETAIL_SECTION_TITLE_SIZE_PX))
                .font_weight(FontWeight::SEMIBOLD)
                .child(title.to_owned()),
        )
        .when(!hide_action, |this| {
            this.child(
                div()
                    .flex_none()
                    .flex()
                    .items_center()
                    .gap(px(DETAIL_SECTION_ACTION_GAP_PX))
                    .child(
                        div()
                            .flex_none()
                            .text_size(px(11.))
                            .text_color(rgb(MUTED))
                            .child(format!("{total} found")),
                    )
                    .when_some(action, |this, action| this.child(action)),
            )
        })
        .into_any_element()
}

fn artwork(url: &str, size: gpui::Pixels, radius: gpui::Pixels) -> AnyElement {
    let base = div()
        .w(size)
        .h(size)
        .flex_none()
        .overflow_hidden()
        .rounded(radius)
        .border_1()
        .border_color(rgb(BORDER))
        .bg(rgb(SURFACE_RAISED));
    if url.is_empty() {
        base.into_any_element()
    } else {
        base.child(
            img(url.to_owned())
                .size_full()
                .rounded(radius)
                .object_fit(ObjectFit::Cover),
        )
        .into_any_element()
    }
}

fn header_artwork(source: HeaderArtwork, size: gpui::Pixels, radius: gpui::Pixels) -> AnyElement {
    let base = div()
        .w(size)
        .h(size)
        .flex_none()
        .overflow_hidden()
        .rounded(radius)
        .border_1()
        .border_color(rgb(BORDER))
        .bg(rgb(SURFACE_RAISED));
    match source {
        HeaderArtwork::Remote(url) if !url.is_empty() => base
            .child(
                img(url)
                    .size_full()
                    .rounded(radius)
                    .object_fit(ObjectFit::Cover),
            )
            .into_any_element(),
        HeaderArtwork::Local(Some(path)) => base
            .child(
                img(path.as_path())
                    .size_full()
                    .rounded(radius)
                    .object_fit(ObjectFit::Cover),
            )
            .into_any_element(),
        HeaderArtwork::Local(None) => base
            .flex()
            .items_center()
            .justify_center()
            .child(local_icon(LocalIcon::ListUl, 0x52525b).size(px(24.)))
            .into_any_element(),
        HeaderArtwork::Remote(_) => base.into_any_element(),
    }
}

pub(crate) fn provider_context(provider: Provider) -> AnyElement {
    let color = match provider {
        Provider::Deezer => DEEZER,
        Provider::SoundCloud => SOUNDCLOUD,
    };
    div()
        .flex_none()
        .mt(px(DETAIL_CONTEXT_OPTICAL_OFFSET_PX))
        .flex()
        .items_center()
        .gap(px(5.))
        .child(provider_logo(provider))
        .child(
            div()
                .text_size(px(11.5))
                .font_weight(FontWeight::MEDIUM)
                .text_color(rgb(color))
                .child(provider.label()),
        )
        .into_any_element()
}

fn local_context() -> AnyElement {
    div()
        .flex_none()
        .mt(px(DETAIL_CONTEXT_OPTICAL_OFFSET_PX))
        .flex()
        .items_center()
        .gap(px(5.))
        .child(local_icon(LocalIcon::FolderOpen, crate::theme::PRIMARY).size(px(13.)))
        .child(
            div()
                .text_size(px(11.5))
                .font_weight(FontWeight::MEDIUM)
                .text_color(rgb(crate::theme::PRIMARY))
                .child(Service::Local.label()),
        )
        .into_any_element()
}

fn provider_logo(provider: Provider) -> AnyElement {
    let (icon, color) = match provider {
        Provider::Deezer => (LocalIcon::Deezer, 0xa238ff),
        Provider::SoundCloud => (LocalIcon::SoundCloud, 0xff5500),
    };
    local_icon(icon, color).size(px(14.)).into_any_element()
}

#[cfg(test)]
mod tests {
    use super::{
        CollectionCardPresentation, CollectionCardTitleAlignment, DETAIL_CONTEXT_OPTICAL_OFFSET_PX,
        detail_description, provider_header_total,
    };
    use crate::search::ResultType;

    #[test]
    fn collection_description_policy_matches_search_for_albums_and_playlists() {
        assert_eq!(
            detail_description(ResultType::Albums, "SoundCloud bio"),
            None
        );
        assert_eq!(detail_description(ResultType::Playlists, "Details"), None);
        assert_eq!(
            detail_description(ResultType::Tracks, "A useful description"),
            Some("A useful description".to_owned())
        );
    }

    #[test]
    fn provider_context_keeps_the_existing_optical_offset() {
        assert_eq!(DETAIL_CONTEXT_OPTICAL_OFFSET_PX, 2.);
    }

    #[test]
    fn discover_hides_redundant_card_chips_without_changing_shared_defaults() {
        let standard = CollectionCardPresentation::default();
        assert!(standard.show_provider);
        assert!(standard.show_badge);
        assert!(standard.show_privacy);

        let discover = CollectionCardPresentation::discover();
        assert!(!discover.show_provider);
        assert!(!discover.show_badge);
        assert!(!discover.show_privacy);
        assert_eq!(
            standard.title_alignment,
            CollectionCardTitleAlignment::KindDefault
        );
        assert_eq!(discover.title_alignment, CollectionCardTitleAlignment::Left);
    }

    #[test]
    fn discover_cards_end_naturally_when_metadata_is_hidden() {
        let source = include_str!("collection_detail.rs")
            .split("#[cfg(test)]")
            .next()
            .expect("collection card production source");
        assert!(!source.contains("DISCOVER_CARD_METADATA_ROW_HEIGHT_PX"));
        assert!(!source.contains("presentation.reserve_metadata_row"));
    }

    #[test]
    fn provider_headers_use_the_artist_chip_right_inset() {
        assert_eq!(super::DETAIL_PROVIDER_RIGHT_MARGIN_PX, 14.);
        let source = include_str!("collection_detail.rs");
        let provider_header = source
            .split("pub(crate) fn render_provider_header")
            .nth(1)
            .and_then(|source| source.split("pub(crate) fn render_artist_header").next())
            .expect("provider header source");
        assert!(provider_header.contains(".mr(px(DETAIL_PROVIDER_RIGHT_MARGIN_PX))"));
    }

    #[test]
    fn provider_headers_hide_collection_totals_but_keep_track_totals() {
        assert_eq!(provider_header_total(ResultType::Albums, Some(12)), None);
        assert_eq!(provider_header_total(ResultType::Playlists, Some(12)), None);
        assert_eq!(
            provider_header_total(ResultType::Tracks, Some(12)),
            Some(12)
        );
        assert_eq!(
            provider_header_total(ResultType::Artists, Some(12)),
            Some(12)
        );
    }

    #[test]
    fn search_and_library_use_the_shared_provider_detail_composition() {
        let search = include_str!("../search/detail_view.rs");
        let library = include_str!("../library/content_view.rs");
        for source in [search, library] {
            assert!(source.contains("render_shared_artist_header"));
            assert!(source.contains("render_shared_artist_section"));
        }
        assert!(search.contains("render_shared_provider_detail"));
        assert!(library.contains("render_shared_provider_header"));
        assert!(search.contains("render_collection_empty"));
        assert!(library.contains("render_collection_empty"));
    }

    #[test]
    fn local_playlist_header_reuses_collection_geometry_and_truncates_description() {
        let source = include_str!("collection_detail.rs");
        let local = source
            .split("pub(crate) fn render_local_playlist_header")
            .nth(1)
            .and_then(|body| body.split("fn render_collection_header").next())
            .expect("Local playlist header should exist");
        assert!(local.contains("render_collection_header"));
        let shared = source
            .split("fn render_collection_header")
            .nth(1)
            .and_then(|body| body.split("pub(crate) fn render_artist_header").next())
            .expect("shared collection header should exist");
        assert!(shared.contains(".truncate()"));
        assert!(shared.contains("DETAIL_PROVIDER_RIGHT_MARGIN_PX"));
        assert!(shared.contains("HeaderContext::Local => local_context()"));
    }
}
