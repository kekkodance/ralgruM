#[cfg(windows)]
mod platform {
    use std::{
        ffi::c_void,
        sync::{Mutex, OnceLock, mpsc::Sender},
    };

    use gpui::{
        App, Bounds, ClickEvent, Context, FontWeight, IntoElement, Render, Role, TitlebarOptions,
        Window, WindowBounds, WindowControlArea, WindowHandle, WindowOptions, div, prelude::*, px,
        rgb, size,
    };
    use windows::{
        Win32::{
            Foundation::{HWND, POINT},
            Graphics::Gdi::ClientToScreen,
            UI::WindowsAndMessaging::{
                CreateWindowExW, DestroyWindow, HWND_TOP, SW_HIDE, SWP_NOACTIVATE,
                SWP_NOOWNERZORDER, SWP_NOZORDER, SWP_SHOWWINDOW, SetWindowPos, ShowWindow,
                WS_CLIPCHILDREN, WS_CLIPSIBLINGS, WS_EX_TOOLWINDOW, WS_POPUP,
            },
        },
        core::{PCWSTR, w},
    };

    use super::super::{HostCommand, command_sender};
    use crate::{
        assets::{LocalIcon, local_icon},
        theme::{BORDER, CHROME, FOREGROUND, MUTED, PRIMARY, SURFACE, SURFACE_RAISED},
        toast::{self, ToastKind},
    };

    const TITLEBAR_HEIGHT: f32 = 32.0;
    const INITIAL_WIDTH: f32 = 370.0;
    const INITIAL_HEIGHT: f32 = 250.0 + TITLEBAR_HEIGHT;

    static EDITOR_WINDOW: OnceLock<Mutex<Option<WindowHandle<EditorWindow>>>> = OnceLock::new();

    pub(crate) fn open(cx: &mut App) {
        let slot = EDITOR_WINDOW.get_or_init(|| Mutex::new(None));
        if let Some(handle) = slot.lock().ok().and_then(|slot| *slot) {
            if handle
                .update(cx, |_, window, _| window.activate_window())
                .is_ok()
            {
                return;
            }
            if let Ok(mut slot) = slot.lock() {
                *slot = None;
            }
        }

        let Some(commands) = command_sender() else {
            show_error(
                cx,
                "Enable MiniMeters Integration before opening its interface",
            );
            return;
        };
        let bounds = Bounds::centered(None, size(px(INITIAL_WIDTH), px(INITIAL_HEIGHT)), cx);
        let opened = cx.open_window(
            WindowOptions {
                window_bounds: Some(WindowBounds::Windowed(bounds)),
                titlebar: Some(TitlebarOptions {
                    title: Some("MiniMeters Integration".into()),
                    appears_transparent: true,
                    ..Default::default()
                }),
                app_owns_titlebar_drag: true,
                focus: true,
                show: true,
                is_resizable: false,
                window_min_size: Some(size(px(320.), px(220.))),
                ..Default::default()
            },
            move |window, cx| build_editor(commands, window, cx),
        );
        match opened {
            Ok(handle) => {
                if let Ok(mut slot) = slot.lock() {
                    *slot = Some(handle);
                }
            }
            Err(error) => show_error(
                cx,
                format!("MiniMeters window could not be opened: {error}"),
            ),
        }
    }

    pub(crate) fn close(cx: &mut App) {
        let Some(handle) = EDITOR_WINDOW
            .get()
            .and_then(|slot| slot.lock().ok())
            .and_then(|slot| *slot)
        else {
            return;
        };
        if handle
            .update(cx, |view, window, cx| view.begin_close(window, cx))
            .is_err()
        {
            clear_window(handle);
        }
    }

    fn build_editor(
        commands: Sender<HostCommand>,
        window: &mut Window,
        cx: &mut App,
    ) -> gpui::Entity<EditorWindow> {
        if let Some(raw) = crate::media_control::window_handle(window) {
            crate::windows_chrome::apply_app_window_chrome(HWND(raw as *mut c_void));
        }
        let container = NativeContainer::new(window);
        let error = container.as_ref().err().cloned();
        let parent_hwnd = container.as_ref().ok().map(NativeContainer::raw);
        let view = cx.new(|_| EditorWindow {
            commands: commands.clone(),
            container: container.ok(),
            attached: false,
            closing: false,
            error,
        });
        let weak = view.downgrade();
        window.on_window_should_close(cx, move |window, cx| {
            weak.update(cx, |view, cx| view.begin_close(window, cx))
                .is_err()
        });
        if let Some(parent_hwnd) = parent_hwnd {
            let (response, receiver) = futures::channel::oneshot::channel();
            if commands
                .send(HostCommand::OpenEditor {
                    parent_hwnd,
                    scale: window.scale_factor() as f64,
                    response,
                })
                .is_err()
            {
                view.update(cx, |view, cx| {
                    view.error = Some("The MiniMeters host is no longer running".into());
                    cx.notify();
                });
            } else {
                let weak = view.downgrade();
                window
                    .spawn(cx, async move |cx| {
                        let result = receiver.await.unwrap_or_else(|_| {
                            Err("The MiniMeters host stopped unexpectedly".into())
                        });
                        let _ = cx.update(|window, cx| {
                            weak.update(cx, |view, cx| {
                                if view.closing {
                                    return;
                                }
                                match result {
                                    Ok((width, height)) => {
                                        let scale = window.scale_factor();
                                        window.resize(size(
                                            px(width as f32 / scale),
                                            px(height as f32 / scale + TITLEBAR_HEIGHT),
                                        ));
                                        view.attached = true;
                                        if let Some(container) = &view.container {
                                            container.show(window, width, height);
                                        }
                                    }
                                    Err(error) => {
                                        show_error(
                                            cx,
                                            format!("MiniMeters UI could not be opened: {error}"),
                                        );
                                        view.error = Some(error);
                                    }
                                }
                                cx.notify();
                            })
                            .ok();
                        });
                    })
                    .detach();
            }
        }
        view
    }

    struct EditorWindow {
        commands: Sender<HostCommand>,
        container: Option<NativeContainer>,
        attached: bool,
        closing: bool,
        error: Option<String>,
    }

    impl EditorWindow {
        fn begin_close(&mut self, window: &mut Window, cx: &mut App) {
            if self.closing {
                return;
            }
            self.closing = true;
            self.attached = false;
            if let Some(container) = &self.container {
                container.hide();
            }
            let handle = window.window_handle().downcast::<EditorWindow>();
            let (response, receiver) = futures::channel::oneshot::channel();
            let sent = self
                .commands
                .send(HostCommand::CloseEditor { response })
                .is_ok();
            window
                .spawn(cx, async move |cx| {
                    if sent {
                        let _ = receiver.await;
                    }
                    let _ = cx.update(|window, _| window.remove_window());
                    if let Some(handle) = handle {
                        clear_window(handle);
                    }
                })
                .detach();
        }
    }

    impl Render for EditorWindow {
        fn render(&mut self, window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
            if let Some(container) = &self.container {
                container.resize(window);
            }
            div()
                .size_full()
                .flex()
                .flex_col()
                .bg(rgb(SURFACE))
                .text_color(rgb(FOREGROUND))
                .child(render_titlebar(cx))
                .child(
                    div()
                        .flex_1()
                        .flex()
                        .items_center()
                        .justify_center()
                        .text_size(px(12.))
                        .text_color(rgb(MUTED))
                        .when_some(self.error.clone(), |this, error| this.child(error))
                        .when(self.error.is_none() && !self.attached, |this| {
                            this.child("Opening MiniMeters...")
                        }),
                )
        }
    }

    fn render_titlebar(cx: &mut Context<EditorWindow>) -> impl IntoElement {
        div()
            .h(px(TITLEBAR_HEIGHT))
            .w_full()
            .flex_none()
            .flex()
            .items_center()
            .justify_between()
            .bg(rgb(CHROME))
            .border_b_1()
            .border_color(rgb(BORDER))
            .child(
                div()
                    .h_full()
                    .flex_1()
                    .flex()
                    .items_center()
                    .px(px(11.))
                    .gap(px(7.))
                    .window_control_area(WindowControlArea::Drag)
                    .text_size(px(11.5))
                    .font_weight(FontWeight::MEDIUM)
                    .text_color(rgb(0xd4d4d8))
                    .child(local_icon(LocalIcon::WaveSquare, PRIMARY).size(px(10.)))
                    .child("MiniMeters Integration"),
            )
            .child(
                div()
                    .h_full()
                    .flex()
                    .child(title_control(
                        LocalIcon::Minus,
                        "Minimize",
                        WindowControlArea::Min,
                        cx,
                    ))
                    .child(title_control(
                        LocalIcon::X,
                        "Close",
                        WindowControlArea::Close,
                        cx,
                    )),
            )
    }

    fn title_control(
        icon: LocalIcon,
        label: &'static str,
        area: WindowControlArea,
        cx: &mut Context<EditorWindow>,
    ) -> impl IntoElement {
        let close = area == WindowControlArea::Close;
        div()
            .id(label)
            .group(label)
            .focusable()
            .tab_stop(true)
            .role(Role::Button)
            .aria_label(label)
            .w(px(46.))
            .h(px(TITLEBAR_HEIGHT))
            .flex()
            .items_center()
            .justify_center()
            .text_color(rgb(MUTED))
            .window_control_area(area)
            .hover(move |style| {
                style
                    .bg(rgb(if close { 0xc42b1c } else { SURFACE_RAISED }))
                    .text_color(rgb(if close { 0xffffff } else { FOREGROUND }))
            })
            .focus_visible(|style| style.border_1().border_color(rgb(PRIMARY)))
            .on_click(cx.listener(move |this, event: &ClickEvent, window, cx| {
                if !matches!(event, ClickEvent::Keyboard(_)) {
                    return;
                }
                match area {
                    WindowControlArea::Min => window.minimize_window(),
                    WindowControlArea::Close => this.begin_close(window, cx),
                    _ => {}
                }
            }))
            .child(
                div()
                    .size(if close { px(10.5) } else { px(11.) })
                    .relative()
                    .child(
                        div()
                            .absolute()
                            .inset_0()
                            .group_hover(label, |style| style.invisible())
                            .child(local_icon(icon, MUTED).size_full()),
                    )
                    .child(
                        div()
                            .absolute()
                            .inset_0()
                            .invisible()
                            .group_hover(label, |style| style.visible())
                            .child(
                                local_icon(icon, if close { 0xffffff } else { FOREGROUND })
                                    .size_full(),
                            ),
                    ),
            )
    }

    struct NativeContainer {
        hwnd: HWND,
        owner: HWND,
    }

    impl NativeContainer {
        fn new(window: &Window) -> Result<Self, String> {
            let parent = crate::media_control::window_handle(window)
                .ok_or("The MiniMeters window handle is unavailable")?;
            let owner = HWND(parent as *mut c_void);
            let hwnd = unsafe {
                CreateWindowExW(
                    WS_EX_TOOLWINDOW,
                    w!("STATIC"),
                    PCWSTR::null(),
                    WS_POPUP | WS_CLIPCHILDREN | WS_CLIPSIBLINGS,
                    0,
                    0,
                    1,
                    1,
                    Some(owner),
                    None,
                    None,
                    None,
                )
            }
            .map_err(|error| format!("MiniMeters container could not be created: {error}"))?;
            Ok(Self { hwnd, owner })
        }

        fn raw(&self) -> isize {
            self.hwnd.0 as isize
        }

        fn resize(&self, window: &Window) {
            let scale = window.scale_factor();
            let viewport = window.viewport_size();
            let width = (f32::from(viewport.width) * scale).round() as i32;
            let titlebar = (TITLEBAR_HEIGHT * scale).round() as i32;
            let height = ((f32::from(viewport.height) - TITLEBAR_HEIGHT) * scale)
                .round()
                .max(1.0) as i32;
            self.set_bounds(titlebar, width.max(1), height, false);
        }

        fn show(&self, window: &Window, width: u32, height: u32) {
            let titlebar = (TITLEBAR_HEIGHT * window.scale_factor()).round() as i32;
            self.set_bounds(titlebar, width as i32, height as i32, true);
        }

        fn hide(&self) {
            let _ = unsafe { ShowWindow(self.hwnd, SW_HIDE) };
        }

        fn set_bounds(&self, titlebar: i32, width: i32, height: i32, show: bool) {
            let mut origin = POINT { x: 0, y: titlebar };
            if !unsafe { ClientToScreen(self.owner, &mut origin) }.as_bool() {
                return;
            }
            let flags = SWP_NOACTIVATE
                | SWP_NOOWNERZORDER
                | if show { SWP_SHOWWINDOW } else { SWP_NOZORDER };
            let insert_after = show.then_some(HWND_TOP);
            let _ = unsafe {
                SetWindowPos(
                    self.hwnd,
                    insert_after,
                    origin.x,
                    origin.y,
                    width.max(1),
                    height.max(1),
                    flags,
                )
            };
        }
    }

    impl Drop for NativeContainer {
        fn drop(&mut self) {
            let _ = unsafe { DestroyWindow(self.hwnd) };
        }
    }

    fn clear_window(handle: WindowHandle<EditorWindow>) {
        if let Some(slot) = EDITOR_WINDOW.get()
            && let Ok(mut slot) = slot.lock()
            && slot.as_ref().is_some_and(|current| *current == handle)
        {
            *slot = None;
        }
    }

    fn show_error(cx: &mut App, message: impl Into<String>) {
        toast::push_global(
            cx,
            ToastKind::Error,
            "MiniMeters UI could not be opened",
            Some(message.into().into()),
        );
    }
}

#[cfg(windows)]
pub(super) use platform::{close, open};

#[cfg(not(windows))]
pub(super) fn open(cx: &mut gpui::App) {
    crate::toast::push_global(
        cx,
        crate::toast::ToastKind::Error,
        "MiniMeters UI could not be opened",
        Some("The MiniMeters integration is currently available only on Windows".into()),
    );
}

#[cfg(not(windows))]
pub(super) fn close(_: &mut gpui::App) {}
