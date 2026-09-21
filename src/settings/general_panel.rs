use std::time::Instant;

use crate::{
    assets::LocalIcon,
    context_menu::{CONTEXT_MENU_BORDER, CONTEXT_MENU_FOREGROUND, CONTEXT_MENU_SURFACE},
    theme::{BACKGROUND, BORDER, FOREGROUND, MUTED, PRIMARY},
};
use gpui::{
    AnimationExt as _, AnyElement, Context, Div, Entity, FontWeight, IntoElement, SharedString,
    div, prelude::*, px, relative, rgb, rgba,
};
use gpui_component::select::{Select, SelectState};

use super::{
    CacheMeterVisual, SettingsView,
    action_button::{
        secondary_action_button, secondary_action_button_disabled,
        secondary_action_button_small_icon,
    },
    service_panel::{panel_heading, settings_card, settings_card_heading},
    settings_switch,
};

impl SettingsView {
    pub(super) fn render_general(&mut self, cx: &mut Context<Self>) -> AnyElement {
        let behavior = self.render_behavior_section(cx);
        let playback = self.render_playback_settings(cx);
        let cache_and_storage = self.render_cache_and_storage_section(cx);
        let heading = panel_heading(
            "General",
            "Choose how ralgruM starts, plays, and uses local storage.",
        )
        .into_any_element();

        div()
            .flex()
            .flex_col()
            .gap(px(14.))
            .children([heading, behavior, playback, cache_and_storage])
            .into_any_element()
    }

    fn render_cache_and_storage_section(&mut self, cx: &mut Context<Self>) -> AnyElement {
        let downloads_dir = self.draft.effective_downloads_dir();
        let downloads_dir_label = downloads_dir.to_string_lossy().into_owned();
        let used_fraction = cache_used_fraction(self.cache_overview.as_ref());
        let cache_meter_visual =
            self.cache_meter_motion
                .prepare(used_fraction, Instant::now(), cx.reduce_motion());

        settings_card()
            .child(settings_card_heading(
                LocalIcon::HardDrive,
                "Cache & storage",
            ))
            .child(control_list(vec![
                control_row(
                    "Cache tracks in the background",
                    "Finish caching a track after playback begins.",
                    settings_switch(
                        "background-audio-cache",
                        self.draft.background_audio_cache,
                        cx.listener(|this, checked: &bool, _, cx| {
                            this.update_draft_and_persist(
                                |draft| draft.background_audio_cache = *checked,
                                cx,
                            );
                        }),
                    )
                    .into_any_element(),
                ),
                control_row(
                    "Audio cache size",
                    "Older cached audio is removed automatically.",
                    settings_select(&self.cache_limit_select, 100.).into_any_element(),
                ),
                div()
                    .w_full()
                    .min_h(px(55.))
                    .py(px(8.))
                    .flex()
                    .items_center()
                    .justify_between()
                    .gap(px(18.))
                    .child(
                        div()
                            .flex_1()
                            .min_w_0()
                            .flex()
                            .flex_col()
                            .gap(px(3.))
                            .child(
                                div()
                                    .text_size(px(12.5))
                                    .line_height(px(15.625))
                                    .font_weight(FontWeight::MEDIUM)
                                    .text_color(rgb(FOREGROUND))
                                    .child("Downloads folder"),
                            )
                            .child(
                                div()
                                    .text_size(px(11.))
                                    .text_color(rgb(MUTED))
                                    .truncate()
                                    .child(downloads_dir_label),
                            ),
                    )
                    .child(
                        div()
                            .flex_none()
                            .flex()
                            .flex_wrap()
                            .gap(px(8.))
                            .child(secondary_action_button_disabled(
                                "change-downloads-folder",
                                Some(LocalIcon::FolderOpen),
                                "Change folder",
                                "Choose a different downloads folder",
                                self.downloads_folder_picker_pending,
                                cx.listener({
                                    let downloads_dir = downloads_dir.clone();
                                    move |this, _, window, cx| {
                                        if this.downloads_folder_picker_pending {
                                            return;
                                        }
                                        this.downloads_folder_picker_pending = true;
                                        cx.notify();
                                        // The native folder picker must not
                                        // run on the UI thread: its modal
                                        // message pump re-enters gpui while
                                        // the App is still borrowed and
                                        // panics. The async dialog runs the
                                        // picker on its own thread instead.
                                        let downloads_dir = downloads_dir.clone();
                                        cx.spawn_in(window, async move |this, cx| {
                                            let folder = rfd::AsyncFileDialog::new()
                                                .set_directory(downloads_dir.as_path())
                                                .pick_folder()
                                                .await;
                                            this.update_in(cx, |this, _, cx| {
                                                this.downloads_folder_picker_pending = false;
                                                if let Some(folder) = folder {
                                                    let path = folder.path().to_path_buf();
                                                    this.update_draft_and_persist(
                                                        |draft| draft.downloads_dir = Some(path),
                                                        cx,
                                                    );
                                                } else {
                                                    cx.notify();
                                                }
                                            })
                                            .ok();
                                        })
                                        .detach();
                                    }
                                }),
                            ))
                            .child(secondary_action_button_disabled(
                                "reset-downloads-folder",
                                Some(LocalIcon::RotateRight),
                                "Reset",
                                "Use the system downloads folder",
                                self.downloads_folder_picker_pending,
                                cx.listener(|this, _, _, cx| {
                                    this.update_draft_and_persist(
                                        |draft| draft.downloads_dir = None,
                                        cx,
                                    );
                                }),
                            )),
                    ),
            ]))
            .child(cache_usage_box(
                cache_status(self.cache_overview.as_ref()),
                cache_meter_visual,
            ))
            .child(
                div()
                    .flex()
                    .items_center()
                    .gap(px(8.))
                    .child(secondary_action_button(
                        "open-downloads-folder-btn",
                        Some(LocalIcon::FolderOpen),
                        "Open Downloads",
                        "Open the downloads folder",
                        cx.listener(|this, _, _, _| {
                            let _ = crate::downloads::open_downloads_folder(
                                &this.draft.effective_downloads_dir(),
                            );
                        }),
                    ))
                    .child(secondary_action_button_small_icon(
                        "clear-audio-cache",
                        Some(LocalIcon::TrashCan),
                        "Clear cache",
                        "Clear cached audio",
                        cx.listener(|this, _, _, cx| {
                            this.clear_audio_cache(cx);
                        }),
                    )),
            )
            .into_any_element()
    }

    fn render_behavior_section(&self, cx: &mut Context<Self>) -> AnyElement {
        settings_card()
            .child(settings_card_heading(
                LocalIcon::WindowMaximize,
                "App behavior",
            ))
            .child(control_list(vec![
                control_row(
                    "Start page",
                    "Choose what appears when ralgruM opens.",
                    settings_select(&self.start_page_select, 130.).into_any_element(),
                ),
                control_row(
                    "Remember navigation choices",
                    "Restore search filters and each library category.",
                    settings_switch(
                        "remember-navigation",
                        self.draft.remember_navigation,
                        cx.listener(|this, checked: &bool, _, cx| {
                            this.update_draft_and_persist(
                                |draft| draft.remember_navigation = *checked,
                                cx,
                            );
                        }),
                    )
                    .into_any_element(),
                ),
                control_row(
                    "Restore window",
                    "Save this preference for future window restoration.",
                    settings_switch(
                        "restore-window",
                        self.draft.restore_window,
                        cx.listener(|this, checked: &bool, _, cx| {
                            this.update_draft_and_persist(
                                |draft| draft.restore_window = *checked,
                                cx,
                            );
                        }),
                    )
                    .into_any_element(),
                ),
                control_row(
                    "Keep running in system tray",
                    "Hide the window when it is closed and keep playback running in the tray.",
                    settings_switch(
                        "close-to-tray",
                        self.draft.close_to_tray,
                        cx.listener(|this, checked: &bool, _, cx| {
                            this.update_draft_and_persist(
                                |draft| draft.close_to_tray = *checked,
                                cx,
                            );
                        }),
                    )
                    .into_any_element(),
                ),
                control_row(
                    "Disable animations",
                    "Turn off interface animations and motion effects.",
                    settings_switch(
                        "disable-animations",
                        self.draft.motion_preference.is_reduced(),
                        cx.listener(|this, checked: &bool, _, cx| {
                            this.update_draft_and_persist(
                                |draft| {
                                    draft.motion_preference = if *checked {
                                        crate::navigation_state::MotionPreference::Reduced
                                    } else {
                                        crate::navigation_state::MotionPreference::Full
                                    };
                                },
                                cx,
                            );
                        }),
                    )
                    .into_any_element(),
                ),
                control_row(
                    "Disable explicit content",
                    "Keep explicit tracks visible, but gray them out and skip them everywhere.",
                    settings_switch(
                        "block-explicit-content",
                        self.draft.block_explicit_content,
                        cx.listener(|this, checked: &bool, _, cx| {
                            this.update_draft_and_persist(
                                |draft| draft.block_explicit_content = *checked,
                                cx,
                            );
                        }),
                    )
                    .into_any_element(),
                ),
                control_row(
                    "Disable AI generated content",
                    "Keep AI generated tracks visible, but gray them out and skip them everywhere.",
                    settings_switch(
                        "block-ai-content",
                        self.draft.block_ai_content,
                        cx.listener(|this, checked: &bool, _, cx| {
                            this.update_draft_and_persist(
                                |draft| draft.block_ai_content = *checked,
                                cx,
                            );
                        }),
                    )
                    .into_any_element(),
                ),
            ]))
            .into_any_element()
    }

    fn render_playback_settings(&self, cx: &mut Context<Self>) -> AnyElement {
        settings_card()
            .child(settings_card_heading(LocalIcon::Play, "Playback"))
            .child(control_list({
                let mut playback_rows = vec![
                    control_row(
                        "Output device",
                        "Choose where ralgruM plays audio.",
                        settings_select(&self.output_device_select, 180.).into_any_element(),
                    ),
                    control_row(
                        "ASIO mode",
                        "Play through an ASIO driver instead of the Windows audio stack.",
                        settings_switch(
                            "asio-mode",
                            self.draft.asio_mode,
                            cx.listener(|this, checked: &bool, window, cx| {
                                this.update_draft_and_persist(
                                    |draft| draft.asio_mode = *checked,
                                    cx,
                                );
                                // The dropdown contents depend on the mode,
                                // so the picker re-renders with the other
                                // list and that mode's saved selection.
                                this.refresh_output_device_select(window, cx);
                            }),
                        )
                        .into_any_element(),
                    ),
                ];
                playback_rows.extend([
                    control_row(
                        "Seamless transitions",
                        "Preload the next track near the end for a clean handoff.",
                        settings_switch(
                            "seamless-playback",
                            self.draft.seamless_playback,
                            cx.listener(|this, checked: &bool, _, cx| {
                                this.update_draft_and_persist(
                                    |draft| draft.seamless_playback = *checked,
                                    cx,
                                );
                            }),
                        )
                        .into_any_element(),
                    ),
                    control_row(
                        "Remember shuffle and repeat",
                        "Keep both playback modes after restarting.",
                        settings_switch(
                            "remember-playback-modes",
                            self.draft.remember_playback_modes,
                            cx.listener(|this, checked: &bool, _, cx| {
                                this.update_draft_and_persist(
                                    |draft| draft.remember_playback_modes = *checked,
                                    cx,
                                );
                            }),
                        )
                        .into_any_element(),
                    ),
                    control_row(
                        "Default lyrics provider",
                        "You can still switch providers from the lyrics pane.",
                        settings_select(&self.lyrics_source_select, 130.).into_any_element(),
                    ),
                    control_row(
                        "Share activity with Discord",
                        "Show the current track in Discord Rich Presence.",
                        settings_switch(
                            "discord-presence",
                            self.draft.discord_presence,
                            cx.listener(|this, checked: &bool, _, cx| {
                                this.update_draft_and_persist(
                                    |draft| draft.discord_presence = *checked,
                                    cx,
                                );
                            }),
                        )
                        .into_any_element(),
                    ),
                ]);
                playback_rows
            }))
            .into_any_element()
    }
}

fn settings_select(
    state: &Entity<SelectState<Vec<SharedString>>>,
    width: f32,
) -> Select<Vec<SharedString>> {
    Select::new(state)
        .w(px(width))
        .h(px(34.))
        .text_size(px(11.5))
        .bg(rgb(BACKGROUND))
        .pl(px(10.))
        .pr(px(10.))
        .icon(crate::assets::widget_icon(LocalIcon::ChevronDown))
        .map_menu(|menu| {
            menu.bg(rgba(CONTEXT_MENU_SURFACE))
                .border_color(rgba(CONTEXT_MENU_BORDER))
                .text_color(rgb(CONTEXT_MENU_FOREGROUND))
                .shadow_lg()
                .into_any_element()
        })
}

fn control_row(title: &'static str, description: &'static str, control: AnyElement) -> Div {
    div()
        .w_full()
        .min_h(px(55.))
        .py(px(8.))
        .flex()
        .items_center()
        .justify_between()
        .gap(px(18.))
        .child(
            div()
                .flex_1()
                .min_w_0()
                .flex()
                .flex_col()
                .gap(px(3.))
                .child(
                    div()
                        .text_size(px(12.5))
                        .line_height(px(15.625))
                        .font_weight(FontWeight::MEDIUM)
                        .text_color(rgb(FOREGROUND))
                        .child(title),
                )
                .child(
                    div()
                        .text_size(px(11.))
                        .text_color(rgb(MUTED))
                        .child(description),
                ),
        )
        .child(div().flex_none().child(control))
}

fn control_list(rows: Vec<Div>) -> Div {
    let total = rows.len();
    let mut list = div().w_full().flex().flex_col();
    for (index, row) in rows.into_iter().enumerate() {
        if index + 1 == total {
            list = list.child(row);
        } else {
            list = list.child(row.border_b_1().border_color(rgba(0xffffff0e)));
        }
    }
    list
}

fn cache_usage_box(status: String, visual: CacheMeterVisual) -> Div {
    let fill = if visual.active {
        div()
            .id("settings-audio-cache-meter")
            .h_full()
            .w(relative(visual.from))
            .rounded_full()
            .bg(rgb(PRIMARY))
            .with_animation(
                ("settings-audio-cache-meter", visual.epoch),
                crate::motion::content(),
                move |this, delta| {
                    this.w(relative(crate::motion::lerp(
                        visual.from,
                        visual.target,
                        delta,
                    )))
                },
            )
            .into_any_element()
    } else {
        div()
            .id("settings-audio-cache-meter")
            .h_full()
            .w(relative(visual.target))
            .rounded_full()
            .bg(rgb(PRIMARY))
            .into_any_element()
    };

    div()
        .px(px(12.))
        .py(px(11.))
        .rounded(px(6.))
        .border_1()
        .border_color(rgba(0xffffff0f))
        .bg(rgba(0x09090b73))
        .child(
            div()
                .flex()
                .items_center()
                .justify_between()
                .gap(px(12.))
                .text_size(px(11.))
                .child(div().text_color(rgb(MUTED)).child("Audio cache"))
                .child(
                    div()
                        .font_weight(FontWeight::MEDIUM)
                        .text_color(rgb(FOREGROUND))
                        .child(status),
                ),
        )
        .child(
            div()
                .mt(px(9.))
                .h(px(4.))
                .w_full()
                .overflow_hidden()
                .rounded_full()
                .bg(rgb(BORDER))
                .child(fill),
        )
}

fn cache_used_fraction(overview: Option<&crate::playback::Overview>) -> f32 {
    let Some(overview) = overview else {
        return 0.;
    };
    if overview.max_bytes == 0 {
        return 0.;
    }
    (overview.used_bytes as f32 / overview.max_bytes as f32).clamp(0., 1.)
}

fn cache_status(overview: Option<&crate::playback::Overview>) -> String {
    let Some(overview) = overview else {
        return "Calculating...".into();
    };
    format!(
        "{} of {} used",
        format_storage_size(overview.used_bytes),
        format_storage_size(overview.max_bytes),
    )
}

fn format_storage_size(bytes: u64) -> String {
    if bytes < 1024 * 1024 {
        return format!("{} KB", bytes / 1024);
    }
    if bytes < 1024 * 1024 * 1024 {
        return format!("{} MB", (bytes as f64 / (1024.0 * 1024.0)).round() as u64);
    }
    let gigabytes = bytes as f64 / (1024.0 * 1024.0 * 1024.0);
    if gigabytes >= 10. {
        format!("{gigabytes:.0} GB")
    } else {
        format!("{gigabytes:.1} GB")
    }
}
pub(super) fn cache_limit_label(limit_mb: u64) -> String {
    if limit_mb >= 1024 && limit_mb.is_multiple_of(1024) {
        format!("{} GB", limit_mb / 1024)
    } else {
        format!("{limit_mb} MB")
    }
}

#[cfg(test)]
mod tests {
    use super::{cache_limit_label, cache_status, cache_used_fraction};
    use crate::playback::Overview;

    #[test]
    fn cache_status_reports_usage_and_background_activity() {
        assert_eq!(
            cache_status(Some(&Overview {
                used_bytes: 3 * 1024 * 1024 / 2,
                max_bytes: 256 * 1024 * 1024,
                block_count: 2,
                active_jobs: 1,
            })),
            "2 MB of 256 MB used"
        );
    }

    #[test]
    fn cache_limit_labels_match_reference_units() {
        assert_eq!(cache_limit_label(256), "256 MB");
        assert_eq!(cache_limit_label(512), "512 MB");
        assert_eq!(cache_limit_label(1024), "1 GB");
        assert_eq!(cache_limit_label(2048), "2 GB");
        assert_eq!(cache_limit_label(4096), "4 GB");
    }

    #[test]
    fn cache_used_fraction_clamps_to_meter_bounds() {
        assert_eq!(cache_used_fraction(None), 0.);
        assert_eq!(
            cache_used_fraction(Some(&Overview {
                used_bytes: 50,
                max_bytes: 200,
                block_count: 1,
                active_jobs: 0,
            })),
            0.25
        );
        assert_eq!(
            cache_used_fraction(Some(&Overview {
                used_bytes: 400,
                max_bytes: 200,
                block_count: 1,
                active_jobs: 0,
            })),
            1.
        );
    }
}
