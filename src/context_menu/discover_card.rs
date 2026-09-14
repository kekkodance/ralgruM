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
