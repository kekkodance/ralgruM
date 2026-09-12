use gpui::{
    AnimationExt, AnyElement, Context, CursorStyle, Entity, IntoElement, KeyDownEvent, Render,
    Role, ScrollHandle, SharedString, Window, div, point, prelude::*, px, rgb,
};
use gpui_component::{
    WindowExt,
    input::{Input, InputEvent, InputState},
    scroll::{Scrollbar, ScrollbarShow},
};

use super::{
    local_persistence::LocalPlaylistMutationOutcome,
    local_playlist_controller::LocalPlaylistCompletion,
    local_playlist_store::LocalPlaylist,
    playlist_client::AddTracksResult,
    playlist_state::CatalogStatus,
    soundcloud_playlist::{
        picker_account_changed, picker_add_account_changed, picker_add_failed,
        playlist_client_unavailable, playlist_login_required,
    },
    view::LibraryView,
};
use crate::app_tooltip::AppTooltipExt;
use crate::playback::PlaybackTrack;
use crate::search::Provider;
use crate::{
    app_button::{primary_button_with_loading, secondary_dialog_button_with_disabled},
    assets::{LocalIcon, local_icon},
    browser_scroll::{BrowserScrollState, BrowserScrollTarget, browser_scroll_surface},
    context_menu::text_field_context_menu,
    dialog_layout::{DialogCloseMotion, DialogCloseTarget, request_dialog_close},
    music_ui::ghost_close_button_with_icon_size,
    theme::{BACKGROUND, BORDER, DANGER, FOREGROUND, MUTED, PRIMARY, SURFACE, SURFACE_RAISED},
};

pub(super) use crate::music_ui::playlist_privacy_icon;

#[derive(Clone, Debug, Eq, PartialEq)]
pub(crate) enum PlaylistPickerTarget {
    Provider {
        track_ids: Vec<String>,
        provider: Provider,
        account_scope: String,
    },
    Local {
        tracks: Vec<PlaybackTrack>,
    },
}

#[derive(Clone, Debug, Eq, PartialEq)]
struct PickerPlaylist {
    id: String,
    title: String,
    subtitle: Option<String>,
    track_count: Option<u64>,
    is_private: Option<bool>,
    local: bool,
}

impl PickerPlaylist {
    fn from_provider(playlist: &super::playlist_client::OwnedPlaylist) -> Self {
        Self {
            id: playlist.id.clone(),
            title: playlist.title.clone(),
            subtitle: playlist_subtitle(playlist),
            track_count: playlist.track_count,
            is_private: Some(playlist.is_private),
            local: false,
        }
    }

    fn from_local(playlist: &LocalPlaylist) -> Self {
        Self {
            id: playlist.id.clone(),
            title: playlist.title.clone(),
            subtitle: nonempty_playlist_text(&playlist.description)
                .map(str::to_owned)
                .or_else(|| Some("Local playlist".into())),
            track_count: Some(playlist.tracks.len() as u64),
            is_private: None,
            local: true,
        }
    }
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
enum PickerMode {
    Provider(Provider),
    Local,
}

impl PlaylistPickerTarget {
    fn mode(&self) -> PickerMode {
        match self {
            Self::Provider { provider, .. } => PickerMode::Provider(*provider),
            Self::Local { .. } => PickerMode::Local,
        }
    }
}

pub(crate) struct PlaylistPicker {
    library: Entity<LibraryView>,
    target: PlaylistPickerTarget,
    selected: Option<String>,
    busy: bool,
    error: Option<SharedString>,
    completed: Option<AddTracksResult>,
    close_motion: DialogCloseMotion,
    create_after_close: bool,
    filter: Entity<InputState>,
    scroll: ScrollHandle,
    browser_scroll: BrowserScrollState,
    last_filter: String,
}

pub(super) const PICKER_DIALOG_WIDTH: f32 = 460.;
pub(super) const PICKER_DIALOG_MAX_WIDTH: f32 = 520.;
pub(super) const PICKER_ROW_HEIGHT: f32 = 44.;
pub(super) const PICKER_ROW_GAP: f32 = 4.;
pub(super) const PICKER_ROW_ICON_GAP: f32 = 14.;
pub(super) const PICKER_ROW_ICON_COLUMN: f32 = 16.;
pub(super) const PICKER_PRIVACY_ICON_SIZE: f32 = 12.;
pub(super) const PICKER_PRIVACY_ICON_GAP: f32 = 6.;
pub(super) const PICKER_PRIVACY_OPTICAL_OFFSET_PX: f32 = 2.;
pub(super) const PICKER_TITLE_SUBTITLE_GAP: f32 = 1.;
/// Hover wash for the selected row. The selected rest state already uses the
/// raised surface with a primary border, so hovering it must shift hue toward
/// primary to read as a state change instead of a no-op.
pub(super) const PICKER_SELECTED_HOVER_BACKGROUND: u32 = 0x6366f13d;
/// Transparent hit padding around each row so adjacent hit areas touch.
/// The 4px inter-row dead zone made edge hovers blink on and off; with the
/// padding the highlight hands off between rows instead. Pitch is unchanged:
/// row height + twice (padding + shell border) still equals `row_stride()`.
pub(super) const PICKER_ROW_HIT_PADDING: f32 = 1.;
pub(super) const PICKER_SCROLLBAR_EDGE_OFFSET_PX: f32 = 14.;
pub(super) const PICKER_LIST_MAX_HEIGHT: f32 = 320.;
pub(super) const PICKER_LIST_VIEWPORT_FRACTION: f32 = 0.6;
const PICKER_DIALOG_CHROME_HEIGHT: f32 = 200.;
const PICKER_ERROR_HEIGHT: f32 = 28.;
pub(super) const PICKER_FILTER_PLACEHOLDER: &str = "What playlist are you looking for?";

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
enum PickerPhase {
    Idle,
    Busy,
    Completed(AddTracksResult),
}

fn picker_phase(busy: bool, completed: Option<AddTracksResult>) -> PickerPhase {
    if let Some(result) = completed {
        PickerPhase::Completed(result)
    } else if busy {
        PickerPhase::Busy
    } else {
        PickerPhase::Idle
    }
}

fn can_confirm(selected: bool, busy: bool, account_changed: bool) -> bool {
    selected && !busy && !account_changed
}

fn can_dismiss(busy: bool) -> bool {
    !busy
}

fn picker_account_error(mode: PickerMode, account_changed: bool) -> Option<&'static str> {
    match (mode, account_changed) {
        (PickerMode::Provider(provider), true) => Some(picker_account_changed(provider)),
        _ => None,
    }
}

pub(super) fn dialog_max_height(viewport_height: f32) -> f32 {
    (viewport_height - 32.).max(0.)
}

pub(super) fn list_max_height(viewport_height: f32) -> f32 {
    (viewport_height * PICKER_LIST_VIEWPORT_FRACTION)
        .min(PICKER_LIST_MAX_HEIGHT)
        .max(0.)
}

pub(super) fn picker_dialog_width(viewport_width: f32) -> f32 {
    (viewport_width - 32.).clamp(0., PICKER_DIALOG_WIDTH)
}

pub(super) fn row_stride() -> f32 {
    PICKER_ROW_HEIGHT + PICKER_ROW_GAP
}

pub(super) fn estimated_dialog_height(
    viewport_height: f32,
    _visible_count: usize,
    has_error: bool,
) -> f32 {
    let mut height = PICKER_DIALOG_CHROME_HEIGHT + list_max_height(viewport_height);
    if has_error {
        height += PICKER_ERROR_HEIGHT;
    }
    height.min(dialog_max_height(viewport_height))
}

pub(super) fn fixed_dialog_height(viewport_height: f32, has_error: bool) -> f32 {
    estimated_dialog_height(viewport_height, usize::MAX, has_error)
}

pub(super) fn toggle_selection(current: Option<String>, clicked_id: &str) -> Option<String> {
    match current {
        Some(selected) if selected == clicked_id => None,
        _ => Some(clicked_id.to_owned()),
    }
}

pub(super) fn track_count_label(count: u64) -> String {
    if count == 1 {
        "1 track".to_owned()
    } else {
        format!("{count} tracks")
    }
}

pub(super) fn playlist_track_meta(track_count: Option<u64>) -> Option<String> {
    track_count.map(track_count_label)
}

pub(super) fn filter_playlists(
    playlists: &[super::playlist_client::OwnedPlaylist],
    query: &str,
) -> Vec<super::playlist_client::OwnedPlaylist> {
    let query = query.trim().to_lowercase();
    if query.is_empty() {
        return playlists.to_vec();
    }
    playlists
        .iter()
        .filter(|playlist| playlist.title.to_lowercase().contains(&query))
        .cloned()
        .collect()
}

fn filter_picker_playlists(playlists: &[PickerPlaylist], query: &str) -> Vec<PickerPlaylist> {
    let query = query.trim().to_lowercase();
    if query.is_empty() {
        return playlists.to_vec();
    }
    playlists
        .iter()
        .filter(|playlist| playlist.title.to_lowercase().contains(&query))
        .cloned()
        .collect()
}

pub(super) fn selection_index_for_id(
    filtered: &[super::playlist_client::OwnedPlaylist],
    selected: Option<&str>,
) -> Option<usize> {
    let selected = selected?;
    filtered.iter().position(|playlist| playlist.id == selected)
}

fn selection_index_for_picker_id(
    filtered: &[PickerPlaylist],
    selected: Option<&str>,
) -> Option<usize> {
    let selected = selected?;
    filtered.iter().position(|playlist| playlist.id == selected)
}

pub(super) fn move_selection(current: Option<usize>, delta: isize, len: usize) -> Option<usize> {
    if len == 0 {
        return None;
    }
    let Some(current) = current else {
        return Some(if delta < 0 { len - 1 } else { 0 });
    };
    let next = (current as isize + delta).clamp(0, len as isize - 1) as usize;
    Some(next)
}

pub(super) fn selection_scroll_offset(
    current_offset_y: f32,
    index: usize,
    list_max: f32,
    content_count: usize,
) -> f32 {
    if list_max <= 0. || content_count == 0 {
        return 0.;
    }
    let stride = row_stride();
    let top = index as f32 * stride;
    let bottom = top + PICKER_ROW_HEIGHT;
    let visible_top = -current_offset_y;
    let visible_bottom = visible_top + list_max;
    let mut target = current_offset_y;
    if top < visible_top {
        target = -top;
    } else if bottom > visible_bottom {
        target = -(bottom - list_max);
    }
    let content_height = content_count as f32 * stride;
    let min_offset = -(content_height - list_max).max(0.);
    target.clamp(min_offset, 0.)
}

fn playlist_subtitle(playlist: &super::playlist_client::OwnedPlaylist) -> Option<String> {
    let owner = playlist.owner.name.trim();
    if owner.is_empty() {
        None
    } else {
        Some(owner.to_owned())
    }
}

fn nonempty_playlist_text(value: &str) -> Option<&str> {
    (!value.trim().is_empty()).then_some(value.trim())
}

impl PlaylistPicker {
    pub(crate) fn open(
        library: Entity<LibraryView>,
        track_ids: Vec<String>,
        account_scope: String,
        provider: Provider,
        window: &mut Window,
        cx: &mut Context<LibraryView>,
    ) {
        if track_ids.is_empty() {
            return;
        }
        Self::open_target(
            library,
            PlaylistPickerTarget::Provider {
                track_ids,
                provider,
                account_scope,
            },
            window,
            cx,
        );
    }

    pub(crate) fn open_local(
        library: Entity<LibraryView>,
        tracks: Vec<PlaybackTrack>,
        window: &mut Window,
        cx: &mut Context<LibraryView>,
    ) {
        if tracks.is_empty() {
            return;
        }
        Self::open_target(library, PlaylistPickerTarget::Local { tracks }, window, cx);
    }

    fn open_target(
        library: Entity<LibraryView>,
        target: PlaylistPickerTarget,
        window: &mut Window,
        cx: &mut Context<LibraryView>,
    ) {
        let filter =
            cx.new(|cx| InputState::new(window, cx).placeholder(PICKER_FILTER_PLACEHOLDER));
        let filter_for_focus = filter.clone();
        let picker = cx.new(|cx| {
            let filter_for_changes = filter.clone();
            cx.subscribe(&filter_for_changes, |_, _, event, cx| {
                if matches!(event, InputEvent::Change) {
                    cx.notify();
                }
            })
            .detach();
            cx.observe(&library, |_, _, cx| cx.notify()).detach();
            Self {
                library: library.clone(),
                target,
                selected: None,
                busy: false,
                error: None,
                completed: None,
                close_motion: DialogCloseMotion::default(),
                create_after_close: false,
                filter,
                scroll: ScrollHandle::new(),
                browser_scroll: BrowserScrollState::new(),
                last_filter: String::new(),
            }
        });
        let picker_content = picker.clone();
        window.open_dialog(cx, move |dialog, dialog_window, cx| {
            let viewport_width = f32::from(dialog_window.viewport_size().width);
            let viewport_height = f32::from(dialog_window.viewport_size().height);
            let state = picker_content.read(cx);
            let fixed = fixed_dialog_height(viewport_height, state.error.is_some());
            let centered_top = crate::dialog_layout::centered_margin_top(viewport_height, fixed);
            let closing = state.close_motion.closing();
            let close_epoch = state.close_motion.epoch();
            let cancel_handle = picker_content.clone();
            dialog
                .w(px(picker_dialog_width(viewport_width)))
                .max_w(px(PICKER_DIALOG_MAX_WIDTH))
                .h(px(fixed))
                .min_h(px(fixed))
                .max_h(px(dialog_max_height(viewport_height)))
                .margin_top(centered_top)
                .p_0()
                .gap_0()
                .bg(gpui::rgba(0x00000000))
                .border_0()
                .rounded(px(8.))
                .close_button(false)
                .overlay(true)
                .overlay_closable(true)
                .keyboard(false)
                .on_cancel(move |_, window, cx| {
                    if can_dismiss(cancel_handle.read(cx).busy) {
                        cancel_handle.update(cx, |this, cx| request_dialog_close(this, window, cx));
                    }
                    false
                })
                .closing(closing, close_epoch)
                .child(picker_content.clone())
        });
        filter_for_focus.update(cx, |input, cx| input.focus(window, cx));
    }

    fn mode(&self) -> PickerMode {
        self.target.mode()
    }

    fn is_local(&self) -> bool {
        matches!(self.mode(), PickerMode::Local)
    }

    fn account_changed(&self, cx: &Context<Self>) -> bool {
        match &self.target {
            PlaylistPickerTarget::Provider { account_scope, .. } => {
                self.library.read(cx).account.read(cx).library_scope() != *account_scope
            }
            PlaylistPickerTarget::Local { .. } => false,
        }
    }

    fn provider_status(&self, cx: &Context<Self>) -> Option<CatalogStatus> {
        match self.mode() {
            PickerMode::Provider(provider) => Some(
                self.library
                    .read(cx)
                    .playlist_catalog(provider)
                    .status
                    .clone(),
            ),
            PickerMode::Local => None,
        }
    }

    fn playlists(&self, cx: &Context<Self>) -> Vec<PickerPlaylist> {
        match &self.target {
            PlaylistPickerTarget::Provider { provider, .. } => self
                .library
                .read(cx)
                .playlist_catalog(*provider)
                .editable_playlists()
                .iter()
                .map(PickerPlaylist::from_provider)
                .collect(),
            PlaylistPickerTarget::Local { .. } => self
                .library
                .read(cx)
                .local_playlists
                .as_ref()
                .map(|store| {
                    store
                        .playlists()
                        .iter()
                        .map(PickerPlaylist::from_local)
                        .collect()
                })
                .unwrap_or_default(),
        }
    }

    fn submit(&mut self, cx: &mut Context<Self>) {
        if self.busy {
            return;
        }
        let Some(playlist_id) = self.selected.clone() else {
            return;
        };
        match self.target.clone() {
            PlaylistPickerTarget::Local { tracks } => {
                self.busy = true;
                self.error = None;
                let picker = cx.entity().clone();
                let result = self.library.update(cx, |library, cx| {
                    let completion: LocalPlaylistCompletion = Box::new(
                        move |result: Result<
                            LocalPlaylistMutationOutcome,
                            super::local_playlist_store::LocalPlaylistError,
                        >,
                              _view: &mut LibraryView,
                              cx: &mut Context<LibraryView>| {
                            picker.update(cx, |picker, cx| {
                                picker.busy = false;
                                match result {
                                    Ok(LocalPlaylistMutationOutcome::Added { count }) => {
                                        picker.completed = Some(AddTracksResult::Added { count });
                                    }
                                    Ok(LocalPlaylistMutationOutcome::AlreadyPresent { .. }) => {
                                        picker.completed =
                                            Some(AddTracksResult::AlreadyPresent { count: 1 });
                                    }
                                    Err(error) => picker.error = Some(error.to_string().into()),
                                    _ => {}
                                }
                                cx.notify();
                            });
                        },
                    );
                    library.add_tracks_to_local_playlist(playlist_id, &tracks, completion, cx)
                });
                if let Err(error) = result {
                    self.busy = false;
                    self.error = Some(error.to_string().into());
                }
                cx.notify();
            }
            PlaylistPickerTarget::Provider {
                provider,
                account_scope,
                ..
            } => {
                let scope = self.library.read(cx).account.read(cx).library_scope();
                if scope != account_scope {
                    self.error = Some(picker_account_changed(provider).into());
                    cx.notify();
                    return;
                }
                match provider {
                    Provider::Deezer => self.submit_deezer(scope, playlist_id, cx),
                    Provider::SoundCloud => self.submit_soundcloud(scope, playlist_id, cx),
                }
            }
        }
    }

    fn submit_deezer(&mut self, scope: String, playlist_id: String, cx: &mut Context<Self>) {
        let Some(arl) = self.library.read(cx).account.read(cx).deezer_arl() else {
            self.error = Some(playlist_login_required(Provider::Deezer).into());
            cx.notify();
            return;
        };
        let user_id = self.library.read(cx).account.read(cx).deezer_user_id();
        let client = match self.library.read(cx).playlist_client.clone() {
            Ok(client) => client,
            Err(_) => {
                self.error = Some(playlist_client_unavailable(Provider::Deezer).into());
                cx.notify();
                return;
            }
        };
        let Some(generation) = self.library.update(cx, |library, _| {
            library.begin_playlist_add(&scope, &playlist_id, Provider::Deezer)
        }) else {
            self.error = Some("The selected playlist is no longer editable.".into());
            cx.notify();
            return;
        };
        let track_ids = match &self.target {
            PlaylistPickerTarget::Provider { track_ids, .. } => track_ids.clone(),
            PlaylistPickerTarget::Local { .. } => return,
        };
        let task_playlist_id = playlist_id.clone();
        let account_scope = match &self.target {
            PlaylistPickerTarget::Provider { account_scope, .. } => account_scope.clone(),
            PlaylistPickerTarget::Local { .. } => return,
        };
        let library = self.library.clone();
        let task = self.library.read(cx).runtime.spawn(async move {
            client
                .add_tracks(arl, user_id, &task_playlist_id, &track_ids)
                .await
        });
        self.busy = true;
        self.error = None;
        cx.spawn(async move |this, cx| {
            let result = task
                .await
                .unwrap_or_else(|_| Err(picker_add_failed(Provider::Deezer).into()));
            this.update(cx, |this, cx| {
                this.busy = false;
                let accepted = library.update(cx, |library, cx| {
                    library.finish_playlist_add(
                        &account_scope,
                        generation,
                        &playlist_id,
                        &result,
                        cx,
                    )
                });
                if !accepted {
                    this.error = Some(picker_add_account_changed(Provider::Deezer).into());
                } else if let Err(error) = result {
                    this.error = Some(error.into());
                } else {
                    this.completed = result.ok();
                }
                cx.notify();
            })
            .ok();
        })
        .detach();
        cx.notify();
    }

    fn submit_soundcloud(&mut self, scope: String, playlist_id: String, cx: &mut Context<Self>) {
        let Some(token) = self
            .library
            .read(cx)
            .account
            .read(cx)
            .soundcloud_mobile_token()
        else {
            self.error = Some(playlist_login_required(Provider::SoundCloud).into());
            cx.notify();
            return;
        };
        let client = match self.library.read(cx).soundcloud_library_client() {
            Ok(client) => client,
            Err(_) => {
                self.error = Some(playlist_client_unavailable(Provider::SoundCloud).into());
                cx.notify();
                return;
            }
        };
        let Some(generation) = self.library.update(cx, |library, _| {
            library.begin_playlist_add(&scope, &playlist_id, Provider::SoundCloud)
        }) else {
            self.error = Some("The selected playlist is no longer editable.".into());
            cx.notify();
            return;
        };
        let track_ids = match &self.target {
            PlaylistPickerTarget::Provider { track_ids, .. } => track_ids.clone(),
            PlaylistPickerTarget::Local { .. } => return,
        };
        let task_playlist_id = playlist_id.clone();
        let account_scope = match &self.target {
            PlaylistPickerTarget::Provider { account_scope, .. } => account_scope.clone(),
            PlaylistPickerTarget::Local { .. } => return,
        };
        let library = self.library.clone();
        let task = self.library.read(cx).runtime.spawn(async move {
            client
                .add_tracks_to_playlist(token, &task_playlist_id, &track_ids)
                .await
        });
        self.busy = true;
        self.error = None;
        cx.spawn(async move |this, cx| {
            let result = task
                .await
                .unwrap_or_else(|_| Err(picker_add_failed(Provider::SoundCloud).into()));
            this.update(cx, |this, cx| {
                this.busy = false;
                let accepted = library.update(cx, |library, cx| {
                    library.finish_soundcloud_playlist_add(
                        &account_scope,
                        generation,
                        &playlist_id,
                        &result,
                        cx,
                    )
                });
                if !accepted {
                    this.error = Some(picker_add_account_changed(Provider::SoundCloud).into());
                } else if let Err(error) = result {
                    this.error = Some(error.into());
                } else {
                    this.completed = result.ok();
                }
                cx.notify();
            })
            .ok();
        })
        .detach();
        cx.notify();
    }

    fn move_keyboard_selection(
        &mut self,
        delta: isize,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        let query = self.filter.read(cx).value().to_string();
        let playlists = self.playlists(cx);
        let filtered = filter_picker_playlists(&playlists, &query);
        if filtered.is_empty() {
            return;
        }
        let current = selection_index_for_picker_id(&filtered, self.selected.as_deref());
        let Some(next) = move_selection(current, delta, filtered.len()) else {
            return;
        };
        self.selected = Some(filtered[next].id.clone());
        self.error = None;
        let list_max = list_max_height(f32::from(window.viewport_size().height));
        let current_offset = f32::from(self.scroll.offset().y);
        let target = selection_scroll_offset(current_offset, next, list_max, filtered.len());
        self.scroll.set_offset(point(px(0.), px(target)));
        cx.notify();
    }

    fn sync_filter_scroll(&mut self, cx: &mut Context<Self>) {
        let query = self.filter.read(cx).value().to_string();
        if query != self.last_filter {
            self.last_filter = query;
            self.scroll.set_offset(point(px(0.), px(0.)));
            self.browser_scroll.reset();
        }
    }
}

impl DialogCloseTarget for PlaylistPicker {
    fn dialog_close_motion(&mut self) -> &mut DialogCloseMotion {
        &mut self.close_motion
    }

    fn after_dialog_close(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        if !self.create_after_close {
            return;
        }
        self.create_after_close = false;
        let target = self.target.clone();
        self.library.update(cx, |library, cx| match target {
            PlaylistPickerTarget::Provider {
                track_ids,
                provider,
                ..
            } => match provider {
                Provider::Deezer => library.open_playlist_create(track_ids, window, cx),
                Provider::SoundCloud => {
                    library.open_soundcloud_playlist_create_with_tracks(track_ids, window, cx)
                }
            },
            PlaylistPickerTarget::Local { tracks } => {
                super::local_playlist_dialog::LocalPlaylistDialog::open_create_with_tracks(
                    cx.entity(),
                    tracks,
                    window,
                    cx,
                )
            }
        });
    }
}

impl Render for PlaylistPicker {
    fn render(&mut self, window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        if matches!(
            picker_phase(self.busy, self.completed),
            PickerPhase::Completed(_)
        ) {
            request_dialog_close(self, window, cx);
        }
        self.sync_filter_scroll(cx);
        let closing = self.close_motion.closing();
        let close_epoch = self.close_motion.epoch();
        let local_mode = self.is_local();
        let status = self.provider_status(cx);
        let playlists = self.playlists(cx);
        let provider_ready = local_mode
            || status
                .as_ref()
                .is_some_and(|status| matches!(status, CatalogStatus::Ready));
        let provider_for_ui = match self.mode() {
            PickerMode::Provider(provider) => Some(provider),
            PickerMode::Local => None,
        };
        let account_changed = self.account_changed(cx);
        let account_error = picker_account_error(self.mode(), account_changed);
        let query = self.filter.read(cx).value().to_string();
        let filtered = filter_picker_playlists(&playlists, &query);
        let selected_visible = self
            .selected
            .as_deref()
            .is_some_and(|id| filtered.iter().any(|playlist| playlist.id == id));
        let viewport_height = f32::from(window.viewport_size().height);
        let list_max = list_max_height(viewport_height);
        let fixed_height =
            fixed_dialog_height(viewport_height, self.error.is_some() || account_changed);
        let dialog_entity = cx.entity();
        let dialog = div()
            .id("playlist-picker-dialog")
            .on_key_down(cx.listener(|this, event: &KeyDownEvent, window, cx| {
                match event.keystroke.key.as_str() {
                    "escape" => {
                        window.prevent_default();
                        if can_dismiss(this.busy) {
                            request_dialog_close(this, window, cx);
                        }
                        cx.stop_propagation();
                    }
                    "enter" => {
                        let query = this.filter.read(cx).value().to_string();
                        let playlists = this.playlists(cx);
                        let filtered = filter_picker_playlists(&playlists, &query);
                        let visible = this
                            .selected
                            .as_deref()
                            .is_some_and(|id| filtered.iter().any(|playlist| playlist.id == id));
                        let account_changed = this.account_changed(cx);
                        if can_confirm(visible, this.busy, account_changed) {
                            window.prevent_default();
                            this.submit(cx);
                            cx.stop_propagation();
                        }
                    }
                    "up" => {
                        window.prevent_default();
                        this.move_keyboard_selection(-1, window, cx);
                        cx.stop_propagation();
                    }
                    "down" => {
                        window.prevent_default();
                        this.move_keyboard_selection(1, window, cx);
                        cx.stop_propagation();
                    }
                    _ => {}
                }
            }))
            .overflow_hidden()
            .rounded(px(8.))
            .border_1()
            .border_color(rgb(BORDER))
            .bg(rgb(BACKGROUND))
            .text_color(rgb(FOREGROUND))
            .h(px(fixed_height))
            .min_h(px(fixed_height))
            .max_h(px(dialog_max_height(viewport_height)))
            .child(
                div()
                    .flex()
                    .items_center()
                    .justify_between()
                    .gap(px(8.))
                    .pl(px(18.))
                    .pr(px(14.))
                    .py(px(16.))
                    .border_b_1()
                    .border_color(rgb(BORDER))
                    .bg(rgb(BACKGROUND))
                    .child(
                        div()
                            .flex()
                            .items_center()
                            .gap(px(8.))
                            .font_weight(gpui::FontWeight::SEMIBOLD)
                            .child(picker_brand_icon(self.mode()))
                            .child(picker_title(self.mode())),
                    )
                    .child(ghost_close_button_with_icon_size(
                        "playlist-picker-close",
                        10.,
                        move |_, window, cx| {
                            dialog_entity.update(cx, |this, cx| {
                                if can_dismiss(this.busy) {
                                    request_dialog_close(this, window, cx);
                                }
                            });
                        },
                    )),
            )
            .child(
                div()
                    .flex()
                    .flex_col()
                    .gap(px(12.))
                    .p(px(18.))
                    .pb(px(14.))
                    .bg(rgb(BACKGROUND))
                    .when_some(account_error, |this, error| this.child(error_text(error)))
                    .child(div().flex_1().min_w_0().child(text_field_context_menu(
                        Input::new(&self.filter).bg(rgb(SURFACE)),
                        self.filter.clone(),
                    )))
                    .when(
                        status
                            .as_ref()
                            .is_some_and(|status| matches!(status, CatalogStatus::Loading)),
                        |this| {
                            this.child(fixed_list_container(
                                list_max,
                                div()
                                    .w_full()
                                    .h_full()
                                    .flex()
                                    .items_center()
                                    .justify_center()
                                    .text_color(rgb(MUTED))
                                    .child("Loading playlists..."),
                            ))
                        },
                    )
                    .when(
                        status
                            .as_ref()
                            .is_some_and(|status| matches!(status, CatalogStatus::Unavailable)),
                        |this| {
                            let provider = provider_for_ui.expect("provider status has provider");
                            this.child(fixed_list_container(
                                list_max,
                                div()
                                    .w_full()
                                    .h_full()
                                    .flex()
                                    .items_center()
                                    .justify_center()
                                    .child(error_text(playlist_login_required(provider))),
                            ))
                        },
                    )
                    .when_some(
                        status.as_ref().and_then(|status| match status {
                            CatalogStatus::Failed(error) => Some(error.clone()),
                            _ => None,
                        }),
                        |this, error| {
                            this.child(fixed_list_container(
                                list_max,
                                div()
                                    .w_full()
                                    .h_full()
                                    .flex()
                                    .items_center()
                                    .justify_center()
                                    .child(error_text(error)),
                            ))
                        },
                    )
                    .when(provider_ready && playlists.is_empty(), |this| {
                        this.child(fixed_list_container(
                            list_max,
                            crate::empty_state::render(
                                if local_mode {
                                    LocalIcon::FolderOpen
                                } else {
                                    LocalIcon::ListUl
                                },
                                if local_mode {
                                    "No Local playlists yet"
                                } else {
                                    "No editable playlists"
                                },
                                if local_mode {
                                    "Create a Local playlist to get started."
                                } else {
                                    "Create a new playlist to get started."
                                },
                                None,
                            ),
                        ))
                    })
                    .when(
                        provider_ready && !playlists.is_empty() && filtered.is_empty(),
                        |this| {
                            this.child(fixed_list_container(
                                list_max,
                                crate::empty_state::render(
                                    LocalIcon::MagnifyingGlass,
                                    "No matching playlists",
                                    "Try a different filter.",
                                    None,
                                ),
                            ))
                        },
                    )
                    .when(provider_ready && !filtered.is_empty(), |this| {
                        let rows = filtered
                            .iter()
                            .map(|playlist| {
                                picker_row(
                                    playlist,
                                    self.selected.as_deref() == Some(&playlist.id),
                                    self.busy || account_changed,
                                    cx,
                                )
                            })
                            .collect::<Vec<_>>();
                        // Row spacing lives in each row's hit padding so
                        // adjacent hit areas touch; see PICKER_ROW_HIT_PADDING.
                        let list_content = div()
                            .id("playlist-picker-list")
                            .flex()
                            .flex_col()
                            .gap(px(0.))
                            .track_scroll(&self.scroll)
                            .overflow_y_scroll()
                            .h(px(list_max))
                            .w_full()
                            .min_w_0()
                            .pr(px(10.))
                            .pb(px(4.))
                            .children(rows)
                            .into_any_element();
                        let scrolled = browser_scroll_surface(
                            "playlist-picker-scroll",
                            list_content,
                            BrowserScrollTarget::Handle(self.scroll.clone()),
                            self.browser_scroll.clone(),
                        );
                        this.child(
                            div()
                                .relative()
                                .w_full()
                                .h(px(list_max))
                                .child(scrolled)
                                .child(
                                    div()
                                        .absolute()
                                        .top_0()
                                        .bottom_0()
                                        .left_0()
                                        .right(px(-PICKER_SCROLLBAR_EDGE_OFFSET_PX))
                                        .child(
                                            Scrollbar::vertical(&self.scroll)
                                                .scrollbar_show(ScrollbarShow::Hover),
                                        ),
                                ),
                        )
                    })
                    .when_some(self.error.clone(), |this, error| {
                        this.child(error_text(error))
                    }),
            )
            .child(
                div()
                    .flex()
                    .items_center()
                    .justify_between()
                    .gap(px(8.))
                    .px(px(18.))
                    .py(px(14.))
                    .border_t_1()
                    .border_color(rgb(BORDER))
                    .bg(rgb(BACKGROUND))
                    .child(secondary_dialog_button_with_disabled(
                        "create-playlist-from-picker",
                        None,
                        "Create Playlist",
                        self.busy || account_changed,
                        cx.listener(|this, _, window, cx| {
                            if !this.busy {
                                this.create_after_close = true;
                                request_dialog_close(this, window, cx);
                            }
                        }),
                    ))
                    .child(
                        div()
                            .flex()
                            .items_center()
                            .justify_end()
                            .gap(px(8.))
                            .when(
                                status.as_ref().is_some_and(|status| {
                                    matches!(status, CatalogStatus::Failed(_))
                                }),
                                |this| {
                                    this.child(secondary_dialog_button_with_disabled(
                                        "retry-playlist-catalog",
                                        None,
                                        "Retry",
                                        self.busy,
                                        cx.listener(|this, _, _, cx| {
                                            this.library.update(cx, |library, cx| {
                                                match this.mode() {
                                                    PickerMode::Provider(Provider::Deezer) => {
                                                        library.retry_playlist_catalog(cx)
                                                    }
                                                    PickerMode::Provider(Provider::SoundCloud) => {
                                                        library
                                                            .retry_soundcloud_playlist_catalog(cx)
                                                    }
                                                    PickerMode::Local => {}
                                                }
                                            })
                                        }),
                                    ))
                                },
                            )
                            .child(secondary_dialog_button_with_disabled(
                                "cancel-playlist-picker",
                                None,
                                "Cancel",
                                self.busy,
                                cx.listener(|this, _, window, cx| {
                                    if !this.busy {
                                        request_dialog_close(this, window, cx);
                                    }
                                }),
                            ))
                            .child(primary_button_with_loading(
                                "confirm-playlist-picker",
                                Some(LocalIcon::Plus),
                                if self.busy { "Adding..." } else { "Add track" },
                                !can_confirm(selected_visible, self.busy, account_changed),
                                self.busy,
                                cx.listener(|this, _, _, cx| this.submit(cx)),
                            )),
                    ),
            );

        if closing {
            dialog
                .with_animation(
                    ("playlist-picker-close", close_epoch),
                    crate::motion::dialog_close(),
                    |this, delta| this.opacity(crate::motion::lerp(1.0, 0.0, delta)),
                )
                .into_any_element()
        } else {
            dialog.into_any_element()
        }
    }
}

fn fixed_list_container(list_max: f32, content: impl IntoElement) -> impl IntoElement {
    div()
        .relative()
        .w_full()
        .h(px(list_max))
        .overflow_hidden()
        .child(
            div()
                .w_full()
                .h_full()
                .flex()
                .flex_col()
                .justify_center()
                .child(content),
        )
}

fn picker_row(
    playlist: &PickerPlaylist,
    selected: bool,
    disabled: bool,
    cx: &mut Context<PlaylistPicker>,
) -> AnyElement {
    let id = playlist.id.clone();
    let select_id = id.clone();
    let toggle_id = id.clone();
    let submit_id = id.clone();
    let row_group = format!("playlist-picker-row-{id}");
    let title = playlist.title.clone();
    let subtitle = playlist.subtitle.clone();
    let right_meta = playlist_track_meta(playlist.track_count);
    let privacy = playlist.is_private.map(playlist_privacy_icon);
    div()
        .id(row_group.clone())
        .group(row_group.clone())
        .focusable()
        .tab_stop(!disabled)
        .role(Role::Button)
        .aria_label(title.clone())
        .w_full()
        .min_w_0()
        .flex_none()
        .py(px(PICKER_ROW_HIT_PADDING))
        .rounded(px(8.))
        .border_1()
        .border_color(gpui::rgba(0x00000000))
        .when(!disabled, |this| {
            this.cursor(CursorStyle::PointingHand)
                .focus_visible(|style| style.border_color(rgb(PRIMARY)))
        })
        .when(disabled, |this| {
            this.opacity(0.6).cursor(CursorStyle::OperationNotAllowed)
        })
        .on_click(cx.listener(move |this, _, _, cx| {
            if disabled {
                return;
            }
            this.selected = toggle_selection(this.selected.clone(), &select_id);
            this.error = None;
            cx.notify();
        }))
        .on_key_down(cx.listener(move |this, event: &KeyDownEvent, window, cx| {
            let key = event.keystroke.key.as_str();
            if key == "space" {
                window.prevent_default();
                if !disabled {
                    this.selected = toggle_selection(this.selected.clone(), &toggle_id);
                    this.error = None;
                    cx.notify();
                }
                cx.stop_propagation();
            } else if key == "enter" {
                window.prevent_default();
                if !disabled {
                    this.selected = Some(submit_id.clone());
                    this.error = None;
                    this.submit(cx);
                }
                cx.stop_propagation();
            }
        }))
        .child(
            div()
                .flex()
                .flex_none()
                .items_center()
                .gap(px(PICKER_ROW_ICON_GAP))
                .px(px(10.))
                .h(px(PICKER_ROW_HEIGHT))
                .w_full()
                .min_w_0()
                .overflow_hidden()
                .rounded(px(6.))
                .border_1()
                .border_color(if selected {
                    rgb(PRIMARY)
                } else {
                    gpui::rgba(0x00000000)
                })
                .bg(if selected {
                    rgb(SURFACE_RAISED)
                } else {
                    gpui::rgba(0x00000000)
                })
                .group_hover(row_group, |style| {
                    if selected {
                        style
                            .bg(gpui::rgba(PICKER_SELECTED_HOVER_BACKGROUND))
                            .border_color(rgb(PRIMARY))
                    } else {
                        style.bg(rgb(SURFACE)).border_color(rgb(BORDER))
                    }
                })
                .child(
                    div()
                        .w(px(PICKER_ROW_ICON_COLUMN))
                        .flex_none()
                        .flex()
                        .items_center()
                        .justify_center()
                        .child(
                            div().relative().child(
                                local_icon(
                                    if selected {
                                        LocalIcon::Check
                                    } else if playlist.local {
                                        LocalIcon::FolderOpen
                                    } else {
                                        LocalIcon::ListUl
                                    },
                                    if selected { PRIMARY } else { MUTED },
                                )
                                .size(px(14.)),
                            ),
                        ),
                )
                .child(
                    div()
                        .flex_1()
                        .min_w_0()
                        .flex()
                        .flex_col()
                        .gap(px(PICKER_TITLE_SUBTITLE_GAP))
                        .child(
                            div()
                                .flex()
                                .items_center()
                                .gap(px(PICKER_PRIVACY_ICON_GAP))
                                .min_w_0()
                                .max_w_full()
                                .child(
                                    div()
                                        .min_w_0()
                                        .max_w_full()
                                        .flex_shrink_1()
                                        .truncate()
                                        .overflow_hidden()
                                        .text_size(px(13.))
                                        .font_weight(gpui::FontWeight::MEDIUM)
                                        .text_color(rgb(FOREGROUND))
                                        .child(title),
                                )
                                .when_some(privacy, |this, privacy| {
                                    this.child(
                                        div()
                                            .id(format!("playlist-picker-privacy-{id}"))
                                            .flex_none()
                                            .flex()
                                            .items_center()
                                            .justify_center()
                                            .relative()
                                            .top(px(PICKER_PRIVACY_OPTICAL_OFFSET_PX))
                                            .app_tooltip(if playlist.is_private == Some(true) {
                                                "Private"
                                            } else {
                                                "Public"
                                            })
                                            .child(
                                                local_icon(privacy, MUTED)
                                                    .size(px(PICKER_PRIVACY_ICON_SIZE)),
                                            ),
                                    )
                                }),
                        )
                        .when_some(subtitle, |this, subtitle| {
                            this.child(
                                div()
                                    .min_w_0()
                                    .truncate()
                                    .overflow_hidden()
                                    .text_size(px(11.))
                                    .text_color(rgb(MUTED))
                                    .child(subtitle),
                            )
                        }),
                )
                .when_some(right_meta, |this, meta| {
                    this.child(
                        div()
                            .flex_none()
                            .whitespace_nowrap()
                            .truncate()
                            .text_size(px(11.))
                            .text_color(rgb(MUTED))
                            .child(meta),
                    )
                }),
        )
        .into_any_element()
}

fn picker_brand_icon(mode: PickerMode) -> impl IntoElement {
    match mode {
        PickerMode::Local => local_icon(LocalIcon::FolderOpen, PRIMARY).size_4(),
        PickerMode::Provider(Provider::Deezer) => local_icon(LocalIcon::Deezer, 0xa238ff).size_4(),
        PickerMode::Provider(Provider::SoundCloud) => {
            local_icon(LocalIcon::SoundCloud, crate::theme::SOUNDCLOUD).size_4()
        }
    }
}

fn picker_title(mode: PickerMode) -> &'static str {
    match mode {
        PickerMode::Local => "Add to Local playlist",
        PickerMode::Provider(Provider::Deezer) => "Add to Deezer playlist",
        PickerMode::Provider(Provider::SoundCloud) => "Add to SoundCloud playlist",
    }
}

fn error_text(error: impl Into<SharedString>) -> impl IntoElement {
    div()
        .text_size(px(12.))
        .text_color(rgb(DANGER))
        .child(error.into())
}

#[cfg(test)]
mod tests {
    use super::{
        AddTracksResult, PICKER_FILTER_PLACEHOLDER, PickerMode, PickerPhase, PickerPlaylist,
        PlaylistPickerTarget, can_confirm, can_dismiss, dialog_max_height, estimated_dialog_height,
        filter_picker_playlists, filter_playlists, fixed_dialog_height, list_max_height,
        move_selection, picker_account_error, picker_dialog_width, picker_phase,
        playlist_privacy_icon, playlist_track_meta, selection_index_for_id,
        selection_index_for_picker_id, selection_scroll_offset, toggle_selection,
        track_count_label,
    };
    use crate::library::playlist_client::{OwnedPlaylist, PlaylistOwner};
    use crate::search::Provider;

    fn playlist(id: &str, title: &str) -> OwnedPlaylist {
        OwnedPlaylist {
            id: id.into(),
            title: title.into(),
            description: String::new(),
            is_private: false,
            is_from_favorite_tracks: false,
            is_collaborative: false,
            owner: PlaylistOwner {
                id: "7".into(),
                name: "Owner".into(),
            },
            artwork: String::new(),
            track_count: None,
        }
    }

    #[test]
    fn picker_state_transitions_disable_confirmation_and_distinguish_results() {
        assert!(!can_confirm(false, false, false));
        assert!(can_confirm(true, false, false));
        assert!(!can_confirm(true, true, false));
        assert!(!can_confirm(true, false, true));
        assert_eq!(picker_phase(false, None), PickerPhase::Idle);
        assert_eq!(picker_phase(true, None), PickerPhase::Busy);
        assert_eq!(
            picker_phase(false, Some(AddTracksResult::Added { count: 1 })),
            PickerPhase::Completed(AddTracksResult::Added { count: 1 })
        );
        assert_eq!(
            picker_phase(false, Some(AddTracksResult::AlreadyPresent { count: 1 })),
            PickerPhase::Completed(AddTracksResult::AlreadyPresent { count: 1 })
        );
    }

    #[test]
    fn picker_filter_matches_titles_case_insensitively() {
        let playlists = vec![
            playlist("1", "Morning Run"),
            playlist("2", "evening chill"),
            playlist("3", "Workout Mix"),
        ];
        assert_eq!(filter_playlists(&playlists, "").len(), 3);
        assert_eq!(filter_playlists(&playlists, "  ").len(), 3);
        let filtered = filter_playlists(&playlists, "EVEN");
        assert_eq!(
            filtered.iter().map(|p| p.id.as_str()).collect::<Vec<_>>(),
            ["2"]
        );
        let filtered = filter_playlists(&playlists, "mix");
        assert_eq!(
            filtered.iter().map(|p| p.id.as_str()).collect::<Vec<_>>(),
            ["3"]
        );
        assert!(filter_playlists(&playlists, "no match").is_empty());
    }

    #[test]
    fn picker_list_height_stays_bounded_to_sixty_percent_of_the_viewport() {
        assert_eq!(list_max_height(800.), 320.);
        assert!((list_max_height(400.) - 240.).abs() < 0.01);
        assert!((list_max_height(100.) - 60.).abs() < 0.01);
        assert_eq!(list_max_height(0.), 0.);
        assert_eq!(dialog_max_height(760.), 728.);
        assert_eq!(dialog_max_height(20.), 0.);
        assert_eq!(picker_dialog_width(1200.), 460.);
        assert_eq!(picker_dialog_width(400.), 368.);
        assert_eq!(picker_dialog_width(20.), 0.);
    }

    #[test]
    fn picker_dialog_keeps_one_fixed_size_while_filtering() {
        let few = estimated_dialog_height(800., 2, false);
        let many = estimated_dialog_height(800., 200, false);
        assert_eq!(few, many);
        assert_eq!(many, 200. + 320.);
        assert_eq!(
            estimated_dialog_height(800., 2, true),
            estimated_dialog_height(800., 2, false) + 28.
        );
        assert_eq!(estimated_dialog_height(300., 200, false), 268.);
        assert_eq!(fixed_dialog_height(800., false), 200. + 320.);
        assert_eq!(
            fixed_dialog_height(800., true),
            fixed_dialog_height(800., false) + 28.
        );
    }

    #[test]
    fn picker_filter_uses_the_requested_single_space_placeholder() {
        assert_eq!(
            PICKER_FILTER_PLACEHOLDER,
            "What playlist are you looking for?"
        );
        let source = include_str!("playlist_picker.rs");
        let production = &source[..source.find("#[cfg(test)]").unwrap()];
        assert!(production.contains("PICKER_FILTER_PLACEHOLDER"));
        assert!(production.contains("What playlist are you looking for?"));
        assert!(!production.contains("What playlist  are you looking for?"));
        assert!(!production.contains("Filter playlists..."));
    }

    #[test]
    fn picker_click_toggles_selection_and_disables_confirm_when_empty() {
        assert_eq!(toggle_selection(None, "7"), Some("7".to_owned()));
        assert_eq!(toggle_selection(Some("7".to_owned()), "7"), None);
        assert_eq!(
            toggle_selection(Some("7".to_owned()), "8"),
            Some("8".to_owned())
        );
        assert!(!can_confirm(false, false, false));
        assert!(can_confirm(true, false, false));
    }

    #[test]
    fn picker_row_meta_uses_singular_and_plural_track_labels() {
        assert_eq!(track_count_label(1), "1 track");
        assert_eq!(track_count_label(34), "34 tracks");
        assert_eq!(track_count_label(0), "0 tracks");
        assert_eq!(playlist_track_meta(Some(34)), Some("34 tracks".to_owned()));
        assert_eq!(playlist_track_meta(Some(1)), Some("1 track".to_owned()));
        assert_eq!(playlist_track_meta(None), None);
    }

    #[test]
    fn picker_privacy_uses_lock_and_earth_icons() {
        assert_eq!(playlist_privacy_icon(true), crate::assets::LocalIcon::Lock);
        assert_eq!(
            playlist_privacy_icon(false),
            crate::assets::LocalIcon::EarthAmericas
        );
        // Both glyphs are square with built-in padding (the lock is the
        // full-viewport variant), so one square box fits both with no clip.
        assert_eq!(super::PICKER_PRIVACY_ICON_SIZE, 12.);
        assert_eq!(super::PICKER_PRIVACY_ICON_GAP, 6.);
        assert_eq!(super::PICKER_PRIVACY_OPTICAL_OFFSET_PX, 2.);
        assert_eq!(super::PICKER_TITLE_SUBTITLE_GAP, 1.);
        let source = include_str!("playlist_picker.rs");
        let production = &source[..source.find("#[cfg(test)]").unwrap()];
        assert!(production.contains("playlist.is_private.map(playlist_privacy_icon)"));
        assert!(!production.contains("LocalIcon::Globe"));
        assert!(production.contains(".size(px(PICKER_PRIVACY_ICON_SIZE))"));
        assert!(production.contains("PICKER_PRIVACY_ICON_GAP"));
        assert!(production.contains("gap(px(PICKER_TITLE_SUBTITLE_GAP))"));
        assert!(production.contains("top(px(PICKER_PRIVACY_OPTICAL_OFFSET_PX))"));
        assert!(production.contains("rgb(MUTED)"));
        // Title line keeps the privacy icon inline next to a truncated name:
        // the name must shrink to fit (not stretch), so the icon hugs the text.
        assert!(production.contains("gap(px(PICKER_PRIVACY_ICON_GAP))"));
        assert!(production.contains(".truncate()"));
        assert!(production.contains(".overflow_hidden()"));
        assert!(production.contains(".min_w_0()"));
        assert!(production.contains(".flex_none()"));
        assert!(production.contains(".flex_shrink_1()"));
        assert!(production.contains(".max_w_full()"));
        assert!(production.contains("playlist-picker-privacy-"));
        assert!(production.contains(".app_tooltip("));
        assert!(production.contains("\"Private\""));
        assert!(production.contains("\"Public\""));
    }

    #[test]
    fn picker_row_keeps_icon_gap_and_check_alignment() {
        assert_eq!(super::PICKER_ROW_ICON_GAP, 14.);
        assert_eq!(super::PICKER_ROW_ICON_COLUMN, 16.);
        assert_eq!(super::PICKER_PRIVACY_ICON_GAP, 6.);
        let source = include_str!("playlist_picker.rs");
        let production = &source[..source.find("#[cfg(test)]").unwrap()];
        assert!(production.contains("gap(px(PICKER_ROW_ICON_GAP))"));
        assert!(production.contains("w(px(PICKER_ROW_ICON_COLUMN))"));
        assert!(production.contains("gap(px(PICKER_PRIVACY_ICON_GAP))"));
        // The selected check aligns exactly like the unselected list icon.
        assert!(!production.contains("PICKER_CHECK_OPTICAL_OFFSET_PX"));
        assert!(!production.contains("top(px(PICKER_CHECK_OPTICAL_OFFSET_PX))"));
    }

    #[test]
    fn picker_selected_hover_differs_from_selected_rest() {
        assert_eq!(super::PICKER_SELECTED_HOVER_BACKGROUND, 0x6366f13d);
        let source = include_str!("playlist_picker.rs");
        let production = &source[..source.find("#[cfg(test)]").unwrap()];
        // Hovering the selected row must read as a state change, not a no-op.
        assert!(production.contains("gpui::rgba(PICKER_SELECTED_HOVER_BACKGROUND)"));
    }

    #[test]
    fn picker_row_hit_areas_touch_without_changing_pitch() {
        // 44px visual + 1px padding + 1px shell border on each side.
        assert_eq!(super::PICKER_ROW_HIT_PADDING, 1.);
        assert_eq!(
            super::PICKER_ROW_HEIGHT + 2. * (super::PICKER_ROW_HIT_PADDING + 1.),
            super::row_stride()
        );
        let source = include_str!("playlist_picker.rs");
        let production = &source[..source.find("#[cfg(test)]").unwrap()];
        // Spacing moved from the list gap into the rows so the dead zone
        // between hit areas is gone.
        assert!(production.contains(".py(px(PICKER_ROW_HIT_PADDING))"));
        // Visuals follow the shell through its group, not element hover.
        assert!(production.contains(".group(row_group.clone())"));
        assert!(production.contains(".group_hover(row_group, |style|"));
    }

    #[test]
    fn picker_footer_left_aligns_create_playlist_and_keeps_shared_contracts() {
        let source = include_str!("playlist_picker.rs");
        let production = &source[..source.find("#[cfg(test)]").unwrap()];
        assert!(production.contains("\"Create Playlist\""));
        assert!(!production.contains("Create new playlist"));
        assert!(production.contains("justify_between"));
        assert!(production.contains("justify_end"));
        assert!(production.contains("secondary_dialog_button_with_disabled"));
        assert!(production.contains("primary_button_with_loading"));
        assert!(production.contains("create-playlist-from-picker"));
        assert!(production.contains("cancel-playlist-picker"));
        assert!(production.contains("confirm-playlist-picker"));
    }

    #[test]
    fn picker_removes_subtitle_and_playlist_counters() {
        let source = include_str!("playlist_picker.rs");
        let production = &source[..source.find("#[cfg(test)]").unwrap()];
        assert!(!production.contains("Choose one of your editable playlists"));
        assert!(!production.contains("33 playlists"));
        assert!(!production.contains("of {} playlists"));
        assert!(!production.contains("{} playlists"));
    }

    #[test]
    fn picker_has_close_button_with_tooltip_and_escape() {
        let source = include_str!("playlist_picker.rs");
        let production = &source[..source.find("#[cfg(test)]").unwrap()];
        assert!(production.contains("ghost_close_button_with_icon_size"));
        assert!(production.contains("\"playlist-picker-close\""));
        assert!(production.contains("\"escape\""));
        assert!(production.contains("can_dismiss"));
    }

    #[test]
    fn picker_rows_stay_focusable_with_stable_ids_and_visible_ring() {
        let source = include_str!("playlist_picker.rs");
        let production = &source[..source.find("#[cfg(test)]").unwrap()];
        assert!(production.contains("playlist-picker-row-"));
        assert!(production.contains("tab_stop(!disabled)"));
        assert!(!production.contains("tab_stop(false)"));
        assert!(production.contains("focus_visible"));
        assert!(production.contains("focusable()"));
    }

    #[test]
    fn picker_keyboard_moves_selection_without_wrapping() {
        assert_eq!(move_selection(None, 1, 0), None);
        assert_eq!(move_selection(None, 1, 3), Some(0));
        assert_eq!(move_selection(None, -1, 3), Some(2));
        assert_eq!(move_selection(Some(0), -1, 3), Some(0));
        assert_eq!(move_selection(Some(2), 1, 3), Some(2));
        assert_eq!(move_selection(Some(0), 1, 3), Some(1));
        let filtered = vec![playlist("1", "One"), playlist("2", "Two")];
        assert_eq!(selection_index_for_id(&filtered, Some("2")), Some(1));
        assert_eq!(selection_index_for_id(&filtered, Some("9")), None);
        assert_eq!(selection_index_for_id(&filtered, None), None);
    }

    #[test]
    fn picker_keyboard_keeps_selection_inside_the_visible_list() {
        assert_eq!(selection_scroll_offset(0., 0, 320., 10), 0.);
        assert_eq!(selection_scroll_offset(0., 7, 320., 10), -60.);
        assert_eq!(selection_scroll_offset(-100., 0, 320., 10), -0.);
        assert_eq!(selection_scroll_offset(0., 0, 0., 10), 0.);
    }

    #[test]
    fn picker_uses_shared_dialog_and_scroll_contracts() {
        let source = include_str!("playlist_picker.rs");
        let production = &source[..source.find("#[cfg(test)]").unwrap()];
        assert!(production.contains("BACKGROUND"));
        assert!(production.contains("secondary_dialog_button_with_disabled"));
        assert!(production.contains("primary_button_with_loading"));
        assert!(production.contains("text_field_context_menu"));
        assert!(production.contains("track_scroll(&self.scroll)"));
        assert!(production.contains("overflow_y_scroll()"));
        assert!(!production.contains("vertical_scrollbar(&self.scroll)"));
        assert!(production.contains("browser_scroll_surface("));
        assert!(production.contains("BrowserScrollTarget::Handle(self.scroll.clone())"));
        assert!(production.contains("h(px(list_max))"));
        assert!(production.contains("Scrollbar::vertical(&self.scroll)"));
        assert!(production.contains("ScrollbarShow::Hover"));
        assert_eq!(production.matches("Scrollbar::vertical").count(), 1);
        assert!(production.contains(".pr(px(10.))"));
        assert_eq!(super::PICKER_SCROLLBAR_EDGE_OFFSET_PX, 14.);
        assert!(production.contains("PICKER_SCROLLBAR_EDGE_OFFSET_PX"));
        assert!(production.contains("right(px(-PICKER_SCROLLBAR_EDGE_OFFSET_PX))"));
        assert!(production.contains(".absolute()"));
        assert!(production.contains("empty_state::render"));
        assert!(production.contains("\"escape\""));
        assert!(production.contains("\"up\""));
        assert!(production.contains("\"down\""));
        assert!(production.contains("\"enter\""));
    }

    #[test]
    fn picker_never_shows_the_red_removing_banner() {
        let source = include_str!("playlist_picker.rs");
        let production = &source[..source.find("#[cfg(test)]").unwrap()];
        assert!(!production.contains("Removing track from playlist"));
        let state_source = include_str!("playlist_state.rs");
        let state_production = &state_source[..state_source.find("#[cfg(test)]").unwrap()];
        assert!(!state_production.contains("Removing track from playlist"));
    }

    #[test]
    fn picker_dismisses_on_escape_and_overlay_click_only_when_idle() {
        assert!(can_dismiss(false));
        assert!(!can_dismiss(true));
    }

    #[test]
    fn picker_registers_overlay_dismiss_without_triggering_underlying_actions() {
        let source = include_str!("playlist_picker.rs");
        let production = &source[..source.find("#[cfg(test)]").unwrap()];
        assert!(production.contains("overlay_closable(true)"));
        assert!(production.contains("on_cancel"));
        assert!(production.contains("can_dismiss"));
        assert!(production.contains("request_dialog_close"));
    }

    #[test]
    fn soundcloud_picker_adds_through_owned_playlists_and_matching_create() {
        let source = include_str!("playlist_picker.rs");
        let production = &source[..source.find("#[cfg(test)]").unwrap()];
        assert!(production.contains("add_tracks_to_playlist"));
        assert!(production.contains("playlist_catalog(*provider)"));
        assert!(production.contains("open_soundcloud_playlist_create_with_tracks"));
        assert!(production.contains("Provider::SoundCloud"));
        assert!(production.contains("picker_title(self.mode())"));
        let submit = production
            .split("fn submit_soundcloud(")
            .nth(1)
            .and_then(|rest| rest.split("fn move_selection(").next())
            .expect("SoundCloud picker submission");
        assert!(submit.contains("soundcloud_mobile_token()"));
        assert!(!submit.contains("soundcloud_token()"));
    }

    #[test]
    fn local_target_uses_local_mode_and_shared_filter_selection() {
        let local_target = PlaylistPickerTarget::Local { tracks: vec![] };
        assert_eq!(local_target.mode(), PickerMode::Local);
        assert_eq!(PickerMode::Local, PickerMode::Local);

        let playlists = vec![
            PickerPlaylist {
                id: "local-1".into(),
                title: "Morning Local".into(),
                subtitle: Some("Local playlist".into()),
                track_count: Some(2),
                is_private: None,
                local: true,
            },
            PickerPlaylist {
                id: "local-2".into(),
                title: "Evening Local".into(),
                subtitle: Some("Description".into()),
                track_count: Some(4),
                is_private: None,
                local: true,
            },
        ];
        let filtered = filter_picker_playlists(&playlists, " EVEN ");
        assert_eq!(filtered.len(), 1);
        assert_eq!(filtered[0].id, "local-2");
        assert_eq!(
            selection_index_for_picker_id(&filtered, Some("local-2")),
            Some(0)
        );
        assert_eq!(
            selection_index_for_picker_id(&filtered, Some("missing")),
            None
        );
        assert!(filtered[0].local);
        assert_eq!(filtered[0].is_private, None);
    }

    #[test]
    fn local_target_never_reads_provider_catalog_or_account_scope() {
        let source = include_str!("playlist_picker.rs");
        let production = &source[..source.find("#[cfg(test)]").unwrap()];
        let local_branch = production
            .split("PlaylistPickerTarget::Local { .. } =>")
            .nth(1)
            .expect("local target branch");
        assert!(local_branch.contains("None"));
        assert!(production.contains("PlaylistPickerTarget::Local { .. } => false"));
        assert!(production.contains(".playlists()"));
    }

    #[test]
    fn picker_account_error_only_appears_after_a_provider_account_change() {
        assert_eq!(
            picker_account_error(PickerMode::Provider(Provider::Deezer), false),
            None
        );
        assert_eq!(
            picker_account_error(PickerMode::Provider(Provider::SoundCloud), true),
            Some("The SoundCloud account changed while this picker was open.")
        );
        assert_eq!(picker_account_error(PickerMode::Local, false), None);
        assert_eq!(picker_account_error(PickerMode::Local, true), None);
    }
}
