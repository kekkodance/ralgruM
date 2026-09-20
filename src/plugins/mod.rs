//! Built-in integrations are compiled into ralgruM and reviewed with the app.
//! Add each integration under `src/plugins/<id>/` and register it in `ALL`.

mod store;

use gpui::{App, Window};

pub(crate) use store::{PluginStore, PluginStoreError};

pub(crate) struct PluginDefinition {
    pub id: &'static str,
    pub name: &'static str,
    pub version: &'static str,
    pub author: &'static str,
    pub description: &'static str,
    pub on_enable: fn(&mut App),
    pub on_disable: fn(&mut App),
    pub open_settings: Option<fn(&mut Window, &mut App)>,
}

// No integrations have been approved yet. Each new plugin is registered here.
const ALL: &[PluginDefinition] = &[];

pub(crate) fn all() -> &'static [PluginDefinition] {
    ALL
}

fn valid_id(id: &str) -> bool {
    if id.is_empty()
        || !id
            .bytes()
            .all(|byte| byte.is_ascii_lowercase() || byte.is_ascii_digit() || byte == b'-')
    {
        return false;
    }
    true
}

pub(crate) fn activate_enabled(store: &PluginStore, cx: &mut App) {
    for plugin in all() {
        debug_assert!(valid_id(plugin.id));
        if store.is_enabled(plugin.id) {
            (plugin.on_enable)(cx);
        }
    }
}

#[cfg(test)]
mod tests {
    use super::{all, valid_id};
    use std::collections::HashSet;

    #[test]
    fn registered_plugins_have_unique_safe_ids_and_metadata() {
        let mut ids = HashSet::new();
        for plugin in all() {
            assert!(ids.insert(plugin.id));
            assert!(valid_id(plugin.id));
            assert!(
                std::path::Path::new(env!("CARGO_MANIFEST_DIR"))
                    .join("src/plugins")
                    .join(plugin.id)
                    .is_dir()
            );
            assert!(!plugin.name.trim().is_empty());
            assert!(!plugin.version.trim().is_empty());
            assert!(!plugin.author.trim().is_empty());
            assert!(!plugin.description.trim().is_empty());
        }
    }

    #[test]
    fn plugin_ids_are_safe_folder_names() {
        assert!(valid_id("counter-strike-2"));
        for invalid in ["", "../other", "bad/name", "UPPER", "a?b", "a b"] {
            assert!(!valid_id(invalid));
        }
    }
}
