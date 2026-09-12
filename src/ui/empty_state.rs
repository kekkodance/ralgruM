use gpui::{AnyElement, FontWeight, div, prelude::*, px, rgb};

use crate::{
    assets::{LocalIcon, local_icon},
    theme::{FOREGROUND, MUTED},
};

const CONTENT_PADDING_Y_PX: f32 = 60.;
const CONTENT_PADDING_X_PX: f32 = 20.;
const ICON_SIZE_PX: f32 = 40.;
const TITLE_MARGIN_TOP_PX: f32 = 12.;
const TITLE_MARGIN_BOTTOM_PX: f32 = 4.;
const TITLE_SIZE_PX: f32 = 16.;
const DESCRIPTION_SIZE_PX: f32 = 13.;
const ACTION_MARGIN_TOP_PX: f32 = 14.;

pub(crate) fn render(
    icon: LocalIcon,
    title: &str,
    description: &str,
    action: Option<AnyElement>,
) -> AnyElement {
    div()
        .flex()
        .flex_col()
        .items_center()
        .py(px(CONTENT_PADDING_Y_PX))
        .px(px(CONTENT_PADDING_X_PX))
        .text_color(rgb(MUTED))
        .child(local_icon(icon, MUTED).size(px(ICON_SIZE_PX)))
        .child(
            div()
                .mt(px(TITLE_MARGIN_TOP_PX))
                .mb(px(TITLE_MARGIN_BOTTOM_PX))
                .text_size(px(TITLE_SIZE_PX))
                .font_weight(FontWeight::SEMIBOLD)
                .text_color(rgb(FOREGROUND))
                .child(title.to_owned()),
        )
        .when(!description.is_empty(), |this| {
            this.child(
                div()
                    .text_size(px(DESCRIPTION_SIZE_PX))
                    .child(description.to_owned()),
            )
        })
        .when_some(action, |this, action| {
            this.child(div().mt(px(ACTION_MARGIN_TOP_PX)).child(action))
        })
        .into_any_element()
}

pub(crate) fn account_required_copy(provider: &str) -> (&'static str, &'static str) {
    match provider {
        "SoundCloud" => (
            "SoundCloud account required",
            "Log in to SoundCloud to load this part of your personal library.",
        ),
        "Deezer" => (
            "Deezer account required",
            "Log in to Deezer to load this part of your personal library.",
        ),
        _ => (
            "Account required",
            "Sign in to load this part of your personal library.",
        ),
    }
}

#[cfg(test)]
mod tests {
    use super::{
        ACTION_MARGIN_TOP_PX, CONTENT_PADDING_X_PX, CONTENT_PADDING_Y_PX, DESCRIPTION_SIZE_PX,
        ICON_SIZE_PX, TITLE_MARGIN_BOTTOM_PX, TITLE_MARGIN_TOP_PX, TITLE_SIZE_PX,
        account_required_copy,
    };

    #[test]
    fn shared_empty_state_preserves_the_search_prompt_layout_contract() {
        assert_eq!(CONTENT_PADDING_Y_PX, 60.);
        assert_eq!(CONTENT_PADDING_X_PX, 20.);
        assert_eq!(ICON_SIZE_PX, 40.);
        assert_eq!(TITLE_MARGIN_TOP_PX, 12.);
        assert_eq!(TITLE_MARGIN_BOTTOM_PX, 4.);
        assert_eq!(TITLE_SIZE_PX, 16.);
        assert_eq!(DESCRIPTION_SIZE_PX, 13.);
        assert_eq!(ACTION_MARGIN_TOP_PX, 14.);
    }

    #[test]
    fn account_required_copy_matches_each_provider() {
        assert_eq!(
            account_required_copy("Deezer"),
            (
                "Deezer account required",
                "Log in to Deezer to load this part of your personal library."
            )
        );
        assert_eq!(
            account_required_copy("SoundCloud"),
            (
                "SoundCloud account required",
                "Log in to SoundCloud to load this part of your personal library."
            )
        );
    }
}
