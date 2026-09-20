//! Pure geometry helpers shared by the behaviour engine and the platform drivers.
//!
//! Nothing in this module touches the OS, so every rule that decides *where* the
//! monkey is allowed to be can be unit tested without a compositor.

/// A 2D point / vector in screen space, in logical pixels.
#[derive(Debug, Clone, Copy, PartialEq, Default)]
pub struct Vec2 {
    pub x: f32,
    pub y: f32,
}

impl Vec2 {
    pub const ZERO: Self = Self { x: 0.0, y: 0.0 };

    #[must_use]
    pub const fn new(x: f32, y: f32) -> Self {
        Self { x, y }
    }

    /// Euclidean length of the vector.
    #[must_use]
    pub fn length(self) -> f32 {
        self.x.hypot(self.y)
    }

    /// Unit vector in the same direction, or `None` for a (near) zero vector.
    #[must_use]
    pub fn normalized(self) -> Option<Self> {
        let len = self.length();
        if len <= f32::EPSILON {
            None
        } else {
            Some(Self::new(self.x / len, self.y / len))
        }
    }

    /// Distance between two points.
    #[must_use]
    pub fn distance_to(self, other: Self) -> f32 {
        (other - self).length()
    }

    /// Direction sector index in `0..8`, counted clockwise from "east" in
    /// screen coordinates (y grows downwards).
    ///
    /// Used for eye-follow (FR-02) and for choosing a walk animation row.
    #[must_use]
    pub fn direction_sector(self) -> u32 {
        use std::f32::consts::PI;
        const SECTOR: f32 = 2.0 * PI / 8.0;
        let angle = self.y.atan2(self.x);
        let normalized = (angle + 2.0 * PI) % (2.0 * PI);
        (((normalized + SECTOR / 2.0) / SECTOR).floor() as u32) % 8
    }
}

impl std::ops::Add for Vec2 {
    type Output = Self;
    fn add(self, rhs: Self) -> Self {
        Self::new(self.x + rhs.x, self.y + rhs.y)
    }
}

impl std::ops::Sub for Vec2 {
    type Output = Self;
    fn sub(self, rhs: Self) -> Self {
        Self::new(self.x - rhs.x, self.y - rhs.y)
    }
}

impl std::ops::Mul<f32> for Vec2 {
    type Output = Self;
    fn mul(self, rhs: f32) -> Self {
        Self::new(self.x * rhs, self.y * rhs)
    }
}

/// An axis-aligned rectangle in logical screen pixels.
///
/// Monitor bounds use this type; `x`/`y` may be negative on multi-head layouts
/// where a monitor sits left of or above the primary one.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Rect {
    pub x: i32,
    pub y: i32,
    pub width: u32,
    pub height: u32,
}

impl Rect {
    #[must_use]
    pub const fn new(x: i32, y: i32, width: u32, height: u32) -> Self {
        Self {
            x,
            y,
            width,
            height,
        }
    }

    #[must_use]
    pub fn left(&self) -> f32 {
        self.x as f32
    }

    #[must_use]
    pub fn top(&self) -> f32 {
        self.y as f32
    }

    #[must_use]
    pub fn right(&self) -> f32 {
        self.x as f32 + self.width as f32
    }

    #[must_use]
    pub fn bottom(&self) -> f32 {
        self.y as f32 + self.height as f32
    }

    #[must_use]
    pub fn center(&self) -> Vec2 {
        Vec2::new(
            self.x as f32 + self.width as f32 / 2.0,
            self.y as f32 + self.height as f32 / 2.0,
        )
    }

    /// True when `point` lies inside the rectangle (left/top inclusive).
    #[must_use]
    pub fn contains(&self, point: Vec2) -> bool {
        point.x >= self.left()
            && point.x < self.right()
            && point.y >= self.top()
            && point.y < self.bottom()
    }

    /// True when a sprite of `size` placed at `origin` overlaps this rectangle.
    #[must_use]
    pub fn overlaps_sprite(&self, origin: Vec2, size: Vec2) -> bool {
        origin.x + size.x > self.left()
            && origin.x < self.right()
            && origin.y + size.y > self.top()
            && origin.y < self.bottom()
    }
}

/// The cached set of monitor rectangles reported by the platform driver (FR-09).
///
/// A layout may legitimately be empty (for example before the compositor has
/// announced any output); every query degrades gracefully in that case instead
/// of panicking or trapping the sprite at the origin.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct ScreenLayout {
    monitors: Vec<Rect>,
}

impl ScreenLayout {
    #[must_use]
    pub fn new(monitors: Vec<Rect>) -> Self {
        Self { monitors }
    }

    #[must_use]
    pub fn is_empty(&self) -> bool {
        self.monitors.is_empty()
    }

    #[must_use]
    pub fn monitors(&self) -> &[Rect] {
        &self.monitors
    }

    /// Smallest rectangle covering every monitor, or `None` when the layout is empty.
    #[must_use]
    pub fn bounding_box(&self) -> Option<Rect> {
        let first = *self.monitors.first()?;
        let mut min_x = first.left();
        let mut min_y = first.top();
        let mut max_x = first.right();
        let mut max_y = first.bottom();
        for monitor in &self.monitors[1..] {
            min_x = min_x.min(monitor.left());
            min_y = min_y.min(monitor.top());
            max_x = max_x.max(monitor.right());
            max_y = max_y.max(monitor.bottom());
        }
        Some(Rect::new(
            min_x as i32,
            min_y as i32,
            (max_x - min_x) as u32,
            (max_y - min_y) as u32,
        ))
    }

    /// The monitor a sprite of `size` at `origin` currently overlaps, preferring
    /// the one whose centre is closest when the sprite straddles several.
    #[must_use]
    pub fn monitor_for_sprite(&self, origin: Vec2, size: Vec2) -> Option<Rect> {
        let sprite_center = Vec2::new(origin.x + size.x / 2.0, origin.y + size.y / 2.0);
        self.monitors
            .iter()
            .filter(|monitor| monitor.overlaps_sprite(origin, size))
            .min_by(|a, b| {
                let da = a.center().distance_to(sprite_center);
                let db = b.center().distance_to(sprite_center);
                da.total_cmp(&db)
            })
            .copied()
    }

    /// The monitor whose centre is nearest to `point`.
    #[must_use]
    pub fn nearest_monitor(&self, point: Vec2) -> Option<Rect> {
        self.monitors
            .iter()
            .min_by(|a, b| {
                a.center()
                    .distance_to(point)
                    .total_cmp(&b.center().distance_to(point))
            })
            .copied()
    }

    /// Clamp a sprite of `size` so that it stays fully inside a monitor (FR-09).
    ///
    /// A sprite that is already on a monitor is clamped to that monitor. A
    /// sprite that has ended up outside every monitor — which happens when a
    /// display is unplugged — is snapped to the floor of the nearest one. When
    /// the layout is empty the origin is returned unchanged, because clamping
    /// to a guessed rectangle would teleport the sprite for no reason.
    #[must_use]
    pub fn clamp_sprite(&self, origin: Vec2, size: Vec2) -> Vec2 {
        let Some(monitor) = self.monitor_for_sprite(origin, size).or_else(|| {
            self.nearest_monitor(Vec2::new(origin.x + size.x / 2.0, origin.y + size.y / 2.0))
        }) else {
            return origin;
        };

        // `max` before `min` keeps the sprite pinned to the left/top edge when it
        // is larger than the monitor, instead of producing an inverted range.
        let x = origin.x.min(monitor.right() - size.x).max(monitor.left());
        let y = origin.y.min(monitor.bottom() - size.y).max(monitor.top());
        Vec2::new(x, y)
    }

    /// Top of the floor barrier for a sprite of `size` at `origin`: the `y` at
    /// which its lower bound rests on the bottom edge of the monitor it is
    /// falling through (FR-08).
    #[must_use]
    pub fn floor_for_sprite(&self, origin: Vec2, size: Vec2) -> Option<f32> {
        let monitor = self.monitor_for_sprite(origin, size).or_else(|| {
            self.nearest_monitor(Vec2::new(origin.x + size.x / 2.0, origin.y + size.y / 2.0))
        })?;
        Some(monitor.bottom() - size.y)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn dual_head() -> ScreenLayout {
        // A 1920x1080 primary with a 1280x1024 monitor to its left, top-aligned.
        ScreenLayout::new(vec![
            Rect::new(0, 0, 1920, 1080),
            Rect::new(-1280, 0, 1280, 1024),
        ])
    }

    #[test]
    fn direction_sectors_cover_the_compass() {
        assert_eq!(Vec2::new(1.0, 0.0).direction_sector(), 0); // east
        assert_eq!(Vec2::new(1.0, 1.0).direction_sector(), 1); // south-east
        assert_eq!(Vec2::new(0.0, 1.0).direction_sector(), 2); // south
        assert_eq!(Vec2::new(-1.0, 0.0).direction_sector(), 4); // west
        assert_eq!(Vec2::new(0.0, -1.0).direction_sector(), 6); // north
    }

    #[test]
    fn direction_sector_is_always_in_range() {
        for degrees in 0..720 {
            let radians = (degrees as f32).to_radians();
            let sector = Vec2::new(radians.cos(), radians.sin()).direction_sector();
            assert!(sector < 8, "sector {sector} out of range at {degrees} deg");
        }
    }

    #[test]
    fn zero_vector_has_no_direction_but_does_not_panic() {
        assert!(Vec2::ZERO.normalized().is_none());
        assert!(Vec2::ZERO.direction_sector() < 8);
    }

    #[test]
    fn clamping_keeps_the_sprite_on_its_own_monitor() {
        let layout = dual_head();
        let size = Vec2::new(128.0, 128.0);
        // Straddling the seam: the sprite belongs to the monitor whose centre is
        // closest, and is pushed fully back inside it.
        assert_eq!(
            layout.clamp_sprite(Vec2::new(-50.0, 500.0), size),
            Vec2::new(-128.0, 500.0)
        );
        assert_eq!(
            layout.clamp_sprite(Vec2::new(1900.0, 500.0), size),
            Vec2::new(1792.0, 500.0)
        );
        assert_eq!(
            layout.clamp_sprite(Vec2::new(400.0, 1000.0), size),
            Vec2::new(400.0, 952.0)
        );
    }

    #[test]
    fn sprite_outside_every_monitor_snaps_to_the_nearest_floor() {
        let layout = ScreenLayout::new(vec![Rect::new(0, 0, 1920, 1080)]);
        let size = Vec2::new(128.0, 128.0);
        let clamped = layout.clamp_sprite(Vec2::new(9000.0, 9000.0), size);
        assert_eq!(clamped, Vec2::new(1792.0, 952.0));
    }

    #[test]
    fn empty_layout_leaves_the_sprite_alone() {
        let layout = ScreenLayout::default();
        let origin = Vec2::new(42.0, 17.0);
        assert_eq!(layout.clamp_sprite(origin, Vec2::new(128.0, 128.0)), origin);
        assert_eq!(
            layout.floor_for_sprite(origin, Vec2::new(128.0, 128.0)),
            None
        );
        assert_eq!(layout.bounding_box(), None);
    }

    #[test]
    fn floor_is_the_bottom_of_the_monitor_under_the_sprite() {
        let layout = dual_head();
        let size = Vec2::new(128.0, 128.0);
        assert_eq!(
            layout.floor_for_sprite(Vec2::new(100.0, 0.0), size),
            Some(1080.0 - 128.0)
        );
        assert_eq!(
            layout.floor_for_sprite(Vec2::new(-600.0, 0.0), size),
            Some(1024.0 - 128.0)
        );
    }

    #[test]
    fn bounding_box_spans_every_monitor() {
        assert_eq!(
            dual_head().bounding_box(),
            Some(Rect::new(-1280, 0, 3200, 1080))
        );
    }
}
