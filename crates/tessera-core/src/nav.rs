//! Directional navigation: which rectangle lies "left of" another one?

use crate::action::Direction;
use crate::geometry::Rect;

/// Picks the nearest neighbor of `from` in direction `dir` from `candidates`.
///
/// Prefers candidates that overlap `from` perpendicular to the direction,
/// then the smallest distance, then the largest overlap.
pub fn neighbor<T: Copy>(
    from: Rect,
    dir: Direction,
    candidates: impl IntoIterator<Item = (T, Rect)>,
) -> Option<T> {
    let (fx, fy) = from.center();
    candidates
        .into_iter()
        .filter_map(|(item, c)| {
            let (cx, cy) = c.center();
            let (ahead, gap, overlap, perpendicular) = match dir {
                Direction::Left => (
                    cx < fx,
                    from.x - c.right(),
                    span_overlap(from.y, from.bottom(), c.y, c.bottom()),
                    (cy - fy).abs(),
                ),
                Direction::Right => (
                    cx > fx,
                    c.x - from.right(),
                    span_overlap(from.y, from.bottom(), c.y, c.bottom()),
                    (cy - fy).abs(),
                ),
                Direction::Up => (
                    cy < fy,
                    from.y - c.bottom(),
                    span_overlap(from.x, from.right(), c.x, c.right()),
                    (cx - fx).abs(),
                ),
                Direction::Down => (
                    cy > fy,
                    c.y - from.bottom(),
                    span_overlap(from.x, from.right(), c.x, c.right()),
                    (cx - fx).abs(),
                ),
            };
            ahead.then_some(((overlap <= 0, gap.max(0), -overlap, perpendicular), item))
        })
        .min_by_key(|(score, _)| *score)
        .map(|(_, item)| item)
}

fn span_overlap(a1: i32, a2: i32, b1: i32, b2: i32) -> i32 {
    a2.min(b2) - a1.max(b1)
}

#[cfg(test)]
mod tests {
    use super::*;
    use Direction::*;

    // A | B
    //   | C
    const A: Rect = Rect::new(0, 0, 100, 200);
    const B: Rect = Rect::new(110, 0, 100, 95);
    const C: Rect = Rect::new(110, 105, 100, 95);

    fn all() -> [(char, Rect); 3] {
        [('a', A), ('b', B), ('c', C)]
    }

    #[test]
    fn finds_neighbors_in_tiling() {
        assert_eq!(neighbor(C, Left, all()), Some('a'));
        assert_eq!(neighbor(B, Left, all()), Some('a'));
        assert_eq!(neighbor(B, Down, all()), Some('c'));
        assert_eq!(neighbor(C, Up, all()), Some('b'));
        assert_eq!(neighbor(A, Left, all()), None);
        assert_eq!(neighbor(B, Right, all()), None);
        assert!(matches!(neighbor(A, Right, all()), Some('b' | 'c')));
    }

    #[test]
    fn prefers_overlapping_over_closer_diagonal() {
        let from = Rect::new(0, 0, 100, 100);
        let diagonal = Rect::new(105, 150, 100, 100);
        let straight = Rect::new(300, 0, 100, 100);
        assert_eq!(
            neighbor(from, Right, [(1, diagonal), (2, straight)]),
            Some(2)
        );
    }

    #[test]
    fn works_across_monitors() {
        let left_monitor_window = Rect::new(-1920, 0, 1920, 1080);
        let here = Rect::new(0, 0, 960, 1080);
        assert_eq!(neighbor(here, Left, [(9, left_monitor_window)]), Some(9));
    }
}
