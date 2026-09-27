use crate::config::AppConfig;
use crate::sprite_renderer::SpriteData;
use std::io;

/// Represents a generic input event.
#[derive(Debug, Clone)]
pub enum InputEvent {
    /// Mouse moved to a new screen position.
    MouseMove { x: f32, y: f32 },
    /// Mouse button pressed.
    MouseDown { x: f32, y: f32, button: MouseButton },
    /// Mouse button released.
    MouseUp { x: f32, y: f32, button: MouseButton },
    /// Keyboard key pressed. `key_code` is platform-specific; `kind` is the
    /// platform-neutral classification the monkey reacts to.
    KeyDown { key_code: u32, kind: KeyKind },
    /// Keyboard key released.
    KeyUp { key_code: u32 },   // Using u32 as a generic placeholder for key codes
    /// Mouse wheel scrolled. `delta` is in wheel notches, positive = down.
    Scroll { delta: f32 },
    /// Window close requested.
    CloseRequested,
    /// Window resized.
    WindowResized { width: u32, height: u32 },
}

/// Keys that get a distinct typing animation.
#[derive(Debug, PartialEq, Eq, Clone, Copy)]
pub enum KeyKind {
    /// Space bar or Enter: both paws thump the keyboard.
    Thump,
    Other,
}

/// Represents a mouse button.
#[derive(Debug, PartialEq, Eq, Clone, Copy)]
pub enum MouseButton {
    Left,
    Right,
    Middle,
    Other(u32), // For additional mouse buttons
}

/// Represents a rectangular area, typically for screen bounds or window dimensions.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Rect {
    pub x: i32,
    pub y: i32,
    pub width: u32,
    pub height: u32,
}

/// Trait defining the interface for platform-specific operations.
pub trait PlatformDriver {
    /// Creates a new platform driver instance.
    fn new(config: &AppConfig) -> io::Result<Self> where Self: Sized;

    /// Creates and configures the application window.
    fn create_window(&mut self, width: u32, height: u32, title: &str) -> io::Result<()>;

    /// Polls for and returns a list of pending input and window events.
    fn poll_events(&mut self) -> Vec<InputEvent>;

    /// Renders a frame using the provided sprite data.
    fn render_frame(&mut self, sprite_data: &SpriteData) -> io::Result<()>;

    /// Retrieves the bounding rectangles for all connected monitors.
    fn get_screen_bounds(&self) -> Vec<Rect>;

    /// Sets the position of the application window.
    fn set_window_position(&mut self, x: i32, y: i32);

    /// Sets the size of the application window.
    fn set_window_size(&mut self, width: u32, height: u32);

    /// Checks if the application is still running (e.g., window not closed).
    fn is_running(&self) -> bool;

    /// Logs a warning message to stderr.
    fn log_warning_stderr(&self, message: &str);

    /// Retrieves the current cursor position in screen coordinates.
    fn get_cursor_pos(&self) -> (f32, f32);
}