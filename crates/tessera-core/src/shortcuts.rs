//! Overview of all shortcuts for the shortcuts window: every action that
//! can be bound, its bindings and where each comes from, and the default
//! shortcuts the configuration no longer has.

use std::collections::HashSet;

use crate::action::Action;
use crate::config::{Config, DEFAULT_CONFIG};
use crate::keys::KeyCombo;

/// Where a binding is configured.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Source {
    /// `[keybindings]`, with the key as written in the file.
    Keybindings(String),
    /// `general.explorer_tabs` binds Win+E.
    ExplorerTabs,
    /// `key` of the `[[scratchpads]]` entry with this index (0-based).
    Scratchpad(usize),
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Binding {
    pub combo: KeyCombo,
    pub source: Source,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Entry {
    pub action: Action,
    /// For people, e.g. `Move to zone 3` or `Scratchpad “notes”`.
    pub label: String,
    pub bindings: Vec<Binding>,
    /// Default shortcuts of this action that nothing is bound to any more.
    pub removed_defaults: Vec<KeyCombo>,
}

/// A configured shortcut that cannot be used.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Invalid {
    /// Where it is, e.g. `[keybindings] "win+foo"`.
    pub place: String,
    pub source: Source,
    pub error: String,
}

#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct Overview {
    /// In the order of [`Action::catalog`], grouped by [`Action::group`].
    pub entries: Vec<Entry>,
    pub invalid: Vec<Invalid>,
}

impl Overview {
    pub fn new(config: &Config) -> Self {
        let mut invalid = Vec::new();
        let mut bound: Vec<(Action, Binding)> = Vec::new();
        let mut taken: HashSet<KeyCombo> = HashSet::new();

        for (key, action_text) in &config.keybindings {
            let place = format!("[keybindings] \"{key}\"");
            let combo = match key.parse::<KeyCombo>() {
                Ok(combo) => combo,
                Err(e) => {
                    invalid.push(Invalid {
                        place,
                        source: Source::Keybindings(key.clone()),
                        error: e.to_string(),
                    });
                    continue;
                }
            };
            let action = match action_text.parse::<Action>() {
                Ok(action) => action,
                Err(e) => {
                    invalid.push(Invalid {
                        place,
                        source: Source::Keybindings(key.clone()),
                        error: e.to_string(),
                    });
                    continue;
                }
            };
            if !taken.insert(combo) {
                invalid.push(Invalid {
                    place,
                    source: Source::Keybindings(key.clone()),
                    error: format!("{} is bound more than once", combo.display_name()),
                });
                continue;
            }
            bound.push((
                action,
                Binding {
                    combo,
                    source: Source::Keybindings(key.clone()),
                },
            ));
        }

        for (i, pad) in config.scratchpads.iter().enumerate() {
            let (Some(key), Ok(number)) = (&pad.key, u8::try_from(i + 1)) else {
                continue;
            };
            let place = format!("[[scratchpads]] \"{}\" key", pad.name);
            match key.parse::<KeyCombo>() {
                Ok(combo) if taken.insert(combo) => bound.push((
                    Action::ToggleScratchpad(number),
                    Binding {
                        combo,
                        source: Source::Scratchpad(i),
                    },
                )),
                Ok(combo) => invalid.push(Invalid {
                    place,
                    source: Source::Scratchpad(i),
                    error: format!("{} is bound more than once", combo.display_name()),
                }),
                Err(e) => invalid.push(Invalid {
                    place,
                    source: Source::Scratchpad(i),
                    error: e.to_string(),
                }),
            }
        }

        // A binding for win+e in [keybindings] takes precedence.
        let win_e: KeyCombo = "win+e".parse().expect("valid key combination");
        if config.general.explorer_tabs && taken.insert(win_e) {
            bound.push((
                Action::OpenExplorer,
                Binding {
                    combo: win_e,
                    source: Source::ExplorerTabs,
                },
            ));
        }

        let scratchpads = u8::try_from(config.scratchpads.len()).unwrap_or(u8::MAX);
        let mut actions = Action::catalog(scratchpads);
        for (action, _) in &bound {
            if !actions.contains(action) {
                actions.push(*action);
            }
        }
        // Stable: catalog order within each group.
        actions.sort_by_key(|a| a.group());

        let defaults = default_bindings();
        let entries = actions
            .into_iter()
            .map(|action| Entry {
                action,
                label: label(config, action),
                bindings: bound
                    .iter()
                    .filter(|(a, _)| *a == action)
                    .map(|(_, b)| b.clone())
                    .collect(),
                removed_defaults: defaults
                    .iter()
                    .filter(|(combo, a)| *a == action && !taken.contains(combo))
                    .map(|(combo, _)| *combo)
                    .collect(),
            })
            .collect();
        Self { entries, invalid }
    }

    /// The action `combo` is bound to, with the binding.
    pub fn find(&self, combo: KeyCombo) -> Option<(&Entry, &Binding)> {
        self.entries.iter().find_map(|entry| {
            entry
                .bindings
                .iter()
                .find(|b| b.combo == combo)
                .map(|b| (entry, b))
        })
    }

    pub fn binding_count(&self) -> usize {
        self.entries.iter().map(|e| e.bindings.len()).sum()
    }

    /// Actions without any binding.
    pub fn unbound_count(&self) -> usize {
        self.entries
            .iter()
            .filter(|e| e.bindings.is_empty())
            .count()
    }
}

/// The shortcuts of the bundled default configuration.
pub fn default_bindings() -> Vec<(KeyCombo, Action)> {
    let config: Config = toml::from_str(DEFAULT_CONFIG).expect("default configuration is valid");
    config
        .keybindings
        .iter()
        .filter_map(|(key, action)| Some((key.parse().ok()?, action.parse().ok()?)))
        .collect()
}

fn label(config: &Config, action: Action) -> String {
    if let Action::ToggleScratchpad(n) = action
        && let Some(pad) = config.scratchpads.get(usize::from(n) - 1)
        && !pad.name.is_empty()
    {
        return format!("Scratchpad “{}”", pad.name);
    }
    action.label()
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::action::Direction;

    fn config(text: &str) -> Config {
        toml::from_str(text).unwrap()
    }

    fn combo(text: &str) -> KeyCombo {
        text.parse().unwrap()
    }

    fn entry(overview: &Overview, action: Action) -> &Entry {
        overview
            .entries
            .iter()
            .find(|e| e.action == action)
            .unwrap()
    }

    #[test]
    fn default_configuration_has_no_removed_or_invalid_shortcuts() {
        let overview = Overview::new(&config(DEFAULT_CONFIG));
        assert!(overview.invalid.is_empty(), "{:?}", overview.invalid);
        assert!(
            overview
                .entries
                .iter()
                .all(|e| e.removed_defaults.is_empty())
        );
        assert_eq!(overview.binding_count(), default_bindings().len());
        assert!(entry(&overview, Action::Quit).bindings.is_empty());
        assert!(overview.unbound_count() > 0);
    }

    #[test]
    fn lists_all_sources_and_removed_defaults() {
        let overview = Overview::new(&config(
            r#"
            [general]
            explorer_tabs = true

            [keybindings]
            "alt+win+right" = "focus right"
            "win+alt+l" = "quit"
            "win+alt+x" = "move-to-zone 12"

            [[scratchpads]]
            name = "notes"
            exe = "notepad\\.exe"
            key = "win+alt+n"
            "#,
        ));
        let right = entry(&overview, Action::Focus(Direction::Right));
        assert_eq!(
            right.bindings,
            vec![Binding {
                combo: combo("win+alt+right"),
                source: Source::Keybindings("alt+win+right".into()),
            }]
        );
        // win+alt+l is taken by quit, so it does not count as removed.
        assert!(right.removed_defaults.is_empty());
        let left = entry(&overview, Action::Focus(Direction::Left));
        assert_eq!(
            left.removed_defaults,
            vec![combo("win+alt+h"), combo("win+alt+left")]
        );

        let pad = entry(&overview, Action::ToggleScratchpad(1));
        assert_eq!(pad.label, "Scratchpad “notes”");
        assert_eq!(pad.bindings[0].source, Source::Scratchpad(0));
        assert_eq!(
            entry(&overview, Action::OpenExplorer).bindings[0].source,
            Source::ExplorerTabs
        );
        // Bound actions outside the catalog are listed too, in their group.
        assert_eq!(entry(&overview, Action::MoveToZone(12)).bindings.len(), 1);
        let (found, _) = overview.find(combo("win+alt+l")).unwrap();
        assert_eq!(found.action, Action::Quit);
        let groups: Vec<_> = overview.entries.iter().map(|e| e.action.group()).collect();
        assert!(groups.windows(2).all(|w| w[0] <= w[1]));
    }

    #[test]
    fn keybinding_for_win_e_beats_explorer_tabs_and_errors_are_listed() {
        let overview = Overview::new(&config(
            r#"
            [general]
            explorer_tabs = true

            [keybindings]
            "win+e" = "quit"
            "win+foo" = "quit"
            "win+alt+q" = "fly away"
            "#,
        ));
        assert!(entry(&overview, Action::OpenExplorer).bindings.is_empty());
        assert_eq!(entry(&overview, Action::Quit).bindings.len(), 1);
        assert_eq!(overview.invalid.len(), 2);
        assert!(overview.invalid[0].place.contains("win+alt+q"));
    }
}
