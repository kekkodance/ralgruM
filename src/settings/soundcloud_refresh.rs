use std::time::Duration;

use chrono::Utc;
use gpui::Context;

use crate::soundcloud_oauth::{self, SoundCloudRefreshCompletion};

/// Let startup work settle before the first token check, then poll often
/// enough that the proactive refresh margin is honored without spinning.
const INITIAL_REFRESH_DELAY: Duration = Duration::from_secs(3);
const REFRESH_CHECK_INTERVAL: Duration = Duration::from_secs(5 * 60);

impl super::SettingsView {
    /// Runs for the lifetime of the settings view, which is the lifetime of
    /// the app. A round refreshes only the tokens that entered their expiry
    /// margin, so an idle account performs no network work.
    pub(crate) fn start_soundcloud_refresh(&self, cx: &mut Context<Self>) {
        cx.spawn(async move |this, cx| {
            cx.background_executor().timer(INITIAL_REFRESH_DELAY).await;
            loop {
                let Some((client, runtime, account, request)) = this
                    .update(cx, |this, cx| {
                        let request = this
                            .account
                            .update(cx, |account, _| account.begin_soundcloud_refresh());
                        (
                            this.http_client.clone(),
                            this.runtime.clone(),
                            this.account.downgrade(),
                            request,
                        )
                    })
                    .ok()
                else {
                    return;
                };
                if let Some((generation, request)) = request {
                    let now = Utc::now().timestamp();
                    let completion = runtime
                        .spawn(async move {
                            soundcloud_oauth::refresh_tokens(&client, &request, now).await
                        })
                        .await
                        .unwrap_or_else(|error| {
                            SoundCloudRefreshCompletion::failed(format!(
                                "The refresh task failed: {error}"
                            ))
                        });
                    let _ = account.update(cx, |account, cx| {
                        account.complete_soundcloud_refresh(generation, completion, cx);
                        cx.notify();
                    });
                }
                cx.background_executor().timer(REFRESH_CHECK_INTERVAL).await;
            }
        })
        .detach();
    }
}
