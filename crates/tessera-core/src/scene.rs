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
    pub title: String,
    /// Zone, 1-based as shown to the user.
    pub zone: usize,
    /// Full path of the executable, used to start the program.
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

    /// Matches live windows to the scene's entries, like `state.json` does
    /// after a restart: same program (exe and class) and the same title
    /// first, then any window of the same program. Each live window and
    /// each entry is used at most once. Sorted by monitor and entry, i.e.
    /// in the scene's order.
    pub fn assign(&self, live: &[WindowProps<'_>]) -> Vec<Assignment> {
        let same_program = |e: &SceneWindow, w: &WindowProps<'_>| {
            e.exe.eq_ignore_ascii_case(w.exe) && e.class == w.class
        };
        let mut taken = vec![false; live.len()];
        let mut result = Vec::new();
        for exact in [true, false] {
            for (m, monitor) in self.monitors.iter().enumerate() {
                for (e, entry) in monitor.windows.iter().enumerate() {
                    if result
                        .iter()
                        .any(|a: &Assignment| a.monitor == m && a.entry == e)
                    {
                        continue;
                    }
                    let found = live.iter().enumerate().position(|(i, w)| {
                        !taken[i] && same_program(entry, w) && (!exact || entry.title == w.title)
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
        }
        result.sort_by_key(|a| (a.monitor, a.entry));
        result
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn window(exe: &str, title: &str, zone: usize) -> SceneWindow {
        SceneWindow {
            exe: exe.into(),
            class: "Cls".into(),
            title: title.into(),
            zone,
            path: String::new(),
        }
    }

    fn props<'a>(exe: &'a str, title: &'a str) -> WindowProps<'a> {
        WindowProps {
            exe,
            class: "Cls",
            title,
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
    fn assigns_same_title_first_then_same_program() {
        let mut s = scene("work", false, &["A", "B"]);
        s.monitors[0].windows = vec![window("code.exe", "a", 1), window("code.exe", "b", 2)];
        s.monitors[1].windows = vec![window("chrome.exe", "x", 1), window("mail.exe", "m", 1)];
        let live = [
            props("Code.exe", "new"),
            props("chrome.exe", "y"),
            props("code.exe", "b"),
        ];
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
        s.monitors[0].windows = vec![window("code.exe", "a \"quoted\" title", 2)];
        s.monitors[0].windows[0].path = "C:\\Program Files\\Code\\Code.exe".into();
        let scenes = Scenes { scenes: vec![s] };
        let text = scenes.to_toml();
        assert_eq!(Scenes::parse(&text).unwrap(), scenes, "{text}");
        assert_eq!(Scenes::parse("").unwrap(), Scenes::default());
    }
}
