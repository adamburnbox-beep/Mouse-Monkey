use serde::{Serialize, Deserialize};
use std::fs;
use std::io;

/// Configuration for the sprite sheet.
#[derive(Debug, Serialize, Deserialize, Clone)] // <--- Ensure these 4 are here
pub struct SpriteConfig {
    pub sheet_path: String,
    pub frame_width: u32,
    pub frame_height: u32,
    pub render_scale: f32,
}

/// A region of each screen the monkey may cross but never rest in, as
/// fractions (0.0-1.0) of the screen size.
#[derive(Debug, Serialize, Deserialize, Clone, Copy)]
pub struct KeepOut {
    pub left: f32,
    pub top: f32,
    pub right: f32,
    pub bottom: f32,
}

impl Default for KeepOut {
    fn default() -> Self {
        Self { left: 0.10, top: 0.14, right: 0.89, bottom: 0.875 }
    }
}

/// Overall application configuration.
#[derive(Debug, Serialize, Deserialize, Clone)] // <--- Add Clone here
pub struct AppConfig {
    pub window_width: u32,
    pub window_height: u32,
    pub window_title: String,
    pub tick_rate_hz: u32,
    pub sprite: SpriteConfig,
    /// Area the monkey must never rest inside. Omit for the default;
    /// set `[keep_out]` all to 0 to disable.
    #[serde(default)]
    pub keep_out: KeepOut,
}

impl AppConfig {
    pub fn load(path: &str) -> io::Result<Self> {
        let contents = fs::read_to_string(path)?;
        toml::from_str(&contents).map_err(|e| io::Error::new(io::ErrorKind::InvalidData, format!("Failed to parse config: {}", e)))
    }
}