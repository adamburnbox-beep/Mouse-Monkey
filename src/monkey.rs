use crate::platform::{InputEvent, MouseButton, PlatformDriver, Rect};
use crate::state::PetState;
use log::{debug, info, warn};
use rand::{thread_rng, Rng};
use std::f32::consts::PI;
use std::time::Instant;

const TAU: f32 = 2.0 * PI;

// ── Drag squish/stretch (03. Mochi Drag) ─────────────────────────────────────
const K_STRETCH: f32 = 0.005;
const K_SQUISH_FACTOR: f32 = 0.5;

// ── Fluid follow tuning ───────────────────────────────────────────────────────
/// Cursor distance (centre→cursor, px) beyond which the monkey starts walking.
const FOLLOW_TRIGGER_DISTANCE_PX: f32 = 130.0;
/// Resting gap the monkey keeps from the cursor; it stops walking here.
/// Kept below the trigger distance so the two thresholds form a hysteresis band
/// and the monkey doesn't flip-flop between Idle and Following.
const FOLLOW_STOP_DISTANCE_PX: f32 = 64.0;
/// Exponential follow responsiveness (higher = snappier, lower = lazier glide).
const FOLLOW_RESPONSIVENESS: f32 = 6.0;
/// Hard cap on walk speed so a large cursor jump glides instead of teleporting.
const MAX_FOLLOW_SPEED_PX_PER_SEC: f32 = 1100.0;
/// Speed (px/s) at which the walk bounce reaches full amplitude.
const WALK_FULL_BOUNCE_SPEED: f32 = 240.0;
const WALK_BOUNCE_FREQ_HZ: f32 = 4.0;
const WALK_BOUNCE_AMPLITUDE_PX: f32 = 7.0;

// ── Idle behaviours ───────────────────────────────────────────────────────────
const GROOMING_VELOCITY_THRESHOLD: f32 = 0.05;
const GROOMING_FRAMES_THRESHOLD: u32 = 120;
const MICRO_WIGGLE_INTERVAL_FRAMES: u64 = 500;
const MICRO_WIGGLE_DURATION_MS: u64 = 400;
const MICRO_WIGGLE_AMPLITUDE_PX: f32 = 2.0;

// ── Typing dance (06. Keyboard) ───────────────────────────────────────────────
/// Time after the last keystroke before the monkey stops hopping.
const TYPING_DECAY_MS: u64 = 600;
/// Hops per second while typing — each hop lands on the opposite foot.
const TYPING_HOP_FREQ_HZ: f32 = 7.0;
/// Peak hop height in px.
const TYPING_HOP_AMPLITUDE_PX: f32 = 16.0;
/// Sideways lean as weight shifts from one foot to the other.
const TYPING_FOOT_LEAN_PX: f32 = 7.0;

// ── Dramatic fling (08) ───────────────────────────────────────────────────────
const DRAMATIC_ACCELERATION_THRESHOLD: f32 = 100.0;
const GRAVITY_PX_PER_S_SQUARED: f32 = 9.8 * 60.0;

pub struct Monkey {
    pub state: PetState,
    pub position: (f32, f32), // Top-left corner of the monkey sprite (physics).
    pub velocity: (f32, f32),
    pub current_animation_frame_index: u32,
    pub current_animation_row: u32,
    pub scale: (f32, f32), // (scale_x, scale_y) for stretching/squishing.
    pub rotation: f32,     // For dramatic fall (radians).

    /// Render-time offset (walk bounce, typing hop/lean, wiggle) layered on top
    /// of `position`. Kept separate so it never feeds back into physics or the
    /// screen-bounds clamp.
    visual_offset: (f32, f32),
    /// -1.0 = facing left, 1.0 = facing right.
    facing: f32,
    /// Continuous phase for the walk bounce, wrapped to [0, TAU).
    walk_phase: f32,
    /// Continuous phase for the typing hop; each whole unit is one hop/foot.
    typing_phase: f32,

    // Timers and counters
    grooming_frames_count: u32,
    key_decay_timer: Option<Instant>,
    micro_wiggle_active_timer: Option<Instant>,
    idle_frame_counter: u64,

    // Mouse state for drag and acceleration
    is_dragging: bool,
    drag_anchor_offset: (f32, f32),
    previous_mouse_pos: (f32, f32),
    current_mouse_pos: (f32, f32),
    previous_mouse_velocity: (f32, f32),
    current_mouse_velocity: (f32, f32),
    mouse_down_pos: Option<(f32, f32)>,

    k_stretch: f32,
    k_squish: f32,
    /// Accumulates elapsed time within the current state for animation frame
    /// cycling. Reset to 0.0 whenever the state changes.
    state_animation_time: f32,

    rng: rand::rngs::ThreadRng,

    sprite_width: u32,
    sprite_height: u32,
}

impl Monkey {
    pub fn new(initial_pos: (f32, f32), sprite_width: u32, sprite_height: u32) -> Self {
        // Seed the tracked cursor at the monkey's own centre so it starts calm
        // (Idle) instead of lurching toward a stale (0,0) cursor on the first tick.
        let initial_cursor = (
            initial_pos.0 + sprite_width as f32 / 2.0,
            initial_pos.1 + sprite_height as f32 / 2.0,
        );
        Self {
            state: PetState::Idle,
            position: initial_pos,
            velocity: (0.0, 0.0),
            current_animation_frame_index: 0,
            current_animation_row: 0,
            scale: (1.0, 1.0),
            rotation: 0.0,

            visual_offset: (0.0, 0.0),
            facing: 1.0,
            walk_phase: 0.0,
            typing_phase: 0.0,

            grooming_frames_count: 0,
            key_decay_timer: None,
            micro_wiggle_active_timer: None,
            idle_frame_counter: 0,

            is_dragging: false,
            drag_anchor_offset: (0.0, 0.0),
            previous_mouse_pos: initial_cursor,
            current_mouse_pos: initial_cursor,
            previous_mouse_velocity: (0.0, 0.0),
            current_mouse_velocity: (0.0, 0.0),
            mouse_down_pos: None,

            k_stretch: K_STRETCH,
            k_squish: K_STRETCH * K_SQUISH_FACTOR,
            state_animation_time: 0.0,

            rng: thread_rng(),

            sprite_width,
            sprite_height,
        }
    }

    /// Screen position the sprite should be drawn at: physics position plus the
    /// transient bounce/lean offset.
    pub fn render_position(&self) -> (f32, f32) {
        (
            self.position.0 + self.visual_offset.0,
            self.position.1 + self.visual_offset.1,
        )
    }

    // Helper to check if a point is within the monkey's bounding box.
    fn contains_point(&self, x: f32, y: f32) -> bool {
        x >= self.position.0
            && x <= self.position.0 + self.sprite_width as f32 * self.scale.0
            && y >= self.position.1
            && y <= self.position.1 + self.sprite_height as f32 * self.scale.1
    }

    // Helper to get the monkey's center.
    fn center(&self) -> (f32, f32) {
        (
            self.position.0 + (self.sprite_width as f32 * self.scale.0) / 2.0,
            self.position.1 + (self.sprite_height as f32 * self.scale.1) / 2.0,
        )
    }

    // Helper to calculate eye direction (02. Eye Follow).
    fn calculate_eye_direction(&self, mouse_x: f32, mouse_y: f32) -> u32 {
        let (cx, cy) = self.center();
        let angle = (mouse_y - cy).atan2(mouse_x - cx); // -PI..PI
        let sector_size = TAU / 8.0; // 45 degrees
        let normalized_angle = (angle + TAU) % TAU;
        ((normalized_angle + PI / 8.0) / sector_size).floor() as u32 % 8
    }

    /// Transition to a new state and reset the per-state animation clock.
    fn transition_to(&mut self, new_state: PetState) {
        self.state = new_state;
        self.state_animation_time = 0.0;
    }

    pub fn tick(&mut self, input_events: &[InputEvent], dt: f32, platform_driver: &dyn PlatformDriver) {
        let current_time = Instant::now();
        let dt = dt.max(0.0001); // Guard against zero/negative dt.

        // Update mouse positions and velocities from input events.
        self.previous_mouse_pos = self.current_mouse_pos;
        self.previous_mouse_velocity = self.current_mouse_velocity;

        let mut mouse_moved_this_tick = false;
        for event in input_events {
            match event {
                InputEvent::MouseMove { x, y } => {
                    self.current_mouse_pos = (*x, *y);
                    self.current_mouse_velocity = (
                        (self.current_mouse_pos.0 - self.previous_mouse_pos.0) / dt,
                        (self.current_mouse_pos.1 - self.previous_mouse_pos.1) / dt,
                    );
                    mouse_moved_this_tick = true;
                }
                InputEvent::MouseDown { x, y, button } => {
                    if *button == MouseButton::Left && self.contains_point(*x, *y) {
                        self.transition_to(PetState::Dragged);
                        self.is_dragging = true;
                        self.drag_anchor_offset = (*x - self.position.0, *y - self.position.1);
                        self.mouse_down_pos = Some((*x, *y));
                        // Sync current_mouse_pos so the Dragged branch (same tick)
                        // doesn't use a stale position and snap the sprite away.
                        self.current_mouse_pos = (*x, *y);
                        self.velocity = (0.0, 0.0);
                        self.scale = (1.0, 1.0);
                        self.rotation = 0.0;
                        info!("Monkey: Transitioned to Dragged state.");
                    }
                }
                InputEvent::MouseUp { .. } => {
                    if self.is_dragging {
                        let acceleration_magnitude = ((self.current_mouse_velocity.0
                            - self.previous_mouse_velocity.0)
                            .powi(2)
                            + (self.current_mouse_velocity.1
                                - self.previous_mouse_velocity.1)
                                .powi(2))
                        .sqrt()
                            / dt;

                        if acceleration_magnitude > DRAMATIC_ACCELERATION_THRESHOLD {
                            self.transition_to(PetState::Dramatic);
                            self.velocity = self.current_mouse_velocity;
                            self.rotation = self
                                .current_mouse_velocity
                                .0
                                .atan2(-self.current_mouse_velocity.1);
                            info!("Monkey: Transitioned to Dramatic state (high acceleration).");
                        } else {
                            self.transition_to(PetState::Idle);
                            info!("Monkey: Transitioned to Idle from Dragged.");
                        }
                        self.is_dragging = false;
                        self.mouse_down_pos = None;
                        self.scale = (1.0, 1.0);
                    }
                }
                InputEvent::KeyDown { .. } => {
                    // Typing interrupts everything except an active drag or fall.
                    if self.state != PetState::Dragged && self.state != PetState::Dramatic {
                        if self.state != PetState::Typing {
                            self.transition_to(PetState::Typing);
                            self.typing_phase = 0.0;
                        }
                        self.key_decay_timer = Some(current_time);
                    }
                }
                _ => {}
            }
        }

        // When no MouseMove events arrive this tick (e.g. at startup, or evdev is
        // inactive and the pointer is outside the input region), poll the driver
        // for the last known cursor position so the monkey still tracks it.
        if !mouse_moved_this_tick {
            let (px, py) = platform_driver.get_cursor_pos();
            if (px - self.current_mouse_pos.0).abs() > 0.5
                || (py - self.current_mouse_pos.1).abs() > 0.5
            {
                self.previous_mouse_pos = self.current_mouse_pos;
                self.current_mouse_pos = (px, py);
                self.current_mouse_velocity = (
                    (self.current_mouse_pos.0 - self.previous_mouse_pos.0) / dt,
                    (self.current_mouse_pos.1 - self.previous_mouse_pos.1) / dt,
                );
            } else {
                // Cursor is genuinely still — decay tracked velocity to zero so
                // grooming/idle thresholds settle.
                self.current_mouse_velocity = (0.0, 0.0);
            }
        }

        // Advance per-state animation clock every tick.
        self.state_animation_time += dt;

        // Reset the transient render offset; states that bounce re-set it below.
        self.visual_offset = (0.0, 0.0);

        match self.state {
            PetState::Idle => {
                self.velocity = (0.0, 0.0);
                self.scale = (1.0, 1.0);
                self.rotation = 0.0;
                self.walk_phase = 0.0;

                let (cx, cy) = self.center();
                let dx = self.current_mouse_pos.0 - cx;
                let dy = self.current_mouse_pos.1 - cy;
                let dist = (dx * dx + dy * dy).sqrt();

                // Start following once the cursor strays past the trigger ring.
                if dist > FOLLOW_TRIGGER_DISTANCE_PX {
                    self.transition_to(PetState::Following);
                } else {
                    // Eye-follow facing + gentle breathing/blink cycle.
                    let sector = self.calculate_eye_direction(
                        self.current_mouse_pos.0,
                        self.current_mouse_pos.1,
                    );
                    self.current_animation_row = sector.min(5);
                    self.current_animation_frame_index =
                        (self.state_animation_time * 2.0) as u32 % 4;

                    // Grooming: cursor resting still over the monkey.
                    if self.contains_point(self.current_mouse_pos.0, self.current_mouse_pos.1) {
                        let mouse_v = (self.current_mouse_velocity.0.powi(2)
                            + self.current_mouse_velocity.1.powi(2))
                        .sqrt();
                        if mouse_v < GROOMING_VELOCITY_THRESHOLD {
                            self.grooming_frames_count += 1;
                            if self.grooming_frames_count >= GROOMING_FRAMES_THRESHOLD {
                                self.transition_to(PetState::Grooming);
                                self.grooming_frames_count = 0;
                                info!("Monkey: Transitioned to Grooming state.");
                            }
                        } else {
                            self.grooming_frames_count = 0;
                        }
                    } else {
                        self.grooming_frames_count = 0;
                    }

                    // Micro-Wiggles: occasional idle jiggle.
                    self.idle_frame_counter += 1;
                    if self.idle_frame_counter >= MICRO_WIGGLE_INTERVAL_FRAMES {
                        self.idle_frame_counter = 0;
                        if self.rng.gen_bool(0.2) {
                            self.transition_to(PetState::MicroWiggle);
                            self.micro_wiggle_active_timer = Some(current_time);
                            info!("Monkey: Transitioned to MicroWiggle state.");
                        }
                    }
                }
            }
            PetState::Following => {
                self.scale = (1.0, 1.0);
                self.rotation = 0.0;

                let (cursor_x, cursor_y) = self.current_mouse_pos;
                let (cx, cy) = self.center();
                let dx = cursor_x - cx;
                let dy = cursor_y - cy;
                let dist = (dx * dx + dy * dy).sqrt();

                if dist <= FOLLOW_STOP_DISTANCE_PX {
                    // Arrived within the resting gap: stop here and idle. Because
                    // we ease toward the cursor itself (below), the monkey crosses
                    // this threshold while still moving inward, so the transition
                    // fires cleanly instead of hovering on the boundary.
                    self.velocity = (0.0, 0.0);
                    self.transition_to(PetState::Idle);
                } else {
                    // Ease the centre toward the cursor; the stop check above ends
                    // the walk once the resting gap is reached.
                    let half_w = self.sprite_width as f32 * self.scale.0 / 2.0;
                    let half_h = self.sprite_height as f32 * self.scale.1 / 2.0;
                    let desired_x = cursor_x - half_w;
                    let desired_y = cursor_y - half_h;

                    // Frame-rate-independent exponential easing toward the target:
                    // fast when far, naturally easing out as it arrives.
                    let smoothing = 1.0 - (-dt * FOLLOW_RESPONSIVENESS).exp();
                    let mut step_x = (desired_x - self.position.0) * smoothing;
                    let mut step_y = (desired_y - self.position.1) * smoothing;

                    // Clamp per-tick movement to the max speed.
                    let max_step = MAX_FOLLOW_SPEED_PX_PER_SEC * dt;
                    let step_len = (step_x * step_x + step_y * step_y).sqrt();
                    if step_len > max_step && step_len > 0.0 {
                        let s = max_step / step_len;
                        step_x *= s;
                        step_y *= s;
                    }

                    self.position.0 += step_x;
                    self.position.1 += step_y;
                    self.velocity = (step_x / dt, step_y / dt);

                    if dx.abs() > 6.0 {
                        self.facing = if dx > 0.0 { 1.0 } else { -1.0 };
                    }

                    // Walk bounce — amplitude scales with current speed so slow
                    // creeping barely bobs while a dash hops energetically.
                    let speed =
                        (self.velocity.0.powi(2) + self.velocity.1.powi(2)).sqrt();
                    let speed_factor = (speed / WALK_FULL_BOUNCE_SPEED).clamp(0.0, 1.0);
                    self.walk_phase =
                        (self.walk_phase + dt * WALK_BOUNCE_FREQ_HZ * TAU) % TAU;
                    self.visual_offset.1 =
                        -(self.walk_phase.sin().abs()) * WALK_BOUNCE_AMPLITUDE_PX * speed_factor;

                    // Face the travel direction (8-way) and cycle walk frames.
                    let angle = dy.atan2(dx);
                    let norm = (angle + TAU) % TAU;
                    let sector = ((norm + PI / 8.0) / (TAU / 8.0)).floor() as u32 % 8;
                    self.current_animation_row = sector;
                    self.current_animation_frame_index =
                        (self.state_animation_time * 8.0) as u32 % 8;
                }
            }
            PetState::Dragged => {
                // Row 7 = surprise/falling poses. Col 0 = arms-wide surprised.
                self.current_animation_row = 7;
                self.current_animation_frame_index = 0;
                if self.is_dragging {
                    self.position.0 = self.current_mouse_pos.0 - self.drag_anchor_offset.0;
                    self.position.1 = self.current_mouse_pos.1 - self.drag_anchor_offset.1;

                    if let Some(down_pos) = self.mouse_down_pos {
                        let delta_y = self.current_mouse_pos.1 - down_pos.1;
                        self.scale.1 = 1.0 + (delta_y * self.k_stretch);
                        self.scale.0 = 1.0 - (delta_y * self.k_squish);
                        self.scale.0 = self.scale.0.max(0.5).min(1.5);
                        self.scale.1 = self.scale.1.max(0.5).min(1.5);
                    }
                }
            }
            PetState::Grooming => {
                // Gentle breathing scale pulse at 1 Hz.
                let breathing_scale =
                    1.0 + 0.02 * (self.state_animation_time * TAU).sin();
                self.scale = (breathing_scale, breathing_scale);

                let mouse_v = (self.current_mouse_velocity.0.powi(2)
                    + self.current_mouse_velocity.1.powi(2))
                .sqrt();
                if !self.contains_point(self.current_mouse_pos.0, self.current_mouse_pos.1)
                    || mouse_v > GROOMING_VELOCITY_THRESHOLD
                {
                    self.transition_to(PetState::Idle);
                    self.scale = (1.0, 1.0);
                    info!("Monkey: Transitioned to Idle from Grooming.");
                }
                // Row 6 = grooming/preening animation. Cycle cols 0-7 at 6 Hz.
                self.current_animation_row = 6;
                self.current_animation_frame_index =
                    (self.state_animation_time * 6.0) as u32 % 8;
            }
            PetState::Typing => {
                self.scale = (1.0, 1.0);
                self.rotation = 0.0;

                // One arch of the hop per whole `typing_phase` unit; the landing
                // foot alternates each unit, so the monkey appears to stomp the
                // keyboard one foot at a time.
                self.typing_phase += dt * TYPING_HOP_FREQ_HZ;
                let hop = (self.typing_phase * PI).sin().abs();
                self.visual_offset.1 = -hop * TYPING_HOP_AMPLITUDE_PX;
                let foot = if (self.typing_phase as i64) % 2 == 0 { -1.0 } else { 1.0 };
                self.visual_offset.0 = foot * TYPING_FOOT_LEAN_PX;

                // Row 7 = active/surprised poses; alternate the column with the
                // foot so the body pose flips with each stomp.
                self.current_animation_row = 7;
                self.current_animation_frame_index = if foot < 0.0 { 1 } else { 2 };

                let expired = self
                    .key_decay_timer
                    .map_or(true, |t| (current_time - t).as_millis() as u64 >= TYPING_DECAY_MS);
                if expired {
                    self.key_decay_timer = None;
                    self.typing_phase = 0.0;
                    self.transition_to(PetState::Idle);
                    info!("Monkey: Transitioned to Idle from Typing.");
                }
            }
            PetState::MicroWiggle => {
                if let Some(timer_start) = self.micro_wiggle_active_timer {
                    let elapsed_ms = (current_time - timer_start).as_millis() as u64;
                    if elapsed_ms >= MICRO_WIGGLE_DURATION_MS {
                        self.micro_wiggle_active_timer = None;
                        self.transition_to(PetState::Idle);
                        info!("Monkey: Transitioned to Idle from MicroWiggle.");
                    } else {
                        // ±2 px horizontal twitch at 6 Hz, applied as a pure
                        // render offset (no physics drift).
                        self.visual_offset.0 = MICRO_WIGGLE_AMPLITUDE_PX
                            * (self.state_animation_time * TAU * 6.0).sin();
                        self.current_animation_row = 5;
                        self.current_animation_frame_index = 0;
                    }
                } else {
                    self.transition_to(PetState::Idle);
                }
            }
            PetState::Dramatic => {
                // Apply gravity and integrate position.
                self.velocity.1 += GRAVITY_PX_PER_S_SQUARED * dt;
                self.position.0 += self.velocity.0 * dt;
                self.position.1 += self.velocity.1 * dt;
                self.rotation += self.velocity.0 * 0.001 * dt;

                // Tumbling frames: Row 7, cycling cols 0-7 at 5 Hz.
                self.current_animation_row = 7;
                self.current_animation_frame_index =
                    (self.state_animation_time * 5.0) as u32 % 8;

                // Detect floor collision against the mapped monitor bounds.
                let screen_bounds = platform_driver.get_screen_bounds();
                let mut collided = false;
                if !screen_bounds.is_empty() {
                    let mut floor_y = f32::MAX;
                    for rect in &screen_bounds {
                        if self.position.0 < rect.x as f32 + rect.width as f32
                            && self.position.0 + self.sprite_width as f32 * self.scale.0
                                > rect.x as f32
                        {
                            floor_y = floor_y.min(
                                rect.y as f32 + rect.height as f32
                                    - self.sprite_height as f32 * self.scale.1,
                            );
                        }
                    }
                    if self.position.1 >= floor_y {
                        collided = true;
                    }
                }
                if collided {
                    self.transition_to(PetState::Idle);
                    self.velocity = (0.0, 0.0);
                    self.rotation = 0.0;
                    self.scale = (1.0, 1.0);
                    info!("Monkey: Transitioned to Idle from Dramatic (floor collision).");
                }
            }
        }

        // Clamp to screen bounds for all states except Dragged (cursor-driven).
        if self.state != PetState::Dragged {
            self.clamp_to_screen_bounds(platform_driver);
        }
    }

    fn clamp_to_screen_bounds(&mut self, platform_driver: &dyn PlatformDriver) {
        let screen_bounds = platform_driver.get_screen_bounds();
        if screen_bounds.is_empty() {
            warn!("No screen bounds found, cannot clamp monkey position.");
            return;
        }

        let mut clamped_x = self.position.0;
        let mut clamped_y = self.position.1;

        let mut is_within_any_screen = false;
        let mut closest_dist_sq = f32::MAX;
        let mut closest_rect: Option<Rect> = None;

        for rect in &screen_bounds {
            let screen_left = rect.x as f32;
            let screen_top = rect.y as f32;
            let screen_right = rect.x as f32 + rect.width as f32;
            let screen_bottom = rect.y as f32 + rect.height as f32;

            let monkey_left = self.position.0;
            let monkey_top = self.position.1;
            let monkey_right = self.position.0 + self.sprite_width as f32 * self.scale.0;
            let monkey_bottom = self.position.1 + self.sprite_height as f32 * self.scale.1;

            if monkey_right > screen_left
                && monkey_left < screen_right
                && monkey_bottom > screen_top
                && monkey_top < screen_bottom
            {
                is_within_any_screen = true;
                clamped_x = monkey_left
                    .max(screen_left)
                    .min(screen_right - self.sprite_width as f32 * self.scale.0);
                clamped_y = monkey_top
                    .max(screen_top)
                    .min(screen_bottom - self.sprite_height as f32 * self.scale.1);
                break;
            }

            let monkey_center_x = (monkey_left + monkey_right) / 2.0;
            let monkey_center_y = (monkey_top + monkey_bottom) / 2.0;
            let screen_center_x = (screen_left + screen_right) / 2.0;
            let screen_center_y = (screen_top + screen_bottom) / 2.0;

            let dx = monkey_center_x - screen_center_x;
            let dy = monkey_center_y - screen_center_y;
            let dist_sq = dx * dx + dy * dy;

            if dist_sq < closest_dist_sq {
                closest_dist_sq = dist_sq;
                closest_rect = Some(*rect);
            }
        }

        if !is_within_any_screen {
            if let Some(rect) = closest_rect {
                clamped_x = rect.x as f32 + (rect.width as f32 / 2.0)
                    - (self.sprite_width as f32 * self.scale.0 / 2.0);
                clamped_y = rect.y as f32 + rect.height as f32
                    - (self.sprite_height as f32 * self.scale.1);
                debug!("Monkey snapped to closest screen: {:?}", rect);
            } else {
                warn!("Could not find a screen to snap the monkey to. Defaulting to (0,0).");
                clamped_x = 0.0;
                clamped_y = 0.0;
            }
        }

        self.position = (clamped_x, clamped_y);
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::config::AppConfig;
    use crate::sprite_renderer::SpriteData;
    use std::io;

    const W: u32 = 128;
    const H: u32 = 128;
    const DT: f32 = 1.0 / 60.0;

    /// Minimal PlatformDriver for headless logic tests: one big monitor and a
    /// fixed cursor position.
    struct MockDriver {
        cursor: (f32, f32),
    }

    impl PlatformDriver for MockDriver {
        fn new(_config: &AppConfig) -> io::Result<Self> {
            Ok(Self { cursor: (0.0, 0.0) })
        }
        fn create_window(&mut self, _w: u32, _h: u32, _t: &str) -> io::Result<()> {
            Ok(())
        }
        fn poll_events(&mut self) -> Vec<InputEvent> {
            Vec::new()
        }
        fn render_frame(&mut self, _s: &SpriteData) -> io::Result<()> {
            Ok(())
        }
        fn get_screen_bounds(&self) -> Vec<Rect> {
            vec![Rect { x: 0, y: 0, width: 4000, height: 4000 }]
        }
        fn set_window_position(&mut self, _x: i32, _y: i32) {}
        fn set_window_size(&mut self, _w: u32, _h: u32) {}
        fn is_running(&self) -> bool {
            true
        }
        fn log_warning_stderr(&self, _m: &str) {}
        fn get_cursor_pos(&self) -> (f32, f32) {
            self.cursor
        }
    }

    fn center_of(m: &Monkey) -> (f32, f32) {
        m.center()
    }

    fn dist(a: (f32, f32), b: (f32, f32)) -> f32 {
        ((a.0 - b.0).powi(2) + (a.1 - b.1).powi(2)).sqrt()
    }

    #[test]
    fn idle_transitions_to_following_when_cursor_far() {
        let mut m = Monkey::new((100.0, 100.0), W, H);
        let driver = MockDriver { cursor: (1000.0, 1000.0) };
        let events = vec![InputEvent::MouseMove { x: 1000.0, y: 1000.0 }];
        m.tick(&events, DT, &driver);
        assert_eq!(m.state, PetState::Following);
    }

    #[test]
    fn following_moves_toward_cursor_smoothly() {
        let mut m = Monkey::new((100.0, 100.0), W, H);
        let driver = MockDriver { cursor: (1500.0, 164.0) };
        let target = (1500.0, 164.0);

        let mut prev_dist = dist(center_of(&m), target);
        let mut moved_at_least_once = false;

        for _ in 0..120 {
            let events = vec![InputEvent::MouseMove { x: target.0, y: target.1 }];
            m.tick(&events, DT, &driver);
            let d = dist(center_of(&m), target);
            // Distance is monotonically non-increasing while approaching.
            assert!(d <= prev_dist + 0.5, "distance increased: {} -> {}", prev_dist, d);
            if d < prev_dist - 0.01 {
                moved_at_least_once = true;
            }
            prev_dist = d;
        }

        assert!(moved_at_least_once, "monkey never moved toward the cursor");
        // It should have closed most of the gap and be resting near the cursor.
        assert!(prev_dist <= FOLLOW_TRIGGER_DISTANCE_PX, "did not get close enough: {}", prev_dist);
    }

    #[test]
    fn following_settles_into_idle_near_cursor() {
        let mut m = Monkey::new((100.0, 100.0), W, H);
        let driver = MockDriver { cursor: (1500.0, 164.0) };
        for _ in 0..600 {
            let events = vec![InputEvent::MouseMove { x: 1500.0, y: 164.0 }];
            m.tick(&events, DT, &driver);
        }
        // It stops chasing — settling into Idle (or Grooming, since the cursor
        // now rests over it).
        assert!(
            m.state == PetState::Idle || m.state == PetState::Grooming,
            "expected a settled state, got {:?}",
            m.state
        );
        // Resting gap is within the stop distance band.
        let d = dist(center_of(&m), (1500.0, 164.0));
        assert!(d <= FOLLOW_TRIGGER_DISTANCE_PX, "rest distance too far: {}", d);
    }

    #[test]
    fn keydown_enters_typing_and_hops_with_alternating_feet() {
        let mut m = Monkey::new((100.0, 100.0), W, H);
        let driver = MockDriver { cursor: (164.0, 164.0) };

        // First tick carries the key press.
        m.tick(&[InputEvent::KeyDown { key_code: 65 }], DT, &driver);
        assert_eq!(m.state, PetState::Typing);

        let mut saw_left_lean = false;
        let mut saw_right_lean = false;
        let mut saw_hop_up = false;

        // Keep typing for ~25 ticks (well within the decay window) with no mouse
        // movement; the hop phase should cross multiple feet.
        for _ in 0..25 {
            m.tick(&[InputEvent::KeyDown { key_code: 65 }], DT, &driver);
            assert_eq!(m.state, PetState::Typing);
            let off = m.render_position();
            // Vertical offset never pushes the monkey below its base position.
            assert!(off.1 <= m.position.1 + 0.001);
            if off.1 < m.position.1 - 0.5 {
                saw_hop_up = true;
            }
            if off.0 < m.position.0 - 0.5 {
                saw_left_lean = true;
            }
            if off.0 > m.position.0 + 0.5 {
                saw_right_lean = true;
            }
        }

        assert!(saw_hop_up, "monkey never hopped up while typing");
        assert!(saw_left_lean && saw_right_lean, "feet did not alternate (L:{} R:{})", saw_left_lean, saw_right_lean);
    }

    #[test]
    fn dragging_follows_the_cursor_anchor() {
        let mut m = Monkey::new((100.0, 100.0), W, H);
        let driver = MockDriver { cursor: (150.0, 150.0) };

        // Press inside the monkey's box.
        m.tick(
            &[InputEvent::MouseDown { x: 150.0, y: 150.0, button: MouseButton::Left }],
            DT,
            &driver,
        );
        assert_eq!(m.state, PetState::Dragged);

        // Drag to a new spot; the anchor offset (50,50) should be preserved.
        m.tick(&[InputEvent::MouseMove { x: 400.0, y: 300.0 }], DT, &driver);
        assert!((m.position.0 - 350.0).abs() < 1.0, "x = {}", m.position.0);
        assert!((m.position.1 - 250.0).abs() < 1.0, "y = {}", m.position.1);
    }
}
