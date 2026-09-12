use gpui::{
    AnimationExt as _, AnyElement, Context, FontWeight, IntoElement, div, prelude::*, px, rgb,
};

use super::{LibraryView, model::Card};
use crate::{
    assets::{LocalIcon, local_icon},
    theme::{FOREGROUND, MUTED},
};

#[derive(Clone, Debug, Eq, PartialEq)]
pub(crate) enum SimilarArtistsState {
    Loading,
    Results(Vec<Card>),
    Empty,
    Failed(String),
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub(crate) struct SimilarArtistsEntry {
    pub(crate) artist_id: String,
    pub(crate) title: String,
    pub(crate) state: SimilarArtistsState,
}

#[derive(Default)]
pub(crate) struct SimilarArtistsNavigation {
    pub(crate) current: Option<SimilarArtistsEntry>,
    stack: Vec<SimilarArtistsEntry>,
    generation: u64,
}

impl SimilarArtistsNavigation {
    pub(crate) fn open(&mut self, artist_id: String, title: String) -> u64 {
        if let Some(current) = self.current.take() {
            self.stack.push(current);
        }
        self.generation = self.generation.wrapping_add(1);
        self.current = Some(SimilarArtistsEntry {
            artist_id,
            title,
            state: SimilarArtistsState::Loading,
        });
        self.generation
    }

    pub(crate) fn complete(&mut self, generation: u64, result: Result<Vec<Card>, String>) -> bool {
        if generation != self.generation {
            return false;
        }
        let Some(current) = self.current.as_mut() else {
            return false;
        };
        current.state = match result {
            Ok(cards) if cards.is_empty() => SimilarArtistsState::Empty,
            Ok(cards) => SimilarArtistsState::Results(cards),
            Err(error) => SimilarArtistsState::Failed(error),
        };
        true
    }

    pub(crate) fn back(&mut self) -> bool {
        if let Some(previous) = self.stack.pop() {
            self.generation = self.generation.wrapping_add(1);
            self.current = Some(previous);
        } else if self.current.take().is_some() {
            self.generation = self.generation.wrapping_add(1);
        } else {
            return false;
        }
        true
    }

    pub(crate) fn reset(&mut self) {
        self.generation = self.generation.wrapping_add(1);
        self.current = None;
        self.stack.clear();
    }

    pub(crate) fn generation(&self) -> u64 {
        self.generation
    }

    pub(crate) fn is_open(&self) -> bool {
        self.current.is_some()
    }
}

pub(super) fn render(
    view: &LibraryView,
    host: &gpui::Entity<LibraryView>,
    navigation: &SimilarArtistsNavigation,
    columns: u16,
    narrow: bool,
    app: &mut Context<LibraryView>,
) -> AnyElement {
    let Some(entry) = navigation.current.as_ref() else {
        return div().into_any_element();
    };
    let heading = div()
        .w_full()
        .flex()
        .items_center()
        .justify_between()
        .gap(px(12.))
        .child(
            div()
                .min_w_0()
                .flex()
                .flex_col()
                .gap(px(3.))
                .child(
                    div()
                        .text_size(px(20.))
                        .font_weight(FontWeight(650.))
                        .truncate()
                        .child("Similar Artists"),
                )
                .child(
                    div()
                        .text_size(px(12.5))
                        .text_color(rgb(MUTED))
                        .truncate()
                        .child(format!("Artists related to {}", entry.title)),
                ),
        );
    let body = match &entry.state {
        SimilarArtistsState::Loading => message(
            LocalIcon::UserGroup,
            "Loading similar artists",
            "Deezer is finding related artists.",
        ),
        SimilarArtistsState::Empty => message(
            LocalIcon::UserGroup,
            "No similar artists",
            "Deezer did not return related artists for this selection.",
        ),
        SimilarArtistsState::Failed(_) => message(
            LocalIcon::TriangleExclamation,
            "Could not load similar artists",
            "Deezer did not return a related artist list.",
        ),
        SimilarArtistsState::Results(cards) => super::cards_view::cards(
            view,
            host,
            cards,
            "similar-artists",
            0,
            columns,
            None,
            false,
            narrow,
            app,
        ),
    };
    div()
        .w_full()
        .flex()
        .flex_col()
        .relative()
        .gap(px(14.))
        .child(heading)
        .child(body)
        .with_animation(
            format!(
                "library-similar-body:{}:{}",
                entry.artist_id,
                navigation.generation()
            ),
            crate::motion::quick_content(),
            |this, delta| {
                this.opacity(crate::motion::lerp(0.0, 1.0, delta))
                    .top(px(crate::motion::lerp(4.0, 0.0, delta)))
            },
        )
        .into_any_element()
}

fn message(icon: LocalIcon, title: &str, description: &str) -> AnyElement {
    div()
        .w_full()
        .min_h(px(300.))
        .flex()
        .flex_col()
        .items_center()
        .justify_center()
        .gap(px(8.))
        .text_color(rgb(MUTED))
        .child(local_icon(icon, MUTED).size(px(32.)))
        .child(
            div()
                .text_size(px(15.))
                .font_weight(FontWeight::SEMIBOLD)
                .text_color(rgb(FOREGROUND))
                .child(title.to_owned()),
        )
        .child(div().text_size(px(12.5)).child(description.to_owned()))
        .into_any_element()
}

#[cfg(test)]
mod tests {
    use super::*;

    fn card(id: &str) -> Card {
        Card {
            id: id.into(),
            title: format!("Artist {id}"),
            source: crate::search::Provider::Deezer,
            kind: super::super::model::Category::Artists,
            ..Card::default()
        }
    }

    #[test]
    fn stale_completion_cannot_replace_a_newer_artist_page() {
        let mut navigation = SimilarArtistsNavigation::default();
        let first = navigation.open("1".into(), "First".into());
        let second = navigation.open("2".into(), "Second".into());
        assert!(!navigation.complete(first, Ok(vec![card("old")])));
        assert!(navigation.complete(second, Ok(vec![card("new")])));
        assert_eq!(
            navigation
                .current
                .as_ref()
                .and_then(|entry| match &entry.state {
                    SimilarArtistsState::Results(cards) =>
                        cards.first().map(|card| card.id.as_str()),
                    _ => None,
                }),
            Some("new")
        );
    }

    #[test]
    fn empty_and_failure_states_are_retained() {
        let mut navigation = SimilarArtistsNavigation::default();
        let empty = navigation.open("1".into(), "Artist".into());
        assert!(navigation.complete(empty, Ok(Vec::new())));
        assert!(matches!(
            navigation.current.as_ref().map(|entry| &entry.state),
            Some(SimilarArtistsState::Empty)
        ));
        let failed = navigation.open("2".into(), "Other".into());
        assert!(navigation.complete(failed, Err("failed".into())));
        assert!(matches!(
            navigation.current.as_ref().map(|entry| &entry.state),
            Some(SimilarArtistsState::Failed(error)) if error == "failed"
        ));
    }

    #[test]
    fn back_restores_nested_artist_page_and_invalidates_pending_work() {
        let mut navigation = SimilarArtistsNavigation::default();
        let first = navigation.open("1".into(), "First".into());
        navigation.complete(first, Ok(vec![card("first")]));
        let second = navigation.open("2".into(), "Second".into());
        assert!(navigation.back());
        assert_eq!(
            navigation
                .current
                .as_ref()
                .map(|entry| entry.artist_id.as_str()),
            Some("1")
        );
        assert!(!navigation.complete(second, Ok(vec![card("stale")])));
        assert!(navigation.back());
        assert!(!navigation.is_open());
    }

    #[test]
    fn reset_discards_the_route_and_invalidates_pending_service_navigation() {
        let mut navigation = SimilarArtistsNavigation::default();
        let generation = navigation.open("1".into(), "Artist".into());
        navigation.reset();

        assert!(!navigation.is_open());
        assert!(!navigation.complete(generation, Ok(vec![card("stale")])));
    }
}
