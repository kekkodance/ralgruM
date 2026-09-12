use std::ops::Range;

use gpui::{App, Entity, InteractiveElement, IntoElement, ParentElement, Window, div, px};
use gpui_component::{
    Icon,
    input::{Copy, Cut, Input, InputState, Paste, SelectAll},
};

use super::{ContextMenuExt, PopupMenu, PopupMenuItem, palette::TEXT_FIELD_CONTEXT_MENU_WIDTH};
use crate::assets::LocalIcon;

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum TextFieldAction {
    Cut,
    Copy,
    Paste,
    SelectAll,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
struct TextFieldMenuEntry {
    action: TextFieldAction,
    label: &'static str,
    shortcut: &'static str,
    icon: LocalIcon,
    enabled: bool,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum TextFieldMenuDefinition {
    Action(TextFieldMenuEntry),
    Separator,
}

#[derive(Clone, Debug, PartialEq, Eq)]
struct TextFieldSnapshot {
    selected_range: Range<usize>,
    selection_reversed: bool,
    value_len: usize,
    editable: bool,
}

impl TextFieldSnapshot {
    fn from_input(input: &Entity<InputState>, options: TextFieldMenuOptions, cx: &App) -> Self {
        let state = input.read(cx);
        let selected_range = state.selected_range();
        Self {
            selection_reversed: !selected_range.is_empty()
                && state.cursor() == selected_range.start,
            selected_range,
            value_len: state.text().len(),
            editable: options.editable,
        }
    }

    fn restore_selection(&self, state: &mut InputState, cx: &mut gpui::Context<InputState>) {
        restore_saved_selection(
            state,
            self.selected_range.clone(),
            self.selection_reversed,
            cx,
        );
    }

    fn definitions(&self) -> [TextFieldMenuDefinition; 5] {
        let has_selection = !self.selected_range.is_empty();
        [
            TextFieldMenuDefinition::Action(TextFieldMenuEntry {
                action: TextFieldAction::Cut,
                label: "Cut",
                shortcut: "Ctrl+X",
                icon: LocalIcon::Scissors,
                enabled: self.editable && has_selection,
            }),
            TextFieldMenuDefinition::Action(TextFieldMenuEntry {
                action: TextFieldAction::Copy,
                label: "Copy",
                shortcut: "Ctrl+C",
                icon: LocalIcon::Copy,
                enabled: has_selection,
            }),
            TextFieldMenuDefinition::Action(TextFieldMenuEntry {
                action: TextFieldAction::Paste,
                label: "Paste",
                shortcut: "Ctrl+V",
                icon: LocalIcon::Paste,
                enabled: self.editable,
            }),
            TextFieldMenuDefinition::Separator,
            TextFieldMenuDefinition::Action(TextFieldMenuEntry {
                action: TextFieldAction::SelectAll,
                label: "Select all",
                shortcut: "Ctrl+A",
                icon: LocalIcon::ObjectGroup,
                enabled: self.value_len > 0,
            }),
        ]
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) struct TextFieldMenuOptions {
    pub(crate) editable: bool,
}

impl Default for TextFieldMenuOptions {
    fn default() -> Self {
        Self { editable: true }
    }
}

pub(crate) fn text_field_context_menu(input: Input, state: Entity<InputState>) -> impl IntoElement {
    text_field_context_menu_with_options(input, state, TextFieldMenuOptions::default())
}

pub(crate) fn text_field_context_menu_with_options(
    input: Input,
    state: Entity<InputState>,
    options: TextFieldMenuOptions,
) -> impl IntoElement {
    let input = input.context_menu(|menu, _, _| menu);
    div()
        .id(("text-field-context-menu", state.entity_id()))
        .child(input)
        .context_menu(move |menu, _, cx| {
            build_text_field_menu(
                menu,
                state.clone(),
                TextFieldSnapshot::from_input(&state, options, cx),
            )
        })
}

fn build_text_field_menu(
    mut menu: PopupMenu,
    state: Entity<InputState>,
    snapshot: TextFieldSnapshot,
) -> PopupMenu {
    for definition in snapshot.definitions() {
        match definition {
            TextFieldMenuDefinition::Separator => menu = menu.separator(),
            TextFieldMenuDefinition::Action(entry) => {
                let saved_selection = snapshot.clone();
                let action = entry.action;
                let action_state = state.clone();
                menu = menu.item(
                    PopupMenuItem::new(entry.label)
                        .icon(Icon::default().path(entry.icon.path()))
                        .shortcut(entry.shortcut)
                        .disabled(!entry.enabled)
                        .on_click(move |_, window, cx| {
                            dispatch_text_field_action(
                                action,
                                &action_state,
                                saved_selection.clone(),
                                window,
                                cx,
                            );
                        }),
                );
            }
        }
    }

    menu.min_w(px(TEXT_FIELD_CONTEXT_MENU_WIDTH))
        .max_w(px(TEXT_FIELD_CONTEXT_MENU_WIDTH))
        .select_first_enabled_on_open()
}

fn dispatch_text_field_action(
    action: TextFieldAction,
    state: &Entity<InputState>,
    snapshot: TextFieldSnapshot,
    window: &mut Window,
    cx: &mut App,
) {
    let should_dispatch = action != TextFieldAction::Paste || clipboard_has_nonempty_text(cx);

    state.update(cx, |state, cx| {
        state.focus(window, cx);
        snapshot.restore_selection(state, cx);
    });

    if !should_dispatch {
        return;
    }

    match action {
        TextFieldAction::Cut => window.dispatch_action(Box::new(Cut), cx),
        TextFieldAction::Copy => window.dispatch_action(Box::new(Copy), cx),
        TextFieldAction::Paste => window.dispatch_action(Box::new(Paste), cx),
        TextFieldAction::SelectAll => window.dispatch_action(Box::new(SelectAll), cx),
    }
}

fn clamp_saved_range(range: Range<usize>, value_len: usize) -> Range<usize> {
    range.start.min(value_len)..range.end.min(value_len)
}

fn restore_saved_selection(
    state: &mut InputState,
    saved_range: Range<usize>,
    selection_reversed: bool,
    cx: &mut gpui::Context<InputState>,
) {
    let restored_range = clamp_saved_range(saved_range, state.text().len());
    let directed_range = if selection_reversed {
        restored_range.end..restored_range.start
    } else {
        restored_range
    };
    state.set_selected_range(directed_range, cx);
}

fn paste_text_should_dispatch(text: Option<&str>) -> bool {
    text.is_some_and(|text| !text.is_empty())
}

fn clipboard_has_nonempty_text(cx: &App) -> bool {
    cx.read_from_clipboard()
        .and_then(|item| item.text())
        .is_some_and(|text| paste_text_should_dispatch(Some(text.as_str())))
}

#[cfg(test)]
mod tests {
    use gpui::{
        AppContext, ClipboardItem, Context, InteractiveElement, IntoElement, Modifiers,
        MouseButton, Render, Styled, TestAppContext, VisualTestContext, Window, div, point, px,
    };
    use gpui_component::input::Input;

    use super::*;

    struct TextFieldTestView {
        input: Entity<InputState>,
        options: TextFieldMenuOptions,
    }

    impl Render for TextFieldTestView {
        fn render(&mut self, _: &mut Window, _: &mut Context<Self>) -> impl IntoElement {
            div()
                .w(px(400.))
                .h(px(40.))
                .debug_selector(|| "text-field-test".into())
                .child(text_field_context_menu_with_options(
                    Input::new(&self.input),
                    self.input.clone(),
                    self.options,
                ))
        }
    }

    fn open_text_field(
        cx: &mut TestAppContext,
        options: TextFieldMenuOptions,
    ) -> (gpui::WindowHandle<gpui_component::Root>, Entity<InputState>) {
        let mut input = None;
        let window = cx.update(|cx| {
            cx.open_window(Default::default(), |window, cx| {
                cx.set_global(gpui_component::Theme::default());
                gpui_component::init(cx);
                crate::context_menu::init(cx);

                let input_state = cx.new(|cx| InputState::new(window, cx));
                input = Some(input_state.clone());
                let view = cx.new(|_| TextFieldTestView {
                    input: input_state,
                    options,
                });
                cx.new(|cx| gpui_component::Root::new(view, window, cx))
            })
            .expect("test window")
        });
        (window, input.expect("input state"))
    }

    #[test]
    fn menu_entries_match_tauri_order_shortcuts_and_enabled_matrix() {
        let enabled = TextFieldSnapshot {
            selected_range: 1..2,
            selection_reversed: false,
            value_len: 3,
            editable: true,
        }
        .definitions();
        assert_eq!(
            enabled.map(|definition| match definition {
                TextFieldMenuDefinition::Action(entry) => Some((
                    entry.label,
                    entry.shortcut,
                    entry.icon.path(),
                    entry.enabled,
                )),
                TextFieldMenuDefinition::Separator => None,
            }),
            [
                Some(("Cut", "Ctrl+X", LocalIcon::Scissors.path(), true)),
                Some(("Copy", "Ctrl+C", LocalIcon::Copy.path(), true)),
                Some(("Paste", "Ctrl+V", LocalIcon::Paste.path(), true)),
                None,
                Some(("Select all", "Ctrl+A", LocalIcon::ObjectGroup.path(), true,)),
            ]
        );

        let disabled = TextFieldSnapshot {
            selected_range: 0..0,
            selection_reversed: false,
            value_len: 0,
            editable: false,
        }
        .definitions();
        assert_eq!(
            disabled.map(|definition| match definition {
                TextFieldMenuDefinition::Action(entry) => Some(entry.enabled),
                TextFieldMenuDefinition::Separator => None,
            }),
            [Some(false), Some(false), Some(false), None, Some(false)]
        );
    }

    #[test]
    fn non_editable_snapshots_only_disable_edit_actions() {
        let definitions = TextFieldSnapshot {
            selected_range: 1..2,
            selection_reversed: false,
            value_len: 3,
            editable: false,
        }
        .definitions();

        assert_eq!(
            definitions.map(|definition| match definition {
                TextFieldMenuDefinition::Action(entry) => Some((entry.action, entry.enabled)),
                TextFieldMenuDefinition::Separator => None,
            }),
            [
                Some((TextFieldAction::Cut, false)),
                Some((TextFieldAction::Copy, true)),
                Some((TextFieldAction::Paste, false)),
                None,
                Some((TextFieldAction::SelectAll, true)),
            ]
        );
    }

    #[test]
    fn saved_ranges_are_clamped_to_the_current_value() {
        assert_eq!(clamp_saved_range(1..8, 3), 1..3);
        assert_eq!(clamp_saved_range(8..8, 3), 3..3);
        assert_eq!(clamp_saved_range(1..2, 3), 1..2);
    }

    #[test]
    fn reversed_saved_ranges_restore_the_start_as_the_active_end() {
        let range = clamp_saved_range(2..8, 5);
        assert_eq!((range.end, range.start), (5, 2));
    }

    #[test]
    fn default_text_field_menu_options_are_editable() {
        assert!(TextFieldMenuOptions::default().editable);
    }

    #[test]
    fn empty_or_absent_clipboard_text_is_a_paste_no_op() {
        assert!(!paste_text_should_dispatch(None));
        assert!(!paste_text_should_dispatch(Some("")));
        assert!(paste_text_should_dispatch(Some(" ")));
        assert!(paste_text_should_dispatch(Some("text")));
    }

    #[gpui::test]
    fn restoring_a_backward_selection_preserves_the_active_end(cx: &mut TestAppContext) {
        let (window, input) = open_text_field(cx, TextFieldMenuOptions::default());
        let mut visual = VisualTestContext::from_window(gpui::AnyWindowHandle::from(window), cx);
        visual.run_until_parked();
        visual.update(|window, cx| {
            _ = window.draw(cx);
        });

        visual.update(|window, cx| {
            input.update(cx, |state, cx| {
                state.set_value("abcdef", window, cx);
                state.set_selected_range(std::ops::Range { start: 5, end: 2 }, cx);
                state.focus(window, cx);
            });
        });

        visual.run_until_parked();
        visual.update(|window, cx| {
            _ = window.draw(cx);
        });

        let snapshot = visual.update(|_, cx| {
            TextFieldSnapshot::from_input(&input, TextFieldMenuOptions::default(), cx)
        });
        assert!(snapshot.selection_reversed);
        assert_eq!(snapshot.selected_range, 2..5);

        visual.update(|window, cx| {
            input.update(cx, |state, cx| {
                state.set_selected_range(0..0, cx);
                state.focus(window, cx);
            });
            dispatch_text_field_action(TextFieldAction::Copy, &input, snapshot.clone(), window, cx);
        });
        visual.run_until_parked();
        let (copied_selection, copied_cursor, copied_text) = visual.read(|cx| {
            let state = input.read(cx);
            let copied_text = cx
                .read_from_clipboard()
                .and_then(|item| item.text())
                .map(|text| text.to_string());
            (state.selected_range(), state.cursor(), copied_text)
        });
        assert_eq!(copied_selection, 2..5);
        assert_eq!(copied_cursor, 2);
        assert_eq!(copied_text.as_deref(), Some("cde"));

        visual.update(|window, cx| {
            input.update(cx, |state, cx| {
                state.set_selected_range(0..0, cx);
                state.focus(window, cx);
            });
            cx.write_to_clipboard(ClipboardItem::new_string(String::new()));
            dispatch_text_field_action(
                TextFieldAction::Paste,
                &input,
                snapshot.clone(),
                window,
                cx,
            );
        });
        visual.run_until_parked();
        let (pasted_selection, pasted_cursor) =
            visual.read(|cx| (input.read(cx).selected_range(), input.read(cx).cursor()));
        assert_eq!(pasted_selection, 2..5);
        assert_eq!(pasted_cursor, 2);

        visual.update(|window, cx| {
            input.update(cx, |state, cx| {
                state.set_selected_range(0..0, cx);
                state.focus(window, cx);
            });
            dispatch_text_field_action(
                TextFieldAction::SelectAll,
                &input,
                snapshot.clone(),
                window,
                cx,
            );
        });
        visual.run_until_parked();
        let (all_selection, all_cursor) =
            visual.read(|cx| (input.read(cx).selected_range(), input.read(cx).cursor()));
        assert_eq!(all_selection, 0..6);
        assert_eq!(all_cursor, 0);

        visual.update(|window, cx| {
            input.update(cx, |state, cx| {
                state.set_value("abc", window, cx);
                snapshot.restore_selection(state, cx);
                assert_eq!(state.selected_range(), 2..3);
                assert_eq!(state.cursor(), 2);
            });
        });
    }

    #[gpui::test]
    fn non_editable_input_menu_disables_cut_and_paste(cx: &mut TestAppContext) {
        let options = TextFieldMenuOptions { editable: false };
        let (window, input) = open_text_field(cx, options);
        let mut visual = VisualTestContext::from_window(gpui::AnyWindowHandle::from(window), cx);
        visual.run_until_parked();
        visual.update(|window, cx| {
            _ = window.draw(cx);
        });

        visual.update(|window, cx| {
            input.update(cx, |state, cx| {
                state.set_value("abcdef", window, cx);
                state.set_selected_range(2..5, cx);
            });
        });
        visual.run_until_parked();
        visual.update(|window, cx| {
            _ = window.draw(cx);
        });

        let menu = visual.update(|window, cx| {
            let snapshot = TextFieldSnapshot::from_input(&input, options, cx);
            PopupMenu::build(window, cx, move |menu, _, _| {
                build_text_field_menu(menu, input.clone(), snapshot)
            })
        });
        let entries = visual.read(|cx| {
            menu.read(cx)
                .menu_items
                .iter()
                .filter_map(|item| match item {
                    PopupMenuItem::Item {
                        label, disabled, ..
                    } => Some((label.to_string(), *disabled)),
                    PopupMenuItem::Separator
                    | PopupMenuItem::ElementItem { .. }
                    | PopupMenuItem::Submenu { .. } => None,
                })
                .collect::<Vec<_>>()
        });

        assert_eq!(
            entries,
            vec![
                ("Cut".to_owned(), true),
                ("Copy".to_owned(), false),
                ("Paste".to_owned(), true),
                ("Select all".to_owned(), false),
            ]
        );
    }

    #[gpui::test]
    fn non_editable_context_menu_live_path_keeps_edit_actions_disabled(cx: &mut TestAppContext) {
        let options = TextFieldMenuOptions { editable: false };
        let (window, input) = open_text_field(cx, options);
        let mut visual = VisualTestContext::from_window(gpui::AnyWindowHandle::from(window), cx);
        visual.run_until_parked();
        visual.update(|window, cx| {
            _ = window.draw(cx);
        });

        visual.update(|window, cx| {
            input.update(cx, |state, cx| {
                state.set_value("abcdef", window, cx);
                state.set_selected_range(2..5, cx);
                state.focus(window, cx);
            });
        });
        visual.run_until_parked();
        visual.update(|window, cx| {
            _ = window.draw(cx);
        });

        let input_bounds = visual
            .debug_bounds("text-field-test")
            .expect("text field bounds");
        let click_position = point(input_bounds.origin.x + px(30.), input_bounds.center().y);
        visual.simulate_mouse_down(click_position, MouseButton::Right, Modifiers::none());
        visual.update(|window, cx| {
            _ = window.draw(cx);
        });

        visual.simulate_keystrokes("enter");
        let (copied_value, copied_text) = visual.read(|cx| {
            let state = input.read(cx);
            let copied_text = cx
                .read_from_clipboard()
                .and_then(|item| item.text())
                .map(|text| text.to_string());
            (state.text().to_string(), copied_text)
        });
        assert_eq!(copied_value, "abcdef");
        assert_eq!(copied_text.as_deref(), Some("cde"));

        visual.update(|window, cx| {
            input.update(cx, |state, cx| {
                state.set_selected_range(2..5, cx);
                state.focus(window, cx);
            });
            cx.write_to_clipboard(ClipboardItem::new_string("X".to_owned()));
        });
        visual.run_until_parked();
        visual.update(|window, cx| {
            _ = window.draw(cx);
        });
        visual.simulate_mouse_down(click_position, MouseButton::Right, Modifiers::none());
        visual.update(|window, cx| {
            _ = window.draw(cx);
        });
        visual.simulate_keystrokes("down enter");

        let value_after_paste_attempt = visual.read(|cx| input.read(cx).text().to_string());
        assert_eq!(value_after_paste_attempt, "abcdef");
        visual.simulate_keystrokes("escape");
    }
}
