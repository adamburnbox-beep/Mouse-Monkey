use crate::platform::{InputEvent, KeyKind, MouseButton, PlatformDriver, Rect};
use crate::state::PetState;
use log::{debug, info, warn};
use rand::{thread_rng, Rng};
use std::collections::VecDeque;
use std::f32::consts::PI;

// Sprite sheet rows (layout is defined in tools/sprites/build.py).
// Rows 0-7 are the eight gaze/travel sectors (0 = right, clockwise).
const ROW_PETTED: u32 = 8;
/// Col 0: held/dangling. Cols 1-7: tumbling flail.
const ROW_HELD: u32 = 9;
/// Cols 0-3: calm typing, cols 4-7: flushed typing.
const ROW_TYPING: u32 = 10;
/// Overheated typing; cols 0-3 and 4-7 alternate the steam puffs.
const ROW_TYPING_HOT: u32 = 11;
/// Cols 0-5: banana peel stages, cols 6-7: eating.
const ROW_SNACK: u32 = 12;
const ROW_BOOP: u32 = 13;
pub const SHEET_ROWS: u32 = 14;

// Hunting: slow walk after inactivity, fast chase after a mouse flick.
const WALK_SPEED_PX_PER_S: f32 = 180.0;
const CHASE_SPEED_PX_PER_S: f32 = 700.0;
const HUNTING_DISTANCE_THRESHOLD_PX: f32 = 10.0;
const CURSOR_INACTIVITY_THRESHOLD_S: f32 = 300.0;
const CHASE_TRIGGER_SPEED_PX_PER_S: f32 = 2200.0;
/// Accumulated time the cursor must spend above the trigger speed, so a single
/// teleport (e.g. cursor recalibration) can't start a chase.
const CHASE_TRIGGER_HOLD_S: f32 = 0.12;
const CHASE_MIN_DISTANCE_PX: f32 = 160.0;
const CHASE_MAX_S: f32 = 4.0;
const CHASE_COOLDOWN_S: f32 = 6.0;

// Typing: paws alternate per keystroke; typing speed heats the monkey up.
const TYPING_LINGER_S: f32 = 1.5;
const PAW_DOWN_S: f32 = 0.11;
const TYPING_RATE_WINDOW_S: f32 = 1.5;
const WARM_ENTER_KEYS_PER_S: f32 = 5.0;
const WARM_EXIT_KEYS_PER_S: f32 = 3.5;
const HOT_ENTER_KEYS_PER_S: f32 = 8.0;
const HOT_EXIT_KEYS_PER_S: f32 = 6.0;

// Petting: back-and-forth strokes over the head.
const PET_MIN_STROKE_FRACTION: f32 = 0.1; // of sprite width
const PET_STROKES_TO_START: usize = 3;
const PET_STROKE_WINDOW_S: f32 = 2.0;
const PET_LINGER_S: f32 = 1.2;

// Clicks and drags.
const CLICK_MAX_MOVE_PX: f32 = 6.0;
const CLICK_MAX_S: f32 = 0.35;
const FLING_SPEED_PX_PER_S: f32 = 1400.0;
const MAX_FLING_SPEED_PX_PER_S: f32 = 2500.0;
const BOOP_FPS: f32 = 12.0;

// Scrolling peels a banana, then the monkey eats it.
const NOTCHES_TO_PEEL: f32 = 12.0;
const SNACK_GIVE_UP_S: f32 = 4.0;
const EAT_S: f32 = 1.4;

const MICRO_WIGGLE_INTERVAL_FRAMES: u64 = 500;
const MICRO_WIGGLE_DURATION_S: f32 = 0.4;
const MICRO_WIGGLE_AMPLITUDE_PX: f32 = 2.0;
const GRAVITY_PX_PER_S_SQUARED: f32 = 9.8 * 60.0;
/// Fraction of speed kept when a thrown monkey bounces off a wall or the ceiling.
const WALL_BOUNCE: f32 = 0.45;

// Mochi jelly: a damped spring drives vertical stretch; horizontal squish
// is half of it to preserve volume (k_squish = 0.5 * k_stretch).
const JELLY_STIFFNESS: f32 = 260.0;
const JELLY_DAMPING: f32 = 14.0;
const JELLY_DRAG_GAIN: f32 = 0.00022;
const JELLY_HANG_STRETCH: f32 = 0.06;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Paw {
    Ready = 0,
    Left = 1,
    Right = 2,
    Both = 3,
}

pub struct Monkey {
    pub state: PetState,
    pub position: (f32, f32), // Top-left corner of the unscaled sprite
    pub velocity: (f32, f32),
    pub current_animation_frame_index: u32,
    pub current_animation_row: u32,
    pub scale: (f32, f32), // Visual stretch/squish; does not affect the hit box

    /// Monkey-local clock in seconds, advanced by `dt` every tick.
    clock: f32,
    state_animation_time: f32,
    cursor_inactivity_s: f32,
    idle_frame_counter: u64,
    gaze_sector: u32,
    rng: rand::rngs::ThreadRng,

    // Mouse tracking
    previous_mouse_pos: (f32, f32),
    current_mouse_pos: (f32, f32),
    current_mouse_velocity: (f32, f32),
    smoothed_mouse_velocity: (f32, f32),
    fast_move_s: f32,
    cursor_known: bool,

    // Dragging / clicking
    is_dragging: bool,
    drag_anchor_offset: (f32, f32),
    press_pos: (f32, f32),
    press_time: f32,

    // Hunting
    hunting_speed: f32,
    chase_deadline: Option<f32>,
    chase_cooldown_until: f32,

    // Typing
    key_times: VecDeque<f32>,
    last_key_time: f32,
    paw: Paw,
    paw_until: f32,
    last_single_paw: Paw,
    typing_heat: u8,

    // Petting
    stroke_dir: i8,
    stroke_travel: f32,
    strokes: VecDeque<f32>,
    last_stroke_time: f32,

    // Snacking
    peel_notches: f32,
    last_scroll_time: f32,
    eat_started: Option<f32>,

    // Micro-wiggle
    micro_wiggle_start: Option<f32>,
    micro_wiggle_base_position: Option<(f32, f32)>,

    // Mochi jelly spring
    jelly: f32,
    jelly_velocity: f32,

    sprite_width: u32,
    sprite_height: u32,
}

impl Monkey {
    pub fn new(initial_pos: (f32, f32), sprite_width: u32, sprite_height: u32) -> Self {
        Self {
            state: PetState::Idle,
            position: initial_pos,
            velocity: (0.0, 0.0),
            current_animation_frame_index: 0,
            current_animation_row: 0,
            scale: (1.0, 1.0),

            clock: 0.0,
            state_animation_time: 0.0,
            cursor_inactivity_s: 0.0,
            idle_frame_counter: 0,
            gaze_sector: 2,
            rng: thread_rng(),

            previous_mouse_pos: (0.0, 0.0),
            current_mouse_pos: (0.0, 0.0),
            current_mouse_velocity: (0.0, 0.0),
            smoothed_mouse_velocity: (0.0, 0.0),
            fast_move_s: 0.0,
            cursor_known: false,

            is_dragging: false,
            drag_anchor_offset: (0.0, 0.0),
            press_pos: (0.0, 0.0),
            press_time: 0.0,

            hunting_speed: WALK_SPEED_PX_PER_S,
            chase_deadline: None,
            chase_cooldown_until: 0.0,

            key_times: VecDeque::new(),
            last_key_time: f32::NEG_INFINITY,
            paw: Paw::Ready,
            paw_until: 0.0,
            last_single_paw: Paw::Right,
            typing_heat: 0,

            stroke_dir: 0,
            stroke_travel: 0.0,
            strokes: VecDeque::new(),
            last_stroke_time: f32::NEG_INFINITY,

            peel_notches: 0.0,
            last_scroll_time: f32::NEG_INFINITY,
            eat_started: None,

            micro_wiggle_start: None,
            micro_wiggle_base_position: None,

            jelly: 0.0,
            jelly_velocity: 0.0,

            sprite_width,
            sprite_height,
        }
    }

    fn contains_point(&self, x: f32, y: f32) -> bool {
        x >= self.position.0
            && x <= self.position.0 + self.sprite_width as f32
            && y >= self.position.1
            && y <= self.position.1 + self.sprite_height as f32
    }

    /// The head occupies roughly the upper-middle of the frame.
    fn over_head(&self, x: f32, y: f32) -> bool {
        let (w, h) = (self.sprite_width as f32, self.sprite_height as f32);
        x >= self.position.0 + w * 0.15
            && x <= self.position.0 + w * 0.85
            && y >= self.position.1 + h * 0.1
            && y <= self.position.1 + h * 0.6
    }

    fn center(&self) -> (f32, f32) {
        (
            self.position.0 + self.sprite_width as f32 / 2.0,
            self.position.1 + self.sprite_height as f32 / 2.0,
        )
    }

    /// Where to draw the (possibly stretched) sprite: hanging from the top
    /// while held, standing on its feet otherwise.
    pub fn render_position(&self) -> (f32, f32) {
        let (w, h) = (self.sprite_width as f32, self.sprite_height as f32);
        let dx = w * (1.0 - self.scale.0) / 2.0;
        let dy = if self.state == PetState::Dragged { 0.0 } else { h * (1.0 - self.scale.1) };
        (self.position.0 + dx, self.position.1 + dy)
    }

    /// 8 sectors of 45°, 0 = right, increasing clockwise (screen y points down).
    fn sector_towards(dx: f32, dy: f32) -> u32 {
        let normalized = (dy.atan2(dx) + 2.0 * PI) % (2.0 * PI);
        ((normalized + PI / 8.0) / (PI / 4.0)).floor() as u32 % 8
    }

    fn transition_to(&mut self, new_state: PetState) {
        if self.state == PetState::MicroWiggle && new_state != PetState::MicroWiggle {
            if let Some(base) = self.micro_wiggle_base_position.take() {
                self.position = base;
            }
            self.micro_wiggle_start = None;
        }
        if self.state == PetState::Hunting && self.chase_deadline.take().is_some() {
            self.chase_cooldown_until = self.clock + CHASE_COOLDOWN_S;
        }
        debug!("Monkey: {:?} -> {:?}", self.state, new_state);
        self.state = new_state;
        self.state_animation_time = 0.0;
    }

    pub fn tick(&mut self, input_events: &[InputEvent], dt: f32, platform_driver: &dyn PlatformDriver) {
        self.clock += dt;
        let now = self.clock;
        let dt_safe = dt.max(0.001);

        self.previous_mouse_pos = self.current_mouse_pos;
        let mut mouse_moved_this_tick = false;

        for event in input_events {
            match event {
                InputEvent::MouseMove { x, y } => {
                    let last = self.current_mouse_pos;
                    self.current_mouse_pos = (*x, *y);
                    self.track_petting(last, (*x, *y));
                    mouse_moved_this_tick = true;
                }
                InputEvent::MouseDown { x, y, button } => {
                    if *button == MouseButton::Left && self.contains_point(*x, *y) {
                        self.transition_to(PetState::Dragged);
                        self.is_dragging = true;
                        self.drag_anchor_offset = (*x - self.position.0, *y - self.position.1);
                        self.press_pos = (*x, *y);
                        self.press_time = now;
                        // Sync so the Dragged branch this tick doesn't use a stale position.
                        self.current_mouse_pos = (*x, *y);
                        self.previous_mouse_pos = (*x, *y);
                        mouse_moved_this_tick = true;
                        self.velocity = (0.0, 0.0);
                        info!("Monkey: picked up.");
                    }
                }
                InputEvent::MouseUp { x, y, .. } => {
                    if self.is_dragging {
                        self.is_dragging = false;
                        self.release((*x, *y), now);
                    }
                }
                InputEvent::KeyDown { kind, .. } => self.on_key(*kind, now),
                InputEvent::Scroll { delta } => self.on_scroll(*delta, now),
                _ => {}
            }
        }

        // Without a MouseMove this tick (e.g. evdev inactive and the pointer is
        // outside our input region), fall back to the driver's last known cursor.
        if !mouse_moved_this_tick {
            let (px, py) = platform_driver.get_cursor_pos();
            if (px - self.current_mouse_pos.0).abs() > 0.5 || (py - self.current_mouse_pos.1).abs() > 0.5 {
                self.current_mouse_pos = (px, py);
                mouse_moved_this_tick = true;
            }
        }
        // The first cursor sample is a jump from (0, 0), not real motion.
        if mouse_moved_this_tick && !self.cursor_known {
            self.previous_mouse_pos = self.current_mouse_pos;
            self.cursor_known = true;
        }

        self.current_mouse_velocity = if mouse_moved_this_tick {
            (
                (self.current_mouse_pos.0 - self.previous_mouse_pos.0) / dt_safe,
                (self.current_mouse_pos.1 - self.previous_mouse_pos.1) / dt_safe,
            )
        } else {
            (0.0, 0.0)
        };
        let alpha = 1.0 - (-dt / 0.06).exp();
        self.smoothed_mouse_velocity.0 += (self.current_mouse_velocity.0 - self.smoothed_mouse_velocity.0) * alpha;
        self.smoothed_mouse_velocity.1 += (self.current_mouse_velocity.1 - self.smoothed_mouse_velocity.1) * alpha;
        let cursor_speed = self.current_mouse_velocity.0.hypot(self.current_mouse_velocity.1);
        if cursor_speed > CHASE_TRIGGER_SPEED_PX_PER_S {
            self.fast_move_s += dt;
        } else {
            self.fast_move_s = (self.fast_move_s - dt).max(0.0);
        }

        if mouse_moved_this_tick {
            self.cursor_inactivity_s = 0.0;
        } else {
            self.cursor_inactivity_s += dt;
        }

        self.update_typing_heat(now);
        while self.strokes.front().map_or(false, |t| now - t > PET_STROKE_WINDOW_S) {
            self.strokes.pop_front();
        }
        let (cx, cy) = self.center();
        self.gaze_sector = Self::sector_towards(self.current_mouse_pos.0 - cx, self.current_mouse_pos.1 - cy);

        self.state_animation_time += dt;
        self.update_state(now, dt, platform_driver);
        self.update_jelly(dt);

        if self.state != PetState::Dragged {
            self.clamp_to_screen_bounds(platform_driver);
        }
    }

    fn release(&mut self, pos: (f32, f32), now: f32) {
        let moved = (pos.0 - self.press_pos.0).hypot(pos.1 - self.press_pos.1);
        let (vx, vy) = self.smoothed_mouse_velocity;
        let speed = vx.hypot(vy);
        if moved < CLICK_MAX_MOVE_PX && now - self.press_time < CLICK_MAX_S {
            self.transition_to(PetState::Booped);
            self.jelly_velocity -= 4.0;
            info!("Monkey: booped.");
        } else if speed > FLING_SPEED_PX_PER_S {
            let k = (MAX_FLING_SPEED_PX_PER_S / speed).min(1.0);
            self.velocity = (vx * k, vy * k);
            self.transition_to(PetState::Dramatic);
            info!("Monkey: thrown at {:.0} px/s.", speed);
        } else {
            self.transition_to(PetState::Idle);
            self.jelly_velocity -= 2.5;
            info!("Monkey: put down.");
        }
    }

    fn on_key(&mut self, kind: KeyKind, now: f32) {
        self.key_times.push_back(now);
        self.last_key_time = now;
        self.paw = match kind {
            KeyKind::Thump => Paw::Both,
            KeyKind::Other => {
                self.last_single_paw = if self.last_single_paw == Paw::Left { Paw::Right } else { Paw::Left };
                self.last_single_paw
            }
        };
        self.paw_until = now + PAW_DOWN_S;
        if !matches!(self.state, PetState::Dragged | PetState::Dramatic | PetState::Typing) {
            self.transition_to(PetState::Typing);
            info!("Monkey: typing along.");
        }
    }

    fn update_typing_heat(&mut self, now: f32) {
        while self.key_times.front().map_or(false, |t| now - t > TYPING_RATE_WINDOW_S) {
            self.key_times.pop_front();
        }
        let rate = self.key_times.len() as f32 / TYPING_RATE_WINDOW_S;
        self.typing_heat = match self.typing_heat {
            0 if rate >= HOT_ENTER_KEYS_PER_S => 2,
            0 if rate >= WARM_ENTER_KEYS_PER_S => 1,
            1 if rate >= HOT_ENTER_KEYS_PER_S => 2,
            1 if rate < WARM_EXIT_KEYS_PER_S => 0,
            2 if rate < WARM_EXIT_KEYS_PER_S => 0,
            2 if rate < HOT_EXIT_KEYS_PER_S => 1,
            level => level,
        };
    }

    fn on_scroll(&mut self, delta: f32, now: f32) {
        let can_snack = match self.state {
            PetState::Idle | PetState::MicroWiggle | PetState::Petted => true,
            PetState::Snacking => self.eat_started.is_none(),
            _ => false,
        };
        if !can_snack {
            return;
        }
        if self.state != PetState::Snacking {
            self.peel_notches = 0.0;
            self.eat_started = None;
            self.transition_to(PetState::Snacking);
        }
        self.peel_notches += delta.abs();
        self.last_scroll_time = now;
        if self.peel_notches >= NOTCHES_TO_PEEL {
            self.eat_started = Some(now);
        }
    }

    /// Counts direction reversals of the cursor while it moves over the head.
    fn track_petting(&mut self, from: (f32, f32), to: (f32, f32)) {
        if !self.over_head(to.0, to.1) || !self.over_head(from.0, from.1) {
            self.stroke_dir = 0;
            self.stroke_travel = 0.0;
            return;
        }
        let dx = to.0 - from.0;
        if dx.abs() < 0.5 {
            return;
        }
        let dir = if dx > 0.0 { 1 } else { -1 };
        if dir == self.stroke_dir {
            self.stroke_travel += dx.abs();
            return;
        }
        let min_stroke = self.sprite_width as f32 * PET_MIN_STROKE_FRACTION;
        if self.stroke_dir != 0 && self.stroke_travel >= min_stroke {
            self.strokes.push_back(self.clock);
            self.last_stroke_time = self.clock;
        }
        self.stroke_dir = dir;
        self.stroke_travel = dx.abs();
    }

    fn update_state(&mut self, now: f32, dt: f32, platform_driver: &dyn PlatformDriver) {
        let t = self.state_animation_time;
        match self.state {
            PetState::Idle => {
                self.velocity = (0.0, 0.0);
                self.current_animation_row = self.gaze_sector;
                self.current_animation_frame_index = (t * 2.0) as u32 % 4;

                let (cx, cy) = self.center();
                let cursor_dist = (self.current_mouse_pos.0 - cx).hypot(self.current_mouse_pos.1 - cy);

                if self.strokes.len() >= PET_STROKES_TO_START {
                    self.transition_to(PetState::Petted);
                    info!("Monkey: being petted, purring.");
                } else if self.fast_move_s >= CHASE_TRIGGER_HOLD_S
                    && now >= self.chase_cooldown_until
                    && cursor_dist > CHASE_MIN_DISTANCE_PX
                {
                    self.hunting_speed = CHASE_SPEED_PX_PER_S;
                    self.transition_to(PetState::Hunting);
                    self.chase_deadline = Some(now + CHASE_MAX_S);
                    info!("Monkey: chasing the cursor.");
                } else if self.cursor_inactivity_s >= CURSOR_INACTIVITY_THRESHOLD_S {
                    self.hunting_speed = WALK_SPEED_PX_PER_S;
                    self.transition_to(PetState::Hunting);
                    info!("Monkey: wandering to the idle cursor.");
                } else {
                    self.idle_frame_counter += 1;
                    if self.idle_frame_counter >= MICRO_WIGGLE_INTERVAL_FRAMES {
                        self.idle_frame_counter = 0;
                        if self.rng.gen_bool(0.2) {
                            self.transition_to(PetState::MicroWiggle);
                            self.micro_wiggle_start = Some(now);
                            self.micro_wiggle_base_position = Some(self.position);
                        }
                    }
                }
            }
            PetState::Dragged => {
                self.current_animation_row = ROW_HELD;
                self.current_animation_frame_index = 0;
                if self.is_dragging {
                    self.position.0 = self.current_mouse_pos.0 - self.drag_anchor_offset.0;
                    self.position.1 = self.current_mouse_pos.1 - self.drag_anchor_offset.1;
                }
            }
            PetState::Hunting => {
                // Aim for the cursor, but only as far as the monkey can fully
                // fit on screen; otherwise it would walk into an edge forever.
                let (target_x, target_y) = self.reachable_target(
                    platform_driver.get_cursor_pos(),
                    &platform_driver.get_screen_bounds(),
                );
                let (monkey_cx, monkey_cy) = self.center();
                let dx = target_x - monkey_cx;
                let dy = target_y - monkey_cy;
                let distance = dx.hypot(dy);
                let chasing = self.chase_deadline.is_some();

                if distance < self.hunting_speed * dt || distance < HUNTING_DISTANCE_THRESHOLD_PX {
                    self.position.0 = target_x - self.sprite_width as f32 / 2.0;
                    self.position.1 = target_y - self.sprite_height as f32 / 2.0;
                    if chasing {
                        // Caught it: celebrate with the boop hop.
                        self.transition_to(PetState::Booped);
                        info!("Monkey: caught the cursor!");
                    } else {
                        self.transition_to(PetState::Idle);
                        info!("Monkey: reached the cursor.");
                    }
                } else if self.chase_deadline.map_or(false, |d| now > d) {
                    self.transition_to(PetState::Idle);
                    info!("Monkey: gave up the chase.");
                } else {
                    self.position.0 += dx / distance * self.hunting_speed * dt;
                    self.position.1 += dy / distance * self.hunting_speed * dt;
                    // Look where it's going; cycle the walk-step columns 4-7.
                    let steps_per_s = if chasing { 14.0 } else { 8.0 };
                    self.current_animation_row = Self::sector_towards(dx, dy);
                    self.current_animation_frame_index = 4 + (t * steps_per_s) as u32 % 4;
                }
            }
            PetState::Petted => {
                self.current_animation_row = ROW_PETTED;
                self.current_animation_frame_index = (t * 6.0) as u32 % 8;
                if now - self.last_stroke_time > PET_LINGER_S {
                    self.strokes.clear();
                    self.transition_to(PetState::Idle);
                }
            }
            PetState::Typing => {
                let paw = if now < self.paw_until { self.paw } else { Paw::Ready } as u32;
                match self.typing_heat {
                    0 => {
                        self.current_animation_row = ROW_TYPING;
                        self.current_animation_frame_index = paw;
                    }
                    1 => {
                        self.current_animation_row = ROW_TYPING;
                        self.current_animation_frame_index = 4 + paw;
                    }
                    _ => {
                        let steam_phase = (t * 4.0) as u32 % 2;
                        self.current_animation_row = ROW_TYPING_HOT;
                        self.current_animation_frame_index = steam_phase * 4 + paw;
                    }
                }
                if now - self.last_key_time > TYPING_LINGER_S {
                    self.transition_to(PetState::Idle);
                }
            }
            PetState::Snacking => {
                self.current_animation_row = ROW_SNACK;
                if let Some(start) = self.eat_started {
                    self.current_animation_frame_index = 6 + ((now - start) * 3.0) as u32 % 2;
                    if now - start > EAT_S {
                        self.eat_started = None;
                        self.peel_notches = 0.0;
                        self.transition_to(PetState::Idle);
                    }
                } else {
                    let stage = (self.peel_notches * 6.0 / NOTCHES_TO_PEEL) as u32;
                    self.current_animation_frame_index = stage.min(5);
                    if now - self.last_scroll_time > SNACK_GIVE_UP_S {
                        self.peel_notches = 0.0;
                        self.transition_to(PetState::Idle);
                    }
                }
            }
            PetState::Booped => {
                self.current_animation_row = ROW_BOOP;
                let frame = (t * BOOP_FPS) as u32;
                self.current_animation_frame_index = frame.min(7);
                if frame >= 8 {
                    self.transition_to(PetState::Idle);
                }
            }
            PetState::MicroWiggle => {
                let start = self.micro_wiggle_start.unwrap_or(now);
                let base = self.micro_wiggle_base_position.unwrap_or(self.position);
                self.current_animation_row = self.gaze_sector;
                self.current_animation_frame_index = 0;
                if now - start >= MICRO_WIGGLE_DURATION_S {
                    self.transition_to(PetState::Idle);
                } else {
                    // ±2 px horizontal wiggle at 6 Hz.
                    self.position.0 = base.0 + MICRO_WIGGLE_AMPLITUDE_PX * (t * 2.0 * PI * 6.0).sin();
                    self.position.1 = base.1;
                }
            }
            PetState::Dramatic => {
                self.velocity.1 += GRAVITY_PX_PER_S_SQUARED * dt;
                self.position.0 += self.velocity.0 * dt;
                self.position.1 += self.velocity.1 * dt;
                self.current_animation_row = ROW_HELD;
                self.current_animation_frame_index = 1 + (t * 10.0) as u32 % 7;

                let screens = platform_driver.get_screen_bounds();
                let Some(screen) = Self::screen_under(&screens, self.center().0) else {
                    // No screen information at all: stop falling rather than vanish.
                    self.velocity = (0.0, 0.0);
                    self.transition_to(PetState::Idle);
                    return;
                };
                let (w, h) = (self.sprite_width as f32, self.sprite_height as f32);
                let left = screen.x as f32;
                let right = (screen.x + screen.width as i32) as f32 - w;
                let top = screen.y as f32;
                let floor = (screen.y + screen.height as i32) as f32 - h;

                if self.position.0 < left || self.position.0 > right {
                    self.position.0 = self.position.0.clamp(left, right.max(left));
                    self.velocity.0 = -self.velocity.0 * WALL_BOUNCE;
                }
                if self.position.1 < top && self.velocity.1 < 0.0 {
                    self.position.1 = top;
                    self.velocity.1 = -self.velocity.1 * WALL_BOUNCE;
                }
                if self.position.1 >= floor {
                    self.position.1 = floor;
                    self.velocity = (0.0, 0.0);
                    self.jelly_velocity -= 5.0;
                    self.transition_to(PetState::Idle);
                    info!("Monkey: landed.");
                }
            }
        }
    }

    /// The screen containing x, or the horizontally nearest one.
    fn screen_under(screens: &[Rect], x: f32) -> Option<Rect> {
        let dist = |r: &Rect| {
            let (l, r) = (r.x as f32, (r.x + r.width as i32) as f32);
            if x < l { l - x } else if x >= r { x - r } else { 0.0 }
        };
        screens.iter().copied().min_by(|a, b| dist(a).total_cmp(&dist(b)))
    }

    fn reachable_target(&self, (x, y): (f32, f32), screens: &[Rect]) -> (f32, f32) {
        let Some(screen) = Self::screen_under(screens, x) else {
            return (x, y);
        };
        let (hw, hh) = (self.sprite_width as f32 / 2.0, self.sprite_height as f32 / 2.0);
        let (l, t) = (screen.x as f32 + hw, screen.y as f32 + hh);
        let r = (screen.x + screen.width as i32) as f32 - hw;
        let b = (screen.y + screen.height as i32) as f32 - hh;
        (x.clamp(l, r.max(l)), y.clamp(t, b.max(t)))
    }

    fn update_jelly(&mut self, dt: f32) {
        let target = match self.state {
            PetState::Dragged => {
                let (vx, vy) = self.smoothed_mouse_velocity;
                JELLY_HANG_STRETCH + ((vy.abs() - 0.6 * vx.abs()) * JELLY_DRAG_GAIN).clamp(-0.25, 0.3)
            }
            // Breathing pulse while purring.
            PetState::Petted => 0.02 * (self.state_animation_time * 2.0 * PI).sin(),
            _ => 0.0,
        };
        let dt = dt.min(0.05);
        let accel = (target - self.jelly) * JELLY_STIFFNESS - self.jelly_velocity * JELLY_DAMPING;
        self.jelly_velocity += accel * dt;
        self.jelly = (self.jelly + self.jelly_velocity * dt).clamp(-0.35, 0.45);
        self.scale = (1.0 - 0.5 * self.jelly, 1.0 + self.jelly);
    }

    fn clamp_to_screen_bounds(&mut self, platform_driver: &dyn PlatformDriver) {
        let screen_bounds = platform_driver.get_screen_bounds();
        if screen_bounds.is_empty() {
            warn!("No screen bounds found, cannot clamp monkey position.");
            return;
        }
        let (w, h) = (self.sprite_width as f32, self.sprite_height as f32);
        let (left, top) = self.position;
        let (right, bottom) = (left + w, top + h);

        let mut closest: Option<(f32, Rect)> = None;
        for rect in &screen_bounds {
            let (sl, st) = (rect.x as f32, rect.y as f32);
            let (sr, sb) = (sl + rect.width as f32, st + rect.height as f32);
            if right > sl && left < sr && bottom > st && top < sb {
                self.position = (left.max(sl).min(sr - w), top.max(st).min(sb - h));
                return;
            }
            let d = ((left + right) / 2.0 - (sl + sr) / 2.0).powi(2)
                + ((top + bottom) / 2.0 - (st + sb) / 2.0).powi(2);
            if closest.map_or(true, |(best, _)| d < best) {
                closest = Some((d, *rect));
            }
        }
        if let Some((_, rect)) = closest {
            // Off every screen: snap to the nearest screen's bottom-center.
            self.position = (
                rect.x as f32 + rect.width as f32 / 2.0 - w / 2.0,
                rect.y as f32 + rect.height as f32 - h,
            );
            debug!("Monkey snapped to closest screen: {:?}", rect);
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::config::AppConfig;
    use crate::sprite_renderer::SpriteData;
    use std::cell::Cell;
    use std::io;

    const DT: f32 = 1.0 / 60.0;

    struct MockDriver {
        cursor: Cell<(f32, f32)>,
        screens: Vec<Rect>,
    }

    impl PlatformDriver for MockDriver {
        fn new(_config: &AppConfig) -> io::Result<Self> {
            Ok(Self { cursor: Cell::new((0.0, 0.0)), screens: Vec::new() })
        }
        fn create_window(&mut self, _: u32, _: u32, _: &str) -> io::Result<()> {
            Ok(())
        }
        fn poll_events(&mut self) -> Vec<InputEvent> {
            Vec::new()
        }
        fn render_frame(&mut self, _: &SpriteData) -> io::Result<()> {
            Ok(())
        }
        fn get_screen_bounds(&self) -> Vec<Rect> {
            self.screens.clone()
        }
        fn set_window_position(&mut self, _: i32, _: i32) {}
        fn set_window_size(&mut self, _: u32, _: u32) {}
        fn is_running(&self) -> bool {
            true
        }
        fn log_warning_stderr(&self, _: &str) {}
        fn get_cursor_pos(&self) -> (f32, f32) {
            self.cursor.get()
        }
    }

    /// Monkey at (500, 500), cursor parked just below it.
    fn setup() -> (Monkey, MockDriver) {
        let driver = MockDriver {
            cursor: Cell::new((564.0, 700.0)),
            screens: vec![Rect { x: 0, y: 0, width: 1920, height: 1080 }],
        };
        let mut m = Monkey::new((500.0, 500.0), 128, 128);
        m.tick(&[], DT, &driver);
        (m, driver)
    }

    fn step(m: &mut Monkey, d: &MockDriver, events: &[InputEvent]) {
        for e in events {
            match e {
                InputEvent::MouseMove { x, y }
                | InputEvent::MouseDown { x, y, .. }
                | InputEvent::MouseUp { x, y, .. } => d.cursor.set((*x, *y)),
                _ => {}
            }
        }
        m.tick(events, DT, d);
    }

    fn idle_for(m: &mut Monkey, d: &MockDriver, seconds: f32) {
        for _ in 0..(seconds / DT) as u32 {
            step(m, d, &[]);
        }
    }

    fn key(kind: KeyKind) -> InputEvent {
        InputEvent::KeyDown { key_code: 30, kind }
    }

    fn mv(x: f32, y: f32) -> InputEvent {
        InputEvent::MouseMove { x, y }
    }

    #[test]
    fn idle_gaze_covers_all_eight_directions() {
        let (mut m, d) = setup();
        step(&mut m, &d, &[mv(564.0, 100.0)]); // straight up
        assert_eq!(m.current_animation_row, 6);
        step(&mut m, &d, &[mv(900.0, 200.0)]); // up-right
        assert_eq!(m.current_animation_row, 7);
    }

    #[test]
    fn typing_alternates_paws_and_thumps_on_space() {
        let (mut m, d) = setup();
        step(&mut m, &d, &[key(KeyKind::Other)]);
        assert_eq!(m.state, PetState::Typing);
        assert_eq!((m.current_animation_row, m.current_animation_frame_index), (ROW_TYPING, 1));
        idle_for(&mut m, &d, 0.3);
        step(&mut m, &d, &[key(KeyKind::Other)]);
        assert_eq!(m.current_animation_frame_index, 2);
        idle_for(&mut m, &d, 0.3);
        step(&mut m, &d, &[key(KeyKind::Thump)]);
        assert_eq!(m.current_animation_frame_index, 3);
        idle_for(&mut m, &d, 0.2);
        assert_eq!(m.current_animation_frame_index, 0, "paws return to ready");
        idle_for(&mut m, &d, TYPING_LINGER_S);
        assert_eq!(m.state, PetState::Idle);
    }

    #[test]
    fn fast_typing_overheats_and_cools_down() {
        let (mut m, d) = setup();
        // ~12 keys per second for two seconds
        for i in 0..120 {
            let events = if i % 5 == 0 { vec![key(KeyKind::Other)] } else { vec![] };
            step(&mut m, &d, &events);
        }
        assert_eq!(m.current_animation_row, ROW_TYPING_HOT);
        // slow down to ~4 keys per second: steam stops, still flushed or calm
        for i in 0..120 {
            let events = if i % 15 == 0 { vec![key(KeyKind::Other)] } else { vec![] };
            step(&mut m, &d, &events);
        }
        assert_eq!(m.current_animation_row, ROW_TYPING);
        idle_for(&mut m, &d, 2.0);
        assert_eq!(m.state, PetState::Idle);
    }

    #[test]
    fn quick_click_boops_then_returns_to_idle() {
        let (mut m, d) = setup();
        let down = InputEvent::MouseDown { x: 560.0, y: 560.0, button: MouseButton::Left };
        let up = InputEvent::MouseUp { x: 561.0, y: 560.0, button: MouseButton::Left };
        step(&mut m, &d, &[down]);
        assert_eq!(m.state, PetState::Dragged);
        step(&mut m, &d, &[up]);
        assert_eq!(m.state, PetState::Booped);
        idle_for(&mut m, &d, 1.0);
        assert_eq!(m.state, PetState::Idle);
    }

    #[test]
    fn slow_drag_puts_down_and_fast_release_throws() {
        let (mut m, d) = setup();
        step(&mut m, &d, &[InputEvent::MouseDown { x: 560.0, y: 520.0, button: MouseButton::Left }]);
        for i in 1..=60 {
            step(&mut m, &d, &[mv(560.0 + i as f32 * 2.0, 520.0)]);
        }
        assert!((m.position.0 - 620.0).abs() < 1.0, "follows the cursor");
        step(&mut m, &d, &[InputEvent::MouseUp { x: 680.0, y: 520.0, button: MouseButton::Left }]);
        assert_eq!(m.state, PetState::Idle);

        step(&mut m, &d, &[InputEvent::MouseDown { x: 680.0, y: 560.0, button: MouseButton::Left }]);
        for i in 1..=10 {
            step(&mut m, &d, &[mv(680.0 + i as f32 * 50.0, 560.0 - i as f32 * 10.0)]);
        }
        step(&mut m, &d, &[InputEvent::MouseUp { x: 1180.0, y: 460.0, button: MouseButton::Left }]);
        assert_eq!(m.state, PetState::Dramatic);
        idle_for(&mut m, &d, 5.0);
        assert_eq!(m.state, PetState::Idle, "lands on the floor");
        assert!((m.position.1 - (1080.0 - 128.0)).abs() < 1.0);
    }

    #[test]
    fn dragging_stretches_like_mochi_and_springs_back() {
        let (mut m, d) = setup();
        step(&mut m, &d, &[InputEvent::MouseDown { x: 560.0, y: 520.0, button: MouseButton::Left }]);
        for i in 1..=20 {
            step(&mut m, &d, &[mv(560.0, 520.0 + i as f32 * 15.0)]);
        }
        assert!(m.scale.1 > 1.1, "stretches when pulled vertically: {:?}", m.scale);
        assert!((m.scale.0 - (1.0 - 0.5 * (m.scale.1 - 1.0))).abs() < 1e-4, "volume rule");
        step(&mut m, &d, &[InputEvent::MouseUp { x: 560.0, y: 820.0, button: MouseButton::Left }]);
        idle_for(&mut m, &d, 2.0);
        assert!((m.scale.1 - 1.0).abs() < 0.01);
    }

    #[test]
    fn stroking_the_head_starts_purring() {
        let (mut m, d) = setup();
        let y = 540.0; // head height
        for stroke in 0..5 {
            for i in 0..15 {
                let x = if stroke % 2 == 0 { 530.0 + i as f32 * 4.0 } else { 590.0 - i as f32 * 4.0 };
                step(&mut m, &d, &[mv(x, y)]);
            }
        }
        assert_eq!(m.state, PetState::Petted);
        assert_eq!(m.current_animation_row, ROW_PETTED);
        idle_for(&mut m, &d, PET_LINGER_S + 0.1);
        assert_eq!(m.state, PetState::Idle);
    }

    #[test]
    fn hovering_still_over_the_monkey_is_not_petting() {
        let (mut m, d) = setup();
        step(&mut m, &d, &[mv(560.0, 540.0)]);
        idle_for(&mut m, &d, 3.0);
        assert_eq!(m.state, PetState::Idle);
    }

    #[test]
    fn fast_mouse_flick_triggers_a_chase_that_ends_in_a_catch() {
        let (mut m, d) = setup();
        for i in 0..20 {
            step(&mut m, &d, &[mv(600.0 + i as f32 * 60.0, 600.0)]);
        }
        assert_eq!(m.state, PetState::Hunting);
        assert!((4..8).contains(&m.current_animation_frame_index), "walk-step columns");
        let mut caught = false;
        for _ in 0..240 {
            step(&mut m, &d, &[]);
            if m.state == PetState::Booped {
                caught = true;
                break;
            }
        }
        assert!(caught);
        idle_for(&mut m, &d, 1.0);
        // A second flick right away is ignored (cooldown).
        for i in 0..20 {
            step(&mut m, &d, &[mv(1700.0 - i as f32 * 60.0, 300.0)]);
        }
        assert_ne!(m.state, PetState::Hunting);
    }

    #[test]
    fn scrolling_peels_a_banana_then_eats_it() {
        let (mut m, d) = setup();
        step(&mut m, &d, &[InputEvent::Scroll { delta: 1.0 }]);
        assert_eq!((m.state, m.current_animation_row, m.current_animation_frame_index), (PetState::Snacking, ROW_SNACK, 0));
        for _ in 0..5 {
            step(&mut m, &d, &[InputEvent::Scroll { delta: -1.0 }]);
        }
        assert_eq!(m.current_animation_frame_index, 3);
        for _ in 0..6 {
            step(&mut m, &d, &[InputEvent::Scroll { delta: 1.0 }]);
        }
        assert!(m.current_animation_frame_index >= 6, "eating");
        idle_for(&mut m, &d, EAT_S + 0.1);
        assert_eq!(m.state, PetState::Idle);
    }

    #[test]
    fn abandoned_snack_is_put_away() {
        let (mut m, d) = setup();
        step(&mut m, &d, &[InputEvent::Scroll { delta: 3.0 }]);
        idle_for(&mut m, &d, SNACK_GIVE_UP_S + 0.1);
        assert_eq!(m.state, PetState::Idle);
    }

    #[test]
    fn typing_interrupts_snacking() {
        let (mut m, d) = setup();
        step(&mut m, &d, &[InputEvent::Scroll { delta: 3.0 }]);
        step(&mut m, &d, &[key(KeyKind::Other)]);
        assert_eq!(m.state, PetState::Typing);
    }

    fn fling(m: &mut Monkey, d: &MockDriver, from: (f32, f32), step_px: (f32, f32)) {
        step(m, d, &[InputEvent::MouseDown { x: from.0, y: from.1, button: MouseButton::Left }]);
        let mut p = from;
        for _ in 0..8 {
            p = (p.0 + step_px.0, p.1 + step_px.1);
            step(m, d, &[mv(p.0, p.1)]);
        }
        step(m, d, &[InputEvent::MouseUp { x: p.0, y: p.1, button: MouseButton::Left }]);
    }

    fn assert_on_floor_and_visible(m: &Monkey) {
        assert_eq!(m.state, PetState::Idle, "landed");
        assert!((m.position.1 - (1080.0 - 128.0)).abs() < 0.5, "on the floor: {:?}", m.position);
        assert!(m.position.0 >= 0.0 && m.position.0 <= 1920.0 - 128.0, "on screen: {:?}", m.position);
    }

    #[test]
    fn thrown_into_a_wall_bounces_and_lands_on_the_floor() {
        let (mut m, d) = setup();
        fling(&mut m, &d, (560.0, 560.0), (60.0, -5.0));
        assert_eq!(m.state, PetState::Dramatic);
        let mut bounced = false;
        for _ in 0..(6.0 / DT) as u32 {
            step(&mut m, &d, &[]);
            bounced |= m.velocity.0 < 0.0;
            if m.state != PetState::Dramatic {
                break;
            }
        }
        assert!(bounced, "bounced off the right wall");
        assert_on_floor_and_visible(&m);
    }

    #[test]
    fn thrown_at_the_ceiling_bounces_down_quickly() {
        let (mut m, d) = setup();
        fling(&mut m, &d, (560.0, 560.0), (0.0, -60.0));
        assert_eq!(m.state, PetState::Dramatic);
        idle_for(&mut m, &d, 3.0);
        assert_on_floor_and_visible(&m);
    }

    #[test]
    fn dropped_off_screen_is_brought_back() {
        let (mut m, d) = setup();
        step(&mut m, &d, &[InputEvent::MouseDown { x: 560.0, y: 560.0, button: MouseButton::Left }]);
        for i in 1..=100 {
            step(&mut m, &d, &[mv(560.0 + i as f32 * 25.0, 560.0 + i as f32 * 20.0)]);
        }
        for _ in 0..30 {
            step(&mut m, &d, &[]);
        }
        step(&mut m, &d, &[InputEvent::MouseUp { x: 3060.0, y: 2560.0, button: MouseButton::Left }]);
        idle_for(&mut m, &d, 0.5);
        assert!(m.position.0 >= 0.0 && m.position.0 <= 1920.0 - 128.0, "{:?}", m.position);
        assert!(m.position.1 >= 0.0 && m.position.1 <= 1080.0 - 128.0, "{:?}", m.position);
    }

    #[test]
    fn thrown_with_no_screen_info_stops_instead_of_falling_forever() {
        let (mut m, mut d) = setup();
        d.screens.clear();
        fling(&mut m, &d, (560.0, 560.0), (60.0, 0.0));
        idle_for(&mut m, &d, 0.2);
        assert_ne!(m.state, PetState::Dramatic);
    }

    #[test]
    fn chasing_a_cursor_in_the_corner_ends_at_the_screen_edge() {
        let (mut m, d) = setup();
        for i in 0..20 {
            step(&mut m, &d, &[mv(600.0 + i as f32 * 66.0, 600.0 - i as f32 * 30.0)]);
        }
        assert_eq!(m.state, PetState::Hunting);
        d.cursor.set((1919.0, 0.0));
        idle_for(&mut m, &d, 3.0);
        assert_ne!(m.state, PetState::Hunting, "reached the corner");
        assert!((m.position.0 - (1920.0 - 128.0)).abs() < 1.0 && m.position.1.abs() < 1.0, "{:?}", m.position);
    }
}
