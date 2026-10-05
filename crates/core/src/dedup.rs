//! Duplicate uploads (decision 55): a file that's here already, byte for byte, isn't kept
//! twice, whoever uploaded it. Every clip's original and playback file has a fingerprint:
//!
//! - `sample_hash`: SHA-256 of three 1 MiB samples (the start, the middle and the end), or
//!   of the whole file when it's no bigger than that. Reading 3 MiB takes milliseconds
//!   whatever the file's size, and two copies of a file always agree on it, so with the
//!   size it finds every copy. Different files can share it too, so it only says "maybe".
//! - `content_hash`: SHA-256 of the SHA-256 of each 8 MiB block, in order. It covers every
//!   byte, so it decides. Blocks rather than one pass so the browser can compute it with
//!   Web Crypto, which hashes a buffer at a time, natively and in parallel.
//!
//! The browser asks before uploading (`check`), with the samples, and only hashes the whole
//! file when they match something. The worker fingerprints what was actually uploaded and
//! refuses a copy the browser couldn't see (`claim_original`): one uploaded at the same
//! time, one whose original wasn't fingerprinted yet, or a client that didn't ask.
//!
//! "Here" is a clip that's processing or ready and not in the trash. A clip in the trash is
//! offered back to its uploader for as long as it can be restored (`Found::InTrash`); to
//! anyone else, and once it's been in the trash for longer, the file is new. Restoring it
//! after the file came back as another clip is refused (`clips::restore`).

use std::path::Path;

use anyhow::{Context, bail};
use sha2::{Digest, Sha256};
use uuid::Uuid;

use crate::{
    clips::TRASH_DAYS,
    storage::{BlobChanged, Container, Storage},
};

/// Bytes in each of the three samples of `sample_hash`.
pub const SAMPLE_BYTES: u64 = 1 << 20;
/// Bytes in each block of `content_hash`.
pub const BLOCK_BYTES: u64 = 8 << 20;

/// Serializes every decision on whether a file is here already (`claim_original`,
/// `clips::restore`), so two of them can't both find the other's file missing.
const LOCK: i64 = 0x636c_6970_6465_6475; // "clipdedu"

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum File {
    Original,
    Playback,
}

impl File {
    fn as_str(self) -> &'static str {
        match self {
            Self::Original => "original",
            Self::Playback => "playback",
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Fingerprint {
    pub bytes: i64,
    pub sample_hash: [u8; 32],
    pub content_hash: [u8; 32],
}

/// Fingerprints a file of `size` bytes fed to it in order, in pieces of any size.
pub struct Fingerprinter {
    size: u64,
    seen: u64,
    /// The sampled ranges, in order and apart.
    samples: Vec<(u64, u64)>,
    sample: Sha256,
    block: Sha256,
    block_seen: u64,
    blocks: Sha256,
}

impl Fingerprinter {
    pub fn new(size: u64) -> Self {
        let samples = if size <= 3 * SAMPLE_BYTES {
            vec![(0, size)]
        } else {
            let middle = (size - SAMPLE_BYTES) / 2;
            vec![
                (0, SAMPLE_BYTES),
                (middle, middle + SAMPLE_BYTES),
                (size - SAMPLE_BYTES, size),
            ]
        };
        Self {
            size,
            seen: 0,
            samples,
            sample: Sha256::new(),
            block: Sha256::new(),
            block_seen: 0,
            blocks: Sha256::new(),
        }
    }

    pub fn update(&mut self, data: &[u8]) {
        let (start, end) = (self.seen, self.seen + data.len() as u64);
        for &(from, to) in &self.samples {
            let (from, to) = (from.max(start), to.min(end));
            if from < to {
                self.sample
                    .update(&data[(from - start) as usize..(to - start) as usize]);
            }
        }
        self.seen = end;
        let mut rest = data;
        while !rest.is_empty() {
            let take = rest.len().min((BLOCK_BYTES - self.block_seen) as usize);
            self.block.update(&rest[..take]);
            self.block_seen += take as u64;
            rest = &rest[take..];
            if self.block_seen == BLOCK_BYTES {
                self.end_block();
            }
        }
    }

    fn end_block(&mut self) {
        let block = std::mem::replace(&mut self.block, Sha256::new()).finalize();
        self.blocks.update(block);
        self.block_seen = 0;
    }

    /// The fingerprint, or an error if it wasn't fed `size` bytes.
    pub fn finish(mut self) -> anyhow::Result<Fingerprint> {
        if self.seen != self.size {
            bail!("expected {} bytes, read {}", self.size, self.seen);
        }
        if self.block_seen > 0 {
            self.end_block();
        }
        Ok(Fingerprint {
            bytes: self.size as i64,
            sample_hash: self.sample.finalize().into(),
            content_hash: self.blocks.finalize().into(),
        })
    }
}

/// Fingerprints a local file. Hashing 2 GiB takes a few seconds of CPU, so it runs off the
/// async threads (the job's lock is refreshed on them meanwhile).
pub async fn of_file(path: &Path) -> anyhow::Result<Fingerprint> {
    let path = path.to_owned();
    tokio::task::spawn_blocking(move || {
        use std::io::Read;
        let mut file =
            std::fs::File::open(&path).with_context(|| format!("opening {}", path.display()))?;
        let mut fp = Fingerprinter::new(file.metadata()?.len());
        let mut buf = vec![0; 1 << 20];
        loop {
            let n = file
                .read(&mut buf)
                .with_context(|| format!("reading {}", path.display()))?;
            if n == 0 {
                break;
            }
            fp.update(&buf[..n]);
        }
        fp.finish()
    })
    .await?
}

/// Fingerprints a blob, streamed (nothing written to disk). With `if_match`, fails with
/// `BlobChanged` if the blob's ETag is no longer that one.
pub async fn of_blob(
    storage: &Storage,
    container: Container,
    blob: &str,
    if_match: Option<&str>,
) -> anyhow::Result<Fingerprint> {
    let mut res = storage
        .fetch(container, blob, false, None, if_match)
        .await?;
    if res.status() == reqwest::StatusCode::PRECONDITION_FAILED {
        return Err(BlobChanged.into());
    }
    if !res.status().is_success() {
        bail!("GET {}/{blob}: {}", container.as_str(), res.status());
    }
    let size = res
        .content_length()
        .context("GET blob: no Content-Length")?;
    let mut fp = Fingerprinter::new(size);
    while let Some(chunk) = res
        .chunk()
        .await
        .map_err(reqwest::Error::without_url)
        .context("reading blob body")?
    {
        fp.update(&chunk);
    }
    fp.finish()
}

/// What `check` found for a file about to be uploaded.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Found {
    /// Nothing like it: upload it.
    Nothing,
    /// A file of the same size with the same samples: ask again with the content hash.
    Maybe,
    /// This clip has the same file.
    Duplicate(Uuid),
    /// One of the viewer's clips in the trash, still restorable, has the same file.
    InTrash(Uuid),
}

/// The fingerprints `f` of clips `c` that hold their file: processing or ready, and not in
/// the trash.
macro_rules! held {
    () => {
        "c.deleted_at IS NULL AND c.status IN ('processing', 'ready')"
    };
}

/// Whether a file `viewer` is about to upload is here already: by its size and samples,
/// and, once those match something, by its `content` hash too.
pub async fn check(
    pool: &sqlx::PgPool,
    viewer: Uuid,
    bytes: i64,
    sample: &[u8; 32],
    content: Option<&[u8; 32]>,
) -> sqlx::Result<Found> {
    let found: Option<(Uuid, bool)> = sqlx::query_as(concat!(
        "SELECT c.id, c.deleted_at IS NULL
           FROM clip_fingerprints f JOIN clips c ON c.id = f.clip_id
          WHERE f.bytes = $1 AND f.sample_hash = $2
            AND ($3::bytea IS NULL OR f.content_hash = $3)
            AND (",
        held!(),
        "       OR (c.owner_id = $4 AND c.status IN ('processing', 'ready')
                    AND c.deleted_at >= now() - make_interval(days => $5)))
          ORDER BY c.deleted_at IS NOT NULL, f.file = 'playback', c.created_at
          LIMIT 1"
    ))
    .bind(bytes)
    .bind(&sample[..])
    .bind(content.map(|c| &c[..]))
    .bind(viewer)
    .bind(TRASH_DAYS as i32)
    .fetch_optional(pool)
    .await?;
    Ok(match (found, content) {
        (None, _) => Found::Nothing,
        (Some(_), None) => Found::Maybe,
        (Some((id, true)), Some(_)) => Found::Duplicate(id),
        (Some((id, false)), Some(_)) => Found::InTrash(id),
    })
}

/// Takes the lock every decision on duplicates holds, until `tx` ends.
pub(crate) async fn lock(tx: &mut sqlx::PgTransaction<'_>) -> sqlx::Result<()> {
    sqlx::query("SELECT pg_advisory_xact_lock($1)")
        .bind(LOCK)
        .execute(&mut **tx)
        .await?;
    Ok(())
}

/// Saves a clip's fingerprint of `file`, replacing the one it had.
pub async fn record<'e>(
    db: impl sqlx::PgExecutor<'e>,
    clip: Uuid,
    file: File,
    fp: &Fingerprint,
) -> sqlx::Result<()> {
    sqlx::query(
        "INSERT INTO clip_fingerprints (clip_id, file, bytes, sample_hash, content_hash)
         VALUES ($1, $2, $3, $4, $5)
         ON CONFLICT (clip_id, file) DO UPDATE
            SET bytes = EXCLUDED.bytes, sample_hash = EXCLUDED.sample_hash,
                content_hash = EXCLUDED.content_hash",
    )
    .bind(clip)
    .bind(file.as_str())
    .bind(fp.bytes)
    .bind(&fp.sample_hash[..])
    .bind(&fp.content_hash[..])
    .execute(db)
    .await?;
    Ok(())
}

/// Records the fingerprint of a clip's original, unless another clip holds the same file
/// (as its original or playback file): then returns that one, and keeps it as the clip's
/// `duplicate_of`.
pub async fn claim_original(
    pool: &sqlx::PgPool,
    clip: Uuid,
    fp: &Fingerprint,
) -> sqlx::Result<Option<Uuid>> {
    let mut tx = pool.begin().await?;
    lock(&mut tx).await?;
    let other: Option<Uuid> = sqlx::query_scalar(concat!(
        "SELECT c.id FROM clip_fingerprints f JOIN clips c ON c.id = f.clip_id
          WHERE f.content_hash = $1 AND f.bytes = $2 AND c.id <> $3 AND ",
        held!(),
        " ORDER BY f.file = 'playback', c.created_at
          LIMIT 1"
    ))
    .bind(&fp.content_hash[..])
    .bind(fp.bytes)
    .bind(clip)
    .fetch_optional(&mut *tx)
    .await?;
    if other.is_none() {
        record(&mut *tx, clip, File::Original, fp).await?;
    }
    // Only when it changes: most never are a duplicate, and so don't wait on the row.
    sqlx::query(
        "UPDATE clips SET duplicate_of = $2 WHERE id = $1 AND duplicate_of IS DISTINCT FROM $2",
    )
    .bind(clip)
    .bind(other)
    .execute(&mut *tx)
    .await?;
    tx.commit().await?;
    Ok(other)
}

/// Another clip that holds one of `clip`'s files. Call it holding `lock`.
pub(crate) async fn held_elsewhere(
    tx: &mut sqlx::PgTransaction<'_>,
    clip: Uuid,
) -> sqlx::Result<Option<Uuid>> {
    sqlx::query_scalar(concat!(
        "SELECT c.id
           FROM clip_fingerprints mine
           JOIN clip_fingerprints f
             ON f.content_hash = mine.content_hash AND f.bytes = mine.bytes
            AND f.clip_id <> mine.clip_id
           JOIN clips c ON c.id = f.clip_id
          WHERE mine.clip_id = $1 AND ",
        held!(),
        " ORDER BY c.created_at
          LIMIT 1"
    ))
    .bind(clip)
    .fetch_optional(&mut **tx)
    .await
}

/// Which of a clip's files have a fingerprint.
pub async fn fingerprinted(pool: &sqlx::PgPool, clip: Uuid) -> sqlx::Result<Vec<File>> {
    let files: Vec<String> =
        sqlx::query_scalar("SELECT file FROM clip_fingerprints WHERE clip_id = $1")
            .bind(clip)
            .fetch_all(pool)
            .await?;
    Ok(files
        .iter()
        .map(|f| match f.as_str() {
            "original" => File::Original,
            _ => File::Playback,
        })
        .collect())
}

/// Records the fingerprint of a clip's playback file `blob`, if that's still the one it
/// plays (a re-encode may have replaced it meanwhile, and recorded its own).
pub async fn record_playback(
    pool: &sqlx::PgPool,
    clip: Uuid,
    blob: &str,
    fp: &Fingerprint,
) -> sqlx::Result<bool> {
    let mut tx = pool.begin().await?;
    let current: Option<Option<String>> =
        sqlx::query_scalar("SELECT playback_blob FROM clips WHERE id = $1 FOR UPDATE")
            .bind(clip)
            .fetch_optional(&mut *tx)
            .await?;
    if current.flatten().as_deref() != Some(blob) {
        return Ok(false);
    }
    record(&mut *tx, clip, File::Playback, fp).await?;
    tx.commit().await?;
    Ok(true)
}

/// Clips that hold the same file, from before duplicate uploads were refused (or two the
/// worker let through): each group's clips share a file, oldest first, and the groups come
/// newest first. For an admin to pick which copies go.
pub async fn copies(pool: &sqlx::PgPool) -> sqlx::Result<Vec<Vec<Uuid>>> {
    let shared: Vec<Vec<Uuid>> = sqlx::query_scalar(concat!(
        "SELECT array_agg(DISTINCT f.clip_id)
           FROM clip_fingerprints f JOIN clips c ON c.id = f.clip_id
          WHERE ",
        held!(),
        " GROUP BY f.content_hash
         HAVING count(DISTINCT f.clip_id) > 1"
    ))
    .fetch_all(pool)
    .await?;
    // One clip's original can match one clip and its playback file another: those are
    // all one group.
    let mut groups: Vec<Vec<Uuid>> = Vec::new();
    for ids in shared {
        let (mut joined, rest): (Vec<_>, Vec<_>) = groups
            .into_iter()
            .partition(|group| group.iter().any(|id| ids.contains(id)));
        let mut group: Vec<Uuid> = joined.drain(..).flatten().chain(ids).collect();
        group.sort();
        group.dedup();
        groups = rest;
        groups.push(group);
    }
    let all: Vec<Uuid> = groups.iter().flatten().copied().collect();
    let created: std::collections::HashMap<Uuid, chrono::DateTime<chrono::Utc>> =
        sqlx::query_as("SELECT id, created_at FROM clips WHERE id = ANY($1)")
            .bind(&all)
            .fetch_all(pool)
            .await?
            .into_iter()
            .collect();
    let when = |id: &Uuid| (created[id], *id);
    for group in &mut groups {
        group.sort_by_key(when);
    }
    groups.sort_by_key(|group| std::cmp::Reverse(when(&group[0])));
    Ok(groups)
}

/// Clips that are here but have no fingerprint of their original yet: until the
/// `fingerprint` jobs from migration 0016 have run, `copies` may miss theirs.
pub async fn unchecked(pool: &sqlx::PgPool) -> sqlx::Result<i64> {
    sqlx::query_scalar(concat!(
        "SELECT count(*) FROM clips c
          WHERE ",
        held!(),
        " AND NOT EXISTS (SELECT FROM clip_fingerprints f
                             WHERE f.clip_id = c.id AND f.file = 'original')"
    ))
    .fetch_one(pool)
    .await
}

/// Lower-case hex, as the API sends hashes.
pub fn to_hex(hash: &[u8; 32]) -> String {
    hash.iter().map(|b| format!("{b:02x}")).collect()
}

/// A SHA-256 from 64 hex digits (either case), or `None`.
pub fn from_hex(text: &str) -> Option<[u8; 32]> {
    if text.len() != 64 || !text.is_ascii() {
        return None;
    }
    let mut out = [0; 32];
    for (i, byte) in out.iter_mut().enumerate() {
        *byte = u8::from_str_radix(&text[2 * i..2 * i + 2], 16).ok()?;
    }
    Some(out)
}

#[cfg(test)]
mod tests {
    use serde_json::json;

    use super::*;
    use crate::{
        clips::{self, ClipStatus, NewClip, Transcoded},
        users::{self, Identity, SignIn},
    };

    /// The test data both this and the browser's tests (web/src/lib/fingerprint.test.ts)
    /// hash, so the two agree on every fingerprint.
    fn data(len: usize) -> Vec<u8> {
        (0..len).map(|i| ((i * 31 + 7) % 251) as u8).collect()
    }

    /// The same files' fingerprints in web/src/lib/fingerprint.test.ts.
    const VECTORS: [(usize, &str, &str); 4] = [
        (
            1000,
            "008549d94fa71e7a0a483d84380d05a923a4b18e79ba1f8a8ddac923956d32ef",
            "a5f998fc390196f6d0c72a3494f38548e21d312ec821426b73b31f5e192ba8eb",
        ),
        // Three samples' worth: hashed whole.
        (
            3 << 20,
            "9c858aecb2673a768ee641b231391f0be6189325d64cb0a68f0e4e1be9bfa68b",
            "bea0986c2a69fa1f209ecc540a4d716cd1dfcf43cda3302c9c6ba29f23cb7635",
        ),
        // A byte more: sampled.
        (
            (3 << 20) + 1,
            "009a6c4a78500ab777302ec92c4c0a11acc574380e8cb9d30be2213169d1b214",
            "566ab5cdde919eb3f3969c00d940c6363a53b348bb338fa48272051890ebf1d8",
        ),
        // Two whole blocks and part of a third.
        (
            (20 << 20) + 123,
            "13e52089fb34511154b7861b29916b17349c828d59fcd6aa0b4ba2e3213bbee9",
            "225cf47c7227ab4ac8eefbfdf00550125ff03073c4aaa797e25f4dd875cf5a48",
        ),
    ];

    #[test]
    fn fingerprints_match_the_browsers() {
        for (len, sample, content) in VECTORS {
            let data = data(len);
            // Fed whole, and in pieces that straddle the samples and blocks.
            for piece in [len, 1, 4093, 1 << 20] {
                if piece == 1 && len > 10_000 {
                    continue;
                }
                let mut fp = Fingerprinter::new(len as u64);
                for chunk in data.chunks(piece) {
                    fp.update(chunk);
                }
                let fp = fp.finish().unwrap();
                assert_eq!(fp.bytes, len as i64);
                assert_eq!(to_hex(&fp.sample_hash), sample, "{len} by {piece}");
                assert_eq!(to_hex(&fp.content_hash), content, "{len} by {piece}");
            }
        }
    }

    #[test]
    fn a_short_or_long_read_is_an_error() {
        let mut fp = Fingerprinter::new(10);
        fp.update(&[1; 9]);
        assert!(fp.finish().is_err());
        let mut fp = Fingerprinter::new(10);
        fp.update(&[1; 11]);
        assert!(fp.finish().is_err());
    }

    #[tokio::test]
    async fn fingerprints_a_file() {
        let (len, sample, content) = VECTORS[3];
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("clip.mp4");
        std::fs::write(&path, data(len)).unwrap();
        let fp = of_file(&path).await.unwrap();
        assert_eq!(
            (to_hex(&fp.sample_hash), to_hex(&fp.content_hash)),
            (sample.into(), content.into())
        );
        assert!(of_file(&dir.path().join("missing.mp4")).await.is_err());
    }

    #[tokio::test]
    async fn fingerprints_a_blob() {
        let Some(storage) = crate::storage::tests::azurite() else {
            return;
        };
        storage.prepare_local(&[]).await.unwrap();
        let (len, sample, content) = VECTORS[2];
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("clip.mp4");
        std::fs::write(&path, data(len)).unwrap();
        let blob = format!("{}/dedup.mp4", Uuid::new_v4());
        storage
            .upload_file(Container::Originals, &blob, &path, "video/mp4")
            .await
            .unwrap();
        let fp = of_blob(&storage, Container::Originals, &blob, None)
            .await
            .unwrap();
        assert_eq!(
            (to_hex(&fp.sample_hash), to_hex(&fp.content_hash)),
            (sample.into(), content.into())
        );
        let etag = storage
            .blob_props(Container::Originals, &blob)
            .await
            .unwrap()
            .unwrap()
            .etag;
        of_blob(&storage, Container::Originals, &blob, Some(&etag))
            .await
            .unwrap();
        let err = of_blob(&storage, Container::Originals, &blob, Some("\"other\""))
            .await
            .unwrap_err();
        assert!(err.is::<BlobChanged>(), "{err:#}");
        let err = of_blob(&storage, Container::Originals, "missing.mp4", None)
            .await
            .unwrap_err();
        assert!(err.to_string().contains("404"), "{err:#}");
    }

    #[test]
    fn hex_round_trips() {
        let hash: [u8; 32] = std::array::from_fn(|i| (i * 8) as u8);
        let text = to_hex(&hash);
        assert_eq!(&text[..6], "000810");
        assert_eq!(from_hex(&text), Some(hash));
        assert_eq!(from_hex(&text.to_uppercase()), Some(hash));
        for bad in [
            "",
            "00",
            &text[1..],
            &format!("{text}0"),
            &text.replace('0', "g"),
        ] {
            assert_eq!(from_hex(bad), None, "{bad}");
        }
        assert_eq!(from_hex(&format!("é{}", &text[2..])), None);
    }

    async fn user(pool: &sqlx::PgPool, name: &str) -> Uuid {
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

    /// A clip of `owner`'s, moved to `processing` (uploaded).
    async fn processing(pool: &sqlx::PgPool, owner: Uuid) -> Uuid {
        let clip = clips::create(
            pool,
            NewClip {
                owner_id: owner,
                game_id: "cs2".into(),
                title: "Ace".into(),
                description: String::new(),
                map: None,
                my_pov: true,
                filename: "ace.mp4".into(),
                bytes: 1000,
            },
        )
        .await
        .unwrap();
        assert!(clips::mark_uploaded(pool, clip.id).await.unwrap());
        clip.id
    }

    fn fp(n: u8) -> Fingerprint {
        Fingerprint {
            bytes: 1000 + i64::from(n),
            sample_hash: [n; 32],
            content_hash: [n + 100; 32],
        }
    }

    async fn status(pool: &sqlx::PgPool, clip: Uuid) -> ClipStatus {
        sqlx::query_scalar("SELECT status FROM clips WHERE id = $1")
            .bind(clip)
            .fetch_one(pool)
            .await
            .unwrap()
    }

    async fn ready(pool: &sqlx::PgPool, clip: Uuid, playback: Fingerprint) {
        let done = Transcoded {
            playback_blob: clips::playback_blob(clip),
            poster_blob: clips::poster_blob(clip),
            duration_ms: 1000,
            width: 1920,
            height: 1080,
            fps: 60.0,
            metadata: json!({}),
            playback_fingerprint: playback,
        };
        assert!(clips::set_ready(pool, clip, &done).await.unwrap());
        assert_eq!(status(pool, clip).await, ClipStatus::Ready);
    }

    #[sqlx::test(migrator = "crate::db::MIGRATOR")]
    async fn finds_files_that_are_here_already(pool: sqlx::PgPool) {
        let sam = user(&pool, "sam").await;
        let kim = user(&pool, "kim").await;
        let clip = processing(&pool, sam).await;
        let (original, playback) = (fp(1), fp(2));
        assert_eq!(
            check(&pool, kim, original.bytes, &original.sample_hash, None)
                .await
                .unwrap(),
            Found::Nothing,
            "not fingerprinted yet"
        );
        assert_eq!(claim_original(&pool, clip, &original).await.unwrap(), None);
        ready(&pool, clip, playback.clone()).await;
        assert_eq!(
            fingerprinted(&pool, clip).await.unwrap().len(),
            2,
            "original and playback"
        );

        // Anyone's upload of either file is found: maybe by its samples, for sure by its
        // content.
        for f in [&original, &playback] {
            for viewer in [sam, kim] {
                let found = |content| check(&pool, viewer, f.bytes, &f.sample_hash, content);
                assert_eq!(found(None).await.unwrap(), Found::Maybe);
                assert_eq!(
                    found(Some(&f.content_hash)).await.unwrap(),
                    Found::Duplicate(clip)
                );
                // Same size and samples, other content: a different file.
                assert_eq!(found(Some(&[9; 32])).await.unwrap(), Found::Nothing);
            }
        }
        // Another size, or other samples: nothing to look at.
        let other = check(&pool, kim, original.bytes + 1, &original.sample_hash, None);
        assert_eq!(other.await.unwrap(), Found::Nothing);
        let other = check(&pool, kim, original.bytes, &[7; 32], None);
        assert_eq!(other.await.unwrap(), Found::Nothing);

        // In the trash: offered back to its uploader, and new to anyone else.
        assert!(clips::soft_delete(&pool, clip).await.unwrap());
        let found = |viewer| check(&pool, viewer, 1001, &[1; 32], Some(&[101; 32]));
        assert_eq!(found(sam).await.unwrap(), Found::InTrash(clip));
        assert_eq!(found(kim).await.unwrap(), Found::Nothing);
        // Past the week it could be restored in: new to everyone.
        sqlx::query("UPDATE clips SET deleted_at = now() - interval '8 days'")
            .execute(&pool)
            .await
            .unwrap();
        assert_eq!(found(sam).await.unwrap(), Found::Nothing);
    }

    /// Failed clips don't hold their file: uploading it again is how some failures are
    /// fixed.
    #[sqlx::test(migrator = "crate::db::MIGRATOR")]
    async fn failed_clips_dont_count(pool: sqlx::PgPool) {
        let sam = user(&pool, "sam").await;
        let clip = processing(&pool, sam).await;
        assert_eq!(claim_original(&pool, clip, &fp(1)).await.unwrap(), None);
        clips::set_failed(&pool, clip, clips::UNREADABLE_ERROR)
            .await
            .unwrap();
        let found = check(&pool, sam, 1001, &[1; 32], Some(&[101; 32]));
        assert_eq!(found.await.unwrap(), Found::Nothing);
        let again = processing(&pool, sam).await;
        assert_eq!(claim_original(&pool, again, &fp(1)).await.unwrap(), None);
    }

    /// The worker refuses a second copy, of the original or the playback file, and only
    /// while the first is here.
    #[sqlx::test(migrator = "crate::db::MIGRATOR")]
    async fn the_worker_keeps_one_copy(pool: sqlx::PgPool) {
        let sam = user(&pool, "sam").await;
        let kim = user(&pool, "kim").await;
        let first = processing(&pool, sam).await;
        assert_eq!(claim_original(&pool, first, &fp(1)).await.unwrap(), None);
        // Its own transcode running again finds only itself.
        assert_eq!(claim_original(&pool, first, &fp(1)).await.unwrap(), None);

        let second = processing(&pool, kim).await;
        assert_eq!(
            claim_original(&pool, second, &fp(1)).await.unwrap(),
            Some(first)
        );
        let duplicate_of: Option<Uuid> =
            sqlx::query_scalar("SELECT duplicate_of FROM clips WHERE id = $1")
                .bind(second)
                .fetch_one(&pool)
                .await
                .unwrap();
        assert_eq!(duplicate_of, Some(first));
        assert!(fingerprinted(&pool, second).await.unwrap().is_empty());

        // Someone's upload of the first one's playback file.
        ready(&pool, first, fp(2)).await;
        let third = processing(&pool, kim).await;
        assert_eq!(
            claim_original(&pool, third, &fp(2)).await.unwrap(),
            Some(first)
        );

        // Once the first is in the trash, the file is new again.
        assert!(clips::soft_delete(&pool, first).await.unwrap());
        assert_eq!(claim_original(&pool, second, &fp(1)).await.unwrap(), None);
        let duplicate_of: Option<Uuid> =
            sqlx::query_scalar("SELECT duplicate_of FROM clips WHERE id = $1")
                .bind(second)
                .fetch_one(&pool)
                .await
                .unwrap();
        assert_eq!(duplicate_of, None);
    }

    /// A clip in the trash can't come back once its file is here again as another clip.
    #[sqlx::test(migrator = "crate::db::MIGRATOR")]
    async fn restoring_a_file_thats_here_again_is_refused(pool: sqlx::PgPool) {
        let sam = user(&pool, "sam").await;
        let first = processing(&pool, sam).await;
        assert_eq!(claim_original(&pool, first, &fp(1)).await.unwrap(), None);
        ready(&pool, first, fp(2)).await;
        assert!(clips::soft_delete(&pool, first).await.unwrap());

        let again = processing(&pool, sam).await;
        assert_eq!(claim_original(&pool, again, &fp(1)).await.unwrap(), None);
        assert_eq!(
            clips::restore(&pool, first).await.unwrap(),
            clips::Restore::Duplicate(again)
        );
        let deleted: bool =
            sqlx::query_scalar("SELECT deleted_at IS NOT NULL FROM clips WHERE id = $1")
                .bind(first)
                .fetch_one(&pool)
                .await
                .unwrap();
        assert!(deleted, "still in the trash");

        // Gone again, the first can come back.
        assert!(clips::soft_delete(&pool, again).await.unwrap());
        assert_eq!(
            clips::restore(&pool, first).await.unwrap(),
            clips::Restore::Restored
        );
    }

    /// Copies from before are listed for an admin: grouped by the file they share, oldest
    /// first, without the ones in the trash.
    #[sqlx::test(migrator = "crate::db::MIGRATOR")]
    async fn lists_the_copies_already_here(pool: sqlx::PgPool) {
        let sam = user(&pool, "sam").await;
        let kim = user(&pool, "kim").await;
        assert!(copies(&pool).await.unwrap().is_empty());
        let mut made = Vec::new();
        for owner in [sam, kim, sam, kim, sam] {
            made.push(processing(&pool, owner).await);
        }
        let [a, b, c, d, e] = made[..] else {
            unreachable!()
        };
        // Not fingerprinted yet: all of them.
        assert_eq!(unchecked(&pool).await.unwrap(), 5);
        // Recorded as the `fingerprint` job does, without refusing anything.
        for (clip, file) in [(a, 1), (b, 1), (c, 2), (d, 3), (e, 9)] {
            record(&pool, clip, File::Original, &fp(file))
                .await
                .unwrap();
        }
        // D's original is C's playback file: C and D are copies too.
        record(&pool, c, File::Playback, &fp(3)).await.unwrap();
        // And A's playback is C's original, so it's one group of four.
        record(&pool, a, File::Playback, &fp(2)).await.unwrap();
        assert_eq!(unchecked(&pool).await.unwrap(), 0);
        assert_eq!(copies(&pool).await.unwrap(), [vec![a, b, c, d]]);

        // Two groups: the newest first.
        record(&pool, a, File::Playback, &fp(8)).await.unwrap();
        record(&pool, e, File::Original, &fp(3)).await.unwrap();
        sqlx::query("UPDATE clips SET created_at = now() - interval '1 day' WHERE id = $1")
            .bind(c)
            .execute(&pool)
            .await
            .unwrap();
        assert_eq!(copies(&pool).await.unwrap(), [vec![a, b], vec![c, d, e]]);

        // A copy in the trash doesn't count, nor does a failed one.
        assert!(clips::soft_delete(&pool, b).await.unwrap());
        clips::set_failed(&pool, e, clips::UNREADABLE_ERROR)
            .await
            .unwrap();
        assert_eq!(copies(&pool).await.unwrap(), [vec![c, d]]);
    }

    /// A playback fingerprint is only kept for the file the clip plays.
    #[sqlx::test(migrator = "crate::db::MIGRATOR")]
    async fn records_the_current_playback_file_only(pool: sqlx::PgPool) {
        let sam = user(&pool, "sam").await;
        let clip = processing(&pool, sam).await;
        let blob = clips::playback_blob(clip);
        assert!(!record_playback(&pool, clip, &blob, &fp(2)).await.unwrap());
        ready(&pool, clip, fp(2)).await;
        assert!(record_playback(&pool, clip, &blob, &fp(3)).await.unwrap());
        assert!(
            !record_playback(&pool, clip, "old.mp4", &fp(4))
                .await
                .unwrap()
        );
        assert!(
            !record_playback(&pool, Uuid::new_v4(), &blob, &fp(4))
                .await
                .unwrap()
        );
        let found = check(&pool, sam, 1003, &[3; 32], Some(&[103; 32]));
        assert_eq!(found.await.unwrap(), Found::Duplicate(clip));
        let found = check(&pool, sam, 1002, &[2; 32], None);
        assert_eq!(found.await.unwrap(), Found::Nothing, "replaced");
        assert_eq!(fingerprinted(&pool, clip).await.unwrap(), [File::Playback]);
    }
}
