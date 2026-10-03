//! `analyse` job: reads a CS2 clip's killfeed (phase 8, shadow mode).
//!
//! Samples the original at 1 fps, cuts the killfeed corner, finds the rows (row-finder
//! model), reads their icons (icon model) and whose they are (red outline), follows each
//! row across frames, and stores the kills and the recording player's summary in
//! `analysis_results`. Nothing is turned into tags yet.
//!
//! The models come from the `models` container (`<name>/<version>/`), are checked against
//! the SHA-256 in their `model-card.json`, and are kept in a local cache between jobs.

use std::{
    io::Read,
    path::{Path, PathBuf},
    process::{Command, Stdio},
    time::{Duration, Instant},
};

use anyhow::{Context, anyhow, bail};
use clipos_core::{
    analysis,
    clips::{self, ClipStatus},
    storage::Container,
};
use clipos_killfeed::{Analyzer, IconReader, Kill, RowFinder, SAMPLE_FPS, crop_box, summarise};
use image::RgbImage;
use serde::Deserialize;
use serde_json::{Value, json};
use sha2::{Digest, Sha256};
use uuid::Uuid;

use crate::{
    JobError, Worker,
    transcode::{CUT_AT_S, allowlists, error_lines, ffprobe, local_or_remote},
};

#[derive(Debug, Clone)]
pub struct AnalyseConfig {
    /// Local cache of downloaded models.
    pub models_dir: PathBuf,
    /// `<name>/<version>` in the `models` container.
    pub rows_model: String,
    pub icons_model: String,
    /// ONNX Runtime threads per model; low, like ffmpeg's, for the shared plan.
    pub threads: usize,
}

impl AnalyseConfig {
    pub fn version(&self) -> String {
        format!("rows={} icons={}", self.rows_model, self.icons_model)
    }
}

#[derive(Debug, Deserialize)]
struct Payload {
    #[serde(rename = "clipId")]
    clip_id: Uuid,
}

pub async fn run(worker: &Worker, payload: &Value) -> Result<(), JobError> {
    let Some(config) = &worker.analyse else {
        return Err(JobError::Permanent(
            "killfeed analysis is turned off on this worker".into(),
        ));
    };
    let Payload { clip_id } = serde_json::from_value(payload.clone())
        .map_err(|e| JobError::Permanent(format!("bad payload: {e}")))?;
    let Some(clip) = clips::get(&worker.pool, clip_id)
        .await
        .map_err(anyhow::Error::from)?
    else {
        tracing::info!(%clip_id, "clip is gone; nothing to analyse");
        return Ok(());
    };
    if clip.status != ClipStatus::Ready {
        tracing::info!(%clip_id, status = ?clip.status, "clip isn't ready; skipping analysis");
        return Ok(());
    }

    let started = Instant::now();
    let rows_model = ensure_model(worker, config, &config.rows_model).await?;
    let icons_model = ensure_model(worker, config, &config.icons_model).await?;

    let dir = worker.transcode.temp_dir.join(format!("analyse-{clip_id}"));
    tokio::fs::create_dir_all(&dir)
        .await
        .with_context(|| format!("creating {}", dir.display()))?;
    let result = async {
        let source = local_or_remote(worker, &clip, &dir).await?.input;
        let probe = ffprobe(&source).await?;
        let video = probe
            .video()
            .ok_or_else(|| JobError::Permanent("the file has no video stream".into()))?;
        // As decoded: ffmpeg turns a phone's portrait clip upright.
        let (width, height) = match video.upright_size() {
            (w, h) if w > 0 && h > 0 => (w as u32, h as u32),
            _ => return Err(JobError::Permanent("the video has no size".into())),
        };
        let input = Input {
            source,
            stream: video.index,
            width,
            height,
        };
        let length = Duration::from_millis(clip.duration_ms.unwrap_or(0).max(0) as u64);
        let deadline = Instant::now() + time_limit(length);
        let threads = config.threads;
        tokio::task::spawn_blocking(move || {
            read_killfeed(&rows_model, &icons_model, &input, threads, deadline)
        })
        .await
        .map_err(anyhow::Error::from)?
        .map_err(JobError::from)
    }
    .await;
    if let Err(e) = tokio::fs::remove_dir_all(&dir).await {
        tracing::warn!(error = %e, dir = %dir.display(), "couldn't remove temp dir");
    }
    let (kills, frames) = result?;

    let summary = summarise(&kills);
    let raw = json!({ "sampleFps": SAMPLE_FPS, "frames": frames, "kills": kills });
    let stats = serde_json::to_value(&summary).map_err(anyhow::Error::from)?;
    analysis::save(&worker.pool, clip_id, &config.version(), &raw, &stats)
        .await
        .map_err(anyhow::Error::from)?;
    tracing::info!(
        %clip_id,
        frames,
        kills = summary.kills,
        my_kills = summary.my_kills,
        multi_kill = summary.multi_kill.as_deref().unwrap_or("-"),
        elapsed_ms = started.elapsed().as_millis() as u64,
        "analysed killfeed"
    );
    Ok(())
}

/// How long reading a clip's killfeed may take before ffmpeg is stopped: well over what a
/// clip of `length` needs on the shared plan, so only a stuck ffmpeg (a stalled read, a
/// pathological file) hits it. In production the models take about 3.5 s a frame at one
/// frame a second, so a clip needs about 3.5 times its length: a 180 s clip took 634 s
/// against the old 120 s + 3x limit of 660 s, and a busier one timed out.
pub(crate) fn time_limit(length: Duration) -> Duration {
    Duration::from_secs(120) + length * 6
}

/// The video to sample: a local path or a read SAS, and the stream to read.
pub(crate) struct Input {
    pub(crate) source: String,
    pub(crate) stream: usize,
    pub(crate) width: u32,
    pub(crate) height: u32,
}

/// Decodes the input at [`SAMPLE_FPS`], cut to the killfeed corner, and runs the models on
/// every frame. Blocking: run it off the async runtime.
fn read_killfeed(
    rows_model: &Path,
    icons_model: &Path,
    input: &Input,
    threads: usize,
    deadline: Instant,
) -> anyhow::Result<(Vec<Kill>, u32)> {
    let mut finder = RowFinder::load(&rows_model.join("model.onnx"), threads)?;
    let mut reader = IconReader::load(&icons_model.join("model.onnx"), threads)?;
    let mut analyzer = Analyzer::new(&mut finder, &mut reader);
    sample_corner(input, deadline, |t, corner| analyzer.push(t, corner))?;
    let frames = analyzer.frames();
    Ok((analyzer.finish(), frames))
}

/// How much of ffmpeg's stderr is kept: the end, where the reason it stopped is.
const STDERR_TAIL: usize = 16 << 10;

/// Decodes the input at [`SAMPLE_FPS`], cut to the killfeed corner, and hands each frame
/// to `frame` with its time. ffmpeg is stopped at `deadline`. Returns how many frames
/// there were.
pub(crate) fn sample_corner(
    input: &Input,
    deadline: Instant,
    mut frame: impl FnMut(f64, &RgbImage) -> anyhow::Result<()>,
) -> anyhow::Result<u32> {
    let (cx, cy, cw, ch) = crop_box(input.width, input.height);
    // The same formats and codecs as the transcode, and no more of it than it kept.
    let mut ffmpeg = Command::new("nice")
        .args(["-n", "10", "ffmpeg", "-v", "error", "-nostdin"])
        .args(allowlists(&input.source))
        .arg("-i")
        .arg(&input.source)
        .arg("-map")
        .arg(format!("0:{}", input.stream))
        .arg("-vf")
        // Cut in RGB: on 4:2:0 video, crop rounds an odd size or offset down to even, and
        // the frames would no longer be the size read below.
        .arg(format!(
            "fps={SAMPLE_FPS},format=rgb24,crop={cw}:{ch}:{cx}:{cy}"
        ))
        .args(["-sn", "-dn", "-t", &CUT_AT_S.to_string()])
        .args(["-f", "rawvideo", "-pix_fmt", "rgb24", "-"])
        .stdin(Stdio::null())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .spawn()
        .context("running ffmpeg")?;
    let mut stdout = ffmpeg.stdout.take().context("ffmpeg stdout")?;
    // Read on its own thread: a damaged file makes ffmpeg write a line per broken
    // macroblock, and once the pipe's 64 KB fill up, ffmpeg would wait on us forever
    // while we wait on its frames.
    let mut stderr = ffmpeg.stderr.take().context("ffmpeg stderr")?;
    let stderr = std::thread::spawn(move || {
        let mut tail = Vec::new();
        let mut chunk = [0u8; 8192];
        while let Ok(n) = stderr.read(&mut chunk) {
            if n == 0 {
                break;
            }
            tail.extend_from_slice(&chunk[..n]);
            if tail.len() > 2 * STDERR_TAIL {
                tail.drain(..tail.len() - STDERR_TAIL);
            }
        }
        tail
    });
    // Watches the clock and stops ffmpeg at the deadline, which ends our reads too.
    let watchdog = std::thread::spawn(move || {
        loop {
            match ffmpeg.try_wait() {
                Ok(Some(status)) => return Ok((status, false)),
                Ok(None) if Instant::now() >= deadline => {
                    let _ = ffmpeg.kill();
                    return ffmpeg.wait().map(|status| (status, true));
                }
                Ok(None) => std::thread::sleep(Duration::from_millis(100)),
                Err(e) => return Err(e),
            }
        }
    });

    let mut buf = vec![0u8; (cw * ch * 3) as usize];
    let mut n = 0u32;
    let mut result = Ok(());
    let mut late = false;
    loop {
        if Instant::now() >= deadline {
            late = true;
            break;
        }
        if stdout.read_exact(&mut buf).is_err() {
            break;
        }
        result = RgbImage::from_raw(cw, ch, buf.clone())
            .context("frame size")
            .and_then(|corner| frame(f64::from(n) / SAMPLE_FPS, &corner));
        if result.is_err() {
            break;
        }
        n += 1;
    }
    // Stopped early (a model failed, or out of time): ffmpeg exits once nobody reads.
    drop(stdout);
    let (status, killed) = watchdog
        .join()
        .map_err(|_| anyhow!("the ffmpeg watchdog panicked"))??;
    let stderr = stderr.join().unwrap_or_default();
    result?;
    if killed || late {
        bail!("reading the killfeed took too long; stopped after {n} frames");
    }
    if !status.success() {
        bail!(
            "ffmpeg failed while sampling frames: {}",
            error_lines(&stderr)
        );
    }
    Ok(n)
}

#[derive(Debug, Deserialize)]
struct ModelCard {
    files: std::collections::HashMap<String, CardFile>,
}

#[derive(Debug, Deserialize)]
struct CardFile {
    sha256: String,
}

/// The files the worker needs from a model version.
const MODEL_FILES: [&str; 2] = ["model.onnx", "classes.json"];

/// Makes sure `<name>/<version>` is in the local cache, downloading and checking it against
/// its model card if not. Returns the local folder.
async fn ensure_model(
    worker: &Worker,
    config: &AnalyseConfig,
    name: &str,
) -> anyhow::Result<PathBuf> {
    let local = config.models_dir.join(name);
    let verified = local.join(".verified");
    if tokio::fs::try_exists(&verified).await.unwrap_or(false) {
        return Ok(local);
    }
    tokio::fs::create_dir_all(&local)
        .await
        .with_context(|| format!("creating {}", local.display()))?;
    let started = Instant::now();
    let card_path = local.join("model-card.json");
    worker
        .storage
        .download_to_file(
            Container::Models,
            &format!("{name}/model-card.json"),
            &card_path,
            None,
        )
        .await
        .with_context(|| format!("downloading the model card of {name}"))?;
    let card: ModelCard = serde_json::from_slice(&tokio::fs::read(&card_path).await?)
        .with_context(|| format!("reading the model card of {name}"))?;
    for file in MODEL_FILES {
        let want = &card
            .files
            .get(file)
            .ok_or_else(|| anyhow!("{name}'s model card lists no {file}"))?
            .sha256;
        let partial = local.join(format!("{file}.partial"));
        worker
            .storage
            .download_to_file(Container::Models, &format!("{name}/{file}"), &partial, None)
            .await
            .with_context(|| format!("downloading {name}/{file}"))?;
        let got = {
            let partial = partial.clone();
            tokio::task::spawn_blocking(move || sha256_file(&partial)).await??
        };
        if &got != want {
            let _ = tokio::fs::remove_file(&partial).await;
            bail!("{name}/{file} doesn't match its model card (sha256 {got}, expected {want})");
        }
        tokio::fs::rename(&partial, local.join(file)).await?;
    }
    tokio::fs::write(&verified, b"").await?;
    tracing::info!(
        model = name,
        elapsed_ms = started.elapsed().as_millis() as u64,
        "downloaded model"
    );
    Ok(local)
}

fn sha256_file(path: &Path) -> anyhow::Result<String> {
    let mut file = std::fs::File::open(path)?;
    let mut hasher = Sha256::new();
    let mut buf = vec![0u8; 1 << 20];
    loop {
        let n = file.read(&mut buf)?;
        if n == 0 {
            break;
        }
        hasher.update(&buf[..n]);
    }
    Ok(hasher
        .finalize()
        .iter()
        .map(|b| format!("{b:02x}"))
        .collect())
}
