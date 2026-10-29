//! Commands triggered via shortcut, IPC or tray.
//!
//! Text form (config and IPC): `"<name> [argument]"`, e.g. `"focus left"`,
//! `"move-to-zone 3"`, `"toggle-float"`.

use std::fmt;
use std::str::FromStr;

use thiserror::Error;

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum Direction {
    Left,
    Right,
    Up,
    Down,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum Cycle {
    Next,
    Prev,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum Resize {
    Grow,
    Shrink,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum Action {
    Focus(Direction),
    Move(Direction),
    Swap(Direction),
    /// Zone 1-based, as shown to the user.
    MoveToZone(u8),
    ToggleFloat,
    ToggleStack,
    CycleStack(Cycle),
    ToggleOrientation,
    Resize(Resize),
    ToggleMonocle,
    CycleLayout(Cycle),
    TogglePause,
    /// Virtual desktop, 1-based.
    SwitchDesktop(u8),
    MoveToDesktop(u8),
    OpenEditor,
    /// New tab in the topmost File Explorer window, otherwise a new window.
    OpenExplorer,
    ReloadConfig,
    Quit,
}

#[derive(Debug, Error, PartialEq, Eq)]
pub enum ActionParseError {
    #[error("empty action")]
    Empty,
    #[error("unknown action `{0}`")]
    Unknown(String),
    #[error("action `{action}` expects {expected}")]
    BadArgument {
        action: String,
        expected: &'static str,
    },
}

impl FromStr for Action {
    type Err = ActionParseError;

    fn from_str(s: &str) -> Result<Self, Self::Err> {
        let mut parts = s.split_whitespace();
        let name = parts
            .next()
            .ok_or(ActionParseError::Empty)?
            .to_ascii_lowercase();
        let arg = parts.next().map(str::to_ascii_lowercase);
        let extra = parts.next();

        let bad = |expected| ActionParseError::BadArgument {
            action: name.clone(),
            expected,
        };
        let none = |action: Action| match (&arg, extra) {
            (None, None) => Ok(action),
            _ => Err(bad("no argument")),
        };
        let one = |expected| match (&arg, extra) {
            (Some(a), None) => Ok(a.as_str()),
            _ => Err(bad(expected)),
        };
        let direction = || {
            const EXPECTED: &str = "a direction (left|right|up|down)";
            match one(EXPECTED)? {
                "left" => Ok(Direction::Left),
                "right" => Ok(Direction::Right),
                "up" => Ok(Direction::Up),
                "down" => Ok(Direction::Down),
                _ => Err(bad(EXPECTED)),
            }
        };
        let cycle = || {
            const EXPECTED: &str = "next or prev";
            match one(EXPECTED)? {
                "next" => Ok(Cycle::Next),
                "prev" | "previous" => Ok(Cycle::Prev),
                _ => Err(bad(EXPECTED)),
            }
        };
        let number = || {
            const EXPECTED: &str = "a number from 1 to 99";
            match one(EXPECTED)?.parse::<u8>() {
                Ok(n @ 1..=99) => Ok(n),
                _ => Err(bad(EXPECTED)),
            }
        };

        match name.as_str() {
            "focus" => Ok(Action::Focus(direction()?)),
            "move" => Ok(Action::Move(direction()?)),
            "swap" => Ok(Action::Swap(direction()?)),
            "move-to-zone" => Ok(Action::MoveToZone(number()?)),
            "toggle-float" => none(Action::ToggleFloat),
            "toggle-stack" => none(Action::ToggleStack),
            "cycle-stack" => Ok(Action::CycleStack(cycle()?)),
            "toggle-orientation" => none(Action::ToggleOrientation),
            "resize" => {
                const EXPECTED: &str = "grow or shrink";
                match one(EXPECTED)? {
                    "grow" => Ok(Action::Resize(Resize::Grow)),
                    "shrink" => Ok(Action::Resize(Resize::Shrink)),
                    _ => Err(bad(EXPECTED)),
                }
            }
            "toggle-monocle" => none(Action::ToggleMonocle),
            "cycle-layout" => Ok(Action::CycleLayout(cycle()?)),
            "toggle-pause" => none(Action::TogglePause),
            "switch-desktop" => Ok(Action::SwitchDesktop(number()?)),
            "move-to-desktop" => Ok(Action::MoveToDesktop(number()?)),
            "open-editor" => none(Action::OpenEditor),
            "open-explorer" => none(Action::OpenExplorer),
            "reload-config" => none(Action::ReloadConfig),
            "quit" => none(Action::Quit),
            _ => Err(ActionParseError::Unknown(name)),
        }
    }
}

impl fmt::Display for Direction {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(match self {
            Direction::Left => "left",
            Direction::Right => "right",
            Direction::Up => "up",
            Direction::Down => "down",
        })
    }
}

impl fmt::Display for Cycle {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(match self {
            Cycle::Next => "next",
            Cycle::Prev => "prev",
        })
    }
}

impl fmt::Display for Action {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Action::Focus(d) => write!(f, "focus {d}"),
            Action::Move(d) => write!(f, "move {d}"),
            Action::Swap(d) => write!(f, "swap {d}"),
            Action::MoveToZone(n) => write!(f, "move-to-zone {n}"),
            Action::ToggleFloat => f.write_str("toggle-float"),
            Action::ToggleStack => f.write_str("toggle-stack"),
            Action::CycleStack(c) => write!(f, "cycle-stack {c}"),
            Action::ToggleOrientation => f.write_str("toggle-orientation"),
            Action::Resize(Resize::Grow) => f.write_str("resize grow"),
            Action::Resize(Resize::Shrink) => f.write_str("resize shrink"),
            Action::ToggleMonocle => f.write_str("toggle-monocle"),
            Action::CycleLayout(c) => write!(f, "cycle-layout {c}"),
            Action::TogglePause => f.write_str("toggle-pause"),
            Action::SwitchDesktop(n) => write!(f, "switch-desktop {n}"),
            Action::MoveToDesktop(n) => write!(f, "move-to-desktop {n}"),
            Action::OpenEditor => f.write_str("open-editor"),
            Action::OpenExplorer => f.write_str("open-explorer"),
            Action::ReloadConfig => f.write_str("reload-config"),
            Action::Quit => f.write_str("quit"),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parses_and_roundtrips() {
        for text in [
            "focus left",
            "move down",
            "swap up",
            "move-to-zone 3",
            "toggle-float",
            "cycle-stack prev",
            "resize grow",
            "cycle-layout next",
            "switch-desktop 2",
            "move-to-desktop 9",
            "open-explorer",
            "quit",
        ] {
            let action: Action = text.parse().unwrap();
            assert_eq!(action.to_string(), text);
        }
    }

    #[test]
    fn is_case_and_whitespace_insensitive() {
        assert_eq!(
            "  Focus   LEFT ".parse::<Action>(),
            Ok(Action::Focus(Direction::Left))
        );
    }

    #[test]
    fn rejects_bad_input() {
        assert_eq!("".parse::<Action>(), Err(ActionParseError::Empty));
        assert!(matches!(
            "fly away".parse::<Action>(),
            Err(ActionParseError::Unknown(_))
        ));
        assert!("focus".parse::<Action>().is_err());
        assert!("focus sideways".parse::<Action>().is_err());
        assert!("toggle-float now".parse::<Action>().is_err());
        assert!("move-to-zone 0".parse::<Action>().is_err());
        assert!("move-to-zone 3 4".parse::<Action>().is_err());
    }
}
