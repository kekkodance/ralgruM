use gpui::{
    Context, FontWeight, IntoElement, Role, SharedString, Window, div, prelude::*, px, rgb, rgba,
};
use gpui_component::input::{Input, InputContentType};
use tokio::task::JoinHandle;

#[path = "murglar_plans.rs"]
mod murglar_plans;

use super::{
    SettingsView,
    action_button::{DangerSecondaryButtonOptions, danger_secondary_button},
    murglar_referral,
    service_panel::{panel_heading, settings_card},
};
use crate::{
    app_button::primary_button_with_loading,
    app_tooltip::AppTooltipExt,
    assets::{LocalIcon, local_icon},
    context_menu::text_field_context_menu,
    murglar_backend::{
        AccountError, AccountProfile, DeviceIdentity, DeviceIdentityStatus, MurglarClient,
    },
    theme::{BORDER, DANGER, FOREGROUND, MUTED},
};

pub(super) fn reset_murglar_plan_motion() {
    murglar_plans::reset_payment_methods_motion();
}

impl SettingsView {
    pub(crate) fn refresh_murglar_profile(&mut self, cx: &mut Context<Self>) {
        let refresh = self.account.update(cx, |account, cx| {
            let refresh = account.refresh();
            cx.notify();
            refresh
        });
        self.request_murglar_profile(refresh, cx);
    }

    fn refresh_murglar_referral(&mut self, cx: &mut Context<Self>) {
        let refresh = self.account.update(cx, |account, cx| {
            let refresh = account.begin_referral_refresh();
            cx.notify();
            refresh
        });
        if let Some((generation, identity, token)) = refresh {
            let client = MurglarClient::with_client(self.http_client.clone(), identity);
            let task = self
                .runtime
                .spawn(async move { Ok(client.account_extras(&token).await) });
            let account = self.account.downgrade();
            cx.spawn(async move |_, cx| {
                let result = account_task_result(task).await;
                account
                    .update(cx, |account, cx| {
                        let extras = result.unwrap_or_else(|_| MurglarClient::empty_extras());
                        if account.complete_referral(generation, extras.referral) {
                            cx.notify();
                        }
                    })
                    .ok();
            })
            .detach();
        }
    }

    pub(super) fn open_murglar_profile(&mut self, cx: &mut Context<Self>) {
        let should_fetch = {
            let account = self.account.read(cx);
            account.token.is_some()
                && !account.loading
                && (account.profile.is_none()
                    || account.referral.is_none()
                    || account.plans.is_none())
        };
        if should_fetch {
            self.refresh_murglar_profile(cx);
        }
    }

    /// Loads the Murglar profile for the sidebar at app start, mirroring the
    /// original's boot-time cached-profile display plus fresh fetch.
    pub(crate) fn fetch_startup_profile(&mut self, cx: &mut Context<Self>) {
        let should_fetch = {
            let account = self.account.read(cx);
            account.profile.is_none() && !account.loading
        };
        if !should_fetch {
            return;
        }
        let request = self
            .account
            .update(cx, |account, _| account.begin_startup_profile());
        self.request_murglar_profile(request, cx);
    }

    fn request_murglar_profile(
        &mut self,
        refresh: Option<(u64, DeviceIdentity, String)>,
        cx: &mut Context<Self>,
    ) {
        if let Some((generation, identity, token)) = refresh {
            let client = MurglarClient::with_client(self.http_client.clone(), identity);
            let task = self.runtime.spawn(async move {
                let (profile, extras) = futures::join!(
                    client.account_profile(&token),
                    client.account_extras(&token)
                );
                Ok((profile, extras))
            });
            let account = self.account.downgrade();
            cx.spawn(async move |_, cx| {
                let result = account_task_result(task).await;
                account
                    .update(cx, |account, cx| {
                        let (profile, extras) = result
                            .unwrap_or_else(|error| (Err(error), MurglarClient::empty_extras()));
                        let profile_changed = account.complete_profile(generation, profile, cx);
                        let extras_changed = account.complete_extras(generation, extras);
                        if profile_changed || extras_changed {
                            cx.notify();
                        }
                    })
                    .ok();
            })
            .detach();
        }
    }

    pub(super) fn submit_login(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        let username = self.username.read(cx).text().to_string();
        let password = self.password.read(cx).text().to_string();
        if username.trim().is_empty() || password.is_empty() {
            self.account.update(cx, |account, cx| {
                account.status = Some("Enter your username and password.".into());
                account.device_limit_exceeded = false;
                cx.notify();
            });
            return;
        }

        self.password
            .update(cx, |input, cx| input.set_value("", window, cx));
        let Some(login) = self.account.update(cx, |account, cx| {
            let login = account.begin_login(cx);
            cx.notify();
            login
        }) else {
            return;
        };

        let http_client = self.http_client.clone();
        let runtime = self.runtime.clone();
        let account = self.account.downgrade();
        cx.spawn(async move |_, cx| {
            // The storage preflight must settle before the network login
            // starts: an unwritable store rejects the login up front
            // instead of stranding a session that cannot be persisted.
            if let Some(preflight) = login.storage_preflight
                && preflight.await.is_err()
            {
                return;
            }
            let client = MurglarClient::with_client(http_client, login.identity);
            let task = runtime.spawn(async move {
                let token = client.exchange_token(username, password).await?;
                Ok((client, token))
            });
            let result = account_task_result(task).await;
            let (client, result) = match result {
                Ok((client, token)) => (Some(client), Ok(token)),
                Err(error) => (None, Err(error)),
            };
            let persisted_token = account
                .update(cx, |account, cx| {
                    let token = account.complete_token_exchange(
                        login.generation,
                        login.login_epoch,
                        result,
                        cx,
                    );
                    cx.notify();
                    token
                })
                .ok()
                .flatten();
            let (Some(client), Some(token)) = (client, persisted_token) else {
                return;
            };
            let profile_task = runtime.spawn(async move {
                let (profile, extras) = futures::join!(
                    client.account_profile(&token),
                    client.account_extras(&token)
                );
                Ok((profile, extras))
            });
            let result = account_task_result(profile_task).await;
            account
                .update(cx, |account, cx| {
                    let (profile, extras) =
                        result.unwrap_or_else(|error| (Err(error), MurglarClient::empty_extras()));
                    let profile_changed = account.complete_profile(login.generation, profile, cx);
                    let extras_changed = account.complete_extras(login.generation, extras);
                    if profile_changed || extras_changed {
                        cx.notify();
                    }
                })
                .ok();
        })
        .detach();
    }

    fn logout(&mut self, _: &mut Window, cx: &mut Context<Self>) {
        self.account.update(cx, |account, cx| {
            account.logout(cx);
            cx.notify();
        });
    }

    fn open_status_page(&mut self, cx: &mut Context<Self>) {
        if let Err(error) = crate::external_url::open_murglar_status() {
            self.account.update(cx, |account, cx| {
                account.status = Some(error.into());
                cx.notify();
            });
        }
    }

    pub(super) fn begin_payment(
        &mut self,
        promotion_id: String,
        merchant_id: String,
        cx: &mut Context<Self>,
    ) {
        let Some((identity, token)) = self.account.read(cx).murglar_credentials() else {
            self.account.update(cx, |account, cx| {
                account.plans_status = None;
                cx.notify();
            });
            crate::toast::push_global(
                cx,
                crate::toast::ToastKind::Warning,
                "Murglar login required",
                Some("Log in to Murglar before choosing a payment method.".into()),
            );
            return;
        };
        let client = MurglarClient::with_client(self.http_client.clone(), identity);
        let requested_token = token.clone();
        let task = self.runtime.spawn(async move {
            let result = client
                .generate_payment_link(&token, &promotion_id, &merchant_id)
                .await;
            Ok::<_, AccountError>((requested_token, result))
        });
        let account = self.account.downgrade();
        cx.spawn(async move |_, cx| {
            let result = account_task_result(task).await;
            account
                .update(cx, |account, cx| {
                    let Ok((requested_token, payment_result)) = result else {
                        return;
                    };
                    if account
                        .murglar_credentials()
                        .is_none_or(|(_, token)| token != requested_token)
                    {
                        return;
                    }

                    let (kind, title, description) = match payment_result {
                        Ok(link) => match crate::external_url::open_payment_url(&link) {
                            Ok(()) => (
                                crate::toast::ToastKind::Success,
                                "Payment page opened",
                                "Continue securely in your browser.".into(),
                            ),
                            Err(error) => (
                                crate::toast::ToastKind::Error,
                                "Could not open payment",
                                error.to_string().into(),
                            ),
                        },
                        Err(error) => (
                            crate::toast::ToastKind::Error,
                            "Could not open payment",
                            error.to_string().into(),
                        ),
                    };
                    account.plans_status = None;
                    crate::toast::push_global(cx, kind, title, Some(description));
                    cx.notify();
                })
                .ok();
        })
        .detach();
    }

    fn copy_referral_code(&mut self, cx: &mut Context<Self>) {
        let Some(code) = self
            .account
            .read(cx)
            .referral
            .as_ref()
            .map(|referral| referral.referral_code.trim().to_owned())
        else {
            return;
        };
        if code.is_empty() {
            return;
        }
        cx.write_to_clipboard(gpui::ClipboardItem::new_string(code));
        crate::toast::push_global(
            cx,
            crate::toast::ToastKind::Success,
            "Referral code copied",
            Some("The Murglar referral code was copied to the clipboard.".into()),
        );
    }

    pub(super) fn render_murglar(&self, cx: &mut Context<Self>) -> impl IntoElement {
        let expanded_murglar_plan = self.expanded_murglar_plan.clone();
        let (
            identity_error,
            session_error,
            session_store_missing,
            signed_in,
            loading,
            status,
            device_limit,
            profile,
            referral,
            plans,
            extras_loading,
            referral_reloading,
            referral_status,
            plans_status,
        ) = {
            let account = self.account.read(cx);
            (
                match &account.identity {
                    DeviceIdentityStatus::Error(error) => Some(error.account_message()),
                    _ => None,
                },
                account.session_error.map(|error| error.to_string()),
                account.session_store.is_none(),
                account.token.is_some(),
                account.loading,
                account.status.clone(),
                account.device_limit_exceeded,
                account.profile.clone(),
                account.referral.clone(),
                account.plans.clone(),
                account.extras_loading,
                account.referral_reloading,
                account.referral_status.clone(),
                account.plans_status.clone(),
            )
        };
        let has_profile = profile.is_some();

        div()
            .flex()
            .flex_col()
            .gap(px(14.))
            .child(
                div()
                    .flex()
                    .items_center()
                    .justify_between()
                    .gap(px(12.))
                    .child(panel_heading(
                        "Murglar Account",
                        "Sign in and review your subscription status.",
                    ))
                    .when(signed_in, |this| {
                        this.child(murglar_status_button(
                            cx.listener(|this, _, _, cx| this.open_status_page(cx)),
                        ))
                    }),
            )
            .when(!signed_in, |this| {
                this.child(
                    settings_card()
                        .id("murglar-account-login")
                        .role(Role::Group)
                        .aria_label("Murglar account login")
                        .child(
                            div()
                                .flex()
                                .items_center()
                                .gap(px(8.))
                                .text_size(px(13.))
                                .font_weight(FontWeight::MEDIUM)
                                .child(local_icon(LocalIcon::UserLock, FOREGROUND).size_4())
                                .child("Account login"),
                        )
                        .child(
                            div()
                                .id("murglar-username-group")
                                .role(Role::Group)
                                .aria_label("Username or email")
                                .flex()
                                .flex_col()
                                .gap(px(6.))
                                .child(
                                    div()
                                        .flex()
                                        .items_center()
                                        .gap(px(6.))
                                        .text_size(px(12.))
                                        .text_color(rgb(MUTED))
                                        .child(local_icon(LocalIcon::User, MUTED).size_3())
                                        .child("Username or email"),
                                )
                                .child(
                                    text_field_context_menu(
                                        Input::new(&self.username)
                                            .content_type(InputContentType::Username),
                                        self.username.clone(),
                                    ),
                                ),
                        )
                        .child(
                            div()
                                .id("murglar-password-group")
                                .role(Role::Group)
                                .aria_label("Password")
                                .flex()
                                .flex_col()
                                .gap(px(6.))
                                .child(
                                    div()
                                        .flex()
                                        .items_center()
                                        .gap(px(6.))
                                        .text_size(px(12.))
                                        .text_color(rgb(MUTED))
                                        .child(local_icon(LocalIcon::Key, MUTED).size_3())
                                        .child("Password"),
                                )
                                .child(
                                    text_field_context_menu(
                                        Input::new(&self.password)
                                            .content_type(InputContentType::Password),
                                        self.password.clone(),
                                    ),
                                ),
                        )
                        .child(primary_button_with_loading(
                            "murglar-login",
                            Some(LocalIcon::LogIn),
                            "Log In to Murglar",
                            identity_error.is_some()
                                || session_store_missing
                                || session_error.is_some(),
                            loading,
                            cx.listener(|this, _, window, cx| {
                                this.submit_login(window, cx);
                            }),
                        ))
                        .when_some(identity_error, |this, message| {
                            this.child(
                                div()
                                    .text_size(px(12.))
                                    .text_color(rgb(DANGER))
                                    .child(message),
                            )
                        }),
                )
            })
            .when_some(status, |this, message| {
                this.child(
                    div()
                        .text_size(px(11.5))
                        .line_height(px(16.675))
                        .text_color(rgb(MUTED))
                        .child(message),
                )
            })
            .when_some(session_error, |this, message| {
                this.child(
                    div()
                        .text_size(px(11.5))
                        .line_height(px(16.675))
                        .text_color(rgb(DANGER))
                        .child(message),
                )
            })
            .when(device_limit, |this| {
                this.child(alert(
                    0xef444459,
                    0x7f1d1d38,
                    0xfecaca,
                    0xfca5a5,
                    "Murglar device limit reached",
                    "Your account already has five registered devices. Contact the Murglar developers to resolve the device registrations, then retry. ralgruM will keep using its existing saved device identity.",
                ))
            })
            .when(signed_in && loading, |this| {
                this.child(
                    settings_card()
                        .text_size(px(12.5))
                        .text_color(rgb(MUTED))
                        .child("Refreshing Murglar profile..."),
                )
                .when(!has_profile, |this| {
                    this.child(murglar_logout_button(loading, cx))
                })
            })
            .when_some(profile, |this, profile| {
                this.when(profile.pass_limit_exceeded, |this| {
                    this.child(alert(
                        0xf59e0b57,
                        0x78350f33,
                        0xfde68a,
                        0xfcd34d,
                        "Murglar Pass limit reached",
                        "Murglar reports that this account has reached its current Pass usage limit. High-quality or blocked-track requests may be rejected.",
                    ))
                })
                .child(profile_card(&profile))
                .child(murglar_logout_button(loading, cx))
            })
            .when(signed_in && extras_loading, |this| {
                this.child(extras_loading_card("Loading referral details…"))
                    .child(extras_loading_card("Loading available plans…"))
            })
            .when_some(referral, |this, referral| {
                this.child(murglar_referral::card(
                    &referral,
                    referral_reloading,
                    cx.listener(|this, _, _, cx| this.copy_referral_code(cx)),
                    cx.listener(|this, _, _, cx| this.refresh_murglar_referral(cx)),
                ))
            })
            .when_some(plans, |this, plans| {
                this.child(murglar_plans::card(
                    &plans,
                    expanded_murglar_plan.as_deref(),
                    cx,
                ))
            })
            .when_some(referral_status, |this, status| {
                this.child(
                    div()
                        .text_size(px(11.5))
                        .text_color(rgb(MUTED))
                        .child(status),
                )
            })
            .when_some(plans_status, |this, status| {
                this.child(
                    div()
                        .text_size(px(11.5))
                        .text_color(rgb(MUTED))
                        .child(status),
                )
            })
            .when(signed_in && !loading && !has_profile, |this| {
                this.child(
                    settings_card()
                        .text_size(px(12.5))
                        .child("Saved Murglar session"),
                )
                .child(murglar_logout_button(loading, cx))
            })
    }
}

async fn account_task_result<T>(
    task: JoinHandle<Result<T, AccountError>>,
) -> Result<T, AccountError> {
    task.await.unwrap_or(Err(AccountError::NetworkUnavailable))
}

pub(super) fn fallback(value: &str) -> &str {
    if value.trim().is_empty() {
        "N/A"
    } else {
        value
    }
}

fn alert(
    border_color: u32,
    background: u32,
    title_color: u32,
    body_color: u32,
    title: &'static str,
    body: &'static str,
) -> impl IntoElement {
    div()
        .flex()
        .flex_col()
        .gap(px(4.))
        .px(px(14.))
        .py(px(12.))
        .bg(rgba(background))
        .border_1()
        .border_color(rgba(border_color))
        .rounded(px(8.))
        .text_size(px(12.5))
        .line_height(px(18.75))
        .child(
            div()
                .text_color(rgb(title_color))
                .font_weight(FontWeight::SEMIBOLD)
                .child(title),
        )
        .child(div().text_color(rgb(body_color)).child(body))
}

fn profile_field(label: &'static str, value: impl Into<SharedString>) -> impl IntoElement {
    div()
        .flex()
        .items_center()
        .justify_between()
        .gap(px(12.))
        .child(
            div()
                .text_size(px(12.5))
                .text_color(rgb(MUTED))
                .child(label),
        )
        .child(
            div()
                .text_size(px(12.5))
                .font_weight(FontWeight::MEDIUM)
                .child(value.into()),
        )
}

fn profile_card(profile: &AccountProfile) -> impl IntoElement {
    div()
        .flex()
        .flex_col()
        .gap(px(7.))
        .p(px(14.))
        .bg(rgba(0x6366f117))
        .border_1()
        .border_color(rgba(0x6366f159))
        .rounded(px(8.))
        .child(
            div()
                .flex()
                .items_center()
                .justify_between()
                .gap(px(12.))
                .pb(px(5.))
                .child(
                    div()
                        .flex()
                        .items_center()
                        .gap(px(8.))
                        .text_size(px(13.))
                        .font_weight(FontWeight::SEMIBOLD)
                        .child(local_icon(LocalIcon::IdCard, FOREGROUND).size(px(13.)))
                        .child("Murglar profile"),
                )
                .child(
                    div()
                        .px(px(7.))
                        .py(px(3.))
                        .rounded(px(999.))
                        .border_1()
                        .border_color(rgba(0x818cf873))
                        .bg(rgba(0x6366f12e))
                        .text_color(rgb(0xc7d2fe))
                        .text_size(px(10.5))
                        .font_weight(FontWeight::SEMIBOLD)
                        .child(profile.pass_status.label()),
                ),
        )
        .child(profile_field("Username", fallback(&profile.username)))
        .child(profile_field("Email", fallback(&profile.email)))
        .child(profile_field(
            "Permanent Premium",
            profile.permanent_premium_status,
        ))
        .child(profile_field(
            "Pass expiration",
            profile.pass_expiration_display.clone(),
        ))
}

fn murglar_logout_button(loading: bool, cx: &mut Context<SettingsView>) -> impl IntoElement {
    div().w_full().flex().child(danger_secondary_button(
        "murglar-logout",
        "Log out",
        loading,
        DangerSecondaryButtonOptions::SERVICE,
        cx.listener(|this, _, window, cx| {
            this.logout(window, cx);
        }),
    ))
}

fn extras_loading_card(copy: &'static str) -> impl IntoElement {
    settings_card()
        .text_size(px(12.5))
        .text_color(rgb(MUTED))
        .child(copy)
}

fn murglar_status_button(
    on_click: impl Fn(&gpui::ClickEvent, &mut Window, &mut gpui::App) + 'static,
) -> impl IntoElement {
    div()
        .id("murglar-status")
        .group("murglar-status")
        .focusable()
        .tab_stop(true)
        .role(Role::Button)
        .aria_label("Open Murglar service status")
        .size(px(34.))
        .mr(px(4.))
        .flex_none()
        .flex()
        .items_center()
        .justify_center()
        .rounded(px(6.))
        .border_1()
        .border_color(rgba(0x00000000))
        .bg(rgba(0x00000000))
        .cursor_pointer()
        .hover(|style| {
            style
                .bg(rgb(BORDER))
                .border_color(rgb(BORDER))
                .text_color(rgb(FOREGROUND))
        })
        .app_tooltip("Open Murglar service status")
        .focus_visible(|style| style.border_color(rgb(crate::theme::PRIMARY)))
        .child(
            div()
                .relative()
                .size(px(14.5))
                .child(
                    div()
                        .absolute()
                        .inset_0()
                        .child(local_icon(LocalIcon::Signal, MUTED).size_full()),
                )
                .child(
                    div()
                        .absolute()
                        .inset_0()
                        .invisible()
                        .group_hover("murglar-status", |style| style.visible())
                        .child(local_icon(LocalIcon::Signal, FOREGROUND).size_full()),
                ),
        )
        .on_click(on_click)
}

#[cfg(test)]
mod tests {
    use super::*;
    use tokio::runtime::Runtime;

    #[test]
    fn tokio_account_task_can_be_spawned_without_entering_runtime_context() {
        let runtime = Runtime::new().unwrap();
        let task = runtime.spawn(async { Ok::<_, AccountError>("ready") });

        assert_eq!(runtime.block_on(account_task_result(task)), Ok("ready"));
    }
}
