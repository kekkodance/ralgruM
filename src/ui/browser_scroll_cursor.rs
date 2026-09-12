use gpui::{CursorStyle, Window};

/// The cursor state used by a vertical middle-click autoscroll surface.
///
/// Chromium uses three dedicated Windows cursor resources for this gesture:
/// a centered vertical pan cursor and one cursor for each direction. `Reset`
/// means that this surface no longer owns the browser cursor.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub(crate) enum BrowserScrollCursor {
    Reset,
    DeadZone,
    Above,
    Below,
}

impl BrowserScrollCursor {
    /// Convert the GPUI directional fallback into the corresponding browser
    /// cursor state. The fallback is also used on platforms without native
    /// Windows cursor resources.
    pub(crate) const fn from_gpui_style(style: CursorStyle) -> Self {
        match style {
            CursorStyle::ResizeUp => Self::Above,
            CursorStyle::ResizeDown => Self::Below,
            CursorStyle::ResizeUpDown => Self::DeadZone,
            _ => Self::Reset,
        }
    }

    /// Classify a pointer offset relative to the middle-click anchor.
    #[cfg(test)]
    pub(crate) const fn from_position(pointer_y: f32, anchor_y: f32, dead_zone_px: f32) -> Self {
        let distance = pointer_y - anchor_y;
        if distance < -dead_zone_px {
            Self::Above
        } else if distance > dead_zone_px {
            Self::Below
        } else {
            Self::DeadZone
        }
    }

    /// Return the closest built-in GPUI cursor for non-Windows platforms and
    /// as a fallback if creating a native cursor fails.
    #[cfg(any(not(windows), test))]
    pub(crate) const fn fallback_style(self) -> Option<CursorStyle> {
        match self {
            Self::Reset => None,
            Self::DeadZone => Some(CursorStyle::ResizeUpDown),
            Self::Above => Some(CursorStyle::ResizeUp),
            Self::Below => Some(CursorStyle::ResizeDown),
        }
    }

    #[cfg(all(windows, not(test)))]
    const fn is_active(self) -> bool {
        !matches!(self, Self::Reset)
    }
}

/// Apply the browser autoscroll cursor for one surface.
///
/// `owner` is the identity of the scroll state that requested the update. It
/// prevents an inactive surface's reset from clearing another surface's
/// active cursor. On Windows the native bridge subclasses the window and
/// chains the WNDPROC that was installed before it, which may be the drag
/// cursor bridge.
pub(crate) fn set_browser_scroll_cursor(
    window: &mut Window,
    cursor: BrowserScrollCursor,
    owner: usize,
) {
    #[cfg(any(not(windows), test))]
    {
        let _ = owner;
        if let Some(style) = cursor.fallback_style() {
            window.set_window_cursor_style(style);
        }
    }

    #[cfg(all(windows, not(test)))]
    windows_impl::set_browser_scroll_cursor(window, cursor, owner);
}

/// Consume a native focus or capture cancellation for one scroll surface.
///
/// Windows can revoke mouse capture without producing a GPUI mouse-up event.
/// The native cursor hook records that transition so the Rust scrolling state
/// can stop on its next render instead of reclaiming the pan cursor.
pub(crate) fn take_browser_scroll_cursor_cancellation(owner: usize) -> bool {
    #[cfg(all(windows, not(test)))]
    {
        windows_impl::take_browser_scroll_cursor_cancellation(owner)
    }

    #[cfg(any(not(windows), test))]
    {
        let _ = owner;
        false
    }
}

/// Reapply GPUI's current cursor after native panning was cancelled outside
/// the normal GPUI mouse-up path.
pub(crate) fn restore_browser_scroll_cursor(window: &mut Window) {
    #[cfg(all(windows, not(test)))]
    windows_impl::restore_browser_scroll_cursor(window);

    #[cfg(any(not(windows), test))]
    let _ = window;
}

const PAN_MIDDLE_VERTICAL: &str = include_str!("../../assets/cursors/pan_middle_vertical.cur.b64");
const PAN_NORTH: &str = include_str!("../../assets/cursors/pan_north.cur.b64");
const PAN_SOUTH: &str = include_str!("../../assets/cursors/pan_south.cur.b64");

/// Decode the selected image from a Windows CUR file into the resource format
/// accepted by `CreateIconFromResourceEx`.
///
/// CUR files use the same image payload as ICO files, with the first two
/// fields of each directory entry carrying the hotspot instead of icon
/// dimensions. The native API expects that hotspot prepended to the selected
/// image payload.
fn cursor_resource(bytes: &[u8]) -> Option<Vec<u8>> {
    if le_u16(bytes, 0)? != 0 || le_u16(bytes, 2)? != 2 {
        return None;
    }
    let entry_count = usize::from(le_u16(bytes, 4)?);
    if entry_count == 0 {
        return None;
    }
    let entries_end = 6usize.checked_add(entry_count.checked_mul(16)?)?;
    if entries_end > bytes.len() {
        return None;
    }

    // Prefer the highest bit depth, then the largest payload. This mirrors
    // the selection used by the existing drag cursor bridge and keeps a
    // monochrome fallback from winning over the color image.
    type CursorCandidate<'a> = (u16, u16, &'a [u8], (u16, usize));
    let mut best: Option<CursorCandidate<'_>> = None;
    for index in 0..entry_count {
        let entry = 6usize.checked_add(index.checked_mul(16)?)?;
        let hotspot_x = le_u16(bytes, entry + 4)?;
        let hotspot_y = le_u16(bytes, entry + 6)?;
        let image_size = usize::try_from(le_u32(bytes, entry + 8)?).ok()?;
        let image_offset = usize::try_from(le_u32(bytes, entry + 12)?).ok()?;
        let image_end = image_offset.checked_add(image_size)?;
        let image = bytes.get(image_offset..image_end)?;
        if image.len() < 16 {
            return None;
        }
        let bit_depth = le_u16(image, 14).unwrap_or_default();
        let score = (bit_depth, image_size);
        if best.as_ref().map(|best| score > best.3).unwrap_or(true) {
            best = Some((hotspot_x, hotspot_y, image, score));
        }
    }

    let (hotspot_x, hotspot_y, image, _) = best?;
    let mut resource = Vec::with_capacity(image.len().checked_add(4)?);
    resource.extend_from_slice(&hotspot_x.to_le_bytes());
    resource.extend_from_slice(&hotspot_y.to_le_bytes());
    resource.extend_from_slice(image);
    Some(resource)
}

fn le_u16(bytes: &[u8], offset: usize) -> Option<u16> {
    Some(u16::from_le_bytes(
        bytes.get(offset..offset.checked_add(2)?)?.try_into().ok()?,
    ))
}

fn le_u32(bytes: &[u8], offset: usize) -> Option<u32> {
    Some(u32::from_le_bytes(
        bytes.get(offset..offset.checked_add(4)?)?.try_into().ok()?,
    ))
}

#[cfg(all(windows, not(test)))]
mod windows_impl {
    use std::{
        collections::{HashMap, HashSet},
        ffi::c_void,
        sync::{Mutex, OnceLock},
    };

    use base64::{Engine as _, engine::general_purpose::STANDARD};
    use gpui::Window;
    use raw_window_handle::{HasWindowHandle, RawWindowHandle};
    use windows::Win32::{
        Foundation::{HWND, LPARAM, LRESULT, WPARAM},
        UI::WindowsAndMessaging::{
            CallWindowProcW, CreateIconFromResourceEx, GWLP_WNDPROC, HCURSOR, HTCLIENT,
            LR_DEFAULTSIZE, SendMessageW, SetCursor, SetWindowLongPtrW, WM_CANCELMODE,
            WM_CAPTURECHANGED, WM_KILLFOCUS, WM_NCDESTROY, WM_SETCURSOR, WM_USER, WNDPROC,
        },
    };

    use super::{BrowserScrollCursor, PAN_MIDDLE_VERTICAL, PAN_NORTH, PAN_SOUTH};

    const GPUI_CURSOR_STYLE_CHANGED: u32 = WM_USER + 1;

    #[derive(Clone, Copy)]
    struct WindowHook {
        original_proc: isize,
        cursor: BrowserScrollCursor,
        owner: Option<usize>,
    }

    static WINDOW_HOOKS: OnceLock<Mutex<HashMap<isize, WindowHook>>> = OnceLock::new();
    static CANCELLED_OWNERS: OnceLock<Mutex<HashSet<usize>>> = OnceLock::new();
    static PAN_MIDDLE_VERTICAL_CURSOR: OnceLock<usize> = OnceLock::new();
    static PAN_NORTH_CURSOR: OnceLock<usize> = OnceLock::new();
    static PAN_SOUTH_CURSOR: OnceLock<usize> = OnceLock::new();

    fn window_hooks() -> &'static Mutex<HashMap<isize, WindowHook>> {
        WINDOW_HOOKS.get_or_init(|| Mutex::new(HashMap::new()))
    }

    fn cancelled_owners() -> &'static Mutex<HashSet<usize>> {
        CANCELLED_OWNERS.get_or_init(|| Mutex::new(HashSet::new()))
    }

    pub(super) fn take_browser_scroll_cursor_cancellation(owner: usize) -> bool {
        cancelled_owners()
            .lock()
            .map(|mut owners| owners.remove(&owner))
            .unwrap_or(false)
    }

    pub(super) fn restore_browser_scroll_cursor(window: &mut Window) {
        if let Some(hwnd) = window_hwnd(window) {
            restore_gpui_cursor(hwnd);
        }
    }

    fn restore_gpui_cursor(hwnd: HWND) {
        unsafe {
            SendMessageW(
                hwnd,
                WM_SETCURSOR,
                Some(WPARAM(hwnd.0 as usize)),
                Some(LPARAM(HTCLIENT as isize)),
            );
        }
    }

    pub(super) fn set_browser_scroll_cursor(
        window: &mut Window,
        cursor: BrowserScrollCursor,
        owner: usize,
    ) {
        let Some(hwnd) = window_hwnd(window) else {
            return;
        };
        let key = hwnd.0 as isize;
        if !set_hook_state(hwnd, cursor, owner) {
            return;
        }
        if !cursor.is_active() {
            // The native pan cursor temporarily sits above GPUI's normal
            // cursor. Re-dispatch WM_SETCURSOR after clearing ownership so
            // mouse-up restores GPUI's current cursor immediately.
            restore_gpui_cursor(hwnd);
            return;
        }
        if crate::drag_cursor::is_drag_cursor_active_hwnd(key) {
            return;
        }

        if let Some(cursor) = cursor_handle(cursor) {
            unsafe {
                SetCursor(Some(cursor));
            }
        }
    }

    fn window_hwnd(window: &Window) -> Option<HWND> {
        match HasWindowHandle::window_handle(window).ok()?.as_raw() {
            RawWindowHandle::Win32(handle) => Some(HWND(handle.hwnd.get() as *mut c_void)),
            _ => None,
        }
    }

    fn ensure_window_hook(hwnd: HWND) -> bool {
        let key = hwnd.0 as isize;
        let Ok(mut hooks) = window_hooks().lock() else {
            return false;
        };
        if hooks.contains_key(&key) {
            return true;
        }

        let original_proc = unsafe {
            SetWindowLongPtrW(
                hwnd,
                GWLP_WNDPROC,
                window_proc as *const () as usize as isize,
            )
        };
        if original_proc == 0 {
            return false;
        }
        hooks.insert(
            key,
            WindowHook {
                original_proc,
                cursor: BrowserScrollCursor::Reset,
                owner: None,
            },
        );
        true
    }

    fn set_hook_state(hwnd: HWND, cursor: BrowserScrollCursor, owner: usize) -> bool {
        if cursor.is_active() && !ensure_window_hook(hwnd) {
            return false;
        }

        let cleared_cancellation = if cursor.is_active() {
            false
        } else {
            cancelled_owners()
                .lock()
                .map(|mut owners| owners.remove(&owner))
                .unwrap_or(false)
        };

        let key = hwnd.0 as isize;
        let Ok(mut hooks) = window_hooks().lock() else {
            return false;
        };
        let Some(hook) = hooks.get_mut(&key) else {
            // Resetting an unhooked window is already complete.
            return !cursor.is_active() && cleared_cancellation;
        };

        if !cursor.is_active() {
            if hook.cursor.is_active() && hook.owner != Some(owner) {
                return false;
            }
            let changed = hook.cursor.is_active() || cleared_cancellation;
            hook.cursor = BrowserScrollCursor::Reset;
            hook.owner = None;
            return changed;
        } else {
            hook.cursor = cursor;
            hook.owner = Some(owner);
        }
        true
    }

    fn set_global_hook_state(hwnd: HWND, cursor: BrowserScrollCursor) {
        let key = hwnd.0 as isize;
        if let Ok(mut hooks) = window_hooks().lock()
            && let Some(hook) = hooks.get_mut(&key)
        {
            if !cursor.is_active()
                && hook.cursor.is_active()
                && let Some(owner) = hook.owner
                && let Ok(mut owners) = cancelled_owners().lock()
            {
                owners.insert(owner);
            }
            hook.cursor = cursor;
            hook.owner = None;
        }
    }

    fn cursor_handle(cursor: BrowserScrollCursor) -> Option<HCURSOR> {
        let slot = match cursor {
            BrowserScrollCursor::Reset => return None,
            BrowserScrollCursor::DeadZone => &PAN_MIDDLE_VERTICAL_CURSOR,
            BrowserScrollCursor::Above => &PAN_NORTH_CURSOR,
            BrowserScrollCursor::Below => &PAN_SOUTH_CURSOR,
        };
        let encoded = match cursor {
            BrowserScrollCursor::Reset => return None,
            BrowserScrollCursor::DeadZone => PAN_MIDDLE_VERTICAL,
            BrowserScrollCursor::Above => PAN_NORTH,
            BrowserScrollCursor::Below => PAN_SOUTH,
        };
        let raw = *slot.get_or_init(|| {
            create_cursor(encoded)
                .map(|cursor| cursor.0 as usize)
                .unwrap_or_default()
        });
        (raw != 0).then_some(HCURSOR(raw as *mut c_void))
    }

    fn create_cursor(encoded: &str) -> Option<HCURSOR> {
        let bytes = STANDARD.decode(encoded.trim()).ok()?;
        let image = super::cursor_resource(&bytes)?;
        let icon = unsafe {
            CreateIconFromResourceEx(&image, false, 0x0003_0000, 0, 0, LR_DEFAULTSIZE).ok()?
        };
        Some(HCURSOR(icon.0))
    }

    unsafe extern "system" fn window_proc(
        hwnd: HWND,
        message: u32,
        wparam: WPARAM,
        lparam: LPARAM,
    ) -> LRESULT {
        let key = hwnd.0 as isize;
        let (original_proc, cursor) = {
            let Ok(hooks) = window_hooks().lock() else {
                return LRESULT(0);
            };
            let Some(hook) = hooks.get(&key).copied() else {
                return LRESULT(0);
            };
            (hook.original_proc, hook.cursor)
        };
        let original_proc: WNDPROC = Some(unsafe {
            std::mem::transmute::<
                isize,
                unsafe extern "system" fn(HWND, u32, WPARAM, LPARAM) -> LRESULT,
            >(original_proc)
        });
        // The previous procedure may be the drag cursor bridge. Always call
        // it first so existing window behavior and cursor ownership survive.
        let result = unsafe { CallWindowProcW(original_proc, hwnd, message, wparam, lparam) };

        if matches!(message, WM_SETCURSOR | GPUI_CURSOR_STYLE_CHANGED)
            && cursor.is_active()
            && !crate::drag_cursor::is_drag_cursor_active_hwnd(key)
        {
            if let Some(cursor) = cursor_handle(cursor) {
                unsafe {
                    SetCursor(Some(cursor));
                }
            }
        } else if matches!(message, WM_KILLFOCUS | WM_CANCELMODE | WM_CAPTURECHANGED) {
            set_global_hook_state(hwnd, BrowserScrollCursor::Reset);
        }

        if message == WM_NCDESTROY
            && let Ok(mut hooks) = window_hooks().lock()
        {
            hooks.remove(&key);
        }

        result
    }
}

#[cfg(test)]
mod tests {
    use base64::{Engine as _, engine::general_purpose::STANDARD};
    use gpui::CursorStyle;

    use super::{
        BrowserScrollCursor, PAN_MIDDLE_VERTICAL, PAN_NORTH, PAN_SOUTH, cursor_resource, le_u16,
        le_u32,
    };
    use crate::browser_scroll::AUTOSCROLL_DEAD_ZONE_PX;

    #[test]
    fn cursor_state_respects_the_dead_zone_boundaries() {
        let anchor = 100.;
        let dead_zone = AUTOSCROLL_DEAD_ZONE_PX;

        assert_eq!(
            BrowserScrollCursor::from_position(anchor, anchor, dead_zone),
            BrowserScrollCursor::DeadZone
        );
        assert_eq!(
            BrowserScrollCursor::from_position(anchor - dead_zone, anchor, dead_zone),
            BrowserScrollCursor::DeadZone
        );
        assert_eq!(
            BrowserScrollCursor::from_position(anchor + dead_zone, anchor, dead_zone),
            BrowserScrollCursor::DeadZone
        );
        assert_eq!(
            BrowserScrollCursor::from_position(anchor - dead_zone - 1., anchor, dead_zone),
            BrowserScrollCursor::Above
        );
        assert_eq!(
            BrowserScrollCursor::from_position(anchor + dead_zone + 1., anchor, dead_zone),
            BrowserScrollCursor::Below
        );
    }

    #[test]
    fn cursor_states_map_to_directional_gpui_fallbacks() {
        assert_eq!(
            BrowserScrollCursor::DeadZone.fallback_style(),
            Some(CursorStyle::ResizeUpDown)
        );
        assert_eq!(
            BrowserScrollCursor::Above.fallback_style(),
            Some(CursorStyle::ResizeUp)
        );
        assert_eq!(
            BrowserScrollCursor::Below.fallback_style(),
            Some(CursorStyle::ResizeDown)
        );
        assert_eq!(BrowserScrollCursor::Reset.fallback_style(), None);
        assert_eq!(
            BrowserScrollCursor::from_gpui_style(CursorStyle::ResizeUpDown),
            BrowserScrollCursor::DeadZone
        );
    }

    #[test]
    fn embedded_chromium_pan_cursors_have_valid_cur_resources() {
        for encoded in [PAN_MIDDLE_VERTICAL, PAN_NORTH, PAN_SOUTH] {
            let bytes = STANDARD.decode(encoded.trim()).unwrap();
            assert_eq!(le_u16(&bytes, 0), Some(0));
            assert_eq!(le_u16(&bytes, 2), Some(2));
            assert_eq!(le_u16(&bytes, 4), Some(1));
            assert_eq!(le_u16(&bytes, 10), Some(16));
            assert_eq!(le_u16(&bytes, 12), Some(16));

            let resource = cursor_resource(&bytes).unwrap();
            assert_eq!(le_u16(&resource, 0), Some(16));
            assert_eq!(le_u16(&resource, 2), Some(16));
            assert_eq!(le_u32(&resource, 4), Some(40));
            assert_eq!(le_u16(&resource, 18), Some(32));
            assert!(resource.len() > 4);
        }
    }

    #[test]
    fn malformed_cursor_resources_are_rejected() {
        assert!(cursor_resource(&[]).is_none());
        assert!(cursor_resource(&[0, 0, 2, 0, 1, 0]).is_none());
    }
}
