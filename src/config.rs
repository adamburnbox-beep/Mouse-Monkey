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

/// Overall application configuration.
#[derive(Debug, Serialize, Deserialize, Clone)] // <--- Add Clone here
pub struct AppConfig {
    pub window_width: u32,
    pub window_height: u32,
    pub window_title: String,
    pub tick_rate_hz: u32,
    pub sprite: SpriteConfig,
}

impl AppConfig {
    pub fn load(path: &str) -> io::Result<Self> {
        let contents = fs::read_to_string(path)?;
        toml::from_str(&contents).map_err(|e| io::Error::new(io::ErrorKind::InvalidData, format!("Failed to parse config: {}", e)))
    }
}