use crate::{
    library::{Category as LibraryCategory, Service as LibraryService},
    navigation_state::{
        AppSettings, LibraryService as StoredLibraryService, MainDestination, SearchSource,
    },
    search::Source,
};

use super::{Nav, RalgrumApp};

pub(super) fn nav_from_stored(destination: MainDestination) -> Nav {
    match destination {
        MainDestination::Discover => Nav::Discover,
        MainDestination::Library => Nav::Library,
        MainDestination::Downloads => Nav::Downloads,
        MainDestination::Cache => Nav::Cache,
    }
}

pub(super) fn search_active_for_nav(nav: Nav) -> bool {
    nav == Nav::Discover
}

pub(super) fn source_from_stored(source: SearchSource) -> Source {
    match source {
        SearchSource::All => Source::All,
        SearchSource::Deezer => Source::Deezer,
        SearchSource::Soundcloud => Source::SoundCloud,
    }
}

pub(super) fn library_selection(settings: &AppSettings) -> (LibraryService, LibraryCategory) {
    if !settings.remember_navigation {
        return (LibraryService::Deezer, LibraryCategory::Tracks);
    }
    let service = match settings.library_service {
        StoredLibraryService::Local => LibraryService::Local,
        StoredLibraryService::Deezer => LibraryService::Deezer,
        StoredLibraryService::Soundcloud => LibraryService::SoundCloud,
    };
    (service, library_category_for_service(settings, service))
}

pub(super) fn library_category_for_service(
    settings: &AppSettings,
    service: LibraryService,
) -> LibraryCategory {
    if !settings.remember_navigation {
        return service.default_category();
    }
    let saved = match service {
        LibraryService::Local => &settings.library_categories.local,
        LibraryService::Deezer => &settings.library_categories.deezer,
        LibraryService::SoundCloud => &settings.library_categories.soundcloud,
    };
    service
        .categories()
        .iter()
        .copied()
        .find(|category| category_id(*category) == saved)
        .unwrap_or_else(|| service.default_category())
}

fn category_id(category: LibraryCategory) -> &'static str {
    match category {
        LibraryCategory::MyTracks => "my-tracks",
        _ => category.action(),
    }
}

impl RalgrumApp {
    pub(super) fn persist_navigation(&mut self, cx: &mut gpui::Context<Self>) {
        if self.import_sync_in_progress {
            return;
        }
        let mut saved = self.settings.read(cx).saved().clone();
        if !saved.remember_navigation {
            return;
        }
        saved.last_main_tab = match self.nav {
            Nav::Discover => MainDestination::Discover,
            Nav::Library => MainDestination::Library,
            Nav::Downloads => MainDestination::Downloads,
            Nav::Cache => MainDestination::Cache,
        };
        saved.source_filter = match self.search.read(cx).source() {
            Source::All => SearchSource::All,
            Source::Deezer => SearchSource::Deezer,
            Source::SoundCloud => SearchSource::Soundcloud,
        };
        saved.search_type = self
            .search
            .read(cx)
            .result_type()
            .label()
            .to_ascii_lowercase()
            .into();
        let (service, category) = self.library.read(cx).selection();
        saved.library_service = match service {
            LibraryService::Local => StoredLibraryService::Local,
            LibraryService::Deezer => StoredLibraryService::Deezer,
            LibraryService::SoundCloud => StoredLibraryService::Soundcloud,
        };
        match service {
            LibraryService::Local => saved.library_categories.local = category_id(category).into(),
            LibraryService::Deezer => {
                saved.library_categories.deezer = category_id(category).into()
            }
            LibraryService::SoundCloud => {
                saved.library_categories.soundcloud = category_id(category).into()
            }
        }
        self.settings
            .update(cx, |settings, _| settings.persist_navigation(saved, false));
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn library_selection_uses_defaults_when_navigation_is_not_remembered() {
        let settings = AppSettings {
            remember_navigation: false,
            library_service: StoredLibraryService::Soundcloud,
            ..AppSettings::default()
        };

        assert_eq!(
            library_selection(&settings),
            (LibraryService::Deezer, LibraryCategory::Tracks)
        );
    }

    #[test]
    fn library_selection_restores_saved_selection_when_navigation_is_remembered() {
        let mut settings = AppSettings {
            library_service: StoredLibraryService::Soundcloud,
            ..AppSettings::default()
        };
        settings.library_categories.soundcloud = "history".into();

        assert_eq!(
            library_selection(&settings),
            (LibraryService::SoundCloud, LibraryCategory::History)
        );
    }

    #[test]
    fn library_selection_restores_local_without_using_provider_categories() {
        let mut settings = AppSettings {
            library_service: StoredLibraryService::Local,
            ..AppSettings::default()
        };
        settings.library_categories.deezer = "flow".into();
        settings.library_categories.soundcloud = "station".into();

        assert_eq!(
            library_selection(&settings),
            (LibraryService::Local, LibraryCategory::Tracks)
        );
    }

    #[test]
    fn library_selection_restores_local_playlists_without_touching_remote_categories() {
        let mut settings = AppSettings {
            library_service: StoredLibraryService::Local,
            ..AppSettings::default()
        };
        settings.library_categories.local = "playlists".into();
        settings.library_categories.deezer = "flow".into();
        settings.library_categories.soundcloud = "station".into();

        assert_eq!(
            library_selection(&settings),
            (LibraryService::Local, LibraryCategory::Playlists)
        );
        assert_eq!(settings.library_categories.deezer, "flow");
        assert_eq!(settings.library_categories.soundcloud, "station");
    }

    #[test]
    fn library_category_for_service_restores_each_independent_slot() {
        let mut settings = AppSettings::default();
        settings.library_categories.local = "playlists".into();
        settings.library_categories.deezer = "flow".into();
        settings.library_categories.soundcloud = "station".into();

        assert_eq!(
            library_category_for_service(&settings, LibraryService::Local),
            LibraryCategory::Playlists
        );
        assert_eq!(
            library_category_for_service(&settings, LibraryService::Deezer),
            LibraryCategory::Flow
        );
        assert_eq!(
            library_category_for_service(&settings, LibraryService::SoundCloud),
            LibraryCategory::Station
        );
    }

    #[test]
    fn library_category_for_service_falls_back_for_incompatible_or_unremembered_values() {
        let mut settings = AppSettings::default();
        settings.library_categories.local = "flow".into();
        settings.library_categories.deezer = "station".into();
        settings.library_categories.soundcloud = "not-a-category".into();

        assert_eq!(
            library_category_for_service(&settings, LibraryService::Local),
            LibraryCategory::Tracks
        );
        assert_eq!(
            library_category_for_service(&settings, LibraryService::Deezer),
            LibraryCategory::Tracks
        );
        assert_eq!(
            library_category_for_service(&settings, LibraryService::SoundCloud),
            LibraryCategory::MyTracks
        );

        settings.remember_navigation = false;
        settings.library_categories.deezer = "flow".into();
        assert_eq!(
            library_category_for_service(&settings, LibraryService::Deezer),
            LibraryCategory::Tracks
        );
    }

    #[test]
    fn search_is_active_only_on_discover() {
        assert!(search_active_for_nav(Nav::Discover));
        assert!(!search_active_for_nav(Nav::Library));
        assert!(!search_active_for_nav(Nav::Downloads));
        assert!(!search_active_for_nav(Nav::Cache));
    }

    #[test]
    fn cache_navigation_round_trips_through_settings() {
        assert_eq!(nav_from_stored(MainDestination::Cache), Nav::Cache);

        let mut settings = AppSettings::default();
        settings.last_main_tab = MainDestination::Cache;
        assert_eq!(nav_from_stored(settings.last_main_tab), Nav::Cache);
    }
}
