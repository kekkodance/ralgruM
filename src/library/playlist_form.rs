use std::ops::Range;

use gpui::{
    Context, CursorStyle, ElementId, Entity, KeyDownEvent, Role, Toggled, Window, div, prelude::*,
    px, rgb,
};
use gpui_component::{Disableable, input::InputState, switch::Switch};

use crate::{
    search::Provider,
    theme::{BACKGROUND, BORDER, FOREGROUND, PRIMARY, SURFACE_RAISED},
};

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub(super) enum PlaylistFormTarget {
    Local,
    Provider(Provider),
}

pub(super) const TITLE_LABEL: &str = "Title";
pub(super) const DESCRIPTION_LABEL: &str = "Description";
pub(super) const TITLE_PLACEHOLDER: &str = "What is this playlist called?";
pub(super) const DESCRIPTION_PLACEHOLDER: &str = "What is this playlist about?";

pub(super) fn title_input(mut input: InputState, target: PlaylistFormTarget) -> InputState {
    let limits = limits(target);
    input = input.placeholder(TITLE_PLACEHOLDER);
    input.validate(move |value, _| {
        value.chars().count() <= limits.title_max_chars && !value.chars().any(char::is_control)
    })
}

pub(super) fn description_input(input: InputState, _target: PlaylistFormTarget) -> InputState {
    input
        .multi_line(true)
        .rows(4)
        .submit_on_enter(false)
        .placeholder(DESCRIPTION_PLACEHOLDER)
}

pub(super) fn truncate_input(value: &str, max_chars: usize) -> String {
    value.chars().take(max_chars).collect()
}

pub(super) fn clamp_selection_to_prefix(
    selection: Range<usize>,
    prefix_bytes: usize,
) -> Range<usize> {
    selection.start.min(prefix_bytes)..selection.end.min(prefix_bytes)
}

pub(super) fn enforce_input_limits<T: 'static>(
    title: &Entity<InputState>,
    description: &Entity<InputState>,
    target: PlaylistFormTarget,
    window: &mut Window,
    cx: &mut Context<T>,
) {
    let limits = limits(target);
    enforce_input_limit(title, limits.title_max_chars, window, cx);
    enforce_input_limit(description, limits.description_max_chars, window, cx);
}

fn enforce_input_limit<T: 'static>(
    input: &Entity<InputState>,
    max_chars: usize,
    window: &mut Window,
    cx: &mut Context<T>,
) {
    let (text, selected_range) = {
        let input = input.read(cx);
        (input.text().to_string(), input.selected_range())
    };
    let limited = truncate_input(&text, max_chars);
    if text == limited {
        return;
    }

    let limit_bytes = limited.len();
    let adjusted_selection = clamp_selection_to_prefix(selected_range, limit_bytes);
    input.update(cx, |input, cx| {
        input.set_selected_range(limit_bytes..text.len(), cx);
        input.replace("", window, cx);
        input.set_selected_range(adjusted_selection, cx);
    });
}

pub(super) fn title_field(input: impl IntoElement) -> impl IntoElement {
    field(TITLE_LABEL, input)
}

pub(super) fn description_field(input: impl IntoElement) -> impl IntoElement {
    field(DESCRIPTION_LABEL, input)
}

pub(super) fn field(label: &'static str, input: impl IntoElement) -> impl IntoElement {
    div()
        .flex()
        .flex_col()
        .gap(px(7.))
        .child(
            div()
                .text_color(rgb(FOREGROUND))
                .text_size(px(12.))
                .font_weight(gpui::FontWeight::MEDIUM)
                .child(label),
        )
        .child(input)
}

pub(super) fn limits(
    target: PlaylistFormTarget,
) -> crate::library::playlist_limits::PlaylistTextLimits {
    match target {
        PlaylistFormTarget::Local => crate::library::playlist_limits::for_local(),
        PlaylistFormTarget::Provider(provider) => {
            crate::library::playlist_limits::for_provider(provider)
        }
    }
}

pub(super) fn validate_draft(
    target: PlaylistFormTarget,
    title: &str,
    description: &str,
) -> Result<(), &'static str> {
    let limits = limits(target);
    let title = title.trim();
    let description = description.trim();
    if title.is_empty() {
        return Err("Enter a playlist title.");
    }
    if title.chars().count() > limits.title_max_chars || title.chars().any(char::is_control) {
        return Err("Playlist title is invalid");
    }
    if description.chars().count() > limits.description_max_chars {
        return Err(match target {
            PlaylistFormTarget::Local => {
                "Playlist descriptions can contain at most 4000 characters."
            }
            PlaylistFormTarget::Provider(provider) => {
                crate::library::playlist_limits::description_limit_message(provider)
            }
        });
    }
    if description
        .chars()
        .any(|character| character.is_control() && !matches!(character, '\n' | '\r' | '\t'))
    {
        return Err("Playlist descriptions cannot contain unsupported control characters.");
    }
    Ok(())
}

pub(super) fn private_playlist_switch<T, F>(
    row_id: impl Into<ElementId>,
    switch_id: impl Into<ElementId>,
    checked: bool,
    disabled: bool,
    cx: &mut Context<T>,
    on_toggle: F,
) -> impl IntoElement
where
    T: 'static,
    F: Fn(&mut T, bool, &mut Context<T>) + Clone + 'static,
{
    let toggle_from_row = on_toggle.clone();
    let toggle_from_keyboard = on_toggle.clone();
    let toggle_from_switch = on_toggle;
    div()
        .min_h(px(44.))
        .id(row_id)
        .focusable()
        .tab_stop(!disabled)
        .role(Role::CheckBox)
        .aria_label("Private playlist")
        .aria_toggled(if checked {
            Toggled::True
        } else {
            Toggled::False
        })
        .flex()
        .items_center()
        .justify_between()
        .gap(px(14.))
        .px(px(11.))
        .border_1()
        .border_color(rgb(BORDER))
        .rounded(px(6.))
        .bg(rgb(BACKGROUND))
        .when(!disabled, |d| {
            d.cursor(CursorStyle::PointingHand)
                .hover(|style| style.border_color(rgb(0x3f3f46)).bg(rgb(SURFACE_RAISED)))
                .on_click(cx.listener(move |this, _, _, cx| {
                    toggle_from_row(this, !checked, cx);
                }))
                .on_key_down(cx.listener(move |this, event: &KeyDownEvent, window, cx| {
                    if crate::tab_keyboard::is_activation_key(event.keystroke.key.as_str()) {
                        window.prevent_default();
                        toggle_from_keyboard(this, !checked, cx);
                    }
                }))
        })
        .when(disabled, |d| {
            d.opacity(0.6).cursor(CursorStyle::OperationNotAllowed)
        })
        .focus_visible(|style| style.border_color(rgb(PRIMARY)))
        .child(
            div()
                .text_size(px(12.))
                .font_weight(gpui::FontWeight::MEDIUM)
                .child("Private playlist"),
        )
        .child(
            Switch::new(switch_id)
                .checked(checked)
                .disabled(disabled)
                .on_click(cx.listener(move |this, next_checked: &bool, _, cx| {
                    toggle_from_switch(this, *next_checked, cx);
                })),
        )
}

#[cfg(test)]
mod tests {
    use super::{
        DESCRIPTION_LABEL, DESCRIPTION_PLACEHOLDER, PlaylistFormTarget, TITLE_LABEL,
        TITLE_PLACEHOLDER, clamp_selection_to_prefix, limits, truncate_input, validate_draft,
    };
    use crate::search::Provider;

    #[test]
    fn local_and_provider_forms_share_the_text_copy() {
        assert_eq!(TITLE_LABEL, "Title");
        assert_eq!(DESCRIPTION_LABEL, "Description");
        assert_eq!(TITLE_PLACEHOLDER, "What is this playlist called?");
        assert_eq!(DESCRIPTION_PLACEHOLDER, "What is this playlist about?");
        assert_eq!(
            limits(PlaylistFormTarget::Local),
            limits(PlaylistFormTarget::Provider(Provider::SoundCloud))
        );
    }

    #[test]
    fn shared_validation_uses_local_and_provider_boundaries() {
        assert!(
            validate_draft(
                PlaylistFormTarget::Local,
                &"a".repeat(100),
                &"b".repeat(4000)
            )
            .is_ok()
        );
        assert!(
            validate_draft(
                PlaylistFormTarget::Provider(Provider::SoundCloud),
                &"a".repeat(101),
                ""
            )
            .is_err()
        );
        assert_eq!(
            validate_draft(PlaylistFormTarget::Local, "  ", ""),
            Err("Enter a playlist title.")
        );
    }

    #[test]
    fn multiline_builder_has_no_single_line_validator() {
        let source = include_str!("playlist_form.rs");
        let start = source
            .find("pub(super) fn description_input")
            .expect("description builder");
        let end = source[start..]
            .find("pub(super) fn title_field")
            .map(|offset| start + offset)
            .expect("description builder end");
        assert!(!source[start..end].contains(".validate("));
    }

    #[test]
    fn shared_limit_helpers_are_unicode_safe_and_preserve_prefix_selection() {
        assert_eq!(truncate_input("é🎵abc", 2), "é🎵");
        assert_eq!(truncate_input("a\nb\nc", 3), "a\nb");
        assert_eq!(clamp_selection_to_prefix(2..12, "é🎵".len()), 2..6);
        assert_eq!(clamp_selection_to_prefix(0..1, 0), 0..0);
    }

    #[test]
    fn all_playlist_forms_use_shared_render_time_limit_enforcement() {
        let create = include_str!("playlist_create_view.rs");
        let edit = include_str!("playlist_dialog.rs");
        let local = include_str!("local_playlist_dialog.rs");
        for source in [create, edit, local] {
            assert!(source.contains("playlist_form::enforce_input_limits"));
        }
        assert!(create.contains("PlaylistFormTarget::Provider(self.provider)"));
        assert!(edit.contains("PlaylistFormTarget::Provider(self.provider)"));
        assert!(local.contains("PlaylistFormTarget::Local"));
        assert_eq!(
            limits(PlaylistFormTarget::Provider(Provider::Deezer)).title_max_chars,
            50
        );
        assert_eq!(
            limits(PlaylistFormTarget::Provider(Provider::Deezer)).description_max_chars,
            200
        );
        assert_eq!(
            limits(PlaylistFormTarget::Provider(Provider::SoundCloud)).title_max_chars,
            100
        );
        assert_eq!(
            limits(PlaylistFormTarget::Provider(Provider::SoundCloud)).description_max_chars,
            4000
        );
        assert_eq!(limits(PlaylistFormTarget::Local).title_max_chars, 100);
        assert_eq!(
            limits(PlaylistFormTarget::Local).description_max_chars,
            4000
        );
    }
}
