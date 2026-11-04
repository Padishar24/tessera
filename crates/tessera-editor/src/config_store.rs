//! Changing the shortcuts in `config.toml` without touching comments or
//! other settings.

use anyhow::{Context, bail};
use tessera_core::action::Action;
use tessera_core::config::{Config, LoadedConfig};
use tessera_core::keys::{KeyCombo, windows_function};
use tessera_core::shortcuts::{Overview, Source};
use toml_edit::{ArrayOfTables, DocumentMut, Item, Key, Table, value};

/// Binds `combo` to `action`. With `replace`, that binding of the action
/// gets the new combination (in its place in the file); otherwise one is
/// added. Whatever else `combo` was bound to loses it. Returns the new file
/// content after it has been validated against `layouts` (`layouts.toml`).
pub fn bind(
    text: &str,
    layouts: &str,
    combo: KeyCombo,
    action: Action,
    replace: Option<&Source>,
) -> anyhow::Result<String> {
    let mut doc = parse(text)?;
    let config: Config = toml::from_str(text).context("config.toml is invalid")?;
    if let Some((_, other)) = Overview::new(&config).find(combo)
        && Some(&other.source) != replace
        // A [keybindings] entry for win+e takes precedence anyway.
        && other.source != Source::ExplorerTabs
    {
        remove(&mut doc, &other.source)?;
    }

    let canonical = combo.to_string();
    match replace {
        Some(Source::Keybindings(key)) => {
            let table = keybindings(&mut doc)?;
            if !table.contains_key(key) {
                bail!("The shortcut `{key}` is no longer in config.toml.");
            }
            rename(table, key, &canonical);
            table[canonical.as_str()] = value(action.to_string());
            mark_windows_function(&mut table[canonical.as_str()], combo);
        }
        Some(Source::Scratchpad(i)) => {
            scratchpad(&mut doc, *i)?["key"] = value(canonical);
        }
        Some(Source::ExplorerTabs) | None => {
            // The first shortcut of a scratchpad goes into its own entry.
            let free_scratchpad = match action {
                Action::ToggleScratchpad(n) => scratchpad(&mut doc, usize::from(n) - 1)
                    .ok()
                    .filter(|pad| !pad.contains_key("key"))
                    .is_some()
                    .then_some(usize::from(n) - 1),
                _ => None,
            };
            match free_scratchpad {
                Some(i) => scratchpad(&mut doc, i)?["key"] = value(canonical),
                None => {
                    let table = keybindings(&mut doc)?;
                    table[canonical.as_str()] = value(action.to_string());
                    mark_windows_function(&mut table[canonical.as_str()], combo);
                }
            }
        }
    }
    validated(text, doc, layouts)
}

/// Removes the binding at `source` (for `explorer_tabs`: turns it off).
pub fn unbind(text: &str, layouts: &str, source: &Source) -> anyhow::Result<String> {
    let mut doc = parse(text)?;
    remove(&mut doc, source)?;
    validated(text, doc, layouts)
}

fn parse(text: &str) -> anyhow::Result<DocumentMut> {
    text.parse().context("config.toml is not valid TOML")
}

fn remove(doc: &mut DocumentMut, source: &Source) -> anyhow::Result<()> {
    match source {
        Source::Keybindings(key) => remove_key(keybindings(doc)?, key),
        Source::Scratchpad(i) => {
            scratchpad(doc, *i)?.remove("key");
        }
        Source::ExplorerTabs => {
            let general = doc
                .entry("general")
                .or_insert(Item::Table(Table::new()))
                .as_table_mut()
                .context("`general` must be a table")?;
            match general
                .get_mut("explorer_tabs")
                .and_then(Item::as_value_mut)
            {
                Some(old) => {
                    let decor = old.decor().clone();
                    *old = false.into();
                    *old.decor_mut() = decor;
                }
                None => general["explorer_tabs"] = value(false),
            }
        }
    }
    Ok(())
}

fn keybindings(doc: &mut DocumentMut) -> anyhow::Result<&mut Table> {
    doc.entry("keybindings")
        .or_insert(Item::Table(Table::new()))
        .as_table_mut()
        .context("`keybindings` must be a table")
}

fn scratchpad(doc: &mut DocumentMut, index: usize) -> anyhow::Result<&mut Table> {
    doc.get_mut("scratchpads")
        .and_then(Item::as_array_of_tables_mut)
        .and_then(|pads: &mut ArrayOfTables| pads.get_mut(index))
        .with_context(|| format!("Scratchpad {} is no longer in config.toml.", index + 1))
}

/// Renames `old` to `new` in its place, keeping the comments above it.
fn rename(table: &mut Table, old: &str, new: &str) {
    if old == new {
        return;
    }
    let keys: Vec<String> = table.iter().map(|(k, _)| k.to_owned()).collect();
    let Some(position) = keys.iter().position(|k| k == old) else {
        return;
    };
    // Items can only be appended, so everything from `old` on is put back
    // in order.
    let tail: Vec<(Key, Item)> = keys[position..]
        .iter()
        .filter_map(|k| {
            let key = table.key(k)?.clone();
            Some((key, table.remove(k)?))
        })
        .collect();
    for (i, (key, item)) in tail.into_iter().enumerate() {
        let key = if i == 0 {
            Key::new(new).with_leaf_decor(key.leaf_decor().clone())
        } else {
            key
        };
        table.insert_formatted(&key, item);
    }
}

/// Removes `key`. Comments above it move to the next key if that one
/// follows directly (same block), e.g. a comment about Win+Alt+number.
fn remove_key(table: &mut Table, key: &str) {
    let keys: Vec<String> = table.iter().map(|(k, _)| k.to_owned()).collect();
    let Some(position) = keys.iter().position(|k| k == key) else {
        return;
    };
    let prefix = table
        .key(key)
        .and_then(|k| k.leaf_decor().prefix())
        .and_then(|p| p.as_str())
        .map(str::to_owned);
    table.remove(key);
    let (Some(prefix), Some(next)) = (prefix, keys.get(position + 1)) else {
        return;
    };
    if let Some(mut next) = table.key_mut(next) {
        let own = next
            .leaf_decor()
            .prefix()
            .and_then(|p| p.as_str())
            .unwrap_or("");
        if prefix.contains('#') && !own.contains('\n') {
            next.leaf_decor_mut().set_prefix(prefix);
        }
    }
}

/// Puts `# ⚠ <Windows function>` behind the value if `combo` takes one
/// over, and removes such a note that no longer fits. Other comments stay.
fn mark_windows_function(item: &mut Item, combo: KeyCombo) {
    let Some(value) = item.as_value_mut() else {
        return;
    };
    let suffix = value
        .decor()
        .suffix()
        .and_then(|s| s.as_str())
        .unwrap_or("")
        .to_owned();
    if suffix.contains('#') && !suffix.contains('⚠') {
        return;
    }
    match windows_function(combo) {
        Some(function) => value.decor_mut().set_suffix(format!("   # ⚠ {function}")),
        None if suffix.contains('⚠') => value.decor_mut().set_suffix(""),
        None => {}
    }
}

/// The new content, if it is a valid configuration. A file that was invalid
/// before may stay invalid: otherwise it could not be repaired here. New
/// lines get the line endings of the file (toml_edit writes `\n`).
fn validated(before: &str, doc: DocumentMut, layouts: &str) -> anyhow::Result<String> {
    let mut text = doc.to_string();
    if before.contains("\r\n") {
        text = text.replace("\r\n", "\n").replace('\n', "\r\n");
    }
    if let Err(e) = LoadedConfig::parse_with_layouts(&text, layouts)
        && LoadedConfig::parse_with_layouts(before, layouts).is_ok()
    {
        return Err(anyhow::Error::new(e).context("The result would be invalid"));
    }
    Ok(text)
}

#[cfg(test)]
mod tests {
    use super::*;
    use tessera_core::action::Direction;
    use tessera_core::config::DEFAULT_CONFIG;
    use tessera_core::layout::DEFAULT_LAYOUTS;

    fn combo(text: &str) -> KeyCombo {
        text.parse().unwrap()
    }

    fn keybindings_of(text: &str) -> Vec<(String, String)> {
        let config: Config = toml::from_str(text).unwrap();
        config.keybindings.into_iter().collect()
    }

    fn line_of<'a>(text: &'a str, key: &str) -> &'a str {
        text.lines()
            .find(|l| l.starts_with(&format!("\"{key}\"")))
            .unwrap()
    }

    #[test]
    fn replacing_keeps_position_comments_and_marks_windows_functions() {
        let text = bind(
            DEFAULT_CONFIG,
            DEFAULT_LAYOUTS,
            combo("win+alt+shift+u"),
            Action::Focus(Direction::Up),
            Some(&Source::Keybindings("win+alt+k".into())),
        )
        .unwrap();
        // In place of the old line, the ⚠ note of win+alt+k is gone.
        let lines: Vec<&str> = text.lines().collect();
        let at = lines
            .iter()
            .position(|l| l.starts_with("\"win+alt+shift+u\""))
            .unwrap();
        assert!(lines[at - 1].starts_with("\"win+alt+j\""));
        assert_eq!(lines[at], "\"win+alt+shift+u\" = \"focus up\"");
        assert!(!text.contains("\"win+alt+k\""));
        // Everything else, including the header comment, is unchanged.
        assert!(text.starts_with(DEFAULT_CONFIG.lines().next().unwrap()));
        assert_eq!(
            keybindings_of(&text).len(),
            keybindings_of(DEFAULT_CONFIG).len()
        );

        let text = bind(
            &text,
            DEFAULT_LAYOUTS,
            combo("win+alt+d"),
            Action::Quit,
            None,
        )
        .unwrap();
        assert_eq!(
            line_of(&text, "win+alt+d"),
            "\"win+alt+d\" = \"quit\"   # ⚠ Show date and time"
        );
    }

    #[test]
    fn a_combination_moves_from_its_old_action() {
        let text = bind(
            DEFAULT_CONFIG,
            DEFAULT_LAYOUTS,
            combo("win+alt+f"),
            Action::ToggleStack,
            Some(&Source::Keybindings("win+alt+s".into())),
        )
        .unwrap();
        let bindings = keybindings_of(&text);
        assert!(bindings.contains(&("win+alt+f".into(), "toggle-stack".into())));
        assert!(!bindings.iter().any(|(_, a)| a == "toggle-float"));
        assert!(!bindings.iter().any(|(k, _)| k == "win+alt+s"));
    }

    #[test]
    fn removing_moves_a_block_comment_to_the_next_line() {
        let text = unbind(
            DEFAULT_CONFIG,
            DEFAULT_LAYOUTS,
            &Source::Keybindings("win+alt+1".into()),
        )
        .unwrap();
        let lines: Vec<&str> = text.lines().collect();
        let at = lines
            .iter()
            .position(|l| l.starts_with("\"win+alt+2\""))
            .unwrap();
        assert!(lines[at - 1].starts_with("# ⚠ Win+Alt+number"));
        assert!(!text.contains("\"win+alt+1\""));
    }

    #[test]
    fn scratchpad_keys_and_explorer_tabs() {
        let config = format!(
            "{DEFAULT_CONFIG}\n[[scratchpads]]\nname = \"notes\"\nexe = \"notepad\\\\.exe\"\n"
        );
        let config = config.replace("explorer_tabs = false", "explorer_tabs = true");
        // The first shortcut of a scratchpad goes into its entry...
        let text = bind(
            &config,
            DEFAULT_LAYOUTS,
            combo("win+alt+y"),
            Action::ToggleScratchpad(1),
            None,
        )
        .unwrap();
        assert!(text.contains("key = \"win+alt+y\""));
        // ...further ones into [keybindings].
        let text = bind(
            &text,
            DEFAULT_LAYOUTS,
            combo("win+alt+shift+y"),
            Action::ToggleScratchpad(1),
            None,
        )
        .unwrap();
        assert_eq!(
            line_of(&text, "win+alt+shift+y"),
            "\"win+alt+shift+y\" = \"toggle-scratchpad 1\""
        );
        let text = unbind(&text, DEFAULT_LAYOUTS, &Source::Scratchpad(0)).unwrap();
        assert!(!text.contains("key = \"win+alt+y\""));

        let text = unbind(&text, DEFAULT_LAYOUTS, &Source::ExplorerTabs).unwrap();
        assert!(text.contains("explorer_tabs = false"));
        assert!(text.contains("# ⚠ Win+E opens a new tab"));
    }

    #[test]
    fn keeps_the_line_endings_of_the_file() {
        let unix = DEFAULT_CONFIG.replace("\r\n", "\n");
        let windows = unix.replace('\n', "\r\n");
        for before in [unix, windows] {
            let text = bind(
                &before,
                DEFAULT_LAYOUTS,
                combo("win+alt+d"),
                Action::Quit,
                None,
            )
            .unwrap();
            let crlf = before.contains("\r\n");
            assert_eq!(text.contains("\r\n"), crlf);
            if crlf {
                assert!(
                    !text.replace("\r\n", "").contains('\n'),
                    "mixed line endings"
                );
            }
        }
    }

    #[test]
    fn rejects_results_that_would_be_invalid_but_allows_repairs() {
        let clash = format!(
            "{DEFAULT_CONFIG}\n[[scratchpads]]\nname = \"a\"\nexe = \"a\\\\.exe\"\n\
             key = \"win+alt+h\"\n"
        );
        // Invalid before (win+alt+h twice): removing one of them is allowed.
        let fixed = unbind(&clash, DEFAULT_LAYOUTS, &Source::Scratchpad(0)).unwrap();
        assert!(LoadedConfig::parse_with_layouts(&fixed, DEFAULT_LAYOUTS).is_ok());
        assert!(
            bind(
                DEFAULT_CONFIG,
                DEFAULT_LAYOUTS,
                combo("win+alt+h"),
                Action::ToggleScratchpad(5),
                None,
            )
            .is_ok(),
            "an action without a scratchpad is accepted, like in config.toml"
        );
        assert!(
            bind(
                DEFAULT_CONFIG,
                DEFAULT_LAYOUTS,
                combo("win+alt+h"),
                Action::Quit,
                Some(&Source::Keybindings("not-there".into())),
            )
            .is_err()
        );
    }
}
