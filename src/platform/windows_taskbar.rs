use std::{
    cell::{Cell, RefCell},
    collections::{HashMap, hash_map::Entry},
    rc::Rc,
};

use futures::channel::mpsc::UnboundedSender;
use windows::{
    Win32::{
        Foundation::{HWND, LPARAM, LRESULT, RPC_E_CHANGED_MODE, WPARAM},
        Graphics::Gdi::{
            BI_RGB, BITMAPINFO, BITMAPINFOHEADER, CreateBitmap, CreateDIBSection, DIB_RGB_COLORS,
            DeleteObject,
        },
        System::{
            Com::{
                CLSCTX_INPROC_SERVER, COINIT_APARTMENTTHREADED, CoCreateInstance, CoInitializeEx,
                CoUninitialize,
            },
            LibraryLoader::GetModuleHandleW,
        },
        UI::{
            Input::KeyboardAndMouse::{
                MOD_NOREPEAT, RegisterHotKey, UnregisterHotKey, VK_MEDIA_NEXT_TRACK,
                VK_MEDIA_PLAY_PAUSE, VK_MEDIA_PREV_TRACK,
            },
            Shell::{
                ITaskbarList3, THB_FLAGS, THB_ICON, THB_TOOLTIP, THBF_DISABLED, THBF_ENABLED,
                THBN_CLICKED, THUMBBUTTON,
            },
            WindowsAndMessaging::{
                CallWindowProcW, CreateIconIndirect, DefWindowProcW, DestroyIcon, GWLP_WNDPROC,
                GetSystemMetrics, HICON, ICON_BIG, ICON_SMALL, ICONINFO, IMAGE_ICON,
                LR_DEFAULTCOLOR, LoadImageW, RegisterWindowMessageW, SM_CXICON, SM_CXSMICON,
                SM_CYICON, SM_CYSMICON, SendMessageW, SetWindowLongPtrW, WM_COMMAND, WM_HOTKEY,
                WM_NCDESTROY, WM_SETICON, WNDPROC,
            },
        },
    },
    core::{BOOL, GUID, IUnknown, PCWSTR, w},
};

use crate::media_control::MediaRequest;

const APP_ICON_RESOURCE_ID: u16 = 1;
const PREVIOUS_BUTTON_ID: u32 = 0xA101;
const TOGGLE_BUTTON_ID: u32 = 0xA102;
const NEXT_BUTTON_ID: u32 = 0xA103;
const MEDIA_HOTKEY_PREVIOUS_ID: i32 = 0xA201;
const MEDIA_HOTKEY_TOGGLE_ID: i32 = 0xA202;
const MEDIA_HOTKEY_NEXT_ID: i32 = 0xA203;
const TRANSPORT_SUPERSAMPLE: usize = 4;
const CLSID_TASKBAR_LIST: GUID = GUID::from_u128(0x56fdf344_fd6d_11d0_958a_006097c9a090);
const PENDING_OPERATION_BUDGET: usize = 8;

thread_local! {
    static CONTROLLERS: RefCell<HashMap<isize, Rc<Controller>>> = RefCell::new(HashMap::new());
}

#[derive(Clone, Copy, Debug, Default, Eq, PartialEq)]
pub(crate) struct TaskbarPlaybackState {
    pub(crate) has_track: bool,
    pub(crate) can_toggle: bool,
    pub(crate) playing: bool,
}

#[derive(Clone, Copy, Debug, Default, Eq, PartialEq)]
struct PendingTaskbarWork {
    add_buttons: bool,
    state: Option<TaskbarPlaybackState>,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
enum PendingTaskbarOperation {
    AddButtons,
    Update(TaskbarPlaybackState),
}

impl PendingTaskbarWork {
    fn queue_add_buttons(&mut self, destroyed: bool) -> bool {
        if destroyed {
            self.clear();
            false
        } else {
            self.add_buttons = true;
            true
        }
    }

    fn queue_state(&mut self, destroyed: bool, state: TaskbarPlaybackState) -> bool {
        if destroyed {
            self.clear();
            false
        } else {
            self.state = Some(state);
            true
        }
    }

    fn next_operation(&mut self) -> Option<PendingTaskbarOperation> {
        if self.add_buttons {
            self.add_buttons = false;
            return Some(PendingTaskbarOperation::AddButtons);
        }
        self.state.take().map(PendingTaskbarOperation::Update)
    }

    fn clear(&mut self) {
        *self = Self::default();
    }

    #[cfg(test)]
    fn is_empty(self) -> bool {
        !self.add_buttons && self.state.is_none()
    }
}

#[derive(Clone, Copy, Debug, Default, Eq, PartialEq)]
struct MediaHotkeyRegistration {
    attempted: bool,
    registered_mask: u8,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
enum MediaHotkeySync {
    None,
    Register,
    Unregister(u8),
}

impl MediaHotkeyRegistration {
    fn begin_sync(&mut self, has_track: bool) -> MediaHotkeySync {
        if has_track {
            if self.attempted {
                MediaHotkeySync::None
            } else {
                self.attempted = true;
                MediaHotkeySync::Register
            }
        } else {
            let had_state = self.attempted || self.registered_mask != 0;
            let registered_mask = self.registered_mask;
            *self = Self::default();
            if had_state {
                MediaHotkeySync::Unregister(registered_mask)
            } else {
                MediaHotkeySync::None
            }
        }
    }

    fn mark_registered(&mut self, bit: u8) {
        self.registered_mask |= bit;
    }
}

#[derive(Clone, Copy)]
struct MediaHotkeyDefinition {
    id: i32,
    virtual_key: u32,
    bit: u8,
    request: MediaRequest,
}

const MEDIA_HOTKEYS: [MediaHotkeyDefinition; 3] = [
    MediaHotkeyDefinition {
        id: MEDIA_HOTKEY_TOGGLE_ID,
        virtual_key: VK_MEDIA_PLAY_PAUSE.0 as u32,
        bit: 1 << 0,
        request: MediaRequest::Toggle,
    },
    MediaHotkeyDefinition {
        id: MEDIA_HOTKEY_PREVIOUS_ID,
        virtual_key: VK_MEDIA_PREV_TRACK.0 as u32,
        bit: 1 << 1,
        request: MediaRequest::Previous,
    },
    MediaHotkeyDefinition {
        id: MEDIA_HOTKEY_NEXT_ID,
        virtual_key: VK_MEDIA_NEXT_TRACK.0 as u32,
        bit: 1 << 2,
        request: MediaRequest::Next,
    },
];

pub(crate) struct TaskbarControls {
    hwnd: isize,
}

impl TaskbarControls {
    pub(crate) fn install(
        hwnd: isize,
        sender: UnboundedSender<MediaRequest>,
    ) -> Result<Self, String> {
        install(hwnd, sender)?;
        Ok(Self { hwnd })
    }

    pub(crate) fn update(&self, state: TaskbarPlaybackState) {
        update(self.hwnd, state);
    }
}

impl Drop for TaskbarControls {
    fn drop(&mut self) {
        uninstall(self.hwnd);
    }
}

struct Controller {
    hwnd: HWND,
    old_wnd_proc: isize,
    taskbar_created_message: u32,
    sender: UnboundedSender<MediaRequest>,
    taskbar: Option<ITaskbarList3>,
    icons: TaskbarIcons,
    state: Cell<TaskbarPlaybackState>,
    media_hotkeys: Cell<MediaHotkeyRegistration>,
    buttons_added: Cell<bool>,
    com_call_in_flight: Cell<bool>,
    destroyed: Cell<bool>,
    draining_pending: Cell<bool>,
    pending: Cell<PendingTaskbarWork>,
    uninitialize_com: bool,
}

type ControllerHandle = Rc<Controller>;

struct ComInitializationGuard {
    uninitialize: bool,
}

impl ComInitializationGuard {
    fn new(uninitialize: bool) -> Self {
        Self { uninitialize }
    }

    fn transfer(&mut self) -> bool {
        std::mem::take(&mut self.uninitialize)
    }
}

impl Drop for ComInitializationGuard {
    fn drop(&mut self) {
        if self.uninitialize {
            unsafe { CoUninitialize() };
        }
    }
}

impl Controller {
    fn buttons(&self) -> [THUMBBUTTON; 3] {
        let state = self.state.get();
        let transport_flags = if state.has_track {
            THBF_ENABLED
        } else {
            THBF_DISABLED
        };
        let toggle_flags = if state.can_toggle {
            THBF_ENABLED
        } else {
            THBF_DISABLED
        };
        [
            thumb_button(
                PREVIOUS_BUTTON_ID,
                self.icons.previous,
                "Previous",
                transport_flags,
            ),
            thumb_button(
                TOGGLE_BUTTON_ID,
                if state.playing {
                    self.icons.pause
                } else {
                    self.icons.play
                },
                if state.playing { "Pause" } else { "Play" },
                toggle_flags,
            ),
            thumb_button(NEXT_BUTTON_ID, self.icons.next, "Next", transport_flags),
        ]
    }

    fn add_buttons(&self) -> windows::core::Result<()> {
        if self.destroyed.get() {
            self.clear_pending();
            return Ok(());
        }
        if self.com_call_in_flight.get() || self.draining_pending.get() {
            self.queue_pending_add_buttons();
            return Ok(());
        }
        self.process_pending();
        if self.destroyed.get() {
            self.clear_pending();
            return Ok(());
        }
        let result = self.perform_add_buttons();
        self.process_pending();
        result
    }

    fn update_buttons(&self, state: TaskbarPlaybackState) {
        if self.destroyed.get() {
            self.clear_pending();
            return;
        }
        self.queue_pending_state(state);
        if self.com_call_in_flight.get() || self.draining_pending.get() {
            return;
        }
        self.process_pending();
    }

    fn sync_media_hotkeys(&self, has_track: bool) {
        let mut registration = self.media_hotkeys.get();
        let sync = registration.begin_sync(has_track);
        self.media_hotkeys.set(registration);
        match sync {
            MediaHotkeySync::None => {}
            MediaHotkeySync::Register => {
                for hotkey in MEDIA_HOTKEYS {
                    let result = unsafe {
                        RegisterHotKey(Some(self.hwnd), hotkey.id, MOD_NOREPEAT, hotkey.virtual_key)
                    };
                    if let Err(error) = result {
                        crate::diagnostics::event(
                            "DEBUG",
                            format!(
                                "media hotkey registration failed for id {}: {error}",
                                hotkey.id
                            ),
                        );
                    } else {
                        let mut registration = self.media_hotkeys.get();
                        registration.mark_registered(hotkey.bit);
                        self.media_hotkeys.set(registration);
                    }
                }
            }
            MediaHotkeySync::Unregister(mask) => {
                unregister_media_hotkeys(self.hwnd, mask);
            }
        }
    }

    fn release_media_hotkeys(&self) {
        let registration = self
            .media_hotkeys
            .replace(MediaHotkeyRegistration::default());
        unregister_media_hotkeys(self.hwnd, registration.registered_mask);
    }

    fn perform_add_buttons(&self) -> windows::core::Result<()> {
        if self.destroyed.get() {
            self.clear_pending();
            return Ok(());
        }
        if self.buttons_added.get() {
            return Ok(());
        }
        if self.com_call_in_flight.replace(true) {
            self.queue_pending_add_buttons();
            return Ok(());
        }
        let Some(taskbar) = self.taskbar.as_ref().cloned() else {
            self.com_call_in_flight.set(false);
            return Ok(());
        };
        let hwnd = self.hwnd;
        let buttons = self.buttons();
        let result = unsafe { taskbar.ThumbBarAddButtons(hwnd, &buttons) };
        drop(taskbar);
        self.com_call_in_flight.set(false);
        if self.destroyed.get() {
            self.clear_pending();
            return Ok(());
        }
        if result.is_ok() {
            self.buttons_added.set(true);
        }
        result
    }

    fn perform_update_buttons(&self) {
        if self.destroyed.get() {
            self.clear_pending();
            return;
        }
        if !self.buttons_added.get() {
            return;
        }
        if self.com_call_in_flight.replace(true) {
            self.queue_pending_state(self.state.get());
            return;
        }
        let Some(taskbar) = self.taskbar.as_ref().cloned() else {
            self.com_call_in_flight.set(false);
            return;
        };
        let hwnd = self.hwnd;
        let buttons = self.buttons();
        let result = unsafe { taskbar.ThumbBarUpdateButtons(hwnd, &buttons) };
        drop(taskbar);
        self.com_call_in_flight.set(false);
        if self.destroyed.get() {
            self.clear_pending();
            return;
        }
        if let Err(error) = result {
            crate::diagnostics::event(
                "DEBUG",
                format!("taskbar playback controls update failed: {error}"),
            );
        }
    }

    fn process_pending(&self) {
        if self.destroyed.get() {
            self.clear_pending();
            return;
        }
        if self.draining_pending.replace(true) {
            return;
        }
        for _ in 0..PENDING_OPERATION_BUDGET {
            if self.destroyed.get() {
                self.clear_pending();
                break;
            }
            let mut pending = self.pending.get();
            let Some(operation) = pending.next_operation() else {
                break;
            };
            self.pending.set(pending);
            match operation {
                PendingTaskbarOperation::AddButtons => {
                    if let Err(error) = self.perform_add_buttons() {
                        crate::diagnostics::event(
                            "DEBUG",
                            format!("taskbar playback controls creation failed: {error}"),
                        );
                    }
                }
                PendingTaskbarOperation::Update(state) => {
                    let changed = self.state.replace(state) != state;
                    self.sync_media_hotkeys(state.has_track);
                    if changed && self.buttons_added.get() {
                        self.perform_update_buttons();
                    }
                }
            }
        }
        if self.destroyed.get() {
            self.clear_pending();
        }
        self.draining_pending.set(false);
    }

    fn clear_pending(&self) {
        self.pending.set(PendingTaskbarWork::default());
    }

    fn queue_pending_add_buttons(&self) {
        let mut pending = self.pending.get();
        pending.queue_add_buttons(self.destroyed.get());
        self.pending.set(pending);
    }

    fn queue_pending_state(&self, state: TaskbarPlaybackState) {
        let mut pending = self.pending.get();
        pending.queue_state(self.destroyed.get(), state);
        self.pending.set(pending);
    }

    fn mark_destroyed(&self) {
        self.destroyed.set(true);
        self.release_media_hotkeys();
        self.clear_pending();
    }
}

impl Drop for Controller {
    fn drop(&mut self) {
        let registration = self
            .media_hotkeys
            .replace(MediaHotkeyRegistration::default());
        unregister_media_hotkeys(self.hwnd, registration.registered_mask);
        drop(self.taskbar.take());
        if self.uninitialize_com {
            unsafe { CoUninitialize() };
        }
    }
}

struct TaskbarIcons {
    app_big: HICON,
    app_small: HICON,
    previous: HICON,
    play: HICON,
    pause: HICON,
    next: HICON,
}

impl TaskbarIcons {
    fn load() -> Result<Self, String> {
        let big_width = unsafe { GetSystemMetrics(SM_CXICON) };
        let big_height = unsafe { GetSystemMetrics(SM_CYICON) };
        let small_width = unsafe { GetSystemMetrics(SM_CXSMICON) };
        let small_height = unsafe { GetSystemMetrics(SM_CYSMICON) };
        let transport_width = small_width.max(16) as usize;
        let transport_height = small_height.max(16) as usize;
        Ok(Self {
            app_big: load_app_icon(big_width, big_height)?,
            app_small: load_app_icon(small_width, small_height)?,
            previous: create_transport_icon(
                TransportGlyph::Previous,
                transport_width,
                transport_height,
            )?,
            play: create_transport_icon(TransportGlyph::Play, transport_width, transport_height)?,
            pause: create_transport_icon(TransportGlyph::Pause, transport_width, transport_height)?,
            next: create_transport_icon(TransportGlyph::Next, transport_width, transport_height)?,
        })
    }
}

impl Drop for TaskbarIcons {
    fn drop(&mut self) {
        for icon in [
            self.app_big,
            self.app_small,
            self.previous,
            self.play,
            self.pause,
            self.next,
        ] {
            let _ = unsafe { DestroyIcon(icon) };
        }
    }
}

#[derive(Clone, Copy)]
enum TransportGlyph {
    Previous,
    Play,
    Pause,
    Next,
}

fn install(hwnd: isize, sender: UnboundedSender<MediaRequest>) -> Result<(), String> {
    if hwnd == 0 {
        return Err("main window handle is null".into());
    }
    let key = hwnd;
    if CONTROLLERS.with(|controllers| controllers.borrow().contains_key(&key)) {
        return Err("taskbar playback controls are already installed for this window".into());
    }
    let hwnd = HWND(hwnd as *mut std::ffi::c_void);
    let com_result = unsafe { CoInitializeEx(None, COINIT_APARTMENTTHREADED) };
    if com_result.is_err() && com_result != RPC_E_CHANGED_MODE {
        return Err(format!(
            "could not initialize COM for taskbar controls: {com_result:?}"
        ));
    }
    let mut com_initialization = ComInitializationGuard::new(com_result.is_ok());
    let taskbar: ITaskbarList3 =
        unsafe { CoCreateInstance(&CLSID_TASKBAR_LIST, None::<&IUnknown>, CLSCTX_INPROC_SERVER) }
            .map_err(|error| format!("could not create Windows taskbar interface: {error}"))?;
    unsafe { taskbar.HrInit() }
        .map_err(|error| format!("could not initialize Windows taskbar interface: {error}"))?;
    let icons = TaskbarIcons::load()?;
    set_window_icons(hwnd, &icons);
    let taskbar_created_message = unsafe { RegisterWindowMessageW(w!("TaskbarButtonCreated")) };
    if taskbar_created_message == 0 {
        return Err("could not register the taskbar-created message".into());
    }
    let taskbar_wnd_proc_address = taskbar_wnd_proc as *const () as usize as isize;
    let old_wnd_proc = unsafe { SetWindowLongPtrW(hwnd, GWLP_WNDPROC, taskbar_wnd_proc_address) };
    if old_wnd_proc == 0 {
        return Err("could not attach taskbar playback message handling".into());
    }
    if old_wnd_proc == taskbar_wnd_proc_address {
        return Err("taskbar playback message handling is already attached".into());
    }
    let controller = Rc::new(Controller {
        hwnd,
        old_wnd_proc,
        taskbar_created_message,
        sender,
        taskbar: Some(taskbar),
        icons,
        state: Cell::new(TaskbarPlaybackState::default()),
        media_hotkeys: Cell::new(MediaHotkeyRegistration::default()),
        buttons_added: Cell::new(false),
        com_call_in_flight: Cell::new(false),
        destroyed: Cell::new(false),
        draining_pending: Cell::new(false),
        pending: Cell::new(PendingTaskbarWork::default()),
        uninitialize_com: com_initialization.transfer(),
    });
    let inserted = CONTROLLERS.with(|controllers| {
        let mut controllers = controllers.borrow_mut();
        match controllers.entry(key) {
            Entry::Vacant(entry) => {
                entry.insert(Rc::clone(&controller));
                true
            }
            Entry::Occupied(_) => false,
        }
    });
    if !inserted {
        return Err("taskbar playback controls became installed during setup".into());
    }
    if let Err(error) = controller.add_buttons() {
        crate::diagnostics::event(
            "DEBUG",
            format!("taskbar playback controls are waiting for taskbar creation: {error}"),
        );
    }
    Ok(())
}

fn uninstall(hwnd: isize) {
    let Some(controller) = controller_for(hwnd) else {
        return;
    };
    controller.mark_destroyed();
    unsafe {
        SetWindowLongPtrW(controller.hwnd, GWLP_WNDPROC, controller.old_wnd_proc);
    }
    let removed = CONTROLLERS.with(|controllers| controllers.borrow_mut().remove(&hwnd));
    drop(removed);
}

fn update(hwnd: isize, state: TaskbarPlaybackState) {
    if let Some(controller) = controller_for(hwnd) {
        controller.update_buttons(state);
    }
}

fn controller_for(hwnd: isize) -> Option<ControllerHandle> {
    CONTROLLERS.with(|controllers| controllers.borrow().get(&hwnd).cloned())
}

fn unregister_media_hotkeys(hwnd: HWND, registered_mask: u8) {
    for hotkey in MEDIA_HOTKEYS {
        if registered_mask & hotkey.bit == 0 {
            continue;
        }
        if let Err(error) = unsafe { UnregisterHotKey(Some(hwnd), hotkey.id) } {
            crate::diagnostics::event(
                "DEBUG",
                format!(
                    "media hotkey unregistration failed for id {}: {error}",
                    hotkey.id
                ),
            );
        }
    }
}

unsafe extern "system" fn taskbar_wnd_proc(
    hwnd: HWND,
    message: u32,
    wparam: WPARAM,
    lparam: LPARAM,
) -> LRESULT {
    let key = hwnd.0 as isize;
    let Some(controller) = controller_for(key) else {
        return unsafe { DefWindowProcW(hwnd, message, wparam, lparam) };
    };
    let old_wnd_proc = controller.old_wnd_proc;
    match taskbar_message_dispatch(message, wparam.0, controller.taskbar_created_message) {
        TaskbarMessageDispatch::Command(request) => {
            let sender = controller.sender.clone();
            let _ = sender.unbounded_send(request);
            return LRESULT(0);
        }
        TaskbarMessageDispatch::TaskbarCreated => {
            if !controller.destroyed.get() {
                controller.buttons_added.set(false);
                if let Err(error) = controller.add_buttons() {
                    crate::diagnostics::event(
                        "DEBUG",
                        format!("taskbar playback controls creation failed: {error}"),
                    );
                }
            }
        }
        TaskbarMessageDispatch::Destroyed => {
            controller.mark_destroyed();
            let removed = CONTROLLERS.with(|controllers| controllers.borrow_mut().remove(&key));
            drop(removed);
        }
        TaskbarMessageDispatch::Forward => {}
    }
    call_previous_wnd_proc(old_wnd_proc, hwnd, message, wparam, lparam)
}

#[derive(Debug, Eq, PartialEq)]
enum TaskbarMessageDispatch {
    Command(MediaRequest),
    TaskbarCreated,
    Destroyed,
    Forward,
}

fn taskbar_message_dispatch(
    message: u32,
    wparam: usize,
    taskbar_created_message: u32,
) -> TaskbarMessageDispatch {
    if message == WM_COMMAND
        && let Some(request) = media_request_for_command(wparam)
    {
        return TaskbarMessageDispatch::Command(request);
    }
    if message == WM_HOTKEY
        && let Some(request) = media_request_for_hotkey(wparam)
    {
        return TaskbarMessageDispatch::Command(request);
    }
    if message == taskbar_created_message {
        return TaskbarMessageDispatch::TaskbarCreated;
    }
    if message == WM_NCDESTROY {
        return TaskbarMessageDispatch::Destroyed;
    }
    TaskbarMessageDispatch::Forward
}

fn media_request_for_command(wparam: usize) -> Option<MediaRequest> {
    let notification = ((wparam >> 16) & 0xffff) as u32;
    if notification != THBN_CLICKED {
        return None;
    }
    match (wparam & 0xffff) as u32 {
        PREVIOUS_BUTTON_ID => Some(MediaRequest::Previous),
        TOGGLE_BUTTON_ID => Some(MediaRequest::Toggle),
        NEXT_BUTTON_ID => Some(MediaRequest::Next),
        _ => None,
    }
}

fn media_request_for_hotkey(wparam: usize) -> Option<MediaRequest> {
    MEDIA_HOTKEYS
        .iter()
        .find(|hotkey| wparam == hotkey.id as usize)
        .map(|hotkey| hotkey.request)
}

fn call_previous_wnd_proc(
    old_wnd_proc: isize,
    hwnd: HWND,
    message: u32,
    wparam: WPARAM,
    lparam: LPARAM,
) -> LRESULT {
    if old_wnd_proc == 0 {
        return unsafe { DefWindowProcW(hwnd, message, wparam, lparam) };
    }
    let previous: WNDPROC = unsafe { std::mem::transmute(old_wnd_proc) };
    unsafe { CallWindowProcW(previous, hwnd, message, wparam, lparam) }
}

fn set_window_icons(hwnd: HWND, icons: &TaskbarIcons) {
    unsafe {
        SendMessageW(
            hwnd,
            WM_SETICON,
            Some(WPARAM(ICON_BIG as usize)),
            Some(LPARAM(icons.app_big.0 as isize)),
        );
        SendMessageW(
            hwnd,
            WM_SETICON,
            Some(WPARAM(ICON_SMALL as usize)),
            Some(LPARAM(icons.app_small.0 as isize)),
        );
    }
}

fn load_app_icon(width: i32, height: i32) -> Result<HICON, String> {
    let module = unsafe { GetModuleHandleW(PCWSTR::null()) }
        .map_err(|error| format!("could not locate the application module: {error}"))?;
    let handle = unsafe {
        LoadImageW(
            Some(module.into()),
            PCWSTR::from_raw(APP_ICON_RESOURCE_ID as usize as *const u16),
            IMAGE_ICON,
            width,
            height,
            LR_DEFAULTCOLOR,
        )
    }
    .map_err(|error| format!("could not load the application icon resource: {error}"))?;
    Ok(HICON(handle.0))
}

fn thumb_button(
    id: u32,
    icon: HICON,
    tooltip: &str,
    flags: windows::Win32::UI::Shell::THUMBBUTTONFLAGS,
) -> THUMBBUTTON {
    THUMBBUTTON {
        dwMask: THB_ICON | THB_TOOLTIP | THB_FLAGS,
        iId: id,
        hIcon: icon,
        szTip: tooltip_text(tooltip),
        dwFlags: flags,
        ..Default::default()
    }
}

fn tooltip_text(value: &str) -> [u16; 260] {
    let mut output = [0u16; 260];
    for (slot, value) in output.iter_mut().take(259).zip(value.encode_utf16()) {
        *slot = value;
    }
    output
}

fn create_transport_icon(
    glyph: TransportGlyph,
    width: usize,
    height: usize,
) -> Result<HICON, String> {
    let bgra = transport_pixels(glyph, width, height);
    let bitmap_info = BITMAPINFO {
        bmiHeader: BITMAPINFOHEADER {
            biSize: std::mem::size_of::<BITMAPINFOHEADER>() as u32,
            biWidth: width as i32,
            biHeight: -(height as i32),
            biPlanes: 1,
            biBitCount: 32,
            biCompression: BI_RGB.0,
            ..Default::default()
        },
        ..Default::default()
    };
    let mut color_bits = std::ptr::null_mut();
    let color_bitmap =
        unsafe { CreateDIBSection(None, &bitmap_info, DIB_RGB_COLORS, &mut color_bits, None, 0) }
            .map_err(|error| format!("could not create taskbar icon bitmap: {error}"))?;
    unsafe {
        std::ptr::copy_nonoverlapping(bgra.as_ptr(), color_bits.cast::<u8>(), bgra.len());
    }

    let mask_stride = width.div_ceil(16) * 2;
    let mask_bits = vec![0u8; mask_stride * height];
    let mask_bitmap = unsafe {
        CreateBitmap(
            width as i32,
            height as i32,
            1,
            1,
            Some(mask_bits.as_ptr().cast()),
        )
    };
    if mask_bitmap.0.is_null() {
        unsafe {
            let _ = DeleteObject(color_bitmap.into());
        }
        return Err("could not create taskbar icon transparency mask".into());
    }

    let icon = unsafe {
        CreateIconIndirect(&ICONINFO {
            fIcon: BOOL(1),
            hbmMask: mask_bitmap,
            hbmColor: color_bitmap,
            ..Default::default()
        })
    }
    .map_err(|error| format!("could not create a taskbar playback icon: {error}"));
    unsafe {
        let _ = DeleteObject(mask_bitmap.into());
        let _ = DeleteObject(color_bitmap.into());
    }
    icon
}

fn transport_pixels(glyph: TransportGlyph, width: usize, height: usize) -> Vec<u8> {
    let mut pixels = vec![0u8; width * height * 4];
    let samples = TRANSPORT_SUPERSAMPLE * TRANSPORT_SUPERSAMPLE;
    for y in 0..height {
        for x in 0..width {
            let mut covered = 0usize;
            for sample_y in 0..TRANSPORT_SUPERSAMPLE {
                for sample_x in 0..TRANSPORT_SUPERSAMPLE {
                    let px = (x as f32 + (sample_x as f32 + 0.5) / TRANSPORT_SUPERSAMPLE as f32)
                        / width as f32;
                    let py = (y as f32 + (sample_y as f32 + 0.5) / TRANSPORT_SUPERSAMPLE as f32)
                        / height as f32;
                    covered += usize::from(transport_glyph_contains(glyph, px, py));
                }
            }
            let alpha = ((covered * 255) / samples) as u8;
            let offset = (y * width + x) * 4;
            pixels[offset..offset + 4].copy_from_slice(&[alpha, alpha, alpha, alpha]);
        }
    }
    pixels
}

fn transport_glyph_contains(glyph: TransportGlyph, x: f32, y: f32) -> bool {
    match glyph {
        TransportGlyph::Play => point_in_triangle(x, y, (0.31, 0.21), (0.76, 0.5), (0.31, 0.79)),
        TransportGlyph::Pause => {
            in_rect(x, y, 0.29, 0.22, 0.43, 0.78) || in_rect(x, y, 0.57, 0.22, 0.71, 0.78)
        }
        TransportGlyph::Previous => {
            in_rect(x, y, 0.24, 0.22, 0.34, 0.78)
                || point_in_triangle(x, y, (0.72, 0.21), (0.35, 0.5), (0.72, 0.79))
        }
        TransportGlyph::Next => {
            in_rect(x, y, 0.66, 0.22, 0.76, 0.78)
                || point_in_triangle(x, y, (0.28, 0.21), (0.65, 0.5), (0.28, 0.79))
        }
    }
}

fn in_rect(x: f32, y: f32, left: f32, top: f32, right: f32, bottom: f32) -> bool {
    x >= left && x <= right && y >= top && y <= bottom
}

fn point_in_triangle(
    x: f32,
    y: f32,
    first: (f32, f32),
    second: (f32, f32),
    third: (f32, f32),
) -> bool {
    let sign = |point: (f32, f32), a: (f32, f32), b: (f32, f32)| {
        (point.0 - b.0) * (a.1 - b.1) - (a.0 - b.0) * (point.1 - b.1)
    };
    let point = (x, y);
    let first_sign = sign(point, first, second);
    let second_sign = sign(point, second, third);
    let third_sign = sign(point, third, first);
    let has_negative = first_sign < 0.0 || second_sign < 0.0 || third_sign < 0.0;
    let has_positive = first_sign > 0.0 || second_sign > 0.0 || third_sign > 0.0;
    !(has_negative && has_positive)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn thumbnail_button_commands_map_to_media_requests() {
        let command = |id: u32| ((THBN_CLICKED as usize) << 16) | id as usize;
        assert_eq!(
            media_request_for_command(command(PREVIOUS_BUTTON_ID)),
            Some(MediaRequest::Previous)
        );
        assert_eq!(
            media_request_for_command(command(TOGGLE_BUTTON_ID)),
            Some(MediaRequest::Toggle)
        );
        assert_eq!(
            media_request_for_command(command(NEXT_BUTTON_ID)),
            Some(MediaRequest::Next)
        );
        assert_eq!(media_request_for_command(PREVIOUS_BUTTON_ID as usize), None);
    }

    #[test]
    fn taskbar_dispatch_is_decided_without_borrowing_the_controller_registry() {
        let command = |id: u32| ((THBN_CLICKED as usize) << 16) | id as usize;
        let taskbar_created_message = 0xC001;

        assert_eq!(
            taskbar_message_dispatch(
                WM_COMMAND,
                command(TOGGLE_BUTTON_ID),
                taskbar_created_message,
            ),
            TaskbarMessageDispatch::Command(MediaRequest::Toggle)
        );
        assert_eq!(
            taskbar_message_dispatch(taskbar_created_message, 0, taskbar_created_message),
            TaskbarMessageDispatch::TaskbarCreated
        );
        assert_eq!(
            taskbar_message_dispatch(WM_NCDESTROY, 0, taskbar_created_message),
            TaskbarMessageDispatch::Destroyed
        );
        assert_eq!(
            taskbar_message_dispatch(WM_COMMAND, 0, taskbar_created_message),
            TaskbarMessageDispatch::Forward
        );
    }

    #[test]
    fn media_hotkey_commands_dispatch_to_transport_requests() {
        let taskbar_created_message = 0xC001;
        assert_eq!(
            taskbar_message_dispatch(
                WM_HOTKEY,
                MEDIA_HOTKEY_TOGGLE_ID as usize,
                taskbar_created_message,
            ),
            TaskbarMessageDispatch::Command(MediaRequest::Toggle)
        );
        assert_eq!(
            taskbar_message_dispatch(
                WM_HOTKEY,
                MEDIA_HOTKEY_PREVIOUS_ID as usize,
                taskbar_created_message,
            ),
            TaskbarMessageDispatch::Command(MediaRequest::Previous)
        );
        assert_eq!(
            taskbar_message_dispatch(
                WM_HOTKEY,
                MEDIA_HOTKEY_NEXT_ID as usize,
                taskbar_created_message,
            ),
            TaskbarMessageDispatch::Command(MediaRequest::Next)
        );
        assert_eq!(
            taskbar_message_dispatch(WM_HOTKEY, 0xA204, taskbar_created_message),
            TaskbarMessageDispatch::Forward
        );
    }

    #[test]
    fn media_hotkey_registration_retries_only_after_track_state_resets() {
        let mut registration = MediaHotkeyRegistration::default();
        assert_eq!(registration.begin_sync(false), MediaHotkeySync::None);
        assert_eq!(registration.begin_sync(true), MediaHotkeySync::Register);
        registration.mark_registered(1 << 0);
        registration.mark_registered(1 << 2);
        assert_eq!(registration.begin_sync(true), MediaHotkeySync::None);
        assert_eq!(
            registration.begin_sync(false),
            MediaHotkeySync::Unregister((1 << 0) | (1 << 2))
        );
        assert_eq!(registration.begin_sync(false), MediaHotkeySync::None);
        assert_eq!(registration.begin_sync(true), MediaHotkeySync::Register);
    }

    #[test]
    fn media_hotkey_ids_are_in_the_app_range_and_do_not_overlap_buttons() {
        for hotkey in MEDIA_HOTKEYS {
            assert!((0..=0xBFFF).contains(&hotkey.id));
            assert_ne!(hotkey.id as u32, PREVIOUS_BUTTON_ID);
            assert_ne!(hotkey.id as u32, TOGGLE_BUTTON_ID);
            assert_ne!(hotkey.id as u32, NEXT_BUTTON_ID);
        }
    }

    #[test]
    fn pending_work_drain_is_bounded_without_recursive_dispatch() {
        let mut pending = PendingTaskbarWork::default();
        pending.queue_add_buttons(false);

        for _ in 0..PENDING_OPERATION_BUDGET {
            assert_eq!(
                pending.next_operation(),
                Some(PendingTaskbarOperation::AddButtons)
            );
            pending.queue_add_buttons(false);
        }

        assert!(!pending.is_empty());
        assert_eq!(
            pending.next_operation(),
            Some(PendingTaskbarOperation::AddButtons)
        );
    }

    #[test]
    fn destroyed_pending_work_is_cleared_and_cannot_be_rescheduled() {
        let mut pending = PendingTaskbarWork::default();
        let state = TaskbarPlaybackState {
            has_track: true,
            can_toggle: true,
            playing: false,
        };

        pending.queue_add_buttons(false);
        pending.queue_state(false, state);
        assert!(!pending.queue_add_buttons(true));
        assert!(pending.is_empty());
        assert!(!pending.queue_state(true, state));
        assert!(pending.is_empty());
    }

    #[test]
    fn pending_state_coalescing_keeps_the_latest_state() {
        let mut pending = PendingTaskbarWork::default();
        let first_state = TaskbarPlaybackState {
            has_track: true,
            can_toggle: true,
            playing: false,
        };
        let latest_state = TaskbarPlaybackState {
            has_track: true,
            can_toggle: true,
            playing: true,
        };

        pending.queue_state(false, first_state);
        pending.queue_state(false, latest_state);

        assert_eq!(
            pending.next_operation(),
            Some(PendingTaskbarOperation::Update(latest_state))
        );
        assert!(pending.is_empty());
    }

    #[test]
    fn newer_external_state_supersedes_retained_state_after_add_buttons() {
        let mut pending = PendingTaskbarWork::default();
        let retained_state = TaskbarPlaybackState {
            has_track: true,
            can_toggle: false,
            playing: false,
        };
        let external_state = TaskbarPlaybackState {
            has_track: true,
            can_toggle: true,
            playing: true,
        };

        pending.queue_state(false, retained_state);
        pending.queue_add_buttons(false);
        pending.queue_state(false, external_state);

        assert_eq!(
            pending.next_operation(),
            Some(PendingTaskbarOperation::AddButtons)
        );
        assert_eq!(
            pending.next_operation(),
            Some(PendingTaskbarOperation::Update(external_state))
        );
        assert!(pending.is_empty());
    }

    #[test]
    fn transport_glyphs_have_visible_pixels_and_distinct_play_states() {
        let opaque_count = |glyph| {
            transport_pixels(glyph, 16, 16)
                .chunks_exact(4)
                .filter(|pixel| pixel[3] != 0)
                .count()
        };
        assert!(opaque_count(TransportGlyph::Previous) > 0);
        assert!(opaque_count(TransportGlyph::Play) > 0);
        assert!(opaque_count(TransportGlyph::Pause) > 0);
        assert!(opaque_count(TransportGlyph::Next) > 0);
        assert_ne!(
            transport_pixels(TransportGlyph::Play, 16, 16),
            transport_pixels(TransportGlyph::Pause, 16, 16)
        );
    }

    #[test]
    fn transport_glyphs_have_antialiased_edges() {
        for glyph in [
            TransportGlyph::Previous,
            TransportGlyph::Play,
            TransportGlyph::Pause,
            TransportGlyph::Next,
        ] {
            assert!(
                transport_pixels(glyph, 16, 16)
                    .chunks_exact(4)
                    .any(|pixel| pixel[3] > 0 && pixel[3] < 255)
            );
        }
    }
}
