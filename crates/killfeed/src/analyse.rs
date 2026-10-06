//! The whole analysis for one clip: feed it each sampled frame, get the kills back.

use image::{RgbImage, imageops};

use crate::{HudLocator, IconReader, Kill, Tracker, crop_box};

/// Frames per second to sample. Rows stay up for 5+ s, so 1 fps still sees each one about
/// five times: on the labelled clips 1 fps (and even 0.5) found the same kills as 2 fps, in
/// half the time. Below 1 fps a row is seen too few times for the tracker to rely on.
pub const SAMPLE_FPS: f64 = 1.0;

pub struct Analyzer<'a> {
    locator: &'a mut HudLocator,
    reader: &'a mut IconReader,
    tracker: Tracker,
    frames: u32,
}

impl<'a> Analyzer<'a> {
    pub fn new(locator: &'a mut HudLocator, reader: &'a mut IconReader) -> Self {
        Self {
            locator,
            reader,
            tracker: Tracker::default(),
            frames: 0,
        }
    }

    /// Adds the whole frame at `t` seconds. The locator finds the rows in it; they are
    /// read, judged and followed in its killfeed corner ([`crop_box`]), at full resolution.
    pub fn push(&mut self, t: f64, frame: &RgbImage) -> anyhow::Result<()> {
        self.frames += 1;
        let cut = crop_box(frame.width(), frame.height());
        let rows = self.locator.rows(frame, cut)?;
        if rows.is_empty() {
            return Ok(());
        }
        let corner = imageops::crop_imm(frame, cut.0, cut.1, cut.2, cut.3).to_image();
        let readings = self.reader.read(&corner, &rows)?;
        self.tracker
            .push(t, &corner, rows.into_iter().zip(readings).collect());
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
