//! Security headers on every response (phase 7's security pass, done in S1 of the
//! redesign). The content security policy lists exactly what the SPA loads: its own
//! scripts, styles, fonts and icons (fonts are bundled, decision 40), Auth0 for tokens,
//! Blob Storage for uploads, posters and playback, and Google profile pictures.

use axum::{
    extract::Request,
    http::{HeaderMap, HeaderName, HeaderValue, header},
    middleware::Next,
    response::Response,
};

#[derive(Clone)]
pub struct SecurityHeaders {
    app_csp: HeaderValue,
    share_csp: HeaderValue,
    hsts: bool,
}

impl SecurityHeaders {
    /// `auth0_domain` like `spawnpoint.eu.auth0.com`; `blob_origin` like
    /// `https://acct.blob.core.windows.net`; `public_url` decides HSTS (https only).
    pub fn new(auth0_domain: &str, blob_origin: &str, public_url: &str) -> Self {
        // The show's live connection: 'self' covers it in current browsers, older Safari
        // wants the ws(s) origin spelled out.
        let ws = public_url
            .trim_end_matches('/')
            .replacen("https://", "wss://", 1)
            .replacen("http://", "ws://", 1);
        let app = policy(auth0_domain, blob_origin, &ws, "'none'");
        // X (Twitter) shows a share link's `twitter:player` card by framing the share page,
        // so that one page may be framed anywhere. Everything else may not.
        let share = policy(auth0_domain, blob_origin, &ws, "*");
        Self {
            app_csp: HeaderValue::from_str(&app).expect("ASCII policy"),
            share_csp: HeaderValue::from_str(&share).expect("ASCII policy"),
            hsts: public_url.starts_with("https://"),
        }
    }

    fn apply(&self, path: &str, headers: &mut HeaderMap) {
        let share_page = path.starts_with("/s/") && path.matches('/').count() == 2;
        let csp = if share_page {
            &self.share_csp
        } else {
            &self.app_csp
        };
        headers.insert(header::CONTENT_SECURITY_POLICY, csp.clone());
        if !share_page {
            headers.insert(header::X_FRAME_OPTIONS, HeaderValue::from_static("DENY"));
        }
        headers.insert(
            header::X_CONTENT_TYPE_OPTIONS,
            HeaderValue::from_static("nosniff"),
        );
        headers.insert(
            header::REFERRER_POLICY,
            HeaderValue::from_static("strict-origin-when-cross-origin"),
        );
        headers.insert(
            HeaderName::from_static("permissions-policy"),
            HeaderValue::from_static(
                "camera=(), microphone=(), geolocation=(), payment=(), usb=(), \
                 interest-cohort=()",
            ),
        );
        headers.insert(
            HeaderName::from_static("cross-origin-opener-policy"),
            HeaderValue::from_static("same-origin"),
        );
        if self.hsts {
            headers.insert(
                header::STRICT_TRANSPORT_SECURITY,
                HeaderValue::from_static("max-age=31536000; includeSubDomains"),
            );
        }
    }
}

fn policy(auth0_domain: &str, blob: &str, ws: &str, frame_ancestors: &str) -> String {
    let auth0 = format!("https://{auth0_domain}");
    [
        "default-src 'self'".to_owned(),
        "script-src 'self'".to_owned(),
        // Media Chrome styles its controls with <style> in shadow roots, and React and
        // Tailwind set style attributes.
        "style-src 'self' 'unsafe-inline'".to_owned(),
        format!(
            "img-src 'self' data: blob: {blob} https://*.googleusercontent.com https://s.gravatar.com https://cdn.auth0.com"
        ),
        format!("media-src 'self' blob: {blob}"),
        "font-src 'self'".to_owned(),
        format!("connect-src 'self' {ws} {auth0} {blob}"),
        "worker-src 'self' blob:".to_owned(),
        // Auth0's SDK only frames its own domain, and only as a fallback.
        format!("frame-src {auth0}"),
        format!("frame-ancestors {frame_ancestors}"),
        "object-src 'none'".to_owned(),
        "base-uri 'self'".to_owned(),
        "form-action 'self'".to_owned(),
    ]
    .join("; ")
}

pub async fn layer(
    axum::extract::State(headers): axum::extract::State<SecurityHeaders>,
    request: Request,
    next: Next,
) -> Response {
    let path = request.uri().path().to_owned();
    let mut response = next.run(request).await;
    headers.apply(&path, response.headers_mut());
    response
}

#[cfg(test)]
mod tests {
    use super::*;

    fn headers_for(path: &str, public_url: &str) -> HeaderMap {
        let s = SecurityHeaders::new(
            "spawnpoint.eu.auth0.com",
            "https://acct.blob.core.windows.net",
            public_url,
        );
        let mut h = HeaderMap::new();
        s.apply(path, &mut h);
        h
    }

    #[test]
    fn app_pages_get_a_strict_policy() {
        let h = headers_for("/clips/123", "https://clips.spawnpoint.run");
        let csp = h[header::CONTENT_SECURITY_POLICY].to_str().unwrap();
        assert!(csp.contains("script-src 'self';"));
        assert!(csp.contains(
            "connect-src 'self' wss://clips.spawnpoint.run https://spawnpoint.eu.auth0.com https://acct.blob.core.windows.net;"
        ));
        assert!(csp.contains("media-src 'self' blob: https://acct.blob.core.windows.net;"));
        assert!(csp.contains("frame-ancestors 'none'"));
        assert_eq!(h[header::X_FRAME_OPTIONS], "DENY");
        assert_eq!(h[header::X_CONTENT_TYPE_OPTIONS], "nosniff");
        assert!(h.contains_key(header::STRICT_TRANSPORT_SECURITY));
    }

    #[test]
    fn share_pages_can_be_framed_but_their_media_cannot() {
        let page = headers_for("/s/exampleShareToken12345", "https://clips.spawnpoint.run");
        let csp = page[header::CONTENT_SECURITY_POLICY].to_str().unwrap();
        assert!(csp.contains("frame-ancestors *"));
        assert!(!page.contains_key(header::X_FRAME_OPTIONS));

        let video = headers_for(
            "/s/exampleShareToken12345/video.mp4",
            "https://clips.spawnpoint.run",
        );
        assert_eq!(video[header::X_FRAME_OPTIONS], "DENY");
    }

    #[test]
    fn no_hsts_over_plain_http() {
        let h = headers_for("/", "http://localhost:5173");
        assert!(!h.contains_key(header::STRICT_TRANSPORT_SECURITY));
    }
}
