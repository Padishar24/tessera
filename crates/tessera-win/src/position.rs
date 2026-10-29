//! Moving and resizing windows, z-order and focus.

use std::mem::size_of;

use tessera_core::config::{Color, DragModifier};
use tessera_core::geometry::Rect;
use windows::Win32::Foundation::{LPARAM, POINT, RECT, WPARAM};
use windows::Win32::Graphics::Dwm::{
    DWMWA_BORDER_COLOR, DWMWA_COLOR_DEFAULT, DwmSetWindowAttribute,
};
use windows::Win32::UI::Input::KeyboardAndMouse::{
    GetAsyncKeyState, INPUT, INPUT_0, INPUT_MOUSE, MOUSEEVENTF_MOVE, MOUSEINPUT, SendInput,
    VK_CONTROL, VK_MENU, VK_SHIFT,
};
use windows::Win32::UI::WindowsAndMessaging::{
    GetCursorPos, GetForegroundWindow, GetWindowRect, PostMessageW, SC_RESTORE, SMTO_ABORTIFHUNG,
    SMTO_NORMAL, SW_MAXIMIZE, SW_MINIMIZE, SW_RESTORE, SWP_ASYNCWINDOWPOS, SWP_NOACTIVATE,
    SWP_NOMOVE, SWP_NOOWNERZORDER, SWP_NOSIZE, SWP_NOZORDER, SendMessageTimeoutW,
    SetForegroundWindow, SetWindowPos, ShowWindowAsync, WM_CLOSE, WM_SYSCOMMAND,
};

use crate::window::{Borders, Window};

/// Maximum time that restoring a maximized window may hold up the
/// daemon.
const RESTORE_TIMEOUT_MS: u32 = 200;

/// Sets the visible area of the window to `rect` (invisible borders
/// are compensated). Maximized windows are restored first.
/// The position change itself is asynchronous so that hung applications don't
/// block the daemon.
pub fn set_rect(window: Window, rect: Rect) {
    let borders = if window.is_maximized() {
        restore_for_measuring(window)
    } else {
        window.borders()
    };
    let Some(borders) = borders else {
        return;
    };
    let outer = borders.grow(rect);

    // SAFETY: asynchronous position change without activation.
    unsafe {
        let _ = SetWindowPos(
            window.0,
            None,
            outer.x,
            outer.y,
            outer.width,
            outer.height,
            SWP_NOACTIVATE | SWP_NOZORDER | SWP_NOOWNERZORDER | SWP_ASYNCWINDOWPOS,
        );
    }
}

/// Restores a maximized window and returns its invisible borders in the
/// normal state.
///
/// The borders can only be measured on the restored window:
/// maximized, it extends beyond the work area on all four sides; in the
/// normal state, not at all at the top. With the maximized borders the
/// window would end up too high and too large by that width. Hence the
/// restore here is synchronous - but not with `ShowWindow`, which would block
/// the daemon indefinitely for a hung application, but via
/// `WM_SYSCOMMAND`/`SC_RESTORE` through `SendMessageTimeoutW`: if the call
/// returns in time, the application has restored the window.
///
/// If the time runs out (the message then stays in the queue and takes
/// effect later), the window is considered hung (then nothing is sent at
/// all) or the application ignores the message, the restore is additionally
/// triggered asynchronously and the borders are estimated from the
/// maximized state. If the window is in a workspace, the window manager
/// corrects the position as soon as the restore arrives.
fn restore_for_measuring(window: Window) -> Option<Borders> {
    // SAFETY: message with timeout; no result buffer.
    unsafe {
        SendMessageTimeoutW(
            window.0,
            WM_SYSCOMMAND,
            WPARAM(SC_RESTORE as usize),
            LPARAM(0),
            SMTO_NORMAL | SMTO_ABORTIFHUNG,
            RESTORE_TIMEOUT_MS,
            None,
        );
    }
    // Don't trust the return value: some applications intercept
    // `WM_SYSCOMMAND` without restoring the window.
    if !window.is_maximized() {
        return window.borders();
    }
    let estimate = window.borders()?.normal_from_maximized();
    // SAFETY: asynchronous message to the window.
    unsafe {
        let _ = ShowWindowAsync(window.0, SW_RESTORE);
    }
    Some(estimate)
}

/// Places `window` directly behind `above` in the z-order.
pub fn place_below(window: Window, above: Window) {
    // SAFETY: z-order only, without activation.
    unsafe {
        let _ = SetWindowPos(
            window.0,
            Some(above.0),
            0,
            0,
            0,
            0,
            SWP_NOMOVE | SWP_NOSIZE | SWP_NOACTIVATE | SWP_NOOWNERZORDER | SWP_ASYNCWINDOWPOS,
        );
    }
}

pub fn maximize(window: Window) {
    // SAFETY: asynchronous message to the window.
    unsafe {
        let _ = ShowWindowAsync(window.0, SW_MAXIMIZE);
    }
}

/// Minimizes a window; the next window in the z-order gets the focus.
pub fn minimize(window: Window) {
    // SAFETY: asynchronous message to the window.
    unsafe {
        let _ = ShowWindowAsync(window.0, SW_MINIMIZE);
    }
}

/// Asks a window to close, like its close button. Posted, so a hung
/// application cannot block the daemon; the application may still ask
/// about unsaved changes.
pub fn close(window: Window) {
    // SAFETY: posting a message to an arbitrary window is safe; it fails if
    // the window no longer exists.
    unsafe {
        let _ = PostMessageW(Some(window.0), WM_CLOSE, WPARAM(0), LPARAM(0));
    }
}

pub fn foreground() -> Option<Window> {
    // SAFETY: pure query.
    let hwnd = unsafe { GetForegroundWindow() };
    (!hwnd.is_invalid()).then_some(Window(hwnd))
}

/// Brings a window to the foreground.
///
/// Windows only allows this for the process that received the last input.
/// If the first attempt fails, Tessera generates a no-op mouse movement,
/// thereby becomes the "last input recipient" itself, and tries
/// again.
pub fn focus(window: Window) -> bool {
    if window.is_minimized() {
        // SAFETY: asynchronous message to the window.
        unsafe {
            let _ = ShowWindowAsync(window.0, SW_RESTORE);
        }
    }
    // SAFETY: SetForegroundWindow is safe for arbitrary handles.
    if unsafe { SetForegroundWindow(window.0) }.as_bool() && foreground() == Some(window) {
        return true;
    }
    let input = INPUT {
        r#type: INPUT_MOUSE,
        Anonymous: INPUT_0 {
            mi: MOUSEINPUT {
                dwFlags: MOUSEEVENTF_MOVE,
                ..Default::default()
            },
        },
    };
    // SAFETY: one valid INPUT with correct size.
    unsafe {
        SendInput(&[input], size_of::<INPUT>() as i32);
        SetForegroundWindow(window.0).as_bool()
    }
}

pub fn cursor_pos() -> (i32, i32) {
    let mut p = POINT::default();
    // SAFETY: output parameter points to a local variable.
    let _ = unsafe { GetCursorPos(&mut p) };
    (p.x, p.y)
}

/// Is the key held that shows the zones while dragging?
pub fn drag_modifier_held(modifier: DragModifier) -> bool {
    let vk = match modifier {
        DragModifier::Always => return true,
        DragModifier::Shift => VK_SHIFT,
        DragModifier::Ctrl => VK_CONTROL,
        DragModifier::Alt => VK_MENU,
    };
    // SAFETY: pure query of the key state.
    unsafe { GetAsyncKeyState(i32::from(vk.0)) < 0 }
}

/// Colors the window border (Windows 11); `None` restores the system color.
pub fn set_border_color(window: Window, color: Option<Color>) {
    let value = match color {
        Some(c) => u32::from(c.r) | (u32::from(c.g) << 8) | (u32::from(c.b) << 16),
        None => DWMWA_COLOR_DEFAULT,
    };
    // SAFETY: DWORD value of matching size; merely fails on Windows 10.
    unsafe {
        let _ = DwmSetWindowAttribute(
            window.0,
            DWMWA_BORDER_COLOR,
            (&raw const value).cast(),
            size_of::<u32>() as u32,
        );
    }
}

/// Window rectangle of one of our own windows (e.g. the editor) given as a raw handle.
pub fn raw_window_rect(hwnd: isize) -> Rect {
    let mut r = RECT::default();
    // SAFETY: output parameter points to a local variable.
    let _ = unsafe { GetWindowRect(windows::Win32::Foundation::HWND(hwnd as *mut _), &mut r) };
    Rect::from_ltrb(r.left, r.top, r.right, r.bottom)
}

/// Sets the window rectangle of one of our own windows in physical pixels.
pub fn set_raw_window_rect(hwnd: isize, rect: Rect) {
    // SAFETY: synchronous move of our own window.
    unsafe {
        let _ = SetWindowPos(
            windows::Win32::Foundation::HWND(hwnd as *mut _),
            None,
            rect.x,
            rect.y,
            rect.width,
            rect.height,
            SWP_NOZORDER | SWP_NOACTIVATE,
        );
    }
}
