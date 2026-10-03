//! Validation of Auth0-issued access tokens (RS256, keys from the tenant's JWKS).

use std::{
    collections::HashMap,
    time::{Duration, Instant},
};

use jsonwebtoken::{Algorithm, DecodingKey, Validation, decode, decode_header, jwk::JwkSet};
use serde::Deserialize;
use tokio::sync::RwLock;

/// Namespace of the custom claims the clipos post-login Action adds to access tokens
/// (`infra/auth0/actions/post-login.js`).
pub const CLAIM_NAMESPACE: &str = "https://clips.spawnpoint.run/";

/// JWKS is re-fetched on an unknown `kid` at most this often, so a stream of forged
/// tokens can't turn the API into a proxy hammering Auth0.
const MIN_JWKS_REFRESH: Duration = Duration::from_secs(60);

#[derive(Debug, Clone, Deserialize)]
pub struct Claims {
    pub sub: String,
    /// When the token expires, in seconds since the epoch (always there: it's validated).
    pub exp: u64,
    #[serde(rename = "https://clips.spawnpoint.run/email")]
    pub email: Option<String>,
    #[serde(rename = "https://clips.spawnpoint.run/name")]
    pub name: Option<String>,
    #[serde(rename = "https://clips.spawnpoint.run/picture")]
    pub picture: Option<String>,
}

#[derive(Debug, thiserror::Error)]
pub enum AuthError {
    #[error("malformed token: {0}")]
    Malformed(String),
    #[error("token signed with an unknown key")]
    UnknownKey,
    #[error("invalid token: {0}")]
    Invalid(String),
    #[error("fetching signing keys failed: {0}")]
    Jwks(String),
}

pub struct JwtVerifier {
    jwks_url: String,
    validation: Validation,
    http: reqwest::Client,
    keys: RwLock<KeyCache>,
}

#[derive(Default)]
struct KeyCache {
    by_kid: HashMap<String, DecodingKey>,
    fetched_at: Option<Instant>,
}

impl JwtVerifier {
    /// `domain` is the Auth0 tenant domain (e.g. `spawnpoint.eu.auth0.com`), `audience`
    /// the API identifier tokens must be issued for.
    pub fn new(domain: &str, audience: &str) -> Self {
        Self::with_jwks_url(
            domain,
            audience,
            format!("https://{domain}/.well-known/jwks.json"),
        )
    }

    /// `new`, with the signing keys fetched from `jwks_url`.
    fn with_jwks_url(domain: &str, audience: &str, jwks_url: String) -> Self {
        let mut validation = Validation::new(Algorithm::RS256);
        validation.set_issuer(&[format!("https://{domain}/")]);
        validation.set_audience(&[audience]);
        validation.set_required_spec_claims(&["exp", "iss", "aud", "sub"]);
        // `nbf` is checked when a token has one; Auth0 doesn't send it, so it isn't required.
        validation.validate_nbf = true;
        validation.leeway = 30;

        Self {
            jwks_url,
            validation,
            http: reqwest::Client::builder()
                .timeout(Duration::from_secs(5))
                .build()
                .expect("static reqwest client config"),
            keys: RwLock::default(),
        }
    }

    pub async fn verify(&self, token: &str) -> Result<Claims, AuthError> {
        let header = decode_header(token).map_err(|e| AuthError::Malformed(e.to_string()))?;
        if header.alg != Algorithm::RS256 {
            return Err(AuthError::Invalid(format!(
                "unexpected alg {:?}",
                header.alg
            )));
        }
        let kid = header
            .kid
            .ok_or_else(|| AuthError::Malformed("missing kid".into()))?;
        let key = self.key(&kid).await?;

        decode::<Claims>(token, &key, &self.validation)
            .map(|data| data.claims)
            .map_err(|e| AuthError::Invalid(e.to_string()))
    }

    async fn key(&self, kid: &str) -> Result<DecodingKey, AuthError> {
        if let Some(key) = self.keys.read().await.by_kid.get(kid) {
            return Ok(key.clone());
        }

        let mut cache = self.keys.write().await;
        // Another request may have refreshed while we waited for the lock.
        if let Some(key) = cache.by_kid.get(kid) {
            return Ok(key.clone());
        }
        if cache
            .fetched_at
            .is_some_and(|at| at.elapsed() < MIN_JWKS_REFRESH)
        {
            return Err(AuthError::UnknownKey);
        }

        let jwks: JwkSet = self
            .http
            .get(&self.jwks_url)
            .send()
            .await
            .and_then(reqwest::Response::error_for_status)
            .map_err(|e| AuthError::Jwks(e.to_string()))?
            .json()
            .await
            .map_err(|e| AuthError::Jwks(e.to_string()))?;

        cache.by_kid = jwks
            .keys
            .iter()
            .filter_map(|jwk| {
                let kid = jwk.common.key_id.clone()?;
                let key = DecodingKey::from_jwk(jwk).ok()?;
                Some((kid, key))
            })
            .collect();
        cache.fetched_at = Some(Instant::now());
        tracing::info!(keys = cache.by_kid.len(), "refreshed JWKS");

        cache.by_kid.get(kid).cloned().ok_or(AuthError::UnknownKey)
    }

    /// Pre-loads a signing key so tests never reach out to a real JWKS endpoint.
    #[cfg(any(test, feature = "test-util"))]
    pub async fn insert_key(&self, kid: &str, key: DecodingKey) {
        let mut cache = self.keys.write().await;
        cache.by_kid.insert(kid.to_owned(), key);
        cache.fetched_at = Some(Instant::now());
    }
}

#[cfg(test)]
mod tests {
    use std::sync::{
        Arc, Mutex,
        atomic::{AtomicUsize, Ordering},
    };

    use jsonwebtoken::{EncodingKey, Header, encode, jwk::Jwk};
    use serde_json::json;

    use super::*;

    const DOMAIN: &str = "tenant.example.auth0.com";
    const AUDIENCE: &str = "http://localhost:8080/api";
    const KID: &str = "test-key";
    // Test-only keypair, generated for this suite and used nowhere else.
    const PRIVATE_PEM: &[u8] = include_bytes!("../tests/fixtures/test_rsa_private.pem");
    const PUBLIC_PEM: &[u8] = include_bytes!("../tests/fixtures/test_rsa_public.pem");

    async fn verifier() -> JwtVerifier {
        let v = JwtVerifier::new(DOMAIN, AUDIENCE);
        v.insert_key(KID, DecodingKey::from_rsa_pem(PUBLIC_PEM).unwrap())
            .await;
        v
    }

    fn token(claims: serde_json::Value, kid: &str) -> String {
        let mut header = Header::new(Algorithm::RS256);
        header.kid = Some(kid.to_owned());
        encode(
            &header,
            &claims,
            &EncodingKey::from_rsa_pem(PRIVATE_PEM).unwrap(),
        )
        .unwrap()
    }

    fn claims(overrides: serde_json::Value) -> serde_json::Value {
        let mut base = json!({
            "iss": format!("https://{DOMAIN}/"),
            "aud": [AUDIENCE, format!("https://{DOMAIN}/userinfo")],
            "sub": "google-oauth2|123",
            "exp": chrono::Utc::now().timestamp() + 600,
            "https://clips.spawnpoint.run/email": "friend@gmail.com",
            "https://clips.spawnpoint.run/name": "Friend",
        });
        base.as_object_mut()
            .unwrap()
            .extend(overrides.as_object().unwrap().clone());
        base
    }

    #[tokio::test]
    async fn accepts_valid_token_and_reads_custom_claims() {
        let c = verifier()
            .await
            .verify(&token(claims(json!({})), KID))
            .await
            .unwrap();
        assert_eq!(c.sub, "google-oauth2|123");
        assert_eq!(c.email.as_deref(), Some("friend@gmail.com"));
        assert_eq!(c.name.as_deref(), Some("Friend"));
        assert_eq!(c.picture, None);
    }

    #[tokio::test]
    async fn rejects_wrong_audience() {
        let t = token(
            claims(json!({ "aud": "https://clips.spawnpoint.run/api" })),
            KID,
        );
        assert!(matches!(
            verifier().await.verify(&t).await,
            Err(AuthError::Invalid(_))
        ));
    }

    #[tokio::test]
    async fn rejects_wrong_issuer() {
        let t = token(claims(json!({ "iss": "https://evil.example.com/" })), KID);
        assert!(matches!(
            verifier().await.verify(&t).await,
            Err(AuthError::Invalid(_))
        ));
    }

    #[tokio::test]
    async fn rejects_expired_token() {
        let t = token(
            claims(json!({ "exp": chrono::Utc::now().timestamp() - 120 })),
            KID,
        );
        assert!(matches!(
            verifier().await.verify(&t).await,
            Err(AuthError::Invalid(_))
        ));
    }

    #[tokio::test]
    async fn unknown_kid_does_not_refetch_within_interval() {
        let t = token(claims(json!({})), "other-key");
        assert!(matches!(
            verifier().await.verify(&t).await,
            Err(AuthError::UnknownKey)
        ));
    }

    #[tokio::test]
    async fn rejects_hs256_token() {
        let mut header = Header::new(Algorithm::HS256);
        header.kid = Some(KID.to_owned());
        let t = encode(
            &header,
            &claims(json!({})),
            &EncodingKey::from_secret(b"guess"),
        )
        .unwrap();
        assert!(matches!(
            verifier().await.verify(&t).await,
            Err(AuthError::Invalid(_))
        ));
    }

    #[tokio::test]
    async fn rejects_token_without_kid() {
        let t = encode(
            &Header::new(Algorithm::RS256),
            &claims(json!({})),
            &EncodingKey::from_rsa_pem(PRIVATE_PEM).unwrap(),
        )
        .unwrap();
        assert!(matches!(
            verifier().await.verify(&t).await,
            Err(AuthError::Malformed(m)) if m == "missing kid"
        ));
    }

    #[tokio::test]
    async fn allows_for_clock_skew_on_expiry() {
        let t = token(
            claims(json!({ "exp": chrono::Utc::now().timestamp() - 10 })),
            KID,
        );
        assert!(verifier().await.verify(&t).await.is_ok());
    }

    #[tokio::test]
    async fn rejects_token_not_yet_valid() {
        let t = token(
            claims(json!({ "nbf": chrono::Utc::now().timestamp() + 120 })),
            KID,
        );
        assert!(matches!(
            verifier().await.verify(&t).await,
            Err(AuthError::Invalid(m)) if m == "ImmatureSignature"
        ));
    }

    #[tokio::test]
    async fn accepts_token_valid_since_the_past() {
        let t = token(
            claims(json!({ "nbf": chrono::Utc::now().timestamp() - 60 })),
            KID,
        );
        assert!(verifier().await.verify(&t).await.is_ok());
    }

    #[tokio::test]
    async fn accepts_token_without_nbf() {
        // Auth0's tokens don't carry `nbf`.
        let t = token(claims(json!({})), KID);
        assert!(verifier().await.verify(&t).await.is_ok());
    }

    #[tokio::test]
    async fn allows_for_clock_skew_on_nbf() {
        let t = token(
            claims(json!({ "nbf": chrono::Utc::now().timestamp() + 10 })),
            KID,
        );
        assert!(verifier().await.verify(&t).await.is_ok());
    }

    /// A stand-in for the tenant's `/.well-known/jwks.json`: answers with `status` and
    /// `body`, counting requests.
    #[derive(Clone)]
    struct Jwks {
        response: Arc<Mutex<(axum::http::StatusCode, String)>>,
        hits: Arc<AtomicUsize>,
        url: String,
    }

    impl Jwks {
        async fn start() -> Self {
            let response = Arc::new(Mutex::new((axum::http::StatusCode::OK, String::new())));
            let hits = Arc::new(AtomicUsize::new(0));
            let (r, h) = (response.clone(), hits.clone());
            let app = axum::Router::new().route(
                "/.well-known/jwks.json",
                axum::routing::get(move || async move {
                    h.fetch_add(1, Ordering::SeqCst);
                    r.lock().unwrap().clone()
                }),
            );
            let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
            let addr = listener.local_addr().unwrap();
            tokio::spawn(async move { axum::serve(listener, app).await.unwrap() });
            Self {
                response,
                hits,
                url: format!("http://{addr}/.well-known/jwks.json"),
            }
        }

        /// Serves the test key under each of `kids`, plus a key without a kid.
        fn serve_keys(&self, kids: &[&str]) {
            let jwk = |kid: Option<&str>| {
                let mut jwk = Jwk::from_encoding_key(
                    &EncodingKey::from_rsa_pem(PRIVATE_PEM).unwrap(),
                    Algorithm::RS256,
                )
                .unwrap();
                jwk.common.key_id = kid.map(str::to_owned);
                jwk
            };
            let mut keys: Vec<Jwk> = kids.iter().map(|k| jwk(Some(k))).collect();
            keys.push(jwk(None));
            self.respond(
                axum::http::StatusCode::OK,
                &serde_json::to_string(&JwkSet { keys }).unwrap(),
            );
        }

        fn respond(&self, status: axum::http::StatusCode, body: &str) {
            *self.response.lock().unwrap() = (status, body.to_owned());
        }

        fn hits(&self) -> usize {
            self.hits.load(Ordering::SeqCst)
        }

        fn verifier(&self) -> JwtVerifier {
            JwtVerifier::with_jwks_url(DOMAIN, AUDIENCE, self.url.clone())
        }
    }

    /// Pretends the last JWKS fetch was longer ago than `MIN_JWKS_REFRESH`.
    async fn age_jwks(v: &JwtVerifier) {
        v.keys.write().await.fetched_at = Instant::now().checked_sub(MIN_JWKS_REFRESH * 2);
    }

    #[test]
    fn keys_come_from_the_tenant() {
        assert_eq!(
            JwtVerifier::new(DOMAIN, AUDIENCE).jwks_url,
            "https://tenant.example.auth0.com/.well-known/jwks.json"
        );
    }

    #[tokio::test]
    async fn fetches_signing_keys_once_and_caches_them() {
        let jwks = Jwks::start().await;
        jwks.serve_keys(&[KID]);
        let v = jwks.verifier();

        let c = v.verify(&token(claims(json!({})), KID)).await.unwrap();
        assert_eq!(c.sub, "google-oauth2|123");
        assert_eq!(jwks.hits(), 1);
        // The key without a kid can't be looked up, so it isn't kept.
        assert_eq!(v.keys.read().await.by_kid.len(), 1);

        v.verify(&token(claims(json!({})), KID)).await.unwrap();
        assert_eq!(jwks.hits(), 1, "cached");

        // An unknown kid right after a fetch doesn't fetch again.
        assert!(matches!(
            v.verify(&token(claims(json!({})), "forged")).await,
            Err(AuthError::UnknownKey)
        ));
        assert_eq!(jwks.hits(), 1);
    }

    #[tokio::test]
    async fn picks_up_rotated_keys_after_the_refresh_interval() {
        let jwks = Jwks::start().await;
        jwks.serve_keys(&["old"]);
        let v = jwks.verifier();
        v.verify(&token(claims(json!({})), "old")).await.unwrap();

        jwks.serve_keys(&["new"]);
        let rotated = token(claims(json!({})), "new");
        assert!(matches!(
            v.verify(&rotated).await,
            Err(AuthError::UnknownKey)
        ));
        assert_eq!(jwks.hits(), 1);

        age_jwks(&v).await;
        v.verify(&rotated).await.unwrap();
        assert_eq!(jwks.hits(), 2);
        // The refresh replaced the keys: the retired one is gone.
        assert!(matches!(
            v.verify(&token(claims(json!({})), "old")).await,
            Err(AuthError::UnknownKey)
        ));

        // A refresh that still doesn't have the kid is an unknown key too.
        age_jwks(&v).await;
        assert!(matches!(
            v.verify(&token(claims(json!({})), "never")).await,
            Err(AuthError::UnknownKey)
        ));
        assert_eq!(jwks.hits(), 3);
    }

    #[tokio::test]
    async fn jwks_failures_are_reported_and_retried() {
        let jwks = Jwks::start().await;
        let v = jwks.verifier();
        let t = token(claims(json!({})), KID);

        jwks.respond(axum::http::StatusCode::SERVICE_UNAVAILABLE, "down");
        assert!(matches!(
            v.verify(&t).await,
            Err(AuthError::Jwks(m)) if m.contains("503")
        ));

        jwks.respond(axum::http::StatusCode::OK, "<html>not jwks</html>");
        assert!(matches!(v.verify(&t).await, Err(AuthError::Jwks(_))));

        // A failed fetch doesn't count against the refresh interval.
        jwks.serve_keys(&[KID]);
        v.verify(&t).await.unwrap();
        assert_eq!(jwks.hits(), 3);
    }

    #[tokio::test]
    async fn unreachable_jwks_is_reported() {
        let v = JwtVerifier::with_jwks_url(DOMAIN, AUDIENCE, "http://127.0.0.1:9/jwks".into());
        assert!(matches!(
            v.verify(&token(claims(json!({})), KID)).await,
            Err(AuthError::Jwks(_))
        ));
    }

    #[tokio::test]
    async fn rejects_garbage() {
        assert!(matches!(
            verifier().await.verify("not.a.jwt").await,
            Err(AuthError::Malformed(_))
        ));
    }
}
