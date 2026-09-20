# Built-in plugins

Plugins are Rust modules compiled into ralgruM, not separately installed code.
Each plugin lives in `src/plugins/<id>/`, where `<id>` is a unique lowercase
letter, digit, or hyphen name. Add its module and `PluginDefinition` to
`src/plugins/mod.rs`. The registry test checks IDs, required metadata, and
that every registered plugin has a matching folder.

The definition supplies the card's name, version, author, and description,
plus enable and disable callbacks. Callbacks run on the GPUI application
thread. They must not block that thread. Enabling a plugin should be safe to
call once on startup or after a user toggle, and disabling it should undo its
active subscriptions and other runtime effects. Only provide `open_settings`
when the plugin has settings. The plugin owns that dialog and its settings
storage; the shared `plugin_settings.json` file only tracks enabled states.

Users receive plugin code and updates through normal ralgruM releases. PRs
adding plugins are reviewed as application code. There is no dynamic loader,
repository installer, or independent plugin update channel.
