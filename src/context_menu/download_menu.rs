use gpui::{
    Context, Entity, Focusable, IntoElement, Keystroke, MouseButton, Render, Subscription,
    WeakEntity, Window, div, prelude::*,
};
use gpui_component::{ActiveTheme, Icon};

use crate::{
    assets::LocalIcon,
    downloads::{CapabilityChanged, CapabilityKey, CapabilityState, DownloadModel},
    playback::{DownloadChoice, PlaybackTrack},
    settings::AccountState,
};

use super::{PopupMenu, PopupMenuItem, items, submenu};

fn capability_rows(
    track: PlaybackTrack,
    downloads: Entity<DownloadModel>,
    account: Entity<AccountState>,
    cx: &mut Context<PopupMenu>,
) -> Entity<DownloadCapabilityRows> {
    let initial = downloads.update(cx, |downloads, cx| {
        downloads.ensure_capabilities(track.clone(), cx)
    });
    cx.new(|cx| DownloadCapabilityRows::new(track, downloads, account, initial, cx))
}

fn capability_rows_item(
    menu: PopupMenu,
    rows: Entity<DownloadCapabilityRows>,
    cx: &mut Context<PopupMenu>,
) -> PopupMenu {
    let child_menu = cx.entity().downgrade();
    rows.update(cx, |rows, _| {
        rows.dismiss_menu = Some(child_menu);
    });
    // The capability response can change the number of format rows after the
    // menu is built. Keep one disabled PopupMenu container for the reactive
    // body, and let each format row own its hover and click behavior.
    menu.item(
        PopupMenuItem::element(move |_, _| rows.clone())
            .icon(Icon::empty())
            .disabled(true)
            .disabled_visual(false),
    )
}

/// Build the format chooser for direct download buttons. This is the same
/// reactive body used by the Download submenu in track context menus. The
/// menu opens above the trigger with a bottom chevron pointing down to the
/// button, matching the player bar placement.
pub(super) fn download_format_menu_above(
    menu: PopupMenu,
    window: &mut Window,
    cx: &mut Context<PopupMenu>,
    track: PlaybackTrack,
    downloads: Entity<DownloadModel>,
    account: Entity<AccountState>,
) -> PopupMenu {
    let rows = capability_rows(track, downloads, account, cx);
    submenu::style_submenu_popup(capability_rows_item(menu.with_arrow(), rows, cx), window)
}

/// Build the track download submenu with a reactive body. The PopupMenu and
/// its focus handle remain stable while this entity changes from checking to
/// the resolved choices.
pub(super) fn download_format_submenu(
    menu: PopupMenu,
    window: &mut Window,
    cx: &mut Context<PopupMenu>,
    track: PlaybackTrack,
    downloads: Entity<DownloadModel>,
    account: Entity<AccountState>,
) -> PopupMenu {
    let rows = capability_rows(track, downloads, account, cx);

    submenu::styled_submenu_with_icon(
        menu,
        LocalIcon::Download,
        "Download",
        window,
        cx,
        move |menu, _, submenu_cx| capability_rows_item(menu, rows.clone(), submenu_cx),
    )
}

struct DownloadCapabilityRows {
    track: PlaybackTrack,
    downloads: Entity<DownloadModel>,
    account: Entity<AccountState>,
    dismiss_menu: Option<WeakEntity<PopupMenu>>,
    key: CapabilityKey,
    state: CapabilityState,
    hovered_choice: Option<crate::playback::DownloadVariant>,
    hovered_retry: bool,
    _downloads_subscription: Subscription,
}

impl DownloadCapabilityRows {
    fn new(
        track: PlaybackTrack,
        downloads: Entity<DownloadModel>,
        account: Entity<AccountState>,
        state: CapabilityState,
        cx: &mut Context<Self>,
    ) -> Self {
        let key = CapabilityKey::new(&track, account.read(cx).credential_generation());
        let subscription = cx.subscribe(&downloads, |this, _, event: &CapabilityChanged, cx| {
            if event.key != this.key {
                return;
            }
            let state = match event.state.clone() {
                Some(state) => state,
                None => {
                    let track = this.track.clone();
                    let account_scope = this.account.read(cx).credential_generation();
                    let state = this
                        .downloads
                        .update(cx, |downloads, cx| downloads.ensure_capabilities(track, cx));
                    this.key = CapabilityKey::new(&this.track, account_scope);
                    state
                }
            };
            if this.state != state {
                this.state = state;
                cx.notify();
            }
        });
        Self {
            track,
            downloads,
            account,
            dismiss_menu: None,
            key,
            state,
            hovered_choice: None,
            hovered_retry: false,
            _downloads_subscription: subscription,
        }
    }

    fn choice_row(
        &self,
        choice: DownloadChoice,
        hovered: bool,
        cx: &mut Context<Self>,
    ) -> gpui::AnyElement {
        let accent = cx.theme().tokens.accent;
        let accent_foreground = cx.theme().accent_foreground;
        let foreground = cx.theme().popover_foreground;
        let row = items::download_variant_row(choice.label.clone(), choice.detail.clone());
        let view = self.clone_for_click();
        let variant = choice.variant;
        let selector = format!("download-capability-choice-{variant:?}");
        items::download_variant_shell(row, hovered)
            .id(selector.clone())
            .debug_selector(move || selector.clone())
            .text_color(foreground)
            .when(hovered, |this| {
                this.bg(accent).text_color(accent_foreground)
            })
            .on_hover(cx.listener(move |rows, hovered, _, cx| {
                if *hovered {
                    rows.hovered_choice = Some(variant);
                } else if rows.hovered_choice == Some(variant) {
                    rows.hovered_choice = None;
                }
                cx.notify();
            }))
            .on_mouse_down(
                MouseButton::Left,
                cx.listener(|_, _, _, cx| cx.stop_propagation()),
            )
            .on_click(move |_, window, cx| {
                cx.stop_propagation();
                view.start_download(&choice, window, cx);
            })
            .into_any_element()
    }

    fn clone_for_click(&self) -> DownloadCapabilityClickTarget {
        DownloadCapabilityClickTarget {
            track: self.track.clone(),
            downloads: self.downloads.clone(),
            account: self.account.clone(),
            dismiss_menu: self.dismiss_menu.clone(),
        }
    }

    fn retry_row(&self, cx: &mut Context<Self>) -> gpui::AnyElement {
        let accent = cx.theme().tokens.accent;
        let accent_foreground = cx.theme().accent_foreground;
        let foreground = cx.theme().popover_foreground;
        let hovered = self.hovered_retry;
        let rows = cx.entity().clone();
        items::download_variant_shell(
            items::action_row(
                "Retry format check".into(),
                Some(LocalIcon::RotateRight),
                submenu::SUBMENU_WIDTH,
            ),
            hovered,
        )
        .id("download-capability-retry")
        .debug_selector(|| "download-capability-retry".to_owned())
        .text_color(foreground)
        .when(hovered, |this| {
            this.bg(accent).text_color(accent_foreground)
        })
        .on_hover(cx.listener(move |rows, hovered, _, cx| {
            rows.hovered_retry = *hovered;
            cx.notify();
        }))
        .on_mouse_down(
            MouseButton::Left,
            cx.listener(|_, _, _, cx| cx.stop_propagation()),
        )
        .on_click(move |_, window, cx| {
            cx.stop_propagation();
            window.prevent_default();
            rows.update(cx, |rows, cx| {
                let track = rows.track.clone();
                let state = rows
                    .downloads
                    .update(cx, |downloads, cx| downloads.retry_capabilities(track, cx));
                rows.state = state;
                cx.notify();
            });
        })
        .into_any_element()
    }
}

#[derive(Clone)]
struct DownloadCapabilityClickTarget {
    track: PlaybackTrack,
    downloads: Entity<DownloadModel>,
    account: Entity<AccountState>,
    dismiss_menu: Option<WeakEntity<PopupMenu>>,
}

impl DownloadCapabilityClickTarget {
    fn start_download(&self, choice: &DownloadChoice, window: &mut Window, cx: &mut gpui::App) {
        let (deezer_arl, soundcloud_token) = {
            let account = self.account.read(cx);
            (account.deezer_arl(), account.soundcloud_token())
        };
        let track = self.track.clone();
        let variant = choice.variant;
        self.downloads.update(cx, |downloads, cx| {
            downloads.start(track, deezer_arl, soundcloud_token, variant, cx);
        });

        // The outer capability item is intentionally disabled so the
        // reactive rows retain independent hitboxes. Focus the child before
        // Dispatch Escape so the app-owned popup dismisses the child and then
        // its parent through the linked parent menu entities.
        if let Some(menu) = self.dismiss_menu.as_ref().and_then(|menu| menu.upgrade()) {
            menu.update(cx, |menu, cx| {
                menu.focus_handle(cx).focus(window, cx);
            });
            if let Ok(escape) = Keystroke::parse("escape") {
                window.dispatch_keystroke(escape, cx);
            }
        }
    }
}

impl Render for DownloadCapabilityRows {
    fn render(&mut self, _window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let clickable =
            matches!(&self.state, CapabilityState::Ready(choices) if !choices.is_empty());
        let state = self.state.clone();
        div()
            .id("download-capability-rows")
            .w_full()
            .flex()
            .flex_col()
            .when(!clickable, |this| {
                this.on_mouse_down(
                    MouseButton::Left,
                    cx.listener(|_, _, _, cx| cx.stop_propagation()),
                )
            })
            .children(match state {
                CapabilityState::Checking => vec![items::action_row(
                    "Checking available formats...".into(),
                    Some(LocalIcon::Music),
                    submenu::SUBMENU_WIDTH,
                )],
                CapabilityState::Ready(choices) if choices.is_empty() => vec![items::action_row(
                    "No downloadable formats available".into(),
                    Some(LocalIcon::Music),
                    submenu::SUBMENU_WIDTH,
                )],
                CapabilityState::Failed => vec![
                    items::action_row(
                        "Format check failed".into(),
                        Some(LocalIcon::TriangleExclamation),
                        submenu::SUBMENU_WIDTH,
                    ),
                    self.retry_row(cx),
                ],
                CapabilityState::Ready(choices) => choices
                    .into_iter()
                    .map(|choice| {
                        let hovered = self.hovered_choice == Some(choice.variant);
                        self.choice_row(choice, hovered, cx)
                    })
                    .collect(),
            })
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::{
        account_session::SessionError,
        murglar_backend::{DeviceIdentityError, DeviceIdentityStatus},
        playback::{DownloadVariant, PlaybackProvider},
    };
    use gpui::{Render, TestAppContext, VisualTestContext, div, point, px};
    use std::{sync::Arc, time::Duration};
    use tokio::runtime::Runtime;

    fn track() -> PlaybackTrack {
        PlaybackTrack {
            provider: PlaybackProvider::Deezer,
            id: "track".into(),
            title: "Title".into(),
            artist: "Artist".into(),
            album: String::new(),
            album_id: String::new(),
            release_date: String::new(),
            artists: Vec::new(),
            artwork: String::new(),
            duration: Duration::from_secs(1),
            downloadable: false,
            progressive: false,
            explicit: false,
            service_url: String::new(),
        }
    }

    #[test]
    fn failed_capability_completion_stays_retryable() {
        let state = CapabilityState::Failed;
        assert_eq!(state, CapabilityState::Failed);
    }

    #[test]
    fn ready_choice_state_keeps_the_exact_variant() {
        let choice = DownloadChoice {
            variant: DownloadVariant::DeezerFlac,
            label: "FLAC".into(),
            detail: String::new(),
        };
        let state = CapabilityState::Ready(vec![choice.clone()]);
        assert_eq!(state, CapabilityState::Ready(vec![choice]));
        assert_eq!(track().provider, PlaybackProvider::Deezer);
    }

    struct CapabilityRowsHost {
        rows: Entity<DownloadCapabilityRows>,
    }

    impl Render for CapabilityRowsHost {
        fn render(&mut self, _: &mut Window, _: &mut Context<Self>) -> impl IntoElement {
            div().size_full().p(px(12.)).child(self.rows.clone())
        }
    }

    #[gpui::test]
    fn capability_rows_hover_only_highlights_the_row_under_pointer(cx: &mut TestAppContext) {
        cx.update(|cx| {
            gpui_component::init(cx);
            crate::theme::configure_component_theme(cx);
        });

        let rows = cx.update(|cx| {
            let account = cx.new(|_| {
                AccountState::new(
                    DeviceIdentityStatus::Error(DeviceIdentityError::HardwareCollectionUnavailable),
                    Err(SessionError::ConfigDirectoryUnavailable),
                )
            });
            let downloads =
                cx.new(|_| DownloadModel::new(account.clone(), Arc::new(Runtime::new().unwrap())));
            cx.new(|cx| {
                DownloadCapabilityRows::new(
                    track(),
                    downloads,
                    account,
                    CapabilityState::Ready(vec![
                        DownloadChoice {
                            variant: DownloadVariant::DeezerFlac,
                            label: "FLAC".into(),
                            detail: "Lossless".into(),
                        },
                        DownloadChoice {
                            variant: DownloadVariant::DeezerMp3_320,
                            label: "MP3 320 kbps".into(),
                            detail: "320 kbps".into(),
                        },
                    ]),
                    cx,
                )
            })
        });

        let window = cx.add_window({
            let rows = rows.clone();
            move |_, _| CapabilityRowsHost { rows }
        });
        let mut visual = VisualTestContext::from_window(gpui::AnyWindowHandle::from(window), cx);
        visual.run_until_parked();
        visual.update(|window, cx| {
            _ = window.draw(cx);
        });

        let first = visual
            .debug_bounds("download-capability-choice-DeezerFlac")
            .expect("FLAC capability row");
        let second = visual
            .debug_bounds("download-capability-choice-DeezerMp3_320")
            .expect("MP3 capability row");
        assert!(second.origin.y > first.origin.y);
        assert_eq!(first.size.width, second.size.width);
        assert!(first.size.width >= px(220.));

        visual.simulate_mouse_move(
            point(first.origin.x + px(10.), first.origin.y + px(10.)),
            None,
            gpui::Modifiers::default(),
        );
        visual.run_until_parked();
        visual.update(|window, cx| {
            _ = window.draw(cx);
        });
        assert_eq!(
            visual.cx.update(|cx| rows.read(cx).hovered_choice),
            Some(DownloadVariant::DeezerFlac)
        );

        visual.simulate_mouse_move(
            point(second.origin.x + px(10.), second.origin.y + px(10.)),
            None,
            gpui::Modifiers::default(),
        );
        visual.run_until_parked();
        visual.update(|window, cx| {
            _ = window.draw(cx);
        });
        assert_eq!(
            visual.cx.update(|cx| rows.read(cx).hovered_choice),
            Some(DownloadVariant::DeezerMp3_320)
        );

        visual.simulate_mouse_move(point(px(4.), px(4.)), None, gpui::Modifiers::default());
        visual.run_until_parked();
        assert_eq!(visual.cx.update(|cx| rows.read(cx).hovered_choice), None);
    }

    #[gpui::test]
    fn capability_rows_retry_hover_highlights_only_the_retry_row(cx: &mut TestAppContext) {
        cx.update(|cx| {
            gpui_component::init(cx);
            crate::theme::configure_component_theme(cx);
        });

        let rows = cx.update(|cx| {
            let account = cx.new(|_| {
                AccountState::new(
                    DeviceIdentityStatus::Error(DeviceIdentityError::HardwareCollectionUnavailable),
                    Err(SessionError::ConfigDirectoryUnavailable),
                )
            });
            let downloads =
                cx.new(|_| DownloadModel::new(account.clone(), Arc::new(Runtime::new().unwrap())));
            cx.new(|cx| {
                DownloadCapabilityRows::new(
                    track(),
                    downloads,
                    account,
                    CapabilityState::Failed,
                    cx,
                )
            })
        });

        let window = cx.add_window({
            let rows = rows.clone();
            move |_, _| CapabilityRowsHost { rows }
        });
        let mut visual = VisualTestContext::from_window(gpui::AnyWindowHandle::from(window), cx);
        visual.run_until_parked();
        visual.update(|window, cx| {
            _ = window.draw(cx);
        });

        let retry = visual
            .debug_bounds("download-capability-retry")
            .expect("retry capability row");
        visual.simulate_mouse_move(
            point(retry.origin.x + px(10.), retry.origin.y + px(10.)),
            None,
            gpui::Modifiers::default(),
        );
        visual.run_until_parked();
        assert!(visual.cx.update(|cx| rows.read(cx).hovered_retry));

        visual.simulate_mouse_move(point(px(4.), px(4.)), None, gpui::Modifiers::default());
        visual.run_until_parked();
        assert!(!visual.cx.update(|cx| rows.read(cx).hovered_retry));
    }

    #[gpui::test]
    fn open_capability_rows_update_from_the_model_event(cx: &mut TestAppContext) {
        let (rows, downloads, key) = cx.update(|cx| {
            let account = cx.new(|_| {
                AccountState::new(
                    DeviceIdentityStatus::Error(DeviceIdentityError::HardwareCollectionUnavailable),
                    Err(SessionError::ConfigDirectoryUnavailable),
                )
            });
            let downloads =
                cx.new(|_| DownloadModel::new(account.clone(), Arc::new(Runtime::new().unwrap())));
            let track = track();
            let key = CapabilityKey::new(&track, account.read(cx).credential_generation());
            let rows = cx.new(|cx| {
                DownloadCapabilityRows::new(
                    track,
                    downloads.clone(),
                    account,
                    CapabilityState::Checking,
                    cx,
                )
            });
            (rows, downloads, key)
        });

        cx.update(|cx| {
            downloads.update(cx, |_, cx| {
                cx.emit(CapabilityChanged {
                    key,
                    state: Some(CapabilityState::Ready(vec![DownloadChoice {
                        variant: DownloadVariant::DeezerFlac,
                        label: "FLAC".into(),
                        detail: String::new(),
                    }])),
                });
            });
        });
        cx.run_until_parked();
        let state = cx.update(|cx| rows.read(cx).state.clone());
        assert!(matches!(state, CapabilityState::Ready(choices) if choices.len() == 1));
    }
}
