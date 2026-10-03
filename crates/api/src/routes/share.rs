//! Public share links.
//!
//! - `POST/DELETE /api/clips/{id}/share` (uploader or admin) turn a clip's link on/off.
//! - `GET /s/{token}` (no sign-in) is the SPA's `index.html` with Open Graph tags, so a
//!   pasted link unfurls in Discord (inline video) and WhatsApp (preview image).
//! - `GET /s/{token}/video.mp4` and `/poster.jpg` stream the file from Blob Storage
//!   (with byte ranges), so the URLs in an embed never expire. Not redirects: Discord's
//!   unfurler doesn't reliably follow them and then shows no embed at all.
//! - `GET /s/{token}/clip.json` feeds the SPA's public player.
//!
//! A revoked link or a deleted clip turns every one of these into a 404.

use axum::{
    body::Body,
    extract::State,
    http::{HeaderMap, HeaderValue, Method, StatusCode, header},
    response::{Html, IntoResponse, Response},
};
use clipos_core::{
    shares::{self, SharedClip},
    storage::Container,
};
use futures_util::TryStreamExt;
use serde::Serialize;
use uuid::Uuid;

use crate::{
    AppState, ErrorBody,
    error::ApiError,
    extract::{CurrentUser, Json, Path},
    routes::clips::{ClipView, editable_clip, editable_ready_clip, reload},
};

/// Turn on the clip's public link (or get the existing one).
#[utoipa::path(
    post,
    path = "/clips/{id}/share",
    tag = "clips",
    security(("bearer" = [])),
    params(("id" = Uuid, Path, description = "Clip id")),
    responses(
        (status = 200, description = "Clip with its `shareUrl`", body = ClipView),
        (status = 400, description = "Not ready yet", body = ErrorBody),
        (status = 403, description = "Not yours", body = ErrorBody),
    )
)]
pub async fn share_clip(
    State(state): State<AppState>,
    CurrentUser(user): CurrentUser,
    Path(id): Path<Uuid>,
) -> Result<Json<ClipView>, ApiError> {
    editable_ready_clip(&state, id, &user).await?;
    shares::share(&state.pool, id, user.id).await?;
    tracing::info!(clip_id = %id, by = %user.handle, "share link on");
    reload(&state, id, &user).await
}

/// Revoke the clip's public link; anything already pasted stops working. Also for a clip
/// in the trash.
#[utoipa::path(
    delete,
    path = "/clips/{id}/share",
    tag = "clips",
    security(("bearer" = [])),
    params(("id" = Uuid, Path, description = "Clip id")),
    responses(
        (status = 200, description = "Clip without a link", body = ClipView),
        (status = 403, description = "Not yours", body = ErrorBody),
    )
)]
pub async fn unshare_clip(
    State(state): State<AppState>,
    CurrentUser(user): CurrentUser,
    Path(id): Path<Uuid>,
) -> Result<Json<ClipView>, ApiError> {
    editable_clip(&state, id, &user).await?;
    shares::revoke(&state.pool, id).await?;
    tracing::info!(clip_id = %id, by = %user.handle, "share link revoked");
    reload(&state, id, &user).await
}

/// `{public_url}/s/{token}`.
pub fn share_url(state: &AppState, token: &str) -> String {
    format!("{}/s/{token}", state.public_url)
}

async fn shared(state: &AppState, token: &str) -> Result<SharedClip, ApiError> {
    shares::resolve(&state.pool, token)
        .await?
        .ok_or(ApiError::NotFound)
}

/// Response headers worth passing through from Blob Storage.
const PASS_THROUGH: [header::HeaderName; 6] = [
    header::CONTENT_TYPE,
    header::CONTENT_LENGTH,
    header::CONTENT_RANGE,
    header::ACCEPT_RANGES,
    header::ETAG,
    header::LAST_MODIFIED,
];

/// The `Range` headers we serve, the ones players send: one span, `bytes=a-b` (a ≤ b)
/// or `bytes=a-`, passed on as is; or the last n bytes, `bytes=-n` (n > 0).
#[derive(Debug, PartialEq)]
enum ByteRange<'a> {
    Span(&'a str),
    Last(u64),
}

fn byte_range(raw: &str) -> Option<ByteRange<'_>> {
    let (first, last) = raw.strip_prefix("bytes=")?.split_once('-')?;
    let number = |s: &str| {
        (!s.is_empty() && s.bytes().all(|b| b.is_ascii_digit()))
            .then(|| s.parse::<u64>().ok())
            .flatten()
    };
    match (number(first), number(last)) {
        (Some(a), Some(b)) if a <= b => Some(ByteRange::Span(raw)),
        (Some(_), None) if last.is_empty() => Some(ByteRange::Span(raw)),
        (None, Some(n)) if first.is_empty() && n > 0 => Some(ByteRange::Last(n)),
        _ => None,
    }
}

/// Streams a blob to the client, honouring `Range` so players can seek. Media bytes go
/// through the api only for public share links (decision 26). Any other `Range` gets a
/// 416.
async fn stream(
    state: &AppState,
    container: Container,
    blob: &str,
    method: &Method,
    headers: &HeaderMap,
) -> Result<Response, ApiError> {
    let unsatisfiable = || Ok(StatusCode::RANGE_NOT_SATISFIABLE.into_response());
    let range = match headers
        .get(header::RANGE)
        .map(|v| v.to_str().ok().and_then(byte_range))
    {
        None => None,
        Some(None) => return unsatisfiable(),
        Some(Some(ByteRange::Span(span))) => Some(span.to_owned()),
        // Blob Storage takes no `bytes=-n`: ask from where the last n bytes start.
        Some(Some(ByteRange::Last(n))) => match state.storage.blob_size(container, blob).await? {
            None => return Err(ApiError::NotFound),
            Some(0) => return unsatisfiable(),
            Some(size) => Some(format!("bytes={}-", size.saturating_sub(n))),
        },
    };
    let upstream = state
        .storage
        .fetch(
            container,
            blob,
            *method == Method::HEAD,
            range.as_deref(),
            None,
        )
        .await?;
    let status = StatusCode::from_u16(upstream.status().as_u16()).map_err(anyhow::Error::from)?;
    if status == StatusCode::NOT_FOUND {
        return Err(ApiError::NotFound);
    }
    // 416: a range past the end of the file, which the player asked for.
    if !(status.is_success() || status == StatusCode::RANGE_NOT_SATISFIABLE) {
        // A 400 is most likely about the range we passed on; anything else is on our side.
        if status == StatusCode::BAD_REQUEST {
            tracing::warn!(%status, blob, ?range, "blob storage refused a share read");
        } else {
            tracing::error!(%status, blob, "blob storage failed a share read");
        }
        return Err(ApiError::BadGateway);
    }

    let mut res = Response::builder().status(status);
    for name in PASS_THROUGH {
        if let Some(value) = upstream.headers().get(name.as_str())
            && let Ok(value) = HeaderValue::from_bytes(value.as_bytes())
        {
            res = res.header(name, value);
        }
    }
    // The link can be revoked at any moment; keep shared caches short.
    res = res.header(header::CACHE_CONTROL, "public, max-age=300");
    let body = if *method == Method::HEAD {
        Body::empty()
    } else {
        // Without the URL, its SAS stays out of any error that's logged.
        Body::from_stream(upstream.bytes_stream().map_err(|e| e.without_url()))
    };
    res.body(body).map_err(|e| anyhow::Error::from(e).into())
}

pub async fn video(
    State(state): State<AppState>,
    Path(token): Path<String>,
    method: Method,
    headers: HeaderMap,
) -> Result<Response, ApiError> {
    let clip = shared(&state, &token).await?;
    stream(
        &state,
        Container::Playback,
        &clip.playback_blob,
        &method,
        &headers,
    )
    .await
}

pub async fn poster(
    State(state): State<AppState>,
    Path(token): Path<String>,
    method: Method,
    headers: HeaderMap,
) -> Result<Response, ApiError> {
    let clip = shared(&state, &token).await?;
    stream(
        &state,
        Container::Posters,
        &clip.poster_blob,
        &method,
        &headers,
    )
    .await
}

/// What the public player shows. Media URLs are the streaming ones, so they never expire.
#[derive(Debug, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct PublicClip {
    pub title: String,
    pub uploader: String,
    pub map: Option<String>,
    pub duration_ms: Option<i32>,
    pub width: Option<i32>,
    pub height: Option<i32>,
    pub fps: Option<f32>,
    pub video_url: String,
    pub poster_url: String,
    pub created_at: chrono::DateTime<chrono::Utc>,
}

pub async fn clip_json(
    State(state): State<AppState>,
    Path(token): Path<String>,
) -> Result<Json<PublicClip>, ApiError> {
    let clip = shared(&state, &token).await?;
    let base = share_url(&state, &token);
    Ok(Json(PublicClip {
        title: clip.title,
        uploader: clip.uploader,
        map: clip.map,
        duration_ms: clip.duration_ms,
        width: clip.width,
        height: clip.height,
        fps: clip.fps,
        video_url: format!("{base}/video.mp4"),
        poster_url: format!("{base}/poster.jpg"),
        created_at: clip.created_at,
    }))
}

/// The SPA shell with link-preview tags. Unknown or revoked links get the plain shell
/// with a 404, and the SPA says the link has expired.
pub async fn page(State(state): State<AppState>, Path(token): Path<String>) -> Response {
    let shell = state.index_html.as_str();
    match shares::resolve(&state.pool, &token).await {
        Ok(Some(clip)) => {
            let mut res = Html(with_meta(
                shell,
                &og_tags(&state, &token, &clip),
                &clip.title,
            ))
            .into_response();
            res.headers_mut()
                .insert(header::CACHE_CONTROL, HeaderValue::from_static("no-cache"));
            res
        }
        Ok(None) => (StatusCode::NOT_FOUND, Html(shell.to_owned())).into_response(),
        Err(e) => {
            tracing::error!(error = %e, "resolving a share link failed");
            (StatusCode::INTERNAL_SERVER_ERROR, Html(shell.to_owned())).into_response()
        }
    }
}

fn og_tags(state: &AppState, token: &str, clip: &SharedClip) -> String {
    let url = share_url(state, token);
    let (w, h) = (clip.width.unwrap_or(1920), clip.height.unwrap_or(1080));
    // The poster is scaled to at most 720 px high (worker), width kept even.
    let poster_h = h.min(720);
    let poster_w = (i64::from(w) * i64::from(poster_h) / i64::from(h.max(1))) / 2 * 2;
    let mut details = vec![format!("Shared by {}", clip.uploader)];
    if let Some(map) = &clip.map {
        details.push(map.clone());
    }
    if let Some(ms) = clip.duration_ms {
        let s = ms / 1000;
        details.push(format!("{}:{:02}", s / 60, s % 60));
    }
    let description = format!("{} · clipos", details.join(" · "));

    let tags = [
        ("property", "og:site_name", "clipos".to_owned()),
        ("property", "og:type", "video.other".to_owned()),
        ("property", "og:title", clip.title.clone()),
        ("property", "og:description", description.clone()),
        ("property", "og:url", url.clone()),
        ("property", "og:image", format!("{url}/poster.jpg")),
        ("property", "og:image:width", poster_w.to_string()),
        ("property", "og:image:height", poster_h.to_string()),
        ("property", "og:video", format!("{url}/video.mp4")),
        (
            "property",
            "og:video:secure_url",
            format!("{url}/video.mp4"),
        ),
        ("property", "og:video:type", "video/mp4".to_owned()),
        ("property", "og:video:width", w.to_string()),
        ("property", "og:video:height", h.to_string()),
        // Discord only plays og:video inline next to a `player` card with a raw stream.
        ("name", "twitter:card", "player".to_owned()),
        ("name", "twitter:title", clip.title.clone()),
        ("name", "twitter:description", description),
        ("name", "twitter:image", format!("{url}/poster.jpg")),
        ("name", "twitter:player", url.clone()),
        ("name", "twitter:player:width", w.to_string()),
        ("name", "twitter:player:height", h.to_string()),
        ("name", "twitter:player:stream", format!("{url}/video.mp4")),
        (
            "name",
            "twitter:player:stream:content_type",
            "video/mp4".to_owned(),
        ),
        ("name", "robots", "noindex".to_owned()),
    ];
    tags.iter()
        .map(|(attr, key, value)| format!("<meta {attr}=\"{key}\" content=\"{}\">", escape(value)))
        .collect::<Vec<_>>()
        .join("\n    ")
}

/// Puts `meta` before `</head>` and sets the `<title>`.
fn with_meta(shell: &str, meta: &str, title: &str) -> String {
    let title_tag = format!("<title>{} · clipos</title>", escape(title));
    let mut html = match (shell.find("<title>"), shell.find("</title>")) {
        (Some(start), Some(end)) if end > start => {
            format!(
                "{}{}{}",
                &shell[..start],
                title_tag,
                &shell[end + "</title>".len()..]
            )
        }
        _ => shell.to_owned(),
    };
    match html.find("</head>") {
        Some(at) => html.insert_str(at, &format!("    {meta}\n  ")),
        None => html.insert_str(0, meta),
    }
    html
}

fn escape(s: &str) -> String {
    s.replace('&', "&amp;")
        .replace('"', "&quot;")
        .replace('<', "&lt;")
        .replace('>', "&gt;")
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn serves_single_byte_ranges_only() {
        for span in ["bytes=0-0", "bytes=0-99", "bytes=100-"] {
            assert_eq!(byte_range(span), Some(ByteRange::Span(span)));
        }
        assert_eq!(byte_range("bytes=-500"), Some(ByteRange::Last(500)));
        for bad in [
            "bytes=abc",
            "bytes=10-5",
            "bytes=0-0,5-6",
            "items=0-1",
            "bytes=-0",
            "bytes=-",
            "bytes=1-2-3",
            "bytes= 0-1",
            "bytes=+1-2",
            "bytes=99999999999999999999-",
            "",
        ] {
            assert_eq!(byte_range(bad), None, "{bad}");
        }
    }

    #[test]
    fn injects_meta_and_title() {
        let shell = "<!doctype html><html><head><meta charset=\"utf-8\"><title>clipos</title></head><body></body></html>";
        let html = with_meta(
            shell,
            "<meta property=\"og:title\" content=\"x\">",
            "Ace <on> \"A\"",
        );
        assert!(html.contains("<title>Ace &lt;on&gt; &quot;A&quot; · clipos</title>"));
        assert!(html.contains("<meta property=\"og:title\" content=\"x\">\n  </head>"));
        assert_eq!(html.matches("<title>").count(), 1);
    }

    #[test]
    fn injects_meta_into_a_bare_shell() {
        let meta = "<meta property=\"og:title\" content=\"x\">";
        // No <title> to replace and no </head> to insert before: the tags go first, and
        // the shell is otherwise kept as it is.
        let html = with_meta("<p>clipos</p>", meta, "Ace");
        assert_eq!(html, format!("{meta}<p>clipos</p>"));
        // A </title> before any <title> isn't a title.
        let shell = "</title><title></head>";
        let html = with_meta(shell, meta, "Ace");
        assert_eq!(html, format!("</title><title>    {meta}\n  </head>"));
    }
}
