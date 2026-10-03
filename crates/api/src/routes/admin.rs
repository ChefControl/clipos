//! Admin-only: the invite allowlist, user access and reading a clip's killfeed again.

use axum::{extract::State, http::StatusCode};
use clipos_core::{
    analysis,
    clips::{self, ClipStatus},
    invites::{self, Invite},
    jobs,
    users::{self, Role, User, UserStatus},
};
use serde::{Deserialize, Serialize};
use serde_json::json;
use utoipa::ToSchema;
use uuid::Uuid;

use crate::{
    AppState, ErrorBody,
    error::ApiError,
    extract::{Admin, Json, Path},
};

/// Every invite, active first, newest first.
#[utoipa::path(
    get,
    path = "/admin/invites",
    tag = "admin",
    security(("bearer" = [])),
    responses(
        (status = 200, description = "Invites", body = [Invite]),
        (status = 403, description = "Not an admin", body = ErrorBody),
    )
)]
pub async fn list_invites(
    State(state): State<AppState>,
    _: Admin,
) -> Result<Json<Vec<Invite>>, ApiError> {
    Ok(Json(invites::list(&state.pool).await?))
}

#[derive(Debug, Deserialize, ToSchema)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct CreateInvite {
    pub email: String,
    /// Defaults to `member`.
    pub role: Option<Role>,
}

/// Invite an email (or re-activate a revoked invite).
#[utoipa::path(
    post,
    path = "/admin/invites",
    tag = "admin",
    security(("bearer" = [])),
    request_body = CreateInvite,
    responses(
        (status = 201, description = "Invite created", body = Invite),
        (status = 400, description = "Invalid email", body = ErrorBody),
        (status = 403, description = "Not an admin", body = ErrorBody),
    )
)]
pub async fn create_invite(
    State(state): State<AppState>,
    Admin(admin): Admin,
    Json(body): Json<CreateInvite>,
) -> Result<(StatusCode, Json<Invite>), ApiError> {
    let email = invites::normalize_email(&body.email)
        .ok_or_else(|| ApiError::BadRequest("that doesn't look like an email address".into()))?;
    let invite = invites::create(
        &state.pool,
        &email,
        body.role.unwrap_or(Role::Member),
        admin.id,
    )
    .await?;
    tracing::info!(admin = %admin.handle, role = ?invite.role, "invite created");
    Ok((StatusCode::CREATED, Json(invite)))
}

/// Emails travel in the body, never the URL, so they stay out of HTTP logs.
#[derive(Debug, Deserialize, ToSchema)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct RevokeInvite {
    pub email: String,
}

/// Revoke an invite. The user and their clips stay, but they can no longer sign in.
#[utoipa::path(
    post,
    path = "/admin/invites/revoke",
    tag = "admin",
    security(("bearer" = [])),
    request_body = RevokeInvite,
    responses(
        (status = 200, description = "Invite revoked", body = Invite),
        (status = 400, description = "Revoking your own invite", body = ErrorBody),
        (status = 404, description = "No active invite for that email", body = ErrorBody),
    )
)]
pub async fn revoke_invite(
    State(state): State<AppState>,
    Admin(admin): Admin,
    Json(body): Json<RevokeInvite>,
) -> Result<Json<Invite>, ApiError> {
    let email = invites::normalize_email(&body.email).ok_or(ApiError::NotFound)?;
    if email == admin.email {
        return Err(ApiError::BadRequest(
            "you can't revoke your own invite".into(),
        ));
    }
    let invite = invites::revoke(&state.pool, &email)
        .await?
        .ok_or(ApiError::NotFound)?;
    tracing::info!(admin = %admin.handle, "invite revoked");
    Ok(Json(invite))
}

/// Every user who has signed in.
#[utoipa::path(
    get,
    path = "/admin/users",
    tag = "admin",
    security(("bearer" = [])),
    responses(
        (status = 200, description = "Users", body = [User]),
        (status = 403, description = "Not an admin", body = ErrorBody),
    )
)]
pub async fn list_users(
    State(state): State<AppState>,
    _: Admin,
) -> Result<Json<Vec<User>>, ApiError> {
    Ok(Json(users::list(&state.pool).await?))
}

#[derive(Debug, Deserialize, ToSchema)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct UpdateUserAccess {
    pub role: Option<Role>,
    /// `disabled` blocks sign-in; their clips stay.
    pub status: Option<UserStatus>,
}

/// Change a user's role or status.
#[utoipa::path(
    patch,
    path = "/admin/users/{id}",
    tag = "admin",
    security(("bearer" = [])),
    params(("id" = Uuid, Path, description = "User id")),
    request_body = UpdateUserAccess,
    responses(
        (status = 200, description = "Updated user", body = User),
        (status = 400, description = "Changing your own access", body = ErrorBody),
        (status = 404, description = "No such user", body = ErrorBody),
    )
)]
pub async fn update_user(
    State(state): State<AppState>,
    Admin(admin): Admin,
    Path(id): Path<Uuid>,
    Json(body): Json<UpdateUserAccess>,
) -> Result<Json<User>, ApiError> {
    if id == admin.id {
        return Err(ApiError::BadRequest(
            "you can't change your own role or status".into(),
        ));
    }
    let user = users::update_access(&state.pool, id, body.role, body.status)
        .await?
        .ok_or(ApiError::NotFound)?;
    tracing::info!(admin = %admin.handle, user = %user.handle, role = ?user.role, status = ?user.status, "user access changed");
    Ok(Json(user))
}

#[derive(Debug, Serialize, ToSchema)]
#[serde(rename_all = "camelCase")]
pub struct AnalysisQueued {
    /// Always true: the clip's analysis is queued or running.
    pub pending: bool,
}

/// Read a clip's killfeed again, e.g. after a fix to the analysis. The new result replaces
/// the old one when the worker is done; until then the analysis is pending.
#[utoipa::path(
    post,
    path = "/admin/clips/{id}/analyse",
    tag = "admin",
    security(("bearer" = [])),
    params(("id" = Uuid, Path, description = "Clip id")),
    responses(
        (status = 202, description = "Analysis queued, or already queued", body = AnalysisQueued),
        (status = 403, description = "Not an admin", body = ErrorBody),
        (status = 404, description = "No such clip", body = ErrorBody),
        (status = 409, description = "The clip isn't published, or has no killfeed to read", body = ErrorBody),
    )
)]
pub async fn analyse_clip(
    State(state): State<AppState>,
    Admin(admin): Admin,
    Path(id): Path<Uuid>,
) -> Result<(StatusCode, Json<AnalysisQueued>), ApiError> {
    let clip = clips::get_including_deleted(&state.pool, id)
        .await?
        .ok_or(ApiError::NotFound)?;
    if clip.status != ClipStatus::Ready || clip.deleted_at.is_some() {
        return Err(ApiError::Conflict(
            "only published clips can be analysed".into(),
        ));
    }
    // As the transcode decides: only then is the killfeed the uploader's.
    if clip.game_id != "cs2" || !clip.my_pov {
        return Err(ApiError::Conflict(
            "only CS2 clips recorded from the uploader's view are analysed".into(),
        ));
    }
    // Two admins at once could still queue it twice, which only reads the clip twice.
    if !analysis::pending(&state.pool, id).await? {
        // Like the transcode's: the clip already plays, so uploads still go first.
        jobs::enqueue_with_priority(
            &state.pool,
            clips::ANALYSE_JOB,
            json!({ "clipId": id }),
            jobs::PRIORITY_LOW,
        )
        .await?;
        tracing::info!(admin = %admin.handle, clip_id = %id, "re-analysis queued");
    }
    Ok((StatusCode::ACCEPTED, Json(AnalysisQueued { pending: true })))
}
