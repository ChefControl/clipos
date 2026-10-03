//! Reading rows with the icon detector (trained on generated rows, see
//! ChefControl/clipos-killfeed-training): a frame's rows are cut with a margin, stacked into a 640x640 sheet exactly as
//! the generator stacks them, and the model finds every icon in one pass.

use std::path::Path;

use anyhow::Context;
use image::{RgbImage, imageops};

use crate::{Detection, Detector, MARKS, Owner, Reading, Row, owner};

// Sheet layout: must match `sheet()` and `MARGIN` in the training repo's synth.py.
const SHEET: u32 = 640;
const SLOT: u32 = 56;
const GAP: u32 = 2;
const MARGIN: f32 = 0.15;
const ROWS_PER_SHEET: usize = ((SHEET - GAP) / (SLOT + GAP)) as usize;

/// Detections below this are ignored. A weapon needs `MIN_WEAPON` and a modifier
/// `MIN_MARK`: below that the row is left without one rather than guessed. On ~9,000 real
/// rows only 13 weapon reads were under 0.5, nearly all rows faded almost to nothing.
const MIN_SCORE: f32 = 0.3;
const MIN_WEAPON: f32 = 0.5;
const MIN_MARK: f32 = 0.5;

pub struct IconReader {
    detector: Detector,
    /// Class index -> icon name, and whether it is a mark (modifier) rather than a weapon.
    classes: Vec<(String, bool)>,
}

impl IconReader {
    /// Loads the model and the `classes.json` exported next to it.
    pub fn load(model: &Path, threads: usize) -> anyhow::Result<Self> {
        let path = model.with_file_name("classes.json");
        let names: Vec<String> = serde_json::from_str(
            &std::fs::read_to_string(&path)
                .with_context(|| format!("reading {}", path.display()))?,
        )?;
        let classes = names
            .into_iter()
            .map(|name| {
                let is_mark = MARKS.contains(&name.as_str());
                (name, is_mark)
            })
            .collect();
        Ok(Self {
            detector: Detector::load(model, threads)?,
            classes,
        })
    }

    /// Reads every row of one frame; `rows` are the row-finder's boxes in `corner`.
    pub fn read(&mut self, corner: &RgbImage, rows: &[Row]) -> anyhow::Result<Vec<Reading>> {
        let mut readings = Vec::with_capacity(rows.len());
        for group in rows.chunks(ROWS_PER_SHEET) {
            let (sheet, slots) = sheet(corner, group);
            let detections = self.detector.detect(&sheet, MIN_SCORE)?;
            for (row, slot) in group.iter().zip(&slots) {
                readings.push(reading(
                    &self.classes,
                    &detections,
                    *slot,
                    owner(corner, row),
                ));
            }
        }
        Ok(readings)
    }
}

/// What the `detections` on a sheet say about the row in `slot`: its weapon, if one is
/// sure enough, and its marks, by name. `classes` are the model's (name, is a mark).
fn reading(
    classes: &[(String, bool)],
    detections: &[Detection],
    slot: (f32, f32),
    owner: Owner,
) -> Reading {
    let mut weapons: Vec<(&str, f32)> = Vec::new();
    let mut marks: Vec<(&str, f32)> = Vec::new();
    for d in detections {
        let y = d.cy * SHEET as f32;
        if y < slot.0 || y >= slot.1 {
            continue;
        }
        let (name, is_mark) = (&classes[d.class].0, classes[d.class].1);
        let list = if is_mark { &mut marks } else { &mut weapons };
        match list.iter_mut().find(|(n, _)| *n == name) {
            Some(entry) => entry.1 = entry.1.max(d.score),
            None => list.push((name.as_str(), d.score)),
        }
    }
    let owned = |(n, s): (&str, f32)| (n.to_owned(), s);
    let weapon = weapons
        .into_iter()
        .max_by(|a, b| a.1.total_cmp(&b.1))
        .filter(|(_, s)| *s >= MIN_WEAPON)
        .map(owned);
    marks.retain(|(_, score)| *score >= MIN_MARK);
    marks.sort_by(|a, b| a.0.cmp(b.0));
    Reading {
        owner,
        weapon,
        modifiers: marks.into_iter().map(owned).collect(),
    }
}

/// The rows cut with a margin and stacked into one sheet, and each row's vertical slot
/// (top, bottom) on it.
pub fn sheet(corner: &RgbImage, rows: &[Row]) -> (RgbImage, Vec<(f32, f32)>) {
    let mut canvas = RgbImage::new(SHEET, SHEET);
    let mut slots = Vec::with_capacity(rows.len());
    let mut y = GAP;
    for row in rows {
        let m = (row.height() * MARGIN).round();
        let x0 = (row.x0 - m).max(0.0) as u32;
        let y0 = (row.y0 - m).max(0.0) as u32;
        let x1 = ((row.x1 + m) as u32).min(corner.width());
        let y1 = ((row.y1 + m) as u32).min(corner.height());
        let (w, h) = (x1.saturating_sub(x0).max(1), y1.saturating_sub(y0).max(1));
        let cut = imageops::crop_imm(corner, x0, y0, w, h).to_image();
        let mut scale = SLOT as f32 / h as f32;
        if w as f32 * scale > (SHEET - 2 * GAP) as f32 {
            scale = (SHEET - 2 * GAP) as f32 / w as f32;
        }
        let (sw, sh) = (
            ((w as f32 * scale).round() as u32).max(1),
            ((h as f32 * scale).round() as u32).max(1),
        );
        let scaled = imageops::resize(&cut, sw, sh, imageops::FilterType::Triangle);
        imageops::replace(&mut canvas, &scaled, i64::from(GAP), i64::from(y));
        slots.push((y as f32, (y + SLOT + GAP) as f32));
        y += SLOT + GAP;
    }
    (canvas, slots)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn stacks_rows_in_slots() {
        let corner = RgbImage::new(810, 540);
        let rows = [
            Row {
                x0: 500.0,
                y0: 70.0,
                x1: 800.0,
                y1: 102.0,
                score: 1.0,
            },
            Row {
                x0: 300.0,
                y0: 105.0,
                x1: 800.0,
                y1: 137.0,
                score: 1.0,
            },
        ];
        let (sheet, slots) = sheet(&corner, &rows);
        assert_eq!(sheet.dimensions(), (640, 640));
        assert_eq!(slots, vec![(2.0, 60.0), (60.0, 118.0)]);
        assert_eq!(ROWS_PER_SHEET, 11);
    }

    #[test]
    fn scales_rows_to_the_slot_and_keeps_wide_ones_on_the_sheet() {
        // A white row on black: the cut, with its margin, is scaled to the 56 px slot.
        let mut corner = RgbImage::new(400, 200);
        for x in 300..400 {
            for y in 20..40 {
                corner.put_pixel(x, y, image::Rgb([255, 255, 255]));
            }
        }
        let row = Row {
            x0: 300.0,
            y0: 20.0,
            x1: 400.0,
            y1: 40.0,
            score: 1.0,
        };
        let (stacked, _) = sheet(&corner, &[row]);
        // Margin 3 px: the cut is 103 x 26 (clipped at the right edge), scaled by 56 / 26.
        let white = |x, y| stacked.get_pixel(x, y)[0] > 200;
        assert!(white(2 + 10, 2 + 28), "the row is on the sheet");
        assert!(!white(2 + 3, 2 + 2), "its margin is not");
        assert!(!white(2 + 230, 2 + 28), "nothing past the cut");

        // A row wider than the sheet is scaled to fit its width instead.
        let wide = RgbImage::from_pixel(2000, 100, image::Rgb([255, 255, 255]));
        let row = Row {
            x0: 0.0,
            y0: 40.0,
            x1: 2000.0,
            y1: 60.0,
            score: 1.0,
        };
        let (sheet, _) = sheet(&wide, &[row]);
        assert!(sheet.get_pixel(637, 5)[0] > 200);
        assert_eq!(sheet.get_pixel(639, 5)[0], 0, "a gap at the right edge");
        assert_eq!(sheet.get_pixel(300, 2 + 30)[0], 0, "shorter than the slot");
    }

    fn at(class: usize, score: f32, y: f32) -> Detection {
        Detection {
            class,
            score,
            cx: 0.5,
            cy: y / SHEET as f32,
            w: 0.1,
            h: 0.05,
        }
    }

    fn classes() -> Vec<(String, bool)> {
        ["ak47", "awp", "headshot", "wallbang", "flash_assist"]
            .into_iter()
            .map(|n| (n.to_owned(), MARKS.contains(&n)))
            .collect()
    }

    #[test]
    fn reads_the_best_weapon_and_the_sure_marks_of_a_slot() {
        let detections = [
            at(0, 0.6, 30.0),
            at(0, 0.9, 31.0), // the same weapon again: its best score counts
            at(1, 0.8, 32.0),
            at(4, 0.9, 20.0),
            at(2, 0.7, 40.0),
            at(2, 0.55, 41.0),
            at(3, 0.45, 30.0), // too unsure for a mark
            at(1, 0.99, 60.0), // the next slot (the bottom is exclusive)
            at(3, 0.99, 1.0),  // the gap above the first slot
        ];
        assert_eq!(
            reading(&classes(), &detections, (2.0, 60.0), Owner::MyKill),
            Reading {
                owner: Owner::MyKill,
                weapon: Some(("ak47".to_owned(), 0.9)),
                modifiers: vec![
                    ("flash_assist".to_owned(), 0.9),
                    ("headshot".to_owned(), 0.7)
                ],
            }
        );
    }

    #[test]
    fn leaves_a_row_without_a_weapon_rather_than_guess() {
        let blank = Reading {
            owner: Owner::Other,
            weapon: None,
            modifiers: Vec::new(),
        };
        let unsure = [at(1, 0.45, 70.0)];
        assert_eq!(
            reading(&classes(), &unsure, (60.0, 118.0), Owner::Other),
            blank
        );
        assert_eq!(reading(&classes(), &[], (60.0, 118.0), Owner::Other), blank);
    }
}
