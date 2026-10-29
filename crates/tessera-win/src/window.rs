//! Access to other processes' top-level windows and the decision whether
//! Tessera manages them.

use std::ffi::c_void;
use std::mem::size_of;
use std::path::Path;
use std::sync::OnceLock;

use tessera_core::geometry::Rect;
use windows::Win32::Foundation::{CloseHandle, ERROR_ACCESS_DENIED, HWND, LPARAM, RECT};
use windows::Win32::Graphics::Dwm::{
    DWM_CLOAKED_SHELL, DWMWA_CLOAKED, DWMWA_EXTENDED_FRAME_BOUNDS, DwmGetWindowAttribute,
};
use windows::Win32::Graphics::Gdi::{
    GetMonitorInfoW, MONITOR_DEFAULTTONEAREST, MONITORINFO, MonitorFromRect,
};
use windows::Win32::System::Threading::{
    GetCurrentProcessId, OpenProcess, PROCESS_NAME_WIN32, PROCESS_QUERY_LIMITED_INFORMATION,
    QueryFullProcessImageNameW,
};
use windows::Win32::UI::WindowsAndMessaging::{
    EnumWindows, GA_ROOT, GW_OWNER, GWL_EXSTYLE, GWL_STYLE, GetAncestor, GetClassNameW, GetWindow,
    GetWindowLongPtrW, GetWindowPlacement, GetWindowRect, GetWindowTextLengthW, GetWindowTextW,
    GetWindowThreadProcessId, IsIconic, IsWindow, IsWindowVisible, IsZoomed, SWP_NOACTIVATE,
    SWP_NOMOVE, SWP_NOOWNERZORDER, SWP_NOSENDCHANGING, SWP_NOSIZE, SWP_NOZORDER, SetWindowPos,
    WINDOWPLACEMENT, WS_CAPTION, WS_CHILD, WS_EX_NOACTIVATE, WS_EX_TOOLWINDOW, WS_THICKFRAME,
};
use windows::core::{BOOL, PWSTR};

/// Handle of a top-level window. Only use on the main thread.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Window(pub HWND);

impl std::hash::Hash for Window {
    fn hash<H: std::hash::Hasher>(&self, state: &mut H) {
        self.id().hash(state);
    }
}

/// Why a window is cloaked by DWM.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Cloak {
    None,
    /// The shell cloaks it - usually because it is on another virtual
    /// desktop. But sleeping UWP apps (e.g. "Settings") end up here too;
    /// only the desktop API can tell.
    OtherDesktop,
    /// The app itself or its owner cloaks it (e.g. a suspended UWP app).
    Hidden,
}

/// Invisible borders of a window: distance between the window rectangle
/// (`GetWindowRect`) and the visible frame (`DWMWA_EXTENDED_FRAME_BOUNDS`).
///
/// On Windows 10/11 a window in the normal state has a few pixels of invisible
/// border on the left, right and bottom (the resize grip), none at the
/// top. Maximized windows, on the other hand, extend beyond the work area on
/// all four sides - their borders are no good for the normal state.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub struct Borders {
    pub left: i32,
    pub top: i32,
    pub right: i32,
    pub bottom: i32,
}

impl Borders {
    /// Borders between the visible area and the window rectangle.
    pub const fn between(visible: Rect, outer: Rect) -> Self {
        Self {
            left: visible.x - outer.x,
            top: visible.y - outer.y,
            right: outer.right() - visible.right(),
            bottom: outer.bottom() - visible.bottom(),
        }
    }

    /// Estimates the borders in the normal state from those in the maximized state.
    ///
    /// In the normal state nothing is invisible at the top, and the 1 px wide
    /// visible frame, which maximized windows lack, counts as part of the
    /// visible area. Measured on Chrome, Qt and UWP at 96 and
    /// 120 dpi; applications with a custom frame may be off by a few
    /// pixels. Only a fallback for when the restored window cannot
    /// be measured.
    pub fn normal_from_maximized(self) -> Self {
        Self {
            left: (self.left - 1).max(0),
            top: 0,
            right: (self.right - 1).max(0),
            bottom: (self.bottom - 1).max(0),
        }
    }

    /// Window rectangle whose visible area is `visible`.
    pub const fn grow(self, visible: Rect) -> Rect {
        Rect::new(
            visible.x - self.left,
            visible.y - self.top,
            visible.width + self.left + self.right,
            visible.height + self.top + self.bottom,
        )
    }

    /// Visible area of the window rectangle `outer`.
    pub const fn shrink(self, outer: Rect) -> Rect {
        Rect::new(
            outer.x + self.left,
            outer.y + self.top,
            outer.width - self.left - self.right,
            outer.height - self.top - self.bottom,
        )
    }
}

/// Is `rect` where Windows moves minimized windows
/// (-32000/-32000)? Such rectangles are no good as a window position.
pub const fn is_iconic_rect(rect: Rect) -> bool {
    rect.x <= -32000 && rect.y <= -32000
}

/// Shell window classes that are never managed.
const SHELL_CLASSES: &[&str] = &[
    "Progman",
    "WorkerW",
    "Shell_TrayWnd",
    "Shell_SecondaryTrayWnd",
    "Windows.UI.Core.CoreWindow",
    "XamlExplorerHostIslandWindow",
    "TopLevelWindowForOverflowXamlIsland",
    "NotifyIconOverflowWindow",
];

impl Window {
    /// For IPC and logs: the handle as a number.
    pub fn id(self) -> u64 {
        self.0.0 as usize as u64
    }

    pub fn from_id(id: u64) -> Self {
        Self(HWND(id as usize as *mut c_void))
    }

    pub fn exists(self) -> bool {
        // SAFETY: IsWindow accepts arbitrary handles.
        unsafe { IsWindow(Some(self.0)).as_bool() }
    }

    pub fn title(self) -> String {
        // SAFETY: buffer is large enough for length + null terminator.
        unsafe {
            let len = GetWindowTextLengthW(self.0);
            if len <= 0 {
                return String::new();
            }
            let mut buf = vec![0u16; len as usize + 1];
            let copied = GetWindowTextW(self.0, &mut buf);
            String::from_utf16_lossy(&buf[..copied.max(0) as usize])
        }
    }

    pub fn class(self) -> String {
        let mut buf = [0u16; 256];
        // SAFETY: buffer with length.
        let len = unsafe { GetClassNameW(self.0, &mut buf) };
        String::from_utf16_lossy(&buf[..len.max(0) as usize])
    }

    pub fn process_id(self) -> u32 {
        let mut pid = 0;
        // SAFETY: output parameter points to a local variable.
        unsafe { GetWindowThreadProcessId(self.0, Some(&mut pid)) };
        pid
    }

    /// File name of the executable, e.g. `notepad.exe`. Empty if the process
    /// may not be queried.
    pub fn exe(self) -> String {
        process_image(self.process_id())
            .and_then(|path| {
                Path::new(&path)
                    .file_name()
                    .map(|n| n.to_string_lossy().into_owned())
            })
            .unwrap_or_default()
    }

    fn style(self) -> u32 {
        // SAFETY: reading the window styles.
        unsafe { GetWindowLongPtrW(self.0, GWL_STYLE) as u32 }
    }

    fn ex_style(self) -> u32 {
        // SAFETY: reading the extended window styles.
        unsafe { GetWindowLongPtrW(self.0, GWL_EXSTYLE) as u32 }
    }

    pub fn is_visible(self) -> bool {
        // SAFETY: pure query.
        unsafe { IsWindowVisible(self.0).as_bool() }
    }

    pub fn is_minimized(self) -> bool {
        // SAFETY: pure query.
        unsafe { IsIconic(self.0).as_bool() }
    }

    pub fn is_maximized(self) -> bool {
        // SAFETY: pure query.
        unsafe { IsZoomed(self.0).as_bool() }
    }

    /// May Tessera move the window? Windows (UIPI) blocks windows of processes
    /// with higher privileges (administrator) with
    /// `ERROR_ACCESS_DENIED`. Checked with a no-op `SetWindowPos`.
    pub fn can_be_moved(self) -> bool {
        // SAFETY: changes neither position, size nor z-order.
        let result = unsafe {
            SetWindowPos(
                self.0,
                None,
                0,
                0,
                0,
                0,
                SWP_NOMOVE
                    | SWP_NOSIZE
                    | SWP_NOZORDER
                    | SWP_NOACTIVATE
                    | SWP_NOOWNERZORDER
                    | SWP_NOSENDCHANGING,
            )
        };
        !matches!(result, Err(e) if e.code() == ERROR_ACCESS_DENIED.to_hresult())
    }

    /// Has a border through which the user can resize it.
    pub fn is_resizable(self) -> bool {
        self.style() & WS_THICKFRAME.0 != 0
    }

    pub fn cloak(self) -> Cloak {
        let mut cloaked = 0u32;
        // SAFETY: output buffer of matching size.
        let ok = unsafe {
            DwmGetWindowAttribute(
                self.0,
                DWMWA_CLOAKED,
                (&raw mut cloaked).cast(),
                size_of::<u32>() as u32,
            )
            .is_ok()
        };
        match cloaked {
            _ if !ok => Cloak::None,
            0 => Cloak::None,
            DWM_CLOAKED_SHELL => Cloak::OtherDesktop,
            _ => Cloak::Hidden,
        }
    }

    /// Visible area without the invisible borders of Windows 10/11.
    pub fn rect(self) -> Rect {
        let mut r = RECT::default();
        // SAFETY: output buffer of matching size.
        let dwm = unsafe {
            DwmGetWindowAttribute(
                self.0,
                DWMWA_EXTENDED_FRAME_BOUNDS,
                (&raw mut r).cast(),
                size_of::<RECT>() as u32,
            )
        };
        if dwm.is_err() {
            // SAFETY: output parameter points to a local variable.
            let _ = unsafe { GetWindowRect(self.0, &mut r) };
        }
        Rect::from_ltrb(r.left, r.top, r.right, r.bottom)
    }

    /// Window rectangle including the invisible borders.
    pub fn outer_rect(self) -> Option<Rect> {
        let mut r = RECT::default();
        // SAFETY: output parameter points to a local variable.
        unsafe { GetWindowRect(self.0, &mut r) }.ok()?;
        Some(Rect::from_ltrb(r.left, r.top, r.right, r.bottom))
    }

    /// Invisible borders in the current state. For maximized windows they
    /// differ from the normal state, see [`Borders`]. Minimized windows
    /// return (as measured) the borders of the normal state.
    pub fn borders(self) -> Option<Borders> {
        let outer = self.outer_rect()?;
        Some(Borders::between(self.rect(), outer))
    }

    /// Visible area in the restored state, in screen coordinates.
    ///
    /// For windows in the normal state this is [`Window::rect`]. For minimized
    /// and maximized windows the position comes from `GetWindowPlacement`
    /// (`rcNormalPosition`) - instead of -32000/-32000 or the whole
    /// work area. For maximized windows the subtracted invisible
    /// borders are estimated ([`Borders::normal_from_maximized`]).
    ///
    /// Limitation: if the window was snapped via Aero Snap before being
    /// minimized, Windows restores it at the snapped position,
    /// not at `rcNormalPosition`.
    pub fn normal_rect(self) -> Rect {
        let minimized = self.is_minimized();
        let maximized = self.is_maximized();
        if !minimized && !maximized {
            return self.rect();
        }
        let mut placement = WINDOWPLACEMENT {
            length: size_of::<WINDOWPLACEMENT>() as u32,
            ..Default::default()
        };
        // SAFETY: output buffer with length set.
        if unsafe { GetWindowPlacement(self.0, &mut placement) }.is_err() {
            return self.rect();
        }
        let n = placement.rcNormalPosition;
        let mut outer = Rect::from_ltrb(n.left, n.top, n.right, n.bottom);
        // For top-level windows without WS_EX_TOOLWINDOW, `rcNormalPosition`
        // is given in workspace coordinates: relative to the work area of the
        // monitor the rectangle is on, instead of to the monitor itself.
        if self.ex_style() & WS_EX_TOOLWINDOW.0 == 0
            && let Some((monitor, work)) = monitor_areas(&n)
        {
            outer = workspace_to_screen(outer, monitor, work);
        }
        let borders = match self.borders() {
            Some(b) if maximized => b.normal_from_maximized(),
            Some(b) => b,
            None => Borders::default(),
        };
        borders.shrink(outer)
    }

    /// Checks whether the window is a normal, manageable app window.
    /// `Err` contains the reason for logs.
    pub fn manageable(self) -> Result<(), &'static str> {
        if !self.exists() {
            return Err("no longer exists");
        }
        if !self.is_visible() {
            return Err("invisible");
        }
        // SAFETY: pure queries.
        let (root, owner) = unsafe { (GetAncestor(self.0, GA_ROOT), GetWindow(self.0, GW_OWNER)) };
        if root != self.0 {
            return Err("not a top-level window");
        }
        if owner.is_ok_and(|o| !o.is_invalid()) {
            return Err("has an owner window (dialog/popup)");
        }
        let style = self.style();
        let ex_style = self.ex_style();
        if style & WS_CHILD.0 != 0 {
            return Err("child window");
        }
        if ex_style & WS_EX_TOOLWINDOW.0 != 0 {
            return Err("tool window");
        }
        if ex_style & WS_EX_NOACTIVATE.0 != 0 {
            return Err("not activatable");
        }
        if style & WS_CAPTION.0 != WS_CAPTION.0 && style & WS_THICKFRAME.0 == 0 {
            return Err("neither title bar nor border");
        }
        if self.cloak() == Cloak::Hidden {
            return Err("cloaked by the app");
        }
        let pid = self.process_id();
        // SAFETY: pure query without parameters.
        if pid == unsafe { GetCurrentProcessId() } {
            return Err("own window");
        }
        // The zone editor runs as a separate process of the same exe and must
        // stay floating above the monitor.
        if is_own_executable(pid) {
            return Err("Tessera window (e.g. zone editor)");
        }
        if SHELL_CLASSES.contains(&self.class().as_str()) {
            return Err("shell window");
        }
        if self.rect().is_empty() && !self.is_minimized() {
            return Err("no area");
        }
        Ok(())
    }
}

/// All top-level windows in z-order (topmost first).
pub fn enumerate() -> Vec<Window> {
    unsafe extern "system" fn collect(hwnd: HWND, lparam: LPARAM) -> BOOL {
        // SAFETY: lparam is the pointer to the Vec from `enumerate`.
        let windows = unsafe { &mut *(lparam.0 as *mut Vec<Window>) };
        windows.push(Window(hwnd));
        true.into()
    }

    let mut windows = Vec::new();
    // SAFETY: the Vec lives for the entire synchronous call.
    let _ = unsafe { EnumWindows(Some(collect), LPARAM(&raw mut windows as isize)) };
    windows
}

/// Monitor and work area rectangle of the monitor closest to `rect`.
///
/// Workspace and screen coordinates differ only by the size of the
/// taskbar; which monitor is meant can therefore be determined from the
/// not yet converted rectangle as well.
fn monitor_areas(rect: &RECT) -> Option<(Rect, Rect)> {
    let mut info = MONITORINFO {
        cbSize: size_of::<MONITORINFO>() as u32,
        ..Default::default()
    };
    // SAFETY: pure queries; output buffer with length set.
    let ok = unsafe {
        let monitor = MonitorFromRect(rect, MONITOR_DEFAULTTONEAREST);
        GetMonitorInfoW(monitor, &mut info).as_bool()
    };
    if !ok {
        return None;
    }
    let to_rect = |r: RECT| Rect::from_ltrb(r.left, r.top, r.right, r.bottom);
    Some((to_rect(info.rcMonitor), to_rect(info.rcWork)))
}

/// Converts a rectangle from workspace coordinates (origin: top-left
/// corner of the work area) to screen coordinates.
fn workspace_to_screen(rect: Rect, monitor: Rect, work: Rect) -> Rect {
    Rect::new(
        rect.x + work.x - monitor.x,
        rect.y + work.y - monitor.y,
        rect.width,
        rect.height,
    )
}

/// Does the process run from the same executable as this process?
fn is_own_executable(pid: u32) -> bool {
    static OWN: OnceLock<Option<String>> = OnceLock::new();
    let own = OWN.get_or_init(|| {
        // SAFETY: pure query.
        process_image(unsafe { GetCurrentProcessId() })
    });
    match (own, process_image(pid)) {
        (Some(own), Some(other)) => own.eq_ignore_ascii_case(&other),
        _ => false,
    }
}

fn process_image(pid: u32) -> Option<String> {
    // SAFETY: handle is closed after the query; buffer with length.
    unsafe {
        let process = OpenProcess(PROCESS_QUERY_LIMITED_INFORMATION, false, pid).ok()?;
        let mut buf = [0u16; 1024];
        let mut len = buf.len() as u32;
        let result = QueryFullProcessImageNameW(
            process,
            PROCESS_NAME_WIN32,
            PWSTR(buf.as_mut_ptr()),
            &mut len,
        );
        let _ = CloseHandle(process);
        result.ok()?;
        Some(String::from_utf16_lossy(&buf[..len as usize]))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn borders_between_grow_and_shrink_are_inverse() {
        let visible = Rect::new(100, 50, 800, 600);
        let outer = Rect::new(93, 50, 814, 607);
        let b = Borders::between(visible, outer);
        assert_eq!(
            b,
            Borders {
                left: 7,
                top: 0,
                right: 7,
                bottom: 7
            }
        );
        assert_eq!(b.grow(visible), outer);
        assert_eq!(b.shrink(outer), visible);
    }

    #[test]
    fn maximized_borders_are_corrected_for_normal_state() {
        // Maximized: 8 px all around; in the normal state 0 at the top, otherwise 7 px.
        let maximized = Borders {
            left: 8,
            top: 8,
            right: 8,
            bottom: 8,
        };
        assert_eq!(
            maximized.normal_from_maximized(),
            Borders {
                left: 7,
                top: 0,
                right: 7,
                bottom: 7
            }
        );
        // Without invisible borders it stays at 0.
        assert_eq!(
            Borders::default().normal_from_maximized(),
            Borders::default()
        );
    }

    #[test]
    fn workspace_coordinates_follow_the_taskbar() {
        let monitor = Rect::new(0, 0, 1920, 1080);
        // Taskbar at the bottom: workspace equals screen coordinates.
        let rect = Rect::new(10, 20, 300, 200);
        assert_eq!(
            workspace_to_screen(rect, monitor, Rect::new(0, 0, 1920, 1032)),
            rect
        );
        // Taskbar on the left or top: shifted by its width or height.
        assert_eq!(
            workspace_to_screen(rect, monitor, Rect::new(62, 0, 1858, 1080)),
            Rect::new(72, 20, 300, 200)
        );
        assert_eq!(
            workspace_to_screen(rect, monitor, Rect::new(0, 48, 1920, 1032)),
            Rect::new(10, 68, 300, 200)
        );
        // Second monitor to the left of the primary monitor.
        let left = Rect::new(-2560, 0, 2560, 1440);
        assert_eq!(
            workspace_to_screen(
                Rect::new(-2500, 100, 800, 600),
                left,
                Rect::new(-2560, 0, 2560, 1392)
            ),
            Rect::new(-2500, 100, 800, 600)
        );
    }

    #[test]
    fn iconic_rect_is_recognized() {
        assert!(is_iconic_rect(Rect::new(-32000, -32000, 160, 28)));
        assert!(!is_iconic_rect(Rect::new(-1920, 0, 1920, 1080)));
        assert!(!is_iconic_rect(Rect::new(0, 0, 0, 0)));
    }
}
