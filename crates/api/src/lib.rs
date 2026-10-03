//! HTTP API for clipos. Also serves the built SPA so web + API ship as one container.

pub mod config;
mod error;
mod extract;
pub mod live;
pub mod openapi;
pub mod ratelimit;
mod routes;
mod security;
mod server;
mod spa;

use std::{path::Path, sync::Arc};

use axum::{
    Router,
    extract::DefaultBodyLimit,
    middleware,
    routing::{get, post},
};
use clipos_core::{auth::JwtVerifier, storage::Storage};
use sqlx::PgPool;
use tower_http::trace::TraceLayer;

pub use error::ErrorBody;
pub use routes::config::PublicConfig;
pub use server::{Server, serve, start};

#[derive(Clone)]
pub struct AppState {
    pub pool: PgPool,
    pub verifier: Arc<JwtVerifier>,
    pub public_config: Arc<PublicConfig>,
    /// Shared secret for `/internal/*` (the Auth0 Action). `None` disables those routes.
    pub internal_secret: Option<Arc<str>>,
    pub storage: Arc<Storage>,
    /// Public origin for share links, e.g. `https://clips.spawnpoint.run` (no trailing /).
    pub public_url: Arc<str>,
    /// The SPA's `index.html`, which share pages extend with link-preview tags.
    pub index_html: Arc<String>,
    pub share_limits: Arc<ratelimit::ShareLimits>,
    /// Shows are open to everyone, not just admins (`SHOWS_FOR`).
    pub shows_for_everyone: bool,
    /// Live rooms of open shows (S5).
    pub hub: Arc<live::Hub>,
}

/// Largest request body the api reads. Every body is a small JSON document (videos go
/// straight to Blob Storage), so this is far above any real one; axum's own is 2 MB.
pub const MAX_BODY_BYTES: usize = 64 * 1024;

/// Minimal shell when there's no built SPA (tests, API-only runs).
pub const FALLBACK_INDEX: &str = "<!doctype html>\n<html lang=\"en\">\n  <head>\n    <meta charset=\"utf-8\" />\n    <title>clipos</title>\n  </head>\n  <body><div id=\"root\"></div></body>\n</html>\n";

/// Full application router: `/healthz`, `/api/*`, `/internal/*`, public share links
/// under `/s/*` (rate-limited per IP, and their media by the byte, per IP and per link),
/// and the SPA for everything else, all with security headers.
pub fn router(state: AppState, static_dir: &Path) -> Router {
    let (api, _) = openapi::api_router().split_for_parts();
    // The show's live connection: a websocket, so it isn't in the OpenAPI spec.
    let api = api.route("/shows/{id}/live", get(live::connect));
    let security = security::SecurityHeaders::new(
        &state.public_config.auth0_domain,
        &state.storage.origin(),
        &state.public_url,
    );
    let share_pages = Router::new()
        .route("/s/{token}", get(routes::share::page))
        .route("/s/{token}/clip.json", get(routes::share::clip_json))
        .layer(middleware::from_fn_with_state(
            state.clone(),
            ratelimit::limit_pages,
        ));
    let share_media = Router::new()
        .route("/s/{token}/video.mp4", get(routes::share::video))
        .route("/s/{token}/poster.jpg", get(routes::share::poster))
        .layer(middleware::from_fn_with_state(
            state.clone(),
            ratelimit::limit_media,
        ));

    Router::new()
        .merge(share_pages)
        .merge(share_media)
        .route("/healthz", get(routes::health::healthz))
        .route(
            "/internal/invites/check",
            post(routes::internal::check_invite),
        )
        .nest("/api", api.fallback(routes::not_found))
        .fallback_service(spa::service(static_dir))
        .layer(DefaultBodyLimit::max(MAX_BODY_BYTES))
        .layer(middleware::from_fn_with_state(security, security::layer))
        .layer(TraceLayer::new_for_http())
        .with_state(state)
}
