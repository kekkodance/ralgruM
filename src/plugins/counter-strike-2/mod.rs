mod config;
mod service;
mod settings;
mod state;
mod view;

use super::PluginDefinition;

pub(super) const DEFINITION: PluginDefinition = PluginDefinition {
    id: "counter-strike-2",
    name: "Counter-Strike 2 Integration",
    icon: crate::assets::LocalIcon::Crosshairs,
    description: "Control music playback and volume automatically during Counter-Strike 2 matches.",
    validate_enable,
    on_enable: service::enable,
    on_disable: service::disable,
    open_ui: None,
    open_settings: Some(view::open),
};

pub(super) const PORT: u16 = 31_982;

fn validate_enable() -> Result<(), String> {
    service::validate_port()?;
    let settings = settings::Settings::load_or_create()?;
    config::install(&settings)?;
    Ok(())
}
