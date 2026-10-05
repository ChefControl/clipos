//! Clips: rows in `clips`, their lifecycle (`uploading` → `processing` → `ready` /
//! `failed`), and the naming of their blobs.

use std::time::Duration;

use chrono::{DateTime, Utc};
use serde::{Deserialize, Serialize};
use serde_json::{Value, json};
use sqlx::PgPool;
use utoipa::ToSchema;
use uuid::Uuid;

use crate::{dedup, jobs};

/// Upload size limit (2 GiB).
pub const MAX_BYTES: i64 = 2 * 1024 * 1024 * 1024;
/// Container formats accepted for upload.
pub const EXTENSIONS: [&str; 3] = ["mp4", "mkv", "mov"];
pub const TRANSCODE_JOB: &str = "transcode";
/// Killfeed analysis, queued after a successful transcode of a CS2 clip recorded from the
/// uploader's own point of view.
pub const ANALYSE_JOB: &str = "analyse";

/// Checks a ready clip's playback file has a keyframe at least every 4 s, and re-encodes
/// it if not (S5c: clips from before the show's 2 s keyframes; migration 0011 queues one
/// per clip, which 0013 moves to the background so uploads go first).
pub const KEYFRAMES_JOB: &str = "keyframes";

/// Fingerprints a clip from before duplicate detection (migration 0017): its original and
/// playback file.
pub const FINGERPRINT_JOB: &str = "fingerprint";

/// Deletes blobs no clip points at any more, e.g. the files a re-encode replaced, once
/// every read link to them has expired. Payload: `{"blobs": [{"container", "blob"}]}`.
pub const DELETE_BLOBS_JOB: &str = "delete_blobs";

/// Lifetime of playback, poster and download links; the SPA refetches well within this.
pub const VIEW_TTL: Duration = Duration::from_secs(2 * 3600);

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, sqlx::Type, ToSchema)]
#[sqlx(type_name = "text", rename_all = "lowercase")]
#[serde(rename_all = "lowercase")]
pub enum ClipStatus {
    Uploading,
    Processing,
    Ready,
    Failed,
}

/// Why a clip failed processing, for the uploader's failed screen. Worked out from the
/// stored message (`clips.error`), whose permanent forms the worker writes with the
/// constants below.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, ToSchema)]
#[serde(rename_all = "camelCase")]
pub enum FailureReason {
    /// Longer than 5 minutes: trim it and upload again.
    TooLong,
    /// No video stream in the file.
    NotAVideo,
    /// Not readable as video: incomplete or corrupt, or changed after the upload finished.
    Unreadable,
    /// The same file is here already, as another clip (`Clip::duplicate_of`).
    Duplicate,
    /// Something on our side; retrying may help.
    Server,
}

pub const TOO_LONG_ERROR: &str = "clips can be at most 5 minutes";
pub const NO_VIDEO_ERROR: &str = "the file has no video stream";
pub const UNREADABLE_ERROR: &str = "the file isn't a video we can read";
/// The original was written again after `complete` (the upload link is still valid for a
/// while), so it's no longer the file that was checked. Uploading again is the way out.
pub const CHANGED_ERROR: &str = "the file changed after the upload finished";
/// The same file is here already (decision 55).
pub const DUPLICATE_ERROR: &str = "this exact file is here already";

impl FailureReason {
    pub fn of(error: &str) -> Self {
        if error.starts_with(TOO_LONG_ERROR) {
            Self::TooLong
        } else if error.starts_with(NO_VIDEO_ERROR) {
            Self::NotAVideo
        } else if error.starts_with(UNREADABLE_ERROR) || error.starts_with(CHANGED_ERROR) {
            Self::Unreadable
        } else if error.starts_with(DUPLICATE_ERROR) {
            Self::Duplicate
        } else {
            Self::Server
        }
    }
}

/// A clip with its uploader, as stored.
#[derive(Debug, Clone, sqlx::FromRow)]
pub struct Clip {
    pub id: Uuid,
    pub owner_id: Uuid,
    pub owner_handle: String,
    pub owner_display_name: String,
    pub owner_avatar_url: Option<String>,
    pub game_id: String,
    pub title: String,
    pub description: String,
    pub map: Option<String>,
    pub my_pov: bool,
    pub status: ClipStatus,
    pub original_blob: String,
    pub original_filename: String,
    pub original_bytes: i64,
    pub playback_blob: Option<String>,
    pub poster_blob: Option<String>,
    pub duration_ms: Option<i32>,
    pub width: Option<i32>,
    pub height: Option<i32>,
    pub fps: Option<f32>,
    pub error: Option<String>,
    pub reaction_count: i32,
    pub created_at: DateTime<Utc>,
    pub deleted_at: Option<DateTime<Utc>>,
    /// Saved for the show until then (see `hold`). Past or NULL: not held.
    pub hold_until: Option<DateTime<Utc>>,
    /// The original's ETag when its upload was completed (see `set_original_etag`). NULL
    /// for clips completed before it was kept.
    pub original_etag: Option<String>,
    /// The clip with the same file, when the worker refused this one for it.
    pub duplicate_of: Option<Uuid>,
}

impl Clip {
    /// Saved for the show: only its uploader sees it (others get a teaser in the lobby).
    pub fn is_held(&self) -> bool {
        self.hold_until.is_some_and(|t| t > Utc::now())
    }
}

macro_rules! clip_select {
    () => {
        "SELECT c.id, c.owner_id, u.handle AS owner_handle, u.display_name AS owner_display_name,
                u.avatar_url AS owner_avatar_url, c.game_id, c.title, c.description, c.map,
                c.my_pov, c.status, c.original_blob, c.original_filename, c.original_bytes,
                c.playback_blob, c.poster_blob, c.duration_ms, c.width, c.height, c.fps,
                c.error, c.reaction_count, c.created_at, c.deleted_at, c.hold_until,
                c.original_etag, c.duplicate_of
           FROM clips c JOIN users u ON u.id = c.owner_id "
    };
}

#[derive(Debug, Clone)]
pub struct NewClip {
    pub owner_id: Uuid,
    pub game_id: String,
    pub title: String,
    pub description: String,
    pub map: Option<String>,
    pub my_pov: bool,
    pub filename: String,
    pub bytes: i64,
}

#[derive(Debug, thiserror::Error)]
pub enum NewClipError {
    #[error("{0}")]
    Invalid(String),
    #[error(transparent)]
    Database(#[from] sqlx::Error),
}

/// What the worker learned from transcoding a clip.
#[derive(Debug, Clone)]
pub struct Transcoded {
    pub playback_blob: String,
    pub poster_blob: String,
    pub duration_ms: i32,
    pub width: i32,
    pub height: i32,
    pub fps: f32,
    pub metadata: Value,
    /// Of the playback file, kept with it (`dedup`).
    pub playback_fingerprint: dedup::Fingerprint,
}

/// The playback file of a clip's first transcode.
pub fn playback_blob(id: Uuid) -> String {
    format!("{id}.mp4")
}

pub fn poster_blob(id: Uuid) -> String {
    format!("{id}.jpg")
}

/// A fresh name suffix for re-encoding a clip that's already ready (the `keyframes` job).
/// Its files get new names instead of replacing the ones in use: a player midway through
/// the old playback file would otherwise get the new file's bytes on its next range
/// request. The row is pointed at them once all are uploaded, then the old ones go.
pub fn new_rev() -> String {
    Uuid::new_v4().simple().to_string()[..8].to_owned()
}

/// The playback file of a re-encode (`new_rev`).
pub fn playback_blob_rev(id: Uuid, rev: &str) -> String {
    format!("{id}-{rev}.mp4")
}

pub fn poster_blob_rev(id: Uuid, rev: &str) -> String {
    format!("{id}-{rev}.jpg")
}

/// A small, heavily blurred poster: what others see of a clip saved for the show.
pub fn teaser_blob(id: Uuid) -> String {
    format!("{id}-teaser.jpg")
}

/// How long a clip saved for the show stays hidden at most (decision 29).
pub const HOLD_DAYS: i32 = 7;

/// "Save it for the show": hides the clip from everyone but its uploader until
/// `HOLD_DAYS` after its upload, or until it plays in a show or the uploader releases it.
/// The week counts from the upload, so holding again never extends it. False (and nothing
/// changes) once that week is over or the clip has played in a show.
pub async fn hold(pool: &PgPool, id: Uuid) -> sqlx::Result<bool> {
    let held = sqlx::query(
        "UPDATE clips SET hold_until = created_at + make_interval(days => $2)
          WHERE id = $1 AND created_at + make_interval(days => $2) > now()
            AND NOT EXISTS (SELECT FROM show_clips
                             WHERE clip_id = $1 AND played_at IS NOT NULL)",
    )
    .bind(id)
    .bind(HOLD_DAYS)
    .execute(pool)
    .await?
    .rows_affected();
    Ok(held > 0)
}

/// "Post now", or it played in a show: everyone can see it.
pub async fn release<'e>(db: impl sqlx::PgExecutor<'e>, id: Uuid) -> sqlx::Result<()> {
    sqlx::query("UPDATE clips SET hold_until = NULL WHERE id = $1 AND hold_until IS NOT NULL")
        .bind(id)
        .execute(db)
        .await?;
    Ok(())
}

/// Makes an uploaded file name safe as a blob name and in a download header: ASCII
/// letters, digits, `.`, `-` and `_`, at most 80 characters, extension lower-cased.
/// Returns `None` for an extension we don't accept.
pub fn sanitize_filename(name: &str) -> Option<String> {
    let name = name.rsplit(['/', '\\']).next().unwrap_or(name).trim();
    let (stem, ext) = name.rsplit_once('.')?;
    let ext = ext.to_ascii_lowercase();
    if !EXTENSIONS.contains(&ext.as_str()) {
        return None;
    }
    let mut clean = String::new();
    for c in stem.chars() {
        let c = if c.is_ascii_alphanumeric() || c == '_' || c == '.' {
            c
        } else {
            '-'
        };
        if !(c == '-' && clean.ends_with('-')) {
            clean.push(c);
        }
    }
    let stem: String = clean.trim_matches(['-', '.']).chars().take(72).collect();
    let stem = if stem.is_empty() { "clip".into() } else { stem };
    Some(format!("{stem}.{ext}"))
}

/// Characters that draw nothing on their own: zero-width spaces and joiners, direction
/// marks and overrides, invisible operators, the byte order mark. A title made only of
/// these looks blank. They stay in titles that have something else (a joiner is part of
/// some emoji).
fn is_invisible(c: char) -> bool {
    matches!(c, '\u{200B}'..='\u{200F}' | '\u{202A}'..='\u{202E}' | '\u{2060}'..='\u{2064}' | '\u{FEFF}')
}

/// A title trimmed, or why it can't be one: 1–100 characters on one line, with something
/// visible in it.
fn clean_title(raw: &str) -> Result<&str, NewClipError> {
    let title = raw.trim();
    if title.contains(['\n', '\r']) {
        return Err(NewClipError::Invalid("a title is one line".into()));
    }
    if title.chars().all(is_invisible) || title.chars().count() > 100 {
        return Err(NewClipError::Invalid(
            "title must be 1–100 characters".into(),
        ));
    }
    Ok(title)
}

/// Validates and inserts a clip in `uploading`; returns it with its original's blob name.
pub async fn create(pool: &PgPool, new: NewClip) -> Result<Clip, NewClipError> {
    let invalid = |m: &str| NewClipError::Invalid(m.into());
    let title = clean_title(&new.title)?;
    let description = new.description.trim();
    if description.chars().count() > 2000 {
        return Err(invalid("description must be at most 2000 characters"));
    }
    let map = new
        .map
        .as_deref()
        .map(str::trim)
        .filter(|m| !m.is_empty())
        .map(str::to_owned);
    if map.as_ref().is_some_and(|m| m.chars().count() > 40) {
        return Err(invalid("map must be at most 40 characters"));
    }
    if !(1..=MAX_BYTES).contains(&new.bytes) {
        return Err(invalid("clips must be at most 2 GiB"));
    }
    let filename = sanitize_filename(&new.filename)
        .ok_or_else(|| invalid("upload an .mp4, .mkv or .mov file"))?;

    let id = Uuid::new_v4();
    let result = sqlx::query(
        "INSERT INTO clips (id, owner_id, game_id, title, description, map, my_pov,
                            original_blob, original_filename, original_bytes)
         VALUES ($1, $2, $3, $4, $5, $6, $7, $8, $9, $10)",
    )
    .bind(id)
    .bind(new.owner_id)
    .bind(&new.game_id)
    .bind(title)
    .bind(description)
    .bind(map)
    .bind(new.my_pov)
    .bind(format!("{id}/{filename}"))
    .bind(&filename)
    .bind(new.bytes)
    .execute(pool)
    .await;
    match result {
        // Foreign key on game_id.
        Err(sqlx::Error::Database(e)) if e.is_foreign_key_violation() => {
            return Err(invalid("unknown game"));
        }
        other => other?,
    };
    Ok(get(pool, id).await?.expect("just inserted"))
}

/// A clip that isn't deleted.
pub async fn get(pool: &PgPool, id: Uuid) -> sqlx::Result<Option<Clip>> {
    sqlx::query_as(concat!(
        clip_select!(),
        "WHERE c.id = $1 AND c.deleted_at IS NULL"
    ))
    .bind(id)
    .fetch_optional(pool)
    .await
}

/// Several clips that aren't deleted, in no particular order.
pub async fn get_many(pool: &PgPool, ids: &[Uuid]) -> sqlx::Result<Vec<Clip>> {
    sqlx::query_as(concat!(
        clip_select!(),
        "WHERE c.id = ANY($1) AND c.deleted_at IS NULL"
    ))
    .bind(ids)
    .fetch_all(pool)
    .await
}

/// A clip even if it's in the trash (for its owner's restore button).
pub async fn get_including_deleted(pool: &PgPool, id: Uuid) -> sqlx::Result<Option<Clip>> {
    sqlx::query_as(concat!(clip_select!(), "WHERE c.id = $1"))
        .bind(id)
        .fetch_optional(pool)
        .await
}

/// Feed filters; `None` means "any".
#[derive(Debug, Clone, Default)]
pub struct Filter {
    pub game: Option<String>,
    pub map: Option<String>,
    /// Uploader's handle.
    pub uploader: Option<String>,
    /// Clips with any of these tags (user or auto); empty means any clip.
    pub tags: Vec<String>,
    /// Handle of a friend tagged as playing in the clip.
    pub player: Option<String>,
    /// Words in the title, description, map or uploader's name or handle.
    pub search: Option<String>,
    /// Clips someone reacted to with this emoji.
    pub reaction: Option<String>,
    /// Clips of the night, or fails: clips someone pressed 🍌 on in a show that ended
    /// (decision 30; the winners among them carry the badge).
    pub night: Option<Night>,
}

/// The archive's show filters (S7).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Deserialize, ToSchema)]
#[serde(rename_all = "lowercase")]
pub enum Night {
    /// Won clip of the night.
    Clip,
    /// Got a 🍌 in a show.
    Fail,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Default, Deserialize, ToSchema)]
#[serde(rename_all = "lowercase")]
pub enum Sort {
    #[default]
    New,
    /// Most reactions first.
    Top,
}

/// Position after the last clip of a page. Opaque to clients.
#[derive(Debug, Clone, PartialEq)]
pub struct Cursor {
    pub reaction_count: i32,
    pub created_at: DateTime<Utc>,
    pub id: Uuid,
}

impl Cursor {
    fn after(clip: &Clip) -> Self {
        Self {
            reaction_count: clip.reaction_count,
            created_at: clip.created_at,
            id: clip.id,
        }
    }

    pub fn encode(&self) -> String {
        use base64::Engine;
        base64::engine::general_purpose::URL_SAFE_NO_PAD.encode(format!(
            "{}|{}|{}",
            self.reaction_count,
            self.created_at.timestamp_micros(),
            self.id
        ))
    }

    /// `None` for anything `encode` didn't make, including a time Postgres can't hold
    /// (before 4714 BC), so a tampered cursor is a "bad cursor" and not a failed query.
    pub fn decode(raw: &str) -> Option<Self> {
        use base64::Engine;
        /// 4714-11-24 BC, Postgres' first timestamp, in microseconds since 1970. Its last
        /// (294276 AD) is past the last one chrono can hold.
        const POSTGRES_FIRST_MICROS: i64 = -210_866_803_200_000_000;
        let bytes = base64::engine::general_purpose::URL_SAFE_NO_PAD
            .decode(raw)
            .ok()?;
        let text = String::from_utf8(bytes).ok()?;
        let mut parts = text.splitn(3, '|');
        let reaction_count = parts.next()?.parse().ok()?;
        let micros: i64 = parts.next()?.parse().ok()?;
        if micros < POSTGRES_FIRST_MICROS {
            return None;
        }
        Some(Self {
            reaction_count,
            created_at: DateTime::from_timestamp_micros(micros)?,
            id: parts.next()?.parse().ok()?,
        })
    }
}

/// The clips `c` that the viewer, `$1`, sees in lists: not in the trash; published, or
/// their own that are processing or failed; and not saved for the show by someone else
/// (decision 29). The feed, tag autocomplete and profile counts all use it.
#[macro_export]
macro_rules! listed_for_viewer {
    () => {
        "c.deleted_at IS NULL
           AND (c.status = 'ready' OR (c.owner_id = $1 AND c.status IN ('processing', 'failed')))
           AND (c.hold_until IS NULL OR c.hold_until <= now() OR c.owner_id = $1)"
    };
}

/// Visible clips matching `filter`, plus the cursor for the next page (if any).
macro_rules! feed_where {
    () => {
        concat!(
            "WHERE ",
            listed_for_viewer!(),
            "
           AND ($2::text IS NULL OR c.game_id = $2)
           AND ($3::text IS NULL OR lower(c.map) = lower($3))
           AND ($4::text IS NULL OR u.handle = $4)
           AND ($5::text[] IS NULL OR EXISTS (
                 SELECT FROM clip_tags ct JOIN tags t ON t.id = ct.tag_id
                  WHERE ct.clip_id = c.id AND t.name = ANY($5)))
           AND ($6::text IS NULL OR EXISTS (
                 SELECT FROM clip_players cp JOIN users pu ON pu.id = cp.user_id
                  WHERE cp.clip_id = c.id AND pu.handle = $6))
           AND ($11::text IS NULL OR c.title ILIKE $11 OR c.description ILIKE $11
                OR c.map ILIKE $11 OR u.display_name ILIKE $11 OR u.handle ILIKE $11)
           AND ($12::text IS NULL OR EXISTS (
                 SELECT FROM reactions r WHERE r.clip_id = c.id AND r.emoji = $12))
           AND ($13::text IS NULL
                OR ($13 = 'clip' AND EXISTS (
                     SELECT FROM shows s WHERE s.status = 'ended' AND s.clip_winner_id = c.id))
                OR ($13 = 'fail' AND EXISTS (
                     SELECT FROM show_reactions sr JOIN shows s ON s.id = sr.show_id
                      WHERE sr.clip_id = c.id AND sr.emoji = '🍌' AND s.status = 'ended'))) "
        )
    };
}

/// `%words%` for ILIKE, with the user's own `%`, `_` and `\` matched literally.
fn like_pattern(words: &str) -> String {
    let escaped = words
        .replace('\\', "\\\\")
        .replace('%', "\\%")
        .replace('_', "\\_");
    format!("%{escaped}%")
}

pub async fn list(
    pool: &PgPool,
    viewer: Uuid,
    filter: &Filter,
    sort: Sort,
    cursor: Option<&Cursor>,
    limit: i64,
) -> sqlx::Result<(Vec<Clip>, Option<Cursor>)> {
    let query = match sort {
        Sort::New => sqlx::query_as::<_, Clip>(concat!(
            clip_select!(),
            feed_where!(),
            "AND ($7::timestamptz IS NULL OR (c.created_at, c.id) < ($7, $8))
             ORDER BY c.created_at DESC, c.id DESC
             LIMIT $10"
        )),
        Sort::Top => sqlx::query_as::<_, Clip>(concat!(
            clip_select!(),
            feed_where!(),
            "AND ($9::int IS NULL OR (c.reaction_count, c.created_at, c.id) < ($9, $7, $8))
             ORDER BY c.reaction_count DESC, c.created_at DESC, c.id DESC
             LIMIT $10"
        )),
    };
    let mut clips = query
        .bind(viewer)
        .bind(&filter.game)
        .bind(&filter.map)
        .bind(&filter.uploader)
        .bind((!filter.tags.is_empty()).then_some(&filter.tags))
        .bind(&filter.player)
        .bind(cursor.map(|c| c.created_at))
        .bind(cursor.map(|c| c.id))
        .bind(cursor.map(|c| c.reaction_count))
        // One extra row tells us whether there's a next page.
        .bind(limit + 1)
        .bind(filter.search.as_deref().map(like_pattern))
        .bind(&filter.reaction)
        .bind(filter.night.map(|n| match n {
            Night::Clip => "clip",
            Night::Fail => "fail",
        }))
        .fetch_all(pool)
        .await?;
    let next = if clips.len() as i64 > limit {
        clips.truncate(limit as usize);
        clips.last().map(Cursor::after)
    } else {
        None
    };
    Ok((clips, next))
}

/// The owner's clips in the trash, newest deletion first.
pub async fn trash(pool: &PgPool, owner: Uuid) -> sqlx::Result<Vec<Clip>> {
    sqlx::query_as(concat!(
        clip_select!(),
        "WHERE c.owner_id = $1 AND c.deleted_at IS NOT NULL ORDER BY c.deleted_at DESC"
    ))
    .bind(owner)
    .fetch_all(pool)
    .await
}

/// Editable clip details. `None` leaves a field as is; `map: Some(None)` clears it.
#[derive(Debug, Clone, Default)]
pub struct ClipEdit {
    pub title: Option<String>,
    pub description: Option<String>,
    pub map: Option<Option<String>>,
    pub my_pov: Option<bool>,
}

/// Saves an edit; `db` may be a transaction that also changes the tags and players.
pub async fn update<'e>(
    db: impl sqlx::PgExecutor<'e>,
    id: Uuid,
    edit: ClipEdit,
) -> Result<(), NewClipError> {
    let invalid = |m: &str| NewClipError::Invalid(m.into());
    let title = match &edit.title {
        Some(t) => Some(clean_title(t)?.to_owned()),
        None => None,
    };
    let description = edit.description.map(|d| d.trim().to_owned());
    if description
        .as_ref()
        .is_some_and(|d| d.chars().count() > 2000)
    {
        return Err(invalid("description must be at most 2000 characters"));
    }
    let map = edit
        .map
        .map(|m| m.map(|m| m.trim().to_owned()).filter(|m| !m.is_empty()));
    if map
        .as_ref()
        .is_some_and(|m| m.as_ref().is_some_and(|m| m.chars().count() > 40))
    {
        return Err(invalid("map must be at most 40 characters"));
    }
    sqlx::query(
        "UPDATE clips SET title = coalesce($2, title), description = coalesce($3, description),
                map = CASE WHEN $4 THEN $5 ELSE map END, my_pov = coalesce($6, my_pov),
                updated_at = now()
          WHERE id = $1",
    )
    .bind(id)
    .bind(title)
    .bind(description)
    .bind(map.is_some())
    .bind(map.flatten())
    .bind(edit.my_pov)
    .execute(db)
    .await?;
    Ok(())
}

/// Days a deleted clip stays restorable before the janitor removes it for good.
pub const TRASH_DAYS: i64 = 7;

/// Moves a clip to the trash and revokes its share link, so a restore doesn't bring an
/// old link back to life.
pub async fn soft_delete(pool: &PgPool, id: Uuid) -> sqlx::Result<bool> {
    let mut tx = pool.begin().await?;
    let deleted = sqlx::query(
        "UPDATE clips SET deleted_at = now(), updated_at = now()
          WHERE id = $1 AND deleted_at IS NULL",
    )
    .bind(id)
    .execute(&mut *tx)
    .await?
    .rows_affected()
        == 1;
    sqlx::query(
        "UPDATE share_links SET revoked_at = now() WHERE clip_id = $1 AND revoked_at IS NULL",
    )
    .bind(id)
    .execute(&mut *tx)
    .await?;
    tx.commit().await?;
    Ok(deleted)
}

/// What came of `restore`.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Restore {
    Restored,
    /// Not in the trash, or in it for longer than `TRASH_DAYS`.
    NotRestorable,
    /// Its file is here again as this clip (decision 55), so it stays in the trash.
    Duplicate(Uuid),
}

/// Brings a clip back from the trash, within `TRASH_DAYS`. After that the janitor may be
/// deleting its files (`expired_trash`), so it stays in the trash. So does a clip whose
/// file was uploaded again since, as another clip: one copy is kept.
///
/// The worker drops the transcode of a clip in the trash, so a clip deleted while it was
/// processing gets a new one, unless one is still queued. One already running may be
/// about to drop the clip too; if it transcodes it instead, the new job finds the clip
/// ready and does nothing.
pub async fn restore(pool: &PgPool, id: Uuid) -> sqlx::Result<Restore> {
    let mut tx = pool.begin().await?;
    dedup::lock(&mut tx).await?;
    let status: Option<ClipStatus> = sqlx::query_scalar(
        "UPDATE clips SET deleted_at = NULL, updated_at = now()
          WHERE id = $1 AND deleted_at >= now() - make_interval(days => $2)
      RETURNING status",
    )
    .bind(id)
    .bind(TRASH_DAYS as i32)
    .fetch_optional(&mut *tx)
    .await?;
    let Some(status) = status else {
        return Ok(Restore::NotRestorable);
    };
    if let Some(other) = dedup::held_elsewhere(&mut tx, id).await? {
        return Ok(Restore::Duplicate(other));
    }
    if status == ClipStatus::Processing {
        let queued: bool = sqlx::query_scalar(
            "SELECT EXISTS (SELECT 1 FROM jobs
                             WHERE kind = $1 AND payload->>'clipId' = $2 AND status = 'queued')",
        )
        .bind(TRANSCODE_JOB)
        .bind(id.to_string())
        .fetch_one(&mut *tx)
        .await?;
        if !queued {
            jobs::enqueue_in(&mut tx, TRANSCODE_JOB, json!({ "clipId": id })).await?;
        }
    }
    tx.commit().await?;
    Ok(Restore::Restored)
}

/// Blobs of a clip to delete before purging its row.
#[derive(Debug, Clone, PartialEq, sqlx::FromRow)]
pub struct PurgeTarget {
    pub id: Uuid,
    pub original_blob: String,
    pub playback_blob: Option<String>,
    pub poster_blob: Option<String>,
}

/// Clips in the trash for longer than `TRASH_DAYS`.
pub async fn expired_trash(pool: &PgPool) -> sqlx::Result<Vec<PurgeTarget>> {
    sqlx::query_as(
        "SELECT id, original_blob, playback_blob, poster_blob FROM clips
          WHERE deleted_at < now() - make_interval(days => $1)
          ORDER BY deleted_at LIMIT 100",
    )
    .bind(TRASH_DAYS as i32)
    .fetch_all(pool)
    .await
}

/// Keeps the original's ETag as `complete` found it, while the clip is still uploading.
/// The upload SAS stays valid for a while after that, so the worker checks the file it
/// reads is still this one, not one written over it since.
pub async fn set_original_etag(pool: &PgPool, id: Uuid, etag: &str) -> sqlx::Result<()> {
    sqlx::query("UPDATE clips SET original_etag = $2 WHERE id = $1 AND status = 'uploading'")
        .bind(id)
        .bind(etag)
        .execute(pool)
        .await?;
    Ok(())
}

/// Moves an uploaded clip to `processing` and queues its transcode, atomically. Returns
/// false if it wasn't `uploading` (already completed: the call is idempotent).
pub async fn mark_uploaded(pool: &PgPool, id: Uuid) -> sqlx::Result<bool> {
    let mut tx = pool.begin().await?;
    let moved = sqlx::query(
        "UPDATE clips SET status = 'processing', updated_at = now()
          WHERE id = $1 AND status = 'uploading'",
    )
    .bind(id)
    .execute(&mut *tx)
    .await?
    .rows_affected()
        == 1;
    if moved {
        jobs::enqueue_in(&mut tx, TRANSCODE_JOB, json!({ "clipId": id })).await?;
    }
    tx.commit().await?;
    Ok(moved)
}

/// Points the clip at its transcoded files, with the playback file's fingerprint. Returns
/// false if the row is gone (purged meanwhile), so the caller can delete the files it
/// uploaded.
pub async fn set_ready(pool: &PgPool, id: Uuid, t: &Transcoded) -> sqlx::Result<bool> {
    let mut tx = pool.begin().await?;
    let updated = sqlx::query(
        "UPDATE clips SET status = 'ready', playback_blob = $2, poster_blob = $3,
                duration_ms = $4, width = $5, height = $6, fps = $7, metadata = $8,
                error = NULL, updated_at = now()
          WHERE id = $1",
    )
    .bind(id)
    .bind(&t.playback_blob)
    .bind(&t.poster_blob)
    .bind(t.duration_ms)
    .bind(t.width)
    .bind(t.height)
    .bind(t.fps)
    .bind(&t.metadata)
    .execute(&mut *tx)
    .await?
    .rows_affected();
    if updated == 0 {
        return Ok(false);
    }
    dedup::record(&mut *tx, id, dedup::File::Playback, &t.playback_fingerprint).await?;
    tx.commit().await?;
    Ok(true)
}

/// Marks a processing clip failed. A clip that's ready already (another run of its
/// transcode finished it) stays ready.
pub async fn set_failed(pool: &PgPool, id: Uuid, error: &str) -> sqlx::Result<()> {
    sqlx::query(
        "UPDATE clips SET status = 'failed', error = $2, updated_at = now()
          WHERE id = $1 AND status = 'processing'",
    )
    .bind(id)
    .bind(error)
    .execute(pool)
    .await?;
    Ok(())
}

/// Puts a failed clip back in the queue (the uploader pressed "Retry").
pub async fn retry(pool: &PgPool, id: Uuid) -> sqlx::Result<bool> {
    let mut tx = pool.begin().await?;
    let moved = sqlx::query(
        "UPDATE clips SET status = 'processing', error = NULL, updated_at = now()
          WHERE id = $1 AND status = 'failed'",
    )
    .bind(id)
    .execute(&mut *tx)
    .await?
    .rows_affected()
        == 1;
    if moved {
        jobs::enqueue_in(&mut tx, TRANSCODE_JOB, json!({ "clipId": id })).await?;
    }
    tx.commit().await?;
    Ok(moved)
}

/// Clips stuck in `uploading` longer than `older_than` (abandoned uploads), oldest first.
pub async fn abandoned_uploads(
    pool: &PgPool,
    older_than: Duration,
) -> sqlx::Result<Vec<(Uuid, String)>> {
    sqlx::query_as(
        "SELECT id, original_blob FROM clips
          WHERE status = 'uploading' AND created_at < now() - make_interval(secs => $1)
          ORDER BY created_at LIMIT 100",
    )
    .bind(older_than.as_secs_f64())
    .fetch_all(pool)
    .await
}

/// Removes a clip row for good (after its blobs are gone): one in the trash for longer
/// than `TRASH_DAYS`, or an upload that never completed. Anything else, e.g. a clip
/// restored or completed meanwhile, is left alone.
pub async fn purge(pool: &PgPool, id: Uuid) -> sqlx::Result<()> {
    sqlx::query(
        "DELETE FROM clips
          WHERE id = $1
            AND (deleted_at < now() - make_interval(days => $2) OR status = 'uploading')",
    )
    .bind(id)
    .bind(TRASH_DAYS as i32)
    .execute(pool)
    .await?;
    Ok(())
}

#[cfg(test)]
mod tests {
    #[test]
    fn failure_reasons_from_worker_messages() {
        use super::FailureReason as F;
        assert_eq!(
            F::of("clips can be at most 5 minutes (this one is 6:12)"),
            F::TooLong
        );
        assert_eq!(F::of("the file has no video stream"), F::NotAVideo);
        assert_eq!(
            F::of("the file isn't a video we can read (is the upload complete and not corrupt?)"),
            F::Unreadable
        );
        assert_eq!(
            F::of("the file changed after the upload finished"),
            F::Unreadable
        );
        assert_eq!(F::of("this exact file is here already"), F::Duplicate);
        assert_eq!(
            F::of("processing failed after 3 attempts (timeout)"),
            F::Server
        );
    }

    use super::*;
    use crate::users::{self, Identity, SignIn};

    #[test]
    fn sanitizes_filenames() {
        let cases = [
            (
                "Counter-strike 2 2026.10.01 - 21.04.33.02.DVR.mp4",
                Some("Counter-strike-2-2026.10.01-21.04.33.02.DVR.mp4"),
            ),
            ("C:\\Users\\me\\Videos\\ace!!.MKV", Some("ace.mkv")),
            ("קליפ.mov", Some("clip.mov")),
            ("clip.avi", None),
            ("noext", None),
            ("../../etc/passwd.mp4", Some("passwd.mp4")),
        ];
        for (input, want) in cases {
            assert_eq!(sanitize_filename(input).as_deref(), want, "{input}");
        }
        let long = format!("{}.mp4", "a".repeat(200));
        assert_eq!(sanitize_filename(&long).unwrap().len(), 72 + 4);
    }

    async fn owner(pool: &PgPool) -> Uuid {
        sqlx::query("INSERT INTO invites (email) VALUES ('sam@gmail.com')")
            .execute(pool)
            .await
            .unwrap();
        let identity = Identity {
            sub: "google-oauth2|sam",
            email: "sam@gmail.com",
            name: Some("Sam"),
            picture: None,
        };
        match users::sign_in(pool, &identity).await.unwrap() {
            SignIn::Allowed(u) => u.id,
            other => panic!("{other:?}"),
        }
    }

    fn new_clip(owner_id: Uuid) -> NewClip {
        NewClip {
            owner_id,
            game_id: "cs2".into(),
            title: " Ace on Mirage ".into(),
            description: String::new(),
            map: Some("Mirage".into()),
            my_pov: true,
            filename: "Counter-strike 2 ace.mp4".into(),
            bytes: 1_000_000,
        }
    }

    async fn feed(pool: &PgPool, viewer: Uuid) -> Vec<Clip> {
        list(pool, viewer, &Filter::default(), Sort::New, None, 50)
            .await
            .unwrap()
            .0
    }

    #[sqlx::test(migrator = "crate::db::MIGRATOR")]
    async fn lifecycle(pool: PgPool) {
        let owner_id = owner(&pool).await;
        let clip = create(&pool, new_clip(owner_id)).await.unwrap();
        assert_eq!(clip.title, "Ace on Mirage");
        assert_eq!(clip.status, ClipStatus::Uploading);
        assert_eq!(
            clip.original_blob,
            format!("{}/Counter-strike-2-ace.mp4", clip.id)
        );
        assert_eq!(clip.owner_handle, "sam");

        // Not in the feed until processed.
        assert!(feed(&pool, Uuid::new_v4()).await.is_empty());

        assert!(mark_uploaded(&pool, clip.id).await.unwrap());
        assert!(!mark_uploaded(&pool, clip.id).await.unwrap(), "idempotent");
        let job = jobs::claim(&pool, "w").await.unwrap().unwrap();
        assert_eq!(job.kind, TRANSCODE_JOB);
        assert_eq!(job.payload, json!({ "clipId": clip.id }));
        assert!(
            jobs::claim(&pool, "w").await.unwrap().is_none(),
            "one job only"
        );

        // The uploader sees it while processing; others don't.
        assert_eq!(feed(&pool, owner_id).await.len(), 1);
        assert!(feed(&pool, Uuid::new_v4()).await.is_empty());

        set_failed(&pool, clip.id, "no video stream").await.unwrap();
        assert!(retry(&pool, clip.id).await.unwrap());
        assert!(!retry(&pool, clip.id).await.unwrap());

        set_ready(
            &pool,
            clip.id,
            &Transcoded {
                playback_blob: playback_blob(clip.id),
                poster_blob: poster_blob(clip.id),
                duration_ms: 12_345,
                width: 1440,
                height: 1080,
                fps: 60.0,
                metadata: json!({}),
                playback_fingerprint: crate::testing::fingerprint(clip.id),
            },
        )
        .await
        .unwrap();
        let ready = get(&pool, clip.id).await.unwrap().unwrap();
        assert_eq!(ready.status, ClipStatus::Ready);
        assert_eq!(ready.width, Some(1440));
        assert_eq!(feed(&pool, Uuid::new_v4()).await.len(), 1);

        // A late failure from another run of the transcode doesn't undo it.
        set_failed(&pool, clip.id, "processing kept crashing")
            .await
            .unwrap();
        let still = get(&pool, clip.id).await.unwrap().unwrap();
        assert_eq!((still.status, still.error), (ClipStatus::Ready, None));
    }

    async fn queued_transcodes(pool: &PgPool) -> i64 {
        sqlx::query_scalar("SELECT count(*) FROM jobs WHERE kind = $1 AND status = 'queued'")
            .bind(TRANSCODE_JOB)
            .fetch_one(pool)
            .await
            .unwrap()
    }

    /// Restoring a clip deleted while it was processing queues its transcode again if the
    /// worker dropped it meanwhile, and doesn't queue a second one if it's still waiting.
    #[sqlx::test(migrator = "crate::db::MIGRATOR")]
    async fn restoring_a_processing_clip_queues_its_transcode(pool: PgPool) {
        let owner_id = owner(&pool).await;
        let clip = create(&pool, new_clip(owner_id)).await.unwrap();
        assert!(mark_uploaded(&pool, clip.id).await.unwrap());

        // Still queued: restored as is.
        assert!(soft_delete(&pool, clip.id).await.unwrap());
        assert_eq!(restore(&pool, clip.id).await.unwrap(), Restore::Restored);
        assert_eq!(queued_transcodes(&pool).await, 1);

        // The worker found it in the trash and dropped the job.
        assert!(soft_delete(&pool, clip.id).await.unwrap());
        let job = jobs::claim(&pool, "w").await.unwrap().unwrap();
        assert!(jobs::complete(&pool, &job, "w").await.unwrap());
        assert_eq!(queued_transcodes(&pool).await, 0);
        assert_eq!(restore(&pool, clip.id).await.unwrap(), Restore::Restored);
        assert_eq!(
            restore(&pool, clip.id).await.unwrap(),
            Restore::NotRestorable,
            "not in the trash"
        );
        let job = jobs::claim(&pool, "w").await.unwrap().unwrap();
        assert_eq!(
            (job.kind.as_str(), &job.payload),
            (TRANSCODE_JOB, &json!({ "clipId": clip.id }))
        );

        // A ready clip needs nothing.
        set_ready(
            &pool,
            clip.id,
            &Transcoded {
                playback_blob: playback_blob(clip.id),
                poster_blob: poster_blob(clip.id),
                duration_ms: 1000,
                width: 1920,
                height: 1080,
                fps: 60.0,
                metadata: json!({}),
                playback_fingerprint: crate::testing::fingerprint(clip.id),
            },
        )
        .await
        .unwrap();
        assert!(soft_delete(&pool, clip.id).await.unwrap());
        assert_eq!(restore(&pool, clip.id).await.unwrap(), Restore::Restored);
        assert_eq!(queued_transcodes(&pool).await, 0);
    }

    #[sqlx::test(migrator = "crate::db::MIGRATOR")]
    async fn rejects_bad_uploads(pool: PgPool) {
        let owner_id = owner(&pool).await;
        let bad = [
            NewClip {
                title: "  ".into(),
                ..new_clip(owner_id)
            },
            NewClip {
                bytes: MAX_BYTES + 1,
                ..new_clip(owner_id)
            },
            NewClip {
                bytes: 0,
                ..new_clip(owner_id)
            },
            NewClip {
                filename: "clip.avi".into(),
                ..new_clip(owner_id)
            },
            NewClip {
                game_id: "valorant".into(),
                ..new_clip(owner_id)
            },
            NewClip {
                map: Some("m".repeat(41)),
                ..new_clip(owner_id)
            },
        ];
        for new in bad {
            let result = create(&pool, new.clone()).await;
            assert!(matches!(result, Err(NewClipError::Invalid(_))), "{new:?}");
        }
    }

    #[sqlx::test(migrator = "crate::db::MIGRATOR")]
    async fn finds_abandoned_uploads(pool: PgPool) {
        let owner_id = owner(&pool).await;
        let clip = create(&pool, new_clip(owner_id)).await.unwrap();
        assert!(
            abandoned_uploads(&pool, Duration::from_secs(3600))
                .await
                .unwrap()
                .is_empty()
        );
        sqlx::query("UPDATE clips SET created_at = now() - interval '2 days'")
            .execute(&pool)
            .await
            .unwrap();
        let stale = abandoned_uploads(&pool, Duration::from_secs(3600))
            .await
            .unwrap();
        assert_eq!(stale, vec![(clip.id, clip.original_blob.clone())]);
        purge(&pool, clip.id).await.unwrap();
        assert!(get(&pool, clip.id).await.unwrap().is_none());
    }

    #[sqlx::test(migrator = "crate::db::MIGRATOR")]
    async fn edits_are_checked(pool: PgPool) {
        let owner_id = owner(&pool).await;
        let clip = create(&pool, new_clip(owner_id)).await.unwrap();
        let refused = |edit: ClipEdit| {
            let pool = pool.clone();
            async move {
                let e = update(&pool, clip.id, edit).await.unwrap_err();
                assert!(matches!(e, NewClipError::Invalid(_)), "{e:?}");
                e.to_string()
            }
        };
        let long = ClipEdit {
            description: Some("d".repeat(2001)),
            ..ClipEdit::default()
        };
        assert_eq!(
            refused(long).await,
            "description must be at most 2000 characters"
        );
        let long = ClipEdit {
            map: Some(Some("m".repeat(41))),
            ..ClipEdit::default()
        };
        assert_eq!(refused(long).await, "map must be at most 40 characters");

        // At the limits (counted in characters, after trimming) they're saved.
        let edit = ClipEdit {
            description: Some(format!(" {} ", "é".repeat(2000))),
            map: Some(Some(format!(" {} ", "m".repeat(40)))),
            ..ClipEdit::default()
        };
        update(&pool, clip.id, edit).await.unwrap();
        let saved = get_including_deleted(&pool, clip.id)
            .await
            .unwrap()
            .unwrap();
        assert_eq!(saved.description.chars().count(), 2000);
        assert_eq!(saved.map.as_deref().map(str::len), Some(40));
    }
}
