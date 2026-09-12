use gpui::{
    AnyElement, App, Bounds, ClickEvent, Pixels, SharedString, Window, canvas, div, fill,
    prelude::*, px, rgb, size,
};

use super::annotation::GENIUS_ACCENT;
use super::core::LyricFragment;

const DOT_SIZE: f32 = 1.5;
const DOT_STEP: f32 = 4.0;

pub(super) fn render(
    id: String,
    line_index: usize,
    text: String,
    on_click: impl Fn(&ClickEvent, &mut Window, &mut App) + 'static,
) -> AnyElement {
    let group: SharedString = format!("genius-fragment-{id}-{line_index}").into();
    div()
        .id(format!("genius-annotation-{id}-{line_index}"))
        .debug_selector(|| format!("genius-annotation-fragment-{id}"))
        .relative()
        .min_w_0()
        .max_w_full()
        .flex_none()
        .group(group.clone())
        .cursor_pointer()
        .hover(|style| style.text_color(rgb(crate::theme::FOREGROUND)))
        .on_click(move |event, window, cx| {
            cx.stop_propagation();
            on_click(event, window, cx);
        })
        .child(text)
        .child(
            div()
                .absolute()
                .left_0()
                .right_0()
                .bottom_0()
                .h(px(DOT_SIZE))
                .group_hover(group.clone(), |style| style.invisible())
                .child(
                    canvas(
                        |_, _, _| (),
                        |bounds, _, window, _| {
                            for dot in dotted_underline_quads(bounds) {
                                window.paint_quad(
                                    fill(dot, rgb(GENIUS_ACCENT)).corner_radii(px(DOT_SIZE * 0.5)),
                                );
                            }
                        },
                    )
                    .size_full(),
                ),
        )
        .child(
            div()
                .absolute()
                .left_0()
                .right_0()
                .bottom_0()
                .h(px(1.))
                .invisible()
                .bg(rgb(GENIUS_ACCENT))
                .group_hover(group, |style| style.visible()),
        )
        .into_any_element()
}

pub(super) fn render_plain(text: String) -> gpui::Div {
    div().min_w_0().max_w_full().child(text)
}

pub(super) fn first_annotation_id(fragments: &[LyricFragment]) -> Option<String> {
    fragments
        .iter()
        .find_map(|fragment| fragment.annotation_id.clone())
}

fn dotted_underline_quads(bounds: Bounds<Pixels>) -> Vec<Bounds<Pixels>> {
    let width = f32::from(bounds.size.width).max(0.);
    let mut dots = Vec::new();
    let mut x = 0.;
    while x + DOT_SIZE <= width {
        dots.push(Bounds::new(
            gpui::point(bounds.left() + px(x), bounds.top()),
            size(px(DOT_SIZE), px(DOT_SIZE)),
        ));
        x += DOT_STEP;
    }
    dots
}

#[cfg(test)]
mod tests {
    use std::{cell::Cell, rc::Rc};

    use gpui::{
        AnyWindowHandle, Bounds, Context, Modifiers, Render, TestAppContext, VisualTestContext,
        point,
    };

    use super::*;

    struct FragmentEventProbe {
        fragment_clicks: Rc<Cell<usize>>,
        row_clicks: Rc<Cell<usize>>,
    }

    struct FragmentWidthProbe;

    struct LongFragmentWidthProbe;

    struct AdjacentFragmentProbe;

    struct LongPlainFragmentProbe;

    impl Render for FragmentWidthProbe {
        fn render(&mut self, _: &mut Window, _: &mut Context<Self>) -> impl IntoElement {
            div()
                .id("genius-width-row")
                .debug_selector(|| "genius-width-row".into())
                .w(px(400.))
                .flex()
                .flex_row()
                .child(render("7".into(), 0, "annotated".into(), |_, _, _| {}))
                .child(
                    div()
                        .debug_selector(|| "genius-plain-sibling".into())
                        .child(" plain text after the annotation"),
                )
        }
    }

    impl Render for LongFragmentWidthProbe {
        fn render(&mut self, _: &mut Window, _: &mut Context<Self>) -> impl IntoElement {
            div()
                .debug_selector(|| "genius-long-width-row".into())
                .w(px(180.))
                .flex()
                .flex_row()
                .child(render(
                    "8".into(),
                    0,
                    "an annotated fragment that must wrap within its lyrics row".into(),
                    |_, _, _| {},
                ))
        }
    }

    impl Render for AdjacentFragmentProbe {
        fn render(&mut self, _: &mut Window, _: &mut Context<Self>) -> impl IntoElement {
            div()
                .debug_selector(|| "genius-adjacent-row".into())
                .w(px(400.))
                .flex()
                .flex_row()
                .flex_wrap()
                .items_baseline()
                .child(render("1".into(), 0, "Bangarang".into(), |_, _, _| {}))
                .child(render_plain(" (".into()))
                .child(render("2".into(), 0, "Bass".into(), |_, _, _| {}))
                .child(render_plain(")".into()))
        }
    }

    impl Render for LongPlainFragmentProbe {
        fn render(&mut self, _: &mut Window, _: &mut Context<Self>) -> impl IntoElement {
            div()
                .flex()
                .flex_col()
                .child(
                    div()
                        .debug_selector(|| "genius-short-plain-row".into())
                        .w(px(180.))
                        .flex()
                        .flex_row()
                        .flex_wrap()
                        .items_baseline()
                        .child(
                            render_plain("Hi".into())
                                .debug_selector(|| "genius-short-plain-fragment".into()),
                        ),
                )
                .child(
                    div()
                        .debug_selector(|| "genius-long-plain-row".into())
                        .w(px(180.))
                        .flex()
                        .flex_row()
                        .flex_wrap()
                        .items_baseline()
                        .child(
                            render_plain(
                                "Now greetings to the world, voice of the one, Big Gong-Zilla alongside Skrillex, dem fe know!".into(),
                            )
                            .debug_selector(|| "genius-long-plain-fragment".into()),
                        ),
                )
        }
    }

    impl Render for FragmentEventProbe {
        fn render(&mut self, _: &mut Window, _: &mut Context<Self>) -> impl IntoElement {
            let fragment_clicks = self.fragment_clicks.clone();
            let row_clicks = self.row_clicks.clone();
            div()
                .id("genius-row-probe")
                .debug_selector(|| "genius-row-probe".into())
                .on_click(move |_, _, _| row_clicks.set(row_clicks.get() + 1))
                .child(render("7".into(), 0, "annotated".into(), move |_, _, _| {
                    fragment_clicks.set(fragment_clicks.get() + 1);
                }))
        }
    }

    #[test]
    fn dotted_underline_uses_repeated_one_and_a_half_pixel_rounds() {
        let dots = dotted_underline_quads(Bounds::new(
            point(px(10.), px(20.)),
            size(px(10.), px(DOT_SIZE)),
        ));

        assert_eq!(dots.len(), 3);
        assert_eq!(dots[0].origin, point(px(10.), px(20.)));
        assert_eq!(dots[1].origin, point(px(14.), px(20.)));
        assert_eq!(dots[2].origin, point(px(18.), px(20.)));
        assert!(
            dots.iter()
                .all(|dot| dot.size == size(px(DOT_SIZE), px(DOT_SIZE)))
        );
    }

    #[test]
    fn annotation_hover_swaps_dots_for_a_solid_line_without_a_fragment_background() {
        let source = include_str!("genius_fragment.rs");
        let production = source
            .split("#[cfg(test)]")
            .next()
            .expect("production source");
        assert!(production.contains("group_hover(group.clone(), |style| style.invisible())"));
        assert!(production.contains(".h(px(1.))"));
        assert!(production.contains("group_hover(group, |style| style.visible())"));
        assert!(production.contains(".size_full(),"));
        assert!(!production.contains("hover(|style| style.bg("));
    }

    #[test]
    fn row_activation_targets_the_first_annotation_only() {
        let fragments = vec![
            LyricFragment {
                text: "plain ".into(),
                annotation_id: None,
            },
            LyricFragment {
                text: "first".into(),
                annotation_id: Some("7".into()),
            },
            LyricFragment {
                text: " second".into(),
                annotation_id: Some("8".into()),
            },
        ];

        assert_eq!(first_annotation_id(&fragments).as_deref(), Some("7"));
        assert_eq!(first_annotation_id(&fragments[..1]), None);
    }

    #[gpui::test]
    fn fragment_click_activates_once_without_bubbling_to_the_row(cx: &mut TestAppContext) {
        cx.update(|cx| {
            gpui_component::init(cx);
            crate::theme::configure_component_theme(cx);
        });
        let fragment_clicks = Rc::new(Cell::new(0));
        let row_clicks = Rc::new(Cell::new(0));
        let window = cx.add_window({
            let fragment_clicks = fragment_clicks.clone();
            let row_clicks = row_clicks.clone();
            move |_, _| FragmentEventProbe {
                fragment_clicks,
                row_clicks,
            }
        });
        let mut visual = VisualTestContext::from_window(AnyWindowHandle::from(window), cx);
        visual.run_until_parked();
        visual.update(|window, cx| {
            _ = window.draw(cx);
        });

        let fragment = visual
            .debug_bounds("genius-annotation-fragment-7")
            .expect("annotation fragment should render");
        visual.simulate_click(fragment.center(), Modifiers::default());

        assert_eq!(fragment_clicks.get(), 1);
        assert_eq!(row_clicks.get(), 0);
    }

    #[gpui::test]
    fn underline_root_is_only_as_wide_as_the_annotated_fragment(cx: &mut TestAppContext) {
        cx.update(|cx| {
            gpui_component::init(cx);
            crate::theme::configure_component_theme(cx);
        });
        let window = cx.add_window(|_, _| FragmentWidthProbe);
        let mut visual = VisualTestContext::from_window(AnyWindowHandle::from(window), cx);
        visual.run_until_parked();
        visual.update(|window, cx| {
            _ = window.draw(cx);
        });

        let row = visual
            .debug_bounds("genius-width-row")
            .expect("row should render");
        let fragment = visual
            .debug_bounds("genius-annotation-fragment-7")
            .expect("annotation fragment should render");
        let sibling = visual
            .debug_bounds("genius-plain-sibling")
            .expect("plain sibling should render");

        assert!(fragment.right() <= sibling.left());
        assert!(f32::from(fragment.size.width) < f32::from(row.size.width) / 2.);
    }

    #[gpui::test]
    fn long_annotated_fragment_stays_within_the_lyrics_row(cx: &mut TestAppContext) {
        cx.update(|cx| {
            gpui_component::init(cx);
            crate::theme::configure_component_theme(cx);
        });
        let window = cx.add_window(|_, _| LongFragmentWidthProbe);
        let mut visual = VisualTestContext::from_window(AnyWindowHandle::from(window), cx);
        visual.run_until_parked();
        visual.update(|window, cx| {
            _ = window.draw(cx);
        });

        let row = visual
            .debug_bounds("genius-long-width-row")
            .expect("long row should render");
        let fragment = visual
            .debug_bounds("genius-annotation-fragment-8")
            .expect("long annotation fragment should render");

        assert!(fragment.left() >= row.left());
        assert!(fragment.right() <= row.right());
    }

    #[gpui::test]
    fn short_adjacent_annotated_fragments_share_the_same_row(cx: &mut TestAppContext) {
        cx.update(|cx| {
            gpui_component::init(cx);
            crate::theme::configure_component_theme(cx);
        });
        let window = cx.add_window(|_, _| AdjacentFragmentProbe);
        let mut visual = VisualTestContext::from_window(AnyWindowHandle::from(window), cx);
        visual.run_until_parked();
        visual.update(|window, cx| {
            _ = window.draw(cx);
        });

        let bangarang = visual
            .debug_bounds("genius-annotation-fragment-1")
            .expect("Bangarang fragment should render");
        let bass = visual
            .debug_bounds("genius-annotation-fragment-2")
            .expect("Bass fragment should render");

        assert_eq!(bangarang.origin.y, bass.origin.y);
        assert!(bangarang.right() <= bass.left());
    }

    #[gpui::test]
    fn long_unannotated_fragment_wraps_within_the_lyrics_row(cx: &mut TestAppContext) {
        cx.update(|cx| {
            gpui_component::init(cx);
            crate::theme::configure_component_theme(cx);
        });
        let window = cx.add_window(|_, _| LongPlainFragmentProbe);
        let mut visual = VisualTestContext::from_window(AnyWindowHandle::from(window), cx);
        visual.run_until_parked();
        visual.update(|window, cx| {
            _ = window.draw(cx);
        });

        let row = visual
            .debug_bounds("genius-long-plain-row")
            .expect("long unannotated row should render");
        let fragment = visual
            .debug_bounds("genius-long-plain-fragment")
            .expect("long unannotated fragment should render");
        let short = visual
            .debug_bounds("genius-short-plain-fragment")
            .expect("short unannotated fragment should render");

        assert!(fragment.left() >= row.left());
        assert!(fragment.right() <= row.right());
        assert!(f32::from(fragment.size.height) > f32::from(short.size.height));
    }
}
