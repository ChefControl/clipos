use axum::extract::State;
use clipos_core::users::{self, ProfileError, ProfileUpdate, User};
use serde::{Deserialize, Serialize};
use utoipa::ToSchema;

use crate::{
    AppState, ErrorBody,
    error::ApiError,
    extract::{CurrentUser, Json},
};

/// You, plus what's switched on for you.
#[derive(Debug, Serialize, ToSchema)]
#[serde(rename_all = "camelCase")]
pub struct Me {
    #[serde(flatten)]
    pub user: User,
    /// The show (lobby, hosting, the hold switch on upload) is open to you.
    pub shows: bool,
    /// ...and to everyone, so uploads default to "Save it for the show".
    pub shows_for_everyone: bool,
}

/// The signed-in user, created on first call.
#[utoipa::path(
    get,
    path = "/me",
    tag = "users",
    security(("bearer" = [])),
    responses(
        (status = 200, description = "Current user", body = Me),
        (status = 401, description = "Missing or invalid token", body = ErrorBody),
        (status = 403, description = "Not invited, account disabled, or token lacks email", body = ErrorBody),
    )
)]
pub async fn me(State(state): State<AppState>, CurrentUser(user): CurrentUser) -> Json<Me> {
    Json(Me {
        shows: crate::routes::shows::shows_open_to(&state, &user),
        shows_for_everyone: state.shows_for_everyone,
        user,
    })
}

/// Profile fields to change; omitted fields stay as they are.
#[derive(Debug, Deserialize, ToSchema)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct UpdateProfile {
    /// 1–50 characters.
    pub display_name: Option<String>,
    /// 2–32 characters: `a–z`, `0–9`, `-`, `_`, starting with a letter or digit.
    pub handle: Option<String>,
    /// Up to 64 characters; an empty string clears it.
    pub steam_name: Option<String>,
}

/// Edit your own profile.
#[utoipa::path(
    patch,
    path = "/me",
    tag = "users",
    security(("bearer" = [])),
    request_body = UpdateProfile,
    responses(
        (status = 200, description = "Updated user", body = User),
        (status = 400, description = "Invalid field", body = ErrorBody),
        (status = 409, description = "Handle taken", body = ErrorBody),
    )
)]
pub async fn update_me(
    State(state): State<AppState>,
    CurrentUser(user): CurrentUser,
    Json(body): Json<UpdateProfile>,
) -> Result<Json<User>, ApiError> {
    let update = ProfileUpdate {
        display_name: body.display_name,
        handle: body.handle,
        steam_name: body.steam_name.map(Some),
    };
    match users::update_profile(&state.pool, user.id, update).await {
        Ok(user) => Ok(Json(user)),
        Err(ProfileError::Invalid(m)) => Err(ApiError::BadRequest(m)),
        Err(ProfileError::HandleTaken) => Err(ApiError::Conflict("that handle is taken".into())),
        Err(ProfileError::Database(e)) => Err(e.into()),
    }
}
