//! Cosmic-style tiling tree for a zone.
//!
//! A new window splits the focused window along its longer
//! side. Windows can be grouped into stacks (tabs); in
//! monocle mode one window fills the whole zone.

use serde::{Deserialize, Serialize};

use crate::action::Direction;
use crate::geometry::Rect;

/// Window identifier (on Windows the HWND as a number).
pub type WindowId = u64;

/// Smallest share of a child in its split.
const MIN_RATIO: f32 = 0.1;

/// Minimum share when moving boundaries in a split with `n` children.
/// If [`MIN_RATIO`] no longer fits for all of them (from `1 / MIN_RATIO` children,
/// e.g. after dissolving a large stack), half the equal share
/// applies – otherwise there would be no room between minimum and maximum
/// and every resize would have no effect.
fn min_ratio(n: usize) -> f32 {
    let n = n.max(1) as f32;
    if MIN_RATIO * n < 1.0 {
        MIN_RATIO
    } else {
        0.5 / n
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum Orientation {
    /// Children side by side (left → right).
    Horizontal,
    /// Children one above the other (top → bottom).
    Vertical,
}

impl Orientation {
    /// Split along the longer side.
    pub fn for_rect(rect: Rect) -> Self {
        if rect.width >= rect.height {
            Orientation::Horizontal
        } else {
            Orientation::Vertical
        }
    }

    pub fn flipped(self) -> Self {
        match self {
            Orientation::Horizontal => Orientation::Vertical,
            Orientation::Vertical => Orientation::Horizontal,
        }
    }
}

#[derive(Debug, Clone, PartialEq)]
pub enum Node {
    Window(WindowId),
    /// Windows on top of each other in the same rectangle, only `active` is visible.
    Stack {
        windows: Vec<WindowId>,
        active: usize,
    },
    Split {
        orientation: Orientation,
        children: Vec<Node>,
        /// Shares of the children, sum 1.
        ratios: Vec<f32>,
    },
}

/// How the windows of a zone are arranged.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum ZoneMode {
    /// Cosmic behavior: a new window splits the focused one along
    /// its longer side.
    #[default]
    Auto,
    /// All windows of the zone in one stack (tabs).
    Stack,
    /// All windows side by side.
    Columns,
    /// All windows one above the other.
    Rows,
}

/// Target position of a window.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Placement {
    pub id: WindowId,
    pub rect: Rect,
    /// `false` for hidden stack windows and windows behind a monocle.
    pub visible: bool,
    /// Belongs to a stack.
    pub stacked: bool,
}

#[derive(Debug, Clone, Default, PartialEq)]
pub struct Tree {
    root: Option<Node>,
    monocle: Option<WindowId>,
}

impl Tree {
    pub fn is_empty(&self) -> bool {
        self.root.is_none()
    }

    pub fn root(&self) -> Option<&Node> {
        self.root.as_ref()
    }

    pub fn contains(&self, id: WindowId) -> bool {
        self.path(id).is_some()
    }

    /// All windows in tree order (left/top first).
    pub fn windows(&self) -> Vec<WindowId> {
        fn collect(node: &Node, out: &mut Vec<WindowId>) {
            match node {
                Node::Window(id) => out.push(*id),
                Node::Stack { windows, .. } => out.extend(windows),
                Node::Split { children, .. } => children.iter().for_each(|c| collect(c, out)),
            }
        }
        let mut out = Vec::new();
        if let Some(root) = &self.root {
            collect(root, &mut out);
        }
        out
    }

    pub fn monocle(&self) -> Option<WindowId> {
        self.monocle
    }

    /// Inserts `id` next to `focus` (or next to the last window). If the
    /// target is part of a stack, the new window goes into the stack.
    pub fn insert(&mut self, id: WindowId, focus: Option<WindowId>, area: Rect, gap: i32) {
        if self.contains(id) {
            return;
        }
        self.monocle = None;
        let Some(target) = focus
            .filter(|f| self.contains(*f))
            .or_else(|| self.windows().last().copied())
        else {
            self.root = Some(Node::Window(id));
            return;
        };
        let target_rect = self
            .layout(area, gap)
            .into_iter()
            .find(|p| p.id == target)
            .map_or(area, |p| p.rect);
        let path = self.path(target).expect("target is in the tree");
        let node = self.node_mut(&path);
        match node {
            Node::Stack { windows, active } => {
                *active += 1;
                windows.insert(*active, id);
            }
            Node::Window(existing) => {
                let existing = *existing;
                *node = Node::Split {
                    orientation: Orientation::for_rect(target_rect),
                    children: vec![Node::Window(existing), Node::Window(id)],
                    ratios: vec![0.5, 0.5],
                };
            }
            Node::Split { .. } => unreachable!("path ends at a window or stack"),
        }
    }

    /// Inserts `id` on side `side` of `target`. If `target` lies in a split
    /// of the same direction, `id` becomes its sibling and shares the space
    /// with `target` (no unnecessary nesting).
    pub fn insert_beside(&mut self, id: WindowId, target: WindowId, side: Direction) -> bool {
        if self.contains(id) {
            return false;
        }
        let Some(path) = self.path(target) else {
            return false;
        };
        self.monocle = None;
        let orientation = match side {
            Direction::Left | Direction::Right => Orientation::Horizontal,
            Direction::Up | Direction::Down => Orientation::Vertical,
        };
        let before = matches!(side, Direction::Left | Direction::Up);

        // Only become a sibling as long as both halves keep the minimum share;
        // otherwise a separate split for `target` (looks the same, but keeps
        // all shares above `MIN_RATIO`).
        if let Some((&index, parent_path)) = path.split_last()
            && let Node::Split {
                orientation: parent_orientation,
                children,
                ratios,
            } = self.node_mut(parent_path)
            && *parent_orientation == orientation
            && ratios[index] / 2.0 >= MIN_RATIO
        {
            let half = ratios[index] / 2.0;
            ratios[index] = half;
            let position = if before { index } else { index + 1 };
            children.insert(position, Node::Window(id));
            ratios.insert(position, half);
            return true;
        }

        let node = self.node_mut(&path);
        let existing = std::mem::replace(node, Node::Window(id));
        let children = if before {
            vec![Node::Window(id), existing]
        } else {
            vec![existing, Node::Window(id)]
        };
        *node = Node::Split {
            orientation,
            children,
            ratios: vec![0.5, 0.5],
        };
        true
    }

    /// Puts `id` as the active window into the stack of `target` (or forms
    /// a new stack with `target`).
    pub fn insert_stacked(&mut self, id: WindowId, target: WindowId) -> bool {
        if self.contains(id) {
            return false;
        }
        let Some(path) = self.path(target) else {
            return false;
        };
        self.monocle = None;
        let node = self.node_mut(&path);
        match node {
            Node::Stack { windows, active } => {
                *active += 1;
                windows.insert(*active, id);
            }
            Node::Window(existing) => {
                *node = Node::Stack {
                    windows: vec![*existing, id],
                    active: 1,
                };
            }
            Node::Split { .. } => unreachable!("path ends at a window or stack"),
        }
        true
    }

    /// Removes `id`; empty splits collapse. `false` if not contained.
    pub fn remove(&mut self, id: WindowId) -> bool {
        let Some(path) = self.path(id) else {
            return false;
        };
        if self.monocle == Some(id) {
            self.monocle = None;
        }
        if let Node::Stack { windows, active } = self.node_mut(&path)
            && windows.len() > 1
        {
            let index = windows.iter().position(|w| *w == id).expect("in the stack");
            windows.remove(index);
            if index < *active || *active >= windows.len() {
                *active = active.saturating_sub(1);
            }
            return true;
        }
        self.remove_node(&path);
        true
    }

    /// Swaps the places of two windows.
    pub fn swap(&mut self, a: WindowId, b: WindowId) -> bool {
        if a == b || !self.contains(a) || !self.contains(b) {
            return false;
        }
        self.map_ids(|id| match id {
            id if id == a => b,
            id if id == b => a,
            id => id,
        });
        true
    }

    /// Puts the not yet contained window `new` in place of `old`.
    pub fn swap_out(&mut self, old: WindowId, new: WindowId) -> bool {
        if !self.contains(old) || self.contains(new) {
            return false;
        }
        self.map_ids(|id| if id == old { new } else { id });
        true
    }

    fn map_ids(&mut self, f: impl Fn(WindowId) -> WindowId) {
        fn visit(node: &mut Node, f: &impl Fn(WindowId) -> WindowId) {
            match node {
                Node::Window(id) => *id = f(*id),
                Node::Stack { windows, .. } => windows.iter_mut().for_each(|w| *w = f(*w)),
                Node::Split { children, .. } => children.iter_mut().for_each(|c| visit(c, f)),
            }
        }
        if let Some(root) = &mut self.root {
            visit(root, &f);
        }
        self.monocle = self.monocle.map(f);
    }

    /// Flips the split that contains `id`.
    pub fn toggle_orientation(&mut self, id: WindowId) -> bool {
        let Some(path) = self.path(id) else {
            return false;
        };
        if path.is_empty() {
            return false;
        }
        if let Node::Split { orientation, .. } = self.node_mut(&path[..path.len() - 1]) {
            *orientation = orientation.flipped();
        }
        true
    }

    /// Grows (`delta > 0`) or shrinks the share of the subtree containing
    /// `id` in its split; the siblings give up space proportionally.
    pub fn resize(&mut self, id: WindowId, delta: f32) -> bool {
        let Some(path) = self.path(id) else {
            return false;
        };
        let Some((&index, parent)) = path.split_last() else {
            return false;
        };
        let Node::Split { ratios, .. } = self.node_mut(parent) else {
            unreachable!("parents are splits");
        };
        let min = min_ratio(ratios.len());
        let n = ratios.len() as f32;
        let old = ratios[index];
        let new = clamp_from(old, old + delta, min, 1.0 - min * (n - 1.0));
        if (new - old).abs() < f32::EPSILON {
            return false;
        }
        let rest_old = 1.0 - old;
        let rest_new = 1.0 - new;
        for (i, r) in ratios.iter_mut().enumerate() {
            *r = if i == index {
                new
            } else {
                (*r * rest_new / rest_old).max(min / 2.0)
            };
        }
        normalize(ratios);
        true
    }

    /// Stack on/off. A stack is dissolved back into side-by-side windows.
    /// A single window is grouped into a stack with its window siblings in
    /// the same split (or becomes a stack on its own, into which new windows
    /// then go).
    pub fn toggle_stack(&mut self, id: WindowId, area: Rect, gap: i32) -> bool {
        let Some(path) = self.path(id) else {
            return false;
        };
        let stack_rect = self
            .layout(area, gap)
            .into_iter()
            .find(|p| p.id == id)
            .map_or(area, |p| p.rect);

        if let Node::Stack { windows, .. } = self.node_mut(&path) {
            let windows = std::mem::take(windows);
            let n = windows.len();
            *self.node_mut(&path) = if n == 1 {
                Node::Window(windows[0])
            } else {
                Node::Split {
                    orientation: Orientation::for_rect(stack_rect),
                    children: windows.into_iter().map(Node::Window).collect(),
                    ratios: vec![1.0 / n as f32; n],
                }
            };
            return true;
        }

        let Some((&index, parent_path)) = path.split_last() else {
            *self.node_mut(&path) = Node::Stack {
                windows: vec![id],
                active: 0,
            };
            return true;
        };
        let Node::Split {
            children, ratios, ..
        } = self.node_mut(parent_path)
        else {
            unreachable!("parents are splits");
        };
        let leaves: Vec<usize> = children
            .iter()
            .enumerate()
            .filter(|(_, c)| matches!(c, Node::Window(_)))
            .map(|(i, _)| i)
            .collect();
        if leaves.len() < 2 {
            children[index] = Node::Stack {
                windows: vec![id],
                active: 0,
            };
            return true;
        }

        let windows: Vec<WindowId> = leaves
            .iter()
            .map(|&i| match children[i] {
                Node::Window(w) => w,
                _ => unreachable!(),
            })
            .collect();
        let ratio: f32 = leaves.iter().map(|&i| ratios[i]).sum();
        let first = leaves[0];
        for &i in leaves.iter().skip(1).rev() {
            children.remove(i);
            ratios.remove(i);
        }
        children[first] = Node::Stack {
            active: windows.iter().position(|w| *w == id).expect("present"),
            windows,
        };
        ratios[first] = ratio;
        if children.len() == 1 {
            let only = children.pop().expect("one child");
            *self.node_mut(parent_path) = only;
        }
        true
    }

    /// Switches to the next/previous window in the stack of `id` and returns it.
    pub fn cycle_stack(&mut self, id: WindowId, forward: bool) -> Option<WindowId> {
        let path = self.path(id)?;
        let Node::Stack { windows, active } = self.node_mut(&path) else {
            return None;
        };
        let n = windows.len();
        *active = if forward {
            (*active + 1) % n
        } else {
            (*active + n - 1) % n
        };
        Some(windows[*active])
    }

    /// Makes `id` visible in its stack (e.g. after a click in the taskbar).
    pub fn activate(&mut self, id: WindowId) -> bool {
        let Some(path) = self.path(id) else {
            return false;
        };
        if let Node::Stack { windows, active } = self.node_mut(&path) {
            let index = windows.iter().position(|w| *w == id).expect("in the stack");
            let changed = *active != index;
            *active = index;
            return changed;
        }
        false
    }

    /// Moves edge `edge` of window `id` by `delta` pixels
    /// (positive = right or down), e.g. after a resize with
    /// the mouse. The nearest split in that direction that still has a
    /// neighbor on that side is adjusted. `false` if there is none (the
    /// edge lies on the border of the zone) or the minimum shares allow no
    /// further movement.
    pub fn resize_edge(
        &mut self,
        id: WindowId,
        edge: Direction,
        delta: i32,
        area: Rect,
        gap: i32,
    ) -> bool {
        if delta == 0 || self.monocle.is_some() {
            return false;
        }
        let Some(path) = self.path(id) else {
            return false;
        };
        let axis = match edge {
            Direction::Left | Direction::Right => Orientation::Horizontal,
            Direction::Up | Direction::Down => Orientation::Vertical,
        };
        let toward_end = matches!(edge, Direction::Right | Direction::Down);

        for depth in (0..path.len()).rev() {
            let (parent, index) = (&path[..depth], path[depth]);
            let rect = self.rect_at(parent, area, gap);
            let Node::Split {
                orientation,
                ratios,
                ..
            } = self.node_mut(parent)
            else {
                unreachable!("parents are splits");
            };
            if *orientation != axis {
                continue;
            }
            let n = ratios.len();
            let (before, after) = match toward_end {
                true if index + 1 < n => (index, index + 1),
                false if index > 0 => (index - 1, index),
                _ => continue,
            };
            let space = available(axis, n, rect, gap).max(1) as f32;
            let min = min_ratio(n);
            // The boundary between `before` and `after` moves by `delta`.
            let shift = clamp_from(
                0.0,
                delta as f32 / space,
                min - ratios[before],
                ratios[after] - min,
            );
            ratios[before] += shift;
            ratios[after] -= shift;
            return shift != 0.0;
        }
        false
    }

    /// Inserts `id` as the zone's mode requires (see
    /// [`ZoneMode`]); `focus` is the neighbor for `Auto`.
    pub fn insert_in_mode(
        &mut self,
        id: WindowId,
        mode: ZoneMode,
        focus: Option<WindowId>,
        area: Rect,
        gap: i32,
    ) {
        if self.contains(id) {
            return;
        }
        let orientation = match mode {
            ZoneMode::Auto => return self.insert(id, focus, area, gap),
            ZoneMode::Stack => {
                let target = focus
                    .filter(|f| self.contains(*f))
                    .or_else(|| self.windows().last().copied());
                match target {
                    Some(target) => {
                        self.insert_stacked(id, target);
                    }
                    None => {
                        self.root = Some(Node::Stack {
                            windows: vec![id],
                            active: 0,
                        });
                    }
                }
                return;
            }
            ZoneMode::Columns => Orientation::Horizontal,
            ZoneMode::Rows => Orientation::Vertical,
        };
        self.monocle = None;
        match self.root.take() {
            None => self.root = Some(Node::Window(id)),
            // Flat split in the desired direction: append, equal size.
            Some(Node::Split {
                orientation: o,
                mut children,
                ..
            }) if o == orientation => {
                children.push(Node::Window(id));
                let n = children.len();
                self.root = Some(Node::Split {
                    orientation,
                    children,
                    ratios: vec![1.0 / n as f32; n],
                });
            }
            Some(other) => {
                self.root = Some(Node::Split {
                    orientation,
                    children: vec![other, Node::Window(id)],
                    ratios: vec![0.5, 0.5],
                });
            }
        }
    }

    /// Rearranges all windows according to `mode` (order is kept); with
    /// `Stack`, `focus` becomes the visible window.
    pub fn rebuild(&mut self, mode: ZoneMode, focus: Option<WindowId>, area: Rect, gap: i32) {
        let windows = self.windows();
        self.root = None;
        self.monocle = None;
        if windows.is_empty() {
            return;
        }
        match mode {
            ZoneMode::Auto => {
                // As when opened one after another: each window splits the previous one.
                let mut previous = None;
                for id in windows {
                    self.insert(id, previous, area, gap);
                    previous = Some(id);
                }
            }
            ZoneMode::Stack => {
                let active = focus
                    .and_then(|f| windows.iter().position(|w| *w == f))
                    .unwrap_or(0);
                self.root = Some(Node::Stack { windows, active });
            }
            ZoneMode::Columns | ZoneMode::Rows if windows.len() == 1 => {
                self.root = Some(Node::Window(windows[0]));
            }
            ZoneMode::Columns | ZoneMode::Rows => {
                let n = windows.len();
                self.root = Some(Node::Split {
                    orientation: if mode == ZoneMode::Columns {
                        Orientation::Horizontal
                    } else {
                        Orientation::Vertical
                    },
                    children: windows.into_iter().map(Node::Window).collect(),
                    ratios: vec![1.0 / n as f32; n],
                });
            }
        }
    }

    pub fn toggle_monocle(&mut self, id: WindowId) -> bool {
        if !self.contains(id) {
            return false;
        }
        self.monocle = if self.monocle == Some(id) {
            None
        } else {
            Some(id)
        };
        true
    }

    /// Computes all target positions. `gap` is the gap between windows.
    pub fn layout(&self, area: Rect, gap: i32) -> Vec<Placement> {
        let mut out = Vec::new();
        let Some(root) = &self.root else {
            return out;
        };
        if let Some(monocle) = self.monocle {
            out.extend(self.windows().into_iter().map(|id| Placement {
                id,
                rect: area,
                visible: id == monocle,
                stacked: false,
            }));
            return out;
        }
        place(root, area, gap, &mut out);
        out
    }

    /// Path (child indices) to the node that directly contains `id`.
    fn path(&self, id: WindowId) -> Option<Vec<usize>> {
        fn find(node: &Node, id: WindowId, path: &mut Vec<usize>) -> bool {
            match node {
                Node::Window(w) => *w == id,
                Node::Stack { windows, .. } => windows.contains(&id),
                Node::Split { children, .. } => children.iter().enumerate().any(|(i, c)| {
                    path.push(i);
                    let found = find(c, id, path);
                    if !found {
                        path.pop();
                    }
                    found
                }),
            }
        }
        let mut path = Vec::new();
        find(self.root.as_ref()?, id, &mut path).then_some(path)
    }

    /// Area of the node at the end of `path`.
    fn rect_at(&self, path: &[usize], area: Rect, gap: i32) -> Rect {
        let mut node = self.root.as_ref().expect("tree not empty");
        let mut rect = area;
        for &i in path {
            let Node::Split {
                orientation,
                children,
                ratios,
            } = node
            else {
                unreachable!("path only leads through splits");
            };
            rect = split_rects(*orientation, ratios, rect, gap)[i];
            node = &children[i];
        }
        rect
    }

    fn node_mut(&mut self, path: &[usize]) -> &mut Node {
        let mut node = self.root.as_mut().expect("tree not empty");
        for &i in path {
            node = match node {
                Node::Split { children, .. } => &mut children[i],
                _ => unreachable!("path only leads through splits"),
            };
        }
        node
    }

    fn remove_node(&mut self, path: &[usize]) {
        let Some((&index, parent_path)) = path.split_last() else {
            self.root = None;
            return;
        };
        let parent = self.node_mut(parent_path);
        let Node::Split {
            children, ratios, ..
        } = parent
        else {
            unreachable!("parents are splits");
        };
        children.remove(index);
        ratios.remove(index);
        normalize(ratios);
        if children.len() == 1 {
            let only = children.pop().expect("one child");
            *parent = only;
        }
    }
}

/// Clamps `value` to `min..=max`, starting from the previous value `current`.
/// If `current` is already outside (e.g. a share below the minimum), the
/// range is extended up to `current`: the value never moves further out, and
/// swapped bounds do not cause a panic in [`f32::clamp`], but
/// at most yield `current`.
pub(crate) fn clamp_from(current: f32, value: f32, min: f32, max: f32) -> f32 {
    let (low, high) = (min.min(current), max.max(current));
    if low <= high {
        value.clamp(low, high)
    } else {
        // Only possible with NaN.
        current
    }
}

fn normalize(ratios: &mut [f32]) {
    let sum: f32 = ratios.iter().sum();
    if sum > 0.0 {
        ratios.iter_mut().for_each(|r| *r /= sum);
    }
}

fn place(node: &Node, rect: Rect, gap: i32, out: &mut Vec<Placement>) {
    match node {
        Node::Window(id) => out.push(Placement {
            id: *id,
            rect,
            visible: true,
            stacked: false,
        }),
        Node::Stack { windows, active } => {
            out.extend(windows.iter().enumerate().map(|(i, &id)| Placement {
                id,
                rect,
                visible: i == *active,
                stacked: true,
            }))
        }
        Node::Split {
            orientation,
            children,
            ratios,
        } => {
            for (child, child_rect) in
                children
                    .iter()
                    .zip(split_rects(*orientation, ratios, rect, gap))
            {
                place(child, child_rect, gap, out);
            }
        }
    }
}

/// Space distributed in a split after subtracting the gaps.
fn available(orientation: Orientation, n: usize, rect: Rect, gap: i32) -> i32 {
    let total = match orientation {
        Orientation::Horizontal => rect.width,
        Orientation::Vertical => rect.height,
    };
    (total - gap * (n as i32 - 1)).max(0)
}

/// Areas of the children of a split.
fn split_rects(orientation: Orientation, ratios: &[f32], rect: Rect, gap: i32) -> Vec<Rect> {
    let n = ratios.len();
    let available = available(orientation, n, rect, gap);
    // Round cumulative boundaries so that no pixels are lost.
    let mut cumulative = 0.0f32;
    let mut start = 0;
    let mut out = Vec::with_capacity(n);
    for (i, ratio) in ratios.iter().enumerate() {
        cumulative += ratio;
        let end = if i == n - 1 {
            available
        } else {
            (cumulative * available as f32).round() as i32
        };
        let offset = start + gap * i as i32;
        let extent = end - start;
        out.push(match orientation {
            Orientation::Horizontal => Rect::new(rect.x + offset, rect.y, extent, rect.height),
            Orientation::Vertical => Rect::new(rect.x, rect.y + offset, rect.width, extent),
        });
        start = end;
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    const AREA: Rect = Rect::new(0, 0, 1000, 600);
    const GAP: i32 = 10;

    fn rects(tree: &Tree) -> Vec<(WindowId, Rect)> {
        tree.layout(AREA, GAP)
            .into_iter()
            .map(|p| (p.id, p.rect))
            .collect()
    }

    fn rect_of(tree: &Tree, id: WindowId) -> Rect {
        tree.layout(AREA, GAP)
            .into_iter()
            .find(|p| p.id == id)
            .unwrap()
            .rect
    }

    /// 1 | 2 / 3 (2 above 3)
    fn three() -> Tree {
        let mut t = Tree::default();
        t.insert(1, None, AREA, GAP);
        t.insert(2, Some(1), AREA, GAP);
        t.insert(3, Some(2), AREA, GAP);
        t
    }

    #[test]
    fn single_window_fills_area() {
        let mut t = Tree::default();
        t.insert(1, None, AREA, GAP);
        assert_eq!(rects(&t), vec![(1, AREA)]);
    }

    #[test]
    fn splits_along_longer_side() {
        let t = three();
        assert_eq!(
            rects(&t),
            vec![
                (1, Rect::new(0, 0, 495, 600)),
                (2, Rect::new(505, 0, 495, 295)),
                (3, Rect::new(505, 305, 495, 295)),
            ]
        );
    }

    #[test]
    fn layout_covers_area_without_lost_pixels() {
        let mut t = Tree::default();
        for id in 1..=7 {
            t.insert(
                id,
                Some(id.saturating_sub(1)),
                Rect::new(0, 0, 1001, 601),
                7,
            );
        }
        let placements = t.layout(Rect::new(0, 0, 1001, 601), 7);
        let area: i64 = placements.iter().map(|p| p.rect.area()).sum();
        // Area + gaps make up the whole, windows do not overlap.
        for (i, a) in placements.iter().enumerate() {
            for b in &placements[i + 1..] {
                assert!(a.rect.intersection(&b.rect).is_empty(), "{a:?} {b:?}");
            }
            assert!(Rect::new(0, 0, 1001, 601).intersection(&a.rect) == a.rect);
        }
        assert!(area > 1001 * 601 * 9 / 10);
    }

    #[test]
    fn remove_collapses_splits() {
        let mut t = three();
        assert!(t.remove(2));
        assert_eq!(
            rects(&t),
            vec![
                (1, Rect::new(0, 0, 495, 600)),
                (3, Rect::new(505, 0, 495, 600))
            ]
        );
        assert!(t.remove(1));
        assert_eq!(rects(&t), vec![(3, AREA)]);
        assert!(t.remove(3));
        assert!(t.is_empty());
        assert!(!t.remove(3));
    }

    #[test]
    fn insert_without_focus_uses_last_window() {
        let mut t = three();
        t.insert(4, None, AREA, GAP);
        // 3 (495x295) is wider than tall → side by side.
        assert_eq!(rect_of(&t, 3).y, rect_of(&t, 4).y);
        assert!(rect_of(&t, 4).x > rect_of(&t, 3).x);
    }

    #[test]
    fn swap_exchanges_positions() {
        let mut t = three();
        let (r1, r3) = (rect_of(&t, 1), rect_of(&t, 3));
        assert!(t.swap(1, 3));
        assert_eq!((rect_of(&t, 3), rect_of(&t, 1)), (r1, r3));
        assert!(!t.swap(1, 99));
    }

    #[test]
    fn toggle_orientation_flips_parent() {
        let mut t = three();
        assert!(t.toggle_orientation(3));
        assert_eq!(rect_of(&t, 2).y, rect_of(&t, 3).y);
        assert!(rect_of(&t, 3).x > rect_of(&t, 2).x);
    }

    #[test]
    fn resize_changes_ratio_within_bounds() {
        let mut t = three();
        assert!(t.resize(1, 0.1));
        assert_eq!(rect_of(&t, 1).width, 594);
        for _ in 0..20 {
            t.resize(1, 0.1);
        }
        assert_eq!(rect_of(&t, 1).width, 891); // 90% of 990
        assert!(!t.resize(1, 0.1));
        assert!(t.resize(1, -0.2));
        // A single window has no split to change.
        let mut single = Tree::default();
        single.insert(1, None, AREA, GAP);
        assert!(!single.resize(1, 0.1));
    }

    #[test]
    fn stack_merges_sibling_windows_and_dissolves() {
        let mut t = three();
        assert!(t.toggle_stack(2, AREA, GAP));
        let placements = t.layout(AREA, GAP);
        let p2 = placements.iter().find(|p| p.id == 2).unwrap();
        let p3 = placements.iter().find(|p| p.id == 3).unwrap();
        assert_eq!(p2.rect, Rect::new(505, 0, 495, 600));
        assert_eq!(p2.rect, p3.rect);
        assert!(p2.visible && !p3.visible && p2.stacked);

        assert_eq!(t.cycle_stack(2, true), Some(3));
        assert_eq!(t.cycle_stack(3, true), Some(2));
        assert_eq!(t.cycle_stack(2, false), Some(3));

        // A new window with focus in the stack ends up in the stack.
        t.insert(4, Some(3), AREA, GAP);
        assert_eq!(rect_of(&t, 4), Rect::new(505, 0, 495, 600));
        assert!(t.layout(AREA, GAP).iter().any(|p| p.id == 4 && p.visible));

        assert!(t.toggle_stack(4, AREA, GAP));
        assert!(t.layout(AREA, GAP).iter().all(|p| p.visible && !p.stacked));
        assert_eq!(t.windows(), vec![1, 2, 3, 4]);
    }

    #[test]
    fn single_window_becomes_stack_of_one() {
        let mut t = Tree::default();
        t.insert(1, None, AREA, GAP);
        assert!(t.toggle_stack(1, AREA, GAP));
        t.insert(2, Some(1), AREA, GAP);
        assert_eq!(rects(&t), vec![(1, AREA), (2, AREA)]);
        assert!(t.remove(2));
        assert!(t.remove(1));
        assert!(t.is_empty());
    }

    #[test]
    fn removing_from_stack_keeps_active_valid() {
        let mut t = Tree::default();
        t.insert(1, None, AREA, GAP);
        t.toggle_stack(1, AREA, GAP);
        t.insert(2, Some(1), AREA, GAP);
        t.insert(3, Some(2), AREA, GAP);
        assert!(t.remove(3));
        assert!(t.layout(AREA, GAP).iter().any(|p| p.id == 2 && p.visible));
        assert!(t.activate(1));
        assert!(t.remove(2));
        assert_eq!(rects(&t), vec![(1, AREA)]);
    }

    #[test]
    fn insert_beside_reuses_parent_split() {
        let mut t = three();
        // 4 right of 1: root is horizontal → sibling, 1 shares its space.
        assert!(t.insert_beside(4, 1, Direction::Right));
        assert_eq!(t.windows(), vec![1, 4, 2, 3]);
        assert_eq!(rect_of(&t, 1).width, rect_of(&t, 4).width);
        assert_eq!(rect_of(&t, 2).x, 510);
        // 5 above 1: new vertical split only for 1.
        assert!(t.insert_beside(5, 1, Direction::Up));
        assert_eq!(rect_of(&t, 5).x, rect_of(&t, 1).x);
        assert!(rect_of(&t, 5).y < rect_of(&t, 1).y);
        assert!(!t.insert_beside(5, 1, Direction::Left));
        assert!(!t.insert_beside(6, 99, Direction::Left));
    }

    #[test]
    fn insert_stacked_forms_and_extends_stack() {
        let mut t = three();
        assert!(t.insert_stacked(4, 2));
        let p = t.layout(AREA, GAP);
        let visible: Vec<_> = p.iter().filter(|p| p.visible).map(|p| p.id).collect();
        assert_eq!(visible, vec![1, 4, 3]);
        assert_eq!(rect_of(&t, 4), rect_of(&t, 2));
        assert!(t.insert_stacked(5, 4));
        assert_eq!(t.windows(), vec![1, 2, 4, 5, 3]);
        assert_eq!(t.cycle_stack(5, true), Some(2));
    }

    #[test]
    fn resize_edge_moves_shared_border() {
        // 1 | 2 / 3: boundary 1|2 at x = 495..505.
        let mut t = three();
        assert!(t.resize_edge(1, Direction::Right, 99, AREA, GAP));
        assert_eq!(rect_of(&t, 1).width, 594);
        assert_eq!(rect_of(&t, 2).x, 604);
        // Left edge of 3 belongs to the same boundary (via the parent split).
        assert!(t.resize_edge(3, Direction::Left, -99, AREA, GAP));
        assert_eq!(rect_of(&t, 1).width, 495);
        // Boundary between 2 and 3.
        assert!(t.resize_edge(2, Direction::Down, 59, AREA, GAP));
        assert_eq!(rect_of(&t, 2).height, 354);
        assert_eq!(rect_of(&t, 3).y, 364);
        // Outer edges: no neighbor.
        assert!(!t.resize_edge(1, Direction::Left, 10, AREA, GAP));
        assert!(!t.resize_edge(2, Direction::Up, 10, AREA, GAP));
        assert!(!t.resize_edge(1, Direction::Right, 0, AREA, GAP));
        // Clamped to MIN_RATIO.
        assert!(t.resize_edge(1, Direction::Right, 5000, AREA, GAP));
        assert_eq!(rect_of(&t, 1).width, 891);
    }

    /// Zone with `n` equally wide windows 1..=n via a dissolved stack.
    fn dissolved_stack(n: WindowId) -> Tree {
        let mut t = Tree::default();
        t.insert(1, None, AREA, GAP);
        t.toggle_stack(1, AREA, GAP);
        for id in 2..=n {
            t.insert(id, Some(id - 1), AREA, GAP);
        }
        assert!(t.toggle_stack(1, AREA, GAP));
        t
    }

    fn ratios(tree: &Tree) -> Vec<f32> {
        match tree.root() {
            Some(Node::Split { ratios, .. }) => ratios.clone(),
            other => panic!("not a split: {other:?}"),
        }
    }

    #[test]
    fn resize_with_many_siblings_does_not_panic() {
        // 12 children: 12 · MIN_RATIO > 1, the old bounds were swapped.
        let mut t = dissolved_stack(12);
        assert_eq!(t.windows().len(), 12);
        assert!(t.resize(1, 0.1));
        assert!(t.resize(1, -0.2));
        assert!(t.resize(1, 5.0));
        assert!(t.resize(1, -5.0));
        let sum: f32 = ratios(&t).iter().sum();
        assert!((sum - 1.0).abs() < 1e-4);
        assert!(t.layout(AREA, GAP).iter().all(|p| p.rect.width > 0));
    }

    #[test]
    fn resize_works_with_many_siblings() {
        // Previously, with ≥ 10 children, minimum = maximum = 1/n, so resizing
        // via keyboard had no effect.
        for n in [10, 12, 25] {
            let mut t = dissolved_stack(n);
            let equal = 1.0 / n as f32;
            assert!(t.resize(1, 0.1), "n = {n}");
            assert!(ratios(&t)[0] > equal, "n = {n}: {:?}", ratios(&t));
            assert!(t.resize(1, -1.0), "n = {n}");
            let shrunk = ratios(&t)[0];
            assert!(shrunk < equal, "n = {n}: {shrunk}");
            // Never below half the equal share.
            assert!(shrunk >= 0.5 / n as f32 - 1e-4, "n = {n}: {shrunk}");
            let sum: f32 = ratios(&t).iter().sum();
            assert!((sum - 1.0).abs() < 1e-4);
        }
        // Few children: MIN_RATIO is still the limit.
        let mut t = dissolved_stack(3);
        assert!(t.resize(1, -1.0));
        assert!((ratios(&t)[0] - MIN_RATIO).abs() < 1e-4);
    }

    #[test]
    fn resize_edge_below_min_ratio_does_not_panic() {
        // Shares below MIN_RATIO (1/12): the old bounds were swapped.
        // The minimum share here is half the equal share, so the
        // boundary can be moved in both directions.
        let mut t = dissolved_stack(12);
        let before = ratios(&t);
        assert!(t.resize_edge(1, Direction::Right, 10, AREA, GAP));
        assert!(ratios(&t)[0] > before[0]);
        assert!(t.resize_edge(2, Direction::Left, -5000, AREA, GAP));
        let after = ratios(&t);
        assert!((after[0] - 0.5 / 12.0).abs() < 1e-4, "{after:?}");
        let sum: f32 = after.iter().sum();
        assert!((sum - 1.0).abs() < 1e-4);

        // Too-small shares may grow, but not shrink further.
        let mut t = Tree {
            root: Some(Node::Split {
                orientation: Orientation::Horizontal,
                children: (1..=3).map(Node::Window).collect(),
                ratios: vec![0.0625, 0.0625, 0.875],
            }),
            monocle: None,
        };
        assert!(!t.resize_edge(1, Direction::Right, 10, AREA, GAP));
        assert!(!t.resize_edge(2, Direction::Left, -99, AREA, GAP));
        assert!(t.resize_edge(2, Direction::Right, 99, AREA, GAP));
        assert!(ratios(&t)[1] > 0.0625);
    }

    #[test]
    fn insert_beside_keeps_min_ratio() {
        // Previously: 0.5 → 0.25 → 0.125 → 0.0625 in the same split.
        let mut t = Tree::default();
        t.insert(1, None, AREA, GAP);
        for id in 2..=8 {
            assert!(t.insert_beside(id, 1, Direction::Right));
        }
        fn check(node: &Node) {
            if let Node::Split {
                children, ratios, ..
            } = node
            {
                assert!(ratios.iter().all(|r| *r >= MIN_RATIO), "{ratios:?}");
                children.iter().for_each(check);
            }
        }
        check(t.root().unwrap());
        assert_eq!(t.windows(), vec![1, 8, 7, 6, 5, 4, 3, 2]);
        assert!(t.resize_edge(1, Direction::Right, 5, AREA, GAP));
    }

    fn visible(t: &Tree) -> Vec<WindowId> {
        t.layout(AREA, GAP)
            .into_iter()
            .filter(|p| p.visible)
            .map(|p| p.id)
            .collect()
    }

    #[test]
    fn insert_in_mode_follows_zone_mode() {
        // Stack: all windows in the same rectangle, the newest visible.
        let mut t = Tree::default();
        for id in 1..=3 {
            t.insert_in_mode(id, ZoneMode::Stack, None, AREA, GAP);
        }
        assert!(rects(&t).iter().all(|(_, r)| *r == AREA));
        assert_eq!(visible(&t), vec![3]);

        // Columns: flat side by side, equally wide.
        let mut t = Tree::default();
        for id in 1..=3 {
            t.insert_in_mode(id, ZoneMode::Columns, Some(1), AREA, GAP);
        }
        let r = rects(&t);
        assert!(r.iter().all(|(_, r)| r.y == 0 && r.height == 600));
        assert_eq!(
            r.iter().map(|(_, r)| r.width).collect::<Vec<_>>(),
            vec![327, 326, 327]
        );

        // Rows in a wide area: still one above the other.
        let mut t = Tree::default();
        for id in 1..=2 {
            t.insert_in_mode(id, ZoneMode::Rows, None, AREA, GAP);
        }
        assert_eq!(
            rects(&t),
            vec![
                (1, Rect::new(0, 0, 1000, 295)),
                (2, Rect::new(0, 305, 1000, 295))
            ]
        );
    }

    #[test]
    fn rebuild_switches_arrangement_and_keeps_order() {
        let mut t = three();
        t.rebuild(ZoneMode::Stack, Some(2), AREA, GAP);
        assert_eq!(t.windows(), vec![1, 2, 3]);
        assert_eq!(visible(&t), vec![2]);

        t.rebuild(ZoneMode::Rows, None, AREA, GAP);
        assert!(rects(&t).iter().all(|(_, r)| r.width == 1000));
        assert_eq!(visible(&t), vec![1, 2, 3]);

        t.rebuild(ZoneMode::Auto, None, AREA, GAP);
        assert_eq!(rects(&t), rects(&three()));

        let mut empty = Tree::default();
        empty.rebuild(ZoneMode::Stack, None, AREA, GAP);
        assert!(empty.is_empty());
    }

    #[test]
    fn monocle_fills_area_and_hides_others() {
        let mut t = three();
        assert!(t.toggle_monocle(2));
        let placements = t.layout(AREA, GAP);
        assert!(placements.iter().all(|p| p.rect == AREA));
        assert_eq!(
            placements
                .iter()
                .filter(|p| p.visible)
                .map(|p| p.id)
                .collect::<Vec<_>>(),
            vec![2]
        );
        assert!(t.toggle_monocle(2));
        assert_eq!(t.monocle(), None);
        // New windows end monocle mode.
        t.toggle_monocle(2);
        t.insert(4, Some(2), AREA, GAP);
        assert_eq!(t.monocle(), None);
    }
}
