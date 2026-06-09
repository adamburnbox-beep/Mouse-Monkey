use std::path::Path;
use log::{error, info};

/// Represents a loaded sprite sheet, holding its texture data and frame information.
pub struct SpriteSheet {
    pub texture: image::RgbaImage,
    pub frame_width: u32,
    pub frame_height: u32,
    pub render_scale: f32,
    pub num_frames_x: u32,
    pub num_frames_y: u32,
}

impl SpriteSheet {
    /// Creates a new `SpriteSheet` instance, loading the image and validating its dimensions.
    ///
    /// This function will hard-fail (panic!) if the sprite sheet is not found,
    /// cannot be loaded, or its dimensions are not multiples of the specified frame dimensions.
    pub fn new(
        sheet_path: &str,
        frame_width: u32,
        frame_height: u32,
        render_scale: f32,
    ) -> Self {
        let path = Path::new(sheet_path);
        if !path.exists() {
            error!("Sprite sheet not found: {}", sheet_path);
            panic!("Hard-fail: Sprite sheet not found at '{}'", sheet_path);
        }

        let texture = match image::open(path) {
            Ok(img) => img.to_rgba8(),
            Err(e) => {
                error!("Failed to open or decode sprite sheet '{}': {}", sheet_path, e);
                panic!("Hard-fail: Failed to load sprite sheet '{}': {}", sheet_path, e);
            }
        };
        let (total_width, total_height) = texture.dimensions();

        if total_width == 0 || total_height == 0 {
            error!("Sprite sheet '{}' has zero dimensions.", sheet_path);
            panic!("Hard-fail: Sprite sheet '{}' has zero dimensions.", sheet_path);
        }

        if total_width % frame_width != 0 || total_height % frame_height != 0 {
            error!(
                "Sprite sheet dimensions ({}, {}) are not multiples of frame dimensions ({}, {}) for '{}'.",
                total_width, total_height, frame_width, frame_height, sheet_path
            );
            panic!(
                "Hard-fail: Sprite sheet dimensions ({}, {}) are not multiples of frame dimensions ({}, {}) for '{}'.",
                total_width, total_height, frame_width, frame_height, sheet_path
            );
        }

        let num_frames_x = total_width / frame_width;
        let num_frames_y = total_height / frame_height;

        info!(
            "Loaded sprite sheet '{}' with dimensions {}x{} ({}x{} frames, {}x{} pixels per frame).",
            sheet_path, total_width, total_height, num_frames_x, num_frames_y, frame_width, frame_height
        );

        Self {
            texture,
            frame_width,
            frame_height,
            render_scale,
            num_frames_x,
            num_frames_y,
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_ascii_visualize() {
        let path = Path::new("assets/sprites/monkey_brown.png");
        let img = image::open(path).unwrap().to_rgba8();
        println!("ASCII representation of 256x256 area (top-left 2x2 grid of 128x128 tiles):");
        
        // Print 256x256 downsampled to 64x128
        for y_idx in 0..64 {
            for x_idx in 0..128 {
                let x = x_idx * 2;
                let y = y_idx * 4;
                let p = img.get_pixel(x, y);
                if p[3] > 128 {
                    // Put a vertical line indicator to show the 128px border
                    if x_idx == 64 {
                        print!("|");
                    } else {
                        print!("#");
                    }
                } else {
                    if x_idx == 64 {
                        print!("|");
                    } else {
                        print!(".");
                    }
                }
            }
            println!();
        }
    }
}

/// Data passed to the rendering backend containing the position, texture source rect, and scaling of the sprite.
pub struct SpriteData<'a> {
    pub sprite_sheet: &'a SpriteSheet,
    pub uv_rect: (u32, u32, u32, u32), // (x_offset, y_offset, width, height) within the sheet
    pub position: (f32, f32),          // (x, y) on the screen window canvas
    pub scale: (f32, f32),             // (x_scale, y_scale)
}