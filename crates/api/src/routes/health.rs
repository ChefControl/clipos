use axum::{extract::State, http::StatusCode, response::IntoResponse};

use crate::AppState;

/// App Service health check. Unhealthy when Postgres is unreachable, so the platform
/// restarts or routes around the instance. `x-clipos-version` tells the deploy workflow
/// which build is answering.
pub async fn healthz(State(state): State<AppState>) -> impl IntoResponse {
    let version = [("x-clipos-version", clipos_core::version())];
    match clipos_core::db::ping(&state.pool).await {
        Ok(()) => (StatusCode::OK, version, "ok"),
        Err(e) => {
            tracing::warn!(error = %e, "health check failed");
            (
                StatusCode::SERVICE_UNAVAILABLE,
                version,
                "database unavailable",
            )
        }
    }
}
