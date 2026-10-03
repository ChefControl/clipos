//! Members (profiles, the "who's in this clip" picker) and tag autocomplete.

use axum::extract::State;
use clipos_core::social::{self, Member};
use serde::{Deserialize, Serialize};
use utoipa::{IntoParams, ToSchema};

use crate::{
    AppState, ErrorBody,
    error::ApiError,
    extract::{CurrentUser, Json, Path, Query},
};

/// Every active member, by name.
#[utoipa::path(
    get,
    path = "/users",
    tag = "users",
    security(("bearer" = [])),
    responses((status = 200, description = "Members", body = [Member]))
)]
pub async fn list_members(
    State(state): State<AppState>,
    _: CurrentUser,
) -> Result<Json<Vec<Member>>, ApiError> {
    Ok(Json(social::members(&state.pool).await?))
}

#[derive(Debug, Serialize, ToSchema)]
#[serde(rename_all = "camelCase")]
pub struct Profile {
    #[serde(flatten)]
    pub member: Member,
    /// Clips they uploaded that you see: as many as `GET /api/clips?uploader={handle}`
    /// lists for you. Others' clips saved for the show (decision 29) and unpublished
    /// clips count only for their uploader.
    pub clip_count: i64,
    /// Clips they play in (tagged by a friend), as many as `?player={handle}` lists.
    pub featured_count: i64,
    /// 🔥 reactions their published clips got.
    pub fire_count: i64,
    /// When they first signed in.
    pub joined_at: chrono::DateTime<chrono::Utc>,
}

/// A member's profile. Their clips: `GET /api/clips?uploader={handle}`.
#[utoipa::path(
    get,
    path = "/users/{handle}",
    tag = "users",
    security(("bearer" = [])),
    params(("handle" = String, Path, description = "Handle")),
    responses(
        (status = 200, description = "Profile", body = Profile),
        (status = 404, description = "No such member", body = ErrorBody),
    )
)]
pub async fn get_profile(
    State(state): State<AppState>,
    CurrentUser(viewer): CurrentUser,
    Path(handle): Path<String>,
) -> Result<Json<Profile>, ApiError> {
    let member = social::member_by_handle(&state.pool, &handle)
        .await?
        .ok_or(ApiError::NotFound)?;
    // The clips the viewer ($1) sees in the feed, so each count matches its list
    // (`?uploader=`, `?player=`): their own unpublished ones count for them, others'
    // clips saved for the show don't.
    let (clip_count, featured_count, fire_count, joined_at) = sqlx::query_as(concat!(
        "SELECT
           (SELECT count(*) FROM clips c WHERE c.owner_id = $2 AND ",
        clipos_core::listed_for_viewer!(),
        "),
           (SELECT count(*) FROM clip_players cp JOIN clips c ON c.id = cp.clip_id
             WHERE cp.user_id = $2 AND ",
        clipos_core::listed_for_viewer!(),
        "),
           (SELECT count(*) FROM reactions r JOIN clips c ON c.id = r.clip_id
             WHERE c.owner_id = $2 AND r.emoji = '🔥' AND ",
        clipos_core::listed_for_viewer!(),
        "),
           (SELECT created_at FROM users WHERE id = $2)"
    ))
    .bind(viewer.id)
    .bind(member.id)
    .fetch_one(&state.pool)
    .await?;
    Ok(Json(Profile {
        member,
        clip_count,
        featured_count,
        fire_count,
        joined_at,
    }))
}

#[derive(Debug, Deserialize, IntoParams)]
#[into_params(parameter_in = Query)]
pub struct TagQuery {
    /// Prefix to complete.
    #[serde(default)]
    pub q: String,
}

/// Tag autocomplete, most used first.
#[utoipa::path(
    get,
    path = "/tags",
    tag = "clips",
    security(("bearer" = [])),
    params(TagQuery),
    responses((status = 200, description = "Matching tags", body = [String]))
)]
pub async fn search_tags(
    State(state): State<AppState>,
    CurrentUser(viewer): CurrentUser,
    Query(q): Query<TagQuery>,
) -> Result<Json<Vec<String>>, ApiError> {
    Ok(Json(
        social::search_tags(&state.pool, viewer.id, &q.q, 10).await?,
    ))
}
