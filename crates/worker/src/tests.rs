use std::{path::Path, process::Command};

use clipos_core::{
    clips::{self, ClipStatus, NewClip},
    storage::StorageConfig,
    users::{self, Identity, SignIn},
};
use serde_json::json;

use super::*;

mod killfeed;

/// Azurite's well-known development account (public, not a secret).
const AZURITE_KEY: &str =
    "Eby8vdM02xNOcqFlqUwJPLlmEtlCDXJ1OUzFT50uSRZ6IFsuFq2UVErCz4I6tq/K1SZFPTOtr/KBHBeksoGMGw==";

fn worker(pool: PgPool) -> Worker {
    Worker {
        pool,
        storage: Arc::new(
            Storage::new(StorageConfig {
                account: "devstoreaccount1".into(),
                blob_endpoint: Some("http://127.0.0.1:10000/devstoreaccount1".into()),
                account_key: Some(AZURITE_KEY.into()),
            })
            .unwrap(),
        ),
        transcode: TranscodeConfig {
            temp_dir: std::env::temp_dir().join("clipos-worker-tests"),
            threads: 2,
            preset: "ultrafast".into(),
            crf: 28,
        },
        analyse: None,
        id: "test-worker".into(),
        // Often, so even a short job's lock is refreshed.
        heartbeat: Duration::from_millis(50),
    }
}

/// End-to-end tests need Azurite and ffmpeg: set `CLIPOS_AZURITE` (CI and `make test`).
fn azurite() -> bool {
    let on = std::env::var_os("CLIPOS_AZURITE").is_some();
    if !on {
        eprintln!("CLIPOS_AZURITE not set; skipping");
    }
    on
}

async fn job_row(pool: &PgPool, id: Uuid) -> (String, Option<String>) {
    sqlx::query_as("SELECT status, last_error FROM jobs WHERE id = $1")
        .bind(id)
        .fetch_one(pool)
        .await
        .unwrap()
}

async fn owner(pool: &PgPool) -> Uuid {
    sqlx::query("INSERT INTO invites (email) VALUES ('sam@gmail.com') ON CONFLICT DO NOTHING")
        .execute(pool)
        .await
        .unwrap();
    let identity = Identity {
        sub: "google-oauth2|sam",
        email: "sam@gmail.com",
        name: Some("Sam"),
        picture: None,
    };
    match users::sign_in(pool, &identity).await.unwrap() {
        SignIn::Allowed(u) => u.id,
        other => panic!("{other:?}"),
    }
}

/// Uploads `file` as a new clip's original and, like `complete`, keeps its ETag and marks
/// it uploaded (queues the transcode).
async fn uploaded_clip(worker: &Worker, file: &Path) -> clips::Clip {
    worker.storage.prepare_local(&[]).await.unwrap();
    let owner_id = owner(&worker.pool).await;
    let clip = clips::create(
        &worker.pool,
        NewClip {
            owner_id,
            game_id: "cs2".into(),
            title: "Test clip".into(),
            description: String::new(),
            map: None,
            my_pov: true,
            filename: "test.mp4".into(),
            bytes: std::fs::metadata(file).unwrap().len() as i64,
        },
    )
    .await
    .unwrap();
    worker
        .storage
        .upload_file(Container::Originals, &clip.original_blob, file, "video/mp4")
        .await
        .unwrap();
    let props = worker
        .storage
        .blob_props(Container::Originals, &clip.original_blob)
        .await
        .unwrap()
        .unwrap();
    clips::set_original_etag(&worker.pool, clip.id, &props.etag)
        .await
        .unwrap();
    assert!(clips::mark_uploaded(&worker.pool, clip.id).await.unwrap());
    clips::get(&worker.pool, clip.id).await.unwrap().unwrap()
}

#[sqlx::test(migrator = "clipos_core::db::MIGRATOR")]
async fn idle_queue_reports_no_work(pool: PgPool) {
    assert!(!worker(pool).run_once().await.unwrap());
}

#[sqlx::test(migrator = "clipos_core::db::MIGRATOR")]
async fn runs_noop_job(pool: PgPool) {
    let id = jobs::enqueue(&pool, "noop", json!({})).await.unwrap();
    assert!(worker(pool.clone()).run_once().await.unwrap());
    assert_eq!(job_row(&pool, id).await, ("succeeded".into(), None));
}

#[sqlx::test(migrator = "clipos_core::db::MIGRATOR")]
async fn unknown_kind_is_failed_for_retry(pool: PgPool) {
    let id = jobs::enqueue(&pool, "mystery", json!({})).await.unwrap();
    assert!(worker(pool.clone()).run_once().await.unwrap());
    let (status, error) = job_row(&pool, id).await;
    assert_eq!(status, "queued");
    assert!(error.unwrap().contains("mystery"));
}

/// A 4:3, 144 fps HEVC clip with audio (like a stretched-res ShadowPlay recording) comes
/// out as 4:3 H.264 at 60 fps, with a poster.
#[sqlx::test(migrator = "clipos_core::db::MIGRATOR")]
async fn transcodes_a_clip(pool: PgPool) {
    if !azurite() {
        return;
    }
    let dir = tempfile::tempdir().unwrap();
    let source = dir.path().join("source.mp4");
    let status = Command::new("ffmpeg")
        .args(["-hide_banner", "-loglevel", "error", "-y"])
        .args(["-f", "lavfi", "-i", "testsrc2=size=1280x960:rate=144"])
        .args(["-f", "lavfi", "-i", "sine=frequency=440"])
        .args([
            "-t",
            "2",
            "-c:v",
            "libx265",
            "-preset",
            "ultrafast",
            "-c:a",
            "aac",
        ])
        .arg(&source)
        .status()
        .expect("ffmpeg is installed");
    assert!(status.success());

    let worker = worker(pool);
    let clip = uploaded_clip(&worker, &source).await;
    assert!(worker.run_once().await.unwrap());

    let done = clips::get(&worker.pool, clip.id).await.unwrap().unwrap();
    assert_eq!(done.status, ClipStatus::Ready, "{:?}", done.error);
    assert_eq!(
        (done.width, done.height),
        (Some(1280), Some(960)),
        "4:3 kept"
    );
    assert!((done.fps.unwrap() - 60.0).abs() < 0.5, "fps {:?}", done.fps);
    assert!((1900..=2100).contains(&done.duration_ms.unwrap()));
    let playback = done.playback_blob.unwrap();
    assert!(
        worker
            .storage
            .blob_size(Container::Playback, &playback)
            .await
            .unwrap()
            .unwrap()
            > 10_000
    );
    let poster = done.poster_blob.unwrap();
    assert!(
        worker
            .storage
            .blob_size(Container::Posters, &poster)
            .await
            .unwrap()
            .unwrap()
            > 1_000
    );
    // The blurred teaser for clips saved for the show.
    let teaser = worker
        .storage
        .blob_size(Container::Posters, &clips::teaser_blob(clip.id))
        .await
        .unwrap()
        .unwrap();
    assert!(teaser > 500, "{teaser}");
}

async fn transcode_mode(pool: &PgPool, id: Uuid) -> String {
    sqlx::query_scalar("SELECT metadata->'transcode'->>'mode' FROM clips WHERE id = $1")
        .bind(id)
        .fetch_one(pool)
        .await
        .unwrap()
}

fn make_h264(path: &Path, extra: &[&str]) {
    let status = Command::new("ffmpeg")
        .args(["-hide_banner", "-loglevel", "error", "-y"])
        .args(["-f", "lavfi", "-i", "testsrc2=size=1920x1080:rate=60"])
        .args(["-f", "lavfi", "-i", "sine=frequency=440"])
        .args([
            "-t",
            "2",
            "-c:v",
            "libx264",
            "-preset",
            "ultrafast",
            "-pix_fmt",
            "yuv420p",
        ])
        .args(extra)
        .args(["-c:a", "aac"])
        .arg(path)
        .status()
        .expect("ffmpeg is installed");
    assert!(status.success());
}

/// A browser-ready 1080p60 H.264/AAC file is only repackaged: same picture, seconds.
#[sqlx::test(migrator = "clipos_core::db::MIGRATOR")]
async fn remuxes_browser_ready_h264(pool: PgPool) {
    if !azurite() {
        return;
    }
    let dir = tempfile::tempdir().unwrap();
    let source = dir.path().join("shadowplay.mkv");
    make_h264(&source, &["-b:v", "8M"]);

    let worker = worker(pool);
    let clip = uploaded_clip(&worker, &source).await;
    assert!(worker.run_once().await.unwrap());

    let done = clips::get(&worker.pool, clip.id).await.unwrap().unwrap();
    assert_eq!(done.status, ClipStatus::Ready, "{:?}", done.error);
    assert_eq!(transcode_mode(&worker.pool, clip.id).await, "remux");
    assert_eq!((done.width, done.height), (Some(1920), Some(1080)));
    assert!((done.fps.unwrap() - 60.0).abs() < 0.5);
}

/// Browser-ready but too heavy for phones: re-encoded.
#[sqlx::test(migrator = "clipos_core::db::MIGRATOR")]
async fn transcodes_high_bitrate_h264(pool: PgPool) {
    if !azurite() {
        return;
    }
    let dir = tempfile::tempdir().unwrap();
    let source = dir.path().join("heavy.mp4");
    make_h264(
        &source,
        &[
            "-b:v", "40M", "-minrate", "40M", "-maxrate", "40M", "-bufsize", "20M",
        ],
    );

    let worker = worker(pool);
    let clip = uploaded_clip(&worker, &source).await;
    assert!(worker.run_once().await.unwrap());
    assert_eq!(transcode_mode(&worker.pool, clip.id).await, "transcode");
}

#[sqlx::test(migrator = "clipos_core::db::MIGRATOR")]
async fn unreadable_upload_fails_the_clip_without_retrying(pool: PgPool) {
    if !azurite() {
        return;
    }
    let dir = tempfile::tempdir().unwrap();
    let junk = dir.path().join("junk.mp4");
    std::fs::write(&junk, vec![0x42u8; 50_000]).unwrap();

    let worker = worker(pool);
    let clip = uploaded_clip(&worker, &junk).await;
    assert!(worker.run_once().await.unwrap());

    let failed = clips::get(&worker.pool, clip.id).await.unwrap().unwrap();
    assert_eq!(failed.status, ClipStatus::Failed);
    assert!(failed.error.unwrap().contains("isn't a video"));
    assert!(!worker.run_once().await.unwrap(), "not retried");
}

#[sqlx::test(migrator = "clipos_core::db::MIGRATOR")]
async fn janitor_removes_abandoned_uploads(pool: PgPool) {
    if !azurite() {
        return;
    }
    let dir = tempfile::tempdir().unwrap();
    let partial = dir.path().join("partial.mp4");
    std::fs::write(&partial, b"half an upload").unwrap();

    let worker = worker(pool);
    worker.storage.prepare_local(&[]).await.unwrap();
    let owner_id = owner(&worker.pool).await;
    let clip = clips::create(
        &worker.pool,
        NewClip {
            owner_id,
            game_id: "cs2".into(),
            title: "Never finished".into(),
            description: String::new(),
            map: None,
            my_pov: true,
            filename: "big.mp4".into(),
            bytes: 1_000_000,
        },
    )
    .await
    .unwrap();
    worker
        .storage
        .upload_file(
            Container::Originals,
            &clip.original_blob,
            &partial,
            "video/mp4",
        )
        .await
        .unwrap();

    assert_eq!(
        worker
            .clean_abandoned_uploads(Duration::from_secs(3600))
            .await
            .unwrap(),
        0
    );
    sqlx::query("UPDATE clips SET created_at = now() - interval '2 days'")
        .execute(&worker.pool)
        .await
        .unwrap();
    assert_eq!(
        worker
            .clean_abandoned_uploads(Duration::from_secs(3600))
            .await
            .unwrap(),
        1
    );
    assert!(clips::get(&worker.pool, clip.id).await.unwrap().is_none());
    assert_eq!(
        worker
            .storage
            .blob_size(Container::Originals, &clip.original_blob)
            .await
            .unwrap(),
        None
    );
}

#[sqlx::test(migrator = "clipos_core::db::MIGRATOR")]
async fn purges_clips_after_a_week_in_the_trash(pool: PgPool) {
    if !azurite() {
        return;
    }
    let dir = tempfile::tempdir().unwrap();
    let file = dir.path().join("c.mp4");
    std::fs::write(&file, b"original").unwrap();
    let worker = worker(pool);
    let clip = uploaded_clip(&worker, &file).await;
    for (container, blob) in [
        (Container::Playback, clips::playback_blob(clip.id)),
        (Container::Posters, clips::poster_blob(clip.id)),
    ] {
        worker
            .storage
            .upload_file(container, &blob, &file, "application/octet-stream")
            .await
            .unwrap();
    }
    clips::set_ready(
        &worker.pool,
        clip.id,
        &clips::Transcoded {
            playback_blob: clips::playback_blob(clip.id),
            poster_blob: clips::poster_blob(clip.id),
            duration_ms: 1000,
            width: 1920,
            height: 1080,
            fps: 60.0,
            metadata: json!({}),
        },
    )
    .await
    .unwrap();
    clips::soft_delete(&worker.pool, clip.id).await.unwrap();

    assert_eq!(worker.purge_trash().await.unwrap(), 0, "still restorable");
    sqlx::query("UPDATE clips SET deleted_at = now() - interval '8 days'")
        .execute(&worker.pool)
        .await
        .unwrap();
    assert_eq!(worker.purge_trash().await.unwrap(), 1);
    assert!(
        clips::get_including_deleted(&worker.pool, clip.id)
            .await
            .unwrap()
            .is_none()
    );
    for (container, blob) in [
        (Container::Originals, clip.original_blob.clone()),
        (Container::Playback, clips::playback_blob(clip.id)),
        (Container::Posters, clips::poster_blob(clip.id)),
    ] {
        assert_eq!(
            worker.storage.blob_size(container, &blob).await.unwrap(),
            None
        );
    }
}

/// The real killfeed analysis, end to end: a CS2 clip is transcoded, which queues the
/// `analyse` job, which downloads both models from the `models` container, checks them
/// against their model cards and stores what it read. Needs the published model folders
/// and a labelled clip, so it only runs when pointed at them:
///
///     CLIPOS_AZURITE=1 ORT_DYLIB_PATH=.../libonnxruntime.dylib \
///     CLIPOS_KILLFEED_MODELS=<dir with killfeed-rows/v1 and killfeed-icons/v1> \
///     CLIPOS_KILLFEED_CLIP=<a labelled CS2 clip>.mp4 \
///     cargo test -p clipos-worker analyses_a_cs2_clip
///
/// The checks below expect a clip on Mirage with a Tec-9 kill at 0:21, then the recording
/// player dying to a USP-S headshot at 0:24; three killfeed rows in all.
#[sqlx::test(migrator = "clipos_core::db::MIGRATOR")]
async fn analyses_a_cs2_clip(pool: PgPool) {
    let (Some(models), Some(clip_file)) = (
        std::env::var_os("CLIPOS_KILLFEED_MODELS"),
        std::env::var_os("CLIPOS_KILLFEED_CLIP"),
    ) else {
        eprintln!("CLIPOS_KILLFEED_MODELS / CLIPOS_KILLFEED_CLIP not set; skipping");
        return;
    };
    if !azurite() {
        return;
    }
    let cache = tempfile::tempdir().unwrap();
    let mut worker = worker(pool.clone());
    worker.analyse = Some(AnalyseConfig {
        models_dir: cache.path().to_owned(),
        rows_model: "killfeed-rows/v1".into(),
        icons_model: "killfeed-icons/v1".into(),
        threads: 4,
    });
    worker.storage.prepare_local(&[]).await.unwrap();
    for model in ["killfeed-rows/v1", "killfeed-icons/v1"] {
        for file in ["model.onnx", "classes.json", "model-card.json"] {
            worker
                .storage
                .upload_file(
                    Container::Models,
                    &format!("{model}/{file}"),
                    &Path::new(&models).join(model).join(file),
                    "application/octet-stream",
                )
                .await
                .unwrap();
        }
    }

    let clip = uploaded_clip(&worker, Path::new(&clip_file)).await;
    assert!(worker.run_once().await.unwrap(), "transcode");
    assert!(worker.run_once().await.unwrap(), "analyse was queued");
    let (version, raw, stats) = clipos_core::analysis::get(&pool, clip.id)
        .await
        .unwrap()
        .expect("analysis stored");

    assert_eq!(version, "rows=killfeed-rows/v1 icons=killfeed-icons/v1");
    assert_eq!(stats["kills"], 3, "{raw}");
    assert_eq!(stats["my_kills"], 1, "{raw}");
    assert_eq!(stats["my_deaths"], 1, "{raw}");
    assert_eq!(stats["weapons"], json!({ "tec9": 1 }), "{raw}");
    assert!(cache.path().join("killfeed-icons/v1/.verified").exists());
}

#[test]
fn reads_the_longest_keyframe_gap() {
    let keyframe_gap = |csv| crate::transcode::read_packets(csv).keyframe_gap;
    // Keyframes at 0, 2 and 9 s, packets to 12 s: the gap is 9 − 2 = 7 s.
    let csv = "0.000,1,K__\n1.000,1,___\n2.000,1,K__\n5.000,1,___\n9.000,1,K__\n12.000,1,___\n";
    assert_eq!(keyframe_gap(csv), Some(7.0));
    // After the last keyframe counts too.
    assert_eq!(keyframe_gap("0.0,0.5,K_\n6.5,0.5,__\n"), Some(6.5));
    assert_eq!(keyframe_gap("1.0,0.5,__\n"), None);
    // A file that starts at 10 s: measured from its first packet, not from 0.
    assert_eq!(keyframe_gap("10.0,1,K_\n11.0,1,K_\n12.0,1,__\n"), Some(1.0));
    assert_eq!(keyframe_gap("10.0,1,__\n13.0,1,K_\n14.0,1,__\n"), Some(3.0));
}

/// The playback file's longest keyframe gap, read through a SAS like the job does.
async fn playback_gap(worker: &Worker, clip: Uuid) -> f64 {
    let done = clips::get(&worker.pool, clip).await.unwrap().unwrap();
    let url = worker
        .storage
        .sas_url(
            Container::Playback,
            &done.playback_blob.unwrap(),
            clipos_core::storage::Access::Read,
            std::time::Duration::from_secs(600),
            &clipos_core::storage::Overrides::default(),
        )
        .await
        .unwrap();
    crate::transcode::max_keyframe_gap(url.as_str())
        .await
        .unwrap()
        .unwrap()
}

/// A browser-ready MKV that starts at 10 s (cut from a longer recording) with a keyframe
/// every second: copied, not mistaken for one with a 10 s gap.
#[sqlx::test(migrator = "clipos_core::db::MIGRATOR")]
async fn remuxes_a_file_that_starts_late(pool: PgPool) {
    if !azurite() {
        return;
    }
    let dir = tempfile::tempdir().unwrap();
    let source = dir.path().join("cut.mkv");
    make_h264(
        &source,
        &["-g", "60", "-b:v", "8M", "-output_ts_offset", "10"],
    );
    let gap = crate::transcode::max_keyframe_gap(source.to_str().unwrap())
        .await
        .unwrap()
        .unwrap();
    assert!(gap <= 1.1, "gap {gap}");

    let worker = worker(pool);
    let clip = uploaded_clip(&worker, &source).await;
    assert!(worker.run_once().await.unwrap());
    assert_eq!(transcode_mode(&worker.pool, clip.id).await, "remux");
}

async fn queue_keyframes(worker: &Worker, clip: Uuid) -> Uuid {
    jobs::enqueue_with_priority(
        &worker.pool,
        clips::KEYFRAMES_JOB,
        json!({ "clipId": clip }),
        jobs::PRIORITY_BACKGROUND,
    )
    .await
    .unwrap()
}

async fn exists(worker: &Worker, container: Container, blob: &str) -> bool {
    worker
        .storage
        .blob_size(container, blob)
        .await
        .unwrap()
        .is_some()
}

/// Browser-ready, but with a keyframe only every 10 s: copying it would make seeks in the
/// show slow, so it's re-encoded with one every 2 s.
#[sqlx::test(migrator = "clipos_core::db::MIGRATOR")]
async fn re_encodes_when_keyframes_are_far_apart(pool: PgPool) {
    if !azurite() {
        return;
    }
    let dir = tempfile::tempdir().unwrap();
    let source = dir.path().join("long-gop.mp4");
    make_h264(
        &source,
        &["-t", "12", "-g", "600", "-keyint_min", "600", "-b:v", "4M"],
    );
    let worker = worker(pool);
    let clip = uploaded_clip(&worker, &source).await;
    assert!(worker.run_once().await.unwrap());
    assert_eq!(transcode_mode(&worker.pool, clip.id).await, "transcode");
    let gap = playback_gap(&worker, clip.id).await;
    assert!(gap <= 2.1, "gap {gap}");
}

/// A ready clip remuxed from a file with a keyframe every second.
async fn remuxed_clip(worker: &Worker, dir: &Path) -> clips::Clip {
    let source = dir.join("fine.mp4");
    make_h264(&source, &["-t", "12", "-g", "60", "-b:v", "4M"]);
    let clip = uploaded_clip(worker, &source).await;
    assert!(worker.run_once().await.unwrap());
    assert_eq!(transcode_mode(&worker.pool, clip.id).await, "remux");
    clip
}

/// The backfill: an older clip whose playback file has far-apart keyframes is re-encoded
/// under new blob names (a player midway through the old file never gets the new one's
/// bytes), and the old files are deleted once every link to them has expired; one that's
/// fine is left alone. Purging the clip later still removes every file.
#[sqlx::test(migrator = "clipos_core::db::MIGRATOR")]
async fn the_keyframes_job_fixes_older_clips(pool: PgPool) {
    if !azurite() {
        return;
    }
    let dir = tempfile::tempdir().unwrap();
    let worker = worker(pool);
    let clip = remuxed_clip(&worker, dir.path()).await;

    // Fine: left alone.
    queue_keyframes(&worker, clip.id).await;
    assert!(worker.run_once().await.unwrap());
    assert_eq!(transcode_mode(&worker.pool, clip.id).await, "remux");

    // An old playback file with a keyframe every 10 s: re-encoded.
    let old = dir.path().join("old.mp4");
    make_h264(&old, &["-t", "12", "-g", "600", "-keyint_min", "600"]);
    let old_playback = clips::playback_blob(clip.id);
    let old_poster = clips::poster_blob(clip.id);
    worker
        .storage
        .upload_file(Container::Playback, &old_playback, &old, "video/mp4")
        .await
        .unwrap();
    assert!(playback_gap(&worker, clip.id).await > 4.0);
    queue_keyframes(&worker, clip.id).await;
    assert!(worker.run_once().await.unwrap());
    let gap = playback_gap(&worker, clip.id).await;
    assert!(gap <= 2.1, "gap {gap}");

    let done = clips::get(&worker.pool, clip.id).await.unwrap().unwrap();
    assert_eq!(done.status, ClipStatus::Ready);
    let playback = done.playback_blob.unwrap();
    let poster = done.poster_blob.unwrap();
    assert_ne!(playback, old_playback);
    assert_ne!(poster, old_poster);
    assert!(exists(&worker, Container::Playback, &playback).await);
    assert!(exists(&worker, Container::Posters, &poster).await);

    // The old files stay while links to them may still be playing: a background job
    // deletes them after the links expire.
    assert!(exists(&worker, Container::Playback, &old_playback).await);
    assert!(exists(&worker, Container::Posters, &old_poster).await);
    let (job, priority, waits_s): (Uuid, i16, f64) = sqlx::query_as(
        "SELECT id, priority, extract(epoch FROM run_after - now())::float8 FROM jobs
          WHERE kind = $1 AND status = 'queued'",
    )
    .bind(clips::DELETE_BLOBS_JOB)
    .fetch_one(&worker.pool)
    .await
    .unwrap();
    assert_eq!(priority, jobs::PRIORITY_BACKGROUND);
    assert!(
        waits_s > clips::VIEW_TTL.as_secs_f64(),
        "runs in {waits_s} s"
    );
    assert!(!worker.run_once().await.unwrap(), "not yet");

    sqlx::query("UPDATE jobs SET run_after = now() WHERE id = $1")
        .bind(job)
        .execute(&worker.pool)
        .await
        .unwrap();
    assert!(worker.run_once().await.unwrap());
    assert_eq!(job_row(&worker.pool, job).await, ("succeeded".into(), None));
    assert!(!exists(&worker, Container::Playback, &old_playback).await);
    assert!(!exists(&worker, Container::Posters, &old_poster).await);

    // A file left behind (the delete job gave up) is purged too.
    worker
        .storage
        .upload_file(Container::Playback, &old_playback, &old, "video/mp4")
        .await
        .unwrap();
    clips::soft_delete(&worker.pool, clip.id).await.unwrap();
    sqlx::query("UPDATE clips SET deleted_at = now() - interval '8 days'")
        .execute(&worker.pool)
        .await
        .unwrap();
    assert_eq!(worker.purge_trash().await.unwrap(), 1);
    for (container, blob) in [
        (Container::Originals, clip.original_blob.clone()),
        (Container::Playback, playback),
        (Container::Playback, old_playback),
        (Container::Posters, poster),
        (Container::Posters, clips::teaser_blob(clip.id)),
    ] {
        assert!(!exists(&worker, container, &blob).await, "{blob}");
    }
}

/// Deleting blobs that are already gone succeeds; a payload naming no real container
/// fails for good.
#[sqlx::test(migrator = "clipos_core::db::MIGRATOR")]
async fn deleting_missing_blobs_is_fine(pool: PgPool) {
    if !azurite() {
        return;
    }
    let worker = worker(pool);
    worker.storage.prepare_local(&[]).await.unwrap();
    let id = Uuid::new_v4();
    let job = worker
        .queue_delete_blobs(
            &[
                (Container::Playback, clips::playback_blob(id)),
                (Container::Posters, clips::poster_blob(id)),
            ],
            Duration::ZERO,
        )
        .await
        .unwrap();
    assert!(worker.run_once().await.unwrap());
    assert_eq!(job_row(&worker.pool, job).await, ("succeeded".into(), None));

    let bad = jobs::enqueue(
        &worker.pool,
        clips::DELETE_BLOBS_JOB,
        json!({ "blobs": [{ "container": "nope", "blob": "x" }] }),
    )
    .await
    .unwrap();
    assert!(worker.run_once().await.unwrap());
    assert_eq!(job_row(&worker.pool, bad).await.0, "failed");
}

/// A playback file that can't be probed (here: it's missing; in production a network or
/// SAS hiccup) is retried, not re-encoded.
#[sqlx::test(migrator = "clipos_core::db::MIGRATOR")]
async fn the_keyframes_job_retries_when_the_probe_fails(pool: PgPool) {
    if !azurite() {
        return;
    }
    let dir = tempfile::tempdir().unwrap();
    let worker = worker(pool);
    let clip = remuxed_clip(&worker, dir.path()).await;
    let playback = clips::playback_blob(clip.id);
    worker
        .storage
        .delete(Container::Playback, &playback)
        .await
        .unwrap();

    let job = queue_keyframes(&worker, clip.id).await;
    assert!(worker.run_once().await.unwrap());
    let (status, error) = job_row(&worker.pool, job).await;
    assert_eq!(status, "queued", "retried");
    assert!(error.unwrap().contains("ffprobe"));
    let after = clips::get(&worker.pool, clip.id).await.unwrap().unwrap();
    assert_eq!(after.playback_blob, Some(playback));
    assert_eq!(transcode_mode(&worker.pool, clip.id).await, "remux");
}

/// A playback file ffprobe reads fine but finds no video keyframes in is re-encoded.
#[sqlx::test(migrator = "clipos_core::db::MIGRATOR")]
async fn the_keyframes_job_re_encodes_when_there_are_no_keyframes(pool: PgPool) {
    if !azurite() {
        return;
    }
    let dir = tempfile::tempdir().unwrap();
    let worker = worker(pool);
    let clip = remuxed_clip(&worker, dir.path()).await;
    let audio_only = dir.path().join("audio.mp4");
    let status = Command::new("ffmpeg")
        .args(["-hide_banner", "-loglevel", "error", "-y"])
        .args(["-f", "lavfi", "-i", "sine=frequency=440", "-t", "2"])
        .args(["-c:a", "aac"])
        .arg(&audio_only)
        .status()
        .expect("ffmpeg is installed");
    assert!(status.success());
    worker
        .storage
        .upload_file(
            Container::Playback,
            &clips::playback_blob(clip.id),
            &audio_only,
            "video/mp4",
        )
        .await
        .unwrap();

    let job = queue_keyframes(&worker, clip.id).await;
    assert!(worker.run_once().await.unwrap());
    assert_eq!(job_row(&worker.pool, job).await, ("succeeded".into(), None));
    let done = clips::get(&worker.pool, clip.id).await.unwrap().unwrap();
    assert_ne!(done.playback_blob, Some(clips::playback_blob(clip.id)));
    let gap = playback_gap(&worker, clip.id).await;
    assert!(gap <= 2.1, "gap {gap}");
}

/// Runs ffmpeg to make a test input.
fn ffmpeg(args: &[&str], output: &Path) {
    let status = Command::new("ffmpeg")
        .args(["-hide_banner", "-loglevel", "error", "-y"])
        .args(args)
        .arg(output)
        .status()
        .expect("ffmpeg is installed");
    assert!(status.success(), "ffmpeg {args:?}");
}

/// Uploads `file` and runs its transcode; returns the clip as it ended up.
async fn processed(worker: &Worker, file: &Path) -> clips::Clip {
    let clip = uploaded_clip(worker, file).await;
    assert!(worker.run_once().await.unwrap());
    clips::get(&worker.pool, clip.id).await.unwrap().unwrap()
}

fn assert_ready(clip: &clips::Clip, size: (i32, i32)) {
    assert_eq!(clip.status, ClipStatus::Ready, "{:?}", clip.error);
    assert_eq!((clip.width, clip.height), (Some(size.0), Some(size.1)));
}

fn assert_failed(clip: &clips::Clip, reason: clips::FailureReason) {
    assert_eq!(clip.status, ClipStatus::Failed);
    let error = clip.error.as_deref().unwrap();
    assert_eq!(clips::FailureReason::of(error), reason, "{error}");
}

/// x264's 4:2:0 needs even sizes: odd ones are rounded down, whatever the codec.
#[sqlx::test(migrator = "clipos_core::db::MIGRATOR")]
async fn transcodes_odd_sizes(pool: PgPool) {
    if !azurite() {
        return;
    }
    let dir = tempfile::tempdir().unwrap();
    let odd = [
        "-f",
        "lavfi",
        "-i",
        "testsrc2=size=1281x721:rate=30",
        "-t",
        "1",
    ];
    let vp9 = dir.path().join("odd.mkv");
    ffmpeg(
        &[
            &odd[..],
            &[
                "-c:v",
                "libvpx-vp9",
                "-deadline",
                "realtime",
                "-cpu-used",
                "8",
            ],
        ]
        .concat(),
        &vp9,
    );
    let h264_444 = dir.path().join("odd-444.mp4");
    ffmpeg(
        &[
            &odd[..],
            &[
                "-c:v",
                "libx264",
                "-preset",
                "ultrafast",
                "-pix_fmt",
                "yuv444p",
            ],
        ]
        .concat(),
        &h264_444,
    );
    let worker = worker(pool);
    for file in [vp9, h264_444] {
        assert_ready(&processed(&worker, &file).await, (1280, 720));
    }
}

/// A 1440×1080 recording with 4:3 pixels (shown 16:9) comes out 1920×1080 with square
/// pixels, not squeezed to 4:3.
#[sqlx::test(migrator = "clipos_core::db::MIGRATOR")]
async fn transcodes_anamorphic_video_to_its_display_shape(pool: PgPool) {
    if !azurite() {
        return;
    }
    let dir = tempfile::tempdir().unwrap();
    let file = dir.path().join("anamorphic.mp4");
    make_h264(&file, &["-t", "1", "-vf", "scale=1440:1080,setsar=4/3"]);
    let worker = worker(pool);
    let done = processed(&worker, &file).await;
    assert_ready(&done, (1920, 1080));
    assert_eq!(transcode_mode(&worker.pool, done.id).await, "transcode");
}

/// The poster comes from the video itself, wherever the file's length puts its middle.
#[sqlx::test(migrator = "clipos_core::db::MIGRATOR")]
async fn posters_of_short_and_one_frame_videos(pool: PgPool) {
    if !azurite() {
        return;
    }
    let dir = tempfile::tempdir().unwrap();
    // The recorder dropped the video after 2 s; the audio runs to 10 s.
    let short_video = dir.path().join("short-video.mp4");
    ffmpeg(
        &[
            "-f",
            "lavfi",
            "-i",
            "testsrc2=size=320x240:rate=30:duration=2",
            "-f",
            "lavfi",
            "-i",
            "sine=duration=10",
            "-c:v",
            "libx264",
            "-preset",
            "ultrafast",
            "-pix_fmt",
            "yuv420p",
            "-c:a",
            "aac",
        ],
        &short_video,
    );
    let one_frame = dir.path().join("one-frame.mp4");
    ffmpeg(
        &[
            "-f",
            "lavfi",
            "-i",
            "testsrc2=size=320x240:rate=30",
            "-frames:v",
            "1",
            "-c:v",
            "libx264",
            "-pix_fmt",
            "yuv420p",
        ],
        &one_frame,
    );
    let worker = worker(pool);
    for file in [short_video, one_frame] {
        let done = processed(&worker, &file).await;
        assert_ready(&done, (320, 240));
        let poster = done.poster_blob.unwrap();
        assert!(exists(&worker, Container::Posters, &poster).await);
        assert!(exists(&worker, Container::Posters, &clips::teaser_blob(done.id)).await);
    }
}

/// A dark, flat 360p clip gives a poster of under 2 KB; its teaser is still made.
#[sqlx::test(migrator = "clipos_core::db::MIGRATOR")]
async fn teases_a_clip_with_a_tiny_poster(pool: PgPool) {
    if !azurite() {
        return;
    }
    let dir = tempfile::tempdir().unwrap();
    let flat = dir.path().join("flat.mp4");
    ffmpeg(
        &[
            "-f",
            "lavfi",
            "-i",
            "color=0x101010:size=640x360:rate=30",
            "-t",
            "2",
            "-c:v",
            "libx264",
            "-pix_fmt",
            "yuv420p",
        ],
        &flat,
    );
    let worker = worker(pool);
    let done = processed(&worker, &flat).await;
    assert_ready(&done, (640, 360));
    let poster = worker
        .storage
        .blob_size(Container::Posters, &done.poster_blob.unwrap())
        .await
        .unwrap()
        .unwrap();
    assert!(poster < 2_048, "{poster}");
    assert!(exists(&worker, Container::Posters, &clips::teaser_blob(done.id)).await);
}

/// Cover art and preview tracks aren't the video, wherever they are in the file; a file
/// with nothing else, or a photo renamed .mp4, isn't a video.
#[sqlx::test(migrator = "clipos_core::db::MIGRATOR")]
async fn finds_the_video_among_other_streams(pool: PgPool) {
    if !azurite() {
        return;
    }
    let dir = tempfile::tempdir().unwrap();
    let cover = dir.path().join("cover.jpg");
    ffmpeg(
        &[
            "-f",
            "lavfi",
            "-i",
            "color=red:size=600x600",
            "-frames:v",
            "1",
        ],
        &cover,
    );
    let video = dir.path().join("video.mp4");
    ffmpeg(
        &[
            "-f",
            "lavfi",
            "-i",
            "testsrc2=size=1280x720:rate=30",
            "-f",
            "lavfi",
            "-i",
            "sine",
            "-t",
            "1",
            "-c:v",
            "libx264",
            "-preset",
            "ultrafast",
            "-pix_fmt",
            "yuv420p",
            "-c:a",
            "aac",
        ],
        &video,
    );
    let (cover_s, video_s) = (cover.to_str().unwrap(), video.to_str().unwrap());

    // Cover art first: still copied as is, video stream only.
    let cover_first = dir.path().join("cover-first.mkv");
    ffmpeg(
        &[
            "-i",
            cover_s,
            "-i",
            video_s,
            "-map",
            "0",
            "-map",
            "1",
            "-c",
            "copy",
            "-disposition:v:0",
            "attached_pic",
        ],
        &cover_first,
    );
    // A 160×90 preview track first, the 90 fps video second (so transcoded to 60).
    let small_first = dir.path().join("small-first.mp4");
    ffmpeg(
        &[
            "-f",
            "lavfi",
            "-i",
            "testsrc2=size=160x90:rate=30",
            "-f",
            "lavfi",
            "-i",
            "testsrc2=size=1280x720:rate=90",
            "-t",
            "1",
            "-map",
            "0",
            "-map",
            "1",
            "-c:v",
            "libx264",
            "-preset",
            "ultrafast",
            "-pix_fmt",
            "yuv420p",
        ],
        &small_first,
    );
    let audio_with_cover = dir.path().join("audio-with-cover.mp4");
    ffmpeg(
        &[
            "-f",
            "lavfi",
            "-i",
            "sine",
            "-i",
            cover_s,
            "-t",
            "3",
            "-map",
            "0",
            "-map",
            "1",
            "-c:a",
            "aac",
            "-c:v",
            "copy",
            "-disposition:v:0",
            "attached_pic",
        ],
        &audio_with_cover,
    );
    let photo = dir.path().join("photo.mp4");
    std::fs::copy(&cover, &photo).unwrap();

    let worker = worker(pool);
    let done = processed(&worker, &cover_first).await;
    assert_ready(&done, (1280, 720));
    assert_eq!(transcode_mode(&worker.pool, done.id).await, "remux");
    let done = processed(&worker, &small_first).await;
    assert_ready(&done, (1280, 720));
    assert!((done.fps.unwrap() - 60.0).abs() < 0.5, "{:?}", done.fps);
    for file in [audio_with_cover, photo] {
        assert_failed(
            &processed(&worker, &file).await,
            clips::FailureReason::NotAVideo,
        );
    }
}

/// The format ffprobe reads `file` as when nothing is refused.
fn format_name(file: &Path) -> String {
    let out = Command::new("ffprobe")
        .args(["-v", "error", "-show_entries", "format=format_name"])
        .args(["-of", "csv=p=0"])
        .arg(file)
        .output()
        .expect("ffprobe is installed");
    String::from_utf8_lossy(&out.stdout).trim().to_owned()
}

/// A file called .mp4 or .mkv that's really in another format ffmpeg reads (an audio
/// file, a TV recording, a picture, a list of other files to play) is refused by its
/// format, before ffmpeg reads it as that: no format but ours is code an upload can
/// reach.
#[sqlx::test(migrator = "clipos_core::db::MIGRATOR")]
async fn refuses_files_in_formats_we_dont_read(pool: PgPool) {
    if !azurite() {
        return;
    }
    let dir = tempfile::tempdir().unwrap();
    let path = |name: &str| dir.path().join(name);
    small_video(&path("video.mp4"), 1);
    ffmpeg(
        &["-f", "lavfi", "-i", "sine", "-t", "1", "-f", "caf"],
        &path("caf.mp4"),
    );
    ffmpeg(
        &[
            "-f",
            "lavfi",
            "-i",
            "testsrc2=size=160x90:rate=10",
            "-t",
            "1",
            "-c:v",
            "mpeg2video",
            "-f",
            "wtv",
        ],
        &path("wtv.mkv"),
    );
    ffmpeg(
        &[
            "-f",
            "lavfi",
            "-i",
            "color=blue:size=64x64",
            "-frames:v",
            "1",
            "-c:v",
            "png",
            "-f",
            "image2pipe",
        ],
        &path("png.mp4"),
    );
    std::fs::write(path("concat.mp4"), "ffconcat version 1.0\nfile video.mp4\n").unwrap();

    let worker = worker(pool);
    for (name, format) in [
        ("caf.mp4", "caf"),
        ("wtv.mkv", "wtv"),
        ("png.mp4", "png_pipe"),
        ("concat.mp4", "concat"),
    ] {
        // What ffmpeg would take it for, left to itself.
        assert_eq!(format_name(&path(name)), format);
        let done = processed(&worker, &path(name)).await;
        assert_failed(&done, clips::FailureReason::NotAVideo);
        let error = done.error.unwrap();
        assert!(
            error.ends_with("(it isn't an MP4, MKV or MOV file)"),
            "{error}"
        );
    }
}

/// Every video and audio codec on the list goes through, in each container; a stream in
/// any other codec gets the file refused, before anything decodes it.
#[sqlx::test(migrator = "clipos_core::db::MIGRATOR")]
async fn reads_the_codecs_we_accept_and_no_others(pool: PgPool) {
    if !azurite() {
        return;
    }
    let dir = tempfile::tempdir().unwrap();
    let subtitles = dir.path().join("subtitles.srt");
    std::fs::write(&subtitles, "1\n00:00:00,000 --> 00:00:00,500\ngg\n").unwrap();
    let picture = ["-f", "lavfi", "-i", "testsrc2=size=320x240:rate=30"];
    let sound = ["-f", "lavfi", "-i", "sine"];
    let worker = worker(pool);
    for (name, codecs) in [
        ("av1.mp4", &["-c:v", "libsvtav1", "-c:a", "aac"][..]),
        ("prores.mov", &["-c:v", "prores_ks", "-c:a", "pcm_s24le"]),
        (
            "vp9.mkv",
            &[
                "-c:v",
                "libvpx-vp9",
                "-deadline",
                "realtime",
                "-cpu-used",
                "8",
                "-c:a",
                "libopus",
            ],
        ),
        (
            "hevc.mkv",
            &["-c:v", "libx265", "-preset", "ultrafast", "-c:a", "flac"],
        ),
        (
            "h264.mkv",
            &[
                "-c:v",
                "libx264",
                "-preset",
                "ultrafast",
                "-c:a",
                "libmp3lame",
            ],
        ),
    ] {
        let file = dir.path().join(name);
        ffmpeg(
            &[&picture[..], &sound, &["-t", "1"], codecs].concat(),
            &file,
        );
        assert_ready(&processed(&worker, &file).await, (320, 240));
    }
    // Text subtitles alongside are fine (and left out).
    let with_subtitles = dir.path().join("subtitled.mkv");
    ffmpeg(
        &[
            &picture[..],
            &["-i", subtitles.to_str().unwrap(), "-t", "1"],
            &["-c:v", "libx264", "-preset", "ultrafast", "-c:s", "srt"],
        ]
        .concat(),
        &with_subtitles,
    );
    assert_ready(&processed(&worker, &with_subtitles).await, (320, 240));

    let mpeg2 = dir.path().join("mpeg2.mkv");
    ffmpeg(
        &[&picture[..], &["-t", "1", "-c:v", "mpeg2video"]].concat(),
        &mpeg2,
    );
    let done = processed(&worker, &mpeg2).await;
    assert_failed(&done, clips::FailureReason::Unreadable);
    assert!(done.error.unwrap().ends_with("(we don't read mpeg2video)"));
}

/// Raw H.264 states its length nowhere; it's measured from its packets, so 5 min 20 s of
/// it is still too long.
#[sqlx::test(migrator = "clipos_core::db::MIGRATOR")]
async fn measures_raw_h264(pool: PgPool) {
    if !azurite() {
        return;
    }
    let dir = tempfile::tempdir().unwrap();
    let raw = |seconds: &str, name: &str| {
        let file = dir.path().join(name);
        ffmpeg(
            &[
                "-f",
                "lavfi",
                "-i",
                "testsrc2=size=160x90:rate=2",
                "-t",
                seconds,
                "-c:v",
                "libx264",
                "-preset",
                "ultrafast",
                "-pix_fmt",
                "yuv420p",
                "-f",
                "h264",
            ],
            &file,
        );
        file
    };
    let worker = worker(pool);
    assert_failed(
        &processed(&worker, &raw("320", "long.mkv")).await,
        clips::FailureReason::TooLong,
    );
    let done = processed(&worker, &raw("10", "short.mkv")).await;
    assert_ready(&done, (160, 90));
    assert!((9_500..=10_500).contains(&done.duration_ms.unwrap()));
}

/// Makes an MKV that runs `seconds` (160×90 H.264 at 2 fps, with `extra` encoder options)
/// but whose header says it lasts `says`: its Segment Duration, which ffprobe gives as the
/// file's length, rewritten.
fn mkv_that_says(path: &Path, seconds: u32, says: f64, extra: &[&str]) {
    let length = seconds.to_string();
    let mut args = vec!["-f", "lavfi", "-i", "testsrc2=size=160x90:rate=2"];
    args.extend(["-t", &length, "-c:v", "libx264", "-preset", "ultrafast"]);
    args.extend(["-pix_fmt", "yuv420p"]);
    args.extend(extra);
    ffmpeg(&args, path);
    let mut bytes = std::fs::read(path).unwrap();
    // The Duration element: ID 0x4489, size 8, then milliseconds as a big-endian f64.
    let at = bytes
        .windows(3)
        .position(|w| w == [0x44, 0x89, 0x88])
        .expect("a Duration element")
        + 3;
    bytes[at..at + 8].copy_from_slice(&(says * 1000.0).to_be_bytes());
    std::fs::write(path, bytes).unwrap();
    let out = Command::new("ffprobe")
        .args(["-v", "error", "-show_entries", "format=duration"])
        .args(["-of", "csv=p=0"])
        .arg(path)
        .output()
        .expect("ffprobe is installed");
    let said: f64 = String::from_utf8_lossy(&out.stdout).trim().parse().unwrap();
    assert_eq!(said, says);
}

/// The 5-minute cap holds on what a file holds, not on what its header says. One that says
/// 10 s but runs 5 min 20 s is too long, copied or encoded; one that says 2 s but runs 30 s
/// is kept, as the 30 s it is.
#[sqlx::test(migrator = "clipos_core::db::MIGRATOR")]
async fn the_length_cap_holds_whatever_the_header_says(pool: PgPool) {
    if !azurite() {
        return;
    }
    let dir = tempfile::tempdir().unwrap();
    // A keyframe every 2 s, so it's copied; one every 125 s (x264's default), so it's
    // encoded.
    let copied = dir.path().join("copied.mkv");
    mkv_that_says(&copied, 320, 10.0, &["-g", "4"]);
    let encoded = dir.path().join("encoded.mkv");
    mkv_that_says(&encoded, 320, 10.0, &[]);

    let worker = worker(pool);
    let (logs, guard) = capture_logs();
    for file in [&copied, &encoded] {
        let done = processed(&worker, file).await;
        assert_failed(&done, clips::FailureReason::TooLong);
        let error = done.error.unwrap();
        assert!(
            error.ends_with("(this one is longer than it says it is)"),
            "{error}"
        );
    }
    drop(guard);
    let logs = logs.text();
    assert_eq!(logs.matches("too far apart to copy").count(), 1, "{logs}");

    let short = dir.path().join("short.mkv");
    mkv_that_says(&short, 30, 2.0, &["-g", "4"]);
    let done = processed(&worker, &short).await;
    assert_ready(&done, (160, 90));
    assert!(
        (29_500..=30_500).contains(&done.duration_ms.unwrap()),
        "{:?}",
        done.duration_ms
    );
}

/// Keyframes every 2 s for the first 30 s, then one in 25 s: the whole file is checked, so
/// it isn't copied with that gap.
#[sqlx::test(migrator = "clipos_core::db::MIGRATOR")]
async fn checks_keyframes_over_the_whole_file(pool: PgPool) {
    if !azurite() {
        return;
    }
    let dir = tempfile::tempdir().unwrap();
    let file = dir.path().join("late-gop.mp4");
    ffmpeg(
        &[
            "-f",
            "lavfi",
            "-i",
            "testsrc2=size=320x180:rate=10",
            "-t",
            "55",
            "-c:v",
            "libx264",
            "-preset",
            "ultrafast",
            "-pix_fmt",
            "yuv420p",
            "-force_key_frames",
            "expr:if(lt(t,30),gte(t,n_forced*2),0)",
            "-g",
            "900",
            "-sc_threshold",
            "0",
        ],
        &file,
    );
    let gap = crate::transcode::max_keyframe_gap(file.to_str().unwrap())
        .await
        .unwrap()
        .unwrap();
    assert!(gap > 20.0, "gap {gap}");

    let worker = worker(pool);
    let done = processed(&worker, &file).await;
    assert_ready(&done, (320, 180));
    assert_eq!(transcode_mode(&worker.pool, done.id).await, "transcode");
    let gap = playback_gap(&worker, done.id).await;
    assert!(gap <= 2.1, "gap {gap}");
}

/// A phone's portrait clip, stored landscape and flagged to turn: copied as is, and
/// recorded portrait.
#[sqlx::test(migrator = "clipos_core::db::MIGRATOR")]
async fn records_a_rotated_remux_upright(pool: PgPool) {
    if !azurite() {
        return;
    }
    let dir = tempfile::tempdir().unwrap();
    let landscape = dir.path().join("landscape.mp4");
    make_h264(&landscape, &["-t", "1", "-g", "60", "-b:v", "4M"]);
    let portrait = dir.path().join("portrait.mp4");
    ffmpeg(
        &[
            "-display_rotation:v:0",
            "90",
            "-i",
            landscape.to_str().unwrap(),
            "-c",
            "copy",
        ],
        &portrait,
    );
    let worker = worker(pool);
    let done = processed(&worker, &portrait).await;
    assert_eq!(transcode_mode(&worker.pool, done.id).await, "remux");
    assert_ready(&done, (1080, 1920));
}

/// Overwrites `count` short runs of `file` (past its first 20 KB, where the header is)
/// with junk, like a disk or a network that mangled it.
fn corrupt(file: &Path, count: usize, len: usize) {
    use std::io::{Seek, SeekFrom, Write};
    let size = std::fs::metadata(file).unwrap().len() as usize;
    let mut f = std::fs::OpenOptions::new().write(true).open(file).unwrap();
    for i in 0..count {
        f.seek(SeekFrom::Start(
            (20_000 + (size - 40_000) * i / count) as u64,
        ))
        .unwrap();
        let junk: Vec<u8> = (0..len).map(|j| (i * 37 + j * 11) as u8).collect();
        f.write_all(&junk).unwrap();
    }
}

/// Samples the killfeed corner of `file` on another thread, so a hang fails the test
/// instead of blocking it, calling `frame` for each frame. Returns the result and how long
/// it took.
fn sample(
    file: &Path,
    limit: Duration,
    mut frame: impl FnMut() + Send + 'static,
) -> (anyhow::Result<u32>, Duration) {
    let input = analyse::Input {
        source: file.to_str().unwrap().into(),
        stream: 0,
        width: 320,
        height: 180,
    };
    let (tx, rx) = std::sync::mpsc::channel();
    let started = std::time::Instant::now();
    std::thread::spawn(move || {
        let deadline = std::time::Instant::now() + limit;
        let result = analyse::sample_corner(&input, deadline, |_, _| {
            frame();
            Ok(())
        });
        let _ = tx.send(result);
    });
    let result = rx
        .recv_timeout(Duration::from_secs(30))
        .expect("sampling hung");
    (result, started.elapsed())
}

/// A badly damaged file makes ffmpeg write a line for every broken macroblock: well over
/// the 64 KB a pipe holds. The killfeed reader keeps reading frames anyway.
#[test]
fn analyse_survives_a_flood_of_ffmpeg_errors() {
    let dir = tempfile::tempdir().unwrap();
    let file = dir.path().join("damaged.mp4");
    ffmpeg(
        &[
            "-f",
            "lavfi",
            "-i",
            "testsrc2=size=320x180:rate=60",
            "-t",
            "20",
            "-c:v",
            "libx264",
            "-preset",
            "ultrafast",
            "-pix_fmt",
            "yuv420p",
            "-movflags",
            "+faststart",
        ],
        &file,
    );
    corrupt(&file, 3000, 32);
    let errors = Command::new("ffmpeg")
        .args(["-v", "error", "-nostdin", "-i"])
        .arg(&file)
        .args(["-vf", "fps=1", "-f", "null", "-"])
        .output()
        .unwrap()
        .stderr;
    assert!(errors.len() > 64 << 10, "only {} bytes", errors.len());

    let (result, _) = sample(&file, Duration::from_secs(60), || {});
    assert!(result.unwrap() >= 19);
}

/// ffmpeg is stopped at the deadline, however slow the frames are to handle.
#[test]
fn analyse_gives_up_at_its_time_limit() {
    let dir = tempfile::tempdir().unwrap();
    let file = dir.path().join("clip.mp4");
    ffmpeg(
        &[
            "-f",
            "lavfi",
            "-i",
            "testsrc2=size=320x180:rate=30",
            "-t",
            "20",
            "-c:v",
            "libx264",
            "-preset",
            "ultrafast",
        ],
        &file,
    );
    let (result, took) = sample(&file, Duration::from_secs(1), || {
        std::thread::sleep(Duration::from_millis(300))
    });
    let error = result.unwrap_err().to_string();
    assert!(error.contains("took too long"), "{error}");
    assert!(took < Duration::from_secs(3), "{took:?}");
    // Scaled to the clip: a 5-minute clip gets longer than a short one.
    assert!(analyse::time_limit(Duration::from_secs(300)) > analyse::time_limit(Duration::ZERO));
}

/// The killfeed reader reads the original with the transcode's allowlists: a file in
/// another format is refused, not decoded.
#[test]
fn analyse_reads_only_formats_we_accept() {
    let dir = tempfile::tempdir().unwrap();
    let file = dir.path().join("audio.mp4");
    ffmpeg(
        &["-f", "lavfi", "-i", "sine", "-t", "1", "-f", "caf"],
        &file,
    );
    let (result, _) = sample(&file, Duration::from_secs(30), || {});
    let error = result.unwrap_err().to_string();
    assert!(error.contains("Format not on whitelist"), "{error}");
}

/// A processing clip with its transcode queued, without Azurite (no file uploaded).
async fn processing_clip(pool: &PgPool) -> Uuid {
    let owner_id = owner(pool).await;
    let clip = clips::create(
        pool,
        NewClip {
            owner_id,
            game_id: "cs2".into(),
            title: "Crashes the worker".into(),
            description: String::new(),
            map: None,
            my_pov: true,
            filename: "bad.mp4".into(),
            bytes: 1_000_000,
        },
    )
    .await
    .unwrap();
    assert!(clips::mark_uploaded(pool, clip.id).await.unwrap());
    clip.id
}

/// Makes the running jobs look orphaned: locked two hours ago, never finished.
async fn go_stale(pool: &PgPool) {
    sqlx::query("UPDATE jobs SET locked_at = now() - interval '2 hours' WHERE status = 'running'")
        .execute(pool)
        .await
        .unwrap();
}

/// A transcode that kills its worker every time (out of memory on a pathological file,
/// say) is given up on after its attempts, and the uploader sees the clip failed.
#[sqlx::test(migrator = "clipos_core::db::MIGRATOR")]
async fn a_transcode_that_keeps_crashing_fails_the_clip(pool: PgPool) {
    let clip = processing_clip(&pool).await;
    let worker = worker(pool.clone());
    for attempt in 1..=jobs::MAX_ATTEMPTS {
        // A worker claims it and dies; the reaper finds it later.
        jobs::claim(&pool, "dies").await.unwrap().unwrap();
        go_stale(&pool).await;
        let reaped = worker
            .reap_stale_jobs(Duration::from_secs(1800))
            .await
            .unwrap();
        if attempt < jobs::MAX_ATTEMPTS {
            assert_eq!(reaped, (1, 0));
            let processing = clips::get(&pool, clip).await.unwrap().unwrap();
            assert_eq!(processing.status, ClipStatus::Processing);
        } else {
            assert_eq!(reaped, (0, 1));
        }
    }
    let failed = clips::get(&pool, clip).await.unwrap().unwrap();
    assert_failed(&failed, clips::FailureReason::Server);
    assert_eq!(
        failed.error.as_deref(),
        Some("processing kept crashing after 3 attempts")
    );
    assert!(!worker.run_once().await.unwrap(), "nothing left to run");
}

/// A run that hung past the reaper and fails when it finally returns doesn't touch the
/// clip another run finished meanwhile, nor put the job back in the queue.
#[sqlx::test(migrator = "clipos_core::db::MIGRATOR")]
async fn a_late_failure_from_a_requeued_run_changes_nothing(pool: PgPool) {
    if !azurite() {
        return;
    }
    let dir = tempfile::tempdir().unwrap();
    let source = dir.path().join("s.mp4");
    make_h264(&source, &["-t", "1"]);
    let worker = worker(pool.clone());
    let clip = uploaded_clip(&worker, &source).await;

    let hung = jobs::claim(&pool, "hung").await.unwrap().unwrap();
    go_stale(&pool).await;
    worker
        .reap_stale_jobs(Duration::from_secs(1800))
        .await
        .unwrap();
    assert!(worker.run_once().await.unwrap());
    assert_ready(
        &clips::get(&pool, clip.id).await.unwrap().unwrap(),
        (1920, 1080),
    );

    assert_eq!(
        jobs::fail(&pool, &hung, "hung", "ffmpeg was killed", true)
            .await
            .unwrap(),
        jobs::Failed::Lost
    );
    assert_eq!(job_row(&pool, hung.id).await, ("succeeded".into(), None));
    assert_ready(
        &clips::get(&pool, clip.id).await.unwrap().unwrap(),
        (1920, 1080),
    );
}

/// A clip deleted while its transcode waited (the worker drops it then) and restored
/// afterwards is processed after all.
#[sqlx::test(migrator = "clipos_core::db::MIGRATOR")]
async fn a_clip_restored_while_processing_is_finished(pool: PgPool) {
    if !azurite() {
        return;
    }
    let dir = tempfile::tempdir().unwrap();
    let source = dir.path().join("s.mp4");
    make_h264(&source, &["-t", "1"]);
    let worker = worker(pool.clone());
    let clip = uploaded_clip(&worker, &source).await;
    assert!(clips::soft_delete(&pool, clip.id).await.unwrap());
    assert!(worker.run_once().await.unwrap(), "dropped");
    assert!(clips::restore(&pool, clip.id).await.unwrap());
    assert!(worker.run_once().await.unwrap(), "queued again");
    assert_ready(
        &clips::get(&pool, clip.id).await.unwrap().unwrap(),
        (1920, 1080),
    );
}

/// The upload link stays writable for a while after `complete`. An original written again
/// since (even with the same bytes) isn't the file that was checked, so the clip fails
/// for good and the uploader is asked for the file again.
#[sqlx::test(migrator = "clipos_core::db::MIGRATOR")]
async fn an_original_changed_after_complete_fails_the_clip(pool: PgPool) {
    if !azurite() {
        return;
    }
    let dir = tempfile::tempdir().unwrap();
    let source = dir.path().join("s.mp4");
    make_h264(&source, &["-t", "1"]);
    let worker = worker(pool.clone());
    let clip = uploaded_clip(&worker, &source).await;
    worker
        .storage
        .upload_file(
            Container::Originals,
            &clip.original_blob,
            &source,
            "video/mp4",
        )
        .await
        .unwrap();
    assert!(worker.run_once().await.unwrap());

    let failed = clips::get(&pool, clip.id).await.unwrap().unwrap();
    assert_failed(&failed, clips::FailureReason::Unreadable);
    assert_eq!(failed.error.as_deref(), Some(clips::CHANGED_ERROR));
    assert!(!worker.run_once().await.unwrap(), "not retried");
}

/// Clips completed before their ETag was kept (migration 0014) are still processed.
#[sqlx::test(migrator = "clipos_core::db::MIGRATOR")]
async fn an_original_without_an_etag_is_processed(pool: PgPool) {
    if !azurite() {
        return;
    }
    let dir = tempfile::tempdir().unwrap();
    let source = dir.path().join("s.mp4");
    make_h264(&source, &["-t", "1"]);
    let worker = worker(pool.clone());
    let clip = uploaded_clip(&worker, &source).await;
    sqlx::query("UPDATE clips SET original_etag = NULL WHERE id = $1")
        .bind(clip.id)
        .execute(&pool)
        .await
        .unwrap();
    assert!(worker.run_once().await.unwrap());
    assert_ready(
        &clips::get(&pool, clip.id).await.unwrap().unwrap(),
        (1920, 1080),
    );
}

/// Read over HTTP (no room for a local copy), the original is compared with what
/// `complete` saw: its ETag when it was kept, its size always.
#[sqlx::test(migrator = "clipos_core::db::MIGRATOR")]
async fn checks_an_original_read_over_http(pool: PgPool) {
    if !azurite() {
        return;
    }
    let changed = |result: Result<(), JobError>| matches!(result, Err(JobError::Permanent(e)) if e == clips::CHANGED_ERROR);
    let dir = tempfile::tempdir().unwrap();
    let source = dir.path().join("s.mp4");
    make_h264(&source, &["-t", "1"]);
    let worker = worker(pool);
    let clip = uploaded_clip(&worker, &source).await;
    assert!(clip.original_etag.is_some());
    transcode::check_original(&worker, &clip).await.unwrap();

    worker
        .storage
        .upload_file(
            Container::Originals,
            &clip.original_blob,
            &source,
            "video/mp4",
        )
        .await
        .unwrap();
    assert!(changed(transcode::check_original(&worker, &clip).await));
    // Without an ETag, the same size passes and another doesn't.
    let old = clips::Clip {
        original_etag: None,
        ..clip
    };
    transcode::check_original(&worker, &old).await.unwrap();
    let bigger = clips::Clip {
        original_bytes: old.original_bytes + 1,
        ..old
    };
    assert!(changed(transcode::check_original(&worker, &bigger).await));
}

/// Read over HTTP (the temp disk too small for a copy), the original transcodes like a
/// local one; overwritten while ffmpeg read it, the clip fails for good.
#[sqlx::test(migrator = "clipos_core::db::MIGRATOR")]
async fn transcodes_an_original_read_over_http(pool: PgPool) {
    if !azurite() {
        return;
    }
    fn changed<T>(result: &Result<T, JobError>) -> bool {
        matches!(result, Err(JobError::Permanent(e)) if e == clips::CHANGED_ERROR)
    }
    let dir = tempfile::tempdir().unwrap();
    let source = dir.path().join("s.mp4");
    small_video(&source, 1);
    let worker = worker(pool);
    let clip = uploaded_clip(&worker, &source).await;

    let original = transcode::remote_original(&worker, &clip).await.unwrap();
    assert!(original.remote);
    assert!(original.input.starts_with("http://127.0.0.1:10000/"));
    let done = transcode::transcode_from(&worker, &clip, dir.path(), original, None)
        .await
        .unwrap();
    assert_eq!((done.width, done.height), (320, 240));
    assert!(
        exists(&worker, Container::Playback, &done.playback_blob).await,
        "uploaded"
    );

    // Written again after ffmpeg was given the link: caught once it's done.
    let original = transcode::remote_original(&worker, &clip).await.unwrap();
    worker
        .storage
        .upload_file(
            Container::Originals,
            &clip.original_blob,
            &source,
            "video/mp4",
        )
        .await
        .unwrap();
    let result = transcode::transcode_from(&worker, &clip, dir.path(), original, None).await;
    assert!(changed(&result));
    // And before ffmpeg starts, for the next try.
    assert!(changed(&transcode::remote_original(&worker, &clip).await));
}

/// A small H.264 video (320×240, 30 fps) `seconds` long, with `extra` encoder options.
fn small_video_with(path: &Path, seconds: u32, extra: &[&str]) {
    let source = format!("testsrc2=size=320x240:rate=30:duration={seconds}");
    let mut args = vec!["-f", "lavfi", "-i", &source, "-c:v", "libx264"];
    args.extend(["-preset", "ultrafast", "-pix_fmt", "yuv420p"]);
    args.extend(extra);
    ffmpeg(&args, path);
}

fn small_video(path: &Path, seconds: u32) {
    small_video_with(path, seconds, &[]);
}

/// The jobs of `kind` for `clip`: (status, seconds until it may run).
async fn jobs_for(pool: &PgPool, kind: &str, clip: Uuid) -> Vec<(String, f64)> {
    sqlx::query_as(
        "SELECT status, extract(epoch FROM run_after - now())::float8 FROM jobs
          WHERE kind = $1 AND payload::text LIKE '%' || $2 || '%' ORDER BY created_at",
    )
    .bind(kind)
    .bind(clip.to_string())
    .fetch_all(pool)
    .await
    .unwrap()
}

/// From now on the database refuses `event` (say `"INSERT ON jobs"`) on rows where `when`
/// holds, the way a broken one would.
async fn refuse(pool: &PgPool, event: &str, when: &str) {
    sqlx::raw_sql(
        "CREATE OR REPLACE FUNCTION refuse() RETURNS trigger LANGUAGE plpgsql AS $$
         BEGIN RAISE EXCEPTION 'refused by the test'; END $$",
    )
    .execute(pool)
    .await
    .unwrap();
    let name = format!("refuse_{}", Uuid::new_v4().simple());
    sqlx::raw_sql(sqlx::AssertSqlSafe(format!(
        "CREATE TRIGGER {name} BEFORE {event} FOR EACH ROW WHEN ({when})
         EXECUTE FUNCTION refuse()"
    )))
    .execute(pool)
    .await
    .unwrap();
}

/// The killfeed is read from CS2 clips recorded from the uploader's own point of view;
/// a clip whose analysis can't be queued is ready all the same.
#[sqlx::test(migrator = "clipos_core::db::MIGRATOR")]
async fn queues_killfeed_analysis_for_the_uploaders_own_view(pool: PgPool) {
    if !azurite() {
        return;
    }
    let dir = tempfile::tempdir().unwrap();
    let source = dir.path().join("s.mp4");
    small_video(&source, 1);
    let mut worker = worker(pool.clone());
    worker.analyse = Some(AnalyseConfig {
        models_dir: dir.path().join("models"),
        rows_model: "killfeed-rows/v1".into(),
        icons_model: "killfeed-icons/v1".into(),
        threads: 1,
    });

    let mine = processed(&worker, &source).await;
    assert_ready(&mine, (320, 240));
    let queued = jobs_for(&pool, clips::ANALYSE_JOB, mine.id).await;
    assert_eq!(queued.len(), 1);
    assert_eq!(queued[0].0, "queued");
    sqlx::query("DELETE FROM jobs WHERE kind = $1")
        .bind(clips::ANALYSE_JOB)
        .execute(&pool)
        .await
        .unwrap();

    // Someone else's point of view: their killfeed isn't the uploader's.
    let theirs = uploaded_clip(&worker, &source).await;
    sqlx::query("UPDATE clips SET my_pov = false WHERE id = $1")
        .bind(theirs.id)
        .execute(&pool)
        .await
        .unwrap();
    assert!(worker.run_once().await.unwrap());
    let theirs = clips::get(&pool, theirs.id).await.unwrap().unwrap();
    assert_ready(&theirs, (320, 240));
    assert!(
        jobs_for(&pool, clips::ANALYSE_JOB, theirs.id)
            .await
            .is_empty()
    );

    // The analysis can't be queued.
    refuse(&pool, "INSERT ON jobs", "NEW.kind = 'analyse'").await;
    let lost = processed(&worker, &source).await;
    assert_ready(&lost, (320, 240));
    assert!(
        jobs_for(&pool, clips::ANALYSE_JOB, lost.id)
            .await
            .is_empty()
    );
    assert_eq!(
        jobs_for(&pool, clips::TRANSCODE_JOB, lost.id).await[0].0,
        "succeeded"
    );
}

/// Without its ETag (completed before migration 0014), an original is checked by size.
#[sqlx::test(migrator = "clipos_core::db::MIGRATOR")]
async fn an_original_of_another_size_fails_the_clip(pool: PgPool) {
    if !azurite() {
        return;
    }
    let dir = tempfile::tempdir().unwrap();
    let source = dir.path().join("s.mp4");
    small_video(&source, 1);
    let worker = worker(pool.clone());
    let clip = uploaded_clip(&worker, &source).await;
    sqlx::query(
        "UPDATE clips SET original_etag = NULL, original_bytes = original_bytes - 1
          WHERE id = $1",
    )
    .bind(clip.id)
    .execute(&pool)
    .await
    .unwrap();
    assert!(worker.run_once().await.unwrap());
    let failed = clips::get(&pool, clip.id).await.unwrap().unwrap();
    assert_eq!(failed.error.as_deref(), Some(clips::CHANGED_ERROR));
}

/// What a transcode did is in the logs: the original, how it was read, the mode, the
/// upload.
#[sqlx::test(migrator = "clipos_core::db::MIGRATOR")]
async fn the_transcode_logs_what_it_did(pool: PgPool) {
    if !azurite() {
        return;
    }
    let dir = tempfile::tempdir().unwrap();
    let source = dir.path().join("s.mp4");
    small_video(&source, 1);
    let worker = worker(pool);
    let clip = uploaded_clip(&worker, &source).await;
    let (logs, guard) = capture_logs();
    assert!(worker.run_once().await.unwrap());
    drop(guard);
    assert_ready(
        &clips::get(&worker.pool, clip.id).await.unwrap().unwrap(),
        (320, 240),
    );
    let logs = logs.text();
    let line = |message: &str| {
        logs.lines()
            .find(|l| l.contains(message))
            .unwrap_or_else(|| panic!("no {message:?} in {logs}"))
            .to_owned()
    };
    assert!(line("probed original").contains("codec=\"h264\""), "{logs}");
    let size = std::fs::metadata(&source).unwrap().len();
    assert!(
        line("downloaded original").contains(&format!("bytes={size}")),
        "{logs}"
    );
    assert!(line("transcoded").contains("mode=\"remux\""), "{logs}");
    assert!(line("uploaded").contains("upload_ms="), "{logs}");
    assert!(line("job succeeded").contains("elapsed_ms="), "{logs}");
}

/// What's logged (info and up) on this thread while the guard lives.
fn capture_logs() -> (Logs, tracing::subscriber::DefaultGuard) {
    let logs = Logs::default();
    let writer = logs.clone();
    let subscriber = tracing_subscriber::fmt()
        .with_writer(move || writer.clone())
        .with_ansi(false)
        .with_max_level(tracing::Level::INFO)
        .finish();
    (logs, tracing::subscriber::set_default(subscriber))
}

#[derive(Clone, Default)]
struct Logs(Arc<std::sync::Mutex<Vec<u8>>>);

impl Logs {
    fn text(&self) -> String {
        String::from_utf8_lossy(&self.0.lock().unwrap()).into_owned()
    }
}

impl std::io::Write for Logs {
    fn write(&mut self, buf: &[u8]) -> std::io::Result<usize> {
        self.0.lock().unwrap().extend_from_slice(buf);
        Ok(buf.len())
    }

    fn flush(&mut self) -> std::io::Result<()> {
        Ok(())
    }
}

/// A job that finished after the reaper gave it to another run is left to that run.
#[sqlx::test(migrator = "clipos_core::db::MIGRATOR")]
async fn a_job_requeued_while_it_ran_is_left_to_its_new_run(pool: PgPool) {
    let id = jobs::enqueue(&pool, "noop", json!({})).await.unwrap();
    let worker = worker(pool.clone());
    let job = jobs::claim(&pool, &worker.id).await.unwrap().unwrap();
    go_stale(&pool).await;
    assert_eq!(
        worker
            .reap_stale_jobs(Duration::from_secs(1800))
            .await
            .unwrap(),
        (1, 0)
    );
    let (logs, guard) = capture_logs();
    worker.run_claimed(job).await.unwrap();
    drop(guard);
    assert_eq!(job_row(&pool, id).await.0, "queued");
    assert!(
        logs.text().contains("requeued while it ran"),
        "{}",
        logs.text()
    );
    // The new run finishes it.
    assert!(worker.run_once().await.unwrap());
    assert_eq!(job_row(&pool, id).await, ("succeeded".into(), None));
}

/// A job that runs long (here a transcode held up at its save) keeps its lock fresh, so
/// the reaper doesn't take it for one whose worker died.
#[sqlx::test(migrator = "clipos_core::db::MIGRATOR")]
async fn a_long_job_keeps_its_lock(pool: PgPool) {
    if !azurite() {
        return;
    }
    let dir = tempfile::tempdir().unwrap();
    let source = dir.path().join("s.mp4");
    small_video(&source, 1);
    let worker = Arc::new(worker(pool.clone()));
    let clip = uploaded_clip(&worker, &source).await;
    // Held until the transcode wants to save it.
    let mut hold = pool.begin().await.unwrap();
    sqlx::query("SELECT 1 FROM clips WHERE id = $1 FOR UPDATE")
        .bind(clip.id)
        .execute(&mut *hold)
        .await
        .unwrap();
    let running = tokio::spawn({
        let worker = worker.clone();
        async move { worker.run_once().await }
    });
    until_waiting_on_a_lock(&pool).await;
    // As if it had been running for two hours: a few beats later, it's fresh again.
    go_stale(&pool).await;
    tokio::time::sleep(Duration::from_millis(300)).await;
    assert_eq!(
        worker
            .reap_stale_jobs(Duration::from_secs(1800))
            .await
            .unwrap(),
        (0, 0)
    );
    hold.rollback().await.unwrap();
    assert!(running.await.unwrap().unwrap());
    assert_ready(
        &clips::get(&pool, clip.id).await.unwrap().unwrap(),
        (320, 240),
    );
}

/// The beats only refresh the run's own lock: once the job was requeued (its run looked
/// hung), the old run's beats leave it alone. A beat that can't reach the database is
/// only logged.
#[sqlx::test(migrator = "clipos_core::db::MIGRATOR")]
async fn an_old_runs_heartbeat_leaves_a_requeued_job_alone(pool: PgPool) {
    let id = jobs::enqueue(&pool, "noop", json!({})).await.unwrap();
    let worker = worker(pool.clone());
    let job = jobs::claim(&pool, &worker.id).await.unwrap().unwrap();
    /// Beats for a while, then is stopped: it never ends on its own.
    async fn beat(worker: &Worker, job: &Job) {
        let beats = tokio::time::timeout(Duration::from_millis(200), worker.keep_locked(job));
        assert!(beats.await.is_err());
    }
    let reap = || worker.reap_stale_jobs(Duration::from_secs(1800));

    go_stale(&pool).await;
    beat(&worker, &job).await;
    assert_eq!(reap().await.unwrap(), (0, 0), "refreshed");
    go_stale(&pool).await;
    assert_eq!(reap().await.unwrap(), (1, 0));
    beat(&worker, &job).await;
    let unlocked: bool = sqlx::query_scalar(
        "SELECT status = 'queued' AND locked_at IS NULL FROM jobs WHERE id = $1",
    )
    .bind(id)
    .fetch_one(&pool)
    .await
    .unwrap();
    assert!(unlocked);

    pool.close().await;
    let (logs, guard) = capture_logs();
    beat(&worker, &job).await;
    drop(guard);
    assert!(
        logs.text().contains("couldn't refresh the job's lock"),
        "{}",
        logs.text()
    );
}

/// A transcode that keeps failing on the server's side tells the uploader, briefly.
#[sqlx::test(migrator = "clipos_core::db::MIGRATOR")]
async fn a_transcode_out_of_attempts_says_what_went_wrong(pool: PgPool) {
    // Nothing was uploaded (and without Azurite there's no storage at all): every try
    // fails to read the original.
    let clip = processing_clip(&pool).await;
    sqlx::query("UPDATE jobs SET attempts = $1 - 1")
        .bind(jobs::MAX_ATTEMPTS)
        .execute(&pool)
        .await
        .unwrap();
    let worker = worker(pool.clone());
    assert!(worker.run_once().await.unwrap());
    let failed = clips::get(&pool, clip).await.unwrap().unwrap();
    assert_failed(&failed, clips::FailureReason::Server);
    let error = failed.error.unwrap();
    let prefix = format!("processing failed after {} attempts (", jobs::MAX_ATTEMPTS);
    assert!(
        error.starts_with(&prefix) && error.ends_with(')'),
        "{error}"
    );
    assert!(!error.contains('\n'));
    assert!(
        error.chars().count() <= prefix.chars().count() + 201,
        "{error}"
    );
}

/// A transcode or keyframes job naming no clip fails for good, touching nothing; one for
/// a clip that's gone, or isn't in the state the job is for, has nothing to do.
#[sqlx::test(migrator = "clipos_core::db::MIGRATOR")]
async fn jobs_for_clips_they_dont_apply_to_do_nothing(pool: PgPool) {
    let worker = worker(pool.clone());
    for kind in [clips::TRANSCODE_JOB, clips::KEYFRAMES_JOB] {
        let id = jobs::enqueue(&pool, kind, json!({})).await.unwrap();
        assert!(worker.run_once().await.unwrap());
        let (status, error) = job_row(&pool, id).await;
        assert_eq!(status, "failed", "{kind}");
        assert!(error.unwrap().starts_with("bad payload"), "{kind}");
    }

    // A clip that's ready already: its transcode (a second run, say) leaves it be.
    let clip = processing_clip(&pool).await;
    let transcoded = clips::Transcoded {
        playback_blob: clips::playback_blob(clip),
        poster_blob: clips::poster_blob(clip),
        duration_ms: 1000,
        width: 640,
        height: 480,
        fps: 30.0,
        metadata: json!({}),
    };
    clips::set_ready(&pool, clip, &transcoded).await.unwrap();
    assert!(worker.run_once().await.unwrap());
    assert_eq!(
        jobs_for(&pool, clips::TRANSCODE_JOB, clip).await[0].0,
        "succeeded"
    );
    let after = clips::get(&pool, clip).await.unwrap().unwrap();
    assert_eq!((after.width, after.height), (Some(640), Some(480)));

    // Keyframes for a clip that's gone, or that isn't ready.
    let processing = processing_clip(&pool).await;
    sqlx::query("DELETE FROM jobs")
        .execute(&pool)
        .await
        .unwrap();
    for id in [Uuid::new_v4(), processing] {
        let job = queue_keyframes(&worker, id).await;
        assert!(worker.run_once().await.unwrap());
        assert_eq!(job_row(&pool, job).await, ("succeeded".into(), None));
    }
    let still = clips::get(&pool, processing).await.unwrap().unwrap();
    assert_eq!(still.status, ClipStatus::Processing);
}

/// A clip that never got its files (it failed, say) is purged all the same.
#[sqlx::test(migrator = "clipos_core::db::MIGRATOR")]
async fn purges_clips_that_never_got_their_files(pool: PgPool) {
    if !azurite() {
        return;
    }
    let worker = worker(pool.clone());
    worker.storage.prepare_local(&[]).await.unwrap();
    let clip = processing_clip(&pool).await;
    clips::soft_delete(&pool, clip).await.unwrap();
    sqlx::query("UPDATE clips SET deleted_at = now() - interval '8 days'")
        .execute(&pool)
        .await
        .unwrap();
    assert_eq!(worker.purge_trash().await.unwrap(), 1);
    assert!(
        clips::get_including_deleted(&pool, clip)
            .await
            .unwrap()
            .is_none()
    );
}

/// A ready 320×240 clip whose playback file has a keyframe only every 7 s, as the
/// keyframes job finds clips from before S5.
async fn clip_with_far_keyframes(worker: &Worker, dir: &Path) -> clips::Clip {
    let source = dir.join("original.mp4");
    small_video(&source, 2);
    let clip = processed(worker, &source).await;
    assert_ready(&clip, (320, 240));
    let old = dir.join("old.mp4");
    small_video_with(
        &old,
        7,
        &["-g", "300", "-keyint_min", "300", "-sc_threshold", "0"],
    );
    worker
        .storage
        .upload_file(
            Container::Playback,
            clip.playback_blob.as_deref().unwrap(),
            &old,
            "video/mp4",
        )
        .await
        .unwrap();
    clip
}

/// A re-encode that fails (here: the original was written again) leaves the clip as it
/// was and has the files it may have written deleted at once; if that can't be queued,
/// the failure is still the job's.
#[sqlx::test(migrator = "clipos_core::db::MIGRATOR")]
async fn a_failed_re_encode_keeps_the_clip_as_it_was(pool: PgPool) {
    if !azurite() {
        return;
    }
    let dir = tempfile::tempdir().unwrap();
    let worker = worker(pool.clone());
    let clip = clip_with_far_keyframes(&worker, dir.path()).await;
    let original = dir.path().join("original.mp4");
    worker
        .storage
        .upload_file(
            Container::Originals,
            &clip.original_blob,
            &original,
            "video/mp4",
        )
        .await
        .unwrap();

    let job = queue_keyframes(&worker, clip.id).await;
    assert!(worker.run_once().await.unwrap());
    let (status, error) = job_row(&pool, job).await;
    assert_eq!(status, "failed");
    assert_eq!(error.as_deref(), Some(clips::CHANGED_ERROR));
    let after = clips::get(&pool, clip.id).await.unwrap().unwrap();
    assert_eq!(after.playback_blob, clip.playback_blob);
    assert_eq!(after.status, ClipStatus::Ready);
    let deletes = jobs_for(&pool, clips::DELETE_BLOBS_JOB, clip.id).await;
    assert_eq!(deletes.len(), 1);
    assert!(deletes[0].1 <= 1.0, "runs at once: {deletes:?}");
    let blobs: Vec<String> = sqlx::query_scalar(
        "SELECT b->>'blob' FROM jobs, jsonb_array_elements(payload->'blobs') b
          WHERE kind = $1",
    )
    .bind(clips::DELETE_BLOBS_JOB)
    .fetch_all(&pool)
    .await
    .unwrap();
    assert_eq!(blobs.len(), 2);
    assert!(
        !blobs.contains(clip.playback_blob.as_ref().unwrap()),
        "{blobs:?}"
    );

    // (That delete is done; the next job to run is the next try.)
    sqlx::query("UPDATE jobs SET status = 'succeeded' WHERE kind = $1")
        .bind(clips::DELETE_BLOBS_JOB)
        .execute(&pool)
        .await
        .unwrap();
    refuse(&pool, "INSERT ON jobs", "NEW.kind = 'delete_blobs'").await;
    let job = queue_keyframes(&worker, clip.id).await;
    assert!(worker.run_once().await.unwrap());
    assert_eq!(job_row(&pool, job).await.0, "failed");
    assert_eq!(
        jobs_for(&pool, clips::DELETE_BLOBS_JOB, clip.id)
            .await
            .len(),
        1
    );
}

/// Waits until the keyframes job for `clip` is under way (its scratch dir exists).
async fn re_encode_started(worker: &Worker, clip: Uuid) {
    let dir = worker.transcode.temp_dir.join(format!("{clip}-rekey"));
    let deadline = std::time::Instant::now() + Duration::from_secs(30);
    while !dir.exists() {
        assert!(std::time::Instant::now() < deadline, "never started");
        tokio::time::sleep(Duration::from_millis(5)).await;
    }
}

/// Waits (at most 30 s) until another connection is blocked on a lock.
async fn until_waiting_on_a_lock(pool: &PgPool) {
    let deadline = std::time::Instant::now() + Duration::from_secs(30);
    loop {
        let waiting: i64 = sqlx::query_scalar(
            "SELECT count(*) FROM pg_stat_activity
              WHERE datname = current_database() AND wait_event_type = 'Lock'",
        )
        .fetch_one(pool)
        .await
        .unwrap();
        if waiting > 0 {
            return;
        }
        assert!(std::time::Instant::now() < deadline, "nothing waited");
        tokio::time::sleep(Duration::from_millis(10)).await;
    }
}

/// A clip purged while its re-encode ran: the new files have nothing pointing at them, so
/// they're deleted at once.
#[sqlx::test(migrator = "clipos_core::db::MIGRATOR")]
async fn a_re_encode_of_a_clip_purged_meanwhile_cleans_up(pool: PgPool) {
    if !azurite() {
        return;
    }
    let dir = tempfile::tempdir().unwrap();
    let worker = Arc::new(worker(pool.clone()));
    let clip = clip_with_far_keyframes(&worker, dir.path()).await;
    let job = queue_keyframes(&worker, clip.id).await;
    let running = tokio::spawn({
        let worker = worker.clone();
        async move { worker.run_once().await }
    });
    re_encode_started(&worker, clip.id).await;
    // The janitor's purge holds the row until the re-encode wants to save, then commits.
    let mut purge = pool.begin().await.unwrap();
    sqlx::query("DELETE FROM clips WHERE id = $1")
        .bind(clip.id)
        .execute(&mut *purge)
        .await
        .unwrap();
    until_waiting_on_a_lock(&pool).await;
    purge.commit().await.unwrap();
    assert!(running.await.unwrap().unwrap());

    assert_eq!(job_row(&pool, job).await, ("succeeded".into(), None));
    let deletes = jobs_for(&pool, clips::DELETE_BLOBS_JOB, clip.id).await;
    assert_eq!(deletes.len(), 1);
    assert!(deletes[0].1 <= 1.0, "runs at once: {deletes:?}");
    // The new files, not the ones the clip had.
    let blobs: Vec<String> = sqlx::query_scalar(
        "SELECT b->>'blob' FROM jobs, jsonb_array_elements(payload->'blobs') b
          WHERE kind = $1",
    )
    .bind(clips::DELETE_BLOBS_JOB)
    .fetch_all(&pool)
    .await
    .unwrap();
    assert_eq!(blobs.len(), 2);
    assert!(
        !blobs.contains(clip.playback_blob.as_ref().unwrap()),
        "{blobs:?}"
    );
    for blob in &blobs {
        let container = if blob.ends_with(".mp4") {
            Container::Playback
        } else {
            Container::Posters
        };
        assert!(
            exists(&worker, container, blob).await,
            "{blob} was uploaded"
        );
    }
}

/// A re-encode whose result can't be saved is retried; the files it wrote stay (the save
/// may have gone through after all), and the clip plays as before meanwhile.
#[sqlx::test(migrator = "clipos_core::db::MIGRATOR")]
async fn a_re_encode_that_cant_be_saved_is_retried(pool: PgPool) {
    if !azurite() {
        return;
    }
    let dir = tempfile::tempdir().unwrap();
    let worker = worker(pool.clone());
    let clip = clip_with_far_keyframes(&worker, dir.path()).await;
    refuse(
        &pool,
        "UPDATE ON clips",
        "OLD.playback_blob IS DISTINCT FROM NEW.playback_blob",
    )
    .await;
    let job = queue_keyframes(&worker, clip.id).await;
    assert!(worker.run_once().await.unwrap());
    let (status, error) = job_row(&pool, job).await;
    assert_eq!(status, "queued");
    assert!(error.unwrap().contains("refused by the test"));
    let after = clips::get(&pool, clip.id).await.unwrap().unwrap();
    assert_eq!(after.playback_blob, clip.playback_blob);
    assert!(
        jobs_for(&pool, clips::DELETE_BLOBS_JOB, clip.id)
            .await
            .is_empty()
    );
}

/// A name that isn't UTF-8 can't be a job's, so it's left alone. (Linux allows such
/// names; macOS refuses them, so there's nothing to check there.)
#[cfg(unix)]
#[test]
fn leaves_names_that_arent_utf8_alone() {
    use std::os::unix::ffi::OsStrExt;
    let dir = tempfile::tempdir().unwrap();
    let odd = dir
        .path()
        .join(std::ffi::OsStr::from_bytes(b"\xff\xfe-rekey"));
    if std::fs::create_dir(&odd).is_err() {
        eprintln!("this filesystem refuses names that aren't UTF-8; skipping");
        return;
    }
    assert_eq!(crate::clean_temp_dir(dir.path()).unwrap(), 0);
    assert!(odd.exists());
}

#[test]
fn cleans_up_after_jobs_cut_short() {
    let dir = tempfile::tempdir().unwrap();
    let id = Uuid::new_v4();
    for name in [
        id.to_string(),
        format!("{id}-rekey"),
        format!("analyse-{id}"),
    ] {
        std::fs::create_dir_all(dir.path().join(&name).join("sub")).unwrap();
        std::fs::write(dir.path().join(&name).join("playback.mp4"), b"half").unwrap();
    }
    // Not a job's: left alone.
    std::fs::create_dir(dir.path().join("keep-me")).unwrap();
    let file = Uuid::new_v4().to_string();
    std::fs::write(dir.path().join(&file), b"a file").unwrap();

    assert_eq!(crate::clean_temp_dir(dir.path()).unwrap(), 3);
    let mut kept: Vec<String> = std::fs::read_dir(dir.path())
        .unwrap()
        .map(|e| e.unwrap().file_name().into_string().unwrap())
        .collect();
    kept.sort();
    let mut want = vec!["keep-me".to_owned(), file];
    want.sort();
    assert_eq!(kept, want);
}
