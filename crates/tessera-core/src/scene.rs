//! Scenes: named snapshots of the arrangement on all monitors (layout, zone
//! modes and which program sits in which zone). Applying a scene restores
//! it; a scene marked `auto` is applied whenever exactly its monitors are
//! connected, e.g. when docking a laptop. Stored in `scenes.toml` next to
//! `config.toml`.

use serde::{Deserialize, Serialize};

use crate::rules::WindowProps;
use crate::tree::ZoneMode;

#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(default)]
pub struct Scenes {
    #[serde(rename = "scene")]
    pub scenes: Vec<Scene>,
}

#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(default)]
pub struct Scene {
    pub name: String,
    /// Apply automatically when exactly the monitors of this scene are
    /// connected.
    pub auto: bool,
    /// When applying, start programs of the scene that have no window.
    pub launch: bool,
    pub monitors: Vec<SceneMonitor>,
}

#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(default)]
pub struct SceneMonitor {
    /// Stable monitor identifier (device path).
    pub id: String,
    /// Display name, for people reading the file.
    pub name: String,
    pub layout: String,
    /// Mode of each zone, in zone order.
    pub zones: Vec<ZoneMode>,
    pub windows: Vec<SceneWindow>,
}

#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(default)]
pub struct SceneWindow {
    pub exe: String,
    pub class: String,
    /// Zone, 1-based as shown to the user.
    pub zone: usize,
    /// Full path of the executable, used to start the program. Only saved
    /// for scenes with `launch`.
    #[serde(skip_serializing_if = "String::is_empty")]
    pub path: String,
}

/// A live window matched to an entry of a scene.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Assignment {
    /// Index into [`Scene::monitors`].
    pub monitor: usize,
    /// Index into that monitor's `windows`.
    pub entry: usize,
    /// Index into the live windows passed to [`Scene::assign`].
    pub window: usize,
}

impl Scenes {
    pub fn parse(text: &str) -> Result<Self, toml::de::Error> {
        toml::from_str(text)
    }

    pub fn to_toml(&self) -> String {
        toml::to_string_pretty(self).expect("scenes are serializable")
    }

    /// Does `text` still contain window titles? Version 1.2.24 and earlier
    /// saved them; titles can contain sensitive data, so such a file should
    /// be written again without them.
    pub fn has_window_titles(text: &str) -> bool {
        let Ok(table) = text.parse::<toml::Table>() else {
            return false;
        };
        table
            .get("scene")
            .and_then(toml::Value::as_array)
            .into_iter()
            .flatten()
            .filter_map(|s| s.get("monitors")?.as_array())
            .flatten()
            .filter_map(|m| m.get("windows")?.as_array())
            .flatten()
            .any(|w| w.get("title").is_some())
    }

    /// Removes the program paths of scenes without `launch`: only starting
    /// programs needs them. Returns whether anything was removed.
    pub fn drop_unused_paths(&mut self) -> bool {
        let mut changed = false;
        for scene in self.scenes.iter_mut().filter(|s| !s.launch) {
            for window in scene.monitors.iter_mut().flat_map(|m| &mut m.windows) {
                if !window.path.is_empty() {
                    window.path.clear();
                    changed = true;
                }
            }
        }
        changed
    }

    pub fn get(&self, name: &str) -> Option<&Scene> {
        self.scenes.iter().find(|s| s.name == name)
    }

    /// Adds a scene or replaces the one with the same name (keeping its
    /// position).
    pub fn upsert(&mut self, scene: Scene) {
        match self.scenes.iter_mut().find(|s| s.name == scene.name) {
            Some(existing) => *existing = scene,
            None => self.scenes.push(scene),
        }
    }

    pub fn remove(&mut self, name: &str) -> bool {
        let before = self.scenes.len();
        self.scenes.retain(|s| s.name != name);
        self.scenes.len() != before
    }

    /// The first `auto` scene for exactly these monitors.
    pub fn auto_for(&self, monitor_ids: &[&str]) -> Option<&Scene> {
        let mut wanted: Vec<&str> = monitor_ids.to_vec();
        wanted.sort_unstable();
        wanted.dedup();
        self.scenes
            .iter()
            .find(|s| s.auto && s.monitor_ids() == wanted)
    }
}

impl Scene {
    /// Monitor identifiers, sorted.
    pub fn monitor_ids(&self) -> Vec<&str> {
        let mut ids: Vec<&str> = self.monitors.iter().map(|m| m.id.as_str()).collect();
        ids.sort_unstable();
        ids.dedup();
        ids
    }

    pub fn window_count(&self) -> usize {
        self.monitors.iter().map(|m| m.windows.len()).sum()
    }

    /// Matches live windows to the scene's entries by program (exe and
    /// class), in order: the n-th entry of a program gets the n-th live
    /// window of it. Each live window and each entry is used at most once.
    /// Sorted by monitor and entry, i.e. in the scene's order.
    pub fn assign(&self, live: &[WindowProps<'_>]) -> Vec<Assignment> {
        let mut taken = vec![false; live.len()];
        let mut result = Vec::new();
        for (m, monitor) in self.monitors.iter().enumerate() {
            for (e, entry) in monitor.windows.iter().enumerate() {
                let found = live.iter().enumerate().position(|(i, w)| {
                    !taken[i] && entry.exe.eq_ignore_ascii_case(w.exe) && entry.class == w.class
                });
                if let Some(window) = found {
                    taken[window] = true;
                    result.push(Assignment {
                        monitor: m,
                        entry: e,
                        window,
                    });
                }
            }
        }
        result
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn window(exe: &str, zone: usize) -> SceneWindow {
        SceneWindow {
            exe: exe.into(),
            class: "Cls".into(),
            zone,
            path: String::new(),
        }
    }

    fn props(exe: &str) -> WindowProps<'_> {
        WindowProps {
            exe,
            class: "Cls",
            title: "any",
        }
    }

    fn scene(name: &str, auto: bool, monitors: &[&str]) -> Scene {
        Scene {
            name: name.into(),
            auto,
            launch: false,
            monitors: monitors
                .iter()
                .map(|id| SceneMonitor {
                    id: (*id).into(),
                    layout: "single".into(),
                    ..Default::default()
                })
                .collect(),
        }
    }

    #[test]
    fn assigns_windows_of_the_same_program_in_order() {
        let mut s = scene("work", false, &["A", "B"]);
        s.monitors[0].windows = vec![window("code.exe", 1), window("code.exe", 2)];
        s.monitors[1].windows = vec![window("chrome.exe", 1), window("mail.exe", 1)];
        let live = [props("Code.exe"), props("chrome.exe"), props("code.exe")];
        let assigned = s.assign(&live);
        assert_eq!(
            assigned,
            vec![
                Assignment {
                    monitor: 0,
                    entry: 0,
                    window: 0
                },
                Assignment {
                    monitor: 0,
                    entry: 1,
                    window: 2
                },
                Assignment {
                    monitor: 1,
                    entry: 0,
                    window: 1
                },
            ]
        );
    }

    #[test]
    fn auto_scene_needs_exactly_its_monitors() {
        let scenes = Scenes {
            scenes: vec![
                scene("manual", false, &["A", "B"]),
                scene("desk", true, &["B", "A"]),
                scene("laptop", true, &["A"]),
            ],
        };
        assert_eq!(scenes.auto_for(&["A", "B"]).unwrap().name, "desk");
        assert_eq!(scenes.auto_for(&["A"]).unwrap().name, "laptop");
        assert!(scenes.auto_for(&["A", "B", "C"]).is_none());
    }

    #[test]
    fn upsert_replaces_by_name_and_remove() {
        let mut scenes = Scenes::default();
        scenes.upsert(scene("a", false, &["A"]));
        scenes.upsert(scene("b", false, &["A"]));
        scenes.upsert(scene("a", true, &["A"]));
        assert_eq!(scenes.scenes.len(), 2);
        assert!(scenes.scenes[0].auto);
        assert!(scenes.remove("a"));
        assert!(!scenes.remove("a"));
        assert_eq!(scenes.scenes.len(), 1);
    }

    #[test]
    fn toml_roundtrip() {
        let mut s = scene("desk", true, &["\\\\?\\DISPLAY#DEL"]);
        s.monitors[0].zones = vec![ZoneMode::Stack, ZoneMode::Auto];
        s.monitors[0].windows = vec![window("code.exe", 2)];
        s.monitors[0].windows[0].path = "C:\\Program Files\\Code\\Code.exe".into();
        let scenes = Scenes { scenes: vec![s] };
        let text = scenes.to_toml();
        assert_eq!(Scenes::parse(&text).unwrap(), scenes, "{text}");
        assert_eq!(Scenes::parse("").unwrap(), Scenes::default());
        assert!(!text.contains("title"), "{text}");
        assert!(!Scenes::has_window_titles(&text));
    }

    #[test]
    fn old_files_with_titles_still_load_and_are_detected() {
        let old = r#"
            [[scene]]
            name = "desk"

            [[scene.monitors]]
            id = "A"
            layout = "single"

            [[scene.monitors.windows]]
            exe = "code.exe"
            class = "Cls"
            title = "secret.txt - Visual Studio Code"
            zone = 1
        "#;
        assert!(Scenes::has_window_titles(old));
        let scenes = Scenes::parse(old).unwrap();
        assert_eq!(scenes.scenes[0].monitors[0].windows[0].exe, "code.exe");
        assert!(!Scenes::has_window_titles(&scenes.to_toml()));
        assert!(!Scenes::has_window_titles("not toml ["));
    }

    #[test]
    fn paths_are_kept_only_for_launch_scenes() {
        let mut quiet = scene("quiet", false, &["A"]);
        quiet.monitors[0].windows = vec![window("code.exe", 1)];
        quiet.monitors[0].windows[0].path = "C:\\Code\\Code.exe".into();
        let mut launching = quiet.clone();
        launching.name = "launching".into();
        launching.launch = true;
        let mut scenes = Scenes {
            scenes: vec![quiet, launching],
        };
        assert!(scenes.drop_unused_paths());
        assert!(scenes.scenes[0].monitors[0].windows[0].path.is_empty());
        assert_eq!(
            scenes.scenes[1].monitors[0].windows[0].path,
            "C:\\Code\\Code.exe"
        );
        assert!(!scenes.drop_unused_paths());
    }
}
