use std::rc::Rc;

use gpui::{App, ClickEvent, MouseButton, Window};

use super::{ContextMenuExt, copied_toast, items, style_entity_menu};

type DiscoverActionHandler = Rc<dyn Fn(&ClickEvent, &mut Window, &mut App)>;

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub(crate) enum DiscoverMenuPrimary {
    PlayMix,
    PlayFlow,
    PlaySelection,
    Open,
}

impl DiscoverMenuPrimary {
    fn label(self) -> &'static str {
        match self {
            Self::PlayMix => "Play mix",
            Self::PlayFlow => "Play Flow",
            Self::PlaySelection => "Play selection",
            Self::Open => "Open",
        }
    }

    fn icon(self) -> crate::assets::LocalIcon {
        match self {
            Self::PlayMix | Self::PlayFlow | Self::PlaySelection => crate::assets::LocalIcon::Play,
            Self::Open => crate::assets::LocalIcon::FolderOpen,
        }
    }
}

#[derive(Clone)]
pub(crate) struct DiscoverMenuAction {
    pub(crate) primary: DiscoverMenuPrimary,
    pub(crate) on_activate: DiscoverActionHandler,
}

impl DiscoverMenuAction {
    pub(crate) fn new<F>(primary: DiscoverMenuPrimary, on_activate: F) -> Self
    where
        F: Fn(&ClickEvent, &mut Window, &mut App) + 'static,
    {
        Self {
            primary,
            on_activate: Rc::new(on_activate),
        }
    }
}

pub(crate) fn discover_card_menu_with_actions<E>(
    element: E,
    title: String,
    actions: Vec<DiscoverMenuAction>,
) -> impl gpui::IntoElement
where
    E: gpui::InteractiveElement + gpui::ParentElement + gpui::Styled + gpui::IntoElement + 'static,
{
    let actions = Rc::new(actions);
    element
        .context_menu(move |menu, _, _| {
            let copy_title = title.clone();
            let menu = actions
                .iter()
                .cloned()
                .fold(style_entity_menu(menu), |menu, action| {
                    let on_activate = action.on_activate.clone();
                    menu.item(items::action_item(
                        action.primary.label(),
                        Some(action.primary.icon()),
                        false,
                        move |event, window, app| on_activate(event, window, app),
                    ))
                });
            menu.separator().item(items::action_item(
                "Copy title",
                Some(crate::assets::LocalIcon::Copy),
                false,
                move |_, _, cx| {
                    cx.write_to_clipboard(gpui::ClipboardItem::new_string(copy_title.clone()));
                    copied_toast("Title", cx);
                },
            ))
        })
        .open_on(MouseButton::Right)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn primary_menu_labels_are_limited_to_the_explicit_discover_action() {
        assert_eq!(DiscoverMenuPrimary::PlayMix.label(), "Play mix");
        assert_eq!(DiscoverMenuPrimary::PlayFlow.label(), "Play Flow");
        assert_eq!(DiscoverMenuPrimary::PlaySelection.label(), "Play selection");
        assert_eq!(DiscoverMenuPrimary::Open.label(), "Open");
        let source = include_str!("discover_card.rs")
            .split("#[cfg(test)]")
            .next()
            .expect("production menu source");
        assert!(source.contains(".open_on(MouseButton::Right)"));
        assert!(source.contains("\"Copy title\""));
        assert!(!source.contains("Play next"));
        assert!(!source.contains("Download"));
        assert!(!source.contains("Favorite"));
    }

    #[test]
    fn multi_action_menu_keeps_copy_after_explicit_actions() {
        let source = include_str!("discover_card.rs");
        assert!(source.contains("discover_card_menu_with_actions"));
        assert!(source.contains("actions.iter().cloned().fold"));
        assert!(source.contains("menu.separator()"));
        assert!(source.contains("\"Copy title\""));
    }
}
