//! One copy of a file (decision 55): the worker fingerprints every upload and refuses one
//! that's here already, and the `fingerprint` job fills in clips from before.

use clipos_core::dedup;

use super::*;

/// (file, bytes, content hash) of each of a clip's fingerprints.
async fn fingerprints(pool: &PgPool, clip: Uuid) -> Vec<(String, i64, Vec<u8>)> {
    sqlx::query_as(
        "SELECT file, bytes, content_hash FROM clip_fingerprints WHERE clip_id = $1 ORDER BY file",
    )
    .bind(clip)
    .fetch_all(pool)
    .await
    .unwrap()
}

/// The same file twice, from anyone, is kept once; so is the first one's playback file,
/// downloaded and uploaded again. Once the first is in the trash, the file is new again,
/// and the first can't come back.
#[sqlx::test(migrator = "clipos_core::db::MIGRATOR")]
async fn a_file_thats_here_already_fails_as_a_duplicate(pool: PgPool) {
    if !azurite() {
        return;
    }
    let dir = tempfile::tempdir().unwrap();
    let source = dir.path().join("s.mp4");
    small_video(&source, 1);
    let worker = worker(pool.clone());

    let first = processed(&worker, &source).await;
    assert_ready(&first, (320, 240));
    let kept = fingerprints(&pool, first.id).await;
    assert_eq!(
        kept.iter().map(|f| f.0.as_str()).collect::<Vec<_>>(),
        ["original", "playback"]
    );
    let source_bytes = std::fs::metadata(&source).unwrap().len() as i64;
    assert_eq!(kept[0].1, source_bytes);
    assert_eq!(first.duplicate_of, None);

    let again = processed(&worker, &source).await;
    assert_failed(&again, clips::FailureReason::Duplicate);
    assert_eq!(again.duplicate_of, Some(first.id));
    assert!(fingerprints(&pool, again.id).await.is_empty());

    let playback = dir.path().join("playback.mp4");
    worker
        .storage
        .download_to_file(
            Container::Playback,
            first.playback_blob.as_deref().unwrap(),
            &playback,
            None,
        )
        .await
        .unwrap();
    let downloaded = processed(&worker, &playback).await;
    assert_failed(&downloaded, clips::FailureReason::Duplicate);
    assert_eq!(downloaded.duplicate_of, Some(first.id));

    // Deleted, the first doesn't hold it any more.
    assert!(clips::soft_delete(&pool, first.id).await.unwrap());
    let new = processed(&worker, &source).await;
    assert_ready(&new, (320, 240));
    assert_eq!(
        clips::restore(&pool, first.id).await.unwrap(),
        clips::Restore::Duplicate(new.id)
    );
}

/// Read over HTTP (no room for a local copy), the original is fingerprinted as it streams,
/// and only if it's still the file `complete` accepted.
#[sqlx::test(migrator = "clipos_core::db::MIGRATOR")]
async fn fingerprints_an_original_read_over_http(pool: PgPool) {
    if !azurite() {
        return;
    }
    let dir = tempfile::tempdir().unwrap();
    let source = dir.path().join("s.mp4");
    small_video(&source, 1);
    let worker = worker(pool.clone());
    let clip = uploaded_clip(&worker, &source).await;

    let remote = transcode::remote_original(&worker, &clip).await.unwrap();
    transcode::claim_original(&worker, &clip, &remote)
        .await
        .unwrap();
    let local = dedup::of_file(&source).await.unwrap();
    assert_eq!(
        fingerprints(&pool, clip.id).await,
        [("original".into(), local.bytes, local.content_hash.to_vec())]
    );

    // Written again since: the clip fails for good.
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
    let result = transcode::claim_original(&worker, &clip, &remote).await;
    assert!(
        matches!(&result, Err(JobError::Permanent(e)) if e == clips::CHANGED_ERROR),
        "{result:?}"
    );
    // Another size than the one `complete` saw, likewise.
    let bigger = clips::Clip {
        original_etag: None,
        original_bytes: clip.original_bytes + 1,
        ..clip
    };
    let result = transcode::claim_original(&worker, &bigger, &remote).await;
    assert!(
        matches!(&result, Err(JobError::Permanent(e)) if e == clips::CHANGED_ERROR),
        "{result:?}"
    );
}

/// Clips from before duplicate detection get their fingerprints from the `fingerprint`
/// job, the same ones a transcode would have kept, in the trash too.
#[sqlx::test(migrator = "clipos_core::db::MIGRATOR")]
async fn the_fingerprint_job_fills_in_older_clips(pool: PgPool) {
    if !azurite() {
        return;
    }
    let dir = tempfile::tempdir().unwrap();
    let source = dir.path().join("s.mp4");
    small_video(&source, 1);
    let worker = worker(pool.clone());
    let clip = processed(&worker, &source).await;
    assert_ready(&clip, (320, 240));
    let kept = fingerprints(&pool, clip.id).await;
    assert_eq!(kept.len(), 2);
    assert!(clips::soft_delete(&pool, clip.id).await.unwrap());

    sqlx::query("DELETE FROM clip_fingerprints")
        .execute(&pool)
        .await
        .unwrap();
    let job = jobs::enqueue(&pool, clips::FINGERPRINT_JOB, json!({ "clipId": clip.id }))
        .await
        .unwrap();
    assert!(worker.run_once().await.unwrap());
    assert_eq!(job_row(&pool, job).await, ("succeeded".into(), None));
    assert_eq!(fingerprints(&pool, clip.id).await, kept);

    // Run again, it finds them done.
    jobs::enqueue(&pool, clips::FINGERPRINT_JOB, json!({ "clipId": clip.id }))
        .await
        .unwrap();
    assert!(worker.run_once().await.unwrap());
    assert_eq!(fingerprints(&pool, clip.id).await, kept);

    // A clip still processing is left to its transcode; one that's gone, to nobody.
    let processing = processing_clip(&pool).await;
    sqlx::query("DELETE FROM jobs")
        .execute(&pool)
        .await
        .unwrap();
    for id in [processing, Uuid::new_v4()] {
        let job = jobs::enqueue(&pool, clips::FINGERPRINT_JOB, json!({ "clipId": id }))
            .await
            .unwrap();
        assert!(worker.run_once().await.unwrap());
        assert_eq!(job_row(&pool, job).await, ("succeeded".into(), None));
        assert!(fingerprints(&pool, id).await.is_empty());
    }
    let job = jobs::enqueue(&pool, clips::FINGERPRINT_JOB, json!({}))
        .await
        .unwrap();
    assert!(worker.run_once().await.unwrap());
    let (status, error) = job_row(&pool, job).await;
    assert_eq!(status, "failed");
    assert!(error.unwrap().starts_with("bad payload"));
}

/// A clip whose original is missing is retried, not fingerprinted as something else.
#[sqlx::test(migrator = "clipos_core::db::MIGRATOR")]
async fn the_fingerprint_job_retries_a_missing_original(pool: PgPool) {
    if !azurite() {
        return;
    }
    let dir = tempfile::tempdir().unwrap();
    let source = dir.path().join("s.mp4");
    small_video(&source, 1);
    let worker = worker(pool.clone());
    let clip = processed(&worker, &source).await;
    sqlx::query("DELETE FROM clip_fingerprints")
        .execute(&pool)
        .await
        .unwrap();
    worker
        .storage
        .delete(Container::Originals, &clip.original_blob)
        .await
        .unwrap();
    let job = jobs::enqueue(&pool, clips::FINGERPRINT_JOB, json!({ "clipId": clip.id }))
        .await
        .unwrap();
    assert!(worker.run_once().await.unwrap());
    let (status, error) = job_row(&pool, job).await;
    assert_eq!(status, "queued", "retried");
    assert!(error.unwrap().contains("404"));
    assert!(fingerprints(&pool, clip.id).await.is_empty());
}
