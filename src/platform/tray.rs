//! Native system-tray integration for the desktop shell.
//!
//! GPUI owns the application's message loop, so the tray icon is kept in a
//! GPUI global and its event channel is drained by a small foreground task.
//! The tray menu itself is a GPUI popup window. This keeps its appearance and
//! behavior aligned with the rest of ralgruM instead of opening a generic
//! native Windows menu.

use gpui::{App, Window};

/// The close callback may only turn a close request into a hide operation when
/// the tray was actually created. This keeps a missing shell/tray dependency
/// from making the application unreachable.
pub(crate) const fn should_hide_on_close(close_to_tray: bool, tray_available: bool) -> bool {
    close_to_tray && tray_available
}

#[cfg(windows)]
mod windows_impl {
    use std::ffi::c_void;

    use futures::{
        StreamExt,
        channel::mpsc::{UnboundedReceiver, unbounded},
    };

    use gpui::{
        AnyWindowHandle, App, Bounds, Context, DisplayId, FocusHandle, Focusable, Global,
        IntoElement, KeyDownEvent, Pixels, Render, Subscription, Window,
        WindowBackgroundAppearance, WindowBounds, WindowKind, WindowOptions, div, point,
        prelude::*, px, rgb, rgba, size,
    };
    use tray_icon::{
        Icon, MouseButton, MouseButtonState, TrayIcon, TrayIconBuilder, TrayIconEvent, TrayIconId,
    };
    use windows::Win32::{
        Foundation::{HWND, POINT},
        Graphics::Gdi::{
            GetMonitorInfoW, HMONITOR, MONITOR_DEFAULTTONEAREST, MONITORINFO, MonitorFromPoint,
        },
        UI::{
            HiDpi::{GetDpiForMonitor, MDT_EFFECTIVE_DPI},
            WindowsAndMessaging::{
                IsIconic, SHOW_WINDOW_CMD, SW_HIDE, SW_RESTORE, SW_SHOW, SetForegroundWindow,
                ShowWindow, USER_DEFAULT_SCREEN_DPI,
            },
        },
    };

    use crate::{
        assets::LocalIcon,
        context_menu::{
            CONTEXT_MENU_BORDER, CONTEXT_MENU_FOREGROUND, CONTEXT_MENU_HOVER,
            CONTEXT_MENU_HOVER_FOREGROUND, CONTEXT_MENU_SURFACE, MENU_ARROW_CANVAS_PX,
            MENU_ARROW_OVERHANG_PX, PopupMenuArrowEdge, menu_arrow,
        },
        media_control,
        shell::RalgrumApp,
    };

    #[cfg(test)]
    use super::should_hide_on_close;

    const MENU_WIDTH: f32 = 208.;
    const MENU_HEIGHT: f32 = 120.;
    const MENU_RADIUS: f32 = 8.;
    const MENU_PADDING: f32 = 6.;
    const MENU_ROW_HEIGHT: f32 = 34.;
    const MENU_GAP: f32 = 6.;

    /// Both native tray menu triggers are disabled because the menu is drawn
    /// by `TrayMenu` below.
    const NATIVE_MENU_ON_LEFT_CLICK: bool = false;
    const NATIVE_MENU_ON_RIGHT_CLICK: bool = false;

    const TRAY_MENU_LABELS: [&str; 3] = ["Open ralgruM", "Settings", "Quit"];

    const APP_ICON: &[u8] = include_bytes!("../../assets/app-icon.png");

    struct TrayController {
        /// The icon removes itself from the notification area when dropped.
        _tray_icon: TrayIcon,
        tray_id: TrayIconId,
        main_window: AnyWindowHandle,
        shell: gpui::WeakEntity<RalgrumApp>,
        menu_window: Option<AnyWindowHandle>,
        quit_requested: bool,
    }

    impl Global for TrayController {}

    #[derive(Clone, Copy, Debug, PartialEq)]
    enum TrayInteraction {
        Show,
        OpenMenu {
            x: f64,
            y: f64,
            tray_rect: tray_icon::Rect,
        },
    }

    #[derive(Clone, Copy, Debug, Eq, PartialEq)]
    enum TaskbarEdge {
        Top,
        Bottom,
        Left,
        Right,
    }

    impl TaskbarEdge {
        const fn arrow_edge(self) -> PopupMenuArrowEdge {
            match self {
                Self::Top => PopupMenuArrowEdge::Top,
                Self::Bottom => PopupMenuArrowEdge::Bottom,
                Self::Left => PopupMenuArrowEdge::Left,
                Self::Right => PopupMenuArrowEdge::Right,
            }
        }

        const fn is_horizontal(self) -> bool {
            matches!(self, Self::Top | Self::Bottom)
        }
    }

    #[derive(Clone, Copy, Debug, PartialEq)]
    struct PhysicalRect {
        left: f32,
        top: f32,
        right: f32,
        bottom: f32,
    }

    impl PhysicalRect {
        fn from_tray_rect(rect: &tray_icon::Rect) -> Option<Self> {
            let width = rect.size.width as f32;
            let height = rect.size.height as f32;
            (width > 0. && height > 0.).then_some(Self {
                left: rect.position.x as f32,
                top: rect.position.y as f32,
                right: rect.position.x as f32 + width,
                bottom: rect.position.y as f32 + height,
            })
        }

        const fn from_point(x: f32, y: f32) -> Self {
            Self {
                left: x,
                top: y,
                right: x,
                bottom: y,
            }
        }

        const fn center(self) -> (f32, f32) {
            ((self.left + self.right) / 2., (self.top + self.bottom) / 2.)
        }

        fn logical(self, scale_factor: f32) -> Self {
            Self {
                left: self.left / scale_factor,
                top: self.top / scale_factor,
                right: self.right / scale_factor,
                bottom: self.bottom / scale_factor,
            }
        }
    }

    #[derive(Clone, Copy, Debug, PartialEq)]
    struct TrayPopupLayout {
        outer: PhysicalRect,
        surface_offset: (f32, f32),
        arrow_offset: f32,
        edge: TaskbarEdge,
    }

    #[derive(Clone, Copy, Debug, Eq, PartialEq)]
    enum TrayMenuAction {
        Open,
        Settings,
        Quit,
    }

    const TRAY_MENU_ACTIONS: [TrayMenuAction; 3] = [
        TrayMenuAction::Open,
        TrayMenuAction::Settings,
        TrayMenuAction::Quit,
    ];

    #[derive(Clone, Copy, Debug, Eq, PartialEq)]
    enum TrayMenuCommand {
        Previous,
        Next,
        Home,
        End,
        Activate,
        Close,
    }

    fn tray_menu_command(key: &str) -> Option<TrayMenuCommand> {
        if key.eq_ignore_ascii_case("up") {
            Some(TrayMenuCommand::Previous)
        } else if key.eq_ignore_ascii_case("down") {
            Some(TrayMenuCommand::Next)
        } else if key.eq_ignore_ascii_case("home") {
            Some(TrayMenuCommand::Home)
        } else if key.eq_ignore_ascii_case("end") {
            Some(TrayMenuCommand::End)
        } else if key.eq_ignore_ascii_case("enter") || key.eq_ignore_ascii_case("space") {
            Some(TrayMenuCommand::Activate)
        } else if key.eq_ignore_ascii_case("escape") {
            Some(TrayMenuCommand::Close)
        } else {
            None
        }
    }

    fn previous_index(current: Option<usize>, len: usize) -> usize {
        debug_assert!(len > 0);
        match current {
            None | Some(0) => len - 1,
            Some(index) => index - 1,
        }
    }

    fn next_index(current: Option<usize>, len: usize) -> usize {
        debug_assert!(len > 0);
        match current {
            None => 0,
            Some(index) if index + 1 == len => 0,
            Some(index) => index + 1,
        }
    }

    pub(super) fn install(
        cx: &mut App,
        window: &Window,
        shell: gpui::WeakEntity<RalgrumApp>,
    ) -> bool {
        if media_control::window_handle(window).is_none() {
            crate::diagnostics::event("WARN", "system tray unavailable: main HWND missing");
            return false;
        }

        let Ok(icon) = application_icon() else {
            crate::diagnostics::event("WARN", "system tray unavailable: app icon failed");
            return false;
        };

        // Do not attach a tray-icon native menu. The click flags must remain
        // disabled as well, otherwise Windows can open its generic menu
        // before the GPUI event pump receives the click.
        let builder = TrayIconBuilder::new()
            .with_menu_on_left_click(NATIVE_MENU_ON_LEFT_CLICK)
            .with_menu_on_right_click(NATIVE_MENU_ON_RIGHT_CLICK)
            .with_tooltip("ralgruM")
            .with_icon(icon);
        let tray_id = builder.id().clone();

        // TrayIconEvent's handler is a one-time process-global registration.
        // Register it immediately before building the icon, after decoding
        // all fallible assets and before the icon can emit an event.
        let (event_sender, event_receiver) = unbounded::<TrayIconEvent>();
        TrayIconEvent::set_event_handler(Some(move |event| {
            forward_tray_event(&event_sender, event);
        }));

        let Ok(tray_icon) = builder.build() else {
            crate::diagnostics::event("WARN", "system tray unavailable: icon creation failed");
            return false;
        };

        cx.set_global(TrayController {
            _tray_icon: tray_icon,
            tray_id,
            main_window: window.window_handle(),
            shell,
            menu_window: None,
            quit_requested: false,
        });
        start_event_pump(cx, event_receiver);
        crate::diagnostics::event("INFO", "system tray ready");
        true
    }

    pub(super) fn quit_requested(cx: &App) -> bool {
        cx.try_global::<TrayController>()
            .is_some_and(|controller| controller.quit_requested)
    }

    pub(super) fn hide_window(window: &Window) {
        if let Some(hwnd) = media_control::window_handle(window) {
            set_window_visibility(hwnd, false);
        }
    }

    fn application_icon() -> Result<Icon, String> {
        let image = image::load_from_memory(APP_ICON)
            .map_err(|error| format!("could not decode app icon: {error}"))?;
        let width = image.width();
        let height = image.height();
        Icon::from_rgba(image.into_rgba8().into_raw(), width, height)
            .map_err(|error| format!("could not prepare tray icon: {error}"))
    }

    fn start_event_pump(cx: &mut App, mut events: UnboundedReceiver<TrayIconEvent>) {
        cx.spawn(async move |cx| {
            while let Some(event) = events.next().await {
                let Some(interaction) = cx.update(|cx| {
                    let controller = cx.try_global::<TrayController>()?;
                    Some(map_tray_event(&event, &controller.tray_id))
                }) else {
                    break;
                };

                if let Some(interaction) = interaction {
                    match interaction {
                        TrayInteraction::Show => {
                            cx.update(|cx| restore_main_window(cx));
                        }
                        TrayInteraction::OpenMenu { x, y, tray_rect } => {
                            cx.update(|cx| open_tray_menu(cx, (x, y), tray_rect));
                        }
                    }
                }
            }
        })
        .detach();
    }

    fn forward_tray_event(
        sender: &futures::channel::mpsc::UnboundedSender<TrayIconEvent>,
        event: TrayIconEvent,
    ) {
        let _ = sender.unbounded_send(event);
    }

    fn map_tray_event(event: &TrayIconEvent, tray_id: &TrayIconId) -> Option<TrayInteraction> {
        let TrayIconEvent::Click {
            id,
            button,
            button_state,
            position,
            rect,
        } = event
        else {
            return None;
        };
        if id != tray_id {
            return None;
        }
        match (button, button_state) {
            (MouseButton::Left, MouseButtonState::Up) => Some(TrayInteraction::Show),
            (MouseButton::Right, MouseButtonState::Down) => Some(TrayInteraction::OpenMenu {
                x: position.x,
                y: position.y,
                tray_rect: *rect,
            }),
            _ => None,
        }
    }

    fn open_tray_menu(cx: &mut App, physical_position: (f64, f64), tray_rect: tray_icon::Rect) {
        if let Some(menu_window) = cx
            .try_global::<TrayController>()
            .and_then(|controller| controller.menu_window)
        {
            if menu_window
                .update(cx, |_, window, _| window.activate_window())
                .is_ok()
            {
                return;
            }
            cx.global_mut::<TrayController>().menu_window = None;
        }

        let popup = popup_bounds(physical_position, tray_rect, cx);
        let bounds = popup.bounds;
        let display_id = popup.display_id;
        let Ok(menu_window) = cx.open_window(
            WindowOptions {
                window_bounds: Some(WindowBounds::Windowed(bounds)),
                titlebar: None,
                focus: true,
                show: true,
                kind: WindowKind::PopUp,
                is_movable: false,
                is_resizable: false,
                is_minimizable: false,
                display_id,
                window_background: WindowBackgroundAppearance::Transparent,
                window_decorations: None,
                ..Default::default()
            },
            move |window, cx| {
                let initially_active = window.is_window_active();
                let menu = cx.new(|cx| {
                    TrayMenu::new(
                        cx,
                        initially_active,
                        popup.edge,
                        popup.surface_offset,
                        popup.arrow_offset,
                    )
                });
                let subscription = menu.update(cx, |menu, cx| {
                    menu.focus_handle.focus(window, cx);
                    cx.observe_window_activation(window, |menu, window, cx| {
                        let was_active = menu.activation_observed;
                        let is_active = window.is_window_active();
                        menu.activation_observed = is_active;
                        let should_close = should_close_on_activation(was_active, is_active);
                        if should_close {
                            menu.close(window, cx);
                        }
                    })
                });
                menu.update(cx, |menu, _| menu._subscriptions.push(subscription));
                menu
            },
        ) else {
            crate::diagnostics::event("WARN", "system tray menu unavailable: popup failed");
            return;
        };
        let menu_window: AnyWindowHandle = menu_window.into();
        cx.global_mut::<TrayController>().menu_window = Some(menu_window);
        // `WindowOptions::focus` and the entity focus handle do not reliably
        // foreground a tray-launched popup on Windows. Explicit activation
        // makes the initial WM_ACTIVATE(true) observable before an outside
        // click can deliver WM_ACTIVATE(false).
        let _ = menu_window.update(cx, |_, window, _| window.activate_window());
    }

    pub(super) fn restore_main_window(cx: &mut App) {
        let Some(main_window) = cx.try_global::<TrayController>().map(|c| c.main_window) else {
            return;
        };
        let _ = main_window.update(cx, |_, window, _| {
            if let Some(hwnd) = media_control::window_handle(window) {
                set_window_visibility(hwnd, true);
            }
            // SW_SHOW leaves maximized windows maximized, while the native
            // helper chooses SW_RESTORE only for minimized windows.
            window.activate_window();
        });
    }

    fn open_general_settings(cx: &mut App) {
        let Some((main_window, shell)) = cx
            .try_global::<TrayController>()
            .map(|c| (c.main_window, c.shell.clone()))
        else {
            return;
        };
        let _ = main_window.update(cx, |_, window, cx| {
            if let Some(hwnd) = media_control::window_handle(window) {
                set_window_visibility(hwnd, true);
            }
            shell
                .update(cx, |app, cx| app.open_general_settings(window, cx))
                .ok();
            window.activate_window();
        });
    }

    fn request_quit(cx: &mut App) {
        if cx.try_global::<TrayController>().is_some() {
            cx.global_mut::<TrayController>().quit_requested = true;
        }
        cx.quit();
    }

    fn close_menu(window: &mut Window, cx: &mut App) {
        let handle = window.window_handle();
        let owns_window = cx
            .try_global::<TrayController>()
            .is_some_and(|controller| controller.menu_window == Some(handle));
        if owns_window {
            cx.global_mut::<TrayController>().menu_window = None;
        }
        window.remove_window();
    }

    struct TrayMenu {
        focus_handle: FocusHandle,
        _subscriptions: Vec<Subscription>,
        activation_observed: bool,
        selected_index: Option<usize>,
        edge: TaskbarEdge,
        surface_offset: (f32, f32),
        arrow_offset: f32,
    }

    impl TrayMenu {
        fn new(
            cx: &mut Context<Self>,
            initially_active: bool,
            edge: TaskbarEdge,
            surface_offset: (f32, f32),
            arrow_offset: f32,
        ) -> Self {
            Self {
                focus_handle: cx.focus_handle(),
                _subscriptions: Vec::new(),
                activation_observed: initially_active,
                selected_index: None,
                edge,
                surface_offset,
                arrow_offset,
            }
        }

        fn close(&mut self, window: &mut Window, cx: &mut Context<Self>) {
            close_menu(window, cx);
        }

        fn activate(
            &mut self,
            action: TrayMenuAction,
            window: &mut Window,
            cx: &mut Context<Self>,
        ) {
            close_menu(window, cx);
            match action {
                TrayMenuAction::Open => restore_main_window(cx),
                TrayMenuAction::Settings => open_general_settings(cx),
                TrayMenuAction::Quit => request_quit(cx),
            }
        }

        fn set_selected_index(&mut self, index: Option<usize>, cx: &mut Context<Self>) {
            if self.selected_index != index {
                self.selected_index = index;
                cx.notify();
            }
        }

        fn row(
            &self,
            index: usize,
            id: &'static str,
            label: &'static str,
            icon: LocalIcon,
            action: TrayMenuAction,
            cx: &mut Context<Self>,
        ) -> impl IntoElement + use<> {
            div()
                .id(id)
                .w_full()
                .h(px(MENU_ROW_HEIGHT))
                .flex()
                .items_center()
                .gap(px(9.))
                .px(px(8.))
                .rounded(px(6.))
                .role(gpui::Role::MenuItem)
                .aria_label(label)
                .text_color(rgb(CONTEXT_MENU_FOREGROUND))
                .text_size(px(12.5))
                .font_weight(gpui::FontWeight(450.))
                .cursor_pointer()
                .when(self.selected_index == Some(index), |style| {
                    style
                        .bg(rgb(CONTEXT_MENU_HOVER))
                        .text_color(rgb(CONTEXT_MENU_HOVER_FOREGROUND))
                })
                .on_mouse_move(cx.listener(move |menu, _, _, cx| {
                    menu.set_selected_index(Some(index), cx);
                }))
                .on_click(cx.listener(move |menu, _, window, cx| {
                    menu.activate(action, window, cx);
                }))
                .child(
                    div()
                        .w(px(20.))
                        .flex()
                        .items_center()
                        .justify_center()
                        // Inherit the row color so tray icons follow the same
                        // base and hover/focus states as GPUI context menus.
                        .child(
                            gpui_component::Icon::default()
                                .path(icon.path())
                                .size(px(14.)),
                        ),
                )
                .child(label)
        }
    }

    fn should_close_on_activation(was_active: bool, is_active: bool) -> bool {
        was_active && !is_active
    }

    impl Focusable for TrayMenu {
        fn focus_handle(&self, _: &App) -> FocusHandle {
            self.focus_handle.clone()
        }
    }

    impl Render for TrayMenu {
        fn render(&mut self, _window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
            let outer_size = tray_outer_size(self.edge);
            let surface = div()
                .id("tray-menu-surface")
                .absolute()
                .left(px(self.surface_offset.0))
                .top(px(self.surface_offset.1))
                .w(px(MENU_WIDTH))
                .h(px(MENU_HEIGHT))
                .focusable()
                .track_focus(&self.focus_handle)
                .key_context("TrayMenu")
                .role(gpui::Role::Menu)
                .p(px(MENU_PADDING))
                .border_1()
                .border_color(rgba(CONTEXT_MENU_BORDER))
                .rounded(px(MENU_RADIUS))
                .bg(rgba(CONTEXT_MENU_SURFACE))
                .text_color(rgb(CONTEXT_MENU_FOREGROUND))
                .font_family(crate::theme::ui_font_family())
                .on_key_down(cx.listener(|menu, event: &KeyDownEvent, window, cx| {
                    let Some(command) = tray_menu_command(event.keystroke.key.as_str()) else {
                        return;
                    };
                    window.prevent_default();
                    cx.stop_propagation();
                    match command {
                        TrayMenuCommand::Previous => {
                            let index =
                                previous_index(menu.selected_index, TRAY_MENU_ACTIONS.len());
                            menu.set_selected_index(Some(index), cx);
                        }
                        TrayMenuCommand::Next => {
                            let index = next_index(menu.selected_index, TRAY_MENU_ACTIONS.len());
                            menu.set_selected_index(Some(index), cx);
                        }
                        TrayMenuCommand::Home => menu.set_selected_index(Some(0), cx),
                        TrayMenuCommand::End => {
                            menu.set_selected_index(Some(TRAY_MENU_ACTIONS.len() - 1), cx)
                        }
                        TrayMenuCommand::Activate => {
                            if let Some(index) = menu.selected_index {
                                if let Some(action) = TRAY_MENU_ACTIONS.get(index).copied() {
                                    menu.activate(action, window, cx);
                                }
                            }
                        }
                        TrayMenuCommand::Close => menu.close(window, cx),
                    }
                }))
                .children([
                    self.row(
                        0,
                        "tray-open",
                        TRAY_MENU_LABELS[0],
                        LocalIcon::WindowRestore,
                        TRAY_MENU_ACTIONS[0],
                        cx,
                    )
                    .into_any_element(),
                    self.row(
                        1,
                        "tray-settings",
                        TRAY_MENU_LABELS[1],
                        LocalIcon::Settings,
                        TRAY_MENU_ACTIONS[1],
                        cx,
                    )
                    .into_any_element(),
                    self.row(
                        2,
                        "tray-quit",
                        TRAY_MENU_LABELS[2],
                        LocalIcon::X,
                        TRAY_MENU_ACTIONS[2],
                        cx,
                    )
                    .into_any_element(),
                ]);

            let arrow = div()
                .child(menu_arrow(self.edge.arrow_edge()))
                .absolute()
                .when(self.edge == TaskbarEdge::Top, |this| {
                    this.left(px(
                        self.surface_offset.0 + self.arrow_offset - MENU_ARROW_CANVAS_PX / 2.
                    ))
                    .top(px(0.))
                })
                .when(self.edge == TaskbarEdge::Bottom, |this| {
                    this.left(px(
                        self.surface_offset.0 + self.arrow_offset - MENU_ARROW_CANVAS_PX / 2.
                    ))
                    .top(px(MENU_HEIGHT - MENU_ARROW_OVERHANG_PX))
                })
                .when(self.edge == TaskbarEdge::Left, |this| {
                    this.left(px(0.)).top(px(
                        self.surface_offset.1 + self.arrow_offset - MENU_ARROW_CANVAS_PX / 2.
                    ))
                })
                .when(self.edge == TaskbarEdge::Right, |this| {
                    this.left(px(MENU_WIDTH - MENU_ARROW_OVERHANG_PX))
                        .top(px(
                            self.surface_offset.1 + self.arrow_offset - MENU_ARROW_CANVAS_PX / 2.
                        ))
                });
            div()
                .id("tray-menu")
                .relative()
                .w(px(outer_size.0))
                .h(px(outer_size.1))
                .child(surface)
                .child(arrow)
        }
    }

    #[derive(Clone, Copy)]
    struct TrayPopupBounds {
        bounds: Bounds<Pixels>,
        display_id: Option<DisplayId>,
        edge: TaskbarEdge,
        surface_offset: (f32, f32),
        arrow_offset: f32,
    }

    fn popup_bounds(
        physical_position: (f64, f64),
        tray_rect: tray_icon::Rect,
        cx: &App,
    ) -> TrayPopupBounds {
        let (x, y) = physical_position;
        let icon_rect = PhysicalRect::from_tray_rect(&tray_rect)
            .unwrap_or_else(|| PhysicalRect::from_point(x as f32, y as f32));
        let monitor_anchor = icon_rect.center();
        if let Some(screen) = screen_geometry(monitor_anchor) {
            let layout = tray_popup_layout(
                icon_rect,
                screen.monitor_area,
                screen.work_area,
                screen.scale_factor,
            );
            return TrayPopupBounds {
                bounds: bounds_from_rect(layout.outer),
                display_id: Some(screen.display_id),
                edge: layout.edge,
                surface_offset: layout.surface_offset,
                arrow_offset: layout.arrow_offset,
            };
        }

        // This fallback is primarily for unusual Windows sessions where the
        // monitor API fails. GPUI's visible display bounds are already in its
        // logical coordinate space, so they remain safe to clamp against.
        let display = cx
            .primary_display()
            .or_else(|| cx.displays().into_iter().next());
        let Some(display) = display else {
            let edge = TaskbarEdge::Bottom;
            let (width, height) = tray_outer_size(edge);
            return TrayPopupBounds {
                bounds: Bounds::new(
                    point(px(x as f32), px(y as f32 - height)),
                    size(px(width), px(height)),
                ),
                display_id: None,
                edge,
                surface_offset: surface_offset(edge),
                arrow_offset: if edge.is_horizontal() {
                    MENU_WIDTH / 2.
                } else {
                    MENU_HEIGHT / 2.
                },
            };
        };
        let visible_bounds = rect_from_bounds(display.visible_bounds());
        let icon_rect = PhysicalRect::from_point(x as f32, y as f32);
        let layout = tray_popup_layout(icon_rect, visible_bounds, visible_bounds, 1.);
        TrayPopupBounds {
            bounds: bounds_from_rect(layout.outer),
            display_id: Some(display.id()),
            edge: layout.edge,
            surface_offset: layout.surface_offset,
            arrow_offset: layout.arrow_offset,
        }
    }

    #[derive(Clone, Copy)]
    struct ScreenGeometry {
        display_id: DisplayId,
        scale_factor: f32,
        monitor_area: PhysicalRect,
        work_area: PhysicalRect,
    }

    fn screen_geometry((x, y): (f32, f32)) -> Option<ScreenGeometry> {
        let physical_point = POINT {
            x: x.round() as i32,
            y: y.round() as i32,
        };
        let monitor = unsafe { MonitorFromPoint(physical_point, MONITOR_DEFAULTTONEAREST) };
        if monitor.is_invalid() {
            return None;
        }
        let mut info = MONITORINFO {
            cbSize: std::mem::size_of::<MONITORINFO>() as u32,
            ..Default::default()
        };
        if !unsafe { GetMonitorInfoW(monitor, &mut info) }.as_bool() {
            return None;
        }
        let scale_factor = monitor_scale_factor(monitor);
        Some(ScreenGeometry {
            display_id: DisplayId::new(monitor.0 as u64),
            scale_factor,
            monitor_area: PhysicalRect {
                left: info.rcMonitor.left as f32,
                top: info.rcMonitor.top as f32,
                right: info.rcMonitor.right as f32,
                bottom: info.rcMonitor.bottom as f32,
            },
            work_area: PhysicalRect {
                left: info.rcWork.left as f32,
                top: info.rcWork.top as f32,
                right: info.rcWork.right as f32,
                bottom: info.rcWork.bottom as f32,
            },
        })
    }

    fn monitor_scale_factor(monitor: HMONITOR) -> f32 {
        let mut dpi_x = 0;
        let mut dpi_y = 0;
        let result =
            unsafe { GetDpiForMonitor(monitor, MDT_EFFECTIVE_DPI, &mut dpi_x, &mut dpi_y) };
        if result.is_ok() && dpi_x > 0 && dpi_x == dpi_y {
            dpi_x as f32 / USER_DEFAULT_SCREEN_DPI as f32
        } else {
            1.
        }
    }

    fn taskbar_edge(
        monitor_area: PhysicalRect,
        work_area: PhysicalRect,
        icon_center: (f32, f32),
    ) -> TaskbarEdge {
        let excluded = [
            (TaskbarEdge::Top, (work_area.top - monitor_area.top).max(0.)),
            (
                TaskbarEdge::Bottom,
                (monitor_area.bottom - work_area.bottom).max(0.),
            ),
            (
                TaskbarEdge::Left,
                (work_area.left - monitor_area.left).max(0.),
            ),
            (
                TaskbarEdge::Right,
                (monitor_area.right - work_area.right).max(0.),
            ),
        ];
        let largest = excluded
            .iter()
            .map(|(_, amount)| *amount)
            .fold(0., f32::max);
        let largest_count = excluded
            .iter()
            .filter(|(_, amount)| (*amount - largest).abs() < 0.5)
            .count();
        if largest > 0. && largest_count == 1 {
            excluded
                .iter()
                .find_map(|(edge, amount)| (*amount == largest).then_some(*edge))
                .unwrap_or_else(|| nearest_monitor_edge(monitor_area, icon_center))
        } else {
            nearest_monitor_edge(monitor_area, icon_center)
        }
    }

    fn nearest_monitor_edge(monitor_area: PhysicalRect, (x, y): (f32, f32)) -> TaskbarEdge {
        let distances = [
            (TaskbarEdge::Top, (y - monitor_area.top).abs()),
            (TaskbarEdge::Bottom, (monitor_area.bottom - y).abs()),
            (TaskbarEdge::Left, (x - monitor_area.left).abs()),
            (TaskbarEdge::Right, (monitor_area.right - x).abs()),
        ];
        distances
            .into_iter()
            .min_by(|(_, left), (_, right)| {
                left.partial_cmp(right).unwrap_or(std::cmp::Ordering::Equal)
            })
            .map(|(edge, _)| edge)
            .unwrap_or(TaskbarEdge::Bottom)
    }

    fn tray_popup_layout(
        icon_rect: PhysicalRect,
        monitor_area: PhysicalRect,
        work_area: PhysicalRect,
        scale_factor: f32,
    ) -> TrayPopupLayout {
        let scale_factor = scale_factor.max(0.01);
        let icon_rect = icon_rect.logical(scale_factor);
        let monitor_area = monitor_area.logical(scale_factor);
        let work_area = work_area.logical(scale_factor);
        let edge = taskbar_edge(monitor_area, work_area, icon_rect.center());
        let (surface_width, surface_height) = (MENU_WIDTH, MENU_HEIGHT);
        let clamped_x = |x: f32| {
            x.clamp(
                work_area.left,
                (work_area.right - surface_width).max(work_area.left),
            )
        };
        let clamped_y = |y: f32| {
            y.clamp(
                work_area.top,
                (work_area.bottom - surface_height).max(work_area.top),
            )
        };
        let (surface_x, surface_y) = match edge {
            TaskbarEdge::Top => (
                clamped_x(icon_rect.center().0 - surface_width / 2.),
                clamped_y(icon_rect.bottom + MENU_GAP),
            ),
            TaskbarEdge::Bottom => (
                clamped_x(icon_rect.center().0 - surface_width / 2.),
                clamped_y(icon_rect.top - MENU_GAP - surface_height),
            ),
            TaskbarEdge::Left => (
                clamped_x(icon_rect.right + MENU_GAP),
                clamped_y(icon_rect.center().1 - surface_height / 2.),
            ),
            TaskbarEdge::Right => (
                clamped_x(icon_rect.left - MENU_GAP - surface_width),
                clamped_y(icon_rect.center().1 - surface_height / 2.),
            ),
        };
        let surface_offset = surface_offset(edge);
        let (outer_width, outer_height) = tray_outer_size(edge);
        let outer = PhysicalRect {
            left: surface_x - surface_offset.0,
            top: surface_y - surface_offset.1,
            right: surface_x - surface_offset.0 + outer_width,
            bottom: surface_y - surface_offset.1 + outer_height,
        };
        let cross_axis = if edge.is_horizontal() {
            icon_rect.center().0 - surface_x
        } else {
            icon_rect.center().1 - surface_y
        };
        let cross_extent = if edge.is_horizontal() {
            surface_width
        } else {
            surface_height
        };
        let arrow_offset = cross_axis.clamp(
            crate::context_menu::POPUP_ARROW_EDGE_INSET_PX,
            (cross_extent - crate::context_menu::POPUP_ARROW_EDGE_INSET_PX)
                .max(crate::context_menu::POPUP_ARROW_EDGE_INSET_PX),
        );
        TrayPopupLayout {
            outer,
            surface_offset,
            arrow_offset,
            edge,
        }
    }

    fn surface_offset(edge: TaskbarEdge) -> (f32, f32) {
        match edge {
            TaskbarEdge::Top => (0., MENU_ARROW_OVERHANG_PX),
            TaskbarEdge::Bottom => (0., 0.),
            TaskbarEdge::Left => (MENU_ARROW_OVERHANG_PX, 0.),
            TaskbarEdge::Right => (0., 0.),
        }
    }

    fn tray_outer_size(edge: TaskbarEdge) -> (f32, f32) {
        if edge.is_horizontal() {
            (MENU_WIDTH, MENU_HEIGHT + MENU_ARROW_CANVAS_PX)
        } else {
            (MENU_WIDTH + MENU_ARROW_CANVAS_PX, MENU_HEIGHT)
        }
    }

    fn bounds_from_rect(rect: PhysicalRect) -> Bounds<Pixels> {
        Bounds::new(
            point(px(rect.left), px(rect.top)),
            size(px(rect.right - rect.left), px(rect.bottom - rect.top)),
        )
    }

    fn rect_from_bounds(bounds: Bounds<Pixels>) -> PhysicalRect {
        PhysicalRect {
            left: bounds.origin.x.as_f32(),
            top: bounds.origin.y.as_f32(),
            right: bounds.origin.x.as_f32() + bounds.size.width.as_f32(),
            bottom: bounds.origin.y.as_f32() + bounds.size.height.as_f32(),
        }
    }

    fn set_window_visibility(hwnd: isize, visible: bool) {
        let hwnd = HWND(hwnd as *mut c_void);
        unsafe {
            if visible {
                let command = show_command(IsIconic(hwnd).as_bool());
                let _ = ShowWindow(hwnd, command);
                let _ = SetForegroundWindow(hwnd);
            } else {
                let _ = ShowWindow(hwnd, SW_HIDE);
            }
        }
    }

    const fn show_command(is_minimized: bool) -> SHOW_WINDOW_CMD {
        if is_minimized { SW_RESTORE } else { SW_SHOW }
    }

    #[cfg(test)]
    mod tests {
        use tray_icon::{MouseButton, MouseButtonState, Rect, TrayIconEvent, TrayIconId, dpi};

        use super::{
            CONTEXT_MENU_BORDER, CONTEXT_MENU_FOREGROUND, CONTEXT_MENU_HOVER,
            CONTEXT_MENU_HOVER_FOREGROUND, CONTEXT_MENU_SURFACE, MENU_GAP, MENU_HEIGHT, MENU_WIDTH,
            NATIVE_MENU_ON_LEFT_CLICK, NATIVE_MENU_ON_RIGHT_CLICK, PhysicalRect, SW_RESTORE,
            SW_SHOW, TRAY_MENU_ACTIONS, TRAY_MENU_LABELS, TaskbarEdge, TrayInteraction,
            TrayMenuAction, TrayMenuCommand, map_tray_event, nearest_monitor_edge, next_index,
            previous_index, should_close_on_activation, show_command, taskbar_edge,
            tray_menu_command, tray_popup_layout,
        };

        #[test]
        fn close_interception_requires_a_working_tray() {
            assert!(super::should_hide_on_close(true, true));
            assert!(!super::should_hide_on_close(true, false));
            assert!(!super::should_hide_on_close(false, true));
        }

        #[test]
        fn showing_only_restores_minimized_windows() {
            assert_eq!(show_command(true), SW_RESTORE);
            assert_eq!(show_command(false), SW_SHOW);
        }

        #[test]
        fn explicit_popup_activation_makes_the_first_outside_click_close() {
            assert!(!should_close_on_activation(false, false));
            // A popup opened from the tray may report inactive before the
            // explicit Window::activate_window call reaches Windows.
            assert!(!should_close_on_activation(false, true));
            assert!(!should_close_on_activation(true, true));
            // Once that activation is observed, the first outside click is a
            // real active-to-inactive transition and must dismiss the menu.
            assert!(should_close_on_activation(true, false));
        }

        #[test]
        fn tray_menu_labels_and_actions_are_exact() {
            assert_eq!(TRAY_MENU_LABELS, ["Open ralgruM", "Settings", "Quit"]);
            assert_eq!(
                TRAY_MENU_ACTIONS,
                [
                    TrayMenuAction::Open,
                    TrayMenuAction::Settings,
                    TrayMenuAction::Quit,
                ]
            );
        }

        #[test]
        fn tray_menu_keys_map_case_insensitively() {
            assert_eq!(tray_menu_command("UP"), Some(TrayMenuCommand::Previous));
            assert_eq!(tray_menu_command("down"), Some(TrayMenuCommand::Next));
            assert_eq!(tray_menu_command("HoMe"), Some(TrayMenuCommand::Home));
            assert_eq!(tray_menu_command("END"), Some(TrayMenuCommand::End));
            assert_eq!(tray_menu_command("ENTER"), Some(TrayMenuCommand::Activate));
            assert_eq!(tray_menu_command("Space"), Some(TrayMenuCommand::Activate));
            assert_eq!(tray_menu_command("eScApE"), Some(TrayMenuCommand::Close));
            assert_eq!(tray_menu_command("left"), None);
        }

        #[test]
        fn tray_menu_navigation_from_no_selection_starts_at_the_expected_edge() {
            assert_eq!(next_index(None, TRAY_MENU_ACTIONS.len()), 0);
            assert_eq!(
                previous_index(None, TRAY_MENU_ACTIONS.len()),
                TRAY_MENU_ACTIONS.len() - 1
            );
        }

        #[test]
        fn tray_menu_navigation_wraps() {
            assert_eq!(previous_index(Some(0), TRAY_MENU_ACTIONS.len()), 2);
            assert_eq!(previous_index(Some(2), TRAY_MENU_ACTIONS.len()), 1);
            assert_eq!(next_index(Some(2), TRAY_MENU_ACTIONS.len()), 0);
            assert_eq!(next_index(Some(0), TRAY_MENU_ACTIONS.len()), 1);
        }

        #[test]
        fn native_tray_menu_is_disabled_on_both_clicks() {
            assert!(!NATIVE_MENU_ON_LEFT_CLICK);
            assert!(!NATIVE_MENU_ON_RIGHT_CLICK);
        }

        #[test]
        fn tray_surface_does_not_add_external_shadow_to_the_arrow_margin() {
            let source = include_str!("tray.rs");
            let surface = source
                .split_once("let surface = div()")
                .and_then(|(_, rest)| rest.split_once("let arrow = div()"))
                .map(|(surface, _)| surface)
                .expect("tray surface render block");
            assert!(!surface.contains(".shadow_lg()"));
            assert!(surface.contains(".bg(rgba(CONTEXT_MENU_SURFACE))"));
        }

        #[test]
        fn tray_palette_reuses_the_shared_context_menu_palette() {
            assert_eq!(
                CONTEXT_MENU_SURFACE,
                crate::context_menu::CONTEXT_MENU_SURFACE
            );
            assert_eq!(
                CONTEXT_MENU_BORDER,
                crate::context_menu::CONTEXT_MENU_BORDER
            );
            assert_eq!(
                CONTEXT_MENU_FOREGROUND,
                crate::context_menu::CONTEXT_MENU_FOREGROUND
            );
            assert_eq!(CONTEXT_MENU_HOVER, crate::context_menu::CONTEXT_MENU_HOVER);
            assert_eq!(
                CONTEXT_MENU_HOVER_FOREGROUND,
                crate::context_menu::CONTEXT_MENU_HOVER_FOREGROUND
            );
        }

        #[test]
        fn tray_click_mapping_uses_left_release_and_right_press_events() {
            let id = TrayIconId::new("ralgrum-test");
            let tray_rect = Rect {
                position: dpi::PhysicalPosition::new(90., 190.),
                size: dpi::PhysicalSize::new(24, 24),
            };
            let event = |button, button_state| TrayIconEvent::Click {
                id: id.clone(),
                position: dpi::PhysicalPosition::new(100., 200.),
                rect: tray_rect,
                button,
                button_state,
            };

            assert_eq!(
                map_tray_event(&event(MouseButton::Left, MouseButtonState::Up), &id),
                Some(TrayInteraction::Show)
            );
            assert_eq!(
                map_tray_event(&event(MouseButton::Right, MouseButtonState::Down), &id),
                Some(TrayInteraction::OpenMenu {
                    x: 100.,
                    y: 200.,
                    tray_rect,
                })
            );
            assert_eq!(
                map_tray_event(&event(MouseButton::Left, MouseButtonState::Down), &id),
                None
            );
            assert_eq!(
                map_tray_event(&event(MouseButton::Right, MouseButtonState::Up), &id),
                None
            );
            assert_eq!(
                map_tray_event(&event(MouseButton::Middle, MouseButtonState::Down), &id),
                None
            );
            assert_eq!(
                map_tray_event(&event(MouseButton::Middle, MouseButtonState::Up), &id),
                None
            );
        }

        #[test]
        fn tray_layout_uses_the_inward_side_for_each_taskbar_edge() {
            let monitor = PhysicalRect {
                left: 0.,
                top: 0.,
                right: 1920.,
                bottom: 1080.,
            };
            let icon = PhysicalRect {
                left: 948.,
                top: 1056.,
                right: 972.,
                bottom: 1080.,
            };
            let bottom = tray_popup_layout(
                icon,
                monitor,
                PhysicalRect {
                    bottom: 1040.,
                    ..monitor
                },
                1.,
            );
            assert_eq!(bottom.edge, TaskbarEdge::Bottom);
            assert_eq!(bottom.surface_offset, (0., 0.));
            // The surface is clamped to the work-area bottom at 1040. The
            // arrow canvas extends six pixels beyond its surface, leaving a
            // six-pixel transparent tail in the outer popup window.
            assert_eq!(bottom.outer.bottom, 1052.);

            let top = tray_popup_layout(
                PhysicalRect {
                    left: 948.,
                    top: 0.,
                    right: 972.,
                    bottom: 24.,
                },
                monitor,
                PhysicalRect {
                    top: 40.,
                    ..monitor
                },
                1.,
            );
            assert_eq!(top.edge, TaskbarEdge::Top);
            assert_eq!(top.surface_offset, (0., 6.));
            // The surface is clamped to the work-area top at 40, leaving the
            // six-pixel arrow overlap above it.
            assert_eq!(top.outer.top, 34.);

            let left = tray_popup_layout(
                PhysicalRect {
                    left: 0.,
                    top: 528.,
                    right: 24.,
                    bottom: 552.,
                },
                monitor,
                PhysicalRect {
                    left: 40.,
                    ..monitor
                },
                1.,
            );
            assert_eq!(left.edge, TaskbarEdge::Left);
            assert_eq!(left.surface_offset, (6., 0.));
            // The surface is clamped to the work-area left at 40, leaving
            // the six-pixel arrow overlap on the taskbar side.
            assert_eq!(left.outer.left, 34.);

            let right = tray_popup_layout(
                PhysicalRect {
                    left: 1896.,
                    top: 528.,
                    right: 1920.,
                    bottom: 552.,
                },
                monitor,
                PhysicalRect {
                    right: 1880.,
                    ..monitor
                },
                1.,
            );
            assert_eq!(right.edge, TaskbarEdge::Right);
            assert_eq!(right.surface_offset, (0., 0.));
            assert_eq!(right.outer.right, 1892.);
        }

        #[test]
        fn tray_layout_uses_the_reduced_gap_for_every_taskbar_edge() {
            assert_eq!(MENU_GAP, 6.);

            let monitor = PhysicalRect {
                left: 0.,
                top: 0.,
                right: 1920.,
                bottom: 1080.,
            };
            let cases = [
                (
                    TaskbarEdge::Top,
                    PhysicalRect {
                        left: 948.,
                        top: 200.,
                        right: 972.,
                        bottom: 224.,
                    },
                    PhysicalRect {
                        top: 80.,
                        ..monitor
                    },
                ),
                (
                    TaskbarEdge::Bottom,
                    PhysicalRect {
                        left: 948.,
                        top: 800.,
                        right: 972.,
                        bottom: 824.,
                    },
                    PhysicalRect {
                        bottom: 1000.,
                        ..monitor
                    },
                ),
                (
                    TaskbarEdge::Left,
                    PhysicalRect {
                        left: 200.,
                        top: 528.,
                        right: 224.,
                        bottom: 552.,
                    },
                    PhysicalRect {
                        left: 80.,
                        ..monitor
                    },
                ),
                (
                    TaskbarEdge::Right,
                    PhysicalRect {
                        left: 1696.,
                        top: 528.,
                        right: 1720.,
                        bottom: 552.,
                    },
                    PhysicalRect {
                        right: 1840.,
                        ..monitor
                    },
                ),
            ];

            for (expected_edge, icon, work_area) in cases {
                let layout = tray_popup_layout(icon, monitor, work_area, 1.);
                assert_eq!(layout.edge, expected_edge);
                let surface = PhysicalRect {
                    left: layout.outer.left + layout.surface_offset.0,
                    top: layout.outer.top + layout.surface_offset.1,
                    right: layout.outer.left + layout.surface_offset.0 + MENU_WIDTH,
                    bottom: layout.outer.top + layout.surface_offset.1 + MENU_HEIGHT,
                };
                let gap = match expected_edge {
                    TaskbarEdge::Top => surface.top - icon.bottom,
                    TaskbarEdge::Bottom => icon.top - surface.bottom,
                    TaskbarEdge::Left => surface.left - icon.right,
                    TaskbarEdge::Right => icon.left - surface.right,
                };
                assert!((gap - MENU_GAP).abs() < f32::EPSILON);
            }
        }

        #[test]
        fn auto_hide_uses_the_nearest_monitor_edge() {
            let monitor = PhysicalRect {
                left: 0.,
                top: 0.,
                right: 1920.,
                bottom: 1080.,
            };
            assert_eq!(taskbar_edge(monitor, monitor, (960., 4.)), TaskbarEdge::Top);
            assert_eq!(
                taskbar_edge(monitor, monitor, (960., 1076.)),
                TaskbarEdge::Bottom
            );
            assert_eq!(
                taskbar_edge(monitor, monitor, (4., 540.)),
                TaskbarEdge::Left
            );
            assert_eq!(
                taskbar_edge(monitor, monitor, (1916., 540.)),
                TaskbarEdge::Right
            );
        }

        #[test]
        fn ambiguous_work_area_uses_icon_nearest_edge() {
            let monitor = PhysicalRect {
                left: 0.,
                top: 0.,
                right: 1920.,
                bottom: 1080.,
            };
            let work = PhysicalRect {
                top: 40.,
                bottom: 1040.,
                ..monitor
            };
            assert_eq!(taskbar_edge(monitor, work, (960., 4.)), TaskbarEdge::Top);
        }

        #[test]
        fn tray_layout_handles_negative_coordinates_and_clamps_surface() {
            let monitor = PhysicalRect {
                left: -1920.,
                top: 0.,
                right: 0.,
                bottom: 1080.,
            };
            let icon = PhysicalRect {
                left: -1920.,
                top: 528.,
                right: -1896.,
                bottom: 552.,
            };
            let layout = tray_popup_layout(
                icon,
                monitor,
                PhysicalRect {
                    left: -1880.,
                    ..monitor
                },
                1.,
            );
            assert_eq!(layout.edge, TaskbarEdge::Left);
            assert!(layout.outer.left >= -1880. - 6.);
            assert!(layout.arrow_offset >= 16.);
            assert!(layout.arrow_offset <= MENU_HEIGHT - 16.);
        }

        #[test]
        fn tray_layout_scales_icon_and_monitor_coordinates_together() {
            let monitor = PhysicalRect {
                left: 0.,
                top: 0.,
                right: 3840.,
                bottom: 2160.,
            };
            let icon = PhysicalRect {
                left: 1896.,
                top: 2112.,
                right: 1920.,
                bottom: 2160.,
            };
            let layout = tray_popup_layout(
                icon,
                monitor,
                PhysicalRect {
                    bottom: 2080.,
                    ..monitor
                },
                2.,
            );
            assert_eq!(layout.edge, TaskbarEdge::Bottom);
            assert_eq!(layout.outer.bottom, 1052.);
        }

        #[test]
        fn nearest_edge_has_a_deterministic_center_tie_break() {
            let monitor = PhysicalRect {
                left: 0.,
                top: 0.,
                right: 100.,
                bottom: 100.,
            };
            assert_eq!(nearest_monitor_edge(monitor, (50., 50.)), TaskbarEdge::Top);
        }

        #[test]
        fn tray_events_are_delivered_by_the_callback_without_polling() {
            use futures::StreamExt;

            let id = TrayIconId::new("ralgrum-event-channel-test");
            let event = TrayIconEvent::Click {
                id: id.clone(),
                position: dpi::PhysicalPosition::new(100., 200.),
                rect: Rect::default(),
                button: MouseButton::Right,
                button_state: MouseButtonState::Down,
            };
            let (sender, mut receiver) = futures::channel::mpsc::unbounded();
            super::forward_tray_event(&sender, event);
            let received = futures::executor::block_on(receiver.next())
                .expect("event callback channel should deliver immediately");
            assert_eq!(
                map_tray_event(&received, &id),
                Some(TrayInteraction::OpenMenu {
                    x: 100.,
                    y: 200.,
                    tray_rect: Rect::default(),
                })
            );
        }
    }
}

#[cfg(windows)]
pub(crate) fn install(
    cx: &mut App,
    window: &Window,
    shell: gpui::WeakEntity<crate::shell::RalgrumApp>,
) -> bool {
    windows_impl::install(cx, window, shell)
}

#[cfg(not(windows))]
pub(crate) fn install(
    _: &mut App,
    _: &Window,
    _: gpui::WeakEntity<crate::shell::RalgrumApp>,
) -> bool {
    false
}

#[cfg(windows)]
pub(crate) fn quit_requested(cx: &App) -> bool {
    windows_impl::quit_requested(cx)
}

#[cfg(not(windows))]
pub(crate) fn quit_requested(_: &App) -> bool {
    false
}

#[cfg(windows)]
pub(crate) fn hide_window(window: &Window) {
    windows_impl::hide_window(window);
}

#[cfg(windows)]
pub(crate) fn restore_main_window(cx: &mut App) {
    windows_impl::restore_main_window(cx);
}

#[cfg(not(windows))]
pub(crate) fn hide_window(_: &Window) {}

#[cfg(not(windows))]
pub(crate) fn restore_main_window(_: &mut App) {}

#[cfg(test)]
mod tests {
    use super::should_hide_on_close;

    #[test]
    fn close_to_tray_requires_both_preference_and_tray() {
        assert!(should_hide_on_close(true, true));
        assert!(!should_hide_on_close(false, true));
    }
}
