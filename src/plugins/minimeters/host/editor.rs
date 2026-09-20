use clack_extensions::gui::{
    GuiApiType, GuiConfiguration, HostGuiImpl, PluginGui, Window as ClapWindow,
};
use clack_host::prelude::*;

use super::{Callbacks, MeterHost};
use crate::plugins::minimeters::HostCommand;

pub(super) struct GuiMainThread;

impl MainThreadHandler<'_> for GuiMainThread {}

impl HostGuiImpl for Callbacks {
    fn resize_hints_changed(&self) {}

    fn request_resize(&self, _new_size: clack_extensions::gui::GuiSize) -> Result<(), HostError> {
        Err(HostError::Message(
            "The MiniMeters editor uses its fixed size",
        ))
    }

    fn request_show(&self) -> Result<(), HostError> {
        Err(HostError::Message("The editor is controlled by ralgruM"))
    }

    fn request_hide(&self) -> Result<(), HostError> {
        Err(HostError::Message("The editor is controlled by ralgruM"))
    }

    fn closed(&self, _was_destroyed: bool) {}
}

pub(super) fn handle_command(
    instance: &mut PluginInstance<MeterHost>,
    editor_open: &mut bool,
    command: HostCommand,
) {
    match command {
        HostCommand::OpenEditor {
            parent_hwnd,
            scale,
            response,
        } => {
            let result = open(instance, editor_open, parent_hwnd, scale);
            let _ = response.send(result);
        }
        HostCommand::CloseEditor { response } => {
            close(instance, editor_open);
            let _ = response.send(());
        }
    }
}

fn open(
    instance: &mut PluginInstance<MeterHost>,
    editor_open: &mut bool,
    parent_hwnd: isize,
    scale: f64,
) -> Result<(u32, u32), String> {
    if *editor_open {
        return Err("The MiniMeters plugin UI is already open".into());
    }
    let handle = instance.plugin_handle();
    let gui = handle
        .get_extension::<PluginGui>()
        .ok_or("MiniMeters Audio Server does not provide a plugin UI")?;
    let configuration = GuiConfiguration {
        api_type: GuiApiType::WIN32,
        is_floating: false,
    };
    if !gui.is_api_supported(&handle, configuration) {
        return Err("MiniMeters Audio Server does not support an embedded Win32 UI".into());
    }
    gui.create(&handle, configuration)
        .map_err(|error| error.to_string())?;
    let result = (|| {
        if let Err(error) = gui.set_scale(&handle, scale) {
            crate::diagnostics::event(
                "DEBUG",
                format!("MiniMeters editor kept its native scale: {error}"),
            );
        }
        let size = gui
            .get_size(&handle)
            .ok_or("MiniMeters did not report a plugin UI size")?;
        // SAFETY: The GPUI-owned child HWND stays alive until CloseEditor has
        // synchronously destroyed the CLAP editor.
        let parent = unsafe { ClapWindow::from_win32_hwnd(parent_hwnd as *mut _) };
        // SAFETY: The parent window lifetime is enforced by the editor window's close handshake.
        unsafe { gui.set_parent(&handle, parent) }.map_err(|error| error.to_string())?;
        gui.show(&handle).map_err(|error| error.to_string())?;
        Ok((size.width, size.height))
    })();
    if result.is_err() {
        gui.destroy(&handle);
    } else {
        *editor_open = true;
    }
    result
}

pub(super) fn close(instance: &mut PluginInstance<MeterHost>, editor_open: &mut bool) {
    if !*editor_open {
        return;
    }
    let handle = instance.plugin_handle();
    if let Some(gui) = handle.get_extension::<PluginGui>() {
        let _ = gui.hide(&handle);
        gui.destroy(&handle);
    }
    *editor_open = false;
}

#[cfg(windows)]
pub(super) fn pump_messages() {
    use windows::Win32::UI::WindowsAndMessaging::{
        DispatchMessageW, MSG, PM_REMOVE, PeekMessageW, TranslateMessage,
    };

    let mut message = MSG::default();
    while unsafe { PeekMessageW(&mut message, None, 0, 0, PM_REMOVE) }.as_bool() {
        unsafe {
            let _ = TranslateMessage(&message);
            DispatchMessageW(&message);
        }
    }
}

#[cfg(not(windows))]
pub(super) fn pump_messages() {}
