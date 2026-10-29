//! Schema and validation of `config.toml`.

use std::collections::BTreeMap;
use std::fmt;
use std::str::FromStr;

use serde::{Deserialize, Serialize};
use thiserror::Error;

use crate::action::Action;
use crate::geometry::{Rect, scale_px};
use crate::keys::KeyCombo;
use crate::layout::{DEFAULT_LAYOUTS, Layouts};
use crate::rules::{RuleConfig, Rules};
use crate::scratchpad::{ScratchpadConfig, Scratchpads};

/// The bundled default configuration; copied to
/// `%APPDATA%\Tessera\config.toml` on first start.
pub const DEFAULT_CONFIG: &str = include_str!("../../../config/config.toml");

/// Raw form of `config.toml`, as deserialized.
#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize)]
#[serde(default, deny_unknown_fields)]
pub struct Config {
    pub general: General,
    pub gaps: Gaps,
    pub appearance: Appearance,
    pub drag: Drag,
    /// Key combination → action, e.g. `"win+alt+h" = "focus left"`.
    pub keybindings: BTreeMap<String, String>,
    pub rules: Vec<RuleConfig>,
    pub scratchpads: Vec<ScratchpadConfig>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(default, deny_unknown_fields)]
pub struct General {
    /// Tile new windows automatically.
    pub auto_tile: bool,
    /// Layout for monitors without a saved assignment.
    pub default_layout: String,
    /// Windows without a resizable frame float automatically.
    pub float_fixed_size: bool,
    /// On exit, restore tiled windows to their original size.
    pub restore_on_exit: bool,
    /// Win+E opens a new tab in an existing File Explorer window instead of a
    /// new window (unless `win+e` is bound to something else).
    pub explorer_tabs: bool,
}

impl Default for General {
    fn default() -> Self {
        Self {
            auto_tile: true,
            default_layout: "single".into(),
            float_fixed_size: true,
            restore_on_exit: true,
            explorer_tabs: false,
        }
    }
}

/// Gaps in logical pixels (scaled per monitor by the DPI factor).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(default, deny_unknown_fields)]
pub struct Gaps {
    /// Between windows within a zone.
    pub inner: u32,
    /// Between zone and edge of the work area.
    pub outer: u32,
}

impl Default for Gaps {
    fn default() -> Self {
        Self { inner: 8, outer: 8 }
    }
}

impl Gaps {
    /// Usable area of a work area (minus outer gap) and the
    /// inner gap, both scaled for `dpi`.
    pub fn usable_area(&self, work_area: Rect, dpi: u32) -> (Rect, i32) {
        let outer = scale_px(self.outer, dpi);
        (
            Rect::new(
                work_area.x + outer,
                work_area.y + outer,
                work_area.width - 2 * outer,
                work_area.height - 2 * outer,
            ),
            scale_px(self.inner, dpi),
        )
    }
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(default, deny_unknown_fields)]
pub struct Appearance {
    /// Color the border of the active window (Windows 11).
    pub active_border: bool,
    pub active_border_color: Color,
    pub zone_color: Color,
    pub zone_highlight_color: Color,
    /// Opacity of the zone overlay, 0.0 – 1.0.
    pub zone_opacity: f32,
    /// Height of the tab bar above stacks in logical pixels.
    pub stack_bar_height: u32,
    /// Close button on each tab of a stack's tab bar.
    pub stack_close_button: bool,
}

impl Default for Appearance {
    fn default() -> Self {
        Self {
            active_border: true,
            active_border_color: Color::rgb(0x62, 0xa0, 0xea),
            zone_color: Color::rgb(0x30, 0x30, 0x38),
            zone_highlight_color: Color::rgb(0x35, 0x84, 0xe4),
            zone_opacity: 0.45,
            stack_bar_height: 28,
            stack_close_button: true,
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum DragModifier {
    Shift,
    Ctrl,
    Alt,
    /// Show zones on every drag.
    Always,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(default, deny_unknown_fields)]
pub struct Drag {
    /// Key held while dragging to show zones.
    pub modifier: DragModifier,
}

impl Default for Drag {
    fn default() -> Self {
        Self {
            modifier: DragModifier::Shift,
        }
    }
}

/// Color in the format `#rrggbb`.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(try_from = "String", into = "String")]
pub struct Color {
    pub r: u8,
    pub g: u8,
    pub b: u8,
}

impl Color {
    pub const fn rgb(r: u8, g: u8, b: u8) -> Self {
        Self { r, g, b }
    }
}

impl FromStr for Color {
    type Err = String;

    fn from_str(s: &str) -> Result<Self, Self::Err> {
        let hex = s
            .strip_prefix('#')
            .filter(|h| h.len() == 6 && h.chars().all(|c| c.is_ascii_hexdigit()))
            .ok_or_else(|| format!("invalid color `{s}`, expected `#rrggbb`"))?;
        let channel = |i: usize| u8::from_str_radix(&hex[i..i + 2], 16).unwrap();
        Ok(Color::rgb(channel(0), channel(2), channel(4)))
    }
}

impl TryFrom<String> for Color {
    type Error = String;

    fn try_from(s: String) -> Result<Self, Self::Error> {
        s.parse()
    }
}

impl From<Color> for String {
    fn from(c: Color) -> Self {
        c.to_string()
    }
}

impl fmt::Display for Color {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "#{:02x}{:02x}{:02x}", self.r, self.g, self.b)
    }
}

#[derive(Debug, Error)]
pub enum ConfigError {
    #[error("syntax error: {0}")]
    Syntax(#[from] toml::de::Error),
    #[error("invalid configuration:\n  - {}", .0.join("\n  - "))]
    Invalid(Vec<String>),
    #[error("layouts.toml: {0}")]
    Layouts(Box<ConfigError>),
}

/// Validated, ready-to-use configuration.
#[derive(Debug, Clone)]
pub struct LoadedConfig {
    pub config: Config,
    /// Sorted by key combination.
    pub bindings: Vec<(KeyCombo, Action)>,
    pub rules: Rules,
    pub scratchpads: Scratchpads,
    pub layouts: Layouts,
}

impl LoadedConfig {
    /// `config.toml` with the built-in layouts.
    pub fn parse(text: &str) -> Result<Self, ConfigError> {
        Self::parse_with_layouts(text, DEFAULT_LAYOUTS)
    }

    /// `config.toml` and `layouts.toml`.
    pub fn parse_with_layouts(text: &str, layouts: &str) -> Result<Self, ConfigError> {
        let config: Config = toml::from_str(text)?;
        let layouts = Layouts::parse(layouts).map_err(|e| ConfigError::Layouts(Box::new(e)))?;
        Self::validate(config, layouts)
    }

    /// The built-in default configuration (guaranteed valid by a test).
    pub fn builtin() -> Self {
        Self::parse(DEFAULT_CONFIG).expect("built-in default configuration is valid")
    }

    pub fn validate(config: Config, layouts: Layouts) -> Result<Self, ConfigError> {
        let mut errors = Vec::new();

        if !(0.0..=1.0).contains(&config.appearance.zone_opacity) {
            errors.push("appearance.zone_opacity must be between 0.0 and 1.0".to_owned());
        }
        if layouts.get(&config.general.default_layout).is_none() {
            errors.push(format!(
                "general.default_layout: layout `{}` does not exist in layouts.toml",
                config.general.default_layout
            ));
        }

        let mut bindings: BTreeMap<KeyCombo, (String, Action)> = BTreeMap::new();
        for (combo_text, action_text) in &config.keybindings {
            let combo = match combo_text.parse::<KeyCombo>() {
                Ok(c) => c,
                Err(e) => {
                    errors.push(format!("keybindings `{combo_text}`: {e}"));
                    continue;
                }
            };
            let action = match action_text.parse::<Action>() {
                Ok(a) => a,
                Err(e) => {
                    errors.push(format!("keybindings `{combo_text}`: {e}"));
                    continue;
                }
            };
            if let Some((other, _)) = bindings.insert(combo, (combo_text.clone(), action)) {
                errors.push(format!(
                    "keybindings: `{other}` and `{combo_text}` are the same key combination"
                ));
            }
        }

        if config.general.explorer_tabs {
            let win_e = "win+e".parse::<KeyCombo>().expect("valid key combination");
            bindings
                .entry(win_e)
                .or_insert_with(|| ("win+e".to_owned(), Action::OpenExplorer));
        }

        let rules = Rules::compile(&config.rules).unwrap_or_else(|e| {
            errors.extend(e);
            Rules::default()
        });
        let scratchpads = Scratchpads::compile(&config.scratchpads).unwrap_or_else(|e| {
            errors.extend(e);
            Scratchpads::default()
        });
        for (i, pad) in config.scratchpads.iter().enumerate() {
            let Some(key) = &pad.key else {
                continue;
            };
            let Ok(number) = u8::try_from(i + 1) else {
                continue;
            };
            match key.parse::<KeyCombo>() {
                Ok(combo) => {
                    let action = Action::ToggleScratchpad(number);
                    if let Some((other, _)) = bindings.insert(combo, (key.clone(), action)) {
                        errors.push(format!(
                            "scratchpad {number}: key `{key}` is already bound as `{other}`"
                        ));
                    }
                }
                Err(e) => errors.push(format!("scratchpad {number}: key `{key}`: {e}")),
            }
        }

        if !errors.is_empty() {
            return Err(ConfigError::Invalid(errors));
        }
        Ok(Self {
            bindings: bindings.into_iter().map(|(c, (_, a))| (c, a)).collect(),
            rules,
            scratchpads,
            layouts,
            config,
        })
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::action::Direction;

    #[test]
    fn builtin_config_is_valid() {
        let loaded = LoadedConfig::builtin();
        assert!(!loaded.bindings.is_empty());
        assert!(!loaded.rules.is_empty());
        let focus_left = "win+alt+h".parse::<KeyCombo>().unwrap();
        assert!(
            loaded
                .bindings
                .contains(&(focus_left, Action::Focus(Direction::Left)))
        );
    }

    #[test]
    fn explorer_tabs_binds_win_e_unless_bound() {
        let win_e = "win+e".parse::<KeyCombo>().unwrap();
        let action_for = |text: &str| {
            let loaded = LoadedConfig::parse(text).unwrap();
            loaded
                .bindings
                .iter()
                .find(|(combo, _)| *combo == win_e)
                .map(|(_, action)| *action)
        };
        assert_eq!(action_for(""), None);
        assert_eq!(
            action_for("[general]\nexplorer_tabs = true"),
            Some(Action::OpenExplorer)
        );
        assert_eq!(
            action_for("[general]\nexplorer_tabs = true\n[keybindings]\n\"win+e\" = \"quit\""),
            Some(Action::Quit)
        );
    }

    #[test]
    fn scratchpad_keys_become_bindings() {
        let text = r#"
            [[scratchpads]]
            name = "term"
            exe = 'WindowsTerminal\.exe'
            key = "win+alt+t"

            [[scratchpads]]
            name = "notes"
            title = "scratch"
        "#;
        let loaded = LoadedConfig::parse(text).unwrap();
        assert_eq!(loaded.scratchpads.len(), 2);
        let key = "win+alt+t".parse::<KeyCombo>().unwrap();
        assert_eq!(loaded.bindings, vec![(key, Action::ToggleScratchpad(1))]);

        let clash = format!("{text}\n[keybindings]\n\"win+alt+t\" = \"quit\"\n");
        let ConfigError::Invalid(errors) = LoadedConfig::parse(&clash).unwrap_err() else {
            panic!("expected validation errors");
        };
        assert_eq!(errors.len(), 1, "{errors:#?}");
    }

    #[test]
    fn empty_file_uses_defaults() {
        let loaded = LoadedConfig::parse("").unwrap();
        assert_eq!(loaded.config.general, General::default());
        assert_eq!(loaded.config.gaps, Gaps::default());
        assert!(loaded.bindings.is_empty());
    }

    #[test]
    fn partial_sections_keep_other_defaults() {
        let loaded = LoadedConfig::parse("[gaps]\ninner = 4\n").unwrap();
        assert_eq!(loaded.config.gaps, Gaps { inner: 4, outer: 8 });
    }

    #[test]
    fn colors_parse_and_print() {
        let loaded =
            LoadedConfig::parse("[appearance]\nactive_border_color = \"#FF8000\"\n").unwrap();
        assert_eq!(
            loaded.config.appearance.active_border_color,
            Color::rgb(255, 128, 0)
        );
        assert_eq!(Color::rgb(255, 128, 0).to_string(), "#ff8000");
        assert!(LoadedConfig::parse("[appearance]\nactive_border_color = \"red\"\n").is_err());
    }

    #[test]
    fn unknown_fields_are_rejected() {
        assert!(matches!(
            LoadedConfig::parse("[gaps]\ninnner = 4\n"),
            Err(ConfigError::Syntax(_))
        ));
    }

    #[test]
    fn collects_all_semantic_errors() {
        let text = r#"
            [appearance]
            zone_opacity = 1.5

            [keybindings]
            "win+alt+h" = "focus left"
            "alt+win+h" = "focus right"
            "win+foo" = "quit"
            "win+alt+q" = "explode"

            [[rules]]
            exe = "("
            float = true
        "#;
        let ConfigError::Invalid(errors) = LoadedConfig::parse(text).unwrap_err() else {
            panic!("expected validation errors");
        };
        assert_eq!(errors.len(), 5, "{errors:#?}");
    }
}
