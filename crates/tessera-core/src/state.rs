//! State that survives a restart (`state.json`): layout and pause per
//! workspace as well as the zone of each window.
//!
//! Window handles are new after a restart; windows are therefore recognized
//! by program, window class and (if possible) title.

use serde::{Deserialize, Serialize};

use crate::rules::WindowProps;
use crate::tree::ZoneMode;

#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(default)]
pub struct SavedState {
    pub workspaces: Vec<SavedWorkspace>,
}

#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(default)]
pub struct SavedWorkspace {
    pub monitor: String,
    pub desktop: String,
    pub layout: String,
    pub paused: bool,
    /// Mode of each zone (auto, stack, columns, rows), in zone order.
    pub zones: Vec<ZoneMode>,
    pub windows: Vec<SavedWindow>,
}

#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(default)]
pub struct SavedWindow {
    pub exe: String,
    pub class: String,
    pub title: String,
    /// Zone (0-based).
    pub zone: usize,
}

impl SavedState {
    pub fn workspace(&self, monitor: &str, desktop: &str) -> Option<&SavedWorkspace> {
        self.workspaces
            .iter()
            .find(|w| w.monitor == monitor && w.desktop == desktop)
    }

    /// Zone a matching window was last in. The entry is consumed so that
    /// multiple windows of the same program each get their own zone back.
    /// Same title takes precedence.
    pub fn take_zone(
        &mut self,
        monitor: &str,
        desktop: &str,
        window: WindowProps<'_>,
    ) -> Option<usize> {
        let ws = self
            .workspaces
            .iter_mut()
            .find(|w| w.monitor == monitor && w.desktop == desktop)?;
        let same_program =
            |w: &SavedWindow| w.exe.eq_ignore_ascii_case(window.exe) && w.class == window.class;
        let index = ws
            .windows
            .iter()
            .position(|w| same_program(w) && w.title == window.title)
            .or_else(|| ws.windows.iter().position(same_program))?;
        Some(ws.windows.remove(index).zone)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn props<'a>(exe: &'a str, title: &'a str) -> WindowProps<'a> {
        WindowProps {
            exe,
            class: "Cls",
            title,
        }
    }

    fn saved(exe: &str, title: &str, zone: usize) -> SavedWindow {
        SavedWindow {
            exe: exe.into(),
            class: "Cls".into(),
            title: title.into(),
            zone,
        }
    }

    #[test]
    fn prefers_same_title_and_consumes_entries() {
        let mut state = SavedState {
            workspaces: vec![SavedWorkspace {
                monitor: "M".into(),
                desktop: "D".into(),
                layout: "columns-2".into(),
                paused: false,
                zones: vec![ZoneMode::Stack, ZoneMode::Auto],
                windows: vec![saved("code.exe", "a", 0), saved("code.exe", "b", 1)],
            }],
        };
        assert_eq!(state.take_zone("M", "D", props("Code.exe", "b")), Some(1));
        assert_eq!(state.take_zone("M", "D", props("code.exe", "new")), Some(0));
        assert_eq!(state.take_zone("M", "D", props("code.exe", "a")), None);
        assert_eq!(state.take_zone("X", "D", props("code.exe", "a")), None);
    }

    #[test]
    fn json_roundtrip_and_missing_fields() {
        let state = SavedState {
            workspaces: vec![SavedWorkspace {
                monitor: "M".into(),
                layout: "single".into(),
                windows: vec![saved("a.exe", "t", 2)],
                ..Default::default()
            }],
        };
        let json = serde_json::to_string(&state).unwrap();
        assert_eq!(serde_json::from_str::<SavedState>(&json).unwrap(), state);
        assert_eq!(
            serde_json::from_str::<SavedState>("{}").unwrap(),
            SavedState::default()
        );
    }
}
