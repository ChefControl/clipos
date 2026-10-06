use std::{net::SocketAddr, path::PathBuf};

use clap::Parser;
use clipos_core::{db::DbAuth, telemetry::LogFormat};

// Every setting comes from the environment (App Service app settings in Azure, `.env`
// locally); flags exist for ad-hoc overrides. (A doc comment here would become `--help`.)
#[derive(Debug, Parser)]
#[command(name = "clipos-worker", version)]
pub struct Config {
    #[arg(long, env = "DATABASE_URL", hide_env_values = true)]
    pub database_url: String,

    /// `password` (in DATABASE_URL) locally, `entra` (managed-identity token) in Azure.
    #[arg(long, env = "DATABASE_AUTH", default_value = "password")]
    pub database_auth: DbAuth,

    #[arg(long, env = "DB_MAX_CONNECTIONS", default_value_t = 4)]
    pub db_max_connections: u32,

    /// Health endpoint. App Service marks a container unhealthy if nothing answers on
    /// its port, so the worker serves `/healthz` even though it takes no other traffic.
    #[arg(long, env = "BIND_ADDR", default_value = "0.0.0.0:8081")]
    pub bind_addr: SocketAddr,

    /// How long to sleep when the queue is empty.
    #[arg(long, env = "POLL_INTERVAL_MS", default_value_t = 2000)]
    pub poll_interval_ms: u64,

    /// Running jobs older than this are assumed orphaned by a crashed worker and requeued.
    #[arg(long, env = "STALE_JOB_SECS", default_value_t = 1800)]
    pub stale_job_secs: u64,

    /// Once asked to stop, how long the running job may still take before it's handed back
    /// to the queue. Keep it under the platform's kill timeout (App Service's
    /// WEBSITES_CONTAINER_STOP_TIME_LIMIT, 120 s at most, set in infra/azure/app.tf), so the
    /// handback happens before the kill.
    #[arg(long, env = "SHUTDOWN_GRACE_SECS", default_value_t = 90)]
    pub shutdown_grace_secs: u64,

    /// Storage account name (`devstoreaccount1` for Azurite).
    #[arg(long, env = "STORAGE_ACCOUNT")]
    pub storage_account: String,

    /// Blob endpoint; defaults to `https://{account}.blob.core.windows.net`.
    #[arg(long, env = "STORAGE_BLOB_ENDPOINT")]
    pub storage_blob_endpoint: Option<String>,

    /// Shared key, Azurite only. In Azure, SAS are signed with a user delegation key.
    #[arg(long, env = "STORAGE_ACCOUNT_KEY", hide_env_values = true)]
    pub storage_account_key: Option<String>,

    /// Scratch space for transcode output.
    #[arg(long, env = "TRANSCODE_DIR", default_value = "/tmp/clipos-transcode")]
    pub transcode_dir: PathBuf,

    /// ffmpeg threads; kept low so a transcode never starves the api on the shared plan.
    #[arg(long, env = "FFMPEG_THREADS", default_value_t = 1)]
    pub ffmpeg_threads: u32,

    #[arg(long, env = "X264_PRESET", default_value = "veryfast")]
    pub x264_preset: String,

    #[arg(long, env = "X264_CRF", default_value_t = 20)]
    pub x264_crf: u32,

    /// Killfeed analysis of CS2 clips (phase 8). Needs ONNX Runtime (ORT_DYLIB_PATH) and
    /// the models, in the worker image or the `models` container.
    #[arg(long, env = "KILLFEED_ANALYSE", default_value_t = true, action = clap::ArgAction::Set)]
    pub killfeed_analyse: bool,

    /// Local cache for downloaded models.
    #[arg(
        long,
        env = "KILLFEED_MODELS_DIR",
        default_value = "/tmp/clipos-models"
    )]
    pub killfeed_models_dir: PathBuf,

    /// Models the worker image carries, as `<name>/<version>/` folders; a version that
    /// isn't there is downloaded from the `models` container.
    #[arg(
        long,
        env = "KILLFEED_BAKED_MODELS_DIR",
        default_value = "/opt/clipos/models"
    )]
    pub killfeed_baked_models_dir: PathBuf,

    /// HUD locator, which finds the killfeed rows: `<name>/<version>`. The versions the
    /// image carries are listed in deploy/worker-models.txt.
    #[arg(long, env = "KILLFEED_HUD_MODEL", default_value = "hud-locator/v1")]
    pub killfeed_hud_model: String,

    /// Icon model, `<name>/<version>`.
    #[arg(
        long,
        env = "KILLFEED_ICONS_MODEL",
        default_value = "killfeed-icons/v1"
    )]
    pub killfeed_icons_model: String,

    /// ONNX Runtime threads per model.
    #[arg(long, env = "KILLFEED_THREADS", default_value_t = 2)]
    pub killfeed_threads: usize,

    /// Uploads still unfinished after this long are deleted by the janitor.
    #[arg(long, env = "ABANDONED_UPLOAD_SECS", default_value_t = 24 * 3600)]
    pub abandoned_upload_secs: u64,

    /// Identifies this instance in `jobs.locked_by`. App Service sets WEBSITE_INSTANCE_ID.
    #[arg(long, env = "WEBSITE_INSTANCE_ID")]
    pub worker_id: Option<String>,

    #[arg(long, env = "LOG_FORMAT", default_value = "pretty")]
    pub log_format: LogFormat,
}
