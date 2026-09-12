use std::ffi::c_void;

use windows::{
    Win32::{
        Foundation::{HWND, LPARAM, WPARAM},
        Graphics::Dwm::{
            DWMWA_BORDER_COLOR, DWMWA_CAPTION_COLOR, DWMWA_TEXT_COLOR,
            DWMWA_USE_IMMERSIVE_DARK_MODE, DWMWINDOWATTRIBUTE, DwmSetWindowAttribute,
        },
        System::LibraryLoader::GetModuleHandleW,
        UI::WindowsAndMessaging::{
            GetSystemMetrics, HICON, ICON_BIG, ICON_SMALL, IMAGE_ICON, LR_DEFAULTCOLOR, LR_SHARED,
            LoadImageW, SM_CXICON, SM_CXSMICON, SM_CYICON, SM_CYSMICON, SendMessageW, WM_SETICON,
        },
    },
    core::PCWSTR,
};

use crate::theme::{BORDER, CHROME};

const APP_ICON_RESOURCE_ID: u16 = 1;
const CAPTION_TEXT: u32 = 0xd4d4d8;

pub(crate) fn apply_app_window_chrome(hwnd: HWND) {
    apply_app_window_icons(hwnd);
    apply_dark_caption(hwnd);
}

fn apply_dark_caption(hwnd: HWND) {
    let dark_mode = 1i32;
    let caption = colorref_from_rgb(CHROME);
    let border = colorref_from_rgb(BORDER);
    let text = colorref_from_rgb(CAPTION_TEXT);
    set_dwm_attribute(hwnd, DWMWA_USE_IMMERSIVE_DARK_MODE, &dark_mode);
    set_dwm_attribute(hwnd, DWMWA_CAPTION_COLOR, &caption);
    set_dwm_attribute(hwnd, DWMWA_BORDER_COLOR, &border);
    set_dwm_attribute(hwnd, DWMWA_TEXT_COLOR, &text);
}

fn set_dwm_attribute<T>(hwnd: HWND, attribute: DWMWINDOWATTRIBUTE, value: &T) {
    let _ = unsafe {
        DwmSetWindowAttribute(
            hwnd,
            attribute,
            value as *const T as *const c_void,
            std::mem::size_of::<T>() as u32,
        )
    };
}

fn apply_app_window_icons(hwnd: HWND) {
    let big = load_shared_app_icon(unsafe { GetSystemMetrics(SM_CXICON) }, unsafe {
        GetSystemMetrics(SM_CYICON)
    });
    let small = load_shared_app_icon(unsafe { GetSystemMetrics(SM_CXSMICON) }, unsafe {
        GetSystemMetrics(SM_CYSMICON)
    });
    if let Ok(icon) = big {
        unsafe {
            SendMessageW(
                hwnd,
                WM_SETICON,
                Some(WPARAM(ICON_BIG as usize)),
                Some(LPARAM(icon.0 as isize)),
            );
        }
    }
    if let Ok(icon) = small {
        unsafe {
            SendMessageW(
                hwnd,
                WM_SETICON,
                Some(WPARAM(ICON_SMALL as usize)),
                Some(LPARAM(icon.0 as isize)),
            );
        }
    }
}

fn load_shared_app_icon(width: i32, height: i32) -> Result<HICON, String> {
    let module = unsafe { GetModuleHandleW(PCWSTR::null()) }
        .map_err(|error| format!("could not locate the application module: {error}"))?;
    let handle = unsafe {
        LoadImageW(
            Some(module.into()),
            PCWSTR::from_raw(APP_ICON_RESOURCE_ID as usize as *const u16),
            IMAGE_ICON,
            width,
            height,
            LR_DEFAULTCOLOR | LR_SHARED,
        )
    }
    .map_err(|error| format!("could not load the shared application icon resource: {error}"))?;
    Ok(HICON(handle.0))
}

const fn colorref_from_rgb(rgb: u32) -> u32 {
    ((rgb & 0x0000ff) << 16) | (rgb & 0x00ff00) | ((rgb & 0xff0000) >> 16)
}

#[cfg(test)]
mod tests {
    use super::{CAPTION_TEXT, colorref_from_rgb};
    use crate::theme::{BORDER, CHROME};

    #[test]
    fn native_caption_uses_main_titlebar_colors_in_windows_color_order() {
        assert_eq!(colorref_from_rgb(CHROME), 0x0e0c0c);
        assert_eq!(colorref_from_rgb(BORDER), 0x2a2727);
        assert_eq!(colorref_from_rgb(CAPTION_TEXT), 0xd8d4d4);
    }
}
