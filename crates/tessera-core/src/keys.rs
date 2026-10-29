//! Key combinations like `"win+alt+h"`.
//!
//! Keys are stored as Windows virtual-key codes (plain numbers, hence
//! platform-independent here). Single special characters like `^` depend on the
//! keyboard layout and are only resolved by the platform layer.

use std::fmt;
use std::str::FromStr;

use thiserror::Error;

#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, Hash, PartialOrd, Ord)]
pub struct Modifiers(u8);

impl Modifiers {
    pub const NONE: Self = Self(0);
    pub const WIN: Self = Self(1);
    pub const CTRL: Self = Self(2);
    pub const ALT: Self = Self(4);
    pub const SHIFT: Self = Self(8);

    pub const fn contains(self, other: Self) -> bool {
        self.0 & other.0 == other.0
    }

    pub const fn union(self, other: Self) -> Self {
        Self(self.0 | other.0)
    }

    pub const fn is_empty(self) -> bool {
        self.0 == 0
    }
}

impl std::ops::BitOr for Modifiers {
    type Output = Self;

    fn bitor(self, rhs: Self) -> Self {
        self.union(rhs)
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, PartialOrd, Ord)]
pub enum Key {
    /// Windows virtual-key code.
    Vk(u16),
    /// Layout-dependent character, e.g. `^` on a German keyboard.
    Char(char),
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, PartialOrd, Ord)]
pub struct KeyCombo {
    pub mods: Modifiers,
    pub key: Key,
}

#[derive(Debug, Error, PartialEq, Eq)]
pub enum KeyParseError {
    #[error("no key specified")]
    MissingKey,
    #[error("more than one key specified (`{0}` and `{1}`)")]
    MultipleKeys(String, String),
    #[error("unknown key `{0}`")]
    UnknownKey(String),
    #[error("modifier `{0}` specified twice")]
    DuplicateModifier(String),
    #[error("at least one modifier (win, ctrl, alt, shift) is required")]
    NoModifier,
    #[error("`{0}` is reserved by Windows and cannot be bound")]
    Reserved(String),
}

const VK_BACK: u16 = 0x08;
const VK_TAB: u16 = 0x09;
const VK_RETURN: u16 = 0x0D;
const VK_ESCAPE: u16 = 0x1B;
const VK_SPACE: u16 = 0x20;
const VK_PRIOR: u16 = 0x21;
const VK_NEXT: u16 = 0x22;
const VK_END: u16 = 0x23;
const VK_HOME: u16 = 0x24;
const VK_LEFT: u16 = 0x25;
const VK_UP: u16 = 0x26;
const VK_RIGHT: u16 = 0x27;
const VK_DOWN: u16 = 0x28;
const VK_INSERT: u16 = 0x2D;
const VK_DELETE: u16 = 0x2E;
const VK_NUMPAD0: u16 = 0x60;
const VK_F1: u16 = 0x70;
const VK_OEM_1: u16 = 0xBA;
const VK_OEM_PLUS: u16 = 0xBB;
const VK_OEM_COMMA: u16 = 0xBC;
const VK_OEM_MINUS: u16 = 0xBD;
const VK_OEM_PERIOD: u16 = 0xBE;
const VK_OEM_2: u16 = 0xBF;
const VK_OEM_3: u16 = 0xC0;
const VK_OEM_4: u16 = 0xDB;
const VK_OEM_5: u16 = 0xDC;
const VK_OEM_6: u16 = 0xDD;
const VK_OEM_7: u16 = 0xDE;
const VK_OEM_8: u16 = 0xDF;
const VK_OEM_102: u16 = 0xE2;

const NAMED_KEYS: &[(&str, u16)] = &[
    ("backspace", VK_BACK),
    ("tab", VK_TAB),
    ("enter", VK_RETURN),
    ("escape", VK_ESCAPE),
    ("space", VK_SPACE),
    ("pageup", VK_PRIOR),
    ("pagedown", VK_NEXT),
    ("end", VK_END),
    ("home", VK_HOME),
    ("left", VK_LEFT),
    ("up", VK_UP),
    ("right", VK_RIGHT),
    ("down", VK_DOWN),
    ("insert", VK_INSERT),
    ("delete", VK_DELETE),
    ("plus", VK_OEM_PLUS),
    ("comma", VK_OEM_COMMA),
    ("minus", VK_OEM_MINUS),
    ("period", VK_OEM_PERIOD),
    ("oem1", VK_OEM_1),
    ("oem2", VK_OEM_2),
    ("oem3", VK_OEM_3),
    ("oem4", VK_OEM_4),
    ("oem5", VK_OEM_5),
    ("oem6", VK_OEM_6),
    ("oem7", VK_OEM_7),
    ("oem8", VK_OEM_8),
    ("oem102", VK_OEM_102),
];

const ALIASES: &[(&str, &str)] = &[
    ("return", "enter"),
    ("esc", "escape"),
    ("pgup", "pageup"),
    ("pgdn", "pagedown"),
    ("ins", "insert"),
    ("del", "delete"),
];

fn parse_key(name: &str) -> Result<Key, KeyParseError> {
    let name = ALIASES
        .iter()
        .find(|(alias, _)| *alias == name)
        .map_or(name, |(_, target)| target);

    if let Some((_, vk)) = NAMED_KEYS.iter().find(|(n, _)| *n == name) {
        return Ok(Key::Vk(*vk));
    }

    let mut chars = name.chars();
    if let (Some(c), None) = (chars.next(), chars.next()) {
        return Ok(match c {
            'a'..='z' => Key::Vk(c.to_ascii_uppercase() as u16),
            '0'..='9' => Key::Vk(c as u16),
            '+' => Key::Vk(VK_OEM_PLUS),
            c => Key::Char(c),
        });
    }

    let numbered = |prefix: &str, max: u16| {
        name.strip_prefix(prefix)
            .and_then(|n| n.parse::<u16>().ok())
            .filter(|n| (1..=max).contains(n) || (prefix == "numpad" && *n == 0))
    };
    if let Some(n) = numbered("numpad", 9) {
        return Ok(Key::Vk(VK_NUMPAD0 + n));
    }
    if let Some(n) = numbered("f", 24) {
        return Ok(Key::Vk(VK_F1 + n - 1));
    }

    Err(KeyParseError::UnknownKey(name.to_owned()))
}

impl FromStr for KeyCombo {
    type Err = KeyParseError;

    fn from_str(s: &str) -> Result<Self, Self::Err> {
        let mut mods = Modifiers::NONE;
        let mut key: Option<(String, Key)> = None;

        // `+` separates parts; a `+` at the end (`win+alt++`) is the plus key.
        let lower = s.trim().to_lowercase();
        let (body, trailing_plus) = match lower.strip_suffix("++") {
            Some(rest) => (rest, true),
            None => (lower.as_str(), false),
        };
        let mut parts: Vec<&str> = body.split('+').map(str::trim).collect();
        if trailing_plus {
            parts.push("+");
        }

        for part in parts.into_iter().filter(|p| !p.is_empty()) {
            let modifier = match part {
                "win" | "super" | "meta" => Some(Modifiers::WIN),
                "ctrl" | "control" => Some(Modifiers::CTRL),
                "alt" => Some(Modifiers::ALT),
                "shift" => Some(Modifiers::SHIFT),
                _ => None,
            };
            match modifier {
                Some(m) if mods.contains(m) => {
                    return Err(KeyParseError::DuplicateModifier(part.to_owned()));
                }
                Some(m) => mods = mods | m,
                None => {
                    if let Some((first, _)) = key {
                        return Err(KeyParseError::MultipleKeys(first, part.to_owned()));
                    }
                    key = Some((part.to_owned(), parse_key(part)?));
                }
            }
        }

        let (_, key) = key.ok_or(KeyParseError::MissingKey)?;
        if mods.is_empty() {
            return Err(KeyParseError::NoModifier);
        }
        let combo = KeyCombo { mods, key };
        if combo == RESERVED_WIN_L || combo == RESERVED_CTRL_ALT_DEL {
            return Err(KeyParseError::Reserved(combo.to_string()));
        }
        Ok(combo)
    }
}

const RESERVED_WIN_L: KeyCombo = KeyCombo {
    mods: Modifiers::WIN,
    key: Key::Vk(b'L' as u16),
};
const RESERVED_CTRL_ALT_DEL: KeyCombo = KeyCombo {
    mods: Modifiers::CTRL.union(Modifiers::ALT),
    key: Key::Vk(VK_DELETE),
};

impl fmt::Display for KeyCombo {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        for (m, name) in [
            (Modifiers::WIN, "win"),
            (Modifiers::CTRL, "ctrl"),
            (Modifiers::ALT, "alt"),
            (Modifiers::SHIFT, "shift"),
        ] {
            if self.mods.contains(m) {
                write!(f, "{name}+")?;
            }
        }
        match self.key {
            Key::Char(c) => write!(f, "{c}"),
            Key::Vk(vk) => match vk {
                0x41..=0x5A => write!(f, "{}", (vk as u8 as char).to_ascii_lowercase()),
                0x30..=0x39 => write!(f, "{}", vk as u8 as char),
                _ if (VK_NUMPAD0..=VK_NUMPAD0 + 9).contains(&vk) => {
                    write!(f, "numpad{}", vk - VK_NUMPAD0)
                }
                _ if (VK_F1..VK_F1 + 24).contains(&vk) => write!(f, "f{}", vk - VK_F1 + 1),
                _ => match NAMED_KEYS.iter().find(|(_, v)| *v == vk) {
                    Some((name, _)) => f.write_str(name),
                    None => write!(f, "vk{vk:#04x}"),
                },
            },
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn combo(s: &str) -> KeyCombo {
        s.parse().unwrap()
    }

    #[test]
    fn parses_modifiers_in_any_order() {
        assert_eq!(combo("alt+win+h"), combo("WIN + ALT + H"));
        assert_eq!(combo("super+alt+h").to_string(), "win+alt+h");
    }

    #[test]
    fn parses_named_and_special_keys() {
        assert_eq!(combo("win+alt+left").key, Key::Vk(VK_LEFT));
        assert_eq!(combo("win+alt+plus").key, Key::Vk(VK_OEM_PLUS));
        assert_eq!(combo("win+alt++").key, Key::Vk(VK_OEM_PLUS));
        assert_eq!(combo("win+alt+minus").key, Key::Vk(VK_OEM_MINUS));
        assert_eq!(combo("win+alt+3").key, Key::Vk(0x33));
        assert_eq!(combo("win+alt+f12").key, Key::Vk(0x7B));
        assert_eq!(combo("win+alt+numpad0").key, Key::Vk(0x60));
        assert_eq!(combo("win+alt+^").key, Key::Char('^'));
        assert_eq!(combo("win+alt+esc").key, Key::Vk(VK_ESCAPE));
    }

    #[test]
    fn display_roundtrips() {
        for s in [
            "win+ctrl+alt+shift+1",
            "win+alt+space",
            "win+alt+^",
            "ctrl+shift+f24",
            "alt+numpad7",
        ] {
            assert_eq!(combo(s).to_string(), s);
            assert_eq!(combo(&combo(s).to_string()), combo(s));
        }
    }

    #[test]
    fn rejects_invalid_combos() {
        use KeyParseError::*;
        assert_eq!("win+alt".parse::<KeyCombo>(), Err(MissingKey));
        assert_eq!("h".parse::<KeyCombo>(), Err(NoModifier));
        assert_eq!(
            "win+win+h".parse::<KeyCombo>(),
            Err(DuplicateModifier("win".into()))
        );
        assert!(matches!(
            "win+h+j".parse::<KeyCombo>(),
            Err(MultipleKeys(..))
        ));
        assert!(matches!("win+foo".parse::<KeyCombo>(), Err(UnknownKey(_))));
        assert!(matches!("win+f25".parse::<KeyCombo>(), Err(UnknownKey(_))));
        assert!(matches!("win+l".parse::<KeyCombo>(), Err(Reserved(_))));
        assert!(matches!(
            "ctrl+alt+del".parse::<KeyCombo>(),
            Err(Reserved(_))
        ));
        // With an additional modifier, L is allowed again.
        assert!("win+alt+l".parse::<KeyCombo>().is_ok());
    }
}
