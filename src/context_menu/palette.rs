//! Shared palette for app-owned GPUI context menus and the system-tray popup.
//!
//! Keeping these values together prevents the two GPUI menu surfaces from
//! drifting while preserving the renderer's established appearance.

/// Near-opaque surface used by app-owned GPUI menus.
pub(crate) const CONTEXT_MENU_SURFACE: u32 = 0x121215fc;
/// Border used by app-owned GPUI menus.
pub(crate) const CONTEXT_MENU_BORDER: u32 = 0x3f3f46eb;
/// Base action text and inherited icon color.
pub(crate) const CONTEXT_MENU_FOREGROUND: u32 = 0xf4f4f5;
/// Hover and keyboard-selected action background.
pub(crate) const CONTEXT_MENU_HOVER: u32 = 0x27272a;
/// Hover and keyboard-selected action text and inherited icon color.
pub(crate) const CONTEXT_MENU_HOVER_FOREGROUND: u32 = 0xfafafa;
/// Separator color retained by the existing GPUI context-menu renderer.
pub(crate) const CONTEXT_MENU_SEPARATOR: u32 = 0x3f3f46c7;

/// Fixed width used by the original Tauri text-field menu.
pub(crate) const TEXT_FIELD_CONTEXT_MENU_WIDTH: f32 = 220.;
/// Shared action-row height used by app-owned context menus.
pub(crate) const CONTEXT_MENU_ACTION_ROW_HEIGHT: f32 = 34.;
/// Shared popup padding used by app-owned context menus.
pub(crate) const CONTEXT_MENU_POPUP_PADDING: f32 = 6.;
/// Gap between adjacent action-row hover backgrounds.
pub(crate) const CONTEXT_MENU_ITEM_GAP: f32 = 2.;
/// Horizontal padding inside a standard action row.
pub(crate) const CONTEXT_MENU_ACTION_HORIZONTAL_PADDING: f32 = 8.;
/// Vertical padding inside a standard action row.
pub(crate) const CONTEXT_MENU_ACTION_VERTICAL_PADDING: f32 = 6.;
/// Width reserved for the leading action icon.
pub(crate) const CONTEXT_MENU_ICON_COLUMN_WIDTH: f32 = 20.;
/// Gap between standard action-row columns.
pub(crate) const CONTEXT_MENU_ACTION_COLUMN_GAP: f32 = 9.;
/// Standard action icon size.
pub(crate) const CONTEXT_MENU_ICON_SIZE: f32 = 13.;
/// Standard action label size.
pub(crate) const CONTEXT_MENU_LABEL_SIZE: f32 = 12.5;
/// Text-field shortcut size.
pub(crate) const CONTEXT_MENU_SHORTCUT_SIZE: f32 = 10.5;
/// Exact disabled foreground used by the original text-field menu.
pub(crate) const TEXT_FIELD_CONTEXT_MENU_DISABLED: u32 = 0x71717a;
/// Exact normal icon foreground used by the original text-field menu.
pub(crate) const TEXT_FIELD_CONTEXT_MENU_ICON: u32 = 0xd4d4d8;
/// Exact selected shortcut foreground used by the original text-field menu.
pub(crate) const TEXT_FIELD_CONTEXT_MENU_SELECTED_SHORTCUT: u32 = 0xa1a1aa;
/// Disabled opacity used by the original text-field menu.
pub(crate) const TEXT_FIELD_CONTEXT_MENU_DISABLED_OPACITY: f32 = 0.72;

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn palette_matches_current_gpui_context_menu_contract() {
        assert_eq!(CONTEXT_MENU_SURFACE, 0x121215fc);
        assert_eq!(CONTEXT_MENU_BORDER, 0x3f3f46eb);
        assert_eq!(CONTEXT_MENU_FOREGROUND, 0xf4f4f5);
        assert_eq!(CONTEXT_MENU_HOVER, 0x27272a);
        assert_eq!(CONTEXT_MENU_HOVER_FOREGROUND, 0xfafafa);
        assert_eq!(TEXT_FIELD_CONTEXT_MENU_WIDTH, 220.);
        assert_eq!(CONTEXT_MENU_ACTION_ROW_HEIGHT, 34.);
        assert_eq!(CONTEXT_MENU_POPUP_PADDING, 6.);
        assert_eq!(CONTEXT_MENU_ITEM_GAP, 2.);
        assert_eq!(CONTEXT_MENU_ICON_COLUMN_WIDTH, 20.);
        assert_eq!(CONTEXT_MENU_ACTION_COLUMN_GAP, 9.);
        assert_eq!(CONTEXT_MENU_LABEL_SIZE, 12.5);
        assert_eq!(CONTEXT_MENU_SHORTCUT_SIZE, 10.5);
        assert_eq!(TEXT_FIELD_CONTEXT_MENU_DISABLED, 0x71717a);
        assert_eq!(TEXT_FIELD_CONTEXT_MENU_ICON, 0xd4d4d8);
        assert_eq!(TEXT_FIELD_CONTEXT_MENU_SELECTED_SHORTCUT, 0xa1a1aa);
        assert_eq!(TEXT_FIELD_CONTEXT_MENU_DISABLED_OPACITY, 0.72);
    }
}
