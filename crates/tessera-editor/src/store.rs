//! Reading and writing `layouts.toml` without touching comments or other
//! layouts.

use anyhow::{Context, bail};
use tessera_core::layout::{Layouts, SplitSpec};
use tessera_core::tree::Orientation;
use toml_edit::{Array, ArrayOfTables, DocumentMut, InlineTable, Item, Table, Value, value};

/// Replaces the layout `previous` (or `name` if `previous` is missing) with
/// a split `spec` named `name`; if there is none, it is appended. Returns
/// the new file content after it has been validated.
pub fn upsert_layout(
    text: &str,
    previous: Option<&str>,
    name: &str,
    spec: &SplitSpec,
) -> anyhow::Result<String> {
    let name = name.trim();
    if name.is_empty() {
        bail!("The layout needs a name.");
    }
    let mut doc: DocumentMut = text.parse().context("layouts.toml is not valid TOML")?;
    let layouts = layouts_mut(&mut doc)?;

    let target = previous.unwrap_or(name);
    let existing = find(layouts, target);
    if let Some(clash) = find(layouts, name)
        && Some(clash) != existing
    {
        bail!("A layout `{name}` already exists.");
    }

    let split = value(Value::InlineTable(to_inline(&spec.compacted())));
    match existing.and_then(|i| layouts.get_mut(i)) {
        Some(table) => {
            for key in ["template", "zones", "canvas", "split"] {
                table.remove(key);
            }
            table["name"] = value(name);
            table["split"] = split;
        }
        None => {
            let mut table = Table::new();
            table["name"] = value(name);
            table["split"] = split;
            layouts.push(table);
        }
    }
    validated(doc)
}

/// Removes the layout `name`. The last layout is kept.
pub fn remove_layout(text: &str, name: &str) -> anyhow::Result<String> {
    let mut doc: DocumentMut = text.parse().context("layouts.toml is not valid TOML")?;
    let layouts = layouts_mut(&mut doc)?;
    let Some(index) = find(layouts, name) else {
        bail!("Layout `{name}` does not exist.");
    };
    if layouts.len() == 1 {
        bail!("The last layout cannot be deleted.");
    }
    layouts.remove(index);
    validated(doc)
}

fn layouts_mut(doc: &mut DocumentMut) -> anyhow::Result<&mut ArrayOfTables> {
    let item = doc
        .entry("layout")
        .or_insert(Item::ArrayOfTables(ArrayOfTables::new()));
    item.as_array_of_tables_mut()
        .context("`layout` must be a list of [[layout]] tables")
}

fn find(layouts: &ArrayOfTables, name: &str) -> Option<usize> {
    layouts
        .iter()
        .position(|t| t.get("name").and_then(Item::as_str).map(str::trim) == Some(name))
}

fn validated(doc: DocumentMut) -> anyhow::Result<String> {
    let text = doc.to_string();
    Layouts::parse(&text).context("The result would be invalid")?;
    Ok(text)
}

fn to_inline(spec: &SplitSpec) -> InlineTable {
    let mut table = InlineTable::new();
    if spec.sizes.is_empty() {
        return table;
    }
    let orientation = match spec.orientation.unwrap_or(Orientation::Horizontal) {
        Orientation::Horizontal => "horizontal",
        Orientation::Vertical => "vertical",
    };
    table.insert("orientation", orientation.into());
    let mut sizes = Array::new();
    for size in &spec.sizes {
        let size = f64::from(*size);
        if size.fract().abs() < 1e-6 {
            sizes.push(size.round() as i64);
        } else {
            sizes.push((size * 10.0).round() / 10.0);
        }
    }
    table.insert("sizes", Value::Array(sizes));
    if !spec.children.is_empty() {
        let mut children = Array::new();
        for child in &spec.children {
            children.push(Value::InlineTable(to_inline(child)));
        }
        table.insert("children", Value::Array(children));
    }
    table.fmt();
    table
}

#[cfg(test)]
mod tests {
    use super::*;
    use tessera_core::layout::DEFAULT_LAYOUTS;

    fn two_thirds() -> SplitSpec {
        let mut spec = SplitSpec::default();
        spec.split_zone(&[], Orientation::Horizontal, 2.0 / 3.0);
        spec.split_zone(&[1], Orientation::Vertical, 0.5);
        spec
    }

    #[test]
    fn appends_new_layout_and_keeps_comments() {
        let text = upsert_layout(DEFAULT_LAYOUTS, None, "my-layout", &two_thirds()).unwrap();
        assert!(text.starts_with(DEFAULT_LAYOUTS.lines().next().unwrap()));
        assert!(text.contains("name = \"my-layout\""));
        assert!(text.contains(
            r#"split = { orientation = "horizontal", sizes = [66.7, 33.3], children = [{}, { orientation = "vertical", sizes = [50, 50] }] }"#
        ));
        let layouts = Layouts::parse(&text).unwrap();
        assert_eq!(layouts.get("my-layout").unwrap().zone_count(), 3);
        assert_eq!(layouts.len(), Layouts::builtin().len() + 1);
    }

    #[test]
    fn replaces_template_layout_in_place_and_renames() {
        let text = upsert_layout(DEFAULT_LAYOUTS, Some("columns-2"), "two", &two_thirds()).unwrap();
        let layouts = Layouts::parse(&text).unwrap();
        assert!(layouts.get("columns-2").is_none());
        assert_eq!(layouts.get("two").unwrap().zone_count(), 3);
        assert_eq!(layouts.len(), Layouts::builtin().len());
        // Position in the file is kept (second layout).
        assert_eq!(layouts.iter().nth(1).unwrap().name, "two");
        assert!(!text.contains("template = \"columns\"\nzones = 2"));
    }

    #[test]
    fn rejects_duplicate_names_and_empty_names() {
        assert!(upsert_layout(DEFAULT_LAYOUTS, Some("single"), "grid-4", &two_thirds()).is_err());
        assert!(upsert_layout(DEFAULT_LAYOUTS, None, "  ", &two_thirds()).is_err());
    }

    #[test]
    fn single_zone_is_saved_as_empty_split() {
        let text = upsert_layout(DEFAULT_LAYOUTS, None, "one", &SplitSpec::default()).unwrap();
        assert!(text.contains("split = {}"));
        assert_eq!(
            Layouts::parse(&text)
                .unwrap()
                .get("one")
                .unwrap()
                .zone_count(),
            1
        );
    }

    #[test]
    fn removes_layouts_but_not_the_last() {
        let text = remove_layout(DEFAULT_LAYOUTS, "grid-4").unwrap();
        assert!(Layouts::parse(&text).unwrap().get("grid-4").is_none());
        assert!(remove_layout(DEFAULT_LAYOUTS, "does-not-exist").is_err());
        let only = "[[layout]]\nname = \"a\"\ntemplate = \"single\"\n";
        assert!(remove_layout(only, "a").is_err());
    }
}
