use crate::{
    app_button::primary_button_with_loading,
    app_tooltip::AppTooltipExt,
    assets::{LocalIcon, local_icon},
    context_menu::text_field_context_menu,
    service_auth::{self, Service},
    theme::{BORDER, DANGER, DEEZER, FOREGROUND, MUTED, SOUNDCLOUD},
};
use gpui::{
    Context, Div, FontWeight, IntoElement, ObjectFit, SharedString, div, img, prelude::*, px, rgb,
    rgba,
};
use gpui_component::input::Input;

use super::{
    SettingsView,
    action_button::{DangerSecondaryButtonOptions, danger_secondary_button},
    settings_switch,
};

pub(super) fn panel_heading(title: &'static str, copy: &'static str) -> impl IntoElement {
    div()
        .flex()
        .flex_col()
        .gap(px(4.))
        .child(
            div()
                .text_size(px(15.))
                .font_weight(FontWeight::SEMIBOLD)
                .child(title),
        )
        .child(
            div()
                .text_size(px(12.5))
                .text_color(rgb(MUTED))
                .line_height(px(18.75))
                .child(copy),
        )
}

pub(super) fn settings_card() -> Div {
    div()
        .flex()
        .flex_col()
        .gap(px(14.))
        .p(px(14.))
        .bg(rgba(0x18181bb8))
        .border_1()
        .border_color(rgb(BORDER))
        .rounded(px(8.))
}

impl SettingsView {
    pub(super) fn render_providers(&self, cx: &mut Context<Self>) -> impl IntoElement + use<> {
        div().flex().flex_col().gap(px(28.)).children([
            self.render_service(Service::Deezer, cx).into_any_element(),
            self.render_service(Service::SoundCloud, cx)
                .into_any_element(),
        ])
    }

    pub(super) fn refresh_soundcloud_identity(&self, cx: &mut Context<Self>) {
        let request = self.account.update(cx, |account, _| {
            let scope = account.library_scope();
            let request = account.begin_soundcloud_identity_with_generation();
            (scope, request)
        });
        let (scope, Some((generation, token))) = request else {
            return;
        };
        let client = self.http_client.clone();
        let task = self
            .runtime
            .spawn(async move { service_auth::soundcloud_identity(&client, &token).await });
        let account = self.account.downgrade();
        cx.spawn(async move |_, cx| {
            let result = task
                .await
                .unwrap_or_else(|error| Err(format!("The profile task failed: {error}")));
            account
                .update(cx, |account, cx| {
                    if account.library_scope() != scope {
                        account.cancel_service_identity(generation, Service::SoundCloud);
                    } else {
                        account.complete_soundcloud_profile(generation, result);
                    }
                    cx.notify();
                })
                .ok();
        })
        .detach();
    }

    pub(super) fn refresh_deezer_identity(&self, cx: &mut Context<Self>) {
        let request = self.account.update(cx, |account, _| {
            let scope = account.library_scope();
            let request = account.begin_deezer_identity_with_generation();
            (scope, request)
        });
        let (scope, Some((generation, arl))) = request else {
            return;
        };
        let client = self.http_client.clone();
        let task = self
            .runtime
            .spawn(async move { service_auth::deezer_identity(&client, &arl).await });
        let account = self.account.downgrade();
        cx.spawn(async move |_, cx| {
            let result = task
                .await
                .unwrap_or_else(|error| Err(format!("The profile task failed: {error}")));
            account
                .update(cx, |account, cx| {
                    if account.library_scope() != scope {
                        account.cancel_service_identity(generation, Service::Deezer);
                    } else {
                        account.complete_deezer_profile(generation, result);
                    }
                    cx.notify();
                })
                .ok();
        })
        .detach();
    }

    fn refresh_service_identity(&self, service: Service, cx: &mut Context<Self>) {
        match service {
            Service::Deezer => self.refresh_deezer_identity(cx),
            Service::SoundCloud => self.refresh_soundcloud_identity(cx),
        }
    }

    fn maybe_refresh_service_identity(&self, service: Service, cx: &mut Context<Self>) {
        let should_refresh = {
            let account = self.account.read(cx);
            account.should_refresh_service_identity(service)
        };
        if should_refresh {
            self.refresh_service_identity(service, cx);
        }
    }

    fn open_service_login(&mut self, service: Service, cx: &mut Context<Self>) {
        let Some(generation) = self.account.update(cx, |account, cx| {
            let generation = account.begin_service_auth(service);
            cx.notify();
            generation
        }) else {
            return;
        };
        let client = self.http_client.clone();
        let task = self
            .runtime
            .spawn(async move { service_auth::web_login(&client, service).await });
        let account = self.account.downgrade();
        cx.spawn(async move |_, cx| {
            let result = task
                .await
                .unwrap_or_else(|error| Err(format!("The login task failed: {error}")));
            account
                .update(cx, |account, cx| {
                    account.complete_service_auth(generation, result, service);
                    cx.notify();
                })
                .ok();
        })
        .detach();
    }

    fn submit_service_login(&mut self, service: Service, cx: &mut Context<Self>) {
        let (desktop, mobile, user_id) = match service {
            Service::Deezer => (
                self.deezer_arl.read(cx).text().to_string(),
                None,
                Some(self.deezer_user_id_input.read(cx).text().to_string()),
            ),
            Service::SoundCloud => (
                self.soundcloud_desktop.read(cx).text().to_string(),
                Some(self.soundcloud_mobile.read(cx).text().to_string()),
                None,
            ),
        };
        let Some(generation) = self.account.update(cx, |account, cx| {
            let generation = account.begin_service_auth(service);
            cx.notify();
            generation
        }) else {
            return;
        };
        let client = self.http_client.clone();
        let task = self.runtime.spawn(async move {
            service_auth::validate(&client, service, desktop, mobile, user_id).await
        });
        let account = self.account.downgrade();
        cx.spawn(async move |_, cx| {
            let result = task
                .await
                .unwrap_or_else(|error| Err(format!("The login task failed: {error}")));
            account
                .update(cx, |account, cx| {
                    account.complete_service_auth(generation, result, service);
                    cx.notify();
                })
                .ok();
        })
        .detach();
    }

    fn logout_service(&mut self, service: Service, cx: &mut Context<Self>) {
        self.account.update(cx, |account, cx| {
            account.logout_service(service);
            cx.notify();
        });
    }

    pub(super) fn render_service(
        &self,
        service: Service,
        cx: &mut Context<Self>,
    ) -> impl IntoElement + use<> {
        self.maybe_refresh_service_identity(service, cx);
        let (copy, icon, accent) = service_content(service);
        let service_key = service_key(service);
        let record_plays = self.draft.record_deezer_plays;
        let search_suggestions = self.draft.soundcloud_search_suggestions;
        let (history_title, history_description) = listen_history_copy();
        let (suggestions_title, suggestions_description) = search_suggestions_copy();
        let (signed_in, loading, identity_loading, status, session_error, identity) = {
            let account = self.account.read(cx);
            (
                account.service_signed_in(service),
                account.service_loading_for(service),
                match service {
                    Service::Deezer => account.deezer_identity_loading(),
                    Service::SoundCloud => account.soundcloud_identity_loading(),
                },
                account.service_status_for(service),
                account.service_session_error_for(service),
                account.service_identity(service).cloned(),
            )
        };
        let identity_name = identity
            .as_ref()
            .map(|profile| profile.username.clone())
            .unwrap_or_else(|| "Account".to_string());
        let identity_avatar = identity
            .as_ref()
            .and_then(|profile| profile.safe_avatar_url().map(str::to_owned));
        div()
            .flex()
            .flex_col()
            .gap(px(14.))
            .child(service_heading(service, icon, accent, copy))
            .child(
                settings_card()
                    .gap(px(12.))
                    .when(!signed_in, |this| {
                        this.child(service_card_heading(LocalIcon::UserLock, FOREGROUND))
                    })
                    .when(!signed_in, |this| {
                        this.child(
                            div()
                                .text_size(px(11.5))
                                .line_height(px(17.25))
                                .text_color(rgb(MUTED))
                                .child(if cfg!(windows) {
                                    "A secure window will open. Sign in there and ralgruM will save the session automatically."
                                        .to_string()
                                } else {
                                    "Enter the required session credentials below to validate and save the account."
                                        .to_string()
                                }),
                        )
                    })
                    .when(signed_in, |this| {
                        this.child(service_identity_row(
                            service,
                            identity_name,
                            identity_avatar.clone(),
                            accent,
                        ))
                    })
                    .when(signed_in && service == Service::Deezer, |this| {
                        this.child(
                            div()
                                .w_full()
                                .min_h(px(58.))
                                .pt(px(10.))
                                .border_t_1()
                                .border_color(rgba(0xffffff12))
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
                                                .font_weight(FontWeight::MEDIUM)
                                                .child(history_title),
                                        )
                                        .child(
                                            div()
                                                .text_size(px(11.5))
                                                .line_height(px(16.675))
                                                .text_color(rgb(MUTED))
                                                .child(history_description),
                                        ),
                                )
                                .child(
                                    settings_switch(
                                        format!("{service_key}-record-plays"),
                                        record_plays,
                                        cx.listener(
                                            move |this, checked: &bool, _, cx| {
                                                this.update_draft_and_persist(
                                                    |draft| draft.record_deezer_plays = *checked,
                                                    cx,
                                                );
                                            },
                                        ),
                                    )
                                ),
                        )
                    })
                    .when(service == Service::SoundCloud, |this| {
                        this.child(
                            provider_preference_row(
                                format!("{service_key}-search-suggestions"),
                                suggestions_title,
                                suggestions_description,
                                search_suggestions,
                                cx.listener(move |this, checked: &bool, _, cx| {
                                    this.update_draft_and_persist(
                                        |draft| {
                                            draft.soundcloud_search_suggestions = *checked
                                        },
                                        cx,
                                    );
                                }),
                            )
                            .pt(px(10.))
                            .border_t_1()
                            .border_color(rgba(0xffffff12)),
                        )
                    })
                    .when(!signed_in, |this| {
                        this.child(
                            div()
                                .id(format!("{service_key}-open-login-tooltip"))
                                .w_full()
                                .child(
                                    primary_button_with_loading(
                                        format!("{service_key}-open-login"),
                                        Some(LocalIcon::LogIn),
                                        "Sign in",
                                        session_error.is_some(),
                                        loading,
                                        cx.listener(move |this, _, _, cx| {
                                            this.open_service_login(service, cx)
                                        }),
                                    )
                                        .h(px(40.))
                                        .w_full()
                                ),
                        )
                    })
                    .when(!signed_in && cfg!(not(windows)), |this| {
                        this.child(credential_fields(self, service)).child(
                            div()
                                .id(format!("{service_key}-validate-login-tooltip"))
                                .app_tooltip("Validate and save this provider session")
                                .child(
                                    primary_button_with_loading(
                                        format!("{service_key}-validate-login"),
                                        Some(LocalIcon::Check),
                                        "Validate and save",
                                        session_error.is_some(),
                                        loading,
                                        cx.listener(move |this, _, _, cx| {
                                            this.submit_service_login(service, cx)
                                        }),
                                    ),
                                ),
                        )
                    })
                    .when(signed_in && identity_loading, |this| {
                        this.child(
                            div()
                                .text_size(px(11.5))
                                .text_color(rgb(MUTED))
                                .child("Loading account profile..."),
                        )
                    })
                    .when_some(status, |this, status| this.child(inline_status(status, false)))
                    .when_some(session_error, |this, error| {
                        this.child(inline_status(error, true))
                    }),
            )
            .when(signed_in, |this| {
                this.child(
                    div().w_full().flex().justify_start().child(
                        danger_secondary_button(
                            format!("{service_key}-logout"),
                            "Log out",
                            loading,
                            DangerSecondaryButtonOptions::SERVICE,
                            cx.listener(move |this, _, _, cx| {
                                this.logout_service(service, cx)
                            }),
                        ),
                    ),
                )
            })
    }
}

fn service_content(service: Service) -> (&'static str, LocalIcon, u32) {
    match service {
        Service::Deezer => (
            "Access your library, Flow, and downloads.",
            LocalIcon::Deezer,
            DEEZER,
        ),
        Service::SoundCloud => (
            "Access your library, original-quality playback, and search suggestions.",
            LocalIcon::SoundCloud,
            SOUNDCLOUD,
        ),
    }
}

fn service_heading(service: Service, icon: LocalIcon, accent: u32, copy: &'static str) -> Div {
    div()
        .flex()
        .flex_col()
        .gap(px(4.))
        .child(
            div()
                .flex()
                .items_center()
                .gap(px(8.))
                .text_size(px(15.))
                .font_weight(FontWeight::SEMIBOLD)
                .child(
                    div()
                        .relative()
                        .top(px(service_heading_icon_offset(service)))
                        .size(px(15.))
                        .child(local_icon(icon, accent).size(px(15.))),
                )
                .child(service_label(service)),
        )
        .child(
            div()
                .text_size(px(12.5))
                .text_color(rgb(MUTED))
                .line_height(px(18.75))
                .child(copy),
        )
}

fn service_heading_icon_offset(service: Service) -> f32 {
    match service {
        Service::Deezer => 0.0,
        Service::SoundCloud => 0.5,
    }
}

fn service_label(service: Service) -> &'static str {
    match service {
        Service::Deezer => "Deezer",
        Service::SoundCloud => "SoundCloud",
    }
}

fn service_key(service: Service) -> &'static str {
    match service {
        Service::Deezer => "deezer",
        Service::SoundCloud => "soundcloud",
    }
}

fn listen_history_copy() -> (&'static str, &'static str) {
    (
        "Save listening history",
        "Notify Deezer about the tracks you're playing to improve recommendations and listening history.",
    )
}

fn search_suggestions_copy() -> (&'static str, &'static str) {
    (
        "Search suggestions",
        "Use SoundCloud's Search API to provide search suggestions. This works without a SoundCloud account too.",
    )
}

fn provider_preference_row(
    id: impl Into<gpui::ElementId>,
    title: &'static str,
    description: &'static str,
    checked: bool,
    on_click: impl Fn(&bool, &mut gpui::Window, &mut gpui::App) + 'static,
) -> Div {
    div()
        .w_full()
        .min_h(px(58.))
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
                        .font_weight(FontWeight::MEDIUM)
                        .child(title),
                )
                .child(
                    div()
                        .text_size(px(11.5))
                        .line_height(px(16.675))
                        .text_color(rgb(MUTED))
                        .child(description),
                ),
        )
        .child(settings_switch(id, checked, on_click))
}

fn service_card_heading(icon: LocalIcon, accent: u32) -> Div {
    div()
        .flex()
        .items_center()
        .gap(px(8.))
        .text_size(px(13.))
        .font_weight(FontWeight::MEDIUM)
        .child(local_icon(icon, accent).size(px(14.)))
        .child("Sign in")
}

fn service_identity_row(
    service: Service,
    username: String,
    avatar_url: Option<String>,
    accent: u32,
) -> Div {
    let icon = match service {
        Service::Deezer => LocalIcon::Deezer,
        Service::SoundCloud => LocalIcon::SoundCloud,
    };
    div()
        .flex()
        .items_center()
        .gap(px(10.))
        .p(px(10.))
        .border_1()
        .border_color(rgba(0xffffff12))
        .bg(rgba(0x09090b73))
        .rounded(px(7.))
        .child(service_avatar(icon, avatar_url, accent))
        .child(
            div()
                .flex()
                .flex_col()
                .gap(px(3.))
                .flex_1()
                .min_w_0()
                .child(
                    div()
                        .text_size(px(13.))
                        .font_weight(FontWeight::MEDIUM)
                        .whitespace_nowrap()
                        .truncate()
                        .child(username),
                )
                .child(
                    div()
                        .text_size(px(11.5))
                        .text_color(rgb(MUTED))
                        .whitespace_nowrap()
                        .truncate()
                        .child("Connected"),
                ),
        )
}

fn service_avatar(icon: LocalIcon, avatar_url: Option<String>, accent: u32) -> Div {
    let fallback = div()
        .size(px(38.))
        .rounded_full()
        .flex()
        .items_center()
        .justify_center()
        .bg(rgba((accent << 8) | 0x42))
        .child(local_icon(icon, FOREGROUND).size(px(17.)));
    div()
        .relative()
        .size(px(38.))
        .rounded_full()
        .overflow_hidden()
        .flex_none()
        .child(fallback)
        .when_some(avatar_url, |this, url| {
            this.child(
                div().absolute().inset_0().child(
                    img(url)
                        .size_full()
                        .rounded_full()
                        .object_fit(ObjectFit::Cover),
                ),
            )
        })
}

fn inline_status(status: SharedString, error: bool) -> Div {
    div()
        .text_size(px(11.5))
        .line_height(px(16.675))
        .text_color(rgb(if error { DANGER } else { MUTED }))
        .child(status)
}

fn credential_fields(view: &SettingsView, service: Service) -> impl IntoElement {
    div().flex().flex_col().gap(px(7.)).children(match service {
        Service::Deezer => vec![
            labeled_input("ARL cookie", &view.deezer_arl).into_any_element(),
            labeled_input(
                "User ID (optional, checked when provided)",
                &view.deezer_user_id_input,
            )
            .into_any_element(),
        ],
        Service::SoundCloud => vec![
            labeled_input("Desktop OAuth token", &view.soundcloud_desktop).into_any_element(),
            labeled_input("Mobile OAuth token", &view.soundcloud_mobile).into_any_element(),
        ],
    })
}

fn labeled_input(
    label: &'static str,
    input: &gpui::Entity<gpui_component::input::InputState>,
) -> impl IntoElement {
    div()
        .flex()
        .flex_col()
        .gap(px(7.))
        .child(div().text_size(px(12.)).text_color(rgb(MUTED)).child(label))
        .child(text_field_context_menu(Input::new(input), input.clone()))
}

#[cfg(test)]
mod tests {
    use super::{
        listen_history_copy, search_suggestions_copy, service_content, service_heading_icon_offset,
        service_key, service_label,
    };
    use crate::service_auth::Service;

    #[test]
    fn service_panel_copy_is_concise_and_provider_neutral() {
        let (deezer_copy, _, _) = service_content(Service::Deezer);
        let (soundcloud_copy, _, _) = service_content(Service::SoundCloud);

        assert_eq!(deezer_copy, "Access your library, Flow, and downloads.");
        assert_eq!(
            soundcloud_copy,
            "Access your library, original-quality playback, and search suggestions."
        );
        assert!(!deezer_copy.contains("Deezer"));
        assert!(!soundcloud_copy.contains("SoundCloud"));
    }

    #[test]
    fn soundcloud_suggestion_copy_mentions_signed_out_support() {
        assert_eq!(
            search_suggestions_copy(),
            (
                "Search suggestions",
                "Use SoundCloud's Search API to provide search suggestions. This works without a SoundCloud account too."
            )
        );
    }

    #[test]
    fn listening_history_copy_is_plain_and_provider_specific() {
        assert_eq!(
            listen_history_copy(),
            (
                "Save listening history",
                "Notify Deezer about the tracks you're playing to improve recommendations and listening history."
            )
        );
        let (title, description) = listen_history_copy();
        let copy = format!("{title} {description}").to_ascii_lowercase();
        assert!(!copy.contains("scrobbl"));
        assert!(!copy.contains("endpoint"));
        assert!(!copy.contains("payload"));
    }

    #[test]
    fn service_action_ids_keep_provider_identity_without_repeating_it_in_copy() {
        assert_eq!(service_key(Service::Deezer), "deezer");
        assert_eq!(service_key(Service::SoundCloud), "soundcloud");
    }

    #[test]
    fn provider_headings_restore_the_canonical_service_names() {
        assert_eq!(service_label(Service::Deezer), "Deezer");
        assert_eq!(service_label(Service::SoundCloud), "SoundCloud");
    }

    #[test]
    fn soundcloud_heading_icon_uses_its_optical_vertical_offset() {
        assert_eq!(service_heading_icon_offset(Service::Deezer), 0.0);
        assert_eq!(service_heading_icon_offset(Service::SoundCloud), 0.5);
    }
}
