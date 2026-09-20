//! Sprite sheet loading and validation (FR-01).
//!
//! The sheet is loaded once at startup and is immutable afterwards: there is no
//! API to swap a texture or edit a pixel at runtime, which is what the PRD asks
//! for and also keeps the memory profile flat.

use std::path::Path;

use image::RgbaImage;

use crate::animation::{Cell, SheetGrid};
use crate::error::{Error, Result};

/// A loaded, validated sprite sheet.
#[derive(Debug)]
pub struct SpriteSheet {
    texture: RgbaImage,
    frame_width: u32,
    frame_height: u32,
    render_scale: f32,
    grid: SheetGrid,
}

impl SpriteSheet {
    /// Load a sheet and check it against the configured frame size.
    ///
    /// This is a hard failure: a missing, corrupt or wrongly sized sheet stops
    /// startup with an explicit message naming the path and the mismatch,
    /// rather than leaving the companion running with nothing to draw.
    pub fn load(
        path: &Path,
        frame_width: u32,
        frame_height: u32,
        render_scale: f32,
    ) -> Result<Self> {
        if frame_width == 0 || frame_height == 0 {
            return Err(Error::Asset(
                "frame dimensions must be greater than 0".to_string(),
            ));
        }
        if !path.exists() {
            return Err(Error::Asset(format!(
                "sprite sheet '{}' does not exist (working directory: {})",
                path.display(),
                std::env::current_dir()
                    .map(|dir| dir.display().to_string())
                    .unwrap_or_else(|_| "unknown".to_string())
            )));
        }

        let texture = image::open(path)
            .map_err(|error| {
                Error::Asset(format!(
                    "cannot decode sprite sheet '{}': {error}",
                    path.display()
                ))
            })?
            .to_rgba8();

        let (width, height) = texture.dimensions();
        if width == 0 || height == 0 {
            return Err(Error::Asset(format!(
                "sprite sheet '{}' is empty",
                path.display()
            )));
        }
        if width % frame_width != 0 || height % frame_height != 0 {
            return Err(Error::Asset(format!(
                "sprite sheet '{}' is {width}x{height}, which is not a whole number of \
                 {frame_width}x{frame_height} frames",
                path.display()
            )));
        }

        let grid = SheetGrid::new(width / frame_width, height / frame_height);
        log::info!(
            "loaded sprite sheet '{}': {width}x{height}, {}x{} frames of {frame_width}x{frame_height} ({:.1} MiB in memory)",
            path.display(),
            grid.columns,
            grid.rows,
            texture.as_raw().len() as f32 / (1024.0 * 1024.0),
        );

        Ok(Self {
            texture,
            frame_width,
            frame_height,
            render_scale,
            grid,
        })
    }

    #[must_use]
    pub const fn frame_width(&self) -> u32 {
        self.frame_width
    }

    #[must_use]
    pub const fn frame_height(&self) -> u32 {
        self.frame_height
    }

    #[must_use]
    pub const fn render_scale(&self) -> f32 {
        self.render_scale
    }

    #[must_use]
    pub const fn grid(&self) -> SheetGrid {
        self.grid
    }

    #[must_use]
    pub fn texture(&self) -> &RgbaImage {
        &self.texture
    }

    /// Bytes held by the decoded texture. Reported at startup so the memory
    /// budget in the PRD can be checked without a profiler.
    #[must_use]
    pub fn texture_bytes(&self) -> usize {
        self.texture.as_raw().len()
    }

    /// Pixel rectangle of a cell, clamped so it can never read out of bounds.
    #[must_use]
    pub fn frame_rect(&self, cell: Cell) -> FrameRect {
        let column = cell.column.min(self.grid.columns.saturating_sub(1));
        let row = cell.row.min(self.grid.rows.saturating_sub(1));
        FrameRect {
            x: column * self.frame_width,
            y: row * self.frame_height,
            width: self.frame_width,
            height: self.frame_height,
        }
    }
}

/// A rectangle of pixels inside the sheet.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct FrameRect {
    pub x: u32,
    pub y: u32,
    pub width: u32,
    pub height: u32,
}

#[cfg(test)]
mod tests {
    use super::*;

    fn sheet_path() -> &'static Path {
        Path::new("assets/sprites/monkey_directional.png")
    }

    #[test]
    fn the_bundled_sheet_loads_as_an_eight_by_eight_grid() {
        let sheet = SpriteSheet::load(sheet_path(), 128, 128, 1.0).expect("bundled sheet");
        assert_eq!(sheet.grid(), SheetGrid::new(8, 8));
        assert_eq!(sheet.texture().dimensions(), (1024, 1024));
    }

    #[test]
    fn a_missing_sheet_is_a_hard_error_naming_the_path() {
        let error = SpriteSheet::load(Path::new("assets/nope.png"), 128, 128, 1.0).unwrap_err();
        assert!(error.to_string().contains("assets/nope.png"), "{error}");
    }

    #[test]
    fn a_frame_size_that_does_not_tile_the_sheet_is_rejected() {
        let error = SpriteSheet::load(sheet_path(), 100, 128, 1.0).unwrap_err();
        assert!(error.to_string().contains("not a whole number"), "{error}");
    }

    #[test]
    fn zero_sized_frames_are_rejected() {
        assert!(SpriteSheet::load(sheet_path(), 0, 128, 1.0).is_err());
    }

    #[test]
    fn frame_rects_stay_inside_the_texture() {
        let sheet = SpriteSheet::load(sheet_path(), 128, 128, 1.0).expect("bundled sheet");
        let (width, height) = sheet.texture().dimensions();
        for column in 0..20 {
            for row in 0..20 {
                let rect = sheet.frame_rect(Cell { column, row });
                assert!(rect.x + rect.width <= width);
                assert!(rect.y + rect.height <= height);
            }
        }
    }
}
