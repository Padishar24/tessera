//! File Explorer: open a new tab in an existing window instead of yet another
//! window (`open-explorer`, Win+E with `explorer_tabs`).

use windows::Win32::Foundation::{HWND, LPARAM, WPARAM};
use windows::Win32::System::Registry::{HKEY_CURRENT_USER, RRF_RT_REG_DWORD, RegGetValueW};
use windows::Win32::UI::Shell::{SEE_MASK_NOASYNC, SHELLEXECUTEINFOW, ShellExecuteExW};
use windows::Win32::UI::WindowsAndMessaging::{
    EnumChildWindows, PostMessageW, SW_SHOWNORMAL, WM_COMMAND,
};
use windows::core::{BOOL, PCWSTR, w};

use crate::position;
use crate::window::{self, Cloak, Window};

/// Explorer's internal command for "new tab" (like Ctrl+T); undocumented.
const CMD_NEW_TAB: usize = 0xA21B;

/// Opens a new tab in the topmost File Explorer window on the current desktop
/// (restored if minimized), otherwise a new Explorer window.
pub fn open_tab_or_window() -> anyhow::Result<()> {
    let target = window::enumerate().into_iter().find_map(|w| {
        let candidate = w.class() == "CabinetWClass" && w.is_visible() && w.cloak() == Cloak::None;
        candidate
            .then(|| first_tab(w).map(|tab| (w, tab)))
            .flatten()
    });
    let Some((window, tab)) = target else {
        return open_window();
    };
    position::focus(window);
    // SAFETY: asynchronous message; if the tab is gone by now, the call just fails.
    unsafe { PostMessageW(Some(tab), WM_COMMAND, WPARAM(CMD_NEW_TAB), LPARAM(0))? };
    Ok(())
}

/// Has the running shell open a new window at the start location chosen in
/// Explorer's options ("Open File Explorer to").
///
/// Deliberately not `explorer.exe`: that starts a second shell process which
/// only forwards the request to the running one and exits, and antivirus
/// behaviour monitoring (e.g. Trend Micro) blocks that as a shell change. The
/// folder "open" verb runs in-process (`DelegateExecute`) and goes straight
/// to the running shell.
fn open_window() -> anyhow::Result<()> {
    let mut info = SHELLEXECUTEINFOW {
        cbSize: size_of::<SHELLEXECUTEINFOW>() as u32,
        // The calling thread has no message loop running during the call.
        fMask: SEE_MASK_NOASYNC,
        lpVerb: w!("open"),
        lpFile: start_location(),
        nShow: SW_SHOWNORMAL.0,
        ..Default::default()
    };
    // SAFETY: `info` and its static strings outlive the synchronous call.
    unsafe { ShellExecuteExW(&raw mut info)? };
    Ok(())
}

/// Shell location for the `LaunchTo` setting (1 = This PC, 2 = Home,
/// 3 = Downloads); Home if unset or unknown, like Explorer itself.
fn start_location() -> PCWSTR {
    let mut launch_to = 0u32;
    let mut size = size_of::<u32>() as u32;
    // SAFETY: DWORD target of matching size.
    let found = unsafe {
        RegGetValueW(
            HKEY_CURRENT_USER,
            w!("Software\\Microsoft\\Windows\\CurrentVersion\\Explorer\\Advanced"),
            w!("LaunchTo"),
            RRF_RT_REG_DWORD,
            None,
            Some((&raw mut launch_to).cast()),
            Some(&mut size),
        )
        .is_ok()
    };
    match found.then_some(launch_to) {
        Some(1) => w!("shell:::{20D04FE0-3AEA-1069-A2D8-08002B30309D}"),
        Some(3) => w!("shell:Downloads"),
        _ => w!("shell:::{F874310E-B6B7-47DC-BC84-B9E6B38F5903}"),
    }
}

/// First tab (`ShellTabWindowClass`) of an Explorer window. Control Panel
/// windows share the class `CabinetWClass` but have no tabs.
fn first_tab(window: Window) -> Option<HWND> {
    unsafe extern "system" fn find(hwnd: HWND, lparam: LPARAM) -> BOOL {
        // SAFETY: lparam is the pointer to the Option from `first_tab`.
        let found = unsafe { &mut *(lparam.0 as *mut Option<HWND>) };
        if Window(hwnd).class() == "ShellTabWindowClass" {
            *found = Some(hwnd);
            return false.into();
        }
        true.into()
    }

    let mut found = None;
    // SAFETY: the Option lives for the entire synchronous call.
    let _ =
        unsafe { EnumChildWindows(Some(window.0), Some(find), LPARAM(&raw mut found as isize)) };
    found
}
