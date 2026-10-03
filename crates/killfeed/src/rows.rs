//! Finding killfeed rows with the trained row-finder model.

use std::path::Path;

use image::RgbImage;

use crate::detector::{Detection, Detector};

/// The killfeed corner of a `width` x `height` frame as `(x, y, w, h)`: the top half,
/// 0.75 x height wide from the right edge. Frames wider than 2:1 are already crops of
/// the killfeed and are used whole. Must match `crop_box` in the training repo's `prepare.py`,
/// which is what the model was trained on.
pub fn crop_box(width: u32, height: u32) -> (u32, u32, u32, u32) {
    if width > 2 * height {
        return (0, 0, width, height);
    }
    let crop_w = width.min((0.75 * f64::from(height)).round() as u32);
    (
        width - crop_w,
        0,
        crop_w,
        (0.5 * f64::from(height)).round() as u32,
    )
}

/// One killfeed row, in pixels of the corner image it was found in.
#[derive(Debug, Clone, Copy, PartialEq, serde::Serialize, serde::Deserialize)]
pub struct Row {
    pub x0: f32,
    pub y0: f32,
    pub x1: f32,
    pub y1: f32,
    /// Model confidence, 0..1.
    pub score: f32,
}

impl Row {
    pub fn width(&self) -> f32 {
        self.x1 - self.x0
    }

    pub fn height(&self) -> f32 {
        self.y1 - self.y0
    }

    fn iou(&self, other: &Row) -> f32 {
        let w = (self.x1.min(other.x1) - self.x0.max(other.x0)).max(0.0);
        let h = (self.y1.min(other.y1) - self.y0.max(other.y0)).max(0.0);
        let inter = w * h;
        inter / (self.width() * self.height() + other.width() * other.height() - inter)
    }
}

/// Rows below this confidence are ignored.
pub const MIN_SCORE: f32 = 0.5;

/// Killfeed rows are right-aligned: in all ~9,000 training rows the right edge is at
/// 89% of the corner's width or more. Boxes ending further left are other HUD panels
/// that look like rows (the "ROUND WON" banner, the round MVP card).
const MIN_RIGHT_EDGE: f32 = 0.8;

/// Row height as a share of the corner's height: 5.2-8.2% for 99.9% of the training
/// rows. CS2's `hud_scaling` (0.5-0.95, default 0.85) can shrink rows a lot but grow
/// them only a little, hence the lopsided range. Taller boxes are street signs and
/// other panels on the map.
const ROW_HEIGHT: std::ops::RangeInclusive<f32> = 0.03..=0.11;

pub struct RowFinder {
    detector: Detector,
}

impl RowFinder {
    /// Loads the row-finder model (`model.onnx` of `killfeed-rows/<version>`).
    pub fn load(model: &Path, threads: usize) -> anyhow::Result<Self> {
        Ok(Self {
            detector: Detector::load(model, threads)?,
        })
    }

    /// Every row in `corner` (a [`crop_box`] cut of a frame), top to bottom.
    pub fn find(&mut self, corner: &RgbImage) -> anyhow::Result<Vec<Row>> {
        let detections = self.detector.detect(corner, MIN_SCORE)?;
        Ok(rows(detections, corner.width(), corner.height()))
    }
}

/// The rows among the row-finder's `detections` in a `width` x `height` corner, in pixels,
/// top to bottom.
fn rows(detections: Vec<Detection>, width: u32, height: u32) -> Vec<Row> {
    let (w, h) = (width as f32, height as f32);
    // A corner wider than 2:1 is a whole frame that is already a killfeed crop, with
    // rows at a different scale (see `crop_box`).
    let check_height = width <= 2 * height;
    let mut rows: Vec<Row> = detections
        .into_iter()
        .filter(|d| d.cx + d.w / 2.0 >= MIN_RIGHT_EDGE)
        .filter(|d| !check_height || ROW_HEIGHT.contains(&d.h))
        .map(|d| Row {
            x0: (d.cx - d.w / 2.0) * w,
            y0: (d.cy - d.h / 2.0) * h,
            x1: (d.cx + d.w / 2.0) * w,
            y1: (d.cy + d.h / 2.0) * h,
            score: d.score,
        })
        .collect();
    // DETR rarely duplicates a box, but drop any near-copies of a stronger one.
    rows.sort_by(|a, b| b.score.total_cmp(&a.score));
    let mut kept: Vec<Row> = Vec::with_capacity(rows.len());
    for row in rows {
        if kept.iter().all(|k| k.iou(&row) < 0.6) {
            kept.push(row);
        }
    }
    kept.sort_by(|a, b| a.y0.total_cmp(&b.y0));
    kept
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn crops_the_killfeed_corner() {
        assert_eq!(crop_box(1920, 1080), (1110, 0, 810, 540));
        assert_eq!(crop_box(1280, 960), (560, 0, 720, 480));
        // Already a killfeed crop: used whole.
        assert_eq!(crop_box(1347, 500), (0, 0, 1347, 500));
        // Narrower than the usual corner: the whole width.
        assert_eq!(crop_box(300, 600), (0, 0, 300, 300));
    }

    fn detection(cx: f32, cy: f32, w: f32, h: f32, score: f32) -> Detection {
        Detection {
            class: 0,
            score,
            cx,
            cy,
            w,
            h,
        }
    }

    #[test]
    fn keeps_right_aligned_rows_of_a_rows_height() {
        let found = rows(
            vec![
                detection(0.75, 0.3, 0.5, 0.07, 0.9),
                detection(0.7, 0.2, 0.6, 0.07, 0.8),
                // Ends at 40 % of the width: another HUD panel.
                detection(0.3, 0.5, 0.2, 0.07, 0.99),
                // Too tall and too short to be a row.
                detection(0.8, 0.6, 0.4, 0.2, 0.99),
                detection(0.8, 0.7, 0.4, 0.02, 0.99),
            ],
            800,
            400,
        );
        // Top to bottom, in the corner's pixels.
        let boxes: Vec<_> = found
            .iter()
            .map(|r| [r.x0, r.y0, r.x1, r.y1].map(f32::round))
            .collect();
        assert_eq!(
            boxes,
            vec![[320.0, 66.0, 800.0, 94.0], [400.0, 106.0, 800.0, 134.0]]
        );
        assert_eq!(found[0].score, 0.8);
    }

    #[test]
    fn rows_of_a_killfeed_crop_can_be_any_height() {
        // Wider than 2:1: the frame is already a crop, its rows taller than in a corner.
        let found = rows(vec![detection(0.8, 0.5, 0.4, 0.3, 0.9)], 1000, 300);
        assert_eq!(found.len(), 1);
        assert_eq!(
            [found[0].x0, found[0].y0, found[0].x1, found[0].y1].map(f32::round),
            [600.0, 105.0, 1000.0, 195.0]
        );
    }

    #[test]
    fn drops_near_copies_of_a_stronger_row() {
        let found = rows(
            vec![
                detection(0.705, 0.205, 0.59, 0.07, 0.7),
                detection(0.7, 0.2, 0.6, 0.07, 0.95),
                // Overlapping, but not nearly the same box: a row of its own.
                detection(0.7, 0.24, 0.6, 0.07, 0.6),
            ],
            800,
            400,
        );
        assert_eq!(
            found.iter().map(|r| r.score).collect::<Vec<_>>(),
            vec![0.95, 0.6]
        );
    }

    #[test]
    fn measures_overlap() {
        let row = |x0, y0, x1, y1| Row {
            x0,
            y0,
            x1,
            y1,
            score: 1.0,
        };
        let a = row(0.0, 0.0, 10.0, 10.0);
        assert_eq!((a.width(), a.height()), (10.0, 10.0));
        assert_eq!(a.iou(&a), 1.0);
        // Half of each: 50 / (100 + 100 - 50).
        assert_eq!(a.iou(&row(5.0, 0.0, 15.0, 10.0)), 50.0 / 150.0);
        assert_eq!(a.iou(&row(20.0, 20.0, 30.0, 30.0)), 0.0);
    }
}
