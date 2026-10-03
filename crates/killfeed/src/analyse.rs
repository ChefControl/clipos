//! The whole analysis for one clip: feed it the killfeed corner of each sampled frame,
//! get the kills back.

use image::RgbImage;

use crate::{IconReader, Kill, RowFinder, Tracker};

/// Frames per second to sample. Rows stay up for 5+ s, so 1 fps still sees each one about
/// five times: on the labelled clips 1 fps (and even 0.5) found the same kills as 2 fps, in
/// half the time. Below 1 fps a row is seen too few times for the tracker to rely on.
pub const SAMPLE_FPS: f64 = 1.0;

pub struct Analyzer<'a> {
    finder: &'a mut RowFinder,
    reader: &'a mut IconReader,
    tracker: Tracker,
    frames: u32,
}

impl<'a> Analyzer<'a> {
    pub fn new(finder: &'a mut RowFinder, reader: &'a mut IconReader) -> Self {
        Self {
            finder,
            reader,
            tracker: Tracker::default(),
            frames: 0,
        }
    }

    /// Adds the frame at `t` seconds; `corner` is its [`crate::crop_box`] cut.
    pub fn push(&mut self, t: f64, corner: &RgbImage) -> anyhow::Result<()> {
        self.frames += 1;
        let rows = self.finder.find(corner)?;
        if rows.is_empty() {
            return Ok(());
        }
        let readings = self.reader.read(corner, &rows)?;
        self.tracker
            .push(t, corner, rows.into_iter().zip(readings).collect());
        Ok(())
    }

    /// How many frames were pushed.
    pub fn frames(&self) -> u32 {
        self.frames
    }

    pub fn finish(self) -> Vec<Kill> {
        self.tracker.finish()
    }
}
