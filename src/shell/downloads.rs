use crate::{
    app_tooltip::AppTooltipExt,
    assets::{LocalIcon, local_icon},
    browser_scroll::{BrowserScrollTarget, browser_scroll_surface},
    downloads::{DownloadJob, DownloadStatus, open_downloads_folder, reveal_file},
    theme::{BORDER, DANGER, FOREGROUND, MUTED, PRIMARY, SURFACE, SURFACE_RAISED},
};
use gpui::{
    AnyElement, Context, FontWeight, IntoElement, Role, SharedString, Window, div, prelude::*, px,
    rgb, rgba,
};
use gpui_component::{progress::Progress, scroll::ScrollableElement};

use super::RalgrumApp;

const DOWNLOADS_CONTENT_BOTTOM_PADDING_PX: f32 = 16.;
const DOWNLOAD_ACTION_ICON_PX: f32 = 13.;

pub(super) fn render_downloads(
    app: &RalgrumApp,
    window: &mut Window,
    cx: &mut Context<RalgrumApp>,
) -> impl IntoElement {
    let downloads_snapshot = app.downloads.read(cx);
    let jobs = downloads_snapshot.jobs.clone();
    let platform_status = downloads_snapshot.platform_status.clone();
    let downloads_dir = app.settings.read(cx).saved().effective_downloads_dir();
    let open = app.downloads.clone();
    let clear = app.downloads.clone();
    let mut rows = Vec::<AnyElement>::with_capacity(jobs.len());
    for job in &jobs {
        rows.push(render_job(job, app.downloads.clone(), cx).into_any_element());
    }
    let has_terminal_jobs = jobs.iter().any(|job| job.is_terminal());
    let metrics = crate::music_ui::shell_metrics(f32::from(window.viewport_size().width));
    let content =
        div()
            .id("downloads-page-content")
            .flex_1()
            .min_h_0()
            .track_scroll(&app.downloads_scroll)
            .overflow_y_scroll()
            .vertical_scrollbar(&app.downloads_scroll)
            .flex()
            .flex_col()
            .child(
                div()
                    .w_full()
                    .flex()
                    .flex_col()
                    .gap(px(12.))
                    .px(px(crate::music_ui::main_content_inset(&metrics)))
                    .pt(px(24.))
                    // Breathing room above the player bar. Padding lives on the
                    // inner content so it only shows at the tail and short lists
                    // keep zero scroll offset.
                    .pb(px(DOWNLOADS_CONTENT_BOTTOM_PADDING_PX))
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
                                                jobs.len(),
                                                if jobs.len() == 1 { "item" } else { "items" }
                                            )),
                                    )
                                    .child(
                                        outline_button(
                                            "downloads-clear",
                                            LocalIcon::ListCheck,
                                            "Clear list",
                                            has_terminal_jobs,
                                        )
                                        .when(
                                            has_terminal_jobs,
                                            |this| {
                                                this.on_click(cx.listener(move |_, _, _, cx| {
                                                    clear.update(cx, |model, cx| {
                                                        model.clear_terminal(cx)
                                                    })
                                                }))
                                            },
                                        ),
                                    )
                                    .child(
                                        outline_button(
                                            "downloads-open-folder",
                                            LocalIcon::FolderOpen,
                                            "Open folder",
                                            true,
                                        )
                                        .on_click(
                                            cx.listener(move |_, _, _, cx| {
                                                open.update(cx, |model, cx| {
                                                    model.set_platform_status(
                                                        open_downloads_folder(&downloads_dir),
                                                        cx,
                                                    )
                                                })
                                            }),
                                        ),
                                    ),
                            ),
                    )
                    .when_some(platform_status, |this, status| {
                        this.child(div().text_color(rgb(DANGER)).child(status))
                    })
                    .when(jobs.is_empty(), |this| {
                        this.child(
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
                                .child(div().text_size(px(13.)).text_color(rgb(MUTED)).child(
                                    "Downloads will appear here as soon as they are queued.",
                                )),
                        )
                    })
                    .when(!jobs.is_empty(), |this| {
                        this.child(div().w_full().flex().flex_col().gap(px(4.)).children(rows))
                    }),
            )
            .into_any_element();
    browser_scroll_surface(
        "downloads-page",
        content,
        BrowserScrollTarget::Handle(app.downloads_scroll.clone()),
        app.downloads_browser_scroll.clone(),
    )
}

fn render_job(
    job: &DownloadJob,
    downloads: gpui::Entity<crate::downloads::DownloadModel>,
    cx: &mut Context<RalgrumApp>,
) -> impl IntoElement {
    let id = job.id;
    div()
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
                .children(progress_bar(job.id, &job.status)),
        )
        .child(quality_badge(job))
        .children(action_cell(id, &job.status, downloads, cx))
}

fn job_artwork(id: u64, artwork: &str) -> impl IntoElement {
    div()
        .relative()
        .w(px(40.))
        .h(px(40.))
        .flex_none()
        .overflow_hidden()
        .rounded(px(6.))
        .border_1()
        .border_color(rgb(BORDER))
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

fn progress_bar(id: u64, status: &DownloadStatus) -> Option<AnyElement> {
    let presentation = progress_presentation(status);
    let (fraction, loading) = match presentation {
        ProgressPresentation::Hidden => return None,
        ProgressPresentation::Determinate(fraction) => (fraction, false),
        ProgressPresentation::Indeterminate => (0., true),
    };
    Some(
        Progress::new(("download-progress", id))
            .mt(px(6.))
            .h(px(3.))
            .color(rgb(PRIMARY))
            .value(fraction * 100.)
            .loading(loading)
            .into_any_element(),
    )
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
    let file_size = |path: &std::path::Path| std::fs::metadata(path).ok().map(|meta| meta.len());
    match &job.status {
        DownloadStatus::Completed(path)
        | DownloadStatus::Skipped(path)
        | DownloadStatus::NeedsConfirmation { path } => job
            .quality
            .as_ref()
            .map(|quality| quality.label(job.track.duration, file_size(path)))
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
    cx: &mut Context<RalgrumApp>,
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
                    .on_click(cx.listener(move |_, _, _, cx| {
                        downloads.update(cx, |model, cx| {
                            model.set_platform_status(reveal_file(&path), cx)
                        })
                    }))
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
                    .on_click(cx.listener(move |_, _, _, cx| {
                        downloads.update(cx, |model, cx| model.cancel(id, cx))
                    }))
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
                    .on_click(cx.listener(move |_, _, _, cx| {
                        downloads.update(cx, |model, cx| model.retry(id, cx))
                    }))
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
    use super::{ProgressPresentation, progress_presentation};
    use crate::downloads::DownloadStatus;

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
}
