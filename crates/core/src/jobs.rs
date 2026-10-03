//! Postgres-backed job queue. Workers claim with `FOR UPDATE SKIP LOCKED`, so any number
//! of worker instances can poll the same table without double-processing a job.

use std::time::Duration;

use serde_json::Value;
use sqlx::PgPool;
use uuid::Uuid;

/// Attempts before a job is marked `failed` for good.
pub const MAX_ATTEMPTS: i32 = 3;

/// Job priorities (`jobs.priority`): `claim` takes the highest first, so lower means less
/// urgent. Someone is waiting on these (an upload's transcode); the column's default.
pub const PRIORITY_NORMAL: i16 = 0;
/// Nobody is waiting on these (backfills over the whole library): they only run when no
/// normal job is ready, so new uploads never queue behind them.
pub const PRIORITY_BACKGROUND: i16 = -10;

#[derive(Debug, Clone, sqlx::FromRow)]
pub struct Job {
    pub id: Uuid,
    pub kind: String,
    pub payload: Value,
    pub attempts: i32,
}

pub async fn enqueue(pool: &PgPool, kind: &str, payload: Value) -> sqlx::Result<Uuid> {
    enqueue_with_priority(pool, kind, payload, PRIORITY_NORMAL).await
}

/// Like `enqueue`, at another priority (e.g. `PRIORITY_BACKGROUND`).
pub async fn enqueue_with_priority(
    pool: &PgPool,
    kind: &str,
    payload: Value,
    priority: i16,
) -> sqlx::Result<Uuid> {
    enqueue_later(pool, kind, payload, priority, Duration::ZERO).await
}

/// Like `enqueue_with_priority`, for a job that mustn't run before `delay` has passed.
pub async fn enqueue_later(
    pool: &PgPool,
    kind: &str,
    payload: Value,
    priority: i16,
    delay: Duration,
) -> sqlx::Result<Uuid> {
    sqlx::query_scalar(
        "INSERT INTO jobs (kind, payload, priority, run_after)
         VALUES ($1, $2, $3, now() + make_interval(secs => $4)) RETURNING id",
    )
    .bind(kind)
    .bind(payload)
    .bind(priority)
    .bind(delay.as_secs_f64())
    .fetch_one(pool)
    .await
}

/// Enqueues inside an existing transaction, so the job only exists if the transaction
/// commits (e.g. together with the state change that needs it).
pub async fn enqueue_in(
    tx: &mut sqlx::PgTransaction<'_>,
    kind: &str,
    payload: Value,
) -> sqlx::Result<Uuid> {
    sqlx::query_scalar("INSERT INTO jobs (kind, payload) VALUES ($1, $2) RETURNING id")
        .bind(kind)
        .bind(payload)
        .fetch_one(&mut **tx)
        .await
}

/// Claims the most urgent runnable job (highest priority, then the oldest), if any.
pub async fn claim(pool: &PgPool, worker_id: &str) -> sqlx::Result<Option<Job>> {
    sqlx::query_as(
        "UPDATE jobs
            SET status = 'running', attempts = attempts + 1, locked_at = now(),
                locked_by = $1, updated_at = now()
          WHERE id = (
                SELECT id FROM jobs
                 WHERE status = 'queued' AND run_after <= now()
                 ORDER BY priority DESC, run_after, created_at
                 FOR UPDATE SKIP LOCKED
                 LIMIT 1)
      RETURNING id, kind, payload, attempts",
    )
    .bind(worker_id)
    .fetch_optional(pool)
    .await
}

/// Marks a run as succeeded. Only the run that holds the job can: a run the reaper gave up
/// on (its worker hung) may finish after the job was requeued and run again, and its late
/// result must not overwrite that. Returns false when the job wasn't this run's any more.
pub async fn complete(pool: &PgPool, job: &Job, worker_id: &str) -> sqlx::Result<bool> {
    Ok(sqlx::query(
        "UPDATE jobs SET status = 'succeeded', locked_at = NULL, locked_by = NULL,
                last_error = NULL, updated_at = now()
          WHERE id = $1 AND status = 'running' AND locked_by = $2 AND attempts = $3",
    )
    .bind(job.id)
    .bind(worker_id)
    .bind(job.attempts)
    .execute(pool)
    .await?
    .rows_affected()
        == 1)
}

/// What `fail` did with a job.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Failed {
    /// Back in the queue, to run again after the backoff.
    Retrying,
    /// Marked `failed` for good.
    GaveUp,
    /// The job wasn't this run's any more (requeued while it ran): nothing changed.
    Lost,
}

/// Records a failed run. The job is retried with exponential backoff (30s, 60s, … capped
/// at 1h) until it has been attempted `MAX_ATTEMPTS` times, then marked `failed`. With
/// `permanent`, it's marked `failed` right away (retrying can't help, e.g. a bad input).
/// Like `complete`, only the run that holds the job can.
pub async fn fail(
    pool: &PgPool,
    job: &Job,
    worker_id: &str,
    error: &str,
    permanent: bool,
) -> sqlx::Result<Failed> {
    let gave_up: Option<bool> = sqlx::query_scalar(
        "UPDATE jobs
            SET status = CASE WHEN $4 OR attempts >= $3 THEN 'failed' ELSE 'queued' END,
                run_after = now() + make_interval(secs => least(power(2, attempts - 1) * 30, 3600)),
                last_error = $2, locked_at = NULL, locked_by = NULL, updated_at = now()
          WHERE id = $1 AND status = 'running' AND locked_by = $5 AND attempts = $6
      RETURNING status = 'failed'",
    )
    .bind(job.id)
    .bind(error)
    .bind(MAX_ATTEMPTS)
    .bind(permanent)
    .bind(worker_id)
    .bind(job.attempts)
    .fetch_optional(pool)
    .await?;
    Ok(match gave_up {
        Some(true) => Failed::GaveUp,
        Some(false) => Failed::Retrying,
        None => Failed::Lost,
    })
}

/// `last_error` of a job given up on because its worker never came back.
pub const STALE_ERROR: &str = "the worker died or hung on every attempt";

/// Jobs `requeue_stale` found orphaned.
#[derive(Debug, Default)]
pub struct Stale {
    /// How many went back in the queue.
    pub requeued: u64,
    /// Out of attempts, so marked `failed`: the caller gives up on them like on any other
    /// final failure.
    pub failed: Vec<Job>,
}

/// Puts jobs whose worker died mid-run back in the queue. A job that already used its
/// `MAX_ATTEMPTS` is failed instead: one that kills or hangs its worker every time (a
/// pathological file) would otherwise come back forever.
pub async fn requeue_stale(pool: &PgPool, older_than: Duration) -> sqlx::Result<Stale> {
    let rows: Vec<(Uuid, String, Value, i32, bool)> = sqlx::query_as(
        "UPDATE jobs
            SET status = CASE WHEN attempts >= $2 THEN 'failed' ELSE 'queued' END,
                last_error = CASE WHEN attempts >= $2 THEN $3 ELSE last_error END,
                locked_at = NULL, locked_by = NULL, updated_at = now()
          WHERE status = 'running' AND locked_at < now() - make_interval(secs => $1)
      RETURNING id, kind, payload, attempts, status = 'failed'",
    )
    .bind(older_than.as_secs_f64())
    .bind(MAX_ATTEMPTS)
    .bind(STALE_ERROR)
    .fetch_all(pool)
    .await?;
    let mut stale = Stale::default();
    for (id, kind, payload, attempts, failed) in rows {
        if failed {
            stale.failed.push(Job {
                id,
                kind,
                payload,
                attempts,
            });
        } else {
            stale.requeued += 1;
        }
    }
    Ok(stale)
}

#[cfg(test)]
mod tests {
    use serde_json::json;

    use super::*;

    async fn status(pool: &PgPool, id: Uuid) -> (String, i32, Option<String>) {
        sqlx::query_as("SELECT status, attempts, last_error FROM jobs WHERE id = $1")
            .bind(id)
            .fetch_one(pool)
            .await
            .unwrap()
    }

    #[sqlx::test(migrator = "crate::db::MIGRATOR")]
    async fn claim_complete_roundtrip(pool: PgPool) {
        assert!(claim(&pool, "w1").await.unwrap().is_none());

        let id = enqueue(&pool, "noop", json!({ "n": 1 })).await.unwrap();
        let job = claim(&pool, "w1").await.unwrap().expect("job");
        assert_eq!((job.id, job.kind.as_str(), job.attempts), (id, "noop", 1));
        assert_eq!(job.payload, json!({ "n": 1 }));
        assert!(
            claim(&pool, "w2").await.unwrap().is_none(),
            "already claimed"
        );

        assert!(complete(&pool, &job, "w1").await.unwrap());
        assert_eq!(status(&pool, id).await.0, "succeeded");
    }

    #[sqlx::test(migrator = "crate::db::MIGRATOR")]
    async fn failures_back_off_then_give_up(pool: PgPool) {
        let id = enqueue(&pool, "noop", json!({})).await.unwrap();

        let job = claim(&pool, "w1").await.unwrap().unwrap();
        assert_eq!(
            fail(&pool, &job, "w1", "boom", false).await.unwrap(),
            Failed::Retrying
        );
        assert_eq!(
            status(&pool, id).await,
            ("queued".into(), 1, Some("boom".into()))
        );
        assert!(
            claim(&pool, "w1").await.unwrap().is_none(),
            "backoff delays the retry"
        );

        // Skip the backoff and exhaust the remaining attempts.
        for _ in 1..MAX_ATTEMPTS {
            sqlx::query("UPDATE jobs SET run_after = now() WHERE id = $1")
                .bind(id)
                .execute(&pool)
                .await
                .unwrap();
            let job = claim(&pool, "w1").await.unwrap().unwrap();
            fail(&pool, &job, "w1", "boom", false).await.unwrap();
        }
        assert_eq!(status(&pool, id).await.0, "failed");
    }

    #[sqlx::test(migrator = "crate::db::MIGRATOR")]
    async fn permanent_failures_are_not_retried(pool: PgPool) {
        let id = enqueue(&pool, "noop", json!({})).await.unwrap();
        let job = claim(&pool, "w1").await.unwrap().unwrap();
        assert_eq!(
            fail(&pool, &job, "w1", "bad input", true).await.unwrap(),
            Failed::GaveUp
        );
        assert_eq!(status(&pool, id).await.0, "failed");
    }

    #[sqlx::test(migrator = "crate::db::MIGRATOR")]
    async fn concurrent_claims_never_share_a_job(pool: PgPool) {
        for _ in 0..20 {
            enqueue(&pool, "noop", json!({})).await.unwrap();
        }
        let workers = (0..8).map(|w| {
            let pool = pool.clone();
            tokio::spawn(async move {
                let mut got = Vec::new();
                while let Some(job) = claim(&pool, &format!("w{w}")).await.unwrap() {
                    got.push(job.id);
                }
                got
            })
        });
        let mut all: Vec<Uuid> = Vec::new();
        for w in workers {
            all.extend(w.await.unwrap());
        }
        let total = all.len();
        all.sort();
        all.dedup();
        assert_eq!((total, all.len()), (20, 20));
    }

    /// A backfill queues a job per clip at once; an upload's transcode queued after them
    /// still runs first.
    #[sqlx::test(migrator = "crate::db::MIGRATOR")]
    async fn uploads_skip_ahead_of_background_jobs(pool: PgPool) {
        for _ in 0..5 {
            enqueue_with_priority(&pool, "keyframes", json!({}), PRIORITY_BACKGROUND)
                .await
                .unwrap();
        }
        let transcode = enqueue(&pool, "transcode", json!({})).await.unwrap();
        assert_eq!(claim(&pool, "w1").await.unwrap().unwrap().id, transcode);
        // Then the backlog.
        assert_eq!(claim(&pool, "w1").await.unwrap().unwrap().kind, "keyframes");
    }

    /// Migration 0013 moves the keyframes jobs 0011 queued to the background, and running
    /// it again changes nothing.
    #[sqlx::test(migrator = "crate::db::MIGRATOR")]
    async fn migration_moves_queued_keyframes_jobs_to_the_background(pool: PgPool) {
        let keyframes = enqueue(&pool, "keyframes", json!({})).await.unwrap();
        let transcode = enqueue(&pool, "transcode", json!({})).await.unwrap();
        for _ in 0..2 {
            sqlx::raw_sql(include_str!("../../../migrations/0013_job_priority.sql"))
                .execute(&pool)
                .await
                .unwrap();
        }
        let priority = |id| {
            sqlx::query_scalar::<_, i16>("SELECT priority FROM jobs WHERE id = $1")
                .bind(id)
                .fetch_one(&pool)
        };
        assert_eq!(priority(keyframes).await.unwrap(), PRIORITY_BACKGROUND);
        assert_eq!(priority(transcode).await.unwrap(), PRIORITY_NORMAL);
    }

    /// Makes the job look orphaned: its worker locked it two hours ago and never came back.
    async fn go_stale(pool: &PgPool, id: Uuid) {
        sqlx::query("UPDATE jobs SET locked_at = now() - interval '2 hours' WHERE id = $1")
            .bind(id)
            .execute(pool)
            .await
            .unwrap();
    }

    async fn reap(pool: &PgPool) -> Stale {
        requeue_stale(pool, Duration::from_secs(1800))
            .await
            .unwrap()
    }

    #[sqlx::test(migrator = "crate::db::MIGRATOR")]
    async fn stale_running_jobs_are_requeued(pool: PgPool) {
        let id = enqueue(&pool, "noop", json!({})).await.unwrap();
        claim(&pool, "dead-worker").await.unwrap().unwrap();
        go_stale(&pool, id).await;

        let stale = reap(&pool).await;
        assert_eq!((stale.requeued, stale.failed.len()), (1, 0));
        assert_eq!(status(&pool, id).await.0, "queued");
    }

    /// A job that kills its worker every time is given up on once it has used its
    /// attempts, not requeued forever.
    #[sqlx::test(migrator = "crate::db::MIGRATOR")]
    async fn a_job_that_keeps_crashing_its_worker_fails(pool: PgPool) {
        let payload = json!({ "clipId": Uuid::new_v4() });
        let id = enqueue(&pool, "transcode", payload.clone()).await.unwrap();
        for attempt in 1..MAX_ATTEMPTS {
            assert_eq!(claim(&pool, "w1").await.unwrap().unwrap().attempts, attempt);
            go_stale(&pool, id).await;
            let stale = reap(&pool).await;
            assert_eq!((stale.requeued, stale.failed.len()), (1, 0));
        }
        claim(&pool, "w1").await.unwrap().unwrap();
        go_stale(&pool, id).await;
        let stale = reap(&pool).await;
        assert_eq!(stale.requeued, 0);
        let [failed] = &stale.failed[..] else {
            panic!("{stale:?}");
        };
        assert_eq!((failed.id, failed.kind.as_str()), (id, "transcode"));
        assert_eq!((&failed.payload, failed.attempts), (&payload, MAX_ATTEMPTS));
        assert_eq!(
            status(&pool, id).await,
            ("failed".into(), MAX_ATTEMPTS, Some(STALE_ERROR.into()))
        );
        assert!(claim(&pool, "w1").await.unwrap().is_none());
    }

    /// A run that hung, was requeued and ran again can't change the outcome when it finally
    /// returns, whether it failed or succeeded.
    #[sqlx::test(migrator = "crate::db::MIGRATOR")]
    async fn a_late_result_from_a_requeued_run_is_dropped(pool: PgPool) {
        let id = enqueue(&pool, "noop", json!({})).await.unwrap();
        let hung = claim(&pool, "a").await.unwrap().unwrap();
        go_stale(&pool, id).await;
        reap(&pool).await;

        // Back in the queue, not yet run again.
        assert_eq!(
            fail(&pool, &hung, "a", "boom", true).await.unwrap(),
            Failed::Lost
        );
        assert!(!complete(&pool, &hung, "a").await.unwrap());
        assert_eq!(status(&pool, id).await.0, "queued");

        // Running on another worker, then done there.
        let rerun = claim(&pool, "b").await.unwrap().unwrap();
        assert!(!complete(&pool, &hung, "a").await.unwrap());
        assert!(complete(&pool, &rerun, "b").await.unwrap());
        assert_eq!(
            fail(&pool, &hung, "a", "boom", false).await.unwrap(),
            Failed::Lost
        );
        assert_eq!(status(&pool, id).await, ("succeeded".into(), 2, None));

        // The same worker id on a later attempt (an instance restarted under the same name)
        // is another run.
        let id = enqueue(&pool, "noop", json!({})).await.unwrap();
        let first = claim(&pool, "a").await.unwrap().unwrap();
        go_stale(&pool, id).await;
        reap(&pool).await;
        let second = claim(&pool, "a").await.unwrap().unwrap();
        assert!(!complete(&pool, &first, "a").await.unwrap());
        assert_eq!(status(&pool, id).await.0, "running");
        assert!(complete(&pool, &second, "a").await.unwrap());
    }
}
