//! Endpoints for the Auth0 post-login Action, outside `/api` and the OpenAPI spec.
//! Authenticated with a shared secret (`INVITE_CHECK_SECRET`), not a user token.

use axum::{extract::State, http::HeaderMap};
use clipos_core::invites;
use serde::{Deserialize, Serialize};

use crate::{AppState, error::ApiError, extract::Json};

pub const SECRET_HEADER: &str = "x-clipos-internal-secret";

#[derive(Debug, Deserialize)]
pub struct InviteCheck {
    email: String,
}

#[derive(Debug, Serialize)]
pub struct InviteCheckResult {
    allowed: bool,
}

/// `POST /internal/invites/check {"email": …}` → `{"allowed": bool}`. A POST body rather
/// than a query string so emails stay out of HTTP logs. 404 when no secret is configured.
pub async fn check_invite(
    State(state): State<AppState>,
    headers: HeaderMap,
    Json(body): Json<InviteCheck>,
) -> Result<Json<InviteCheckResult>, ApiError> {
    let Some(expected) = state.internal_secret.as_deref() else {
        return Err(ApiError::NotFound);
    };
    let given = headers
        .get(SECRET_HEADER)
        .map(|v| v.as_bytes())
        .unwrap_or_default();
    if !constant_time_eq(given, expected.as_bytes()) {
        return Err(ApiError::Unauthorized("bad internal secret".into()));
    }

    let allowed = match invites::normalize_email(&body.email) {
        Some(email) => invites::may_sign_in(&state.pool, &email).await?,
        None => false,
    };
    if !allowed {
        tracing::info!("sign-in refused by the invite check");
    }
    Ok(Json(InviteCheckResult { allowed }))
}

/// Compares without short-circuiting on the first differing byte, so response timing
/// doesn't reveal how much of a guessed secret was right.
fn constant_time_eq(a: &[u8], b: &[u8]) -> bool {
    a.len() == b.len() && a.iter().zip(b).fold(0u8, |acc, (x, y)| acc | (x ^ y)) == 0
}
