//! Monitors with a stable identifier.

use std::collections::HashMap;
use std::mem::size_of;

use tessera_core::geometry::Rect;
use windows::Win32::Devices::Display::{
    DISPLAYCONFIG_DEVICE_INFO_GET_SOURCE_NAME, DISPLAYCONFIG_DEVICE_INFO_GET_TARGET_NAME,
    DISPLAYCONFIG_MODE_INFO, DISPLAYCONFIG_PATH_INFO, DISPLAYCONFIG_SOURCE_DEVICE_NAME,
    DISPLAYCONFIG_TARGET_DEVICE_NAME, DisplayConfigGetDeviceInfo, GetDisplayConfigBufferSizes,
    QDC_ONLY_ACTIVE_PATHS, QueryDisplayConfig,
};
use windows::Win32::Foundation::{ERROR_SUCCESS, LPARAM, RECT};
use windows::Win32::Graphics::Gdi::{
    EnumDisplayMonitors, GetMonitorInfoW, HDC, HMONITOR, MONITOR_DEFAULTTONEAREST, MONITORINFO,
    MONITORINFOEXW, MonitorFromWindow,
};
use windows::Win32::UI::HiDpi::{GetDpiForMonitor, MDT_EFFECTIVE_DPI};
use windows::Win32::UI::WindowsAndMessaging::MONITORINFOF_PRIMARY;
use windows::core::BOOL;

use crate::window::Window;

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Monitor {
    pub handle: HMONITOR,
    /// Device path of the monitor; stays the same across reboots as long as it is
    /// connected to the same port. Falls back to the GDI name.
    pub id: String,
    /// Display name, e.g. `DELL U2720Q`.
    pub name: String,
    /// GDI device name, e.g. `\\.\DISPLAY1`.
    pub device: String,
    pub rect: Rect,
    pub work_area: Rect,
    pub dpi: u32,
    pub primary: bool,
}

/// All active monitors, the primary one first, then from left to right.
pub fn enumerate() -> Vec<Monitor> {
    unsafe extern "system" fn collect(
        monitor: HMONITOR,
        _: HDC,
        _: *mut RECT,
        lparam: LPARAM,
    ) -> BOOL {
        // SAFETY: lparam is the pointer to the Vec from `enumerate`.
        let handles = unsafe { &mut *(lparam.0 as *mut Vec<HMONITOR>) };
        handles.push(monitor);
        true.into()
    }

    let mut handles: Vec<HMONITOR> = Vec::new();
    // SAFETY: the Vec lives for the entire synchronous call.
    let _ = unsafe {
        EnumDisplayMonitors(None, None, Some(collect), LPARAM(&raw mut handles as isize))
    };

    let names = display_config_names();
    let mut monitors: Vec<Monitor> = handles
        .into_iter()
        .filter_map(|h| describe(h, &names))
        .collect();
    monitors.sort_by_key(|m| (!m.primary, m.rect.x, m.rect.y));
    monitors
}

/// The monitor on which most of the window lies.
pub fn handle_for_window(window: Window) -> HMONITOR {
    // SAFETY: pure query.
    unsafe { MonitorFromWindow(window.0, MONITOR_DEFAULTTONEAREST) }
}

fn describe(handle: HMONITOR, names: &HashMap<String, (String, String)>) -> Option<Monitor> {
    let mut info = MONITORINFOEXW {
        monitorInfo: MONITORINFO {
            cbSize: size_of::<MONITORINFOEXW>() as u32,
            ..Default::default()
        },
        ..Default::default()
    };
    // SAFETY: cbSize identifies the extended structure.
    let ok = unsafe { GetMonitorInfoW(handle, &raw mut info.monitorInfo).as_bool() };
    if !ok {
        return None;
    }
    let device = wide_to_string(&info.szDevice);

    let (mut dpi, mut dpi_y) = (96, 96);
    // SAFETY: output parameters point to local variables.
    let _ = unsafe { GetDpiForMonitor(handle, MDT_EFFECTIVE_DPI, &mut dpi, &mut dpi_y) };

    let to_rect = |r: RECT| Rect::from_ltrb(r.left, r.top, r.right, r.bottom);
    let (id, name) = names
        .get(&device)
        .cloned()
        .unwrap_or_else(|| (device.clone(), device.clone()));
    Some(Monitor {
        handle,
        id,
        name,
        rect: to_rect(info.monitorInfo.rcMonitor),
        work_area: to_rect(info.monitorInfo.rcWork),
        dpi,
        primary: info.monitorInfo.dwFlags & MONITORINFOF_PRIMARY != 0,
        device,
    })
}

/// GDI device name -> (device path, display name) via the display config API.
fn display_config_names() -> HashMap<String, (String, String)> {
    let mut names = HashMap::new();
    let (mut path_count, mut mode_count) = (0u32, 0u32);
    // SAFETY: buffer sizes are queried beforehand and passed exactly.
    unsafe {
        if GetDisplayConfigBufferSizes(QDC_ONLY_ACTIVE_PATHS, &mut path_count, &mut mode_count)
            != ERROR_SUCCESS
        {
            return names;
        }
        let mut paths = vec![DISPLAYCONFIG_PATH_INFO::default(); path_count as usize];
        let mut modes = vec![DISPLAYCONFIG_MODE_INFO::default(); mode_count as usize];
        if QueryDisplayConfig(
            QDC_ONLY_ACTIVE_PATHS,
            &mut path_count,
            paths.as_mut_ptr(),
            &mut mode_count,
            modes.as_mut_ptr(),
            None,
        ) != ERROR_SUCCESS
        {
            return names;
        }
        paths.truncate(path_count as usize);

        for path in &paths {
            let mut source = DISPLAYCONFIG_SOURCE_DEVICE_NAME::default();
            source.header.r#type = DISPLAYCONFIG_DEVICE_INFO_GET_SOURCE_NAME;
            source.header.size = size_of::<DISPLAYCONFIG_SOURCE_DEVICE_NAME>() as u32;
            source.header.adapterId = path.sourceInfo.adapterId;
            source.header.id = path.sourceInfo.id;

            let mut target = DISPLAYCONFIG_TARGET_DEVICE_NAME::default();
            target.header.r#type = DISPLAYCONFIG_DEVICE_INFO_GET_TARGET_NAME;
            target.header.size = size_of::<DISPLAYCONFIG_TARGET_DEVICE_NAME>() as u32;
            target.header.adapterId = path.targetInfo.adapterId;
            target.header.id = path.targetInfo.id;

            if DisplayConfigGetDeviceInfo(&mut source.header) != ERROR_SUCCESS.0 as i32
                || DisplayConfigGetDeviceInfo(&mut target.header) != ERROR_SUCCESS.0 as i32
            {
                continue;
            }
            let device = wide_to_string(&source.viewGdiDeviceName);
            let path_id = wide_to_string(&target.monitorDevicePath);
            let mut friendly = wide_to_string(&target.monitorFriendlyDeviceName);
            if friendly.is_empty() {
                friendly = device.clone();
            }
            let id = if path_id.is_empty() {
                device.clone()
            } else {
                path_id
            };
            names.insert(device, (id, friendly));
        }
    }
    names
}

fn wide_to_string(buf: &[u16]) -> String {
    let len = buf.iter().position(|&c| c == 0).unwrap_or(buf.len());
    String::from_utf16_lossy(&buf[..len])
}
