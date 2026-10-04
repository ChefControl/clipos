//! Members (profiles, the "who's in this clip" picker) and tag autocomplete.

use axum::extract::State;
use std::collections::HashMap;

use clipos_core::{
    clips, shows,
    social::{self, Member},
};
use serde::{Deserialize, Serialize};
use utoipa::{IntoParams, ToSchema};
use uuid::Uuid;

use super::clips::{ClipView, views};
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
    /// Their shows: hosted, and the trophies their clips won. Only for people the show is
    /// open to.
    pub shows: Option<ProfileShows>,
}

#[derive(Debug, Serialize, ToSchema)]
#[serde(rename_all = "camelCase")]
pub struct ProfileShows {
    /// Shows they hosted that ended.
    pub hosted: i64,
    /// Clips of theirs that won clip or fail of the night, newest show first.
    pub trophies: Vec<Trophy>,
}

#[derive(Debug, Serialize, ToSchema)]
#[serde(rename_all = "camelCase")]
pub struct Trophy {
    #[schema(inline)]
    pub category: shows::Category,
    pub clip: ClipView,
    pub show_id: Uuid,
    pub show_started_at: Option<chrono::DateTime<chrono::Utc>>,
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
    let shows = if crate::routes::shows::shows_open_to(&state, &viewer) {
        let wins = shows::wins(&state.pool, member.id).await?;
        let ids: Vec<Uuid> = wins.iter().map(|w| w.clip_id).collect();
        let by_id: HashMap<Uuid, ClipView> = views(
            &state,
            clips::get_many(&state.pool, &ids).await?,
            &viewer,
            false,
        )
        .await?
        .into_iter()
        .map(|v| (v.id, v))
        .collect();
        Some(ProfileShows {
            hosted: shows::hosted(&state.pool, member.id).await?,
            trophies: wins
                .into_iter()
                .filter_map(|w| {
                    // The same clip can win both: each trophy gets its own copy.
                    let clip = by_id.get(&w.clip_id).cloned()?;
                    Some(Trophy {
                        category: w.category,
                        clip,
                        show_id: w.show_id,
                        show_started_at: w.started_at,
                    })
                })
                .collect(),
        })
    } else {
        None
    };
    Ok(Json(Profile {
        member,
        clip_count,
        featured_count,
        fire_count,
        joined_at,
        shows,
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
