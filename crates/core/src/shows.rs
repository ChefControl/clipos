//! Shows (docs/PLAN.md, redesign, S4): tonight's lineup, the lobby → live → finale → ended
//! lifecycle, who joined, show reactions and the finale vote. One show at a time for the
//! whole group (a unique index on open shows enforces it). The live part, everyone's
//! players kept in sync, is the show hub in S5; this module is the stored state it drives.

use chrono::{DateTime, Utc};
use serde::{Deserialize, Serialize};
use sqlx::{PgConnection, PgPool};
use utoipa::ToSchema;
use uuid::Uuid;

use crate::social;

/// The fail button in the show's reaction dock. Not a clip-page reaction.
pub const BANANA: &str = "🍌";

/// How far back the first show reaches: tonight starts this many days before the first
/// show opened (or before now, while there's never been one).
const FIRST_SHOW_DAYS: i32 = 7;

/// How far past a clip's end a show reaction's moment may be.
const REACTION_SLACK_MS: i32 = 1000;

/// The most clips one show plays (decision 56). Tonight's clips past it wait for the next
/// show as spares.
pub const MAX_CLIPS: usize = 10;

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, sqlx::Type, ToSchema)]
#[sqlx(type_name = "text", rename_all = "lowercase")]
#[serde(rename_all = "lowercase")]
pub enum ShowStatus {
    /// Created, people joining; the host can reorder and drop clips.
    Lobby,
    /// Playing the lineup.
    Live,
    /// Voting for clip and fail of the night.
    Finale,
    /// Done; winners stored.
    Ended,
    /// Everyone left; no finale, nothing counts as played.
    Abandoned,
}

impl ShowStatus {
    pub fn is_open(self) -> bool {
        matches!(self, Self::Lobby | Self::Live | Self::Finale)
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize, sqlx::Type, ToSchema)]
#[sqlx(type_name = "text", rename_all = "lowercase")]
#[serde(rename_all = "lowercase")]
pub enum Category {
    /// Clip of the night.
    Clip,
    /// Fail of the night.
    Fail,
}

#[derive(Debug, Clone, sqlx::FromRow)]
pub struct Show {
    pub id: Uuid,
    pub host_id: Uuid,
    pub status: ShowStatus,
    pub created_at: DateTime<Utc>,
    pub started_at: Option<DateTime<Utc>>,
    pub ended_at: Option<DateTime<Utc>>,
    pub clip_winner_id: Option<Uuid>,
    pub fail_winner_id: Option<Uuid>,
    /// When it went to the finale: the vote's windows run from it (`vote_windows`).
    pub finale_at: Option<DateTime<Utc>>,
}

/// How long each category of the finale vote runs (decision 31).
pub const VOTE_SECS: i64 = 20;

/// The finale vote's windows: fail of the night first, then clip of the night, 20 s each
/// (decision 31). With no 🍌 tonight there's no fail vote, and clip of the night starts
/// straight away. The screens count down to them; the host's ends the show when the last
/// one closes, and votes count until then.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct VoteWindows {
    pub fail: Option<(DateTime<Utc>, DateTime<Utc>)>,
    pub clip: (DateTime<Utc>, DateTime<Utc>),
}

pub fn vote_windows(finale_at: DateTime<Utc>, any_fails: bool) -> VoteWindows {
    let each = chrono::Duration::seconds(VOTE_SECS);
    let clip_from = if any_fails {
        finale_at + each
    } else {
        finale_at
    };
    VoteWindows {
        fail: any_fails.then_some((finale_at, finale_at + each)),
        clip: (clip_from, clip_from + each),
    }
}

#[derive(Debug, Clone, PartialEq, Eq, sqlx::FromRow)]
pub struct LineupClip {
    pub clip_id: Uuid,
    pub position: i32,
    pub added_by: Option<Uuid>,
    pub dropped: bool,
    /// Dropped because the lineup was full (`MAX_CLIPS`), not by the host: it waits for
    /// the next show, and doesn't count as dropped there (decision 56).
    pub spare: bool,
    pub played_at: Option<DateTime<Utc>>,
}

#[derive(Debug, Clone, PartialEq, Eq, sqlx::FromRow)]
pub struct Participant {
    pub user_id: Uuid,
    pub joined_at: DateTime<Utc>,
    pub ready: bool,
}

#[derive(Debug, Clone, PartialEq, Eq, sqlx::FromRow)]
pub struct ShowReaction {
    pub clip_id: Uuid,
    pub user_id: Uuid,
    pub emoji: String,
    pub at_ms: i32,
}

#[derive(Debug, Clone, PartialEq, Eq, sqlx::FromRow)]
pub struct VoteCount {
    pub category: Category,
    pub clip_id: Uuid,
    pub votes: i64,
}

/// The host's pick when a category's vote ties (decision 31).
#[derive(Debug, Clone, Copy, Default, Deserialize, ToSchema)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct TieBreak {
    pub clip: Option<Uuid>,
    pub fail: Option<Uuid>,
}

/// The tied clips of each category when `end` needs the host's pick; empty for a
/// category that isn't tied.
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, ToSchema)]
pub struct Ties {
    pub clip: Vec<Uuid>,
    pub fail: Vec<Uuid>,
}

#[derive(Debug, thiserror::Error)]
pub enum ShowError {
    /// No such show, or no such clip as far as the user can tell.
    #[error("not found")]
    NotFound,
    #[error("{0}")]
    Forbidden(&'static str),
    #[error("{0}")]
    Conflict(String),
    #[error("{0}")]
    Invalid(String),
    /// A vote tied; the host has to pick among the tied clips (see `end`).
    #[error("the vote is tied")]
    Tie(Ties),
    #[error(transparent)]
    Db(#[from] sqlx::Error),
}

type Result<T> = std::result::Result<T, ShowError>;

macro_rules! show_columns {
    () => {
        "id, host_id, status, created_at, started_at, ended_at, clip_winner_id, fail_winner_id, \
         finale_at"
    };
}

/// The show in progress (lobby, live or finale), if any.
pub async fn open(pool: &PgPool) -> sqlx::Result<Option<Show>> {
    sqlx::query_as(concat!(
        "SELECT ",
        show_columns!(),
        " FROM shows WHERE status IN ('lobby', 'live', 'finale')"
    ))
    .fetch_optional(pool)
    .await
}

pub async fn get(pool: &PgPool, id: Uuid) -> sqlx::Result<Option<Show>> {
    sqlx::query_as(concat!(
        "SELECT ",
        show_columns!(),
        " FROM shows WHERE id = $1"
    ))
    .bind(id)
    .fetch_optional(pool)
    .await
}

/// Ended shows, newest first.
pub async fn past(pool: &PgPool, limit: i64) -> sqlx::Result<Vec<Show>> {
    sqlx::query_as(concat!(
        "SELECT ",
        show_columns!(),
        " FROM shows WHERE status = 'ended'
          ORDER BY ended_at DESC, id DESC LIMIT $1"
    ))
    .bind(limit)
    .fetch_all(pool)
    .await
}

/// Tonight's clips, in upload order: published clips no ended show has played, uploaded
/// since a week before the first show (decisions 28, 36). Every clip stays until a show
/// plays it, so one that was dropped, not reached, still processing when the show opened,
/// or uploaded while it was live (those go to the next show) all come back. A clip that
/// was ever in a lineup comes back too, however old (someone added it from the archive).
/// Except a clip dropped in two ended shows: it stops coming back (decision 28). It stays
/// in the archive, and anyone in a show can still add it (`add_clip`). An abandoned show's
/// drops don't count, and nor do spares, which only didn't fit (decision 56).
pub async fn tonight(pool: &PgPool) -> sqlx::Result<Vec<Uuid>> {
    sqlx::query_scalar(
        "SELECT c.id FROM clips c
          WHERE c.status = 'ready' AND c.deleted_at IS NULL
            AND NOT EXISTS (
                SELECT FROM show_clips sc JOIN shows s ON s.id = sc.show_id
                 WHERE sc.clip_id = c.id AND s.status = 'ended' AND sc.played_at IS NOT NULL)
            AND (c.created_at > COALESCE((SELECT min(created_at) FROM shows), now())
                                - make_interval(days => $1)
                 OR EXISTS (SELECT FROM show_clips sc WHERE sc.clip_id = c.id))
            AND (SELECT count(*) FROM show_clips sc JOIN shows s ON s.id = sc.show_id
                  WHERE sc.clip_id = c.id AND sc.dropped AND NOT sc.spare
                    AND s.status = 'ended') < 2
          ORDER BY c.created_at, c.id",
    )
    .bind(FIRST_SHOW_DAYS)
    .fetch_all(pool)
    .await
}

/// Opens a show with tonight's clips as its lineup, the first `MAX_CLIPS` of them (the rest
/// are spares); the host joins it. Anyone can host (decision 28), but only one show can be
/// open. Clips that turn up while it's in the lobby join at `start`.
pub async fn create(pool: &PgPool, host: Uuid) -> Result<Show> {
    let clips = tonight(pool).await?;
    let mut tx = pool.begin().await?;
    let show: Show = sqlx::query_as(concat!(
        "INSERT INTO shows (host_id) VALUES ($1) RETURNING ",
        show_columns!()
    ))
    .bind(host)
    .fetch_one(&mut *tx)
    .await
    .map_err(|e| match &e {
        sqlx::Error::Database(d) if d.constraint() == Some("shows_one_open_idx") => {
            ShowError::Conflict("a show is already on".into())
        }
        _ => e.into(),
    })?;
    append(&mut tx, show.id, host, &clips).await?;
    sqlx::query("INSERT INTO show_participants (show_id, user_id) VALUES ($1, $2)")
        .bind(show.id)
        .bind(host)
        .execute(&mut *tx)
        .await?;
    tx.commit().await?;
    Ok(show)
}

/// Puts `clips` at the end of the lineup, in order, added by `by`: as many as fit under
/// `MAX_CLIPS`, and the rest as spares.
async fn append(
    conn: &mut sqlx::PgConnection,
    show: Uuid,
    by: Uuid,
    clips: &[Uuid],
) -> sqlx::Result<()> {
    sqlx::query(
        "INSERT INTO show_clips (show_id, clip_id, position, added_by, dropped, spare)
         SELECT $1, c, at.next + (n - 1)::int, $2, n > room.free, n > room.free
           FROM unnest($3::uuid[]) WITH ORDINALITY AS t(c, n),
                (SELECT COALESCE(max(position) + 1, 0) AS next FROM show_clips
                  WHERE show_id = $1) AS at,
                (SELECT $4 - count(*) FILTER (WHERE NOT dropped) AS free FROM show_clips
                  WHERE show_id = $1) AS room",
    )
    .bind(show)
    .bind(by)
    .bind(clips)
    .bind(MAX_CLIPS as i64)
    .execute(conn)
    .await?;
    Ok(())
}

pub async fn lineup(pool: &PgPool, show: Uuid) -> sqlx::Result<Vec<LineupClip>> {
    sqlx::query_as(
        "SELECT clip_id, position, added_by, dropped, spare, played_at FROM show_clips
          WHERE show_id = $1 ORDER BY position, clip_id",
    )
    .bind(show)
    .fetch_all(pool)
    .await
}

pub async fn participants(pool: &PgPool, show: Uuid) -> sqlx::Result<Vec<Participant>> {
    sqlx::query_as(
        "SELECT user_id, joined_at, ready FROM show_participants
          WHERE show_id = $1 ORDER BY joined_at, user_id",
    )
    .bind(show)
    .fetch_all(pool)
    .await
}

/// Every tap, in clip order and time, for the replay.
pub async fn reactions(pool: &PgPool, show: Uuid) -> sqlx::Result<Vec<ShowReaction>> {
    sqlx::query_as(
        "SELECT r.clip_id, r.user_id, r.emoji, r.at_ms FROM show_reactions r
           JOIN show_clips sc ON sc.show_id = r.show_id AND sc.clip_id = r.clip_id
          WHERE r.show_id = $1 ORDER BY sc.position, r.at_ms, r.id",
    )
    .bind(show)
    .fetch_all(pool)
    .await
}

/// Clips someone marked with 🍌 in this show: the fail of the night contenders. Only
/// clips it played, and not ones moved to the trash since.
pub async fn fail_contenders(pool: &PgPool, show: Uuid) -> sqlx::Result<Vec<Uuid>> {
    sqlx::query_scalar(
        "SELECT DISTINCT r.clip_id FROM show_reactions r
           JOIN show_clips sc ON sc.show_id = r.show_id AND sc.clip_id = r.clip_id
           JOIN clips c ON c.id = r.clip_id
          WHERE r.show_id = $1 AND r.emoji = $2 AND NOT sc.dropped
            AND sc.played_at IS NOT NULL AND c.deleted_at IS NULL",
    )
    .bind(show)
    .bind(BANANA)
    .fetch_all(pool)
    .await
}

/// Votes per clip and category. A clip moved to the trash since loses its votes.
pub async fn tally<'e>(db: impl sqlx::PgExecutor<'e>, show: Uuid) -> sqlx::Result<Vec<VoteCount>> {
    sqlx::query_as(
        "SELECT v.category, v.clip_id, count(*) AS votes FROM show_votes v
           JOIN clips c ON c.id = v.clip_id
          WHERE v.show_id = $1 AND c.deleted_at IS NULL
          GROUP BY v.category, v.clip_id ORDER BY v.category, votes DESC, v.clip_id",
    )
    .bind(show)
    .fetch_all(db)
    .await
}

/// A show that's still open, for something only open shows allow.
async fn open_show(pool: &PgPool, id: Uuid) -> Result<Show> {
    let show = get(pool, id).await?.ok_or(ShowError::NotFound)?;
    if !show.status.is_open() {
        return Err(ShowError::Conflict("the show is over".into()));
    }
    Ok(show)
}

/// `open_show`, with the show row locked until `tx` ends. Lineup changes take this lock,
/// so they go one at a time: two clips added at once get a place each.
async fn lock_open_show(tx: &mut PgConnection, id: Uuid) -> Result<Show> {
    let show: Show = sqlx::query_as(concat!(
        "SELECT ",
        show_columns!(),
        " FROM shows WHERE id = $1 FOR UPDATE"
    ))
    .bind(id)
    .fetch_optional(&mut *tx)
    .await?
    .ok_or(ShowError::NotFound)?;
    if !show.status.is_open() {
        return Err(ShowError::Conflict("the show is over".into()));
    }
    Ok(show)
}

fn host_only(show: &Show, by: Uuid) -> Result<()> {
    if show.host_id != by {
        return Err(ShowError::Forbidden("only the host can do that"));
    }
    Ok(())
}

fn in_status(show: &Show, allowed: &[ShowStatus], what: &str) -> Result<()> {
    if !allowed.contains(&show.status) {
        return Err(ShowError::Conflict(format!(
            "can't {what} while the show is {:?}",
            show.status
        )));
    }
    Ok(())
}

async fn is_participant<'e>(
    db: impl sqlx::PgExecutor<'e>,
    show: Uuid,
    user: Uuid,
) -> sqlx::Result<bool> {
    sqlx::query_scalar(
        "SELECT EXISTS (SELECT FROM show_participants WHERE show_id = $1 AND user_id = $2)",
    )
    .bind(show)
    .bind(user)
    .fetch_one(db)
    .await
}

/// Whether `user` is in a live show whose lineup has `clip` (not dropped): they may
/// fetch it to play even while it's held, since the show is about to play it. Not in the
/// lobby, where anyone who joins could watch every held clip before the show starts.
pub async fn in_live_lineup(pool: &PgPool, clip: Uuid, user: Uuid) -> sqlx::Result<bool> {
    sqlx::query_scalar(
        "SELECT EXISTS (
            SELECT FROM show_clips sc
              JOIN shows s ON s.id = sc.show_id
              JOIN show_participants p ON p.show_id = s.id AND p.user_id = $2
             WHERE sc.clip_id = $1 AND NOT sc.dropped AND s.status = 'live')",
    )
    .bind(clip)
    .bind(user)
    .fetch_one(pool)
    .await
}

/// The host reorders the clips still to play and drops the ones left out (they stay for
/// the next show), up to `MAX_CLIPS` in all with the played ones. The whole lineup is
/// numbered again: played clips first, in the order they played (anyone may have played
/// them out of order), then `order`, then the dropped ones. A spare left out stays a
/// spare. The lineup is read with its rows locked: a clip starting to play right
/// then (`mark_played`) is waited for and counts as played, so it's never dropped.
pub async fn set_lineup(pool: &PgPool, id: Uuid, by: Uuid, order: &[Uuid]) -> Result<()> {
    let mut tx = pool.begin().await?;
    let show = lock_open_show(&mut tx, id).await?;
    host_only(&show, by)?;
    in_status(
        &show,
        &[ShowStatus::Lobby, ShowStatus::Live],
        "change the lineup",
    )?;
    let rows: Vec<LineupClip> = sqlx::query_as(
        "SELECT clip_id, position, added_by, dropped, spare, played_at FROM show_clips
          WHERE show_id = $1 ORDER BY position, clip_id FOR UPDATE",
    )
    .bind(id)
    .fetch_all(&mut *tx)
    .await?;
    let waiting = |c: &Uuid| {
        rows.iter()
            .any(|r| r.clip_id == *c && r.played_at.is_none())
    };
    if let Some(stray) = order.iter().find(|c| !waiting(c)) {
        return Err(ShowError::Invalid(format!(
            "clip {stray} isn't waiting in this show's lineup"
        )));
    }
    if let Some(twice) = order
        .iter()
        .enumerate()
        .find_map(|(i, c)| order[..i].contains(c).then_some(c))
    {
        return Err(ShowError::Invalid(format!(
            "clip {twice} is in the order twice"
        )));
    }
    let mut played: Vec<&LineupClip> = rows.iter().filter(|r| r.played_at.is_some()).collect();
    played.sort_by_key(|r| (r.played_at, r.position));
    if played.len() + order.len() > MAX_CLIPS {
        return Err(ShowError::Invalid(format!(
            "a show has at most {MAX_CLIPS} clips"
        )));
    }
    let dropped = rows
        .iter()
        .filter(|r| r.played_at.is_none() && !order.contains(&r.clip_id));
    let mut clips = Vec::with_capacity(rows.len());
    let mut drops = Vec::with_capacity(rows.len());
    let mut spares = Vec::with_capacity(rows.len());
    for (clip, drop, spare) in played
        .iter()
        .map(|r| (r.clip_id, false, false))
        .chain(order.iter().map(|c| (*c, false, false)))
        .chain(dropped.map(|r| (r.clip_id, true, r.dropped && r.spare)))
    {
        clips.push(clip);
        drops.push(drop);
        spares.push(spare);
    }
    let positions: Vec<i32> = (0..clips.len() as i32).collect();
    sqlx::query(
        "UPDATE show_clips sc
            SET position = t.position, dropped = t.dropped, spare = t.spare
           FROM unnest($2::uuid[], $3::int[], $4::bool[], $5::bool[])
                AS t(clip_id, position, dropped, spare)
          WHERE sc.show_id = $1 AND sc.clip_id = t.clip_id",
    )
    .bind(id)
    .bind(&clips)
    .bind(&positions)
    .bind(&drops)
    .bind(&spares)
    .execute(&mut *tx)
    .await?;
    tx.commit().await?;
    Ok(())
}

/// Anyone in the show adds a clip to the end of the queue (decision 31), while it has
/// fewer than `MAX_CLIPS`. A dropped clip or a spare comes back the same way. A clip saved
/// for the show is only its uploader's to add: to anyone else it doesn't exist (decision
/// 29), like someone else's unpublished clip.
pub async fn add_clip(pool: &PgPool, id: Uuid, by: Uuid, clip: Uuid) -> Result<()> {
    let mut tx = pool.begin().await?;
    let show = lock_open_show(&mut tx, id).await?;
    in_status(&show, &[ShowStatus::Lobby, ShowStatus::Live], "add clips")?;
    if !is_participant(&mut *tx, id, by).await? {
        return Err(ShowError::Forbidden("join the show first"));
    }
    let found: Option<(Uuid, bool, bool)> = sqlx::query_as(
        "SELECT owner_id, status = 'ready', coalesce(hold_until > now(), false) FROM clips
          WHERE id = $1 AND deleted_at IS NULL",
    )
    .bind(clip)
    .fetch_optional(&mut *tx)
    .await?;
    match found {
        Some((owner, ready, held)) if owner == by || (ready && !held) => {
            if !ready {
                return Err(ShowError::Invalid("that clip isn't ready to play".into()));
            }
        }
        _ => return Err(ShowError::NotFound),
    }
    let (count, there): (i64, bool) = sqlx::query_as(
        "SELECT count(*) FILTER (WHERE NOT dropped),
                coalesce(bool_or(clip_id = $2 AND NOT dropped), false)
           FROM show_clips WHERE show_id = $1",
    )
    .bind(id)
    .bind(clip)
    .fetch_one(&mut *tx)
    .await?;
    if !there && count >= MAX_CLIPS as i64 {
        return Err(ShowError::Conflict(format!(
            "a show has at most {MAX_CLIPS} clips"
        )));
    }
    let changed = sqlx::query(
        "INSERT INTO show_clips (show_id, clip_id, position, added_by)
         VALUES ($1, $2, (SELECT COALESCE(max(position) + 1, 0) FROM show_clips
                           WHERE show_id = $1), $3)
         ON CONFLICT (show_id, clip_id) DO UPDATE
            SET dropped = false, spare = false, position = EXCLUDED.position,
                added_by = EXCLUDED.added_by
          WHERE show_clips.dropped",
    )
    .bind(id)
    .bind(clip)
    .bind(by)
    .execute(&mut *tx)
    .await?
    .rows_affected();
    if changed == 0 {
        return Err(ShowError::Conflict(
            "that clip is already in the lineup".into(),
        ));
    }
    tx.commit().await?;
    Ok(())
}

/// Joining is open until the show ends; late joiners vote on everything (decision 33).
pub async fn join(pool: &PgPool, id: Uuid, user: Uuid) -> Result<()> {
    open_show(pool, id).await?;
    sqlx::query(
        "INSERT INTO show_participants (show_id, user_id) VALUES ($1, $2)
         ON CONFLICT DO NOTHING",
    )
    .bind(id)
    .bind(user)
    .execute(pool)
    .await?;
    Ok(())
}

pub async fn set_ready(pool: &PgPool, id: Uuid, user: Uuid, ready: bool) -> Result<()> {
    open_show(pool, id).await?;
    let changed =
        sqlx::query("UPDATE show_participants SET ready = $3 WHERE show_id = $1 AND user_id = $2")
            .bind(id)
            .bind(user)
            .bind(ready)
            .execute(pool)
            .await?
            .rows_affected();
    if changed == 0 {
        return Err(ShowError::Forbidden("join the show first"));
    }
    Ok(())
}

/// Moves an open show from one status to the next, if it's still in `from`.
async fn advance(pool: &PgPool, id: Uuid, from: ShowStatus, to: ShowStatus) -> Result<()> {
    // Going to the finale starts the vote's clock (`vote_windows`).
    let changed = sqlx::query(
        "UPDATE shows SET status = $3,
                finale_at = CASE WHEN $3 = 'finale' THEN now() ELSE finale_at END
          WHERE id = $1 AND status = $2",
    )
    .bind(id)
    .bind(from)
    .bind(to)
    .execute(pool)
    .await?
    .rows_affected();
    if changed == 0 {
        return Err(ShowError::Conflict("the show moved on meanwhile".into()));
    }
    Ok(())
}

/// The host presses Start, ready or not (decision 31). Clips uploaded while the show was
/// in the lobby (or still processing when it opened) join the end of the lineup while it
/// has room (the rest are spares); from now on new ones wait for the next show (decision
/// 28).
pub async fn start(pool: &PgPool, id: Uuid, by: Uuid) -> Result<()> {
    let show = open_show(pool, id).await?;
    host_only(&show, by)?;
    in_status(&show, &[ShowStatus::Lobby], "start")?;
    let known: Vec<Uuid> = lineup(pool, id).await?.iter().map(|l| l.clip_id).collect();
    let late: Vec<Uuid> = tonight(pool)
        .await?
        .into_iter()
        .filter(|c| !known.contains(c))
        .collect();
    let mut tx = pool.begin().await?;
    let changed = sqlx::query(
        "UPDATE shows SET status = 'live', started_at = now() WHERE id = $1 AND status = 'lobby'",
    )
    .bind(id)
    .execute(&mut *tx)
    .await?
    .rows_affected();
    if changed == 0 {
        return Err(ShowError::Conflict("the show moved on meanwhile".into()));
    }
    append(&mut tx, id, by, &late).await?;
    tx.commit().await?;
    Ok(())
}

/// A clip started playing for everyone. Anyone in the show steers it (decision 56).
pub async fn mark_played(pool: &PgPool, id: Uuid, by: Uuid, clip: Uuid) -> Result<()> {
    let show = open_show(pool, id).await?;
    in_status(&show, &[ShowStatus::Live], "play clips")?;
    if !is_participant(pool, id, by).await? {
        return Err(ShowError::Forbidden("join the show first"));
    }
    let mut tx = pool.begin().await?;
    // `released_hold`: this show took the clip's hold away, which `abandon` gives back.
    let changed = sqlx::query(
        "UPDATE show_clips sc SET played_at = COALESCE(sc.played_at, now()),
                released_hold = sc.released_hold OR coalesce(c.hold_until > now(), false)
           FROM clips c
          WHERE sc.show_id = $1 AND sc.clip_id = $2 AND NOT sc.dropped AND c.id = sc.clip_id",
    )
    .bind(id)
    .bind(clip)
    .execute(&mut *tx)
    .await?
    .rows_affected();
    if changed == 0 {
        return Err(ShowError::Invalid("that clip isn't in the lineup".into()));
    }
    // Everyone has seen it now: a clip saved for the show is released (decision 29).
    crate::clips::release(&mut *tx, clip).await?;
    tx.commit().await?;
    Ok(())
}

/// To the finale: after the last clip, or early when the host ends the show (decision 33).
/// Clips not played yet go back to tonight.
pub async fn finale(pool: &PgPool, id: Uuid, by: Uuid) -> Result<()> {
    let show = open_show(pool, id).await?;
    host_only(&show, by)?;
    in_status(&show, &[ShowStatus::Live], "go to the finale")?;
    advance(pool, id, ShowStatus::Live, ShowStatus::Finale).await
}

/// A tap in the reaction dock while a clip plays: kept for the replay, and (except 🍌)
/// the tapper's reaction on the clip page turns on.
pub async fn react(
    pool: &PgPool,
    id: Uuid,
    user: Uuid,
    clip: Uuid,
    emoji: &str,
    at_ms: i32,
) -> Result<()> {
    // What the tap says is checked first: it needs no database.
    if emoji != BANANA && !social::EMOJIS.contains(&emoji) {
        return Err(ShowError::Invalid("unsupported reaction".into()));
    }
    if at_ms < 0 {
        return Err(ShowError::Invalid("the moment can't be negative".into()));
    }
    let show = open_show(pool, id).await?;
    in_status(&show, &[ShowStatus::Live], "react")?;
    if !is_participant(pool, id, user).await? {
        return Err(ShowError::Forbidden("join the show first"));
    }
    // Only to a clip that has played: a 🍌 on one nobody has seen would make it a fail
    // contender. Not to one in the trash: its page takes no reactions either.
    let length: Option<Option<i32>> = sqlx::query_scalar(
        "SELECT c.duration_ms FROM show_clips sc JOIN clips c ON c.id = sc.clip_id
          WHERE sc.show_id = $1 AND sc.clip_id = $2 AND NOT sc.dropped
            AND sc.played_at IS NOT NULL AND c.deleted_at IS NULL",
    )
    .bind(id)
    .bind(clip)
    .fetch_optional(pool)
    .await?;
    let Some(length) = length else {
        return Err(ShowError::Invalid(
            "that clip hasn't played in this show, or it's in the trash".into(),
        ));
    };
    // The moment is in the clip (a second's slack for players that run a little over), so
    // the replay can float it.
    if length.is_some_and(|ms| at_ms > ms.saturating_add(REACTION_SLACK_MS)) {
        return Err(ShowError::Invalid(
            "that moment is past the clip's end".into(),
        ));
    }
    sqlx::query(
        "INSERT INTO show_reactions (show_id, clip_id, user_id, emoji, at_ms)
         VALUES ($1, $2, $3, $4, $5)",
    )
    .bind(id)
    .bind(clip)
    .bind(user)
    .bind(emoji)
    .bind(at_ms)
    .execute(pool)
    .await?;
    if emoji != BANANA {
        social::react(pool, clip, user, emoji, true)
            .await
            .map_err(|e| match e {
                social::SocialError::Database(e) => ShowError::Db(e),
                other => ShowError::Invalid(other.to_string()),
            })?;
    }
    Ok(())
}

/// A vote in the finale (decision 31): people in the show, for a played clip that isn't in
/// the trash, their own included (decision 56); fail votes only for clips marked with 🍌.
/// Voting again changes the vote. A vote never lands after the show ended: the insert
/// checks the finale is still on, and waits for an `end` that's counting.
pub async fn vote(
    pool: &PgPool,
    id: Uuid,
    voter: Uuid,
    category: Category,
    clip: Uuid,
) -> Result<()> {
    let show = open_show(pool, id).await?;
    in_status(&show, &[ShowStatus::Finale], "vote")?;
    if !is_participant(pool, id, voter).await? {
        return Err(ShowError::Forbidden("only people in the show vote"));
    }
    // Your own clip too: friends clip each other (decision 56).
    let played: bool = sqlx::query_scalar(
        "SELECT EXISTS (
            SELECT FROM show_clips sc JOIN clips c ON c.id = sc.clip_id
             WHERE sc.show_id = $1 AND sc.clip_id = $2 AND NOT sc.dropped
               AND sc.played_at IS NOT NULL AND c.deleted_at IS NULL)",
    )
    .bind(id)
    .bind(clip)
    .fetch_one(pool)
    .await?;
    if !played {
        return Err(ShowError::Invalid("that clip wasn't played".into()));
    }
    if category == Category::Fail && !fail_contenders(pool, id).await?.contains(&clip) {
        return Err(ShowError::Invalid(
            "nobody marked that clip as a fail".into(),
        ));
    }
    // FOR SHARE: `end` holds the show row FOR UPDATE while it counts, so this waits for
    // it and then finds the show ended (the tally `end` stored is final).
    let cast = sqlx::query(
        "INSERT INTO show_votes (show_id, voter_id, category, clip_id)
         SELECT $1, $2, $3, $4
          WHERE EXISTS (SELECT FROM shows WHERE id = $1 AND status = 'finale' FOR SHARE)
         ON CONFLICT (show_id, voter_id, category)
         DO UPDATE SET clip_id = EXCLUDED.clip_id, created_at = now()",
    )
    .bind(id)
    .bind(voter)
    .bind(category)
    .bind(clip)
    .execute(pool)
    .await?
    .rows_affected();
    if cast == 0 {
        return Err(ShowError::Conflict("the vote is over".into()));
    }
    Ok(())
}

/// The winner of one category: the most votes, or the host's pick among a tie. `Err`
/// holds the tied clips when the host hasn't picked one of them.
fn winner(
    counts: &[VoteCount],
    category: Category,
    pick: Option<Uuid>,
) -> std::result::Result<Option<Uuid>, Vec<Uuid>> {
    let mine: Vec<&VoteCount> = counts.iter().filter(|c| c.category == category).collect();
    let Some(top) = mine.iter().map(|c| c.votes).max() else {
        return Ok(None);
    };
    let tied: Vec<Uuid> = mine
        .iter()
        .filter(|c| c.votes == top)
        .map(|c| c.clip_id)
        .collect();
    match tied.as_slice() {
        [one] => Ok(Some(*one)),
        _ => match pick {
            Some(p) if tied.contains(&p) => Ok(Some(p)),
            _ => Err(tied),
        },
    }
}

/// The host ends the finale: winners are stored and the show is over. A tied category
/// needs the host's pick in `tie_break`; else `ShowError::Tie` names the tied clips of
/// every tied category, so the host breaks them all at once. The show row stays locked
/// from the count to the end, so no vote slips in between (see `vote`).
pub async fn end(pool: &PgPool, id: Uuid, by: Uuid, tie_break: TieBreak) -> Result<Show> {
    let mut tx = pool.begin().await?;
    let show: Show = sqlx::query_as(concat!(
        "SELECT ",
        show_columns!(),
        " FROM shows WHERE id = $1 FOR UPDATE"
    ))
    .bind(id)
    .fetch_optional(&mut *tx)
    .await?
    .ok_or(ShowError::NotFound)?;
    if !show.status.is_open() {
        return Err(ShowError::Conflict("the show is over".into()));
    }
    host_only(&show, by)?;
    in_status(&show, &[ShowStatus::Finale], "end the show")?;
    let counts = tally(&mut *tx, id).await?;
    let (clip_winner, fail_winner) = match (
        winner(&counts, Category::Clip, tie_break.clip),
        winner(&counts, Category::Fail, tie_break.fail),
    ) {
        (Ok(clip), Ok(fail)) => (clip, fail),
        (clip, fail) => {
            return Err(ShowError::Tie(Ties {
                clip: clip.err().unwrap_or_default(),
                fail: fail.err().unwrap_or_default(),
            }));
        }
    };
    let ended: Show = sqlx::query_as(concat!(
        "UPDATE shows SET status = 'ended', ended_at = now(),
                clip_winner_id = $2, fail_winner_id = $3
          WHERE id = $1 RETURNING ",
        show_columns!()
    ))
    .bind(id)
    .bind(clip_winner)
    .bind(fail_winner)
    .fetch_one(&mut *tx)
    .await?;
    tx.commit().await?;
    Ok(ended)
}

/// Which of `clips` won clip of the night, and which fail of the night, in a show that
/// ended: for the badges on clip cards (S7).
pub async fn winners_among(
    pool: &PgPool,
    clips: &[Uuid],
) -> sqlx::Result<(
    std::collections::HashSet<Uuid>,
    std::collections::HashSet<Uuid>,
)> {
    let rows: Vec<(Option<Uuid>, Option<Uuid>)> = sqlx::query_as(
        "SELECT clip_winner_id, fail_winner_id FROM shows
          WHERE status = 'ended'
            AND (clip_winner_id = ANY($1) OR fail_winner_id = ANY($1))",
    )
    .bind(clips)
    .fetch_all(pool)
    .await?;
    let pick = |c: Option<Uuid>| c.filter(|c| clips.contains(c));
    Ok((
        rows.iter().filter_map(|r| pick(r.0)).collect(),
        rows.iter().filter_map(|r| pick(r.1)).collect(),
    ))
}

/// The ended show a clip last played in, and where in it: the clip page's "Played at …
/// show" line (S7).
#[derive(Debug, Clone, PartialEq, sqlx::FromRow)]
pub struct PlayedIn {
    pub show_id: Uuid,
    pub started_at: Option<DateTime<Utc>>,
    /// 1-based, in the order the show played its clips.
    pub position: i64,
    /// How many clips it played.
    pub count: i64,
    pub clip_winner_id: Option<Uuid>,
}

pub async fn played_in(pool: &PgPool, clip: Uuid) -> sqlx::Result<Option<PlayedIn>> {
    sqlx::query_as(
        "SELECT s.id AS show_id, s.started_at,
                (SELECT count(*) FROM show_clips o
                  WHERE o.show_id = s.id AND NOT o.dropped AND o.played_at IS NOT NULL
                    AND (o.played_at, o.position) <= (sc.played_at, sc.position)) AS position,
                (SELECT count(*) FROM show_clips o
                  WHERE o.show_id = s.id AND NOT o.dropped AND o.played_at IS NOT NULL) AS count,
                s.clip_winner_id
           FROM show_clips sc JOIN shows s ON s.id = sc.show_id
          WHERE sc.clip_id = $1 AND NOT sc.dropped AND sc.played_at IS NOT NULL
            AND s.status = 'ended'
          ORDER BY s.ended_at DESC LIMIT 1",
    )
    .bind(clip)
    .fetch_optional(pool)
    .await
}

/// One of someone's clips that won a category of a show that ended.
#[derive(Debug, Clone, PartialEq, sqlx::FromRow)]
pub struct Win {
    pub category: Category,
    pub clip_id: Uuid,
    pub show_id: Uuid,
    pub started_at: Option<DateTime<Utc>>,
}

/// The trophies of `owner`'s clips, newest show first (a profile's trophy shelf).
pub async fn wins(pool: &PgPool, owner: Uuid) -> sqlx::Result<Vec<Win>> {
    sqlx::query_as(
        "SELECT w.category, w.clip_id, s.id AS show_id, s.started_at
           FROM shows s
           CROSS JOIN LATERAL (VALUES ('clip', s.clip_winner_id), ('fail', s.fail_winner_id))
                AS w(category, clip_id)
           JOIN clips c ON c.id = w.clip_id
          WHERE s.status = 'ended' AND c.owner_id = $1 AND c.deleted_at IS NULL
          ORDER BY s.ended_at DESC, w.category",
    )
    .bind(owner)
    .fetch_all(pool)
    .await
}

/// How many shows `user` hosted that ended.
pub async fn hosted(pool: &PgPool, user: Uuid) -> sqlx::Result<i64> {
    sqlx::query_scalar("SELECT count(*) FROM shows WHERE host_id = $1 AND status = 'ended'")
        .bind(user)
        .fetch_one(pool)
        .await
}

/// The hub's last saved live state (S5), to resume after a restart.
pub async fn live_state(pool: &PgPool, id: Uuid) -> sqlx::Result<Option<serde_json::Value>> {
    sqlx::query_scalar("SELECT live_state FROM shows WHERE id = $1")
        .bind(id)
        .fetch_optional(pool)
        .await
        .map(Option::flatten)
}

/// "Take over as host" (decision 33): the hub checks the host has been gone long enough.
/// Only from `from`, the host the hub saw leave, so when two people take over at once only
/// the first wins. Returns whether this call did (false also when the show is over).
pub async fn set_host(pool: &PgPool, id: Uuid, from: Uuid, to: Uuid) -> Result<bool> {
    let changed = sqlx::query(
        "UPDATE shows SET host_id = $3
          WHERE id = $1 AND host_id = $2 AND status IN ('lobby', 'live', 'finale')",
    )
    .bind(id)
    .bind(from)
    .bind(to)
    .execute(pool)
    .await?
    .rows_affected();
    if changed == 0 {
        return Ok(false);
    }
    join(pool, id, to).await?;
    Ok(true)
}

/// Saves the hub's live state unless a newer one (a higher `seq`) is already stored: two
/// host tabs can save at once, and the older save may land last.
pub async fn save_newer_live_state(
    pool: &PgPool,
    id: Uuid,
    seq: i64,
    state: &serde_json::Value,
) -> sqlx::Result<()> {
    sqlx::query(
        "UPDATE shows SET live_state = $2
          WHERE id = $1 AND coalesce((live_state->>'seq')::bigint, -1) < $3",
    )
    .bind(id)
    .bind(state)
    .bind(seq)
    .execute(pool)
    .await?;
    Ok(())
}

/// Everyone left (S5's hub decides when): the show ends without a finale, and nothing in
/// it counts as played, so its clips stay for the next show (decision 33). Clips it
/// released by playing them are saved for the show again, until a week after their
/// upload as before (decision 41), so they don't spoil the show that will play them.
pub async fn abandon(pool: &PgPool, id: Uuid) -> Result<()> {
    let mut tx = pool.begin().await?;
    let changed = sqlx::query(
        "UPDATE shows SET status = 'abandoned', ended_at = now()
          WHERE id = $1 AND status IN ('lobby', 'live', 'finale')",
    )
    .bind(id)
    .execute(&mut *tx)
    .await?
    .rows_affected();
    if changed == 0 {
        return Err(ShowError::Conflict("the show is over".into()));
    }
    sqlx::query(
        "UPDATE clips c SET hold_until = c.created_at + make_interval(days => $2)
           FROM show_clips sc
          WHERE sc.show_id = $1 AND sc.clip_id = c.id AND sc.released_hold
            AND c.created_at + make_interval(days => $2) > now()",
    )
    .bind(id)
    .bind(crate::clips::HOLD_DAYS)
    .execute(&mut *tx)
    .await?;
    sqlx::query("UPDATE show_clips SET played_at = NULL, released_hold = false WHERE show_id = $1")
        .bind(id)
        .execute(&mut *tx)
        .await?;
    tx.commit().await?;
    Ok(())
}

#[cfg(test)]
mod tests {
    #[test]
    fn the_vote_runs_fail_then_clip_or_clip_alone() {
        let at = chrono::DateTime::parse_from_rfc3339("2026-10-02T20:00:00Z")
            .unwrap()
            .with_timezone(&chrono::Utc);
        let s = |n: i64| at + chrono::Duration::seconds(n);
        let both = super::vote_windows(at, true);
        assert_eq!(both.fail, Some((s(0), s(20))));
        assert_eq!(both.clip, (s(20), s(40)));
        let clip_only = super::vote_windows(at, false);
        assert_eq!(clip_only.fail, None);
        assert_eq!(clip_only.clip, (s(0), s(20)));
    }

    use super::*;
    use crate::social::tests::{ready_clip, user};

    async fn aged(pool: &PgPool, clip: Uuid, days: i32) {
        sqlx::query(
            "UPDATE clips SET created_at = now() - make_interval(days => $2) WHERE id = $1",
        )
        .bind(clip)
        .bind(days)
        .execute(pool)
        .await
        .unwrap();
    }

    fn ids(rows: &[LineupClip], dropped: bool) -> Vec<Uuid> {
        rows.iter()
            .filter(|r| r.dropped == dropped)
            .map(|r| r.clip_id)
            .collect()
    }

    #[sqlx::test(migrator = "crate::db::MIGRATOR")]
    async fn tonight_is_this_weeks_clips_before_the_first_show(pool: PgPool) {
        let sam = user(&pool, "sam").await;
        let old = ready_clip(&pool, sam, "Old", "Nuke").await;
        aged(&pool, old, 10).await;
        let a = ready_clip(&pool, sam, "A", "Nuke").await;
        let b = ready_clip(&pool, sam, "B", "Nuke").await;
        assert_eq!(tonight(&pool).await.unwrap(), [a, b]);

        let show = create(&pool, sam).await.unwrap();
        assert_eq!(show.status, ShowStatus::Lobby);
        assert_eq!(ids(&lineup(&pool, show.id).await.unwrap(), false), [a, b]);
        assert_eq!(participants(&pool, show.id).await.unwrap()[0].user_id, sam);
        assert_eq!(open(&pool).await.unwrap().unwrap().id, show.id);
        // One show at a time.
        assert!(matches!(
            create(&pool, sam).await,
            Err(ShowError::Conflict(_))
        ));
    }

    #[sqlx::test(migrator = "crate::db::MIGRATOR")]
    async fn a_whole_show(pool: PgPool) {
        let sam = user(&pool, "sam").await;
        let kim = user(&pool, "kim").await;
        let lee = user(&pool, "lee").await;
        let a = ready_clip(&pool, sam, "A", "Nuke").await;
        let b = ready_clip(&pool, kim, "B", "Nuke").await;
        let c = ready_clip(&pool, lee, "C", "Nuke").await;
        let old = ready_clip(&pool, kim, "Old", "Nuke").await;
        aged(&pool, old, 30).await;
        let show = create(&pool, sam).await.unwrap().id;

        // The host reorders and drops; only the host.
        assert!(matches!(
            set_lineup(&pool, show, kim, &[b, a]).await,
            Err(ShowError::Forbidden(_))
        ));
        set_lineup(&pool, show, sam, &[b, a]).await.unwrap();
        let rows = lineup(&pool, show).await.unwrap();
        assert_eq!(ids(&rows, false), [b, a]);
        assert_eq!(ids(&rows, true), [c]);
        assert!(matches!(
            set_lineup(&pool, show, sam, &[old]).await,
            Err(ShowError::Invalid(_))
        ));

        // Joining, then adding: an old clip, and the dropped one back at the end.
        assert!(matches!(
            add_clip(&pool, show, kim, old).await,
            Err(ShowError::Forbidden(_))
        ));
        join(&pool, show, kim).await.unwrap();
        join(&pool, show, lee).await.unwrap();
        set_ready(&pool, show, kim, true).await.unwrap();
        add_clip(&pool, show, kim, old).await.unwrap();
        add_clip(&pool, show, lee, c).await.unwrap();
        assert!(matches!(
            add_clip(&pool, show, lee, c).await,
            Err(ShowError::Conflict(_))
        ));
        assert_eq!(
            ids(&lineup(&pool, show).await.unwrap(), false),
            [b, a, old, c]
        );

        // Live: only the host starts; anyone in the show plays clips.
        assert!(matches!(
            start(&pool, show, kim).await,
            Err(ShowError::Forbidden(_))
        ));
        start(&pool, show, sam).await.unwrap();
        let outsider = user(&pool, "outsider").await;
        assert!(matches!(
            mark_played(&pool, show, outsider, b).await,
            Err(ShowError::Forbidden(_))
        ));
        mark_played(&pool, show, kim, b).await.unwrap();
        for clip in [a, c] {
            mark_played(&pool, show, sam, clip).await.unwrap();
        }
        // Reactions: 🍌 makes a fail contender; 🔥 also turns on kim's 🔥 on the clip.
        react(&pool, show, kim, a, BANANA, 420).await.unwrap();
        react(&pool, show, kim, a, "🔥", 500).await.unwrap();
        react(&pool, show, lee, a, "🔥", 510).await.unwrap();
        assert!(matches!(
            react(&pool, show, kim, a, "🍕", 1).await,
            Err(ShowError::Invalid(_))
        ));
        // A clip is 1 s long here; a moment over a second past its end is refused.
        assert!(matches!(
            react(&pool, show, kim, a, "🔥", 2_001).await,
            Err(ShowError::Invalid(_))
        ));
        let on_page = &social::reactions_for(&pool, &[a], kim).await.unwrap()[&a];
        assert_eq!((on_page[0].emoji.as_str(), on_page[0].count), ("🔥", 2));
        assert_eq!(reactions(&pool, show).await.unwrap().len(), 3);
        assert_eq!(fail_contenders(&pool, show).await.unwrap(), [a]);

        // The finale: no voting before it, for an unplayed clip, or a fail vote for a clip
        // nobody marked. Your own clip is fine (friends clip each other), and voting
        // again changes the vote.
        assert!(matches!(
            vote(&pool, show, kim, Category::Clip, a).await,
            Err(ShowError::Conflict(_))
        ));
        finale(&pool, show, sam).await.unwrap();
        vote(&pool, show, kim, Category::Clip, b).await.unwrap();
        assert!(matches!(
            vote(&pool, show, kim, Category::Clip, old).await,
            Err(ShowError::Invalid(_))
        ));
        assert!(matches!(
            vote(&pool, show, kim, Category::Fail, c).await,
            Err(ShowError::Invalid(_))
        ));
        vote(&pool, show, kim, Category::Clip, a).await.unwrap();
        vote(&pool, show, lee, Category::Clip, b).await.unwrap();
        vote(&pool, show, kim, Category::Fail, a).await.unwrap();

        // Clip of the night is tied: the host picks.
        match end(&pool, show, sam, TieBreak::default()).await {
            Err(ShowError::Tie(Ties { mut clip, fail })) => {
                clip.sort();
                let mut want = vec![a, b];
                want.sort();
                assert_eq!((clip, fail), (want, vec![]));
            }
            other => panic!("{other:?}"),
        }
        let ended = end(
            &pool,
            show,
            sam,
            TieBreak {
                clip: Some(b),
                fail: None,
            },
        )
        .await
        .unwrap();
        assert_eq!(ended.status, ShowStatus::Ended);
        assert_eq!(
            (ended.clip_winner_id, ended.fail_winner_id),
            (Some(b), Some(a))
        );
        assert!(open(&pool).await.unwrap().is_none());
        assert_eq!(past(&pool, 10).await.unwrap()[0].id, show);

        // Next time: the old clip that never played comes back; the played ones don't; a
        // clip uploaded after this show started is in.
        let d = ready_clip(&pool, lee, "D", "Nuke").await;
        assert_eq!(tonight(&pool).await.unwrap(), [old, d]);
    }

    #[sqlx::test(migrator = "crate::db::MIGRATOR")]
    async fn a_held_clip_is_hidden_until_posted_played_or_a_week_later(pool: PgPool) {
        let sam = user(&pool, "sam").await;
        let kim = user(&pool, "kim").await;
        let feed = |viewer: Uuid| {
            let pool = pool.clone();
            async move {
                crate::clips::list(
                    &pool,
                    viewer,
                    &crate::clips::Filter::default(),
                    crate::clips::Sort::New,
                    None,
                    50,
                )
                .await
                .unwrap()
                .0
                .into_iter()
                .map(|c| c.id)
                .collect::<Vec<_>>()
            }
        };
        let a = ready_clip(&pool, sam, "A", "Nuke").await;
        let b = ready_clip(&pool, sam, "B", "Nuke").await;
        crate::clips::hold(&pool, a).await.unwrap();
        crate::clips::hold(&pool, b).await.unwrap();
        assert!(
            crate::clips::get(&pool, a)
                .await
                .unwrap()
                .unwrap()
                .is_held()
        );
        // Only sam sees them; they're still tonight's clips.
        assert!(feed(kim).await.is_empty());
        assert_eq!(feed(sam).await.len(), 2);
        assert_eq!(tonight(&pool).await.unwrap(), [a, b]);

        // "Post now".
        crate::clips::release(&pool, b).await.unwrap();
        assert_eq!(feed(kim).await, [b]);

        // Played in a show.
        let show = create(&pool, sam).await.unwrap().id;
        start(&pool, show, sam).await.unwrap();
        mark_played(&pool, show, sam, a).await.unwrap();
        assert!(
            !crate::clips::get(&pool, a)
                .await
                .unwrap()
                .unwrap()
                .is_held()
        );

        // A week later on its own.
        let c = ready_clip(&pool, sam, "C", "Nuke").await;
        crate::clips::hold(&pool, c).await.unwrap();
        sqlx::query("UPDATE clips SET hold_until = now() - interval '1 minute' WHERE id = $1")
            .bind(c)
            .execute(&pool)
            .await
            .unwrap();
        assert!(feed(kim).await.contains(&c));
    }

    /// A clip that's uploaded but still processing.
    async fn processing_clip(pool: &PgPool, owner: Uuid, title: &str) -> Uuid {
        let clip = crate::clips::create(
            pool,
            crate::clips::NewClip {
                owner_id: owner,
                game_id: "cs2".into(),
                title: title.into(),
                description: String::new(),
                map: None,
                my_pov: true,
                filename: "c.mp4".into(),
                bytes: 10,
            },
        )
        .await
        .unwrap();
        crate::clips::mark_uploaded(pool, clip.id).await.unwrap();
        clip.id
    }

    async fn finish_processing(pool: &PgPool, clip: Uuid) {
        crate::clips::set_ready(
            pool,
            clip,
            &crate::clips::Transcoded {
                playback_blob: crate::clips::playback_blob(clip),
                poster_blob: crate::clips::poster_blob(clip),
                duration_ms: 1000,
                width: 1920,
                height: 1080,
                fps: 60.0,
                metadata: serde_json::json!({}),
                playback_fingerprint: crate::testing::fingerprint(clip),
            },
        )
        .await
        .unwrap();
    }

    /// Plays `clips` in a show of their own (nothing else in the lineup) and ends it.
    async fn play_show(pool: &PgPool, host: Uuid, clips: &[Uuid]) {
        let show = create(pool, host).await.unwrap().id;
        set_lineup(pool, show, host, clips).await.unwrap();
        start(pool, show, host).await.unwrap();
        for clip in clips {
            mark_played(pool, show, host, *clip).await.unwrap();
        }
        finale(pool, show, host).await.unwrap();
        end(pool, show, host, TieBreak::default()).await.unwrap();
    }

    #[sqlx::test(migrator = "crate::db::MIGRATOR")]
    async fn tonight_keeps_every_clip_until_a_show_plays_it(pool: PgPool) {
        let sam = user(&pool, "sam").await;
        let a = ready_clip(&pool, sam, "A", "Nuke").await;
        let dropped = ready_clip(&pool, sam, "Dropped", "Nuke").await;
        let slow = processing_clip(&pool, sam, "Slow").await;
        let later = processing_clip(&pool, sam, "Later").await;
        let show = create(&pool, sam).await.unwrap().id;
        assert_eq!(
            ids(&lineup(&pool, show).await.unwrap(), false),
            [a, dropped]
        );
        set_lineup(&pool, show, sam, &[a]).await.unwrap();

        // In the lobby: a new upload, and one that was processing when the show opened.
        // Both join the end of the lineup at Start.
        let lobby = ready_clip(&pool, sam, "Lobby", "Nuke").await;
        finish_processing(&pool, slow).await;
        start(&pool, show, sam).await.unwrap();
        let rows = lineup(&pool, show).await.unwrap();
        assert_eq!(ids(&rows, false), [a, slow, lobby]);
        assert_eq!(ids(&rows, true), [dropped]);

        // While it's live, new clips wait for the next show.
        let live = ready_clip(&pool, sam, "Live", "Nuke").await;
        finish_processing(&pool, later).await;
        assert_eq!(
            ids(&lineup(&pool, show).await.unwrap(), false),
            [a, slow, lobby]
        );
        mark_played(&pool, show, sam, a).await.unwrap();
        mark_played(&pool, show, sam, lobby).await.unwrap();
        finale(&pool, show, sam).await.unwrap();
        end(&pool, show, sam, TieBreak::default()).await.unwrap();

        // Next show: the dropped clip, the one not reached, the one that finished
        // processing during the show and the live upload; never the played ones.
        assert_eq!(tonight(&pool).await.unwrap(), [dropped, slow, later, live]);
        play_show(&pool, sam, &[dropped]).await;
        assert_eq!(tonight(&pool).await.unwrap(), [slow, later, live]);
        // Weeks later, a clip nobody has played is still waiting.
        aged(&pool, live, 30).await;
        assert!(tonight(&pool).await.unwrap().contains(&live));
    }

    #[sqlx::test(migrator = "crate::db::MIGRATOR")]
    async fn a_clip_dropped_in_two_shows_stops_coming_back(pool: PgPool) {
        let sam = user(&pool, "sam").await;
        let a = ready_clip(&pool, sam, "A", "Nuke").await;
        let x = ready_clip(&pool, sam, "X", "Nuke").await;
        // Dropped once: it comes back.
        play_show(&pool, sam, &[a]).await;
        assert_eq!(tonight(&pool).await.unwrap(), [x]);

        // An abandoned show's drops don't count.
        let y = ready_clip(&pool, sam, "Y", "Nuke").await;
        let show = create(&pool, sam).await.unwrap().id;
        set_lineup(&pool, show, sam, &[]).await.unwrap();
        abandon(&pool, show).await.unwrap();
        assert_eq!(tonight(&pool).await.unwrap(), [x, y]);

        // Dropped in a second ended show: x is gone from tonight; y (one drop) stays.
        let b = ready_clip(&pool, sam, "B", "Nuke").await;
        play_show(&pool, sam, &[b]).await;
        assert_eq!(tonight(&pool).await.unwrap(), [y]);

        // Still in the archive, and it can be added by hand.
        assert!(crate::clips::get(&pool, x).await.unwrap().is_some());
        let show = create(&pool, sam).await.unwrap().id;
        add_clip(&pool, show, sam, x).await.unwrap();
        assert_eq!(ids(&lineup(&pool, show).await.unwrap(), false), [y, x]);
    }

    #[sqlx::test(migrator = "crate::db::MIGRATOR")]
    async fn a_show_plays_at_most_ten_clips_and_the_rest_are_spares(pool: PgPool) {
        let sam = user(&pool, "sam").await;
        let mut clips = Vec::new();
        for n in 0..12 {
            clips.push(ready_clip(&pool, sam, &format!("Clip {n}"), "Nuke").await);
        }
        let show = create(&pool, sam).await.unwrap().id;
        let rows = lineup(&pool, show).await.unwrap();
        assert_eq!(ids(&rows, false), clips[..10]);
        assert_eq!(ids(&rows, true), clips[10..]);
        assert!(rows.iter().all(|r| r.spare == r.dropped));

        // Full: nothing more goes in, by hand or by putting a spare back.
        let late = ready_clip(&pool, sam, "Late", "Nuke").await;
        let full = |r: Result<()>| matches!(r, Err(ShowError::Conflict(m)) if m.contains("10"));
        assert!(full(add_clip(&pool, show, sam, late).await));
        assert!(full(add_clip(&pool, show, sam, clips[10]).await));
        assert!(matches!(
            set_lineup(&pool, show, sam, &clips[..11]).await,
            Err(ShowError::Invalid(_))
        ));
        // Already in: that's what it says, full or not.
        assert!(matches!(
            add_clip(&pool, show, sam, clips[0]).await,
            Err(ShowError::Conflict(m)) if m.contains("already")
        ));

        // The host drops one and puts a spare back in its place; the other spare stays a
        // spare, the dropped one is dropped.
        let mut order = clips[1..10].to_vec();
        set_lineup(&pool, show, sam, &order).await.unwrap();
        add_clip(&pool, show, sam, clips[10]).await.unwrap();
        order.push(clips[10]);
        let rows = lineup(&pool, show).await.unwrap();
        assert_eq!(ids(&rows, false), order);
        let spare = |c: Uuid| rows.iter().find(|r| r.clip_id == c).unwrap().spare;
        assert!(!spare(clips[0]) && spare(clips[11]) && !spare(clips[10]));

        // Clips that turn up in the lobby are spares at Start when the lineup is full.
        start(&pool, show, sam).await.unwrap();
        let rows = lineup(&pool, show).await.unwrap();
        assert!(
            rows.iter()
                .any(|r| r.clip_id == late && r.dropped && r.spare)
        );
        for c in &order {
            mark_played(&pool, show, sam, *c).await.unwrap();
        }
        finale(&pool, show, sam).await.unwrap();
        end(&pool, show, sam, TieBreak::default()).await.unwrap();

        // Next time the spares and the dropped one come back.
        assert_eq!(tonight(&pool).await.unwrap(), [clips[0], clips[11], late]);

        // Only the host's drops count towards stopping a clip coming back: ten older clips
        // fill the next show, and all three are spares in a second ended show.
        let mut fill = Vec::new();
        for n in 0..10 {
            let c = ready_clip(&pool, sam, &format!("Fill {n}"), "Nuke").await;
            aged(&pool, c, 1).await;
            fill.push(c);
        }
        play_show(&pool, sam, &fill).await;
        assert_eq!(tonight(&pool).await.unwrap(), [clips[0], clips[11], late]);
    }

    #[sqlx::test(migrator = "crate::db::MIGRATOR")]
    async fn the_lineup_renumbers_around_clips_played_out_of_order(pool: PgPool) {
        let sam = user(&pool, "sam").await;
        let a = ready_clip(&pool, sam, "A", "Nuke").await;
        let b = ready_clip(&pool, sam, "B", "Nuke").await;
        let c = ready_clip(&pool, sam, "C", "Nuke").await;
        let d = ready_clip(&pool, sam, "D", "Nuke").await;
        let show = create(&pool, sam).await.unwrap().id;
        start(&pool, show, sam).await.unwrap();
        mark_played(&pool, show, sam, d).await.unwrap();
        mark_played(&pool, show, sam, b).await.unwrap();

        // Played clips first, in the order they played, then the host's order.
        set_lineup(&pool, show, sam, &[c, a]).await.unwrap();
        let rows = lineup(&pool, show).await.unwrap();
        assert_eq!(ids(&rows, false), [d, b, c, a]);
        let positions: Vec<i32> = rows.iter().map(|r| r.position).collect();
        assert_eq!(positions, [0, 1, 2, 3]);

        // Dropping keeps every position distinct; the dropped clip goes last.
        set_lineup(&pool, show, sam, &[a]).await.unwrap();
        let rows = lineup(&pool, show).await.unwrap();
        assert_eq!(ids(&rows, false), [d, b, a]);
        assert_eq!(ids(&rows, true), [c]);
        assert_eq!(rows.last().unwrap().position, 3);

        // A clip twice, or one that played, is refused.
        for order in [&[a, a][..], &[d, a][..]] {
            assert!(matches!(
                set_lineup(&pool, show, sam, order).await,
                Err(ShowError::Invalid(_))
            ));
        }
    }

    #[sqlx::test(migrator = "crate::db::MIGRATOR")]
    async fn only_played_clips_take_reactions_and_trashed_ones_drop_out(pool: PgPool) {
        let sam = user(&pool, "sam").await;
        let kim = user(&pool, "kim").await;
        let a = ready_clip(&pool, sam, "A", "Nuke").await;
        let b = ready_clip(&pool, sam, "B", "Nuke").await;
        let c = ready_clip(&pool, sam, "C", "Nuke").await;
        let show = create(&pool, sam).await.unwrap().id;
        join(&pool, show, kim).await.unwrap();
        start(&pool, show, sam).await.unwrap();
        mark_played(&pool, show, sam, a).await.unwrap();
        mark_played(&pool, show, sam, b).await.unwrap();

        // Not to a clip that hasn't played yet: 🍌 there would make a fail contender.
        assert!(matches!(
            react(&pool, show, kim, c, BANANA, 0).await,
            Err(ShowError::Invalid(_))
        ));
        react(&pool, show, kim, a, BANANA, 0).await.unwrap();
        react(&pool, show, kim, b, BANANA, 0).await.unwrap();
        let mut contenders = fail_contenders(&pool, show).await.unwrap();
        contenders.sort();
        let mut want = vec![a, b];
        want.sort();
        assert_eq!(contenders, want);

        // b goes to the trash during the finale: no longer a contender, its votes stop
        // counting and it takes no more.
        finale(&pool, show, sam).await.unwrap();
        vote(&pool, show, kim, Category::Fail, b).await.unwrap();
        vote(&pool, show, kim, Category::Clip, a).await.unwrap();
        crate::clips::soft_delete(&pool, b).await.unwrap();
        assert_eq!(fail_contenders(&pool, show).await.unwrap(), [a]);
        assert!(matches!(
            vote(&pool, show, kim, Category::Clip, b).await,
            Err(ShowError::Invalid(_))
        ));
        let ended = end(&pool, show, sam, TieBreak::default()).await.unwrap();
        assert_eq!(
            (ended.clip_winner_id, ended.fail_winner_id),
            (Some(a), None)
        );
    }

    #[sqlx::test(migrator = "crate::db::MIGRATOR")]
    async fn a_vote_never_lands_after_the_show_ends(pool: PgPool) {
        let sam = user(&pool, "sam").await;
        let kim = user(&pool, "kim").await;
        let a = ready_clip(&pool, sam, "A", "Nuke").await;
        let show = create(&pool, sam).await.unwrap().id;
        join(&pool, show, kim).await.unwrap();
        start(&pool, show, sam).await.unwrap();
        mark_played(&pool, show, sam, a).await.unwrap();
        finale(&pool, show, sam).await.unwrap();

        // `end` is counting (it holds the show row) when kim's vote arrives; it ends the
        // show, and the vote finds it over instead of landing uncounted.
        let mut ending = pool.begin().await.unwrap();
        sqlx::query("SELECT FROM shows WHERE id = $1 FOR UPDATE")
            .bind(show)
            .execute(&mut *ending)
            .await
            .unwrap();
        let voting = tokio::spawn({
            let pool = pool.clone();
            async move { vote(&pool, show, kim, Category::Clip, a).await }
        });
        tokio::time::sleep(std::time::Duration::from_millis(300)).await;
        sqlx::query("UPDATE shows SET status = 'ended', ended_at = now() WHERE id = $1")
            .bind(show)
            .execute(&mut *ending)
            .await
            .unwrap();
        ending.commit().await.unwrap();
        assert!(matches!(voting.await.unwrap(), Err(ShowError::Conflict(_))));
        assert!(tally(&pool, show).await.unwrap().is_empty());
    }

    #[sqlx::test(migrator = "crate::db::MIGRATOR")]
    async fn both_ties_come_back_at_once(pool: PgPool) {
        let sam = user(&pool, "sam").await;
        let kim = user(&pool, "kim").await;
        let lee = user(&pool, "lee").await;
        let a = ready_clip(&pool, sam, "A", "Nuke").await;
        let b = ready_clip(&pool, sam, "B", "Nuke").await;
        let show = create(&pool, sam).await.unwrap().id;
        join(&pool, show, kim).await.unwrap();
        join(&pool, show, lee).await.unwrap();
        start(&pool, show, sam).await.unwrap();
        for clip in [a, b] {
            mark_played(&pool, show, sam, clip).await.unwrap();
            react(&pool, show, kim, clip, BANANA, 0).await.unwrap();
        }
        finale(&pool, show, sam).await.unwrap();
        for (voter, clip) in [(kim, a), (lee, b)] {
            vote(&pool, show, voter, Category::Clip, clip)
                .await
                .unwrap();
            vote(&pool, show, voter, Category::Fail, clip)
                .await
                .unwrap();
        }
        let mut want = vec![a, b];
        want.sort();
        match end(&pool, show, sam, TieBreak::default()).await {
            Err(ShowError::Tie(Ties { mut clip, mut fail })) => {
                clip.sort();
                fail.sort();
                assert_eq!((clip, fail), (want.clone(), want));
            }
            other => panic!("{other:?}"),
        }
        let pick = TieBreak {
            clip: Some(a),
            fail: Some(b),
        };
        let ended = end(&pool, show, sam, pick).await.unwrap();
        assert_eq!(
            (ended.clip_winner_id, ended.fail_winner_id),
            (Some(a), Some(b))
        );
    }

    #[sqlx::test(migrator = "crate::db::MIGRATOR")]
    async fn a_hold_ends_a_week_after_the_upload(pool: PgPool) {
        let sam = user(&pool, "sam").await;
        let a = ready_clip(&pool, sam, "A", "Nuke").await;
        let until = |clip: Uuid| {
            let pool = pool.clone();
            async move {
                let c = crate::clips::get(&pool, clip).await.unwrap().unwrap();
                (c.hold_until, c.created_at)
            }
        };
        assert!(crate::clips::hold(&pool, a).await.unwrap());
        let (held, created) = until(a).await;
        assert_eq!(held, Some(created + chrono::Duration::days(7)));

        // Holding again later doesn't push it back.
        aged(&pool, a, 3).await;
        assert!(crate::clips::hold(&pool, a).await.unwrap());
        let (held, created) = until(a).await;
        assert_eq!(held, Some(created + chrono::Duration::days(7)));

        // Over a week old: refused.
        aged(&pool, a, 8).await;
        assert!(!crate::clips::hold(&pool, a).await.unwrap());

        // Played in a show: refused, so it stays released.
        let b = ready_clip(&pool, sam, "B", "Nuke").await;
        play_show(&pool, sam, &[b]).await;
        assert!(!crate::clips::hold(&pool, b).await.unwrap());
        assert!(until(b).await.0.is_none());
    }

    #[sqlx::test(migrator = "crate::db::MIGRATOR")]
    async fn an_abandoned_show_plays_nothing(pool: PgPool) {
        let sam = user(&pool, "sam").await;
        let a = ready_clip(&pool, sam, "A", "Nuke").await;
        let show = create(&pool, sam).await.unwrap().id;
        start(&pool, show, sam).await.unwrap();
        mark_played(&pool, show, sam, a).await.unwrap();
        abandon(&pool, show).await.unwrap();
        assert_eq!(
            get(&pool, show).await.unwrap().unwrap().status,
            ShowStatus::Abandoned
        );
        assert!(lineup(&pool, show).await.unwrap()[0].played_at.is_none());
        assert_eq!(tonight(&pool).await.unwrap(), [a]);
        // A new show can open.
        create(&pool, sam).await.unwrap();
    }

    #[sqlx::test(migrator = "crate::db::MIGRATOR")]
    async fn a_show_needs_a_real_host(pool: PgPool) {
        assert!(matches!(
            create(&pool, Uuid::new_v4()).await,
            Err(ShowError::Db(_))
        ));
        assert!(open(&pool).await.unwrap().is_none());
    }

    #[sqlx::test(migrator = "crate::db::MIGRATOR")]
    async fn a_show_thats_over_takes_no_changes(pool: PgPool) {
        let sam = user(&pool, "sam").await;
        let a = ready_clip(&pool, sam, "A", "Nuke").await;
        let show = create(&pool, sam).await.unwrap().id;
        abandon(&pool, show).await.unwrap();
        let over =
            |r: Result<()>| matches!(r, Err(ShowError::Conflict(m)) if m == "the show is over");
        assert!(over(join(&pool, show, sam).await));
        assert!(over(set_ready(&pool, show, sam, true).await));
        assert!(over(add_clip(&pool, show, sam, a).await));
        assert!(over(set_lineup(&pool, show, sam, &[a]).await));
        assert!(over(abandon(&pool, show).await));
        assert_eq!(
            get(&pool, show).await.unwrap().unwrap().status,
            ShowStatus::Abandoned
        );
    }

    #[sqlx::test(migrator = "crate::db::MIGRATOR")]
    async fn only_people_in_the_show_take_part(pool: PgPool) {
        let sam = user(&pool, "sam").await;
        let kim = user(&pool, "kim").await;
        let lee = user(&pool, "lee").await;
        let a = ready_clip(&pool, sam, "A", "Nuke").await;
        let show = create(&pool, sam).await.unwrap().id;
        join(&pool, show, kim).await.unwrap();
        let outsider = |r: Result<()>| matches!(r, Err(ShowError::Forbidden(_)));

        // Lee never joined.
        assert!(outsider(set_ready(&pool, show, lee, true).await));
        start(&pool, show, sam).await.unwrap();
        mark_played(&pool, show, sam, a).await.unwrap();
        assert!(outsider(react(&pool, show, lee, a, "🔥", 0).await));
        // A moment before the clip starts is refused, whoever taps.
        assert!(matches!(
            react(&pool, show, kim, a, "🔥", -1).await,
            Err(ShowError::Invalid(m)) if m == "the moment can't be negative"
        ));
        react(&pool, show, kim, a, "🔥", 0).await.unwrap();
        finale(&pool, show, sam).await.unwrap();
        assert!(outsider(vote(&pool, show, lee, Category::Clip, a).await));
        vote(&pool, show, kim, Category::Clip, a).await.unwrap();
        assert_eq!(tally(&pool, show).await.unwrap().len(), 1);
    }

    #[sqlx::test(migrator = "crate::db::MIGRATOR")]
    async fn the_lineup_takes_only_clips_that_can_play(pool: PgPool) {
        let sam = user(&pool, "sam").await;
        let a = ready_clip(&pool, sam, "A", "Nuke").await;
        let show = create(&pool, sam).await.unwrap().id;
        // Sam's own upload, still processing.
        let processing = crate::clips::create(
            &pool,
            crate::clips::NewClip {
                owner_id: sam,
                game_id: "cs2".into(),
                title: "Still processing".into(),
                description: String::new(),
                map: None,
                my_pov: true,
                filename: "p.mp4".into(),
                bytes: 10,
            },
        )
        .await
        .unwrap();
        crate::clips::mark_uploaded(&pool, processing.id)
            .await
            .unwrap();
        assert!(matches!(
            add_clip(&pool, show, sam, processing.id).await,
            Err(ShowError::Invalid(m)) if m == "that clip isn't ready to play"
        ));

        start(&pool, show, sam).await.unwrap();
        // Only clips in the lineup play.
        assert!(matches!(
            mark_played(&pool, show, sam, processing.id).await,
            Err(ShowError::Invalid(m)) if m == "that clip isn't in the lineup"
        ));
        mark_played(&pool, show, sam, a).await.unwrap();
        // The finale's lineup is final.
        finale(&pool, show, sam).await.unwrap();
        assert!(matches!(
            set_lineup(&pool, show, sam, &[]).await,
            Err(ShowError::Conflict(m)) if m.starts_with("can't change the lineup")
        ));
        assert_eq!(ids(&lineup(&pool, show).await.unwrap(), false), [a]);
    }

    #[sqlx::test(migrator = "crate::db::MIGRATOR")]
    async fn a_show_moves_on_only_from_where_it_is(pool: PgPool) {
        let sam = user(&pool, "sam").await;
        let show = create(&pool, sam).await.unwrap().id;
        assert!(matches!(
            advance(&pool, show, ShowStatus::Live, ShowStatus::Finale).await,
            Err(ShowError::Conflict(m)) if m == "the show moved on meanwhile"
        ));
        assert_eq!(
            get(&pool, show).await.unwrap().unwrap().status,
            ShowStatus::Lobby
        );
    }

    /// A start that finds the show gone from the lobby by the time it writes (abandoned
    /// meanwhile) changes nothing.
    #[sqlx::test(migrator = "crate::db::MIGRATOR")]
    async fn a_start_racing_an_abandon_changes_nothing(pool: PgPool) {
        let sam = user(&pool, "sam").await;
        let early = ready_clip(&pool, sam, "Early", "Nuke").await;
        let show = create(&pool, sam).await.unwrap().id;
        let late = ready_clip(&pool, sam, "Late", "Nuke").await;
        let mut tx = pool.begin().await.unwrap();
        sqlx::query("UPDATE shows SET status = 'abandoned', ended_at = now() WHERE id = $1")
            .bind(show)
            .execute(&mut *tx)
            .await
            .unwrap();
        let starting = tokio::spawn({
            let pool = pool.clone();
            async move { start(&pool, show, sam).await }
        });
        crate::testing::until_waiting_on_a_lock(&pool).await;
        tx.commit().await.unwrap();
        assert!(matches!(
            starting.await.unwrap(),
            Err(ShowError::Conflict(m)) if m == "the show moved on meanwhile"
        ));
        let show = get(&pool, show).await.unwrap().unwrap();
        assert_eq!(show.status, ShowStatus::Abandoned);
        assert!(show.started_at.is_none());
        // The clip that turned up in the lobby didn't join it.
        let rows = lineup(&pool, show.id).await.unwrap();
        assert_eq!(ids(&rows, false), [early]);
        assert_ne!(early, late);
    }
}
