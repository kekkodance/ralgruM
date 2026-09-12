use gpui::{App, px, rgb};
use gpui_component::{
    scroll::ScrollbarShow,
    theme::{Theme, ThemeMode, ThemeTokens},
};

pub(crate) const BACKGROUND: u32 = 0x09090b;
pub(crate) const CHROME: u32 = 0x0c0c0e;
pub(crate) const SURFACE: u32 = 0x121215;
pub(crate) const SURFACE_RAISED: u32 = 0x18181b;
pub(crate) const BORDER: u32 = 0x27272a;
pub(crate) const SCROLLBAR_THUMB: u32 = 0x3f3f46;
pub(crate) const FOREGROUND: u32 = 0xfafafa;
pub(crate) const MUTED: u32 = 0xa1a1aa;
pub(crate) const PRIMARY: u32 = 0x6366f1;
pub(crate) const DEEZER: u32 = 0xa238ff;
pub(crate) const SOUNDCLOUD: u32 = 0xff5500;
pub(crate) const DANGER: u32 = 0xef4444;

pub(crate) fn ui_font_family() -> &'static str {
    match std::env::consts::OS {
        // gpui maps this virtual name to the SF system font on macOS.
        "macos" => ".SystemUIFont",
        "windows" => "Segoe UI",
        // Widely installed on Linux; if missing, gpui falls back through its
        // own font stack (Ubuntu, Noto Sans, Arial and others).
        _ => "DejaVu Sans",
    }
}

pub(crate) fn configure_component_theme(cx: &mut App) {
    Theme::change(ThemeMode::Dark, None, cx);

    let theme = Theme::global_mut(cx);
    theme.font_family = ui_font_family().into();
    theme.radius = px(6.);
    theme.shadow = false;
    theme.background = rgb(BACKGROUND).into();
    theme.foreground = rgb(FOREGROUND).into();
    theme.border = rgb(BORDER).into();
    theme.input = rgb(BORDER).into();
    theme.caret = rgb(FOREGROUND).into();
    theme.muted = rgb(SURFACE_RAISED).into();
    theme.muted_foreground = rgb(MUTED).into();
    theme.accent = rgb(PRIMARY).into();
    theme.accent_foreground = rgb(FOREGROUND).into();
    theme.primary = rgb(PRIMARY).into();
    theme.primary_foreground = rgb(FOREGROUND).into();
    theme.ring = rgb(PRIMARY).into();
    theme.selection = rgb(PRIMARY).into();
    theme.switch_thumb = rgb(FOREGROUND).into();
    theme.overlay = gpui::Hsla::from(gpui::rgba(0x000000cc));
    theme.tokens = ThemeTokens::from(&theme.colors);
    // Menus highlight with the original's neutral zinc hover instead of the
    // indigo accent, which stays reserved for switches, sliders, and rings.
    // The popover tint matches .entity-context-menu's near-opaque background.
    theme.tokens.accent = gpui::Hsla::from(rgb(BORDER)).into();
    theme.tokens.accent_foreground = gpui::Hsla::from(rgb(FOREGROUND)).into();
    theme.tokens.scrollbar_thumb = gpui::Hsla::from(rgb(SCROLLBAR_THUMB)).into();
    theme.tokens.popover = gpui::Hsla::from(gpui::rgba(0x121215fc)).into();
    // Hover reveals hidden scrollbars while keeping auto hide otherwise.
    // The default scrolling mode only shows the thumb during scroll and never
    // tracks hover while hidden, so hidden bars stay hidden on mouse hover.
    theme.scrollbar_show = ScrollbarShow::Hover;
}

#[cfg(test)]
mod tests {
    use gpui::TestAppContext;

    use gpui_component::ActiveTheme;
    use gpui_component::scroll::ScrollbarShow;

    #[gpui::test]
    fn component_theme_reveals_hidden_scrollbars_on_hover(cx: &mut TestAppContext) {
        cx.update(|cx| {
            gpui_component::init(cx);
            super::configure_component_theme(cx);

            assert_eq!(cx.theme().scrollbar_show, ScrollbarShow::Hover);
        });
    }

    #[gpui::test]
    fn component_input_ring_matches_custom_keyboard_focus_border(cx: &mut TestAppContext) {
        cx.update(|cx| {
            gpui_component::init(cx);
            super::configure_component_theme(cx);

            assert_eq!(cx.theme().ring, gpui::rgb(super::PRIMARY).into());
        });
    }
}
