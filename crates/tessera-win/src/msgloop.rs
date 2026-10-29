//! Win32 message loop of the main thread and waking it from other threads.

use std::ffi::c_void;

use windows::Win32::Foundation::{HWND, LPARAM, LRESULT, WPARAM};
use windows::Win32::System::LibraryLoader::GetModuleHandleW;
use windows::Win32::UI::WindowsAndMessaging::{
    CreateWindowExW, DefWindowProcW, DestroyWindow, DispatchMessageW, GetMessageW, KillTimer, MSG,
    PBT_APMRESUMEAUTOMATIC, PostMessageW, PostQuitMessage, RegisterClassW, SPI_SETWORKAREA,
    SetTimer, TranslateMessage, WM_APP, WM_DISPLAYCHANGE, WM_DPICHANGED, WM_POWERBROADCAST,
    WM_SETTINGCHANGE, WM_TIMER, WNDCLASSW, WS_EX_TOOLWINDOW, WS_POPUP,
};
use windows::core::w;

use crate::events::{self, Event};

const WM_APP_WAKE: u32 = WM_APP + 1;

/// Invisible top-level window of the daemon.
///
/// - Target for [`Waker`]: a window instead of `PostThreadMessageW`, because
///   thread messages get lost in modal loops (e.g. an open tray menu).
/// - Receives broadcasts such as `WM_DISPLAYCHANGE` and `WM_SETTINGCHANGE` and
///   passes them on as [`Event`]. Hence not a message-only window: those
///   receive no broadcasts.
pub struct MessageWindow(HWND);

impl MessageWindow {
    pub fn new() -> anyhow::Result<Self> {
        let class = w!("Tessera.MessageWindow");
        // SAFETY: standard window class/creation; the window lives until Drop.
        unsafe {
            let instance = GetModuleHandleW(None)?;
            let wc = WNDCLASSW {
                lpfnWndProc: Some(wndproc),
                hInstance: instance.into(),
                lpszClassName: class,
                ..Default::default()
            };
            // Fails if the class is already registered - harmless.
            RegisterClassW(&wc);
            // Never shown; WS_EX_TOOLWINDOW keeps it out of Alt+Tab.
            let hwnd = CreateWindowExW(
                WS_EX_TOOLWINDOW,
                class,
                w!("Tessera"),
                WS_POPUP,
                0,
                0,
                0,
                0,
                None,
                None,
                Some(instance.into()),
                None,
            )?;
            Ok(Self(hwnd))
        }
    }

    pub fn waker(&self) -> Waker {
        Waker(self.0.0 as usize)
    }

    pub fn timers(&self) -> Timers {
        Timers(self.0.0 as usize)
    }
}

/// Timers on the main thread; expiry arrives as [`Event::Timer`].
/// Only use on the thread of the [`MessageWindow`].
#[derive(Debug, Clone, Copy)]
pub struct Timers(usize);

impl Timers {
    /// Starts (or replaces) timer `id` with interval `ms`.
    pub fn set(&self, id: usize, ms: u32) {
        // SAFETY: own window; without a callback, WM_TIMER goes to the window procedure.
        unsafe { SetTimer(Some(self.hwnd()), id, ms, None) };
    }

    pub fn kill(&self, id: usize) {
        // SAFETY: own window; unknown timers are ignored.
        unsafe {
            let _ = KillTimer(Some(self.hwnd()), id);
        }
    }

    fn hwnd(&self) -> HWND {
        HWND(self.0 as *mut c_void)
    }
}

impl Drop for MessageWindow {
    fn drop(&mut self) {
        // SAFETY: own window, only destroyed here.
        unsafe {
            let _ = DestroyWindow(self.0);
        }
    }
}

extern "system" fn wndproc(hwnd: HWND, msg: u32, wparam: WPARAM, lparam: LPARAM) -> LRESULT {
    // WM_APP_WAKE needs no handling: the message loop calls its callback
    // after every message.
    match msg {
        WM_DISPLAYCHANGE | WM_DPICHANGED => events::push(Event::DisplayChanged),
        WM_SETTINGCHANGE if wparam.0 as u32 == SPI_SETWORKAREA.0 => {
            events::push(Event::WorkAreaChanged)
        }
        WM_POWERBROADCAST if wparam.0 as u32 == PBT_APMRESUMEAUTOMATIC => {
            events::push(Event::Resumed)
        }
        WM_TIMER => {
            events::push(Event::Timer(wparam.0));
            return LRESULT(0);
        }
        _ => {}
    }
    // SAFETY: forwarding to the default procedure.
    unsafe { DefWindowProcW(hwnd, msg, wparam, lparam) }
}

/// Wakes the main thread's message loop from any thread.
#[derive(Debug, Clone, Copy)]
pub struct Waker(usize);

impl Waker {
    pub fn wake(&self) {
        // SAFETY: PostMessageW is thread-safe; if the window is already gone,
        // the call just fails.
        unsafe {
            let _ = PostMessageW(
                Some(HWND(self.0 as *mut c_void)),
                WM_APP_WAKE,
                WPARAM(0),
                LPARAM(0),
            );
        }
    }
}

/// Processes messages until [`quit`] is called. `after_message`
/// runs after every message (including wake-ups via [`Waker`]).
pub fn run(mut after_message: impl FnMut()) -> i32 {
    let mut msg = MSG::default();
    loop {
        // SAFETY: standard message loop on the thread that owns the windows.
        unsafe {
            if GetMessageW(&mut msg, None, 0, 0).0 <= 0 {
                return msg.wParam.0 as i32;
            }
            let _ = TranslateMessage(&msg);
            DispatchMessageW(&msg);
        }
        after_message();
    }
}

/// Ends [`run`] on the current thread.
pub fn quit(exit_code: i32) {
    // SAFETY: only puts WM_QUIT into the current thread's queue.
    unsafe { PostQuitMessage(exit_code) }
}
