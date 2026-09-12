use std::collections::HashMap;

use gpui::{
    AnyElement, App, ClickEvent, FontWeight, IntoElement, ScrollHandle, Window, div, point,
    prelude::*, px, rgb, rgba,
};
use gpui_component::scroll::{Scrollbar, ScrollbarShow};
use tokio_util::sync::CancellationToken;

use crate::{
    assets::{LocalIcon, local_icon},
    browser_scroll::{BrowserScrollState, BrowserScrollTarget, browser_scroll_surface},
    music_ui::ghost_close_button,
    theme::{BORDER, DANGER, FOREGROUND, MUTED, SURFACE, SURFACE_RAISED},
};

use super::client::{GeniusAnnotation, GeniusReferent};

pub(super) const GENIUS_ACCENT: u32 = 0xffff99;
const PANEL_PREFERRED_HEIGHT: f32 = 250.;
const PANEL_OUTER_CHROME_HEIGHT: f32 = 21.;

fn panel_inner_height_bounds(max_height: f32) -> (f32, f32) {
    let inner_max_height = (max_height - PANEL_OUTER_CHROME_HEIGHT).max(0.);
    let inner_min_height =
        (PANEL_PREFERRED_HEIGHT.min(max_height) - PANEL_OUTER_CHROME_HEIGHT).max(0.);
    (inner_min_height, inner_max_height)
}

#[derive(Clone, Debug)]
pub(super) enum AnnotationState {
    Loading,
    Ready(GeniusReferent),
    Error(String),
}

pub(super) enum AnnotationOpen {
    Unchanged,
    Shown,
    Fetch(u64, CancellationToken),
}

pub(super) struct AnnotationController {
    state: Option<AnnotationState>,
    selected_index: usize,
    generation: u64,
    cancellation: CancellationToken,
    scroll: ScrollHandle,
    browser_scroll: BrowserScrollState,
    cache: HashMap<String, GeniusReferent>,
    current_id: Option<String>,
}

impl AnnotationController {
    pub(super) fn new() -> Self {
        Self {
            state: None,
            selected_index: 0,
            generation: 0,
            cancellation: CancellationToken::new(),
            scroll: ScrollHandle::new(),
            browser_scroll: BrowserScrollState::new(),
            cache: HashMap::new(),
            current_id: None,
        }
    }

    pub(super) fn open(&mut self, id: &str) -> AnnotationOpen {
        if self.state.is_some() && self.current_id.as_deref() == Some(id) {
            return AnnotationOpen::Unchanged;
        }
        if let Some(cached) = self.cache.get(id).cloned() {
            self.invalidate_inflight();
            self.current_id = Some(id.to_owned());
            self.selected_index = 0;
            self.reset_scroll();
            self.state = Some(AnnotationState::Ready(cached));
            return AnnotationOpen::Shown;
        }
        self.current_id = Some(id.to_owned());
        let (generation, cancellation) = self.begin();
        AnnotationOpen::Fetch(generation, cancellation)
    }

    pub(super) fn begin(&mut self) -> (u64, CancellationToken) {
        self.invalidate_inflight();
        self.selected_index = 0;
        self.reset_scroll();
        self.state = Some(AnnotationState::Loading);
        (self.generation, self.cancellation.clone())
    }

    pub(super) fn complete(
        &mut self,
        generation: u64,
        result: Result<GeniusReferent, String>,
    ) -> bool {
        if generation != self.generation || self.cancellation.is_cancelled() {
            return false;
        }
        self.selected_index = 0;
        self.state = Some(match result {
            Ok(value) => {
                if let Some(id) = self.current_id.as_ref() {
                    self.cache.insert(id.clone(), value.clone());
                }
                AnnotationState::Ready(value)
            }
            Err(error) => AnnotationState::Error(error),
        });
        true
    }

    pub(super) fn close(&mut self) {
        self.invalidate_inflight();
        self.selected_index = 0;
        self.state = None;
        self.current_id = None;
    }

    pub(super) fn clear_cache(&mut self) {
        self.cache.clear();
    }

    fn invalidate_inflight(&mut self) {
        self.cancellation.cancel();
        self.cancellation = CancellationToken::new();
        self.generation = self.generation.wrapping_add(1);
    }

    pub(super) fn state(&self) -> Option<&AnnotationState> {
        self.state.as_ref()
    }

    pub(super) fn selected_index(&self) -> usize {
        let max = match &self.state {
            Some(AnnotationState::Ready(value)) => value.annotations.len().saturating_sub(1),
            _ => 0,
        };
        self.selected_index.min(max)
    }

    pub(super) fn select_prev(&mut self) -> bool {
        self.shift_selected(-1)
    }

    pub(super) fn select_next(&mut self) -> bool {
        self.shift_selected(1)
    }

    fn shift_selected(&mut self, delta: i32) -> bool {
        let Some(AnnotationState::Ready(value)) = &self.state else {
            return false;
        };
        let count = value.annotations.len();
        if count <= 1 {
            return false;
        }
        let next = (self.selected_index() as i32 + delta).rem_euclid(count as i32) as usize;
        if next == self.selected_index {
            return false;
        }
        self.selected_index = next;
        self.reset_scroll();
        true
    }

    fn reset_scroll(&mut self) {
        self.browser_scroll.reset();
        self.scroll.set_offset(point(px(0.), px(0.)));
    }

    pub(super) fn scroll(&self) -> &ScrollHandle {
        &self.scroll
    }

    pub(super) fn browser_scroll(&self) -> BrowserScrollState {
        self.browser_scroll.clone()
    }
}

pub(super) fn panel(
    state: &AnnotationState,
    selected_index: usize,
    scroll: &ScrollHandle,
    browser_scroll: BrowserScrollState,
    max_height: f32,
    on_close: impl Fn(&ClickEvent, &mut Window, &mut App) + 'static,
    on_prev: impl Fn(&ClickEvent, &mut Window, &mut App) + 'static,
    on_next: impl Fn(&ClickEvent, &mut Window, &mut App) + 'static,
) -> AnyElement {
    let (inner_min_height, inner_max_height) = panel_inner_height_bounds(max_height);
    let (body, footer) = match state {
        AnnotationState::Loading => (loading_body(), None),
        AnnotationState::Ready(value) => {
            let count = value.annotations.len();
            let selected = selected_index.min(count.saturating_sub(1));
            let footer = value.annotations.get(selected).map(|annotation| {
                contribution_footer(annotation, selected, count, on_prev, on_next)
            });
            (ready_body(value, selected), footer)
        }
        AnnotationState::Error(error) => (error_body(error), None),
    };
    let scroll_content = div()
        .id("genius-annotation-scroll")
        .debug_selector(|| "genius-annotation-scroll".into())
        .flex_1()
        .min_h_0()
        .track_scroll(scroll)
        .overflow_y_scroll()
        .p(px(12.))
        .pr(px(18.))
        .child(body)
        .into_any_element();
    let scroll_surface = browser_scroll_surface(
        "genius-annotation-browser-scroll",
        scroll_content,
        BrowserScrollTarget::Handle(scroll.clone()),
        browser_scroll,
    );

    div()
        .id("genius-annotation-panel")
        .debug_selector(|| "genius-annotation-panel".into())
        .flex_none()
        .w_full()
        .max_h(px(max_height))
        .pt(px(10.))
        .pb(px(10.))
        .border_t_1()
        .border_color(rgb(BORDER))
        .child(
            div()
                .w_full()
                .min_h(px(inner_min_height))
                .max_h(px(inner_max_height))
                .flex()
                .flex_col()
                .overflow_hidden()
                .rounded(px(8.))
                .border_1()
                .border_color(rgb(BORDER))
                .bg(rgb(SURFACE_RAISED))
                .child(
                    div()
                        .flex_none()
                        .h(px(45.))
                        .px(px(12.))
                        .flex()
                        .items_center()
                        .gap(px(8.))
                        .border_b_1()
                        .border_color(rgb(BORDER))
                        .child(local_icon(LocalIcon::Brain, GENIUS_ACCENT).size(px(14.)))
                        .child(
                            div()
                                .flex_1()
                                .flex()
                                .items_baseline()
                                .gap(px(4.))
                                .text_size(px(14.))
                                .font_weight(FontWeight::SEMIBOLD)
                                .child(div().text_color(rgb(GENIUS_ACCENT)).child("Genius"))
                                .child(div().text_color(rgb(FOREGROUND)).child("Annotation")),
                        )
                        .child(
                            div()
                                .id("genius-annotation-close-control")
                                .debug_selector(|| "genius-annotation-close-control".into())
                                .flex_none()
                                .child(ghost_close_button("genius-annotation-close", on_close)),
                        ),
                )
                .child(
                    div()
                        .id("genius-annotation-scroll-frame")
                        .relative()
                        .flex_1()
                        .min_h_0()
                        .flex()
                        .flex_col()
                        .child(scroll_surface)
                        .child(
                            div()
                                .debug_selector(|| "genius-annotation-scrollbar".into())
                                .absolute()
                                .top_0()
                                .right_0()
                                .bottom_0()
                                .left_0()
                                .child(
                                    Scrollbar::vertical(scroll)
                                        .scrollbar_show(ScrollbarShow::Hover)
                                        .id("genius-annotation-scrollbar-control"),
                                ),
                        ),
                )
                .when_some(footer, |this, footer| this.child(footer)),
        )
        .into_any_element()
}

fn loading_body() -> AnyElement {
    div()
        .w_full()
        .py(px(24.))
        .px(px(16.))
        .flex()
        .flex_col()
        .items_center()
        .text_center()
        .child(
            div()
                .mb(px(12.))
                .child(local_icon(LocalIcon::MagnifyingGlass, MUTED).size(px(40.))),
        )
        .child(
            div()
                .mb(px(4.))
                .text_size(px(16.))
                .font_weight(FontWeight::SEMIBOLD)
                .text_color(rgb(FOREGROUND))
                .child("Finding annotation"),
        )
        .child(
            div()
                .text_size(px(13.))
                .line_height(px(19.5))
                .text_color(rgb(MUTED))
                .child("Searching Genius for this lyric note..."),
        )
        .into_any_element()
}

fn ready_body(value: &GeniusReferent, selected: usize) -> AnyElement {
    let mut body = div().w_full().flex().flex_col().gap(px(12.)).child(
        div()
            .w_full()
            .flex()
            .items_start()
            .gap(px(9.))
            .p(px(10.))
            .rounded(px(7.))
            .border_1()
            .border_color(rgba(0xffff9947))
            .bg(rgb(SURFACE))
            .text_size(px(13.))
            .line_height(px(19.5))
            .text_color(rgb(FOREGROUND))
            .child(local_icon(LocalIcon::QuoteRight, GENIUS_ACCENT).size(px(11.)))
            .child(
                div()
                    .min_w_0()
                    .font_weight(FontWeight::MEDIUM)
                    .child(value.fragment.clone()),
            ),
    );
    if let Some(annotation) = value.annotations.get(selected) {
        body = body.child(
            div()
                .w_full()
                .text_size(px(13.5))
                .line_height(px(20.))
                .text_color(rgb(FOREGROUND))
                .child(annotation.text.clone()),
        );
    }
    body.into_any_element()
}

fn contribution_footer(
    value: &GeniusAnnotation,
    selected: usize,
    count: usize,
    on_prev: impl Fn(&ClickEvent, &mut Window, &mut App) + 'static,
    on_next: impl Fn(&ClickEvent, &mut Window, &mut App) + 'static,
) -> gpui::Div {
    div()
        .debug_selector(|| "genius-annotation-footer".into())
        .flex_none()
        .w_full()
        .px(px(12.))
        .py(px(8.))
        .flex()
        .items_center()
        .gap_x(px(12.))
        .border_t_1()
        .border_color(rgb(BORDER))
        .text_size(px(11.5))
        .text_color(rgb(MUTED))
        .child(
            div().flex_1().min_w_0().flex().items_center().child(
                div()
                    .debug_selector(|| "genius-annotation-votes".into())
                    .flex_none()
                    .flex()
                    .items_center()
                    .gap(px(5.))
                    .child(local_icon(LocalIcon::Heart, MUTED).size(px(10.)))
                    .child(format!("{} votes", value.votes)),
            ),
        )
        .when(count > 1, |this| {
            this.child(contribution_picker(selected, count, on_prev, on_next))
        })
        .child(
            div()
                .flex_1()
                .min_w_0()
                .flex()
                .items_center()
                .justify_end()
                .child(
                    div()
                        .debug_selector(|| "genius-annotation-author".into())
                        .min_w_0()
                        .flex()
                        .items_center()
                        .justify_end()
                        .gap(px(5.))
                        .child(
                            div()
                                .flex_none()
                                .child(local_icon(LocalIcon::User, MUTED).size(px(10.))),
                        )
                        .child(
                            div()
                                .debug_selector(|| "genius-annotation-author-text".into())
                                .min_w_0()
                                .truncate()
                                .child(value.author.clone()),
                        ),
                ),
        )
}

fn contribution_picker(
    selected: usize,
    count: usize,
    on_prev: impl Fn(&ClickEvent, &mut Window, &mut App) + 'static,
    on_next: impl Fn(&ClickEvent, &mut Window, &mut App) + 'static,
) -> impl IntoElement {
    div()
        .id("genius-annotation-picker")
        .debug_selector(|| "genius-annotation-picker".into())
        .flex_none()
        .flex_shrink_0()
        .min_w(px(56.))
        .flex()
        .items_center()
        .justify_center()
        .gap(px(4.))
        .child(picker_button(
            "genius-annotation-picker-prev",
            LocalIcon::ArrowLeft,
            on_prev,
        ))
        .child(
            div()
                .debug_selector(|| "genius-annotation-picker-index".into())
                .flex_none()
                .whitespace_nowrap()
                .child(format!("{}/{}", selected + 1, count)),
        )
        .child(picker_button(
            "genius-annotation-picker-next",
            LocalIcon::ArrowRight,
            on_next,
        ))
}

fn picker_button(
    id: &'static str,
    icon: LocalIcon,
    on_click: impl Fn(&ClickEvent, &mut Window, &mut App) + 'static,
) -> impl IntoElement {
    div()
        .id(id)
        .debug_selector(move || id.into())
        .size(px(18.))
        .flex_none()
        .flex()
        .items_center()
        .justify_center()
        .rounded(px(4.))
        .cursor_pointer()
        .hover(|style| style.bg(rgb(BORDER)))
        .on_click(move |event, window, cx| {
            cx.stop_propagation();
            on_click(event, window, cx);
        })
        .child(local_icon(icon, MUTED).size(px(8.)))
}

fn error_body(error: &str) -> AnyElement {
    div()
        .flex()
        .items_start()
        .gap(px(9.))
        .text_size(px(13.5))
        .line_height(px(20.))
        .text_color(rgb(DANGER))
        .child(local_icon(LocalIcon::TriangleExclamation, DANGER).size(px(13.)))
        .child(
            div()
                .min_w_0()
                .child("The Genius annotation could not be loaded. ")
                .child(error.to_owned()),
        )
        .into_any_element()
}

#[cfg(test)]
mod tests {
    use std::{cell::Cell, rc::Rc};

    use gpui::{
        AnyWindowHandle, Context, Modifiers, Render, TestAppContext, VisualTestContext, Window,
    };

    use super::*;

    struct AnnotationPanelProbe {
        clicked: Rc<Cell<bool>>,
        selected: usize,
        state: AnnotationState,
        scroll: ScrollHandle,
        browser_scroll: BrowserScrollState,
        max_height: f32,
    }

    impl Render for AnnotationPanelProbe {
        fn render(&mut self, _: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
            div()
                .w(px(260.))
                .h(px(500.))
                .flex()
                .flex_col()
                .child(div().flex_1())
                .child(panel(
                    &self.state,
                    self.selected,
                    &self.scroll,
                    self.browser_scroll.clone(),
                    self.max_height,
                    {
                        let clicked = self.clicked.clone();
                        move |_, _, _| clicked.set(true)
                    },
                    cx.listener(|this, _, _, cx| {
                        this.selected = this.selected.saturating_sub(1);
                        cx.notify();
                    }),
                    cx.listener(|this, _, _, cx| {
                        this.selected = this.selected.saturating_add(1);
                        cx.notify();
                    }),
                ))
        }
    }

    fn annotation(fragment: &str) -> GeniusReferent {
        GeniusReferent {
            fragment: fragment.to_owned(),
            annotations: vec![GeniusAnnotation {
                text: "Explanation".to_owned(),
                votes: 12,
                author: "Contributor".to_owned(),
            }],
        }
    }

    fn two_annotations() -> GeniusReferent {
        GeniusReferent {
            fragment: "[Produced by Diplo & Skrillex]".to_owned(),
            annotations: vec![
                GeniusAnnotation {
                    text: "This song makes me think of the first time I heard it.".to_owned(),
                    votes: 70,
                    author: "Skrillex".to_owned(),
                },
                GeniusAnnotation {
                    text: "It's funny how people find this song.".to_owned(),
                    votes: 55,
                    author: "Diplo".to_owned(),
                },
            ],
        }
    }

    fn overflowing_annotation() -> GeniusReferent {
        let mut value = annotation("A long annotated lyric fragment");
        value.annotations[0].text = (0..48)
            .map(|index| format!("Paragraph {index}: enough annotation text to require scrolling."))
            .collect::<Vec<_>>()
            .join("\n\n");
        value
    }

    #[test]
    fn closing_during_load_rejects_the_stale_completion() {
        let mut controller = AnnotationController::new();
        let (generation, _) = controller.begin();

        controller.close();

        assert!(!controller.complete(generation, Ok(annotation("first fragment"))));
        assert!(controller.state().is_none());
    }

    #[test]
    fn a_new_request_cannot_be_overwritten_by_an_older_one() {
        let mut controller = AnnotationController::new();
        let (older, _) = controller.begin();
        let (newer, _) = controller.begin();

        assert!(!controller.complete(older, Ok(annotation("older fragment"))));
        assert!(controller.complete(newer, Ok(annotation("newer fragment"))));
        assert!(matches!(
            controller.state(),
            Some(AnnotationState::Ready(value)) if value.fragment == "newer fragment"
        ));
    }

    #[test]
    fn multiple_contributions_are_shown_one_at_a_time() {
        let mut controller = AnnotationController::new();
        let (generation, _) = controller.begin();
        assert!(controller.complete(generation, Ok(two_annotations())));
        assert_eq!(controller.selected_index(), 0);
        assert!(controller.select_next());
        assert_eq!(controller.selected_index(), 1);
        assert!(controller.select_next());
        assert_eq!(controller.selected_index(), 0);
        assert!(controller.select_prev());
        assert_eq!(controller.selected_index(), 1);
    }

    #[test]
    fn a_single_contribution_has_no_picker_navigation() {
        let mut controller = AnnotationController::new();
        let (generation, _) = controller.begin();
        assert!(controller.complete(generation, Ok(annotation("fragment"))));
        assert!(!controller.select_next());
        assert!(!controller.select_prev());
        assert_eq!(controller.selected_index(), 0);
    }

    #[test]
    fn cached_annotation_skips_a_second_fetch() {
        let mut controller = AnnotationController::new();
        let AnnotationOpen::Fetch(generation, _) = controller.open("42") else {
            panic!("first open should fetch");
        };
        assert!(matches!(controller.state(), Some(AnnotationState::Loading)));
        assert!(controller.complete(generation, Ok(annotation("cached fragment"))));
        assert!(matches!(controller.open("42"), AnnotationOpen::Unchanged));

        controller.close();
        assert!(controller.state().is_none());
        assert!(matches!(controller.open("42"), AnnotationOpen::Shown));
        assert!(matches!(
            controller.state(),
            Some(AnnotationState::Ready(value)) if value.fragment == "cached fragment"
        ));

        let AnnotationOpen::Fetch(other_generation, _) = controller.open("99") else {
            panic!("a different referent should fetch");
        };
        assert!(matches!(controller.state(), Some(AnnotationState::Loading)));
        assert!(matches!(controller.open("42"), AnnotationOpen::Shown));
        assert!(matches!(
            controller.state(),
            Some(AnnotationState::Ready(value)) if value.fragment == "cached fragment"
        ));
        assert!(!controller.complete(other_generation, Ok(annotation("stale fragment"))));

        controller.close();
        controller.clear_cache();
        assert!(matches!(controller.open("42"), AnnotationOpen::Fetch(_, _)));
        assert!(matches!(controller.state(), Some(AnnotationState::Loading)));
    }

    #[test]
    fn opening_an_annotation_resets_its_scroll_position() {
        let mut controller = AnnotationController::new();
        controller.scroll().set_offset(point(px(0.), px(-120.)));

        controller.begin();

        assert_eq!(controller.scroll().offset(), point(px(0.), px(0.)));
    }

    #[test]
    fn panel_contract_has_close_control_and_bounded_app_scrollbar() {
        let implementation = include_str!("annotation.rs")
            .split_once("#[cfg(test)]")
            .map_or_else(
                || panic!("annotation implementation section is missing"),
                |(implementation, _)| implementation,
            );
        assert!(implementation.contains("genius-annotation-close"));
        assert!(implementation.contains("ghost_close_button"));
        assert!(implementation.contains(".max_h(px(max_height))"));
        assert!(implementation.contains("panel_inner_height_bounds(max_height)"));
        assert!(implementation.contains(".min_h(px(inner_min_height))"));
        assert!(implementation.contains(".track_scroll(scroll)"));
        assert!(implementation.contains(".overflow_y_scroll()"));
        assert!(implementation.contains("Scrollbar::vertical(scroll)"));
        assert!(implementation.contains("ScrollbarShow::Hover"));
        // The outer panel pads symmetrically so the card never touches the
        // bottom edge of the lyrics view.
        assert!(implementation.contains(".pt(px(10.))"));
        assert!(implementation.contains(".pb(px(10.))"));
        assert!(implementation.contains("browser_scroll_surface("));
        assert!(implementation.contains("BrowserScrollTarget::Handle(scroll.clone())"));
        assert!(implementation.contains("genius-annotation-picker"));
        assert!(implementation.contains("contribution_picker("));
        assert!(implementation.contains(".flex_shrink_0()"));
        assert!(implementation.contains("genius-annotation-footer"));
        assert!(implementation.contains("contribution_footer("));
        assert!(
            implementation.contains("if let Some(annotation) = value.annotations.get(selected)")
        );
    }

    #[test]
    fn footer_keeps_votes_left_and_the_bare_author_at_the_far_right() {
        let implementation = include_str!("annotation.rs")
            .split_once("#[cfg(test)]")
            .map_or_else(
                || panic!("annotation implementation section is missing"),
                |(implementation, _)| implementation,
            );
        assert!(implementation.contains(".justify_end()"));
        assert!(implementation.contains(".truncate()"));
        assert!(implementation.contains(".child(value.author.clone())"));
        assert!(!implementation.contains(".ml_auto()"));
        assert!(!implementation.contains("format!(\"by {}\""));
        let footer_start = implementation
            .find("fn contribution_footer(")
            .unwrap_or_else(|| panic!("contribution footer is missing"));
        let footer_end = implementation[footer_start..]
            .find("fn contribution_picker(")
            .map(|offset| footer_start + offset)
            .unwrap_or_else(|| panic!("contribution picker is missing"));
        let footer = &implementation[footer_start..footer_end];
        assert!(footer.contains(".border_t_1()"));
        assert!(footer.contains(".border_color(rgb(BORDER))"));
        assert!(footer.contains(".flex_none()"));
        assert!(!footer.contains(".overflow_y_scroll()"));
    }

    #[test]
    fn loading_body_uses_the_centered_message_contract() {
        let implementation = include_str!("annotation.rs")
            .split_once("#[cfg(test)]")
            .map_or_else(
                || panic!("annotation implementation section is missing"),
                |(implementation, _)| implementation,
            );
        let loading_start = implementation
            .find("fn loading_body()")
            .unwrap_or_else(|| panic!("loading body is missing"));
        let loading_end = implementation[loading_start..]
            .find("fn ready_body(")
            .map(|offset| loading_start + offset)
            .unwrap_or_else(|| panic!("loading body end marker is missing"));
        let loading = &implementation[loading_start..loading_end];
        for contract in [
            ".w_full()",
            ".flex()",
            ".flex_col()",
            ".items_center()",
            ".text_center()",
            ".mb(px(12.))",
            "local_icon(LocalIcon::MagnifyingGlass, MUTED).size(px(40.))",
            ".mb(px(4.))",
            ".text_size(px(16.))",
            ".font_weight(FontWeight::SEMIBOLD)",
            ".text_color(rgb(FOREGROUND))",
            ".text_size(px(13.))",
            ".line_height(px(19.5))",
            ".text_color(rgb(MUTED))",
            "\"Finding annotation\"",
            "\"Searching Genius for this lyric note...\"",
        ] {
            assert!(loading.contains(contract), "missing contract: {contract}");
        }
        assert!(!loading.contains("RotateRight"));
        assert!(!loading.contains("Loading Genius annotation"));
    }

    #[gpui::test]
    fn short_annotation_is_content_sized_and_keeps_author_visible(cx: &mut TestAppContext) {
        cx.update(|cx| {
            gpui_component::init(cx);
            crate::theme::configure_component_theme(cx);
        });
        let clicked = Rc::new(Cell::new(false));
        let window = cx.add_window({
            let clicked = clicked.clone();
            move |_, _| AnnotationPanelProbe {
                clicked,
                selected: 0,
                state: AnnotationState::Ready(annotation("fragment")),
                scroll: ScrollHandle::new(),
                browser_scroll: BrowserScrollState::new(),
                max_height: 260.,
            }
        });
        let mut visual = VisualTestContext::from_window(AnyWindowHandle::from(window), cx);
        visual.run_until_parked();
        visual.update(|window, cx| {
            _ = window.draw(cx);
        });

        let panel = visual
            .debug_bounds("genius-annotation-panel")
            .expect("annotation panel should render");
        let votes = visual
            .debug_bounds("genius-annotation-votes")
            .expect("annotation votes should render");
        let author = visual
            .debug_bounds("genius-annotation-author")
            .expect("annotation author should render");
        let author_text = visual
            .debug_bounds("genius-annotation-author-text")
            .expect("annotation author text should render");

        assert!(
            f32::from(panel.size.height) < 260.,
            "short panel height was {}",
            f32::from(panel.size.height)
        );
        assert_eq!(f32::from(panel.size.height), PANEL_PREFERRED_HEIGHT);
        assert!(f32::from(author.size.width) > 0.);
        assert!(f32::from(author_text.size.width) > 0.);
        assert!(author.left() >= votes.right());
        assert!(author_text.right() <= author.right());
        visual
            .debug_bounds("genius-annotation-footer")
            .expect("single-contribution notes keep footer chrome");
        assert!(visual.debug_bounds("genius-annotation-picker").is_none());
    }

    #[gpui::test]
    fn multiple_contributions_keep_a_centered_footer_picker(cx: &mut TestAppContext) {
        cx.update(|cx| {
            gpui_component::init(cx);
            crate::theme::configure_component_theme(cx);
        });
        let window = cx.add_window(move |_, _| AnnotationPanelProbe {
            clicked: Rc::new(Cell::new(false)),
            selected: 0,
            state: AnnotationState::Ready(two_annotations()),
            scroll: ScrollHandle::new(),
            browser_scroll: BrowserScrollState::new(),
            max_height: 260.,
        });
        let mut visual = VisualTestContext::from_window(AnyWindowHandle::from(window), cx);
        visual.run_until_parked();
        visual.update(|window, cx| {
            _ = window.draw(cx);
        });

        let votes = visual
            .debug_bounds("genius-annotation-votes")
            .expect("annotation votes should render");
        let picker = visual
            .debug_bounds("genius-annotation-picker")
            .expect("contribution picker should render");
        let author = visual
            .debug_bounds("genius-annotation-author")
            .expect("annotation author should render");
        let index = visual
            .debug_bounds("genius-annotation-picker-index")
            .expect("picker index should render");
        let author_text = visual
            .debug_bounds("genius-annotation-author-text")
            .expect("annotation author text should render");
        let footer = visual
            .debug_bounds("genius-annotation-footer")
            .expect("annotation footer should render");
        let scroll = visual
            .debug_bounds("genius-annotation-scroll")
            .expect("annotation scroll viewport should render");

        assert!(picker.left() >= votes.right());
        assert!(picker.right() <= author.left());
        assert!(
            f32::from(picker.size.width) > 8.,
            "contribution picker width was {}",
            f32::from(picker.size.width)
        );
        assert!(f32::from(index.size.width) > 0.);
        assert!(author_text.right() <= author.right());
        assert!(
            f32::from(footer.top()) + 0.5 >= f32::from(scroll.bottom()),
            "footer should sit under the scroll, not inside it"
        );
        assert!(picker.top() >= footer.top());
        assert!(picker.bottom() <= footer.bottom());

        let next = visual
            .debug_bounds("genius-annotation-picker-next")
            .expect("next contribution control should render");
        visual.simulate_click(next.center(), Modifiers::default());
        visual.run_until_parked();
        visual.update(|window, cx| {
            _ = window.draw(cx);
        });

        let selected = window
            .update(cx, |probe, _, _| probe.selected)
            .expect("annotation probe should stay alive");
        assert_eq!(selected, 1);
    }

    #[gpui::test]
    fn overflowing_annotation_has_scroll_range_and_mouse_wheel_scrolls(cx: &mut TestAppContext) {
        cx.update(|cx| {
            gpui_component::init(cx);
            crate::theme::configure_component_theme(cx);
        });
        let scroll = ScrollHandle::new();
        let window = cx.add_window({
            let scroll = scroll.clone();
            move |_, _| AnnotationPanelProbe {
                clicked: Rc::new(Cell::new(false)),
                selected: 0,
                state: AnnotationState::Ready(overflowing_annotation()),
                scroll,
                browser_scroll: BrowserScrollState::new(),
                max_height: 260.,
            }
        });
        let mut visual = VisualTestContext::from_window(AnyWindowHandle::from(window), cx);
        visual.run_until_parked();
        visual.update(|window, cx| {
            _ = window.draw(cx);
        });

        assert!(
            f32::from(scroll.max_offset().y).abs() > 0.,
            "overflowing annotation should expose a non-zero scroll range"
        );
        let scroll_bounds = visual
            .debug_bounds("genius-annotation-scroll")
            .expect("annotation scroll viewport should render");

        visual.simulate_event(gpui::ScrollWheelEvent {
            position: scroll_bounds.center(),
            delta: gpui::ScrollDelta::Lines(gpui::point(0., -3.)),
            ..Default::default()
        });
        for _ in 0..8 {
            visual.run_until_parked();
            visual.update(|window, cx| {
                window.simulate_next_frame(cx);
                _ = window.draw(cx);
            });
        }

        assert!(
            scroll.offset().y < px(0.),
            "discrete mouse-wheel input should move the annotation scroll offset"
        );
    }

    #[gpui::test]
    fn overflowing_annotation_scrollbar_track_changes_the_scroll_offset(cx: &mut TestAppContext) {
        cx.update(|cx| {
            gpui_component::init(cx);
            crate::theme::configure_component_theme(cx);
        });
        let scroll = ScrollHandle::new();
        let window = cx.add_window({
            let scroll = scroll.clone();
            move |_, _| AnnotationPanelProbe {
                clicked: Rc::new(Cell::new(false)),
                selected: 0,
                state: AnnotationState::Ready(overflowing_annotation()),
                scroll,
                browser_scroll: BrowserScrollState::new(),
                max_height: 260.,
            }
        });
        let mut visual = VisualTestContext::from_window(AnyWindowHandle::from(window), cx);
        visual.run_until_parked();
        visual.update(|window, cx| {
            _ = window.draw(cx);
        });

        let scroll_bounds = visual
            .debug_bounds("genius-annotation-scroll")
            .expect("annotation scroll viewport should render");

        // Make the scrolling-only scrollbar visible before exercising its track.
        visual.simulate_event(gpui::ScrollWheelEvent {
            position: scroll_bounds.center(),
            delta: gpui::ScrollDelta::Pixels(gpui::point(px(0.), px(-40.))),
            ..Default::default()
        });
        visual.update(|window, cx| {
            _ = window.draw(cx);
        });
        let before_track_click = scroll.offset().y;

        let track_position = gpui::point(
            scroll_bounds.right() - px(3.),
            scroll_bounds.top() + scroll_bounds.size.height * 0.8,
        );
        visual.simulate_click(track_position, Modifiers::default());
        visual.update(|window, cx| {
            _ = window.draw(cx);
        });

        assert!(
            scroll.offset().y < before_track_click,
            "clicking the annotation scrollbar track should move the scroll offset"
        );
    }

    #[gpui::test]
    fn overflowing_annotation_scrollbar_thumb_can_be_dragged(cx: &mut TestAppContext) {
        cx.update(|cx| {
            gpui_component::init(cx);
            crate::theme::configure_component_theme(cx);
        });
        let scroll = ScrollHandle::new();
        let window = cx.add_window({
            let scroll = scroll.clone();
            move |_, _| AnnotationPanelProbe {
                clicked: Rc::new(Cell::new(false)),
                selected: 0,
                state: AnnotationState::Ready(overflowing_annotation()),
                scroll,
                browser_scroll: BrowserScrollState::new(),
                max_height: 260.,
            }
        });
        let mut visual = VisualTestContext::from_window(AnyWindowHandle::from(window), cx);
        visual.run_until_parked();
        visual.update(|window, cx| {
            _ = window.draw(cx);
        });

        let scroll_bounds = visual
            .debug_bounds("genius-annotation-scroll")
            .expect("annotation scroll viewport should render");
        let scrollbar_bounds = visual
            .debug_bounds("genius-annotation-scrollbar")
            .expect("annotation scrollbar overlay should render");

        // Make the scrolling-only thumb visible and move it slightly off the top edge.
        visual.simulate_event(gpui::ScrollWheelEvent {
            position: scroll_bounds.center(),
            delta: gpui::ScrollDelta::Pixels(gpui::point(px(0.), px(-30.))),
            ..Default::default()
        });
        visual.update(|window, cx| {
            _ = window.draw(cx);
        });
        let before_drag = scroll.offset().y;

        let thumb_position = gpui::point(
            scrollbar_bounds.right() - px(8.),
            scrollbar_bounds.top() + px(24.),
        );
        let drag_position = gpui::point(thumb_position.x, thumb_position.y + px(70.));
        visual.simulate_mouse_down(
            thumb_position,
            gpui::MouseButton::Left,
            Modifiers::default(),
        );
        std::thread::sleep(std::time::Duration::from_millis(10));
        visual.simulate_mouse_move(
            drag_position,
            Some(gpui::MouseButton::Left),
            Modifiers::default(),
        );
        visual.simulate_mouse_up(drag_position, gpui::MouseButton::Left, Modifiers::default());
        visual.update(|window, cx| {
            _ = window.draw(cx);
        });

        assert!(
            scroll.offset().y < before_drag,
            "dragging the annotation scrollbar thumb downward should move the scroll offset"
        );
    }

    #[test]
    fn panel_height_is_readable_normally_and_yields_to_short_viewports() {
        assert_eq!(panel_inner_height_bounds(260.), (229., 239.));
        assert_eq!(panel_inner_height_bounds(180.), (159., 159.));
        assert_eq!(panel_inner_height_bounds(8.), (0., 0.));

        for max_height in [8., 63., 180., 260., 360.] {
            let (minimum, maximum) = panel_inner_height_bounds(max_height);
            assert!(minimum <= maximum);
            assert!(maximum <= (max_height - PANEL_OUTER_CHROME_HEIGHT).max(0.));
        }
    }

    #[gpui::test]
    fn rendered_panel_prefers_250px_and_preserves_chrome_at_tiny_caps(cx: &mut TestAppContext) {
        cx.update(|cx| {
            gpui_component::init(cx);
            crate::theme::configure_component_theme(cx);
        });

        for (max_height, expected_height) in [(260., 250.), (180., 180.), (8., 21.)] {
            let window = cx.add_window(move |_, _| AnnotationPanelProbe {
                clicked: Rc::new(Cell::new(false)),
                selected: 0,
                state: AnnotationState::Ready(annotation("fragment")),
                scroll: ScrollHandle::new(),
                browser_scroll: BrowserScrollState::new(),
                max_height,
            });
            let mut visual = VisualTestContext::from_window(AnyWindowHandle::from(window), cx);
            visual.run_until_parked();
            visual.update(|window, cx| {
                _ = window.draw(cx);
            });

            let panel = visual
                .debug_bounds("genius-annotation-panel")
                .expect("annotation panel should render");
            assert_eq!(f32::from(panel.size.height), expected_height);
        }
    }

    #[gpui::test]
    fn close_control_is_visible_and_activates_its_handler(cx: &mut TestAppContext) {
        cx.update(|cx| {
            gpui_component::init(cx);
            crate::theme::configure_component_theme(cx);
        });
        let clicked = Rc::new(Cell::new(false));
        let window = cx.add_window({
            let clicked = clicked.clone();
            move |_, _| AnnotationPanelProbe {
                clicked,
                selected: 0,
                state: AnnotationState::Ready(annotation("fragment")),
                scroll: ScrollHandle::new(),
                browser_scroll: BrowserScrollState::new(),
                max_height: 260.,
            }
        });
        let mut visual = VisualTestContext::from_window(AnyWindowHandle::from(window), cx);
        visual.run_until_parked();
        visual.update(|window, cx| {
            _ = window.draw(cx);
        });

        let close = visual
            .debug_bounds("genius-annotation-close-control")
            .expect("annotation close button should be rendered");
        visual.simulate_click(close.center(), Modifiers::default());

        assert!(clicked.get());
    }
}
