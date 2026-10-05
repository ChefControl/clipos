//! Tags, friends in a clip, and emoji reactions.

use std::collections::{HashMap, HashSet};

use serde::Serialize;
use sqlx::{Acquire, PgConnection, PgPool, Postgres};
use utoipa::ToSchema;
use uuid::Uuid;

/// Reactions offered in the UI. Anything else is rejected.
pub const EMOJIS: [&str; 6] = ["🔥", "😂", "💀", "🐐", "😮", "👏"];
pub const MAX_TAGS: usize = 10;
pub const MAX_PLAYERS: usize = 10;

/// `"Smoke Kill!"` → `smoke-kill`. `None` if nothing usable is left or it's too long.
pub fn normalize_tag(raw: &str) -> Option<String> {
    let mut tag = String::new();
    for c in raw.trim().trim_start_matches('#').chars() {
        let c = c.to_ascii_lowercase();
        if c.is_ascii_lowercase() || c.is_ascii_digit() {
            tag.push(c);
        } else if (c == '-' || c == '_' || c.is_whitespace())
            && !tag.is_empty()
            && !tag.ends_with('-')
        {
            tag.push('-');
        }
    }
    let tag = tag.trim_end_matches('-').to_owned();
    (!tag.is_empty() && tag.len() <= 24).then_some(tag)
}

#[derive(Debug, thiserror::Error)]
pub enum SocialError {
    #[error("{0}")]
    Invalid(String),
    #[error(transparent)]
    Database(#[from] sqlx::Error),
}

/// Replaces a clip's user tags (auto tags from analysis are kept). `db` may be a
/// transaction that also saves the rest of an edit.
pub async fn set_tags<'c>(
    db: impl Acquire<'c, Database = Postgres>,
    clip_id: Uuid,
    raw: &[String],
) -> Result<Vec<String>, SocialError> {
    let too_many = || SocialError::Invalid(format!("at most {MAX_TAGS} tags"));
    // The same tag twice counts once (the edit form sends what was typed), but a list
    // far longer than the cap is refused before any of it is looked at.
    if raw.len() > 2 * MAX_TAGS {
        return Err(too_many());
    }
    let mut tags = raw
        .iter()
        .map(|t| {
            normalize_tag(t).ok_or_else(|| {
                SocialError::Invalid(format!("invalid tag {t:?} (letters, digits, -; up to 24)"))
            })
        })
        .collect::<Result<Vec<_>, _>>()?;
    let mut seen = HashSet::new();
    tags.retain(|tag| seen.insert(tag.clone()));
    if tags.len() > MAX_TAGS {
        return Err(too_many());
    }

    let mut tx = db.begin().await?;
    sqlx::query("DELETE FROM clip_tags WHERE clip_id = $1 AND source = 'user'")
        .bind(clip_id)
        .execute(&mut *tx)
        .await?;
    sqlx::query("INSERT INTO tags (name) SELECT unnest($1::text[]) ON CONFLICT (name) DO NOTHING")
        .bind(&tags)
        .execute(&mut *tx)
        .await?;
    // A tag the analysis also found becomes the uploader's own: it stays if a later
    // analysis no longer finds it.
    sqlx::query(
        "INSERT INTO clip_tags (clip_id, tag_id)
         SELECT $1, id FROM tags WHERE name = ANY($2)
         ON CONFLICT (clip_id, tag_id) DO UPDATE SET source = 'user'",
    )
    .bind(clip_id)
    .bind(&tags)
    .execute(&mut *tx)
    .await?;
    tx.commit().await?;
    Ok(tags)
}

/// Replaces a clip's auto tags (from killfeed analysis). Tags the uploader set keep their
/// `user` source.
pub async fn replace_auto_tags(
    conn: &mut PgConnection,
    clip_id: Uuid,
    tags: &[String],
) -> sqlx::Result<()> {
    sqlx::query("DELETE FROM clip_tags WHERE clip_id = $1 AND source = 'auto'")
        .bind(clip_id)
        .execute(&mut *conn)
        .await?;
    sqlx::query("INSERT INTO tags (name) SELECT unnest($1::text[]) ON CONFLICT (name) DO NOTHING")
        .bind(tags)
        .execute(&mut *conn)
        .await?;
    sqlx::query(
        "INSERT INTO clip_tags (clip_id, tag_id, source)
         SELECT $1, id, 'auto' FROM tags WHERE name = ANY($2)
         ON CONFLICT DO NOTHING",
    )
    .bind(clip_id)
    .bind(tags)
    .execute(&mut *conn)
    .await?;
    Ok(())
}

/// A clip's tags: the uploader's own, and those found by killfeed analysis.
#[derive(Debug, Default, Clone, PartialEq, Eq)]
pub struct ClipTags {
    pub user: Vec<String>,
    pub auto: Vec<String>,
}

/// Tags per clip, alphabetical within each kind.
pub async fn tags_for(pool: &PgPool, clip_ids: &[Uuid]) -> sqlx::Result<HashMap<Uuid, ClipTags>> {
    let rows: Vec<(Uuid, String, String)> = sqlx::query_as(
        "SELECT ct.clip_id, t.name, ct.source FROM clip_tags ct JOIN tags t ON t.id = ct.tag_id
          WHERE ct.clip_id = ANY($1) ORDER BY t.name",
    )
    .bind(clip_ids)
    .fetch_all(pool)
    .await?;
    let mut out: HashMap<Uuid, ClipTags> = HashMap::new();
    for (clip, tag, source) in rows {
        let tags = out.entry(clip).or_default();
        if source == "auto" {
            tags.auto.push(tag);
        } else {
            tags.user.push(tag);
        }
    }
    Ok(out)
}

/// Tags starting with `prefix`, most used first (autocomplete). Only tags on clips the
/// `viewer` sees in the feed: a tag on someone's clip saved for the show, in the trash or
/// not published yet would give it away (decision 29).
pub async fn search_tags(
    pool: &PgPool,
    viewer: Uuid,
    prefix: &str,
    limit: i64,
) -> sqlx::Result<Vec<String>> {
    let prefix = normalize_tag(prefix).unwrap_or_default();
    sqlx::query_scalar(concat!(
        "SELECT t.name FROM tags t
           JOIN clip_tags ct ON ct.tag_id = t.id
           JOIN clips c ON c.id = ct.clip_id
          WHERE t.name LIKE $2 || '%' AND ",
        crate::listed_for_viewer!(),
        " GROUP BY t.name
          ORDER BY count(*) DESC, t.name
          LIMIT $3"
    ))
    .bind(viewer)
    .bind(prefix)
    .bind(limit)
    .fetch_all(pool)
    .await
}

/// A user as other members see them (no email).
#[derive(Debug, Clone, Serialize, sqlx::FromRow, ToSchema)]
#[serde(rename_all = "camelCase")]
pub struct Member {
    #[serde(skip)]
    pub clip_id: Option<Uuid>,
    pub id: Uuid,
    pub handle: String,
    pub display_name: String,
    pub avatar_url: Option<String>,
    pub steam_name: Option<String>,
}

/// Replaces who's tagged as playing in a clip. Someone new must be an active member;
/// friends tagged already stay even if they've been disabled since (the edit dialog sends
/// the whole list back). `db` may be a transaction that also saves the rest of an edit.
pub async fn set_players<'c>(
    db: impl Acquire<'c, Database = Postgres>,
    clip_id: Uuid,
    user_ids: &[Uuid],
) -> Result<(), SocialError> {
    let mut ids = user_ids.to_vec();
    ids.sort();
    ids.dedup();
    if ids.len() > MAX_PLAYERS {
        return Err(SocialError::Invalid(format!(
            "at most {MAX_PLAYERS} players"
        )));
    }
    let mut tx = db.begin().await?;
    let known: i64 = sqlx::query_scalar(
        "SELECT count(*) FROM users
          WHERE id = ANY($1)
            AND (status = 'active'
                 OR id IN (SELECT user_id FROM clip_players WHERE clip_id = $2))",
    )
    .bind(&ids)
    .bind(clip_id)
    .fetch_one(&mut *tx)
    .await?;
    if known != ids.len() as i64 {
        return Err(SocialError::Invalid("unknown player".into()));
    }
    sqlx::query("DELETE FROM clip_players WHERE clip_id = $1")
        .bind(clip_id)
        .execute(&mut *tx)
        .await?;
    sqlx::query("INSERT INTO clip_players (clip_id, user_id) SELECT $1, unnest($2::uuid[])")
        .bind(clip_id)
        .bind(&ids)
        .execute(&mut *tx)
        .await?;
    tx.commit().await?;
    Ok(())
}

pub async fn players_for(
    pool: &PgPool,
    clip_ids: &[Uuid],
) -> sqlx::Result<HashMap<Uuid, Vec<Member>>> {
    let rows: Vec<Member> = sqlx::query_as(
        "SELECT cp.clip_id, u.id, u.handle, u.display_name, u.avatar_url, u.steam_name
           FROM clip_players cp JOIN users u ON u.id = cp.user_id
          WHERE cp.clip_id = ANY($1) ORDER BY u.display_name",
    )
    .bind(clip_ids)
    .fetch_all(pool)
    .await?;
    let mut out: HashMap<Uuid, Vec<Member>> = HashMap::new();
    for m in rows {
        out.entry(m.clip_id.expect("selected")).or_default().push(m);
    }
    Ok(out)
}

/// Every active member (for the "who's in this clip" picker).
pub async fn members(pool: &PgPool) -> sqlx::Result<Vec<Member>> {
    sqlx::query_as(
        "SELECT NULL::uuid AS clip_id, id, handle, display_name, avatar_url, steam_name
           FROM users WHERE status = 'active' ORDER BY display_name",
    )
    .fetch_all(pool)
    .await
}

/// Members by id, disabled ones included (a show keeps who was in it).
pub async fn members_by_ids(pool: &PgPool, ids: &[Uuid]) -> sqlx::Result<Vec<Member>> {
    sqlx::query_as(
        "SELECT NULL::uuid AS clip_id, id, handle, display_name, avatar_url, steam_name
           FROM users WHERE id = ANY($1)",
    )
    .bind(ids)
    .fetch_all(pool)
    .await
}

pub async fn member_by_handle(pool: &PgPool, handle: &str) -> sqlx::Result<Option<Member>> {
    sqlx::query_as(
        "SELECT NULL::uuid AS clip_id, id, handle, display_name, avatar_url, steam_name
           FROM users WHERE handle = $1",
    )
    .bind(handle.to_lowercase())
    .fetch_optional(pool)
    .await
}

#[derive(Debug, Clone, PartialEq, Serialize, ToSchema)]
#[serde(rename_all = "camelCase")]
pub struct ReactionCount {
    pub emoji: String,
    pub count: i64,
    /// The viewer reacted with this emoji.
    pub mine: bool,
}

/// Adds or removes the viewer's `emoji` on a clip and keeps `clips.reaction_count` in step.
/// One statement each way, counting up or down by what it inserted or deleted: a recount
/// would miss a reaction another request adds at the same time.
pub async fn react(
    pool: &PgPool,
    clip_id: Uuid,
    user_id: Uuid,
    emoji: &str,
    on: bool,
) -> Result<(), SocialError> {
    if !EMOJIS.contains(&emoji) {
        return Err(SocialError::Invalid("unsupported reaction".into()));
    }
    if on {
        sqlx::query(
            "WITH added AS (
                 INSERT INTO reactions (clip_id, user_id, emoji) VALUES ($1, $2, $3)
                 ON CONFLICT DO NOTHING RETURNING clip_id)
             UPDATE clips SET reaction_count = reaction_count + 1
              WHERE id = $1 AND EXISTS (SELECT FROM added)",
        )
    } else {
        sqlx::query(
            "WITH removed AS (
                 DELETE FROM reactions WHERE clip_id = $1 AND user_id = $2 AND emoji = $3
                 RETURNING clip_id)
             UPDATE clips SET reaction_count = reaction_count - 1
              WHERE id = $1 AND EXISTS (SELECT FROM removed)",
        )
    }
    .bind(clip_id)
    .bind(user_id)
    .bind(emoji)
    .execute(pool)
    .await?;
    Ok(())
}

/// Reaction counts per clip, in `EMOJIS` order, only emojis someone used.
pub async fn reactions_for(
    pool: &PgPool,
    clip_ids: &[Uuid],
    viewer: Uuid,
) -> sqlx::Result<HashMap<Uuid, Vec<ReactionCount>>> {
    let rows: Vec<(Uuid, String, i64, bool)> = sqlx::query_as(
        "SELECT clip_id, emoji, count(*), bool_or(user_id = $2)
           FROM reactions WHERE clip_id = ANY($1)
          GROUP BY clip_id, emoji",
    )
    .bind(clip_ids)
    .bind(viewer)
    .fetch_all(pool)
    .await?;
    let mut out: HashMap<Uuid, Vec<ReactionCount>> = HashMap::new();
    for (clip, emoji, count, mine) in rows {
        out.entry(clip)
            .or_default()
            .push(ReactionCount { emoji, count, mine });
    }
    for list in out.values_mut() {
        list.sort_by_key(|r| EMOJIS.iter().position(|e| *e == r.emoji));
    }
    Ok(out)
}

#[cfg(test)]
pub(crate) mod tests {
    use super::*;
    use crate::{
        clips::{self, ClipStatus, Cursor, Filter, NewClip, Sort, Transcoded},
        users::{self, Identity, SignIn},
    };

    pub(crate) async fn user(pool: &PgPool, name: &str) -> Uuid {
        let email = format!("{name}@gmail.com");
        sqlx::query("INSERT INTO invites (email) VALUES ($1)")
            .bind(&email)
            .execute(pool)
            .await
            .unwrap();
        let sub = format!("google-oauth2|{name}");
        let identity = Identity {
            sub: &sub,
            email: &email,
            name: Some(name),
            picture: None,
        };
        match users::sign_in(pool, &identity).await.unwrap() {
            SignIn::Allowed(u) => u.id,
            other => panic!("{other:?}"),
        }
    }

    pub(crate) async fn ready_clip(pool: &PgPool, owner_id: Uuid, title: &str, map: &str) -> Uuid {
        let clip = clips::create(
            pool,
            NewClip {
                owner_id,
                game_id: "cs2".into(),
                title: title.into(),
                description: String::new(),
                map: Some(map.into()),
                my_pov: true,
                filename: "c.mp4".into(),
                bytes: 10,
            },
        )
        .await
        .unwrap();
        clips::mark_uploaded(pool, clip.id).await.unwrap();
        clips::set_ready(
            pool,
            clip.id,
            &Transcoded {
                playback_blob: clips::playback_blob(clip.id),
                poster_blob: clips::poster_blob(clip.id),
                duration_ms: 1000,
                width: 1920,
                height: 1080,
                fps: 60.0,
                metadata: serde_json::json!({}),
                playback_fingerprint: crate::testing::fingerprint(clip.id),
            },
        )
        .await
        .unwrap();
        clip.id
    }

    async fn titles(pool: &PgPool, viewer: Uuid, filter: Filter, sort: Sort) -> Vec<String> {
        clips::list(pool, viewer, &filter, sort, None, 50)
            .await
            .unwrap()
            .0
            .into_iter()
            .map(|c| c.title)
            .collect()
    }

    #[sqlx::test(migrator = "crate::db::MIGRATOR")]
    async fn tags_players_reactions_and_filters(pool: PgPool) {
        let sam = user(&pool, "sam").await;
        let kim = user(&pool, "kim").await;
        let ace = ready_clip(&pool, sam, "Ace", "Mirage").await;
        let clutch = ready_clip(&pool, kim, "Clutch", "Inferno").await;

        let tags = set_tags(
            &pool,
            ace,
            &["Ace".into(), "#ace".into(), "smoke kill".into()],
        )
        .await
        .unwrap();
        assert_eq!(tags, ["ace", "smoke-kill"]);
        assert!(matches!(
            set_tags(&pool, ace, &["!!!".into()]).await,
            Err(SocialError::Invalid(_))
        ));
        assert_eq!(
            tags_for(&pool, &[ace]).await.unwrap()[&ace].user,
            ["ace", "smoke-kill"]
        );

        // Auto tags sit beside the uploader's own; one the uploader also set stays theirs.
        let mut conn = pool.acquire().await.unwrap();
        replace_auto_tags(&mut conn, ace, &["ace".into(), "hs".into()])
            .await
            .unwrap();
        assert_eq!(
            tags_for(&pool, &[ace]).await.unwrap()[&ace],
            ClipTags {
                user: vec!["ace".into(), "smoke-kill".into()],
                auto: vec!["hs".into()],
            }
        );
        replace_auto_tags(&mut conn, ace, &[]).await.unwrap();
        set_tags(&pool, ace, &["hs".into()]).await.unwrap();
        replace_auto_tags(&mut conn, ace, &["hs".into(), "4k".into()])
            .await
            .unwrap();
        assert_eq!(
            tags_for(&pool, &[ace]).await.unwrap()[&ace],
            ClipTags {
                user: vec!["hs".into()],
                auto: vec!["4k".into()],
            }
        );
        // Editing the uploader's tags keeps the auto ones.
        set_tags(&pool, ace, &["ace".into(), "smoke-kill".into()])
            .await
            .unwrap();
        assert_eq!(tags_for(&pool, &[ace]).await.unwrap()[&ace].auto, ["4k"]);
        assert_eq!(
            search_tags(&pool, sam, "SM", 10).await.unwrap(),
            ["smoke-kill"]
        );

        set_players(&pool, clutch, &[sam, kim, sam]).await.unwrap();
        assert_eq!(
            players_for(&pool, &[clutch]).await.unwrap()[&clutch].len(),
            2
        );
        assert!(matches!(
            set_players(&pool, clutch, &[Uuid::new_v4()]).await,
            Err(SocialError::Invalid(_))
        ));

        react(&pool, clutch, sam, "🔥", true).await.unwrap();
        react(&pool, clutch, sam, "🔥", true).await.unwrap();
        react(&pool, clutch, kim, "🔥", true).await.unwrap();
        react(&pool, clutch, kim, "💀", true).await.unwrap();
        react(&pool, clutch, kim, "💀", false).await.unwrap();
        assert!(matches!(
            react(&pool, clutch, kim, "🍕", true).await,
            Err(SocialError::Invalid(_))
        ));
        let counts = &reactions_for(&pool, &[clutch], sam).await.unwrap()[&clutch];
        assert_eq!(
            counts,
            &[ReactionCount {
                emoji: "🔥".into(),
                count: 2,
                mine: true
            }]
        );

        let any = Filter::default();
        assert_eq!(
            titles(&pool, sam, any.clone(), Sort::New).await,
            ["Clutch", "Ace"]
        );
        assert_eq!(
            titles(&pool, sam, any.clone(), Sort::Top).await,
            ["Clutch", "Ace"]
        );
        let only = |f: Filter| titles(&pool, sam, f, Sort::New);
        assert_eq!(
            only(Filter {
                map: Some("mirage".into()),
                ..any.clone()
            })
            .await,
            ["Ace"]
        );
        assert_eq!(
            only(Filter {
                uploader: Some("kim".into()),
                ..any.clone()
            })
            .await,
            ["Clutch"]
        );
        assert_eq!(
            only(Filter {
                tags: vec!["ace".into()],
                ..any.clone()
            })
            .await,
            ["Ace"]
        );
        // Any of several tags, auto tags included ("4K and aces").
        assert_eq!(
            only(Filter {
                tags: vec!["4k".into(), "nope".into()],
                ..any.clone()
            })
            .await,
            ["Ace"]
        );
        assert_eq!(
            only(Filter {
                search: Some("CLUT".into()),
                ..any.clone()
            })
            .await,
            ["Clutch"]
        );
        // The uploader's name, and the user's % matched literally.
        assert_eq!(
            only(Filter {
                search: Some("kim".into()),
                ..any.clone()
            })
            .await,
            ["Clutch"]
        );
        assert!(
            only(Filter {
                search: Some("%".into()),
                ..any.clone()
            })
            .await
            .is_empty()
        );
        assert_eq!(
            only(Filter {
                reaction: Some("🔥".into()),
                ..any.clone()
            })
            .await,
            ["Clutch"]
        );
        assert!(
            only(Filter {
                reaction: Some("💀".into()),
                ..any.clone()
            })
            .await
            .is_empty()
        );
        assert_eq!(
            only(Filter {
                player: Some("sam".into()),
                ..any.clone()
            })
            .await,
            ["Clutch"]
        );
        assert!(
            only(Filter {
                game: Some("valorant".into()),
                ..any.clone()
            })
            .await
            .is_empty()
        );
    }

    #[sqlx::test(migrator = "crate::db::MIGRATOR")]
    async fn tag_search_offers_only_tags_of_clips_the_viewer_sees(pool: PgPool) {
        let sam = user(&pool, "sam").await;
        let kim = user(&pool, "kim").await;
        let smoke = ready_clip(&pool, kim, "Smoke", "Nuke").await;
        set_tags(&pool, smoke, &["smoke".into()]).await.unwrap();
        for title in ["A", "B"] {
            let held = ready_clip(&pool, sam, title, "Nuke").await;
            set_tags(&pool, held, &["smash".into()]).await.unwrap();
            assert!(clips::hold(&pool, held).await.unwrap());
        }
        let trashed = ready_clip(&pool, sam, "Trashed", "Nuke").await;
        set_tags(&pool, trashed, &["spray".into()]).await.unwrap();
        clips::soft_delete(&pool, trashed).await.unwrap();
        let failed = ready_clip(&pool, sam, "Failed", "Nuke").await;
        set_tags(&pool, failed, &["swing".into()]).await.unwrap();
        // It fails while processing (`set_failed` leaves a ready clip alone).
        sqlx::query("UPDATE clips SET status = 'processing' WHERE id = $1")
            .bind(failed)
            .execute(&pool)
            .await
            .unwrap();
        clips::set_failed(&pool, failed, "no video").await.unwrap();

        // Held, trashed and failed clips' tags are only offered to their uploader (and
        // not for the trashed one at all: it's out of the feed for everyone).
        assert_eq!(search_tags(&pool, kim, "s", 10).await.unwrap(), ["smoke"]);
        assert_eq!(
            search_tags(&pool, sam, "s", 10).await.unwrap(),
            ["smash", "smoke", "swing"]
        );
    }

    #[sqlx::test(migrator = "crate::db::MIGRATOR")]
    async fn pages_through_the_feed(pool: PgPool) {
        let sam = user(&pool, "sam").await;
        for i in 0..5 {
            ready_clip(&pool, sam, &format!("clip {i}"), "Nuke").await;
        }
        for sort in [Sort::New, Sort::Top] {
            let mut seen = Vec::new();
            let mut cursor: Option<Cursor> = None;
            loop {
                let (page, next) =
                    clips::list(&pool, sam, &Filter::default(), sort, cursor.as_ref(), 2)
                        .await
                        .unwrap();
                seen.extend(page.into_iter().map(|c| c.id));
                match next {
                    Some(c) => cursor = Some(Cursor::decode(&c.encode()).unwrap()),
                    None => break,
                }
            }
            let mut unique = seen.clone();
            unique.sort();
            unique.dedup();
            assert_eq!((seen.len(), unique.len()), (5, 5), "{sort:?}");
        }
        assert!(Cursor::decode("not a cursor").is_none());
    }

    #[sqlx::test(migrator = "crate::db::MIGRATOR")]
    async fn trash_and_restore(pool: PgPool) {
        let sam = user(&pool, "sam").await;
        let id = ready_clip(&pool, sam, "Oops", "Train").await;
        assert!(clips::soft_delete(&pool, id).await.unwrap());
        assert!(clips::get(&pool, id).await.unwrap().is_none());
        assert_eq!(clips::trash(&pool, sam).await.unwrap().len(), 1);
        assert!(clips::expired_trash(&pool).await.unwrap().is_empty());
        assert_eq!(
            clips::restore(&pool, id).await.unwrap(),
            clips::Restore::Restored
        );
        assert_eq!(
            clips::get(&pool, id).await.unwrap().unwrap().status,
            ClipStatus::Ready
        );

        clips::soft_delete(&pool, id).await.unwrap();
        sqlx::query("UPDATE clips SET deleted_at = now() - interval '8 days'")
            .execute(&pool)
            .await
            .unwrap();
        let expired = clips::expired_trash(&pool).await.unwrap();
        assert_eq!(expired.len(), 1);
        assert_eq!(expired[0].playback_blob, Some(clips::playback_blob(id)));
    }

    #[sqlx::test(migrator = "crate::db::MIGRATOR")]
    async fn tags_and_players_are_capped(pool: PgPool) {
        let sam = user(&pool, "sam").await;
        let clip = ready_clip(&pool, sam, "Ace", "Nuke").await;
        set_tags(&pool, clip, &["ace".into()]).await.unwrap();

        // The same tag twice counts once, so ten distinct ones, each written twice, pass.
        let ten: Vec<String> = (0..MAX_TAGS)
            .flat_map(|i| [format!("t{i}"), format!("#T{i}")])
            .collect();
        assert_eq!(set_tags(&pool, clip, &ten).await.unwrap().len(), MAX_TAGS);
        let eleven: Vec<String> = (0..=MAX_TAGS).map(|i| format!("t{i}")).collect();
        let Err(SocialError::Invalid(message)) = set_tags(&pool, clip, &eleven).await else {
            panic!("eleven tags were accepted");
        };
        assert_eq!(message, "at most 10 tags");
        // Up to twice the cap may be repeats; a longer list is refused before any of it is
        // read, so it doesn't matter that none of these is a tag.
        let junk = vec![String::from("?"); 2 * MAX_TAGS + 1];
        let Err(SocialError::Invalid(message)) = set_tags(&pool, clip, &junk).await else {
            panic!("a long list was read");
        };
        assert_eq!(message, "at most 10 tags");
        // Enough distinct tags that comparing each with the rest would take minutes.
        let flood: Vec<String> = (0..200_000).map(|i| format!("t{i}")).collect();
        let started = std::time::Instant::now();
        assert!(set_tags(&pool, clip, &flood).await.is_err());
        assert!(started.elapsed() < std::time::Duration::from_secs(1));
        // Nothing changed.
        assert_eq!(
            tags_for(&pool, &[clip]).await.unwrap()[&clip].user.len(),
            MAX_TAGS
        );

        let mut ids = Vec::new();
        for i in 0..=MAX_PLAYERS {
            ids.push(user(&pool, &format!("p{i}")).await);
        }
        // Repeats count once here too.
        let mut ten = ids[..MAX_PLAYERS].to_vec();
        ten.push(ids[0]);
        set_players(&pool, clip, &ten).await.unwrap();
        let Err(SocialError::Invalid(message)) = set_players(&pool, clip, &ids).await else {
            panic!("eleven players were accepted");
        };
        assert_eq!(message, "at most 10 players");
        assert_eq!(
            players_for(&pool, &[clip]).await.unwrap()[&clip].len(),
            MAX_PLAYERS
        );
    }

    #[test]
    fn normalizes_tags() {
        assert_eq!(normalize_tag("Smoke Kill!").as_deref(), Some("smoke-kill"));
        assert_eq!(normalize_tag("#ACE").as_deref(), Some("ace"));
        assert_eq!(normalize_tag("1v3 clutch").as_deref(), Some("1v3-clutch"));
        assert_eq!(normalize_tag("  --  "), None);
        assert_eq!(normalize_tag(&"x".repeat(25)), None);
    }
}
