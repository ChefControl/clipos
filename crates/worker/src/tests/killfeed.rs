//! The `analyse` job with the stand-in killfeed models of `crates/killfeed/tests/models`
//! (see `make_models.py` there for what they answer). Running them needs ONNX Runtime:
//! set `ORT_DYLIB_PATH` to its library, as in production. Without it those tests are
//! skipped, unless `CLIPOS_REQUIRE_ORT` is set (CI), which makes that a failure.

use std::path::PathBuf;

use clipos_core::{analysis, jobs};
use sha2::{Digest, Sha256};

use super::*;

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

/// The stand-in model folders, laid out like published ones.
fn stand_ins() -> PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR")).join("../killfeed/tests/models")
}

const HUD: &str = "hud-locator/test";
const ICONS: &str = "killfeed-icons/test";

/// A worker that analyses with the `hud` and `icons` models, caching them in `cache`. Its
/// image carries no models.
fn analysing(pool: PgPool, cache: &Path, hud: &str, icons: &str) -> Worker {
    let mut worker = worker(pool);
    worker.analyse = Some(AnalyseConfig {
        models_dir: cache.to_owned(),
        baked_dir: cache.join("no-baked-models"),
        hud_model: hud.into(),
        icons_model: icons.into(),
        threads: 1,
    });
    worker
}

/// Uploads `files` (name, contents) to the `models` container as `name`'s files.
async fn publish(worker: &Worker, name: &str, files: &[(&str, Vec<u8>)]) {
    worker.storage.prepare_local(&[]).await.unwrap();
    let dir = tempfile::tempdir().unwrap();
    for (file, contents) in files {
        let path = dir.path().join(file);
        std::fs::write(&path, contents).unwrap();
        worker
            .storage
            .upload_file(
                Container::Models,
                &format!("{name}/{file}"),
                &path,
                "application/octet-stream",
            )
            .await
            .unwrap();
    }
}

/// The files of the stand-in `kind` (`hud` or `icons`).
fn stand_in(kind: &str) -> Vec<(&'static str, Vec<u8>)> {
    ["model.onnx", "classes.json", "model-card.json"]
        .into_iter()
        .map(|file| {
            (
                file,
                std::fs::read(stand_ins().join(kind).join(file)).unwrap(),
            )
        })
        .collect()
}

/// Publishes both stand-ins as [`HUD`] and [`ICONS`].
async fn publish_stand_ins(worker: &Worker) {
    publish(worker, HUD, &stand_in("hud")).await;
    publish(worker, ICONS, &stand_in("icons")).await;
}

/// A model card listing `files` (name, contents) with their SHA-256.
fn card(files: &[(&str, &[u8])]) -> Vec<u8> {
    let files: serde_json::Map<String, Value> = files
        .iter()
        .map(|(name, contents)| {
            let sha256: String = Sha256::digest(contents)
                .iter()
                .map(|b| format!("{b:02x}"))
                .collect();
            ((*name).to_owned(), json!({ "sha256": sha256 }))
        })
        .collect();
    serde_json::to_vec(&json!({ "files": files })).unwrap()
}

/// A 4 s, 640 x 360 clip whose killfeed corner (x 370.., see `crop_box`) has, where the
/// stand-in HUD locator puts its rows, the player's red outline on the first and the red
/// fill of their death on the second. With `dark_start`, the first half second is black.
fn killfeed_clip(path: &Path, dark_start: bool) {
    let mut video = "color=c=0x808080:s=640x360:r=30:d=4,\
        drawbox=x=478:y=30:w=162:h=2:color=0xDC141E:t=fill,\
        drawbox=x=478:y=40:w=162:h=2:color=0xDC141E:t=fill,\
        drawbox=x=504:y=48:w=136:h=12:color=0x5A1419:t=fill"
        .to_owned();
    if dark_start {
        video += ",drawbox=x=0:y=0:w=640:h=360:color=black:t=fill:enable='lt(t,0.5)'";
    }
    ffmpeg(
        &[
            "-f",
            "lavfi",
            "-i",
            &video,
            "-c:v",
            "libx264",
            "-preset",
            "ultrafast",
            "-crf",
            "8",
            "-pix_fmt",
            "yuv420p",
        ],
        path,
    );
}

/// Uploads `file` as a clip and marks it ready without transcoding it, as if it had been.
/// Its transcode job is dropped, so the queue only has what the test adds.
async fn ready_clip(worker: &Worker, file: &Path) -> Uuid {
    let clip = uploaded_clip(worker, file).await;
    make_ready(&worker.pool, clip.id).await;
    clip.id
}

async fn make_ready(pool: &PgPool, clip: Uuid) {
    sqlx::query("UPDATE clips SET status = 'ready', duration_ms = 4000 WHERE id = $1")
        .bind(clip)
        .execute(pool)
        .await
        .unwrap();
    sqlx::query("DELETE FROM jobs").execute(pool).await.unwrap();
}

fn payload(clip: Uuid) -> Value {
    json!({ "clipId": clip })
}

fn retried(result: Result<(), JobError>) -> String {
    match result {
        Err(JobError::Retry(e)) => format!("{e:#}"),
        other => panic!("expected a retry, got {other:?}"),
    }
}

fn permanent(result: Result<(), JobError>) -> String {
    match result {
        Err(JobError::Permanent(message)) => message,
        other => panic!("expected a permanent failure, got {other:?}"),
    }
}

/// The whole job: a CS2 clip is transcoded, which queues its analysis, which downloads
/// the models, checks them against their cards, reads the killfeed and stores it.
#[sqlx::test(migrator = "clipos_core::db::MIGRATOR")]
async fn analyses_a_clip_with_the_stand_in_models(pool: PgPool) {
    if !azurite() || !ort() {
        return;
    }
    let cache = tempfile::tempdir().unwrap();
    let worker = analysing(pool.clone(), cache.path(), HUD, ICONS);
    publish_stand_ins(&worker).await;
    let dir = tempfile::tempdir().unwrap();
    let file = dir.path().join("cs2.mp4");
    killfeed_clip(&file, true);

    let clip = uploaded_clip(&worker, &file).await;
    assert!(worker.run_once().await.unwrap(), "transcode");
    assert!(worker.run_once().await.unwrap(), "analyse was queued");
    let (version, raw, stats) = analysis::get(&pool, clip.id)
        .await
        .unwrap()
        .expect("analysis stored");

    assert_eq!(version, "hud=hud-locator/test icons=killfeed-icons/test");
    assert_eq!(raw["sampleFps"], 1.0);
    // Four frames; in the first, which is dark, the stand-in finds no rows.
    assert_eq!(raw["frames"], 4, "{raw}");
    let kills = raw["kills"].as_array().unwrap();
    assert_eq!(kills.len(), 2, "{raw}");
    assert_eq!(kills[0]["t"], 1.0);
    assert_eq!(kills[0]["sightings"], 3);
    assert_eq!(kills[0]["owner"], "my_kill");
    assert_eq!(kills[0]["weapon"], "ak47");
    assert_eq!(kills[0]["modifiers"], json!(["flash_assist", "headshot"]));
    assert_eq!(kills[1]["owner"], "my_death");
    assert_eq!(kills[1]["weapon"], "awp");
    assert_eq!(
        stats,
        json!({
            "kills": 2,
            "my_kills": 1,
            "my_deaths": 1,
            "multi_kill": null,
            "weapons": { "ak47": 1 },
            "modifiers": { "headshot": 1 },
        })
    );

    // The models are cached and checked; the scratch dir is gone.
    for (model, kind) in [(HUD, "hud"), (ICONS, "icons")] {
        let local = cache.path().join(model);
        assert!(local.join(".verified").exists());
        for file in ["model.onnx", "classes.json"] {
            assert_eq!(
                std::fs::read(local.join(file)).unwrap(),
                std::fs::read(stand_ins().join(kind).join(file)).unwrap()
            );
        }
    }
    let scratch = worker
        .transcode
        .temp_dir
        .join(format!("analyse-{}", clip.id));
    assert!(!scratch.exists());
}

/// Copies the stand-ins into `dir` as an image's baked models, [`HUD`] and [`ICONS`].
fn bake_stand_ins(dir: &Path) {
    for (name, kind) in [(HUD, "hud"), (ICONS, "icons")] {
        let folder = dir.join(name);
        std::fs::create_dir_all(&folder).unwrap();
        for (file, contents) in stand_in(kind) {
            std::fs::write(folder.join(file), contents).unwrap();
        }
    }
}

/// Models the worker image carries are used where they are: nothing is downloaded or
/// copied into the cache, and storage has none to give.
#[sqlx::test(migrator = "clipos_core::db::MIGRATOR")]
async fn uses_the_models_in_the_image(pool: PgPool) {
    if !azurite() || !ort() {
        return;
    }
    let cache = tempfile::tempdir().unwrap();
    let baked = tempfile::tempdir().unwrap();
    bake_stand_ins(baked.path());
    let mut worker = analysing(pool.clone(), cache.path(), HUD, ICONS);
    worker.analyse.as_mut().unwrap().baked_dir = baked.path().to_owned();
    let dir = tempfile::tempdir().unwrap();
    let file = dir.path().join("cs2.mp4");
    killfeed_clip(&file, false);
    let clip = ready_clip(&worker, &file).await;

    analyse::run(&worker, &payload(clip)).await.unwrap();
    let (version, raw, _) = analysis::get(&pool, clip).await.unwrap().unwrap();
    assert_eq!(version, "hud=hud-locator/test icons=killfeed-icons/test");
    assert_eq!(raw["kills"].as_array().unwrap().len(), 2, "{raw}");
    assert!(!cache.path().join(HUD).exists());
    assert!(!cache.path().join(ICONS).exists());
}

/// A baked model that doesn't match its card means a broken image: an error, not a
/// quiet download of another copy.
#[sqlx::test(migrator = "clipos_core::db::MIGRATOR")]
async fn refuses_a_broken_model_in_the_image(pool: PgPool) {
    if !azurite() {
        return;
    }
    let cache = tempfile::tempdir().unwrap();
    let baked = tempfile::tempdir().unwrap();
    bake_stand_ins(baked.path());
    std::fs::write(baked.path().join(HUD).join("model.onnx"), b"something else").unwrap();
    let mut worker = analysing(pool.clone(), cache.path(), HUD, ICONS);
    worker.analyse.as_mut().unwrap().baked_dir = baked.path().to_owned();
    publish_stand_ins(&worker).await;
    let clip = processing_clip(&pool).await;
    make_ready(&pool, clip).await;

    let error = retried(analyse::run(&worker, &payload(clip)).await);
    assert!(
        error.contains("the image's hud-locator/test/model.onnx doesn't match its model card"),
        "{error}"
    );
    assert!(!cache.path().join(HUD).exists(), "nothing downloaded");
    assert!(analysis::get(&pool, clip).await.unwrap().is_none());
}

/// Models already in the cache are used as they are, without asking storage. A clip
/// without a killfeed has no kills.
#[sqlx::test(migrator = "clipos_core::db::MIGRATOR")]
async fn uses_cached_models(pool: PgPool) {
    if !azurite() || !ort() {
        return;
    }
    let cache = tempfile::tempdir().unwrap();
    // Names nothing was published as: only the cache has them.
    let (hud, icons) = (
        format!("hud-locator/cached-{}", Uuid::new_v4()),
        format!("killfeed-icons/cached-{}", Uuid::new_v4()),
    );
    for (name, kind) in [(&hud, "hud"), (&icons, "icons")] {
        let local = cache.path().join(name);
        std::fs::create_dir_all(&local).unwrap();
        for (file, contents) in stand_in(kind) {
            std::fs::write(local.join(file), contents).unwrap();
        }
        std::fs::write(local.join(".verified"), b"").unwrap();
    }
    let worker = analysing(pool.clone(), cache.path(), &hud, &icons);
    let dir = tempfile::tempdir().unwrap();
    let file = dir.path().join("dark.mp4");
    ffmpeg(
        &[
            "-f",
            "lavfi",
            "-i",
            "color=c=black:s=640x360:r=30:d=3",
            "-c:v",
            "libx264",
            "-preset",
            "ultrafast",
            "-pix_fmt",
            "yuv420p",
        ],
        &file,
    );
    let clip = ready_clip(&worker, &file).await;

    analyse::run(&worker, &payload(clip)).await.unwrap();
    let (version, raw, stats) = analysis::get(&pool, clip).await.unwrap().unwrap();
    assert_eq!(version, format!("hud={hud} icons={icons}"));
    assert_eq!(raw["frames"], 3);
    assert_eq!(raw["kills"], json!([]));
    assert_eq!(
        (&stats["kills"], &stats["my_kills"]),
        (&json!(0), &json!(0))
    );
}

/// Jobs that have nothing to do, or can never do it.
#[sqlx::test(migrator = "clipos_core::db::MIGRATOR")]
async fn skips_what_it_cannot_analyse(pool: PgPool) {
    let cache = tempfile::tempdir().unwrap();
    let missing = "hud-locator/never-published";
    let worker = analysing(pool.clone(), cache.path(), missing, missing);

    // Bad payloads fail for good.
    let message = permanent(analyse::run(&worker, &json!({ "clip": 1 })).await);
    assert!(message.starts_with("bad payload"), "{message}");
    // A deleted clip, or one that isn't ready, has nothing to analyse; the models aren't
    // even fetched.
    analyse::run(&worker, &payload(Uuid::new_v4()))
        .await
        .unwrap();
    let processing = processing_clip(&pool).await;
    analyse::run(&worker, &payload(processing)).await.unwrap();
    assert!(analysis::get(&pool, processing).await.unwrap().is_none());
    assert_eq!(std::fs::read_dir(cache.path()).unwrap().count(), 0);

    // A worker with the analysis turned off fails its analyse jobs for good.
    let off = super::worker(pool.clone());
    let message = permanent(analyse::run(&off, &payload(processing)).await);
    assert_eq!(message, "killfeed analysis is turned off on this worker");
}

/// Models that aren't published (yet) are a failure to retry, not the clip's fault.
#[sqlx::test(migrator = "clipos_core::db::MIGRATOR")]
async fn retries_while_the_models_are_missing(pool: PgPool) {
    if !azurite() {
        return;
    }
    let cache = tempfile::tempdir().unwrap();
    let missing = format!("hud-locator/missing-{}", Uuid::new_v4());
    let worker = analysing(pool.clone(), cache.path(), &missing, ICONS);
    worker.storage.prepare_local(&[]).await.unwrap();
    let clip = processing_clip(&pool).await;
    make_ready(&pool, clip).await;

    let job = jobs::enqueue(&pool, clips::ANALYSE_JOB, payload(clip))
        .await
        .unwrap();
    assert!(worker.run_once().await.unwrap());
    let (status, error) = job_row(&pool, job).await;
    assert_eq!(status, "queued", "retried later");
    let error = error.unwrap();
    assert!(
        error.contains(&format!("downloading the model card of {missing}"))
            && error.contains("404"),
        "{error}"
    );
    assert!(analysis::get(&pool, clip).await.unwrap().is_none());
    assert!(!cache.path().join(&missing).join(".verified").exists());
}

/// A model is only used once its files match its card.
#[sqlx::test(migrator = "clipos_core::db::MIGRATOR")]
async fn checks_models_against_their_cards(pool: PgPool) {
    if !azurite() {
        return;
    }
    let cache = tempfile::tempdir().unwrap();
    let clip = processing_clip(&pool).await;
    make_ready(&pool, clip).await;
    let hud = stand_in("hud");
    let (model, classes) = (hud[0].1.clone(), hud[1].1.clone());

    // A card that leaves out a file.
    let name = format!("hud-locator/partial-{}", Uuid::new_v4());
    let worker = analysing(pool.clone(), cache.path(), &name, ICONS);
    publish(
        &worker,
        &name,
        &[
            ("model.onnx", model.clone()),
            ("classes.json", classes.clone()),
            ("model-card.json", card(&[("model.onnx", &model)])),
        ],
    )
    .await;
    let error = retried(analyse::run(&worker, &payload(clip)).await);
    assert!(
        error.contains("model card lists no classes.json"),
        "{error}"
    );

    // A model that isn't the one its card describes (damaged, or replaced).
    let name = format!("hud-locator/tampered-{}", Uuid::new_v4());
    let worker = analysing(pool.clone(), cache.path(), &name, ICONS);
    publish(
        &worker,
        &name,
        &[
            ("model.onnx", b"something else".to_vec()),
            ("classes.json", classes.clone()),
            (
                "model-card.json",
                card(&[("model.onnx", &model), ("classes.json", &classes)]),
            ),
        ],
    )
    .await;
    let error = retried(analyse::run(&worker, &payload(clip)).await);
    assert!(
        error.contains(&format!("{name}/model.onnx doesn't match its model card")),
        "{error}"
    );
    let local = cache.path().join(&name);
    assert!(!local.join("model.onnx.partial").exists());
    assert!(!local.join("model.onnx").exists());
    assert!(!local.join(".verified").exists());

    // A card that isn't one.
    let name = format!("hud-locator/no-card-{}", Uuid::new_v4());
    let worker = analysing(pool.clone(), cache.path(), &name, ICONS);
    publish(&worker, &name, &[("model-card.json", b"<html>".to_vec())]).await;
    let error = retried(analyse::run(&worker, &payload(clip)).await);
    assert!(
        error.contains(&format!("reading the model card of {name}")),
        "{error}"
    );
    assert!(analysis::get(&pool, clip).await.unwrap().is_none());
}

/// A model file that matches its card but isn't a model (published broken): retried, so
/// it runs once a fixed version is out, and the scratch dir is cleaned up.
#[sqlx::test(migrator = "clipos_core::db::MIGRATOR")]
async fn retries_when_a_model_cannot_be_loaded(pool: PgPool) {
    if !azurite() || !ort() {
        return;
    }
    let cache = tempfile::tempdir().unwrap();
    let name = format!("killfeed-icons/broken-{}", Uuid::new_v4());
    let worker = analysing(pool.clone(), cache.path(), HUD, &name);
    publish_stand_ins(&worker).await;
    let broken = b"not an onnx model".to_vec();
    let classes = b"[\"ak47\"]".to_vec();
    publish(
        &worker,
        &name,
        &[
            ("model.onnx", broken.clone()),
            ("classes.json", classes.clone()),
            (
                "model-card.json",
                card(&[("model.onnx", &broken), ("classes.json", &classes)]),
            ),
        ],
    )
    .await;
    let dir = tempfile::tempdir().unwrap();
    let file = dir.path().join("cs2.mp4");
    killfeed_clip(&file, false);
    let clip = ready_clip(&worker, &file).await;

    let error = retried(analyse::run(&worker, &payload(clip)).await);
    assert!(
        error.contains("loading") && error.contains(&name),
        "{error}"
    );
    assert!(analysis::get(&pool, clip).await.unwrap().is_none());
    let scratch = worker.transcode.temp_dir.join(format!("analyse-{clip}"));
    assert!(!scratch.exists());
}

/// A clip whose original has no video, or no picture size, can never be analysed.
#[sqlx::test(migrator = "clipos_core::db::MIGRATOR")]
async fn originals_without_a_usable_video_fail_for_good(pool: PgPool) {
    if !azurite() {
        return;
    }
    let cache = tempfile::tempdir().unwrap();
    let worker = analysing(pool.clone(), cache.path(), HUD, ICONS);
    publish_stand_ins(&worker).await;
    let dir = tempfile::tempdir().unwrap();

    let voice = dir.path().join("voice.m4a");
    ffmpeg(&["-f", "lavfi", "-i", "sine=d=2", "-c:a", "aac"], &voice);
    let clip = ready_clip(&worker, &voice).await;
    let message = permanent(analyse::run(&worker, &payload(clip)).await);
    assert_eq!(message, "the file has no video stream");
    assert!(analysis::get(&pool, clip).await.unwrap().is_none());

    // An MP4 that names an H.264 stream but gives its size nowhere: not in its sample
    // entry, not in its avcC (blanked, so no SPS), not in its frames (blank too). ffprobe
    // knows there is a video, but not its size.
    let blank = dir.path().join("blank.mp4");
    ffmpeg(
        &[
            "-f",
            "lavfi",
            "-i",
            "testsrc2=s=320x180:r=10:d=2",
            "-c:v",
            "libx264",
            "-pix_fmt",
            "yuv420p",
            // The index first, so blanking everything after `mdat` leaves it.
            "-movflags",
            "+faststart",
        ],
        &blank,
    );
    let mut bytes = std::fs::read(&blank).unwrap();
    let find = |bytes: &[u8], what: &[u8], from: usize| {
        from + bytes[from..].windows(4).position(|w| w == what).unwrap()
    };
    // Width and height come after the fourcc, 6 reserved bytes, the data reference index
    // and 16 bytes more. (The first "avc1" is a brand in `ftyp`.)
    let entry = find(&bytes, b"avc1", find(&bytes, b"stsd", 0));
    bytes[entry + 28..entry + 32].fill(0);
    let avcc = find(&bytes, b"avcC", entry);
    let size = u32::from_be_bytes(bytes[avcc - 4..avcc].try_into().unwrap()) as usize;
    bytes[avcc + 4..avcc - 4 + size].fill(0);
    let mdat = find(&bytes, b"mdat", 0);
    bytes[mdat + 4..].fill(0xff);
    std::fs::write(&blank, bytes).unwrap();
    let clip = ready_clip(&worker, &blank).await;
    let message = permanent(analyse::run(&worker, &payload(clip)).await);
    assert_eq!(message, "the video has no size");
}

/// A clip none of whose frames decode: ffprobe reads its header, but ffmpeg gives no
/// frames at all and fails, which is retried like any other ffmpeg failure. After
/// `MAX_ATTEMPTS` the job fails for good, and the clip stays ready without an analysis
/// (decided in the quality checkpoint, C-07).
#[sqlx::test(migrator = "clipos_core::db::MIGRATOR")]
async fn a_clip_without_a_frame_to_read_is_retried_then_given_up_on(pool: PgPool) {
    if !azurite() || !ort() {
        return;
    }
    let cache = tempfile::tempdir().unwrap();
    let worker = analysing(pool.clone(), cache.path(), HUD, ICONS);
    publish_stand_ins(&worker).await;
    let dir = tempfile::tempdir().unwrap();
    let file = dir.path().join("blank.mp4");
    ffmpeg(
        &[
            "-f",
            "lavfi",
            "-i",
            "testsrc2=s=320x180:r=10:d=2",
            "-c:v",
            "libx264",
            "-pix_fmt",
            "yuv420p",
            "-movflags",
            "+faststart",
        ],
        &file,
    );
    // Zero every frame, keeping the header.
    let mut bytes = std::fs::read(&file).unwrap();
    let mdat = bytes.windows(4).position(|w| w == b"mdat").unwrap() + 4;
    bytes[mdat..].fill(0);
    std::fs::write(&file, bytes).unwrap();
    let clip = ready_clip(&worker, &file).await;

    let error = retried(analyse::run(&worker, &payload(clip)).await);
    assert!(
        error.contains("ffmpeg failed while sampling frames"),
        "{error}"
    );
    assert!(analysis::get(&pool, clip).await.unwrap().is_none());

    // Through the queue: each run fails and is retried after its backoff (skipped here),
    // until the job runs out of attempts.
    let job = jobs::enqueue(&pool, clips::ANALYSE_JOB, payload(clip))
        .await
        .unwrap();
    let mut runs = 0;
    loop {
        sqlx::query("UPDATE jobs SET run_after = now() WHERE id = $1")
            .bind(job)
            .execute(&pool)
            .await
            .unwrap();
        if !worker.run_once().await.unwrap() {
            break;
        }
        runs += 1;
    }
    assert_eq!(runs, jobs::MAX_ATTEMPTS);
    let (status, attempts): (String, i32) =
        sqlx::query_as("SELECT status, attempts FROM jobs WHERE id = $1")
            .bind(job)
            .fetch_one(&pool)
            .await
            .unwrap();
    assert_eq!((status.as_str(), attempts), ("failed", jobs::MAX_ATTEMPTS));
    let ready = clips::get(&pool, clip).await.unwrap().unwrap();
    assert_eq!(ready.status, ClipStatus::Ready);
    assert_eq!(ready.error, None);
    assert!(!analysis::pending(&pool, clip).await.unwrap());
    assert!(analysis::get(&pool, clip).await.unwrap().is_none());
}

/// ffmpeg's own failures are reported with its reason; a frame that can't be read
/// (a model failing) stops the sampling with that error.
#[test]
fn sampling_reports_failures() {
    let dir = tempfile::tempdir().unwrap();
    let file = dir.path().join("clip.mp4");
    ffmpeg(
        &[
            "-f",
            "lavfi",
            "-i",
            "testsrc2=size=320x180:rate=10",
            "-t",
            "5",
            "-c:v",
            "libx264",
            "-preset",
            "ultrafast",
        ],
        &file,
    );
    let input = |stream| analyse::Input {
        source: file.to_str().unwrap().into(),
        stream,
        width: 320,
        height: 180,
    };
    let deadline = || std::time::Instant::now() + Duration::from_secs(30);

    // There is no stream 3.
    let error = analyse::sample_frames(&input(3), deadline(), |_, _| Ok(()))
        .unwrap_err()
        .to_string();
    assert!(
        error.starts_with("ffmpeg failed while sampling frames"),
        "{error}"
    );
    assert!(error.contains("matches no streams"), "{error}");

    let mut seen = Vec::new();
    let error = analyse::sample_frames(&input(0), deadline(), |t, frame| {
        seen.push(t);
        assert_eq!(frame.dimensions(), (320, 180));
        if t >= 2.0 {
            anyhow::bail!("the model failed");
        }
        Ok(())
    })
    .unwrap_err();
    assert_eq!(error.to_string(), "the model failed");
    assert_eq!(seen, vec![0.0, 1.0, 2.0]);
}

/// Whole frames come out as decoded, at the stream's size, pixel for pixel.
#[test]
fn samples_whole_frames() {
    let dir = tempfile::tempdir().unwrap();
    let file = dir.path().join("stripe.mp4");
    // Black, with a white stripe down the corner's first 10 columns.
    ffmpeg(
        &[
            "-f",
            "lavfi",
            "-i",
            "color=c=black:s=320x180:r=10:d=5,drawbox=x=185:y=0:w=10:h=180:color=white:t=fill",
            "-c:v",
            "libx264",
            "-preset",
            "ultrafast",
            "-pix_fmt",
            "yuv420p",
        ],
        &file,
    );
    let input = analyse::Input {
        source: file.to_str().unwrap().into(),
        stream: 0,
        width: 320,
        height: 180,
    };
    let deadline = std::time::Instant::now() + Duration::from_secs(30);
    let mut times = Vec::new();
    let frames = analyse::sample_frames(&input, deadline, |t, frame| {
        times.push(t);
        assert_eq!(frame.dimensions(), (320, 180));
        for y in [0, 90, 179] {
            assert!(frame.get_pixel(189, y)[0] > 200, "stripe at row {y} of {t}");
            assert!(frame.get_pixel(215, y)[0] < 50, "black at row {y} of {t}");
        }
        Ok(())
    })
    .unwrap();
    assert_eq!(frames, 5);
    assert_eq!(times, vec![0.0, 1.0, 2.0, 3.0, 4.0]);
}
