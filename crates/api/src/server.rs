//! Startup: everything `clipos-api` does between reading its config and serving, so the
//! binary is only config, logging and signal handling, and tests can run the real thing.

use std::{future::Future, net::SocketAddr, path::PathBuf, sync::Arc};

use anyhow::Context;
use clipos_core::{
    auth::JwtVerifier,
    db, invites,
    storage::{Storage, StorageConfig},
};
use tokio::net::TcpListener;

use crate::{AppState, FALLBACK_INDEX, PublicConfig, config::Config, ratelimit::ShareLimits};

/// Prepares the api (`start`), then serves until `shutdown` resolves.
pub async fn serve(
    config: Config,
    shutdown: impl Future<Output = ()> + Send + 'static,
) -> anyhow::Result<()> {
    start(config).await?.run(shutdown).await
}

/// An api that's migrated, seeded, connected and bound to its address, but not yet
/// answering requests.
pub struct Server {
    listener: TcpListener,
    state: AppState,
    static_dir: PathBuf,
}

/// Connects to Postgres and migrates it, seeds the admins, prepares Azurite (locally),
/// starts the show hub and binds `config.bind_addr`.
pub async fn start(config: Config) -> anyhow::Result<Server> {
    let pool = db::connect(
        &config.database_url,
        config.db_max_connections,
        config.database_auth,
    )
    .await?;
    db::migrate(&pool).await?;
    if let Some(role) = &config.db_worker_role {
        db::grant_table_access(&pool, role).await?;
    }

    let admin_emails = config
        .admin_emails
        .iter()
        .filter(|e| !e.trim().is_empty())
        .map(|e| {
            invites::normalize_email(e).with_context(|| format!("invalid ADMIN_EMAILS entry {e:?}"))
        })
        .collect::<anyhow::Result<Vec<_>>>()?;
    if admin_emails.is_empty() {
        tracing::warn!("ADMIN_EMAILS is empty; only existing invites can sign in");
    }
    invites::seed_admins(&pool, &admin_emails).await?;

    let storage = Storage::new(StorageConfig {
        account: config.storage_account.clone(),
        blob_endpoint: config.storage_blob_endpoint.clone(),
        account_key: config.storage_account_key.clone(),
    })?;
    let origins: Vec<&str> = config
        .local_cors_origins
        .iter()
        .map(String::as_str)
        .collect();
    storage
        .prepare_local(&origins)
        .await
        .context("preparing Azurite (is it running? `make deps`)")?;

    let hub = Arc::new(crate::live::Hub::new(pool.clone()));
    let state = AppState {
        pool,
        verifier: Arc::new(JwtVerifier::new(
            &config.auth0_domain,
            &config.auth0_audience,
        )),
        public_config: Arc::new(PublicConfig {
            auth0_domain: config.auth0_domain.clone(),
            auth0_client_id: config.auth0_client_id.clone(),
            auth0_audience: config.auth0_audience.clone(),
        }),
        internal_secret: config
            .invite_check_secret
            .as_deref()
            .filter(|s| !s.is_empty())
            .map(Arc::from),
        storage: Arc::new(storage),
        public_url: Arc::from(config.public_url.trim_end_matches('/')),
        index_html: Arc::new(
            std::fs::read_to_string(config.static_dir.join("index.html"))
                .unwrap_or_else(|_| FALLBACK_INDEX.to_owned()),
        ),
        share_limits: Arc::new(ShareLimits::new(
            config.share_rate_per_minute,
            config.share_media_mib_per_minute,
            config.share_link_media_mib_per_minute,
            config.share_media_streams_per_client,
        )),
        shows_for_everyone: config.shows_for == "everyone",
        hub,
    };
    tokio::spawn(crate::live::Hub::run(state.clone()));

    if !config.static_dir.join("index.html").exists() {
        tracing::warn!(dir = %config.static_dir.display(), "no built SPA found; only /api will work");
    }

    let listener = TcpListener::bind(config.bind_addr)
        .await
        .with_context(|| format!("binding {}", config.bind_addr))?;
    Ok(Server {
        listener,
        state,
        static_dir: config.static_dir,
    })
}

impl Server {
    /// Where it's listening (the actual port when `BIND_ADDR` asked for port 0).
    pub fn local_addr(&self) -> std::io::Result<SocketAddr> {
        self.listener.local_addr()
    }

    /// Serves requests until `shutdown` resolves, then lets those in flight finish.
    pub async fn run(
        self,
        shutdown: impl Future<Output = ()> + Send + 'static,
    ) -> anyhow::Result<()> {
        tracing::info!(addr = %self.local_addr()?, version = clipos_core::version(), "listening");
        // Connect info gives the rate limiter a client address when there's no proxy header.
        let app = crate::router(self.state, &self.static_dir)
            .into_make_service_with_connect_info::<SocketAddr>();
        axum::serve(self.listener, app)
            .with_graceful_shutdown(shutdown)
            .await?;
        tracing::info!("shut down");
        Ok(())
    }
}
