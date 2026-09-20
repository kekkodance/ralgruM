use crate::{
    app_button::SETTINGS_SECONDARY_SMALL_ICON_SIZE,
    app_tooltip::AppTooltipExt,
    assets::{LocalIcon, local_icon},
    cache::CACHE_ICON,
    settings::{
        Category as SettingsCategory, DangerSecondaryButtonOptions, SidebarPass,
        danger_secondary_button, neutral_secondary_button,
    },
    theme::{BORDER, CHROME, DANGER, FOREGROUND, MUTED, PRIMARY, SURFACE},
};
use gpui::{
    AnimationExt as _, Context, IntoElement, KeyDownEvent, Role, Window, div, prelude::*, px, rgb,
    rgba,
};

use super::{
    Nav, RalgrumApp, SidebarBottomVisual,
    sidebar_badge::{
        DOWNLOAD_BADGE_NUMERAL_OFFSET_PX, SidebarDownloadBadgeVisual,
        download_badge_position_endpoints,
    },
};

mod sidebar_update;

const SIDEBAR_NAV_HOVER_BACKGROUND: u32 = 0xffffff0a;
const SIDEBAR_NAV_ACTIVE_BACKGROUND: u32 = 0xffffff12;
const FULL_SETTINGS_BUTTON_SIZE: f32 = 30.;
const SIDEBAR_DESKTOP_WIDTH_PX: f32 = 240.;
const SIDEBAR_COMPACT_WIDTH_PX: f32 = 68.;
const SIDEBAR_HORIZONTAL_PADDING_PX: f32 = 16.;
const SIDEBAR_RIGHT_PADDING_PX: f32 = 15.;
const SIDEBAR_BORDER_WIDTH_PX: f32 = 1.;
const SIDEBAR_EXPANDED_CONTENT_WIDTH_PX: f32 = SIDEBAR_DESKTOP_WIDTH_PX
    - SIDEBAR_HORIZONTAL_PADDING_PX
    - SIDEBAR_RIGHT_PADDING_PX
    - SIDEBAR_BORDER_WIDTH_PX;
const SIDEBAR_COMPACT_CONTENT_WIDTH_PX: f32 = SIDEBAR_COMPACT_WIDTH_PX
    - SIDEBAR_HORIZONTAL_PADDING_PX
    - SIDEBAR_RIGHT_PADDING_PX
    - SIDEBAR_BORDER_WIDTH_PX;
const COMPACT_SETTINGS_BUTTON_SIZE: f32 = SIDEBAR_COMPACT_CONTENT_WIDTH_PX;
const COMPACT_SETTINGS_BUTTON_RADIUS_PX: f32 = 6.;
const COMPACT_SETTINGS_ICON_SIZE: f32 = 16.;
pub(super) const SIDEBAR_ICON_GLYPH_SIZE: f32 = crate::music_ui::NAVIGATION_ICON_GLYPH_SIZE;
pub(super) const SIDEBAR_ICON_SLOT_SIZE_PX: f32 = 18.;
const SIDEBAR_NAV_HORIZONTAL_PADDING_PX: f32 = 9.;
const SETTINGS_ICON_SIZE: f32 = 14.;
const ACCOUNT_STATUS_ICON_OFFSET_PX: f32 = 1.;
const SIDEBAR_SETTINGS_CATEGORY_GAP_PX: f32 = 4.;
const SIDEBAR_COMPACT_CONTROL_GAP_PX: f32 = SIDEBAR_SETTINGS_CATEGORY_GAP_PX;
const SIDEBAR_COMPACT_SETTINGS_ACTION_GROUP_GAP_PX: f32 = 8.;
const SIDEBAR_BOTTOM_ACCOUNT_HEADROOM_PX: f32 = 8.;
const SIDEBAR_ACCOUNT_CARD_HEIGHT_PX: f32 = 80.;
const SIDEBAR_UPDATE_ACCOUNT_GAP_PX: f32 = 8.;
const SIDEBAR_COMPACT_LOGOUT_BOTTOM_PX: f32 =
    COMPACT_SETTINGS_BUTTON_SIZE + SIDEBAR_COMPACT_SETTINGS_ACTION_GROUP_GAP_PX;
const SIDEBAR_COMPACT_TRANSFER_BOTTOM_PX: f32 = SIDEBAR_COMPACT_LOGOUT_BOTTOM_PX
    + COMPACT_SETTINGS_BUTTON_SIZE
    + SIDEBAR_COMPACT_CONTROL_GAP_PX;
const SIDEBAR_EXPANDED_TRANSFER_BOTTOM_PX: f32 =
    DangerSecondaryButtonOptions::SIDEBAR.height + SIDEBAR_COMPACT_CONTROL_GAP_PX;
const SIDEBAR_BOTTOM_HOST_HEIGHT_PX: f32 = 3. * COMPACT_SETTINGS_BUTTON_SIZE
    + SIDEBAR_COMPACT_SETTINGS_ACTION_GROUP_GAP_PX
    + SIDEBAR_COMPACT_CONTROL_GAP_PX
    + SIDEBAR_BOTTOM_ACCOUNT_HEADROOM_PX;
const SIDEBAR_UPDATE_COMPACT_HOST_HEIGHT_PX: f32 =
    SIDEBAR_BOTTOM_HOST_HEIGHT_PX + COMPACT_SETTINGS_BUTTON_SIZE + SIDEBAR_COMPACT_CONTROL_GAP_PX;
const SIDEBAR_UPDATE_EXPANDED_BOTTOM_PX: f32 = SIDEBAR_EXPANDED_TRANSFER_BOTTOM_PX
    + DangerSecondaryButtonOptions::SIDEBAR.height
    + SIDEBAR_COMPACT_CONTROL_GAP_PX;
const SIDEBAR_UPDATE_EXPANDED_DEFAULT_BOTTOM_PX: f32 =
    SIDEBAR_ACCOUNT_CARD_HEIGHT_PX + SIDEBAR_UPDATE_ACCOUNT_GAP_PX;
const SIDEBAR_UPDATE_COMPACT_DEFAULT_BOTTOM_PX: f32 =
    COMPACT_SETTINGS_BUTTON_SIZE + SIDEBAR_COMPACT_SETTINGS_ACTION_GROUP_GAP_PX;
const SIDEBAR_UPDATE_COMPACT_SETTINGS_BOTTOM_PX: f32 = SIDEBAR_COMPACT_TRANSFER_BOTTOM_PX
    + COMPACT_SETTINGS_BUTTON_SIZE
    + SIDEBAR_COMPACT_CONTROL_GAP_PX;

fn update_badge_bottom(compact: bool, settings_mode: bool) -> f32 {
    match (compact, settings_mode) {
        (false, false) => SIDEBAR_UPDATE_EXPANDED_DEFAULT_BOTTOM_PX,
        (false, true) => SIDEBAR_UPDATE_EXPANDED_BOTTOM_PX,
        (true, false) => SIDEBAR_UPDATE_COMPACT_DEFAULT_BOTTOM_PX,
        (true, true) => SIDEBAR_UPDATE_COMPACT_SETTINGS_BOTTOM_PX,
    }
}

fn update_badge_bottom_at(compact: bool, visual: SidebarBottomVisual, delta: f32) -> f32 {
    let (_, settings) = visual.fractions_at(delta);
    crate::motion::lerp(
        update_badge_bottom(compact, false),
        update_badge_bottom(compact, true),
        settings,
    )
}

fn sidebar_bottom_host_height(compact: bool, show_update: bool) -> f32 {
    if compact && show_update {
        SIDEBAR_UPDATE_COMPACT_HOST_HEIGHT_PX
    } else {
        SIDEBAR_BOTTOM_HOST_HEIGHT_PX
    }
}
const SIDEBAR_BOTTOM_EXPANDED_ACCOUNT_LAYER_ID: &str = "sidebar-bottom-expanded-account";
const SIDEBAR_BOTTOM_EXPANDED_LOGOUT_LAYER_ID: &str = "sidebar-bottom-expanded-logout";
const SIDEBAR_BOTTOM_EXPANDED_TRANSFER_LAYER_ID: &str = "sidebar-bottom-expanded-settings-transfer";
const SIDEBAR_BOTTOM_COMPACT_LOGOUT_LAYER_ID: &str = "sidebar-bottom-compact-logout";
const SIDEBAR_BOTTOM_COMPACT_TRANSFER_LAYER_ID: &str = "sidebar-bottom-compact-settings-transfer";
const SIDEBAR_BOTTOM_COMPACT_SETTINGS_LAYER_ID: &str = "sidebar-bottom-compact-settings";
const SIDEBAR_EXPANDED_LOGOUT_ID: &str = "sidebar-logout-all-expanded";
const SIDEBAR_COMPACT_LOGOUT_ID: &str = "sidebar-logout-all-compact";
const SIDEBAR_EXPANDED_TRANSFER_ID: &str = "sidebar-settings-transfer-expanded";
const SIDEBAR_COMPACT_TRANSFER_ID: &str = "sidebar-settings-transfer-compact";
const SIDEBAR_COMPACT_SETTINGS_ID: &str = "shell-settings-control-compact";
const LOGOUT_ALL_TOOLTIP: &str = "Logs out of all active services\n(Murglar, SoundCloud, Deezer)";
const SETTINGS_TRANSFER_TOOLTIP: &str = "Import or Export Settings";

#[derive(Clone, Copy, Debug, PartialEq)]
struct SidebarBottomOpacities {
    account: f32,
    expanded_logout: f32,
    expanded_transfer: f32,
    compact_settings: f32,
    compact_logout: f32,
    compact_transfer: f32,
}

fn sidebar_bottom_opacities(compact: f32, settings: f32) -> SidebarBottomOpacities {
    let compact = crate::motion::clamp_unit(compact);
    let settings = crate::motion::clamp_unit(settings);
    SidebarBottomOpacities {
        account: (1.0 - compact) * (1.0 - settings),
        expanded_logout: (1.0 - compact) * settings,
        expanded_transfer: (1.0 - compact) * settings,
        compact_settings: compact,
        compact_logout: compact * settings,
        compact_transfer: compact * settings,
    }
}

fn sidebar_bottom_opacities_at(visual: SidebarBottomVisual, delta: f32) -> SidebarBottomOpacities {
    let (compact, settings) = visual.fractions_at(delta);
    sidebar_bottom_opacities(compact, settings)
}

fn sidebar_update_opacities_at(visual: SidebarBottomVisual, delta: f32) -> (f32, f32, f32) {
    let (compact, settings) = visual.fractions_at(delta);
    (
        1.0 - compact,
        compact * (1.0 - settings),
        compact * settings,
    )
}

fn sidebar_expanded_content_width(narrow_content: bool) -> f32 {
    if narrow_content {
        crate::music_ui::MOBILE_DRAWER_WIDTH
            - SIDEBAR_HORIZONTAL_PADDING_PX
            - SIDEBAR_RIGHT_PADDING_PX
            - SIDEBAR_BORDER_WIDTH_PX
    } else {
        SIDEBAR_EXPANDED_CONTENT_WIDTH_PX
    }
}

fn sidebar_pass_value(sidebar_pass: SidebarPass) -> (String, u32) {
    match sidebar_pass {
        SidebarPass::Active {
            expiration,
            expiring_soon,
        } => (expiration, if expiring_soon { DANGER } else { FOREGROUND }),
        SidebarPass::Inactive => ("N/A".to_owned(), FOREGROUND),
    }
}

impl RalgrumApp {
    fn select_nav(&mut self, nav: Nav, window: &mut Window, cx: &mut Context<Self>) {
        let already_selected = self.nav == nav;
        if already_selected && nav != Nav::Discover {
            return;
        }
        self.search.update(cx, |search, cx| {
            search.set_search_active(super::search_active_for_nav(nav));
            if should_reset_discover_home(nav, already_selected) {
                search.return_to_discover_home(window, cx);
            } else if nav != Nav::Discover {
                search.close_detail_for_main_navigation(cx);
            }
        });
        if already_selected {
            self.close_mobile_sidebar(cx);
            cx.notify();
            return;
        }
        if nav == Nav::Library {
            self.select_library(window, cx);
        } else {
            self.nav = nav;
            if nav == Nav::Downloads {
                self.downloads.update(cx, |model, cx| model.mark_read(cx));
            }
            self.persist_navigation(cx);
            cx.notify();
        }
        self.close_mobile_sidebar(cx);
    }

    pub(crate) fn open_general_settings(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        self.settings.update(cx, |settings, cx| {
            settings.begin_session(SettingsCategory::General, window, cx);
        });
        self.settings_mode = true;
        self.close_mobile_sidebar(cx);
        self.settings_category_focus[SettingsCategory::General.index()].focus(window, cx);
        cx.notify();
    }

    fn open_settings_now(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        self.open_general_settings(window, cx);
    }

    fn request_settings_save(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        self.settings
            .update(cx, |settings, cx| settings.request_save(window, cx));
        self.close_mobile_sidebar(cx);
    }

    fn activate_settings_button(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        if self.settings_mode {
            self.request_settings_save(window, cx);
        } else {
            self.open_settings_now(window, cx);
        }
    }

    fn nav_item(
        &self,
        label: &'static str,
        icon: Option<LocalIcon>,
        selected: bool,
        nav: Nav,
        compact: bool,
        badge_visual: SidebarDownloadBadgeVisual,
        cx: &mut Context<Self>,
    ) -> impl IntoElement + use<> {
        let text = if selected { FOREGROUND } else { MUTED };

        div()
            .id(label)
            .focusable()
            .tab_stop(true)
            .role(Role::Button)
            .aria_label(label)
            .aria_selected(selected)
            .h(px(36.))
            .w_full()
            .flex()
            .relative()
            .items_center()
            .px(px(SIDEBAR_NAV_HORIZONTAL_PADDING_PX))
            .group(label)
            .rounded(px(6.))
            .when(selected, |this| this.bg(rgb(BORDER)))
            .when(!selected, |this| this.bg(rgba(0x00000000)))
            .text_color(rgb(text))
            .text_size(px(13.5))
            .font_weight(if selected {
                gpui::FontWeight::SEMIBOLD
            } else {
                gpui::FontWeight::MEDIUM
            })
            .cursor_pointer()
            .hover(|style| {
                if selected {
                    style.bg(rgb(BORDER)).text_color(rgb(FOREGROUND))
                } else {
                    style
                        .bg(rgba(SIDEBAR_NAV_HOVER_BACKGROUND))
                        .text_color(rgb(FOREGROUND))
                }
            })
            .active(|style| {
                style
                    .bg(rgba(SIDEBAR_NAV_ACTIVE_BACKGROUND))
                    .text_color(rgb(FOREGROUND))
            })
            .when_some(icon, |this, icon| {
                this.child(
                    div()
                        .size(px(SIDEBAR_ICON_SLOT_SIZE_PX))
                        .flex_none()
                        .flex()
                        .items_center()
                        .justify_center()
                        .relative()
                        .child(
                            div()
                                .absolute()
                                .inset_0()
                                .flex()
                                .items_center()
                                .justify_center()
                                .group_hover(label, |style| style.invisible())
                                .child(local_icon(icon, text).size(px(SIDEBAR_ICON_GLYPH_SIZE))),
                        )
                        .child(
                            div()
                                .absolute()
                                .inset_0()
                                .flex()
                                .items_center()
                                .justify_center()
                                .invisible()
                                .group_hover(label, |style| style.visible())
                                .child(
                                    local_icon(icon, FOREGROUND).size(px(SIDEBAR_ICON_GLYPH_SIZE)),
                                ),
                        ),
                )
                .gap(px(if compact { 0. } else { 12. }))
            })
            .when(nav == Nav::Downloads, |this| {
                this.child(Self::download_badge(badge_visual))
            })
            .when(!compact, |this| this.child(label))
            .when(compact, |this| this.app_tooltip_right(label))
            .focus_visible(|style| style.border_1().border_color(rgb(PRIMARY)))
            .on_click(cx.listener(move |this, _, window, cx| {
                this.select_nav(nav, window, cx);
            }))
            .on_key_down(cx.listener(move |this, event: &KeyDownEvent, window, cx| {
                if crate::tab_keyboard::is_activation_key(event.keystroke.key.as_str()) {
                    window.prevent_default();
                    this.select_nav(nav, window, cx);
                }
            }))
    }

    fn download_badge(visual: SidebarDownloadBadgeVisual) -> impl IntoElement {
        let ((from_top, target_top), (from_right, target_right)) =
            download_badge_position_endpoints(visual.responsive);
        let count = visual.presence.count.min(99).to_string();
        div()
            .id("sidebar-download-badge")
            .absolute()
            .top(px(target_top))
            .right(px(target_right))
            .with_animation(
                ("sidebar-download-badge-position", visual.responsive.epoch),
                crate::motion::content(),
                move |this, delta| {
                    this.top(px(crate::motion::lerp(from_top, target_top, delta)))
                        .right(px(crate::motion::lerp(from_right, target_right, delta)))
                },
            )
            .child(
                div()
                    .min_w(px(13.))
                    .h(px(13.))
                    .px(px(3.))
                    .rounded_full()
                    .bg(rgb(PRIMARY))
                    .text_color(rgb(FOREGROUND))
                    .text_size(px(8.))
                    .flex()
                    .items_center()
                    .justify_center()
                    .opacity(visual.presence.target)
                    .when(visual.presence.hidden, |this| this.invisible())
                    .child(
                        div()
                            .relative()
                            .left(px(DOWNLOAD_BADGE_NUMERAL_OFFSET_PX))
                            .child(count),
                    )
                    .with_animation(
                        ("sidebar-download-badge-presence", visual.presence.epoch),
                        crate::motion::content(),
                        move |this, delta| {
                            let opacity = crate::motion::lerp(
                                visual.presence.from,
                                visual.presence.target,
                                delta,
                            );
                            if opacity == 0. {
                                this.opacity(opacity).invisible()
                            } else {
                                this.opacity(opacity).visible()
                            }
                        },
                    ),
            )
    }

    fn settings_nav_item(
        &self,
        category: SettingsCategory,
        compact: bool,
        cx: &mut Context<Self>,
    ) -> impl IntoElement + use<> {
        let selected = self.settings.read(cx).category() == category;
        let index = category.index();
        let focus = self.settings_category_focus[index].clone();
        let tab_focus = self.settings_category_focus.clone();
        let label = category.label();
        let id = match category {
            SettingsCategory::General => "settings-category-general",
            SettingsCategory::Murglar => "settings-category-murglar",
            SettingsCategory::Providers => "settings-category-providers",
            SettingsCategory::Plugins => "settings-category-plugins",
            SettingsCategory::About => "settings-category-about",
        };
        let text = if selected { FOREGROUND } else { MUTED };

        div()
            .id(id)
            .track_focus(&focus)
            .tab_stop(selected)
            .role(Role::Tab)
            .aria_label(label)
            .aria_selected(selected)
            .h(px(36.))
            .w_full()
            .flex()
            .items_center()
            .px(px(SIDEBAR_NAV_HORIZONTAL_PADDING_PX))
            .gap(px(if compact { 0. } else { 12. }))
            .rounded(px(6.))
            .when(selected, |this| this.bg(rgb(BORDER)))
            .when(!selected, |this| this.bg(rgba(0x00000000)))
            .text_color(rgb(text))
            .text_size(px(13.5))
            .font_weight(if selected {
                gpui::FontWeight::SEMIBOLD
            } else {
                gpui::FontWeight::MEDIUM
            })
            .cursor_pointer()
            .hover(|style| {
                if selected {
                    style.bg(rgb(BORDER)).text_color(rgb(FOREGROUND))
                } else {
                    style
                        .bg(rgba(SIDEBAR_NAV_HOVER_BACKGROUND))
                        .text_color(rgb(FOREGROUND))
                }
            })
            .active(|style| {
                style
                    .bg(rgba(SIDEBAR_NAV_ACTIVE_BACKGROUND))
                    .text_color(rgb(FOREGROUND))
            })
            .child(
                div()
                    .size(px(SIDEBAR_ICON_SLOT_SIZE_PX))
                    .flex_none()
                    .flex()
                    .items_center()
                    .justify_center()
                    .child(
                        local_icon(
                            category.icon(),
                            if selected {
                                category.active_icon_color()
                            } else {
                                text
                            },
                        )
                        .size(px(SIDEBAR_ICON_GLYPH_SIZE)),
                    ),
            )
            .when(!compact, |this| this.child(label))
            .when(compact, |this| this.app_tooltip_right(label))
            .focus_visible(|style| style.border_1().border_color(rgb(PRIMARY)))
            .on_click(cx.listener(move |this, _, window, cx| {
                this.settings.update(cx, |settings, cx| {
                    settings.select_category(category, window, cx);
                });
                this.close_mobile_sidebar(cx);
            }))
            .on_key_down(cx.listener(move |this, event: &KeyDownEvent, window, cx| {
                if event.keystroke.key == "escape" && this.settings_mode {
                    // Escape leaves the settings page the same way the back
                    // button does, with the drafts persisted on the way out.
                    window.prevent_default();
                    cx.stop_propagation();
                    this.request_settings_save(window, cx);
                    return;
                }
                if crate::tab_keyboard::is_activation_key(event.keystroke.key.as_str()) {
                    window.prevent_default();
                    this.settings.update(cx, |settings, cx| {
                        settings.select_category(category, window, cx);
                    });
                    this.close_mobile_sidebar(cx);
                    return;
                }
                let Some(next) = crate::tab_keyboard::next_tab_index(
                    event.keystroke.key.as_str(),
                    index,
                    tab_focus.len(),
                ) else {
                    return;
                };
                window.prevent_default();
                let next_category = SettingsCategory::ALL[next];
                this.settings.update(cx, |settings, cx| {
                    settings.select_category(next_category, window, cx);
                });
                tab_focus[next].focus(window, cx);
            }))
    }

    fn full_settings_button(&self, cx: &mut Context<Self>) -> impl IntoElement + use<> {
        let label = if self.settings_mode {
            "Back"
        } else {
            "Open settings"
        };
        let icon = if self.settings_mode {
            LocalIcon::ArrowLeft
        } else {
            LocalIcon::Settings
        };
        div()
            .id("shell-settings-control")
            .focusable()
            .tab_stop(true)
            .role(Role::Button)
            .aria_label(label)
            .size(px(FULL_SETTINGS_BUTTON_SIZE))
            .flex_none()
            .flex()
            .items_center()
            .justify_center()
            .group("full-settings")
            .rounded(px(6.))
            .border_1()
            .border_color(rgb(BORDER))
            .bg(rgb(SURFACE))
            .text_color(rgb(MUTED))
            .cursor_pointer()
            .hover(|style| {
                style
                    .bg(rgb(BORDER))
                    .border_color(rgb(BORDER))
                    .text_color(rgb(FOREGROUND))
            })
            .child(
                div()
                    .size(px(SETTINGS_ICON_SIZE))
                    .relative()
                    .child(
                        div()
                            .absolute()
                            .inset_0()
                            .group_hover("full-settings", |style| style.invisible())
                            .child(local_icon(icon, MUTED).size_full()),
                    )
                    .child(
                        div()
                            .absolute()
                            .inset_0()
                            .invisible()
                            .group_hover("full-settings", |style| style.visible())
                            .child(local_icon(icon, FOREGROUND).size_full()),
                    ),
            )
            .app_tooltip_right(label)
            .focus_visible(|style| style.border_1().border_color(rgb(PRIMARY)))
            .on_click(cx.listener(|this, _, window, cx| {
                this.activate_settings_button(window, cx);
            }))
            .on_key_down(cx.listener(|this, event: &KeyDownEvent, window, cx| {
                if event.keystroke.key == "escape" && this.settings_mode {
                    window.prevent_default();
                    cx.stop_propagation();
                    this.request_settings_save(window, cx);
                    return;
                }
                if crate::tab_keyboard::is_activation_key(event.keystroke.key.as_str()) {
                    window.prevent_default();
                    this.activate_settings_button(window, cx);
                }
            }))
    }

    fn compact_settings_button(
        &self,
        interactive: bool,
        cx: &mut Context<Self>,
    ) -> impl IntoElement + use<> {
        let label = if self.settings_mode {
            "Back"
        } else {
            "Settings"
        };
        let icon = if self.settings_mode {
            LocalIcon::ArrowLeft
        } else {
            LocalIcon::Settings
        };
        div()
            .id(SIDEBAR_COMPACT_SETTINGS_ID)
            .focusable()
            .tab_stop(true)
            .role(Role::Button)
            .aria_label(label)
            .size(px(COMPACT_SETTINGS_BUTTON_SIZE))
            .flex_none()
            .flex()
            .items_center()
            .justify_center()
            .group("compact-settings")
            .rounded(px(COMPACT_SETTINGS_BUTTON_RADIUS_PX))
            .border_1()
            .border_color(rgb(BORDER))
            .bg(rgb(SURFACE))
            .text_color(rgb(MUTED))
            .cursor_pointer()
            .hover(|style| {
                style
                    .bg(rgb(BORDER))
                    .border_color(rgb(BORDER))
                    .text_color(rgb(FOREGROUND))
            })
            .child(
                div()
                    .size(px(18.))
                    .flex()
                    .items_center()
                    .justify_center()
                    .relative()
                    .child(
                        div()
                            .absolute()
                            .inset_0()
                            .flex()
                            .items_center()
                            .justify_center()
                            .group_hover("compact-settings", |style| style.invisible())
                            .child(local_icon(icon, MUTED).size(px(COMPACT_SETTINGS_ICON_SIZE))),
                    )
                    .child(
                        div()
                            .absolute()
                            .inset_0()
                            .flex()
                            .items_center()
                            .justify_center()
                            .invisible()
                            .group_hover("compact-settings", |style| style.visible())
                            .child(
                                local_icon(icon, FOREGROUND).size(px(COMPACT_SETTINGS_ICON_SIZE)),
                            ),
                    ),
            )
            .app_tooltip_right(label)
            .focus_visible(|style| style.border_1().border_color(rgb(PRIMARY)))
            .tab_stop(interactive)
            .when(!interactive, |this| this.invisible())
            .on_click(cx.listener(|this, _, window, cx| {
                this.activate_settings_button(window, cx);
            }))
            .on_key_down(cx.listener(|this, event: &KeyDownEvent, window, cx| {
                if event.keystroke.key == "escape" && this.settings_mode {
                    window.prevent_default();
                    cx.stop_propagation();
                    this.request_settings_save(window, cx);
                    return;
                }
                if crate::tab_keyboard::is_activation_key(event.keystroke.key.as_str()) {
                    window.prevent_default();
                    this.activate_settings_button(window, cx);
                }
            }))
    }

    fn logout_all_button(
        &self,
        compact: bool,
        interactive: bool,
        cx: &mut Context<Self>,
    ) -> impl IntoElement + use<> {
        let options = if compact {
            DangerSecondaryButtonOptions::SIDEBAR_COMPACT
        } else {
            DangerSecondaryButtonOptions::SIDEBAR
        };
        let id = if compact {
            SIDEBAR_COMPACT_LOGOUT_ID
        } else {
            SIDEBAR_EXPANDED_LOGOUT_ID
        };
        let button = danger_secondary_button(
            id,
            "Log Out All",
            false,
            options,
            cx.listener(|this, _, window, cx| {
                this.settings.update(cx, |settings, cx| {
                    settings.confirm_logout_all(window, cx);
                });
            }),
        );
        button
            .when(compact, |this| this.app_tooltip_right(LOGOUT_ALL_TOOLTIP))
            .tab_stop(interactive)
            .when(!interactive, |this| this.invisible())
    }

    fn settings_transfer_button(
        &self,
        compact: bool,
        interactive: bool,
        cx: &mut Context<Self>,
    ) -> impl IntoElement + use<> {
        let options = if compact {
            DangerSecondaryButtonOptions::SIDEBAR_COMPACT
        } else {
            DangerSecondaryButtonOptions::SIDEBAR
        };
        let id = if compact {
            SIDEBAR_COMPACT_TRANSFER_ID
        } else {
            SIDEBAR_EXPANDED_TRANSFER_ID
        };
        let button = neutral_secondary_button(
            id,
            LocalIcon::FileImport,
            "Import / Export",
            options,
            SETTINGS_SECONDARY_SMALL_ICON_SIZE,
            cx.listener(|this, _, window, cx| {
                this.activate_settings_transfer(window, cx);
            }),
        );
        button
            .when(compact, |this| {
                this.app_tooltip_right(SETTINGS_TRANSFER_TOOLTIP)
            })
            .tab_stop(interactive)
            .when(!interactive, |this| this.invisible())
            .on_key_down(cx.listener(|this, event: &KeyDownEvent, window, cx| {
                if crate::tab_keyboard::is_activation_key(event.keystroke.key.as_str()) {
                    window.prevent_default();
                    this.activate_settings_transfer(window, cx);
                }
            }))
    }

    fn activate_settings_transfer(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        self.settings.update(cx, |settings, cx| {
            settings.open_settings_transfer(window, cx);
        });
    }
}

fn should_reset_discover_home(nav: Nav, already_selected: bool) -> bool {
    nav == Nav::Discover && already_selected
}

pub(super) fn render_sidebar(
    app: &RalgrumApp,
    window: &Window,
    cx: &mut Context<RalgrumApp>,
    bottom_visual: SidebarBottomVisual,
    badge_visual: SidebarDownloadBadgeVisual,
) -> impl IntoElement + use<> {
    let metrics = crate::music_ui::shell_metrics(f32::from(window.viewport_size().width));
    let discover = app.nav_item(
        "Discover",
        Some(LocalIcon::Compass),
        app.nav == Nav::Discover,
        Nav::Discover,
        metrics.compact_desktop,
        badge_visual,
        cx,
    );
    let library = app.nav_item(
        "Library",
        Some(LocalIcon::Layers),
        app.nav == Nav::Library,
        Nav::Library,
        metrics.compact_desktop,
        badge_visual,
        cx,
    );
    let downloads = app.nav_item(
        "Downloads",
        Some(LocalIcon::Download),
        app.nav == Nav::Downloads,
        Nav::Downloads,
        metrics.compact_desktop,
        badge_visual,
        cx,
    );
    let cache = app.nav_item(
        "Cache",
        Some(CACHE_ICON),
        app.nav == Nav::Cache,
        Nav::Cache,
        metrics.compact_desktop,
        badge_visual,
        cx,
    );
    let settings_categories: Vec<_> = SettingsCategory::ALL
        .iter()
        .copied()
        .map(|category| app.settings_nav_item(category, metrics.compact_desktop, cx))
        .collect();
    let (sidebar_username, sidebar_premium, sidebar_pass) = {
        let account = app.account.read(cx);
        (
            account.sidebar_username().to_owned(),
            account.sidebar_premium(),
            account.sidebar_pass(),
        )
    };
    let show_update = app.updater.read(cx).has_offer();

    div()
        .w_full()
        .min_w_0()
        .h_full()
        .flex()
        .flex_col()
        .pl(px(SIDEBAR_HORIZONTAL_PADDING_PX))
        .pr(px(SIDEBAR_RIGHT_PADDING_PX))
        .py(px(20.))
        .gap(px(4.))
        .bg(rgb(CHROME))
        .border_r_1()
        .border_color(rgb(BORDER))
        .child(
            div()
                .h(px(38.))
                .mb(px(22.))
                .flex()
                .items_center()
                .justify_between()
                .child(
                    div()
                        .flex()
                        .items_center()
                        .gap(px(10.))
                        .child(
                            div()
                                .w(px(34.))
                                .h(px(34.))
                                .flex_none()
                                .flex()
                                .items_center()
                                .justify_center()
                                .rounded(px(6.))
                                .bg(rgb(PRIMARY))
                                .child(
                                    local_icon(LocalIcon::Music, FOREGROUND)
                                        .size(px(SIDEBAR_ICON_GLYPH_SIZE)),
                                ),
                        )
                        .when(!metrics.compact_desktop, |this| {
                            this.child(
                                div()
                                    .text_size(px(20.))
                                    .font_weight(gpui::FontWeight::BOLD)
                                    .child("ralgruM"),
                            )
                        }),
                )
                .when(!metrics.compact_desktop, |this| {
                    this.child(app.full_settings_button(cx))
                }),
        )
        .when(!app.settings_mode, |this| {
            this.child(discover)
                .child(library)
                .child(downloads)
                .child(cache)
        })
        .when(app.settings_mode, |this| {
            this.child(
                div()
                    .id("settings-category-tabs")
                    .role(Role::TabList)
                    .aria_label("Settings categories")
                    .w_full()
                    .flex()
                    .flex_col()
                    .gap(px(SIDEBAR_SETTINGS_CATEGORY_GAP_PX))
                    .children(settings_categories),
            )
        })
        .child(div().flex_1())
        .child(sidebar_bottom(
            app,
            sidebar_username,
            sidebar_premium,
            sidebar_pass,
            metrics.narrow_content,
            metrics.compact_desktop,
            show_update,
            bottom_visual,
            cx,
        ))
}

fn sidebar_bottom(
    app: &RalgrumApp,
    sidebar_username: String,
    sidebar_premium: &'static str,
    sidebar_pass: SidebarPass,
    narrow_content: bool,
    compact: bool,
    show_update: bool,
    bottom_visual: SidebarBottomVisual,
    cx: &mut Context<RalgrumApp>,
) -> impl IntoElement + use<> {
    let expanded_width = sidebar_expanded_content_width(narrow_content);
    let initial_opacities = sidebar_bottom_opacities_at(bottom_visual, 0.0);
    let target_opacities = sidebar_bottom_opacities_at(bottom_visual, 1.0);

    let account_layer = div()
        .id(SIDEBAR_BOTTOM_EXPANDED_ACCOUNT_LAYER_ID)
        .absolute()
        .left_0()
        .bottom_0()
        .w(px(expanded_width))
        .opacity(initial_opacities.account)
        .when(initial_opacities.account == 0.0, |this| this.invisible())
        .child(
            div()
                .w_full()
                .h(px(SIDEBAR_ACCOUNT_CARD_HEIGHT_PX))
                .p(px(12.))
                .rounded(px(6.))
                .border_1()
                .border_color(rgb(BORDER))
                .bg(rgb(SURFACE))
                .flex()
                .flex_col()
                .justify_center()
                .gap(px(8.))
                .text_size(px(11.5))
                .text_color(rgb(MUTED))
                .child(
                    div()
                        .flex()
                        .items_center()
                        .justify_center()
                        .gap(px(7.))
                        .child(local_icon(LocalIcon::User, MUTED).size_3())
                        .child(
                            div()
                                .min_w_0()
                                .truncate()
                                .text_size(px(17.))
                                .font_weight(gpui::FontWeight::SEMIBOLD)
                                .text_color(rgb(FOREGROUND))
                                .child(sidebar_username),
                        ),
                )
                .child(
                    div()
                        .w_full()
                        .flex()
                        .items_center()
                        .justify_between()
                        .child(
                            div()
                                .flex()
                                .items_center()
                                .gap(px(4.))
                                .child("Premium:")
                                .child(
                                    div()
                                        .relative()
                                        .top(px(ACCOUNT_STATUS_ICON_OFFSET_PX))
                                        .child(if sidebar_premium == "Yes" {
                                            local_icon(LocalIcon::Check, 0x22c55e).size(px(10.))
                                        } else {
                                            local_icon(LocalIcon::X, FOREGROUND).size(px(10.))
                                        }),
                                ),
                        )
                        .child(
                            div()
                                .flex()
                                .items_center()
                                .gap(px(4.))
                                .child("Pass:")
                                .child({
                                    let (value, color) = sidebar_pass_value(sidebar_pass);
                                    div()
                                        .font_weight(gpui::FontWeight::MEDIUM)
                                        .text_color(rgb(color))
                                        .child(value)
                                }),
                        ),
                ),
        )
        .with_animation(
            (
                SIDEBAR_BOTTOM_EXPANDED_ACCOUNT_LAYER_ID,
                bottom_visual.epoch,
            ),
            crate::motion::panel(),
            move |this, delta| {
                let opacity = sidebar_bottom_opacities_at(bottom_visual, delta).account;
                let this = this.opacity(opacity);
                if opacity == 0.0 {
                    this.invisible()
                } else {
                    this.visible()
                }
            },
        );

    let expanded_logout_layer = div()
        .id(SIDEBAR_BOTTOM_EXPANDED_LOGOUT_LAYER_ID)
        .absolute()
        .left_0()
        .bottom_0()
        .w(px(expanded_width))
        .opacity(initial_opacities.expanded_logout)
        .when(initial_opacities.expanded_logout == 0.0, |this| {
            this.invisible()
        })
        .child(app.logout_all_button(false, target_opacities.expanded_logout > 0.0, cx))
        .with_animation(
            (SIDEBAR_BOTTOM_EXPANDED_LOGOUT_LAYER_ID, bottom_visual.epoch),
            crate::motion::panel(),
            move |this, delta| {
                let opacity = sidebar_bottom_opacities_at(bottom_visual, delta).expanded_logout;
                let this = this.opacity(opacity);
                if opacity == 0.0 {
                    this.invisible()
                } else {
                    this.visible()
                }
            },
        );

    let expanded_transfer_layer = div()
        .id(SIDEBAR_BOTTOM_EXPANDED_TRANSFER_LAYER_ID)
        .absolute()
        .left_0()
        .bottom(px(SIDEBAR_EXPANDED_TRANSFER_BOTTOM_PX))
        .w(px(expanded_width))
        .opacity(initial_opacities.expanded_transfer)
        .when(initial_opacities.expanded_transfer == 0.0, |this| {
            this.invisible()
        })
        .child(app.settings_transfer_button(false, target_opacities.expanded_transfer > 0.0, cx))
        .with_animation(
            (
                SIDEBAR_BOTTOM_EXPANDED_TRANSFER_LAYER_ID,
                bottom_visual.epoch,
            ),
            crate::motion::panel(),
            move |this, delta| {
                let opacity = sidebar_bottom_opacities_at(bottom_visual, delta).expanded_transfer;
                let this = this.opacity(opacity);
                if opacity == 0.0 {
                    this.invisible()
                } else {
                    this.visible()
                }
            },
        );

    let compact_logout_layer = div()
        .id(SIDEBAR_BOTTOM_COMPACT_LOGOUT_LAYER_ID)
        .absolute()
        .bottom(px(SIDEBAR_COMPACT_LOGOUT_BOTTOM_PX))
        .left_0()
        .size(px(COMPACT_SETTINGS_BUTTON_SIZE))
        .opacity(initial_opacities.compact_logout)
        .when(initial_opacities.compact_logout == 0.0, |this| {
            this.invisible()
        })
        .child(app.logout_all_button(true, target_opacities.compact_logout > 0.0, cx))
        .with_animation(
            (SIDEBAR_BOTTOM_COMPACT_LOGOUT_LAYER_ID, bottom_visual.epoch),
            crate::motion::panel(),
            move |this, delta| {
                let opacity = sidebar_bottom_opacities_at(bottom_visual, delta).compact_logout;
                let this = this.opacity(opacity);
                if opacity == 0.0 {
                    this.invisible()
                } else {
                    this.visible()
                }
            },
        );

    let compact_transfer_layer = div()
        .id(SIDEBAR_BOTTOM_COMPACT_TRANSFER_LAYER_ID)
        .absolute()
        .bottom(px(SIDEBAR_COMPACT_TRANSFER_BOTTOM_PX))
        .left_0()
        .size(px(COMPACT_SETTINGS_BUTTON_SIZE))
        .opacity(initial_opacities.compact_transfer)
        .when(initial_opacities.compact_transfer == 0.0, |this| {
            this.invisible()
        })
        .child(app.settings_transfer_button(true, target_opacities.compact_transfer > 0.0, cx))
        .with_animation(
            (
                SIDEBAR_BOTTOM_COMPACT_TRANSFER_LAYER_ID,
                bottom_visual.epoch,
            ),
            crate::motion::panel(),
            move |this, delta| {
                let opacity = sidebar_bottom_opacities_at(bottom_visual, delta).compact_transfer;
                let this = this.opacity(opacity);
                if opacity == 0.0 {
                    this.invisible()
                } else {
                    this.visible()
                }
            },
        );

    let compact_settings_layer = div()
        .id(SIDEBAR_BOTTOM_COMPACT_SETTINGS_LAYER_ID)
        .absolute()
        .left_0()
        .bottom_0()
        .size(px(COMPACT_SETTINGS_BUTTON_SIZE))
        .opacity(initial_opacities.compact_settings)
        .when(initial_opacities.compact_settings == 0.0, |this| {
            this.invisible()
        })
        .child(app.compact_settings_button(target_opacities.compact_settings > 0.0, cx))
        .with_animation(
            (
                SIDEBAR_BOTTOM_COMPACT_SETTINGS_LAYER_ID,
                bottom_visual.epoch,
            ),
            crate::motion::panel(),
            move |this, delta| {
                let opacity = sidebar_bottom_opacities_at(bottom_visual, delta).compact_settings;
                let this = this.opacity(opacity);
                if opacity == 0.0 {
                    this.invisible()
                } else {
                    this.visible()
                }
            },
        );

    div()
        .id("sidebar-bottom-host")
        .relative()
        .w_full()
        .h(px(sidebar_bottom_host_height(compact, show_update)))
        .flex_none()
        .overflow_x_hidden()
        .child(account_layer)
        .child(expanded_logout_layer)
        .child(expanded_transfer_layer)
        .child(compact_logout_layer)
        .child(compact_transfer_layer)
        .child(compact_settings_layer)
        .when(show_update, |this| {
            this.child(sidebar_update::render_update_layers(
                app,
                compact,
                expanded_width,
                bottom_visual,
                cx,
            ))
        })
}

#[cfg(test)]
mod tests {
    use super::{
        ACCOUNT_STATUS_ICON_OFFSET_PX, COMPACT_SETTINGS_BUTTON_RADIUS_PX,
        COMPACT_SETTINGS_BUTTON_SIZE, COMPACT_SETTINGS_ICON_SIZE, FULL_SETTINGS_BUTTON_SIZE,
        LOGOUT_ALL_TOOLTIP, SETTINGS_ICON_SIZE, SIDEBAR_ACCOUNT_CARD_HEIGHT_PX,
        SIDEBAR_BORDER_WIDTH_PX, SIDEBAR_BOTTOM_ACCOUNT_HEADROOM_PX, SIDEBAR_BOTTOM_HOST_HEIGHT_PX,
        SIDEBAR_COMPACT_CONTENT_WIDTH_PX, SIDEBAR_COMPACT_CONTROL_GAP_PX,
        SIDEBAR_COMPACT_LOGOUT_BOTTOM_PX, SIDEBAR_COMPACT_LOGOUT_ID,
        SIDEBAR_COMPACT_SETTINGS_ACTION_GROUP_GAP_PX, SIDEBAR_COMPACT_SETTINGS_ID,
        SIDEBAR_COMPACT_TRANSFER_BOTTOM_PX, SIDEBAR_COMPACT_TRANSFER_ID, SIDEBAR_COMPACT_WIDTH_PX,
        SIDEBAR_DESKTOP_WIDTH_PX, SIDEBAR_EXPANDED_CONTENT_WIDTH_PX, SIDEBAR_EXPANDED_LOGOUT_ID,
        SIDEBAR_EXPANDED_TRANSFER_BOTTOM_PX, SIDEBAR_EXPANDED_TRANSFER_ID,
        SIDEBAR_HORIZONTAL_PADDING_PX, SIDEBAR_NAV_ACTIVE_BACKGROUND, SIDEBAR_NAV_HOVER_BACKGROUND,
        SIDEBAR_RIGHT_PADDING_PX, SIDEBAR_SETTINGS_CATEGORY_GAP_PX, sidebar_bottom_host_height,
        sidebar_bottom_opacities, sidebar_expanded_content_width, sidebar_pass_value,
        sidebar_update_opacities_at, update_badge_bottom, update_badge_bottom_at,
    };
    use crate::{
        settings::SidebarPass,
        theme::{DANGER, FOREGROUND},
    };

    #[test]
    fn sidebar_reference_colors_and_sizes() {
        assert_eq!(SIDEBAR_NAV_HOVER_BACKGROUND, 0xffffff0a);
        assert_eq!(SIDEBAR_NAV_ACTIVE_BACKGROUND, 0xffffff12);
        assert_eq!(FULL_SETTINGS_BUTTON_SIZE, 30.);
        assert_eq!(COMPACT_SETTINGS_BUTTON_SIZE, 36.);
        assert_eq!(COMPACT_SETTINGS_BUTTON_RADIUS_PX, 6.);
        assert_eq!(COMPACT_SETTINGS_ICON_SIZE, 16.);
        assert_eq!(SETTINGS_ICON_SIZE, 14.);
        assert_eq!(ACCOUNT_STATUS_ICON_OFFSET_PX, 1.);
    }

    #[test]
    fn sidebar_bottom_opacities_cover_all_modes_and_the_midpoint() {
        assert_eq!(
            sidebar_bottom_opacities(0.0, 0.0),
            super::SidebarBottomOpacities {
                account: 1.0,
                expanded_logout: 0.0,
                expanded_transfer: 0.0,
                compact_settings: 0.0,
                compact_logout: 0.0,
                compact_transfer: 0.0,
            }
        );
        assert_eq!(
            sidebar_bottom_opacities(0.0, 1.0),
            super::SidebarBottomOpacities {
                account: 0.0,
                expanded_logout: 1.0,
                expanded_transfer: 1.0,
                compact_settings: 0.0,
                compact_logout: 0.0,
                compact_transfer: 0.0,
            }
        );
        assert_eq!(
            sidebar_bottom_opacities(1.0, 0.0),
            super::SidebarBottomOpacities {
                account: 0.0,
                expanded_logout: 0.0,
                expanded_transfer: 0.0,
                compact_settings: 1.0,
                compact_logout: 0.0,
                compact_transfer: 0.0,
            }
        );
        assert_eq!(
            sidebar_bottom_opacities(1.0, 1.0),
            super::SidebarBottomOpacities {
                account: 0.0,
                expanded_logout: 0.0,
                expanded_transfer: 0.0,
                compact_settings: 1.0,
                compact_logout: 1.0,
                compact_transfer: 1.0,
            }
        );
        assert_eq!(
            sidebar_bottom_opacities(0.5, 0.5),
            super::SidebarBottomOpacities {
                account: 0.25,
                expanded_logout: 0.25,
                expanded_transfer: 0.25,
                compact_settings: 0.5,
                compact_logout: 0.25,
                compact_transfer: 0.25,
            }
        );
    }

    #[test]
    fn sidebar_content_widths_match_desktop_compact_and_mobile_endpoints() {
        assert_eq!(SIDEBAR_BOTTOM_ACCOUNT_HEADROOM_PX, 8.);
        assert_eq!(SIDEBAR_SETTINGS_CATEGORY_GAP_PX, 4.);
        assert_eq!(SIDEBAR_COMPACT_CONTROL_GAP_PX, 4.);
        assert_eq!(
            SIDEBAR_COMPACT_CONTROL_GAP_PX,
            SIDEBAR_SETTINGS_CATEGORY_GAP_PX
        );
        assert_eq!(SIDEBAR_COMPACT_SETTINGS_ACTION_GROUP_GAP_PX, 8.);
        assert_eq!(SIDEBAR_COMPACT_LOGOUT_BOTTOM_PX, 44.);
        assert_eq!(
            SIDEBAR_COMPACT_TRANSFER_BOTTOM_PX,
            SIDEBAR_COMPACT_LOGOUT_BOTTOM_PX
                + COMPACT_SETTINGS_BUTTON_SIZE
                + SIDEBAR_COMPACT_CONTROL_GAP_PX
        );
        assert_eq!(SIDEBAR_COMPACT_TRANSFER_BOTTOM_PX, 84.);
        assert_eq!(SIDEBAR_EXPANDED_TRANSFER_BOTTOM_PX, 38.);
        assert_eq!(
            SIDEBAR_BOTTOM_HOST_HEIGHT_PX,
            3. * COMPACT_SETTINGS_BUTTON_SIZE
                + SIDEBAR_COMPACT_SETTINGS_ACTION_GROUP_GAP_PX
                + SIDEBAR_COMPACT_CONTROL_GAP_PX
                + SIDEBAR_BOTTOM_ACCOUNT_HEADROOM_PX
        );
        assert_eq!(SIDEBAR_BOTTOM_HOST_HEIGHT_PX, 128.);
        assert_eq!(SIDEBAR_DESKTOP_WIDTH_PX, 240.);
        assert_eq!(SIDEBAR_COMPACT_WIDTH_PX, 68.);
        assert_eq!(SIDEBAR_HORIZONTAL_PADDING_PX, 16.);
        assert_eq!(SIDEBAR_RIGHT_PADDING_PX, 15.);
        assert_eq!(SIDEBAR_BORDER_WIDTH_PX, 1.);
        assert_eq!(SIDEBAR_EXPANDED_CONTENT_WIDTH_PX, 240. - 16. - 15. - 1.);
        assert_eq!(SIDEBAR_EXPANDED_CONTENT_WIDTH_PX, 208.);
        assert_eq!(SIDEBAR_COMPACT_CONTENT_WIDTH_PX, 68. - 16. - 15. - 1.);
        assert_eq!(SIDEBAR_COMPACT_CONTENT_WIDTH_PX, 36.);
        assert_eq!(
            sidebar_expanded_content_width(false),
            SIDEBAR_EXPANDED_CONTENT_WIDTH_PX
        );
        assert_eq!(
            sidebar_expanded_content_width(true),
            crate::music_ui::MOBILE_DRAWER_WIDTH
                - SIDEBAR_HORIZONTAL_PADDING_PX
                - SIDEBAR_RIGHT_PADDING_PX
                - SIDEBAR_BORDER_WIDTH_PX
        );
        assert_eq!(
            sidebar_expanded_content_width(true),
            crate::music_ui::MOBILE_DRAWER_WIDTH - 32.
        );
    }

    #[test]
    fn updater_sits_above_the_visible_bottom_controls_in_every_sidebar_mode() {
        assert_eq!(update_badge_bottom(false, false), 88.);
        assert_eq!(update_badge_bottom(false, true), 76.);
        assert_eq!(update_badge_bottom(true, false), 44.);
        assert_eq!(update_badge_bottom(true, true), 124.);
        assert_eq!(sidebar_bottom_host_height(false, true), 128.);
        assert_eq!(sidebar_bottom_host_height(true, false), 128.);
        assert_eq!(sidebar_bottom_host_height(true, true), 168.);
        assert_eq!(
            update_badge_bottom(false, false) - SIDEBAR_ACCOUNT_CARD_HEIGHT_PX,
            8.
        );
        assert!(sidebar_bottom_host_height(false, true) >= update_badge_bottom(false, false) + 36.);
        assert!(sidebar_bottom_host_height(true, true) >= update_badge_bottom(true, true) + 36.);
    }

    #[test]
    fn updater_follows_settings_motion_in_expanded_and_compact_sidebar() {
        let expanded = super::SidebarBottomVisual {
            compact_from: 0.0,
            compact_target: 0.0,
            settings_from: 0.0,
            settings_target: 1.0,
            epoch: 1,
        };
        assert_eq!(update_badge_bottom_at(false, expanded, 0.0), 88.);
        assert_eq!(update_badge_bottom_at(false, expanded, 0.5), 82.);
        assert_eq!(update_badge_bottom_at(false, expanded, 1.0), 76.);

        let compact = super::SidebarBottomVisual {
            compact_from: 1.0,
            compact_target: 1.0,
            ..expanded
        };
        assert_eq!(sidebar_update_opacities_at(compact, 0.0), (0.0, 1.0, 0.0));
        assert_eq!(sidebar_update_opacities_at(compact, 0.5), (0.0, 0.5, 0.5));
        assert_eq!(sidebar_update_opacities_at(compact, 1.0), (0.0, 0.0, 1.0));
        assert_eq!(update_badge_bottom_at(true, compact, 0.0), 44.);
        assert_eq!(update_badge_bottom_at(true, compact, 1.0), 124.);
    }

    #[test]
    fn sidebar_bottom_action_ids_keep_compact_and_expanded_logout_distinct() {
        assert_ne!(SIDEBAR_COMPACT_LOGOUT_ID, SIDEBAR_EXPANDED_LOGOUT_ID);
        assert_eq!(
            SIDEBAR_COMPACT_SETTINGS_ID,
            "shell-settings-control-compact"
        );
        assert_eq!(COMPACT_SETTINGS_BUTTON_SIZE, 36.);
        assert_eq!(
            SIDEBAR_COMPACT_TRANSFER_ID,
            "sidebar-settings-transfer-compact"
        );
        assert_eq!(
            SIDEBAR_EXPANDED_TRANSFER_ID,
            "sidebar-settings-transfer-expanded"
        );
    }

    #[test]
    fn logout_all_tooltip_places_the_service_list_on_its_own_line() {
        assert_eq!(
            LOGOUT_ALL_TOOLTIP,
            "Logs out of all active services\n(Murglar, SoundCloud, Deezer)"
        );
    }

    #[test]
    fn inactive_pass_uses_the_logged_out_value_style() {
        assert_eq!(
            sidebar_pass_value(SidebarPass::Inactive),
            ("N/A".to_owned(), FOREGROUND)
        );
    }

    #[test]
    fn active_pass_preserves_its_date_and_warning_color() {
        assert_eq!(
            sidebar_pass_value(SidebarPass::Active {
                expiration: "31/08/2026".to_owned(),
                expiring_soon: false,
            }),
            ("31/08/2026".to_owned(), FOREGROUND)
        );
        assert_eq!(
            sidebar_pass_value(SidebarPass::Active {
                expiration: "31/08/2026".to_owned(),
                expiring_soon: true,
            }),
            ("31/08/2026".to_owned(), DANGER)
        );
    }
}
