use gpui::{
    AnyElement, App, ClickEvent, Context, CursorStyle, Div, FontWeight, IntoElement, Role,
    SharedString, Stateful, Window, div, prelude::*, px, rgb, rgba,
};
use gpui_component::Disableable as _;

use super::{
    SettingsView,
    service_panel::{panel_heading, settings_card},
    settings_switch,
};

use crate::{
    app_tooltip::AppTooltipExt,
    assets::{LocalIcon, local_icon},
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
        let write_pending = self.plugin_writes.is_pending(plugin.id);
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
                            .items_center()
                            .gap(px(9.))
                            .child(local_icon(plugin.icon, FOREGROUND).size(px(16.)))
                            .child(
                                div()
                                    .text_size(px(14.))
                                    .font_weight(FontWeight::SEMIBOLD)
                                    .text_color(rgb(FOREGROUND))
                                    .child(plugin.name),
                            ),
                    )
                    .child(
                        div()
                            .flex()
                            .flex_wrap()
                            .items_center()
                            .gap(px(8.))
                            .when_some(plugin.open_ui, |this, open_ui| {
                                this.child(plugin_ui_button(
                                    format!("plugin-ui-{}", plugin.id),
                                    !enabled || write_pending,
                                    move |_, window, cx| open_ui(window, cx),
                                ))
                            })
                            .when_some(plugin.open_settings, |this, open_settings| {
                                this.child(plugin_icon_action_button(
                                    format!("plugin-settings-{}", plugin.id),
                                    LocalIcon::Settings,
                                    "Open plugin settings",
                                    "Enable the plugin to change its settings",
                                    "Open plugin settings",
                                    !enabled || write_pending,
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
                                .disabled(self.plugin_store.is_none() || write_pending),
                            ),
                    ),
            )
            .child(
                div()
                    .text_size(px(12.5))
                    .line_height(px(18.75))
                    .text_color(rgb(MUTED))
                    .child(plugin.description),
            )
            .into_any_element()
    }

    fn set_plugin_enabled(
        &mut self,
        plugin: &'static PluginDefinition,
        enabled: bool,
        cx: &mut Context<Self>,
    ) {
        let Some(store) = self.plugin_store.as_ref() else {
            self.plugin_error = Some("Plugin storage is unavailable.".into());
            cx.notify();
            return;
        };
        if store.is_enabled(plugin.id) == enabled {
            return;
        }
        if !self.plugin_writes.enqueue(plugin, enabled) {
            return;
        }
        self.start_next_plugin_write(cx);
        cx.notify();
    }

    fn start_next_plugin_write(&mut self, cx: &mut Context<Self>) {
        let Some(request) = self.plugin_writes.begin_next() else {
            return;
        };
        let Some(store) = self.plugin_store.clone() else {
            self.plugin_writes.finish(request.plugin.id);
            self.plugin_error = Some("Plugin storage is unavailable.".into());
            self.start_next_plugin_write(cx);
            cx.notify();
            return;
        };
        let plugin = request.plugin;
        let enabled = request.enabled;
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
                this.plugin_writes.finish(plugin.id);
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
                this.start_next_plugin_write(cx);
                cx.notify();
            })
            .ok();
        })
        .detach();
    }
}

fn plugin_icon_action_button(
    id: impl Into<SharedString>,
    icon: LocalIcon,
    aria_label: &'static str,
    disabled_tooltip: &'static str,
    enabled_tooltip: &'static str,
    disabled: bool,
    on_click: impl Fn(&ClickEvent, &mut Window, &mut App) + 'static,
) -> Stateful<Div> {
    let id = id.into();
    let hover_group = id.clone();
    let button = div()
        .id(id)
        .when(!disabled, |this| this.group(hover_group.clone()))
        .role(Role::Button)
        .aria_label(aria_label)
        .size(px(34.))
        .flex_none()
        .flex()
        .items_center()
        .justify_center()
        .rounded(px(6.))
        .border_1()
        .border_color(rgba(0x00000000))
        .bg(rgba(0x00000000))
        .child(
            local_icon(icon, MUTED)
                .size(px(13.))
                .group_hover(hover_group, |style| style.text_color(rgb(FOREGROUND))),
        )
        .app_tooltip(if disabled {
            disabled_tooltip
        } else {
            enabled_tooltip
        });
    if disabled {
        button.opacity(0.5).cursor(CursorStyle::OperationNotAllowed)
    } else {
        button
            .focusable()
            .tab_stop(true)
            .cursor_pointer()
            .hover(|style| style.bg(rgb(crate::theme::BORDER)))
            .focus_visible(|style| style.border_color(rgb(crate::theme::PRIMARY)))
            .on_click(on_click)
    }
}

fn plugin_ui_button(
    id: impl Into<SharedString>,
    disabled: bool,
    on_click: impl Fn(&ClickEvent, &mut Window, &mut App) + 'static,
) -> Stateful<Div> {
    plugin_icon_action_button(
        id,
        LocalIcon::ArrowUpRightFromSquare,
        "Open plugin UI",
        "Enable the plugin to open its UI",
        "Open plugin UI",
        disabled,
        on_click,
    )
}
