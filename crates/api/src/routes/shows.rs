//! Shows (docs/PLAN.md, redesign, S4): tonight's lineup, opening a show, joining it, the
//! host's controls, show reactions and the finale vote, and past shows for the archive.
//! Admins only until the show opens to everyone (`SHOWS_FOR`, decision 40). The live sync
//! (everyone's player in step) is the show hub, `crate::live`.

use std::collections::HashMap;

use axum::{extract::State, http::StatusCode};
use chrono::{DateTime, Utc};
use clipos_core::{
    clips,
    shows::{self, Category, Show, ShowError, ShowStatus, TieBreak},
    social::{self, Member},
    users::{Role, User},
};
use serde::{Deserialize, Serialize};
use utoipa::ToSchema;
use uuid::Uuid;

use super::clips::{ClipView, views};
use crate::{
    AppState, ErrorBody,
    error::ApiError,
    extract::{CurrentUser, Json, Path},
};

/// How many past shows the archive gets.
const PAST_SHOWS: i64 = 50;

impl From<ShowError> for ApiError {
    fn from(e: ShowError) -> Self {
        match e {
            ShowError::NotFound => ApiError::NotFound,
            ShowError::Forbidden(m) => ApiError::Forbidden(m.into()),
            ShowError::Conflict(m) => ApiError::Conflict(m),
            ShowError::Invalid(m) => ApiError::BadRequest(m),
            ShowError::Tie(ties) => ApiError::Tied {
                message: format!(
                    "the {} tied: pick one of the tied clips",
                    match (ties.clip.is_empty(), ties.fail.is_empty()) {
                        (false, true) => "clip of the night vote is",
                        (true, false) => "fail of the night vote is",
                        _ => "clip and fail of the night votes are",
                    }
                ),
                ties,
            },
            ShowError::Db(e) => e.into(),
        }
    }
}

/// Whether the show is open to this user yet (`SHOWS_FOR`, decision 40).
pub(crate) fn shows_open_to(state: &AppState, user: &User) -> bool {
    state.shows_for_everyone || user.role == Role::Admin
}

/// The signed-in user, if the show is open to them yet. Otherwise shows don't exist.
fn allowed(state: &AppState, user: &User) -> Result<(), ApiError> {
    if shows_open_to(state, user) {
        Ok(())
    } else {
        Err(ApiError::NotFound)
    }
}

#[derive(Debug, Serialize, ToSchema)]
#[serde(rename_all = "camelCase")]
pub struct LineupEntry {
    pub clip: ClipView,
    pub position: i32,
    /// Left out by the host, or a spare; stays for the next show.
    pub dropped: bool,
    /// Left out because the show was full (10 clips), not by the host (decision 57).
    pub spare: bool,
    pub played_at: Option<DateTime<Utc>>,
    /// Who put it in the lineup (the host for tonight's clips).
    pub added_by: Option<Uuid>,
}

#[derive(Debug, Serialize, ToSchema)]
#[serde(rename_all = "camelCase")]
pub struct ParticipantView {
    pub member: Member,
    pub joined_at: DateTime<Utc>,
    /// Clicked "I'm ready, sound on".
    pub ready: bool,
}

#[derive(Debug, Serialize, ToSchema)]
#[serde(rename_all = "camelCase")]
pub struct VoteCountView {
    #[schema(inline)]
    pub category: Category,
    pub clip_id: Uuid,
    pub votes: i64,
}

#[derive(Debug, Serialize, ToSchema)]
#[serde(rename_all = "camelCase")]
pub struct ShowReactionView {
    pub clip_id: Uuid,
    pub user_id: Uuid,
    pub emoji: String,
    /// When in the clip, in milliseconds.
    pub at_ms: i32,
}

#[derive(Debug, Default, Serialize, ToSchema)]
#[serde(rename_all = "camelCase")]
pub struct MyVotes {
    pub clip: Option<Uuid>,
    pub fail: Option<Uuid>,
}

/// Who has voted in each category (user ids), not for what.
#[derive(Debug, Default, Serialize, ToSchema)]
#[serde(rename_all = "camelCase")]
pub struct Voters {
    pub clip: Vec<Uuid>,
    pub fail: Vec<Uuid>,
}

/// When each category of the finale vote runs, 20 s each (decision 31): every screen
/// counts down to the same moments.
#[derive(Debug, Serialize, ToSchema)]
#[serde(rename_all = "camelCase")]
pub struct FinaleView {
    pub started_at: DateTime<Utc>,
    /// Fail of the night, first; none when nobody pressed 🍌 tonight.
    pub fail_from: Option<DateTime<Utc>>,
    pub fail_until: Option<DateTime<Utc>>,
    pub clip_from: DateTime<Utc>,
    pub clip_until: DateTime<Utc>,
}

#[derive(Debug, Serialize, ToSchema)]
#[serde(rename_all = "camelCase")]
pub struct ShowView {
    pub id: Uuid,
    #[schema(inline)]
    pub status: ShowStatus,
    pub host: Member,
    pub created_at: DateTime<Utc>,
    pub started_at: Option<DateTime<Utc>>,
    pub ended_at: Option<DateTime<Utc>>,
    /// In order: played clips first, then what's still to come; dropped ones last.
    pub lineup: Vec<LineupEntry>,
    pub participants: Vec<ParticipantView>,
    /// Clips someone marked with 🍌: the fail of the night contenders.
    pub fail_contenders: Vec<Uuid>,
    /// Votes per clip, once the show has ended. Empty until then, for everyone, so the
    /// reveal isn't spoiled (decision 43).
    pub votes: Vec<VoteCountView>,
    /// Who has voted so far, per category: the finale's "3 of 4 voted".
    pub voters: Voters,
    /// The viewer's own votes.
    pub my_votes: MyVotes,
    pub clip_winner_id: Option<Uuid>,
    pub fail_winner_id: Option<Uuid>,
    /// The vote's timing, from when the show went to its finale.
    pub finale: Option<FinaleView>,
    /// Every reaction tap, in clip order and time, for the replay.
    pub reactions: Vec<ShowReactionView>,
}

async fn show_view(state: &AppState, show: Show, viewer: &User) -> Result<ShowView, ApiError> {
    let pool = &state.pool;
    let lineup = shows::lineup(pool, show.id).await?;
    let participants = shows::participants(pool, show.id).await?;
    let clip_ids: Vec<Uuid> = lineup.iter().map(|l| l.clip_id).collect();
    let mut clip_views: HashMap<Uuid, ClipView> = views(
        state,
        clips::get_many(pool, &clip_ids).await?,
        viewer,
        false,
    )
    .await?
    .into_iter()
    .map(|v| (v.id, v))
    .collect();
    let mut people: Vec<Uuid> = participants.iter().map(|p| p.user_id).collect();
    people.push(show.host_id);
    let mut members: HashMap<Uuid, Member> = social::members_by_ids(pool, &people)
        .await?
        .into_iter()
        .map(|m| (m.id, m))
        .collect();
    let fail_contenders = shows::fail_contenders(pool, show.id).await?;
    let votes = if show.status == ShowStatus::Ended {
        shows::tally(pool, show.id).await?
    } else {
        Vec::new()
    };
    let cast: Vec<(Uuid, Category, Uuid)> = sqlx::query_as(
        "SELECT voter_id, category, clip_id FROM show_votes
          WHERE show_id = $1 ORDER BY created_at, voter_id",
    )
    .bind(show.id)
    .fetch_all(pool)
    .await?;
    let (mut voters, mut my_votes) = (Voters::default(), MyVotes::default());
    for (voter, category, clip) in cast {
        let (who, mine) = match category {
            Category::Clip => (&mut voters.clip, &mut my_votes.clip),
            Category::Fail => (&mut voters.fail, &mut my_votes.fail),
        };
        who.push(voter);
        if voter == viewer.id {
            *mine = Some(clip);
        }
    }
    let mut entries: Vec<LineupEntry> = lineup
        .into_iter()
        // A clip deleted since it was added just drops out of the view.
        .filter_map(|l| {
            Some(LineupEntry {
                clip: clip_views.remove(&l.clip_id)?,
                position: l.position,
                dropped: l.dropped,
                spare: l.spare,
                played_at: l.played_at,
                added_by: l.added_by,
            })
        })
        .collect();
    entries.sort_by_key(|e| (e.dropped, e.position));
    Ok(ShowView {
        id: show.id,
        status: show.status,
        host: members
            .get(&show.host_id)
            .cloned()
            .ok_or_else(|| anyhow::anyhow!("show host {} missing", show.host_id))?,
        created_at: show.created_at,
        started_at: show.started_at,
        ended_at: show.ended_at,
        lineup: entries,
        participants: participants
            .into_iter()
            .filter_map(|p| {
                Some(ParticipantView {
                    member: members.remove(&p.user_id)?,
                    joined_at: p.joined_at,
                    ready: p.ready,
                })
            })
            .collect(),
        finale: show.finale_at.map(|at| {
            let w = shows::vote_windows(at, !fail_contenders.is_empty());
            FinaleView {
                started_at: at,
                fail_from: w.fail.map(|f| f.0),
                fail_until: w.fail.map(|f| f.1),
                clip_from: w.clip.0,
                clip_until: w.clip.1,
            }
        }),
        fail_contenders,
        votes: votes
            .into_iter()
            .map(|v| VoteCountView {
                category: v.category,
                clip_id: v.clip_id,
                votes: v.votes,
            })
            .collect(),
        voters,
        my_votes,
        clip_winner_id: show.clip_winner_id,
        fail_winner_id: show.fail_winner_id,
        reactions: shows::reactions(pool, show.id)
            .await?
            .into_iter()
            .map(|r| ShowReactionView {
                clip_id: r.clip_id,
                user_id: r.user_id,
                emoji: r.emoji,
                at_ms: r.at_ms,
            })
            .collect(),
    })
}

async fn reload(state: &AppState, id: Uuid, viewer: &User) -> Result<Json<ShowView>, ApiError> {
    let show = shows::get(&state.pool, id)
        .await?
        .ok_or(ApiError::NotFound)?;
    Ok(Json(show_view(state, show, viewer).await?))
}

#[derive(Debug, Serialize, ToSchema)]
#[serde(rename_all = "camelCase")]
pub struct Tonight {
    /// The show that's on (lobby, live or finale), if any.
    pub show: Option<ShowView>,
    /// With no show on: the clips the next one would play, in upload order.
    pub clips: Vec<ClipView>,
    /// The last show that ended, for the lobby's "Last show" line.
    pub last_show: Option<PastShow>,
}

/// Tonight: the show that's on, or the clips the next one would have.
#[utoipa::path(
    get,
    path = "/shows/tonight",
    tag = "shows",
    security(("bearer" = [])),
    responses(
        (status = 200, description = "Tonight", body = Tonight),
        (status = 404, description = "Shows aren't open to you yet", body = ErrorBody),
    )
)]
pub async fn tonight(
    State(state): State<AppState>,
    CurrentUser(user): CurrentUser,
) -> Result<Json<Tonight>, ApiError> {
    allowed(&state, &user)?;
    let show = match shows::open(&state.pool).await? {
        Some(show) => Some(show_view(&state, show, &user).await?),
        None => None,
    };
    let clips = if show.is_some() {
        Vec::new()
    } else {
        let ids = shows::tonight(&state.pool).await?;
        let mut by_id: HashMap<Uuid, clips::Clip> = clips::get_many(&state.pool, &ids)
            .await?
            .into_iter()
            .map(|c| (c.id, c))
            .collect();
        let ordered = ids.iter().filter_map(|id| by_id.remove(id)).collect();
        views(&state, ordered, &user, false).await?
    };
    let last_show = match shows::past(&state.pool, 1).await?.into_iter().next() {
        Some(s) => Some(past_show(&state, s, &user).await?),
        None => None,
    };
    Ok(Json(Tonight {
        show,
        clips,
        last_show,
    }))
}

/// The show that's on, in brief: what every page's "Live · Join" pill needs.
#[derive(Debug, Serialize, ToSchema)]
#[serde(rename_all = "camelCase")]
pub struct CurrentShow {
    pub id: Uuid,
    #[schema(inline)]
    pub status: ShowStatus,
    pub host: Member,
    pub started_at: Option<DateTime<Utc>>,
}

/// The show that's on (lobby, live or finale), or null. Cheap enough for every page to
/// ask now and then.
#[utoipa::path(
    get,
    path = "/shows/current",
    tag = "shows",
    security(("bearer" = [])),
    responses(
        (status = 200, description = "The show that's on, or null", body = Option<CurrentShow>),
        (status = 404, description = "Shows aren't open to you yet", body = ErrorBody),
    )
)]
pub async fn current_show(
    State(state): State<AppState>,
    CurrentUser(user): CurrentUser,
) -> Result<Json<Option<CurrentShow>>, ApiError> {
    allowed(&state, &user)?;
    let Some(show) = shows::open(&state.pool).await? else {
        return Ok(Json(None));
    };
    let host = social::members_by_ids(&state.pool, &[show.host_id])
        .await?
        .into_iter()
        .next()
        .ok_or_else(|| anyhow::anyhow!("show host {} missing", show.host_id))?;
    Ok(Json(Some(CurrentShow {
        id: show.id,
        status: show.status,
        host,
        started_at: show.started_at,
    })))
}

/// Opens a show with tonight's clips; you host it. One show at a time (409 otherwise).
#[utoipa::path(
    post,
    path = "/shows",
    tag = "shows",
    security(("bearer" = [])),
    responses(
        (status = 201, description = "The new show", body = ShowView),
        (status = 404, description = "Shows aren't open to you yet", body = ErrorBody),
        (status = 409, description = "A show is already on", body = ErrorBody),
    )
)]
pub async fn create_show(
    State(state): State<AppState>,
    CurrentUser(user): CurrentUser,
) -> Result<(StatusCode, Json<ShowView>), ApiError> {
    allowed(&state, &user)?;
    let show = shows::create(&state.pool, user.id).await?;
    Ok((
        StatusCode::CREATED,
        Json(show_view(&state, show, &user).await?),
    ))
}

/// One show, past or present, with everything for its page and replay.
#[utoipa::path(
    get,
    path = "/shows/{id}",
    tag = "shows",
    security(("bearer" = [])),
    params(("id" = Uuid, Path, description = "Show id")),
    responses(
        (status = 200, description = "The show", body = ShowView),
        (status = 404, description = "No such show, or shows aren't open to you yet", body = ErrorBody),
    )
)]
pub async fn get_show(
    State(state): State<AppState>,
    CurrentUser(user): CurrentUser,
    Path(id): Path<Uuid>,
) -> Result<Json<ShowView>, ApiError> {
    allowed(&state, &user)?;
    reload(&state, id, &user).await
}

#[derive(Debug, Serialize, ToSchema)]
#[serde(rename_all = "camelCase")]
pub struct PastShow {
    pub id: Uuid,
    pub host: Member,
    pub started_at: Option<DateTime<Utc>>,
    pub ended_at: Option<DateTime<Utc>>,
    /// Who was there.
    pub participants: Vec<Member>,
    /// The clips it played, in order.
    pub clips: Vec<ClipView>,
    pub clip_winner_id: Option<Uuid>,
    pub fail_winner_id: Option<Uuid>,
    /// Votes the winners got, and how many people voted in each category.
    pub clip_winner_votes: i64,
    pub fail_winner_votes: i64,
    pub clip_voters: i64,
    pub fail_voters: i64,
}

async fn past_show(state: &AppState, show: Show, viewer: &User) -> Result<PastShow, ApiError> {
    let pool = &state.pool;
    let played: Vec<Uuid> = shows::lineup(pool, show.id)
        .await?
        .into_iter()
        .filter(|l| l.played_at.is_some() && !l.dropped)
        .map(|l| l.clip_id)
        .collect();
    let mut by_id: HashMap<Uuid, clips::Clip> = clips::get_many(pool, &played)
        .await?
        .into_iter()
        .map(|c| (c.id, c))
        .collect();
    let ordered = played.iter().filter_map(|id| by_id.remove(id)).collect();
    let people: Vec<Uuid> = shows::participants(pool, show.id)
        .await?
        .into_iter()
        .map(|p| p.user_id)
        .chain([show.host_id])
        .collect();
    let mut members: HashMap<Uuid, Member> = social::members_by_ids(pool, &people)
        .await?
        .into_iter()
        .map(|m| (m.id, m))
        .collect();
    let host = members
        .get(&show.host_id)
        .cloned()
        .ok_or_else(|| anyhow::anyhow!("show host {} missing", show.host_id))?;
    let participants = people[..people.len() - 1]
        .iter()
        .filter_map(|id| members.remove(id))
        .collect();
    let tally = shows::tally(pool, show.id).await?;
    let votes_for = |cat: Category, clip: Option<Uuid>| {
        tally
            .iter()
            .filter(|v| v.category == cat && Some(v.clip_id) == clip)
            .map(|v| v.votes)
            .sum()
    };
    let voters = |cat: Category| {
        tally
            .iter()
            .filter(|v| v.category == cat)
            .map(|v| v.votes)
            .sum()
    };
    Ok(PastShow {
        id: show.id,
        host,
        started_at: show.started_at,
        ended_at: show.ended_at,
        participants,
        clips: views(state, ordered, viewer, false).await?,
        clip_winner_votes: votes_for(Category::Clip, show.clip_winner_id),
        fail_winner_votes: votes_for(Category::Fail, show.fail_winner_id),
        clip_voters: voters(Category::Clip),
        fail_voters: voters(Category::Fail),
        clip_winner_id: show.clip_winner_id,
        fail_winner_id: show.fail_winner_id,
    })
}

/// Shows that ended, newest first: the archive's past shows.
#[utoipa::path(
    get,
    path = "/shows",
    tag = "shows",
    security(("bearer" = [])),
    responses(
        (status = 200, description = "Past shows", body = [PastShow]),
        (status = 404, description = "Shows aren't open to you yet", body = ErrorBody),
    )
)]
pub async fn list_shows(
    State(state): State<AppState>,
    CurrentUser(user): CurrentUser,
) -> Result<Json<Vec<PastShow>>, ApiError> {
    allowed(&state, &user)?;
    let mut out = Vec::new();
    for show in shows::past(&state.pool, PAST_SHOWS).await? {
        out.push(past_show(&state, show, &user).await?);
    }
    Ok(Json(out))
}

#[derive(Debug, Deserialize, ToSchema)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct SetLineup {
    /// The clips still to play, in order, at most 10 with the played ones. Ones left out
    /// are dropped (they stay for the next show).
    pub clip_ids: Vec<Uuid>,
}

/// The host reorders and drops the clips still to play.
#[utoipa::path(
    put,
    path = "/shows/{id}/lineup",
    tag = "shows",
    security(("bearer" = [])),
    params(("id" = Uuid, Path, description = "Show id")),
    request_body = SetLineup,
    responses(
        (status = 200, description = "The show", body = ShowView),
        (status = 400, description = "A clip that isn't waiting in the lineup, one twice, or more than 10 clips", body = ErrorBody),
        (status = 403, description = "Not the host", body = ErrorBody),
        (status = 404, description = "No such show, or shows aren't open to you yet", body = ErrorBody),
        (status = 409, description = "Not in the lobby or live", body = ErrorBody),
    )
)]
pub async fn set_lineup(
    State(state): State<AppState>,
    CurrentUser(user): CurrentUser,
    Path(id): Path<Uuid>,
    Json(body): Json<SetLineup>,
) -> Result<Json<ShowView>, ApiError> {
    allowed(&state, &user)?;
    shows::set_lineup(&state.pool, id, user.id, &body.clip_ids).await?;
    state.hub.show_changed(id);
    reload(&state, id, &user).await
}

#[derive(Debug, Deserialize, ToSchema)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct AddClip {
    pub clip_id: Uuid,
}

/// Anyone in the show adds a clip to the end of the queue, up to 10 clips.
#[utoipa::path(
    post,
    path = "/shows/{id}/clips",
    tag = "shows",
    security(("bearer" = [])),
    params(("id" = Uuid, Path, description = "Show id")),
    request_body = AddClip,
    responses(
        (status = 200, description = "The show", body = ShowView),
        (status = 400, description = "Your clip isn't ready to play", body = ErrorBody),
        (status = 403, description = "Not in the show", body = ErrorBody),
        (status = 404, description = "No such show or clip, or shows aren't open to you yet", body = ErrorBody),
        (status = 409, description = "Already in the lineup, the show has 10 clips, or not in the lobby or live", body = ErrorBody),
    )
)]
pub async fn add_clip(
    State(state): State<AppState>,
    CurrentUser(user): CurrentUser,
    Path(id): Path<Uuid>,
    Json(body): Json<AddClip>,
) -> Result<Json<ShowView>, ApiError> {
    allowed(&state, &user)?;
    shows::add_clip(&state.pool, id, user.id, body.clip_id).await?;
    state.hub.show_changed(id);
    reload(&state, id, &user).await
}

/// Join the show (until it ends).
#[utoipa::path(
    post,
    path = "/shows/{id}/join",
    tag = "shows",
    security(("bearer" = [])),
    params(("id" = Uuid, Path, description = "Show id")),
    responses(
        (status = 200, description = "The show", body = ShowView),
        (status = 404, description = "No such show, or shows aren't open to you yet", body = ErrorBody),
        (status = 409, description = "The show is over", body = ErrorBody),
    )
)]
pub async fn join_show(
    State(state): State<AppState>,
    CurrentUser(user): CurrentUser,
    Path(id): Path<Uuid>,
) -> Result<Json<ShowView>, ApiError> {
    allowed(&state, &user)?;
    shows::join(&state.pool, id, user.id).await?;
    state.hub.show_changed(id);
    reload(&state, id, &user).await
}

#[derive(Debug, Deserialize, ToSchema)]
#[serde(deny_unknown_fields)]
pub struct SetReady {
    pub ready: bool,
}

/// "I'm ready, sound on" (or not).
#[utoipa::path(
    put,
    path = "/shows/{id}/ready",
    tag = "shows",
    security(("bearer" = [])),
    params(("id" = Uuid, Path, description = "Show id")),
    request_body = SetReady,
    responses(
        (status = 200, description = "The show", body = ShowView),
        (status = 400, description = "Unreadable body", body = ErrorBody),
        (status = 403, description = "Not in the show", body = ErrorBody),
        (status = 404, description = "No such show, or shows aren't open to you yet", body = ErrorBody),
        (status = 409, description = "The show is over", body = ErrorBody),
    )
)]
pub async fn set_ready(
    State(state): State<AppState>,
    CurrentUser(user): CurrentUser,
    Path(id): Path<Uuid>,
    Json(body): Json<SetReady>,
) -> Result<Json<ShowView>, ApiError> {
    allowed(&state, &user)?;
    shows::set_ready(&state.pool, id, user.id, body.ready).await?;
    state.hub.show_changed(id);
    reload(&state, id, &user).await
}

/// The host starts the show.
#[utoipa::path(
    post,
    path = "/shows/{id}/start",
    tag = "shows",
    security(("bearer" = [])),
    params(("id" = Uuid, Path, description = "Show id")),
    responses(
        (status = 200, description = "The show", body = ShowView),
        (status = 403, description = "Not the host", body = ErrorBody),
        (status = 404, description = "No such show, or shows aren't open to you yet", body = ErrorBody),
        (status = 409, description = "Not in the lobby", body = ErrorBody),
    )
)]
pub async fn start_show(
    State(state): State<AppState>,
    CurrentUser(user): CurrentUser,
    Path(id): Path<Uuid>,
) -> Result<Json<ShowView>, ApiError> {
    allowed(&state, &user)?;
    shows::start(&state.pool, id, user.id).await?;
    state.hub.show_changed(id);
    reload(&state, id, &user).await
}

/// Anyone in the show: this clip started playing for everyone.
#[utoipa::path(
    post,
    path = "/shows/{id}/clips/{clip_id}/played",
    tag = "shows",
    security(("bearer" = [])),
    params(
        ("id" = Uuid, Path, description = "Show id"),
        ("clip_id" = Uuid, Path, description = "Clip id"),
    ),
    responses(
        (status = 200, description = "The show", body = ShowView),
        (status = 400, description = "Not in the lineup", body = ErrorBody),
        (status = 403, description = "Not in the show", body = ErrorBody),
        (status = 404, description = "No such show, or shows aren't open to you yet", body = ErrorBody),
        (status = 409, description = "The show isn't live", body = ErrorBody),
    )
)]
pub async fn mark_played(
    State(state): State<AppState>,
    CurrentUser(user): CurrentUser,
    Path((id, clip_id)): Path<(Uuid, Uuid)>,
) -> Result<Json<ShowView>, ApiError> {
    allowed(&state, &user)?;
    shows::mark_played(&state.pool, id, user.id, clip_id).await?;
    state.hub.show_changed(id);
    reload(&state, id, &user).await
}

#[derive(Debug, Deserialize, ToSchema)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct NewShowReaction {
    pub clip_id: Uuid,
    /// One of the six reactions, or 🍌 (marks a fail).
    pub emoji: String,
    /// When in the clip, in milliseconds.
    pub at_ms: i32,
}

/// A tap in the reaction dock while a clip plays.
#[utoipa::path(
    post,
    path = "/shows/{id}/reactions",
    tag = "shows",
    security(("bearer" = [])),
    params(("id" = Uuid, Path, description = "Show id")),
    request_body = NewShowReaction,
    responses(
        (status = 204, description = "Kept"),
        (status = 400, description = "Unknown emoji, a clip that hasn't played or is in the trash, or a moment outside the clip", body = ErrorBody),
        (status = 403, description = "Not in the show", body = ErrorBody),
        (status = 404, description = "No such show, or shows aren't open to you yet", body = ErrorBody),
        (status = 409, description = "The show isn't live", body = ErrorBody),
    )
)]
pub async fn react(
    State(state): State<AppState>,
    CurrentUser(user): CurrentUser,
    Path(id): Path<Uuid>,
    Json(body): Json<NewShowReaction>,
) -> Result<StatusCode, ApiError> {
    allowed(&state, &user)?;
    shows::react(
        &state.pool,
        id,
        user.id,
        body.clip_id,
        &body.emoji,
        body.at_ms,
    )
    .await?;
    Ok(StatusCode::NO_CONTENT)
}

/// The host goes to the finale: after the last clip, or to end the show early.
#[utoipa::path(
    post,
    path = "/shows/{id}/finale",
    tag = "shows",
    security(("bearer" = [])),
    params(("id" = Uuid, Path, description = "Show id")),
    responses(
        (status = 200, description = "The show", body = ShowView),
        (status = 403, description = "Not the host", body = ErrorBody),
        (status = 404, description = "No such show, or shows aren't open to you yet", body = ErrorBody),
        (status = 409, description = "The show isn't live", body = ErrorBody),
    )
)]
pub async fn finale(
    State(state): State<AppState>,
    CurrentUser(user): CurrentUser,
    Path(id): Path<Uuid>,
) -> Result<Json<ShowView>, ApiError> {
    allowed(&state, &user)?;
    shows::finale(&state.pool, id, user.id).await?;
    state.hub.show_changed(id);
    reload(&state, id, &user).await
}

#[derive(Debug, Deserialize, ToSchema)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct CastVote {
    pub clip_id: Uuid,
}

/// Your vote in the finale for clip (`clip`) or fail (`fail`) of the night.
#[utoipa::path(
    put,
    path = "/shows/{id}/votes/{category}",
    tag = "shows",
    security(("bearer" = [])),
    params(
        ("id" = Uuid, Path, description = "Show id"),
        ("category" = inline(Category), Path, description = "`clip` or `fail`"),
    ),
    request_body = CastVote,
    responses(
        (status = 200, description = "The show", body = ShowView),
        (status = 400, description = "Not a clip you can vote for", body = ErrorBody),
        (status = 403, description = "Not in the show", body = ErrorBody),
        (status = 404, description = "No such show, or shows aren't open to you yet", body = ErrorBody),
        (status = 409, description = "Not in the finale", body = ErrorBody),
    )
)]
pub async fn vote(
    State(state): State<AppState>,
    CurrentUser(user): CurrentUser,
    Path((id, category)): Path<(Uuid, Category)>,
    Json(body): Json<CastVote>,
) -> Result<Json<ShowView>, ApiError> {
    allowed(&state, &user)?;
    shows::vote(&state.pool, id, user.id, category, body.clip_id).await?;
    state.hub.show_changed(id);
    reload(&state, id, &user).await
}

#[derive(Debug, Default, Deserialize, ToSchema)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct EndShow {
    /// The host's picks for tied categories.
    #[serde(default)]
    pub tie_break: TieBreak,
}

/// The host ends the show: winners are stored. A tied category needs the host's pick
/// (409 with the tied clips of every tied category in `tied`). The body is optional.
#[utoipa::path(
    post,
    path = "/shows/{id}/end",
    tag = "shows",
    security(("bearer" = [])),
    params(("id" = Uuid, Path, description = "Show id")),
    request_body = Option<EndShow>,
    responses(
        (status = 200, description = "The ended show", body = ShowView),
        (status = 400, description = "Unreadable body", body = ErrorBody),
        (status = 403, description = "Not the host", body = ErrorBody),
        (status = 404, description = "No such show, or shows aren't open to you yet", body = ErrorBody),
        (status = 409, description = "A vote is tied (`tied` names the clips), or not in the finale", body = ErrorBody),
    )
)]
pub async fn end_show(
    State(state): State<AppState>,
    CurrentUser(user): CurrentUser,
    Path(id): Path<Uuid>,
    body: Option<Json<EndShow>>,
) -> Result<Json<ShowView>, ApiError> {
    allowed(&state, &user)?;
    let tie_break = body.map(|Json(b)| b.tie_break).unwrap_or_default();
    shows::end(&state.pool, id, user.id, tie_break).await?;
    state.hub.show_changed(id);
    reload(&state, id, &user).await
}
