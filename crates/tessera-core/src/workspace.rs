//! Workspace of a monitor on a virtual desktop: the zones
//! of a layout, each with its own tiling tree.

use crate::action::Direction;
use crate::geometry::Rect;
use crate::nav;
use crate::tree::{Placement, Tree, WindowId, ZoneMode};

#[derive(Debug, Clone, Default, PartialEq)]
pub struct Zone {
    pub rect: Rect,
    pub tree: Tree,
    /// How new windows are arranged in this zone.
    pub mode: ZoneMode,
}

/// Where a dragged window lands.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct DropTarget {
    pub zone: usize,
    /// Window under the mouse pointer and the side at which it is inserted.
    pub beside: Option<(WindowId, DropSide)>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum DropSide {
    Edge(Direction),
    /// Center: into the stack of the target window.
    Stack,
}

#[derive(Debug, Clone, PartialEq)]
pub struct Workspace {
    /// Name of the layout the zones come from.
    pub layout: String,
    zones: Vec<Zone>,
    /// Gap between windows in pixels.
    gap: i32,
    /// Paused: windows are still sorted in, but not positioned.
    pub paused: bool,
    /// Last focused window; new windows go next to it.
    pub last_focus: Option<WindowId>,
}

impl Workspace {
    pub fn new(layout: impl Into<String>, zones: &[Rect], gap: i32) -> Self {
        Self {
            layout: layout.into(),
            zones: zones
                .iter()
                .map(|rect| Zone {
                    rect: *rect,
                    tree: Tree::default(),
                    mode: ZoneMode::Auto,
                })
                .collect(),
            gap,
            paused: false,
            last_focus: None,
        }
    }

    /// One zone covering the whole area (pure auto-tiling).
    pub fn single(area: Rect, gap: i32) -> Self {
        Self::new("single", &[area], gap)
    }

    pub fn zones(&self) -> &[Zone] {
        &self.zones
    }

    pub fn gap(&self) -> i32 {
        self.gap
    }

    /// New zone areas. With the same zone count the trees are kept;
    /// otherwise the windows are distributed in order across the new zones.
    pub fn set_zones(&mut self, rects: &[Rect], gap: i32) {
        self.gap = gap;
        if rects.len() == self.zones.len() {
            for (zone, rect) in self.zones.iter_mut().zip(rects) {
                zone.rect = *rect;
            }
            return;
        }
        let windows = self.windows();
        self.zones = rects
            .iter()
            .map(|rect| Zone {
                rect: *rect,
                tree: Tree::default(),
                mode: ZoneMode::Auto,
            })
            .collect();
        let (m, n) = (windows.len(), self.zones.len());
        for (i, id) in windows.into_iter().enumerate() {
            // Few windows: one per zone. Many: contiguous groups.
            let index = if m <= n { i } else { i * n / m };
            let zone = &mut self.zones[index];
            zone.tree
                .insert_in_mode(id, zone.mode, None, zone.rect, gap);
        }
    }

    /// Sets the mode of a zone and rearranges its windows accordingly;
    /// `focus` stays visible in a stack.
    pub fn set_zone_mode(&mut self, zone: usize, mode: ZoneMode, focus: Option<WindowId>) {
        let gap = self.gap;
        if let Some(zone) = self.zones.get_mut(zone) {
            zone.mode = mode;
            zone.tree.rebuild(mode, focus, zone.rect, gap);
        }
    }

    /// Applies saved zone modes if the zone count matches (the
    /// layout may have changed since).
    pub fn restore_modes(&mut self, modes: &[ZoneMode]) {
        if modes.len() != self.zones.len() {
            return;
        }
        for (index, mode) in modes.iter().enumerate() {
            self.set_zone_mode(index, *mode, None);
        }
    }

    pub fn contains(&self, id: WindowId) -> bool {
        self.zone_of(id).is_some()
    }

    pub fn zone_of(&self, id: WindowId) -> Option<usize> {
        self.zones.iter().position(|z| z.tree.contains(id))
    }

    pub fn windows(&self) -> Vec<WindowId> {
        self.zones.iter().flat_map(|z| z.tree.windows()).collect()
    }

    pub fn is_empty(&self) -> bool {
        self.zones.iter().all(|z| z.tree.is_empty())
    }

    /// Inserts a window: into `zone`, otherwise into the zone of the last
    /// focused window, otherwise into the first. `next_to` is the neighbor in
    /// the tree (default: last focused window).
    pub fn insert(&mut self, id: WindowId, zone: Option<usize>, next_to: Option<WindowId>) {
        if self.contains(id) {
            return;
        }
        let focus = next_to.or(self.last_focus);
        let index = zone
            .filter(|z| *z < self.zones.len())
            .or_else(|| focus.and_then(|f| self.zone_of(f)))
            .unwrap_or(0);
        let gap = self.gap;
        let zone = &mut self.zones[index];
        zone.tree
            .insert_in_mode(id, zone.mode, focus, zone.rect, gap);
    }

    pub fn remove(&mut self, id: WindowId) -> bool {
        if self.last_focus == Some(id) {
            self.last_focus = None;
        }
        self.zones.iter_mut().any(|z| z.tree.remove(id))
    }

    /// Moves a (contained or new) window into zone `zone`.
    pub fn move_to_zone(&mut self, id: WindowId, zone: usize) -> bool {
        if zone >= self.zones.len() || self.zone_of(id) == Some(zone) {
            return false;
        }
        self.remove(id);
        let gap = self.gap;
        let target = &mut self.zones[zone];
        target
            .tree
            .insert_in_mode(id, target.mode, None, target.rect, gap);
        true
    }

    pub fn placements(&self) -> Vec<Placement> {
        self.zones
            .iter()
            .flat_map(|z| z.tree.layout(z.rect, self.gap))
            .collect()
    }

    pub fn rect_of(&self, id: WindowId) -> Option<Rect> {
        self.placements()
            .into_iter()
            .find(|p| p.id == id)
            .map(|p| p.rect)
    }

    /// Visible neighbor in direction `dir` within this workspace.
    pub fn neighbor(&self, id: WindowId, dir: Direction) -> Option<WindowId> {
        let placements = self.placements();
        let from = placements.iter().find(|p| p.id == id)?.rect;
        nav::neighbor(
            from,
            dir,
            placements
                .iter()
                .filter(|p| p.visible && p.id != id)
                .map(|p| (p.id, p.rect)),
        )
    }

    /// Swaps with the neighbor in direction `dir` (also across zones).
    /// `false` if there is no neighbor there.
    pub fn swap_dir(&mut self, id: WindowId, dir: Direction) -> bool {
        let Some(other) = self.neighbor(id, dir) else {
            return false;
        };
        self.swap(id, other)
    }

    /// Moves a window in direction `dir`. If a window of the same zone comes
    /// first, the two swap places. If another zone comes first (empty or
    /// not), the window moves into it and is sorted in there
    /// automatically. `false` if nothing follows in that direction
    /// (edge of the workspace).
    pub fn move_dir(&mut self, id: WindowId, dir: Direction) -> bool {
        #[derive(Clone, Copy)]
        enum Target {
            Window(WindowId),
            Zone(usize),
        }
        let Some(own) = self.zone_of(id) else {
            return false;
        };
        let zone = &self.zones[own];
        let placements = zone.tree.layout(zone.rect, self.gap);
        let Some(from) = placements.iter().find(|p| p.id == id).map(|p| p.rect) else {
            return false;
        };
        let same_zone = placements
            .iter()
            .filter(|p| p.visible && p.id != id)
            .map(|p| (Target::Window(p.id), p.rect));
        // Other zones count as a whole, not via their windows.
        let other_zones = self
            .zones
            .iter()
            .enumerate()
            .filter(|(i, _)| *i != own)
            .map(|(i, z)| (Target::Zone(i), z.rect));
        match nav::neighbor(from, dir, same_zone.chain(other_zones)) {
            Some(Target::Window(other)) => self.swap(id, other),
            Some(Target::Zone(zone)) => self.move_to_zone(id, zone),
            None => false,
        }
    }

    pub fn swap(&mut self, a: WindowId, b: WindowId) -> bool {
        let (Some(za), Some(zb)) = (self.zone_of(a), self.zone_of(b)) else {
            return false;
        };
        if za == zb {
            return self.zones[za].tree.swap(a, b);
        }
        // Across zones: placeholder swap across both trees.
        const PLACEHOLDER: WindowId = WindowId::MAX;
        let ok = self.zones[za].tree.swap_out(a, PLACEHOLDER)
            && self.zones[zb].tree.swap_out(b, a)
            && self.zones[za].tree.swap_out(PLACEHOLDER, b);
        debug_assert!(ok);
        ok
    }

    /// Puts `new`, which is not in this workspace, in the place of `old`
    /// (for swapping windows across workspaces).
    pub fn replace(&mut self, old: WindowId, new: WindowId) -> bool {
        if self.contains(new) {
            return false;
        }
        if self.last_focus == Some(old) {
            self.last_focus = None;
        }
        self.zones
            .iter_mut()
            .find(|z| z.tree.contains(old))
            .is_some_and(|z| z.tree.swap_out(old, new))
    }

    /// Zone under the point; for overlapping zones the smallest.
    pub fn zone_at(&self, x: i32, y: i32) -> Option<usize> {
        self.zones
            .iter()
            .enumerate()
            .filter(|(_, z)| z.rect.contains_point(x, y))
            .min_by_key(|(_, z)| z.rect.area())
            .map(|(i, _)| i)
    }

    /// Target for a dragged window `dragged` at point `(x, y)`.
    pub fn drop_target(&self, x: i32, y: i32, dragged: WindowId) -> Option<DropTarget> {
        let zone = self.zone_at(x, y)?;
        let z = &self.zones[zone];
        let beside = z
            .tree
            .layout(z.rect, self.gap)
            .into_iter()
            .filter(|p| p.visible && p.id != dragged)
            .min_by_key(|p| {
                // Window under the pointer, otherwise the nearest one.
                let (cx, cy) = p.rect.center();
                (
                    !p.rect.contains_point(x, y),
                    i64::from(cx - x).pow(2) + i64::from(cy - y).pow(2),
                )
            })
            .map(|p| (p.id, drop_side(p.rect, x, y)));
        Some(DropTarget { zone, beside })
    }

    /// Inserts `id` at the target (removing it from its old place first).
    pub fn insert_at(&mut self, id: WindowId, target: DropTarget) {
        if target.zone >= self.zones.len() {
            return;
        }
        self.remove(id);
        let gap = self.gap;
        let zone = &mut self.zones[target.zone];
        let inserted = match target.beside {
            Some((other, DropSide::Edge(dir))) => zone.tree.insert_beside(id, other, dir),
            Some((other, DropSide::Stack)) => zone.tree.insert_stacked(id, other),
            None => false,
        };
        if !inserted {
            zone.tree
                .insert_in_mode(id, zone.mode, None, zone.rect, gap);
        }
    }

    /// Where `id` would end up after [`insert_at`](Self::insert_at).
    pub fn preview(&self, id: WindowId, target: DropTarget) -> Option<Rect> {
        let mut copy = self.clone();
        copy.insert_at(id, target);
        copy.rect_of(id)
    }

    /// Applies a resize done with the mouse: every edge that moved from
    /// `before` to `after` moves the matching boundary in the tree.
    pub fn resize_edges(&mut self, id: WindowId, before: Rect, after: Rect) -> bool {
        /// Smaller deviations are rounding, not dragging.
        const THRESHOLD: i32 = 2;
        let edges = [
            (Direction::Left, after.x - before.x),
            (Direction::Right, after.right() - before.right()),
            (Direction::Up, after.y - before.y),
            (Direction::Down, after.bottom() - before.bottom()),
        ];
        self.with_tree(id, |tree, area, gap| {
            edges
                .into_iter()
                .filter(|(_, delta)| delta.abs() > THRESHOLD)
                .fold(false, |changed, (edge, delta)| {
                    tree.resize_edge(id, edge, delta, area, gap) || changed
                })
        })
        .unwrap_or(false)
    }

    /// Runs `f` on the tree containing `id`.
    pub fn with_tree<R>(
        &mut self,
        id: WindowId,
        f: impl FnOnce(&mut Tree, Rect, i32) -> R,
    ) -> Option<R> {
        let gap = self.gap;
        let zone = self.zones.iter_mut().find(|z| z.tree.contains(id))?;
        Some(f(&mut zone.tree, zone.rect, gap))
    }
}

/// Middle third = stack, otherwise the nearest edge.
fn drop_side(rect: Rect, x: i32, y: i32) -> DropSide {
    let fx = (x - rect.x) as f32 / rect.width.max(1) as f32;
    let fy = (y - rect.y) as f32 / rect.height.max(1) as f32;
    let middle = 1.0 / 3.0..2.0 / 3.0;
    if middle.contains(&fx) && middle.contains(&fy) {
        return DropSide::Stack;
    }
    let edges = [
        (fx, Direction::Left),
        (1.0 - fx, Direction::Right),
        (fy, Direction::Up),
        (1.0 - fy, Direction::Down),
    ];
    let (_, dir) = edges
        .into_iter()
        .min_by(|a, b| a.0.total_cmp(&b.0))
        .expect("four edges");
    DropSide::Edge(dir)
}

#[cfg(test)]
mod tests {
    use super::*;

    const AREA: Rect = Rect::new(0, 0, 1000, 600);
    const LEFT: Rect = Rect::new(0, 0, 500, 600);
    const RIGHT: Rect = Rect::new(500, 0, 500, 600);

    fn two_zones() -> Workspace {
        Workspace::new("columns-2", &[LEFT, RIGHT], 10)
    }

    #[test]
    fn new_windows_go_next_to_last_focus() {
        let mut ws = Workspace::single(AREA, 10);
        ws.insert(1, None, None);
        ws.insert(2, None, None);
        ws.last_focus = Some(1);
        ws.insert(3, None, None);
        // 1 was split (495x600 → one above the other), 2 stays on the right.
        let r1 = ws.rect_of(1).unwrap();
        let r3 = ws.rect_of(3).unwrap();
        assert_eq!(r1.x, r3.x);
        assert!(r3.y > r1.y);
        assert_eq!(ws.rect_of(2).unwrap().x, 505);
    }

    #[test]
    fn swap_dir_and_neighbor() {
        let mut ws = Workspace::single(AREA, 10);
        ws.insert(1, None, None);
        ws.insert(2, None, Some(1));
        assert_eq!(ws.neighbor(1, Direction::Right), Some(2));
        assert_eq!(ws.neighbor(1, Direction::Left), None);
        assert!(ws.swap_dir(1, Direction::Right));
        assert_eq!(ws.rect_of(1).unwrap().x, 505);
        assert!(!ws.swap_dir(1, Direction::Right));
    }

    #[test]
    fn swap_across_zones() {
        let mut ws = two_zones();
        ws.insert(1, Some(0), None);
        ws.insert(2, Some(1), None);
        assert_eq!(ws.zone_of(2), Some(1));
        assert!(ws.swap(1, 2));
        assert_eq!((ws.zone_of(1), ws.zone_of(2)), (Some(1), Some(0)));
        assert_eq!(ws.rect_of(1), Some(RIGHT));
    }

    #[test]
    fn replace_takes_over_the_place_of_another_window() {
        let mut ws = two_zones();
        ws.insert(1, Some(0), None);
        ws.insert(2, Some(1), None);
        ws.last_focus = Some(2);
        assert!(ws.replace(2, 3));
        assert_eq!((ws.zone_of(2), ws.zone_of(3)), (None, Some(1)));
        assert_eq!(ws.rect_of(3), Some(RIGHT));
        assert_eq!(ws.last_focus, None);
        // Not for windows already in the workspace or missing ones.
        assert!(!ws.replace(1, 3));
        assert!(!ws.replace(9, 4));
    }

    #[test]
    fn move_dir_enters_empty_zone() {
        let mut ws = two_zones();
        ws.insert(1, Some(0), None);
        ws.insert(2, Some(0), Some(1));
        assert!(ws.move_dir(2, Direction::Right));
        assert_eq!(ws.zone_of(2), Some(1));
        assert_eq!(ws.rect_of(1), Some(LEFT));
        // To the right is now a window in another zone → move instead of swap.
        assert!(ws.move_dir(1, Direction::Right));
        assert_eq!((ws.zone_of(1), ws.zone_of(2)), (Some(1), Some(1)));
        assert!(ws.zones()[0].tree.is_empty());
        assert!(!ws.move_dir(1, Direction::Right));
    }

    #[test]
    fn zone_mode_drives_insertion_and_survives_resize() {
        let mut ws = two_zones();
        ws.set_zone_mode(1, ZoneMode::Stack, None);
        ws.insert(1, Some(1), None);
        ws.insert(2, Some(1), None);
        ws.insert(3, Some(0), None);
        // Zone 2 is a stack: both windows fill it.
        assert_eq!(ws.rect_of(1), Some(RIGHT));
        assert_eq!(ws.rect_of(2), Some(RIGHT));
        // Same zone count (e.g. taskbar moved): mode stays.
        ws.set_zones(&[LEFT, RIGHT], 10);
        assert_eq!(ws.zones()[1].mode, ZoneMode::Stack);
        // Switching rearranges.
        ws.set_zone_mode(1, ZoneMode::Rows, Some(1));
        assert_eq!(ws.rect_of(1), Some(Rect::new(500, 0, 500, 295)));

        // Apply saved modes only if the zone count matches.
        let mut fresh = two_zones();
        fresh.restore_modes(&[ZoneMode::Auto]);
        assert_eq!(fresh.zones()[1].mode, ZoneMode::Auto);
        fresh.restore_modes(&[ZoneMode::Columns, ZoneMode::Stack]);
        assert_eq!(fresh.zones()[1].mode, ZoneMode::Stack);
    }

    #[test]
    fn move_dir_swaps_only_within_the_zone() {
        let mut ws = two_zones();
        // Left 1 above 2 (zone is taller than wide), right 3.
        ws.insert(1, Some(0), None);
        ws.insert(2, Some(0), Some(1));
        ws.insert(3, Some(1), None);
        let (top, bottom) = (ws.rect_of(1).unwrap(), ws.rect_of(2).unwrap());
        assert!(top.y < bottom.y);

        // Neighbor in the same zone → swap.
        assert!(ws.move_dir(1, Direction::Down));
        assert_eq!((ws.rect_of(1), ws.rect_of(2)), (Some(bottom), Some(top)));

        // Next target is another zone → move, 3 is not swapped.
        assert!(ws.move_dir(2, Direction::Right));
        assert_eq!(ws.zone_of(2), Some(1));
        assert_eq!(ws.zone_of(3), Some(1));
        assert_eq!(ws.rect_of(1), Some(LEFT));
        assert_eq!(ws.zones()[1].tree.windows(), vec![3, 2]);

        // Edge of the workspace.
        assert!(!ws.move_dir(1, Direction::Left));
    }

    #[test]
    fn move_to_zone() {
        let mut ws = two_zones();
        ws.insert(1, Some(0), None);
        assert!(ws.move_to_zone(1, 1));
        assert_eq!(ws.rect_of(1), Some(RIGHT));
        assert!(!ws.move_to_zone(1, 1));
        assert!(!ws.move_to_zone(1, 5));
        // New window directly into a zone.
        assert!(ws.move_to_zone(2, 0));
        assert_eq!(ws.rect_of(2), Some(LEFT));
    }

    #[test]
    fn layout_change_redistributes_windows() {
        let mut ws = Workspace::single(AREA, 10);
        for id in 1..=4 {
            ws.insert(id, None, None);
        }
        ws.set_zones(&[LEFT, RIGHT], 10);
        assert_eq!(ws.zones()[0].tree.windows(), vec![1, 2]);
        assert_eq!(ws.zones()[1].tree.windows(), vec![3, 4]);
        // Same zone count: trees stay, only the areas change.
        ws.set_zones(
            &[Rect::new(0, 0, 400, 600), Rect::new(400, 0, 600, 600)],
            10,
        );
        assert_eq!(ws.zones()[1].tree.windows(), vec![3, 4]);
        // More zones than windows: one per zone.
        let thirds = [
            Rect::new(0, 0, 333, 600),
            Rect::new(333, 0, 333, 600),
            Rect::new(666, 0, 334, 600),
        ];
        let mut few = two_zones();
        few.insert(1, Some(0), None);
        few.insert(2, Some(0), None);
        few.set_zones(&thirds, 10);
        assert_eq!(few.zone_of(1), Some(0));
        assert_eq!(few.zone_of(2), Some(1));
    }

    #[test]
    fn drop_target_sides_and_stack() {
        let mut ws = two_zones();
        ws.insert(1, Some(0), None);
        ws.insert(9, Some(1), None);
        // Empty zone or only the dragged window itself → no neighbor.
        assert_eq!(
            ws.drop_target(750, 300, 9),
            Some(DropTarget {
                zone: 1,
                beside: None
            })
        );
        let at = |x, y| ws.drop_target(x, y, 9).unwrap().beside.unwrap().1;
        assert_eq!(at(250, 300), DropSide::Stack);
        assert_eq!(at(20, 300), DropSide::Edge(Direction::Left));
        assert_eq!(at(480, 300), DropSide::Edge(Direction::Right));
        assert_eq!(at(250, 10), DropSide::Edge(Direction::Up));
        assert_eq!(at(250, 590), DropSide::Edge(Direction::Down));
        assert_eq!(ws.drop_target(2000, 0, 9), None);
    }

    #[test]
    fn insert_at_and_preview() {
        let mut ws = two_zones();
        ws.insert(1, Some(0), None);
        ws.insert(9, Some(1), None);
        let target = ws.drop_target(480, 300, 9).unwrap();
        let preview = ws.preview(9, target).unwrap();
        assert_eq!(preview, Rect::new(255, 0, 245, 600));
        // Preview changes nothing.
        assert_eq!(ws.zone_of(9), Some(1));
        ws.insert_at(9, target);
        assert_eq!(ws.rect_of(9), Some(preview));
        assert_eq!(ws.rect_of(1), Some(Rect::new(0, 0, 245, 600)));
        assert!(ws.zones()[1].tree.is_empty());

        let stack = ws.drop_target(120, 300, 2).unwrap();
        ws.insert_at(2, stack);
        assert_eq!(ws.rect_of(2), ws.rect_of(1));
    }

    #[test]
    fn resize_edges_follow_mouse() {
        let mut ws = Workspace::single(AREA, 10);
        ws.insert(1, None, None);
        ws.insert(2, None, Some(1));
        let before = ws.rect_of(1).unwrap();
        // Right edge 100 px to the right, plus 1 px rounding noise on the left.
        let after = Rect::new(before.x + 1, before.y, before.width + 99, before.height);
        assert!(ws.resize_edges(1, before, after));
        assert_eq!(ws.rect_of(1).unwrap().width, 595);
        assert_eq!(ws.rect_of(2).unwrap().x, 605);
        // Only outer edges moved → nothing to do.
        let before = ws.rect_of(1).unwrap();
        let after = Rect::new(before.x - 50, before.y, before.width + 50, before.height);
        assert!(!ws.resize_edges(1, before, after));
    }

    #[test]
    fn remove_clears_last_focus() {
        let mut ws = Workspace::single(AREA, 10);
        ws.insert(1, None, None);
        ws.last_focus = Some(1);
        assert!(ws.remove(1));
        assert_eq!(ws.last_focus, None);
        assert!(ws.is_empty());
    }
}
