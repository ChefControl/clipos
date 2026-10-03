//! Clips: upload (create + complete), browse, view, edit, react, delete/restore, download.
//!
//! Video bytes never pass through the API: the browser uploads straight to Blob Storage
//! with a write SAS for one blob and plays or downloads with read SAS URLs.

use std::{
    collections::{BTreeMap, HashMap},
    time::Duration,
};

use axum::{extract::State, http::StatusCode};
use chrono::{DateTime, Utc};
use clipos_core::{
    analysis,
    clips::{
        self, Clip, ClipEdit, ClipStatus, Cursor, Filter, NewClip, NewClipError, Sort, VIEW_TTL,
    },
    shares, shows,
    social::{self, Member, ReactionCount, SocialError},
    storage::{Access, Container, Overrides},
    users::{Role, User},
};
use serde::{Deserialize, Serialize};
use utoipa::{IntoParams, ToSchema};
use uuid::Uuid;

use crate::{
    AppState, ErrorBody,
    error::ApiError,
    extract::{CurrentUser, Json, Path, Query},
};

const PAGE_SIZE: i64 = 24;

/// How long a browser has to finish uploading `bytes`: 15 minutes, plus a minute per
/// 100 MB, at most 2 hours. Short, because the write SAS can't be taken back after
/// `complete`: until it expires it could write over the original (the worker checks the
/// ETag `complete` kept).
fn upload_ttl(bytes: i64) -> Duration {
    const MB_100: u64 = 100 * 1000 * 1000;
    let per_100_mb = (bytes.max(0) as u64).div_ceil(MB_100);
    Duration::from_secs((15 + per_100_mb) * 60).min(Duration::from_secs(2 * 3600))
}

#[derive(Debug, Serialize, ToSchema)]
#[serde(rename_all = "camelCase")]
pub struct Uploader {
    pub handle: String,
    pub display_name: String,
    pub avatar_url: Option<String>,
}

#[derive(Debug, Serialize, ToSchema)]
#[serde(rename_all = "camelCase")]
pub struct ClipView {
    pub id: Uuid,
    pub title: String,
    pub description: String,
    pub game_id: String,
    pub map: Option<String>,
    pub my_pov: bool,
    pub status: ClipStatus,
    /// Why processing failed. Only shown to the uploader.
    pub error: Option<String>,
    /// The kind of failure, for the uploader's failed screen.
    #[schema(inline)]
    pub failure_reason: Option<clips::FailureReason>,
    pub uploader: Uploader,
    pub is_mine: bool,
    /// The viewer may edit and delete it (uploader or admin).
    pub can_edit: bool,
    pub original_filename: String,
    pub original_bytes: i64,
    pub duration_ms: Option<i32>,
    pub width: Option<i32>,
    pub height: Option<i32>,
    pub fps: Option<f32>,
    /// The uploader's tags (what editing the clip changes).
    pub tags: Vec<String>,
    /// Tags found by killfeed analysis, e.g. `4k`, `hs`, `smoke-kill`.
    pub auto_tags: Vec<String>,
    /// The uploader's best round from killfeed analysis: `2k`, `3k`, `4k` or `ace`.
    pub multi_kill: Option<String>,
    /// Friends tagged as playing in it.
    pub players: Vec<Member>,
    pub reactions: Vec<ReactionCount>,
    pub reaction_count: i32,
    /// Read SAS (2 h) once ready; only on the single-clip endpoints.
    pub playback_url: Option<String>,
    /// Read SAS (2 h) once ready.
    pub poster_url: Option<String>,
    pub created_at: DateTime<Utc>,
    /// Set while it's in the trash (restorable for 7 days).
    pub deleted_at: Option<DateTime<Utc>>,
    /// Public link while sharing is on; only shown to those who can manage it.
    pub share_url: Option<String>,
    /// Saved for the show until then; only shown to the uploader.
    pub held_until: Option<DateTime<Utc>>,
    /// Someone else's clip saved for the show: only its title, uploader, map and length,
    /// with a blurred poster. No tags, killfeed or reactions, and no playback except for
    /// people in the live show whose lineup has it (`GET /api/clips/{id}`, to preload it).
    pub teaser: bool,
}

fn can_edit(clip: &Clip, viewer: &User) -> bool {
    clip.owner_id == viewer.id || viewer.role == Role::Admin
}

async fn read_url(
    state: &AppState,
    container: Container,
    blob: Option<&str>,
) -> Result<Option<String>, ApiError> {
    match blob {
        Some(blob) => Ok(Some(
            state
                .storage
                .sas_url(
                    container,
                    blob,
                    Access::Read,
                    VIEW_TTL,
                    &Overrides::default(),
                )
                .await?
                .to_string(),
        )),
        None => Ok(None),
    }
}

/// Builds views for several clips with one query each for tags, players and reactions.
pub(crate) async fn views(
    state: &AppState,
    clips: Vec<Clip>,
    viewer: &User,
    with_playback: bool,
) -> Result<Vec<ClipView>, ApiError> {
    let ids: Vec<Uuid> = clips.iter().map(|c| c.id).collect();
    let mut tags = social::tags_for(&state.pool, &ids).await?;
    let mut multi_kills = analysis::multi_kills(&state.pool, &ids).await?;
    let mut players = social::players_for(&state.pool, &ids).await?;
    let mut reactions = social::reactions_for(&state.pool, &ids, viewer.id).await?;
    let mut share_tokens = shares::active_tokens(&state.pool, &ids).await?;

    let mut out = Vec::with_capacity(clips.len());
    for clip in clips {
        let ready = clip.status == ClipStatus::Ready;
        let is_mine = clip.owner_id == viewer.id;
        // Someone else's clip saved for the show (decision 29).
        let held = clip.is_held();
        let teaser = held && !is_mine;
        if teaser {
            out.push(teaser_view(state, clip).await?);
            continue;
        }
        let poster_url = if ready {
            read_url(state, Container::Posters, clip.poster_blob.as_deref()).await?
        } else {
            None
        };
        let playback_url = if ready && with_playback {
            read_url(state, Container::Playback, clip.playback_blob.as_deref()).await?
        } else {
            None
        };
        let can_edit = can_edit(&clip, viewer);
        let share_url = share_tokens
            .remove(&clip.id)
            .filter(|_| can_edit)
            .map(|token| crate::routes::share::share_url(state, &token));
        let tags = tags.remove(&clip.id).unwrap_or_default();
        out.push(ClipView {
            share_url,
            tags: tags.user,
            auto_tags: tags.auto,
            multi_kill: multi_kills.remove(&clip.id),
            players: players.remove(&clip.id).unwrap_or_default(),
            reactions: reactions.remove(&clip.id).unwrap_or_default(),
            id: clip.id,
            title: clip.title,
            description: clip.description,
            game_id: clip.game_id,
            map: clip.map,
            my_pov: clip.my_pov,
            status: clip.status,
            failure_reason: (is_mine && clip.status == ClipStatus::Failed)
                .then(|| clip.error.as_deref().map(clips::FailureReason::of))
                .flatten(),
            error: if is_mine { clip.error } else { None },
            uploader: Uploader {
                handle: clip.owner_handle,
                display_name: clip.owner_display_name,
                avatar_url: clip.owner_avatar_url,
            },
            is_mine,
            can_edit,
            original_filename: clip.original_filename,
            original_bytes: clip.original_bytes,
            duration_ms: clip.duration_ms,
            width: clip.width,
            height: clip.height,
            fps: clip.fps,
            reaction_count: clip.reaction_count,
            playback_url,
            poster_url,
            created_at: clip.created_at,
            deleted_at: clip.deleted_at,
            held_until: if is_mine && held {
                clip.hold_until
            } else {
                None
            },
            teaser: false,
        });
    }
    Ok(out)
}

/// What others see of a clip saved for the show: title, uploader, map, length and a
/// blurred poster (the worker makes it). Nothing that spoils it. Nobody else can open it
/// (`visible_clip`), admins included, so it offers no editing.
async fn teaser_view(state: &AppState, clip: Clip) -> Result<ClipView, ApiError> {
    let poster_url = match clip.status {
        ClipStatus::Ready => {
            read_url(
                state,
                Container::Posters,
                Some(&clips::teaser_blob(clip.id)),
            )
            .await?
        }
        _ => None,
    };
    Ok(ClipView {
        id: clip.id,
        title: clip.title,
        description: String::new(),
        game_id: clip.game_id,
        map: clip.map,
        my_pov: clip.my_pov,
        status: clip.status,
        error: None,
        failure_reason: None,
        uploader: Uploader {
            handle: clip.owner_handle,
            display_name: clip.owner_display_name,
            avatar_url: clip.owner_avatar_url,
        },
        is_mine: false,
        can_edit: false,
        original_filename: String::new(),
        original_bytes: 0,
        duration_ms: clip.duration_ms,
        width: clip.width,
        height: clip.height,
        fps: clip.fps,
        tags: Vec::new(),
        auto_tags: Vec::new(),
        multi_kill: None,
        players: Vec::new(),
        reactions: Vec::new(),
        reaction_count: 0,
        playback_url: None,
        poster_url,
        created_at: clip.created_at,
        deleted_at: None,
        share_url: None,
        held_until: None,
        teaser: true,
    })
}

async fn view(state: &AppState, clip: Clip, viewer: &User) -> Result<ClipView, ApiError> {
    Ok(views(state, vec![clip], viewer, true).await?.remove(0))
}

/// A clip the viewer may see: not deleted, or deleted but theirs to restore. Unfinished
/// uploads are private to their uploader; clips still processing or that failed to their
/// uploader and admins (decision 42).
async fn visible_clip(state: &AppState, id: Uuid, viewer: &User) -> Result<Clip, ApiError> {
    let clip = clips::get_including_deleted(&state.pool, id)
        .await?
        .ok_or(ApiError::NotFound)?;
    let hidden = (clip.deleted_at.is_some() && !can_edit(&clip, viewer))
        || (clip.status == ClipStatus::Uploading && clip.owner_id != viewer.id)
        || (clip.status != ClipStatus::Ready && !can_edit(&clip, viewer))
        // Saved for the show: only the uploader opens it until it's released.
        || (clip.is_held() && clip.owner_id != viewer.id);
    if hidden {
        return Err(ApiError::NotFound);
    }
    Ok(clip)
}

/// A clip the viewer may change (uploader or admin).
pub(crate) async fn editable_clip(
    state: &AppState,
    id: Uuid,
    viewer: &User,
) -> Result<Clip, ApiError> {
    let clip = visible_clip(state, id, viewer).await?;
    if !can_edit(&clip, viewer) {
        return Err(ApiError::Forbidden(
            "only the uploader or an admin can do that".into(),
        ));
    }
    Ok(clip)
}

pub(crate) async fn reload(
    state: &AppState,
    id: Uuid,
    viewer: &User,
) -> Result<Json<ClipView>, ApiError> {
    let clip = clips::get_including_deleted(&state.pool, id)
        .await?
        .ok_or(ApiError::NotFound)?;
    Ok(Json(view(state, clip, viewer).await?))
}

/// An editable clip that's published (ready and not in the trash), e.g. to share it.
pub(crate) async fn editable_ready_clip(
    state: &AppState,
    id: Uuid,
    viewer: &User,
) -> Result<Clip, ApiError> {
    let clip = editable_clip(state, id, viewer).await?;
    if clip.status != ClipStatus::Ready || clip.deleted_at.is_some() {
        return Err(ApiError::BadRequest(
            "only published clips can be shared".into(),
        ));
    }
    Ok(clip)
}

fn invalid(e: NewClipError) -> ApiError {
    match e {
        NewClipError::Invalid(m) => ApiError::BadRequest(m),
        NewClipError::Database(e) => e.into(),
    }
}

fn social_err(e: SocialError) -> ApiError {
    match e {
        SocialError::Invalid(m) => ApiError::BadRequest(m),
        SocialError::Database(e) => e.into(),
    }
}

// ---------------------------------------------------------------------------
// Upload
// ---------------------------------------------------------------------------

#[derive(Debug, Deserialize, ToSchema)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct CreateClip {
    /// 1–100 characters.
    pub title: String,
    #[serde(default)]
    pub description: String,
    /// Defaults to `cs2`.
    pub game_id: Option<String>,
    pub map: Option<String>,
    /// Recorded from your own point of view.
    #[serde(default = "yes")]
    pub my_pov: bool,
    /// Original file name; `.mp4`, `.mkv` or `.mov`.
    pub filename: String,
    /// File size; at most 2 GiB.
    pub bytes: i64,
    /// "Save it for the show": hidden from others until it plays in a show, you post it,
    /// or a week passes. Ignored while shows aren't open to you.
    #[serde(default)]
    pub hold: bool,
}

fn yes() -> bool {
    true
}

#[derive(Debug, Serialize, ToSchema)]
#[serde(rename_all = "camelCase")]
pub struct CreatedClip {
    pub clip: ClipView,
    /// Write SAS for the original: upload with Put Block + Put Block List, then call
    /// `POST /api/clips/{id}/complete`. Valid for 15 minutes plus a minute per 100 MB
    /// (at most 2 h).
    pub upload_url: String,
}

/// Start an upload.
#[utoipa::path(
    post,
    path = "/clips",
    tag = "clips",
    security(("bearer" = [])),
    request_body = CreateClip,
    responses(
        (status = 201, description = "Clip created; upload to `uploadUrl`", body = CreatedClip),
        (status = 400, description = "Invalid title, file type or size", body = ErrorBody),
    )
)]
pub async fn create_clip(
    State(state): State<AppState>,
    CurrentUser(user): CurrentUser,
    Json(body): Json<CreateClip>,
) -> Result<(StatusCode, Json<CreatedClip>), ApiError> {
    let clip = clips::create(
        &state.pool,
        NewClip {
            owner_id: user.id,
            game_id: body.game_id.unwrap_or_else(|| "cs2".into()),
            title: body.title,
            description: body.description,
            map: body.map,
            my_pov: body.my_pov,
            filename: body.filename,
            bytes: body.bytes,
        },
    )
    .await
    .map_err(invalid)?;
    let clip = if body.hold && crate::routes::shows::shows_open_to(&state, &user) {
        clips::hold(&state.pool, clip.id).await?;
        clips::get_including_deleted(&state.pool, clip.id)
            .await?
            .ok_or(ApiError::NotFound)?
    } else {
        clip
    };
    let upload_url = state
        .storage
        .sas_url(
            Container::Originals,
            &clip.original_blob,
            Access::Write,
            upload_ttl(clip.original_bytes),
            &Overrides::default(),
        )
        .await?;
    tracing::info!(clip_id = %clip.id, bytes = clip.original_bytes, "upload started");
    Ok((
        StatusCode::CREATED,
        Json(CreatedClip {
            clip: view(&state, clip, &user).await?,
            upload_url: upload_url.to_string(),
        }),
    ))
}

async fn own_clip(state: &AppState, id: Uuid, user: &User) -> Result<Clip, ApiError> {
    let clip = clips::get(&state.pool, id)
        .await?
        .ok_or(ApiError::NotFound)?;
    if clip.owner_id != user.id {
        return Err(ApiError::Forbidden("only the uploader can do that".into()));
    }
    Ok(clip)
}

/// Finish an upload: checks the blob is complete, keeps its ETag and queues the
/// transcode. Safe to call again (e.g. after a network error).
#[utoipa::path(
    post,
    path = "/clips/{id}/complete",
    tag = "clips",
    security(("bearer" = [])),
    params(("id" = Uuid, Path, description = "Clip id")),
    responses(
        (status = 200, description = "Processing", body = ClipView),
        (status = 400, description = "Upload missing or incomplete", body = ErrorBody),
        (status = 403, description = "Not your clip", body = ErrorBody),
        (status = 404, description = "No such clip", body = ErrorBody),
    )
)]
pub async fn complete_clip(
    State(state): State<AppState>,
    CurrentUser(user): CurrentUser,
    Path(id): Path<Uuid>,
) -> Result<Json<ClipView>, ApiError> {
    let clip = own_clip(&state, id, &user).await?;
    if clip.status == ClipStatus::Uploading {
        let blob = state
            .storage
            .blob_props(Container::Originals, &clip.original_blob)
            .await?
            .ok_or_else(|| ApiError::BadRequest("nothing has been uploaded yet".into()))?;
        let size = blob.size as i64;
        if size > clip.original_bytes {
            return Err(ApiError::BadRequest(format!(
                "the upload is bigger than declared: {size} bytes, not {}",
                clip.original_bytes
            )));
        }
        if size < clip.original_bytes {
            return Err(ApiError::BadRequest(format!(
                "upload incomplete: {size} of {} bytes",
                clip.original_bytes
            )));
        }
        clips::set_original_etag(&state.pool, id, &blob.etag).await?;
        clips::mark_uploaded(&state.pool, id).await?;
        tracing::info!(clip_id = %id, "upload complete; transcode queued");
    }
    reload(&state, id, &user).await
}

/// Retry processing a failed clip.
#[utoipa::path(
    post,
    path = "/clips/{id}/retry",
    tag = "clips",
    security(("bearer" = [])),
    params(("id" = Uuid, Path, description = "Clip id")),
    responses(
        (status = 200, description = "Processing again", body = ClipView),
        (status = 400, description = "The clip hasn't failed", body = ErrorBody),
        (status = 403, description = "Not your clip", body = ErrorBody),
    )
)]
pub async fn retry_clip(
    State(state): State<AppState>,
    CurrentUser(user): CurrentUser,
    Path(id): Path<Uuid>,
) -> Result<Json<ClipView>, ApiError> {
    own_clip(&state, id, &user).await?;
    if !clips::retry(&state.pool, id).await? {
        return Err(ApiError::BadRequest(
            "only failed clips can be retried".into(),
        ));
    }
    reload(&state, id, &user).await
}

/// "Save it for the show" after uploading (the upload starts before the switch is set).
/// The hold ends a week after the upload however often it's set (decision 29).
#[utoipa::path(
    post,
    path = "/clips/{id}/hold",
    tag = "clips",
    security(("bearer" = [])),
    params(("id" = Uuid, Path, description = "Clip id")),
    responses(
        (status = 200, description = "Saved for the show", body = ClipView),
        (status = 403, description = "Not your clip", body = ErrorBody),
        (status = 404, description = "Shows aren't open to you yet", body = ErrorBody),
        (status = 409, description = "Uploaded over a week ago, or already played in a show", body = ErrorBody),
    )
)]
pub async fn hold_clip(
    State(state): State<AppState>,
    CurrentUser(user): CurrentUser,
    Path(id): Path<Uuid>,
) -> Result<Json<ClipView>, ApiError> {
    if !crate::routes::shows::shows_open_to(&state, &user) {
        return Err(ApiError::NotFound);
    }
    own_clip(&state, id, &user).await?;
    if !clips::hold(&state.pool, id).await? {
        return Err(ApiError::Conflict(
            "this clip can't be saved for the show any more: it was uploaded over a week ago \
             or has played in a show"
                .into(),
        ));
    }
    reload(&state, id, &user).await
}

/// "Post now": a clip saved for the show becomes visible to everyone.
#[utoipa::path(
    post,
    path = "/clips/{id}/release",
    tag = "clips",
    security(("bearer" = [])),
    params(("id" = Uuid, Path, description = "Clip id")),
    responses(
        (status = 200, description = "Visible to everyone", body = ClipView),
        (status = 403, description = "Not your clip", body = ErrorBody),
    )
)]
pub async fn release_clip(
    State(state): State<AppState>,
    CurrentUser(user): CurrentUser,
    Path(id): Path<Uuid>,
) -> Result<Json<ClipView>, ApiError> {
    own_clip(&state, id, &user).await?;
    clips::release(&state.pool, id).await?;
    reload(&state, id, &user).await
}

// ---------------------------------------------------------------------------
// Browse
// ---------------------------------------------------------------------------

#[derive(Debug, Deserialize, IntoParams)]
#[serde(rename_all = "camelCase")]
#[into_params(parameter_in = Query)]
pub struct ClipQuery {
    /// `new` (default) or `top` (most reactions).
    #[param(inline)]
    pub sort: Option<Sort>,
    pub game: Option<String>,
    pub map: Option<String>,
    /// Uploader's handle.
    pub uploader: Option<String>,
    /// A tag, or several separated by commas: clips with any of them (`4k,ace`). Tags
    /// with nothing usable in them (`!!!`) match no clip.
    pub tag: Option<String>,
    /// Handle of a friend playing in the clip.
    pub player: Option<String>,
    /// Search the title, description, map and uploader.
    pub q: Option<String>,
    /// Clips someone reacted to with this emoji (one of the six reactions).
    pub reaction: Option<String>,
    /// `nextCursor` from the previous page.
    pub cursor: Option<String>,
}

#[derive(Debug, Serialize, ToSchema)]
#[serde(rename_all = "camelCase")]
pub struct ClipPage {
    pub clips: Vec<ClipView>,
    /// Pass as `cursor` for the next page; absent on the last page.
    pub next_cursor: Option<String>,
}

fn non_empty(v: Option<String>) -> Option<String> {
    v.map(|s| s.trim().to_owned()).filter(|s| !s.is_empty())
}

/// Clips, newest or most-reacted first, with optional filters. Everyone's ready clips plus
/// your own that are processing or failed.
#[utoipa::path(
    get,
    path = "/clips",
    tag = "clips",
    security(("bearer" = [])),
    params(ClipQuery),
    responses(
        (status = 200, description = "A page of clips", body = ClipPage),
        (status = 400, description = "Bad cursor", body = ErrorBody),
    )
)]
pub async fn list_clips(
    State(state): State<AppState>,
    CurrentUser(user): CurrentUser,
    Query(q): Query<ClipQuery>,
) -> Result<Json<ClipPage>, ApiError> {
    let cursor = match non_empty(q.cursor) {
        Some(raw) => {
            Some(Cursor::decode(&raw).ok_or_else(|| ApiError::BadRequest("bad cursor".into()))?)
        }
        None => None,
    };
    let reaction = non_empty(q.reaction);
    if let Some(r) = &reaction
        && !social::EMOJIS.contains(&r.as_str())
    {
        return Err(ApiError::BadRequest("unknown reaction".into()));
    }
    let tags: Option<Vec<String>> =
        non_empty(q.tag).map(|t| t.split(',').filter_map(social::normalize_tag).collect());
    // Like a tag nobody used: `?tag=!!!` has no usable tag, and no tags would mean "any".
    if tags.as_ref().is_some_and(Vec::is_empty) {
        return Ok(Json(ClipPage {
            clips: Vec::new(),
            next_cursor: None,
        }));
    }
    let filter = Filter {
        game: non_empty(q.game),
        map: non_empty(q.map),
        uploader: non_empty(q.uploader).map(|h| h.to_lowercase()),
        tags: tags.unwrap_or_default(),
        player: non_empty(q.player).map(|h| h.to_lowercase()),
        search: non_empty(q.q).map(|s| s.chars().take(100).collect()),
        reaction,
    };
    let (clips, next) = clips::list(
        &state.pool,
        user.id,
        &filter,
        q.sort.unwrap_or_default(),
        cursor.as_ref(),
        PAGE_SIZE,
    )
    .await?;
    Ok(Json(ClipPage {
        clips: views(&state, clips, &user, false).await?,
        next_cursor: next.map(|c| c.encode()),
    }))
}

/// One clip, with playback links once it's ready.
#[utoipa::path(
    get,
    path = "/clips/{id}",
    tag = "clips",
    security(("bearer" = [])),
    params(("id" = Uuid, Path, description = "Clip id")),
    responses(
        (status = 200, description = "Clip", body = ClipView),
        (status = 404, description = "No such clip", body = ErrorBody),
    )
)]
pub async fn get_clip(
    State(state): State<AppState>,
    CurrentUser(user): CurrentUser,
    Path(id): Path<Uuid>,
) -> Result<Json<ClipView>, ApiError> {
    match visible_clip(&state, id, &user).await {
        Ok(clip) => Ok(Json(view(&state, clip, &user).await?)),
        Err(ApiError::NotFound) => show_clip(&state, id, &user).await,
        Err(e) => Err(e),
    }
}

/// Someone else's clip saved for the show, for a viewer in the live show whose lineup
/// has it: the teaser plus its playback link, so the show page can preload the next clip
/// before loading it releases the hold (decision 35). Lists, search, profiles and
/// reactions still don't show it.
async fn show_clip(state: &AppState, id: Uuid, viewer: &User) -> Result<Json<ClipView>, ApiError> {
    let clip = clips::get(&state.pool, id)
        .await?
        .filter(|c| c.is_held() && c.status == ClipStatus::Ready)
        .ok_or(ApiError::NotFound)?;
    if !crate::routes::shows::shows_open_to(state, viewer)
        || !shows::in_live_lineup(&state.pool, id, viewer.id).await?
    {
        return Err(ApiError::NotFound);
    }
    let playback = read_url(state, Container::Playback, clip.playback_blob.as_deref()).await?;
    let mut view = teaser_view(state, clip).await?;
    view.playback_url = playback;
    Ok(Json(view))
}

/// Your clips in the trash (restorable for 7 days after deletion).
#[utoipa::path(
    get,
    path = "/me/trash",
    tag = "clips",
    security(("bearer" = [])),
    responses((status = 200, description = "Deleted clips, most recent first", body = [ClipView]))
)]
pub async fn trash(
    State(state): State<AppState>,
    CurrentUser(user): CurrentUser,
) -> Result<Json<Vec<ClipView>>, ApiError> {
    let clips = clips::trash(&state.pool, user.id).await?;
    Ok(Json(views(&state, clips, &user, false).await?))
}

// ---------------------------------------------------------------------------
// Edit, delete, restore
// ---------------------------------------------------------------------------

/// Fields to change; omitted fields stay as they are.
#[derive(Debug, Deserialize, ToSchema)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct UpdateClip {
    pub title: Option<String>,
    pub description: Option<String>,
    /// `null` or an empty string clears it.
    #[serde(default, deserialize_with = "present")]
    #[schema(value_type = Option<String>)]
    pub map: Option<Option<String>>,
    pub my_pov: Option<bool>,
    /// Replaces the tags (up to 10; letters, digits and `-`).
    pub tags: Option<Vec<String>>,
    /// Replaces the friends tagged as playing (user ids, up to 10). Friends tagged
    /// already may stay after they've been disabled; new ones must be active members.
    pub players: Option<Vec<Uuid>>,
}

/// A field that's in the body, `null` included (`Some(None)`); one that's left out is
/// `None` (with `#[serde(default)]`).
fn present<'de, D, T>(d: D) -> Result<Option<Option<T>>, D::Error>
where
    D: serde::Deserializer<'de>,
    T: Deserialize<'de>,
{
    Option::<T>::deserialize(d).map(Some)
}

/// Edit a clip (uploader or admin). All of it is saved, or nothing is. Not while it's in
/// the trash.
#[utoipa::path(
    patch,
    path = "/clips/{id}",
    tag = "clips",
    security(("bearer" = [])),
    params(("id" = Uuid, Path, description = "Clip id")),
    request_body = UpdateClip,
    responses(
        (status = 200, description = "Updated clip", body = ClipView),
        (status = 400, description = "Invalid field", body = ErrorBody),
        (status = 403, description = "Not yours", body = ErrorBody),
        (status = 404, description = "No such clip, or it's in the trash", body = ErrorBody),
    )
)]
pub async fn update_clip(
    State(state): State<AppState>,
    CurrentUser(user): CurrentUser,
    Path(id): Path<Uuid>,
    Json(body): Json<UpdateClip>,
) -> Result<Json<ClipView>, ApiError> {
    let clip = editable_clip(&state, id, &user).await?;
    if clip.deleted_at.is_some() {
        return Err(ApiError::NotFound);
    }
    let mut tx = state.pool.begin().await?;
    clips::update(
        &mut *tx,
        id,
        ClipEdit {
            title: body.title,
            description: body.description,
            map: body.map,
            my_pov: body.my_pov,
        },
    )
    .await
    .map_err(invalid)?;
    if let Some(tags) = &body.tags {
        social::set_tags(&mut *tx, id, tags)
            .await
            .map_err(social_err)?;
    }
    if let Some(players) = &body.players {
        social::set_players(&mut *tx, id, players)
            .await
            .map_err(social_err)?;
    }
    tx.commit().await?;
    reload(&state, id, &user).await
}

/// Move a clip to the trash; it can be restored for 7 days. Its share link is revoked.
#[utoipa::path(
    delete,
    path = "/clips/{id}",
    tag = "clips",
    security(("bearer" = [])),
    params(("id" = Uuid, Path, description = "Clip id")),
    responses(
        (status = 200, description = "In the trash", body = ClipView),
        (status = 403, description = "Not yours", body = ErrorBody),
    )
)]
pub async fn delete_clip(
    State(state): State<AppState>,
    CurrentUser(user): CurrentUser,
    Path(id): Path<Uuid>,
) -> Result<Json<ClipView>, ApiError> {
    editable_clip(&state, id, &user).await?;
    clips::soft_delete(&state.pool, id).await?;
    tracing::info!(clip_id = %id, by = %user.handle, "clip deleted");
    // In the lineup of the show that's on: its live room unloads the clip if it's loaded.
    if let Some(show) = shows::open(&state.pool).await?
        && shows::lineup(&state.pool, show.id)
            .await?
            .iter()
            .any(|l| l.clip_id == id)
    {
        state.hub.show_changed(show.id);
    }
    reload(&state, id, &user).await
}

/// Bring a clip back from the trash.
#[utoipa::path(
    post,
    path = "/clips/{id}/restore",
    tag = "clips",
    security(("bearer" = [])),
    params(("id" = Uuid, Path, description = "Clip id")),
    responses(
        (status = 200, description = "Restored", body = ClipView),
        (status = 400, description = "Not in the trash", body = ErrorBody),
        (status = 403, description = "Not yours", body = ErrorBody),
        (status = 409, description = "In the trash for over 7 days: being deleted", body = ErrorBody),
    )
)]
pub async fn restore_clip(
    State(state): State<AppState>,
    CurrentUser(user): CurrentUser,
    Path(id): Path<Uuid>,
) -> Result<Json<ClipView>, ApiError> {
    let clip = editable_clip(&state, id, &user).await?;
    if !clips::restore(&state.pool, id).await? {
        return Err(match clip.deleted_at {
            None => ApiError::BadRequest("the clip isn't in the trash".into()),
            Some(_) => ApiError::Conflict(
                "the clip was in the trash for over 7 days and is being deleted".into(),
            ),
        });
    }
    reload(&state, id, &user).await
}

#[derive(Debug, Serialize, ToSchema)]
#[serde(rename_all = "camelCase")]
pub struct DownloadLink {
    /// Read SAS for the original that downloads with its file name (2 h).
    pub url: String,
}

/// Link to download the original upload.
#[utoipa::path(
    get,
    path = "/clips/{id}/download",
    tag = "clips",
    security(("bearer" = [])),
    params(("id" = Uuid, Path, description = "Clip id")),
    responses(
        (status = 200, description = "Download link", body = DownloadLink),
        (status = 404, description = "No such clip, or not uploaded yet", body = ErrorBody),
    )
)]
pub async fn download_clip(
    State(state): State<AppState>,
    CurrentUser(user): CurrentUser,
    Path(id): Path<Uuid>,
) -> Result<Json<DownloadLink>, ApiError> {
    let clip = visible_clip(&state, id, &user).await?;
    if clip.status == ClipStatus::Uploading {
        return Err(ApiError::NotFound);
    }
    // The file name is already restricted to [A-Za-z0-9._-] (clips::sanitize_filename).
    let url = state
        .storage
        .sas_url(
            Container::Originals,
            &clip.original_blob,
            Access::Read,
            VIEW_TTL,
            &Overrides {
                content_disposition: Some(format!(
                    "attachment; filename=\"{}\"",
                    clip.original_filename
                )),
                content_type: None,
            },
        )
        .await?;
    Ok(Json(DownloadLink {
        url: url.to_string(),
    }))
}

// ---------------------------------------------------------------------------
// Reactions
// ---------------------------------------------------------------------------

async fn set_reaction(
    state: &AppState,
    user: &User,
    id: Uuid,
    emoji: &str,
    on: bool,
) -> Result<Json<Vec<ReactionCount>>, ApiError> {
    let clip = visible_clip(state, id, user).await?;
    if clip.status != ClipStatus::Ready || clip.deleted_at.is_some() {
        return Err(ApiError::BadRequest(
            "you can only react to published clips".into(),
        ));
    }
    social::react(&state.pool, id, user.id, emoji, on)
        .await
        .map_err(social_err)?;
    let mut all: HashMap<Uuid, Vec<ReactionCount>> =
        social::reactions_for(&state.pool, &[id], user.id).await?;
    Ok(Json(all.remove(&id).unwrap_or_default()))
}

/// React to a clip. Allowed emojis: 🔥 😂 💀 🐐 😮 👏.
#[utoipa::path(
    put,
    path = "/clips/{id}/reactions/{emoji}",
    tag = "clips",
    security(("bearer" = [])),
    params(
        ("id" = Uuid, Path, description = "Clip id"),
        ("emoji" = String, Path, description = "Emoji (URL-encoded)"),
    ),
    responses(
        (status = 200, description = "The clip's reactions", body = [ReactionCount]),
        (status = 400, description = "Unsupported emoji or unpublished clip", body = ErrorBody),
    )
)]
pub async fn add_reaction(
    State(state): State<AppState>,
    CurrentUser(user): CurrentUser,
    Path((id, emoji)): Path<(Uuid, String)>,
) -> Result<Json<Vec<ReactionCount>>, ApiError> {
    set_reaction(&state, &user, id, &emoji, true).await
}

/// Remove your reaction.
#[utoipa::path(
    delete,
    path = "/clips/{id}/reactions/{emoji}",
    tag = "clips",
    security(("bearer" = [])),
    params(
        ("id" = Uuid, Path, description = "Clip id"),
        ("emoji" = String, Path, description = "Emoji (URL-encoded)"),
    ),
    responses((status = 200, description = "The clip's reactions", body = [ReactionCount]))
)]
pub async fn remove_reaction(
    State(state): State<AppState>,
    CurrentUser(user): CurrentUser,
    Path((id, emoji)): Path<(Uuid, String)>,
) -> Result<Json<Vec<ReactionCount>>, ApiError> {
    set_reaction(&state, &user, id, &emoji, false).await
}

// ---------------------------------------------------------------------------
// Killfeed analysis
// ---------------------------------------------------------------------------

#[derive(Debug, Serialize, ToSchema)]
#[serde(rename_all = "lowercase")]
pub enum AnalysisStatus {
    /// The worker is reading the killfeed.
    Pending,
    Done,
}

/// The uploader's stats from the killfeed.
#[derive(Debug, Serialize, ToSchema)]
#[serde(rename_all = "camelCase")]
pub struct AnalysisStats {
    /// Kills in the clip, anyone's.
    pub kills: u32,
    pub my_kills: u32,
    pub my_deaths: u32,
    /// Most of the uploader's kills in one round: `2k`, `3k`, `4k` or `ace`.
    pub multi_kill: Option<String>,
    /// The uploader's kills per weapon (CS2 names: `ak47`, `awp`, ...).
    pub weapons: BTreeMap<String, u32>,
    /// The uploader's kills per modifier (`headshot`, `wallbang`, `through_smoke`, ...).
    pub modifiers: BTreeMap<String, u32>,
}

#[derive(Debug, Serialize, ToSchema)]
#[serde(rename_all = "camelCase")]
pub enum KillOwner {
    /// A red-outlined row: the uploader's kill.
    MyKill,
    /// A red-filled row: the uploader died.
    MyDeath,
    Other,
}

/// One killfeed row, followed from when it appeared.
#[derive(Debug, Serialize, ToSchema)]
#[serde(rename_all = "camelCase")]
pub struct KillView {
    /// Seconds from the start of the clip.
    pub t: f64,
    pub owner: KillOwner,
    /// CS2's name, e.g. `ak47`; absent when it couldn't be read.
    pub weapon: Option<String>,
    /// `headshot`, `wallbang`, `through_smoke`, `noscope`, `blind`, `in_air`, ...
    pub modifiers: Vec<String>,
}

#[derive(Debug, Serialize, ToSchema)]
#[serde(rename_all = "camelCase")]
pub struct ClipAnalysis {
    pub status: AnalysisStatus,
    /// Absent while pending.
    pub stats: Option<AnalysisStats>,
    /// Every kill in the killfeed, in order.
    pub kills: Vec<KillView>,
}

// The analysis as the worker stores it (`clipos_killfeed::Summary` and `Kill`, snake_case).
// Kept apart from the views above so the OpenAPI schema describes what is actually sent.

#[derive(Deserialize)]
struct StoredStats {
    kills: u32,
    my_kills: u32,
    my_deaths: u32,
    multi_kill: Option<String>,
    weapons: BTreeMap<String, u32>,
    modifiers: BTreeMap<String, u32>,
}

#[derive(Deserialize)]
#[serde(rename_all = "snake_case")]
enum StoredOwner {
    MyKill,
    MyDeath,
    Other,
}

#[derive(Deserialize)]
struct StoredKill {
    t: f64,
    owner: StoredOwner,
    weapon: Option<String>,
    #[serde(default)]
    modifiers: Vec<String>,
}

#[derive(Deserialize)]
struct StoredRaw {
    #[serde(default)]
    kills: Vec<StoredKill>,
}

impl From<StoredStats> for AnalysisStats {
    fn from(s: StoredStats) -> Self {
        Self {
            kills: s.kills,
            my_kills: s.my_kills,
            my_deaths: s.my_deaths,
            multi_kill: s.multi_kill,
            weapons: s.weapons,
            modifiers: s.modifiers,
        }
    }
}

impl From<StoredKill> for KillView {
    fn from(k: StoredKill) -> Self {
        Self {
            t: k.t,
            owner: match k.owner {
                StoredOwner::MyKill => KillOwner::MyKill,
                StoredOwner::MyDeath => KillOwner::MyDeath,
                StoredOwner::Other => KillOwner::Other,
            },
            weapon: k.weapon,
            modifiers: k.modifiers,
        }
    }
}

/// What killfeed analysis read from a clip. 404 when it was never analysed (not CS2, or
/// not recorded from the uploader's point of view). Pending while an analysis is queued or
/// running, also when it will replace an earlier one (an admin's re-analyse).
#[utoipa::path(
    get,
    path = "/clips/{id}/analysis",
    tag = "clips",
    security(("bearer" = [])),
    params(("id" = Uuid, Path, description = "Clip id")),
    responses(
        (status = 200, description = "The analysis, or that it's pending", body = ClipAnalysis),
        (status = 404, description = "No such clip, or no analysis", body = ErrorBody),
    )
)]
pub async fn get_analysis(
    State(state): State<AppState>,
    CurrentUser(user): CurrentUser,
    Path(id): Path<Uuid>,
) -> Result<Json<ClipAnalysis>, ApiError> {
    visible_clip(&state, id, &user).await?;
    if analysis::pending(&state.pool, id).await? {
        return Ok(Json(ClipAnalysis {
            status: AnalysisStatus::Pending,
            stats: None,
            kills: Vec::new(),
        }));
    }
    let Some((_, raw, stats)) = analysis::get(&state.pool, id).await? else {
        return Err(ApiError::NotFound);
    };
    let stats: StoredStats = serde_json::from_value(stats)
        .map_err(|e| ApiError::Internal(anyhow::anyhow!("stored analysis stats: {e}")))?;
    let raw: StoredRaw = serde_json::from_value(raw)
        .map_err(|e| ApiError::Internal(anyhow::anyhow!("stored analysis kills: {e}")))?;
    Ok(Json(ClipAnalysis {
        status: AnalysisStatus::Done,
        stats: Some(stats.into()),
        kills: raw.kills.into_iter().map(Into::into).collect(),
    }))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn uploads_get_15_minutes_and_a_minute_per_100_mb() {
        let minutes = |bytes: i64| upload_ttl(bytes).as_secs() / 60;
        assert_eq!(minutes(1), 16);
        assert_eq!(minutes(100_000_000), 16);
        assert_eq!(minutes(100_000_001), 17);
        assert_eq!(minutes(clips::MAX_BYTES), 15 + 22);
        assert_eq!(minutes(i64::MAX), 120);
    }
}
