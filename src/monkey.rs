//! The behaviour state machine (FR-02 to FR-09).
//!
//! This module is deliberately free of platform types: it is driven by a slice
//! of [`InputEvent`]s, a delta time and a [`World`] snapshot, and it keeps all
//! of its clocks in accumulated seconds rather than reading the wall clock. That
//! makes every behaviour in the PRD reproducible in a unit test.

use rand::rngs::SmallRng;
use rand::{Rng, SeedableRng};

use crate::animation::{AnimKind, Pose};
use crate::config::BehaviourConfig;
use crate::geometry::{ScreenLayout, Vec2};
use crate::platform::{InputEvent, MouseButton};
use crate::state::PetState;

/// Ratio between the horizontal squish and the vertical stretch during a drag.
/// Fixed by the PRD at one half, which keeps the sprite's area roughly constant.
const SQUISH_RATIO: f32 = 0.5;

/// Number of acceleration samples averaged before deciding that a drag has
/// turned into a flick (FR-08).
const ACCELERATION_WINDOW: usize = 4;

/// The outside world as the monkey sees it during one tick.
#[derive(Debug, Clone, Copy)]
pub struct World<'a> {
    /// Cursor position reported by the driver, used when no move event arrived.
    pub cursor: Vec2,
    /// Cached monitor rectangles (FR-09).
    pub screens: &'a ScreenLayout,
}

/// The companion's simulated state.
#[derive(Debug)]
pub struct Monkey {
    state: PetState,
    /// Top-left corner of the sprite in screen coordinates.
    position: Vec2,
    velocity: Vec2,
    /// Unscaled sprite size, in pixels.
    sprite_size: Vec2,
    /// Signed squash/stretch factor; 0.0 is at rest (FR-03).
    stretch: f32,
    /// Extra scale applied by the grooming breathing pulse (FR-05).
    breathing: f32,

    behaviour: BehaviourConfig,
    rng: SmallRng,

    // Cursor tracking.
    cursor: Vec2,
    previous_cursor: Vec2,
    cursor_velocity: Vec2,
    previous_cursor_velocity: Vec2,
    acceleration_samples: [f32; ACCELERATION_WINDOW],
    acceleration_cursor: usize,
    /// False until the first cursor sample has been seen. The first sample has
    /// no predecessor, so it must not be turned into a velocity.
    cursor_known: bool,

    // Clocks, all in seconds.
    state_time: f32,
    cursor_still_time: f32,
    groom_hold_time: f32,
    scratch_time: f32,
    idle_time_since_wiggle_roll: f32,

    // Drag bookkeeping.
    drag_anchor: Vec2,
    /// Position the monkey returns to when a micro-wiggle ends (FR-07).
    wiggle_origin: Vec2,
}

impl Monkey {
    /// Create a monkey at `position`, seeded from the system clock.
    #[must_use]
    pub fn new(position: Vec2, sprite_size: Vec2, behaviour: BehaviourConfig) -> Self {
        let seed = std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .map_or(0, |elapsed| elapsed.as_nanos() as u64);
        Self::with_seed(position, sprite_size, behaviour, seed)
    }

    /// Create a monkey with a fixed random seed, so idle twitches are
    /// reproducible in tests.
    #[must_use]
    pub fn with_seed(
        position: Vec2,
        sprite_size: Vec2,
        behaviour: BehaviourConfig,
        seed: u64,
    ) -> Self {
        Self {
            state: PetState::Idle,
            position,
            velocity: Vec2::ZERO,
            sprite_size,
            stretch: 0.0,
            breathing: 0.0,
            behaviour,
            rng: SmallRng::seed_from_u64(seed),
            cursor: Vec2::ZERO,
            previous_cursor: Vec2::ZERO,
            cursor_velocity: Vec2::ZERO,
            previous_cursor_velocity: Vec2::ZERO,
            acceleration_samples: [0.0; ACCELERATION_WINDOW],
            acceleration_cursor: 0,
            cursor_known: false,
            state_time: 0.0,
            cursor_still_time: 0.0,
            groom_hold_time: 0.0,
            scratch_time: 0.0,
            idle_time_since_wiggle_roll: 0.0,
            drag_anchor: Vec2::ZERO,
            wiggle_origin: position,
        }
    }

    #[must_use]
    pub const fn state(&self) -> PetState {
        self.state
    }

    #[must_use]
    pub const fn position(&self) -> Vec2 {
        self.position
    }

    #[must_use]
    pub const fn velocity(&self) -> Vec2 {
        self.velocity
    }

    /// Current squash and stretch, as `(x, y)` scale factors.
    #[must_use]
    pub fn scale(&self) -> (f32, f32) {
        let breath = 1.0 + self.breathing;
        (
            (1.0 - self.stretch * SQUISH_RATIO) * breath,
            (1.0 + self.stretch) * breath,
        )
    }

    /// On-screen size of the sprite, including squash and stretch.
    #[must_use]
    pub fn size(&self) -> Vec2 {
        let (scale_x, scale_y) = self.scale();
        Vec2::new(self.sprite_size.x * scale_x, self.sprite_size.y * scale_y)
    }

    /// Centre of the sprite.
    #[must_use]
    pub fn center(&self) -> Vec2 {
        let size = self.size();
        Vec2::new(
            self.position.x + size.x / 2.0,
            self.position.y + size.y / 2.0,
        )
    }

    /// Whether a screen point lies on the sprite.
    #[must_use]
    pub fn contains(&self, point: Vec2) -> bool {
        let size = self.size();
        point.x >= self.position.x
            && point.x <= self.position.x + size.x
            && point.y >= self.position.y
            && point.y <= self.position.y + size.y
    }

    /// What the renderer should draw this frame.
    #[must_use]
    pub fn pose(&self) -> Pose {
        let kind = match self.state {
            PetState::Idle | PetState::MicroWiggle => AnimKind::Gaze,
            PetState::Hunting => AnimKind::Walk,
            PetState::Grooming => AnimKind::Groom,
            PetState::Scratching => AnimKind::Scratch,
            PetState::Dragged => AnimKind::Drag,
            PetState::Dramatic => AnimKind::Tumble,
        };
        Pose {
            kind,
            elapsed: self.state_time,
            sector: self.gaze_sector(),
        }
    }

    /// Direction sector from the sprite's centre to the cursor (FR-02).
    #[must_use]
    pub fn gaze_sector(&self) -> u32 {
        (self.cursor - self.center()).direction_sector()
    }

    /// Advance the simulation by `dt` seconds.
    pub fn tick(&mut self, events: &[InputEvent], dt: f32, world: &World<'_>) {
        // A tick with no elapsed time (or a bogus one after a suspend) would
        // divide by zero in the velocity maths; skip it rather than poison the
        // state with infinities.
        if !dt.is_finite() || dt <= 0.0 {
            return;
        }

        self.track_cursor(events, dt, world);
        self.handle_events(events);

        self.state_time += dt;

        match self.state {
            PetState::Idle => self.tick_idle(dt),
            PetState::Dragged => self.tick_dragged(dt),
            PetState::Hunting => self.tick_hunting(dt),
            PetState::Grooming => self.tick_grooming(dt),
            PetState::Scratching => self.tick_scratching(dt),
            PetState::MicroWiggle => self.tick_micro_wiggle(dt),
            PetState::Dramatic => self.tick_dramatic(dt, world),
        }

        // The pointer owns the sprite's position while it is held; every other
        // state is clamped to the monitor layout (FR-09).
        if self.state != PetState::Dragged {
            self.position = world.screens.clamp_sprite(self.position, self.size());
        }
    }

    /// Snap back onto a monitor after a display topology change (FR-09).
    pub fn handle_display_change(&mut self, screens: &ScreenLayout) {
        self.position = screens.clamp_sprite(self.position, self.size());
        self.wiggle_origin = self.position;
    }

    // ── cursor bookkeeping ──────────────────────────────────────────────────

    fn track_cursor(&mut self, events: &[InputEvent], dt: f32, world: &World<'_>) {
        let latest = events
            .iter()
            .rev()
            .find_map(|event| match event {
                InputEvent::MouseMove { position }
                | InputEvent::MouseDown { position, .. }
                | InputEvent::MouseUp { position, .. } => Some(*position),
                _ => None,
            })
            .unwrap_or(world.cursor);

        if !self.cursor_known {
            // First sample: adopt it without inventing a velocity out of the
            // distance from the origin.
            self.cursor = latest;
            self.previous_cursor = latest;
            self.cursor_velocity = Vec2::ZERO;
            self.previous_cursor_velocity = Vec2::ZERO;
            self.cursor_known = true;
            self.cursor_still_time += dt;
            return;
        }

        self.previous_cursor = self.cursor;
        self.previous_cursor_velocity = self.cursor_velocity;

        let delta = latest - self.previous_cursor;
        // Sub-pixel jitter is not movement: the "variance is exactly zero"
        // condition in FR-04 would otherwise never be met on a real desktop.
        let moved = delta.length() >= 0.5;

        self.cursor = latest;
        self.cursor_velocity = if moved {
            delta * (1.0 / dt)
        } else {
            Vec2::ZERO
        };

        let acceleration = (self.cursor_velocity - self.previous_cursor_velocity) * (1.0 / dt);
        self.acceleration_samples[self.acceleration_cursor] = acceleration.length();
        self.acceleration_cursor = (self.acceleration_cursor + 1) % ACCELERATION_WINDOW;

        if moved {
            self.cursor_still_time = 0.0;
        } else {
            self.cursor_still_time += dt;
        }
    }

    /// Mean of the acceleration window (FR-08's moving average filter).
    fn mean_acceleration(&self) -> f32 {
        let sum: f32 = self.acceleration_samples.iter().sum();
        sum / ACCELERATION_WINDOW as f32
    }

    fn handle_events(&mut self, events: &[InputEvent]) {
        for event in events {
            match event {
                InputEvent::MouseDown { position, button } => {
                    if *button == MouseButton::Left && self.contains(*position) {
                        self.begin_drag(*position);
                    }
                }
                InputEvent::MouseUp { button, .. } => {
                    if *button == MouseButton::Left && self.state == PetState::Dragged {
                        self.end_drag();
                    }
                }
                InputEvent::KeyDown { .. } => {
                    // Any key at all, including while the monkey is held (FR-06).
                    if self.state != PetState::Dragged {
                        if self.state != PetState::Scratching {
                            self.transition_to(PetState::Scratching);
                        }
                        self.scratch_time = 0.0;
                    }
                }
                _ => {}
            }
        }
    }

    fn begin_drag(&mut self, pointer: Vec2) {
        self.transition_to(PetState::Dragged);
        // Whatever the pointer did on its way to the monkey is not part of the
        // drag, so the flick detector starts from a clean window.
        self.acceleration_samples = [0.0; ACCELERATION_WINDOW];
        self.drag_anchor = pointer - self.position;
        self.velocity = Vec2::ZERO;
        self.stretch = 0.0;
        self.breathing = 0.0;
        log::debug!("picked up at {pointer:?}");
    }

    fn end_drag(&mut self) {
        if self.mean_acceleration() > self.behaviour.fling_acceleration_px_per_sec2 {
            self.fling();
        } else {
            self.transition_to(PetState::Idle);
            self.stretch = 0.0;
        }
    }

    /// Drop the cursor lock and start tumbling (FR-08).
    fn fling(&mut self) {
        self.transition_to(PetState::Dramatic);
        self.velocity = self.cursor_velocity;
        self.stretch = 0.0;
        log::debug!("flung at {:?} px/s", self.velocity);
    }

    fn transition_to(&mut self, state: PetState) {
        if self.state == state {
            return;
        }
        log::debug!("{} -> {}", self.state, state);
        self.state = state;
        self.state_time = 0.0;
        match state {
            PetState::Idle => {
                self.velocity = Vec2::ZERO;
                self.breathing = 0.0;
                self.groom_hold_time = 0.0;
                self.idle_time_since_wiggle_roll = 0.0;
            }
            PetState::MicroWiggle => self.wiggle_origin = self.position,
            PetState::Hunting => {
                // Restart the stillness clock, or arriving at the cursor would
                // immediately satisfy the hunt condition again and the monkey
                // would twitch between walking and standing for ever.
                self.cursor_still_time = 0.0;
            }
            PetState::Grooming => self.groom_hold_time = 0.0,
            PetState::Scratching => self.scratch_time = 0.0,
            _ => {}
        }
    }

    // ── per-state updates ───────────────────────────────────────────────────

    fn tick_idle(&mut self, dt: f32) {
        self.velocity = Vec2::ZERO;
        self.relax_stretch(dt);
        self.breathing = 0.0;

        // FR-04: a cursor that has not moved for a long time gets hunted.
        if self.cursor_still_time >= self.behaviour.hunt_after_idle_secs {
            self.transition_to(PetState::Hunting);
            return;
        }

        // FR-05: a still cursor resting on the sprite starts grooming.
        if self.contains(self.cursor)
            && self.cursor_velocity.length() < self.behaviour.groom_velocity_px_per_sec
        {
            self.groom_hold_time += dt;
            if self.groom_hold_time >= self.behaviour.groom_hold_secs {
                self.transition_to(PetState::Grooming);
                return;
            }
        } else {
            self.groom_hold_time = 0.0;
        }

        // FR-07: roll for an idle twitch on a fixed interval.
        self.idle_time_since_wiggle_roll += dt;
        if self.idle_time_since_wiggle_roll >= self.behaviour.wiggle_interval_secs {
            self.idle_time_since_wiggle_roll = 0.0;
            if self.rng.gen_bool(self.behaviour.wiggle_chance) {
                self.transition_to(PetState::MicroWiggle);
            }
        }
    }

    fn tick_dragged(&mut self, dt: f32) {
        self.position = self.cursor - self.drag_anchor;

        // FR-03: frame-to-frame vertical movement drives the stretch, which
        // then relaxes back to rest so the sprite does not stay deformed.
        let delta_y = self.cursor.y - self.previous_cursor.y;
        self.stretch += delta_y * self.behaviour.drag_stretch_per_px;
        self.relax_stretch(dt);

        // FR-08: a flick mid-drag drops the cursor lock immediately.
        if self.mean_acceleration() > self.behaviour.fling_acceleration_px_per_sec2 {
            self.fling();
        }
    }

    fn tick_hunting(&mut self, dt: f32) {
        self.relax_stretch(dt);
        let to_cursor = self.cursor - self.center();
        let distance = to_cursor.length();
        let step = self.behaviour.hunt_speed_px_per_sec * dt;

        if distance <= self.behaviour.hunt_arrival_px.max(step) {
            let size = self.size();
            self.position = Vec2::new(self.cursor.x - size.x / 2.0, self.cursor.y - size.y / 2.0);
            self.transition_to(PetState::Idle);
            return;
        }

        if let Some(direction) = to_cursor.normalized() {
            self.position = self.position + direction * step;
        }
    }

    fn tick_grooming(&mut self, dt: f32) {
        self.relax_stretch(dt);

        // FR-05: low-amplitude sine pulse on the canvas scale.
        let phase = self.state_time * std::f32::consts::TAU * self.behaviour.breathing_hz;
        self.breathing = self.behaviour.breathing_amplitude * phase.sin();

        if !self.contains(self.cursor)
            || self.cursor_velocity.length() > self.behaviour.groom_velocity_px_per_sec
        {
            self.transition_to(PetState::Idle);
        }
    }

    fn tick_scratching(&mut self, dt: f32) {
        self.relax_stretch(dt);
        self.scratch_time += dt;
        let decay = self.behaviour.scratch_decay_ms as f32 / 1000.0;
        if self.scratch_time >= decay {
            self.transition_to(PetState::Idle);
        }
    }

    fn tick_micro_wiggle(&mut self, dt: f32) {
        self.relax_stretch(dt);
        let duration = self.behaviour.wiggle_duration_ms as f32 / 1000.0;
        if self.state_time >= duration {
            self.position = self.wiggle_origin;
            self.transition_to(PetState::Idle);
            return;
        }
        let phase = self.state_time * std::f32::consts::TAU * self.behaviour.wiggle_hz;
        self.position = Vec2::new(
            self.wiggle_origin.x + self.behaviour.wiggle_amplitude_px * phase.sin(),
            self.wiggle_origin.y,
        );
    }

    fn tick_dramatic(&mut self, dt: f32, world: &World<'_>) {
        self.relax_stretch(dt);
        self.velocity = Vec2::new(
            self.velocity.x,
            self.velocity.y + self.behaviour.gravity_px_per_sec2 * dt,
        );
        self.position = self.position + self.velocity * dt;

        // FR-08 + FR-09: fall until the lower bound meets the mapped floor.
        let Some(floor) = world.screens.floor_for_sprite(self.position, self.size()) else {
            // With no known monitors there is no floor to land on; stop the fall
            // rather than accelerate forever off-screen.
            if self.state_time > 5.0 {
                self.transition_to(PetState::Idle);
            }
            return;
        };

        if self.position.y >= floor {
            self.position = Vec2::new(self.position.x, floor);
            let bounce = self.behaviour.landing_bounce;
            if bounce > 0.0 && self.velocity.y.abs() > 50.0 {
                self.velocity = Vec2::new(self.velocity.x * bounce, -self.velocity.y * bounce);
            } else {
                self.transition_to(PetState::Idle);
            }
        }
    }

    /// Ease the squash/stretch back towards rest, frame-rate independently.
    fn relax_stretch(&mut self, dt: f32) {
        let limit = self.behaviour.drag_scale_limit;
        self.stretch = self.stretch.clamp(-limit, limit);
        let decay = (-self.behaviour.drag_relax_per_sec * dt).exp();
        self.stretch *= decay;
        if self.stretch.abs() < 1.0e-4 {
            self.stretch = 0.0;
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::geometry::Rect;

    const SPRITE: Vec2 = Vec2 { x: 128.0, y: 128.0 };
    const DT: f32 = 1.0 / 60.0;

    fn layout() -> ScreenLayout {
        ScreenLayout::new(vec![Rect::new(0, 0, 1920, 1080)])
    }

    fn monkey_at(position: Vec2) -> Monkey {
        Monkey::with_seed(position, SPRITE, BehaviourConfig::default(), 42)
    }

    /// Run `ticks` frames with the cursor parked at `cursor`.
    fn run(monkey: &mut Monkey, screens: &ScreenLayout, cursor: Vec2, ticks: u32) {
        let world = World { cursor, screens };
        for _ in 0..ticks {
            monkey.tick(&[], DT, &world);
        }
    }

    #[test]
    fn a_new_monkey_is_idle() {
        assert_eq!(monkey_at(Vec2::new(100.0, 100.0)).state(), PetState::Idle);
    }

    #[test]
    fn zero_and_nan_deltas_are_ignored() {
        let screens = layout();
        let mut monkey = monkey_at(Vec2::new(100.0, 100.0));
        let world = World {
            cursor: Vec2::new(500.0, 500.0),
            screens: &screens,
        };
        monkey.tick(&[], 0.0, &world);
        monkey.tick(&[], f32::NAN, &world);
        monkey.tick(&[], -1.0, &world);
        assert_eq!(monkey.position(), Vec2::new(100.0, 100.0));
        assert!(monkey.scale().0.is_finite() && monkey.scale().1.is_finite());
    }

    // ── FR-02 ───────────────────────────────────────────────────────────────

    #[test]
    fn the_gaze_sector_follows_the_cursor_around_the_sprite() {
        let screens = layout();
        let mut monkey = monkey_at(Vec2::new(900.0, 500.0));
        let center = monkey.center();
        let mut seen = std::collections::HashSet::new();
        for degrees in (0..360).step_by(15) {
            let radians = (degrees as f32).to_radians();
            let cursor = center + Vec2::new(radians.cos(), radians.sin()) * 300.0;
            run(&mut monkey, &screens, cursor, 1);
            seen.insert(monkey.pose().sector);
        }
        assert_eq!(
            seen.len(),
            8,
            "every direction sector should be reachable: {seen:?}"
        );
    }

    // ── FR-03 ───────────────────────────────────────────────────────────────

    #[test]
    fn clicking_the_sprite_picks_it_up_and_it_follows_the_pointer() {
        let screens = layout();
        let mut monkey = monkey_at(Vec2::new(400.0, 400.0));
        let grab = Vec2::new(420.0, 420.0);
        let world = World {
            cursor: grab,
            screens: &screens,
        };
        monkey.tick(
            &[InputEvent::MouseDown {
                position: grab,
                button: MouseButton::Left,
            }],
            DT,
            &world,
        );
        assert_eq!(monkey.state(), PetState::Dragged);

        let moved = Vec2::new(500.0, 430.0);
        monkey.tick(
            &[InputEvent::MouseMove { position: moved }],
            DT,
            &World {
                cursor: moved,
                screens: &screens,
            },
        );
        // The grab offset is preserved.
        assert_eq!(monkey.position(), Vec2::new(480.0, 410.0));
    }

    #[test]
    fn clicking_next_to_the_sprite_does_not_pick_it_up() {
        let screens = layout();
        let mut monkey = monkey_at(Vec2::new(400.0, 400.0));
        let miss = Vec2::new(100.0, 100.0);
        monkey.tick(
            &[InputEvent::MouseDown {
                position: miss,
                button: MouseButton::Left,
            }],
            DT,
            &World {
                cursor: miss,
                screens: &screens,
            },
        );
        assert_eq!(monkey.state(), PetState::Idle);
    }

    #[test]
    fn dragging_downwards_stretches_and_dragging_up_squishes() {
        let screens = layout();
        let mut monkey = monkey_at(Vec2::new(400.0, 400.0));
        let grab = Vec2::new(420.0, 420.0);
        monkey.tick(
            &[InputEvent::MouseDown {
                position: grab,
                button: MouseButton::Left,
            }],
            DT,
            &World {
                cursor: grab,
                screens: &screens,
            },
        );

        let down = Vec2::new(420.0, 440.0);
        monkey.tick(
            &[InputEvent::MouseMove { position: down }],
            DT,
            &World {
                cursor: down,
                screens: &screens,
            },
        );
        let (scale_x, scale_y) = monkey.scale();
        assert!(scale_y > 1.0, "expected vertical stretch, got {scale_y}");
        assert!(scale_x < 1.0, "expected horizontal squish, got {scale_x}");
        // Volume preservation: the squish is exactly half the stretch.
        assert!(((scale_y - 1.0) / 2.0 - (1.0 - scale_x)).abs() < 1.0e-5);
    }

    #[test]
    fn the_stretch_relaxes_back_to_rest_when_the_drag_stops() {
        let screens = layout();
        let mut monkey = monkey_at(Vec2::new(400.0, 400.0));
        let grab = Vec2::new(420.0, 420.0);
        monkey.tick(
            &[InputEvent::MouseDown {
                position: grab,
                button: MouseButton::Left,
            }],
            DT,
            &World {
                cursor: grab,
                screens: &screens,
            },
        );
        let down = Vec2::new(420.0, 460.0);
        monkey.tick(
            &[InputEvent::MouseMove { position: down }],
            DT,
            &World {
                cursor: down,
                screens: &screens,
            },
        );
        run(&mut monkey, &screens, down, 120);
        let (scale_x, scale_y) = monkey.scale();
        assert!((scale_x - 1.0).abs() < 1.0e-3, "{scale_x}");
        assert!((scale_y - 1.0).abs() < 1.0e-3, "{scale_y}");
    }

    #[test]
    fn releasing_a_slow_drag_returns_to_idle() {
        let screens = layout();
        let mut monkey = monkey_at(Vec2::new(400.0, 400.0));
        let grab = Vec2::new(420.0, 420.0);
        monkey.tick(
            &[InputEvent::MouseDown {
                position: grab,
                button: MouseButton::Left,
            }],
            DT,
            &World {
                cursor: grab,
                screens: &screens,
            },
        );
        run(&mut monkey, &screens, grab, 10);
        monkey.tick(
            &[InputEvent::MouseUp {
                position: grab,
                button: MouseButton::Left,
            }],
            DT,
            &World {
                cursor: grab,
                screens: &screens,
            },
        );
        assert_eq!(monkey.state(), PetState::Idle);
    }

    // ── FR-04 ───────────────────────────────────────────────────────────────

    #[test]
    fn a_motionless_cursor_eventually_gets_hunted_down() {
        let screens = layout();
        let mut monkey = monkey_at(Vec2::new(100.0, 100.0));
        let cursor = Vec2::new(1500.0, 900.0);
        let idle_ticks = (BehaviourConfig::default().hunt_after_idle_secs / DT) as u32 + 2;
        run(&mut monkey, &screens, cursor, idle_ticks);
        assert_eq!(monkey.state(), PetState::Hunting);

        // It walks there and stops on arrival. The cursor is then resting on
        // the sprite, so grooming taking over is expected.
        run(&mut monkey, &screens, cursor, 60 * 20);
        assert_ne!(
            monkey.state(),
            PetState::Hunting,
            "the hunt should have ended"
        );
        assert!(
            monkey.center().distance_to(cursor) <= 12.0,
            "{:?}",
            monkey.center()
        );
    }

    #[test]
    fn moving_the_cursor_resets_the_hunt_timer() {
        let screens = layout();
        let mut monkey = monkey_at(Vec2::new(100.0, 100.0));
        let hunt_ticks = (BehaviourConfig::default().hunt_after_idle_secs / DT) as u32;
        run(
            &mut monkey,
            &screens,
            Vec2::new(800.0, 800.0),
            hunt_ticks - 10,
        );
        // One real movement, then almost long enough again.
        let world = World {
            cursor: Vec2::new(801.0, 800.0),
            screens: &screens,
        };
        monkey.tick(
            &[InputEvent::MouseMove {
                position: Vec2::new(801.0, 800.0),
            }],
            DT,
            &world,
        );
        run(
            &mut monkey,
            &screens,
            Vec2::new(801.0, 800.0),
            hunt_ticks - 10,
        );
        assert_eq!(monkey.state(), PetState::Idle);
    }

    // ── FR-05 ───────────────────────────────────────────────────────────────

    #[test]
    fn a_still_cursor_resting_on_the_sprite_starts_grooming() {
        let screens = layout();
        let mut monkey = monkey_at(Vec2::new(400.0, 400.0));
        let on_sprite = Vec2::new(460.0, 460.0);
        run(&mut monkey, &screens, on_sprite, 1);
        let hold = (BehaviourConfig::default().groom_hold_secs / DT) as u32 + 2;
        run(&mut monkey, &screens, on_sprite, hold);
        assert_eq!(monkey.state(), PetState::Grooming);

        // The breathing pulse stays within the configured amplitude.
        for _ in 0..120 {
            run(&mut monkey, &screens, on_sprite, 1);
            let (scale_x, scale_y) = monkey.scale();
            assert!((scale_x - 1.0).abs() <= 0.021, "{scale_x}");
            assert!((scale_y - 1.0).abs() <= 0.021, "{scale_y}");
        }
    }

    #[test]
    fn moving_the_cursor_away_ends_grooming() {
        let screens = layout();
        let mut monkey = monkey_at(Vec2::new(400.0, 400.0));
        let on_sprite = Vec2::new(460.0, 460.0);
        run(&mut monkey, &screens, on_sprite, 1);
        run(&mut monkey, &screens, on_sprite, 140);
        assert_eq!(monkey.state(), PetState::Grooming);

        let away = Vec2::new(1000.0, 200.0);
        monkey.tick(
            &[InputEvent::MouseMove { position: away }],
            DT,
            &World {
                cursor: away,
                screens: &screens,
            },
        );
        assert_eq!(monkey.state(), PetState::Idle);
    }

    // ── FR-06 ───────────────────────────────────────────────────────────────

    #[test]
    fn any_key_starts_scratching_and_it_decays() {
        let screens = layout();
        let mut monkey = monkey_at(Vec2::new(400.0, 400.0));
        let cursor = Vec2::new(50.0, 50.0);
        let world = World {
            cursor,
            screens: &screens,
        };
        monkey.tick(&[InputEvent::KeyDown { key_code: 30 }], DT, &world);
        assert_eq!(monkey.state(), PetState::Scratching);

        // Still scratching just before the decay window closes.
        run(&mut monkey, &screens, cursor, 40);
        assert_eq!(monkey.state(), PetState::Scratching);

        run(&mut monkey, &screens, cursor, 20);
        assert_eq!(monkey.state(), PetState::Idle);
    }

    #[test]
    fn more_typing_keeps_the_scratch_alive() {
        let screens = layout();
        let mut monkey = monkey_at(Vec2::new(400.0, 400.0));
        let cursor = Vec2::new(50.0, 50.0);
        for _ in 0..10 {
            let world = World {
                cursor,
                screens: &screens,
            };
            monkey.tick(&[InputEvent::KeyDown { key_code: 30 }], DT, &world);
            run(&mut monkey, &screens, cursor, 30);
            assert_eq!(monkey.state(), PetState::Scratching);
        }
    }

    // ── FR-07 ───────────────────────────────────────────────────────────────

    #[test]
    fn idle_micro_wiggles_happen_and_always_return_to_the_original_spot() {
        let screens = layout();
        let start = Vec2::new(400.0, 400.0);
        let mut monkey = monkey_at(start);
        let cursor = Vec2::new(50.0, 50.0);

        let mut wiggled = false;
        let mut max_offset: f32 = 0.0;
        for _ in 0..60 * 120 {
            run(&mut monkey, &screens, cursor, 1);
            if monkey.state() == PetState::MicroWiggle {
                wiggled = true;
                max_offset = max_offset.max((monkey.position().x - start.x).abs());
                assert_eq!(monkey.position().y, start.y, "wiggle must stay horizontal");
            }
        }
        assert!(
            wiggled,
            "a micro-wiggle should occur within two minutes of idling"
        );
        assert!(
            max_offset <= 2.0 + 1.0e-3,
            "wiggle amplitude {max_offset} exceeds +/-2 px"
        );
        assert_eq!(
            monkey.position(),
            start,
            "the monkey should return to its spot"
        );
    }

    // ── FR-08 ───────────────────────────────────────────────────────────────

    #[test]
    fn a_hard_flick_mid_drag_drops_the_monkey_and_it_falls_to_the_floor() {
        let screens = layout();
        let mut monkey = monkey_at(Vec2::new(400.0, 200.0));
        let grab = Vec2::new(420.0, 220.0);
        monkey.tick(
            &[InputEvent::MouseDown {
                position: grab,
                button: MouseButton::Left,
            }],
            DT,
            &World {
                cursor: grab,
                screens: &screens,
            },
        );

        // A flick: a large jump in one frame after a slow drag.
        let mut cursor = grab;
        for _ in 0..3 {
            cursor = cursor + Vec2::new(2.0, 0.0);
            monkey.tick(
                &[InputEvent::MouseMove { position: cursor }],
                DT,
                &World {
                    cursor,
                    screens: &screens,
                },
            );
        }
        cursor = cursor + Vec2::new(400.0, 0.0);
        monkey.tick(
            &[InputEvent::MouseMove { position: cursor }],
            DT,
            &World {
                cursor,
                screens: &screens,
            },
        );
        assert_eq!(
            monkey.state(),
            PetState::Dramatic,
            "a flick should drop the cursor lock"
        );

        run(&mut monkey, &screens, cursor, 60 * 10);
        assert_ne!(
            monkey.state(),
            PetState::Dramatic,
            "the fall should have ended"
        );
        let floor = 1080.0 - 128.0;
        assert!(
            (monkey.position().y - floor).abs() < 1.0,
            "{:?}",
            monkey.position()
        );
    }

    #[test]
    fn a_gentle_drag_never_becomes_dramatic() {
        let screens = layout();
        let mut monkey = monkey_at(Vec2::new(400.0, 200.0));
        let mut cursor = Vec2::new(420.0, 220.0);
        monkey.tick(
            &[InputEvent::MouseDown {
                position: cursor,
                button: MouseButton::Left,
            }],
            DT,
            &World {
                cursor,
                screens: &screens,
            },
        );
        for _ in 0..120 {
            cursor = cursor + Vec2::new(1.5, 1.0);
            monkey.tick(
                &[InputEvent::MouseMove { position: cursor }],
                DT,
                &World {
                    cursor,
                    screens: &screens,
                },
            );
            assert_eq!(monkey.state(), PetState::Dragged);
        }
    }

    // ── FR-09 ───────────────────────────────────────────────────────────────

    #[test]
    fn the_monkey_is_always_clamped_to_a_monitor() {
        let screens = layout();
        let mut monkey = monkey_at(Vec2::new(1900.0, 1000.0));
        run(&mut monkey, &screens, Vec2::new(10.0, 10.0), 5);
        assert!(monkey.position().x <= 1920.0 - 128.0);
        assert!(monkey.position().y <= 1080.0 - 128.0);
    }

    #[test]
    fn unplugging_a_monitor_snaps_the_monkey_back_onto_a_remaining_one() {
        let dual = ScreenLayout::new(vec![
            Rect::new(0, 0, 1920, 1080),
            Rect::new(1920, 0, 1920, 1080),
        ]);
        let mut monkey = monkey_at(Vec2::new(2500.0, 500.0));
        run(&mut monkey, &dual, Vec2::new(2500.0, 500.0), 1);
        assert_eq!(monkey.position(), Vec2::new(2500.0, 500.0));

        let single = ScreenLayout::new(vec![Rect::new(0, 0, 1920, 1080)]);
        monkey.handle_display_change(&single);
        assert!(
            monkey.position().x <= 1920.0 - 128.0,
            "{:?}",
            monkey.position()
        );
    }

    #[test]
    fn an_empty_layout_does_not_teleport_the_monkey() {
        let empty = ScreenLayout::default();
        let mut monkey = monkey_at(Vec2::new(300.0, 300.0));
        run(&mut monkey, &empty, Vec2::new(10.0, 10.0), 10);
        assert_eq!(monkey.position(), Vec2::new(300.0, 300.0));
    }

    #[test]
    fn the_simulation_never_produces_a_non_finite_position() {
        let screens = layout();
        let mut monkey = monkey_at(Vec2::new(400.0, 400.0));
        for step in 0..5_000 {
            let cursor = Vec2::new(
                ((step as f32) * 0.37).sin() * 4000.0,
                ((step as f32) * 0.11).cos() * 4000.0,
            );
            let events = match step % 7 {
                0 => vec![InputEvent::MouseMove { position: cursor }],
                1 => vec![InputEvent::MouseDown {
                    position: cursor,
                    button: MouseButton::Left,
                }],
                2 => vec![InputEvent::MouseUp {
                    position: cursor,
                    button: MouseButton::Left,
                }],
                3 => vec![InputEvent::KeyDown {
                    key_code: step as u32,
                }],
                _ => vec![],
            };
            monkey.tick(
                &events,
                DT,
                &World {
                    cursor,
                    screens: &screens,
                },
            );
            assert!(monkey.position().x.is_finite(), "step {step}");
            assert!(monkey.position().y.is_finite(), "step {step}");
            assert!(monkey.scale().0.is_finite() && monkey.scale().1.is_finite());
        }
    }
}
