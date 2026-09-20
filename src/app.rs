//! The engine loop: the one place where config, assets, behaviour and a
//! platform driver meet.
//!
//! The loop is deliberately hand-rolled rather than borrowed from a game
//! engine, as the PRD requires: one fixed-rate tick that polls input, advances
//! the state machine and draws a single sprite.

use std::time::{Duration, Instant};

use crate::animation::AnimationSet;
use crate::config::AppConfig;
use crate::error::Result;
use crate::geometry::{ScreenLayout, Vec2};
use crate::monkey::{Monkey, World};
use crate::platform::{Frame, InputEvent, PlatformDriver};
use crate::shutdown::ShutdownSignal;
use crate::sprite::SpriteSheet;

/// A tick longer than this is treated as a pause (laptop lid, SIGSTOP, a
/// scheduler stall) and clamped, so the monkey does not teleport across the
/// desktop when the process wakes up again.
const MAX_TICK_SECONDS: f32 = 0.25;

/// The assembled application: everything except the driver, which is passed in
/// so the same engine can run against Wayland, Win32 or the headless driver.
#[derive(Debug)]
pub struct App {
    config: AppConfig,
    sheet: SpriteSheet,
    animations: AnimationSet,
    monkey: Monkey,
    screens: ScreenLayout,
    /// Frames drawn since startup, reported on exit.
    frames: u64,
}

impl App {
    /// Load assets and build the engine from a validated config.
    pub fn new(config: AppConfig) -> Result<Self> {
        let sheet_path = config.resolved_sheet_path();
        let sheet = SpriteSheet::load(
            &sheet_path,
            config.sprite.frame_width,
            config.sprite.frame_height,
            config.sprite.render_scale,
        )?;

        let animations = AnimationSet::new(&config.animation, sheet.grid());
        for warning in animations.warnings(&config.animation) {
            log::warn!("{warning}");
        }

        let sprite_size = Vec2::new(
            sheet.frame_width() as f32 * sheet.render_scale(),
            sheet.frame_height() as f32 * sheet.render_scale(),
        );
        let start = Vec2::new(
            (config.window_width as f32 - sprite_size.x) / 2.0,
            (config.window_height as f32 - sprite_size.y) / 2.0,
        );
        let monkey = Monkey::new(start, sprite_size, config.behaviour.clone());

        Ok(Self {
            config,
            sheet,
            animations,
            monkey,
            screens: ScreenLayout::default(),
            frames: 0,
        })
    }

    #[must_use]
    pub const fn config(&self) -> &AppConfig {
        &self.config
    }

    #[must_use]
    pub const fn monkey(&self) -> &Monkey {
        &self.monkey
    }

    #[must_use]
    pub const fn frames(&self) -> u64 {
        self.frames
    }

    /// Create the overlay and cache the initial monitor layout.
    pub fn start(&mut self, driver: &mut dyn PlatformDriver) -> Result<()> {
        driver.create_window(
            self.config.window_width,
            self.config.window_height,
            &self.config.window_title,
        )?;
        self.screens = driver.screen_layout();
        if self.screens.is_empty() {
            log::warn!("no monitor layout reported yet; boundary clamping starts once one arrives");
        } else {
            log::info!(
                "{} monitor(s) mapped: {:?}",
                self.screens.monitors().len(),
                self.screens.monitors()
            );
        }
        if !driver.input_scope().is_global() {
            // Required by the PRD: say so loudly, on stderr, not just in the log.
            eprintln!(
                "warning: global input capture is unavailable; the monkey only sees input \
                 inside its own window. See the Permissions section of the README."
            );
        }
        log::info!(
            "estimated steady-state sprite memory: {:.1} MiB",
            self.sheet.texture_bytes() as f32 / (1024.0 * 1024.0)
        );
        Ok(())
    }

    /// Run one tick: poll, simulate, draw. Returns `false` once the loop should
    /// stop.
    pub fn tick(&mut self, driver: &mut dyn PlatformDriver, dt: f32) -> Result<bool> {
        let events = driver.poll_events();

        let mut layout_changed = false;
        for event in &events {
            match event {
                InputEvent::CloseRequested => {
                    log::info!("close requested");
                    driver.request_shutdown();
                    return Ok(false);
                }
                InputEvent::SurfaceResized { width, height } => {
                    log::debug!("surface resized to {width}x{height}");
                    layout_changed = true;
                }
                _ => {}
            }
        }

        let latest_layout = driver.screen_layout();
        if latest_layout != self.screens {
            log::info!(
                "display topology changed: {} monitor(s)",
                latest_layout.monitors().len()
            );
            self.screens = latest_layout;
            layout_changed = true;
        }
        if layout_changed {
            self.monkey.handle_display_change(&self.screens);
        }

        let dt = dt.clamp(0.0, MAX_TICK_SECONDS);
        let world = World {
            cursor: driver.cursor_position(),
            screens: &self.screens,
        };
        self.monkey.tick(&events, dt, &world);

        let frame = Frame {
            sheet: &self.sheet,
            cell: self.animations.resolve(self.monkey.pose()),
            position: self.monkey.position(),
            scale: self.monkey.scale(),
        };
        driver.render_frame(&frame)?;
        self.frames += 1;

        Ok(driver.is_running())
    }

    /// Run until the driver stops or `shutdown` is signalled.
    pub fn run(
        &mut self,
        driver: &mut dyn PlatformDriver,
        shutdown: &ShutdownSignal,
    ) -> Result<()> {
        self.start(driver)?;

        let target = self.config.tick_duration();
        let mut previous = Instant::now();

        while driver.is_running() && !shutdown.is_triggered() {
            let now = Instant::now();
            let dt = now.duration_since(previous).as_secs_f32();
            previous = now;

            if !self.tick(driver, dt)? {
                break;
            }

            let spent = Instant::now().duration_since(now);
            if let Some(remaining) = target.checked_sub(spent) {
                std::thread::sleep(remaining);
            }
        }

        if shutdown.is_triggered() {
            log::info!("shutdown signal received");
            driver.request_shutdown();
        }
        log::info!("stopped after {} frame(s)", self.frames);
        Ok(())
    }

    /// Run a fixed number of ticks at a fixed delta. Used by `--self-test` and
    /// by integration tests; never sleeps.
    pub fn run_fixed(
        &mut self,
        driver: &mut dyn PlatformDriver,
        ticks: u64,
        dt: Duration,
    ) -> Result<()> {
        self.start(driver)?;
        let dt = dt.as_secs_f32();
        for _ in 0..ticks {
            if !self.tick(driver, dt)? {
                break;
            }
        }
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::platform::headless::HeadlessDriver;
    use crate::platform::MouseButton;
    use crate::state::PetState;

    fn app() -> App {
        let mut config = AppConfig::default();
        config.sprite.sheet_path = "assets/sprites/monkey_directional.png".into();
        App::new(config).expect("app should build from the shipped defaults")
    }

    #[test]
    fn a_short_run_draws_one_frame_per_tick() {
        let mut app = app();
        let mut driver = HeadlessDriver::new(1920, 1080);
        app.run_fixed(&mut driver, 120, Duration::from_micros(16_667))
            .expect("run");
        assert_eq!(driver.rendered_frames(), 120);
        assert_eq!(app.frames(), 120);
    }

    #[test]
    fn a_close_request_stops_the_loop_immediately() {
        let mut app = app();
        let mut driver = HeadlessDriver::new(1920, 1080);
        app.start(&mut driver).expect("start");
        driver.push_event(InputEvent::CloseRequested);
        assert!(!app.tick(&mut driver, 0.016).expect("tick"));
    }

    #[test]
    fn a_long_stall_does_not_teleport_the_monkey() {
        let mut app = app();
        let mut driver = HeadlessDriver::new(1920, 1080);
        app.start(&mut driver).expect("start");
        let before = app.monkey().position();
        // Ten seconds of "elapsed" time in a single tick, as if resumed from sleep.
        app.tick(&mut driver, 10.0).expect("tick");
        let after = app.monkey().position();
        assert!(before.distance_to(after) < 100.0, "{before:?} -> {after:?}");
    }

    #[test]
    fn the_drawn_cell_tracks_the_state() {
        let mut app = app();
        let mut driver = HeadlessDriver::new(1920, 1080);
        app.start(&mut driver).expect("start");
        app.tick(&mut driver, 0.016).expect("tick");
        let idle_cell = driver.last_render().expect("a frame").1;

        let center = app.monkey().center();
        driver.push_event(InputEvent::MouseDown {
            position: center,
            button: MouseButton::Left,
        });
        app.tick(&mut driver, 0.016).expect("tick");
        assert_eq!(app.monkey().state(), PetState::Dragged);
        let drag_cell = driver.last_render().expect("a frame").1;
        assert_ne!(idle_cell, drag_cell);
    }

    #[test]
    fn hot_plugging_a_monitor_is_picked_up_by_the_loop() {
        let mut app = app();
        let mut driver = HeadlessDriver::new(1920, 1080);
        app.start(&mut driver).expect("start");
        app.tick(&mut driver, 0.016).expect("tick");

        driver.set_layout(ScreenLayout::new(vec![crate::geometry::Rect::new(
            0, 0, 640, 480,
        )]));
        app.tick(&mut driver, 0.016).expect("tick");
        let position = app.monkey().position();
        assert!(
            position.x <= 640.0 - 128.0 && position.y <= 480.0 - 128.0,
            "{position:?}"
        );
    }
}
