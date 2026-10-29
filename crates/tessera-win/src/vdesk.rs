//! Virtual desktops: assignment via the documented
//! `IVirtualDesktopManager` API, everything else via `winvd` (undocumented
//! COM API, requires Windows 11 24H2 build 26100.2605 or later).

use windows::Win32::System::Com::{
    CLSCTX_ALL, COINIT_APARTMENTTHREADED, CoCreateInstance, CoInitializeEx,
};
use windows::Win32::System::Registry::{
    HKEY_LOCAL_MACHINE, RRF_RT_REG_DWORD, RRF_RT_REG_SZ, RegGetValueW,
};
use windows::Win32::UI::Shell::{IVirtualDesktopManager, VirtualDesktopManager};
use windows::core::{GUID, w};

use crate::window::Window;

#[derive(Debug, Clone)]
pub struct DesktopInfo {
    pub index: u32,
    pub id: String,
    pub name: String,
}

#[derive(Debug, Clone)]
pub struct ProbeResult {
    pub desktops: Vec<DesktopInfo>,
    pub current: u32,
}

/// Queries all virtual desktops via `winvd`.
pub fn probe() -> Result<ProbeResult, String> {
    let err = |e: winvd::Error| format!("{e:?}");
    let desktops = winvd::get_desktops()
        .map_err(err)?
        .into_iter()
        .map(|d| {
            Ok(DesktopInfo {
                index: d.get_index().map_err(err)?,
                id: format!("{:?}", d.get_id().map_err(err)?),
                name: d.get_name().map_err(err)?,
            })
        })
        .collect::<Result<Vec<_>, String>>()?;
    let current = winvd::get_current_desktop()
        .and_then(|d| d.get_index())
        .map_err(err)?;
    Ok(ProbeResult { desktops, current })
}

/// Windows build incl. update revision, e.g. `26200.6899`.
pub fn os_build() -> Option<String> {
    let key = w!("SOFTWARE\\Microsoft\\Windows NT\\CurrentVersion");

    let mut buf = [0u16; 32];
    let mut size = std::mem::size_of_val(&buf) as u32;
    // SAFETY: buffer size correctly given in bytes.
    let build_ok = unsafe {
        RegGetValueW(
            HKEY_LOCAL_MACHINE,
            key,
            w!("CurrentBuild"),
            RRF_RT_REG_SZ,
            None,
            Some(buf.as_mut_ptr().cast()),
            Some(&mut size),
        )
        .is_ok()
    };
    if !build_ok {
        return None;
    }
    let len = buf.iter().position(|&c| c == 0).unwrap_or(buf.len());
    let build = String::from_utf16_lossy(&buf[..len]);

    let mut ubr = 0u32;
    let mut size = std::mem::size_of::<u32>() as u32;
    // SAFETY: DWORD target of matching size.
    let ubr_ok = unsafe {
        RegGetValueW(
            HKEY_LOCAL_MACHINE,
            key,
            w!("UBR"),
            RRF_RT_REG_DWORD,
            None,
            Some((&raw mut ubr).cast()),
            Some(&mut size),
        )
        .is_ok()
    };
    Some(if ubr_ok {
        format!("{build}.{ubr}")
    } else {
        build
    })
}

/// Initializes COM (STA) for the current thread.
pub fn init_com() -> anyhow::Result<()> {
    // SAFETY: once on the main thread before all COM calls.
    unsafe { CoInitializeEx(None, COINIT_APARTMENTTHREADED).ok()? };
    Ok(())
}

/// Documented desktop API (`IVirtualDesktopManager`): window -> desktop assignment.
pub struct DesktopManager(IVirtualDesktopManager);

impl DesktopManager {
    /// Requires a prior [`init_com`].
    pub fn new() -> anyhow::Result<Self> {
        // SAFETY: COM is initialized.
        let manager = unsafe { CoCreateInstance(&VirtualDesktopManager, None, CLSCTX_ALL)? };
        Ok(Self(manager))
    }

    /// GUID of the desktop the window is on.
    pub fn window_desktop(&self, window: Window) -> Option<String> {
        // SAFETY: pure query.
        let id = unsafe { self.0.GetWindowDesktopId(window.0) }.ok()?;
        (id != GUID::zeroed()).then(|| format!("{id:?}"))
    }

    pub fn is_on_current_desktop(&self, window: Window) -> Option<bool> {
        // SAFETY: pure query.
        unsafe { self.0.IsWindowOnCurrentVirtualDesktop(window.0) }
            .ok()
            .map(|b| b.as_bool())
    }
}

/// GUID of the current desktop (via `winvd`).
pub fn current_desktop() -> Option<String> {
    winvd::get_current_desktop()
        .and_then(|d| d.get_id())
        .ok()
        .map(|id| format!("{id:?}"))
}

fn winvd_error(e: winvd::Error) -> String {
    format!("Desktop API: {e:?}")
}

/// GUIDs of all desktops in order.
pub fn desktop_ids() -> Result<Vec<String>, String> {
    winvd::get_desktops()
        .map_err(winvd_error)?
        .into_iter()
        .map(|d| d.get_id().map(|id| format!("{id:?}")).map_err(winvd_error))
        .collect()
}

/// Creates enough desktops so that there are at least `count`.
fn ensure_desktops(count: usize) -> Result<(), String> {
    let existing = winvd::get_desktop_count().map_err(winvd_error)? as usize;
    for _ in existing..count {
        winvd::create_desktop().map_err(winvd_error)?;
    }
    Ok(())
}

/// Switches to desktop `index` (0-based); missing desktops are created.
pub fn switch_to(index: usize) -> Result<(), String> {
    ensure_desktops(index + 1)?;
    winvd::switch_desktop(index as u32).map_err(winvd_error)
}

/// Moves a window to desktop `index` (0-based; created if needed)
/// and returns its GUID.
pub fn move_window(window: Window, index: usize) -> Result<String, String> {
    ensure_desktops(index + 1)?;
    // winvd uses an older windows version with its own HWND type.
    let hwnd = windows_058::Win32::Foundation::HWND(window.0.0);
    winvd::move_window_to_desktop(index as u32, &hwnd).map_err(winvd_error)?;
    let id = winvd::get_desktop(index as u32)
        .get_id()
        .map_err(winvd_error)?;
    Ok(format!("{id:?}"))
}

/// Create a new desktop (for `tesserac`).
pub fn create() -> Result<(), String> {
    winvd::create_desktop().map(|_| ()).map_err(winvd_error)
}

/// Remove desktop `index` (0-based); its windows move to `fallback`.
pub fn remove(index: usize, fallback: usize) -> Result<(), String> {
    winvd::remove_desktop(index as u32, fallback as u32).map_err(winvd_error)
}

/// What the daemon learns about changes to the desktops.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum DesktopNotice {
    /// Another desktop is now active.
    Switched,
    Created,
    /// Desktop removed; its windows are now on `fallback`.
    Removed {
        removed: String,
        fallback: String,
    },
    /// Window is now on another desktop.
    WindowMoved(u64),
    /// The listener has ended (e.g. because its thread aborted after an
    /// Explorer restart); no further events will arrive.
    Stopped,
}

/// Runs as long as the value lives.
pub struct DesktopListener {
    _thread: winvd::DesktopEventThread,
}

/// Listens for desktop events. `on_notice` runs on its own
/// thread and must return immediately.
pub fn listen(
    on_notice: impl Fn(DesktopNotice) + Send + 'static,
) -> Result<DesktopListener, String> {
    let (tx, rx) = std::sync::mpsc::channel::<winvd::DesktopEvent>();
    let thread = winvd::listen_desktop_events(tx).map_err(winvd_error)?;
    let id = |d: &winvd::Desktop| d.get_id().map(|id| format!("{id:?}")).unwrap_or_default();
    std::thread::Builder::new()
        .name("desktops".into())
        .spawn(move || {
            // Ends when the listener (and thus the sender) goes away.
            for event in rx {
                let notice = match event {
                    winvd::DesktopEvent::DesktopChanged { .. } => DesktopNotice::Switched,
                    winvd::DesktopEvent::DesktopCreated(_) => DesktopNotice::Created,
                    winvd::DesktopEvent::DesktopDestroyed {
                        destroyed,
                        fallback,
                    } => DesktopNotice::Removed {
                        removed: id(&destroyed),
                        fallback: id(&fallback),
                    },
                    winvd::DesktopEvent::WindowChanged(hwnd) => {
                        DesktopNotice::WindowMoved(hwnd.0 as usize as u64)
                    }
                    _ => continue,
                };
                on_notice(notice);
            }
            on_notice(DesktopNotice::Stopped);
        })
        .map_err(|e| e.to_string())?;
    Ok(DesktopListener { _thread: thread })
}
