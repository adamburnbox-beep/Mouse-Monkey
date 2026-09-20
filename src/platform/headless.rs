//! A driver that talks to no windowing system at all.
//!
//! It backs `--self-test`, which runs the real engine loop on any machine
//! (including CI containers with no display), and it lets tests drive the
//! behaviour engine with a scripted cursor and monitor layout.

use crate::error::Result;
use crate::geometry::{Rect, ScreenLayout, Vec2};

use super::{Frame, InputEvent, InputScope, PlatformDriver};

/// A no-op [`PlatformDriver`] with a scriptable event queue.
#[derive(Debug)]
pub struct HeadlessDriver {
    layout: ScreenLayout,
    cursor: Vec2,
    queued: Vec<InputEvent>,
    running: bool,
    /// Number of frames handed to [`PlatformDriver::render_frame`].
    rendered_frames: u64,
    /// The last frame's position and cell, for assertions and the self-test.
    last_render: Option<(Vec2, crate::animation::Cell)>,
}

impl HeadlessDriver {
    /// A driver reporting a single monitor of the given size.
    #[must_use]
    pub fn new(width: u32, height: u32) -> Self {
        Self::with_layout(ScreenLayout::new(vec![Rect::new(0, 0, width, height)]))
    }

    /// A driver reporting an arbitrary monitor layout.
    #[must_use]
    pub fn with_layout(layout: ScreenLayout) -> Self {
        let cursor = layout
            .monitors()
            .first()
            .map_or(Vec2::ZERO, |monitor| monitor.center());
        Self {
            layout,
            cursor,
            queued: Vec::new(),
            running: true,
            rendered_frames: 0,
            last_render: None,
        }
    }

    /// Queue an event for the next [`PlatformDriver::poll_events`] call.
    pub fn push_event(&mut self, event: InputEvent) {
        self.queued.push(event);
    }

    /// Move the reported cursor and queue the matching move event.
    pub fn move_cursor(&mut self, position: Vec2) {
        self.cursor = position;
        self.queued.push(InputEvent::MouseMove { position });
    }

    /// Replace the monitor layout, as a hot-plug event would (FR-09).
    pub fn set_layout(&mut self, layout: ScreenLayout) {
        self.layout = layout;
    }

    #[must_use]
    pub const fn rendered_frames(&self) -> u64 {
        self.rendered_frames
    }

    #[must_use]
    pub const fn last_render(&self) -> Option<(Vec2, crate::animation::Cell)> {
        self.last_render
    }
}

impl PlatformDriver for HeadlessDriver {
    fn create_window(&mut self, _width: u32, _height: u32, _title: &str) -> Result<()> {
        Ok(())
    }

    fn poll_events(&mut self) -> Vec<InputEvent> {
        std::mem::take(&mut self.queued)
    }

    fn render_frame(&mut self, frame: &Frame<'_>) -> Result<()> {
        self.rendered_frames += 1;
        self.last_render = Some((frame.position, frame.cell));
        Ok(())
    }

    fn screen_layout(&self) -> ScreenLayout {
        self.layout.clone()
    }

    fn cursor_position(&self) -> Vec2 {
        self.cursor
    }

    fn is_running(&self) -> bool {
        self.running
    }

    fn request_shutdown(&mut self) {
        self.running = false;
    }

    fn input_scope(&self) -> InputScope {
        InputScope::Global
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn events_are_drained_exactly_once() {
        let mut driver = HeadlessDriver::new(1920, 1080);
        driver.move_cursor(Vec2::new(10.0, 20.0));
        assert_eq!(driver.poll_events().len(), 1);
        assert!(driver.poll_events().is_empty());
        assert_eq!(driver.cursor_position(), Vec2::new(10.0, 20.0));
    }

    #[test]
    fn the_cursor_starts_at_the_centre_of_the_first_monitor() {
        let driver = HeadlessDriver::new(1920, 1080);
        assert_eq!(driver.cursor_position(), Vec2::new(960.0, 540.0));
    }

    #[test]
    fn shutdown_is_observable() {
        let mut driver = HeadlessDriver::new(800, 600);
        assert!(driver.is_running());
        driver.request_shutdown();
        assert!(!driver.is_running());
    }
}
