//! Finding killfeed rows with the HUD locator, the model that finds every element of CS2's
//! HUD (`hud-locator/<version>`, trained in ChefControl/clipos-killfeed-training).

use std::path::Path;

use anyhow::{Context, anyhow};
use image::RgbImage;

use crate::{
    Row,
    detector::{Detection, Detector},
};

/// Boxes below this confidence are ignored.
pub const MIN_SCORE: f32 = 0.5;

/// The locator's class for killfeed rows (its `classes.json` names them).
const KILLFEED_ROW: &str = "killfeed_row";

pub struct HudLocator {
    detector: Detector,
    /// The index of [`KILLFEED_ROW`] among the model's classes.
    row_class: usize,
}

impl HudLocator {
    /// Loads the locator (`model.onnx` of `hud-locator/<version>`, with its `classes.json`
    /// next to it).
    pub fn load(model: &Path, threads: usize) -> anyhow::Result<Self> {
        let path = model.with_file_name("classes.json");
        let classes: Vec<String> = serde_json::from_str(
            &std::fs::read_to_string(&path)
                .with_context(|| format!("reading {}", path.display()))?,
        )
        .with_context(|| format!("reading {}", path.display()))?;
        let row_class = classes
            .iter()
            .position(|c| c == KILLFEED_ROW)
            .ok_or_else(|| anyhow!("{} has no {KILLFEED_ROW} class", path.display()))?;
        Ok(Self {
            detector: Detector::load(model, threads)?,
            row_class,
        })
    }

    /// The killfeed rows of a whole `frame`, in the pixels of its `corner` (a
    /// [`crate::crop_box`] cut, as `(x, y, w, h)`), top to bottom. The locator sees the
    /// whole frame, so nothing that only looks like a row in the corner (a street sign,
    /// the chat) is one; rows whose middle is outside the corner are left out.
    pub fn rows(
        &mut self,
        frame: &RgbImage,
        corner: (u32, u32, u32, u32),
    ) -> anyhow::Result<Vec<Row>> {
        let detections = self.detector.detect(frame, MIN_SCORE)?;
        Ok(rows(
            detections,
            self.row_class,
            (frame.width(), frame.height()),
            corner,
        ))
    }
}

/// The killfeed rows among the locator's `detections` of a `size` frame, moved into the
/// pixels of its `corner`.
fn rows(
    detections: Vec<Detection>,
    row_class: usize,
    (width, height): (u32, u32),
    (cx, cy, cw, ch): (u32, u32, u32, u32),
) -> Vec<Row> {
    let (fw, fh) = (width as f32, height as f32);
    let (cx, cy, cw, ch) = (cx as f32, cy as f32, cw as f32, ch as f32);
    let mut rows: Vec<Row> = detections
        .into_iter()
        .filter(|d| d.class == row_class)
        .map(|d| Row {
            x0: (d.cx - d.w / 2.0) * fw - cx,
            y0: (d.cy - d.h / 2.0) * fh - cy,
            x1: (d.cx + d.w / 2.0) * fw - cx,
            y1: (d.cy + d.h / 2.0) * fh - cy,
            score: d.score,
        })
        .filter(|r| {
            let (mx, my) = ((r.x0 + r.x1) / 2.0, (r.y0 + r.y1) / 2.0);
            (0.0..cw).contains(&mx) && (0.0..ch).contains(&my)
        })
        .map(|r| Row {
            x0: r.x0.max(0.0),
            y0: r.y0.max(0.0),
            x1: r.x1.min(cw),
            y1: r.y1.min(ch),
            ..r
        })
        .collect();
    // Stacked rows overlap a little; a box mostly covering a stronger one is the same row.
    rows.sort_by(|a, b| b.score.total_cmp(&a.score));
    let mut kept: Vec<Row> = Vec::with_capacity(rows.len());
    for row in rows {
        if kept.iter().all(|k| k.iou(&row) < 0.5) {
            kept.push(row);
        }
    }
    kept.sort_by(|a, b| a.y0.total_cmp(&b.y0));
    kept
}

#[cfg(test)]
mod tests {
    use super::*;

    fn detection(class: usize, cx: f32, cy: f32, w: f32, h: f32, score: f32) -> Detection {
        Detection {
            class,
            score,
            cx,
            cy,
            w,
            h,
        }
    }

    #[test]
    fn keeps_the_killfeed_rows_in_the_corner() {
        // A 1000 x 500 frame; its corner is x 500.., y ..250.
        let found = rows(
            vec![
                detection(1, 0.8, 0.2, 0.3, 0.04, 0.7),
                detection(1, 0.85, 0.1, 0.2, 0.04, 0.9),
                // A near-copy of the first, weaker: the same row.
                detection(1, 0.802, 0.201, 0.3, 0.04, 0.6),
                // Another element.
                detection(0, 0.9, 0.1, 0.1, 0.1, 0.99),
                // A row-like box in the chat, bottom left.
                detection(1, 0.2, 0.7, 0.3, 0.04, 0.9),
                // Reaching past the corner's edge: cut to it.
                detection(1, 0.9, 0.3, 0.4, 0.04, 0.8),
            ],
            1,
            (1000, 500),
            (500, 0, 500, 250),
        );
        let boxes: Vec<_> = found
            .iter()
            .map(|r| [r.x0, r.y0, r.x1, r.y1].map(f32::round))
            .collect();
        assert_eq!(
            boxes,
            vec![
                [250.0, 40.0, 450.0, 60.0],
                [150.0, 90.0, 450.0, 110.0],
                [200.0, 140.0, 500.0, 160.0],
            ]
        );
        assert_eq!(found[1].score, 0.7);
    }
}
