use std::collections::{HashMap, HashSet};

use super::*;

impl PlaybackModel {
    /// Classify every Deezer album represented in the live queue. Playback
    /// owns this request so queues created outside Search receive the same
    /// metadata and blocking behavior as search-result queues.
    pub(crate) fn refresh_deezer_ai_content(&mut self, cx: &mut Context<Self>) {
        if let Some(abort) = self.ai_enrichment_abort.take() {
            abort.abort();
        }

        let mut seen = HashSet::new();
        let album_ids = self
            .state
            .queue
            .iter()
            .filter(|track| track.provider == PlaybackProvider::Deezer)
            .map(|track| track.album_id.trim())
            .filter(|id| {
                !id.is_empty()
                    && id.bytes().all(|byte| byte.is_ascii_digit())
                    && seen.insert((*id).to_owned())
            })
            .map(str::to_owned)
            .collect::<Vec<_>>();
        if album_ids.is_empty() {
            return;
        }

        let Some(arl) = self.account.read(cx).deezer_arl() else {
            self.warn_ai_lookup_unavailable(
                "Sign in to Deezer so ralgruM can identify AI generated tracks.",
                cx,
            );
            return;
        };
        let Ok(client) = self.ai_client.clone() else {
            self.warn_ai_lookup_unavailable("The AI metadata client could not be created.", cx);
            return;
        };

        self.ai_enrichment_id = self.ai_enrichment_id.wrapping_add(1);
        let request_id = self.ai_enrichment_id;
        let task = self
            .runtime
            .spawn(async move { client.deezer_ai_content(album_ids, arl).await });
        self.ai_enrichment_abort = Some(task.abort_handle());
        cx.spawn(async move |this, cx| {
            let result = task.await;
            this.update(cx, |this, cx| {
                if this.ai_enrichment_id != request_id {
                    return;
                }
                this.ai_enrichment_abort = None;
                match result {
                    Ok(Ok(albums)) => {
                        this.ai_warning_shown = false;
                        this.apply_deezer_ai_content(&albums, cx);
                    }
                    Ok(Err(error)) => {
                        this.warn_ai_lookup_unavailable(error.message.as_str(), cx);
                    }
                    Err(_) => {}
                }
            })
            .ok();
        })
        .detach();
    }

    /// Apply metadata from any producer and immediately enforce the active
    /// blocking preference if the current track changed classification.
    pub(crate) fn apply_deezer_ai_content(
        &mut self,
        albums: &HashMap<String, bool>,
        cx: &mut Context<Self>,
    ) {
        if !self.state.apply_deezer_ai_content(albums) {
            return;
        }
        if self
            .state
            .current()
            .is_some_and(|track| self.state.content_blocked(track))
        {
            self.next(cx);
        } else {
            cx.notify();
        }
    }

    fn warn_ai_lookup_unavailable(&mut self, detail: &str, cx: &mut Context<Self>) {
        if !self.state.block_ai || self.ai_warning_shown {
            return;
        }
        self.ai_warning_shown = true;
        crate::toast::push_global(
            cx,
            crate::toast::ToastKind::Warning,
            "AI content blocking unavailable",
            Some(detail.to_owned().into()),
        );
    }
}

impl Drop for PlaybackModel {
    fn drop(&mut self) {
        if let Some(abort) = self.ai_enrichment_abort.take() {
            abort.abort();
        }
    }
}
