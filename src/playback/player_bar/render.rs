use super::*;

impl Render for PlaybackView {
    fn render(&mut self, window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let viewport = window.viewport_size();
        let metrics = crate::music_ui::shell_metrics_for_viewport(
            f32::from(viewport.width),
            f32::from(viewport.height),
        );
        let (
            current,
            status,
            position,
            duration,
            buffered,
            volume,
            generation,
            seek_commit_epoch,
            shuffle_enabled,
            repeat_mode,
            right_sidebar,
            loading_from_cache,
            player_bar_open,
            pending_queue_load,
        ) = {
            let model = self.model.read(cx);
            let state = &model.state;
            (
                state.current().cloned(),
                state.status,
                state.position,
                state.duration,
                state.buffered,
                state.volume,
                state.generation,
                model.seek_commit_epoch(),
                state.shuffle_enabled,
                state.repeat_mode,
                state.right_sidebar,
                model.loading_from_cache(),
                state.player_bar_open(),
                state.pending_queue_load(),
            )
        };
        // Loading a SmartMix queue begins before an audio track exists. Keep
        // rendering that request as a loading player even if a transport
        // lifecycle update temporarily reports Empty.
        let status = if pending_queue_load && current.is_none() {
            PlaybackStatus::Loading
        } else {
            status
        };
        let seek_control_enabled =
            matches!(status, PlaybackStatus::Playing | PlaybackStatus::Paused);
        if self.seek_control_enabled != Some(seek_control_enabled) {
            self.seek_control_enabled = Some(seek_control_enabled);
            self.model.update(cx, |model, _| {
                model.set_seek_slider_enabled(seek_control_enabled);
            });
        }
        if !seek_control_enabled && self.seek_pointer_state.get().active {
            self.seek_pointer_state.set(SeekPointerState::default());
            window.release_pointer();
        }
        let narrow = metrics.narrow_content;
        let compact = metrics.compact_player;
        let open = player_bar_open;
        let quality = if open {
            self.model.read(cx).resolved_quality().map(str::to_owned)
        } else {
            None
        };
        let previous_quality_label = self.last_quality_label.clone();
        if !open {
            self.last_quality_label.clear();
            self.last_quality_generation = None;
        } else {
            update_quality_label_for_generation(
                &mut self.last_quality_label,
                &mut self.last_quality_generation,
                generation,
                quality.as_deref(),
            );
        }
        let quality_label = self.last_quality_label.clone();
        let quality_text_opacity =
            quality_text_opacity_endpoints(&previous_quality_label, &quality_label);
        let quality_badge_opacity =
            quality_badge_opacity_endpoints(&previous_quality_label, &quality_label);
        let now = Instant::now();
        let viewport_width = f32::from(viewport.width).max(0.);
        let layout =
            PlayerBarLayout::from_geometry(desktop_player_geometry(viewport_width, compact));
        let layout_visual =
            self.player_bar_layout_motion
                .prepare(compact, layout, now, cx.reduce_motion());
        let (padding, column_gap, center_width, side_width) = layout.geometry();
        if status == PlaybackStatus::Empty {
            if self.volume_pointer_state.get().is_active() {
                self.volume_pointer_state.set(VolumePointerState::default());
                window.release_pointer();
            }
            self.buffered_motion = BufferedMotion::default();
            self.seek_fill_motion.reset(seek_commit_epoch);
            self.last_seek_commit_epoch = seek_commit_epoch;
            self.last_display_progress = 0.;
            return render_player_bar_host(narrow, false, None);
        }
        let favorite_key = current_favorite_key(current.as_ref());
        let (favorite, favorite_pending) = favorite_key.as_ref().map_or((None, false), |key| {
            let favorites = self.favorites.read(cx);
            (favorites.favorite(key), favorites.pending(key))
        });
        let progress = if duration.is_zero() {
            0.0
        } else {
            position.as_secs_f32() / duration.as_secs_f32()
        };
        let seek_preview = self.model.read(cx).seek_preview_fraction();
        let display_progress = seekbar_display_progress(progress, seek_preview);
        let seek_fill_visual = if let Some(preview) = seek_preview {
            self.seek_fill_motion.set_displayed(preview);
            self.last_display_progress = preview;
            SeekFillVisual::direct(preview)
        } else {
            if seek_commit_epoch != self.last_seek_commit_epoch {
                self.last_seek_commit_epoch = seek_commit_epoch;
            }
            let visual = self.seek_fill_motion.prepare(
                seek_commit_epoch,
                self.last_display_progress,
                progress,
                now,
                cx.reduce_motion(),
            );
            let visual = if visual.active {
                visual
            } else {
                SeekFillVisual::direct(progress)
            };
            self.last_display_progress = if visual.active {
                self.seek_fill_motion.displayed_at(now)
            } else {
                progress
            };
            visual
        };
        let display_position = seek_preview
            .map(|_| duration.mul_f32(display_progress))
            .unwrap_or(position);
        let buffered_fraction = if duration.is_zero() {
            0.0
        } else {
            (buffered.as_secs_f32() / duration.as_secs_f32()).clamp(0.0, 1.0)
        };
        let buffered_visual =
            self.buffered_motion
                .prepare(generation, buffered_fraction, now, cx.reduce_motion());
        if self.seek_slider.read(cx).value() != SliderValue::Single(display_progress) {
            self.seek_slider.update(cx, |slider, cx| {
                slider.set_value(display_progress, window, cx);
            });
        }
        if self.volume_slider.read(cx).value() != SliderValue::Single(volume) {
            self.volume_slider.update(cx, |slider, cx| {
                slider.set_value(volume, window, cx);
            });
        }
        let volume_motion_mode = volume_motion_mode_for_render(&self.volume_pointer_state);
        let volume_visual =
            self.volume_motion
                .prepare(volume, now, cx.reduce_motion(), volume_motion_mode);
        let volume_displayed_value = self.volume_motion.displayed_at(now);
        let play_enabled = matches!(
            status,
            PlaybackStatus::Playing
                | PlaybackStatus::Paused
                | PlaybackStatus::Ended
                | PlaybackStatus::Loading
        );
        // During a preloaded track transition the status briefly reports
        // Loading while playback continues. Keep the seek bar gated on the
        // settled states so only the transport button stays continuous.
        let seek_bar_enabled = matches!(
            status,
            PlaybackStatus::Playing | PlaybackStatus::Paused | PlaybackStatus::Ended
        );
        let next_enabled = self.model.read(cx).state.can_next();
        let previous_enabled = current.is_some();
        let download_track = current.clone();
        let downloads_for_button = self.downloads.clone();
        let account_for_button = self.account.clone();
        let download_button = move |cx: &mut Context<Self>, interactive: Rc<Cell<bool>>| {
            if let Some(track) = download_track.clone() {
                let listener_interactive = interactive.clone();
                let menu_interactive = interactive.clone();
                let button = div()
                    .id("player-download")
                    .group("player-download")
                    .hover(|style| style.bg(rgb(BORDER)))
                    .size(px(34.))
                    .rounded(px(PLAYER_ACTION_BUTTON_RADIUS_PX))
                    .flex()
                    .items_center()
                    .justify_center()
                    .cursor_pointer()
                    .app_tooltip("Download")
                    .child(hover_icon(
                        "player-download",
                        LocalIcon::Download.path(),
                        14.5,
                        MUTED,
                        FOREGROUND,
                    ))
                    .on_click(move |_, _, cx| {
                        if listener_interactive.get() {
                            cx.stop_propagation();
                        }
                    });
                // The player bar sits at the window bottom, so open the format
                // menu above the button with its bottom edge against the button
                // top edge and a chevron pointing down to the button.
                // While the layout motion runs, keep the plain button so a
                // click cannot open the menu mid transition.
                if menu_interactive.get() {
                    context_menu::track_download_button_above(
                        button,
                        track,
                        downloads_for_button.clone(),
                        account_for_button.clone(),
                    )
                    .into_any_element()
                } else {
                    button.into_any_element()
                }
            } else {
                let listener_interactive = interactive.clone();
                bare_action_button_with_interactivity(
                    "player-download",
                    LocalIcon::Download.path(),
                    false,
                    None,
                    !download_available(None),
                    true,
                    "Download",
                    {
                        let listener = cx.listener(|this, _, _, cx| {
                            cx.stop_propagation();
                            this.download_current(cx);
                        });
                        move |event, window, cx| {
                            if listener_interactive.get() {
                                listener(event, window, cx)
                            }
                        }
                    },
                )
            }
        };
        let lyrics_button = || {
            let model = self.model.clone();
            let selected = right_sidebar == RightSidebar::Lyrics;
            bare_action_button(
                "lyrics-toggle",
                LocalIcon::QuoteRight.path(),
                selected,
                None,
                current.is_none(),
                "Lyrics",
                move |_, _, cx| model.update(cx, |model, cx| model.toggle_lyrics(cx)),
            )
        };
        let queue_button = || {
            let model = self.model.clone();
            let selected = right_sidebar == RightSidebar::Queue;
            bare_action_button(
                "queue-toggle",
                LocalIcon::ListUl.path(),
                selected,
                None,
                current.is_none(),
                "Queue",
                move |_, _, cx| model.update(cx, |model, cx| model.toggle_queue(cx)),
            )
        };

        let artist_navigation = current.as_ref().and_then(|track| {
            current_artist_navigation(track, status, self.external_track_navigation.as_ref())
        });
        let title_for_motion = current_track_title(current.as_ref(), status);
        let subtitle_for_motion = current_subtitle(current.as_ref(), status, loading_from_cache);
        let rendered_artist_for_motion = rendered_current_artist_text(
            current.as_ref(),
            status,
            subtitle_for_motion,
            artist_navigation.as_ref(),
        );
        // The track's flags are known from the moment it is queued, so the
        // width math sees the badges during loading too.
        let show_explicit_badge = current.as_ref().is_some_and(|track| track.explicit);
        let show_ai_badge = current.as_ref().is_some_and(|track| track.ai_generated);
        let intrinsic_text_width = (!narrow && favorite_key.is_some()).then(|| {
            current_text_intrinsic_width(
                window,
                title_for_motion,
                &rendered_artist_for_motion,
                show_explicit_badge,
                show_ai_badge,
            )
        });
        let text_width_visual = intrinsic_text_width.map(|intrinsic| {
            let target = current_text_width_for_layout(layout, compact, true, intrinsic);
            self.text_width_motion.prepare(
                target,
                now,
                layout_visual.active,
                layout_visual.mode_changed,
                cx.reduce_motion(),
            )
        });
        let favorite_visual = intrinsic_text_width.map(|intrinsic| {
            let target = RightControlGeometry {
                left: favorite_left_for_layout(layout, compact, intrinsic),
                top: favorite_top(compact),
                width: 34.,
                height: 34.,
            };
            self.favorite_motion.prepare(
                compact,
                target,
                now,
                layout_visual.active,
                cx.reduce_motion(),
            )
        });
        let quality_width = quality_badge_width(window, quality_label.as_str());
        let quality_target = right_control_geometry(
            RightControlKind::Quality,
            layout,
            viewport_width,
            compact,
            quality_width,
        );
        let quality_visual = self.quality_motion.prepare(
            compact,
            quality_target,
            now,
            layout_visual.active,
            cx.reduce_motion(),
        );
        let right_control_visuals = std::array::from_fn(|index| {
            let kind = match index {
                0 => RightControlKind::Quality,
                1 => RightControlKind::Lyrics,
                2 => RightControlKind::Queue,
                _ => RightControlKind::Volume,
            };
            let target =
                right_control_geometry(kind, layout, viewport_width, compact, quality_width);
            if kind == RightControlKind::Quality {
                RectMotionVisual {
                    from: target,
                    target,
                    ..RectMotionVisual::default()
                }
            } else {
                self.right_control_motion[index].prepare(
                    target,
                    now,
                    layout_visual.active,
                    layout_visual.mode_changed,
                    cx.reduce_motion(),
                )
            }
        });
        let download_target = download_position(
            window,
            viewport_width,
            layout,
            compact,
            quality_label.as_str(),
        );
        let download_motion_visual = self.download_motion.prepare(
            compact,
            download_target,
            now,
            layout_visual.active,
            cx.reduce_motion(),
        );
        let download_visual = (!narrow).then_some(download_motion_visual);
        let download_interactive = Rc::new(Cell::new(!download_motion_visual.active));
        let favorite_button = favorite_key.is_some().then(|| {
            let (feedback_start, feedback_end) = favorite_feedback_opacity(favorite_pending);
            let interactive = Rc::new(Cell::new(
                favorite_visual.is_none_or(|visual| !visual.active),
            ));
            let listener_interactive = interactive.clone();
            let button = bare_action_button_with_interactivity(
                "player-favorite",
                LocalIcon::Heart.path(),
                favorite == Some(true),
                Some(FAVORITE_PINK),
                favorite_pending,
                true,
                "Favorite Track",
                {
                    let listener = cx.listener(|this, _, _, cx| {
                        cx.stop_propagation();
                        this.toggle_current_favorite(cx);
                    });
                    move |event, window, cx| {
                        if listener_interactive.get() {
                            listener(event, window, cx)
                        }
                    }
                },
            );
            let button = div()
                .size(px(34.))
                .flex_none()
                .child(button)
                .with_animation(
                    format!(
                        "player-favorite-feedback-{}-{favorite_pending}",
                        favorite == Some(true)
                    ),
                    crate::motion::interaction(),
                    move |this, delta| {
                        this.opacity(crate::motion::lerp(feedback_start, feedback_end, delta))
                    },
                )
                .into_any_element();
            (button, interactive)
        });
        let favorite_interactive = favorite_button
            .as_ref()
            .map(|(_, interactive)| interactive.clone());
        let favorite_button = favorite_button.map(|(button, _)| button);
        let volume_offset_visual = self.volume_offset_motion.prepare(
            volume_container_offset_target(!compact),
            now,
            layout_visual.active,
            layout_visual.mode_changed,
            cx.reduce_motion(),
        );

        let current_width_animation = (!narrow && layout_visual.active).then(|| {
            (
                current_block_width(layout_visual.from),
                current_block_width(layout),
                layout_visual.epoch,
            )
        });

        let current_block = match current.as_ref() {
            None => render_current(
                None,
                &self.artwork_hold,
                status,
                loading_from_cache,
                (!narrow).then_some(layout),
                compact,
                narrow,
                favorite_button,
                current_width_animation,
                None,
                text_width_visual,
                favorite_visual,
                favorite_interactive.clone(),
                window,
            ),
            Some(track) => {
                let rendered = render_current(
                    Some(track),
                    &self.artwork_hold,
                    status,
                    loading_from_cache,
                    (!narrow).then_some(layout),
                    compact,
                    narrow,
                    favorite_button,
                    current_width_animation,
                    artist_navigation,
                    text_width_visual,
                    favorite_visual,
                    favorite_interactive,
                    window,
                );
                context_menu::current_menu(
                    div().id("current-track-context").child(rendered),
                    track.clone(),
                    self.model.clone(),
                    self.downloads.clone(),
                    self.account.clone(),
                    self.library.clone(),
                    self.external_track_navigation.clone(),
                )
            }
        };

        let download_for_center = narrow.then(|| download_button(cx, download_interactive.clone()));
        let download_for_desktop =
            (!narrow).then(|| download_button(cx, download_interactive.clone()));

        let center = div()
            .when(narrow, |this| this.w_full())
            .when(!narrow, |this| this.w(px(center_width)).flex_none())
            .min_w_0()
            .flex()
            .flex_col()
            .items_center()
            .gap(px(6.))
            .child(
                div()
                    .flex()
                    .items_center()
                    .gap(px(if narrow { 8. } else { 16. }))
                    // The mobile controls share this row with the extra action
                    // buttons, matching the original mobile footer markup.
                    .when(narrow, |this| {
                        this.child(animated_quality_badge(
                            generation,
                            quality_label.as_str(),
                            260.,
                            quality_text_opacity,
                            quality_badge_opacity,
                        ))
                        .children(download_for_center)
                    })
                    .child(transport_button(
                        "shuffle",
                        LocalIcon::Shuffle.path(),
                        shuffle_enabled,
                        current.is_some(),
                        14.,
                        "Shuffle",
                        {
                            let model = self.model.clone();
                            move |_, _, cx| model.update(cx, |model, cx| model.toggle_shuffle(cx))
                        },
                    ))
                    .child(transport_button(
                        "previous",
                        LocalIcon::Previous.path(),
                        false,
                        previous_enabled,
                        12.,
                        "Previous",
                        {
                            let model = self.model.clone();
                            move |_, _, cx| model.update(cx, |model, cx| model.previous(cx))
                        },
                    ))
                    .child(play_pause_button(
                        matches!(status, PlaybackStatus::Playing | PlaybackStatus::Loading),
                        play_enabled,
                        {
                            let model = self.model.clone();
                            move |_, _, cx| model.update(cx, |model, cx| model.toggle(cx))
                        },
                    ))
                    .child(transport_button(
                        "next",
                        LocalIcon::Next.path(),
                        false,
                        next_enabled,
                        12.,
                        "Next",
                        {
                            let model = self.model.clone();
                            move |_, _, cx| {
                                if model.read(cx).state.can_next() {
                                    model.update(cx, |model, cx| model.next(cx));
                                }
                            }
                        },
                    ))
                    .child(repeat_button(repeat_mode, current.is_some(), {
                        let model = self.model.clone();
                        move |_, _, cx| model.update(cx, |model, cx| model.cycle_repeat(cx))
                    }))
                    .when(narrow, |this| {
                        this.child(lyrics_button()).child(queue_button())
                    }),
            )
            .child(
                div()
                    .w_full()
                    .flex()
                    .items_center()
                    .gap(px(if narrow { 8. } else { 10. }))
                    .child(time_text(display_position))
                    .child(progress_control(
                        self.model.clone(),
                        self.seek_pointer_state.clone(),
                        buffered_visual,
                        seek_fill_visual,
                        seek_bar_enabled,
                        cx,
                    ))
                    .child(time_text(duration)),
            );

        let center = if !narrow && layout_visual.active {
            let center_layout_visual = layout_visual;
            center
                .with_animation(
                    ("player-center-layout", layout_visual.epoch),
                    crate::motion::panel(),
                    move |this, delta| {
                        let width = center_layout_visual.at(delta).center_width;
                        this.w(px(width)).min_w(px(width))
                    },
                )
                .into_any_element()
        } else {
            center.into_any_element()
        };

        let right = if narrow {
            div()
                .w_full()
                .flex()
                .items_center()
                .justify_end()
                .gap(px(12.))
                .into_any_element()
        } else {
            let volume_level = self.model.read(cx).state.volume_icon_level();
            let volume = volume_controls(
                self.model.clone(),
                self.volume_slider.clone(),
                self.volume_pointer_state.clone(),
                volume_level,
                volume_visual,
                volume_displayed_value,
                volume_offset_visual,
            );
            player_right_controls(
                side_width,
                generation,
                quality_label.as_str(),
                quality_text_opacity,
                quality_badge_opacity,
                quality_visual,
                lyrics_button(),
                queue_button(),
                volume,
                window,
                right_control_visuals,
            )
        };

        let right = if !narrow && layout_visual.active {
            let right_layout_visual = layout_visual;
            div()
                .w(px(side_width))
                .min_w(px(side_width))
                .h_full()
                .flex_none()
                .child(right)
                .with_animation(
                    ("player-right-layout", layout_visual.epoch),
                    crate::motion::panel(),
                    move |this, delta| {
                        let width = right_layout_visual.at(delta).side_width;
                        this.w(px(width)).min_w(px(width))
                    },
                )
                .into_any_element()
        } else {
            right
        };

        let current_column = div()
            .when(narrow, |this| this.w_full())
            .when(!narrow, |this| this.w(px(side_width)).flex_none())
            .min_w_0()
            .child(current_block);
        let current_column = if !narrow && layout_visual.active {
            let current_layout_visual = layout_visual;
            current_column
                .with_animation(
                    ("player-current-column-layout", layout_visual.epoch),
                    crate::motion::panel(),
                    move |this, delta| {
                        let width = current_layout_visual.at(delta).side_width;
                        this.w(px(width)).min_w(px(width))
                    },
                )
                .into_any_element()
        } else {
            current_column.into_any_element()
        };

        let player_row = div()
            .flex_none()
            .when(narrow, |this| this.h_auto().flex_col())
            .when(!narrow, |this| this.h(px(PLAYER_BAR_DESKTOP_HEIGHT_PX)))
            .w_full()
            .flex()
            .items_center()
            .gap(px(if narrow { 8. } else { column_gap }))
            .px(px(if narrow { 10. } else { padding }))
            .child(current_column)
            .child(center)
            .child(right);

        let player_row = if !narrow && layout_visual.active {
            let row_layout_visual = layout_visual;
            player_row
                .with_animation(
                    ("player-row-layout", layout_visual.epoch),
                    crate::motion::panel(),
                    move |this, delta| {
                        let layout = row_layout_visual.at(delta);
                        this.gap(px(layout.column_gap)).px(px(layout.padding))
                    },
                )
                .into_any_element()
        } else {
            player_row.into_any_element()
        };

        let player_content = div()
            .relative()
            .w_full()
            .child(player_row)
            .children(download_for_desktop.map(|button| {
                let visual = download_visual.expect("desktop download has a motion visual");
                let interactive = download_interactive.clone();
                let download = div()
                    .id("player-download-layout")
                    .absolute()
                    .left(px(visual.target.left))
                    .top(px(visual.target.top))
                    .w(px(visual.target.width))
                    .h(px(visual.target.height))
                    .child(button);
                if visual.active {
                    download
                        .left(px(visual.from.left))
                        .top(px(visual.from.top))
                        .w(px(visual.from.width))
                        .h(px(visual.from.height))
                        .opacity(visual.from_opacity)
                        .with_animation(
                            ("player-download-layout", visual.epoch),
                            crate::motion::relocation_fade(),
                            move |this, delta| {
                                let delta = visual.started_at.map_or(delta, |started_at| {
                                    fade_motion_progress(started_at, Instant::now())
                                });
                                interactive.set(delta >= 1.);
                                let geometry =
                                    fade_motion_geometry(visual.from, visual.target, delta);
                                this.left(px(geometry.left))
                                    .top(px(geometry.top))
                                    .w(px(geometry.width))
                                    .h(px(geometry.height))
                                    .opacity(fade_motion_opacity(
                                        visual.from_opacity,
                                        visual.target_opacity,
                                        delta,
                                    ))
                            },
                        )
                        .into_any_element()
                } else {
                    interactive.set(true);
                    download.into_any_element()
                }
            }))
            .child(close_button(compact, {
                let model = self.model.clone();
                move |_, _, cx| model.update(cx, |model, cx| model.close(cx))
            }));

        render_player_bar_host(narrow, true, Some(player_content.into_any_element()))
    }
}

pub(super) fn render_player_bar_host(
    narrow: bool,
    open: bool,
    content: Option<AnyElement>,
) -> AnyElement {
    let has_content = content.is_some();
    let mut host = div()
        .id("player-bar-host")
        .relative()
        .w_full()
        .overflow_hidden()
        .border_t_1()
        .border_color(rgb(BORDER))
        .bg(rgb(0x09090b))
        .children(content);

    if narrow {
        host = if has_content && open {
            host.h_auto()
        } else {
            host.h(px(0.))
        };
    } else {
        host = host.h(px(if open {
            PLAYER_BAR_DESKTOP_HEIGHT_PX
        } else {
            0.
        }));
    }
    host.opacity(if open { 1. } else { 0. }).into_any_element()
}
