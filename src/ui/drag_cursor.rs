use gpui::{CursorStyle, Window};

/// The native cursor state used while a drag is active.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum DragCursorState {
    /// A drag is active and the pointer is holding the surface.
    Grabbing,
    /// The drag ended or was cancelled.
    Reset,
}

/// Identifies the surface that currently owns the native drag cursor.
///
/// Normal hover cursors stay in GPUI's element tree. Ownership is only needed
/// for the window-wide closed hand while an active drag crosses element
/// boundaries.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub(crate) enum DragCursorOwner {
    Carousel(usize),
    Playlist,
    Queue,
}

impl DragCursorOwner {
    pub(crate) const fn carousel(id: usize) -> Self {
        Self::Carousel(id)
    }

    pub(crate) const fn queue() -> Self {
        Self::Queue
    }

    pub(crate) const fn playlist() -> Self {
        Self::Playlist
    }
}

/// Returns whether a reset request is allowed to clear the active owner.
///
/// An empty cursor slot is safe to reset, and an owner may clear itself. A
/// stale callback from another surface must leave the current drag untouched.
pub(crate) const fn can_reset_drag_cursor(
    active_owner: Option<DragCursorOwner>,
    requested_owner: DragCursorOwner,
) -> bool {
    match (active_owner, requested_owner) {
        (None, _)
        | (Some(DragCursorOwner::Playlist), DragCursorOwner::Playlist)
        | (Some(DragCursorOwner::Queue), DragCursorOwner::Queue) => true,
        (Some(DragCursorOwner::Carousel(active_id)), DragCursorOwner::Carousel(requested_id)) => {
            active_id == requested_id
        }
        _ => false,
    }
}

/// Cursor shown while a drag is active.
pub(crate) const fn grabbing_cursor() -> CursorStyle {
    CursorStyle::ClosedHand
}

/// Update the native drag cursor only when `owner` owns the active drag.
///
/// A late mouse-up or mouse-exit callback from another surface must not clear
/// a newer drag's closed hand. Resetting an owner also deliberately leaves the
/// native cursor alone; the next GPUI frame restores the element's
/// `cursor_pointer()` style without an intermediate arrow.
pub(crate) fn set_drag_cursor_owned(
    window: &Window,
    state: DragCursorState,
    owner: DragCursorOwner,
) {
    #[cfg(windows)]
    windows_impl::set_drag_cursor_owned(window, state, owner);

    #[cfg(not(windows))]
    let _ = (window, state, owner);
}

/// Whether the window-wide drag cursor currently owns the native cursor.
///
/// The browser autoscroll cursor is another window-level cursor bridge. It
/// uses this query before applying a pan cursor so a chained WNDPROC cannot
/// replace an active drag's closed hand.
#[cfg(all(windows, not(test)))]
pub(crate) fn is_drag_cursor_active_hwnd(hwnd: isize) -> bool {
    windows_impl::is_drag_cursor_active_hwnd(hwnd)
}

#[cfg(windows)]
mod windows_impl {
    use std::{
        collections::HashMap,
        ffi::c_void,
        sync::{Mutex, OnceLock},
    };

    use base64::{Engine as _, engine::general_purpose::STANDARD};
    use gpui::Window;
    use raw_window_handle::{HasWindowHandle, RawWindowHandle};
    use windows::Win32::{
        Foundation::{HWND, LPARAM, LRESULT, WPARAM},
        UI::WindowsAndMessaging::{
            CallWindowProcW, CreateIconFromResourceEx, GWLP_WNDPROC, HCURSOR, LR_DEFAULTSIZE,
            SetCursor, SetWindowLongPtrW, WM_CANCELMODE, WM_CAPTURECHANGED, WM_KILLFOCUS,
            WM_NCDESTROY, WM_SETCURSOR, WM_USER, WNDPROC,
        },
    };

    use super::{DragCursorOwner, DragCursorState};

    const GPUI_CURSOR_STYLE_CHANGED: u32 = WM_USER + 1;

    const CLOSED_HAND: &str = include_str!("../../assets/cursors/hand_grabbing.cur.b64");

    #[derive(Clone, Copy)]
    struct WindowHook {
        original_proc: isize,
        cursor: DragCursorState,
        owner: Option<DragCursorOwner>,
    }

    static WINDOW_HOOKS: OnceLock<Mutex<HashMap<isize, WindowHook>>> = OnceLock::new();
    static CLOSED_CURSOR: OnceLock<usize> = OnceLock::new();

    fn window_hooks() -> &'static Mutex<HashMap<isize, WindowHook>> {
        WINDOW_HOOKS.get_or_init(|| Mutex::new(HashMap::new()))
    }

    #[cfg(all(windows, not(test)))]
    pub(super) fn is_drag_cursor_active_hwnd(hwnd: isize) -> bool {
        window_hooks()
            .lock()
            .ok()
            .and_then(|hooks| {
                hooks
                    .get(&hwnd)
                    .map(|hook| hook.cursor == DragCursorState::Grabbing)
            })
            .unwrap_or(false)
    }

    pub(super) fn set_drag_cursor_owned(
        window: &Window,
        state: DragCursorState,
        owner: DragCursorOwner,
    ) {
        let should_set_cursor = window_hwnd(window)
            .map(|hwnd| set_owned_hook_state(hwnd, state, owner))
            .unwrap_or(true);
        if should_set_cursor {
            unsafe {
                set_native_cursor(state);
            }
        }
    }

    fn window_hwnd(window: &Window) -> Option<HWND> {
        match HasWindowHandle::window_handle(window).ok()?.as_raw() {
            RawWindowHandle::Win32(handle) => Some(HWND(handle.hwnd.get() as *mut c_void)),
            _ => None,
        }
    }

    fn ensure_window_hook(hwnd: HWND) {
        let key = hwnd.0 as isize;
        let Ok(mut hooks) = window_hooks().lock() else {
            return;
        };
        if hooks.contains_key(&key) {
            return;
        }

        let original_proc = unsafe {
            SetWindowLongPtrW(
                hwnd,
                GWLP_WNDPROC,
                window_proc as *const () as usize as isize,
            )
        };
        if original_proc != 0 {
            hooks.insert(
                key,
                WindowHook {
                    original_proc,
                    cursor: DragCursorState::Reset,
                    owner: None,
                },
            );
        }
    }

    fn set_global_hook_state(hwnd: HWND, cursor: DragCursorState) {
        if cursor != DragCursorState::Reset {
            ensure_window_hook(hwnd);
        }
        let key = hwnd.0 as isize;
        if let Ok(mut hooks) = window_hooks().lock()
            && let Some(hook) = hooks.get_mut(&key)
        {
            hook.cursor = cursor;
            hook.owner = None;
        }
    }

    fn set_owned_hook_state(hwnd: HWND, cursor: DragCursorState, owner: DragCursorOwner) -> bool {
        if cursor == DragCursorState::Grabbing {
            ensure_window_hook(hwnd);
        }

        let key = hwnd.0 as isize;
        let Ok(mut hooks) = window_hooks().lock() else {
            return true;
        };
        let Some(hook) = hooks.get_mut(&key) else {
            return false;
        };

        match cursor {
            DragCursorState::Grabbing => {
                hook.cursor = DragCursorState::Grabbing;
                hook.owner = Some(owner);
                true
            }
            DragCursorState::Reset => {
                if hook.cursor == DragCursorState::Grabbing
                    && !super::can_reset_drag_cursor(hook.owner, owner)
                {
                    return false;
                }
                hook.cursor = DragCursorState::Reset;
                hook.owner = None;
                true
            }
        }
    }

    unsafe fn set_native_cursor(state: DragCursorState) {
        match state {
            DragCursorState::Grabbing => {
                if let Some(cursor) = cursor_handle(state) {
                    unsafe {
                        SetCursor(Some(cursor));
                    }
                }
            }
            DragCursorState::Reset => {}
        }
    }

    fn cursor_handle(state: DragCursorState) -> Option<HCURSOR> {
        if state == DragCursorState::Reset {
            return None;
        }
        let raw = *CLOSED_CURSOR.get_or_init(|| {
            create_cursor(CLOSED_HAND)
                .map(|cursor| cursor.0 as usize)
                .unwrap_or_default()
        });
        (raw != 0).then_some(HCURSOR(raw as *mut c_void))
    }

    fn create_cursor(encoded: &str) -> Option<HCURSOR> {
        let bytes = STANDARD.decode(encoded.trim()).ok()?;
        let image = cursor_resource(&bytes)?;
        let icon = unsafe {
            CreateIconFromResourceEx(&image, false, 0x0003_0000, 0, 0, LR_DEFAULTSIZE).ok()?
        };
        Some(HCURSOR(icon.0))
    }

    fn cursor_resource(bytes: &[u8]) -> Option<Vec<u8>> {
        if le_u16(bytes, 0)? != 0 || le_u16(bytes, 2)? != 2 {
            return None;
        }
        let entry_count = usize::from(le_u16(bytes, 4)?);
        if entry_count == 0 {
            return None;
        }
        // Prefer the color image when a CUR file also carries a monochrome fallback.
        type CursorCandidate<'a> = (u16, u16, &'a [u8], (u16, usize));
        let mut best: Option<CursorCandidate<'_>> = None;
        for index in 0..entry_count {
            let entry = 6usize.checked_add(index.checked_mul(16)?)?;
            let hotspot_x = le_u16(bytes, entry + 4)?;
            let hotspot_y = le_u16(bytes, entry + 6)?;
            let image_size = usize::try_from(le_u32(bytes, entry + 8)?).ok()?;
            let image_offset = usize::try_from(le_u32(bytes, entry + 12)?).ok()?;
            let image = bytes.get(image_offset..image_offset.checked_add(image_size)?)?;
            let bit_depth = le_u16(image, 14).unwrap_or_default();
            let score = (bit_depth, image_size);
            if best.as_ref().map(|best| score > best.3).unwrap_or(true) {
                best = Some((hotspot_x, hotspot_y, image, score));
            }
        }
        let (hotspot_x, hotspot_y, image, _) = best?;
        let mut resource = Vec::with_capacity(image.len() + 4);
        resource.extend_from_slice(&hotspot_x.to_le_bytes());
        resource.extend_from_slice(&hotspot_y.to_le_bytes());
        resource.extend_from_slice(image);
        Some(resource)
    }

    fn le_u16(bytes: &[u8], offset: usize) -> Option<u16> {
        Some(u16::from_le_bytes(
            bytes.get(offset..offset + 2)?.try_into().ok()?,
        ))
    }

    fn le_u32(bytes: &[u8], offset: usize) -> Option<u32> {
        Some(u32::from_le_bytes(
            bytes.get(offset..offset + 4)?.try_into().ok()?,
        ))
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
        let result = unsafe { CallWindowProcW(original_proc, hwnd, message, wparam, lparam) };

        if matches!(message, WM_SETCURSOR | GPUI_CURSOR_STYLE_CHANGED)
            && cursor == DragCursorState::Grabbing
        {
            if let Some(cursor) = cursor_handle(cursor) {
                unsafe {
                    SetCursor(Some(cursor));
                }
            }
        } else if matches!(message, WM_KILLFOCUS | WM_CANCELMODE | WM_CAPTURECHANGED) {
            set_global_hook_state(hwnd, DragCursorState::Reset);
        }

        if message == WM_NCDESTROY
            && let Ok(mut hooks) = window_hooks().lock()
        {
            hooks.remove(&key);
        }

        result
    }

    #[cfg(test)]
    mod cursor_tests {
        use base64::Engine as _;

        use super::{CLOSED_HAND, STANDARD, cursor_resource, le_u16};

        #[test]
        fn embedded_closed_cursor_has_a_valid_cursor_resource() {
            let bytes = STANDARD.decode(CLOSED_HAND.trim()).unwrap();
            let resource = cursor_resource(&bytes).unwrap();

            assert_eq!(le_u16(&bytes, 0), Some(0));
            assert_eq!(le_u16(&bytes, 2), Some(2));
            assert_eq!(le_u16(&bytes, 4), Some(2));
            assert_eq!(bytes.get(6), Some(&32));
            assert_eq!(bytes.get(7), Some(&32));
            assert_eq!(le_u16(&bytes, 10), Some(13));
            assert_eq!(le_u16(&bytes, 12), Some(13));
            assert_eq!(le_u16(&resource, 18), Some(24));
            assert!(resource.len() > 4);
        }
    }
}

#[cfg(test)]
mod tests {
    use super::{DragCursorOwner, DragCursorState, can_reset_drag_cursor, grabbing_cursor};
    use gpui::CursorStyle;

    #[test]
    fn drag_states_are_distinct_and_resettable() {
        assert_ne!(DragCursorState::Grabbing, DragCursorState::Reset);
    }

    #[test]
    fn supported_platforms_preserve_closed_hands() {
        assert_eq!(grabbing_cursor(), CursorStyle::ClosedHand);
    }

    #[test]
    fn stale_owner_resets_cannot_clear_a_new_drag() {
        let first_carousel = DragCursorOwner::carousel(1);
        let second_carousel = DragCursorOwner::carousel(2);

        assert!(can_reset_drag_cursor(None, first_carousel));
        assert!(can_reset_drag_cursor(Some(first_carousel), first_carousel));
        assert!(!can_reset_drag_cursor(
            Some(first_carousel),
            second_carousel
        ));
        assert!(!can_reset_drag_cursor(
            Some(DragCursorOwner::Queue),
            first_carousel
        ));
        assert!(can_reset_drag_cursor(
            Some(DragCursorOwner::Playlist),
            DragCursorOwner::playlist()
        ));
        assert!(!can_reset_drag_cursor(
            Some(DragCursorOwner::Playlist),
            DragCursorOwner::queue()
        ));
    }
}
