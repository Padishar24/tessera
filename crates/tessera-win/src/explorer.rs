//! File Explorer: open a new tab in an existing window instead of yet another
//! window (`open-explorer`, Win+E with `explorer_tabs`).

use std::process::Command;

use windows::Win32::Foundation::{HWND, LPARAM, WPARAM};
use windows::Win32::UI::WindowsAndMessaging::{EnumChildWindows, PostMessageW, WM_COMMAND};
use windows::core::BOOL;

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
        Command::new("explorer.exe").spawn()?;
        return Ok(());
    };
    position::focus(window);
    // SAFETY: asynchronous message; if the tab is gone by now, the call just fails.
    unsafe { PostMessageW(Some(tab), WM_COMMAND, WPARAM(CMD_NEW_TAB), LPARAM(0))? };
    Ok(())
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
