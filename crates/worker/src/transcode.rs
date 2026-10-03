//! `transcode` job: original (any of mp4/mkv/mov, HEVC/H.264, any frame rate) → H.264
//! MP4 that every browser plays, at most 1080p and 60 fps, source aspect ratio kept
//! (4:3 stays 4:3), plus a poster frame.
//!
//! ffmpeg reads the original straight from Blob Storage through a read SAS (no download
//! step), writes the output to a temp dir, and we upload it from there.
//!
//! An upload is whatever a member sent, so every ffmpeg and ffprobe run on it reads only
//! the formats and codecs we accept (`allowlists`), writes no more than the longest clip
//! we keep (`CUT_AT_S`) and is stopped if it runs too long.

use std::{
    path::{Path, PathBuf},
    process::{Output, Stdio},
    time::{Duration, Instant},
};

use anyhow::{Context, anyhow, bail};
use clipos_core::{
    clips::{self, Clip, ClipStatus, Transcoded},
    jobs,
    storage::{Access, BlobChanged, Container, Overrides},
};
use serde::Deserialize;
use serde_json::{Value, json};
use tokio::process::Command;
use uuid::Uuid;

use crate::{JobError, Worker};

/// Longest clip we accept (plus a little slack for container rounding).
const MAX_DURATION_S: f64 = 5.0 * 60.0 + 2.0;
/// ffmpeg writes at most this much of an upload: a little more than the longest clip we
/// accept, so one that's too long still comes out too long (and is refused), but one whose
/// header says it's shorter than it is can't make the worker encode it all.
pub(crate) const CUT_AT_S: f64 = MAX_DURATION_S + 5.0;
/// How long one ffmpeg run may take before it's stopped: 50 minutes. 30 s of 1080p60 took
/// 70 s to encode on P1v3 (0.43× real time), so the longest clip needs about 12 there; B2
/// is slower, but only a stuck ffmpeg (a stalled read, a pathological file) gets near this.
const FFMPEG_TIME_LIMIT: Duration = Duration::from_secs(10 * MAX_DURATION_S as u64);
/// How long one ffprobe run may take: 10 minutes. It reads headers, or every packet
/// without decoding them: seconds from disk, a minute or two for 2 GiB over HTTP.
const FFPROBE_TIME_LIMIT: Duration = Duration::from_secs(2 * MAX_DURATION_S as u64);
const MAX_HEIGHT: i64 = 1080;
const MAX_FPS: f64 = 60.0;

/// Containers an upload may be in: MP4/MOV, MKV/WebM, and raw H.264 or HEVC (what some
/// recorders write to a pipe). ffmpeg reads hundreds of formats, whatever the file is
/// called; a file in any other one is refused once ffmpeg has seen the bytes that say what
/// it is, before it reads it as that.
const FORMATS: &str = "mov,mp4,m4a,3gp,3g2,mj2,matroska,webm,h264,hevc";
/// Decoders ffmpeg may use on an upload: the video codecs recorders write (AV1 decodes
/// with dav1d or libaom), cover art, their audio, and text subtitles. ffprobe opens one for
/// every stream in the file, so a stream in anything else gets the file refused.
const DECODERS: &str = "h264,hevc,av1,libdav1d,libaom-av1,vp8,vp9,prores,mjpeg,png,\
    aac,mp3float,mp3,opus,vorbis,flac,alac,ac3,eac3,\
    pcm_s16le,pcm_s16be,pcm_s24le,pcm_s24be,pcm_s32le,pcm_f32le,\
    mov_text,subrip,srt,ass,ssa,webvtt";

/// ffmpeg's and ffprobe's input options for reading an upload, or a file made from one, at
/// `input`: only the formats and decoders above, and only the protocol `input` itself
/// needs, so the file can't send ffmpeg anywhere else (a playlist's segments, another file
/// on disk, a URL).
pub(crate) fn allowlists(input: &str) -> Vec<String> {
    let protocols = if input.starts_with("https://") {
        "https,tls,tcp"
    } else if input.starts_with("http://") {
        // Azurite.
        "http,tcp"
    } else {
        "file"
    };
    [
        "-protocol_whitelist",
        protocols,
        "-format_whitelist",
        FORMATS,
        "-codec_whitelist",
        DECODERS,
    ]
    .map(String::from)
    .into()
}

#[derive(Debug, Clone)]
pub struct TranscodeConfig {
    /// Scratch space for the output before upload.
    pub temp_dir: PathBuf,
    /// ffmpeg `-threads`: low, so a transcode never starves the api on the shared plan.
    pub threads: u32,
    /// x264 `-preset`.
    pub preset: String,
    pub crf: u32,
}

#[derive(Debug, Deserialize)]
struct Payload {
    #[serde(rename = "clipId")]
    clip_id: Uuid,
}

pub async fn run(worker: &Worker, payload: &Value) -> Result<(), JobError> {
    let Payload { clip_id } = serde_json::from_value(payload.clone())
        .map_err(|e| JobError::Permanent(format!("bad payload: {e}")))?;
    let Some(clip) = clips::get(&worker.pool, clip_id)
        .await
        .map_err(anyhow::Error::from)?
    else {
        tracing::info!(%clip_id, "clip is gone; nothing to do");
        return Ok(());
    };
    if clip.status != ClipStatus::Processing {
        tracing::info!(%clip_id, status = ?clip.status, "clip isn't processing; skipping");
        return Ok(());
    }

    let dir = worker.transcode.temp_dir.join(clip_id.to_string());
    tokio::fs::create_dir_all(&dir)
        .await
        .with_context(|| format!("creating {}", dir.display()))?;
    let result = transcode(worker, &clip, &dir, None).await;
    // Always clean up the scratch files, whatever happened.
    if let Err(e) = tokio::fs::remove_dir_all(&dir).await {
        tracing::warn!(error = %e, dir = %dir.display(), "couldn't remove temp dir");
    }
    let transcoded = result?;
    clips::set_ready(&worker.pool, clip_id, &transcoded)
        .await
        .map_err(anyhow::Error::from)?;
    // The killfeed is only the uploader's own when they recorded it from their point of view.
    if worker.analyse.is_some() && clip.game_id == "cs2" && clip.my_pov {
        // The clip is ready either way; a lost analysis can be queued again later.
        if let Err(e) = jobs::enqueue(
            &worker.pool,
            clips::ANALYSE_JOB,
            json!({ "clipId": clip_id }),
        )
        .await
        {
            tracing::warn!(%clip_id, error = %e, "couldn't queue killfeed analysis");
        }
    }
    Ok(())
}

/// The `keyframes` job: a ready clip whose playback file has keyframes too far apart for
/// quick seeks in the show is re-encoded from its original (decision 35). Clips that are
/// fine are left alone.
pub async fn rekey(worker: &Worker, payload: &Value) -> Result<(), JobError> {
    let Payload { clip_id } = serde_json::from_value(payload.clone())
        .map_err(|e| JobError::Permanent(format!("bad payload: {e}")))?;
    let Some(clip) = clips::get(&worker.pool, clip_id)
        .await
        .map_err(anyhow::Error::from)?
    else {
        return Ok(());
    };
    let (ClipStatus::Ready, Some(playback)) = (clip.status, clip.playback_blob.as_deref()) else {
        return Ok(());
    };
    let url = worker
        .storage
        .sas_url(
            Container::Playback,
            playback,
            Access::Read,
            Duration::from_secs(3600),
            &Overrides::default(),
        )
        .await?;
    // A probe that fails (network, SAS) is retried rather than taken as a reason to
    // re-encode.
    let gap = max_keyframe_gap(url.as_str()).await?;
    if gap.is_some_and(|g| g <= MAX_KEYFRAME_GAP_S) {
        tracing::info!(%clip_id, ?gap, "keyframes are fine");
        return Ok(());
    }
    tracing::info!(%clip_id, ?gap, "keyframes too far apart; re-encoding");
    let dir = worker.transcode.temp_dir.join(format!("{clip_id}-rekey"));
    tokio::fs::create_dir_all(&dir)
        .await
        .with_context(|| format!("creating {}", dir.display()))?;
    // New blob names: players may be reading the current files right now.
    let rev = clips::new_rev();
    let result = transcode(worker, &clip, &dir, Some(&rev)).await;
    if let Err(e) = tokio::fs::remove_dir_all(&dir).await {
        tracing::warn!(error = %e, dir = %dir.display(), "couldn't remove temp dir");
    }
    let new = vec![
        (Container::Playback, clips::playback_blob_rev(clip_id, &rev)),
        (Container::Posters, clips::poster_blob_rev(clip_id, &rev)),
    ];
    // Until the row points at the new files, the old ones stay in use, so whatever
    // failed, the clip still plays as before. Files no longer used are deleted by a
    // `delete_blobs` job, retried until they're gone.
    let (unused, delay, result) = match result {
        Ok(transcoded) => match clips::set_ready(&worker.pool, clip_id, &transcoded).await {
            // Players still on links to the old files play them to the end.
            Ok(true) => {
                let mut old = vec![(Container::Playback, playback.to_owned())];
                old.extend(clip.poster_blob.map(|p| (Container::Posters, p)));
                (old, REPLACED_FILES_KEPT, Ok(()))
            }
            // Purged while we worked.
            Ok(false) => (new, Duration::ZERO, Ok(())),
            // Maybe written after all, so the new files stay.
            Err(e) => (vec![], Duration::ZERO, Err(anyhow::Error::from(e).into())),
        },
        // Some of the new files may be uploaded already.
        Err(e) => (new, Duration::ZERO, Err(e)),
    };
    if !unused.is_empty()
        && let Err(e) = worker.queue_delete_blobs(&unused, delay).await
    {
        tracing::warn!(%clip_id, error = %e, ?unused, "couldn't queue deleting unused blobs");
    }
    result
}

/// How long the files a re-encode replaced are kept: until every link to them handed out
/// before the switch has expired, plus a margin.
const REPLACED_FILES_KEPT: Duration = Duration::from_secs(clips::VIEW_TTL.as_secs() + 15 * 60);

/// Transcodes (or remuxes) the original and uploads the playback file, poster and teaser.
/// `rev` gives the playback file and poster new names (`clips::new_rev`), for a clip
/// that's already ready; `None` for the first transcode.
async fn transcode(
    worker: &Worker,
    clip: &Clip,
    dir: &Path,
    rev: Option<&str>,
) -> Result<Transcoded, JobError> {
    let source = local_or_remote(worker, clip, dir).await?;
    transcode_from(worker, clip, dir, source, rev).await
}

/// `transcode`, reading the original from `source`.
pub(crate) async fn transcode_from(
    worker: &Worker,
    clip: &Clip,
    dir: &Path,
    Source {
        input: source,
        remote,
    }: Source,
    rev: Option<&str>,
) -> Result<Transcoded, JobError> {
    let clip_id = clip.id;

    // A photo renamed .mp4 is refused by its format here.
    let probe = ffprobe(&source).await?;
    let video = probe
        .video()
        .ok_or_else(|| JobError::Permanent(clips::NO_VIDEO_ERROR.into()))?;
    let stream = video.index.to_string();
    // Raw H.264 and MKVs written to a pipe don't say how long they are; their packets do.
    let mut duration = probe.duration().or_else(|| video.duration());
    let mut packets = None;
    if duration.is_none() {
        let scanned = scan_packets(&source, &stream).await?;
        duration = scanned.duration;
        packets = Some(scanned);
    }
    if let Some(duration) = duration
        && duration > MAX_DURATION_S
    {
        return Err(too_long(duration));
    }
    let source_fps = video.fps();
    tracing::info!(
        codec = video.codec_name.as_deref().unwrap_or("?"),
        stream = video.index,
        width = video.width,
        height = video.height,
        fps = source_fps,
        duration_s = duration,
        bytes = probe.format.size.as_deref().unwrap_or("?"),
        "probed original"
    );

    let output = dir.join("playback.mp4");
    let started = Instant::now();
    let has_audio = probe.audio().is_some();
    let transcode_args = || {
        ffmpeg_args(
            source.as_str(),
            &output,
            &worker.transcode,
            video,
            has_audio,
        )
    };
    let mut mode = if can_remux(&probe, duration.unwrap_or(0.0)) {
        // A copy keeps the original's keyframes, so they must be close enough to seek to.
        let gap = match packets {
            Some(packets) => Ok(packets.keyframe_gap),
            None => scan_packets(&source, &stream)
                .await
                .map(|packets| packets.keyframe_gap),
        };
        match gap {
            Ok(Some(gap)) if gap <= MAX_KEYFRAME_GAP_S => Mode::Remux,
            Ok(gap) => {
                tracing::info!(?gap, "keyframes too far apart to copy; transcoding");
                Mode::Transcode
            }
            // A new upload loses nothing by being transcoded; no need to hold it up with
            // retries.
            Err(e) => {
                tracing::warn!(error = %format!("{e:#}"), "couldn't read keyframes; transcoding");
                Mode::Transcode
            }
        }
    } else {
        Mode::Transcode
    };
    if mode == Mode::Remux {
        // Already browser-ready: just rewrite the container (seconds, not minutes). If
        // the copy fails for any reason, fall back to a full transcode.
        if let Err(e) = run_ffmpeg(&remux_args(&source, &output, video.index, has_audio)).await {
            tracing::warn!(error = %format!("{e:#}"), "remux failed; transcoding instead");
            mode = Mode::Transcode;
            run_ffmpeg(&transcode_args()).await?;
        }
    } else {
        run_ffmpeg(&transcode_args()).await?;
    }
    // Read over HTTP, the original could have been overwritten while ffmpeg read it.
    if remote {
        check_original(worker, clip).await?;
    }
    let wall = started.elapsed();
    let output_bytes = tokio::fs::metadata(&output)
        .await
        .map(|m| m.len())
        .unwrap_or(0);

    // Dimensions, frame rate and duration of what we actually produced (odd sizes,
    // anamorphic pixels and fps capping are all applied by now; a transcode applies the
    // rotation too, a remux only flags it).
    let out = ffprobe(output.to_str().expect("utf-8 temp path")).await?;
    let out_video = out
        .video()
        .ok_or_else(|| anyhow!("transcode produced no video stream"))?;
    let out_duration = out.duration().or(duration).unwrap_or(0.0);
    // The length that counts is what came out, not what the source said: its header can
    // say 10 s of a file that runs for 400, or nothing at all.
    if out_duration > MAX_DURATION_S {
        return Err(if out_duration < CUT_AT_S - 1.0 {
            too_long(out_duration)
        } else {
            // ffmpeg stopped at `CUT_AT_S`; how much longer it runs, only reading all of
            // it would tell.
            JobError::Permanent(format!(
                "{} (this one is longer than it says it is)",
                clips::TOO_LONG_ERROR
            ))
        });
    }

    let poster = dir.join("poster.jpg");
    make_poster(&output, &poster, out_video).await?;
    // What others see of the clip while it's saved for the show (S3): small and blurred
    // beyond reading, so it teases without spoiling.
    let teaser = dir.join("teaser.jpg");
    run_ffmpeg(&teaser_args(&poster, &teaser)).await?;

    let speed = if wall.as_secs_f64() > 0.0 {
        out_duration / wall.as_secs_f64()
    } else {
        0.0
    };
    tracing::info!(
        mode = mode.as_str(),
        wall_ms = wall.as_millis() as u64,
        speed = format!("{speed:.2}x"),
        output_bytes,
        threads = worker.transcode.threads,
        preset = %worker.transcode.preset,
        "transcoded"
    );

    let (playback_blob, poster_blob) = match rev {
        Some(rev) => (
            clips::playback_blob_rev(clip_id, rev),
            clips::poster_blob_rev(clip_id, rev),
        ),
        None => (clips::playback_blob(clip_id), clips::poster_blob(clip_id)),
    };
    let upload_started = Instant::now();
    worker
        .storage
        .upload_file(Container::Playback, &playback_blob, &output, "video/mp4")
        .await?;
    worker
        .storage
        .upload_file(Container::Posters, &poster_blob, &poster, "image/jpeg")
        .await?;
    // The teaser keeps its name even on a re-encode: a small image always fetched whole,
    // so replacing it in place is safe.
    worker
        .storage
        .upload_file(
            Container::Posters,
            &clips::teaser_blob(clip_id),
            &teaser,
            "image/jpeg",
        )
        .await?;
    tracing::info!(
        upload_ms = upload_started.elapsed().as_millis() as u64,
        "uploaded"
    );

    // As players show it: a remuxed phone clip is stored landscape and flagged portrait.
    let (width, height) = out_video.upright_size();
    Ok(Transcoded {
        playback_blob,
        poster_blob,
        duration_ms: (out_duration * 1000.0).round() as i32,
        width: width as i32,
        height: height as i32,
        fps: out_video.fps() as f32,
        metadata: json!({
            "source": {
                "format": probe.format.format_name,
                "bytes": probe.format.size.as_deref().and_then(|s| s.parse::<u64>().ok()),
                "bitRate": probe.format.bit_rate.as_deref().and_then(|s| s.parse::<u64>().ok()),
                "video": {
                    "codec": video.codec_name,
                    "width": video.width,
                    "height": video.height,
                    "fps": source_fps,
                    "pixFmt": video.pix_fmt,
                },
                "audioCodec": probe.audio().and_then(|a| a.codec_name.clone()),
            },
            "transcode": {
                "mode": mode.as_str(),
                "wallMs": wall.as_millis() as u64,
                "speed": speed,
                "outputBytes": output_bytes,
                "threads": worker.transcode.threads,
                "preset": worker.transcode.preset,
                "crf": worker.transcode.crf,
            },
        }),
    })
}

/// Where ffmpeg reads the original from.
pub(crate) struct Source {
    /// A local path or a read SAS URL.
    pub input: String,
    /// Read over HTTP, so it can still change while ffmpeg reads it (see `check_original`).
    pub remote: bool,
}

/// Where ffmpeg reads the original: a local copy when the temp disk has room (fast:
/// MP4s with their index at the end make ffmpeg seek, and over HTTP every seek reopens
/// the connection, which once made a 345 MB remux take 7 minutes), otherwise a read SAS.
///
/// Either way it must still be the file `complete` accepted: the upload link stays
/// writable for a while after that, so it could have been overwritten since.
pub(crate) async fn local_or_remote(
    worker: &Worker,
    clip: &Clip,
    dir: &Path,
) -> Result<Source, JobError> {
    // Room for the original, an output about as big, and some slack.
    let needed = (clip.original_bytes.max(0) as u64) * 2 + (512 << 20);
    let free = free_bytes(dir).await;
    if free.is_some_and(|free| free >= needed) {
        return local_copy(worker, clip, dir).await;
    }
    tracing::warn!(
        free_bytes = free,
        needed_bytes = needed,
        "not enough temp disk for a local copy; reading the original over HTTP"
    );
    remote_original(worker, clip).await
}

/// Downloads the original into `dir`, if it's still the file `complete` accepted.
pub(crate) async fn local_copy(
    worker: &Worker,
    clip: &Clip,
    dir: &Path,
) -> Result<Source, JobError> {
    let ext = clip
        .original_blob
        .rsplit_once('.')
        .map_or("bin", |(_, e)| e);
    let path = dir.join(format!("original.{ext}"));
    let started = Instant::now();
    let downloaded = worker
        .storage
        .download_to_file(
            Container::Originals,
            &clip.original_blob,
            &path,
            clip.original_etag.as_deref(),
        )
        .await;
    let bytes = match downloaded {
        Err(e) if e.is::<BlobChanged>() => return Err(changed()),
        result => result?,
    };
    // Clips completed before their ETag was kept can only be checked by size.
    if bytes != clip.original_bytes as u64 {
        return Err(changed());
    }
    let secs = started.elapsed().as_secs_f64().max(0.001);
    tracing::info!(
        bytes,
        download_ms = (secs * 1000.0) as u64,
        mb_per_s = format!("{:.1}", bytes as f64 / secs / 1e6),
        "downloaded original"
    );
    Ok(Source {
        input: path.to_string_lossy().into_owned(),
        remote: false,
    })
}

/// A read SAS for the original, if it's still the file `complete` accepted.
pub(crate) async fn remote_original(worker: &Worker, clip: &Clip) -> Result<Source, JobError> {
    // ffmpeg can't send `If-Match`, so compare before it starts (and the caller once more
    // after it's done).
    check_original(worker, clip).await?;
    let url = worker
        .storage
        .sas_url(
            Container::Originals,
            &clip.original_blob,
            Access::Read,
            Duration::from_secs(6 * 3600),
            &Overrides::default(),
        )
        .await?;
    Ok(Source {
        input: url.to_string(),
        remote: true,
    })
}

/// Fails for good unless the original is still the file `complete` accepted: the same
/// size and, when it was kept (not before migration 0014), the same ETag.
pub(crate) async fn check_original(worker: &Worker, clip: &Clip) -> Result<(), JobError> {
    let props = worker
        .storage
        .blob_props(Container::Originals, &clip.original_blob)
        .await?
        .ok_or_else(|| anyhow!("the original is missing"))?;
    let same_etag = clip
        .original_etag
        .as_ref()
        .is_none_or(|etag| *etag == props.etag);
    if !same_etag || props.size != clip.original_bytes as u64 {
        return Err(changed());
    }
    Ok(())
}

fn changed() -> JobError {
    JobError::Permanent(clips::CHANGED_ERROR.into())
}

/// Free bytes on the filesystem holding `dir`, from `df` (no unsafe statvfs).
pub async fn free_bytes(dir: &Path) -> Option<u64> {
    let out = Command::new("df").arg("-Pk").arg(dir).output().await.ok()?;
    let text = String::from_utf8_lossy(&out.stdout);
    let available_kb: u64 = text
        .lines()
        .nth(1)?
        .split_whitespace()
        .nth(3)?
        .parse()
        .ok()?;
    Some(available_kb * 1024)
}

/// Keyframes further apart than this make seeks slow; such files are re-encoded with one
/// every 2 s (decision 35). x264's default (a keyframe every 250 frames, 4.17 s at 60 fps)
/// lands just above it, so nearly every clip transcoded before S5 was re-encoded once.
pub(crate) const MAX_KEYFRAME_GAP_S: f64 = 4.0;

/// The longest stretch without a keyframe in a playback file (its only video stream). See
/// `scan_packets`.
pub(crate) async fn max_keyframe_gap(input: &str) -> anyhow::Result<Option<f64>> {
    Ok(scan_packets(input, "v:0").await?.keyframe_gap)
}

/// What a video stream's packets say about it.
#[derive(Debug, Default, PartialEq)]
pub(crate) struct Packets {
    /// The longest stretch without a keyframe. `None` if there's nothing to measure (no
    /// packets, no keyframes or no timestamps) or ffprobe can't read the data:
    /// re-encoding fixes all of those.
    pub(crate) keyframe_gap: Option<f64>,
    /// From the first packet to the end of the last, or the packets' durations added up
    /// when they have no timestamps (raw H.264).
    pub(crate) duration: Option<f64>,
}

/// Reads the timestamp, duration and keyframe flag of every packet of one stream
/// (`stream`, an ffprobe stream specifier) over the whole file. Nothing is decoded, so
/// 5 minutes of 1080p60 takes under a tenth of a second from local disk. An error means
/// ffprobe couldn't get at the file (network, SAS), which a retry may fix.
pub(crate) async fn scan_packets(input: &str, stream: &str) -> anyhow::Result<Packets> {
    let mut ffprobe = Command::new("ffprobe");
    ffprobe
        .args(["-v", "error"])
        .args(allowlists(input))
        .args([
            "-select_streams",
            stream,
            "-show_entries",
            "packet=pts_time,duration_time,flags",
            "-of",
            "csv=p=0",
        ])
        .arg(input);
    let out = run_within(ffprobe, "ffprobe", FFPROBE_TIME_LIMIT).await?;
    if !out.status.success() {
        let stderr = error_lines(&out.stderr);
        if unreadable(&stderr) {
            return Ok(Packets::default());
        }
        bail!("ffprobe failed reading packets: {stderr}");
    }
    Ok(read_packets(&String::from_utf8_lossy(&out.stdout)))
}

/// From ffprobe's `pts_time,duration_time,flags` lines. The keyframe gap counts the
/// stretches from the first packet to the first keyframe and from the last keyframe to the
/// last packet, and is measured from the first packet, not 0: an MKV can start at 10 s.
pub(crate) fn read_packets(csv: &str) -> Packets {
    let mut keys = Vec::new();
    let mut span = None::<(f64, f64)>;
    let mut end = None::<f64>;
    let mut total = 0.0;
    for line in csv.lines() {
        let mut parts = line.split(',');
        let pts = parts.next().and_then(|p| p.parse::<f64>().ok());
        let length = parts.next().and_then(|d| d.parse::<f64>().ok());
        let key = parts.next().is_some_and(|f| f.contains('K'));
        total += length.unwrap_or(0.0);
        let Some(t) = pts else {
            continue;
        };
        span = Some(span.map_or((t, t), |(first, last)| (first.min(t), last.max(t))));
        end = Some(end.unwrap_or(t).max(t + length.unwrap_or(0.0)));
        if key {
            keys.push(t);
        }
    }
    let duration = match (span, end) {
        (Some((first, _)), Some(end)) => Some(end - first),
        _ => (total > 0.0).then_some(total),
    };
    keys.sort_by(f64::total_cmp);
    let keyframe_gap = match (span, keys.first(), keys.last()) {
        (Some((first, last)), Some(first_key), Some(last_key)) => {
            let mut gap = (first_key - first).max(last - last_key);
            for pair in keys.windows(2) {
                gap = gap.max(pair[1] - pair[0]);
            }
            Some(gap)
        }
        _ => None,
    };
    Packets {
        keyframe_gap,
        duration,
    }
}

/// Above this, even a browser-ready H.264 file is re-encoded so phones can stream it.
const REMUX_MAX_BIT_RATE: f64 = 20_000_000.0;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Mode {
    /// Copy the streams into a fresh MP4 with the index up front.
    Remux,
    /// Re-encode to H.264/AAC.
    Transcode,
}

impl Mode {
    fn as_str(self) -> &'static str {
        match self {
            Self::Remux => "remux",
            Self::Transcode => "transcode",
        }
    }
}

/// Whether the original already plays everywhere as is: 8-bit 4:2:0 progressive H.264
/// with square pixels at most 1080p / 60 fps, AAC or no audio, and a bitrate phones can
/// stream. ShadowPlay's and Medal's defaults usually are; HEVC, 1440p and 144 fps
/// recordings aren't.
fn can_remux(probe: &Probe, duration_s: f64) -> bool {
    let Some(video) = probe.video() else {
        return false;
    };
    let bit_rate = probe
        .format
        .bit_rate
        .as_deref()
        .and_then(|b| b.parse::<f64>().ok())
        .or_else(|| {
            let bytes: f64 = probe.format.size.as_deref()?.parse().ok()?;
            (duration_s > 0.0).then(|| bytes * 8.0 / duration_s)
        });
    video.codec_name.as_deref() == Some("h264")
        && matches!(video.pix_fmt.as_deref(), Some("yuv420p" | "yuvj420p"))
        && video.height.is_some_and(|h| h <= MAX_HEIGHT)
        && video.fps() <= MAX_FPS + 0.5
        && video.square_pixels()
        && !video.interlaced()
        && probe
            .audio()
            .is_none_or(|a| a.codec_name.as_deref() == Some("aac"))
        && bit_rate.is_some_and(|b| b <= REMUX_MAX_BIT_RATE)
}

/// The output options every ffmpeg run on an upload ends with: no subtitle or data
/// streams, and no more than `CUT_AT_S` of it.
fn upload_output_args() -> [String; 4] {
    [
        "-sn".into(),
        "-dn".into(),
        "-t".into(),
        CUT_AT_S.to_string(),
    ]
}

fn remux_args(input: &str, output: &Path, video_index: usize, has_audio: bool) -> Vec<String> {
    let mut args: Vec<String> = ["-hide_banner", "-nostdin", "-y", "-loglevel", "error"]
        .map(String::from)
        .into();
    args.extend(allowlists(input));
    args.extend(["-i", input, "-map", &format!("0:{video_index}")].map(String::from));
    if has_audio {
        args.extend(["-map", "0:a:0"].map(String::from));
    }
    args.extend(
        [
            "-c",
            "copy",
            "-tag:v",
            "avc1",
            "-map_metadata",
            "-1",
            "-movflags",
            "+faststart",
        ]
        .map(String::from),
    );
    args.extend(upload_output_args());
    args.push(output.to_string_lossy().into_owned());
    args
}

fn ffmpeg_args(
    input: &str,
    output: &Path,
    config: &TranscodeConfig,
    video: &Stream,
    has_audio: bool,
) -> Vec<String> {
    let mut filters = Vec::new();
    if video.interlaced() {
        // A capture card's 1080i, say: combing on every moving edge otherwise.
        filters.push("bwdif=mode=send_frame".to_owned());
    }
    // Height capped at 1080 (never upscaled), width from the display aspect ratio (an
    // anamorphic 1440×1080 with 4:3 pixels becomes 1920×1080), both even (4:2:0 needs
    // it, so 1281×721 becomes 1280×720), square pixels.
    filters.extend([
        format!("scale='round(oh*dar/2)*2':'trunc(min({MAX_HEIGHT},ih)/2)*2':flags=bicubic"),
        "setsar=1".to_owned(),
    ]);
    if video.fps() > MAX_FPS + 0.5 {
        filters.push(format!("fps={MAX_FPS}"));
    }
    filters.push("format=yuv420p".into());

    let mut args: Vec<String> = [
        "-hide_banner",
        "-nostdin",
        "-y",
        "-loglevel",
        "error",
        "-threads",
        &config.threads.to_string(),
    ]
    .map(String::from)
    .into();
    args.extend(allowlists(input));
    args.extend(["-i", input, "-map", &format!("0:{}", video.index)].map(String::from));
    if has_audio {
        // OBS can record several tracks (game, mic, …); the first is the game mix.
        args.extend(["-map", "0:a:0"].map(String::from));
    }
    args.extend(
        [
            "-vf",
            &filters.join(","),
            "-c:v",
            "libx264",
            "-preset",
            &config.preset,
            "-crf",
            &config.crf.to_string(),
            "-profile:v",
            "high",
            // A keyframe every 2 s, whatever the frame rate: seeks in the show land fast
            // (decision 35).
            "-force_key_frames",
            "expr:gte(t,n_forced*2)",
            "-threads",
            &config.threads.to_string(),
        ]
        .map(String::from),
    );
    if has_audio {
        args.extend(["-c:a", "aac", "-b:a", "160k", "-ac", "2"].map(String::from));
    }
    args.extend(
        [
            "-map_metadata",
            "-1",
            // Moov atom up front so playback starts before the whole file downloads.
            "-movflags",
            "+faststart",
        ]
        .map(String::from),
    );
    args.extend(upload_output_args());
    args.push(output.to_string_lossy().into_owned());
    args
}

/// From the poster we wrote, so only a JPEG from disk. The format is named, not probed: a
/// poster small enough for the first probe (a dark or flat frame) reads as `jpeg_pipe`,
/// which the allowlist would refuse.
fn teaser_args(poster: &Path, output: &Path) -> Vec<String> {
    [
        "-hide_banner",
        "-nostdin",
        "-y",
        "-loglevel",
        "error",
        "-protocol_whitelist",
        "file",
        "-format_whitelist",
        "image2",
        "-codec_whitelist",
        "mjpeg",
        "-f",
        "image2",
        "-i",
    ]
    .map(String::from)
    .into_iter()
    .chain([poster.to_string_lossy().into_owned()])
    .chain(["-vf", "scale=320:-2,gblur=sigma=14", "-q:v", "6"].map(String::from))
    .chain([output.to_string_lossy().into_owned()])
    .collect()
}

/// Grabs the poster from the middle of the playback file's video, or from its first frame
/// if that writes nothing: the seek can still land past the last frame of an odd file.
/// ffmpeg fails when it writes nothing, so a failed first try falls back too.
async fn make_poster(playback: &Path, poster: &Path, video: &Stream) -> Result<(), JobError> {
    let written = || async { tokio::fs::metadata(poster).await.is_ok_and(|m| m.len() > 0) };
    let _ = tokio::fs::remove_file(poster).await;
    let at_seek = run_ffmpeg(&poster_args(playback, poster, poster_time(video))).await;
    if !written().await {
        let error = at_seek.err().map(|e| format!("{e:#}"));
        tracing::info!(
            ?error,
            "no frame at the poster's seek; using the first frame"
        );
        run_ffmpeg(&poster_args(playback, poster, 0.0)).await?;
    }
    if !written().await {
        return Err(JobError::Permanent(format!(
            "{} (no frame of it could be decoded)",
            clips::UNREADABLE_ERROR
        )));
    }
    Ok(())
}

/// Halfway through the video stream, and at least a frame before its end. The video's own
/// length, not the file's: a recording whose video stopped early has audio running on
/// past it.
fn poster_time(video: &Stream) -> f64 {
    let Some(duration) = video.duration() else {
        return 0.0;
    };
    let frame = match video.fps() {
        fps if fps > 0.0 => 1.0 / fps,
        _ => 0.0,
    };
    (duration / 2.0).min(duration - frame).max(0.0)
}

fn poster_args(input: &Path, output: &Path, at_s: f64) -> Vec<String> {
    let input = input.to_string_lossy().into_owned();
    let mut args: Vec<String> = ["-hide_banner", "-nostdin", "-y", "-loglevel", "error"]
        .map(String::from)
        .into();
    args.extend(allowlists(&input));
    args.extend([
        "-ss".into(),
        format!("{at_s:.3}"),
        "-i".into(),
        input,
        "-frames:v".into(),
        "1".into(),
        "-vf".into(),
        "scale=-2:'min(720,ih)'".into(),
        "-q:v".into(),
        "3".into(),
        "-sn".into(),
        "-dn".into(),
        output.to_string_lossy().into_owned(),
    ]);
    args
}

/// Runs ffmpeg at low CPU priority (`nice`), so the api on the same plan stays responsive.
async fn run_ffmpeg(args: &[String]) -> anyhow::Result<()> {
    let mut ffmpeg = Command::new("nice");
    ffmpeg.args(["-n", "10", "ffmpeg"]).args(args);
    let out = run_within(ffmpeg, "ffmpeg", FFMPEG_TIME_LIMIT).await?;
    if !out.status.success() {
        bail!("ffmpeg failed: {}", error_lines(&out.stderr));
    }
    Ok(())
}

/// Runs `command` (ffmpeg or ffprobe, `what`) to the end and returns what it printed, or
/// kills it once it has run for `limit`.
async fn run_within(mut command: Command, what: &str, limit: Duration) -> anyhow::Result<Output> {
    command.stdin(Stdio::null()).kill_on_drop(true);
    tokio::time::timeout(limit, command.output())
        .await
        .map_err(|_| anyhow!("{what} ran for over {limit:?}; stopped it"))?
        .with_context(|| format!("running {what} (is it installed?)"))
}

#[derive(Debug, Deserialize)]
pub(crate) struct Probe {
    #[serde(default)]
    streams: Vec<Stream>,
    format: Format,
}

#[derive(Debug, Default, Deserialize)]
pub(crate) struct Stream {
    /// Its index in the file, for `-map 0:{index}`.
    pub(crate) index: usize,
    codec_type: String,
    codec_name: Option<String>,
    pub(crate) width: Option<i64>,
    pub(crate) height: Option<i64>,
    pix_fmt: Option<String>,
    avg_frame_rate: Option<String>,
    r_frame_rate: Option<String>,
    duration: Option<String>,
    /// `16:9`, `4:3`, …; `1:1` (or missing) for square pixels.
    sample_aspect_ratio: Option<String>,
    /// `progressive`, or `tt`, `bb`, `tb`, `bt` for interlaced.
    field_order: Option<String>,
    #[serde(default)]
    disposition: Disposition,
    /// A phone's portrait recording is stored landscape with a display matrix saying to
    /// turn it.
    #[serde(default)]
    side_data_list: Vec<SideData>,
}

#[derive(Debug, Default, Deserialize)]
struct Disposition {
    /// Cover art or a thumbnail, not the video.
    #[serde(default)]
    attached_pic: u8,
}

#[derive(Debug, Default, Deserialize)]
struct SideData {
    rotation: Option<f64>,
}

#[derive(Debug, Deserialize)]
struct Format {
    format_name: Option<String>,
    duration: Option<String>,
    size: Option<String>,
    bit_rate: Option<String>,
}

impl Probe {
    /// The video: the largest video stream that isn't cover art, then the longest. The
    /// first one may be a thumbnail or a cover image.
    pub(crate) fn video(&self) -> Option<&Stream> {
        self.streams
            .iter()
            .filter(|s| s.codec_type == "video" && s.disposition.attached_pic == 0)
            // `max_by` keeps the last of equals; reversed, that's the first.
            .rev()
            .max_by(|a, b| {
                (a.width.unwrap_or(0) * a.height.unwrap_or(0))
                    .cmp(&(b.width.unwrap_or(0) * b.height.unwrap_or(0)))
                    .then(
                        a.duration()
                            .unwrap_or(0.0)
                            .total_cmp(&b.duration().unwrap_or(0.0)),
                    )
            })
    }

    fn audio(&self) -> Option<&Stream> {
        self.streams.iter().find(|s| s.codec_type == "audio")
    }

    fn duration(&self) -> Option<f64> {
        self.format.duration.as_deref()?.parse().ok()
    }
}

impl Stream {
    fn duration(&self) -> Option<f64> {
        self.duration.as_deref()?.parse().ok()
    }

    fn square_pixels(&self) -> bool {
        match self.sample_aspect_ratio.as_deref() {
            // 0:1 means unknown, taken as square like players do.
            None | Some("1:1" | "0:1") => true,
            Some(_) => false,
        }
    }

    fn interlaced(&self) -> bool {
        matches!(self.field_order.as_deref(), Some("tt" | "bb" | "tb" | "bt"))
    }

    /// Width and height as players show it, turned by its display matrix.
    pub(crate) fn upright_size(&self) -> (i64, i64) {
        let (width, height) = (self.width.unwrap_or(0), self.height.unwrap_or(0));
        let quarter_turn = self
            .side_data_list
            .iter()
            .filter_map(|d| d.rotation)
            .any(|r| (r.round() as i64).rem_euclid(180) == 90);
        if quarter_turn {
            (height, width)
        } else {
            (width, height)
        }
    }

    /// Average frame rate (ShadowPlay and OBS often record variable frame rate).
    fn fps(&self) -> f64 {
        [&self.avg_frame_rate, &self.r_frame_rate]
            .into_iter()
            .flatten()
            .filter_map(|r| parse_rate(r))
            .find(|f| *f > 0.0)
            .unwrap_or(0.0)
    }
}

fn parse_rate(rate: &str) -> Option<f64> {
    let (num, den) = rate.split_once('/')?;
    let (num, den): (f64, f64) = (num.parse().ok()?, den.parse().ok()?);
    (den > 0.0).then_some(num / den)
}

pub(crate) async fn ffprobe(input: &str) -> Result<Probe, JobError> {
    let mut ffprobe = Command::new("ffprobe");
    ffprobe
        .args(["-v", "error"])
        .args(allowlists(input))
        .args(["-print_format", "json", "-show_format", "-show_streams"])
        .arg(input);
    let out = run_within(ffprobe, "ffprobe", FFPROBE_TIME_LIMIT).await?;
    if !out.status.success() {
        if let Some(reason) = refused(&String::from_utf8_lossy(&out.stderr)) {
            return Err(JobError::Permanent(reason));
        }
        let stderr = error_lines(&out.stderr);
        if unreadable(&stderr) {
            return Err(JobError::Permanent(format!(
                "{} (is the upload complete and not corrupt?)",
                clips::UNREADABLE_ERROR
            )));
        }
        return Err(anyhow!("ffprobe failed: {stderr}").into());
    }
    serde_json::from_slice(&out.stdout)
        .context("parsing ffprobe output")
        .map_err(Into::into)
}

/// Why, for the uploader, when it was the allowlists that stopped ffprobe: a format we
/// don't read (ffprobe doesn't say which), or a codec (it names the decoder). A refused
/// protocol isn't the file's fault, so it isn't one of these.
fn refused(stderr: &str) -> Option<String> {
    if stderr.contains("Format not on whitelist") {
        return Some(format!(
            "{} (it isn't an MP4, MKV or MOV file)",
            clips::NO_VIDEO_ERROR
        ));
    }
    let (_, rest) = stderr.split_once("Codec (")?;
    let (codec, _) = rest.split_once(") not on whitelist")?;
    Some(format!(
        "{} (we don't read {codec})",
        clips::UNREADABLE_ERROR
    ))
}

/// Whether ffprobe failed on the file's data, which is the file's fault; anything else
/// (network, auth) may pass.
fn unreadable(stderr: &str) -> bool {
    stderr.contains("Invalid data found") || stderr.contains("moov atom not found")
}

/// The gist of ffmpeg's or ffprobe's stderr: the first line, which is usually the cause
/// (libx264's "height not divisible by 2"), and the last two, how it gave up. The lines in
/// between are mostly each thread reporting the same failure.
pub(crate) fn error_lines(stderr: &[u8]) -> String {
    let text = String::from_utf8_lossy(stderr);
    let lines: Vec<&str> = text.lines().filter(|l| !l.trim().is_empty()).collect();
    if lines.len() <= 3 {
        return lines.join(" | ");
    }
    format!(
        "{} | … | {}",
        lines[0],
        lines[lines.len() - 2..].join(" | ")
    )
}

fn too_long(duration: f64) -> JobError {
    JobError::Permanent(format!(
        "{} (this one is {}:{:02})",
        clips::TOO_LONG_ERROR,
        (duration / 60.0) as u32,
        (duration % 60.0) as u32
    ))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[tokio::test]
    async fn reports_free_disk_space() {
        let free = free_bytes(&std::env::temp_dir()).await;
        assert!(free.is_some_and(|b| b > 0), "{free:?}");
        assert_eq!(free_bytes(Path::new("/definitely/not/here")).await, None);
    }

    #[test]
    fn parses_frame_rates() {
        assert_eq!(parse_rate("60/1"), Some(60.0));
        assert!((parse_rate("30000/1001").unwrap() - 29.97).abs() < 0.01);
        assert_eq!(parse_rate("0/0"), None);
    }

    fn config() -> TranscodeConfig {
        TranscodeConfig {
            temp_dir: PathBuf::from("/tmp"),
            threads: 1,
            preset: "veryfast".into(),
            crf: 20,
        }
    }

    fn video_at(fps: &str) -> Stream {
        Stream {
            codec_type: "video".into(),
            avg_frame_rate: Some(fps.into()),
            ..Stream::default()
        }
    }

    fn vf(video: &Stream) -> String {
        let args = ffmpeg_args("in", Path::new("out.mp4"), &config(), video, true);
        args[args.iter().position(|a| a == "-vf").unwrap() + 1].clone()
    }

    #[test]
    fn caps_frame_rate_only_above_60() {
        assert!(vf(&video_at("144/1")).contains("fps=60"));
        assert!(!vf(&video_at("60/1")).contains("fps="));
        assert!(!vf(&video_at("60000/1001")).contains("fps="));
    }

    #[test]
    fn deinterlaces_only_interlaced_sources() {
        let interlaced = Stream {
            field_order: Some("tt".into()),
            ..video_at("30/1")
        };
        assert!(vf(&interlaced).starts_with("bwdif"));
        assert!(!vf(&video_at("30/1")).contains("bwdif"));
    }

    #[test]
    fn skips_audio_when_there_is_none() {
        let args = ffmpeg_args(
            "in",
            Path::new("out.mp4"),
            &config(),
            &video_at("60/1"),
            false,
        );
        assert!(!args.contains(&"0:a:0".to_owned()));
        assert!(!args.contains(&"aac".to_owned()));
    }

    fn probe(json: Value) -> Probe {
        serde_json::from_value(json).unwrap()
    }

    #[test]
    fn picks_the_largest_video_stream_that_isnt_cover_art() {
        let cover = json!({ "attached_pic": 1 });
        let picked = |streams: Value| {
            probe(json!({ "streams": streams, "format": {} }))
                .video()
                .map(|v| v.index)
        };
        // Cover art first, and bigger than the video.
        assert_eq!(
            picked(json!([
                { "index": 0, "codec_type": "video", "width": 3000, "height": 3000, "disposition": cover },
                { "index": 1, "codec_type": "video", "width": 1280, "height": 720 },
            ])),
            Some(1)
        );
        // A thumbnail track first.
        assert_eq!(
            picked(json!([
                { "index": 0, "codec_type": "audio" },
                { "index": 1, "codec_type": "video", "width": 160, "height": 90 },
                { "index": 2, "codec_type": "video", "width": 1920, "height": 1080 },
            ])),
            Some(2)
        );
        // Same size: the longer, then the first.
        assert_eq!(
            picked(json!([
                { "index": 0, "codec_type": "video", "width": 640, "height": 360, "duration": "1.0" },
                { "index": 1, "codec_type": "video", "width": 640, "height": 360, "duration": "9.0" },
                { "index": 2, "codec_type": "video", "width": 640, "height": 360, "duration": "9.0" },
            ])),
            Some(1)
        );
        // Audio with a cover: no video.
        assert_eq!(
            picked(json!([
                { "index": 0, "codec_type": "audio" },
                { "index": 1, "codec_type": "video", "width": 600, "height": 600, "disposition": cover },
            ])),
            None
        );
    }

    #[test]
    fn turns_the_size_by_the_display_matrix() {
        let rotated = |rotation: f64| {
            probe(json!({
                "streams": [{
                    "index": 0, "codec_type": "video", "width": 1920, "height": 1080,
                    "side_data_list": [{ "side_data_type": "Display Matrix", "rotation": rotation }],
                }],
                "format": {},
            }))
            .video()
            .unwrap()
            .upright_size()
        };
        assert_eq!(rotated(90.0), (1080, 1920));
        assert_eq!(rotated(-90.0), (1080, 1920));
        assert_eq!(rotated(270.0), (1080, 1920));
        assert_eq!(rotated(180.0), (1920, 1080));
        assert_eq!(rotated(0.0), (1920, 1080));
    }

    #[test]
    fn poster_is_from_the_videos_own_middle() {
        let video = |duration: &str| Stream {
            duration: Some(duration.into()),
            ..video_at("30/1")
        };
        assert_eq!(poster_time(&video("2.0")), 1.0);
        // One frame: its start.
        assert_eq!(poster_time(&video("0.033333")), 0.0);
        assert_eq!(poster_time(&video_at("30/1")), 0.0);
    }

    /// Makes a one-second 320×240 H.264 test video at `path`.
    fn one_second_video(path: &Path) {
        let status = std::process::Command::new("ffmpeg")
            .args(["-hide_banner", "-loglevel", "error", "-y"])
            .args(["-f", "lavfi", "-i", "testsrc2=size=320x240:rate=30"])
            .args(["-t", "1", "-c:v", "libx264", "-pix_fmt", "yuv420p"])
            .arg(path)
            .status()
            .expect("ffmpeg is installed");
        assert!(status.success());
    }

    /// The seek can land past the last frame when the video's length is off (ffmpeg then
    /// writes nothing and fails): the poster comes from the first frame instead.
    #[tokio::test]
    async fn a_poster_past_the_last_frame_falls_back_to_the_first() {
        let dir = tempfile::tempdir().unwrap();
        let playback = dir.path().join("playback.mp4");
        one_second_video(&playback);
        // Says it's 100 s long: the middle is 50 s in, well past its one second.
        let video = Stream {
            duration: Some("100".into()),
            ..video_at("30/1")
        };
        assert_eq!(poster_time(&video), 50.0);
        let poster = dir.path().join("poster.jpg");
        make_poster(&playback, &poster, &video).await.unwrap();
        let written = std::fs::read(&poster).unwrap();
        assert!(written.starts_with(&[0xFF, 0xD8]), "a JPEG");
    }

    #[tokio::test]
    async fn no_poster_from_a_file_without_frames() {
        let dir = tempfile::tempdir().unwrap();
        let junk = dir.path().join("junk.mp4");
        std::fs::write(&junk, vec![0x42u8; 4096]).unwrap();
        let poster = dir.path().join("poster.jpg");
        let result = make_poster(&junk, &poster, &video_at("30/1")).await;
        assert!(result.is_err());
        assert!(!poster.exists());
    }

    #[tokio::test]
    async fn ffmpeg_and_ffprobe_failures_keep_the_cause() {
        let dir = tempfile::tempdir().unwrap();
        let missing = dir.path().join("missing.mp4");
        let missing = missing.to_str().unwrap();
        // Not the file's fault (it isn't there to blame): retried.
        let result = ffprobe(missing).await;
        assert!(matches!(result, Err(JobError::Retry(_))), "{result:?}");
        if let Err(JobError::Retry(e)) = result {
            let e = e.to_string();
            assert!(e.starts_with("ffprobe failed: "), "{e}");
            assert!(e.contains("No such file or directory"), "{e}");
        }
        let args: Vec<String> = ["-hide_banner", "-loglevel", "error", "-i", missing]
            .map(String::from)
            .into();
        let e = run_ffmpeg(&args).await.unwrap_err().to_string();
        assert!(e.starts_with("ffmpeg failed: "), "{e}");
        assert!(e.contains("No such file or directory"), "{e}");
    }

    #[tokio::test]
    async fn packets_of_unreadable_data_measure_nothing() {
        let dir = tempfile::tempdir().unwrap();
        let junk = dir.path().join("junk.mp4");
        std::fs::write(&junk, vec![0x42u8; 4096]).unwrap();
        // The file's fault: nothing to measure, which a re-encode fixes (not an error).
        let packets = scan_packets(junk.to_str().unwrap(), "v:0").await.unwrap();
        assert_eq!(packets, Packets::default());
        // Not the file's fault: an error, which a retry may fix.
        let missing = dir.path().join("missing.mp4");
        let e = scan_packets(missing.to_str().unwrap(), "v:0")
            .await
            .unwrap_err();
        assert!(
            e.to_string().starts_with("ffprobe failed reading packets"),
            "{e}"
        );
    }

    #[test]
    fn only_a_video_can_be_remuxed() {
        let audio_only = probe(json!({
            "streams": [{ "index": 0, "codec_type": "audio", "codec_name": "aac" }],
            "format": { "bit_rate": "128000" },
        }));
        assert!(!can_remux(&audio_only, 10.0));
        // Without a frame rate the poster is simply from the middle.
        let video = Stream {
            duration: Some("3.0".into()),
            ..Stream::default()
        };
        assert_eq!(poster_time(&video), 1.5);
    }

    #[test]
    fn reads_lengths_from_packets() {
        // Timestamps: from the first packet to the end of the last.
        let packets = read_packets("10.0,0.5,K_\n10.5,0.5,__\n11.0,0.5,K_\n");
        assert_eq!(packets.duration, Some(1.5));
        assert_eq!(packets.keyframe_gap, Some(1.0));
        // Raw H.264: no timestamps, only durations.
        let raw = "N/A,0.200000,K__\nN/A,0.200000,___\nN/A,0.200000,___\n";
        let packets = read_packets(raw);
        assert!((packets.duration.unwrap() - 0.6).abs() < 1e-9);
        assert_eq!(packets.keyframe_gap, None);
        assert_eq!(read_packets(""), Packets::default());
    }

    #[test]
    fn allowlists_name_only_the_protocol_the_input_needs() {
        let protocols = |input: &str| {
            let args = allowlists(input);
            args[args
                .iter()
                .position(|a| a == "-protocol_whitelist")
                .unwrap()
                + 1]
            .clone()
        };
        assert_eq!(protocols("/tmp/clipos-transcode/x/original.mp4"), "file");
        assert_eq!(
            protocols("https://media.blob.core.windows.net/originals/x.mp4?sig=s"),
            "https,tls,tcp"
        );
        assert_eq!(
            protocols("http://127.0.0.1:10000/devstoreaccount1/originals/x.mp4"),
            "http,tcp"
        );
    }

    /// Every ffmpeg run on an upload has the allowlists before its input and writes no
    /// subtitles or data; those that write the clip stop at `CUT_AT_S`. The teaser reads
    /// only the JPEG we wrote.
    #[test]
    fn every_run_on_an_upload_is_fenced_in() {
        let input = "/tmp/clipos-transcode/x/original.mkv";
        let split = |args: &[String]| {
            let at = args.iter().position(|a| a == "-i").unwrap();
            (args[..at].join(" "), args[at..].to_vec())
        };
        let video = video_at("60/1");
        let clip_runs = [
            remux_args(input, Path::new("out.mp4"), 0, true),
            ffmpeg_args(input, Path::new("out.mp4"), &config(), &video, true),
        ];
        let poster = poster_args(Path::new(input), Path::new("poster.jpg"), 1.0);
        for args in clip_runs.iter().chain([&poster]) {
            let (before, after) = split(args);
            assert!(before.contains(&allowlists(input).join(" ")), "{args:?}");
            assert!(after.contains(&"-sn".into()) && after.contains(&"-dn".into()));
        }
        for args in &clip_runs {
            let at = args.iter().position(|a| a == "-t").unwrap();
            assert_eq!(args[at + 1], CUT_AT_S.to_string());
        }
        let (before, _) = split(&teaser_args(
            Path::new("poster.jpg"),
            Path::new("teaser.jpg"),
        ));
        assert!(
            before.ends_with(
                "-protocol_whitelist file -format_whitelist image2 -codec_whitelist mjpeg \
                 -f image2"
            ),
            "{before}"
        );
    }

    /// However long the upload, ffmpeg writes no more than `CUT_AT_S` of it, copied or
    /// encoded.
    #[tokio::test]
    async fn ffmpeg_stops_writing_at_the_cut() {
        let dir = tempfile::tempdir().unwrap();
        let long = dir.path().join("long.mkv");
        let status = std::process::Command::new("ffmpeg")
            .args(["-hide_banner", "-loglevel", "error", "-y"])
            .args([
                "-f",
                "lavfi",
                "-i",
                "testsrc2=size=160x90:rate=2",
                "-t",
                "320",
            ])
            .args([
                "-c:v",
                "libx264",
                "-preset",
                "ultrafast",
                "-pix_fmt",
                "yuv420p",
            ])
            .arg(&long)
            .status()
            .expect("ffmpeg is installed");
        assert!(status.success());
        let input = long.to_str().unwrap();
        let copied = dir.path().join("copied.mp4");
        run_ffmpeg(&remux_args(input, &copied, 0, false))
            .await
            .unwrap();
        let encoded = dir.path().join("encoded.mp4");
        run_ffmpeg(&ffmpeg_args(
            input,
            &encoded,
            &config(),
            &video_at("2/1"),
            false,
        ))
        .await
        .unwrap();
        for out in [copied, encoded] {
            let probe = ffprobe(out.to_str().unwrap()).await.unwrap();
            let duration = probe.duration().unwrap();
            assert!(
                (CUT_AT_S - 1.0..=CUT_AT_S).contains(&duration),
                "{duration}"
            );
        }
    }

    #[tokio::test]
    async fn stops_a_run_past_its_time_limit() {
        let dir = tempfile::tempdir().unwrap();
        let finished = dir.path().join("finished");
        let mut slow = Command::new("sh");
        slow.arg("-c")
            .arg(format!("sleep 1; touch '{}'", finished.display()));
        let started = Instant::now();
        let e = run_within(slow, "sh", Duration::from_millis(200))
            .await
            .unwrap_err();
        assert!(started.elapsed() < Duration::from_secs(1));
        assert_eq!(e.to_string(), "sh ran for over 200ms; stopped it");
        // Killed, not left running.
        tokio::time::sleep(Duration::from_millis(1500)).await;
        assert!(!finished.exists());
    }

    #[test]
    fn says_what_the_allowlists_refused() {
        let format = "[caf @ 0x1] Format not on whitelist 'mov,mp4'\nx.mp4: Invalid argument\n";
        assert_eq!(
            refused(format).as_deref(),
            Some("the file has no video stream (it isn't an MP4, MKV or MOV file)")
        );
        let codec = "[mpeg2video @ 0x1] Codec (mpeg2video) not on whitelist 'h264,hevc'\n";
        assert_eq!(
            refused(codec).as_deref(),
            Some("the file isn't a video we can read (we don't read mpeg2video)")
        );
        // A refused protocol is our mistake, not the file's.
        let protocol = "[tls @ 0x1] Protocol 'tls' not on whitelist 'https,tcp'!\n";
        assert_eq!(refused(protocol), None);
        assert_eq!(refused("[mov @ 0x1] moov atom not found"), None);
    }

    #[test]
    fn keeps_the_cause_of_an_ffmpeg_failure() {
        let stderr = b"[libx264 @ 0x1] height not divisible by 2 (1282x721)\n\
            [vost#0:0/libx264] Error while opening encoder\n\
            [vf#0:0] Error sending frames to consumers\n\
            [vf#0:0] Task finished with error code\n\
            [out#0/mp4] Nothing was written into output file\n";
        let summary = error_lines(stderr);
        assert!(summary.starts_with("[libx264 @ 0x1] height not divisible by 2"));
        assert!(summary.ends_with("Nothing was written into output file"));
        assert_eq!(error_lines(b"one\n\ntwo\n"), "one | two");
    }
}
