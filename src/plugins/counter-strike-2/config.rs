use std::{collections::BTreeMap, path::PathBuf};

use cs2_gsi::cfg::GsiCfg;

use super::{PORT, settings::Settings};

pub(super) fn install(settings: &Settings) -> Result<PathBuf, String> {
    let mut config =
        GsiCfg::for_localhost("ralgruM", PORT).with_auth("token", settings.auth_token().to_owned());
    config.timeout = 0.5;
    config.buffer = 0.1;
    config.throttle = 0.1;
    config.heartbeat = 5.0;
    config.data = required_sections();
    config
        .write_to_cs2()
        .map_err(|error| format!("Counter-Strike 2 integration could not be installed: {error}"))
}

fn required_sections() -> BTreeMap<String, String> {
    ["provider", "map", "round", "player_id", "player_state"]
        .into_iter()
        .map(|section| (section.to_owned(), "1".to_owned()))
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn integration_requests_only_playback_relevant_sections() {
        let sections = required_sections();
        assert_eq!(sections.len(), 5);
        assert!(!sections.contains_key("allplayers_state"));
        assert!(!sections.contains_key("player_weapons"));
    }
}
