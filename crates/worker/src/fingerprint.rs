//! `fingerprint` job: fingerprints a clip from before duplicate detection (migration 0016,
//! decision 55), its original and its playback file, streamed from Blob Storage. New
//! uploads are fingerprinted by their transcode.

use clipos_core::{
    clips::{self, ClipStatus},
    dedup::{self, File},
    storage::Container,
};
use serde::Deserialize;
use serde_json::Value;
use uuid::Uuid;

use crate::{JobError, Worker};

#[derive(Debug, Deserialize)]
struct Payload {
    #[serde(rename = "clipId")]
    clip_id: Uuid,
}

pub async fn run(worker: &Worker, payload: &Value) -> Result<(), JobError> {
    let Payload { clip_id } = serde_json::from_value(payload.clone())
        .map_err(|e| JobError::Permanent(format!("bad payload: {e}")))?;
    // In the trash too: its uploader is offered it back rather than uploading it again.
    let Some(clip) = clips::get_including_deleted(&worker.pool, clip_id)
        .await
        .map_err(anyhow::Error::from)?
    else {
        return Ok(());
    };
    // A clip still processing gets its fingerprints from its transcode.
    if clip.status != ClipStatus::Ready {
        return Ok(());
    }
    let done = dedup::fingerprinted(&worker.pool, clip_id)
        .await
        .map_err(anyhow::Error::from)?;
    if !done.contains(&File::Original) {
        let fp = dedup::of_blob(
            &worker.storage,
            Container::Originals,
            &clip.original_blob,
            None,
        )
        .await?;
        dedup::record(&worker.pool, clip_id, File::Original, &fp)
            .await
            .map_err(anyhow::Error::from)?;
    }
    if !done.contains(&File::Playback)
        && let Some(blob) = &clip.playback_blob
    {
        let fp = dedup::of_blob(&worker.storage, Container::Playback, blob, None).await?;
        dedup::record_playback(&worker.pool, clip_id, blob, &fp)
            .await
            .map_err(anyhow::Error::from)?;
    }
    tracing::info!(%clip_id, "fingerprinted");
    Ok(())
}
