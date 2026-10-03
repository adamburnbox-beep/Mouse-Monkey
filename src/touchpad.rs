//! Turns a touchpad's absolute finger reports into relative pointer motion
//! and two-finger scroll notches, roughly the way a compositor would.

/// Screen pixels per millimetre of one-finger travel.
const PX_PER_MM: f32 = 12.0;
/// Millimetres of two-finger travel per scroll-wheel notch.
const SCROLL_MM_PER_NOTCH: f32 = 4.0;

const BTN_TOOL_FINGER: u16 = 0x145;
const BTN_TOOL_QUINTTAP: u16 = 0x148;
const BTN_TOUCH: u16 = 0x14a;
const BTN_TOOL_DOUBLETAP: u16 = 0x14d;
const BTN_TOOL_TRIPLETAP: u16 = 0x14e;
const BTN_TOOL_QUADTAP: u16 = 0x14f;
const ABS_X: u16 = 0x00;
const ABS_Y: u16 = 0x01;

#[derive(Debug, Default, Clone, Copy, PartialEq)]
pub struct Report {
    pub dx: f32,
    pub dy: f32,
    /// Scroll-wheel notches, positive = fingers moved down.
    pub scroll: f32,
}

pub struct Touchpad {
    px_per_unit: (f32, f32),
    units_per_notch: f32,
    touching: bool,
    tool_fingers: u8,
    pos: (Option<i32>, Option<i32>),
    last: Option<(i32, i32)>,
    last_fingers: u8,
    scroll_accum: f32,
}

impl Touchpad {
    /// `resolution` is in device units per millimetre (0 if the device doesn't
    /// say); `range` is the axis's max - min.
    pub fn new(resolution: (i32, i32), range: (i32, i32)) -> Self {
        // Without a resolution, assume the pad is about 100 mm wide.
        let units_per_mm = |res: i32, range: i32| {
            if res > 0 { res as f32 } else { (range.max(100) as f32) / 100.0 }
        };
        let (ux, uy) = (units_per_mm(resolution.0, range.0), units_per_mm(resolution.1, range.1));
        Self {
            px_per_unit: (PX_PER_MM / ux, PX_PER_MM / uy),
            units_per_notch: SCROLL_MM_PER_NOTCH * uy,
            touching: false,
            tool_fingers: 0,
            pos: (None, None),
            last: None,
            last_fingers: 0,
            scroll_accum: 0.0,
        }
    }

    pub fn key(&mut self, code: u16, value: i32) {
        let down = value != 0;
        match code {
            BTN_TOUCH => self.touching = down,
            BTN_TOOL_FINGER..=BTN_TOOL_QUINTTAP | BTN_TOOL_DOUBLETAP..=BTN_TOOL_QUADTAP => {
                let count = match code {
                    BTN_TOOL_FINGER => 1,
                    BTN_TOOL_DOUBLETAP => 2,
                    BTN_TOOL_TRIPLETAP => 3,
                    _ => 4,
                };
                if down {
                    self.tool_fingers = count;
                } else if self.tool_fingers == count {
                    self.tool_fingers = 0;
                }
            }
            _ => {}
        }
    }

    pub fn abs(&mut self, axis: u16, value: i32) {
        match axis {
            ABS_X => self.pos.0 = Some(value),
            ABS_Y => self.pos.1 = Some(value),
            _ => {}
        }
    }

    /// Call on every SYN_REPORT; returns the motion since the previous one.
    pub fn sync(&mut self) -> Report {
        let fingers = if !self.touching {
            0
        } else if self.tool_fingers > 0 {
            self.tool_fingers
        } else {
            1
        };
        let mut report = Report::default();
        // A finger landing, lifting or changing count would read as a jump.
        if fingers != self.last_fingers {
            self.last = None;
            self.scroll_accum = 0.0;
        }
        self.last_fingers = fingers;
        let (Some(x), Some(y)) = self.pos else {
            return report;
        };
        if fingers == 0 {
            self.last = None;
            return report;
        }
        if let Some((lx, ly)) = self.last {
            let (ux, uy) = ((x - lx) as f32, (y - ly) as f32);
            match fingers {
                1 => {
                    report.dx = ux * self.px_per_unit.0;
                    report.dy = uy * self.px_per_unit.1;
                }
                2 => {
                    self.scroll_accum += uy;
                    let notches = (self.scroll_accum / self.units_per_notch).trunc();
                    self.scroll_accum -= notches * self.units_per_notch;
                    report.scroll = notches;
                }
                _ => {}
            }
        }
        self.last = Some((x, y));
        report
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// 40 units per mm, like a typical modern touchpad.
    fn pad() -> Touchpad {
        Touchpad::new((40, 40), (4000, 2600))
    }

    fn frame(t: &mut Touchpad, x: i32, y: i32) -> Report {
        t.abs(ABS_X, x);
        t.abs(ABS_Y, y);
        t.sync()
    }

    #[test]
    fn one_finger_moves_the_pointer() {
        let mut t = pad();
        t.key(BTN_TOUCH, 1);
        t.key(BTN_TOOL_FINGER, 1);
        assert_eq!(frame(&mut t, 1000, 1000), Report::default(), "landing is not motion");
        let r = frame(&mut t, 1040, 980); // 1 mm right, 0.5 mm up
        assert!((r.dx - 12.0).abs() < 1e-3 && (r.dy + 6.0).abs() < 1e-3, "{:?}", r);
    }

    #[test]
    fn lifting_and_landing_elsewhere_is_not_a_jump() {
        let mut t = pad();
        t.key(BTN_TOUCH, 1);
        t.key(BTN_TOOL_FINGER, 1);
        frame(&mut t, 1000, 1000);
        frame(&mut t, 1100, 1000);
        t.key(BTN_TOUCH, 0);
        t.key(BTN_TOOL_FINGER, 0);
        t.sync();
        t.key(BTN_TOUCH, 1);
        t.key(BTN_TOOL_FINGER, 1);
        assert_eq!(frame(&mut t, 3000, 2000), Report::default());
    }

    #[test]
    fn two_fingers_scroll_instead_of_moving() {
        let mut t = pad();
        t.key(BTN_TOUCH, 1);
        t.key(BTN_TOOL_DOUBLETAP, 1);
        frame(&mut t, 1000, 1000);
        let mut notches = 0.0;
        for i in 1..=20 {
            let r = frame(&mut t, 1000, 1000 + i * 40); // 20 mm down in total
            assert_eq!((r.dx, r.dy), (0.0, 0.0));
            notches += r.scroll;
        }
        assert_eq!(notches, 5.0);
    }

    #[test]
    fn missing_resolution_falls_back_to_pad_size() {
        let t = Touchpad::new((0, 0), (1000, 600));
        assert!((t.px_per_unit.0 - 1.2).abs() < 1e-4);
    }
}
