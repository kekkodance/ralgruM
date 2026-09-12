use std::time::Duration;

use gpui::Context;

use super::{RightSidebarView, SettingsView};

const SAVE_DELAY: Duration = Duration::from_millis(250);

impl SettingsView {
    pub(super) fn install_runtime_settings_flush(&self, cx: &mut Context<Self>) {
        cx.on_app_quit(|this, _| {
            this.pending_runtime_save.take();
            let task = this.store.as_ref().map(|store| {
                let write = store.prepare_persist(this.saved.clone());
                this.runtime.spawn_blocking(move || write.persist())
            });
            async move {
                if let Some(task) = task
                    && let Ok(Err(error)) = task.await
                {
                    crate::diagnostics::event(
                        "WARN",
                        format!("Could not save final preferences: {error}"),
                    );
                }
            }
        })
        .detach();
    }

    pub(crate) fn persist_runtime_state(
        &mut self,
        preferences: (f32, bool, crate::playback::RepeatMode, bool),
        sidebar: (bool, RightSidebarView),
        cx: &mut Context<Self>,
    ) {
        if self.import_sync_pending {
            return;
        }
        let Some(store) = self.store.as_ref() else {
            return;
        };
        let mut settings = self.saved.clone();
        let volume_changed = settings.apply_volume_preferences(preferences.0, preferences.1);
        let modes_changed = settings.apply_playback_modes(preferences.2, preferences.3);
        let sidebar_changed =
            settings.right_sidebar_open != sidebar.0 || settings.right_sidebar_view != sidebar.1;
        settings.right_sidebar_open = sidebar.0;
        settings.right_sidebar_view = sidebar.1;
        if !volume_changed && !modes_changed && !sidebar_changed {
            return;
        }
        let write = store.prepare_persist(settings.clone());
        self.saved = settings.clone();
        if !self.session_active {
            self.draft = settings;
        }
        let runtime = self.runtime.clone();
        let executor = cx.background_executor().clone();
        self.pending_runtime_save = Some(cx.spawn(async move |this, cx| {
            executor.timer(SAVE_DELAY).await;
            let worker_write = write.clone();
            let result = runtime.spawn_blocking(move || worker_write.persist()).await;
            let error = match result {
                Ok(Ok(_)) => None,
                Ok(Err(error)) => Some(error.to_string()),
                Err(_) => Some("The settings writer stopped unexpectedly.".to_owned()),
            };
            let _ = this.update(cx, |this, cx| {
                if write.is_current() {
                    if let Some(error) = error {
                        this.save_error = Some(error.into());
                        cx.notify();
                    } else if let Some(store) = this.store.as_mut() {
                        store.accept_persisted(&write);
                    }
                }
            });
        }));
    }
}
