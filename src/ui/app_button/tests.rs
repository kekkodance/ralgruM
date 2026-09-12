use super::*;

#[test]
fn dialog_buttons_build_matching_geometry() {
    let mut primary = primary_button("primary", None, "Save", |_, _, _| {});
    let mut neutral =
        secondary_dialog_button_with_disabled("neutral", None, "Cancel", false, |_, _, _| {});
    let mut danger =
        danger_secondary_dialog_button_with_disabled("danger", None, "Delete", false, |_, _, _| {});
    assert_eq!(primary.style().size.height, neutral.style().size.height);
    assert_eq!(primary.style().size.height, danger.style().size.height);
    assert_eq!(
        primary.style().corner_radii.top_left,
        neutral.style().corner_radii.top_left
    );
    assert_eq!(neutral.style().padding.left, danger.style().padding.left);
    assert_eq!(
        neutral.style().border_widths.left,
        danger.style().border_widths.left
    );
}

#[test]
fn page_buttons_keep_their_compact_height_and_disabled_feedback() {
    let mut enabled =
        secondary_page_action_button_with_disabled("enabled", None, "Open", false, |_, _, _| {});
    let mut disabled =
        secondary_page_action_button_with_disabled("disabled", None, "Open", true, |_, _, _| {});
    assert_eq!(enabled.style().size.height, Some(px(32.).into()));
    assert_eq!(disabled.style().size.height, enabled.style().size.height);
    assert_eq!(disabled.style().opacity, Some(0.5));
    assert_ne!(disabled.style().mouse_cursor, enabled.style().mouse_cursor);
}

#[test]
fn loading_and_explicitly_disabled_primary_buttons_are_inert_states() {
    assert!(primary_button_is_disabled(false, true));
    assert!(primary_button_is_disabled(true, false));
    assert!(primary_button_is_disabled(true, true));
    assert!(!primary_button_is_disabled(false, false));
}

#[test]
fn plain_close_button_builds_the_small_hit_target() {
    let mut button = plain_x_button("close", "close-group", "Close", true);
    assert_eq!(button.style().size.width, Some(px(24.).into()));
    assert_eq!(button.style().size.height, Some(px(24.).into()));
}
