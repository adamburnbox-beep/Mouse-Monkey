mod config;
mod platform;
mod sprite_renderer;
mod state;
mod monkey;

#[cfg(target_os = "linux")]
mod wayland;
#[cfg(target_os = "windows")]
mod win32;

use crate::config::AppConfig;
use crate::platform::PlatformDriver;
use crate::sprite_renderer::{SpriteSheet, SpriteData};
use crate::monkey::Monkey;
use std::io;
use std::time::{Duration, Instant};
use log::info;

fn main() -> io::Result<()> {
    env_logger::init();
    info!("Starting monkey_companion application...");

    // 1. Load configuration
    // Assumes 'monkey_companion.toml' exists in the project root.
    let config = AppConfig::load("monkey_companion.toml")?;
    info!("Configuration loaded: {:?}", config);

    // 2. Initialize PlatformDriver based on target OS
    // This uses conditional compilation to select the correct driver.
    #[cfg(target_os = "linux")]
    let mut platform_driver = wayland::WaylandDriver::new(&config)?;
    #[cfg(target_os = "windows")]
    let mut platform_driver = win32::Win32Driver::new(&config)?;

    // 3. Create window
    platform_driver.create_window(
        config.window_width,
        config.window_height,
        &config.window_title,
    )?;

    // 4. Load sprite sheet
    let sprite_sheet = SpriteSheet::new(
        &config.sprite.sheet_path,
        config.sprite.frame_width,
        config.sprite.frame_height,
        config.sprite.render_scale,
    );

    // 5. Initialize Monkey
    let initial_monkey_pos = (
        (config.window_width / 2) as f32,
        (config.window_height / 2) as f32,
    );
    let mut monkey = Monkey::new(
        initial_monkey_pos,
        sprite_sheet.frame_width,
        sprite_sheet.frame_height,
    );

    let target_frame_duration = Duration::from_secs_f32(1.0 / config.tick_rate_hz as f32);
    let mut last_frame_time = Instant::now();

    // Main application loop
    while platform_driver.is_running() {
        let current_time = Instant::now();
        let elapsed_time = current_time.duration_since(last_frame_time);
        last_frame_time = current_time;
        let dt = elapsed_time.as_secs_f32(); // Delta time in seconds

        // Input processing
        let input_events = platform_driver.poll_events();
        for event in &input_events {
            if let crate::platform::InputEvent::CloseRequested = event {
                info!("Close requested. Shutting down.");
                return Ok(());
            }
        }

        // State updates (physics, animations, state transitions)
        monkey.tick(&input_events, dt, &platform_driver);

        // Rendering
        let sprite_data = SpriteData {
            sprite_sheet: &sprite_sheet,
            uv_rect: (
                monkey.current_animation_frame_index * sprite_sheet.frame_width,
                monkey.current_animation_row * sprite_sheet.frame_height,
                sprite_sheet.frame_width,
                sprite_sheet.frame_height,
            ),
            position: monkey.position,
            scale: monkey.scale,
        };
        platform_driver.render_frame(&sprite_data)?;

        // Frame rate control
        let frame_duration = Instant::now().duration_since(current_time);
        if frame_duration < target_frame_duration {
            std::thread::sleep(target_frame_duration - frame_duration);
        }
    }

    info!("Application shut down gracefully.");
    Ok(())
}