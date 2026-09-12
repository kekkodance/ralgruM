use gpui::{Context, Entity, SharedString, WeakEntity};

use crate::{
    downloads::{DownloadModel, DownloadNotice, format_label},
    toast::{ToastAction, ToastKind},
};

use super::{Nav, RalgrumApp};

/// Wire download lifecycle notices to the shell so presentation follows the
/// currently visible destination rather than making the download model aware
/// of navigation state.
pub(super) fn subscribe_download_notices(
    downloads: &Entity<DownloadModel>,
    cx: &mut Context<RalgrumApp>,
) {
    let download_host = downloads.downgrade();
    cx.subscribe(downloads, move |app, _, notice: &DownloadNotice, cx| {
        handle_download_notice(app, download_host.clone(), notice, cx);
    })
    .detach();
}

fn handle_download_notice(
    app: &mut RalgrumApp,
    download_host: WeakEntity<DownloadModel>,
    notice: &DownloadNotice,
    cx: &mut Context<RalgrumApp>,
) {
    match notice {
        DownloadNotice::Conflict { id, title, quality } => {
            let id = *id;
            let ignore_host = download_host.clone();
            let overwrite_host = download_host;
            let description: SharedString = conflict_description(title, quality).into();
            app.toasts.update(cx, |toasts, cx| {
                toasts.push_actionable_keyed(
                    format!("download-conflict-{id}"),
                    ToastKind::Warning,
                    "File Already Exists",
                    Some(description),
                    [
                        ToastAction::secondary("Ignore", move |_, _, cx| {
                            if let Some(downloads) = ignore_host.upgrade() {
                                let _ = downloads.update(cx, |downloads, cx| {
                                    downloads.ignore_conflict(id, cx);
                                });
                            }
                        }),
                        ToastAction::danger_secondary("Overwrite", move |_, _, cx| {
                            if let Some(downloads) = overwrite_host.upgrade() {
                                let _ = downloads.update(cx, |downloads, cx| {
                                    downloads.overwrite_conflict(id, cx);
                                });
                            }
                        }),
                    ],
                    cx,
                );
            });
        }
        DownloadNotice::ConflictCleared { id } => {
            app.toasts.update(cx, |toasts, cx| {
                toasts.remove_key(&format!("download-conflict-{id}"), cx);
            });
        }
        DownloadNotice::BatchConflict { key, existing } => {
            let key = *key;
            let ignore_host = download_host.clone();
            let overwrite_host = download_host;
            app.toasts.update(cx, |toasts, cx| {
                toasts.push_actionable_keyed(
                    batch_conflict_toast_key(key),
                    ToastKind::Warning,
                    "Files Already Exist",
                    Some(batch_conflict_description(*existing).into()),
                    [
                        ToastAction::secondary("Ignore", move |_, _, cx| {
                            if let Some(downloads) = ignore_host.upgrade() {
                                let _ = downloads.update(cx, |downloads, cx| {
                                    downloads.ignore_batch_conflict(key, cx);
                                });
                            }
                        }),
                        ToastAction::danger_secondary("Overwrite", move |_, _, cx| {
                            if let Some(downloads) = overwrite_host.upgrade() {
                                let _ = downloads.update(cx, |downloads, cx| {
                                    downloads.overwrite_batch_conflict(key, cx);
                                });
                            }
                        }),
                    ],
                    cx,
                );
            });
        }
        DownloadNotice::BatchConflictCleared { key } => {
            app.toasts.update(cx, |toasts, cx| {
                toasts.remove_key(&batch_conflict_toast_key(*key), cx);
            });
        }
        DownloadNotice::Completed { title, quality } => {
            if completion_toast_visible(app.nav, app.settings_mode) {
                app.toasts.update(cx, |toasts, cx| {
                    toasts.push(
                        ToastKind::Success,
                        "Track Downloaded",
                        Some(completed_description(title, quality).into()),
                        cx,
                    );
                });
            }
        }
        DownloadNotice::BatchCompleted { saved } => {
            if completion_toast_visible(app.nav, app.settings_mode) {
                app.toasts.update(cx, |toasts, cx| {
                    let description = if *saved == 1 {
                        "1 track downloaded.".to_owned()
                    } else {
                        format!("{saved} tracks downloaded.")
                    };
                    toasts.push(
                        ToastKind::Success,
                        "Downloads Finished",
                        Some(description.into()),
                        cx,
                    );
                });
            }
        }
    }
}

fn batch_conflict_toast_key(key: u64) -> String {
    format!("download-batch-conflict-{key}")
}

fn batch_conflict_description(existing: usize) -> String {
    if existing == 1 {
        "1 track already exists. Choose whether to keep it or replace it.".to_owned()
    } else {
        format!("{existing} tracks already exist. Choose whether to keep them or replace them.")
    }
}

fn notice_quality_label(quality: &str) -> Option<String> {
    const UNAVAILABLE_SUFFIX: &str = " bitrate unavailable";
    let quality = quality.trim();

    if quality.is_empty() || quality.eq_ignore_ascii_case("Quality unavailable") {
        return None;
    }

    if let Some(format) = quality.strip_suffix(UNAVAILABLE_SUFFIX) {
        let format = format.trim();
        return (!format.is_empty()).then(|| format_label(format));
    }

    if quality.eq_ignore_ascii_case("bitrate unavailable") {
        None
    } else {
        Some(quality.to_owned())
    }
}

fn conflict_description(title: &str, quality: &str) -> String {
    match notice_quality_label(quality) {
        Some(quality) => format!("{title} · {quality}"),
        None => title.to_owned(),
    }
}

fn completed_description(title: &str, quality: &str) -> String {
    match notice_quality_label(quality) {
        Some(quality) => format!("{title} downloaded at {quality}."),
        None => format!("{title} downloaded."),
    }
}

fn completion_toast_visible(nav: Nav, settings_mode: bool) -> bool {
    settings_mode || nav != Nav::Downloads
}

#[cfg(test)]
mod tests {
    use super::{
        Nav, batch_conflict_description, batch_conflict_toast_key, completed_description,
        completion_toast_visible, conflict_description, notice_quality_label,
    };

    #[test]
    fn completion_is_hidden_only_for_visible_downloads_page() {
        assert!(!completion_toast_visible(Nav::Downloads, false));
        assert!(completion_toast_visible(Nav::Downloads, true));
        assert!(completion_toast_visible(Nav::Discover, false));
        assert!(completion_toast_visible(Nav::Library, false));
        assert!(completion_toast_visible(Nav::Cache, false));
    }

    #[test]
    fn unavailable_notice_quality_falls_back_to_the_format_label() {
        assert_eq!(
            notice_quality_label("WAV bitrate unavailable"),
            Some("WAV".into())
        );
        assert_eq!(
            notice_quality_label("M4A/MP4 AAC bitrate unavailable"),
            Some("M4A/MP4 AAC".into())
        );
    }

    #[test]
    fn available_notice_quality_is_unchanged() {
        assert_eq!(
            notice_quality_label("MP3 320kbps"),
            Some("MP3 320kbps".into())
        );
    }

    #[test]
    fn missing_notice_quality_is_omitted() {
        assert_eq!(notice_quality_label(""), None);
        assert_eq!(notice_quality_label("  "), None);
        assert_eq!(notice_quality_label("Quality unavailable"), None);
        assert_eq!(notice_quality_label(" bitrate unavailable"), None);
        assert_eq!(notice_quality_label("bitrate unavailable"), None);
    }

    #[test]
    fn missing_quality_keeps_notice_copy_clean() {
        assert_eq!(conflict_description("Song Title", ""), "Song Title");
        assert_eq!(
            completed_description("Song Title", "Quality unavailable"),
            "Song Title downloaded."
        );
    }

    #[test]
    fn conflict_copy_stays_compact_and_identifies_track_quality() {
        assert_eq!(
            conflict_description("Song Title", "MP3 320kbps"),
            "Song Title · MP3 320kbps"
        );
        assert!(!conflict_description("Song Title", "MP3 320kbps").contains("Choose"));
    }

    #[test]
    fn unavailable_quality_is_sanitized_in_both_notice_copies() {
        let conflict = conflict_description("Song Title", "WAV bitrate unavailable");
        let completed = completed_description("Song Title", "WAV bitrate unavailable");
        assert_eq!(conflict, "Song Title · WAV");
        assert_eq!(completed, "Song Title downloaded at WAV.");
        assert!(!conflict.contains("bitrate unavailable"));
        assert!(!completed.contains("bitrate unavailable"));
    }

    #[test]
    fn batch_conflict_copy_is_actionable_for_one_or_many_tracks() {
        assert_eq!(
            batch_conflict_description(1),
            "1 track already exists. Choose whether to keep it or replace it."
        );
        assert_eq!(
            batch_conflict_description(3),
            "3 tracks already exist. Choose whether to keep them or replace them."
        );
        assert_eq!(batch_conflict_toast_key(4), "download-batch-conflict-4");
    }
}
