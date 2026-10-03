use axum::extract::State;
use serde::Serialize;
use utoipa::ToSchema;

use crate::{AppState, extract::Json};

/// Settings the SPA needs before it can log in. Served at runtime so one image works
/// for every environment.
#[derive(Debug, Clone, Serialize, ToSchema)]
#[serde(rename_all = "camelCase")]
pub struct PublicConfig {
    pub auth0_domain: String,
    pub auth0_client_id: String,
    pub auth0_audience: String,
}

#[utoipa::path(
    get,
    path = "/config",
    tag = "meta",
    responses((status = 200, description = "Public client configuration", body = PublicConfig))
)]
pub async fn config(State(state): State<AppState>) -> Json<PublicConfig> {
    Json(state.public_config.as_ref().clone())
}
