//! What one killfeed row says: whose it is, its weapon and its modifiers.

use image::{Rgb, RgbImage};
use serde::Serialize;

use crate::Row;

/// The kill modifiers tags are made from.
pub const KILL_MODIFIERS: [&str; 6] = [
    "headshot",
    "wallbang",
    "through_smoke",
    "noscope",
    "blind",
    "in_air",
];

/// Every mark the icon model knows: the kill modifiers, and three it reads so they aren't
/// mistaken for anything else. Its other classes are weapons.
pub const MARKS: [&str; 9] = [
    "headshot",
    "wallbang",
    "through_smoke",
    "noscope",
    "blind",
    "in_air",
    "flash_assist",
    "domination",
    "revenge",
];

/// Whose row it is, from the recording player's point of view.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum Owner {
    /// Red outline: the recording player got this kill.
    MyKill,
    /// Dark red fill: the recording player died.
    MyDeath,
    Other,
}

#[derive(Debug, Clone, PartialEq, Serialize)]
pub struct Reading {
    pub owner: Owner,
    /// The weapon (CS2's name, e.g. `ak47`) and its score (0..1).
    pub weapon: Option<(String, f32)>,
    /// Marks seen in the row, with scores.
    pub modifiers: Vec<(String, f32)>,
}

/// How much a pixel stands out as red (the own-row outline and fill).
fn redness(p: &Rgb<u8>) -> i32 {
    i32::from(p[0]) - i32::from(p[1].max(p[2]))
}

pub fn owner(corner: &RgbImage, row: &Row) -> Owner {
    let (w, h) = (corner.width() as i32, corner.height() as i32);
    let px = |x: i32, y: i32| corner.get_pixel(x.clamp(0, w - 1) as u32, y.clamp(0, h - 1) as u32);
    let (x0, x1) = (row.x0.round() as i32, row.x1.round() as i32);
    let (y0, y1) = (row.y0.round() as i32, row.y1.round() as i32);
    let row_h = (y1 - y0).max(1);

    // Red outline: the top and bottom edges are red along most of the row. Search a few
    // pixels around each edge, since the model's box is not pixel-exact.
    let band = (row_h / 8).max(2);
    let inner = (x0 + row_h / 2)..(x1 - row_h / 2);
    let edge_red = |y: i32| {
        let columns = inner.clone().step_by(2);
        let total = columns.clone().count().max(1);
        let red = columns
            .filter(|&x| {
                (y - band..=y + band).any(|yy| {
                    let p = px(x, yy);
                    p[0] >= 110 && redness(p) >= 60
                })
            })
            .count();
        red as f32 / total as f32
    };
    if edge_red(y0) >= 0.6 && edge_red(y1) >= 0.6 {
        return Owner::MyKill;
    }

    // Red fill: the row's dark background is clearly redder than what's around it (the
    // death screen tints everything red, so compare against the surroundings).
    let median = |mut v: Vec<i32>| {
        v.sort_unstable();
        v.get(v.len() / 2).copied().unwrap_or(0)
    };
    let dark_redness = |xs: std::ops::Range<i32>| {
        let mut v = Vec::new();
        for y in (y0 + band + 1)..(y1 - band) {
            for x in xs.clone().step_by(2) {
                let p = px(x, y);
                let lum = (u32::from(p[0]) + u32::from(p[1]) + u32::from(p[2])) / 3;
                if lum < 120 {
                    v.push(redness(p));
                }
            }
        }
        median(v)
    };
    let fill = dark_redness(inner.clone());
    let outside = dark_redness((x0 - row_h).max(0)..(x0 - 2).max(0));
    if fill >= 35 && fill - outside >= 20 {
        Owner::MyDeath
    } else {
        Owner::Other
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    const ROW: Row = Row {
        x0: 100.0,
        y0: 40.0,
        x1: 300.0,
        y1: 64.0,
        score: 1.0,
    };

    /// Paints the rectangle `x0..x1`, `y0..y1` of `corner` `color`.
    fn paint(corner: &mut RgbImage, (x0, y0, x1, y1): (u32, u32, u32, u32), color: [u8; 3]) {
        for x in x0..x1 {
            for y in y0..y1 {
                corner.put_pixel(x, y, Rgb(color));
            }
        }
    }

    /// A 320 x 120 corner of `background` with [`ROW`] filled with `row`.
    fn corner(background: [u8; 3], row: [u8; 3]) -> RgbImage {
        let mut corner = RgbImage::from_pixel(320, 120, Rgb(background));
        paint(&mut corner, (100, 40, 300, 64), row);
        corner
    }

    #[test]
    fn a_red_outline_is_the_players_kill() {
        let mut c = corner([90, 100, 110], [30, 30, 30]);
        // Two pixels of red along the top and bottom, a pixel or two off the box.
        paint(&mut c, (100, 41, 300, 43), [200, 20, 30]);
        paint(&mut c, (100, 62, 300, 64), [200, 20, 30]);
        assert_eq!(owner(&c, &ROW), Owner::MyKill);

        // Only along the top: a red banner above the row, not an outline.
        let mut c = corner([90, 100, 110], [30, 30, 30]);
        paint(&mut c, (100, 41, 300, 43), [200, 20, 30]);
        assert_eq!(owner(&c, &ROW), Owner::Other);

        // Dim red (or orange) isn't the outline either.
        let mut c = corner([90, 100, 110], [30, 30, 30]);
        paint(&mut c, (100, 41, 300, 43), [100, 10, 10]);
        paint(&mut c, (100, 62, 300, 64), [230, 190, 30]);
        assert_eq!(owner(&c, &ROW), Owner::Other);
    }

    #[test]
    fn a_red_fill_is_the_players_death() {
        // Dark red on a gray scene.
        assert_eq!(
            owner(&corner([60, 60, 60], [90, 20, 25]), &ROW),
            Owner::MyDeath
        );
        // Bright surroundings are no comparison: only the row's dark pixels count.
        assert_eq!(
            owner(&corner([200, 200, 200], [90, 20, 25]), &ROW),
            Owner::MyDeath
        );
        // The death screen tints everything red: a row as red as its surroundings is
        // someone else's.
        assert_eq!(
            owner(&corner([100, 40, 40], [100, 30, 30]), &ROW),
            Owner::Other
        );
        // A plain dark row.
        assert_eq!(
            owner(&corner([60, 60, 60], [30, 30, 35]), &ROW),
            Owner::Other
        );
    }

    #[test]
    fn rows_at_the_corners_edge() {
        // Nothing left of the row to compare with: the fill alone decides.
        let mut c = RgbImage::from_pixel(200, 60, Rgb([60, 60, 60]));
        paint(&mut c, (0, 0, 200, 24), [90, 20, 25]);
        let row = Row {
            x0: 0.0,
            y0: 0.0,
            x1: 200.0,
            y1: 24.0,
            score: 1.0,
        };
        assert_eq!(owner(&c, &row), Owner::MyDeath);
        // A box reaching past the image is read up to its edge.
        let row = Row {
            x0: -5.0,
            y0: -3.0,
            x1: 230.0,
            y1: 24.0,
            ..row
        };
        assert_eq!(owner(&c, &row), Owner::MyDeath);
        // A box too thin to have an inside.
        let row = Row {
            y0: 10.0,
            y1: 10.0,
            ..row
        };
        assert_eq!(owner(&c, &row), Owner::Other);
    }

    #[test]
    fn redness_is_red_over_the_strongest_other_channel() {
        assert_eq!(redness(&Rgb([200, 20, 30])), 170);
        assert_eq!(redness(&Rgb([20, 200, 30])), -180);
        assert!(MARKS.starts_with(&KILL_MODIFIERS));
    }
}
