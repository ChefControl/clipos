//! Following rows across frames, so each kill is counted once.
//!
//! A row stays up for about 5-7 s and moves up as older rows expire. A row in a new frame
//! continues a track when it hasn't moved down and either looks the same (a small
//! grayscale snapshot of the whole row, names included, compared with zero-mean
//! correlation) or shows the same icons at about the same width (rows fading in or out,
//! or over a changing background, can look different from frame to frame).

use std::collections::HashMap;

use image::{GrayImage, RgbImage, imageops};
use serde::Serialize;

use crate::{Owner, Reading, Row};

/// A row unseen for longer than this has left the killfeed. Covers short gaps where a
/// flashbang or an overlay hides it.
const MAX_GAP_S: f64 = 2.0;
/// Snapshot similarity needed to continue a track.
const MIN_SIMILARITY: f32 = 0.7;
const THUMB: (u32, u32) = (96, 8);

/// One kill: a row followed from when it appeared until it left.
#[derive(Debug, Clone, Serialize)]
pub struct Kill {
    /// When the row first appeared, in seconds from the start of the clip.
    pub t: f64,
    pub last_seen: f64,
    pub sightings: usize,
    pub owner: Owner,
    pub weapon: Option<String>,
    pub modifiers: Vec<String>,
    /// The clearest sighting (best weapon match): frame time and the row's box there.
    pub example: (f64, Row),
}

struct Track {
    first: f64,
    last: f64,
    row: Row,
    thumb: Vec<f32>,
    readings: Vec<Reading>,
    /// When and where each reading was taken.
    seen: Vec<(f64, Row)>,
}

#[derive(Default)]
pub struct Tracker {
    active: Vec<Track>,
    done: Vec<Track>,
}

impl Tracker {
    /// Adds a frame's rows (top to bottom) with their readings.
    pub fn push(&mut self, t: f64, corner: &RgbImage, rows: Vec<(Row, Reading)>) {
        let (expired, active): (Vec<_>, Vec<_>) = std::mem::take(&mut self.active)
            .into_iter()
            .partition(|track| t - track.last > MAX_GAP_S);
        self.done.extend(expired);
        self.active = active;

        let rows: Vec<(Row, Reading, Vec<f32>)> = rows
            .into_iter()
            .map(|(row, reading)| {
                let thumb = thumbnail(corner, &row);
                (row, reading, thumb)
            })
            .collect();
        // Every (row, track) pair that could be the same row, best match first.
        let mut pairs = Vec::new();
        for (i, (row, reading, thumb)) in rows.iter().enumerate() {
            for (j, track) in self.active.iter().enumerate() {
                let moved_down = row.y0 - track.row.y0 > 0.5 * track.row.height();
                if moved_down {
                    continue;
                }
                let similarity = correlation(thumb, &track.thumb);
                // A row fading in or out looks different from frame to frame; the same
                // icons in a row of the same width are the same row too.
                let same_content = track.readings.last().is_some_and(|last| {
                    same_icons(last, reading)
                        && (row.width() - track.row.width()).abs() <= 0.06 * track.row.width()
                });
                if similarity >= MIN_SIMILARITY || same_content {
                    pairs.push((
                        similarity.max(if same_content { MIN_SIMILARITY } else { 0.0 }),
                        i,
                        j,
                    ));
                }
            }
        }
        pairs.sort_by(|a, b| b.0.total_cmp(&a.0));
        let mut row_track: HashMap<usize, usize> = HashMap::new();
        let mut taken = vec![false; self.active.len()];
        for (_, i, j) in pairs {
            if !taken[j] && !row_track.contains_key(&i) {
                taken[j] = true;
                row_track.insert(i, j);
            }
        }
        for (i, (row, reading, thumb)) in rows.into_iter().enumerate() {
            match row_track.get(&i) {
                Some(&j) => {
                    let track = &mut self.active[j];
                    track.last = t;
                    track.row = row;
                    track.thumb = thumb;
                    track.readings.push(reading);
                    track.seen.push((t, row));
                }
                None => self.active.push(Track {
                    first: t,
                    last: t,
                    row,
                    thumb,
                    readings: vec![reading],
                    seen: vec![(t, row)],
                }),
            }
        }
    }

    /// The kills, in order. A row seen in a single frame is dropped as noise: real rows
    /// stay up for seconds.
    pub fn finish(mut self) -> Vec<Kill> {
        self.done.append(&mut self.active);
        let mut kills: Vec<Kill> = self
            .done
            .into_iter()
            .filter(|track| track.readings.len() >= 2)
            .map(summarise)
            .collect();
        kills.sort_by(|a, b| a.t.total_cmp(&b.t));
        kills
    }
}

/// Combines a track's readings: the weapon with the highest total score, modifiers seen
/// in at least half the readings, and the most common owner.
fn summarise(track: Track) -> Kill {
    let n = track.readings.len();
    let mut weapons: HashMap<&str, f32> = HashMap::new();
    let mut modifiers: HashMap<&str, usize> = HashMap::new();
    let mut owners: HashMap<Owner, usize> = HashMap::new();
    for reading in &track.readings {
        if let Some((name, score)) = &reading.weapon {
            *weapons.entry(name).or_default() += score;
        }
        for (name, _) in &reading.modifiers {
            *modifiers.entry(name).or_default() += 1;
        }
        *owners.entry(reading.owner).or_default() += 1;
    }
    let weapon = weapons
        .into_iter()
        .max_by(|a, b| a.1.total_cmp(&b.1))
        .map(|(name, _)| name.to_owned());
    let mut modifiers: Vec<String> = modifiers
        .into_iter()
        .filter(|(_, count)| 2 * count >= n)
        .map(|(name, _)| name.to_owned())
        .collect();
    modifiers.sort();
    let owner = owners
        .into_iter()
        .max_by_key(|(_, count)| *count)
        .map_or(Owner::Other, |(owner, _)| owner);
    let clearest = (0..n)
        .max_by(|&a, &b| {
            let score = |i: usize| track.readings[i].weapon.as_ref().map_or(0.0, |(_, s)| *s);
            score(a).total_cmp(&score(b))
        })
        .unwrap_or(0);
    Kill {
        example: track.seen[clearest],
        t: track.first,
        last_seen: track.last,
        sightings: n,
        owner,
        weapon,
        modifiers,
    }
}

/// A small grayscale picture of the whole row, names included.
fn thumbnail(corner: &RgbImage, row: &Row) -> Vec<f32> {
    let x0 = row.x0.max(0.0) as u32;
    let y0 = row.y0.max(0.0) as u32;
    let w = (row.width() as u32).clamp(1, corner.width() - x0.min(corner.width() - 1));
    let h = (row.height() as u32).clamp(1, corner.height() - y0.min(corner.height() - 1));
    let crop = imageops::crop_imm(corner, x0, y0, w, h).to_image();
    let gray: GrayImage = imageops::grayscale(&crop);
    imageops::resize(&gray, THUMB.0, THUMB.1, imageops::FilterType::Triangle)
        .pixels()
        .map(|p| f32::from(p[0]))
        .collect()
}

/// Same weapon and the same modifiers.
fn same_icons(a: &Reading, b: &Reading) -> bool {
    fn names(r: &Reading) -> (Option<&str>, Vec<&str>) {
        let mut m: Vec<&str> = r.modifiers.iter().map(|(n, _)| n.as_str()).collect();
        m.sort_unstable();
        (r.weapon.as_ref().map(|(n, _)| n.as_str()), m)
    }
    a.weapon.is_some() && names(a) == names(b)
}

/// Zero-mean normalised correlation, -1..1.
fn correlation(a: &[f32], b: &[f32]) -> f32 {
    let mean = |v: &[f32]| v.iter().sum::<f32>() / v.len() as f32;
    let (ma, mb) = (mean(a), mean(b));
    let (mut ab, mut aa, mut bb) = (0.0, 0.0, 0.0);
    for (x, y) in a.iter().zip(b) {
        let (x, y) = (x - ma, y - mb);
        ab += x * y;
        aa += x * x;
        bb += y * y;
    }
    if aa == 0.0 || bb == 0.0 {
        0.0
    } else {
        ab / (aa * bb).sqrt()
    }
}

#[cfg(test)]
mod tests {
    use image::Rgb;

    use super::*;

    const WIDTH: f32 = 200.0;
    const HEIGHT: f32 = 16.0;

    /// A 300 x 200 corner with rows `(look, y)`: each a 200 x 16 strip of noise, right
    /// aligned, the same for the same `look` and unrelated for different ones.
    fn corner(rows: &[(u32, f32)]) -> RgbImage {
        let mut corner = RgbImage::new(300, 200);
        for &(look, y) in rows {
            for x in 100..300 {
                let v = noise(x / 5, look);
                for dy in 0..HEIGHT as u32 {
                    corner.put_pixel(x, y as u32 + dy, Rgb([v, v, v]));
                }
            }
        }
        corner
    }

    /// A hash of `(x, look)`, for noise.
    fn noise(x: u32, look: u32) -> u8 {
        let mut h = x.wrapping_mul(0x9e37_79b1) ^ look.wrapping_mul(0x85eb_ca77);
        h ^= h >> 15;
        h = h.wrapping_mul(0x2c1b_3c6d);
        h ^= h >> 12;
        h = h.wrapping_mul(0x297a_2d39);
        h ^= h >> 15;
        (h >> 24) as u8
    }

    fn row(y: f32) -> Row {
        Row {
            x0: 300.0 - WIDTH,
            y0: y,
            x1: 300.0,
            y1: y + HEIGHT,
            score: 0.9,
        }
    }

    fn reading(owner: Owner, weapon: Option<(&str, f32)>, modifiers: &[&str]) -> Reading {
        Reading {
            owner,
            weapon: weapon.map(|(n, s)| (n.to_owned(), s)),
            modifiers: modifiers.iter().map(|m| ((*m).to_owned(), 0.9)).collect(),
        }
    }

    fn ak(score: f32) -> Option<(&'static str, f32)> {
        Some(("ak47", score))
    }

    /// Pushes a frame whose rows are `(look, y, reading)`.
    fn push(tracker: &mut Tracker, t: f64, rows: &[(u32, f32, Reading)]) {
        let looks: Vec<(u32, f32)> = rows.iter().map(|(look, y, _)| (*look, *y)).collect();
        tracker.push(
            t,
            &corner(&looks),
            rows.iter().map(|(_, y, r)| (row(*y), r.clone())).collect(),
        );
    }

    #[test]
    fn follows_rows_as_they_move_up() {
        let mut tracker = Tracker::default();
        let mine = |weapon, modifiers: &[&str]| reading(Owner::MyKill, weapon, modifiers);
        let other = |weapon| reading(Owner::Other, weapon, &[]);
        push(
            &mut tracker,
            0.0,
            &[(1, 40.0, mine(ak(0.9), &["headshot"]))],
        );
        push(
            &mut tracker,
            1.0,
            &[
                (1, 40.0, mine(ak(0.6), &["headshot"])),
                (2, 60.0, other(Some(("awp", 0.8)))),
            ],
        );
        push(
            &mut tracker,
            2.0,
            &[
                (1, 40.0, reading(Owner::Other, Some(("m4a1", 0.95)), &[])),
                (2, 60.0, other(None)),
            ],
        );
        // An older row expired: both move up.
        push(
            &mut tracker,
            3.0,
            &[
                (1, 20.0, mine(ak(0.7), &["headshot", "wallbang"])),
                (2, 40.0, other(Some(("awp", 0.6)))),
            ],
        );
        let kills = tracker.finish();
        assert_eq!(kills.len(), 2, "{kills:?}");

        let first = &kills[0];
        assert_eq!((first.t, first.last_seen, first.sightings), (0.0, 3.0, 4));
        // The highest total score (2.2 against 0.95), modifiers in at least half the
        // readings and the most common owner.
        assert_eq!(first.weapon.as_deref(), Some("ak47"));
        assert_eq!(first.modifiers, vec!["headshot"]);
        assert_eq!(first.owner, Owner::MyKill);
        // The clearest sighting is the best single weapon match.
        assert_eq!(first.example, (2.0, row(40.0)));

        let second = &kills[1];
        assert_eq!(
            (second.t, second.last_seen, second.sightings),
            (1.0, 3.0, 3)
        );
        assert_eq!(second.weapon.as_deref(), Some("awp"));
        assert!(second.modifiers.is_empty());
        assert_eq!(second.owner, Owner::Other);
        assert_eq!(second.example, (1.0, row(60.0)));
    }

    #[test]
    fn a_row_seen_once_is_noise() {
        let mut tracker = Tracker::default();
        let r = reading(Owner::Other, ak(0.9), &[]);
        push(&mut tracker, 0.0, &[(1, 40.0, r.clone())]);
        push(
            &mut tracker,
            1.0,
            &[(1, 40.0, r.clone()), (2, 60.0, r.clone())],
        );
        let kills = tracker.finish();
        assert_eq!(kills.len(), 1);
        assert_eq!(kills[0].sightings, 2);
        assert!(Tracker::default().finish().is_empty());
    }

    #[test]
    fn a_row_gone_for_over_two_seconds_has_left() {
        let mut tracker = Tracker::default();
        let r = reading(Owner::Other, ak(0.9), &[]);
        // Hidden for a second (a flashbang): still the same row.
        push(&mut tracker, 0.0, &[(1, 40.0, r.clone())]);
        push(&mut tracker, 2.0, &[(1, 40.0, r.clone())]);
        // Gone for longer: the same weapon in the same place later is another kill.
        push(&mut tracker, 4.5, &[(1, 40.0, r.clone())]);
        push(&mut tracker, 5.5, &[(1, 40.0, r.clone())]);
        let kills = tracker.finish();
        let times: Vec<_> = kills.iter().map(|k| (k.t, k.last_seen)).collect();
        assert_eq!(times, vec![(0.0, 2.0), (4.5, 5.5)]);
    }

    #[test]
    fn rows_never_move_down() {
        let mut tracker = Tracker::default();
        let r = reading(Owner::Other, ak(0.9), &[]);
        push(&mut tracker, 0.0, &[(1, 20.0, r.clone())]);
        push(&mut tracker, 1.0, &[(1, 24.0, r.clone())]); // a few pixels: jitter
        push(&mut tracker, 2.0, &[(1, 44.0, r.clone())]);
        push(&mut tracker, 3.0, &[(1, 44.0, r.clone())]);
        let kills = tracker.finish();
        let times: Vec<_> = kills.iter().map(|k| (k.t, k.sightings)).collect();
        assert_eq!(times, vec![(0.0, 2), (2.0, 2)]);
    }

    #[test]
    fn a_fading_row_is_followed_by_its_icons() {
        let mut tracker = Tracker::default();
        let r = reading(Owner::MyKill, ak(0.9), &["headshot"]);
        push(&mut tracker, 0.0, &[(1, 40.0, r.clone())]);
        // Looks different (fading over a changing background), same icons, same width.
        push(&mut tracker, 1.0, &[(2, 40.0, r.clone())]);
        // Looks different and other icons: a new row.
        push(
            &mut tracker,
            2.0,
            &[(3, 40.0, reading(Owner::Other, ak(0.9), &[]))],
        );
        // No weapon read: nothing to go by.
        let blank = reading(Owner::Other, None, &[]);
        push(&mut tracker, 3.0, &[(4, 40.0, blank.clone())]);
        push(&mut tracker, 4.0, &[(5, 40.0, blank)]);
        let kills = tracker.finish();
        let sightings: Vec<_> = kills.iter().map(|k| (k.t, k.sightings)).collect();
        assert_eq!(sightings, vec![(0.0, 2)]);
    }

    #[test]
    fn a_wider_row_with_the_same_icons_is_another_row() {
        let mut tracker = Tracker::default();
        let r = reading(Owner::Other, ak(0.9), &[]);
        push(&mut tracker, 0.0, &[(1, 40.0, r.clone())]);
        let wider = Row {
            x0: 300.0 - 1.2 * WIDTH,
            ..row(40.0)
        };
        tracker.push(1.0, &corner(&[(2, 40.0)]), vec![(wider, r.clone())]);
        push(&mut tracker, 2.0, &[(1, 40.0, r)]);
        let kills = tracker.finish();
        assert_eq!(kills.len(), 1);
        assert_eq!((kills[0].t, kills[0].last_seen), (0.0, 2.0));
    }

    #[test]
    fn each_track_continues_one_row() {
        // Two rows that look alike in the next frame: one continues the row, the other
        // starts a new one.
        let mut tracker = Tracker::default();
        let r = reading(Owner::Other, ak(0.9), &[]);
        push(&mut tracker, 0.0, &[(1, 40.0, r.clone())]);
        push(
            &mut tracker,
            1.0,
            &[(1, 20.0, r.clone()), (1, 40.0, r.clone())],
        );
        push(&mut tracker, 2.0, &[(1, 20.0, r.clone()), (1, 40.0, r)]);
        let kills = tracker.finish();
        let seen: Vec<_> = kills.iter().map(|k| (k.t, k.sightings)).collect();
        assert_eq!(seen, vec![(0.0, 3), (1.0, 2)]);
    }

    #[test]
    fn compares_snapshots_by_correlation() {
        let a = [1.0, 2.0, 3.0, 4.0];
        assert!((correlation(&a, &a) - 1.0).abs() < 1e-6);
        assert!((correlation(&a, &[8.0, 6.0, 4.0, 2.0]) + 1.0).abs() < 1e-6);
        // Brighter and stronger is still the same picture.
        assert!((correlation(&a, &[12.0, 14.0, 16.0, 18.0]) - 1.0).abs() < 1e-6);
        // A flat picture is like nothing.
        assert_eq!(correlation(&a, &[5.0; 4]), 0.0);
        assert_eq!(correlation(&[0.0; 4], &a), 0.0);
    }

    #[test]
    fn snapshots_stay_inside_the_corner() {
        let c = corner(&[(1, 40.0)]);
        let past = Row {
            x0: -10.0,
            y0: 190.0,
            x1: 400.0,
            y1: 230.0,
            score: 1.0,
        };
        let inside = Row {
            x0: 0.0,
            x1: 300.0,
            y1: 200.0,
            ..past
        };
        let thumb = thumbnail(&c, &past);
        assert_eq!(thumb.len(), (THUMB.0 * THUMB.1) as usize);
        assert_eq!(thumb, thumbnail(&c, &inside));
        // The test rows: the same look matches, different looks don't.
        let look = |n| thumbnail(&corner(&[(n, 40.0)]), &row(40.0));
        assert!(correlation(&look(1), &look(1)) > 0.999);
        for other in 2..6 {
            assert!(correlation(&look(1), &look(other)) < MIN_SIMILARITY);
        }
    }

    #[test]
    fn same_icons_needs_a_weapon() {
        let hs = reading(Owner::Other, ak(0.9), &["headshot", "wallbang"]);
        let sh = reading(Owner::MyKill, ak(0.5), &["wallbang", "headshot"]);
        assert!(same_icons(&hs, &sh), "order and scores don't matter");
        assert!(!same_icons(&hs, &reading(Owner::Other, ak(0.9), &[])));
        let blank = reading(Owner::Other, None, &[]);
        assert!(!same_icons(&blank, &blank));
    }
}
