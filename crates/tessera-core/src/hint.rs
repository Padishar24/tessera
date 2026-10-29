//! Labels for window hints: every visible window gets a short sequence of
//! letters, and typing it jumps to that window.

/// Letters in order of preference: home row first, then the rows above and
/// below. Only letters, so that every label can be typed as a virtual-key
/// code on any keyboard layout.
pub const ALPHABET: &str = "asdfjklghqweruioptyzxcvbnm";

/// `count` distinct labels, none of which is a prefix of another: single
/// letters as long as they suffice, otherwise two letters each. At most
/// 26 × 26 labels; for more windows the list is shorter than `count`.
pub fn labels(count: usize) -> Vec<String> {
    let letters: Vec<char> = ALPHABET.chars().collect();
    if count <= letters.len() {
        return letters[..count].iter().map(char::to_string).collect();
    }
    letters
        .iter()
        .flat_map(|a| letters.iter().map(move |b| format!("{a}{b}")))
        .take(count)
        .collect()
}

/// What the letters typed so far select.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum HintMatch {
    /// The label at this index was typed completely.
    Exact(usize),
    /// Some labels start with the letters; keep typing.
    Prefix,
    /// No label starts with the letters.
    None,
}

pub fn resolve(labels: &[String], typed: &str) -> HintMatch {
    if let Some(index) = labels.iter().position(|l| l == typed) {
        HintMatch::Exact(index)
    } else if labels.iter().any(|l| l.starts_with(typed)) {
        HintMatch::Prefix
    } else {
        HintMatch::None
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn single_letters_while_they_suffice() {
        assert_eq!(labels(3), ["a", "s", "d"]);
        assert_eq!(labels(26).len(), 26);
        assert!(labels(0).is_empty());
    }

    #[test]
    fn two_letters_beyond_the_alphabet_and_no_prefixes() {
        let many = labels(30);
        assert_eq!(many.len(), 30);
        assert!(many.iter().all(|l| l.len() == 2));
        for a in &many {
            assert_eq!(many.iter().filter(|b| b.starts_with(a.as_str())).count(), 1);
        }
        assert_eq!(labels(10_000).len(), 26 * 26);
    }

    #[test]
    fn resolves_typed_letters() {
        let l = labels(30);
        assert_eq!(resolve(&l, "a"), HintMatch::Prefix);
        assert_eq!(resolve(&l, "as"), HintMatch::Exact(1));
        assert_eq!(resolve(&l, "x"), HintMatch::None);
        let l = labels(3);
        assert_eq!(resolve(&l, "d"), HintMatch::Exact(2));
        assert_eq!(resolve(&l, "f"), HintMatch::None);
    }
}
