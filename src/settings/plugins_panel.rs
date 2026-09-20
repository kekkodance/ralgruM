use gpui::{AnyElement, Context, FontWeight, IntoElement, div, prelude::*, px, rgb};
use gpui_component::Disableable as _;

use super::{
    SettingsView,
    action_button::secondary_action_button,
    service_panel::{panel_heading, settings_card},
    settings_switch,
};
use crate::{
    assets::LocalIcon,
    plugins::{self, PluginDefinition},
    theme::{FOREGROUND, MUTED},
};

impl SettingsView {
    pub(super) fn render_plugins(&mut self, cx: &mut Context<Self>) -> AnyElement {
        let plugins = plugins::all();
        div()
            .flex()
            .flex_col()
            .gap(px(14.))
            .child(panel_heading(
                "Plugins",
                "Integrations and Extensions built into ralgruM.",
            ))
            .when_some(
                self.plugin_error.clone().filter(|_| !plugins.is_empty()),
                |this, error| {
                    this.child(
                        settings_card()
                            .text_color(rgb(crate::theme::DANGER))
                            .child(error),
                    )
                },
            )
            .when(plugins.is_empty(), |this| {
                this.child(settings_card().child(crate::empty_state::render(
                    LocalIcon::Layers,
                    "No plugins yet",
                    "Built-in integrations will appear here when they are added to ralgruM.",
                    None,
                )))
            })
            .children(plugins.iter().map(|plugin| self.plugin_card(plugin, cx)))
            .into_any_element()
    }

    fn plugin_card(&self, plugin: &'static PluginDefinition, cx: &mut Context<Self>) -> AnyElement {
        let enabled = self
            .plugin_store
            .as_ref()
            .is_some_and(|store| store.is_enabled(plugin.id));
        settings_card()
            .child(
                div()
                    .flex()
                    .flex_wrap()
                    .items_center()
                    .justify_between()
                    .gap(px(12.))
                    .child(
                        div()
                            .flex()
                            .items_baseline()
                            .gap(px(9.))
                            .child(
                                div()
                                    .text_size(px(14.))
                                    .font_weight(FontWeight::SEMIBOLD)
                                    .text_color(rgb(FOREGROUND))
                                    .child(plugin.name),
                            )
                            .child(
                                div()
                                    .text_size(px(12.))
                                    .text_color(rgb(MUTED))
                                    .child(plugin.version),
                            ),
                    )
                    .child(
                        div()
                            .flex()
                            .flex_wrap()
                            .items_center()
                            .gap(px(8.))
                            .when_some(plugin.open_settings, |this, open_settings| {
                                this.child(secondary_action_button(
                                    format!("plugin-settings-{}", plugin.id),
                                    Some(LocalIcon::Settings),
                                    "Settings",
                                    "Open plugin settings",
                                    move |_, window, cx| open_settings(window, cx),
                                ))
                            })
                            .child(
                                settings_switch(
                                    format!("plugin-enabled-{}", plugin.id),
                                    enabled,
                                    cx.listener(move |this, checked: &bool, _, cx| {
                                        this.set_plugin_enabled(plugin, *checked, cx);
                                    }),
                                )
                                .label(if enabled { "Enabled" } else { "Disabled" })
                                .text_size(px(12.))
                                .text_color(rgb(MUTED))
                                .disabled(self.plugin_store.is_none() || self.plugin_write_pending),
                            ),
                    ),
            )
            .child(
                div()
                    .flex()
                    .flex_col()
                    .gap(px(3.))
                    .child(
                        div()
                            .text_size(px(12.))
                            .text_color(rgb(MUTED))
                            .child(format!("By {}", plugin.author)),
                    )
                    .child(
                        div()
                            .text_size(px(12.5))
                            .line_height(px(18.75))
                            .text_color(rgb(MUTED))
                            .child(plugin.description),
                    ),
            )
            .into_any_element()
    }

    fn set_plugin_enabled(
        &mut self,
        plugin: &'static PluginDefinition,
        enabled: bool,
        cx: &mut Context<Self>,
    ) {
        if self.plugin_write_pending {
            return;
        }
        let Some(store) = self.plugin_store.clone() else {
            self.plugin_error = Some("Plugin storage is unavailable.".into());
            cx.notify();
            return;
        };
        if store.is_enabled(plugin.id) == enabled {
            return;
        }
        self.plugin_write_pending = true;
        let task = self.runtime.spawn_blocking(move || {
            if enabled {
                (plugin.validate_enable)()?;
            }
            store
                .with_enabled(plugin.id, enabled)
                .map_err(|error| error.to_string())
        });
        cx.spawn(async move |this, cx| {
            let result = task.await;
            this.update(cx, |this, cx| {
                this.plugin_write_pending = false;
                match result {
                    Ok(Ok(store)) => {
                        this.plugin_store = Some(store);
                        this.plugin_error = None;
                        if enabled {
                            (plugin.on_enable)(cx);
                        } else {
                            (plugin.on_disable)(cx);
                        }
                    }
                    Ok(Err(error)) => {
                        crate::toast::push_global(
                            cx,
                            crate::toast::ToastKind::Error,
                            if enabled {
                                "Plugin could not be enabled"
                            } else {
                                "Plugin could not be disabled"
                            },
                            Some(error.clone().into()),
                        );
                        this.plugin_error = Some(error.into());
                    }
                    Err(error) => {
                        this.plugin_error =
                            Some(format!("Plugin settings task failed: {error}").into())
                    }
                }
                cx.notify();
            })
            .ok();
        })
        .detach();
    }
}
