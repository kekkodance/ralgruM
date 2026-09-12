use gpui::{Context, Window};

use crate::{
    navigation_state::{AppSettings, RightSidebarView},
    playback::RightSidebar,
    search::ResultType,
};

use super::{
    RalgrumApp,
    navigation::{library_selection, nav_from_stored, search_active_for_nav, source_from_stored},
};

fn result_type_from_stored(value: &str) -> ResultType {
    match value {
        "tracks" => ResultType::Tracks,
        "albums" => ResultType::Albums,
        "artists" => ResultType::Artists,
        "playlists" => ResultType::Playlists,
        _ => ResultType::All,
    }
}

fn right_sidebar_from_stored(settings: &AppSettings) -> RightSidebar {
    match settings.right_sidebar_view {
        RightSidebarView::Lyrics => RightSidebar::Lyrics,
        RightSidebarView::Queue => RightSidebar::Queue,
    }
}

impl RalgrumApp {
    pub(super) fn apply_imported_settings(
        &mut self,
        imported: AppSettings,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        self.import_sync_in_progress = true;

        let right_sidebar = right_sidebar_from_stored(&imported);
        self.playback.update(cx, |playback, cx| {
            playback.apply_imported_preferences(
                imported.volume,
                imported.muted,
                imported.repeat_mode,
                imported.shuffle_enabled,
                imported.right_sidebar_open,
                right_sidebar,
                window,
                cx,
            );
        });

        let source = source_from_stored(imported.source_filter);
        let result_type = result_type_from_stored(&imported.search_type);
        self.search.update(cx, |search, cx| {
            search.apply_imported_preferences(
                source,
                result_type,
                imported.soundcloud_search_suggestions,
                imported.search_history.clone(),
                cx,
            );
        });

        if imported.remember_navigation {
            self.nav = nav_from_stored(imported.last_main_tab);
            self.external_detail_return = None;
            let nav = self.nav;
            self.search.update(cx, |search, _| {
                search.set_search_active(search_active_for_nav(nav));
            });
            let (service, category) = library_selection(&imported);
            self.library
                .update(cx, |library, cx| library.load(service, category, cx));
        }

        let right_sidebar = self.playback.read(cx).state.right_sidebar;
        self.sync_right_sidebar_transition(right_sidebar, cx);
        self.settings.update(cx, |settings, _| {
            settings.finish_import_sync();
        });
        self.import_sync_in_progress = false;
        cx.notify();
    }
}

#[cfg(test)]
mod tests {
    use super::{result_type_from_stored, right_sidebar_from_stored};
    use crate::{
        navigation_state::{AppSettings, RightSidebarView},
        playback::RightSidebar,
        search::ResultType,
    };

    #[test]
    fn imported_search_and_sidebar_values_map_to_runtime_domains() {
        assert_eq!(result_type_from_stored("tracks"), ResultType::Tracks);
        assert_eq!(result_type_from_stored("unexpected"), ResultType::All);

        let mut settings = AppSettings {
            right_sidebar_open: true,
            right_sidebar_view: RightSidebarView::Queue,
            ..AppSettings::default()
        };
        assert_eq!(right_sidebar_from_stored(&settings), RightSidebar::Queue);

        settings.right_sidebar_open = false;
        assert_eq!(right_sidebar_from_stored(&settings), RightSidebar::Queue);

        settings.right_sidebar_view = RightSidebarView::Lyrics;
        assert_eq!(right_sidebar_from_stored(&settings), RightSidebar::Lyrics);
    }
}
