use crate::{
    app_tooltip::AppTooltipExt,
    assets::{LocalIcon, local_icon},
    library::Service as LibraryService,
    search::Source,
    theme::{FOREGROUND, MUTED},
};
use gpui::{
    AnimationExt as _, AnyElement, Context, IntoElement, KeyDownEvent, Role, div, prelude::*, px,
    rgb, rgba,
};

use super::RalgrumApp;

fn selector_tab_stop(selected: bool, interactive: bool) -> bool {
    selected && interactive
}

#[derive(Clone, Copy)]
pub(super) struct SourceItem {
    pub(super) label: &'static str,
    pub(super) icon: LocalIcon,
    pub(super) source: Source,
    pub(super) full_width: f32,
    pub(super) compact_width: f32,
    pub(super) label_width: f32,
}

#[derive(Clone, Copy)]
pub(super) struct LibraryServiceItem {
    pub(super) label: &'static str,
    pub(super) icon: LocalIcon,
    pub(super) service: LibraryService,
    pub(super) full_width: f32,
    pub(super) compact_width: f32,
    pub(super) label_width: f32,
}

#[derive(Clone, Copy)]
pub(super) enum PlatformItem {
    Search(SourceItem),
    Library(LibraryServiceItem),
}

fn source_icon(icon: LocalIcon, color: u32) -> gpui::Svg {
    let rendered = local_icon(icon, color).flex_none();
    if matches!(icon, LocalIcon::SoundCloud) {
        rendered.w(px(15.)).h(px(12.))
    } else {
        rendered.size_3()
    }
}

impl RalgrumApp {
    pub(super) fn platform_item(
        &self,
        item: PlatformItem,
        responsive: crate::motion::ResponsiveModeVisual,
        interactive: bool,
        cx: &mut Context<Self>,
    ) -> AnyElement {
        let (label, icon, source, service, full_width, compact_width, label_width) = match item {
            PlatformItem::Search(item) => {
                let service = match item.source {
                    Source::All => None,
                    Source::Deezer => Some(LibraryService::Deezer),
                    Source::SoundCloud => Some(LibraryService::SoundCloud),
                };
                (
                    item.label,
                    item.icon,
                    Some(item.source),
                    service,
                    item.full_width,
                    item.compact_width,
                    item.label_width,
                )
            }
            PlatformItem::Library(item) => (
                item.label,
                item.icon,
                None,
                Some(item.service),
                item.full_width,
                item.compact_width,
                item.label_width,
            ),
        };
        let library_route = source.is_none();
        let selected = if library_route {
            self.library.read(cx).selection().0 == service.expect("library item service")
        } else {
            self.search.read(cx).source() == source.expect("search item source")
        };
        let index = if library_route {
            match service.expect("library item service") {
                LibraryService::Local => 0,
                LibraryService::Deezer => 1,
                LibraryService::SoundCloud => 2,
            }
        } else {
            match source.expect("search item source") {
                Source::All => 0,
                Source::Deezer => 1,
                Source::SoundCloud => 2,
            }
        };
        let focus = if library_route {
            self.library_service_tab_focus[index].clone()
        } else {
            self.source_tab_focus[index].clone()
        };
        let tab_focus = if library_route {
            self.library_service_tab_focus.clone()
        } else {
            self.source_tab_focus.clone()
        };
        let text = if selected { 0xffffff } else { MUTED };
        let label_animation_id = match (source, service) {
            (Some(Source::All), _) => "platform-selector-label-all",
            (Some(Source::Deezer), _) | (_, Some(LibraryService::Deezer)) => {
                "platform-selector-label-deezer"
            }
            (Some(Source::SoundCloud), _) | (_, Some(LibraryService::SoundCloud)) => {
                "platform-selector-label-soundcloud"
            }
            (_, Some(LibraryService::Local)) => "platform-selector-label-local",
            _ => unreachable!("platform item must have a source or service"),
        };
        let item_animation_id = match (source, service) {
            (Some(Source::All), _) => "platform-selector-item-all",
            (Some(Source::Deezer), _) | (_, Some(LibraryService::Deezer)) => {
                "platform-selector-item-deezer"
            }
            (Some(Source::SoundCloud), _) | (_, Some(LibraryService::SoundCloud)) => {
                "platform-selector-item-soundcloud"
            }
            (_, Some(LibraryService::Local)) => "platform-selector-item-local",
            _ => unreachable!("platform item must have a source or service"),
        };
        let responsive = if source == Some(Source::All) {
            crate::motion::ResponsiveModeVisual {
                from: 0.0,
                target: 0.0,
                target_compact: false,
                epoch: responsive.epoch,
            }
        } else {
            responsive
        };
        let (from_width, target_width) = responsive.endpoints(full_width, compact_width);
        let (from_padding, target_padding) = responsive.endpoints(14., 0.);
        let (from_gap, target_gap) = responsive.endpoints(6., 0.);
        let (from_label_width, target_label_width) = responsive.endpoints(label_width, 0.);
        let (from_opacity, target_opacity) = responsive.endpoints(1., 0.);
        let local_optical_offset = if library_route && service == Some(LibraryService::Local) {
            2.
        } else {
            0.
        };
        let label_element = div()
            .flex_none()
            .min_w(px(0.))
            .overflow_hidden()
            .whitespace_nowrap()
            .w(px(target_label_width))
            .opacity(target_opacity)
            .when(local_optical_offset > 0., |this| {
                this.relative().left(px(local_optical_offset))
            })
            .child(label)
            .with_animation(
                (label_animation_id, responsive.epoch),
                crate::motion::content(),
                move |this, delta| {
                    this.w(px(crate::motion::lerp(
                        from_label_width,
                        target_label_width,
                        delta,
                    )))
                    .opacity(crate::motion::lerp(
                        from_opacity,
                        target_opacity,
                        delta,
                    ))
                },
            )
            .into_any_element();
        let item_element = div()
            .id(item_animation_id)
            .role(Role::Tab)
            .aria_label(label)
            .aria_selected(selected)
            .h(px(30.))
            .w(px(target_width))
            .px(px(target_padding))
            .flex()
            .items_center()
            .justify_center()
            .gap(px(target_gap))
            .overflow_hidden()
            .rounded(px(6.))
            .bg(rgba(0x00000000))
            .border_1()
            .border_color(rgba(0x00000000))
            .text_color(rgb(text))
            .text_size(px(13.))
            .font_weight(gpui::FontWeight::MEDIUM)
            .child(
                div()
                    .flex_none()
                    .flex()
                    .items_center()
                    .justify_center()
                    .when(
                        responsive.target_compact || local_optical_offset > 0.,
                        |this| {
                            this.relative().left(px(if responsive.target_compact {
                                2.
                            } else {
                                0.
                            } + local_optical_offset))
                        },
                    )
                    .child(source_icon(icon, text)),
            )
            .when(!responsive.target_compact, |this| this.child(label_element))
            .when(interactive, |this| {
                this.track_focus(&focus)
                    .tab_stop(selector_tab_stop(selected, interactive))
                    .cursor_pointer()
                    .hover(|style| {
                        style.text_color(rgb(if selected { 0xffffff } else { FOREGROUND }))
                    })
                    .when(responsive.target_compact, |this| this.app_tooltip(label))
                    .focus_visible(|style| {
                        style.border_1().border_color(rgb(crate::theme::PRIMARY))
                    })
                    .on_click(cx.listener(move |this, _, window, cx| {
                        if library_route {
                            this.select_library_service(
                                service.expect("library item service"),
                                window,
                                cx,
                            );
                        } else {
                            this.select_source(source.expect("search item source"), window, cx);
                        }
                    }))
                    .on_key_down(cx.listener(move |this, event: &KeyDownEvent, window, cx| {
                        if crate::tab_keyboard::is_activation_key(event.keystroke.key.as_str()) {
                            window.prevent_default();
                            if library_route {
                                this.select_library_service(
                                    service.expect("library item service"),
                                    window,
                                    cx,
                                );
                            } else {
                                this.select_source(source.expect("search item source"), window, cx);
                            }
                            return;
                        }
                        let Some(next) = crate::tab_keyboard::next_tab_index(
                            event.keystroke.key.as_str(),
                            index,
                            tab_focus.len(),
                        ) else {
                            return;
                        };
                        window.prevent_default();
                        if library_route {
                            let next_service = [
                                LibraryService::Local,
                                LibraryService::Deezer,
                                LibraryService::SoundCloud,
                            ][next];
                            this.select_library_service(next_service, window, cx);
                        } else {
                            let next_source =
                                [Source::All, Source::Deezer, Source::SoundCloud][next];
                            this.select_source(next_source, window, cx);
                        }
                        tab_focus[next].focus(window, cx);
                    }))
            });
        item_element
            .with_animation(
                (item_animation_id, responsive.epoch),
                crate::motion::content(),
                move |this, delta| {
                    this.w(px(crate::motion::lerp(from_width, target_width, delta)))
                        .px(px(crate::motion::lerp(from_padding, target_padding, delta)))
                        .gap(px(crate::motion::lerp(from_gap, target_gap, delta)))
                },
            )
            .into_any_element()
    }
}

#[cfg(test)]
mod tests {
    use super::{PlatformItem, selector_tab_stop};
    use crate::{assets::LocalIcon, library::Service as LibraryService};

    #[test]
    fn inactive_selector_layers_have_no_tab_stop() {
        assert!(!selector_tab_stop(false, false));
        assert!(!selector_tab_stop(true, false));
        assert!(selector_tab_stop(true, true));
        assert!(!selector_tab_stop(false, true));
    }

    #[test]
    fn library_platform_items_start_with_local_and_keep_search_separate() {
        let items = [
            PlatformItem::Library(super::LibraryServiceItem {
                label: "Local",
                icon: LocalIcon::FolderOpen,
                service: LibraryService::Local,
                full_width: 70.,
                compact_width: 34.,
                label_width: 32.,
            }),
            PlatformItem::Library(super::LibraryServiceItem {
                label: "Deezer",
                icon: LocalIcon::Deezer,
                service: LibraryService::Deezer,
                full_width: 90.,
                compact_width: 34.,
                label_width: 40.,
            }),
            PlatformItem::Library(super::LibraryServiceItem {
                label: "SoundCloud",
                icon: LocalIcon::SoundCloud,
                service: LibraryService::SoundCloud,
                full_width: 122.,
                compact_width: 34.,
                label_width: 72.,
            }),
        ];

        assert!(
            matches!(items[0], PlatformItem::Library(item) if item.service == LibraryService::Local)
        );
        assert!(
            matches!(items[0], PlatformItem::Library(item) if item.icon == LocalIcon::FolderOpen)
        );
        assert!(
            matches!(items[1], PlatformItem::Library(item) if item.service == LibraryService::Deezer)
        );
        assert!(
            matches!(items[2], PlatformItem::Library(item) if item.service == LibraryService::SoundCloud)
        );
    }
}
