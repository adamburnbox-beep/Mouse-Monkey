use crate::platform::{InputEvent, MouseButton, PlatformDriver, Rect};
use crate::state::PetState;
use log::{debug, info, warn};
use rand::{thread_rng, Rng};
use std::f32::consts::PI;
use std::time::{Duration, Instant};

// Constants for monkey behavior
const K_STRETCH: f32 = 0.005;
const K_SQUISH_FACTOR: f32 = 0.5;
/// Hunting walk speed in pixels per second (not per frame).
const HUNTING_SPEED_PX_PER_SEC: f32 = 180.0;
const HUNTING_DISTANCE_THRESHOLD_PX: f32 = 10.0;
const CURSOR_INACTIVITY_THRESHOLD_S: u64 = 300;
const GROOMING_VELOCITY_THRESHOLD: f32 = 0.05;
const GROOMING_FRAMES_THRESHOLD: u32 = 120;
const SCRATCH_DECAY_MS: u64 = 750;
const MICRO_WIGGLE_INTERVAL_FRAMES: u64 = 500;
const MICRO_WIGGLE_DURATION_MS: u64 = 400;
const MICRO_WIGGLE_AMPLITUDE_PX: f32 = 2.0;
const DRAMATIC_ACCELERATION_THRESHOLD: f32 = 100.0;
const GRAVITY_PX_PER_S_SQUARED: f32 = 9.8 * 60.0;

pub struct Monkey {
    pub state: PetState,
    pub position: (f32, f32), // Top-left corner of the monkey sprite
    pub velocity: (f32, f32),
    pub current_animation_frame_index: u32,
    pub current_animation_row: u32, // For different animation rows in sprite sheet
    pub scale: (f32, f32), // (scale_x, scale_y) for stretching/squishing
    pub rotation: f32, // For dramatic fall (radians)

    // Timers and counters
    last_input_time: Instant,
    cursor_inactivity_timer: Duration,
    grooming_frames_count: u32,
    scratch_decay_timer: Option<Instant>,
    micro_wiggle_active_timer: Option<Instant>,
    micro_wiggle_base_position: Option<(f32, f32)>, // For MicroWiggles
    idle_frame_counter: u64,

    // Mouse state for drag and acceleration
    is_dragging: bool,
    drag_anchor_offset: (f32, f32), // Offset from monkey's top-left to mouse click point
    previous_mouse_pos: (f32, f32),
    current_mouse_pos: (f32, f32),
    previous_mouse_velocity: (f32, f32),
    current_mouse_velocity: (f32, f32),
    mouse_down_pos: Option<(f32, f32)>, // Position where mouse was pressed down

    // Configuration values (derived from constants)
    k_stretch: f32,
    k_squish: f32,
    /// Hunting walk speed in px/s.
    hunting_speed: f32,
    grooming_velocity_threshold: f32,
    grooming_frames_threshold: u32,
    scratch_decay_ms: u64,
    micro_wiggle_interval_frames: u64,
    micro_wiggle_duration_ms: u64,
    micro_wiggle_amplitude: f32,
    dramatic_acceleration_threshold: f32,
    gravity: f32,
    /// Accumulates elapsed time within the current state for animation frame cycling.
    /// Reset to 0.0 whenever the state changes.
    state_animation_time: f32,

    rng: rand::rngs::ThreadRng,

    // Sprite dimensions for bounding box calculations
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
            rotation: 0.0,

            last_input_time: Instant::now(),
            cursor_inactivity_timer: Duration::ZERO,
            grooming_frames_count: 0,
            scratch_decay_timer: None,
            micro_wiggle_active_timer: None,
            micro_wiggle_base_position: None,
            idle_frame_counter: 0,

            is_dragging: false,
            drag_anchor_offset: (0.0, 0.0),
            previous_mouse_pos: (0.0, 0.0),
            current_mouse_pos: (0.0, 0.0),
            previous_mouse_velocity: (0.0, 0.0),
            current_mouse_velocity: (0.0, 0.0),
            mouse_down_pos: None,

            k_stretch: K_STRETCH,
            k_squish: K_STRETCH * K_SQUISH_FACTOR,
            hunting_speed: HUNTING_SPEED_PX_PER_SEC,
            grooming_velocity_threshold: GROOMING_VELOCITY_THRESHOLD,
            grooming_frames_threshold: GROOMING_FRAMES_THRESHOLD,
            scratch_decay_ms: SCRATCH_DECAY_MS,
            micro_wiggle_interval_frames: MICRO_WIGGLE_INTERVAL_FRAMES,
            micro_wiggle_duration_ms: MICRO_WIGGLE_DURATION_MS,
            micro_wiggle_amplitude: MICRO_WIGGLE_AMPLITUDE_PX,
            dramatic_acceleration_threshold: DRAMATIC_ACCELERATION_THRESHOLD,
            gravity: GRAVITY_PX_PER_S_SQUARED,
            state_animation_time: 0.0,

            rng: thread_rng(),

            sprite_width,
            sprite_height,
        }
    }

    // Helper to check if a point is within the monkey's bounding box
    fn contains_point(&self, x: f32, y: f32) -> bool {
        x >= self.position.0
            && x <= self.position.0 + self.sprite_width as f32 * self.scale.0
            && y >= self.position.1
            && y <= self.position.1 + self.sprite_height as f32 * self.scale.1
    }

    // Helper to get the monkey's center
    fn center(&self) -> (f32, f32) {
        (
            self.position.0 + (self.sprite_width as f32 * self.scale.0) / 2.0,
            self.position.1 + (self.sprite_height as f32 * self.scale.1) / 2.0,
        )
    }

    // Helper to calculate eye direction (02. Eye Follow)
    fn calculate_eye_direction(&self, mouse_x: f32, mouse_y: f32) -> u32 {
        let (cx, cy) = self.center();
        let angle = (mouse_y - cy).atan2(mouse_x - cx); // Angle in radians from -PI to PI

        // Map angle to 8 sectors (0-7)
        let sector_size = 2.0 * PI / 8.0; // 45 degrees in radians
        let normalized_angle = (angle + 2.0 * PI) % (2.0 * PI); // Normalize to 0 to 2PI
        let sector_index = ((normalized_angle + PI / 8.0) / sector_size).floor() as u32 % 8;
        sector_index
    }

    /// Transition to a new state and reset the per-state animation clock.
    fn transition_to(&mut self, new_state: crate::state::PetState) {
        self.state = new_state;
        self.state_animation_time = 0.0;
    }

    pub fn tick(&mut self, input_events: &[InputEvent], dt: f32, platform_driver: &dyn PlatformDriver) {
        let current_time = Instant::now();

        // Update mouse positions and velocities from input events
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
                    self.last_input_time = current_time;
                    mouse_moved_this_tick = true;
                }
                InputEvent::MouseDown { x, y, button } => {
                    if *button == MouseButton::Left && self.contains_point(*x, *y) {
                        self.transition_to(PetState::Dragged);
                        self.is_dragging = true;
                        self.drag_anchor_offset = (*x - self.position.0, *y - self.position.1);
                        self.mouse_down_pos = Some((*x, *y));
                        // Sync current_mouse_pos to the click location so the
                        // Dragged state branch (same tick) doesn't use a stale
                        // position, which would snap the sprite to (0,0).
                        self.current_mouse_pos = (*x, *y);
                        self.velocity = (0.0, 0.0);
                        self.scale = (1.0, 1.0);
                        self.rotation = 0.0;
                        info!("Monkey: Transitioned to Dragged state.");
                    }
                    self.last_input_time = current_time;
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
                            / dt.max(0.001);

                        if acceleration_magnitude > self.dramatic_acceleration_threshold {
                            self.transition_to(PetState::Dramatic);
                            self.velocity = self.current_mouse_velocity;
                            self.rotation = self.current_mouse_velocity
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
                    self.last_input_time = current_time;
                }
                InputEvent::KeyDown { .. } => {
                    // Typing while holding the monkey shouldn't drop it.
                    if self.state != PetState::Dragged {
                        self.transition_to(PetState::Scratching);
                        self.scratch_decay_timer = Some(current_time);
                        info!("Monkey: Transitioned to Scratching state.");
                    }
                    self.last_input_time = current_time;
                }
                _ => {}
            }
        }

        // When no MouseMove events arrive this tick (e.g. at startup, or evdev
        // is inactive and pointer is outside the input region), poll the
        // platform driver for the last known cursor position so the monkey
        // still tracks the cursor (eye follow, hunting target, etc.).
        if !mouse_moved_this_tick {
            let (px, py) = platform_driver.get_cursor_pos();
            // Only update if the position actually changed from what we know.
            if (px - self.current_mouse_pos.0).abs() > 0.5
                || (py - self.current_mouse_pos.1).abs() > 0.5
            {
                self.previous_mouse_pos = self.current_mouse_pos;
                self.current_mouse_pos = (px, py);
                self.current_mouse_velocity = (
                    (self.current_mouse_pos.0 - self.previous_mouse_pos.0) / dt.max(0.001),
                    (self.current_mouse_pos.1 - self.previous_mouse_pos.1) / dt.max(0.001),
                );
                self.last_input_time = current_time;
                mouse_moved_this_tick = true;
            }
        }

        // Advance per-state animation clock every tick.
        self.state_animation_time += dt;

        // Update cursor inactivity timer (04. Monkey Hunt)
        if mouse_moved_this_tick {
            self.cursor_inactivity_timer = Duration::ZERO;
        } else {
            self.cursor_inactivity_timer += Duration::from_secs_f32(dt);
        }

        // State-specific logic
        match self.state {
            PetState::Idle => {
                self.velocity = (0.0, 0.0);
                self.scale = (1.0, 1.0);
                self.rotation = 0.0;

                // Eye Follow: map the cursor direction to an animation row.
                // Rows 0–5 show the monkey with different gazes/expressions.
                // Rows 6–7 are reserved for grooming/scratching states.
                // Cycle through columns 0–3 slowly (2 Hz) to give the idle
                // monkey a subtle breathing/blinking animation.
                let sector_index = self.calculate_eye_direction(
                    self.current_mouse_pos.0,
                    self.current_mouse_pos.1,
                );
                // Map 8 sectors → 6 rows (0-5) to avoid grooming/scratching rows.
                self.current_animation_row = sector_index.min(5);
                self.current_animation_frame_index =
                    (self.state_animation_time * 2.0) as u32 % 4;

                // Monkey Hunt: trigger after prolonged cursor inactivity.
                if self.cursor_inactivity_timer.as_secs() >= CURSOR_INACTIVITY_THRESHOLD_S {
                    self.transition_to(PetState::Hunting);
                    info!("Monkey: Transitioned to Hunting (cursor inactivity).");
                }

                // Grooming Mode: cursor hovering still over the monkey.
                if self.contains_point(self.current_mouse_pos.0, self.current_mouse_pos.1) {
                    let mouse_v = (self.current_mouse_velocity.0.powi(2)
                        + self.current_mouse_velocity.1.powi(2))
                    .sqrt();
                    if mouse_v < self.grooming_velocity_threshold {
                        self.grooming_frames_count += 1;
                        if self.grooming_frames_count >= self.grooming_frames_threshold {
                            self.transition_to(PetState::Grooming);
                            info!("Monkey: Transitioned to Grooming state.");
                            self.grooming_frames_count = 0;
                        }
                    } else {
                        self.grooming_frames_count = 0;
                    }
                } else {
                    self.grooming_frames_count = 0;
                }

                // Micro-Wiggles: occasional idle jiggle.
                self.idle_frame_counter += 1;
                if self.idle_frame_counter >= self.micro_wiggle_interval_frames {
                    self.idle_frame_counter = 0;
                    if self.rng.gen_bool(0.2) {
                        self.transition_to(PetState::MicroWiggle);
                        self.micro_wiggle_active_timer = Some(current_time);
                        self.micro_wiggle_base_position = Some(self.position);
                        info!("Monkey: Transitioned to MicroWiggle state.");
                    }
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
                        // Ensure scale doesn't go negative or too extreme
                        self.scale.0 = self.scale.0.max(0.5).min(1.5);
                        self.scale.1 = self.scale.1.max(0.5).min(1.5);
                    }
                }
            }
            PetState::Hunting => {
                let (target_x, target_y) = platform_driver.get_cursor_pos();
                let (monkey_cx, monkey_cy) = self.center();

                let dx = target_x - monkey_cx;
                let dy = target_y - monkey_cy;
                let distance = (dx.powi(2) + dy.powi(2)).sqrt();

                // Speed is in px/s; multiply by dt (seconds) to get displacement.
                if distance < self.hunting_speed * dt || distance < HUNTING_DISTANCE_THRESHOLD_PX {
                    self.transition_to(PetState::Idle);
                    self.position.0 =
                        target_x - (self.sprite_width as f32 * self.scale.0) / 2.0;
                    self.position.1 =
                        target_y - (self.sprite_height as f32 * self.scale.1) / 2.0;
                    info!("Monkey: Reached cursor — back to Idle.");
                } else {
                    let nx = dx / distance;
                    let ny = dy / distance;
                    self.position.0 += nx * self.hunting_speed * dt;
                    self.position.1 += ny * self.hunting_speed * dt;

                    // Use the direction-of-movement sector to pick the eye-direction row,
                    // so the monkey looks where it is walking.
                    let angle = dy.atan2(dx);
                    let norm_angle = (angle + 2.0 * PI) % (2.0 * PI);
                    let sector_index =
                        ((norm_angle + PI / 8.0) / (2.0 * PI / 8.0)).floor() as u32 % 8;
                    // Row = direction sector (0-7), cycle cols 0-7 at 8 Hz for walking.
                    self.current_animation_row = sector_index;
                    self.current_animation_frame_index =
                        (self.state_animation_time * 8.0) as u32 % 8;
                }
            }
            PetState::Grooming => {
                // Gentle breathing scale pulse at 1 Hz.
                let breathing_scale =
                    1.0 + 0.02 * (self.state_animation_time * PI * 2.0).sin();
                self.scale = (breathing_scale, breathing_scale);

                let mouse_v = (self.current_mouse_velocity.0.powi(2)
                    + self.current_mouse_velocity.1.powi(2))
                .sqrt();
                if !self.contains_point(self.current_mouse_pos.0, self.current_mouse_pos.1)
                    || mouse_v > self.grooming_velocity_threshold
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
            PetState::Scratching => {
                // Row 7 = scratching/surprised poses. Cycle cols 0-7 at 12 Hz (rapid).
                self.current_animation_row = 7;
                self.current_animation_frame_index =
                    (self.state_animation_time * 12.0) as u32 % 8;

                if let Some(timer_start) = self.scratch_decay_timer {
                    if (current_time - timer_start).as_millis() as u64 >= self.scratch_decay_ms {
                        self.transition_to(PetState::Idle);
                        self.scratch_decay_timer = None;
                        info!("Monkey: Transitioned to Idle from Scratching.");
                    }
                }
            }
            PetState::MicroWiggle => {
                if let Some(timer_start) = self.micro_wiggle_active_timer {
                    let elapsed_ms = (current_time - timer_start).as_millis() as u64;
                    if elapsed_ms >= self.micro_wiggle_duration_ms {
                        if let Some(base_pos) = self.micro_wiggle_base_position.take() {
                            self.position = base_pos;
                        }
                        self.micro_wiggle_active_timer = None;
                        self.transition_to(PetState::Idle);
                        info!("Monkey: Transitioned to Idle from MicroWiggle.");
                    } else {
                        let base_pos =
                            self.micro_wiggle_base_position.unwrap_or(self.position);
                        // ±2 px horizontal wiggle at 6 Hz using per-state clock.
                        let wiggle_offset = self.micro_wiggle_amplitude
                            * (self.state_animation_time * 2.0 * PI * 6.0).sin();
                        self.position.0 = base_pos.0 + wiggle_offset;
                        self.position.1 = base_pos.1;
                        // Use row 5 (up-left gaze) for the wide-eyed wiggle expression.
                        self.current_animation_row = 5;
                        self.current_animation_frame_index = 0;
                    }
                } else {
                    self.transition_to(PetState::Idle);
                    if let Some(base_pos) = self.micro_wiggle_base_position.take() {
                        self.position = base_pos;
                    }
                }
            }
            PetState::Dramatic => {
                // Apply gravity and integrate position.
                self.velocity.1 += self.gravity * dt;
                self.position.0 += self.velocity.0 * dt;
                self.position.1 += self.velocity.1 * dt;
                self.rotation += self.velocity.0 * 0.001 * dt;

                // Tumbling frames: Row 7 (surprise/fall poses), cycling cols 0-7 at 5 Hz.
                self.current_animation_row = 7;
                self.current_animation_frame_index =
                    (self.state_animation_time * 5.0) as u32 % 8;

                // Detect floor collision.
                let screen_bounds = platform_driver.get_screen_bounds();
                let mut collided = false;
                if !screen_bounds.is_empty() {
                    let mut floor_y = f32::MAX;
                    for rect in &screen_bounds {
                        if self.position.0 < rect.x as f32 + rect.width as f32
                            && self.position.0
                                + self.sprite_width as f32 * self.scale.0
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

        // Clamp to screen bounds for all states except Dragged (where the cursor
        // drives the position directly; clamping is still applied after release).
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

            // Check if monkey is currently within this screen
            if monkey_right > screen_left && monkey_left < screen_right &&
               monkey_bottom > screen_top && monkey_top < screen_bottom {
                is_within_any_screen = true;
                // Clamp to this screen
                clamped_x = monkey_left.max(screen_left).min(screen_right - self.sprite_width as f32 * self.scale.0);
                clamped_y = monkey_top.max(screen_top).min(screen_bottom - self.sprite_height as f32 * self.scale.1);
                break; // Found a screen it's in, clamp to it and exit
            }

            // If not within any screen, find the closest one to snap to
            // Calculate distance to this rect (center to center)
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
                // Snap to the closest screen's bottom-center
                clamped_x = rect.x as f32 + (rect.width as f32 / 2.0) - (self.sprite_width as f32 * self.scale.0 / 2.0);
                clamped_y = rect.y as f32 + rect.height as f32 - (self.sprite_height as f32 * self.scale.1);
                debug!("Monkey snapped to closest screen: {:?}", rect);
            } else {
                // Fallback if no screens or closest rect found (shouldn't happen with non-empty screen_bounds)
                warn!("Could not find a screen to snap the monkey to. Defaulting to (0,0).");
                clamped_x = 0.0;
                clamped_y = 0.0;
            }
        }

        self.position = (clamped_x, clamped_y);
    }
}