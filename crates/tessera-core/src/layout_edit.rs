//! Editing split layouts for the zone editor: splitting and removing zones,
//! moving dividers. Works on [`SplitSpec`] and computes the same pixels
//! as [`Layout::zones`](crate::layout::Layout::zones).

use crate::geometry::Rect;
use crate::layout::{SplitSpec, split_available, split_parts};
use crate::tree::{Orientation, clamp_from};

/// Smallest share of a part in its split (in percent).
const MIN_SHARE: f32 = 5.0;

/// Path (child indices) to a node in the split tree.
pub type Path = Vec<usize>;

/// Boundary between part `index` and `index + 1` of the split at `path`.
#[derive(Debug, Clone, PartialEq)]
pub struct Divider {
    pub path: Path,
    pub index: usize,
    pub orientation: Orientation,
    /// The gap strip between the two parts (width `spacing`).
    pub rect: Rect,
}

impl SplitSpec {
    /// Explicit form: every split has as many `children` as `sizes`,
    /// sizes in percent.
    pub fn normalized(&self) -> SplitSpec {
        if self.sizes.is_empty() {
            return SplitSpec::default();
        }
        let sum: f32 = self.sizes.iter().sum();
        SplitSpec {
            orientation: Some(self.orientation.unwrap_or(Orientation::Horizontal)),
            sizes: self.sizes.iter().map(|s| s / sum * 100.0).collect(),
            children: (0..self.sizes.len())
                .map(|i| {
                    self.children
                        .get(i)
                        .map_or_else(SplitSpec::default, Self::normalized)
                })
                .collect(),
        }
    }

    /// Compact form for saving: splits with only one part are dropped,
    /// pure zone children are omitted, sizes rounded to 0.1%.
    pub fn compacted(&self) -> SplitSpec {
        if self.sizes.is_empty() {
            return SplitSpec::default();
        }
        if self.sizes.len() == 1 {
            return self
                .children
                .first()
                .map_or_else(SplitSpec::default, Self::compacted);
        }
        let children: Vec<SplitSpec> = self.children.iter().map(Self::compacted).collect();
        let all_zones = children.iter().all(|c| c.sizes.is_empty());
        SplitSpec {
            orientation: self.orientation,
            sizes: self
                .sizes
                .iter()
                .map(|s| (s * 10.0).round() / 10.0)
                .collect(),
            children: if all_zones { Vec::new() } else { children },
        }
    }

    /// All zones with path, in the same order as `Layout::zones`.
    /// Expects the [`normalized`](Self::normalized) form.
    pub fn zones_with_paths(&self, area: Rect, spacing: i32) -> Vec<(Path, Rect)> {
        fn walk(
            spec: &SplitSpec,
            rect: Rect,
            spacing: i32,
            path: &mut Path,
            out: &mut Vec<(Path, Rect)>,
        ) {
            if spec.sizes.is_empty() {
                out.push((path.clone(), rect));
                return;
            }
            for (i, part) in split_parts(spec, rect, spacing).into_iter().enumerate() {
                path.push(i);
                walk(&spec.children[i], part, spacing, path, out);
                path.pop();
            }
        }
        let mut out = Vec::new();
        walk(self, area, spacing, &mut Vec::new(), &mut out);
        out
    }

    /// All dividers between adjacent parts. Expects the normalized form.
    pub fn dividers(&self, area: Rect, spacing: i32) -> Vec<Divider> {
        fn walk(
            spec: &SplitSpec,
            rect: Rect,
            spacing: i32,
            path: &mut Path,
            out: &mut Vec<Divider>,
        ) {
            if spec.sizes.is_empty() {
                return;
            }
            let orientation = spec.orientation.unwrap_or(Orientation::Horizontal);
            let parts = split_parts(spec, rect, spacing);
            for (i, pair) in parts.windows(2).enumerate() {
                let (a, b) = (pair[0], pair[1]);
                out.push(Divider {
                    path: path.clone(),
                    index: i,
                    orientation,
                    rect: match orientation {
                        Orientation::Horizontal => Rect::from_ltrb(a.right(), a.y, b.x, a.bottom()),
                        Orientation::Vertical => Rect::from_ltrb(a.x, a.bottom(), a.right(), b.y),
                    },
                });
            }
            for (i, part) in parts.into_iter().enumerate() {
                path.push(i);
                walk(&spec.children[i], part, spacing, path, out);
                path.pop();
            }
        }
        let mut out = Vec::new();
        walk(self, area, spacing, &mut Vec::new(), &mut out);
        out
    }

    /// Splits the zone at `path` at `fraction` (0–1) of its width or height.
    /// If it already lies in a split of the same direction, it gets a sibling
    /// there instead of a new nested split.
    pub fn split_zone(&mut self, path: &[usize], orientation: Orientation, fraction: f32) -> bool {
        let fraction = fraction.clamp(0.05, 0.95);
        if !self.node(path).is_some_and(|n| n.sizes.is_empty()) {
            return false;
        }
        if let Some((&index, parent_path)) = path.split_last()
            && let Some(parent) = self.node_mut(parent_path)
            && parent.orientation == Some(orientation)
        {
            let size = parent.sizes[index];
            parent.sizes[index] = size * fraction;
            parent.sizes.insert(index + 1, size * (1.0 - fraction));
            parent.children.insert(index + 1, SplitSpec::default());
            return true;
        }
        let Some(zone) = self.node_mut(path) else {
            return false;
        };
        *zone = SplitSpec {
            orientation: Some(orientation),
            sizes: vec![fraction * 100.0, (1.0 - fraction) * 100.0],
            children: vec![SplitSpec::default(), SplitSpec::default()],
        };
        true
    }

    /// Removes the zone at `path`; its space goes to the neighbors. A split
    /// with only one part left is replaced by that part. The last zone
    /// remains.
    pub fn remove_zone(&mut self, path: &[usize]) -> bool {
        let Some((&index, parent_path)) = path.split_last() else {
            return false;
        };
        let Some(parent) = self.node_mut(parent_path) else {
            return false;
        };
        if index >= parent.sizes.len() || !parent.children[index].sizes.is_empty() {
            return false;
        }
        parent.sizes.remove(index);
        parent.children.remove(index);
        if parent.sizes.len() == 1 {
            let only = parent.children.pop().expect("one child");
            *parent = only;
        }
        true
    }

    /// Moves divider `index` of the split at `path` by `delta` pixels
    /// (positive = right/down). Parts below `MIN_SHARE` (e.g. from the
    /// configuration) may grow, but not shrink further.
    pub fn move_divider(
        &mut self,
        path: &[usize],
        index: usize,
        delta: i32,
        area: Rect,
        spacing: i32,
    ) -> bool {
        let Some(rect) = self.node_rect(path, area, spacing) else {
            return false;
        };
        let Some(split) = self.node_mut(path) else {
            return false;
        };
        if index + 1 >= split.sizes.len() {
            return false;
        }
        let sum: f32 = split.sizes.iter().sum();
        let available = split_available(split, rect, spacing).max(1) as f32;
        let min = MIN_SHARE / 100.0 * sum;
        let shift = clamp_from(
            0.0,
            delta as f32 / available * sum,
            min - split.sizes[index],
            split.sizes[index + 1] - min,
        );
        split.sizes[index] += shift;
        split.sizes[index + 1] -= shift;
        shift.abs() > f32::EPSILON
    }

    fn node(&self, path: &[usize]) -> Option<&SplitSpec> {
        path.iter().try_fold(self, |node, &i| node.children.get(i))
    }

    fn node_mut(&mut self, path: &[usize]) -> Option<&mut SplitSpec> {
        path.iter()
            .try_fold(self, |node, &i| node.children.get_mut(i))
    }

    fn node_rect(&self, path: &[usize], area: Rect, spacing: i32) -> Option<Rect> {
        let mut node = self;
        let mut rect = area;
        for &i in path {
            rect = *split_parts(node, rect, spacing).get(i)?;
            node = node.children.get(i)?;
        }
        Some(rect)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::layout::Layouts;

    const AREA: Rect = Rect::new(0, 0, 1000, 600);

    fn single() -> SplitSpec {
        SplitSpec::default()
    }

    fn rects(spec: &SplitSpec) -> Vec<Rect> {
        spec.zones_with_paths(AREA, 10)
            .into_iter()
            .map(|(_, r)| r)
            .collect()
    }

    #[test]
    fn normalized_matches_layout_zones() {
        let layouts = Layouts::builtin();
        for layout in layouts.iter() {
            let Some(spec) = layout.split() else { continue };
            assert_eq!(
                rects(&spec.normalized()),
                layout.zones(AREA, 10),
                "{}",
                layout.name
            );
        }
    }

    #[test]
    fn split_and_merge_zones() {
        let mut spec = single();
        assert!(spec.split_zone(&[], Orientation::Horizontal, 0.5));
        assert_eq!(
            rects(&spec),
            vec![Rect::new(0, 0, 495, 600), Rect::new(505, 0, 495, 600)]
        );
        // Same direction → sibling instead of nesting.
        assert!(spec.split_zone(&[1], Orientation::Horizontal, 0.5));
        assert_eq!(spec.sizes.len(), 3);
        assert!(spec.children.iter().all(|c| c.sizes.is_empty()));
        // Other direction → new split.
        assert!(spec.split_zone(&[0], Orientation::Vertical, 0.25));
        assert_eq!(spec.children[0].sizes.len(), 2);
        assert_eq!(rects(&spec).len(), 4);
        // A split is not a zone.
        assert!(!spec.split_zone(&[0], Orientation::Vertical, 0.5));

        assert!(spec.remove_zone(&[0, 1]));
        assert!(
            spec.children[0].sizes.is_empty(),
            "split with one part is dropped"
        );
        assert!(spec.remove_zone(&[2]));
        assert!(spec.remove_zone(&[1]));
        assert_eq!(spec, single());
        assert!(!spec.remove_zone(&[]));
    }

    #[test]
    fn dividers_and_moving_them() {
        let mut spec = single();
        spec.split_zone(&[], Orientation::Horizontal, 0.5);
        spec.split_zone(&[1], Orientation::Vertical, 0.5);
        let dividers = spec.dividers(AREA, 10);
        assert_eq!(dividers.len(), 2);
        assert_eq!(dividers[0].rect, Rect::new(495, 0, 10, 600));
        assert_eq!(dividers[1].path, vec![1]);
        assert_eq!(dividers[1].rect, Rect::new(505, 295, 495, 10));

        assert!(spec.move_divider(&[], 0, 99, AREA, 10));
        assert_eq!(rects(&spec)[0].width, 594);
        // Clamped to MIN_SHARE.
        spec.move_divider(&[], 0, 5000, AREA, 10);
        assert_eq!(rects(&spec)[0].width, 941);
        assert!(!spec.move_divider(&[], 1, 10, AREA, 10));
    }

    #[test]
    fn moving_divider_between_tiny_parts_does_not_panic() {
        // 5% split into 2.5% + 2.5%: both below MIN_SHARE, previously swapped bounds.
        let mut spec = single();
        spec.split_zone(&[], Orientation::Horizontal, 0.05);
        spec.split_zone(&[0], Orientation::Horizontal, 0.5);
        assert_eq!(spec.sizes.len(), 3);
        let before = spec.sizes.clone();
        assert!(!spec.move_divider(&[], 0, 10, AREA, 10));
        assert!(!spec.move_divider(&[], 0, -10, AREA, 10));
        assert_eq!(spec.sizes, before);
        // The too-small part may grow at the expense of its large neighbor.
        assert!(!spec.move_divider(&[], 1, -50, AREA, 10));
        assert!(spec.move_divider(&[], 1, 50, AREA, 10));
        assert!(spec.sizes[1] > before[1]);
    }

    #[test]
    fn compacted_roundtrip() {
        let mut spec = single();
        spec.split_zone(&[], Orientation::Horizontal, 2.0 / 3.0);
        spec.split_zone(&[1], Orientation::Vertical, 0.5);
        let compact = spec.compacted();
        assert_eq!(compact.sizes, vec![66.7, 33.3]);
        assert!(compact.children[0].sizes.is_empty());
        assert!(compact.children[1].children.is_empty());
        assert_eq!(rects(&compact.normalized()).len(), 3);
    }
}
