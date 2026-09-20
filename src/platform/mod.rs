//! The boundary between the behaviour engine and the operating system.
//!
//! Everything below this module is platform specific and selected with `cfg`
//! gates; everything above it is portable and unit testable. Exactly one driver
//! is compiled into a given binary, plus the always-available [`HeadlessDriver`]
//! used by tests and by `--self-test`.

pub mod headless;

#[cfg(target_os = "linux")]
pub mod wayland;
#[cfg(target_os = "windows")]
pub mod win32;

use crate::animation::Cell;
use crate::error::Result;
use crate::geometry::{ScreenLayout, Vec2};
use crate::sprite::SpriteSheet;

/// An input event, already normalised to screen coordinates.
#[derive(Debug, Clone, Copy, PartialEq)]
pub enum InputEvent {
    /// The cursor moved to a screen position.
    MouseMove { position: Vec2 },
    /// A mouse button went down at a screen position.
    MouseDown { position: Vec2, button: MouseButton },
    /// A mouse button was released at a screen position.
    MouseUp { position: Vec2, button: MouseButton },
    /// A key went down. `key_code` is platform specific and only used to tell
    /// presses apart, never interpreted as text (FR-06).
    KeyDown { key_code: u32 },
    /// A key was released.
    KeyUp { key_code: u32 },
    /// The compositor or window manager asked the overlay to close.
    CloseRequested,
    /// The overlay surface changed size.
    SurfaceResized { width: u32, height: u32 },
}

/// A mouse button.
#[derive(Debug, PartialEq, Eq, Clone, Copy, Hash)]
pub enum MouseButton {
    Left,
    Right,
    Middle,
    Other(u32),
}

/// Everything a driver needs in order to draw one frame.
#[derive(Debug)]
pub struct Frame<'a> {
    pub sheet: &'a SpriteSheet,
    /// Which cell of the sheet to draw.
    pub cell: Cell,
    /// Top-left corner of the sprite, in screen coordinates.
    pub position: Vec2,
    /// Squash and stretch, before [`SpriteSheet::render_scale`] is applied.
    pub scale: (f32, f32),
}

impl Frame<'_> {
    /// On-screen size of the sprite in pixels, including every scale factor.
    #[must_use]
    pub fn size(&self) -> Vec2 {
        Vec2::new(
            self.sheet.frame_width() as f32 * self.scale.0 * self.sheet.render_scale(),
            self.sheet.frame_height() as f32 * self.scale.1 * self.sheet.render_scale(),
        )
    }
}

/// How the driver tracks input that happens outside the overlay.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum InputScope {
    /// Global hooks are active: the companion sees the whole desktop (FR-02,
    /// FR-04, FR-06).
    Global,
    /// Global hooks were refused; only events delivered to the overlay surface
    /// are seen. Required fallback behaviour on locked-down Wayland sessions.
    WindowLocal,
}

impl InputScope {
    #[must_use]
    pub const fn is_global(self) -> bool {
        matches!(self, Self::Global)
    }
}

/// The operations the engine needs from a windowing system.
pub trait PlatformDriver {
    /// Create and configure the transparent, click-through overlay.
    fn create_window(&mut self, width: u32, height: u32, title: &str) -> Result<()>;

    /// Drain the events that have arrived since the last call.
    fn poll_events(&mut self) -> Vec<InputEvent>;

    /// Draw one frame.
    fn render_frame(&mut self, frame: &Frame<'_>) -> Result<()>;

    /// The cached monitor layout (FR-09).
    fn screen_layout(&self) -> ScreenLayout;

    /// The most recent known cursor position, in screen coordinates.
    fn cursor_position(&self) -> Vec2;

    /// Whether the overlay is still alive.
    fn is_running(&self) -> bool;

    /// Ask the driver to shut down at the next opportunity.
    fn request_shutdown(&mut self);

    /// Whether global input capture is available in this session.
    fn input_scope(&self) -> InputScope;
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn frame_size_includes_render_scale_and_squash() {
        let sheet = SpriteSheet::load(
            std::path::Path::new("assets/sprites/monkey_directional.png"),
            128,
            128,
            2.0,
        )
        .expect("bundled sheet");
        let frame = Frame {
            sheet: &sheet,
            cell: Cell::default(),
            position: Vec2::ZERO,
            scale: (0.5, 1.5),
        };
        assert_eq!(frame.size(), Vec2::new(128.0, 384.0));
    }
}
