//! Protocol between `tesserac` and the daemon.
//!
//! Per connection the client sends exactly one [`Request`] as a JSON line and
//! receives exactly one [`Response`] as a JSON line in return.

use serde::{Deserialize, Serialize};

use crate::geometry::Rect;
use crate::tree::ZoneMode;

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "type", rename_all = "kebab-case")]
pub enum Request {
    /// An action in text form, e.g. `"focus left"`.
    Action { action: String },
    /// Snapshot of monitors and managed windows.
    QueryState,
    /// Set the layout on a monitor (current desktop).
    SetLayout { monitor: String, layout: String },
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(tag = "type", rename_all = "kebab-case")]
pub enum Response {
    Ok,
    Error { message: String },
    State(StateSnapshot),
}

#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize)]
pub struct StateSnapshot {
    pub monitors: Vec<MonitorState>,
    pub windows: Vec<WindowState>,
    /// Foreground window, if it is managed.
    pub foreground: Option<u64>,
    /// GUID of the current virtual desktop.
    pub current_desktop: Option<String>,
    /// GUIDs of all virtual desktops in order.
    pub desktops: Vec<String>,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct MonitorState {
    /// Stable identifier (device path), stays the same across restarts.
    pub id: String,
    /// Display name, e.g. `DELL U2720Q`.
    pub name: String,
    /// GDI device name, e.g. `\\.\DISPLAY1`.
    pub device: String,
    pub rect: Rect,
    pub work_area: Rect,
    pub dpi: u32,
    pub primary: bool,
    /// Tiling is paused on this monitor.
    pub paused: bool,
    /// Active layout on the current desktop.
    pub layout: String,
    /// Zones of the layout (empty as long as the monitor has not had a window yet).
    pub zones: Vec<Rect>,
    /// Mode of each zone (same order as `zones`).
    pub zone_modes: Vec<ZoneMode>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "kebab-case")]
pub enum ShowState {
    Normal,
    Minimized,
    Maximized,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct WindowState {
    pub hwnd: u64,
    pub exe: String,
    pub class: String,
    pub title: String,
    /// Visible area without invisible borders.
    pub rect: Rect,
    pub show: ShowState,
    /// Frame is resizable.
    pub resizable: bool,
    /// Lies in a tiling tree (otherwise floating or minimized).
    pub tiled: bool,
    /// Zone (1-based), if tiled.
    pub zone: Option<u8>,
    /// Identifier of the monitor containing the largest part of the window.
    pub monitor: Option<String>,
    /// GUID of the virtual desktop.
    pub desktop: Option<String>,
    /// The shell hides the window because it is on another desktop.
    pub on_other_desktop: bool,
    pub rule_float: bool,
    pub rule_zone: Option<u8>,
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn request_json_shape() {
        let action = Request::Action {
            action: "focus left".into(),
        };
        assert_eq!(
            serde_json::to_string(&action).unwrap(),
            r#"{"type":"action","action":"focus left"}"#
        );
        assert_eq!(
            serde_json::to_string(&Request::QueryState).unwrap(),
            r#"{"type":"query-state"}"#
        );
    }

    #[test]
    fn response_roundtrip() {
        let state = Response::State(StateSnapshot {
            current_desktop: Some("abc".into()),
            ..Default::default()
        });
        for response in [
            Response::Ok,
            Response::Error {
                message: "broken".into(),
            },
            state,
        ] {
            let json = serde_json::to_string(&response).unwrap();
            assert_eq!(serde_json::from_str::<Response>(&json).unwrap(), response);
        }
    }
}
