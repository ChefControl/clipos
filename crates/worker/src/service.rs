//! Startup and the main loop: everything `clipos-worker` does between reading its config
//! and stopping, so the binary is only config, logging and signal handling, and tests can
//! run the real thing.

use std::{future::Future, net::SocketAddr, sync::Arc, time::Duration};

use anyhow::Context;
use axum::{Router, extract::State, http::StatusCode, response::IntoResponse, routing::get};
use clipos_core::{
    db,
    storage::{Storage, StorageConfig},
};
use sqlx::PgPool;
use tokio_util::sync::CancellationToken;

use crate::{AnalyseConfig, TranscodeConfig, Worker, config::Config};

/// How often to check whether the api has applied the migrations this build expects.
const SCHEMA_POLL: Duration = Duration::from_secs(5);
const REAP_EVERY: Duration = Duration::from_secs(60);
const JANITOR_EVERY: Duration = Duration::from_secs(3600);

/// Serves `/healthz`, waits for the schema, then runs jobs until `shutdown` resolves.
/// `shutdown` is first polled once connected to Postgres (the binary's signal handlers
/// are installed then). A job in progress gets `SHUTDOWN_GRACE_SECS` to finish; one that
/// doesn't is handed back to the queue for the next worker.
pub async fn run(
    config: Config,
    shutdown: impl Future<Output = ()> + Send + 'static,
) -> anyhow::Result<()> {
    let worker_id = config
        .worker_id
        .clone()
        .unwrap_or_else(|| format!("worker-{}", &uuid::Uuid::new_v4().simple().to_string()[..8]));
    let pool = db::connect(
        &config.database_url,
        config.db_max_connections,
        config.database_auth,
    )
    .await?;
    let shutdown = {
        let token = CancellationToken::new();
        let cancel = token.clone();
        tokio::spawn(async move {
            shutdown.await;
            tracing::info!("shutdown requested; giving the current job its grace period");
            cancel.cancel();
        });
        token
    };

    let health = tokio::spawn(serve_health(
        config.bind_addr,
        pool.clone(),
        shutdown.clone(),
    ));

    // The api owns migrations; never process jobs against an older schema.
    if !wait_for_schema(&pool, SCHEMA_POLL, &shutdown).await? {
        return Ok(());
    }

    let storage = Storage::new(StorageConfig {
        account: config.storage_account.clone(),
        blob_endpoint: config.storage_blob_endpoint.clone(),
        account_key: config.storage_account_key.clone(),
    })?;
    let worker = Arc::new(Worker {
        pool: pool.clone(),
        storage: Arc::new(storage),
        transcode: TranscodeConfig {
            temp_dir: config.transcode_dir.clone(),
            threads: config.ffmpeg_threads,
            preset: config.x264_preset.clone(),
            crf: config.x264_crf,
        },
        analyse: config.killfeed_analyse.then(|| AnalyseConfig {
            models_dir: config.killfeed_models_dir.clone(),
            rows_model: config.killfeed_rows_model.clone(),
            icons_model: config.killfeed_icons_model.clone(),
            threads: config.killfeed_threads,
        }),
        id: worker_id.clone(),
        // A tenth of what the reaper waits, so a few missed beats never look like a dead
        // worker.
        heartbeat: Duration::from_secs((config.stale_job_secs / 10).max(1)),
        shutdown: shutdown.clone(),
        shutdown_grace: Duration::from_secs(config.shutdown_grace_secs),
    });
    tokio::spawn(reap_stale_jobs(
        worker.clone(),
        Duration::from_secs(config.stale_job_secs),
        REAP_EVERY,
        shutdown.clone(),
    ));
    tokio::spawn(janitor(
        worker.clone(),
        Duration::from_secs(config.abandoned_upload_secs),
        JANITOR_EVERY,
        shutdown.clone(),
    ));

    std::fs::create_dir_all(&config.transcode_dir)
        .with_context(|| format!("creating {}", config.transcode_dir.display()))?;
    // A job that was running when the worker last stopped (killed, crashed, deployed over)
    // never cleaned up its scratch files, and a few of those fill the temp disk.
    match crate::clean_temp_dir(&config.transcode_dir) {
        Ok(0) => {}
        Ok(n) => tracing::info!(count = n, "removed leftover temp dirs"),
        Err(e) => tracing::warn!(error = %e, "couldn't clean the temp dir"),
    }
    // Measured first: an await inside `info!` would make this future `!Send`.
    let temp_free_bytes = crate::transcode::free_bytes(&config.transcode_dir).await;
    tracing::info!(
        %worker_id,
        version = clipos_core::version(),
        temp_dir = %config.transcode_dir.display(),
        temp_free_bytes,
        "worker started"
    );
    run_jobs(
        &worker,
        Duration::from_millis(config.poll_interval_ms),
        &shutdown,
    )
    .await;

    health.await??;
    tracing::info!("worker stopped");
    Ok(())
}

/// Polls until every migration this build knows is applied. False if `shutdown` came
/// first.
async fn wait_for_schema(
    pool: &PgPool,
    every: Duration,
    shutdown: &CancellationToken,
) -> anyhow::Result<bool> {
    while !db::schema_is_current(pool).await? {
        tracing::info!("waiting for the api to apply migrations");
        tokio::select! {
            _ = tokio::time::sleep(every) => {},
            _ = shutdown.cancelled() => return Ok(false),
        }
    }
    Ok(true)
}

/// Runs jobs back to back while there are any, sleeping `poll` when the queue is empty,
/// until `shutdown`. A failed claim is logged and retried after `poll`.
async fn run_jobs(worker: &Worker, poll: Duration, shutdown: &CancellationToken) {
    while !shutdown.is_cancelled() {
        match worker.run_once().await {
            Ok(true) => continue,
            Ok(false) => {}
            Err(e) => tracing::error!(error = %format!("{e:#}"), "job loop error"),
        }
        tokio::select! {
            _ = tokio::time::sleep(poll) => {},
            _ = shutdown.cancelled() => {},
        }
    }
}

async fn serve_health(
    addr: SocketAddr,
    pool: PgPool,
    shutdown: CancellationToken,
) -> anyhow::Result<()> {
    let app = Router::new()
        .route("/healthz", get(healthz))
        .with_state(pool);
    let listener = tokio::net::TcpListener::bind(addr)
        .await
        .with_context(|| format!("binding {addr}"))?;
    axum::serve(listener, app)
        .with_graceful_shutdown(async move { shutdown.cancelled().await })
        .await?;
    Ok(())
}

async fn healthz(State(pool): State<PgPool>) -> impl IntoResponse {
    let version = [("x-clipos-version", clipos_core::version())];
    match db::ping(&pool).await {
        Ok(()) => (StatusCode::OK, version, "ok"),
        Err(_) => (
            StatusCode::SERVICE_UNAVAILABLE,
            version,
            "database unavailable",
        ),
    }
}

/// Every `every` (and once at startup): requeues jobs whose worker died.
async fn reap_stale_jobs(
    worker: Arc<Worker>,
    older_than: Duration,
    every: Duration,
    shutdown: CancellationToken,
) {
    let mut tick = tokio::time::interval(every);
    loop {
        tokio::select! {
            _ = tick.tick() => {},
            _ = shutdown.cancelled() => return,
        }
        match worker.reap_stale_jobs(older_than).await {
            Ok((0, 0)) => {}
            Ok((requeued, failed)) => tracing::warn!(requeued, failed, "reaped stale jobs"),
            Err(e) => tracing::error!(error = %format!("{e:#}"), "requeueing stale jobs failed"),
        }
    }
}

/// Every `every` (hourly, and once at startup): deletes uploads that were never
/// completed, and clips past their 7 days in the trash.
async fn janitor(
    worker: Arc<Worker>,
    older_than: Duration,
    every: Duration,
    shutdown: CancellationToken,
) {
    let mut tick = tokio::time::interval(every);
    loop {
        tokio::select! {
            _ = tick.tick() => {},
            _ = shutdown.cancelled() => return,
        }
        match worker.clean_abandoned_uploads(older_than).await {
            Ok(0) => {}
            Ok(n) => tracing::info!(count = n, "removed abandoned uploads"),
            Err(e) => tracing::error!(error = %format!("{e:#}"), "janitor failed"),
        }
        match worker.purge_trash().await {
            Ok(0) => {}
            Ok(n) => tracing::info!(count = n, "purged deleted clips"),
            Err(e) => tracing::error!(error = %format!("{e:#}"), "purging the trash failed"),
        }
    }
}

#[cfg(test)]
mod tests {
    use std::path::Path;

    use clipos_core::{
        clips::{self, NewClip},
        db::DbAuth,
        jobs,
        telemetry::LogFormat,
        users::{self, Identity, SignIn},
    };
    use serde_json::json;
    use sqlx::postgres::PgPoolOptions;
    use uuid::Uuid;

    use super::*;

    /// Azurite's well-known development account (public, not a secret).
    const AZURITE_KEY: &str =
        "Eby8vdM02xNOcqFlqUwJPLlmEtlCDXJ1OUzFT50uSRZ6IFsuFq2UVErCz4I6tq/K1SZFPTOtr/KBHBeksoGMGw==";

    /// Tests that touch blobs need Azurite: set `CLIPOS_AZURITE` (CI and `make test`).
    fn azurite() -> bool {
        let on = std::env::var_os("CLIPOS_AZURITE").is_some();
        if !on {
            eprintln!("CLIPOS_AZURITE not set; skipping");
        }
        on
    }

    /// The URL of the test database behind `pool`, on `DATABASE_URL`'s server.
    async fn database_url(pool: &PgPool) -> String {
        let name: String = sqlx::query_scalar("SELECT current_database()::text")
            .fetch_one(pool)
            .await
            .unwrap();
        let server = std::env::var("DATABASE_URL").expect("DATABASE_URL");
        let (server, _) = server.rsplit_once('/').unwrap();
        format!("{server}/{name}")
    }

    /// A port nothing listens on right now.
    fn free_port() -> SocketAddr {
        std::net::TcpListener::bind("127.0.0.1:0")
            .unwrap()
            .local_addr()
            .unwrap()
    }

    /// A config like the one the binary parses from the environment, polling quickly.
    fn config(database_url: String, transcode_dir: &Path) -> Config {
        Config {
            database_url,
            database_auth: DbAuth::Password,
            db_max_connections: 4,
            bind_addr: free_port(),
            poll_interval_ms: 20,
            stale_job_secs: 1800,
            shutdown_grace_secs: 90,
            storage_account: "devstoreaccount1".into(),
            storage_blob_endpoint: Some("http://127.0.0.1:10000/devstoreaccount1".into()),
            storage_account_key: Some(AZURITE_KEY.into()),
            transcode_dir: transcode_dir.to_owned(),
            ffmpeg_threads: 1,
            x264_preset: "ultrafast".into(),
            x264_crf: 28,
            killfeed_analyse: true,
            killfeed_models_dir: transcode_dir.join("models"),
            killfeed_rows_model: "killfeed-rows/v1".into(),
            killfeed_icons_model: "killfeed-icons/v1".into(),
            killfeed_threads: 1,
            abandoned_upload_secs: 3600,
            worker_id: None,
            log_format: LogFormat::Pretty,
        }
    }

    /// Runs the worker in the background; send on the returned channel to stop it.
    fn start(
        config: Config,
    ) -> (
        tokio::sync::oneshot::Sender<()>,
        tokio::task::JoinHandle<anyhow::Result<()>>,
    ) {
        let (stop, stopped) = tokio::sync::oneshot::channel::<()>();
        let running = tokio::spawn(run(config, async {
            stopped.await.ok();
        }));
        (stop, running)
    }

    async fn stop(
        stop: tokio::sync::oneshot::Sender<()>,
        running: tokio::task::JoinHandle<anyhow::Result<()>>,
    ) -> anyhow::Result<()> {
        stop.send(()).unwrap();
        tokio::time::timeout(Duration::from_secs(30), running)
            .await
            .expect("stops when asked")
            .unwrap()
    }

    /// Polls `check` until it's true, for up to 30 s, while the worker keeps running.
    async fn eventually<F: Future<Output = bool>>(
        what: &str,
        running: &mut tokio::task::JoinHandle<anyhow::Result<()>>,
        mut check: impl FnMut() -> F,
    ) {
        let deadline = tokio::time::Instant::now() + Duration::from_secs(30);
        while !check().await {
            if running.is_finished() {
                panic!("stopped before {what}: {:?}", running.await);
            }
            assert!(tokio::time::Instant::now() < deadline, "never: {what}");
            tokio::time::sleep(Duration::from_millis(20)).await;
        }
    }

    /// The status of `GET /healthz` on `addr`, or `None` if nothing answers. A bare
    /// HTTP/1.1 request: the worker has no HTTP client.
    async fn health(addr: SocketAddr) -> Option<u16> {
        use tokio::io::{AsyncReadExt, AsyncWriteExt};
        let mut stream = tokio::net::TcpStream::connect(addr).await.ok()?;
        stream
            .write_all(
                format!("GET /healthz HTTP/1.1\r\nHost: {addr}\r\nConnection: close\r\n\r\n")
                    .as_bytes(),
            )
            .await
            .ok()?;
        let mut response = String::new();
        stream.read_to_string(&mut response).await.ok()?;
        response.split(' ').nth(1)?.parse().ok()
    }

    async fn job_status(pool: &PgPool, id: Uuid) -> String {
        sqlx::query_scalar("SELECT status FROM jobs WHERE id = $1")
            .bind(id)
            .fetch_one(pool)
            .await
            .unwrap()
    }

    /// Whether the clip's row is still there, in the trash or not.
    async fn clip_exists(pool: &PgPool, id: Uuid) -> bool {
        sqlx::query_scalar("SELECT EXISTS (SELECT 1 FROM clips WHERE id = $1)")
            .bind(id)
            .fetch_one(pool)
            .await
            .unwrap()
    }

    async fn new_clip(pool: &PgPool) -> Uuid {
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
        let SignIn::Allowed(owner) = users::sign_in(pool, &identity).await.unwrap() else {
            panic!("not allowed in");
        };
        let clip = clips::create(
            pool,
            NewClip {
                owner_id: owner.id,
                game_id: "cs2".into(),
                title: "A clip".into(),
                description: String::new(),
                map: None,
                my_pov: true,
                filename: "clip.mp4".into(),
                bytes: 1000,
            },
        )
        .await
        .unwrap();
        clip.id
    }

    #[sqlx::test(migrator = "clipos_core::db::MIGRATOR")]
    async fn runs_jobs_and_housekeeping_until_shut_down(pool: PgPool) {
        if !azurite() {
            return;
        }
        // Housekeeping runs once at startup: a job orphaned by a dead worker, an upload
        // abandoned two days ago and a clip past its days in the trash.
        let orphaned = jobs::enqueue(&pool, "noop", json!({})).await.unwrap();
        jobs::claim(&pool, "dead-worker").await.unwrap().unwrap();
        sqlx::query("UPDATE jobs SET locked_at = now() - interval '2 hours' WHERE id = $1")
            .bind(orphaned)
            .execute(&pool)
            .await
            .unwrap();
        let queued = jobs::enqueue(&pool, "noop", json!({})).await.unwrap();
        let abandoned = new_clip(&pool).await;
        sqlx::query("UPDATE clips SET created_at = now() - interval '2 days' WHERE id = $1")
            .bind(abandoned)
            .execute(&pool)
            .await
            .unwrap();
        let trashed = new_clip(&pool).await;
        sqlx::query("UPDATE clips SET deleted_at = now() - interval '8 days' WHERE id = $1")
            .bind(trashed)
            .execute(&pool)
            .await
            .unwrap();
        // Scratch left by a job that was running when the worker last stopped.
        let temp = tempfile::tempdir().unwrap();
        let leftover = temp.path().join(Uuid::new_v4().to_string());
        std::fs::create_dir(&leftover).unwrap();
        std::fs::write(temp.path().join("keep.txt"), "not ours").unwrap();

        let config = config(database_url(&pool).await, temp.path());
        let addr = config.bind_addr;
        let (stop_tx, mut running) = start(config);

        eventually("both jobs ran", &mut running, || async {
            job_status(&pool, orphaned).await == "succeeded"
                && job_status(&pool, queued).await == "succeeded"
        })
        .await;
        eventually("the janitor cleaned up", &mut running, || async {
            !clip_exists(&pool, abandoned).await && !clip_exists(&pool, trashed).await
        })
        .await;
        assert!(!leftover.exists());
        assert!(temp.path().join("keep.txt").exists());
        eventually("healthz answers", &mut running, || async {
            health(addr).await == Some(200)
        })
        .await;

        // The next job queued is picked up by the polling loop.
        let later = jobs::enqueue(&pool, "noop", json!({})).await.unwrap();
        eventually("the new job ran", &mut running, || async {
            job_status(&pool, later).await == "succeeded"
        })
        .await;

        stop(stop_tx, running).await.unwrap();
        assert_eq!(health(addr).await, None, "no longer listening");
    }

    #[sqlx::test(migrations = false)]
    async fn waits_for_the_api_to_migrate(pool: PgPool) {
        let shutdown = CancellationToken::new();
        let waiting = {
            let (pool, shutdown) = (pool.clone(), shutdown.clone());
            tokio::spawn(async move {
                wait_for_schema(&pool, Duration::from_millis(20), &shutdown).await
            })
        };
        tokio::time::sleep(Duration::from_millis(200)).await;
        assert!(!waiting.is_finished(), "nothing migrated yet");

        db::migrate(&pool).await.unwrap();
        let ready = tokio::time::timeout(Duration::from_secs(5), waiting)
            .await
            .expect("notices the migration")
            .unwrap()
            .unwrap();
        assert!(ready);
    }

    #[sqlx::test(migrations = false)]
    async fn shuts_down_while_waiting_for_migrations(pool: PgPool) {
        let temp = tempfile::tempdir().unwrap();
        let config = config(database_url(&pool).await, temp.path());
        let addr = config.bind_addr;
        let (stop_tx, mut running) = start(config);

        // Healthy while waiting: the database is up, just not migrated.
        eventually("healthz answers", &mut running, || async {
            health(addr).await == Some(200)
        })
        .await;
        stop(stop_tx, running).await.unwrap();
        // It never got as far as running jobs (or creating the temp dir's contents).
        assert_eq!(std::fs::read_dir(temp.path()).unwrap().count(), 0);
    }

    /// Without its health port the worker still runs jobs (App Service would restart it),
    /// and says why when it stops.
    #[sqlx::test(migrator = "clipos_core::db::MIGRATOR")]
    async fn a_taken_health_port_is_reported_at_shutdown(pool: PgPool) {
        let taken = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
        let temp = tempfile::tempdir().unwrap();
        let mut config = config(database_url(&pool).await, temp.path());
        config.bind_addr = taken.local_addr().unwrap();
        config.worker_id = Some("instance-7".into());
        let (stop_tx, mut running) = start(config);

        let id = jobs::enqueue(&pool, "noop", json!({})).await.unwrap();
        eventually("the job ran", &mut running, || async {
            job_status(&pool, id).await == "succeeded"
        })
        .await;
        let err = stop(stop_tx, running).await.unwrap_err();
        assert!(
            format!("{err:#}").starts_with(&format!("binding {}", taken.local_addr().unwrap())),
            "{err:#}"
        );
    }

    #[tokio::test]
    async fn needs_the_database() {
        let temp = tempfile::tempdir().unwrap();
        let (_stop, running) = start(config("not a url".into(), temp.path()));
        let err = running.await.unwrap().unwrap_err();
        assert!(
            format!("{err:#}").starts_with("parsing DATABASE_URL"),
            "{err:#}"
        );
    }

    /// A worker whose database is down: every query fails fast.
    fn worker_without_database() -> Arc<Worker> {
        let pool = PgPoolOptions::new()
            .acquire_timeout(Duration::from_millis(200))
            .connect_lazy("postgres://clipos:clipos@127.0.0.1:9/clipos")
            .unwrap();
        Arc::new(Worker {
            pool,
            storage: Arc::new(
                Storage::new(StorageConfig {
                    account: "devstoreaccount1".into(),
                    blob_endpoint: Some("http://127.0.0.1:9/devstoreaccount1".into()),
                    account_key: Some(AZURITE_KEY.into()),
                })
                .unwrap(),
            ),
            transcode: TranscodeConfig {
                temp_dir: std::env::temp_dir(),
                threads: 1,
                preset: "ultrafast".into(),
                crf: 28,
            },
            analyse: None,
            id: "test-worker".into(),
            heartbeat: Duration::from_secs(60),
            shutdown: CancellationToken::new(),
            shutdown_grace: Duration::from_secs(90),
        })
    }

    /// A database outage is logged and retried: the loops keep going until shutdown.
    #[tokio::test]
    async fn loops_outlive_a_database_outage() {
        let worker = worker_without_database();
        let shutdown = CancellationToken::new();
        let every = Duration::from_millis(10);
        let loops = [
            tokio::spawn(reap_stale_jobs(
                worker.clone(),
                Duration::from_secs(60),
                every,
                shutdown.clone(),
            )),
            tokio::spawn(janitor(
                worker.clone(),
                Duration::from_secs(60),
                every,
                shutdown.clone(),
            )),
            {
                let (worker, shutdown) = (worker.clone(), shutdown.clone());
                tokio::spawn(async move { run_jobs(&worker, every, &shutdown).await })
            },
        ];
        // Long enough for every loop to fail at least twice.
        tokio::time::sleep(Duration::from_millis(1000)).await;
        assert!(loops.iter().all(|l| !l.is_finished()));

        shutdown.cancel();
        for l in loops {
            tokio::time::timeout(Duration::from_secs(5), l)
                .await
                .expect("stops on shutdown")
                .unwrap();
        }
    }

    #[tokio::test]
    async fn healthz_reports_a_database_outage() {
        let res = healthz(State(worker_without_database().pool.clone()))
            .await
            .into_response();
        assert_eq!(res.status(), StatusCode::SERVICE_UNAVAILABLE);
        assert_eq!(res.headers()["x-clipos-version"], clipos_core::version());
    }

    /// The janitor's blob deletes failing (Blob Storage down) don't stop it either.
    #[sqlx::test(migrator = "clipos_core::db::MIGRATOR")]
    async fn the_janitor_outlives_a_storage_outage(pool: PgPool) {
        let abandoned = new_clip(&pool).await;
        let trashed = new_clip(&pool).await;
        sqlx::query("UPDATE clips SET deleted_at = now() - interval '8 days' WHERE id = $1")
            .bind(trashed)
            .execute(&pool)
            .await
            .unwrap();
        let mut worker = Arc::into_inner(worker_without_database()).unwrap();
        worker.pool = pool.clone();
        let shutdown = CancellationToken::new();
        let janitor = tokio::spawn(janitor(
            Arc::new(worker),
            Duration::ZERO,
            Duration::from_millis(10),
            shutdown.clone(),
        ));
        tokio::time::sleep(Duration::from_millis(300)).await;
        assert!(!janitor.is_finished());
        shutdown.cancel();
        janitor.await.unwrap();
        // The rows stay until their blobs are gone.
        assert!(clip_exists(&pool, abandoned).await);
        assert!(clip_exists(&pool, trashed).await);
    }
}
