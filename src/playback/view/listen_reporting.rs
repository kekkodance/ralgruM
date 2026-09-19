use super::*;

impl PlaybackModel {
    pub(super) fn record_deezer_listen(
        &self,
        resolver: &StreamResolver,
        payload: serde_json::Value,
        arl: crate::search::DeezerArl,
        completed_listen: bool,
        cx: &mut Context<Self>,
    ) {
        let resolver = resolver.clone();
        let history_changed = self.listen_history_changed.clone();
        let entity = cx.entity().clone();
        let account_scope = self.account.read(cx).library_scope();
        let task = self
            .runtime
            .spawn(async move { resolver.record_deezer_listen(payload, arl).await });
        cx.spawn(async move |_, cx| {
            let request_succeeded = task.await.is_ok_and(|succeeded| succeeded);
            if listen_history_changed_on_completion(completed_listen, request_succeeded) {
                entity.update(cx, |this, cx| {
                    if !listen_history_scope_matches(
                        &account_scope,
                        &this.account.read(cx).library_scope(),
                    ) {
                        return;
                    }
                    history_changed.mark(PlaybackProvider::Deezer);
                    cx.notify();
                });
            }
        })
        .detach();
    }

    pub(super) fn begin_listen_reporting(
        &mut self,
        resolver: &StreamResolver,
        track: &PlaybackTrack,
        deezer_arl: Option<crate::search::DeezerArl>,
        cx: &mut Context<Self>,
    ) {
        match track.provider {
            PlaybackProvider::Deezer if self.record_deezer_plays => {
                let Some(arl) = deezer_arl else {
                    return;
                };
                self.deezer_listen = Some(DeezerListenSession::start(
                    track.id.clone(),
                    self.state.shuffle_enabled,
                    self.state.status == PlaybackStatus::Playing,
                ));
                self.record_deezer_listen(resolver, deezer_next_media(&track.id), arl, false, cx);
            }
            PlaybackProvider::SoundCloud => {
                let Some(report) =
                    SoundCloudListenReport::from_playback(&self.state.context, &track.id)
                else {
                    return;
                };
                let Some(token) = self.account.read(cx).soundcloud_mobile_token() else {
                    return;
                };
                self.record_soundcloud_listen(resolver, report, token, cx);
            }
            _ => {}
        }
    }

    pub(super) fn record_soundcloud_listen(
        &self,
        resolver: &StreamResolver,
        report: SoundCloudListenReport,
        token: crate::search::SoundCloudToken,
        cx: &mut Context<Self>,
    ) {
        let resolver = resolver.clone();
        let history_changed = self.listen_history_changed.clone();
        let entity = cx.entity().clone();
        let account_scope = self.account.read(cx).library_scope();
        let task = self
            .runtime
            .spawn(async move { resolver.record_soundcloud_listen(report, token).await });
        cx.spawn(async move |_, cx| {
            if task.await.is_ok_and(|succeeded| succeeded) {
                entity.update(cx, |this, cx| {
                    if !listen_history_scope_matches(
                        &account_scope,
                        &this.account.read(cx).library_scope(),
                    ) {
                        return;
                    }
                    history_changed.mark(PlaybackProvider::SoundCloud);
                    cx.notify();
                });
            }
        })
        .detach();
    }

    pub(super) fn finish_deezer_listen(&mut self, cx: &mut Context<Self>) {
        let Some(session) = self.deezer_listen.take() else {
            return;
        };
        if !self.record_deezer_plays {
            return;
        }
        let Some(arl) = self.account.read(cx).deezer_arl() else {
            return;
        };
        let Ok(resolver) = self.resolver.clone() else {
            return;
        };
        self.record_deezer_listen(&resolver, session.finish(), arl, true, cx);
    }
}
