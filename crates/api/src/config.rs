use std::{net::SocketAddr, path::PathBuf};

use clap::Parser;
use clipos_core::{db::DbAuth, telemetry::LogFormat};

/// Every setting comes from the environment (App Service app settings in Azure, `.env`
/// locally); flags exist for ad-hoc overrides.
#[derive(Debug, Parser)]
#[command(name = "clipos-api", version)]
pub struct Config {
    #[arg(long, env = "DATABASE_URL", hide_env_values = true)]
    pub database_url: String,

    /// `password` (in DATABASE_URL) locally, `entra` (managed-identity token) in Azure.
    #[arg(long, env = "DATABASE_AUTH", default_value = "password")]
    pub database_auth: DbAuth,

    #[arg(long, env = "DB_MAX_CONNECTIONS", default_value_t = 10)]
    pub db_max_connections: u32,

    /// Worker's database login, granted read/write on the api's tables after migrating.
    /// Unset locally, where both binaries share one login.
    #[arg(long, env = "DB_WORKER_ROLE")]
    pub db_worker_role: Option<String>,

    #[arg(long, env = "BIND_ADDR", default_value = "0.0.0.0:8080")]
    pub bind_addr: SocketAddr,

    /// Directory holding the built SPA (`web/dist`).
    #[arg(long, env = "STATIC_DIR", default_value = "web/dist")]
    pub static_dir: PathBuf,

    /// Auth0 tenant domain, e.g. `spawnpoint.eu.auth0.com`.
    #[arg(long, env = "AUTH0_DOMAIN")]
    pub auth0_domain: String,

    /// API identifier access tokens must be issued for.
    #[arg(long, env = "AUTH0_AUDIENCE")]
    pub auth0_audience: String,

    /// SPA client ID handed to the browser via `/api/config`.
    #[arg(long, env = "AUTH0_CLIENT_ID")]
    pub auth0_client_id: String,

    /// Comma-separated emails that are always invited as admins (seeded at startup), so
    /// the first admin can sign in and nobody can lock them out.
    #[arg(
        long,
        env = "ADMIN_EMAILS",
        value_delimiter = ',',
        hide_env_values = true
    )]
    pub admin_emails: Vec<String>,

    /// Shared secret the Auth0 post-login Action sends to `/internal/invites/check`.
    /// Unset locally: the dev login skips that check (Auth0 can't reach localhost).
    #[arg(long, env = "INVITE_CHECK_SECRET", hide_env_values = true)]
    pub invite_check_secret: Option<String>,

    /// Storage account name (`devstoreaccount1` for Azurite).
    #[arg(long, env = "STORAGE_ACCOUNT")]
    pub storage_account: String,

    /// Blob endpoint; defaults to `https://{account}.blob.core.windows.net`.
    #[arg(long, env = "STORAGE_BLOB_ENDPOINT")]
    pub storage_blob_endpoint: Option<String>,

    /// Shared key, Azurite only. In Azure, SAS are signed with a user delegation key.
    #[arg(long, env = "STORAGE_ACCOUNT_KEY", hide_env_values = true)]
    pub storage_account_key: Option<String>,

    /// Public origin used in share links (`https://clips.spawnpoint.run` in Azure).
    #[arg(long, env = "PUBLIC_URL", default_value = "http://localhost:5173")]
    pub public_url: String,

    /// Requests per minute per IP (per /64 for IPv6) on share pages and their
    /// `clip.json` (bursts up to half that).
    #[arg(long, env = "SHARE_RATE_PER_MINUTE", default_value_t = 120)]
    pub share_rate_per_minute: u32,

    /// MiB per minute per IP streamed from share links' videos and posters (bursts up to
    /// half that), each request at least 1 MiB. Past it, streams slow down to that pace
    /// and new requests are refused. The default is about twice the top bitrate, so one
    /// viewer's playback never waits on it.
    #[arg(long, env = "SHARE_MEDIA_MIB_PER_MINUTE", default_value_t = 300)]
    pub share_media_mib_per_minute: u32,

    /// MiB per minute streamed from one share link's video and poster, to everyone
    /// together (bursts up to half that).
    #[arg(long, env = "SHARE_LINK_MEDIA_MIB_PER_MINUTE", default_value_t = 2048)]
    pub share_link_media_mib_per_minute: u32,

    /// Videos and posters one IP may be streaming at once.
    #[arg(long, env = "SHARE_MEDIA_STREAMS_PER_CLIENT", default_value_t = 6)]
    pub share_media_streams_per_client: usize,

    /// Origins allowed to upload to Azurite from the browser (local dev only; in Azure
    /// CORS is set by infra/azure).
    #[arg(
        long,
        env = "LOCAL_CORS_ORIGINS",
        value_delimiter = ',',
        default_value = "http://localhost:5173"
    )]
    pub local_cors_origins: Vec<String>,

    /// Who can see the show (docs/PLAN.md, redesign): `admins` while it's built and tried
    /// out, `everyone` once it opens (decision 40).
    #[arg(
        long,
        env = "SHOWS_FOR",
        default_value = "admins",
        value_parser = ["admins", "everyone"]
    )]
    pub shows_for: String,

    #[arg(long, env = "LOG_FORMAT", default_value = "pretty")]
    pub log_format: LogFormat,
}
