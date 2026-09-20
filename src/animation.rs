//! Mapping from behaviour states to cells of the sprite sheet.
//!
//! The behaviour engine never names a row or a column: it asks for a [`Pose`]
//! ("gaze towards sector 3", "tumble, 0.4 s in") and the [`AnimationSet`] built
//! from the config turns that into a concrete [`Cell`]. That indirection is what
//! lets a different sheet be dropped in through `monkey_companion.toml` without
//! touching the state machine.

use crate::config::AnimationConfig;

/// The frame grid of a sprite sheet, in cells.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct SheetGrid {
    pub columns: u32,
    pub rows: u32,
}

impl SheetGrid {
    #[must_use]
    pub const fn new(columns: u32, rows: u32) -> Self {
        Self { columns, rows }
    }
}

/// A single cell of the sheet.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub struct Cell {
    pub column: u32,
    pub row: u32,
}

/// Which animation the monkey wants to play.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum AnimKind {
    /// Looking towards the cursor; the sector selects the frame (FR-02).
    Gaze,
    /// Walking towards the cursor; the sector selects the facing (FR-04).
    Walk,
    /// Self-grooming loop (FR-05).
    Groom,
    /// Rapid head-scratching loop (FR-06).
    Scratch,
    /// Held by the pointer (FR-03).
    Drag,
    /// Tumbling through the air (FR-08).
    Tumble,
}

/// What the behaviour engine wants drawn this frame.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct Pose {
    pub kind: AnimKind,
    /// Seconds spent in the current animation, used to pick the frame.
    pub elapsed: f32,
    /// Direction sector in `0..8`; only meaningful for [`AnimKind::Gaze`] and
    /// [`AnimKind::Walk`].
    pub sector: u32,
}

/// A run of cells on one row, played at a fixed rate.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct Clip {
    row: u32,
    first_column: u32,
    frames: u32,
    fps: f32,
}

impl Clip {
    /// Build a clip and force it inside `grid`, so a mis-configured sheet can
    /// never index outside the texture.
    #[must_use]
    pub fn clamped(row: u32, first_column: u32, frames: u32, fps: f32, grid: SheetGrid) -> Self {
        let row = row.min(grid.rows.saturating_sub(1));
        let first_column = first_column.min(grid.columns.saturating_sub(1));
        let available = grid.columns - first_column;
        let frames = frames.clamp(1, available.max(1));
        let fps = if fps.is_finite() && fps > 0.0 {
            fps
        } else {
            1.0
        };
        Self {
            row,
            first_column,
            frames,
            fps,
        }
    }

    #[must_use]
    pub const fn frames(&self) -> u32 {
        self.frames
    }

    /// The cell to draw `elapsed` seconds into the clip. The clip loops.
    #[must_use]
    pub fn cell_at(&self, elapsed: f32) -> Cell {
        let elapsed = if elapsed.is_finite() && elapsed > 0.0 {
            elapsed
        } else {
            0.0
        };
        let step = (elapsed * self.fps) as u32 % self.frames;
        Cell {
            column: self.first_column + step,
            row: self.row,
        }
    }

    /// The cell `offset` frames into the clip, wrapping. Used where the frame is
    /// selected by direction rather than by time.
    #[must_use]
    pub fn cell_by_index(&self, offset: u32) -> Cell {
        Cell {
            column: self.first_column + offset % self.frames,
            row: self.row,
        }
    }
}

/// Every clip the companion can play, already validated against the sheet.
#[derive(Debug, Clone)]
pub struct AnimationSet {
    grid: SheetGrid,
    gaze: Clip,
    walk: Clip,
    groom: Clip,
    scratch: Clip,
    drag: Clip,
    tumble: Clip,
}

impl AnimationSet {
    /// Build the set from config, clamping every clip to `grid`.
    #[must_use]
    pub fn new(config: &AnimationConfig, grid: SheetGrid) -> Self {
        Self {
            grid,
            gaze: Clip::clamped(config.gaze_row, 0, grid.columns, 1.0, grid),
            walk: Clip::clamped(config.walk_row, 0, grid.columns, config.walk_fps, grid),
            groom: Clip::clamped(
                config.groom_row,
                config.groom_first_column,
                config.groom_frames,
                config.groom_fps,
                grid,
            ),
            scratch: Clip::clamped(
                config.scratch_row,
                config.scratch_first_column,
                config.scratch_frames,
                config.scratch_fps,
                grid,
            ),
            drag: Clip::clamped(config.drag_row, config.drag_column, 1, 1.0, grid),
            tumble: Clip::clamped(config.tumble_row, 0, grid.columns, config.tumble_fps, grid),
        }
    }

    #[must_use]
    pub const fn grid(&self) -> SheetGrid {
        self.grid
    }

    /// Resolve a pose to the sheet cell that should be drawn.
    #[must_use]
    pub fn resolve(&self, pose: Pose) -> Cell {
        match pose.kind {
            AnimKind::Gaze => self.gaze.cell_by_index(pose.sector),
            AnimKind::Walk => self.walk.cell_at(pose.elapsed),
            AnimKind::Groom => self.groom.cell_at(pose.elapsed),
            AnimKind::Scratch => self.scratch.cell_at(pose.elapsed),
            AnimKind::Drag => self.drag.cell_at(pose.elapsed),
            AnimKind::Tumble => self.tumble.cell_at(pose.elapsed),
        }
    }

    /// Human-readable warnings about clips the sheet cannot fully express.
    /// Reported once at startup rather than silently ignored.
    #[must_use]
    pub fn warnings(&self, config: &AnimationConfig) -> Vec<String> {
        let mut warnings = Vec::new();
        if self.grid.columns < 8 {
            warnings.push(format!(
                "sprite sheet has {} column(s); 8 are needed to show a distinct gaze per \
                 direction sector, so some directions will share a frame",
                self.grid.columns
            ));
        }
        for (name, row) in [
            ("gaze_row", config.gaze_row),
            ("walk_row", config.walk_row),
            ("groom_row", config.groom_row),
            ("scratch_row", config.scratch_row),
            ("drag_row", config.drag_row),
            ("tumble_row", config.tumble_row),
        ] {
            if row >= self.grid.rows {
                warnings.push(format!(
                    "[animation] {name} = {row} is outside the sheet ({} row(s)); clamped to {}",
                    self.grid.rows,
                    self.grid.rows.saturating_sub(1)
                ));
            }
        }
        warnings
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    const GRID: SheetGrid = SheetGrid {
        columns: 8,
        rows: 8,
    };

    #[test]
    fn clip_loops_over_its_frames() {
        let clip = Clip::clamped(6, 1, 3, 10.0, GRID);
        assert_eq!(clip.cell_at(0.0), Cell { column: 1, row: 6 });
        assert_eq!(clip.cell_at(0.1), Cell { column: 2, row: 6 });
        assert_eq!(clip.cell_at(0.2), Cell { column: 3, row: 6 });
        assert_eq!(clip.cell_at(0.3), Cell { column: 1, row: 6 });
    }

    #[test]
    fn clip_never_leaves_the_sheet() {
        let tiny = SheetGrid::new(2, 2);
        let clip = Clip::clamped(99, 99, 99, 0.0, tiny);
        for step in 0..50 {
            let cell = clip.cell_at(step as f32 * 0.1);
            assert!(cell.column < tiny.columns, "column {} escaped", cell.column);
            assert!(cell.row < tiny.rows, "row {} escaped", cell.row);
        }
    }

    #[test]
    fn non_finite_elapsed_is_treated_as_the_first_frame() {
        let clip = Clip::clamped(0, 0, 4, 8.0, GRID);
        assert_eq!(clip.cell_at(f32::NAN), Cell { column: 0, row: 0 });
        assert_eq!(clip.cell_at(-5.0), Cell { column: 0, row: 0 });
    }

    #[test]
    fn every_sector_maps_to_its_own_gaze_frame_on_an_eight_wide_sheet() {
        let set = AnimationSet::new(&AnimationConfig::default(), GRID);
        let cells: Vec<Cell> = (0..8)
            .map(|sector| {
                set.resolve(Pose {
                    kind: AnimKind::Gaze,
                    elapsed: 0.0,
                    sector,
                })
            })
            .collect();
        for (index, cell) in cells.iter().enumerate() {
            assert_eq!(cell.column, index as u32);
        }
    }

    #[test]
    fn a_narrow_sheet_is_reported_rather_than_ignored() {
        let config = AnimationConfig::default();
        let set = AnimationSet::new(&config, SheetGrid::new(4, 4));
        let warnings = set.warnings(&config);
        assert!(
            warnings.iter().any(|w| w.contains("column")),
            "{warnings:?}"
        );
        assert!(
            warnings.iter().any(|w| w.contains("groom_row")),
            "{warnings:?}"
        );
    }
}
