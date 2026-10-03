//! Public share links: one revocable token per clip that lets anyone watch it.

use base64::{Engine, engine::general_purpose::URL_SAFE_NO_PAD};
use chrono::{DateTime, Utc};
use sqlx::PgPool;
use uuid::Uuid;

/// A fresh token: 128 random bits as 22 URL-safe characters.
pub fn new_token() -> String {
    URL_SAFE_NO_PAD.encode(Uuid::new_v4().as_bytes())
}

/// Cheap shape check before touching the database (also keeps junk out of logs).
pub fn is_token(raw: &str) -> bool {
    raw.len() == 22
        && raw
            .bytes()
            .all(|b| b.is_ascii_alphanumeric() || b == b'-' || b == b'_')
}

/// The clip's live link, creating one if there is none.
pub async fn share(pool: &PgPool, clip_id: Uuid, by: Uuid) -> sqlx::Result<String> {
    if let Some(token) = active_token(pool, clip_id).await? {
        return Ok(token);
    }
    let inserted: Option<String> = sqlx::query_scalar(
        "INSERT INTO share_links (token, clip_id, created_by) VALUES ($1, $2, $3)
         ON CONFLICT (clip_id) WHERE revoked_at IS NULL DO NOTHING
         RETURNING token",
    )
    .bind(new_token())
    .bind(clip_id)
    .bind(by)
    .fetch_optional(pool)
    .await?;
    match inserted {
        Some(token) => Ok(token),
        // Lost a race with a concurrent share: use theirs.
        None => Ok(active_token(pool, clip_id)
            .await?
            .expect("a live link exists")),
    }
}

pub async fn active_token(pool: &PgPool, clip_id: Uuid) -> sqlx::Result<Option<String>> {
    sqlx::query_scalar("SELECT token FROM share_links WHERE clip_id = $1 AND revoked_at IS NULL")
        .bind(clip_id)
        .fetch_optional(pool)
        .await
}

/// Live tokens for several clips (to show the link to whoever may manage it).
pub async fn active_tokens(
    pool: &PgPool,
    clip_ids: &[Uuid],
) -> sqlx::Result<std::collections::HashMap<Uuid, String>> {
    let rows: Vec<(Uuid, String)> = sqlx::query_as(
        "SELECT clip_id, token FROM share_links WHERE clip_id = ANY($1) AND revoked_at IS NULL",
    )
    .bind(clip_ids)
    .fetch_all(pool)
    .await?;
    Ok(rows.into_iter().collect())
}

/// Revokes the clip's live link. Returns whether there was one.
pub async fn revoke(pool: &PgPool, clip_id: Uuid) -> sqlx::Result<bool> {
    Ok(sqlx::query(
        "UPDATE share_links SET revoked_at = now() WHERE clip_id = $1 AND revoked_at IS NULL",
    )
    .bind(clip_id)
    .execute(pool)
    .await?
    .rows_affected()
        > 0)
}

/// What the public may see of a shared clip.
#[derive(Debug, Clone, sqlx::FromRow)]
pub struct SharedClip {
    pub clip_id: Uuid,
    pub title: String,
    pub map: Option<String>,
    pub uploader: String,
    pub playback_blob: String,
    pub poster_blob: String,
    pub duration_ms: Option<i32>,
    pub width: Option<i32>,
    pub height: Option<i32>,
    pub fps: Option<f32>,
    pub created_at: DateTime<Utc>,
}

/// The clip behind a live token, if it's still watchable (ready, not deleted).
pub async fn resolve(pool: &PgPool, token: &str) -> sqlx::Result<Option<SharedClip>> {
    if !is_token(token) {
        return Ok(None);
    }
    sqlx::query_as(
        "SELECT c.id AS clip_id, c.title, c.map, u.display_name AS uploader,
                c.playback_blob, c.poster_blob, c.duration_ms, c.width, c.height, c.fps,
                c.created_at
           FROM share_links s
           JOIN clips c ON c.id = s.clip_id
           JOIN users u ON u.id = c.owner_id
          WHERE s.token = $1 AND s.revoked_at IS NULL
            AND c.deleted_at IS NULL AND c.status = 'ready'
            AND c.playback_blob IS NOT NULL AND c.poster_blob IS NOT NULL",
    )
    .bind(token)
    .fetch_optional(pool)
    .await
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::{
        clips::{self, NewClip, Transcoded},
        users::{self, Identity, SignIn},
    };

    #[test]
    fn tokens_are_url_safe_and_unique() {
        let a = new_token();
        assert!(is_token(&a), "{a}");
        assert_ne!(a, new_token());
        assert!(!is_token("short"));
        assert!(!is_token("../../../../../etc/pass"));
    }

    async fn ready_clip(pool: &PgPool) -> (Uuid, Uuid) {
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
        let owner = match users::sign_in(pool, &identity).await.unwrap() {
            SignIn::Allowed(u) => u.id,
            other => panic!("{other:?}"),
        };
        let clip = clips::create(
            pool,
            NewClip {
                owner_id: owner,
                game_id: "cs2".into(),
                title: "Ace".into(),
                description: String::new(),
                map: None,
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
                width: 1280,
                height: 960,
                fps: 60.0,
                metadata: serde_json::json!({}),
            },
        )
        .await
        .unwrap();
        (clip.id, owner)
    }

    #[sqlx::test(migrator = "crate::db::MIGRATOR")]
    async fn share_resolve_revoke(pool: PgPool) {
        let (clip, owner) = ready_clip(&pool).await;
        let token = share(&pool, clip, owner).await.unwrap();
        assert_eq!(
            share(&pool, clip, owner).await.unwrap(),
            token,
            "one live link"
        );
        let shared = resolve(&pool, &token).await.unwrap().unwrap();
        assert_eq!(
            (shared.title.as_str(), shared.uploader.as_str()),
            ("Ace", "Sam")
        );
        assert_eq!(shared.width, Some(1280));

        assert!(revoke(&pool, clip).await.unwrap());
        assert!(resolve(&pool, &token).await.unwrap().is_none());
        let again = share(&pool, clip, owner).await.unwrap();
        assert_ne!(
            again, token,
            "re-sharing makes a new link; the old one stays dead"
        );

        clips::soft_delete(&pool, clip).await.unwrap();
        assert!(
            resolve(&pool, &again).await.unwrap().is_none(),
            "deleted clips go dark"
        );
        // The trash revoked it, so a restore doesn't bring it back.
        clips::restore(&pool, clip).await.unwrap();
        assert!(resolve(&pool, &again).await.unwrap().is_none());
        assert_eq!(active_token(&pool, clip).await.unwrap(), None);
    }

    /// Two shares at once: the one that loses the insert hands out the winner's link.
    #[sqlx::test(migrator = "crate::db::MIGRATOR")]
    async fn sharing_at_once_gives_one_link(pool: PgPool) {
        let (clip, owner) = ready_clip(&pool).await;
        // The first share has inserted its link but not committed yet.
        let first = new_token();
        let mut tx = pool.begin().await.unwrap();
        sqlx::query("INSERT INTO share_links (token, clip_id, created_by) VALUES ($1, $2, $3)")
            .bind(&first)
            .bind(clip)
            .bind(owner)
            .execute(&mut *tx)
            .await
            .unwrap();
        let second = tokio::spawn({
            let pool = pool.clone();
            async move { share(&pool, clip, owner).await }
        });
        crate::testing::until_waiting_on_a_lock(&pool).await;
        tx.commit().await.unwrap();
        assert_eq!(second.await.unwrap().unwrap(), first);
        let links: i64 = sqlx::query_scalar("SELECT count(*) FROM share_links WHERE clip_id = $1")
            .bind(clip)
            .fetch_one(&pool)
            .await
            .unwrap();
        assert_eq!(links, 1);
    }
}
