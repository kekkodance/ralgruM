use gpui::{Entity, IntoElement};
use gpui_component::{
    Icon, Sizable,
    button::Button,
    menu::{DropdownMenu, PopupMenu, PopupMenuItem},
};

use crate::{
    assets::LocalIcon,
    downloads::DownloadModel,
    playback::{
        DownloadVariant, PlaybackProvider, PlaybackTrack, deezer_collection_download_choices,
    },
    settings::AccountState,
};

pub(crate) fn collection_download_button(
    id: String,
    tracks: Vec<PlaybackTrack>,
    downloads: Entity<DownloadModel>,
    account: Entity<AccountState>,
) -> gpui::AnyElement {
    Button::new(format!("collection-download-{id}"))
        .small()
        .icon(Icon::default().path(LocalIcon::Download.path()))
        .label("Download")
        .dropdown_menu(move |menu, window, cx| {
            let (deezer_arl, soundcloud_token, murglar_token) = {
                let account_snapshot = account.read(cx);
                (
                    account_snapshot.deezer_arl().is_some(),
                    account_snapshot.soundcloud_token().is_some(),
                    account_snapshot.murglar_media_credentials().is_some(),
                )
            };
            let availability = crate::playback::collection_download_availability(
                &tracks,
                deezer_arl,
                soundcloud_token,
                murglar_token,
            );
            let provider = tracks.first().map(|track| track.provider);
            let submenu_tracks = tracks.clone();
            let submenu_downloads = downloads.clone();
            let submenu_account = account.clone();
            super::submenu::native_styled_submenu_with_icon(
                menu,
                LocalIcon::Download,
                "Download format",
                window,
                cx,
                move |menu, _, _| {
                    let add = |menu: PopupMenu,
                               label: &'static str,
                               detail: &'static str,
                               variant: DownloadVariant| {
                        let tracks = submenu_tracks.clone();
                        let downloads = submenu_downloads.clone();
                        let account = submenu_account.clone();
                        menu.item(
                            PopupMenuItem::element(move |_, _| {
                                super::items::download_variant_shell(
                                    super::items::download_variant_row(label, detail),
                                    false,
                                )
                            })
                            .on_click(move |_, _, cx| {
                                let (deezer_arl, soundcloud_token) = {
                                    let account = account.read(cx);
                                    (account.deezer_arl(), account.soundcloud_token())
                                };
                                downloads.update(cx, |model, cx| {
                                    model.start_batch(
                                        tracks.clone(),
                                        deezer_arl,
                                        soundcloud_token,
                                        variant,
                                        cx,
                                    );
                                });
                            }),
                        )
                    };
                    if provider == Some(PlaybackProvider::Deezer) {
                        deezer_collection_download_choices(deezer_arl, murglar_token)
                            .iter()
                            .copied()
                            .fold(menu, |menu, choice| {
                                add(menu, choice.label, choice.detail, choice.variant)
                            })
                    } else {
                        let menu = if availability.best {
                            add(
                                menu,
                                DownloadVariant::Best.label(),
                                super::items::soundcloud_download_detail(DownloadVariant::Best),
                                DownloadVariant::Best,
                            )
                        } else {
                            menu
                        };
                        let menu = if availability.original {
                            add(
                                menu,
                                DownloadVariant::Original.label(),
                                super::items::soundcloud_download_detail(DownloadVariant::Original),
                                DownloadVariant::Original,
                            )
                        } else {
                            menu
                        };
                        let menu = if availability.murglar {
                            add(
                                menu,
                                DownloadVariant::Murglar.label(),
                                super::items::soundcloud_download_detail(DownloadVariant::Murglar),
                                DownloadVariant::Murglar,
                            )
                        } else {
                            menu
                        };
                        if availability.standard {
                            add(
                                menu,
                                DownloadVariant::Standard.label(),
                                super::items::soundcloud_download_detail(DownloadVariant::Standard),
                                DownloadVariant::Standard,
                            )
                        } else {
                            menu
                        }
                    }
                },
            )
        })
        .into_any_element()
}
