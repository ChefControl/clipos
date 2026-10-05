//! Background job runner. Claims jobs from Postgres and dispatches them by `kind`.

use std::{convert::Infallible, sync::Arc, time::Duration};

use clipos_core::{
    clips,
    jobs::{self, Job},
    storage::{Container, Storage},
};
use serde::Deserialize;
use serde_json::{Value, json};
use sqlx::PgPool;
use tokio_util::sync::CancellationToken;
use tracing::Instrument;
use uuid::Uuid;

pub mod analyse;
pub mod config;
pub mod fingerprint;
mod service;
pub mod transcode;

pub use analyse::AnalyseConfig;
pub use service::run;
pub use transcode::TranscodeConfig;

pub struct Worker {
    pub pool: PgPool,
    pub storage: Arc<Storage>,
    pub transcode: TranscodeConfig,
    /// Killfeed analysis of CS2 clips; `None` turns it off (no jobs queued).
    pub analyse: Option<AnalyseConfig>,
    /// Identifies this instance in `jobs.locked_by`.
    pub id: String,
    /// How often a running job's lock (`jobs.locked_at`) is refreshed, so the reaper only
    /// requeues jobs whose worker stopped, never a long one still going. Well under
    /// `STALE_JOB_SECS`.
    pub heartbeat: Duration,
    /// Cancelled when the worker is asked to stop (SIGTERM on a deploy or restart).
    pub shutdown: CancellationToken,
    /// How long a running job may still take once `shutdown` is cancelled. A job that
    /// isn't done by then is handed back to the queue (`jobs::release`) for the next
    /// worker, rather than killed with the container and left for the reaper.
    pub shutdown_grace: Duration,
}

/// Why a job failed, which decides whether it's retried.
#[derive(Debug)]
pub enum JobError {
    /// The input is bad (no video, too long, …): fail now, with this message for the user.
    Permanent(String),
    /// Anything else (network, storage, a crash): retried with backoff.
    Retry(anyhow::Error),
}

impl<E: Into<anyhow::Error>> From<E> for JobError {
    fn from(e: E) -> Self {
        Self::Retry(e.into())
    }
}

impl Worker {
    /// Claims and runs at most one job. Returns whether a job was found, so the caller can
    /// decide whether to poll again immediately or sleep.
    pub async fn run_once(&self) -> anyhow::Result<bool> {
        let Some(job) = jobs::claim(&self.pool, &self.id).await? else {
            return Ok(false);
        };

        let span =
            tracing::info_span!("job", id = %job.id, kind = %job.kind, attempt = job.attempts);
        self.run_claimed(job).instrument(span).await?;
        Ok(true)
    }

    async fn run_claimed(&self, job: Job) -> anyhow::Result<()> {
        let started = std::time::Instant::now();
        let elapsed_ms = || started.elapsed().as_millis() as u64;
        // The lock is refreshed for as long as the job runs, and no longer.
        let outcome = tokio::select! {
            outcome = self.handle(&job) => outcome,
            never = self.keep_locked(&job) => match never {},
            () = self.out_of_time() => {
                // Dropping `handle` stops the job (ffmpeg is killed with it); it starts
                // over from scratch on the next worker, as every job can.
                if jobs::release(&self.pool, &job, &self.id).await? {
                    tracing::warn!(
                        elapsed_ms = elapsed_ms(),
                        "stopping before the job finished; handed it back to the queue"
                    );
                }
                return Ok(());
            }
        };
        let (error, permanent) = match outcome {
            Ok(()) => {
                if jobs::complete(&self.pool, &job, &self.id).await? {
                    tracing::info!(elapsed_ms = elapsed_ms(), "job succeeded");
                } else {
                    tracing::warn!(
                        elapsed_ms = elapsed_ms(),
                        "job succeeded, but it was requeued while it ran; leaving it to the new run"
                    );
                }
                return Ok(());
            }
            Err(JobError::Permanent(message)) => (message, true),
            Err(JobError::Retry(e)) => (format!("{e:#}"), false),
        };

        let failed = jobs::fail(&self.pool, &job, &self.id, &error, permanent).await?;
        tracing::warn!(%error, permanent, ?failed, "job failed");
        if failed == jobs::Failed::GaveUp {
            let message = if permanent {
                error
            } else {
                // Internal errors can be long and technical; keep the first line.
                let first = error.lines().next().unwrap_or(&error);
                let short: String = first.chars().take(200).collect();
                format!(
                    "processing failed after {} attempts ({short})",
                    job.attempts
                )
            };
            self.on_gave_up(&job, &message).await?;
        }
        Ok(())
    }

    /// Resolves `shutdown_grace` after `shutdown` is cancelled: when a running job has had
    /// its chance to finish.
    async fn out_of_time(&self) {
        self.shutdown.cancelled().await;
        tokio::time::sleep(self.shutdown_grace).await;
    }

    /// Refreshes the running `job`'s lock every `heartbeat`, until dropped; never returns.
    /// Only while the job is still this run's, like `jobs::complete`: once it was requeued
    /// (its run looked hung), the new run's lock is left alone.
    async fn keep_locked(&self, job: &Job) -> Infallible {
        let mut tick = tokio::time::interval(self.heartbeat);
        // The first tick is now, just after `claim` locked it.
        tick.tick().await;
        loop {
            tick.tick().await;
            let refreshed = sqlx::query(
                "UPDATE jobs SET locked_at = now()
                  WHERE id = $1 AND status = 'running' AND locked_by = $2 AND attempts = $3",
            )
            .bind(job.id)
            .bind(&self.id)
            .bind(job.attempts)
            .execute(&self.pool)
            .await;
            // A missed beat or two is fine: the reaper waits far longer.
            if let Err(e) = refreshed {
                tracing::warn!(error = %e, "couldn't refresh the job's lock");
            }
        }
    }

    /// Requeues jobs whose worker died mid-run (`jobs::requeue_stale`), and gives up on
    /// those that already used their attempts like on any other final failure. Returns how
    /// many were requeued and how many failed.
    pub async fn reap_stale_jobs(&self, older_than: Duration) -> anyhow::Result<(u64, usize)> {
        let stale = jobs::requeue_stale(&self.pool, older_than).await?;
        for job in &stale.failed {
            tracing::warn!(
                id = %job.id,
                kind = %job.kind,
                attempts = job.attempts,
                "job kept crashing its worker; gave up"
            );
            let message = format!("processing kept crashing after {} attempts", job.attempts);
            self.on_gave_up(job, &message).await?;
        }
        Ok((stale.requeued, stale.failed.len()))
    }

    async fn handle(&self, job: &Job) -> Result<(), JobError> {
        match job.kind.as_str() {
            clips::TRANSCODE_JOB => transcode::run(self, &job.payload).await,
            clips::ANALYSE_JOB => analyse::run(self, &job.payload).await,
            clips::KEYFRAMES_JOB => transcode::rekey(self, &job.payload).await,
            clips::FINGERPRINT_JOB => fingerprint::run(self, &job.payload).await,
            clips::DELETE_BLOBS_JOB => self.delete_blobs(&job.payload).await,
            // Smoke-test job used to verify the pipeline end to end.
            "noop" => Ok(()),
            // A job that never finishes, for the shutdown tests.
            #[cfg(test)]
            "hang" => std::future::pending().await,
            other => Err(anyhow::anyhow!("no handler for job kind {other:?}").into()),
        }
    }

    /// A job failed for good: surface it on the clip so the uploader sees why (`message`).
    async fn on_gave_up(&self, job: &Job, message: &str) -> anyhow::Result<()> {
        if job.kind != clips::TRANSCODE_JOB {
            return Ok(());
        }
        let Some(clip_id) = clip_id(&job.payload) else {
            return Ok(());
        };
        clips::set_failed(&self.pool, clip_id, message).await?;
        Ok(())
    }

    /// Deletes uploads abandoned for longer than `older_than` (their original blob, if
    /// any, then the row). Returns how many were removed.
    pub async fn clean_abandoned_uploads(&self, older_than: Duration) -> anyhow::Result<usize> {
        let stale = clips::abandoned_uploads(&self.pool, older_than).await?;
        for (id, blob) in &stale {
            self.storage.delete(Container::Originals, blob).await?;
            clips::purge(&self.pool, *id).await?;
        }
        Ok(stale.len())
    }
}

impl Worker {
    /// Permanently removes clips that have been in the trash for `clips::TRASH_DAYS`:
    /// their original, playback and poster blobs, then the row. Returns how many.
    pub async fn purge_trash(&self) -> anyhow::Result<usize> {
        let expired = clips::expired_trash(&self.pool).await?;
        for clip in &expired {
            self.storage
                .delete(Container::Originals, &clip.original_blob)
                .await?;
            if let Some(blob) = &clip.playback_blob {
                self.storage.delete(Container::Playback, blob).await?;
            }
            if let Some(blob) = &clip.poster_blob {
                self.storage.delete(Container::Posters, blob).await?;
            }
            // The first transcode's names too, in case a re-encode moved the clip to new
            // ones but couldn't delete these. Clips transcoded before S3 have no teaser.
            // Deleting a missing blob is fine.
            self.storage
                .delete(Container::Playback, &clips::playback_blob(clip.id))
                .await?;
            self.storage
                .delete(Container::Posters, &clips::poster_blob(clip.id))
                .await?;
            self.storage
                .delete(Container::Posters, &clips::teaser_blob(clip.id))
                .await?;
            clips::purge(&self.pool, clip.id).await?;
        }
        Ok(expired.len())
    }
}

#[derive(Debug, Deserialize)]
struct DeleteBlobs {
    blobs: Vec<BlobRef>,
}

#[derive(Debug, Deserialize)]
struct BlobRef {
    container: String,
    blob: String,
}

impl Worker {
    /// Queues a background `delete_blobs` job for blobs no clip points at any more, to run
    /// after `delay`.
    pub(crate) async fn queue_delete_blobs(
        &self,
        blobs: &[(Container, String)],
        delay: Duration,
    ) -> sqlx::Result<Uuid> {
        let blobs: Vec<Value> = blobs
            .iter()
            .map(|(container, blob)| json!({ "container": container.as_str(), "blob": blob }))
            .collect();
        jobs::enqueue_later(
            &self.pool,
            clips::DELETE_BLOBS_JOB,
            json!({ "blobs": blobs }),
            jobs::PRIORITY_BACKGROUND,
            delay,
        )
        .await
    }

    /// The `delete_blobs` job. Already gone is fine, so a retry after a partial run just
    /// carries on; any other failure is retried through the queue.
    async fn delete_blobs(&self, payload: &Value) -> Result<(), JobError> {
        let DeleteBlobs { blobs } = serde_json::from_value(payload.clone())
            .map_err(|e| JobError::Permanent(format!("bad payload: {e}")))?;
        for BlobRef { container, blob } in blobs {
            let container = Container::from_name(&container)
                .ok_or_else(|| JobError::Permanent(format!("no container {container:?}")))?;
            self.storage.delete(container, &blob).await?;
        }
        Ok(())
    }
}

fn clip_id(payload: &Value) -> Option<Uuid> {
    payload.get("clipId")?.as_str()?.parse().ok()
}

/// Removes the per-clip scratch dirs jobs leave in `dir` (`{id}`, `{id}-rekey`,
/// `analyse-{id}`) when the worker stops mid-job. Run at startup, before any job, so none
/// is in use. Anything else in `dir` is left alone. Returns how many were removed.
pub fn clean_temp_dir(dir: &std::path::Path) -> std::io::Result<usize> {
    let mut removed = 0;
    for entry in std::fs::read_dir(dir)? {
        let entry = entry?;
        let name = entry.file_name();
        let Some(name) = name.to_str() else {
            continue;
        };
        let id = name.strip_prefix("analyse-").unwrap_or(name);
        let id = id.strip_suffix("-rekey").unwrap_or(id);
        if entry.file_type()?.is_dir() && id.parse::<Uuid>().is_ok() {
            std::fs::remove_dir_all(entry.path())?;
            removed += 1;
        }
    }
    Ok(removed)
}

#[cfg(test)]
mod tests;
