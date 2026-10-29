//! Scratchpads: windows that stay minimized until a shortcut summons them
//! in front of everything else; the same shortcut hides them again.
//! Configured in `config.toml` under `[[scratchpads]]`.

use serde::{Deserialize, Serialize};

use crate::geometry::Rect;
use crate::rules::{Matcher, WindowProps};

/// Actions address scratchpads with a number from 1 to 99.
pub const MAX_SCRATCHPADS: usize = 99;

/// Scratchpad as written in `config.toml`.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(default, deny_unknown_fields)]
pub struct ScratchpadConfig {
    /// Name for messages and logs.
    pub name: String,
    /// Which window it is: patterns like in `[[rules]]`.
    pub exe: Option<String>,
    pub class: Option<String>,
    pub title: Option<String>,
    /// Program started when the scratchpad is summoned but no window
    /// matches.
    pub command: Option<String>,
    pub args: Vec<String>,
    /// Shortcut that shows and hides it, e.g. `"win+alt+t"`.
    pub key: Option<String>,
    /// Size in percent of the monitor's work area.
    pub width: u8,
    pub height: u8,
    pub position: ScratchpadPosition,
    /// Hide it again as soon as another window gets the focus.
    pub hide_on_focus_loss: bool,
}

impl Default for ScratchpadConfig {
    fn default() -> Self {
        Self {
            name: String::new(),
            exe: None,
            class: None,
            title: None,
            command: None,
            args: Vec::new(),
            key: None,
            width: 60,
            height: 60,
            position: ScratchpadPosition::Center,
            hide_on_focus_loss: false,
        }
    }
}

#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum ScratchpadPosition {
    /// In the middle of the work area.
    #[default]
    Center,
    /// Centered horizontally at the top edge, like a drop-down terminal.
    Top,
}

#[derive(Debug, Clone)]
pub struct Scratchpad {
    pub config: ScratchpadConfig,
    matcher: Matcher,
}

impl Scratchpad {
    pub fn name(&self) -> &str {
        &self.config.name
    }

    pub fn matches(&self, props: WindowProps<'_>) -> bool {
        self.matcher.matches(props)
    }

    /// Where the scratchpad appears in a work area.
    pub fn rect(&self, work_area: Rect) -> Rect {
        let width = work_area.width * i32::from(self.config.width) / 100;
        let height = work_area.height * i32::from(self.config.height) / 100;
        let x = work_area.x + (work_area.width - width) / 2;
        let y = match self.config.position {
            ScratchpadPosition::Center => work_area.y + (work_area.height - height) / 2,
            ScratchpadPosition::Top => work_area.y,
        };
        Rect::new(x, y, width, height)
    }
}

/// Replaces `%NAME%` with the value of the environment variable `NAME`, as
/// `cmd.exe` does; unknown variables and a lone `%` stay as they are.
pub fn expand_env(text: &str, lookup: impl Fn(&str) -> Option<String>) -> String {
    let mut out = String::with_capacity(text.len());
    let mut rest = text;
    while let Some(start) = rest.find('%') {
        out.push_str(&rest[..start]);
        let after = &rest[start + 1..];
        match after.find('%') {
            Some(end) if end > 0 => match lookup(&after[..end]) {
                Some(value) => {
                    out.push_str(&value);
                    rest = &after[end + 1..];
                }
                None => {
                    // Keep the first `%`; the second may open a variable.
                    out.push('%');
                    rest = after;
                }
            },
            _ => {
                out.push('%');
                rest = after;
            }
        }
    }
    out.push_str(rest);
    out
}

/// Validated scratchpads in configuration order.
#[derive(Debug, Clone, Default)]
pub struct Scratchpads(Vec<Scratchpad>);

impl Scratchpads {
    /// Compiles the scratchpads; errors name the scratchpad (1-based).
    /// Shortcuts (`key`) are checked together with the other bindings.
    pub fn compile(configs: &[ScratchpadConfig]) -> Result<Self, Vec<String>> {
        let mut scratchpads = Vec::with_capacity(configs.len());
        let mut errors = Vec::new();
        if configs.len() > MAX_SCRATCHPADS {
            errors.push(format!(
                "at most {MAX_SCRATCHPADS} scratchpads are possible"
            ));
        }
        for (i, cfg) in configs.iter().enumerate() {
            let context = format!("scratchpad {}", i + 1);
            if cfg.name.trim().is_empty() {
                errors.push(format!("{context}: specify a name"));
            } else if configs[..i].iter().any(|other| other.name == cfg.name) {
                errors.push(format!("{context}: name `{}` is used twice", cfg.name));
            }
            if cfg.exe.is_none() && cfg.class.is_none() && cfg.title.is_none() {
                errors.push(format!("{context}: specify at least exe, class or title"));
            }
            for (field, value) in [("width", cfg.width), ("height", cfg.height)] {
                if !(10..=100).contains(&value) {
                    errors.push(format!(
                        "{context}: {field} must be between 10 and 100 (percent)"
                    ));
                }
            }
            let matcher = Matcher::compile(
                &context,
                cfg.exe.as_deref(),
                cfg.class.as_deref(),
                cfg.title.as_deref(),
                &mut errors,
            );
            scratchpads.push(Scratchpad {
                config: cfg.clone(),
                matcher,
            });
        }
        if errors.is_empty() {
            Ok(Self(scratchpads))
        } else {
            Err(errors)
        }
    }

    pub fn get(&self, index: usize) -> Option<&Scratchpad> {
        self.0.get(index)
    }

    /// Index of the first scratchpad the window belongs to.
    pub fn find(&self, props: WindowProps<'_>) -> Option<usize> {
        self.0.iter().position(|s| s.matches(props))
    }

    pub fn position(&self, name: &str) -> Option<usize> {
        self.0.iter().position(|s| s.name() == name)
    }

    pub fn iter(&self) -> impl Iterator<Item = &Scratchpad> {
        self.0.iter()
    }

    pub fn len(&self) -> usize {
        self.0.len()
    }

    pub fn is_empty(&self) -> bool {
        self.0.is_empty()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn config(name: &str, exe: &str) -> ScratchpadConfig {
        ScratchpadConfig {
            name: name.into(),
            exe: Some(exe.into()),
            ..Default::default()
        }
    }

    fn props<'a>(exe: &'a str, title: &'a str) -> WindowProps<'a> {
        WindowProps {
            exe,
            class: "Cls",
            title,
        }
    }

    #[test]
    fn finds_the_first_matching_scratchpad() {
        let mut notes = config("notes", "notepad\\.exe");
        notes.title = Some("scratch".into());
        let pads = Scratchpads::compile(&[notes, config("term", "WindowsTerminal\\.exe")]).unwrap();
        assert_eq!(pads.find(props("Notepad.exe", "scratch.txt")), Some(0));
        assert_eq!(pads.find(props("notepad.exe", "todo.txt")), None);
        assert_eq!(pads.find(props("WindowsTerminal.exe", "pwsh")), Some(1));
        assert_eq!(pads.position("term"), Some(1));
    }

    #[test]
    fn rect_is_a_share_of_the_work_area() {
        let mut cfg = config("term", "wt\\.exe");
        cfg.width = 50;
        cfg.height = 40;
        let work = Rect::new(100, 0, 2000, 1000);
        let pads = Scratchpads::compile(std::slice::from_ref(&cfg)).unwrap();
        assert_eq!(
            pads.get(0).unwrap().rect(work),
            Rect::new(600, 300, 1000, 400)
        );
        cfg.position = ScratchpadPosition::Top;
        let pads = Scratchpads::compile(&[cfg]).unwrap();
        assert_eq!(
            pads.get(0).unwrap().rect(work),
            Rect::new(600, 0, 1000, 400)
        );
    }

    #[test]
    fn expands_environment_variables() {
        let lookup = |name: &str| (name == "USERPROFILE").then(|| "C:\\Users\\me".to_owned());
        assert_eq!(
            expand_env("%USERPROFILE%\\notes.txt", lookup),
            "C:\\Users\\me\\notes.txt"
        );
        assert_eq!(expand_env("100% %NOPE% done", lookup), "100% %NOPE% done");
        assert_eq!(expand_env("a%%b%", lookup), "a%%b%");
        assert_eq!(expand_env("50%%USERPROFILE%", lookup), "50%C:\\Users\\me");
    }

    #[test]
    fn reports_invalid_scratchpads() {
        let mut too_small = config("small", "a\\.exe");
        too_small.width = 5;
        let errors = Scratchpads::compile(&[
            ScratchpadConfig::default(),
            config("x", "("),
            config("x", "b\\.exe"),
            too_small,
        ])
        .unwrap_err();
        // No name, no pattern, bad pattern, duplicate name, width.
        assert_eq!(errors.len(), 5, "{errors:#?}");
        assert!(errors[2].starts_with("scratchpad 2: invalid pattern"));
    }
}
