#[derive(Clone, Copy, Debug, PartialEq, Eq)]
struct Rect {
    x: u32,
    y: u32,
    width: u32,
    height: u32,
}

impl Rect {
    fn right(self) -> u32 {
        self.x + self.width
    }

    fn bottom(self) -> u32 {
        self.y + self.height
    }

    fn intersects(self, other: Self) -> bool {
        self.x < other.right()
            && other.x < self.right()
            && self.y < other.bottom()
            && other.y < self.bottom()
    }

    fn contains(self, other: Self) -> bool {
        self.x <= other.x
            && self.y <= other.y
            && self.right() >= other.right()
            && self.bottom() >= other.bottom()
    }
}

/// Incremental MaxRects packing. Existing rectangles never move when the atlas
/// grows.
#[derive(Clone, Debug)]
pub(super) struct RectPacker {
    width: u32,
    height: u32,
    occupied: Vec<Rect>,
    free: Vec<Rect>,
}

impl RectPacker {
    pub(super) fn new(width: u32, height: u32) -> Self {
        let free = if width == 0 || height == 0 {
            Vec::new()
        } else {
            vec![Rect { x: 0, y: 0, width, height }]
        };
        Self { width, height, occupied: Vec::new(), free }
    }

    /// Reserves an existing atlas rectangle; overlapping reservations form a
    /// union.
    pub(super) fn reserve(&mut self, x: u32, y: u32, width: u32, height: u32) -> bool {
        if width == 0
            || height == 0
            || x.checked_add(width).is_none_or(|right| right > self.width)
            || y.checked_add(height).is_none_or(|bottom| bottom > self.height)
        {
            return false;
        }
        let rect = Rect { x, y, width, height };
        self.subtract(rect);
        self.occupied.push(rect);
        true
    }

    pub(super) fn allocate(&mut self, width: u32, height: u32) -> Option<(u32, u32)> {
        if width == 0 || height == 0 {
            return None;
        }
        let slot = self
            .free
            .iter()
            .filter(|rect| rect.width >= width && rect.height >= height)
            .min_by_key(|rect| {
                let dx = rect.width - width;
                let dy = rect.height - height;
                (dx.min(dy), dx.max(dy), rect.y, rect.x)
            })
            .copied()?;
        self.reserve(slot.x, slot.y, width, height);
        Some((slot.x, slot.y))
    }

    pub(super) fn grow(&mut self, width: u32, height: u32) -> bool {
        if width < self.width || height < self.height || width == 0 || height == 0 {
            return false;
        }
        if width == self.width && height == self.height {
            return true;
        }
        self.width = width;
        self.height = height;
        self.free = vec![Rect { x: 0, y: 0, width, height }];
        // Rebuilding free space allows new glyphs to straddle the old atlas
        // boundary.
        for index in 0..self.occupied.len() {
            self.subtract(self.occupied[index]);
        }
        true
    }

    fn subtract(&mut self, used: Rect) {
        let mut remaining = Vec::new();
        let mut splits = Vec::new();
        for free in self.free.drain(..) {
            if !free.intersects(used) {
                remaining.push(free);
                continue;
            }
            if used.x > free.x {
                splits.push(Rect { width: used.x - free.x, ..free });
            }
            if used.right() < free.right() {
                splits.push(Rect { x: used.right(), width: free.right() - used.right(), ..free });
            }
            if used.y > free.y {
                splits.push(Rect { height: used.y - free.y, ..free });
            }
            if used.bottom() < free.bottom() {
                splits.push(Rect {
                    y: used.bottom(),
                    height: free.bottom() - used.bottom(),
                    ..free
                });
            }
        }
        // Untouched rectangles are already pruned; only compare the new splits.
        for rect in splits {
            if !remaining.iter().any(|other| other.contains(rect)) {
                remaining.retain(|other| !rect.contains(*other));
                remaining.push(rect);
            }
        }
        self.free = remaining;
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn reservations_validate_bounds_and_can_overlap() {
        let mut packer = RectPacker::new(14, 8);
        assert!(!packer.reserve(u32::MAX, 0, 2, 1));
        assert!(!packer.reserve(0, 7, 1, 2));
        assert!(!packer.reserve(0, 0, 0, 1));
        assert!(packer.reserve(0, 0, 4, 8));
        assert!(packer.reserve(2, 0, 4, 8));
        assert_eq!(packer.allocate(8, 8), Some((6, 0)));
        assert_eq!(packer.allocate(1, 1), None);
    }

    #[test]
    fn growth_keeps_positions_and_recovers_space_across_the_old_boundary() {
        let mut packer = RectPacker::new(8, 8);
        assert_eq!(packer.allocate(8, 4), Some((0, 0)));
        assert!(!packer.grow(7, 16));
        assert!(packer.grow(16, 8));
        assert_eq!(packer.allocate(12, 4), Some((0, 4)));
        assert_eq!(packer.allocate(8, 4), Some((8, 0)));
        assert_eq!(packer.allocate(4, 4), Some((12, 4)));
        assert_eq!(packer.allocate(1, 1), None);
    }

    #[test]
    fn mixed_glyph_sizes_never_overlap_reserved_or_allocated_pixels() {
        let mut packer = RectPacker::new(32, 24);
        let reserved = Rect { x: 10, y: 6, width: 6, height: 7 };
        assert!(packer.reserve(reserved.x, reserved.y, reserved.width, reserved.height));
        let mut occupied = vec![reserved];
        let mut seed = 17u32;
        for _ in 0..300 {
            seed = seed.wrapping_mul(1664525).wrapping_add(1013904223);
            let width = 1 + seed % 9;
            let height = 1 + (seed >> 16) % 7;
            if let Some((x, y)) = packer.allocate(width, height) {
                let rect = Rect { x, y, width, height };
                assert!(rect.right() <= 32 && rect.bottom() <= 24);
                assert!(occupied.iter().all(|previous| !rect.intersects(*previous)));
                occupied.push(rect);
            }
        }
        assert!(occupied.len() > 10);
    }
}
