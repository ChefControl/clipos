//! The models' plumbing, run on the tiny stand-in models in `tests/models` (see
//! `make_models.py` there for what they answer). Needs ONNX Runtime: set `ORT_DYLIB_PATH`
//! to its library, as the worker does. Without it these tests are skipped, unless
//! `CLIPOS_REQUIRE_ORT` is set (CI), which makes that a failure.

use std::path::{Path, PathBuf};

use clipos_killfeed::{
    Analyzer, Detector, IconReader, Owner, Reading, Row, RowFinder, crop_box, summarise,
};
use image::{Rgb, RgbImage};

fn ort() -> bool {
    if std::env::var_os("ORT_DYLIB_PATH").is_some() {
        return true;
    }
    assert!(
        std::env::var_os("CLIPOS_REQUIRE_ORT").is_none(),
        "CLIPOS_REQUIRE_ORT is set, but ORT_DYLIB_PATH isn't"
    );
    eprintln!("ORT_DYLIB_PATH not set; skipping");
    false
}

fn model(path: &str) -> PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR"))
        .join("tests/models")
        .join(path)
}

fn sigmoid(x: f32) -> f32 {
    1.0 / (1.0 + (-x).exp())
}

fn close(a: f32, b: f32) -> bool {
    (a - b).abs() < 1e-4
}

const GRAY: Rgb<u8> = Rgb([128, 128, 128]);

/// The killfeed corner of a 640 x 360 frame.
const CORNER: (u32, u32) = (270, 180);

fn paint(corner: &mut RgbImage, (x0, y0, x1, y1): (u32, u32, u32, u32), color: [u8; 3]) {
    for x in x0..x1 {
        for y in y0..y1 {
            corner.put_pixel(x, y, Rgb(color));
        }
    }
}

/// A gray corner where the stand-in row finder's first row has the player's red outline
/// and its second the red fill of their death.
fn corner_with_my_rows() -> RgbImage {
    let mut corner = RgbImage::from_pixel(CORNER.0, CORNER.1, GRAY);
    // First row: 108..270 x 29.7..42.3.
    paint(&mut corner, (108, 30, 270, 32), [220, 20, 30]);
    paint(&mut corner, (108, 40, 270, 42), [220, 20, 30]);
    // Second row: 135..270 x 47.7..60.3.
    paint(&mut corner, (135, 48, 270, 60), [90, 20, 25]);
    corner
}

#[test]
fn detects_whatever_the_model_answers() {
    if !ort() {
        return;
    }
    let mut detector = Detector::load(&model("rows/model.onnx"), 1).unwrap();
    let gray = RgbImage::from_pixel(100, 100, GRAY);
    let all = detector.detect(&gray, 0.0).unwrap();
    assert_eq!(all.len(), 6);
    assert_eq!(all[0].class, 0);
    assert!(close(all[0].score, sigmoid(4.0)));
    assert_eq!(
        (all[0].cx, all[0].cy, all[0].w, all[0].h),
        (0.7, 0.2, 0.6, 0.07)
    );
    assert!(close(all[5].score, sigmoid(-3.0)));
    // The last box is too unsure.
    assert_eq!(detector.detect(&gray, 0.5).unwrap().len(), 5);
    // The stand-in sees nothing in a dark frame: the input reaches the model.
    let black = RgbImage::new(100, 100);
    assert!(detector.detect(&black, 0.5).unwrap().is_empty());
}

#[test]
fn refuses_models_it_cannot_run() {
    if !ort() {
        return;
    }
    let error = Detector::load(&model("dynamic.onnx"), 1).err().unwrap();
    assert_eq!(error.to_string(), "model input has no fixed size");

    let error = Detector::load(&model("missing.onnx"), 1).err().unwrap();
    assert!(
        format!("{error:#}").contains("loading") && format!("{error:#}").contains("missing.onnx"),
        "{error:#}"
    );

    let dir = tempfile::tempdir().unwrap();
    let corrupt = dir.path().join("model.onnx");
    std::fs::write(&corrupt, b"not a model").unwrap();
    let error = Detector::load(&corrupt, 1).err().unwrap();
    assert!(format!("{error:#}").contains("loading"), "{error:#}");
    let error = RowFinder::load(&corrupt, 1).err().unwrap();
    assert!(format!("{error:#}").contains("loading"), "{error:#}");
    // The icon reader needs its classes next to the model.
    let error = IconReader::load(&corrupt, 1).err().unwrap();
    assert!(format!("{error:#}").contains("classes.json"), "{error:#}");
    std::fs::write(dir.path().join("classes.json"), b"{}").unwrap();
    assert!(IconReader::load(&corrupt, 1).is_err(), "not a list");
    std::fs::write(dir.path().join("classes.json"), b"[\"ak47\"]").unwrap();
    let error = IconReader::load(&corrupt, 1).err().unwrap();
    assert!(format!("{error:#}").contains("loading"), "{error:#}");
}

/// Models that load but weren't exported the way the crate reads them fail with an error.
#[test]
fn reports_models_that_do_not_fit() {
    if !ort() {
        return;
    }
    let image = RgbImage::from_pixel(16, 16, GRAY);
    for (misfit, want) in [
        ("input-name", "Invalid input name: input"),
        ("int-dets", "Cannot extract Tensor<f32> from Tensor<i64>"),
        ("int-labels", "Cannot extract Tensor<f32> from Tensor<i64>"),
    ] {
        let mut detector = Detector::load(&model(&format!("misfits/{misfit}.onnx")), 1).unwrap();
        let error = detector.detect(&image, 0.5).unwrap_err().to_string();
        assert_eq!(error, want, "{misfit}");
    }
}

#[test]
fn finds_rows() {
    if !ort() {
        return;
    }
    let mut finder = RowFinder::load(&model("rows/model.onnx"), 1).unwrap();
    // Of the six boxes: two rows. The near-copy, the one ending too far left, the too tall
    // and the too unsure are dropped.
    let rows = finder
        .find(&RgbImage::from_pixel(CORNER.0, CORNER.1, GRAY))
        .unwrap();
    let boxes: Vec<_> = rows
        .iter()
        .map(|r| [r.x0, r.y0, r.x1, r.y1].map(f32::round))
        .collect();
    assert_eq!(
        boxes,
        vec![[108.0, 30.0, 270.0, 42.0], [135.0, 48.0, 270.0, 60.0]]
    );
    assert!(close(rows[0].score, sigmoid(4.0)));
    assert!(close(rows[1].score, sigmoid(3.0)));

    // A frame that is already a killfeed crop: the tall box is a row too.
    let rows = finder.find(&RgbImage::from_pixel(600, 100, GRAY)).unwrap();
    assert_eq!(rows.len(), 3);
    assert_eq!(rows[2].y0.round(), 50.0);

    assert!(
        finder
            .find(&RgbImage::new(CORNER.0, CORNER.1))
            .unwrap()
            .is_empty()
    );
}

fn reading(owner: Owner, weapon: Option<(&str, f32)>, modifiers: &[(&str, f32)]) -> Reading {
    Reading {
        owner,
        weapon: weapon.map(|(n, s)| (n.to_owned(), sigmoid(s))),
        modifiers: modifiers
            .iter()
            .map(|(n, s)| ((*n).to_owned(), sigmoid(*s)))
            .collect(),
    }
}

fn assert_reads(got: &Reading, want: &Reading) {
    assert_eq!(got.owner, want.owner);
    let names = |r: &Reading| {
        (
            r.weapon.as_ref().map(|w| w.0.clone()),
            r.modifiers.iter().map(|m| m.0.clone()).collect::<Vec<_>>(),
        )
    };
    assert_eq!(names(got), names(want));
    let scores = |r: &Reading| {
        r.weapon
            .iter()
            .chain(&r.modifiers)
            .map(|w| w.1)
            .collect::<Vec<_>>()
    };
    for (a, b) in scores(got).into_iter().zip(scores(want)) {
        assert!(close(a, b), "{got:?} != {want:?}");
    }
}

#[test]
fn reads_rows() {
    if !ort() {
        return;
    }
    let mut finder = RowFinder::load(&model("rows/model.onnx"), 1).unwrap();
    let mut reader = IconReader::load(&model("icons/model.onnx"), 1).unwrap();
    let corner = corner_with_my_rows();
    let rows = finder.find(&corner).unwrap();
    let readings = reader.read(&corner, &rows).unwrap();
    assert_eq!(readings.len(), 2);
    // The AK-47 beats the AWP; the headshot and flash assist are sure enough, the
    // wallbang isn't.
    assert_reads(
        &readings[0],
        &reading(
            Owner::MyKill,
            Some(("ak47", 2.0)),
            &[("flash_assist", 3.0), ("headshot", 1.0)],
        ),
    );
    assert_reads(
        &readings[1],
        &reading(Owner::MyDeath, Some(("awp", 0.5)), &[]),
    );
    assert!(reader.read(&corner, &[]).unwrap().is_empty());
}

#[test]
fn reads_more_rows_than_fit_on_a_sheet() {
    if !ort() {
        return;
    }
    let mut reader = IconReader::load(&model("icons/model.onnx"), 1).unwrap();
    let corner = RgbImage::from_pixel(400, 600, GRAY);
    let rows: Vec<Row> = (0..12)
        .map(|i| Row {
            x0: 100.0,
            y0: 40.0 * i as f32,
            x1: 400.0,
            y1: 40.0 * i as f32 + 30.0,
            score: 0.9,
        })
        .collect();
    let readings = reader.read(&corner, &rows).unwrap();
    let weapons: Vec<_> = readings
        .iter()
        .map(|r| r.weapon.as_ref().map(|w| w.0.as_str()))
        .collect();
    // Eleven rows on the first sheet (the stand-in answers for slots 1-3 and 11), the
    // twelfth alone on a second one.
    let mut want = vec![None; 12];
    want[0] = Some("ak47");
    want[1] = Some("awp");
    want[10] = Some("awp");
    want[11] = Some("ak47");
    assert_eq!(weapons, want);
    assert!(readings.iter().all(|r| r.owner == Owner::Other));
}

#[test]
fn analyses_a_clip() {
    if !ort() {
        return;
    }
    let mut finder = RowFinder::load(&model("rows/model.onnx"), 1).unwrap();
    let mut reader = IconReader::load(&model("icons/model.onnx"), 1).unwrap();
    assert_eq!(crop_box(640, 360), (370, 0, CORNER.0, CORNER.1));
    let mut analyzer = Analyzer::new(&mut finder, &mut reader);
    // A dark first second (no rows), then the two rows for three seconds.
    analyzer
        .push(0.0, &RgbImage::new(CORNER.0, CORNER.1))
        .unwrap();
    let corner = corner_with_my_rows();
    for t in 1..4 {
        analyzer.push(f64::from(t), &corner).unwrap();
    }
    assert_eq!(analyzer.frames(), 4);
    let kills = analyzer.finish();
    assert_eq!(kills.len(), 2, "{kills:?}");
    assert_eq!(
        (kills[0].t, kills[0].last_seen, kills[0].sightings),
        (1.0, 3.0, 3)
    );
    assert_eq!(kills[0].owner, Owner::MyKill);
    assert_eq!(kills[0].weapon.as_deref(), Some("ak47"));
    assert_eq!(kills[0].modifiers, vec!["flash_assist", "headshot"]);
    assert_eq!(kills[1].owner, Owner::MyDeath);
    assert_eq!(kills[1].weapon.as_deref(), Some("awp"));

    let summary = summarise(&kills);
    assert_eq!(
        (summary.kills, summary.my_kills, summary.my_deaths),
        (2, 1, 1)
    );
    assert_eq!(
        summary.weapons.into_iter().collect::<Vec<_>>(),
        vec![("ak47".into(), 1)]
    );
    // A flash assist isn't a kill modifier.
    assert_eq!(
        summary.modifiers.into_iter().collect::<Vec<_>>(),
        vec![("headshot".into(), 1)]
    );
    assert_eq!(summary.multi_kill, None);
}
