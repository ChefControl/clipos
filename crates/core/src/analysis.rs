//! Killfeed analysis results (phase 8), stored per clip.

use std::collections::HashMap;

use serde_json::Value;
use sqlx::PgPool;
use uuid::Uuid;

use crate::{clips::ANALYSE_JOB, social};

/// Modifier (as the analysis names it) → its auto tag. A tag is given when at least one of
/// the recording player's kills has the modifier. Kept in step with the backfill in
/// `migrations/0007_auto_tags.sql`.
pub const MODIFIER_TAGS: [(&str, &str); 6] = [
    ("headshot", "hs"),
    ("wallbang", "wallbang"),
    ("through_smoke", "smoke-kill"),
    ("noscope", "noscope"),
    ("blind", "blind-kill"),
    ("in_air", "air-kill"),
];

/// The auto tags for a clip's stats: its multi-kill (`2k`, `3k`, `4k`, `ace`) and one per
/// modifier the player killed with. Weapons get no tags.
pub fn auto_tags(stats: &Value) -> Vec<String> {
    let mut tags = Vec::new();
    if let Some(multi) = stats["multi_kill"].as_str() {
        tags.push(multi.to_lowercase());
    }
    for (modifier, tag) in MODIFIER_TAGS {
        if stats["modifiers"][modifier].as_u64().unwrap_or(0) > 0 {
            tags.push(tag.to_owned());
        }
    }
    tags
}

/// Stores a clip's analysis and its auto tags, replacing any earlier ones (a re-run after
/// a model update). A person's correction of the earlier result is dropped with it.
pub async fn save(
    pool: &PgPool,
    clip_id: Uuid,
    analyser_version: &str,
    raw: &Value,
    stats: &Value,
) -> sqlx::Result<()> {
    let mut tx = pool.begin().await?;
    sqlx::query(
        "INSERT INTO analysis_results (clip_id, analyser_version, raw, stats)
         VALUES ($1, $2, $3, $4)
         ON CONFLICT (clip_id) DO UPDATE
            SET analyser_version = EXCLUDED.analyser_version, raw = EXCLUDED.raw,
                stats = EXCLUDED.stats, corrected = NULL, created_at = now()",
    )
    .bind(clip_id)
    .bind(analyser_version)
    .bind(raw)
    .bind(stats)
    .execute(&mut *tx)
    .await?;
    social::replace_auto_tags(&mut tx, clip_id, &auto_tags(stats)).await?;
    tx.commit().await
}

/// A clip's stored analysis: `(analyser_version, raw, stats)`.
pub async fn get(pool: &PgPool, clip_id: Uuid) -> sqlx::Result<Option<(String, Value, Value)>> {
    sqlx::query_as("SELECT analyser_version, raw, stats FROM analysis_results WHERE clip_id = $1")
        .bind(clip_id)
        .fetch_optional(pool)
        .await
}

/// The recording player's multi-kill per clip (`2k` … `ace`), for clips that have one.
pub async fn multi_kills(pool: &PgPool, clip_ids: &[Uuid]) -> sqlx::Result<HashMap<Uuid, String>> {
    let rows: Vec<(Uuid, String)> = sqlx::query_as(
        "SELECT clip_id, stats->>'multi_kill' FROM analysis_results
          WHERE clip_id = ANY($1) AND stats->>'multi_kill' IS NOT NULL",
    )
    .bind(clip_ids)
    .fetch_all(pool)
    .await?;
    Ok(rows.into_iter().collect())
}

/// Whether an analysis of the clip is queued or running.
pub async fn pending(pool: &PgPool, clip_id: Uuid) -> sqlx::Result<bool> {
    sqlx::query_scalar(
        "SELECT EXISTS (SELECT FROM jobs WHERE kind = $1 AND status IN ('queued', 'running')
                          AND payload->>'clipId' = $2)",
    )
    .bind(ANALYSE_JOB)
    .bind(clip_id.to_string())
    .fetch_one(pool)
    .await
}

#[cfg(test)]
mod tests {
    use serde_json::json;

    use super::*;

    #[test]
    fn auto_tags_from_stats() {
        let stats = json!({
            "kills": 8, "my_kills": 4, "my_deaths": 0, "multi_kill": "4k",
            "weapons": {"ak47": 4},
            "modifiers": {"headshot": 1, "through_smoke": 1, "wallbang": 1},
        });
        assert_eq!(auto_tags(&stats), ["4k", "hs", "wallbang", "smoke-kill"]);
        let quiet = json!({"kills": 9, "my_kills": 0, "my_deaths": 0, "multi_kill": null,
                           "weapons": {}, "modifiers": {}});
        assert!(auto_tags(&quiet).is_empty());
    }

    #[sqlx::test(migrator = "crate::db::MIGRATOR")]
    async fn save_writes_auto_tags_and_multi_kill(pool: PgPool) {
        use crate::{
            jobs,
            social::tests::{ready_clip, user},
        };
        let sam = user(&pool, "sam").await;
        let clip = ready_clip(&pool, sam, "Ace", "Mirage").await;
        assert!(!pending(&pool, clip).await.unwrap());
        jobs::enqueue(&pool, ANALYSE_JOB, json!({ "clipId": clip }))
            .await
            .unwrap();
        assert!(pending(&pool, clip).await.unwrap());

        let stats = json!({"multi_kill": "4k", "modifiers": {"headshot": 2}});
        save(&pool, clip, "v1", &json!({"kills": []}), &stats)
            .await
            .unwrap();
        assert_eq!(multi_kills(&pool, &[clip]).await.unwrap()[&clip], "4k");
        let tags = &social::tags_for(&pool, &[clip]).await.unwrap()[&clip];
        assert_eq!(tags.auto, ["4k", "hs"]);

        // A re-run replaces them.
        save(
            &pool,
            clip,
            "v2",
            &json!({"kills": []}),
            &json!({"modifiers": {}}),
        )
        .await
        .unwrap();
        assert!(multi_kills(&pool, &[clip]).await.unwrap().is_empty());
        assert!(
            !social::tags_for(&pool, &[clip])
                .await
                .unwrap()
                .contains_key(&clip)
        );
    }

    /// The backfill migration gives clips analysed before auto tags existed the same tags
    /// `save` would.
    #[sqlx::test(migrator = "crate::db::MIGRATOR")]
    async fn backfill_matches_auto_tags(pool: PgPool) {
        use crate::social::tests::{ready_clip, user};
        let sam = user(&pool, "sam").await;
        let clip = ready_clip(&pool, sam, "Ace", "Mirage").await;
        let stats = json!({"multi_kill": "ace", "modifiers":
            {"headshot": 3, "wallbang": 0, "through_smoke": 1, "noscope": 1, "blind": 1, "in_air": 1}});
        sqlx::query(
            "INSERT INTO analysis_results (clip_id, analyser_version, raw, stats)
             VALUES ($1, 'v1', '{}', $2)",
        )
        .bind(clip)
        .bind(&stats)
        .execute(&pool)
        .await
        .unwrap();
        sqlx::raw_sql(include_str!("../../../migrations/0007_auto_tags.sql"))
            .execute(&pool)
            .await
            .unwrap();
        let mut backfilled = social::tags_for(&pool, &[clip]).await.unwrap()[&clip]
            .auto
            .clone();
        let mut expected = auto_tags(&stats);
        backfilled.sort();
        expected.sort();
        assert_eq!(backfilled, expected);
    }
}
