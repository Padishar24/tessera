//! Zone layouts (FancyZones part) from `layouts.toml`.
//!
//! A layout divides a monitor's work area into zones. Each zone later
//! becomes its own tiling tree.

use std::collections::HashSet;

use serde::{Deserialize, Serialize};

use crate::action::Cycle;
use crate::config::ConfigError;
use crate::geometry::Rect;
use crate::tree::Orientation;

/// The bundled layouts; copied to
/// `%APPDATA%\Tessera\layouts.toml` on first start.
pub const DEFAULT_LAYOUTS: &str = include_str!("../../../config/layouts.toml");

/// Maximum number of zones per layout.
const MAX_ZONES: usize = 32;

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "kebab-case")]
pub enum Template {
    /// One zone covering the whole area (pure auto-tiling).
    Single,
    Columns,
    Rows,
    Grid,
    /// Large center, smaller sides.
    PriorityGrid,
}

/// Recursive split. Without `sizes` the node is a zone.
#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize)]
#[serde(default, deny_unknown_fields)]
pub struct SplitSpec {
    pub orientation: Option<Orientation>,
    /// Relative sizes of the parts, e.g. `[2, 1]`.
    pub sizes: Vec<f32>,
    /// Subdivision of the parts; empty = each part is a zone.
    pub children: Vec<SplitSpec>,
}

/// A `[[layout]]` entry. Exactly one of `template`, `split`, `canvas`.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct LayoutConfig {
    pub name: String,
    pub template: Option<Template>,
    /// Number of zones for templates.
    pub zones: Option<u8>,
    pub split: Option<SplitSpec>,
    /// Floating zones in percent: `[x, y, width, height]`, may overlap.
    pub canvas: Option<Vec<[f32; 4]>>,
}

#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize)]
#[serde(default, deny_unknown_fields)]
pub struct LayoutsFile {
    pub layout: Vec<LayoutConfig>,
}

#[derive(Debug, Clone, PartialEq)]
enum Shape {
    Split(SplitSpec),
    Canvas(Vec<[f32; 4]>),
}

/// Validated layout.
#[derive(Debug, Clone, PartialEq)]
pub struct Layout {
    pub name: String,
    shape: Shape,
}

impl Layout {
    /// Split tree of the layout; `None` for canvas layouts.
    pub fn split(&self) -> Option<&SplitSpec> {
        match &self.shape {
            Shape::Split(spec) => Some(spec),
            Shape::Canvas(_) => None,
        }
    }

    pub fn zone_count(&self) -> usize {
        match &self.shape {
            Shape::Split(spec) => count_zones(spec),
            Shape::Canvas(rects) => rects.len(),
        }
    }

    /// Zones in pixels; `spacing` is the gap between adjacent zones.
    pub fn zones(&self, area: Rect, spacing: i32) -> Vec<Rect> {
        let mut out = Vec::new();
        match &self.shape {
            Shape::Split(spec) => split_zones(spec, area, spacing, &mut out),
            Shape::Canvas(rects) => {
                let half = spacing / 2;
                for [x, y, w, h] in rects {
                    let left = area.x + pct(area.width, *x);
                    let top = area.y + pct(area.height, *y);
                    let right = area.x + pct(area.width, x + w);
                    let bottom = area.y + pct(area.height, y + h);
                    // Gap only at edges that are not on the border of the area.
                    let inset = |edge: i32, border: i32| if edge == border { 0 } else { half };
                    out.push(Rect::from_ltrb(
                        left + inset(left, area.x),
                        top + inset(top, area.y),
                        right - inset(right, area.right()),
                        bottom - inset(bottom, area.bottom()),
                    ));
                }
            }
        }
        out
    }
}

fn pct(total: i32, percent: f32) -> i32 {
    (total as f32 * percent / 100.0).round() as i32
}

fn count_zones(spec: &SplitSpec) -> usize {
    if spec.sizes.is_empty() {
        1
    } else if spec.children.is_empty() {
        spec.sizes.len()
    } else {
        spec.children.iter().map(count_zones).sum()
    }
}

fn split_zones(spec: &SplitSpec, rect: Rect, spacing: i32, out: &mut Vec<Rect>) {
    if spec.sizes.is_empty() {
        out.push(rect);
        return;
    }
    for (i, part) in split_parts(spec, rect, spacing).into_iter().enumerate() {
        match spec.children.get(i) {
            Some(child) => split_zones(child, part, spacing, out),
            None => out.push(part),
        }
    }
}

/// Space distributed in a split after subtracting the gaps.
pub(crate) fn split_available(spec: &SplitSpec, rect: Rect, spacing: i32) -> i32 {
    let n = spec.sizes.len() as i32;
    let total = match spec.orientation.unwrap_or(Orientation::Horizontal) {
        Orientation::Horizontal => rect.width,
        Orientation::Vertical => rect.height,
    };
    (total - spacing * (n - 1)).max(0)
}

/// Areas of the parts of a split (without further dividing the children).
pub(crate) fn split_parts(spec: &SplitSpec, rect: Rect, spacing: i32) -> Vec<Rect> {
    let orientation = spec.orientation.unwrap_or(Orientation::Horizontal);
    let n = spec.sizes.len();
    let available = split_available(spec, rect, spacing);
    let sum: f32 = spec.sizes.iter().sum();
    let mut cumulative = 0.0;
    let mut start = 0;
    let mut out = Vec::with_capacity(n);
    for (i, size) in spec.sizes.iter().enumerate() {
        cumulative += size / sum;
        let end = if i == n - 1 {
            available
        } else {
            (cumulative * available as f32).round() as i32
        };
        let offset = start + spacing * i as i32;
        out.push(match orientation {
            Orientation::Horizontal => Rect::new(rect.x + offset, rect.y, end - start, rect.height),
            Orientation::Vertical => Rect::new(rect.x, rect.y + offset, rect.width, end - start),
        });
        start = end;
    }
    out
}

fn even(orientation: Orientation, n: usize) -> SplitSpec {
    if n <= 1 {
        return SplitSpec::default();
    }
    SplitSpec {
        orientation: Some(orientation),
        sizes: vec![1.0; n],
        children: Vec::new(),
    }
}

/// Split tree of a template with `zones` zones.
pub fn template_spec(template: Template, zones: usize) -> SplitSpec {
    match template {
        Template::Single => SplitSpec::default(),
        Template::Columns => even(Orientation::Horizontal, zones),
        Template::Rows => even(Orientation::Vertical, zones),
        Template::Grid => {
            let cols = (zones as f32).sqrt().ceil() as usize;
            let rows = zones.div_ceil(cols);
            let mut remaining = zones;
            let children: Vec<SplitSpec> = (0..rows)
                .map(|row| {
                    let in_row = remaining.div_ceil(rows - row);
                    remaining -= in_row;
                    even(Orientation::Horizontal, in_row)
                })
                .collect();
            if rows == 1 {
                return children.into_iter().next().unwrap_or_default();
            }
            SplitSpec {
                orientation: Some(Orientation::Vertical),
                sizes: vec![1.0; rows],
                children,
            }
        }
        Template::PriorityGrid => match zones {
            0 | 1 => SplitSpec::default(),
            2 => SplitSpec {
                orientation: Some(Orientation::Horizontal),
                sizes: vec![2.0, 1.0],
                children: Vec::new(),
            },
            n => {
                // Center 50%, rest distributed across the two side columns.
                let left = (n - 1).div_ceil(2);
                let right = n - 1 - left;
                SplitSpec {
                    orientation: Some(Orientation::Horizontal),
                    sizes: vec![1.0, 2.0, 1.0],
                    children: vec![
                        even(Orientation::Vertical, left),
                        SplitSpec::default(),
                        even(Orientation::Vertical, right),
                    ],
                }
            }
        },
    }
}

fn check_split(spec: &SplitSpec, name: &str, errors: &mut Vec<String>) {
    if spec.sizes.iter().any(|s| !(s.is_finite() && *s > 0.0)) {
        errors.push(format!("layout `{name}`: sizes must be positive"));
    }
    if spec.sizes.is_empty() && (!spec.children.is_empty() || spec.orientation.is_some()) {
        errors.push(format!(
            "layout `{name}`: orientation/children require sizes"
        ));
    }
    if !spec.children.is_empty() && spec.children.len() != spec.sizes.len() {
        errors.push(format!(
            "layout `{name}`: {} sizes, but {} children",
            spec.sizes.len(),
            spec.children.len()
        ));
    }
    for child in &spec.children {
        check_split(child, name, errors);
    }
}

/// All validated layouts in file order.
#[derive(Debug, Clone, PartialEq)]
pub struct Layouts(Vec<Layout>);

impl Layouts {
    pub fn parse(text: &str) -> Result<Self, ConfigError> {
        let file: LayoutsFile = toml::from_str(text)?;
        Self::validate(file)
    }

    pub fn builtin() -> Self {
        Self::parse(DEFAULT_LAYOUTS).expect("built-in layouts are valid")
    }

    pub fn validate(file: LayoutsFile) -> Result<Self, ConfigError> {
        let mut errors = Vec::new();
        let mut names = HashSet::new();
        let mut layouts = Vec::new();

        if file.layout.is_empty() {
            errors.push("at least one [[layout]] is required".to_owned());
        }
        for cfg in file.layout {
            let name = cfg.name.trim().to_owned();
            if name.is_empty() {
                errors.push("layout without a name".to_owned());
                continue;
            }
            if !names.insert(name.clone()) {
                errors.push(format!("layout `{name}` is a duplicate"));
                continue;
            }
            let shape = match (cfg.template, cfg.split, cfg.canvas) {
                (Some(template), None, None) => {
                    let zones = usize::from(cfg.zones.unwrap_or(match template {
                        Template::Single => 1,
                        Template::Columns | Template::Rows => 2,
                        Template::Grid => 4,
                        Template::PriorityGrid => 3,
                    }));
                    if !(1..=MAX_ZONES).contains(&zones) {
                        errors.push(format!("layout `{name}`: zones must be 1–{MAX_ZONES}"));
                        continue;
                    }
                    Shape::Split(template_spec(template, zones))
                }
                (None, Some(split), None) => {
                    if cfg.zones.is_some() {
                        errors.push(format!("layout `{name}`: zones only applies to template"));
                    }
                    check_split(&split, &name, &mut errors);
                    Shape::Split(split)
                }
                (None, None, Some(rects)) => {
                    if cfg.zones.is_some() {
                        errors.push(format!("layout `{name}`: zones only applies to template"));
                    }
                    if rects.is_empty() {
                        errors.push(format!("layout `{name}`: canvas is empty"));
                    }
                    for [x, y, w, h] in &rects {
                        let ok = *x >= 0.0
                            && *y >= 0.0
                            && *w > 0.0
                            && *h > 0.0
                            && x + w <= 100.0 + f32::EPSILON
                            && y + h <= 100.0 + f32::EPSILON;
                        if !ok {
                            errors.push(format!(
                                "layout `{name}`: canvas zone [{x}, {y}, {w}, {h}] is not within 0–100%"
                            ));
                        }
                    }
                    Shape::Canvas(rects)
                }
                _ => {
                    errors.push(format!(
                        "layout `{name}`: specify exactly one of template, split or canvas"
                    ));
                    continue;
                }
            };
            let layout = Layout { name, shape };
            if layout.zone_count() > MAX_ZONES {
                errors.push(format!(
                    "layout `{}`: more than {MAX_ZONES} zones",
                    layout.name
                ));
            }
            layouts.push(layout);
        }

        if errors.is_empty() {
            Ok(Layouts(layouts))
        } else {
            Err(ConfigError::Invalid(errors))
        }
    }

    pub fn get(&self, name: &str) -> Option<&Layout> {
        self.0.iter().find(|l| l.name == name)
    }

    pub fn iter(&self) -> impl Iterator<Item = &Layout> {
        self.0.iter()
    }

    pub fn len(&self) -> usize {
        self.0.len()
    }

    pub fn is_empty(&self) -> bool {
        self.0.is_empty()
    }

    /// Next/previous layout after `current` (cyclic).
    pub fn cycle(&self, current: &str, cycle: Cycle) -> &Layout {
        let n = self.0.len();
        let index = self.0.iter().position(|l| l.name == current);
        let next = match (index, cycle) {
            (None, _) => 0,
            (Some(i), Cycle::Next) => (i + 1) % n,
            (Some(i), Cycle::Prev) => (i + n - 1) % n,
        };
        &self.0[next]
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    const AREA: Rect = Rect::new(0, 0, 1000, 600);

    fn layout(toml_text: &str) -> Layout {
        let mut layouts = Layouts::parse(toml_text).unwrap();
        layouts.0.remove(0)
    }

    #[test]
    fn builtin_layouts_are_valid() {
        let layouts = Layouts::builtin();
        assert!(layouts.get("single").is_some());
        assert!(layouts.len() >= 5);
    }

    #[test]
    fn columns_with_spacing() {
        let l = layout("[[layout]]\nname = \"c\"\ntemplate = \"columns\"\nzones = 3\n");
        assert_eq!(
            l.zones(AREA, 10),
            vec![
                Rect::new(0, 0, 327, 600),
                Rect::new(337, 0, 326, 600),
                Rect::new(673, 0, 327, 600),
            ]
        );
    }

    #[test]
    fn grid_and_priority_counts() {
        for n in 1..=9u8 {
            for template in ["grid", "priority-grid", "columns", "rows"] {
                let l = layout(&format!(
                    "[[layout]]\nname = \"x\"\ntemplate = \"{template}\"\nzones = {n}\n"
                ));
                assert_eq!(l.zone_count(), usize::from(n), "{template} {n}");
                assert_eq!(l.zones(AREA, 8).len(), usize::from(n), "{template} {n}");
            }
        }
    }

    #[test]
    fn grid_of_five_is_three_plus_two() {
        let l = layout("[[layout]]\nname = \"g\"\ntemplate = \"grid\"\nzones = 5\n");
        let zones = l.zones(AREA, 0);
        assert_eq!(zones.iter().filter(|z| z.y == 0).count(), 3);
        assert_eq!(zones.iter().filter(|z| z.y == 300).count(), 2);
    }

    #[test]
    fn custom_split() {
        let l = layout(
            r#"
            [[layout]]
            name = "focus"
            split = { orientation = "horizontal", sizes = [2, 1], children = [{}, { orientation = "vertical", sizes = [1, 1] }] }
            "#,
        );
        assert_eq!(
            l.zones(Rect::new(0, 0, 900, 600), 0),
            vec![
                Rect::new(0, 0, 600, 600),
                Rect::new(600, 0, 300, 300),
                Rect::new(600, 300, 300, 300),
            ]
        );
    }

    #[test]
    fn canvas_spacing_only_between_zones() {
        let l = layout("[[layout]]\nname = \"c\"\ncanvas = [[0, 0, 50, 100], [50, 0, 50, 100]]\n");
        assert_eq!(
            l.zones(AREA, 10),
            vec![Rect::new(0, 0, 495, 600), Rect::new(505, 0, 495, 600)]
        );
    }

    #[test]
    fn cycle_wraps() {
        let layouts = Layouts::parse(
            "[[layout]]\nname = \"a\"\ntemplate = \"single\"\n[[layout]]\nname = \"b\"\ntemplate = \"columns\"\n",
        )
        .unwrap();
        assert_eq!(layouts.cycle("a", Cycle::Next).name, "b");
        assert_eq!(layouts.cycle("b", Cycle::Next).name, "a");
        assert_eq!(layouts.cycle("a", Cycle::Prev).name, "b");
        assert_eq!(layouts.cycle("gone", Cycle::Next).name, "a");
    }

    #[test]
    fn rejects_invalid_layouts() {
        let text = r#"
            [[layout]]
            name = "duplicate"
            template = "single"

            [[layout]]
            name = "duplicate"
            template = "single"

            [[layout]]
            name = "both"
            template = "rows"
            canvas = [[0, 0, 100, 100]]

            [[layout]]
            name = "children"
            split = { orientation = "vertical", sizes = [1, 1], children = [{}] }

            [[layout]]
            name = "outside"
            canvas = [[60, 0, 50, 100]]

            [[layout]]
            name = "many"
            template = "columns"
            zones = 40
        "#;
        let ConfigError::Invalid(errors) = Layouts::parse(text).unwrap_err() else {
            panic!("expected validation errors");
        };
        assert_eq!(errors.len(), 5, "{errors:#?}");
    }
}
