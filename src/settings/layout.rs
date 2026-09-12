pub(super) const NARROW_BREAKPOINT: f32 = 640.;
pub(super) const SETTINGS_CONTENT_MAX_WIDTH: f32 = 900.;
const WIDE_CONTENT_PADDING: f32 = 32.;
const NARROW_CONTENT_PADDING: f32 = 16.;

pub(super) fn content_padding(viewport_width: f32) -> f32 {
    if is_narrow(viewport_width) {
        NARROW_CONTENT_PADDING
    } else {
        WIDE_CONTENT_PADDING
    }
}

#[cfg(test)]
pub(super) fn folder_row_stacked(viewport_width: f32) -> bool {
    viewport_width <= crate::music_ui::SETTINGS_FOLDER_STACK_MAX
}

pub(super) fn is_narrow(viewport_width: f32) -> bool {
    viewport_width <= NARROW_BREAKPOINT
}

#[cfg(test)]
mod tests {
    use super::{SETTINGS_CONTENT_MAX_WIDTH, content_padding, folder_row_stacked, is_narrow};

    #[test]
    fn main_pane_content_stays_capped_and_readable() {
        assert_eq!(SETTINGS_CONTENT_MAX_WIDTH, 900.);
    }

    #[test]
    fn content_padding_tracks_the_narrow_breakpoint() {
        assert_eq!(content_padding(641.), 32.);
        assert_eq!(content_padding(640.), 16.);
    }

    #[test]
    fn narrow_layout_switches_at_the_reference_breakpoint() {
        assert!(!is_narrow(641.));
        assert!(is_narrow(640.));
    }

    #[test]
    fn download_folder_controls_stack_at_640() {
        assert!(!folder_row_stacked(641.));
        assert!(folder_row_stacked(640.));
    }
}
