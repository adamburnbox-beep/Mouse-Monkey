//! Loading, defaulting and validating `monkey_companion.toml`.
//!
//! Every field has a default, so an older or partial config keeps working; every
//! field is also validated, so a nonsensical value is reported at startup rather
//! than turning into a `NaN` position three minutes later.

use std::fs;
use std::path::{Path, PathBuf};

use serde::{Deserialize, Serialize};

use crate::error::{Error, Result};

/// The file name looked for in each of the standard locations.
pub const CONFIG_FILE_NAME: &str = "monkey_companion.toml";

/// Sprite sheet settings (FR-01).
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
#[serde(deny_unknown_fields, default)]
pub struct SpriteConfig {
    /// Path to the sheet, absolute or relative to the config file.
    pub sheet_path: PathBuf,
    pub frame_width: u32,
    pub frame_height: u32,
    /// Uniform scale applied on top of any squash/stretch.
    pub render_scale: f32,
}

impl Default for SpriteConfig {
    fn default() -> Self {
        Self {
            sheet_path: PathBuf::from("assets/sprites/monkey_directional.png"),
            frame_width: 128,
            frame_height: 128,
            render_scale: 1.0,
        }
    }
}

/// Behaviour tuning. The defaults are the values named in `docs/PRD.md`,
/// converted from "per frame at 60 Hz" to per-second units so that the
/// behaviour does not change when the tick rate does.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
#[serde(deny_unknown_fields, default)]
pub struct BehaviourConfig {
    /// Cursor stillness before the hunt begins, in seconds (FR-04).
    pub hunt_after_idle_secs: f32,
    /// Walk speed while hunting, in pixels per second (FR-04: 3 px/frame).
    pub hunt_speed_px_per_sec: f32,
    /// Distance at which the hunt ends, in pixels (FR-04).
    pub hunt_arrival_px: f32,
    /// Cursor speed below which a hover counts as "still", in px/s (FR-05).
    pub groom_velocity_px_per_sec: f32,
    /// How long the cursor must rest on the sprite before grooming (FR-05:
    /// 120 frames at 60 Hz).
    pub groom_hold_secs: f32,
    /// Amplitude of the grooming breathing pulse (FR-05: 1.0 +/- 0.02).
    pub breathing_amplitude: f32,
    /// Frequency of the breathing pulse, in Hz.
    pub breathing_hz: f32,
    /// Time without a key press before scratching ends, in ms (FR-06).
    pub scratch_decay_ms: u64,
    /// Average gap between micro-wiggle rolls, in seconds (FR-07: 500 frames).
    pub wiggle_interval_secs: f32,
    /// Probability that a roll produces a wiggle (FR-07).
    pub wiggle_chance: f64,
    /// Duration of one wiggle, in ms (FR-07).
    pub wiggle_duration_ms: u64,
    /// Wiggle amplitude in pixels (FR-07: +/- 2 px).
    pub wiggle_amplitude_px: f32,
    /// Wiggle frequency in Hz (FR-07: 6 Hz).
    pub wiggle_hz: f32,
    /// Vertical stretch per pixel of drag movement (FR-03).
    pub drag_stretch_per_px: f32,
    /// How quickly the squash/stretch relaxes back to rest, per second (FR-03).
    pub drag_relax_per_sec: f32,
    /// Hard limit on squash/stretch, as a scale factor either side of 1.0.
    pub drag_scale_limit: f32,
    /// Mean cursor acceleration that counts as a flick, in px/s^2 (FR-08).
    ///
    /// A single frame at 60 Hz that jumps the cursor by 40 px is roughly
    /// 150,000 px/s^2 before the moving average divides it by the window size,
    /// which is about where a deliberate flick sits; an ordinary fast drag of
    /// 20 px per frame stays well under it.
    pub fling_acceleration_px_per_sec2: f32,
    /// Gravity during the dramatic fall, in px/s^2 (FR-08: 9.8 px/frame^2).
    pub gravity_px_per_sec2: f32,
    /// Fraction of speed kept when the monkey lands (0.0 = no bounce).
    pub landing_bounce: f32,
}

impl Default for BehaviourConfig {
    fn default() -> Self {
        Self {
            hunt_after_idle_secs: 300.0,
            hunt_speed_px_per_sec: 180.0,
            hunt_arrival_px: 10.0,
            groom_velocity_px_per_sec: 0.05,
            groom_hold_secs: 2.0,
            breathing_amplitude: 0.02,
            breathing_hz: 1.0,
            scratch_decay_ms: 750,
            wiggle_interval_secs: 8.333,
            wiggle_chance: 0.2,
            wiggle_duration_ms: 400,
            wiggle_amplitude_px: 2.0,
            wiggle_hz: 6.0,
            drag_stretch_per_px: 0.05,
            drag_relax_per_sec: 6.0,
            drag_scale_limit: 0.5,
            fling_acceleration_px_per_sec2: 35_000.0,
            gravity_px_per_sec2: 588.0,
            landing_bounce: 0.0,
        }
    }
}

/// Which cells of the sheet each animation uses.
///
/// The defaults describe the bundled 8x8 `monkey_directional.png`: rows 0-5 are
/// standing poses with varying gaze, row 6 holds the grooming and head-scratch
/// poses, and row 7 holds the surprised and tumbling poses.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
#[serde(deny_unknown_fields, default)]
pub struct AnimationConfig {
    /// Row whose columns are used as the eight gaze directions (FR-02).
    pub gaze_row: u32,
    /// Row cycled while walking to the cursor (FR-04).
    pub walk_row: u32,
    pub walk_fps: f32,
    pub groom_row: u32,
    pub groom_first_column: u32,
    pub groom_frames: u32,
    pub groom_fps: f32,
    pub scratch_row: u32,
    pub scratch_first_column: u32,
    pub scratch_frames: u32,
    pub scratch_fps: f32,
    /// Single frame shown while the monkey is held.
    pub drag_row: u32,
    pub drag_column: u32,
    /// Row cycled while tumbling (FR-08).
    pub tumble_row: u32,
    pub tumble_fps: f32,
}

impl Default for AnimationConfig {
    fn default() -> Self {
        Self {
            gaze_row: 1,
            walk_row: 3,
            walk_fps: 8.0,
            groom_row: 6,
            groom_first_column: 1,
            groom_frames: 3,
            groom_fps: 6.0,
            scratch_row: 6,
            scratch_first_column: 4,
            scratch_frames: 4,
            scratch_fps: 12.0,
            drag_row: 7,
            drag_column: 0,
            tumble_row: 7,
            tumble_fps: 10.0,
        }
    }
}

/// The whole application configuration.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
#[serde(deny_unknown_fields, default)]
pub struct AppConfig {
    /// Overlay width in logical pixels. Clamped to the desktop at runtime.
    pub window_width: u32,
    pub window_height: u32,
    pub window_title: String,
    /// Simulation and redraw rate.
    pub tick_rate_hz: u32,
    pub sprite: SpriteConfig,
    #[serde(alias = "behavior")]
    pub behaviour: BehaviourConfig,
    pub animation: AnimationConfig,
    /// Where this config came from; not read from the file itself.
    #[serde(skip)]
    pub source: Option<PathBuf>,
}

impl Default for AppConfig {
    fn default() -> Self {
        Self {
            window_width: 1920,
            window_height: 1080,
            window_title: "Mouse Monkey Companion".to_string(),
            tick_rate_hz: 60,
            sprite: SpriteConfig::default(),
            behaviour: BehaviourConfig::default(),
            animation: AnimationConfig::default(),
            source: None,
        }
    }
}

impl AppConfig {
    /// Parse a config from a TOML string, then validate it.
    pub fn from_toml(contents: &str, path: &Path) -> Result<Self> {
        let mut config: Self = toml::from_str(contents).map_err(|error| Error::ConfigParse {
            path: path.to_path_buf(),
            message: error.to_string(),
        })?;
        config.source = Some(path.to_path_buf());
        config.validate()?;
        Ok(config)
    }

    /// Read and validate a config file.
    pub fn load(path: &Path) -> Result<Self> {
        let contents = fs::read_to_string(path).map_err(|source| Error::ConfigRead {
            path: path.to_path_buf(),
            source,
        })?;
        Self::from_toml(&contents, path)
    }

    /// Load the first config found in the standard locations, or fall back to
    /// the built-in defaults when there is none.
    ///
    /// Search order: `$XDG_CONFIG_HOME/monkey-companion/` (or `%APPDATA%` on
    /// Windows), the working directory, then the directory holding the
    /// executable — so a portable unpacked release works with no setup.
    pub fn load_default() -> Result<Self> {
        for candidate in Self::search_paths() {
            if candidate.is_file() {
                return Self::load(&candidate);
            }
        }
        log::warn!("no {CONFIG_FILE_NAME} found in any standard location; using built-in defaults");
        let config = Self::default();
        config.validate()?;
        Ok(config)
    }

    /// The candidate config paths, in the order they are tried.
    #[must_use]
    pub fn search_paths() -> Vec<PathBuf> {
        let mut paths = Vec::new();

        if let Some(dir) = platform_config_dir() {
            paths.push(dir.join(CONFIG_FILE_NAME));
        }
        paths.push(PathBuf::from(CONFIG_FILE_NAME));
        if let Ok(exe) = std::env::current_exe() {
            if let Some(dir) = exe.parent() {
                paths.push(dir.join(CONFIG_FILE_NAME));
            }
        }
        paths
    }

    /// Resolve the sprite sheet path relative to the config file it came from,
    /// so running the binary from another directory still finds the art.
    #[must_use]
    pub fn resolved_sheet_path(&self) -> PathBuf {
        let sheet = &self.sprite.sheet_path;
        if sheet.is_absolute() {
            return sheet.clone();
        }
        if let Some(base) = self.source.as_ref().and_then(|p| p.parent()) {
            let candidate = base.join(sheet);
            if candidate.exists() {
                return candidate;
            }
        }
        if sheet.exists() {
            return sheet.clone();
        }
        if let Some(dir) = std::env::current_exe()
            .ok()
            .and_then(|exe| exe.parent().map(Path::to_path_buf))
        {
            let candidate = dir.join(sheet);
            if candidate.exists() {
                return candidate;
            }
        }
        sheet.clone()
    }

    /// Reject values that would make the engine misbehave.
    pub fn validate(&self) -> Result<()> {
        let invalid = |message: String| Err(Error::ConfigInvalid(message));

        if self.window_width == 0 || self.window_height == 0 {
            return invalid(format!(
                "window_width and window_height must be greater than 0 (got {}x{})",
                self.window_width, self.window_height
            ));
        }
        if !(1..=480).contains(&self.tick_rate_hz) {
            return invalid(format!(
                "tick_rate_hz must be between 1 and 480 (got {})",
                self.tick_rate_hz
            ));
        }
        if self.sprite.frame_width == 0 || self.sprite.frame_height == 0 {
            return invalid(format!(
                "sprite.frame_width and sprite.frame_height must be greater than 0 (got {}x{})",
                self.sprite.frame_width, self.sprite.frame_height
            ));
        }
        if !self.sprite.render_scale.is_finite() || self.sprite.render_scale <= 0.0 {
            return invalid(format!(
                "sprite.render_scale must be a positive number (got {})",
                self.sprite.render_scale
            ));
        }
        if self.sprite.sheet_path.as_os_str().is_empty() {
            return invalid("sprite.sheet_path must not be empty".to_string());
        }

        let b = &self.behaviour;
        for (name, value) in [
            ("hunt_after_idle_secs", b.hunt_after_idle_secs),
            ("hunt_speed_px_per_sec", b.hunt_speed_px_per_sec),
            ("hunt_arrival_px", b.hunt_arrival_px),
            ("groom_velocity_px_per_sec", b.groom_velocity_px_per_sec),
            ("groom_hold_secs", b.groom_hold_secs),
            ("breathing_amplitude", b.breathing_amplitude),
            ("breathing_hz", b.breathing_hz),
            ("wiggle_interval_secs", b.wiggle_interval_secs),
            ("wiggle_amplitude_px", b.wiggle_amplitude_px),
            ("wiggle_hz", b.wiggle_hz),
            ("drag_stretch_per_px", b.drag_stretch_per_px),
            ("drag_relax_per_sec", b.drag_relax_per_sec),
            ("drag_scale_limit", b.drag_scale_limit),
            (
                "fling_acceleration_px_per_sec2",
                b.fling_acceleration_px_per_sec2,
            ),
            ("gravity_px_per_sec2", b.gravity_px_per_sec2),
            ("landing_bounce", b.landing_bounce),
        ] {
            if !value.is_finite() || value < 0.0 {
                return invalid(format!(
                    "behaviour.{name} must be a finite non-negative number (got {value})"
                ));
            }
        }
        if !(0.0..=1.0).contains(&b.wiggle_chance) {
            return invalid(format!(
                "behaviour.wiggle_chance must be between 0.0 and 1.0 (got {})",
                b.wiggle_chance
            ));
        }
        if b.drag_scale_limit >= 1.0 {
            return invalid(format!(
                "behaviour.drag_scale_limit must be below 1.0, or the sprite could collapse \
                 to zero width (got {})",
                b.drag_scale_limit
            ));
        }
        if b.landing_bounce > 1.0 {
            return invalid(format!(
                "behaviour.landing_bounce must not exceed 1.0 (got {})",
                b.landing_bounce
            ));
        }
        for (name, value) in [
            ("walk_fps", self.animation.walk_fps),
            ("groom_fps", self.animation.groom_fps),
            ("scratch_fps", self.animation.scratch_fps),
            ("tumble_fps", self.animation.tumble_fps),
        ] {
            if !value.is_finite() || value <= 0.0 {
                return invalid(format!(
                    "animation.{name} must be a positive number (got {value})"
                ));
            }
        }
        Ok(())
    }

    /// Duration of one tick.
    #[must_use]
    pub fn tick_duration(&self) -> std::time::Duration {
        std::time::Duration::from_secs_f64(1.0 / f64::from(self.tick_rate_hz))
    }
}

/// The per-user config directory for this application, if one can be determined.
fn platform_config_dir() -> Option<PathBuf> {
    #[cfg(windows)]
    {
        std::env::var_os("APPDATA").map(|base| PathBuf::from(base).join("monkey-companion"))
    }
    #[cfg(not(windows))]
    {
        if let Some(base) = std::env::var_os("XDG_CONFIG_HOME").filter(|v| !v.is_empty()) {
            return Some(PathBuf::from(base).join("monkey-companion"));
        }
        std::env::var_os("HOME")
            .filter(|v| !v.is_empty())
            .map(|home| PathBuf::from(home).join(".config").join("monkey-companion"))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn parse(contents: &str) -> Result<AppConfig> {
        AppConfig::from_toml(contents, Path::new("test.toml"))
    }

    #[test]
    fn defaults_are_valid() {
        AppConfig::default()
            .validate()
            .expect("shipped defaults must validate");
    }

    #[test]
    fn an_empty_config_is_all_defaults() {
        let config = parse("").expect("empty config should load");
        assert_eq!(config.tick_rate_hz, AppConfig::default().tick_rate_hz);
        assert_eq!(config.behaviour, BehaviourConfig::default());
    }

    #[test]
    fn the_original_config_layout_still_loads() {
        // The config shipped before behaviour/animation sections existed.
        let config = parse(
            r#"
            window_width = 1920
            window_height = 1080
            window_title = "Mouse Monkey Companion"
            tick_rate_hz = 60

            [sprite]
            sheet_path = "assets/sprites/monkey_directional.png"
            frame_width = 128
            frame_height = 128
            render_scale = 1.0
            "#,
        )
        .expect("legacy config should load");
        assert_eq!(config.window_width, 1920);
        assert_eq!(config.sprite.frame_width, 128);
        assert_eq!(config.behaviour, BehaviourConfig::default());
    }

    #[test]
    fn american_spelling_of_the_behaviour_section_is_accepted() {
        let config = parse("[behavior]\nhunt_speed_px_per_sec = 42.0\n").expect("alias");
        assert_eq!(config.behaviour.hunt_speed_px_per_sec, 42.0);
    }

    #[test]
    fn unknown_keys_are_reported_instead_of_silently_ignored() {
        let error = parse("windo_width = 100\n").unwrap_err();
        assert!(matches!(error, Error::ConfigParse { .. }), "{error:?}");
    }

    #[test]
    fn impossible_values_are_rejected() {
        assert!(parse("tick_rate_hz = 0").is_err());
        assert!(parse("tick_rate_hz = 100000").is_err());
        assert!(parse("window_width = 0").is_err());
        assert!(parse("[sprite]\nframe_width = 0").is_err());
        assert!(parse("[sprite]\nrender_scale = 0.0").is_err());
        assert!(parse("[sprite]\nrender_scale = nan").is_err());
        assert!(parse("[behaviour]\nwiggle_chance = 1.5").is_err());
        assert!(parse("[behaviour]\ngravity_px_per_sec2 = -1.0").is_err());
        assert!(parse("[behaviour]\ndrag_scale_limit = 1.0").is_err());
        assert!(parse("[animation]\nwalk_fps = 0.0").is_err());
    }

    #[test]
    fn a_missing_file_names_the_path() {
        let error = AppConfig::load(Path::new("/definitely/not/here.toml")).unwrap_err();
        assert!(
            error.to_string().contains("/definitely/not/here.toml"),
            "{error}"
        );
    }

    #[test]
    fn search_paths_include_the_working_directory() {
        let paths = AppConfig::search_paths();
        assert!(
            paths.iter().any(|p| p == Path::new(CONFIG_FILE_NAME)),
            "{paths:?}"
        );
    }

    #[test]
    fn the_sheet_path_resolves_next_to_the_config_file() {
        let config = AppConfig {
            source: Some(PathBuf::from("some/dir/monkey_companion.toml")),
            sprite: SpriteConfig {
                sheet_path: PathBuf::from("nowhere/sheet.png"),
                ..SpriteConfig::default()
            },
            ..AppConfig::default()
        };
        // Nothing exists on disk, so the configured path is returned unchanged
        // and the asset loader reports the miss with the original path.
        assert_eq!(
            config.resolved_sheet_path(),
            PathBuf::from("nowhere/sheet.png")
        );
    }

    #[test]
    fn tick_duration_matches_the_rate() {
        let config = AppConfig {
            tick_rate_hz: 50,
            ..AppConfig::default()
        };
        assert_eq!(config.tick_duration(), std::time::Duration::from_millis(20));
    }
}
