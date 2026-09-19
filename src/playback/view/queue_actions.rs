use super::*;

impl PlaybackModel {
    /// Queues a batch of tracks next or last. With nothing playing the batch
    /// becomes the queue, matching addToQueue in the original app.
    pub(crate) fn enqueue_tracks(
        &mut self,
        additions: Vec<PlaybackTrack>,
        last: bool,
        cx: &mut Context<Self>,
    ) {
        if additions.is_empty() {
            return;
        }
        if self.state.current_index.is_none() {
            self.replace_queue(additions, 0, cx);
            return;
        }
        let should_play = self.state.status == PlaybackStatus::Ended;
        let count = self.state.insert_queue_additions(additions, last);
        self.reset_extension_state();
        crate::toast::push_global(
            cx,
            crate::toast::ToastKind::Success,
            if last {
                "Added to end of queue"
            } else {
                "Playing next"
            },
            Some(
                format!(
                    "{count} {} added.",
                    if count == 1 { "track" } else { "tracks" }
                )
                .into(),
            ),
        );
        if should_play {
            self.next(cx);
        } else {
            cx.notify();
        }
    }

    /// Supplies a read-only probe that resolves a track's format and size
    /// through the playback resolver without starting playback.
    pub(crate) fn track_info_probe(&self, cx: &gpui::App) -> Option<super::super::TrackInfoProbe> {
        let account = self.account.read(cx);
        Some(super::super::TrackInfoProbe::new(
            self.resolver.clone().ok()?,
            self.runtime.clone(),
            account.deezer_arl(),
            account.soundcloud_token(),
            account.murglar_media_credentials(),
        ))
    }

    pub(crate) fn toggle_queue(&mut self, cx: &mut Context<Self>) {
        self.state.toggle_sidebar(RightSidebar::Queue);
        cx.notify();
    }

    pub(crate) fn toggle_shuffle(&mut self, cx: &mut Context<Self>) {
        self.state.toggle_shuffle();
        cx.notify();
    }

    pub(crate) fn cycle_repeat(&mut self, cx: &mut Context<Self>) {
        self.state.cycle_repeat_mode();
        cx.notify();
    }

    pub(crate) fn playback_preferences(
        &self,
    ) -> (f32, bool, super::super::state::RepeatMode, bool) {
        (
            self.state.persisted_volume(),
            self.state.muted,
            self.state.repeat_mode,
            self.state.shuffle_enabled,
        )
    }

    fn remove(&mut self, index: usize, cx: &mut Context<Self>) {
        if self.state.remove(index) {
            self.reset_extension_state();
            cx.notify();
        }
    }

    pub(crate) fn select_from_queue(&mut self, index: usize, cx: &mut Context<Self>) {
        self.select(index, cx);
    }

    pub(crate) fn remove_from_queue(&mut self, index: usize, cx: &mut Context<Self>) {
        self.remove(index, cx);
    }

    pub(crate) fn enqueue_next(&mut self, index: usize, cx: &mut Context<Self>) {
        if self.state.enqueue_next(index) {
            if self.state.status == PlaybackStatus::Ended {
                self.next(cx);
            } else {
                cx.notify();
            }
        }
    }

    pub(crate) fn enqueue_last(&mut self, index: usize, cx: &mut Context<Self>) {
        if self.state.enqueue_last(index) {
            if self.state.status == PlaybackStatus::Ended {
                self.next(cx);
            } else {
                cx.notify();
            }
        }
    }

    pub(crate) fn enqueue_track(
        &mut self,
        track: PlaybackTrack,
        last: bool,
        cx: &mut Context<Self>,
    ) {
        if self.state.current_index.is_none() {
            self.replace_queue(vec![track], 0, cx);
            return;
        }
        let should_play = self.state.status == PlaybackStatus::Ended;
        self.state.insert_queue_additions(vec![track], last);
        self.reset_extension_state();
        if should_play {
            self.next(cx);
        } else {
            cx.notify();
        }
    }

    pub(crate) fn reorder(&mut self, from: usize, to: usize, cx: &mut Context<Self>) {
        if self.state.reorder(from, to) {
            self.reset_extension_state();
            cx.notify();
        }
    }
}
