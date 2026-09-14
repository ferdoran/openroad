//! The resinfo grammar's sprite-sheet animation, as one description.
//!
//! Idea: six controls in the whole 247-file corpus carry the flipbook keys
//! (`FrameCount`, `WidthCount`, `HeightCount`, `Speed`, `EnableLoop`,
//! `ImageWidth`, `ImageHeight`) and they all animate the same way — walk a
//! row-major grid of equal tiles at a fixed millisecond period and loop. The
//! keys fully describe the animation, so the only thing a consumer has to do is
//! transcribe them and ask for the source rect at a time.
//!
//! It lives here rather than in whichever window needed it first because the
//! two low-vitals overlays are authored **twice** at different sheet sizes: the
//! mini-info's `pmi_*_cha_effect_caution.ddj` pair is 512x64 (128x32 tiles) and
//! the quick-party board's `pmi_*_quick_effect_caution.ddj` pair is 256x64
//! (64x32 tiles). Same 8 frames, same 4x2 grid, same 100 ms — different tiles.
//! One copy of the arithmetic with the sheet as data is the difference between
//! that being a fact and being a bug waiting for whoever edits one of the two.

use bevy::prelude::*;

/// One `Style=512` sprite sheet, transcribed from its control's own keys.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Flipbook {
    /// `FrameCount`.
    pub frame_count: usize,
    /// `WidthCount` — tiles per row.
    pub cols: usize,
    /// `HeightCount` — rows of tiles.
    pub rows: usize,
    /// `Speed`, in milliseconds per frame.
    pub frame_ms: f32,
    /// `ImageWidth` / `ImageHeight` of the sheet.
    pub sheet: (f32, f32),
}

impl Flipbook {
    /// One tile's size. The corpus's sheets all divide exactly, and a control's
    /// declared rect equals this — which is what makes a transcription
    /// checkable without a screenshot.
    pub fn tile(&self) -> (f32, f32) {
        (
            self.sheet.0 / self.cols as f32,
            self.sheet.1 / self.rows as f32,
        )
    }

    /// Frame index at `elapsed` seconds, looping (`EnableLoop=1`).
    ///
    /// Negative time cannot happen from `Time::elapsed_secs`, but clamping it
    /// costs nothing and keeps the cast from wrapping if it ever does.
    pub fn frame_at(&self, elapsed_secs: f32) -> usize {
        if self.frame_count == 0 || self.frame_ms <= 0.0 {
            return 0;
        }
        let ticks = (elapsed_secs * 1000.0 / self.frame_ms).max(0.0) as usize;
        ticks % self.frame_count
    }

    /// Source rect of frame `n`, row-major across the grid.
    pub fn frame_rect(&self, frame: usize) -> Rect {
        let (tile_w, tile_h) = self.tile();
        let frame = if self.frame_count == 0 {
            0
        } else {
            frame % self.frame_count
        };
        let col = (frame % self.cols) as f32;
        let row = (frame / self.cols) as f32;
        let (x, y) = (col * tile_w, row * tile_h);
        Rect::new(x, y, x + tile_w, y + tile_h)
    }

    /// The source rect to draw at `elapsed` seconds.
    pub fn rect_at(&self, elapsed_secs: f32) -> Rect {
        self.frame_rect(self.frame_at(elapsed_secs))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// The mini-info pair, whose keys this module was extracted from.
    const CHA: Flipbook = Flipbook {
        frame_count: 8,
        cols: 4,
        rows: 2,
        frame_ms: 100.0,
        sheet: (512.0, 64.0),
    };
    /// The quick-party pair — same animation, half the sheet.
    const QUICK: Flipbook = Flipbook {
        frame_count: 8,
        cols: 4,
        rows: 2,
        frame_ms: 100.0,
        sheet: (256.0, 64.0),
    };

    /// Frames step at `Speed` and loop at `FrameCount`.
    #[test]
    fn frames_step_at_the_declared_speed_and_loop() {
        assert_eq!(CHA.frame_at(0.0), 0);
        assert_eq!(CHA.frame_at(0.099), 0);
        assert_eq!(CHA.frame_at(0.1), 1);
        assert_eq!(CHA.frame_at(0.75), 7);
        assert_eq!(CHA.frame_at(0.8), 0, "EnableLoop=1");
    }

    /// The grid is walked row-major: frame 4 is the start of the second row,
    /// not the fifth column of the first.
    #[test]
    fn the_grid_is_row_major() {
        let (tile_w, tile_h) = CHA.tile();
        assert_eq!((tile_w, tile_h), (128.0, 32.0));
        assert_eq!(CHA.frame_rect(0), Rect::new(0.0, 0.0, 128.0, 32.0));
        assert_eq!(CHA.frame_rect(3).min.x, 3.0 * tile_w);
        assert_eq!(
            CHA.frame_rect(4),
            Rect::new(0.0, tile_h, tile_w, 2.0 * tile_h)
        );
        // and it wraps rather than running off the sheet
        assert_eq!(CHA.frame_rect(8), CHA.frame_rect(0));
    }

    /// The whole reason this is shared: the two sheets animate identically but
    /// crop different tiles, so the sheet size cannot be a constant.
    #[test]
    fn the_same_animation_crops_different_tiles_per_sheet() {
        assert_eq!(CHA.frame_at(0.35), QUICK.frame_at(0.35));
        assert_eq!(CHA.tile(), (128.0, 32.0));
        assert_eq!(QUICK.tile(), (64.0, 32.0));
        assert_ne!(CHA.frame_rect(1), QUICK.frame_rect(1));
    }

    /// A degenerate description must not divide by zero or wrap a cast.
    #[test]
    fn a_stopped_flipbook_stays_on_its_first_frame() {
        let stopped = Flipbook {
            frame_ms: 0.0,
            ..CHA
        };
        assert_eq!(stopped.frame_at(10.0), 0);
        assert_eq!(CHA.frame_at(-1.0), 0);
    }
}
