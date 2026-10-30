//! Input capture: low-level hooks see keys and buttons and can swallow them; raw input gives
//! the mouse's movement, which keeps coming when the cursor is held against a screen edge.
//!
//! The hooks and the window that receives raw input live on one thread of their own, which runs
//! a message loop. They report to a channel, and `WinSource` reads from it.

use std::ffi::c_void;
use std::mem::{size_of, zeroed};
use std::ptr::{null, null_mut};
use std::sync::atomic::{AtomicBool, AtomicI32, AtomicU32, AtomicU64, Ordering};
use std::sync::mpsc::{self, Receiver, RecvTimeoutError, Sender};
use std::sync::{Mutex, OnceLock};
use std::thread::{self, JoinHandle};
use std::time::{Duration, Instant};

use connected_core::platform::{PlatformError, Result};
use connected_core::{InputEvent, InputSource};
use connected_proto::{ButtonState, MouseButton};
use windows_sys::Win32::Foundation::{HWND, LPARAM, LRESULT, POINT, WPARAM};
use windows_sys::Win32::System::LibraryLoader::GetModuleHandleW;
use windows_sys::Win32::System::SystemInformation::GetTickCount;
use windows_sys::Win32::System::Threading::GetCurrentThreadId;
use windows_sys::Win32::UI::Input::{
    GetRawInputData, HRAWINPUT, MOUSE_MOVE_ABSOLUTE, RAWINPUT, RAWINPUTDEVICE, RAWINPUTHEADER,
    RID_INPUT, RIDEV_INPUTSINK, RIM_TYPEMOUSE, RegisterRawInputDevices,
};
use windows_sys::Win32::UI::WindowsAndMessaging::{
    CallNextHookEx, CreateWindowExW, DefWindowProcW, DestroyWindow, DispatchMessageW, GetMessageW,
    CreateCursor, GetCursorPos, GetSystemMetrics, HHOOK, KBDLLHOOKSTRUCT, LLKHF_EXTENDED, LLKHF_INJECTED, LLMHF_INJECTED, MSG,
    MSLLHOOKSTRUCT, PostThreadMessageW, RegisterClassW, SM_CXSCREEN, SM_CYSCREEN, SPI_SETCURSORS, SetCursorPos, SetSystemCursor, SetTimer, SetWindowsHookExW, SystemParametersInfoW, TranslateMessage,
    UnhookWindowsHookEx, WH_KEYBOARD_LL, WH_MOUSE_LL, WM_INPUT, WM_KEYDOWN, WM_KEYUP,
    WM_LBUTTONDOWN, WM_LBUTTONUP, WM_MBUTTONDOWN, WM_MBUTTONUP, WM_MOUSEHWHEEL, WM_MOUSEMOVE,
    WM_MOUSEWHEEL, WM_QUIT, WM_RBUTTONDOWN, WM_TIMER, WM_RBUTTONUP, WM_SYSKEYDOWN, WM_SYSKEYUP,
    WM_XBUTTONDOWN, WM_XBUTTONUP, WNDCLASSW, XBUTTON1,
};

use crate::winkeys;

/// How long opening the capture waits for the hook thread to say it is ready.
const READY_TIMEOUT: Duration = Duration::from_secs(5);

/// What the hook functions, which Windows calls with no way to pass anything in, share with
/// the rest of the program. There is one capture at a time.
struct Shared {
    events: Sender<InputEvent>,
    /// While set, mouse buttons, keys and cursor movement are swallowed: they reach
    /// `next_event_timeout` but not the programs on this computer.
    capture: bool,
}

static SHARED: Mutex<Option<Shared>> = Mutex::new(None);

/// When raw input last delivered mouse movement, in milliseconds since the first call (0 for
/// never). While it is recent, it is what reports movement. When it is not, the hook's own
/// positions stand in for it: they miss a push against a screen edge, but moving still works.
static LAST_RAW_MS: AtomicU64 = AtomicU64::new(0);
static RAW_MOVES: AtomicU64 = AtomicU64::new(0);
static HOOK_MOVES: AtomicU64 = AtomicU64::new(0);
/// Movement this soon after capture begins is not counted: the clock hooks use ticks coarsely.
const CAPTURE_SETTLE_MS: u32 = 20;
/// How long after raw input's last movement the hook's positions are still left out.
const RAW_RECENT_MS: u64 = 150;

fn now_ms() -> u64 {
    static START: OnceLock<Instant> = OnceLock::new();
    START.get_or_init(Instant::now).elapsed().as_millis() as u64 + 1
}

fn raw_is_recent() -> bool {
    let last = LAST_RAW_MS.load(Ordering::Relaxed);
    last != 0 && now_ms().saturating_sub(last) < RAW_RECENT_MS
}
/// The time (in the clock hooks stamp their events with) before which movement is not counted
/// while capturing. Events already on their way when capture began carry positions from before
/// the cursor was moved to the anchor, and would count as a huge jump.
static CAPTURE_SINCE: AtomicU32 = AtomicU32::new(0);
/// Whether captured movement is being counted against the anchor (as opposed to only blocked).
static ANCHORED: AtomicBool = AtomicBool::new(false);
static LAST_HOOK_X: AtomicI32 = AtomicI32::new(i32::MIN);
static LAST_HOOK_Y: AtomicI32 = AtomicI32::new(i32::MIN);

fn with_shared<R>(action: impl FnOnce(&mut Shared) -> R) -> Option<R> {
    let mut guard = SHARED.lock().ok()?;
    guard.as_mut().map(action)
}

/// Reports an event and says whether it should be swallowed.
fn report(event: InputEvent) -> bool {
    with_shared(|shared| {
        let _ = shared.events.send(event);
        shared.capture
    })
    .unwrap_or(false)
}

pub struct WinSource {
    events: Receiver<InputEvent>,
    thread_id: u32,
    thread: Option<JoinHandle<()>>,
}

impl WinSource {
    pub fn open() -> Result<Self> {
        // A cursor left hidden by a run that was cut short is put back.
        show_cursor();
        let (events, receiver) = mpsc::channel();
        {
            let mut shared = SHARED.lock().map_err(|_| PlatformError::Other("capture state is poisoned".into()))?;
            if shared.is_some() {
                return Err(PlatformError::Other("input is already being captured".into()));
            }
            *shared = Some(Shared { events, capture: false });
        }

        let (ready_tx, ready_rx) = mpsc::channel();
        let thread = thread::Builder::new()
            .name("connected-windows-hooks".into())
            .spawn(move || hook_thread(&ready_tx))
            .map_err(|err| PlatformError::Other(format!("cannot start the capture thread: {err}")))?;

        match ready_rx.recv_timeout(READY_TIMEOUT) {
            Ok(Ok(thread_id)) => Ok(Self { events: receiver, thread_id, thread: Some(thread) }),
            Ok(Err(message)) => {
                let _ = thread.join();
                clear_shared();
                Err(PlatformError::Other(message))
            }
            Err(_) => {
                clear_shared();
                Err(PlatformError::Other("the capture thread did not start".into()))
            }
        }
    }
}

fn clear_shared() {
    if let Ok(mut shared) = SHARED.lock() {
        *shared = None;
    }
}

impl InputSource for WinSource {
    fn next_event_timeout(&mut self, timeout: Duration) -> Result<Option<InputEvent>> {
        match self.events.recv_timeout(timeout) {
            Ok(event) => Ok(Some(event)),
            Err(RecvTimeoutError::Timeout) => Ok(None),
            Err(RecvTimeoutError::Disconnected) => Err(PlatformError::Other("input capture stopped".into())),
        }
    }

    fn describe(&self) -> String {
        format!(
            "mouse movement: {} from raw input, {} from hook positions",
            RAW_MOVES.load(Ordering::Relaxed),
            HOOK_MOVES.load(Ordering::Relaxed)
        )
    }

    fn set_blocking(&mut self, block: bool) -> Result<()> {
        ANCHORED.store(false, Ordering::Relaxed);
        if !block {
            LAST_HOOK_X.store(i32::MIN, Ordering::Relaxed);
        }
        with_shared(|shared| shared.capture = block)
            .ok_or_else(|| PlatformError::Other("input capture stopped".into()))
    }

    fn set_capture(&mut self, capture: bool) -> Result<()> {
        if capture {
            // SAFETY: plain calls with no pointers.
            unsafe {
                let (x, y) = (GetSystemMetrics(SM_CXSCREEN) / 2, GetSystemMetrics(SM_CYSCREEN) / 2);
                SetCursorPos(x, y);
            }
            // SAFETY: an argument-free query.
            CAPTURE_SINCE.store(unsafe { GetTickCount() }.wrapping_add(CAPTURE_SETTLE_MS), Ordering::Relaxed);
            ANCHORED.store(true, Ordering::Relaxed);
            hide_cursor();
        } else {
            ANCHORED.store(false, Ordering::Relaxed);
            show_cursor();
            // The positions seen before and after capturing are not a movement.
            LAST_HOOK_X.store(i32::MIN, Ordering::Relaxed);
        }
        with_shared(|shared| shared.capture = capture)
            .ok_or_else(|| PlatformError::Other("input capture stopped".into()))
    }
}

impl Drop for WinSource {
    fn drop(&mut self) {
        // Let go of everything first, then end the message loop of the hook thread.
        let _ = with_shared(|shared| shared.capture = false);
        show_cursor();
        // SAFETY: a plain call; if the thread is already gone it simply fails.
        unsafe {
            PostThreadMessageW(self.thread_id, WM_QUIT, 0, 0);
        }
        if let Some(thread) = self.thread.take() {
            let _ = thread.join();
        }
        clear_shared();
    }
}

// ---- hiding the cursor ----

/// The standard cursors (`OCR_*`): the arrow, text, busy, and so on.
const SYSTEM_CURSORS: [u32; 13] = [
    32512, 32513, 32514, 32515, 32516, 32642, 32643, 32644, 32645, 32646, 32648, 32649, 32650,
];

/// Replaces every standard cursor with an invisible one. Windows has no call to hide the cursor
/// for the whole desktop, only for the windows of the calling thread.
fn hide_cursor() {
    let module = unsafe { GetModuleHandleW(null()) };
    // All bits set in the AND plane and none in the XOR plane leave the screen as it is.
    let and_plane = [0xFFu8; 128];
    let xor_plane = [0u8; 128];
    for id in SYSTEM_CURSORS {
        // SAFETY: the planes are 32x32 bits, as the size passed says. Windows takes the handle
        // over, so a new one is made for each cursor.
        unsafe {
            let blank = CreateCursor(module, 0, 0, 32, 32, and_plane.as_ptr().cast(), xor_plane.as_ptr().cast());
            if !blank.is_null() {
                SetSystemCursor(blank, id);
            }
        }
    }
}

/// Puts the user's cursors back, from the settings they chose.
fn show_cursor() {
    // SAFETY: a plain call; the pointer argument is unused for this action.
    unsafe {
        SystemParametersInfoW(SPI_SETCURSORS, 0, null_mut(), 0);
    }
}

// ---- the hook thread ----

/// Installs the hooks and the raw input window, tells `ready` the thread's id, and runs the
/// message loop until `WM_QUIT`.
fn hook_thread(ready: &Sender<std::result::Result<u32, String>>) {
    // SAFETY: Win32 calls made in the order and with the arguments the API describes; every
    // handle created here is released before the thread ends.
    unsafe {
        let module = GetModuleHandleW(null());
        let keyboard = SetWindowsHookExW(WH_KEYBOARD_LL, Some(keyboard_proc), module, 0);
        let mouse = SetWindowsHookExW(WH_MOUSE_LL, Some(mouse_proc), module, 0);
        if keyboard.is_null() || mouse.is_null() {
            unhook(keyboard);
            unhook(mouse);
            let _ = ready.send(Err("Windows would not install the input hooks".into()));
            return;
        }

        let window = match create_raw_input_window(module) {
            Ok(window) => window,
            Err(message) => {
                unhook(keyboard);
                unhook(mouse);
                let _ = ready.send(Err(message));
                return;
            }
        };

        let _ = ready.send(Ok(GetCurrentThreadId()));

        let mut message: MSG = zeroed();
        while GetMessageW(&mut message, null_mut(), 0, 0) > 0 {
            TranslateMessage(&message);
            DispatchMessageW(&message);
        }

        unhook(keyboard);
        unhook(mouse);
        DestroyWindow(window);
    }
}

/// # Safety
/// `hook` is null or a hook from `SetWindowsHookExW` that is not yet removed.
unsafe fn unhook(hook: HHOOK) {
    if !hook.is_null() {
        // SAFETY: as the caller promises.
        unsafe {
            UnhookWindowsHookEx(hook);
        }
    }
}

/// A window that is never shown and only receives raw mouse input, even when another
/// program is in front. It is a top-level window rather than a message-only one, because
/// Windows does not reliably deliver raw input to those.
///
/// # Safety
/// `module` is this program's module handle.
unsafe fn create_raw_input_window(
    module: windows_sys::Win32::Foundation::HINSTANCE,
) -> std::result::Result<HWND, String> {
    let class_name: Vec<u16> = "ConnectedRawInput".encode_utf16().chain(Some(0)).collect();
    let class = WNDCLASSW {
        style: 0,
        lpfnWndProc: Some(window_proc),
        cbClsExtra: 0,
        cbWndExtra: 0,
        hInstance: module,
        hIcon: null_mut(),
        hCursor: null_mut(),
        hbrBackground: null_mut(),
        lpszMenuName: null(),
        lpszClassName: class_name.as_ptr(),
    };
    // SAFETY: `class` and the name it points to are valid for these calls. Registering a
    // class that already exists fails, which is fine: the window can still be made with it.
    unsafe {
        RegisterClassW(&class);
        let window = CreateWindowExW(
            0,
            class_name.as_ptr(),
            class_name.as_ptr(),
            0,
            0,
            0,
            0,
            0,
            null_mut(),
            null_mut(),
            module,
            null(),
        );
        if window.is_null() {
            return Err("Windows would not create the window that receives mouse movement".into());
        }
        if !register_raw_mouse(window) {
            DestroyWindow(window);
            return Err("Windows would not deliver raw mouse input".into());
        }
        // A process has one raw mouse registration, and a window toolkit in the same process
        // (the app's own window) takes it over or removes it as its window gains and loses focus.
        // Registering again every so often takes it back.
        SetTimer(window, REREGISTER_TIMER, REREGISTER_EVERY_MS, None);
        Ok(window)
    }
}

const REREGISTER_TIMER: usize = 1;
const REREGISTER_EVERY_MS: u32 = 250;

/// Asks for raw mouse input to be sent to `window`, even while another program is in front.
///
/// # Safety
/// `window` is a window of the calling thread.
unsafe fn register_raw_mouse(window: HWND) -> bool {
    let device = RAWINPUTDEVICE {
        usUsagePage: 0x01, // generic desktop
        usUsage: 0x02,     // mouse
        dwFlags: RIDEV_INPUTSINK,
        hwndTarget: window,
    };
    // SAFETY: one valid device description of the size passed.
    unsafe { RegisterRawInputDevices(&device, 1, size_of::<RAWINPUTDEVICE>() as u32) != 0 }
}

unsafe extern "system" fn window_proc(window: HWND, message: u32, wparam: WPARAM, lparam: LPARAM) -> LRESULT {
    if message == WM_INPUT {
        // SAFETY: for `WM_INPUT`, `lparam` is the handle of raw input data.
        unsafe { read_raw_input(lparam as HRAWINPUT) };
    } else if message == WM_TIMER && wparam == REREGISTER_TIMER {
        // SAFETY: this window belongs to the thread that runs this procedure.
        unsafe { register_raw_mouse(window) };
    }
    // SAFETY: the arguments are those Windows passed in.
    unsafe { DefWindowProcW(window, message, wparam, lparam) }
}

/// Turns raw mouse movement into an event. Windows keeps it coming when the cursor is stopped
/// at a screen edge, which the position the hooks report would not.
///
/// # Safety
/// `handle` is the handle from a `WM_INPUT` message.
unsafe fn read_raw_input(handle: HRAWINPUT) {
    // SAFETY: RAWINPUT is plain data, and the size and header size passed are those of the types.
    unsafe {
        let mut raw: RAWINPUT = zeroed();
        let mut size = size_of::<RAWINPUT>() as u32;
        let read = GetRawInputData(
            handle,
            RID_INPUT,
            &mut raw as *mut RAWINPUT as *mut c_void,
            &mut size,
            size_of::<RAWINPUTHEADER>() as u32,
        );
        if read == u32::MAX || raw.header.dwType != RIM_TYPEMOUSE {
            return;
        }
        // While capturing, movement is read from the hook against the anchor instead.
        if with_shared(|shared| shared.capture).unwrap_or(false) {
            return;
        }
        let mouse = raw.data.mouse;
        // Devices that report where they are, not how far they moved, are left to the hooks.
        if mouse.usFlags & MOUSE_MOVE_ABSOLUTE != 0 {
            return;
        }
        if mouse.lLastX != 0 || mouse.lLastY != 0 {
            LAST_RAW_MS.store(now_ms(), Ordering::Relaxed);
            RAW_MOVES.fetch_add(1, Ordering::Relaxed);
            report(InputEvent::MouseMove { dx: mouse.lLastX, dy: mouse.lLastY });
        }
    }
}

// ---- the hooks ----

unsafe extern "system" fn keyboard_proc(code: i32, wparam: WPARAM, lparam: LPARAM) -> LRESULT {
    if code >= 0 {
        // SAFETY: for the keyboard hook, `lparam` points to a KBDLLHOOKSTRUCT.
        let info = unsafe { &*(lparam as *const KBDLLHOOKSTRUCT) };
        // Keys this program sent itself are left alone.
        if info.flags & LLKHF_INJECTED == 0 {
            let state = match wparam as u32 {
                WM_KEYDOWN | WM_SYSKEYDOWN => Some(ButtonState::Down),
                WM_KEYUP | WM_SYSKEYUP => Some(ButtonState::Up),
                _ => None,
            };
            if let Some(state) = state {
                let extended = info.flags & LLKHF_EXTENDED != 0;
                let swallow = match winkeys::hid_from_scan(info.scanCode as u16, extended) {
                    Some(usage) => report(InputEvent::Key { usage, state }),
                    // A key with no usage cannot be sent on, but it must not leak either.
                    None => with_shared(|shared| shared.capture).unwrap_or(false),
                };
                if swallow {
                    return 1;
                }
            }
        }
    }
    // SAFETY: passing the arguments on, as a hook should.
    unsafe { CallNextHookEx(null_mut(), code, wparam, lparam) }
}

unsafe extern "system" fn mouse_proc(code: i32, wparam: WPARAM, lparam: LPARAM) -> LRESULT {
    if code >= 0 {
        // SAFETY: for the mouse hook, `lparam` points to a MSLLHOOKSTRUCT.
        let info = unsafe { &*(lparam as *const MSLLHOOKSTRUCT) };
        if info.flags & LLMHF_INJECTED == 0 && mouse_event(wparam as u32, info) {
            return 1;
        }
    }
    // SAFETY: passing the arguments on, as a hook should.
    unsafe { CallNextHookEx(null_mut(), code, wparam, lparam) }
}

/// Reports a mouse message and says whether to swallow it. Movement is reported by raw input,
/// so here it is only swallowed, while capturing.
fn mouse_event(message: u32, info: &MSLLHOOKSTRUCT) -> bool {
    let button = |button, state| report(InputEvent::MouseButton { button, state });
    // The high word of `mouseData` says which extra button, or how far the wheel turned.
    let high = (info.mouseData >> 16) as u16;
    let extra = if high == XBUTTON1 { MouseButton::Back } else { MouseButton::Forward };
    let wheel = i32::from(high as i16);

    match message {
        WM_MOUSEMOVE => {
            let (x, y) = (info.pt.x, info.pt.y);
            if with_shared(|shared| shared.capture).unwrap_or(false) {
                if !ANCHORED.load(Ordering::Relaxed) {
                    return true;
                }
                // Still on its way from before capture began: its position is from before the
                // cursor was moved, and would count as a jump of the distance between them.
                if (info.time.wrapping_sub(CAPTURE_SINCE.load(Ordering::Relaxed)) as i32) < 0 {
                    return true;
                }
                // Swallowed movement leaves the cursor where it is, so the hook's position minus
                // the cursor's is how far the mouse moved. (Measuring from the anchor instead
                // would count any early drift away from it again with every movement.)
                let mut at = POINT { x, y };
                // SAFETY: `at` is valid for writes.
                unsafe { GetCursorPos(&mut at) };
                let (dx, dy) = (x - at.x, y - at.y);
                if dx == 0 && dy == 0 {
                    return true;
                }
                HOOK_MOVES.fetch_add(1, Ordering::Relaxed);
                return report(InputEvent::MouseMove { dx, dy });
            }
            let (last_x, last_y) = (LAST_HOOK_X.swap(x, Ordering::Relaxed), LAST_HOOK_Y.swap(y, Ordering::Relaxed));
            if !raw_is_recent() && last_x != i32::MIN && (x != last_x || y != last_y) {
                HOOK_MOVES.fetch_add(1, Ordering::Relaxed);
                report(InputEvent::MouseMove { dx: x - last_x, dy: y - last_y })
            } else {
                with_shared(|shared| shared.capture).unwrap_or(false)
            }
        }
        WM_LBUTTONDOWN => button(MouseButton::Left, ButtonState::Down),
        WM_LBUTTONUP => button(MouseButton::Left, ButtonState::Up),
        WM_RBUTTONDOWN => button(MouseButton::Right, ButtonState::Down),
        WM_RBUTTONUP => button(MouseButton::Right, ButtonState::Up),
        WM_MBUTTONDOWN => button(MouseButton::Middle, ButtonState::Down),
        WM_MBUTTONUP => button(MouseButton::Middle, ButtonState::Up),
        WM_XBUTTONDOWN => button(extra, ButtonState::Down),
        WM_XBUTTONUP => button(extra, ButtonState::Up),
        WM_MOUSEWHEEL => report(InputEvent::Wheel { dx: 0, dy: wheel }),
        WM_MOUSEHWHEEL => report(InputEvent::Wheel { dx: wheel, dy: 0 }),
        _ => false,
    }
}
