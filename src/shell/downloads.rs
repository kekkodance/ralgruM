use crate::{
    app_tooltip::AppTooltipExt,
    assets::{LocalIcon, local_icon},
    browser_scroll::{BrowserScrollTarget, FixedListScrollHandle, browser_scroll_surface},
    downloads::{DownloadJob, DownloadStatus, open_downloads_folder, reveal_file},
    theme::{BORDER, DANGER, FOREGROUND, MUTED, PRIMARY, SURFACE, SURFACE_RAISED},
};
use gpui::{
    AnyElement, Context, FontWeight, IntoElement, ListState, Role, SharedString, Window, div,
    prelude::*, px, rgb, rgba,
};
use gpui_component::{progress::Progress, scroll::Scrollbar, scroll::ScrollbarShow};
use std::path::PathBuf;

use super::RalgrumApp;

const DOWNLOADS_CONTENT_BOTTOM_PADDING_PX: f32 = 16.;
const DOWNLOAD_ACTION_ICON_PX: f32 = 13.;

/// Uniform geometry for the virtualized downloads list. Every row reserves
/// the progress bar slot, active or not, so all rows measure one height
/// and the page can use exact fixed-extent scroll math: gpui discards
/// list size hints after a width change, so the pitch-derived fixed-handle
/// math is the scrollbar and wheel source of truth.
pub(super) const DOWNLOAD_ROW_HEIGHT_PX: f32 = 70.5;
pub(super) const DOWNLOAD_ROW_GAP_PX: f32 = 4.;
pub(super) const DOWNLOAD_ROW_PITCH_PX: f32 = DOWNLOAD_ROW_HEIGHT_PX + DOWNLOAD_ROW_GAP_PX;
const DOWNLOAD_LIST_OVERDRAW_ROWS: f32 = 12.;
pub(super) const DOWNLOAD_LIST_OVERDRAW_PX: f32 =
    DOWNLOAD_ROW_PITCH_PX * DOWNLOAD_LIST_OVERDRAW_ROWS;

pub(super) fn render_downloads(
    app: &RalgrumApp,
    window: &mut Window,
    cx: &mut Context<RalgrumApp>,
) -> impl IntoElement {
    let (count, platform_status, has_terminal_jobs) = {
        let snapshot = app.downloads.read(cx);
        (
            snapshot.jobs.len(),
            snapshot.platform_status.clone(),
            snapshot.jobs.iter().any(DownloadJob::is_terminal),
        )
    };
    let downloads_dir = app.settings.read(cx).saved().effective_downloads_dir();
    // Keep the list state in step with the job count before the list is
    // built, so the element tree and the scroll math agree this frame.
    sync_downloads_list_state(&app.downloads_list_state, count);
    // The page doubles as the refresh trigger: after the frame is out, ask
    // the model to fill any missing file caches. The deferred update keeps
    // rendering free of model mutation and disk access; the model probes
    // on its background executor and notifies once results land, so this
    // is a no-op whenever every visible job is already cached.
    let refresh = app.downloads.clone();
    cx.defer(move |cx| {
        refresh.update(cx, |model, cx| model.refresh_file_stats(cx));
    });
    let metrics = crate::music_ui::shell_metrics(f32::from(window.viewport_size().width));
    let gutter = crate::music_ui::main_content_inset(&metrics);
    let fixed_scroll = FixedListScrollHandle::new(
        app.downloads_list_state.clone(),
        count,
        px(DOWNLOAD_ROW_PITCH_PX),
    );
    // Rows read the job straight from the model at paint time, so the page
    // never clones the history and only the visible slice of rows is ever
    // built, however long the list grows.
    let row_downloads = app.downloads.clone();
    let list = gpui::list(
        app.downloads_list_state.clone(),
        move |index, _window, app| {
            let snapshot = row_downloads.read(app);
            let Some(job) = snapshot.jobs.get(index) else {
                return div().into_any_element();
            };
            download_list_item(job, row_downloads.clone())
        },
    );
    let body = div()
        .id("downloads-page-content")
        .size_full()
        .min_h_0()
        .px(px(gutter))
        // Breathing room above the player bar. Padding lives on the body
        // so it only shows at the tail and short lists keep zero scroll
        // offset.
        .pb(px(DOWNLOADS_CONTENT_BOTTOM_PADDING_PX))
        .when(count == 0, |this| this.child(downloads_empty_state()))
        .when(count > 0, |this| {
            this.child(list.w_full().h_full().min_h_0())
        });
    let scroll_viewport = div()
        .id("downloads-page-scroll-viewport")
        .relative()
        .flex_1()
        .min_h_0()
        .child(body)
        .child(
            div()
                .absolute()
                .inset_0()
                .child(Scrollbar::vertical(&fixed_scroll).scrollbar_show(ScrollbarShow::Hover)),
        )
        .into_any_element();
    div()
        .id("downloads-page")
        .size_full()
        .flex()
        .flex_col()
        .child(downloads_header(
            count,
            has_terminal_jobs,
            platform_status,
            downloads_dir,
            app.downloads.clone(),
            gutter,
        ))
        .child(browser_scroll_surface(
            "downloads-page-scroll",
            scroll_viewport,
            BrowserScrollTarget::FixedList(fixed_scroll),
            app.downloads_browser_scroll.clone(),
        ))
        .into_any_element()
}

/// Keep the page's list state in step with the job count. Jobs are only
/// ever appended at the tail, so growth splices the new range without
/// disturbing the scroll position; a shrink resets to the top with the
/// uniform height reapplied, which also re-arms the size hints gpui
/// discards after a width change.
fn sync_downloads_list_state(state: &ListState, count: usize) {
    let previous = state.item_count();
    if count > previous {
        state.splice(previous..previous, count - previous);
        state
            .clone()
            .with_uniform_item_height(px(DOWNLOAD_ROW_PITCH_PX));
    } else if count < previous {
        state.reset_with_uniform_height(count, px(DOWNLOAD_ROW_PITCH_PX));
    }
}

fn downloads_header(
    count: usize,
    has_terminal_jobs: bool,
    platform_status: Option<String>,
    downloads_dir: PathBuf,
    downloads: gpui::Entity<crate::downloads::DownloadModel>,
    gutter: f32,
) -> impl IntoElement {
    let clear = downloads.clone();
    let open = downloads.clone();
    div()
        .id("downloads-page-header")
        .flex_none()
        .w_full()
        .px(px(gutter))
        .pt(px(24.))
        .pb(px(12.))
        .flex()
        .flex_col()
        .gap(px(8.))
        .child(
            div()
                .flex()
                .items_center()
                .justify_between()
                .flex_wrap()
                .gap(px(12.))
                .child(
                    div()
                        .text_size(px(20.))
                        .font_weight(FontWeight(650.))
                        .text_color(rgb(FOREGROUND))
                        .child("Downloads"),
                )
                .child(
                    div()
                        .flex()
                        .items_center()
                        .flex_none()
                        .gap(px(10.))
                        .child(
                            div()
                                .flex_none()
                                .text_size(px(13.))
                                .text_color(rgb(MUTED))
                                .child(format!(
                                    "{} {}",
                                    count,
                                    if count == 1 { "item" } else { "items" }
                                )),
                        )
                        .child(
                            outline_button(
                                "downloads-clear",
                                LocalIcon::ListCheck,
                                "Clear list",
                                has_terminal_jobs,
                            )
                            .when(has_terminal_jobs, |this| {
                                this.on_click(move |_, _, cx| {
                                    clear.update(cx, |model, cx| model.clear_terminal(cx));
                                })
                            }),
                        )
                        .child(
                            outline_button(
                                "downloads-open-folder",
                                LocalIcon::FolderOpen,
                                "Open folder",
                                true,
                            )
                            .on_click(move |_, _, cx| {
                                open.update(cx, |model, cx| {
                                    model.set_platform_status(
                                        open_downloads_folder(&downloads_dir),
                                        cx,
                                    )
                                });
                            }),
                        ),
                ),
        )
        .when_some(platform_status, |this, status| {
            this.child(div().text_color(rgb(DANGER)).child(status))
        })
}

fn downloads_empty_state() -> impl IntoElement {
    div()
        .w_full()
        .py(px(60.))
        .flex()
        .flex_col()
        .items_center()
        .gap(px(7.))
        .child(local_icon(LocalIcon::Download, 0x71717a).size(px(40.)))
        .child(
            div()
                .text_size(px(16.))
                .font_weight(FontWeight::SEMIBOLD)
                .text_color(rgb(FOREGROUND))
                .child("No downloads yet"),
        )
        .child(
            div()
                .text_size(px(13.))
                .text_color(rgb(MUTED))
                .child("Downloads will appear here as soon as they are queued."),
        )
}

/// One virtualized row: a fixed-height slot wrapping the job row. The
/// wrapper pins the uniform pitch, so the list state's size hints and the
/// fixed-extent scroll math stay exact no matter which states the rows
/// render.
fn download_list_item(
    job: &DownloadJob,
    downloads: gpui::Entity<crate::downloads::DownloadModel>,
) -> AnyElement {
    div()
        .w_full()
        .flex_none()
        .h(px(DOWNLOAD_ROW_PITCH_PX))
        .child(render_job(job, downloads))
        .into_any_element()
}

fn render_job(
    job: &DownloadJob,
    downloads: gpui::Entity<crate::downloads::DownloadModel>,
) -> impl IntoElement {
    let id = job.id;
    div()
        .min_h(px(DOWNLOAD_ROW_HEIGHT_PX))
        .w_full()
        .flex()
        .items_center()
        .gap(px(9.))
        .py(px(7.))
        .px(px(6.))
        .rounded(px(6.))
        .border_1()
        .border_color(rgba(0x00000000))
        .hover(|style| style.bg(rgb(SURFACE)).border_color(rgb(BORDER)))
        .child(job_artwork(id, &job.track.artwork))
        .child(
            div()
                .flex_1()
                .min_w_0()
                .flex()
                .flex_col()
                .child(
                    div()
                        .text_size(px(12.5))
                        .line_height(px(15.625))
                        .font_weight(FontWeight::SEMIBOLD)
                        .text_color(rgb(FOREGROUND))
                        .truncate()
                        .child(job.track.title.clone()),
                )
                .child(
                    div()
                        .mt(px(2.))
                        .text_size(px(11.))
                        .line_height(px(13.75))
                        .text_color(rgb(MUTED))
                        .truncate()
                        .child(job.track.artist.clone()),
                )
                .child(status_line(&job.status))
                .child(progress_bar(job.id, &job.status)),
        )
        .child(quality_badge(job))
        .children(action_cell(id, &job.status, downloads))
}

fn job_artwork(id: u64, artwork: &str) -> impl IntoElement {
    div()
        .relative()
        .w(px(40.))
        .h(px(40.))
        .flex_none()
        .overflow_hidden()
        .rounded(px(6.))
        .bg(rgb(SURFACE_RAISED))
        .when(!artwork.is_empty(), |this| {
            this.child(
                crate::artwork_reveal::artwork_reveal(
                    ("download-artwork-reveal", id),
                    artwork.to_owned(),
                )
                .size_full()
                .rounded(px(6.)),
            )
        })
}

fn status_line(status: &DownloadStatus) -> AnyElement {
    let (color, text) = match status {
        DownloadStatus::Queued => (MUTED, "Queued".to_owned()),
        DownloadStatus::Resolving => (MUTED, "Preparing download…".to_owned()),
        DownloadStatus::Downloading { downloaded, total } => {
            let done = bytes(*downloaded);
            let text = match total {
                Some(total) => {
                    let percent = ((*downloaded as f64 / (*total).max(1) as f64) * 100.)
                        .round()
                        .min(100.) as u64;
                    format!("{percent}% · {done} of {}", bytes(*total))
                }
                None => format!("{done} downloaded"),
            };
            (MUTED, text)
        }
        DownloadStatus::NeedsConfirmation { .. } => (MUTED, "File already exists".to_owned()),
        DownloadStatus::Completed(_) => (MUTED, "Downloaded".to_owned()),
        DownloadStatus::Skipped(_) => (MUTED, "Skipped (existing file kept)".to_owned()),
        DownloadStatus::Failed(error) => (0xfca5a5, error.clone()),
        DownloadStatus::Cancelled => (MUTED, "Cancelled".to_owned()),
    };
    div()
        .mt(px(3.))
        .text_size(px(10.5))
        .line_height(px(13.125))
        .text_color(rgb(color))
        .truncate()
        .child(text)
        .into_any_element()
}

#[derive(Clone, Copy, Debug, PartialEq)]
enum ProgressPresentation {
    Hidden,
    Determinate(f32),
    Indeterminate,
}

fn progress_presentation(status: &DownloadStatus) -> ProgressPresentation {
    match status {
        DownloadStatus::Queued => ProgressPresentation::Determinate(0.),
        DownloadStatus::Resolving | DownloadStatus::Downloading { total: None, .. } => {
            ProgressPresentation::Indeterminate
        }
        DownloadStatus::Downloading {
            downloaded,
            total: Some(total),
        } => {
            ProgressPresentation::Determinate((*downloaded as f32 / (*total).max(1) as f32).min(1.))
        }
        _ => ProgressPresentation::Hidden,
    }
}

/// The progress bar slot is reserved in every state, including terminal
/// ones, so active and terminal rows measure the same height and the
/// virtualized list stays uniform; terminal states leave the slot empty.
fn progress_bar(id: u64, status: &DownloadStatus) -> AnyElement {
    match progress_presentation(status) {
        ProgressPresentation::Hidden => div().mt(px(6.)).h(px(3.)).into_any_element(),
        ProgressPresentation::Determinate(fraction) => progress_bar_visual(id, fraction, false),
        ProgressPresentation::Indeterminate => progress_bar_visual(id, 0., true),
    }
}

fn progress_bar_visual(id: u64, fraction: f32, loading: bool) -> AnyElement {
    Progress::new(("download-progress", id))
        .mt(px(6.))
        .h(px(3.))
        .color(rgb(PRIMARY))
        .value(fraction * 100.)
        .loading(loading)
        .into_any_element()
}

fn quality_badge(job: &DownloadJob) -> impl IntoElement {
    div()
        .h(px(20.))
        .min_w(px(42.))
        .px(px(7.))
        .py(px(2.))
        .max_w(px(150.))
        .flex_none()
        .flex()
        .items_center()
        .justify_center()
        .rounded(px(4.))
        .border_1()
        .border_color(rgb(BORDER))
        .bg(rgb(SURFACE_RAISED))
        .text_size(px(11.))
        .font_weight(FontWeight::SEMIBOLD)
        .text_color(rgb(FOREGROUND))
        .child(
            div()
                .relative()
                .top(px(-1.))
                .min_w_0()
                .truncate()
                .child(quality_label(job)),
        )
}

fn quality_label(job: &DownloadJob) -> String {
    match &job.status {
        DownloadStatus::Completed(_)
        | DownloadStatus::Skipped(_)
        | DownloadStatus::NeedsConfirmation { .. } => job
            .quality
            .as_ref()
            .map(|quality| quality.label(job.track.duration, job.file_size()))
            .unwrap_or_else(|| "Quality unavailable".to_owned()),
        DownloadStatus::Queued => "Queued".to_owned(),
        DownloadStatus::Failed(_) => "Failed".to_owned(),
        DownloadStatus::Cancelled => "Cancelled".to_owned(),
        DownloadStatus::Downloading { .. } | DownloadStatus::Resolving => "Downloading".to_owned(),
    }
}

fn action_cell(
    id: u64,
    status: &DownloadStatus,
    downloads: gpui::Entity<crate::downloads::DownloadModel>,
) -> Option<AnyElement> {
    match status {
        DownloadStatus::Cancelled => None,
        DownloadStatus::Completed(path)
        | DownloadStatus::Skipped(path)
        | DownloadStatus::NeedsConfirmation { path } => {
            let path = path.clone();
            let button_id = SharedString::from(format!("download-reveal-{id}"));
            Some(
                icon_action_button(button_id.clone())
                    .role(Role::Button)
                    .aria_label("Show in File Explorer")
                    .app_tooltip("Show in File Explorer")
                    .child(
                        local_icon(LocalIcon::FolderOpen, MUTED)
                            .w(px(DOWNLOAD_ACTION_ICON_PX))
                            .h(px(DOWNLOAD_ACTION_ICON_PX))
                            .group_hover(button_id, |style| style.text_color(rgb(FOREGROUND))),
                    )
                    .on_click(move |_, _, cx| {
                        downloads.update(cx, |model, cx| {
                            model.set_platform_status(reveal_file(&path), cx)
                        });
                    })
                    .into_any_element(),
            )
        }
        DownloadStatus::Queued | DownloadStatus::Resolving | DownloadStatus::Downloading { .. } => {
            let button_id = SharedString::from(format!("download-cancel-{id}"));
            let tooltip = match status {
                DownloadStatus::Queued => "Cancel queued download",
                _ => "Cancel download",
            };
            Some(
                icon_action_button(button_id.clone())
                    .role(Role::Button)
                    .aria_label(tooltip)
                    .app_tooltip(tooltip)
                    .child(
                        local_icon(LocalIcon::X, MUTED)
                            .w(px(DOWNLOAD_ACTION_ICON_PX))
                            .h(px(DOWNLOAD_ACTION_ICON_PX))
                            .group_hover(button_id, |style| style.text_color(rgb(FOREGROUND))),
                    )
                    .on_click(move |_, _, cx| {
                        downloads.update(cx, |model, cx| model.cancel(id, cx));
                    })
                    .into_any_element(),
            )
        }
        DownloadStatus::Failed(_) => {
            let button_id = SharedString::from(format!("download-retry-{id}"));
            Some(
                icon_action_button(button_id.clone())
                    .role(Role::Button)
                    .aria_label("Retry download")
                    .app_tooltip("Retry download")
                    .child(
                        local_icon(LocalIcon::RotateRight, MUTED)
                            .w(px(DOWNLOAD_ACTION_ICON_PX))
                            .h(px(DOWNLOAD_ACTION_ICON_PX))
                            .group_hover(button_id, |style| style.text_color(rgb(FOREGROUND))),
                    )
                    .on_click(move |_, _, cx| {
                        downloads.update(cx, |model, cx| model.retry(id, cx));
                    })
                    .into_any_element(),
            )
        }
    }
}

fn icon_action_button(id: SharedString) -> gpui::Stateful<gpui::Div> {
    div()
        .id(id.clone())
        .group(id)
        .focusable()
        .tab_stop(true)
        .size(px(34.))
        .flex_none()
        .flex()
        .items_center()
        .justify_center()
        .cursor_pointer()
        .rounded(px(6.))
        .border_1()
        .border_color(rgba(0x00000000))
        .hover(|style| style.bg(rgb(BORDER)))
        .focus_visible(|style| style.border_color(rgb(PRIMARY)))
}

fn outline_button(
    id: &'static str,
    icon: LocalIcon,
    label: &'static str,
    enabled: bool,
) -> gpui::Stateful<gpui::Div> {
    div()
        .id(id)
        .focusable()
        .tab_stop(enabled)
        .role(Role::Button)
        .aria_label(label)
        .h(px(34.))
        .px(px(16.))
        .flex_none()
        .flex()
        .items_center()
        .gap(px(8.))
        .rounded(px(6.))
        .border_1()
        .border_color(rgb(BORDER))
        .text_size(px(13.))
        .font_weight(FontWeight::MEDIUM)
        .text_color(rgb(FOREGROUND))
        .when(enabled, |this| {
            this.cursor_pointer().hover(|style| style.bg(rgb(BORDER)))
        })
        .when(!enabled, |this| this.opacity(0.45))
        .focus_visible(|style| style.border_color(rgb(PRIMARY)))
        .child(local_icon(icon, MUTED).size(px(13.)))
        .child(label.to_owned())
}

fn bytes(value: u64) -> String {
    let kilobyte = 1024.;
    let megabyte = kilobyte * 1024.;
    let gigabyte = megabyte * 1024.;
    let value = value as f64;
    if value < megabyte {
        format!("{:.0} KB", value / kilobyte)
    } else if value < gigabyte {
        format!("{:.0} MB", value / megabyte)
    } else {
        let gigabytes = value / gigabyte;
        if gigabytes >= 10. {
            format!("{gigabytes:.0} GB")
        } else {
            format!("{gigabytes:.1} GB")
        }
    }
}

#[cfg(test)]
mod tests {
    use super::{
        DOWNLOAD_LIST_OVERDRAW_PX, DOWNLOAD_ROW_GAP_PX, DOWNLOAD_ROW_HEIGHT_PX,
        DOWNLOAD_ROW_PITCH_PX, ProgressPresentation, progress_presentation,
        sync_downloads_list_state,
    };
    use crate::account_session::SessionError;
    use crate::downloads::{DownloadFileStat, DownloadJob, DownloadModel, DownloadStatus};
    use crate::murglar_backend::{DeviceIdentityError, DeviceIdentityStatus};
    use crate::playback::{DownloadVariant, PlaybackProvider, PlaybackTrack};
    use crate::settings::AccountState;
    use gpui::{AppContext as _, ListAlignment, ListOffset, ListState, prelude::*, px};
    use std::sync::Arc;
    use std::time::Duration;
    use tokio::runtime::Runtime;

    fn soundcloud_track(id: &str, title: &str) -> PlaybackTrack {
        PlaybackTrack {
            downloadable: true,
            progressive: false,
            provider: PlaybackProvider::SoundCloud,
            id: id.into(),
            title: title.into(),
            artist: "Artist".into(),
            album: String::new(),
            album_id: String::new(),
            release_date: String::new(),
            artists: Vec::new(),
            artwork: String::new(),
            duration: Duration::from_secs(10),
            explicit: false,
            ai_generated: false,
            service_url: String::new(),
        }
    }

    fn quality_job_with_missing_destination() -> DownloadJob {
        let mut job = DownloadJob::new(1, soundcloud_track("1", "Song"), DownloadVariant::Standard);
        job.quality = Some(crate::downloads::ResolvedQuality {
            format: "MP3".into(),
            source_size: Some(80_000),
            declared_bitrate: None,
        });
        job.status = DownloadStatus::Completed("Z:/missing/destination/song.mp3".into());
        job
    }

    #[test]
    fn quality_label_uses_the_cached_file_stat_instead_of_the_filesystem() {
        let mut job = quality_job_with_missing_destination();
        // The destination does not exist, so a filesystem probe would
        // report no size and fall back to the source hint.
        job.file = Some(DownloadFileStat {
            exists: true,
            size: Some(160_000),
            modified: None,
        });
        assert_eq!(super::quality_label(&job), "MP3 128kbps");

        // Without a cached probe the label falls back to the source hint,
        // still without touching the disk.
        job.file = None;
        assert_eq!(super::quality_label(&job), "MP3 64kbps");

        // A probe that found no file yields no size either.
        job.file = Some(DownloadFileStat::default());
        assert_eq!(super::quality_label(&job), "MP3 64kbps");
    }

    #[test]
    fn queued_download_progress_starts_at_zero() {
        assert_eq!(
            progress_presentation(&DownloadStatus::Queued),
            ProgressPresentation::Determinate(0.)
        );
    }

    #[test]
    fn progress_presentation_keeps_unknown_and_known_sizes_distinct() {
        assert_eq!(
            progress_presentation(&DownloadStatus::Resolving),
            ProgressPresentation::Indeterminate
        );
        assert_eq!(
            progress_presentation(&DownloadStatus::Downloading {
                downloaded: 128,
                total: None,
            }),
            ProgressPresentation::Indeterminate
        );
        assert_eq!(
            progress_presentation(&DownloadStatus::Downloading {
                downloaded: 25,
                total: Some(100),
            }),
            ProgressPresentation::Determinate(0.25)
        );
        assert_eq!(
            progress_presentation(&DownloadStatus::Downloading {
                downloaded: 128,
                total: Some(100),
            }),
            ProgressPresentation::Determinate(1.)
        );
    }

    #[test]
    fn terminal_download_states_have_no_progress_bar() {
        assert_eq!(
            progress_presentation(&DownloadStatus::Completed("track.flac".into())),
            ProgressPresentation::Hidden
        );
        assert_eq!(
            progress_presentation(&DownloadStatus::Failed("network error".into())),
            ProgressPresentation::Hidden
        );
    }

    #[test]
    fn download_row_pitch_reserves_the_progress_slot_and_the_row_gap() {
        // The text column is the title (15.625) plus the artist (2 +
        // 13.75) plus the status line (3 + 13.125), and the row carries 7
        // + 7 vertical padding. The progress slot (6 + 3) is reserved in
        // every state, so terminal rows measure the same height as
        // downloading ones.
        let text_column = 15.625 + 2. + 13.75 + 3. + 13.125;
        let progress_slot = 6. + 3.;
        let row_padding = 7. + 7.;
        assert_eq!(
            DOWNLOAD_ROW_HEIGHT_PX,
            text_column + progress_slot + row_padding
        );
        assert_eq!(DOWNLOAD_ROW_GAP_PX, 4.);
        assert_eq!(
            DOWNLOAD_ROW_PITCH_PX,
            DOWNLOAD_ROW_HEIGHT_PX + DOWNLOAD_ROW_GAP_PX
        );
        // Uniform pitches are what let the page feed the scrollbar from
        // fixed-extent math, so the overdraw must be whole pitches.
        assert_eq!(DOWNLOAD_LIST_OVERDRAW_PX, DOWNLOAD_ROW_PITCH_PX * 12.);
    }

    #[test]
    fn appended_jobs_extend_the_list_without_disturbing_the_scroll() {
        let state = ListState::new(8, ListAlignment::Top, px(DOWNLOAD_LIST_OVERDRAW_PX))
            .with_uniform_item_height(px(DOWNLOAD_ROW_PITCH_PX));
        state.scroll_to(ListOffset {
            item_ix: 3,
            offset_in_item: px(7.),
        });

        sync_downloads_list_state(&state, 12);

        assert_eq!(state.item_count(), 12);
        assert_eq!(state.logical_scroll_top().item_ix, 3);
        assert_eq!(f32::from(state.logical_scroll_top().offset_in_item), 7.);
    }

    #[test]
    fn a_smaller_job_list_resets_to_the_top_with_uniform_hints() {
        let state = ListState::new(12, ListAlignment::Top, px(DOWNLOAD_LIST_OVERDRAW_PX))
            .with_uniform_item_height(px(DOWNLOAD_ROW_PITCH_PX));
        state.scroll_to(ListOffset {
            item_ix: 5,
            offset_in_item: px(2.),
        });

        sync_downloads_list_state(&state, 4);

        assert_eq!(state.item_count(), 4);
        assert_eq!(state.logical_scroll_top().item_ix, 0);
        assert_eq!(f32::from(state.logical_scroll_top().offset_in_item), 0.);
    }

    #[test]
    fn an_unchanged_job_count_leaves_the_list_state_alone() {
        let state = ListState::new(6, ListAlignment::Top, px(DOWNLOAD_LIST_OVERDRAW_PX))
            .with_uniform_item_height(px(DOWNLOAD_ROW_PITCH_PX));
        state.scroll_to(ListOffset {
            item_ix: 2,
            offset_in_item: px(5.),
        });

        sync_downloads_list_state(&state, 6);

        assert_eq!(state.item_count(), 6);
        assert_eq!(state.logical_scroll_top().item_ix, 2);
        assert_eq!(f32::from(state.logical_scroll_top().offset_in_item), 5.);
    }

    /// The page renders through the list state, so the extent has to be
    /// exact for the uniform pitch no matter which states the rows carry:
    /// a drawn list measures the real rows and the reserved progress slot
    /// keeps terminal and active rows identical.
    #[gpui::test]
    fn download_rows_measure_one_uniform_pitch_through_the_list_state(
        cx: &mut gpui::TestAppContext,
    ) {
        struct DownloadRows(ListState, gpui::Entity<DownloadModel>);
        impl gpui::Render for DownloadRows {
            fn render(
                &mut self,
                _: &mut gpui::Window,
                _: &mut gpui::Context<Self>,
            ) -> impl gpui::IntoElement {
                let state = self.0.clone();
                let downloads = self.1.clone();
                gpui::div().size_full().child(
                    gpui::list(state, move |index, _window, app| {
                        let snapshot = downloads.read(app);
                        let Some(job) = snapshot.jobs.get(index) else {
                            return gpui::div().into_any_element();
                        };
                        super::download_list_item(job, downloads.clone())
                    })
                    .w_full()
                    .h_full(),
                )
            }
        }

        let account = cx.update(|cx| {
            cx.new(|_| {
                AccountState::new(
                    DeviceIdentityStatus::Error(DeviceIdentityError::HardwareCollectionUnavailable),
                    Err(SessionError::ConfigDirectoryUnavailable),
                )
            })
        });
        let downloads = cx.new(|_| {
            let mut model = DownloadModel::new(account, Arc::new(Runtime::new().unwrap()));
            let mut completed =
                DownloadJob::new(1, soundcloud_track("1", "Done"), DownloadVariant::Standard);
            completed.status = DownloadStatus::Completed("Z:/downloads/done.mp3".into());
            let mut downloading = DownloadJob::new(
                2,
                soundcloud_track("2", "Active"),
                DownloadVariant::Standard,
            );
            downloading.status = DownloadStatus::Downloading {
                downloaded: 64,
                total: Some(128),
            };
            let mut queued = DownloadJob::new(
                3,
                soundcloud_track("3", "Waiting"),
                DownloadVariant::Standard,
            );
            queued.status = DownloadStatus::Queued;
            let mut failed = DownloadJob::new(
                4,
                soundcloud_track("4", "Broken"),
                DownloadVariant::Standard,
            );
            failed.status = DownloadStatus::Failed("network error".into());
            let mut cancelled =
                DownloadJob::new(5, soundcloud_track("5", "Gone"), DownloadVariant::Standard);
            cancelled.status = DownloadStatus::Cancelled;
            model.jobs = vec![completed, downloading, queued, failed, cancelled];
            model
        });
        let count = 5;

        let state = ListState::new(count, ListAlignment::Top, px(DOWNLOAD_LIST_OVERDRAW_PX))
            .with_uniform_item_height(px(DOWNLOAD_ROW_PITCH_PX));
        let cx = cx.add_empty_window();
        // The rows render gpui-component progress bars, so the drawn list
        // needs the component theme the real window provides.
        cx.update(|_, cx| {
            cx.set_global(gpui_component::Theme::default());
            gpui_component::init(cx);
        });
        let view = cx.update(|_, cx| cx.new(|_| DownloadRows(state.clone(), downloads.clone())));
        cx.draw(
            gpui::point(px(0.), px(0.)),
            gpui::size(px(700.), px(200.)),
            {
                let view = view.clone();
                move |_, _| view.into_any_element()
            },
        );

        // The reserved progress slot keeps every mixed row at one pitch,
        // so the virtualized extent is exact rather than estimated.
        assert_eq!(
            f32::from(state.max_offset_for_scrollbar().y),
            count as f32 * DOWNLOAD_ROW_PITCH_PX - 200.
        );
    }
}
