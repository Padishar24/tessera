//! Geometry in physical screen pixels.

use serde::{Deserialize, Serialize};

#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, Hash, Serialize, Deserialize)]
pub struct Rect {
    pub x: i32,
    pub y: i32,
    pub width: i32,
    pub height: i32,
}

impl Rect {
    pub const fn new(x: i32, y: i32, width: i32, height: i32) -> Self {
        Self {
            x,
            y,
            width,
            height,
        }
    }

    /// From Win32 form (left, top, right, bottom).
    pub const fn from_ltrb(left: i32, top: i32, right: i32, bottom: i32) -> Self {
        Self::new(left, top, right - left, bottom - top)
    }

    pub const fn right(&self) -> i32 {
        self.x + self.width
    }

    pub const fn bottom(&self) -> i32 {
        self.y + self.height
    }

    pub const fn is_empty(&self) -> bool {
        self.width <= 0 || self.height <= 0
    }

    pub const fn center(&self) -> (i32, i32) {
        (self.x + self.width / 2, self.y + self.height / 2)
    }

    pub const fn contains_point(&self, x: i32, y: i32) -> bool {
        x >= self.x && x < self.right() && y >= self.y && y < self.bottom()
    }

    /// Intersection; empty if the rectangles do not overlap.
    pub fn intersection(&self, other: &Rect) -> Rect {
        let left = self.x.max(other.x);
        let top = self.y.max(other.y);
        let right = self.right().min(other.right());
        let bottom = self.bottom().min(other.bottom());
        if right <= left || bottom <= top {
            Rect::default()
        } else {
            Rect::from_ltrb(left, top, right, bottom)
        }
    }

    pub fn area(&self) -> i64 {
        if self.is_empty() {
            0
        } else {
            i64::from(self.width) * i64::from(self.height)
        }
    }
}

/// Scales `px` logical pixels (relative to 96 DPI) to physical pixels at
/// `dpi`, rounded half up. Very large values are clamped to `i32::MAX`
/// instead of overflowing.
pub fn scale_px(px: u32, dpi: u32) -> i32 {
    let scaled = (i64::from(px) * i64::from(dpi) + 48) / 96;
    i32::try_from(scaled).unwrap_or(i32::MAX)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn scale_px_rounds_to_nearest() {
        assert_eq!(scale_px(8, 96), 8);
        assert_eq!(scale_px(8, 120), 10);
        assert_eq!(scale_px(8, 144), 12);
        assert_eq!(scale_px(1, 150), 2);
        assert_eq!(scale_px(1, 100), 1);
        assert_eq!(scale_px(0, 192), 0);
        assert_eq!(scale_px(u32::MAX, 192), i32::MAX);
    }

    #[test]
    fn ltrb_roundtrip() {
        let r = Rect::from_ltrb(-1920, 0, 0, 1080);
        assert_eq!(r, Rect::new(-1920, 0, 1920, 1080));
        assert_eq!((r.right(), r.bottom()), (0, 1080));
        assert_eq!(r.center(), (-960, 540));
    }

    #[test]
    fn contains_is_half_open() {
        let r = Rect::new(0, 0, 10, 10);
        assert!(r.contains_point(0, 0));
        assert!(r.contains_point(9, 9));
        assert!(!r.contains_point(10, 5));
    }

    #[test]
    fn intersection_and_area() {
        let a = Rect::new(0, 0, 100, 100);
        let b = Rect::new(50, 80, 100, 100);
        assert_eq!(a.intersection(&b), Rect::new(50, 80, 50, 20));
        assert_eq!(a.intersection(&b).area(), 1000);
        assert!(a.intersection(&Rect::new(100, 0, 10, 10)).is_empty());
        assert_eq!(Rect::new(0, 0, -5, 10).area(), 0);
    }
}
