use super::model::{Card, Page, Section, Track};

pub(crate) fn filtered_page(page: &Page, query: &str) -> Page {
    let query = normalize(query);
    if query.is_empty() {
        return page.clone();
    }

    let terms = query.split_whitespace().collect::<Vec<_>>();
    let filter_cards = |cards: &[Card]| {
        cards
            .iter()
            .filter(|card| matches_fields(&terms, &[&card.title, &card.subtitle]))
            .cloned()
            .collect()
    };
    let filter_tracks = |tracks: &[Track]| {
        matching_track_indices(tracks, &query)
            .into_iter()
            .map(|index| tracks[index].clone())
            .collect()
    };

    let (tracks, cards, sections) = if page.uses_sections() {
        let sections = page
            .sections
            .iter()
            .map(|section| {
                let tracks: Vec<Track> = filter_tracks(&section.tracks);
                let cards: Vec<Card> = filter_cards(&section.cards);
                Section {
                    title: section.title.clone(),
                    description: section.description.clone(),
                    total: tracks.len() + cards.len(),
                    show_count: section.show_count,
                    layout: section.layout,
                    preview_limit: section.preview_limit,
                    card_row: section.card_row,
                    tracks,
                    cards,
                    empty_message: section.empty_message.clone(),
                }
            })
            .collect::<Vec<_>>();
        (Vec::new(), Vec::new(), sections)
    } else {
        (
            filter_tracks(&page.tracks),
            filter_cards(&page.cards),
            Vec::new(),
        )
    };

    let mut filtered = Page {
        title: page.title.clone(),
        subtitle: page.subtitle.clone(),
        description: page.description.clone(),
        album_info: page.album_info.clone(),
        service_url: page.service_url.clone(),
        artwork: page.artwork.clone(),
        platform: page.platform,
        count_noun: page.count_noun.clone(),
        meta_text: page.meta_text.clone(),
        show_count: page.show_count,
        total: 0,
        raw_loaded_count: page.raw_loaded_count,
        normalized_count: page.normalized_count,
        authoritative_total: page.authoritative_total,
        tracks,
        cards,
        sections,
        next_flow_tuner: page.next_flow_tuner.clone(),
        resolved_smart_mix_title: page.resolved_smart_mix_title.clone(),
        clear_remaining_tracks: page.clear_remaining_tracks,
        empty_title: "No matches on this page".into(),
        empty_description: "Nothing here matches your search query.".into(),
    };
    filtered.total = filtered.displayed_total();
    filtered
}

/// Return the source positions of the tracks visible for a library query.
///
/// The renderer uses these positions for both the index column and playback
/// queue lookup. Keeping this beside the filtering predicate prevents the
/// visible list and its source-position mapping from drifting apart.
pub(crate) fn matching_track_indices(tracks: &[Track], query: &str) -> Vec<usize> {
    let query = normalize(query);
    let terms = query.split_whitespace().collect::<Vec<_>>();
    tracks
        .iter()
        .enumerate()
        .filter(|(_, track)| {
            query.is_empty() || matches_fields(&terms, &[&track.title, &track.artist])
        })
        .map(|(index, _)| index)
        .collect()
}

fn contains_ignore_case(haystack: &str, needle: &str) -> bool {
    if needle.is_empty() {
        return true;
    }
    if haystack.len() < needle.len() {
        return false;
    }
    if haystack.is_ascii() && needle.is_ascii() {
        let haystack_bytes = haystack.as_bytes();
        let needle_bytes = needle.as_bytes();
        haystack_bytes.windows(needle_bytes.len()).any(|window| {
            window
                .iter()
                .zip(needle_bytes)
                .all(|(h, n)| h.to_ascii_lowercase() == *n)
        })
    } else {
        haystack.to_lowercase().contains(needle)
    }
}

fn matches_fields(terms: &[&str], fields: &[&str]) -> bool {
    terms
        .iter()
        .all(|term| fields.iter().any(|field| contains_ignore_case(field, term)))
}

fn normalize(value: &str) -> String {
    value.trim().to_lowercase()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn filters_page_items_without_mutating_the_source() {
        let page = Page {
            total: 2,
            tracks: vec![Track {
                title: "Beyonce - Halo".into(),
                artist: "Beyonce".into(),
                ..Track::default()
            }],
            cards: vec![Card {
                title: "Live Set".into(),
                subtitle: "Other Artist".into(),
                ..Card::default()
            }],
            sections: vec![Section {
                title: "Popular".into(),
                cards: vec![Card {
                    title: "Halo Collection".into(),
                    subtitle: "Beyonce".into(),
                    ..Card::default()
                }],
                total: 1,
                ..Section::default()
            }],
            ..Page::default()
        };
        let filtered = filtered_page(&page, "  BEYONCE ");
        assert!(filtered.tracks.is_empty());
        assert!(filtered.cards.is_empty());
        assert_eq!(filtered.sections[0].cards.len(), 1);
        assert_eq!(filtered.sections[0].total, 1);
        assert_eq!(filtered.total, 1);
        assert_eq!(page.total, 2);
    }

    #[test]
    fn removes_sections_without_matching_items() {
        let page = Page {
            sections: vec![
                Section {
                    title: "Match".into(),
                    tracks: vec![Track {
                        title: "Song".into(),
                        ..Track::default()
                    }],
                    ..Section::default()
                },
                Section {
                    title: "No match".into(),
                    tracks: vec![Track {
                        title: "Other".into(),
                        ..Track::default()
                    }],
                    ..Section::default()
                },
            ],
            ..Page::default()
        };

        let filtered = filtered_page(&page, "song");

        assert_eq!(filtered.sections.len(), 2);
        assert_eq!(filtered.sections[0].title, "Match");
        assert!(filtered.sections[1].tracks.is_empty());
        assert_eq!(filtered.sections[1].total, 0);
    }

    #[test]
    fn filtered_section_totals_follow_visible_items_without_changing_page_total_source() {
        let page = Page {
            total: 3,
            sections: vec![Section {
                total: 3,
                tracks: vec![
                    Track {
                        title: "Keep me".into(),
                        artist: "Artist".into(),
                        ..Track::default()
                    },
                    Track {
                        title: "Drop me".into(),
                        artist: "Other".into(),
                        ..Track::default()
                    },
                ],
                cards: vec![Card {
                    title: "Keep card".into(),
                    subtitle: "Artist".into(),
                    ..Card::default()
                }],
                ..Section::default()
            }],
            ..Page::default()
        };

        let filtered = filtered_page(&page, "artist");

        assert_eq!(filtered.total, 2);
        assert_eq!(filtered.sections[0].total, 2);
        assert_eq!(page.total, 3);
        assert_eq!(page.sections[0].total, 3);
    }

    #[test]
    fn reports_the_no_match_state() {
        let filtered = filtered_page(&Page::default(), "  missing  ");
        assert_eq!(filtered.empty_title, "No matches on this page");
        assert_eq!(
            filtered.empty_description,
            "Nothing here matches your search query."
        );
    }

    #[test]
    fn filtering_preserves_radio_continuation_metadata() {
        let page = Page {
            next_flow_tuner: Some(crate::library::FlowTuner::initial(
                crate::library::FlowMode::Discovery,
            )),
            clear_remaining_tracks: true,
            tracks: vec![Track {
                title: "Keep".into(),
                ..Track::default()
            }],
            ..Page::default()
        };
        let filtered = filtered_page(&page, "keep");
        assert_eq!(filtered.next_flow_tuner, page.next_flow_tuner);
        assert!(filtered.clear_remaining_tracks);
    }

    #[test]
    fn filtering_flat_pages_recalculates_the_visible_total() {
        let page = Page {
            total: 20,
            tracks: vec![
                Track {
                    title: "Keep".into(),
                    ..Track::default()
                },
                Track {
                    title: "Drop".into(),
                    ..Track::default()
                },
            ],
            ..Page::default()
        };

        let filtered = filtered_page(&page, "keep");

        assert_eq!(filtered.tracks.len(), 1);
        assert_eq!(filtered.total, 1);
    }

    #[test]
    fn matching_track_indices_preserve_source_positions_and_duplicates() {
        let tracks = (0..48)
            .map(|index| Track {
                title: if index == 43 || index == 44 {
                    "Keep this track".into()
                } else {
                    format!("Track {index}")
                },
                ..Track::default()
            })
            .collect::<Vec<_>>();

        assert_eq!(matching_track_indices(&tracks, "keep"), vec![43, 44]);
    }

    #[test]
    fn filtering_sectioned_pages_does_not_count_duplicated_flat_items() {
        let duplicate = Track {
            title: "Keep".into(),
            ..Track::default()
        };
        let page = Page {
            tracks: vec![duplicate.clone()],
            sections: vec![Section {
                tracks: vec![duplicate],
                ..Section::default()
            }],
            ..Page::default()
        };

        let filtered = filtered_page(&page, "keep");

        assert!(filtered.tracks.is_empty());
        assert_eq!(filtered.sections[0].tracks.len(), 1);
        assert_eq!(filtered.total, 1);
    }

    #[test]
    fn filtered_empty_sections_remain_visible_with_the_frozen_fallback() {
        let filtered = filtered_page(
            &Page {
                sections: vec![Section {
                    title: "Popular Tracks".into(),
                    tracks: vec![Track {
                        title: "Keep".into(),
                        ..Track::default()
                    }],
                    ..Section::default()
                }],
                ..Page::default()
            },
            "missing",
        );

        assert_eq!(filtered.sections.len(), 1);
        assert!(filtered.sections[0].tracks.is_empty());
        assert!(filtered.sections[0].empty_message.is_empty());
        assert!(filtered.is_empty());
        assert_eq!(filtered.total, 0);
    }
}
