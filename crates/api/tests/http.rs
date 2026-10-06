use std::sync::Arc;

use axum::{
    Router,
    body::Body,
    http::{Request, StatusCode, header},
};
use clipos_api::{
    AppState, PublicConfig,
    live::Timing,
    ratelimit::{RateLimiter, ShareLimits, StreamLimit},
};
use clipos_core::{
    auth::JwtVerifier,
    storage::{Container, Storage, StorageConfig},
};
use http_body_util::BodyExt;
use jsonwebtoken::{Algorithm, DecodingKey, EncodingKey, Header, encode};
use serde_json::{Value, json};
use sqlx::PgPool;
use tower::ServiceExt;

const DOMAIN: &str = "tenant.example.auth0.com";
const AUDIENCE: &str = "http://localhost:8080/api";
const KID: &str = "test-key";
const INTERNAL_SECRET: &str = "internal-test-secret";
/// Requests one client may make to share pages before being rate-limited (tests).
const SHARE_BURST: u32 = 20;
/// MiB one client may stream from share links' videos and posters, each request at least
/// 1 (tests).
const MEDIA_BURST: u32 = 16;
/// MiB one link's video and poster may stream to everyone together (tests).
const LINK_MEDIA_BURST: u32 = 24;
/// Videos and posters one client may be streaming at once (tests).
const MEDIA_STREAMS: usize = 4;
/// Azurite's well-known development account (public, not a secret).
const AZURITE_KEY: &str =
    "Eby8vdM02xNOcqFlqUwJPLlmEtlCDXJ1OUzFT50uSRZ6IFsuFq2UVErCz4I6tq/K1SZFPTOtr/KBHBeksoGMGw==";
// Test-only keypair shared with clipos-core's unit tests.
const PRIVATE_PEM: &[u8] = include_bytes!("../../core/tests/fixtures/test_rsa_private.pem");
const PUBLIC_PEM: &[u8] = include_bytes!("../../core/tests/fixtures/test_rsa_public.pem");

struct TestApp {
    router: Router,
    /// For running the show hub's housekeeping by hand.
    state: AppState,
    _static_dir: tempfile::TempDir,
}

async fn app(pool: PgPool) -> TestApp {
    app_with(pool, false).await
}

/// `shows_for_everyone`: the show is open to members, not just admins.
async fn app_with(pool: PgPool, shows_for_everyone: bool) -> TestApp {
    app_timed(pool, shows_for_everyone, Timing::default()).await
}

/// `app_with`, with the show hub's timeouts shortened.
async fn app_timed(pool: PgPool, shows_for_everyone: bool, timing: Timing) -> TestApp {
    let verifier = JwtVerifier::new(DOMAIN, AUDIENCE);
    verifier
        .insert_key(KID, DecodingKey::from_rsa_pem(PUBLIC_PEM).unwrap())
        .await;

    let static_dir = tempfile::tempdir().unwrap();
    std::fs::write(
        static_dir.path().join("index.html"),
        "<!doctype html><title>clipos</title>",
    )
    .unwrap();

    let state = AppState {
        hub: Arc::new(clipos_api::live::Hub::with_timing(pool.clone(), timing)),
        pool,
        verifier: Arc::new(verifier),
        public_config: Arc::new(PublicConfig {
            auth0_domain: DOMAIN.into(),
            auth0_client_id: "spa-client".into(),
            auth0_audience: AUDIENCE.into(),
        }),
        internal_secret: Some(Arc::from(INTERNAL_SECRET)),
        storage: Arc::new(
            Storage::new(StorageConfig {
                account: "devstoreaccount1".into(),
                blob_endpoint: Some("http://127.0.0.1:10000/devstoreaccount1".into()),
                account_key: Some(AZURITE_KEY.into()),
            })
            .unwrap(),
        ),
        public_url: Arc::from("https://clips.example"),
        index_html: Arc::new(clipos_api::FALLBACK_INDEX.to_owned()),
        // Refill at 1/min, so the bursts alone decide when tests get limited, however slow
        // the machine.
        share_limits: Arc::new(ShareLimits {
            pages: RateLimiter::new(1, SHARE_BURST),
            media: RateLimiter::new(1, MEDIA_BURST),
            link_media: RateLimiter::new(1, LINK_MEDIA_BURST),
            streams: StreamLimit::new(MEDIA_STREAMS),
        }),
        shows_for_everyone,
    };
    TestApp {
        router: clipos_api::router(state.clone(), static_dir.path()),
        state,
        _static_dir: static_dir,
    }
}

fn token(extra: Value) -> String {
    let mut claims = json!({
        "iss": format!("https://{DOMAIN}/"),
        "aud": AUDIENCE,
        "sub": "google-oauth2|42",
        "exp": chrono::Utc::now().timestamp() + 600,
    });
    claims
        .as_object_mut()
        .unwrap()
        .extend(extra.as_object().unwrap().clone());
    let mut header = Header::new(Algorithm::RS256);
    header.kid = Some(KID.into());
    encode(
        &header,
        &claims,
        &EncodingKey::from_rsa_pem(PRIVATE_PEM).unwrap(),
    )
    .unwrap()
}

async fn get(app: &TestApp, path: &str, bearer: Option<&str>) -> (StatusCode, String) {
    send(app, "GET", path, bearer, None, &[]).await
}

async fn send(
    app: &TestApp,
    method: &str,
    path: &str,
    bearer: Option<&str>,
    body: Option<Value>,
    headers: &[(&str, &str)],
) -> (StatusCode, String) {
    let mut req = Request::builder().method(method).uri(path);
    if let Some(t) = bearer {
        req = req.header(header::AUTHORIZATION, format!("Bearer {t}"));
    }
    for (name, value) in headers {
        req = req.header(*name, *value);
    }
    let body = match body {
        Some(json) => {
            req = req.header(header::CONTENT_TYPE, "application/json");
            Body::from(json.to_string())
        }
        None => Body::empty(),
    };
    let res = app
        .router
        .clone()
        .oneshot(req.body(body).unwrap())
        .await
        .unwrap();
    let status = res.status();
    let body = res.into_body().collect().await.unwrap().to_bytes();
    (status, String::from_utf8(body.to_vec()).unwrap())
}

/// Token for `sub` with the custom email claim the post-login Action adds.
fn user_token(sub: &str, email: &str) -> String {
    token(json!({ "sub": sub, "https://clips.spawnpoint.run/email": email }))
}

async fn invite(pool: &PgPool, email: &str) {
    sqlx::query("INSERT INTO invites (email) VALUES ($1)")
        .bind(email)
        .execute(pool)
        .await
        .unwrap();
}

/// Seeds `admin@gmail.com` like `ADMIN_EMAILS` does and returns its token.
async fn admin_token(pool: &PgPool) -> String {
    clipos_core::invites::seed_admins(pool, &["admin@gmail.com".into()])
        .await
        .unwrap();
    user_token("google-oauth2|admin", "admin@gmail.com")
}

fn json(body: &str) -> Value {
    serde_json::from_str(body).unwrap()
}

#[sqlx::test(migrator = "clipos_core::db::MIGRATOR")]
async fn healthz_is_ok_with_database(pool: PgPool) {
    let app = app(pool).await;
    assert_eq!(
        get(&app, "/healthz", None).await,
        (StatusCode::OK, "ok".into())
    );

    let res = app
        .router
        .clone()
        .oneshot(Request::get("/healthz").body(Body::empty()).unwrap())
        .await
        .unwrap();
    assert_eq!(res.headers()["x-clipos-version"], clipos_core::version());
}

#[sqlx::test(migrator = "clipos_core::db::MIGRATOR")]
async fn every_response_carries_security_headers(pool: PgPool) {
    let app = app(pool).await;
    for path in ["/", "/clips/whatever", "/api/me", "/healthz"] {
        let res = app
            .router
            .clone()
            .oneshot(Request::get(path).body(Body::empty()).unwrap())
            .await
            .unwrap();
        let h = res.headers();
        let csp = h["content-security-policy"].to_str().unwrap();
        assert!(csp.contains("script-src 'self'"), "{path}: {csp}");
        assert!(
            csp.contains("http://127.0.0.1:10000"),
            "{path}: blob origin missing: {csp}"
        );
        assert_eq!(h["x-content-type-options"], "nosniff", "{path}");
        assert_eq!(h["x-frame-options"], "DENY", "{path}");
        assert!(h.contains_key("strict-transport-security"), "{path}");
    }
}

#[sqlx::test(migrator = "clipos_core::db::MIGRATOR")]
async fn config_is_public(pool: PgPool) {
    let app = app(pool).await;
    let (status, body) = get(&app, "/api/config", None).await;
    assert_eq!(status, StatusCode::OK);
    assert_eq!(
        json(&body),
        json!({ "auth0Domain": DOMAIN, "auth0ClientId": "spa-client", "auth0Audience": AUDIENCE })
    );
}

#[sqlx::test(migrator = "clipos_core::db::MIGRATOR")]
async fn me_requires_a_valid_token(pool: PgPool) {
    let app = app(pool).await;

    let (status, body) = get(&app, "/api/me", None).await;
    assert_eq!(status, StatusCode::UNAUTHORIZED);
    assert_eq!(json(&body)["error"], "unauthorized");

    let (status, _) = get(&app, "/api/me", Some("garbage")).await;
    assert_eq!(status, StatusCode::UNAUTHORIZED);
}

#[sqlx::test(migrator = "clipos_core::db::MIGRATOR")]
async fn me_rejects_token_without_email_claim(pool: PgPool) {
    let app = app(pool).await;
    let (status, body) = get(&app, "/api/me", Some(&token(json!({})))).await;
    assert_eq!(status, StatusCode::FORBIDDEN);
    assert!(
        json(&body)["message"]
            .as_str()
            .unwrap()
            .contains("post-login Action")
    );
}

#[sqlx::test(migrator = "clipos_core::db::MIGRATOR")]
async fn me_creates_the_user_on_first_call(pool: PgPool) {
    invite(&pool, "robin@example.com").await;
    let app = app(pool).await;
    let t = token(json!({
        "https://clips.spawnpoint.run/email": "Robin@Example.com",
        "https://clips.spawnpoint.run/name": "Robin",
        "https://clips.spawnpoint.run/picture": "https://example.com/robin.png",
    }));

    let (status, body) = get(&app, "/api/me", Some(&t)).await;
    assert_eq!(status, StatusCode::OK, "{body}");
    let user = json(&body);
    assert_eq!(user["email"], "robin@example.com");
    assert_eq!(user["handle"], "robin");
    assert_eq!(user["displayName"], "Robin");
    assert_eq!(user["avatarUrl"], "https://example.com/robin.png");
    assert!(
        user.get("auth0Sub").is_none(),
        "internal id is never exposed"
    );

    let (_, again) = get(&app, "/api/me", Some(&t)).await;
    assert_eq!(json(&again)["id"], user["id"]);
}

#[sqlx::test(migrator = "clipos_core::db::MIGRATOR")]
async fn unknown_api_paths_are_json_404s(pool: PgPool) {
    let app = app(pool).await;
    let (status, body) = get(&app, "/api/nope", None).await;
    assert_eq!(status, StatusCode::NOT_FOUND);
    assert_eq!(json(&body)["error"], "not_found");
}

#[sqlx::test(migrator = "clipos_core::db::MIGRATOR")]
async fn client_routes_fall_back_to_the_spa(pool: PgPool) {
    let app = app(pool).await;
    for path in ["/", "/me", "/u/someone"] {
        let (status, body) = get(&app, path, None).await;
        assert_eq!(status, StatusCode::OK, "{path}");
        assert!(body.contains("<title>clipos</title>"), "{path}");
    }
}

#[sqlx::test(migrator = "clipos_core::db::MIGRATOR")]
async fn uninvited_users_are_refused(pool: PgPool) {
    let app = app(pool).await;
    let (status, body) = get(
        &app,
        "/api/me",
        Some(&user_token("google-oauth2|9", "stranger@gmail.com")),
    )
    .await;
    assert_eq!(status, StatusCode::FORBIDDEN);
    assert_eq!(json(&body)["error"], "not_invited");
}

#[sqlx::test(migrator = "clipos_core::db::MIGRATOR")]
async fn profile_can_be_edited(pool: PgPool) {
    invite(&pool, "sam@gmail.com").await;
    let app = app(pool).await;
    let t = user_token("google-oauth2|1", "sam@gmail.com");

    let (status, body) = send(
        &app,
        "PATCH",
        "/api/me",
        Some(&t),
        Some(json!({ "displayName": "Sammy", "handle": "sammy", "steamName": "s4m" })),
        &[],
    )
    .await;
    assert_eq!(status, StatusCode::OK, "{body}");
    let user = json(&body);
    assert_eq!(user["displayName"], "Sammy");
    assert_eq!(user["handle"], "sammy");
    assert_eq!(user["steamName"], "s4m");

    let (status, body) = send(
        &app,
        "PATCH",
        "/api/me",
        Some(&t),
        Some(json!({ "handle": "no spaces" })),
        &[],
    )
    .await;
    assert_eq!(status, StatusCode::BAD_REQUEST);
    assert_eq!(json(&body)["error"], "bad_request");

    // Someone else's handle.
    invite(&app.state.pool, "kim@gmail.com").await;
    let kim = user_token("google-oauth2|2", "kim@gmail.com");
    get(&app, "/api/me", Some(&kim)).await;
    let (status, body) = send(
        &app,
        "PATCH",
        "/api/me",
        Some(&t),
        Some(json!({ "handle": "kim" })),
        &[],
    )
    .await;
    assert_eq!(status, StatusCode::CONFLICT);
    assert_eq!(json(&body)["message"], "that handle is taken");
    let (_, body) = get(&app, "/api/me", Some(&t)).await;
    assert_eq!(json(&body)["handle"], "sammy");
}

#[sqlx::test(migrator = "clipos_core::db::MIGRATOR")]
async fn admin_invites_and_revokes(pool: PgPool) {
    let admin = admin_token(&pool).await;
    let app = app(pool).await;
    let friend = user_token("google-oauth2|friend", "friend@gmail.com");

    // Members can't reach admin endpoints.
    let (status, _) = get(&app, "/api/admin/invites", Some(&friend)).await;
    assert_eq!(status, StatusCode::FORBIDDEN);

    let (status, body) = send(
        &app,
        "POST",
        "/api/admin/invites",
        Some(&admin),
        Some(json!({ "email": " Friend@Gmail.com " })),
        &[],
    )
    .await;
    assert_eq!(status, StatusCode::CREATED, "{body}");
    assert_eq!(json(&body)["email"], "friend@gmail.com");
    assert_eq!(json(&body)["role"], "member");
    assert_eq!(json(&body)["invitedBy"], "admin");

    let (status, body) = get(&app, "/api/me", Some(&friend)).await;
    assert_eq!(status, StatusCode::OK, "{body}");
    assert_eq!(json(&body)["role"], "member");

    let (status, body) = get(&app, "/api/admin/invites", Some(&admin)).await;
    assert_eq!(status, StatusCode::OK);
    let invites = json(&body);
    let friend_invite = invites
        .as_array()
        .unwrap()
        .iter()
        .find(|i| i["email"] == "friend@gmail.com")
        .unwrap();
    assert!(friend_invite["acceptedAt"].is_string());

    let (status, _) = send(
        &app,
        "POST",
        "/api/admin/invites/revoke",
        Some(&admin),
        Some(json!({ "email": "friend@gmail.com" })),
        &[],
    )
    .await;
    assert_eq!(status, StatusCode::OK);
    let (status, body) = get(&app, "/api/me", Some(&friend)).await;
    assert_eq!(status, StatusCode::FORBIDDEN);
    assert_eq!(json(&body)["error"], "not_invited");

    let (status, _) = send(
        &app,
        "POST",
        "/api/admin/invites",
        Some(&admin),
        Some(json!({ "email": "not-an-email" })),
        &[],
    )
    .await;
    assert_eq!(status, StatusCode::BAD_REQUEST);

    let (status, _) = send(
        &app,
        "POST",
        "/api/admin/invites/revoke",
        Some(&admin),
        Some(json!({ "email": "admin@gmail.com" })),
        &[],
    )
    .await;
    assert_eq!(
        status,
        StatusCode::BAD_REQUEST,
        "admins can't revoke themselves"
    );
}

#[sqlx::test(migrator = "clipos_core::db::MIGRATOR")]
async fn admin_disables_and_promotes_users(pool: PgPool) {
    let admin = admin_token(&pool).await;
    invite(&pool, "friend@gmail.com").await;
    let app = app(pool).await;
    let friend = user_token("google-oauth2|friend", "friend@gmail.com");
    let (_, body) = get(&app, "/api/me", Some(&friend)).await;
    let friend_id = json(&body)["id"].as_str().unwrap().to_owned();
    let (_, body) = get(&app, "/api/me", Some(&admin)).await;
    let admin_id = json(&body)["id"].as_str().unwrap().to_owned();
    // A member, signed in and invited, still isn't an admin.
    let (status, body) = get(&app, "/api/admin/users", Some(&friend)).await;
    assert_eq!(status, StatusCode::FORBIDDEN);
    assert_eq!(json(&body)["message"], "admins only");

    let (status, body) = send(
        &app,
        "PATCH",
        &format!("/api/admin/users/{friend_id}"),
        Some(&admin),
        Some(json!({ "status": "disabled" })),
        &[],
    )
    .await;
    assert_eq!(status, StatusCode::OK, "{body}");
    let (status, body) = get(&app, "/api/me", Some(&friend)).await;
    assert_eq!(status, StatusCode::FORBIDDEN);
    assert_eq!(json(&body)["error"], "account_disabled");

    let (status, body) = send(
        &app,
        "PATCH",
        &format!("/api/admin/users/{friend_id}"),
        Some(&admin),
        Some(json!({ "status": "active", "role": "admin" })),
        &[],
    )
    .await;
    assert_eq!(status, StatusCode::OK, "{body}");
    let (status, _) = get(&app, "/api/admin/users", Some(&friend)).await;
    assert_eq!(status, StatusCode::OK, "promoted to admin");

    let (status, _) = send(
        &app,
        "PATCH",
        &format!("/api/admin/users/{admin_id}"),
        Some(&admin),
        Some(json!({ "status": "disabled" })),
        &[],
    )
    .await;
    assert_eq!(
        status,
        StatusCode::BAD_REQUEST,
        "admins can't lock themselves out"
    );
}

#[sqlx::test(migrator = "clipos_core::db::MIGRATOR")]
async fn internal_invite_check(pool: PgPool) {
    invite(&pool, "friend@gmail.com").await;
    let app = app(pool).await;
    let check = |email: &'static str, secret: &'static str| {
        let app = &app;
        async move {
            send(
                app,
                "POST",
                "/internal/invites/check",
                None,
                Some(json!({ "email": email })),
                &[("x-clipos-internal-secret", secret)],
            )
            .await
        }
    };

    let (status, body) = check("Friend@Gmail.com", INTERNAL_SECRET).await;
    assert_eq!(status, StatusCode::OK);
    assert_eq!(json(&body), json!({ "allowed": true }));

    let (status, body) = check("stranger@gmail.com", INTERNAL_SECRET).await;
    assert_eq!(status, StatusCode::OK);
    assert_eq!(json(&body), json!({ "allowed": false }));

    let (status, _) = check("friend@gmail.com", "wrong").await;
    assert_eq!(status, StatusCode::UNAUTHORIZED);

    // Not an email at all: not allowed, without asking the database.
    let (status, body) = check("not an email", INTERNAL_SECRET).await;
    assert_eq!(status, StatusCode::OK);
    assert_eq!(json(&body), json!({ "allowed": false }));

    // Without a secret configured the endpoint doesn't exist, whatever is sent.
    let state = AppState {
        internal_secret: None,
        ..app.state.clone()
    };
    let unconfigured = TestApp {
        router: clipos_api::router(state.clone(), app._static_dir.path()),
        state,
        _static_dir: tempfile::tempdir().unwrap(),
    };
    for secret in [INTERNAL_SECRET, ""] {
        let (status, _) = send(
            &unconfigured,
            "POST",
            "/internal/invites/check",
            None,
            Some(json!({ "email": "friend@gmail.com" })),
            &[("x-clipos-internal-secret", secret)],
        )
        .await;
        assert_eq!(status, StatusCode::NOT_FOUND);
    }
}

/// Upload flow against Azurite: set `CLIPOS_AZURITE` (CI and `make test`).
#[sqlx::test(migrator = "clipos_core::db::MIGRATOR")]
async fn upload_complete_and_view(pool: PgPool) {
    if std::env::var_os("CLIPOS_AZURITE").is_none() {
        eprintln!("CLIPOS_AZURITE not set; skipping");
        return;
    }
    invite(&pool, "sam@gmail.com").await;
    invite(&pool, "kim@gmail.com").await;
    let app = app(pool).await;
    let sam = user_token("google-oauth2|sam", "sam@gmail.com");
    let kim = user_token("google-oauth2|kim", "kim@gmail.com");
    get(&app, "/api/me", Some(&kim)).await;

    let (status, body) = send(
        &app,
        "POST",
        "/api/clips",
        Some(&sam),
        Some(json!({ "title": "Ace", "map": "Mirage", "filename": "ace.mp4", "bytes": 5 })),
        &[],
    )
    .await;
    assert_eq!(status, StatusCode::CREATED, "{body}");
    let created = json(&body);
    let id = created["clip"]["id"].as_str().unwrap().to_owned();
    assert_eq!(created["clip"]["status"], "uploading");
    let upload_url = created["uploadUrl"].as_str().unwrap().to_owned();
    // Create the containers like the api does at startup.
    Storage::new(StorageConfig {
        account: "devstoreaccount1".into(),
        blob_endpoint: Some("http://127.0.0.1:10000/devstoreaccount1".into()),
        account_key: Some(AZURITE_KEY.into()),
    })
    .unwrap()
    .prepare_local(&[])
    .await
    .unwrap();

    let complete = format!("/api/clips/{id}/complete");
    let (status, body) = send(&app, "POST", &complete, Some(&sam), None, &[]).await;
    assert_eq!(status, StatusCode::BAD_REQUEST);
    assert!(json(&body)["message"].as_str().unwrap().contains("nothing"));

    // The browser's upload, straight to storage with the SAS.
    let put = reqwest::Client::new()
        .put(&upload_url)
        .header("x-ms-blob-type", "BlockBlob")
        .body("hello")
        .send()
        .await
        .unwrap();
    assert!(put.status().is_success(), "{}", put.status());

    // Only the uploader can complete, and unfinished uploads are hidden from others.
    let (status, _) = send(&app, "POST", &complete, Some(&kim), None, &[]).await;
    assert_eq!(status, StatusCode::FORBIDDEN);
    let (status, _) = get(&app, &format!("/api/clips/{id}"), Some(&kim)).await;
    assert_eq!(status, StatusCode::NOT_FOUND);

    for _ in 0..2 {
        let (status, body) = send(&app, "POST", &complete, Some(&sam), None, &[]).await;
        assert_eq!(status, StatusCode::OK, "{body}");
        assert_eq!(json(&body)["status"], "processing");
    }

    let (_, body) = get(&app, "/api/clips", Some(&sam)).await;
    assert_eq!(
        json(&body)["clips"].as_array().unwrap().len(),
        1,
        "uploader sees it processing"
    );
    let (_, body) = get(&app, "/api/clips", Some(&kim)).await;
    assert!(
        json(&body)["clips"].as_array().unwrap().is_empty(),
        "others wait until it's ready"
    );
}

#[sqlx::test(migrator = "clipos_core::db::MIGRATOR")]
async fn rejects_bad_uploads(pool: PgPool) {
    invite(&pool, "sam@gmail.com").await;
    let app = app(pool).await;
    let sam = user_token("google-oauth2|sam", "sam@gmail.com");
    for body in [
        json!({ "title": "x", "filename": "clip.avi", "bytes": 10 }),
        json!({ "title": "x", "filename": "clip.mp4", "bytes": 3_000_000_000_i64 }),
        json!({ "title": "", "filename": "clip.mp4", "bytes": 10 }),
    ] {
        let (status, _) = send(
            &app,
            "POST",
            "/api/clips",
            Some(&sam),
            Some(body.clone()),
            &[],
        )
        .await;
        assert_eq!(status, StatusCode::BAD_REQUEST, "{body}");
    }
}

/// An upload `owner_email` started (`uploaded`: and completed, so it's processing),
/// created directly in the database.
async fn unpublished_clip(
    pool: &PgPool,
    owner_sub: &str,
    owner_email: &str,
    uploaded: bool,
) -> String {
    use clipos_core::{clips, users};
    let identity = users::Identity {
        sub: owner_sub,
        email: owner_email,
        name: None,
        picture: None,
    };
    let users::SignIn::Allowed(owner) = users::sign_in(pool, &identity).await.unwrap() else {
        panic!("{owner_email} isn't invited");
    };
    let clip = clips::create(
        pool,
        clips::NewClip {
            owner_id: owner.id,
            game_id: "cs2".into(),
            title: "Unpublished".into(),
            description: String::new(),
            map: None,
            my_pov: true,
            filename: "u.mp4".into(),
            bytes: 10,
        },
    )
    .await
    .unwrap();
    if uploaded {
        clips::mark_uploaded(pool, clip.id).await.unwrap();
    }
    clip.id.to_string()
}

#[sqlx::test(migrator = "clipos_core::db::MIGRATOR")]
async fn a_failed_clip_can_be_retried_by_its_uploader(pool: PgPool) {
    invite(&pool, "sam@gmail.com").await;
    invite(&pool, "kim@gmail.com").await;
    let id = unpublished_clip(&pool, "google-oauth2|sam", "sam@gmail.com", true).await;
    let app = app(pool.clone()).await;
    let sam = user_token("google-oauth2|sam", "sam@gmail.com");
    let kim = user_token("google-oauth2|kim", "kim@gmail.com");
    let retry = format!("/api/clips/{id}/retry");
    let queued = || async {
        sqlx::query_scalar::<_, i64>(
            "SELECT count(*) FROM jobs WHERE status = 'queued' AND payload->>'clipId' = $1",
        )
        .bind(&id)
        .fetch_one(&pool)
        .await
        .unwrap()
    };

    // Still processing: nothing to retry.
    let (status, body) = send(&app, "POST", &retry, Some(&sam), None, &[]).await;
    assert_eq!(status, StatusCode::BAD_REQUEST);
    assert_eq!(json(&body)["message"], "only failed clips can be retried");

    // The transcode gave up.
    sqlx::query("UPDATE jobs SET status = 'failed' WHERE payload->>'clipId' = $1")
        .bind(&id)
        .execute(&pool)
        .await
        .unwrap();
    clipos_core::clips::set_failed(&pool, id.parse().unwrap(), "ffmpeg failed: boom")
        .await
        .unwrap();
    assert_eq!(queued().await, 0);

    // Only the uploader retries it.
    let (status, _) = send(&app, "POST", &retry, Some(&kim), None, &[]).await;
    assert_eq!(status, StatusCode::FORBIDDEN);
    assert_eq!(queued().await, 0);
    let (status, body) = send(&app, "POST", &retry, Some(&sam), None, &[]).await;
    assert_eq!(status, StatusCode::OK, "{body}");
    assert_eq!(json(&body)["status"], "processing");
    assert!(json(&body)["error"].is_null());
    assert_eq!(queued().await, 1);
}

/// Actions on a clip in a state they don't apply to are refused, not half done.
#[sqlx::test(migrator = "clipos_core::db::MIGRATOR")]
async fn clip_actions_need_the_right_state(pool: PgPool) {
    invite(&pool, "sam@gmail.com").await;
    let processing = unpublished_clip(&pool, "google-oauth2|sam", "sam@gmail.com", true).await;
    let uploading = unpublished_clip(&pool, "google-oauth2|sam", "sam@gmail.com", false).await;
    let ready = ready_clip(&pool, "google-oauth2|sam", "sam@gmail.com", "Ace").await;
    let app = app(pool.clone()).await;
    let sam = user_token("google-oauth2|sam", "sam@gmail.com");
    let refused = |method: &'static str, path: String| {
        let (app, sam) = (&app, &sam);
        async move {
            let (status, body) = send(app, method, &path, Some(sam), None, &[]).await;
            assert_eq!(status, StatusCode::BAD_REQUEST, "{method} {path}: {body}");
            json(&body)["message"].as_str().unwrap().to_owned()
        }
    };

    // Not published yet: no link, no reactions.
    assert_eq!(
        refused("POST", format!("/api/clips/{processing}/share")).await,
        "only published clips can be shared"
    );
    assert_eq!(
        refused(
            "PUT",
            format!("/api/clips/{processing}/reactions/%F0%9F%94%A5")
        )
        .await,
        "you can only react to published clips"
    );
    // Not in the trash: nothing to restore.
    assert_eq!(
        refused("POST", format!("/api/clips/{ready}/restore")).await,
        "the clip isn't in the trash"
    );
    // A feed filter by a reaction nobody can give.
    assert_eq!(
        refused("GET", "/api/clips?reaction=pizza".into()).await,
        "unknown reaction"
    );
    let reacted: i64 = sqlx::query_scalar("SELECT count(*) FROM reactions")
        .fetch_one(&pool)
        .await
        .unwrap();
    assert_eq!(reacted, 0);

    // An upload that hasn't finished has nothing to download yet.
    let path = format!("/api/clips/{uploading}/download");
    let (status, _) = get(&app, &path, Some(&sam)).await;
    assert_eq!(status, StatusCode::NOT_FOUND);

    // A ready clip whose poster is missing still plays; it just has no poster.
    sqlx::query("UPDATE clips SET poster_blob = NULL WHERE id = $1::uuid")
        .bind(&ready)
        .execute(&pool)
        .await
        .unwrap();
    let (status, body) = get(&app, &format!("/api/clips/{ready}"), Some(&sam)).await;
    assert_eq!(status, StatusCode::OK, "{body}");
    assert!(json(&body)["posterUrl"].is_null());
    assert!(json(&body)["playbackUrl"].is_string());
}

/// An upload smaller than declared isn't complete yet (Azurite).
#[sqlx::test(migrator = "clipos_core::db::MIGRATOR")]
async fn an_incomplete_upload_isnt_completed(pool: PgPool) {
    let Some(storage) = azurite_storage().await else {
        return;
    };
    invite(&pool, "sam@gmail.com").await;
    let id = unpublished_clip(&pool, "google-oauth2|sam", "sam@gmail.com", false).await;
    let app = app(pool.clone()).await;
    let sam = user_token("google-oauth2|sam", "sam@gmail.com");
    let dir = tempfile::tempdir().unwrap();
    let half = dir.path().join("half.mp4");
    std::fs::write(&half, b"hello").unwrap();
    let clip = clipos_core::clips::get(&pool, id.parse().unwrap())
        .await
        .unwrap()
        .unwrap();
    storage
        .upload_file(
            Container::Originals,
            &clip.original_blob,
            &half,
            "video/mp4",
        )
        .await
        .unwrap();
    let path = format!("/api/clips/{id}/complete");
    let (status, body) = send(&app, "POST", &path, Some(&sam), None, &[]).await;
    assert_eq!(status, StatusCode::BAD_REQUEST);
    assert_eq!(json(&body)["message"], "upload incomplete: 5 of 10 bytes");
    let still = clipos_core::clips::get(&pool, id.parse().unwrap())
        .await
        .unwrap()
        .unwrap();
    assert_eq!(still.status, clipos_core::clips::ClipStatus::Uploading);
}

/// From now on the database refuses `event` (say `"INSERT ON clips"`) on rows where `when`
/// holds, the way a broken one would.
async fn refuse(pool: &PgPool, event: &str, when: &str) {
    sqlx::raw_sql(
        "CREATE OR REPLACE FUNCTION refuse() RETURNS trigger LANGUAGE plpgsql AS $$
         BEGIN RAISE EXCEPTION 'refused by the test'; END $$",
    )
    .execute(pool)
    .await
    .unwrap();
    let name = format!("refuse_{}", uuid::Uuid::new_v4().simple());
    sqlx::raw_sql(sqlx::AssertSqlSafe(format!(
        "CREATE TRIGGER {name} BEFORE {event} FOR EACH ROW WHEN ({when})
         EXECUTE FUNCTION refuse()"
    )))
    .execute(pool)
    .await
    .unwrap();
}

/// Takes `table` away, so anything reading it fails the way a broken database would.
async fn lose_table(pool: &PgPool, table: &str) {
    sqlx::raw_sql(sqlx::AssertSqlSafe(format!(
        "ALTER TABLE {table} RENAME TO {table}_lost"
    )))
    .execute(pool)
    .await
    .unwrap();
}

/// A database that fails a request is a 500 with an error body: never a wrong answer, and
/// never a request that half went through.
#[sqlx::test(migrator = "clipos_core::db::MIGRATOR")]
async fn database_failures_are_server_errors(pool: PgPool) {
    let admin = admin_token(&pool).await;
    invite(&pool, "sam@gmail.com").await;
    let clip = ready_clip(&pool, "google-oauth2|sam", "sam@gmail.com", "Ace").await;
    let app = app_with(pool.clone(), true).await;
    let sam = user_token("google-oauth2|sam", "sam@gmail.com");
    let show = new_show(&app, &admin, false).await;
    let failed = |method: &'static str, path: String, body: Option<Value>| {
        let (app, sam) = (&app, &sam);
        async move {
            let (status, text) = send(app, method, &path, Some(sam), body, &[]).await;
            assert_eq!(
                status,
                StatusCode::INTERNAL_SERVER_ERROR,
                "{method} {path}: {text}"
            );
            assert!(is_error_body(&text), "{text}");
        }
    };

    refuse(&pool, "INSERT ON clips", "true").await;
    let upload = json!({ "title": "Ace", "filename": "ace.mp4", "bytes": 5 });
    failed("POST", "/api/clips".into(), Some(upload)).await;

    refuse(&pool, "INSERT ON reactions", "true").await;
    failed(
        "PUT",
        format!("/api/clips/{clip}/reactions/%F0%9F%94%A5"),
        None,
    )
    .await;

    // Only profile edits: signing in still updates the row.
    refuse(
        &pool,
        "UPDATE ON users",
        "OLD.display_name IS DISTINCT FROM NEW.display_name",
    )
    .await;
    failed(
        "PATCH",
        "/api/me".into(),
        Some(json!({ "displayName": "Sammy" })),
    )
    .await;

    refuse(&pool, "INSERT ON show_participants", "true").await;
    failed("POST", format!("/api/shows/{show}/join"), None).await;

    lose_table(&pool, "clips").await;
    failed("GET", format!("/api/clips/{clip}"), None).await;

    // A share page that can't be looked up is still the app's shell, for it to explain.
    lose_table(&pool, "share_links").await;
    let res = raw(&app, &format!("/s/{}", "A".repeat(22)), &[]).await;
    assert_eq!(res.status(), StatusCode::INTERNAL_SERVER_ERROR);
    let body = res.into_body().collect().await.unwrap().to_bytes();
    assert_eq!(body, clipos_api::FALLBACK_INDEX.as_bytes());

    let left: (i64, i64) = sqlx::query_as(
        "SELECT (SELECT count(*) FROM reactions), (SELECT count(*) FROM show_participants)",
    )
    .fetch_one(&pool)
    .await
    .unwrap();
    assert_eq!(left, (0, 1), "only the host is in the show");
    let (_, body) = get(&app, "/api/me", Some(&sam)).await;
    assert_ne!(json(&body)["displayName"], "Sammy");
}

/// A published clip owned by `owner_email`, created directly in the database.
async fn ready_clip(pool: &PgPool, owner_sub: &str, owner_email: &str, title: &str) -> String {
    use clipos_core::{clips, users};
    let owner = match users::sign_in(
        pool,
        &users::Identity {
            sub: owner_sub,
            email: owner_email,
            name: None,
            picture: None,
        },
    )
    .await
    .unwrap()
    {
        users::SignIn::Allowed(u) => u,
        other => panic!("{other:?}"),
    };
    let clip = clips::create(
        pool,
        clips::NewClip {
            owner_id: owner.id,
            game_id: "cs2".into(),
            title: title.into(),
            description: String::new(),
            map: Some("Mirage".into()),
            my_pov: true,
            filename: "Ace Clip.mp4".into(),
            bytes: 10,
        },
    )
    .await
    .unwrap();
    clips::mark_uploaded(pool, clip.id).await.unwrap();
    clips::set_ready(
        pool,
        clip.id,
        &clips::Transcoded {
            playback_blob: clips::playback_blob(clip.id),
            poster_blob: clips::poster_blob(clip.id),
            duration_ms: 5000,
            width: 1920,
            height: 1080,
            fps: 60.0,
            metadata: json!({}),
            playback_fingerprint: clipos_core::testing::fingerprint(clip.id),
        },
    )
    .await
    .unwrap();
    clip.id.to_string()
}

#[sqlx::test(migrator = "clipos_core::db::MIGRATOR")]
async fn edit_react_delete_restore(pool: PgPool) {
    invite(&pool, "sam@gmail.com").await;
    invite(&pool, "kim@gmail.com").await;
    let id = ready_clip(&pool, "google-oauth2|sam", "sam@gmail.com", "Ace").await;
    let app = app(pool).await;
    let sam = user_token("google-oauth2|sam", "sam@gmail.com");
    let kim = user_token("google-oauth2|kim", "kim@gmail.com");
    let (_, body) = get(&app, "/api/me", Some(&kim)).await;
    let kim_id = json(&body)["id"].as_str().unwrap().to_owned();
    let path = format!("/api/clips/{id}");

    // Viewing: playback + poster links, and the viewer's permissions.
    let (status, body) = get(&app, &path, Some(&kim)).await;
    assert_eq!(status, StatusCode::OK, "{body}");
    let clip = json(&body);
    assert!(clip["playbackUrl"].as_str().unwrap().contains("/playback/"));
    assert!(clip["posterUrl"].as_str().unwrap().contains("/posters/"));
    assert_eq!(clip["canEdit"], false);

    // Only the uploader (or an admin) edits.
    let edit = json!({ "title": "Ace on A", "map": "", "tags": ["Ace", "1v3 clutch"], "players": [kim_id] });
    let (status, _) = send(&app, "PATCH", &path, Some(&kim), Some(edit.clone()), &[]).await;
    assert_eq!(status, StatusCode::FORBIDDEN);
    let (status, body) = send(&app, "PATCH", &path, Some(&sam), Some(edit), &[]).await;
    assert_eq!(status, StatusCode::OK, "{body}");
    let clip = json(&body);
    assert_eq!(clip["title"], "Ace on A");
    assert_eq!(clip["map"], Value::Null);
    assert_eq!(clip["tags"], json!(["1v3-clutch", "ace"]));
    assert_eq!(clip["players"][0]["handle"], "kim");
    assert!(
        clip["players"][0].get("email").is_none(),
        "no emails for members"
    );

    // Filters by tag and player.
    let (_, body) = get(&app, "/api/clips?tag=ace&player=kim&sort=top", Some(&sam)).await;
    assert_eq!(json(&body)["clips"].as_array().unwrap().len(), 1);
    let (status, _) = get(&app, "/api/clips?cursor=garbage", Some(&sam)).await;
    assert_eq!(status, StatusCode::BAD_REQUEST);

    // Reactions.
    let fire = format!("{path}/reactions/%F0%9F%94%A5");
    let (status, body) = send(&app, "PUT", &fire, Some(&kim), None, &[]).await;
    assert_eq!(status, StatusCode::OK, "{body}");
    assert_eq!(
        json(&body),
        json!([{ "emoji": "🔥", "count": 1, "mine": true }])
    );
    let (status, _) = send(
        &app,
        "PUT",
        &format!("{path}/reactions/pizza"),
        Some(&kim),
        None,
        &[],
    )
    .await;
    assert_eq!(status, StatusCode::BAD_REQUEST);
    let (_, body) = send(&app, "DELETE", &fire, Some(&kim), None, &[]).await;
    assert_eq!(json(&body), json!([]));

    // Download link forces the original's file name.
    let (_, body) = get(&app, &format!("{path}/download"), Some(&kim)).await;
    assert!(
        json(&body)["url"]
            .as_str()
            .unwrap()
            .contains("rscd=attachment")
    );

    // Trash: gone for others, restorable by the owner.
    let (status, _) = send(&app, "DELETE", &path, Some(&kim), None, &[]).await;
    assert_eq!(status, StatusCode::FORBIDDEN);
    let (status, body) = send(&app, "DELETE", &path, Some(&sam), None, &[]).await;
    assert_eq!(status, StatusCode::OK);
    assert!(json(&body)["deletedAt"].is_string());
    let (status, _) = get(&app, &path, Some(&kim)).await;
    assert_eq!(status, StatusCode::NOT_FOUND);
    let (_, body) = get(&app, "/api/me/trash", Some(&sam)).await;
    assert_eq!(json(&body).as_array().unwrap().len(), 1);
    let (status, body) = send(
        &app,
        "POST",
        &format!("{path}/restore"),
        Some(&sam),
        None,
        &[],
    )
    .await;
    assert_eq!(status, StatusCode::OK, "{body}");
    let (status, _) = get(&app, &path, Some(&kim)).await;
    assert_eq!(status, StatusCode::OK);
}

/// What the worker does with a new upload's original before transcoding it: keeps its
/// fingerprint, or finds the clip that has the same file.
async fn claim_original(
    pool: &PgPool,
    clip: &str,
    fp: &clipos_core::dedup::Fingerprint,
) -> Option<uuid::Uuid> {
    clipos_core::dedup::claim_original(pool, clip.parse().unwrap(), fp)
        .await
        .unwrap()
}

/// Asking whether a file is here before uploading it (decision 55): by its samples, then by
/// all of it; a copy of anyone's clip is a duplicate, one of your own in the trash can be
/// restored, and what the worker refused says which clip it copies.
#[sqlx::test(migrator = "clipos_core::db::MIGRATOR")]
async fn duplicate_uploads(pool: PgPool) {
    use clipos_core::{clips, dedup};
    invite(&pool, "sam@gmail.com").await;
    invite(&pool, "kim@gmail.com").await;
    let ace = ready_clip(&pool, "google-oauth2|sam", "sam@gmail.com", "Ace").await;
    let file = dedup::Fingerprint {
        bytes: 123_456,
        sample_hash: [1; 32],
        content_hash: [2; 32],
    };
    dedup::record(&pool, ace.parse().unwrap(), dedup::File::Original, &file)
        .await
        .unwrap();
    let app = app(pool.clone()).await;
    let sam = user_token("google-oauth2|sam", "sam@gmail.com");
    let kim = user_token("google-oauth2|kim", "kim@gmail.com");
    let check = |token: &str, body: Value| {
        let (app, token) = (&app, token.to_owned());
        async move {
            let (status, body) = send(
                app,
                "POST",
                "/api/clips/check",
                Some(&token),
                Some(body),
                &[],
            )
            .await;
            (
                status,
                if status == StatusCode::OK {
                    json(&body)
                } else {
                    json!(body)
                },
            )
        }
    };
    let sample = dedup::to_hex(&file.sample_hash);
    let content = dedup::to_hex(&file.content_hash);

    // Same size and samples: maybe. Then all of it decides.
    let (status, found) = check(&kim, json!({ "bytes": 123_456, "sampleHash": sample })).await;
    assert_eq!(status, StatusCode::OK);
    assert_eq!(found, json!({ "result": "verify", "clip": null }));
    let (_, found) = check(
        &kim,
        json!({ "bytes": 123_456, "sampleHash": sample, "contentHash": content.to_uppercase() }),
    )
    .await;
    assert_eq!(found["result"], "duplicate");
    assert_eq!(found["clip"]["id"], ace.as_str());
    assert_eq!(found["clip"]["title"], "Ace");
    let other = dedup::to_hex(&[3; 32]);
    let (_, found) = check(
        &kim,
        json!({ "bytes": 123_456, "sampleHash": sample, "contentHash": other }),
    )
    .await;
    assert_eq!(found["result"], "new");
    let (_, found) = check(&kim, json!({ "bytes": 123_457, "sampleHash": sample })).await;
    assert_eq!(found["result"], "new");

    // What isn't a size or a hash.
    for bad in [
        json!({ "bytes": 0, "sampleHash": sample }),
        json!({ "bytes": clips::MAX_BYTES + 1, "sampleHash": sample }),
        json!({ "bytes": 10, "sampleHash": "abc" }),
        json!({ "bytes": 10, "sampleHash": sample, "contentHash": "zz" }),
    ] {
        let (status, _) = check(&kim, bad.clone()).await;
        assert_eq!(status, StatusCode::BAD_REQUEST, "{bad}");
    }
    let (status, _) = check(&kim, json!({ "bytes": 10, "sampleHash": sample, "x": 1 })).await;
    assert!(status.is_client_error());
    let (status, _) = send(&app, "POST", "/api/clips/check", None, Some(json!({})), &[]).await;
    assert_eq!(status, StatusCode::UNAUTHORIZED);

    // In the trash: its uploader is offered it back; to anyone else it's new.
    let path = format!("/api/clips/{ace}");
    let (status, _) = send(&app, "DELETE", &path, Some(&sam), None, &[]).await;
    assert_eq!(status, StatusCode::OK);
    let whole = json!({ "bytes": 123_456, "sampleHash": sample, "contentHash": content });
    let (_, found) = check(&sam, whole.clone()).await;
    assert_eq!(found["result"], "inTrash");
    assert_eq!(found["clip"]["id"], ace.as_str());
    assert!(found["clip"]["deletedAt"].is_string());
    let (_, found) = check(&kim, whole.clone()).await;
    assert_eq!(found["result"], "new");

    // Kim uploads it, so it's here again: Sam's can't come back.
    let again = ready_clip(&pool, "google-oauth2|kim", "kim@gmail.com", "Kim's ace").await;
    assert_eq!(claim_original(&pool, &again, &file).await, None);
    let (status, body) = send(
        &app,
        "POST",
        &format!("{path}/restore"),
        Some(&sam),
        None,
        &[],
    )
    .await;
    assert_eq!(status, StatusCode::CONFLICT, "{body}");
    assert!(
        json(&body)["message"]
            .as_str()
            .unwrap()
            .contains("uploaded again")
    );
    let (_, found) = check(&sam, whole.clone()).await;
    assert_eq!(found["result"], "duplicate");
    assert_eq!(found["clip"]["id"], again.as_str());

    // Sam uploads it anyway: the worker refuses it, and says which clip has it.
    let refused = ready_clip(&pool, "google-oauth2|sam", "sam@gmail.com", "Copy").await;
    sqlx::query("UPDATE clips SET status = 'processing' WHERE id = $1::uuid")
        .bind(&refused)
        .execute(&pool)
        .await
        .unwrap();
    assert_eq!(
        claim_original(&pool, &refused, &file).await,
        Some(again.parse().unwrap())
    );
    clips::set_failed(&pool, refused.parse().unwrap(), clips::DUPLICATE_ERROR)
        .await
        .unwrap();
    let (_, body) = get(&app, &format!("/api/clips/{refused}"), Some(&sam)).await;
    let view = json(&body);
    assert_eq!(view["failureReason"], "duplicate");
    assert_eq!(view["duplicateOf"], again.as_str());

    // Unless that clip is one Sam can't open (still processing), nor can Sam see it in
    // the answer to a check.
    sqlx::query("UPDATE clips SET status = 'processing' WHERE id = $1::uuid")
        .bind(&again)
        .execute(&pool)
        .await
        .unwrap();
    let (_, body) = get(&app, &format!("/api/clips/{refused}"), Some(&sam)).await;
    assert_eq!(json(&body)["duplicateOf"], Value::Null);
    let (_, found) = check(&sam, whole).await;
    assert_eq!(found, json!({ "result": "duplicate", "clip": null }));
}

/// Copies uploaded before duplicates were refused are listed for admins, who delete the
/// extra ones as they'd delete any clip.
#[sqlx::test(migrator = "clipos_core::db::MIGRATOR")]
async fn admins_see_copies_already_here(pool: PgPool) {
    use clipos_core::dedup;
    invite(&pool, "sam@gmail.com").await;
    invite(&pool, "kim@gmail.com").await;
    let admin = admin_token(&pool).await;
    let first = ready_clip(&pool, "google-oauth2|sam", "sam@gmail.com", "First").await;
    let second = ready_clip(&pool, "google-oauth2|kim", "kim@gmail.com", "Second").await;
    let other = ready_clip(&pool, "google-oauth2|kim", "kim@gmail.com", "Other").await;
    let app = app(pool.clone()).await;
    let kim = user_token("google-oauth2|kim", "kim@gmail.com");

    let (status, _) = get(&app, "/api/admin/duplicates", Some(&kim)).await;
    assert_eq!(status, StatusCode::FORBIDDEN);
    let (status, body) = get(&app, "/api/admin/duplicates", Some(&admin)).await;
    assert_eq!(status, StatusCode::OK, "{body}");
    assert_eq!(json(&body), json!({ "groups": [], "unchecked": 3 }));

    // As the `fingerprint` job records them: two are the same file.
    let file = |n: u8| dedup::Fingerprint {
        bytes: 1000,
        sample_hash: [n; 32],
        content_hash: [n; 32],
    };
    for (clip, n) in [(&first, 1), (&second, 1), (&other, 2)] {
        dedup::record(
            &pool,
            clip.parse().unwrap(),
            dedup::File::Original,
            &file(n),
        )
        .await
        .unwrap();
    }
    let (_, body) = get(&app, "/api/admin/duplicates", Some(&admin)).await;
    let listed = json(&body);
    assert_eq!(listed["unchecked"], 0);
    let groups = listed["groups"].as_array().unwrap();
    assert_eq!(groups.len(), 1);
    let ids: Vec<&str> = groups[0]
        .as_array()
        .unwrap()
        .iter()
        .map(|c| c["id"].as_str().unwrap())
        .collect();
    assert_eq!(ids, [first.as_str(), second.as_str()], "oldest first");
    assert_eq!(groups[0][1]["uploader"]["handle"], "kim");
    assert_eq!(groups[0][1]["canEdit"], true);

    // The admin deletes the second copy: it's gone from the list, and Kim can't bring it
    // back while the first is here.
    let path = format!("/api/clips/{second}");
    let (status, _) = send(&app, "DELETE", &path, Some(&admin), None, &[]).await;
    assert_eq!(status, StatusCode::OK);
    let (_, body) = get(&app, "/api/admin/duplicates", Some(&admin)).await;
    assert_eq!(json(&body)["groups"], json!([]));
    let (status, _) = send(
        &app,
        "POST",
        &format!("{path}/restore"),
        Some(&kim),
        None,
        &[],
    )
    .await;
    assert_eq!(status, StatusCode::CONFLICT);
}

/// Does what the worker's `analyse` job does with the clip's queued job: claims it, stores
/// `raw` and `stats` as the clip's analysis and marks the job done.
async fn run_analysis(pool: &PgPool, clip: &str, raw: &Value, stats: &Value) {
    use clipos_core::{analysis, clips::ANALYSE_JOB, jobs};
    // `ready_clip` leaves the transcodes queued, though the clips are ready.
    sqlx::query("UPDATE jobs SET status = 'succeeded' WHERE kind <> $1")
        .bind(ANALYSE_JOB)
        .execute(pool)
        .await
        .unwrap();
    let job = jobs::claim(pool, "worker").await.unwrap().expect("a job");
    assert_eq!(job.kind, ANALYSE_JOB);
    assert_eq!(job.payload, json!({ "clipId": clip }));
    analysis::save(pool, clip.parse().unwrap(), "v1", raw, stats)
        .await
        .unwrap();
    assert!(jobs::complete(pool, &job, "worker").await.unwrap());
}

#[sqlx::test(migrator = "clipos_core::db::MIGRATOR")]
async fn killfeed_analysis(pool: PgPool) {
    use clipos_core::{clips::ANALYSE_JOB, jobs};
    invite(&pool, "sam@gmail.com").await;
    let id = ready_clip(&pool, "google-oauth2|sam", "sam@gmail.com", "Ace").await;
    let app = app(pool.clone()).await;
    let sam = user_token("google-oauth2|sam", "sam@gmail.com");
    let path = format!("/api/clips/{id}/analysis");

    // Never analysed: nothing to show.
    let (status, _) = get(&app, &path, Some(&sam)).await;
    assert_eq!(status, StatusCode::NOT_FOUND);

    // Queued: pending.
    jobs::enqueue(&pool, ANALYSE_JOB, json!({ "clipId": id }))
        .await
        .unwrap();
    let (status, body) = get(&app, &path, Some(&sam)).await;
    assert_eq!(status, StatusCode::OK, "{body}");
    assert_eq!(
        json(&body),
        json!({ "status": "pending", "stats": null, "kills": [] })
    );

    // Stored as the worker stores it, and the job done.
    let raw = json!({ "sampleFps": 1.0, "frames": 40, "kills": [
        { "t": 18.0, "last_seen": 24.0, "sightings": 7, "owner": "my_kill", "weapon": "ak47",
          "modifiers": ["through_smoke"], "example": [18.0, { "x0": 1.0, "y0": 2.0, "x1": 3.0, "y1": 4.0, "score": 0.9 }] },
        { "t": 24.0, "last_seen": 30.0, "sightings": 6, "owner": "my_death", "weapon": "usp_silencer",
          "modifiers": ["headshot"], "example": [24.0, { "x0": 1.0, "y0": 2.0, "x1": 3.0, "y1": 4.0, "score": 0.9 }] },
        { "t": 25.0, "last_seen": 31.0, "sightings": 6, "owner": "other", "weapon": null,
          "modifiers": [], "example": [25.0, { "x0": 1.0, "y0": 2.0, "x1": 3.0, "y1": 4.0, "score": 0.9 }] },
    ]});
    let stats = json!({ "kills": 3, "my_kills": 1, "my_deaths": 1, "multi_kill": "2k",
                        "weapons": { "ak47": 1 }, "modifiers": { "through_smoke": 1 } });
    run_analysis(&pool, &id, &raw, &stats).await;
    let (status, body) = get(&app, &path, Some(&sam)).await;
    assert_eq!(status, StatusCode::OK, "{body}");
    assert_eq!(
        json(&body),
        json!({
            "status": "done",
            "stats": { "kills": 3, "myKills": 1, "myDeaths": 1, "multiKill": "2k",
                       "weapons": { "ak47": 1 }, "modifiers": { "through_smoke": 1 } },
            "kills": [
                { "t": 18.0, "owner": "myKill", "weapon": "ak47", "modifiers": ["through_smoke"] },
                { "t": 24.0, "owner": "myDeath", "weapon": "usp_silencer", "modifiers": ["headshot"] },
                { "t": 25.0, "owner": "other", "weapon": null, "modifiers": [] },
            ],
        })
    );

    // Auto tags and the multi-kill on the clip and in lists; editing tags leaves them.
    let clip_path = format!("/api/clips/{id}");
    let edit = json!({ "tags": ["clutch"] });
    let (status, body) = send(&app, "PATCH", &clip_path, Some(&sam), Some(edit), &[]).await;
    assert_eq!(status, StatusCode::OK, "{body}");
    let clip = json(&body);
    assert_eq!(clip["tags"], json!(["clutch"]));
    assert_eq!(clip["autoTags"], json!(["2k", "smoke-kill"]));
    assert_eq!(clip["multiKill"], "2k");
    let (_, body) = get(&app, "/api/clips?tag=smoke-kill", Some(&sam)).await;
    let listed = &json(&body)["clips"];
    assert_eq!(listed.as_array().unwrap().len(), 1);
    assert_eq!(listed[0]["multiKill"], "2k");
}

/// An admin has a clip's killfeed read again (quality checkpoint C-01): its analysis is
/// pending until the worker is done, then the new result replaces the old one.
#[sqlx::test(migrator = "clipos_core::db::MIGRATOR")]
async fn admins_can_have_a_clip_analysed_again(pool: PgPool) {
    use clipos_core::{clips::ANALYSE_JOB, jobs};
    invite(&pool, "sam@gmail.com").await;
    let id = ready_clip(&pool, "google-oauth2|sam", "sam@gmail.com", "Ace").await;
    let admin = admin_token(&pool).await;
    let app = app(pool.clone()).await;
    let sam = user_token("google-oauth2|sam", "sam@gmail.com");
    let path = format!("/api/admin/clips/{id}/analyse");
    let analysis = format!("/api/clips/{id}/analysis");
    let queued = || async {
        let jobs: Vec<(Value, i16)> = sqlx::query_as(
            "SELECT payload, priority FROM jobs WHERE kind = $1 AND status = 'queued'",
        )
        .bind(ANALYSE_JOB)
        .fetch_all(&pool)
        .await
        .unwrap();
        jobs
    };

    // Analysed once, with the old result.
    jobs::enqueue(&pool, ANALYSE_JOB, json!({ "clipId": id }))
        .await
        .unwrap();
    let old = json!({ "kills": 2, "my_kills": 2, "my_deaths": 0, "multi_kill": "2k",
                      "weapons": { "ak47": 2 }, "modifiers": {} });
    run_analysis(&pool, &id, &json!({ "kills": [] }), &old).await;

    // Admins only.
    let (status, _) = send(&app, "POST", &path, None, None, &[]).await;
    assert_eq!(status, StatusCode::UNAUTHORIZED);
    let (status, body) = send(&app, "POST", &path, Some(&sam), None, &[]).await;
    assert_eq!(status, StatusCode::FORBIDDEN, "{body}");
    let unknown = format!("/api/admin/clips/{}/analyse", uuid::Uuid::new_v4());
    let (status, _) = send(&app, "POST", &unknown, Some(&admin), None, &[]).await;
    assert_eq!(status, StatusCode::NOT_FOUND);
    assert!(queued().await.is_empty());

    // Queued once at low priority (uploads go first), however often it's asked for.
    for _ in 0..2 {
        let (status, body) = send(&app, "POST", &path, Some(&admin), None, &[]).await;
        assert_eq!(status, StatusCode::ACCEPTED, "{body}");
        assert_eq!(json(&body), json!({ "pending": true }));
    }
    assert_eq!(
        queued().await,
        [(json!({ "clipId": id }), jobs::PRIORITY_LOW)]
    );

    // Pending until it has run, even though there's an older result.
    let (status, body) = get(&app, &analysis, Some(&sam)).await;
    assert_eq!(status, StatusCode::OK, "{body}");
    assert_eq!(
        json(&body),
        json!({ "status": "pending", "stats": null, "kills": [] })
    );
    let new = json!({ "kills": 1, "my_kills": 1, "my_deaths": 0, "multi_kill": null,
                      "weapons": { "awp": 1 }, "modifiers": { "noscope": 1 } });
    run_analysis(&pool, &id, &json!({ "kills": [] }), &new).await;
    let (status, body) = get(&app, &analysis, Some(&sam)).await;
    assert_eq!(status, StatusCode::OK, "{body}");
    let body = json(&body);
    assert_eq!(body["status"], "done");
    assert_eq!(body["stats"]["weapons"], json!({ "awp": 1 }));
    assert!(body["stats"]["multiKill"].is_null());
    let (_, body) = get(&app, &format!("/api/clips/{id}"), Some(&sam)).await;
    assert_eq!(json(&body)["autoTags"], json!(["noscope"]));

    // Only clips that are published and have a killfeed of the uploader's.
    let refused = |clip: String| {
        let (app, admin) = (&app, &admin);
        async move {
            let path = format!("/api/admin/clips/{clip}/analyse");
            let (status, body) = send(app, "POST", &path, Some(admin), None, &[]).await;
            assert_eq!(status, StatusCode::CONFLICT, "{clip}: {body}");
            json(&body)["message"].as_str().unwrap().to_owned()
        }
    };
    let processing = unpublished_clip(&pool, "google-oauth2|sam", "sam@gmail.com", true).await;
    assert_eq!(
        refused(processing).await,
        "only published clips can be analysed"
    );
    let other_view = ready_clip(&pool, "google-oauth2|sam", "sam@gmail.com", "Their ace").await;
    sqlx::query("UPDATE clips SET my_pov = false WHERE id = $1::uuid")
        .bind(&other_view)
        .execute(&pool)
        .await
        .unwrap();
    assert_eq!(
        refused(other_view).await,
        "only CS2 clips recorded from the uploader's view are analysed"
    );
    let (status, _) = send(
        &app,
        "DELETE",
        &format!("/api/clips/{id}"),
        Some(&sam),
        None,
        &[],
    )
    .await;
    assert_eq!(status, StatusCode::OK);
    assert_eq!(refused(id).await, "only published clips can be analysed");
    assert!(queued().await.is_empty());
}

#[sqlx::test(migrator = "clipos_core::db::MIGRATOR")]
async fn profiles_and_members(pool: PgPool) {
    invite(&pool, "sam@gmail.com").await;
    let id = ready_clip(&pool, "google-oauth2|sam", "sam@gmail.com", "Ace").await;
    let app = app(pool).await;
    let sam = user_token("google-oauth2|sam", "sam@gmail.com");
    let path = format!("/api/clips/{id}/reactions/%F0%9F%94%A5");
    let (status, body) = send(&app, "PUT", &path, Some(&sam), None, &[]).await;
    assert!(status.is_success(), "{status} {body}");

    let (status, body) = get(&app, "/api/users/sam", Some(&sam)).await;
    assert_eq!(status, StatusCode::OK, "{body}");
    assert_eq!(json(&body)["handle"], "sam");
    assert_eq!(json(&body)["clipCount"], 1);
    assert_eq!(json(&body)["fireCount"], 1);
    assert!(json(&body)["joinedAt"].is_string());
    assert!(json(&body).get("email").is_none());
    let (status, _) = get(&app, "/api/users/nobody", Some(&sam)).await;
    assert_eq!(status, StatusCode::NOT_FOUND);
    let (_, body) = get(&app, "/api/users", Some(&sam)).await;
    assert_eq!(json(&body).as_array().unwrap().len(), 1);
}

async fn raw(app: &TestApp, path: &str, headers: &[(&str, &str)]) -> axum::response::Response {
    let mut req = Request::get(path);
    for (k, v) in headers {
        req = req.header(*k, *v);
    }
    app.router
        .clone()
        .oneshot(req.body(Body::empty()).unwrap())
        .await
        .unwrap()
}

#[sqlx::test(migrator = "clipos_core::db::MIGRATOR")]
async fn share_links(pool: PgPool) {
    invite(&pool, "sam@gmail.com").await;
    invite(&pool, "kim@gmail.com").await;
    let id = ready_clip(
        &pool,
        "google-oauth2|sam",
        "sam@gmail.com",
        "Ace <on> \"A\"",
    )
    .await;
    let app = app(pool).await;
    let sam = user_token("google-oauth2|sam", "sam@gmail.com");
    let kim = user_token("google-oauth2|kim", "kim@gmail.com");
    let share = format!("/api/clips/{id}/share");

    let (status, _) = send(&app, "POST", &share, Some(&kim), None, &[]).await;
    assert_eq!(
        status,
        StatusCode::FORBIDDEN,
        "only the uploader or an admin"
    );
    let (status, body) = send(&app, "POST", &share, Some(&sam), None, &[]).await;
    assert_eq!(status, StatusCode::OK, "{body}");
    let url = json(&body)["shareUrl"].as_str().unwrap().to_owned();
    let token = url
        .strip_prefix("https://clips.example/s/")
        .unwrap()
        .to_owned();
    assert_eq!(token.len(), 22);
    let (_, body) = send(&app, "POST", &share, Some(&sam), None, &[]).await;
    assert_eq!(
        json(&body)["shareUrl"],
        url,
        "sharing twice keeps the same link"
    );
    let (_, body) = get(&app, &format!("/api/clips/{id}"), Some(&kim)).await;
    assert_eq!(
        json(&body)["shareUrl"],
        Value::Null,
        "only managers see the link"
    );

    // Link preview page, no sign-in.
    let (status, html) = get(&app, &format!("/s/{token}"), None).await;
    assert_eq!(status, StatusCode::OK);
    assert!(
        html.contains("<title>Ace &lt;on&gt; &quot;A&quot; · clipos</title>"),
        "{html}"
    );
    assert!(html.contains(&format!(
        "<meta property=\"og:video\" content=\"https://clips.example/s/{token}/video.mp4\">"
    )));
    assert!(html.contains("<meta property=\"og:image:height\" content=\"720\">"));
    assert!(html.contains(
        "<meta property=\"og:description\" content=\"Shared by sam · Mirage · 0:05 · clipos\">"
    ));

    assert!(html.contains("<meta name=\"twitter:card\" content=\"player\">"));
    assert!(html.contains(&format!(
        "<meta name=\"twitter:player:stream\" content=\"https://clips.example/s/{token}/video.mp4\">"
    )));

    // Media is streamed straight from storage (Discord doesn't follow redirects), with
    // byte ranges for seeking. Needs Azurite for the blobs themselves.
    if std::env::var_os("CLIPOS_AZURITE").is_some() {
        let storage = Storage::new(StorageConfig {
            account: "devstoreaccount1".into(),
            blob_endpoint: Some("http://127.0.0.1:10000/devstoreaccount1".into()),
            account_key: Some(AZURITE_KEY.into()),
        })
        .unwrap();
        storage.prepare_local(&[]).await.unwrap();
        let dir = tempfile::tempdir().unwrap();
        let video_file = dir.path().join("v.mp4");
        std::fs::write(
            &video_file,
            (0..=255u8).cycle().take(10_000).collect::<Vec<u8>>(),
        )
        .unwrap();
        let id: uuid::Uuid = id.parse().unwrap();
        storage
            .upload_file(
                Container::Playback,
                &clipos_core::clips::playback_blob(id),
                &video_file,
                "video/mp4",
            )
            .await
            .unwrap();
        storage
            .upload_file(
                Container::Posters,
                &clipos_core::clips::poster_blob(id),
                &video_file,
                "image/jpeg",
            )
            .await
            .unwrap();

        let res = raw(&app, &format!("/s/{token}/video.mp4"), &[]).await;
        assert_eq!(res.status(), StatusCode::OK);
        assert_eq!(res.headers()["content-type"], "video/mp4");
        assert_eq!(res.headers()["content-length"], "10000");
        assert_eq!(res.headers()["accept-ranges"], "bytes");
        assert!(res.headers().get("location").is_none(), "no redirect");
        let body = res.into_body().collect().await.unwrap().to_bytes();
        assert_eq!(body.len(), 10_000);

        let res = raw(
            &app,
            &format!("/s/{token}/video.mp4"),
            &[("range", "bytes=100-199")],
        )
        .await;
        assert_eq!(res.status(), StatusCode::PARTIAL_CONTENT);
        assert_eq!(res.headers()["content-range"], "bytes 100-199/10000");
        let body = res.into_body().collect().await.unwrap().to_bytes();
        assert_eq!(body[0], 100);
        assert_eq!(body.len(), 100);

        let head = app
            .router
            .clone()
            .oneshot(
                Request::head(format!("/s/{token}/poster.jpg"))
                    .body(Body::empty())
                    .unwrap(),
            )
            .await
            .unwrap();
        assert_eq!(head.status(), StatusCode::OK);
        assert_eq!(head.headers()["content-type"], "image/jpeg");
    }
    let (status, body) = get(&app, &format!("/s/{token}/clip.json"), None).await;
    assert_eq!(status, StatusCode::OK);
    assert_eq!(
        json(&body)["videoUrl"],
        format!("https://clips.example/s/{token}/video.mp4")
    );
    assert!(json(&body).get("email").is_none());

    // Revoked: everything goes dark.
    let (status, body) = send(&app, "DELETE", &share, Some(&sam), None, &[]).await;
    assert_eq!(status, StatusCode::OK);
    assert_eq!(json(&body)["shareUrl"], Value::Null);
    for suffix in ["", "/video.mp4", "/poster.jpg", "/clip.json"] {
        let res = raw(&app, &format!("/s/{token}{suffix}"), &[]).await;
        assert_eq!(res.status(), StatusCode::NOT_FOUND, "{suffix}");
    }
    let res = raw(&app, "/s/not-a-real-token/video.mp4", &[]).await;
    assert_eq!(res.status(), StatusCode::NOT_FOUND);
}

#[sqlx::test(migrator = "clipos_core::db::MIGRATOR")]
async fn share_routes_are_rate_limited_per_client(pool: PgPool) {
    async fn hit(app: &TestApp, ip: &str) -> StatusCode {
        raw(
            app,
            "/s/AAAAAAAAAAAAAAAAAAAAAA/clip.json",
            &[("x-forwarded-for", ip)],
        )
        .await
        .status()
    }
    let app = app(pool).await;
    for _ in 0..SHARE_BURST {
        assert_eq!(hit(&app, "203.0.113.9:1").await, StatusCode::NOT_FOUND);
    }
    let res = raw(
        &app,
        "/s/AAAAAAAAAAAAAAAAAAAAAA",
        &[("x-forwarded-for", "203.0.113.9:2")],
    )
    .await;
    assert_eq!(res.status(), StatusCode::TOO_MANY_REQUESTS);
    assert_eq!(res.headers()["retry-after"], "30");
    assert_eq!(
        hit(&app, "198.51.100.4:1").await,
        StatusCode::NOT_FOUND,
        "other clients are fine"
    );

    // An IPv6 client is its /64: another address in it is the same client, the next /64
    // over isn't.
    for _ in 0..SHARE_BURST {
        assert_eq!(
            hit(&app, "[2001:db8:0:1::1]:1").await,
            StatusCode::NOT_FOUND
        );
    }
    assert_eq!(
        hit(&app, "[2001:db8:0:1:ffff::2]:1").await,
        StatusCode::TOO_MANY_REQUESTS
    );
    assert_eq!(
        hit(&app, "[2001:db8:0:2::1]:1").await,
        StatusCode::NOT_FOUND
    );
}

/// Videos and posters, which stream whole files, have their own budget per client, and
/// each request costs at least 1 MiB of it.
#[sqlx::test(migrator = "clipos_core::db::MIGRATOR")]
async fn share_media_is_limited_per_client(pool: PgPool) {
    // A link of its own each time, so only the client's limit counts.
    async fn hit(app: &TestApp, file: &str, ip: &str) -> axum::response::Response {
        let link = uuid::Uuid::new_v4().simple();
        raw(
            app,
            &format!("/s/{link}/{file}"),
            &[("x-forwarded-for", ip)],
        )
        .await
    }
    let app = app(pool).await;
    for file in ["video.mp4", "poster.jpg"].repeat(MEDIA_BURST as usize / 2) {
        let res = hit(&app, file, "203.0.113.9:1").await;
        assert_eq!(res.status(), StatusCode::NOT_FOUND, "{file}");
    }
    let res = hit(&app, "video.mp4", "203.0.113.9:2").await;
    assert_eq!(res.status(), StatusCode::TOO_MANY_REQUESTS);
    assert_eq!(res.headers()["retry-after"], "30");
    let res = hit(&app, "poster.jpg", "203.0.113.9:3").await;
    assert_eq!(res.status(), StatusCode::TOO_MANY_REQUESTS);
    // Share pages are limited apart from the media.
    let res = hit(&app, "clip.json", "203.0.113.9:4").await;
    assert_eq!(res.status(), StatusCode::NOT_FOUND);
    // Others are fine, and an IPv6 client is its /64 here too.
    for _ in 0..MEDIA_BURST {
        let res = hit(&app, "video.mp4", "[2001:db8:0:1::1]:1").await;
        assert_eq!(res.status(), StatusCode::NOT_FOUND);
    }
    let res = hit(&app, "video.mp4", "[2001:db8:0:1::2]:1").await;
    assert_eq!(res.status(), StatusCode::TOO_MANY_REQUESTS);
}

/// One link's video and poster stream only so much, to everyone together.
#[sqlx::test(migrator = "clipos_core::db::MIGRATOR")]
async fn share_media_is_capped_per_link(pool: PgPool) {
    /// `path` for the client at 198.51.100.`client`.
    async fn hit(app: &TestApp, path: &str, client: u32) -> StatusCode {
        let ip = format!("198.51.100.{client}:1");
        raw(app, path, &[("x-forwarded-for", &ip)]).await.status()
    }
    let app = app(pool).await;
    let (video, poster) = (
        "/s/BBBBBBBBBBBBBBBBBBBBBB/video.mp4",
        "/s/BBBBBBBBBBBBBBBBBBBBBB/poster.jpg",
    );
    // A client over its own limit doesn't use up the link's.
    for i in 0..MEDIA_BURST + 5 {
        let expected = if i < MEDIA_BURST {
            StatusCode::NOT_FOUND
        } else {
            StatusCode::TOO_MANY_REQUESTS
        };
        assert_eq!(hit(&app, video, 1).await, expected, "request {i}");
    }
    // What's left of the link's goes to others, each well within their own limit.
    for client in 0..LINK_MEDIA_BURST - MEDIA_BURST {
        assert_eq!(hit(&app, poster, 10 + client).await, StatusCode::NOT_FOUND);
    }
    assert_eq!(hit(&app, video, 99).await, StatusCode::TOO_MANY_REQUESTS);
    assert_eq!(hit(&app, poster, 99).await, StatusCode::TOO_MANY_REQUESTS);
    // Its page still opens, and other links' media are fine.
    assert_eq!(
        hit(&app, "/s/BBBBBBBBBBBBBBBBBBBBBB", 99).await,
        StatusCode::NOT_FOUND
    );
    assert_eq!(
        hit(&app, "/s/CCCCCCCCCCCCCCCCCCCCCC/video.mp4", 99).await,
        StatusCode::NOT_FOUND
    );
}

/// A client can have only so many videos and posters streaming at once.
#[sqlx::test(migrator = "clipos_core::db::MIGRATOR")]
async fn share_media_streams_are_capped_per_client(pool: PgPool) {
    async fn hit(app: &TestApp, ip: &str) -> axum::response::Response {
        raw(
            app,
            "/s/DDDDDDDDDDDDDDDDDDDDDD/video.mp4",
            &[("x-forwarded-for", ip)],
        )
        .await
    }
    let app = app(pool).await;
    // A response is streaming until its body has been read or dropped.
    let mut open = Vec::new();
    for _ in 0..MEDIA_STREAMS {
        let res = hit(&app, "203.0.113.9:1").await;
        assert_eq!(res.status(), StatusCode::NOT_FOUND);
        open.push(res);
    }
    let res = hit(&app, "203.0.113.9:2").await;
    assert_eq!(res.status(), StatusCode::TOO_MANY_REQUESTS);
    assert_eq!(res.headers()["retry-after"], "30");
    assert_eq!(
        hit(&app, "198.51.100.4:1").await.status(),
        StatusCode::NOT_FOUND,
        "other clients are fine"
    );
    // One is read to the end and another dropped: room for two more.
    open.pop().unwrap().into_body().collect().await.unwrap();
    open.pop();
    for _ in 0..2 {
        let res = hit(&app, "203.0.113.9:3").await;
        assert_eq!(res.status(), StatusCode::NOT_FOUND);
        open.push(res);
    }
    assert_eq!(
        hit(&app, "203.0.113.9:4").await.status(),
        StatusCode::TOO_MANY_REQUESTS
    );
    // Refused for having too many open, it paid nothing: what's left of its budget is
    // there once they're done.
    drop(open);
    let spent = MEDIA_STREAMS as u32 + 2;
    for _ in spent..MEDIA_BURST {
        assert_eq!(
            hit(&app, "203.0.113.9:5").await.status(),
            StatusCode::NOT_FOUND
        );
    }
    assert_eq!(
        hit(&app, "203.0.113.9:6").await.status(),
        StatusCode::TOO_MANY_REQUESTS
    );
}

/// Media are paid for by the byte: a whole video costs its size, out of the client's
/// budget and the link's.
#[sqlx::test(migrator = "clipos_core::db::MIGRATOR")]
async fn share_media_is_paid_for_by_the_byte(pool: PgPool) {
    async fn first_byte(app: &TestApp, video: &str, ip: &str) -> StatusCode {
        let headers = [("x-forwarded-for", ip), ("range", "bytes=0-0")];
        raw(app, video, &headers).await.status()
    }
    let Some(storage) = azurite_storage().await else {
        return;
    };
    invite(&pool, "sam@gmail.com").await;
    let id = ready_clip(&pool, "google-oauth2|sam", "sam@gmail.com", "Ace").await;
    let app = app(pool).await;
    let sam = user_token("google-oauth2|sam", "sam@gmail.com");
    let dir = tempfile::tempdir().unwrap();
    let file = dir.path().join("v.mp4");
    std::fs::write(&file, vec![7u8; 3 << 20]).unwrap();
    let blob = clipos_core::clips::playback_blob(id.parse().unwrap());
    storage
        .upload_file(Container::Playback, &blob, &file, "video/mp4")
        .await
        .unwrap();
    let (_, body) = send(
        &app,
        "POST",
        &format!("/api/clips/{id}/share"),
        Some(&sam),
        None,
        &[],
    )
    .await;
    let video = format!("{}/video.mp4", json(&body)["shareUrl"].as_str().unwrap())
        .replace("https://clips.example", "");

    // The whole 3 MiB video.
    let res = raw(&app, &video, &[("x-forwarded-for", "203.0.113.9:1")]).await;
    assert_eq!(res.status(), StatusCode::OK);
    let body = res.into_body().collect().await.unwrap().to_bytes();
    assert_eq!(body.len(), 3 << 20);
    // The rest of the client's budget goes 1 MiB at a time on its first byte.
    for i in 3..MEDIA_BURST {
        let status = first_byte(&app, &video, "203.0.113.9:2").await;
        assert_eq!(status, StatusCode::PARTIAL_CONTENT, "{i} MiB");
    }
    let status = first_byte(&app, &video, "203.0.113.9:3").await;
    assert_eq!(status, StatusCode::TOO_MANY_REQUESTS);
    // The link paid for all of that too, and has the rest for others.
    for i in MEDIA_BURST..LINK_MEDIA_BURST {
        let status = first_byte(&app, &video, "198.51.100.4:1").await;
        assert_eq!(status, StatusCode::PARTIAL_CONTENT, "{i} MiB");
    }
    let status = first_byte(&app, &video, "198.51.100.4:2").await;
    assert_eq!(status, StatusCode::TOO_MANY_REQUESTS);
}

#[sqlx::test(migrator = "clipos_core::db::MIGRATOR")]
async fn shows_are_admin_only_until_they_open(pool: PgPool) {
    let admin = admin_token(&pool).await;
    invite(&pool, "sam@gmail.com").await;
    let app = app(pool).await;
    let sam = user_token("google-oauth2|sam", "sam@gmail.com");
    let (status, _) = get(&app, "/api/shows/tonight", Some(&sam)).await;
    assert_eq!(status, StatusCode::NOT_FOUND);
    let (status, _) = send(&app, "POST", "/api/shows", Some(&sam), None, &[]).await;
    assert_eq!(status, StatusCode::NOT_FOUND);
    let (status, body) = get(&app, "/api/shows/tonight", Some(&admin)).await;
    assert_eq!(status, StatusCode::OK, "{body}");
    assert!(json(&body)["show"].is_null());
}

/// After a show: its clip of the night and the clips someone pressed 🍌 on have their
/// filters and badges, each clip says where it played and what beat it, and the winners'
/// profiles have their trophies. None of it for someone the show isn't open to.
#[sqlx::test(migrator = "clipos_core::db::MIGRATOR")]
async fn a_shows_winners_in_the_archive_and_on_profiles(pool: PgPool) {
    let admin = admin_token(&pool).await;
    invite(&pool, "sam@gmail.com").await;
    invite(&pool, "kim@gmail.com").await;
    let a = ready_clip(&pool, "google-oauth2|sam", "sam@gmail.com", "A").await;
    let b = ready_clip(&pool, "google-oauth2|kim", "kim@gmail.com", "B").await;
    let members_only = app_with(pool.clone(), false).await;
    let app = app_with(pool, true).await;
    let sam = user_token("google-oauth2|sam", "sam@gmail.com");
    let kim = user_token("google-oauth2|kim", "kim@gmail.com");
    let call = |method: &'static str, path: String, who: &str, body: Option<Value>| {
        let (app, who) = (&app, who.to_owned());
        async move { send(app, method, &path, Some(&who), body, &[]).await }
    };
    let id = new_show(&app, &admin, false).await;
    for who in [&sam, &kim] {
        let (status, _) = call("POST", format!("/api/shows/{id}/join"), who, None).await;
        assert_eq!(status, StatusCode::OK);
    }
    let (status, _) = call("POST", format!("/api/shows/{id}/start"), &admin, None).await;
    assert_eq!(status, StatusCode::OK);
    for clip in [&a, &b] {
        let path = format!("/api/shows/{id}/clips/{clip}/played");
        let (status, body) = call("POST", path, &admin, None).await;
        assert_eq!(status, StatusCode::OK, "{body}");
    }
    let tap = json!({ "clipId": b, "emoji": "🍌", "atMs": 1000 });
    let (status, _) = call(
        "POST",
        format!("/api/shows/{id}/reactions"),
        &sam,
        Some(tap),
    )
    .await;
    assert_eq!(status, StatusCode::NO_CONTENT);
    let (status, _) = call("POST", format!("/api/shows/{id}/finale"), &admin, None).await;
    assert_eq!(status, StatusCode::OK);
    let pick = |clip: &str| Some(json!({ "clipId": clip }));
    for (who, cat, clip) in [(&kim, "clip", &a), (&admin, "clip", &a), (&sam, "fail", &b)] {
        let path = format!("/api/shows/{id}/votes/{cat}");
        let (status, body) = call("PUT", path, who, pick(clip)).await;
        assert_eq!(status, StatusCode::OK, "{body}");
    }
    let (status, body) = call("POST", format!("/api/shows/{id}/end"), &admin, None).await;
    assert_eq!(status, StatusCode::OK, "{body}");

    // The archive's filters: clips of the night, and every clip that got a 🍌.
    let titles = |body: &str| -> Vec<String> {
        json(body)["clips"]
            .as_array()
            .unwrap()
            .iter()
            .map(|c| c["title"].as_str().unwrap().to_owned())
            .collect()
    };
    let (_, body) = get(&app, "/api/clips?night=clip", Some(&kim)).await;
    assert_eq!(titles(&body), ["A"]);
    let (_, body) = get(&app, "/api/clips?night=fail", Some(&kim)).await;
    assert_eq!(titles(&body), ["B"]);
    let (status, _) = get(&app, "/api/clips?night=best", Some(&kim)).await;
    assert_eq!(status, StatusCode::BAD_REQUEST);

    // The badges, and where each played.
    let (_, body) = get(&app, &format!("/api/clips/{a}"), Some(&kim)).await;
    let clip_a = json(&body);
    assert_eq!(clip_a["clipOfTheNight"], true);
    assert_eq!(clip_a["failOfTheNight"], false);
    assert_eq!(clip_a["playedIn"]["showId"], id.as_str());
    assert_eq!(clip_a["playedIn"]["position"], 1);
    assert_eq!(clip_a["playedIn"]["count"], 2);
    assert!(clip_a["playedIn"]["lostTo"].is_null());
    let (_, body) = get(&app, &format!("/api/clips/{b}"), Some(&kim)).await;
    let clip_b = json(&body);
    assert_eq!(clip_b["failOfTheNight"], true);
    assert_eq!(clip_b["playedIn"]["position"], 2);
    assert_eq!(clip_b["playedIn"]["lostTo"]["title"], "A");
    // Lists carry the badges, not where they played.
    let (_, body) = get(&app, "/api/clips", Some(&kim)).await;
    let listed = json(&body)["clips"].as_array().unwrap().clone();
    assert!(listed.iter().all(|c| c["playedIn"].is_null()));
    assert!(listed.iter().any(|c| c["clipOfTheNight"] == true));

    // Profiles: shows hosted, and trophies.
    let (_, body) = get(&app, "/api/users/sam", Some(&kim)).await;
    let shows = &json(&body)["shows"];
    assert_eq!(shows["hosted"], 0);
    assert_eq!(shows["trophies"].as_array().unwrap().len(), 1);
    assert_eq!(shows["trophies"][0]["category"], "clip");
    assert_eq!(shows["trophies"][0]["clip"]["title"], "A");
    assert_eq!(shows["trophies"][0]["showId"], id.as_str());
    let (_, body) = get(&app, "/api/users/admin", Some(&kim)).await;
    assert_eq!(json(&body)["shows"]["hosted"], 1);

    // While shows are admins-only, members see none of it.
    let (status, body) = get(&members_only, "/api/clips?night=clip", Some(&kim)).await;
    assert_eq!(status, StatusCode::OK);
    assert!(titles(&body).is_empty());
    let (_, body) = get(&members_only, &format!("/api/clips/{a}"), Some(&kim)).await;
    assert_eq!(json(&body)["clipOfTheNight"], false);
    assert!(json(&body)["playedIn"].is_null());
    let (_, body) = get(&members_only, "/api/users/sam", Some(&kim)).await;
    assert!(json(&body)["shows"].is_null());
}

/// Nobody pressed 🍌: the finale goes straight to clip of the night. Before the finale
/// there's no vote clock at all.
#[sqlx::test(migrator = "clipos_core::db::MIGRATOR")]
async fn a_finale_without_fails_votes_on_the_clip_alone(pool: PgPool) {
    let admin = admin_token(&pool).await;
    invite(&pool, "sam@gmail.com").await;
    let a = ready_clip(&pool, "google-oauth2|sam", "sam@gmail.com", "A").await;
    let app = app(pool).await;
    let show = new_show(&app, &admin, true).await;
    let (_, body) = get(&app, &format!("/api/shows/{show}"), Some(&admin)).await;
    assert!(json(&body)["finale"].is_null());
    let path = format!("/api/shows/{show}/clips/{a}/played");
    let (status, _) = send(&app, "POST", &path, Some(&admin), None, &[]).await;
    assert_eq!(status, StatusCode::OK);
    let path = format!("/api/shows/{show}/finale");
    let (status, body) = send(&app, "POST", &path, Some(&admin), None, &[]).await;
    assert_eq!(status, StatusCode::OK, "{body}");
    let finale = &json(&body)["finale"];
    assert!(finale["failFrom"].is_null() && finale["failUntil"].is_null());
    assert_eq!(finale["clipFrom"], finale["startedAt"]);
}

/// Every page's "Live · Join" pill: the show that's on, in brief, from the lobby to the
/// finale; nothing once it's over, and nothing for people the show isn't open to.
#[sqlx::test(migrator = "clipos_core::db::MIGRATOR")]
async fn the_current_show_for_the_live_pill(pool: PgPool) {
    let admin = admin_token(&pool).await;
    invite(&pool, "sam@gmail.com").await;
    ready_clip(&pool, "google-oauth2|sam", "sam@gmail.com", "A").await;
    let app = app(pool).await;
    let sam = user_token("google-oauth2|sam", "sam@gmail.com");

    let (status, body) = get(&app, "/api/shows/current", Some(&admin)).await;
    assert_eq!(status, StatusCode::OK, "{body}");
    assert!(json(&body).is_null());

    let show = new_show(&app, &admin, false).await;
    let (_, body) = get(&app, "/api/shows/current", Some(&admin)).await;
    let current = json(&body);
    assert_eq!(current["id"], show.as_str());
    assert_eq!(current["status"], "lobby");
    assert_eq!(current["host"]["handle"], "admin");
    assert!(current["startedAt"].is_null());

    let path = format!("/api/shows/{show}/start");
    let (status, _) = send(&app, "POST", &path, Some(&admin), None, &[]).await;
    assert_eq!(status, StatusCode::OK);
    let (_, body) = get(&app, "/api/shows/current", Some(&admin)).await;
    assert_eq!(json(&body)["status"], "live");
    assert!(json(&body)["startedAt"].is_string());

    // Not open to Sam yet: there's no such thing.
    let (status, _) = get(&app, "/api/shows/current", Some(&sam)).await;
    assert_eq!(status, StatusCode::NOT_FOUND);

    let path = format!("/api/shows/{show}/finale");
    let (status, _) = send(&app, "POST", &path, Some(&admin), None, &[]).await;
    assert_eq!(status, StatusCode::OK);
    let path = format!("/api/shows/{show}/end");
    let (status, body) = send(&app, "POST", &path, Some(&admin), None, &[]).await;
    assert_eq!(status, StatusCode::OK, "{body}");
    let (_, body) = get(&app, "/api/shows/current", Some(&admin)).await;
    assert!(json(&body).is_null());
}

#[sqlx::test(migrator = "clipos_core::db::MIGRATOR")]
async fn a_show_from_lobby_to_winners(pool: PgPool) {
    let admin = admin_token(&pool).await;
    invite(&pool, "sam@gmail.com").await;
    invite(&pool, "kim@gmail.com").await;
    let a = ready_clip(&pool, "google-oauth2|sam", "sam@gmail.com", "A").await;
    let b = ready_clip(&pool, "google-oauth2|kim", "kim@gmail.com", "B").await;
    let app = app_with(pool, true).await;
    let sam = user_token("google-oauth2|sam", "sam@gmail.com");
    let kim = user_token("google-oauth2|kim", "kim@gmail.com");
    let call = |method: &'static str, path: String, who: String, body: Option<Value>| {
        let app = &app;
        async move { send(app, method, &path, Some(&who), body, &[]).await }
    };

    // Tonight, before a show: the clips the show would play.
    let (_, body) = get(&app, "/api/shows/tonight", Some(&sam)).await;
    let tonight = json(&body);
    assert!(tonight["show"].is_null());
    assert_eq!(tonight["clips"].as_array().unwrap().len(), 2);

    let (status, body) = call("POST", "/api/shows".into(), admin.clone(), None).await;
    assert_eq!(status, StatusCode::CREATED, "{body}");
    let show = json(&body);
    let id = show["id"].as_str().unwrap().to_owned();
    assert_eq!(show["status"], "lobby");
    assert_eq!(show["lineup"].as_array().unwrap().len(), 2);
    assert_eq!(show["host"]["handle"], "admin");
    let (status, _) = call("POST", "/api/shows".into(), sam.clone(), None).await;
    assert_eq!(status, StatusCode::CONFLICT);

    for who in [&sam, &kim] {
        let (status, body) = call("POST", format!("/api/shows/{id}/join"), who.clone(), None).await;
        assert_eq!(status, StatusCode::OK, "{body}");
    }
    let (status, _) = call("POST", format!("/api/shows/{id}/start"), sam.clone(), None).await;
    assert_eq!(status, StatusCode::FORBIDDEN);
    let (status, _) = call(
        "POST",
        format!("/api/shows/{id}/start"),
        admin.clone(),
        None,
    )
    .await;
    assert_eq!(status, StatusCode::OK);
    for clip in [&a, &b] {
        let path = format!("/api/shows/{id}/clips/{clip}/played");
        let (status, body) = call("POST", path, admin.clone(), None).await;
        assert_eq!(status, StatusCode::OK, "{body}");
    }

    let tap = |clip: &str, emoji: &str| json!({ "clipId": clip, "emoji": emoji, "atMs": 1200 });
    let reactions = format!("/api/shows/{id}/reactions");
    let (status, _) = call("POST", reactions.clone(), kim.clone(), Some(tap(&a, "🍌"))).await;
    assert_eq!(status, StatusCode::NO_CONTENT);
    let (status, _) = call("POST", reactions.clone(), sam.clone(), Some(tap(&b, "🔥"))).await;
    assert_eq!(status, StatusCode::NO_CONTENT);
    let (status, _) = call("POST", reactions, sam.clone(), Some(tap(&b, "🍕"))).await;
    assert_eq!(status, StatusCode::BAD_REQUEST);

    let (status, body) = call(
        "POST",
        format!("/api/shows/{id}/finale"),
        admin.clone(),
        None,
    )
    .await;
    assert_eq!(status, StatusCode::OK, "{body}");
    assert_eq!(json(&body)["failContenders"], json!([a]));
    // Someone pressed 🍌: fail of the night first, then clip of the night, 20 s each.
    let finale = &json(&body)["finale"];
    let at = |k: &str| {
        chrono::DateTime::parse_from_rfc3339(finale[k].as_str().unwrap())
            .unwrap()
            .timestamp_millis()
    };
    assert_eq!(at("failFrom"), at("startedAt"));
    assert_eq!(at("failUntil") - at("failFrom"), 20_000);
    assert_eq!(at("clipFrom"), at("failUntil"));
    assert_eq!(at("clipUntil") - at("clipFrom"), 20_000);

    let vote = |cat: &str| format!("/api/shows/{id}/votes/{cat}");
    let pick = |clip: &str| Some(json!({ "clipId": clip }));
    // Your own clip too (voting again changes it); no fail vote for a clip nobody marked.
    let (status, body) = call("PUT", vote("clip"), sam.clone(), pick(&a)).await;
    assert_eq!(status, StatusCode::OK, "{body}");
    let (status, _) = call("PUT", vote("fail"), sam.clone(), pick(&b)).await;
    assert_eq!(status, StatusCode::BAD_REQUEST);
    let (status, body) = call("PUT", vote("clip"), sam.clone(), pick(&b)).await;
    assert_eq!(status, StatusCode::OK, "{body}");
    assert_eq!(json(&body)["myVotes"]["clip"], json!(b));
    call("PUT", vote("clip"), kim.clone(), pick(&a)).await;
    call("PUT", vote("fail"), kim.clone(), pick(&a)).await;

    // A tie: the host has to pick.
    let end = format!("/api/shows/{id}/end");
    let (status, body) = call("POST", end.clone(), admin.clone(), Some(json!({}))).await;
    assert_eq!(status, StatusCode::CONFLICT, "{body}");
    let (status, body) = call(
        "POST",
        end,
        admin.clone(),
        Some(json!({ "tieBreak": { "clip": b } })),
    )
    .await;
    assert_eq!(status, StatusCode::OK, "{body}");
    let ended = json(&body);
    assert_eq!(ended["status"], "ended");
    assert_eq!(ended["clipWinnerId"], json!(b));
    assert_eq!(ended["failWinnerId"], json!(a));
    assert_eq!(ended["reactions"].as_array().unwrap().len(), 2);

    // The archive's past shows, and the show's 🔥 on the clip page.
    let (_, body) = get(&app, "/api/shows", Some(&kim)).await;
    let past = json(&body);
    assert_eq!(past.as_array().unwrap().len(), 1);
    assert_eq!(past[0]["clips"].as_array().unwrap().len(), 2);
    assert_eq!(past[0]["clipVoters"], 2);
    assert_eq!(past[0]["clipWinnerVotes"], 1);
    assert_eq!(past[0]["participants"].as_array().unwrap().len(), 3);
    let (_, body) = get(&app, &format!("/api/clips/{b}"), Some(&kim)).await;
    assert_eq!(json(&body)["reactions"][0]["emoji"], "🔥");
}

#[sqlx::test(migrator = "clipos_core::db::MIGRATOR")]
async fn a_clip_saved_for_the_show(pool: PgPool) {
    let admin = admin_token(&pool).await;
    invite(&pool, "sam@gmail.com").await;
    invite(&pool, "kim@gmail.com").await;
    let id = ready_clip(&pool, "google-oauth2|sam", "sam@gmail.com", "Ace").await;
    clipos_core::clips::hold(&pool, id.parse().unwrap())
        .await
        .unwrap();
    let app = app(pool).await;
    let sam = user_token("google-oauth2|sam", "sam@gmail.com");
    let kim = user_token("google-oauth2|kim", "kim@gmail.com");

    // What's switched on: shows are admin-only here.
    let (_, body) = get(&app, "/api/me", Some(&kim)).await;
    assert_eq!(json(&body)["shows"], false);
    let (_, body) = get(&app, "/api/me", Some(&admin)).await;
    assert_eq!(json(&body)["shows"], true);
    assert_eq!(json(&body)["showsForEveryone"], false);

    // Hidden from others, in the archive and by link; the uploader sees when it's free.
    let (_, body) = get(&app, "/api/clips", Some(&kim)).await;
    assert!(json(&body)["clips"].as_array().unwrap().is_empty());
    let (status, _) = get(&app, &format!("/api/clips/{id}"), Some(&kim)).await;
    assert_eq!(status, StatusCode::NOT_FOUND);
    let (_, body) = get(&app, &format!("/api/clips/{id}"), Some(&sam)).await;
    assert!(json(&body)["heldUntil"].is_string());
    assert_eq!(json(&body)["teaser"], false);

    // In tonight's lineup others get the teaser: title and length, blurred poster only.
    let (_, body) = get(&app, "/api/shows/tonight", Some(&admin)).await;
    let teaser = &json(&body)["clips"][0];
    assert_eq!(teaser["teaser"], true);
    assert_eq!(teaser["title"], "Ace");
    assert!(teaser["playbackUrl"].is_null());
    assert!(
        teaser["posterUrl"]
            .as_str()
            .unwrap()
            .contains("-teaser.jpg")
    );
    assert!(teaser["heldUntil"].is_null());

    // "Post now": only the uploader; then everyone sees it.
    let release = format!("/api/clips/{id}/release");
    let (status, _) = send(&app, "POST", &release, Some(&kim), None, &[]).await;
    assert_eq!(status, StatusCode::FORBIDDEN);
    let (status, body) = send(&app, "POST", &release, Some(&sam), None, &[]).await;
    assert_eq!(status, StatusCode::OK, "{body}");
    assert!(json(&body)["heldUntil"].is_null());
    let (status, _) = get(&app, &format!("/api/clips/{id}"), Some(&kim)).await;
    assert_eq!(status, StatusCode::OK);
}

#[sqlx::test(migrator = "clipos_core::db::MIGRATOR")]
async fn the_hold_switch_needs_the_show(pool: PgPool) {
    let admin = admin_token(&pool).await;
    invite(&pool, "sam@gmail.com").await;
    let app = app(pool).await;
    let sam = user_token("google-oauth2|sam", "sam@gmail.com");
    let upload = json!({ "title": "Ace", "filename": "a.mp4", "bytes": 1000, "hold": true });
    let (status, body) = send(
        &app,
        "POST",
        "/api/clips",
        Some(&admin),
        Some(upload.clone()),
        &[],
    )
    .await;
    assert_eq!(status, StatusCode::CREATED, "{body}");
    assert!(json(&body)["clip"]["heldUntil"].is_string());
    // Shows aren't open to sam yet, so the switch does nothing.
    let (status, body) = send(&app, "POST", "/api/clips", Some(&sam), Some(upload), &[]).await;
    assert_eq!(status, StatusCode::CREATED, "{body}");
    assert!(json(&body)["clip"]["heldUntil"].is_null());
    let id = json(&body)["clip"]["id"].as_str().unwrap().to_owned();
    let (status, _) = send(
        &app,
        "POST",
        &format!("/api/clips/{id}/hold"),
        Some(&sam),
        None,
        &[],
    )
    .await;
    assert_eq!(status, StatusCode::NOT_FOUND);
}

#[sqlx::test(migrator = "clipos_core::db::MIGRATOR")]
async fn a_held_clip_plays_only_for_people_in_the_show(pool: PgPool) {
    let admin = admin_token(&pool).await;
    for who in ["sam", "kim", "lee"] {
        invite(&pool, &format!("{who}@gmail.com")).await;
    }
    let id = ready_clip(&pool, "google-oauth2|sam", "sam@gmail.com", "Ace").await;
    clipos_core::clips::hold(&pool, id.parse().unwrap())
        .await
        .unwrap();
    // kim plays in it; sam gives it a 🔥.
    let kim_id = ready_clip(&pool, "google-oauth2|kim", "kim@gmail.com", "Kim's").await;
    let kim_user = clipos_core::clips::get(&pool, kim_id.parse().unwrap())
        .await
        .unwrap()
        .unwrap()
        .owner_id;
    clipos_core::social::set_players(&pool, id.parse().unwrap(), &[kim_user])
        .await
        .unwrap();
    let app = app_with(pool, true).await;
    let sam = user_token("google-oauth2|sam", "sam@gmail.com");
    let kim = user_token("google-oauth2|kim", "kim@gmail.com");
    let lee = user_token("google-oauth2|lee", "lee@gmail.com");
    let fire = format!("/api/clips/{id}/reactions/%F0%9F%94%A5");
    let (status, _) = send(&app, "PUT", &fire, Some(&sam), None, &[]).await;
    assert!(status.is_success());

    let (status, body) = send(&app, "POST", "/api/shows", Some(&admin), None, &[]).await;
    assert_eq!(status, StatusCode::CREATED, "{body}");
    let show = json(&body)["id"].as_str().unwrap().to_owned();
    // Nobody can edit a teaser, admins included: they can't open it either.
    let teaser = &json(&body)["lineup"][0]["clip"];
    assert_eq!(
        (&teaser["teaser"], &teaser["canEdit"]),
        (&json!(true), &json!(false))
    );
    let (status, _) = send(
        &app,
        "POST",
        &format!("/api/shows/{show}/join"),
        Some(&kim),
        None,
        &[],
    )
    .await;
    assert_eq!(status, StatusCode::OK);

    // In the lobby it's still a spoiler, even for kim.
    let path = format!("/api/clips/{id}");
    let (status, _) = get(&app, &path, Some(&kim)).await;
    assert_eq!(status, StatusCode::NOT_FOUND);
    let (status, _) = send(
        &app,
        "POST",
        &format!("/api/shows/{show}/start"),
        Some(&admin),
        None,
        &[],
    )
    .await;
    assert_eq!(status, StatusCode::OK);

    // Live, kim is in the show: she can fetch it to play (the next clip preloads), still
    // as a teaser. lee isn't: it doesn't exist for him.
    let (status, body) = get(&app, &path, Some(&kim)).await;
    assert_eq!(status, StatusCode::OK, "{body}");
    let clip = json(&body);
    assert_eq!(clip["teaser"], true);
    assert!(clip["playbackUrl"].as_str().unwrap().contains(".mp4"));
    assert!(clip["reactions"].as_array().unwrap().is_empty());
    let (status, _) = get(&app, &path, Some(&lee)).await;
    assert_eq!(status, StatusCode::NOT_FOUND);

    // Still hidden everywhere else for kim: the archive, reactions, profile counts.
    let (_, body) = get(&app, "/api/clips", Some(&kim)).await;
    assert_eq!(json(&body)["clips"].as_array().unwrap().len(), 1);
    let (status, _) = send(&app, "PUT", &fire, Some(&kim), None, &[]).await;
    assert_eq!(status, StatusCode::NOT_FOUND);
    let (_, body) = get(&app, "/api/users/sam", Some(&kim)).await;
    assert_eq!(
        (&json(&body)["clipCount"], &json(&body)["fireCount"]),
        (&json!(0), &json!(0))
    );
    let (_, body) = get(&app, "/api/users/kim", Some(&kim)).await;
    assert_eq!(json(&body)["featuredCount"], 0);
    // Its uploader counts it.
    let (_, body) = get(&app, "/api/users/sam", Some(&sam)).await;
    assert_eq!(
        (&json(&body)["clipCount"], &json(&body)["fireCount"]),
        (&json!(1), &json!(1))
    );
    let (_, body) = get(&app, "/api/users/kim", Some(&sam)).await;
    assert_eq!(json(&body)["featuredCount"], 1);

    // Dropped from the lineup: gone again.
    let lineup = json!({ "clipIds": [kim_id] });
    let (status, body) = send(
        &app,
        "PUT",
        &format!("/api/shows/{show}/lineup"),
        Some(&admin),
        Some(lineup),
        &[],
    )
    .await;
    assert_eq!(status, StatusCode::OK, "{body}");
    let (status, _) = get(&app, &path, Some(&kim)).await;
    assert_eq!(status, StatusCode::NOT_FOUND);
}

#[sqlx::test(migrator = "clipos_core::db::MIGRATOR")]
async fn a_hold_cant_outlast_the_week_after_upload(pool: PgPool) {
    invite(&pool, "sam@gmail.com").await;
    let id = ready_clip(&pool, "google-oauth2|sam", "sam@gmail.com", "Ace").await;
    let app = app_with(pool.clone(), true).await;
    let sam = user_token("google-oauth2|sam", "sam@gmail.com");
    let hold = format!("/api/clips/{id}/hold");
    let (status, body) = send(&app, "POST", &hold, Some(&sam), None, &[]).await;
    assert_eq!(status, StatusCode::OK, "{body}");
    sqlx::query("UPDATE clips SET created_at = now() - interval '8 days'")
        .execute(&pool)
        .await
        .unwrap();
    let (status, body) = send(&app, "POST", &hold, Some(&sam), None, &[]).await;
    assert_eq!(status, StatusCode::CONFLICT, "{body}");
}

#[sqlx::test(migrator = "clipos_core::db::MIGRATOR")]
async fn ending_a_show_names_every_tie_and_needs_no_body(pool: PgPool) {
    let admin = admin_token(&pool).await;
    invite(&pool, "sam@gmail.com").await;
    invite(&pool, "kim@gmail.com").await;
    let a = ready_clip(&pool, "google-oauth2|admin", "admin@gmail.com", "A").await;
    let b = ready_clip(&pool, "google-oauth2|admin", "admin@gmail.com", "B").await;
    let app = app_with(pool, true).await;
    let sam = user_token("google-oauth2|sam", "sam@gmail.com");
    let kim = user_token("google-oauth2|kim", "kim@gmail.com");
    let call = |method: &'static str, path: String, who: &str, body: Option<Value>| {
        let app = &app;
        let who = who.to_owned();
        async move { send(app, method, &path, Some(&who), body, &[]).await }
    };
    let (_, body) = call("POST", "/api/shows".into(), &admin, None).await;
    let id = json(&body)["id"].as_str().unwrap().to_owned();
    for who in [&sam, &kim] {
        call("POST", format!("/api/shows/{id}/join"), who, None).await;
    }
    call("POST", format!("/api/shows/{id}/start"), &admin, None).await;
    for clip in [&a, &b] {
        call(
            "POST",
            format!("/api/shows/{id}/clips/{clip}/played"),
            &admin,
            None,
        )
        .await;
        let tap = json!({ "clipId": clip, "emoji": "🍌", "atMs": 0 });
        let (status, _) = call(
            "POST",
            format!("/api/shows/{id}/reactions"),
            &sam,
            Some(tap),
        )
        .await;
        assert_eq!(status, StatusCode::NO_CONTENT);
    }
    call("POST", format!("/api/shows/{id}/finale"), &admin, None).await;
    for (who, clip) in [(&sam, &a), (&kim, &b)] {
        for cat in ["clip", "fail"] {
            let pick = json!({ "clipId": clip });
            let (status, body) = call(
                "PUT",
                format!("/api/shows/{id}/votes/{cat}"),
                who,
                Some(pick),
            )
            .await;
            assert_eq!(status, StatusCode::OK, "{body}");
        }
    }

    // Both categories tied: one 409 names the tied clips of each. No body is fine.
    let end = format!("/api/shows/{id}/end");
    let (status, body) = call("POST", end.clone(), &admin, None).await;
    assert_eq!(status, StatusCode::CONFLICT, "{body}");
    let err = json(&body);
    assert_eq!(err["error"], "conflict");
    for cat in ["clip", "fail"] {
        let mut tied: Vec<&str> = err["tied"][cat]
            .as_array()
            .unwrap()
            .iter()
            .map(|v| v.as_str().unwrap())
            .collect();
        tied.sort();
        let mut want = vec![a.as_str(), b.as_str()];
        want.sort();
        assert_eq!(tied, want, "{cat}");
    }
    let picks = json!({ "tieBreak": { "clip": a, "fail": b } });
    let (status, body) = call("POST", end, &admin, Some(picks)).await;
    assert_eq!(status, StatusCode::OK, "{body}");
    assert_eq!(
        (&json(&body)["clipWinnerId"], &json(&body)["failWinnerId"]),
        (&json!(a), &json!(b))
    );

    // Other errors don't carry `tied`.
    let (status, body) = call("POST", format!("/api/shows/{id}/end"), &admin, None).await;
    assert_eq!(status, StatusCode::CONFLICT);
    assert!(json(&body).get("tied").is_none());
}

/// Friends say they're ready in the lobby; a tie in the fail vote alone is named alone;
/// the ended show is tonight's "last show".
#[sqlx::test(migrator = "clipos_core::db::MIGRATOR")]
async fn ready_in_the_lobby_and_a_tie_for_the_fail_alone(pool: PgPool) {
    let admin = admin_token(&pool).await;
    for name in ["sam", "kim", "lee"] {
        invite(&pool, &format!("{name}@gmail.com")).await;
    }
    let a = ready_clip(&pool, "google-oauth2|admin", "admin@gmail.com", "A").await;
    let b = ready_clip(&pool, "google-oauth2|admin", "admin@gmail.com", "B").await;
    let app = app_with(pool, true).await;
    let sam = user_token("google-oauth2|sam", "sam@gmail.com");
    let kim = user_token("google-oauth2|kim", "kim@gmail.com");
    let lee = user_token("google-oauth2|lee", "lee@gmail.com");
    let call = |method: &'static str, path: String, who: &str, body: Option<Value>| {
        let app = &app;
        let who = who.to_owned();
        async move { send(app, method, &path, Some(&who), body, &[]).await }
    };
    let id = new_show(&app, &admin, false).await;
    for who in [&sam, &kim] {
        call("POST", format!("/api/shows/{id}/join"), who, None).await;
    }

    // Ready, then not: the show says so. Only people in the show.
    let ready = format!("/api/shows/{id}/ready");
    let (status, body) = call("PUT", ready.clone(), &sam, Some(json!({ "ready": true }))).await;
    assert_eq!(status, StatusCode::OK, "{body}");
    let readies = |body: &str| {
        json(body)["participants"]
            .as_array()
            .unwrap()
            .iter()
            .filter(|p| p["ready"] == true)
            .count()
    };
    assert_eq!(readies(&body), 1);
    let (_, body) = call("PUT", ready.clone(), &kim, Some(json!({ "ready": true }))).await;
    assert_eq!(readies(&body), 2);
    let (_, body) = call("PUT", ready.clone(), &kim, Some(json!({ "ready": false }))).await;
    assert_eq!(readies(&body), 1);
    let (status, _) = call("PUT", ready, &lee, Some(json!({ "ready": true }))).await;
    assert_eq!(status, StatusCode::FORBIDDEN);

    call("POST", format!("/api/shows/{id}/start"), &admin, None).await;
    for clip in [&a, &b] {
        let played = format!("/api/shows/{id}/clips/{clip}/played");
        call("POST", played, &admin, None).await;
        let tap = json!({ "clipId": clip, "emoji": "🍌", "atMs": 0 });
        call(
            "POST",
            format!("/api/shows/{id}/reactions"),
            &sam,
            Some(tap),
        )
        .await;
    }
    call("POST", format!("/api/shows/{id}/finale"), &admin, None).await;
    // Both want A for the clip of the night, but split on the fail.
    for (who, fail) in [(&sam, &a), (&kim, &b)] {
        for (cat, clip) in [("clip", &a), ("fail", fail)] {
            let path = format!("/api/shows/{id}/votes/{cat}");
            let (status, body) = call("PUT", path, who, Some(json!({ "clipId": clip }))).await;
            assert_eq!(status, StatusCode::OK, "{body}");
        }
    }
    let end = format!("/api/shows/{id}/end");
    let (status, body) = call("POST", end.clone(), &admin, None).await;
    assert_eq!(status, StatusCode::CONFLICT, "{body}");
    let err = json(&body);
    assert_eq!(
        err["message"],
        "the fail of the night vote is tied: pick one of the tied clips"
    );
    assert_eq!(err["tied"]["clip"], json!([]));
    assert_eq!(err["tied"]["fail"].as_array().unwrap().len(), 2);
    let pick = json!({ "tieBreak": { "fail": b } });
    let (status, body) = call("POST", end, &admin, Some(pick)).await;
    assert_eq!(status, StatusCode::OK, "{body}");

    // The next lobby's "last show" line.
    let (status, body) = get(&app, "/api/shows/tonight", Some(&kim)).await;
    assert_eq!(status, StatusCode::OK, "{body}");
    let last = &json(&body)["lastShow"];
    assert_eq!(last["id"], json!(id));
    assert_eq!(last["clipWinnerId"], json!(a));
    assert_eq!(last["failWinnerId"], json!(b));
}

// ---------------------------------------------------------------------------
// QA fixes (2026-10-03).
// ---------------------------------------------------------------------------

fn is_error_body(body: &str) -> bool {
    serde_json::from_str::<Value>(body)
        .is_ok_and(|v| v["error"].is_string() && v["message"].is_string())
}

/// A request axum can't read still gets an `ErrorBody`, with axum's status.
#[sqlx::test(migrator = "clipos_core::db::MIGRATOR")]
async fn unreadable_requests_get_error_bodies(pool: PgPool) {
    let admin = admin_token(&pool).await;
    invite(&pool, "sam@gmail.com").await;
    let id = ready_clip(&pool, "google-oauth2|sam", "sam@gmail.com", "Ace").await;
    let app = app_with(pool, true).await;
    let sam = user_token("google-oauth2|sam", "sam@gmail.com");
    let new_clip = |extra: Value| {
        let mut body = json!({ "title": "x", "filename": "a.mp4", "bytes": 1 });
        body.as_object_mut()
            .unwrap()
            .extend(extra.as_object().unwrap().clone());
        Some(body)
    };
    let cases = [
        (
            "unknown field",
            send(
                &app,
                "POST",
                "/api/clips",
                Some(&sam),
                new_clip(json!({ "bogus": 1 })),
                &[],
            )
            .await,
            StatusCode::UNPROCESSABLE_ENTITY,
        ),
        (
            "wrong type",
            send(
                &app,
                "POST",
                "/api/clips",
                Some(&sam),
                new_clip(json!({ "title": 5 })),
                &[],
            )
            .await,
            StatusCode::UNPROCESSABLE_ENTITY,
        ),
        (
            "no body",
            send(&app, "POST", "/api/clips", Some(&sam), None, &[]).await,
            StatusCode::UNSUPPORTED_MEDIA_TYPE,
        ),
        (
            "text/plain",
            send(
                &app,
                "PATCH",
                &format!("/api/clips/{id}"),
                Some(&sam),
                None,
                &[("content-type", "text/plain")],
            )
            .await,
            StatusCode::UNSUPPORTED_MEDIA_TYPE,
        ),
        (
            "bad uuid",
            get(&app, "/api/clips/not-a-uuid", Some(&sam)).await,
            StatusCode::BAD_REQUEST,
        ),
        (
            "bad sort",
            get(&app, "/api/clips?sort=sideways", Some(&sam)).await,
            StatusCode::BAD_REQUEST,
        ),
        (
            "bad category",
            send(
                &app,
                "PUT",
                &format!("/api/shows/{id}/votes/best"),
                Some(&admin),
                Some(json!({ "clipId": id })),
                &[],
            )
            .await,
            StatusCode::BAD_REQUEST,
        ),
        (
            "number too big",
            send(
                &app,
                "POST",
                &format!("/api/shows/{id}/reactions"),
                Some(&admin),
                Some(json!({ "clipId": id, "emoji": "🔥", "atMs": 99_999_999_999i64 })),
                &[],
            )
            .await,
            StatusCode::UNPROCESSABLE_ENTITY,
        ),
        (
            "an optional body that isn't JSON",
            send(
                &app,
                "POST",
                &format!("/api/shows/{id}/end"),
                Some(&admin),
                None,
                &[("content-type", "text/plain")],
            )
            .await,
            StatusCode::UNSUPPORTED_MEDIA_TYPE,
        ),
    ];
    for (what, (status, body), want) in cases {
        assert_eq!(status, want, "{what}: {body}");
        assert!(is_error_body(&body), "{what}: {body}");
    }
    let (_, body) = send(&app, "POST", "/api/clips", Some(&sam), None, &[]).await;
    assert_eq!(json(&body)["error"], "unsupported_media_type");
}

/// An edit that fails is saved not at all, and one that keeps a friend who has been
/// disabled since works.
#[sqlx::test(migrator = "clipos_core::db::MIGRATOR")]
async fn an_edit_saves_all_or_nothing(pool: PgPool) {
    let admin = admin_token(&pool).await;
    invite(&pool, "sam@gmail.com").await;
    invite(&pool, "kim@gmail.com").await;
    invite(&pool, "lee@gmail.com").await;
    let id = ready_clip(&pool, "google-oauth2|sam", "sam@gmail.com", "Ace").await;
    let app = app(pool).await;
    let sam = user_token("google-oauth2|sam", "sam@gmail.com");
    let me = |t: String| {
        let app = &app;
        async move { json(&get(app, "/api/me", Some(&t)).await.1)["id"].clone() }
    };
    let kim = me(user_token("google-oauth2|kim", "kim@gmail.com")).await;
    let lee = me(user_token("google-oauth2|lee", "lee@gmail.com")).await;
    let path = format!("/api/clips/{id}");
    let patch = |body: Value| send(&app, "PATCH", &path, Some(&sam), Some(body), &[]);

    for bad in [
        json!({ "title": "Renamed", "tags": ["!!!"] }),
        json!({ "map": "Nuke", "players": [uuid::Uuid::new_v4()] }),
    ] {
        let (status, body) = patch(bad).await;
        assert_eq!(status, StatusCode::BAD_REQUEST, "{body}");
    }
    let clip = json(&get(&app, &path, Some(&sam)).await.1);
    assert_eq!(
        (&clip["title"], &clip["map"]),
        (&json!("Ace"), &json!("Mirage"))
    );

    // kim is tagged, then disabled. The edit dialog sends the players back as they are.
    let (status, _) = patch(json!({ "players": [kim] })).await;
    assert_eq!(status, StatusCode::OK);
    for who in [&kim, &lee] {
        let (status, _) = send(
            &app,
            "PATCH",
            &format!("/api/admin/users/{}", who.as_str().unwrap()),
            Some(&admin),
            Some(json!({ "status": "disabled" })),
            &[],
        )
        .await;
        assert_eq!(status, StatusCode::OK);
    }
    let (status, body) =
        patch(json!({ "title": "Ace on B", "map": "Mirage", "tags": [], "players": [kim] })).await;
    assert_eq!(status, StatusCode::OK, "{body}");
    assert_eq!(json(&body)["title"], "Ace on B");
    assert_eq!(json(&body)["players"][0]["id"], kim);
    // Someone disabled who isn't in it yet can't be added.
    let (status, _) = patch(json!({ "players": [kim, lee] })).await;
    assert_eq!(status, StatusCode::BAD_REQUEST);

    // `map: null` clears the map, like "".
    let (status, body) = patch(json!({ "map": null })).await;
    assert_eq!(status, StatusCode::OK, "{body}");
    assert_eq!(json(&body)["map"], Value::Null);
    assert_eq!(json(&body)["title"], "Ace on B", "left out: unchanged");

    // Not in the trash.
    send(&app, "DELETE", &path, Some(&sam), None, &[]).await;
    let (status, body) = patch(json!({ "title": "Edited in the trash" })).await;
    assert_eq!(status, StatusCode::NOT_FOUND, "{body}");
}

/// Bodies are small JSON, so one far bigger than any edit is refused unread; and a tag
/// list far longer than the cap is refused before its tags are looked at.
#[sqlx::test(migrator = "clipos_core::db::MIGRATOR")]
async fn edits_are_bounded(pool: PgPool) {
    invite(&pool, "sam@gmail.com").await;
    let id = ready_clip(&pool, "google-oauth2|sam", "sam@gmail.com", "Ace").await;
    let app = app(pool).await;
    let sam = user_token("google-oauth2|sam", "sam@gmail.com");
    let path = format!("/api/clips/{id}");
    let patch = |body: Value| send(&app, "PATCH", &path, Some(&sam), Some(body), &[]);

    let description = "x".repeat(clipos_api::MAX_BODY_BYTES);
    let (status, body) = patch(json!({ "description": description })).await;
    assert_eq!(status, StatusCode::PAYLOAD_TOO_LARGE, "{body}");
    assert_eq!(json(&body)["error"], "too_large");

    // About as many tags as fit in a body.
    let tags: Vec<String> = (0..5_000).map(|i| format!("t{i}")).collect();
    assert!(json!({ "tags": tags }).to_string().len() < clipos_api::MAX_BODY_BYTES);
    let (status, body) = patch(json!({ "tags": tags })).await;
    assert_eq!(status, StatusCode::BAD_REQUEST, "{body}");
    assert_eq!(json(&body)["message"], "at most 10 tags");

    // Repeats still count once.
    let (status, body) =
        patch(json!({ "tags": ["Ace", "ace", "#ACE", "1v3 clutch", "1v3-clutch"] })).await;
    assert_eq!(status, StatusCode::OK, "{body}");
    assert_eq!(json(&body)["tags"], json!(["1v3-clutch", "ace"]));
    assert_eq!(json(&body)["title"], "Ace", "nothing else changed");
}

/// Titles need something visible, on one line; a description is trimmed before it's
/// measured.
#[sqlx::test(migrator = "clipos_core::db::MIGRATOR")]
async fn titles_and_descriptions_are_checked(pool: PgPool) {
    invite(&pool, "sam@gmail.com").await;
    let id = ready_clip(&pool, "google-oauth2|sam", "sam@gmail.com", "Ace").await;
    let app = app(pool).await;
    let sam = user_token("google-oauth2|sam", "sam@gmail.com");
    let create = |title: &str, description: String| {
        let body =
            json!({ "title": title, "description": description, "filename": "a.mp4", "bytes": 1 });
        send(&app, "POST", "/api/clips", Some(&sam), Some(body), &[])
    };
    for title in [
        "\u{200b}",
        " \u{202e}\u{feff} ",
        "\u{2060}\u{200d}",
        "line\nbreak",
        "a\rb",
    ] {
        let (status, body) = create(title, String::new()).await;
        assert_eq!(status, StatusCode::BAD_REQUEST, "{title:?}: {body}");
        let (status, _) = send(
            &app,
            "PATCH",
            &format!("/api/clips/{id}"),
            Some(&sam),
            Some(json!({ "title": title })),
            &[],
        )
        .await;
        assert_eq!(status, StatusCode::BAD_REQUEST, "{title:?}");
    }
    // A joiner inside an emoji is fine.
    let (status, body) = create("👨\u{200d}👩\u{200d}👧 ace", String::new()).await;
    assert_eq!(status, StatusCode::CREATED, "{body}");
    let (status, body) = create("x", format!("  {}  ", "d".repeat(2000))).await;
    assert_eq!(status, StatusCode::CREATED, "{body}");
    assert_eq!(
        json(&body)["clip"]["description"].as_str().unwrap().len(),
        2000
    );
    let (status, _) = create("x", "d".repeat(2001)).await;
    assert_eq!(status, StatusCode::BAD_REQUEST);
}

/// `clips.reaction_count` (sort=top and its cursors) stays equal to the reactions stored
/// when people react at the same time.
#[sqlx::test(migrator = "clipos_core::db::MIGRATOR")]
async fn reactions_at_the_same_time_keep_the_count(pool: PgPool) {
    invite(&pool, "sam@gmail.com").await;
    invite(&pool, "kim@gmail.com").await;
    let id = ready_clip(&pool, "google-oauth2|sam", "sam@gmail.com", "Ace").await;
    let clip: uuid::Uuid = id.parse().unwrap();
    let app = app(pool.clone()).await;
    let sam = user_token("google-oauth2|sam", "sam@gmail.com");
    let kim = user_token("google-oauth2|kim", "kim@gmail.com");
    let kim_id: uuid::Uuid = json(&get(&app, "/api/me", Some(&kim)).await.1)["id"]
        .as_str()
        .unwrap()
        .parse()
        .unwrap();
    let counts = || async {
        let row: (i32, i64) = sqlx::query_as(
            "SELECT reaction_count, (SELECT count(*) FROM reactions WHERE clip_id = $1)
               FROM clips WHERE id = $1",
        )
        .bind(clip)
        .fetch_one(&pool)
        .await
        .unwrap();
        (i64::from(row.0), row.1)
    };
    let emojis = [
        "%F0%9F%94%A5",
        "%F0%9F%98%82",
        "%F0%9F%92%80",
        "%F0%9F%90%90",
        "%F0%9F%98%AE",
        "%F0%9F%91%8F",
    ];
    let at_once = |method: &'static str, who: Vec<String>| {
        let mut tasks = Vec::new();
        for emoji in emojis {
            for token in &who {
                let req = Request::builder()
                    .method(method)
                    .uri(format!("/api/clips/{id}/reactions/{emoji}"))
                    .header(header::AUTHORIZATION, format!("Bearer {token}"))
                    .body(Body::empty())
                    .unwrap();
                let router = app.router.clone();
                tasks.push(tokio::spawn(async move {
                    router.oneshot(req).await.unwrap().status()
                }));
            }
        }
        tasks
    };
    for task in at_once("PUT", vec![sam.clone(), kim.clone()]) {
        assert_eq!(task.await.unwrap(), StatusCode::OK);
    }
    assert_eq!(counts().await, (12, 12));
    for task in at_once("DELETE", vec![kim.clone()]) {
        assert_eq!(task.await.unwrap(), StatusCode::OK);
    }
    assert_eq!(counts().await, (6, 6));

    // kim's reaction is mid-transaction, holding the clip row, while sam reacts: sam's
    // count has to include kim's once it lands.
    let mut kims = pool.begin().await.unwrap();
    sqlx::query(
        "WITH added AS (
             INSERT INTO reactions (clip_id, user_id, emoji) VALUES ($1, $2, '🔥')
             RETURNING clip_id)
         UPDATE clips SET reaction_count = reaction_count + 1 WHERE id = $1",
    )
    .bind(clip)
    .bind(kim_id)
    .execute(&mut *kims)
    .await
    .unwrap();
    let sams = {
        let router = app.router.clone();
        let req = Request::builder()
            .method("DELETE")
            .uri(format!("/api/clips/{id}/reactions/{}", emojis[1]))
            .header(header::AUTHORIZATION, format!("Bearer {sam}"))
            .body(Body::empty())
            .unwrap();
        tokio::spawn(async move { router.oneshot(req).await.unwrap().status() })
    };
    tokio::time::sleep(std::time::Duration::from_millis(300)).await;
    kims.commit().await.unwrap();
    assert_eq!(sams.await.unwrap(), StatusCode::OK);
    assert_eq!(counts().await, (6, 6));
}

/// Azurite with its containers made, when `CLIPOS_AZURITE` is set (CI and `make test`).
async fn azurite_storage() -> Option<Storage> {
    if std::env::var_os("CLIPOS_AZURITE").is_none() {
        eprintln!("CLIPOS_AZURITE not set; skipping");
        return None;
    }
    let storage = Storage::new(StorageConfig {
        account: "devstoreaccount1".into(),
        blob_endpoint: Some("http://127.0.0.1:10000/devstoreaccount1".into()),
        account_key: Some(AZURITE_KEY.into()),
    })
    .unwrap();
    storage.prepare_local(&[]).await.unwrap();
    Some(storage)
}

/// A shared clip's media with odd `Range` headers: a 416 for ones we don't pass on,
/// Blob Storage's own 416 past the end, and a 404 for a missing file. Never a 500.
#[sqlx::test(migrator = "clipos_core::db::MIGRATOR")]
async fn shared_media_refuses_odd_ranges(pool: PgPool) {
    let Some(storage) = azurite_storage().await else {
        return;
    };
    invite(&pool, "sam@gmail.com").await;
    let id = ready_clip(&pool, "google-oauth2|sam", "sam@gmail.com", "Ace").await;
    let app = app(pool).await;
    let sam = user_token("google-oauth2|sam", "sam@gmail.com");
    let dir = tempfile::tempdir().unwrap();
    let file = dir.path().join("v.mp4");
    std::fs::write(&file, vec![7u8; 1000]).unwrap();
    let blob = clipos_core::clips::playback_blob(id.parse().unwrap());
    storage
        .upload_file(Container::Playback, &blob, &file, "video/mp4")
        .await
        .unwrap();
    let (_, body) = send(
        &app,
        "POST",
        &format!("/api/clips/{id}/share"),
        Some(&sam),
        None,
        &[],
    )
    .await;
    let video = format!("{}/video.mp4", json(&body)["shareUrl"].as_str().unwrap())
        .replace("https://clips.example", "");
    for (range, want) in [
        ("bytes=0-9", StatusCode::PARTIAL_CONTENT),
        ("bytes=-10", StatusCode::PARTIAL_CONTENT),
        ("bytes=5000-", StatusCode::RANGE_NOT_SATISFIABLE),
        ("bytes=abc", StatusCode::RANGE_NOT_SATISFIABLE),
        ("bytes=10-5", StatusCode::RANGE_NOT_SATISFIABLE),
        ("bytes=0-0,5-6", StatusCode::RANGE_NOT_SATISFIABLE),
        ("items=0-1", StatusCode::RANGE_NOT_SATISFIABLE),
        ("bytes=-0", StatusCode::RANGE_NOT_SATISFIABLE),
        (
            "bytes=99999999999999999999-",
            StatusCode::RANGE_NOT_SATISFIABLE,
        ),
    ] {
        let res = raw(&app, &video, &[("range", range)]).await;
        assert_eq!(res.status(), want, "{range}");
    }
    // The last 10 bytes, which Blob Storage is asked for from where they start.
    let res = raw(&app, &video, &[("range", "bytes=-10")]).await;
    assert_eq!(res.headers()["content-range"], "bytes 990-999/1000");
    let res = raw(&app, &video, &[("range", "bytes=-5000")]).await;
    assert_eq!(res.headers()["content-range"], "bytes 0-999/1000");
    // The poster was never uploaded, so it has no last bytes either.
    let poster = video.replace("video.mp4", "poster.jpg");
    assert_eq!(
        raw(&app, &poster, &[]).await.status(),
        StatusCode::NOT_FOUND
    );
    assert_eq!(
        raw(&app, &poster, &[("range", "bytes=-10")]).await.status(),
        StatusCode::NOT_FOUND
    );
    // An empty file has no last 10 bytes.
    let empty = dir.path().join("empty.jpg");
    std::fs::write(&empty, b"").unwrap();
    let poster_blob = clipos_core::clips::poster_blob(id.parse().unwrap());
    storage
        .upload_file(Container::Posters, &poster_blob, &empty, "image/jpeg")
        .await
        .unwrap();
    assert_eq!(
        raw(&app, &poster, &[("range", "bytes=-10")]).await.status(),
        StatusCode::RANGE_NOT_SATISFIABLE
    );
}

/// Blob storage that answers every request with `status`, like a failing service.
async fn failing_storage(status: StatusCode) -> Storage {
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let addr = listener.local_addr().unwrap();
    let service = Router::new().fallback(move || async move { status });
    tokio::spawn(async move { axum::serve(listener, service).await.unwrap() });
    Storage::new(StorageConfig {
        account: "devstoreaccount1".into(),
        blob_endpoint: Some(format!("http://{addr}/devstoreaccount1")),
        account_key: Some(AZURITE_KEY.into()),
    })
    .unwrap()
}

/// When Blob Storage fails a share read, the player gets a 502 (not storage's own status
/// passed on as if it were ours), whether storage blamed the range or itself.
#[sqlx::test(migrator = "clipos_core::db::MIGRATOR")]
async fn shared_media_is_a_bad_gateway_when_storage_fails(pool: PgPool) {
    invite(&pool, "sam@gmail.com").await;
    let id = ready_clip(&pool, "google-oauth2|sam", "sam@gmail.com", "Ace").await;
    let app = app(pool).await;
    let sam = user_token("google-oauth2|sam", "sam@gmail.com");
    let (status, body) = send(
        &app,
        "POST",
        &format!("/api/clips/{id}/share"),
        Some(&sam),
        None,
        &[],
    )
    .await;
    assert_eq!(status, StatusCode::OK, "{body}");
    let video = format!("{}/video.mp4", json(&body)["shareUrl"].as_str().unwrap())
        .replace("https://clips.example", "");
    for (failure, range) in [
        (StatusCode::BAD_REQUEST, Some("bytes=0-9")),
        (StatusCode::INTERNAL_SERVER_ERROR, None),
        (StatusCode::SERVICE_UNAVAILABLE, Some("bytes=0-")),
        (StatusCode::FORBIDDEN, None),
    ] {
        let state = AppState {
            storage: Arc::new(failing_storage(failure).await),
            ..app.state.clone()
        };
        let broken = TestApp {
            router: clipos_api::router(state.clone(), app._static_dir.path()),
            state,
            _static_dir: tempfile::tempdir().unwrap(),
        };
        let headers: Vec<(&str, &str)> = range.map(|r| ("range", r)).into_iter().collect();
        let res = raw(&broken, &video, &headers).await;
        assert_eq!(res.status(), StatusCode::BAD_GATEWAY, "{failure}");
        let body = res.into_body().collect().await.unwrap().to_bytes();
        assert!(
            is_error_body(std::str::from_utf8(&body).unwrap()),
            "{failure}"
        );
    }
}

/// URL-safe base64 without padding, for making cursors by hand.
fn b64url(data: &[u8]) -> String {
    const ALPHABET: &[u8; 64] = b"ABCDEFGHIJKLMNOPQRSTUVWXYZabcdefghijklmnopqrstuvwxyz0123456789-_";
    let mut out = String::new();
    for chunk in data.chunks(3) {
        let b = [
            chunk[0],
            *chunk.get(1).unwrap_or(&0),
            *chunk.get(2).unwrap_or(&0),
        ];
        let n = (u32::from(b[0]) << 16) | (u32::from(b[1]) << 8) | u32::from(b[2]);
        for i in 0..=chunk.len() {
            out.push(ALPHABET[((n >> (18 - 6 * i)) & 63) as usize] as char);
        }
    }
    out
}

/// A tampered cursor is a 400 "bad cursor", whatever's in it; a far-off but possible
/// time just finds nothing.
#[sqlx::test(migrator = "clipos_core::db::MIGRATOR")]
async fn tampered_cursors_are_bad_requests(pool: PgPool) {
    invite(&pool, "sam@gmail.com").await;
    ready_clip(&pool, "google-oauth2|sam", "sam@gmail.com", "Ace").await;
    let app = app(pool).await;
    let sam = user_token("google-oauth2|sam", "sam@gmail.com");
    let nil = "00000000-0000-0000-0000-000000000000";
    let cursor = |count: &str, micros: &str| b64url(format!("{count}|{micros}|{nil}").as_bytes());
    let bad = [
        cursor("1", "99999999999999999999"),
        cursor("2147483648", "0"),
        cursor("0", "9223372036854775807"),
        // Past what chrono holds (262143 AD).
        cursor("0", "8210298412799999999"),
        // Before 4714 BC: chrono has it, Postgres doesn't.
        cursor("0", "-262135596800000000"),
        cursor("0", "-210866803200000001"),
        b64url(b"||"),
        "%00".into(),
    ];
    let fine = [
        cursor("-5", "0"),
        cursor("0", "-210866803200000000"),
        cursor("0", "-62135596800000000"),
        cursor("0", "253402300799999999"),
    ];
    for sort in ["new", "top"] {
        for c in &bad {
            let (status, body) = get(
                &app,
                &format!("/api/clips?sort={sort}&cursor={c}"),
                Some(&sam),
            )
            .await;
            assert_eq!(status, StatusCode::BAD_REQUEST, "{sort} {c}: {body}");
            assert_eq!(json(&body)["message"], "bad cursor");
        }
        for c in &fine {
            let (status, body) = get(
                &app,
                &format!("/api/clips?sort={sort}&cursor={c}"),
                Some(&sam),
            )
            .await;
            assert_eq!(status, StatusCode::OK, "{sort} {c}: {body}");
        }
    }
}

/// The janitor lists clips over a week in the trash, deletes their files, then the rows.
/// A restore can't land in between, and a clip restored before can't be purged.
#[sqlx::test(migrator = "clipos_core::db::MIGRATOR")]
async fn a_restore_never_races_the_purge(pool: PgPool) {
    use clipos_core::clips;
    invite(&pool, "sam@gmail.com").await;
    let old = ready_clip(&pool, "google-oauth2|sam", "sam@gmail.com", "Old").await;
    let recent = ready_clip(&pool, "google-oauth2|sam", "sam@gmail.com", "Recent").await;
    let app = app(pool.clone()).await;
    let sam = user_token("google-oauth2|sam", "sam@gmail.com");
    for (id, days) in [(&old, 8), (&recent, 6)] {
        send(
            &app,
            "DELETE",
            &format!("/api/clips/{id}"),
            Some(&sam),
            None,
            &[],
        )
        .await;
        sqlx::query(
            "UPDATE clips SET deleted_at = now() - make_interval(days => $2) WHERE id = $1::uuid",
        )
        .bind(id)
        .bind(days)
        .execute(&pool)
        .await
        .unwrap();
    }
    let expired = clips::expired_trash(&pool).await.unwrap();
    assert_eq!(expired.len(), 1);
    assert_eq!(expired[0].id.to_string(), old);

    // Over a week: the files may be going, so it stays in the trash.
    let (status, body) = send(
        &app,
        "POST",
        &format!("/api/clips/{old}/restore"),
        Some(&sam),
        None,
        &[],
    )
    .await;
    assert_eq!(status, StatusCode::CONFLICT, "{body}");
    clips::purge(&pool, expired[0].id).await.unwrap();
    let (status, _) = get(&app, &format!("/api/clips/{old}"), Some(&sam)).await;
    assert_eq!(status, StatusCode::NOT_FOUND);

    // Restored in time: a purge that still had it listed leaves it alone.
    let (status, body) = send(
        &app,
        "POST",
        &format!("/api/clips/{recent}/restore"),
        Some(&sam),
        None,
        &[],
    )
    .await;
    assert_eq!(status, StatusCode::OK, "{body}");
    clips::purge(&pool, recent.parse().unwrap()).await.unwrap();
    let (status, _) = get(&app, &format!("/api/clips/{recent}"), Some(&sam)).await;
    assert_eq!(status, StatusCode::OK);
}

/// The upload SAS lasts minutes, not hours, and `complete` keeps the original's ETag for
/// the worker to check (the SAS still writes until it expires).
#[sqlx::test(migrator = "clipos_core::db::MIGRATOR")]
async fn an_upload_is_short_lived_and_completes_once(pool: PgPool) {
    if azurite_storage().await.is_none() {
        return;
    }
    invite(&pool, "sam@gmail.com").await;
    let app = app(pool.clone()).await;
    let sam = user_token("google-oauth2|sam", "sam@gmail.com");
    let new = json!({ "title": "x", "filename": "a.mp4", "bytes": 5 });
    let (_, body) = send(&app, "POST", "/api/clips", Some(&sam), Some(new), &[]).await;
    let created = json(&body);
    let id = created["clip"]["id"].as_str().unwrap().to_owned();
    let url = created["uploadUrl"].as_str().unwrap().to_owned();
    let expiry = reqwest::Url::parse(&url)
        .unwrap()
        .query_pairs()
        .find(|(k, _)| k == "se")
        .unwrap()
        .1
        .parse::<chrono::DateTime<chrono::Utc>>()
        .unwrap();
    let left = expiry - chrono::Utc::now();
    assert!(
        left > chrono::Duration::minutes(15) && left <= chrono::Duration::minutes(16),
        "{left}"
    );

    let put = |data: &'static str| {
        reqwest::Client::new()
            .put(&url)
            .header("x-ms-blob-type", "BlockBlob")
            .body(data)
            .send()
    };
    let complete = format!("/api/clips/{id}/complete");
    put("hello world").await.unwrap();
    let (status, body) = send(&app, "POST", &complete, Some(&sam), None, &[]).await;
    assert_eq!(status, StatusCode::BAD_REQUEST);
    assert_eq!(
        json(&body)["message"],
        "the upload is bigger than declared: 11 bytes, not 5"
    );

    // Two completes at once: one transcode, and the ETag of what was uploaded.
    let etag = put("hello").await.unwrap().headers()["etag"]
        .to_str()
        .unwrap()
        .to_owned();
    let (a, b) = tokio::join!(
        send(&app, "POST", &complete, Some(&sam), None, &[]),
        send(&app, "POST", &complete, Some(&sam), None, &[])
    );
    assert_eq!(
        (a.0, b.0),
        (StatusCode::OK, StatusCode::OK),
        "{} {}",
        a.1,
        b.1
    );
    let (jobs, kept): (i64, Option<String>) = sqlx::query_as(
        "SELECT (SELECT count(*) FROM jobs WHERE kind = 'transcode'), original_etag
           FROM clips WHERE id = $1::uuid",
    )
    .bind(&id)
    .fetch_one(&pool)
    .await
    .unwrap();
    assert_eq!((jobs, kept), (1, Some(etag)));
}

/// Tag autocomplete offers only tags of clips the viewer sees, and a tag filter with no
/// usable tag in it matches no clip instead of every clip.
#[sqlx::test(migrator = "clipos_core::db::MIGRATOR")]
async fn tags_only_find_what_the_viewer_sees(pool: PgPool) {
    use clipos_core::{clips, social};
    invite(&pool, "sam@gmail.com").await;
    invite(&pool, "kim@gmail.com").await;
    let ace = ready_clip(&pool, "google-oauth2|sam", "sam@gmail.com", "Ace").await;
    let held = ready_clip(&pool, "google-oauth2|sam", "sam@gmail.com", "Held").await;
    let trashed = ready_clip(&pool, "google-oauth2|sam", "sam@gmail.com", "Old").await;
    for (id, tag) in [
        (&ace, "zz-ace"),
        (&held, "zz-knife-ace"),
        (&trashed, "zz-trashed"),
    ] {
        social::set_tags(&pool, id.parse().unwrap(), &[tag.into()])
            .await
            .unwrap();
    }
    clips::hold(&pool, held.parse().unwrap()).await.unwrap();
    clips::soft_delete(&pool, trashed.parse().unwrap())
        .await
        .unwrap();
    let app = app_with(pool, true).await;
    let sam = user_token("google-oauth2|sam", "sam@gmail.com");
    let kim = user_token("google-oauth2|kim", "kim@gmail.com");
    let (_, body) = get(&app, "/api/tags?q=zz", Some(&kim)).await;
    assert_eq!(json(&body), json!(["zz-ace"]));
    let (_, body) = get(&app, "/api/tags?q=zz", Some(&sam)).await;
    assert_eq!(json(&body), json!(["zz-ace", "zz-knife-ace"]));

    for query in ["tag=%21%21%21", "tag=%2C", "tag=%23", "tag=%21%21%21,%23"] {
        let (status, body) = get(&app, &format!("/api/clips?{query}"), Some(&sam)).await;
        assert_eq!(status, StatusCode::OK, "{body}");
        assert_eq!(json(&body)["clips"], json!([]), "{query}");
    }
    // An empty one is no filter, and a junk tag beside a real one is just ignored.
    for (query, want) in [("tag=", 2), ("tag=%21%21%21,zz-ace", 1)] {
        let (_, body) = get(&app, &format!("/api/clips?{query}"), Some(&sam)).await;
        assert_eq!(
            json(&body)["clips"].as_array().unwrap().len(),
            want,
            "{query}"
        );
    }
}

/// Clips added to a show at the same moment each get a place of their own.
#[sqlx::test(migrator = "clipos_core::db::MIGRATOR")]
async fn clips_added_at_once_get_a_place_each(pool: PgPool) {
    let admin = admin_token(&pool).await;
    invite(&pool, "kim@gmail.com").await;
    let app = app_with(pool.clone(), true).await;
    let kim = user_token("google-oauth2|kim", "kim@gmail.com");
    let show = new_show(&app, &admin, true).await;
    send(
        &app,
        "POST",
        &format!("/api/shows/{show}/join"),
        Some(&kim),
        None,
        &[],
    )
    .await;
    let mut clips = Vec::new();
    for i in 0..8 {
        clips.push(
            ready_clip(
                &pool,
                "google-oauth2|kim",
                "kim@gmail.com",
                &format!("k{i}"),
            )
            .await,
        );
    }
    let mut tasks = Vec::new();
    for (i, clip) in clips.iter().enumerate() {
        let who = if i % 2 == 0 { &kim } else { &admin };
        let req = Request::post(format!("/api/shows/{show}/clips"))
            .header(header::AUTHORIZATION, format!("Bearer {who}"))
            .header(header::CONTENT_TYPE, "application/json")
            .body(Body::from(json!({ "clipId": clip }).to_string()))
            .unwrap();
        let router = app.router.clone();
        tasks.push(tokio::spawn(async move {
            router.oneshot(req).await.unwrap().status()
        }));
    }
    for task in tasks {
        assert_eq!(task.await.unwrap(), StatusCode::OK);
    }
    let positions: Vec<i32> =
        sqlx::query_scalar("SELECT position FROM show_clips WHERE show_id = $1::uuid ORDER BY 1")
            .bind(&show)
            .fetch_all(&pool)
            .await
            .unwrap();
    assert_eq!(positions, (0..8).collect::<Vec<_>>());
}

/// `PUT /lineup` while a clip starts playing (the `mark_played` update, held open in a
/// transaction): the edit waits for it and keeps the clip as played, not dropped.
#[sqlx::test(migrator = "clipos_core::db::MIGRATOR")]
async fn a_lineup_edit_never_drops_a_clip_that_just_played(pool: PgPool) {
    let admin = admin_token(&pool).await;
    invite(&pool, "sam@gmail.com").await;
    let a = ready_clip(&pool, "google-oauth2|sam", "sam@gmail.com", "A").await;
    let b = ready_clip(&pool, "google-oauth2|sam", "sam@gmail.com", "B").await;
    let app = app_with(pool.clone(), true).await;
    let show = new_show(&app, &admin, true).await;

    let mut play = pool.begin().await.unwrap();
    sqlx::query(
        "UPDATE show_clips SET played_at = COALESCE(played_at, now())
          WHERE show_id = $1::uuid AND clip_id = $2::uuid AND NOT dropped",
    )
    .bind(&show)
    .bind(&a)
    .execute(&mut *play)
    .await
    .unwrap();
    let lineup_path = format!("/api/shows/{show}/lineup");
    let edit = send(
        &app,
        "PUT",
        &lineup_path,
        Some(&admin),
        Some(json!({ "clipIds": [b] })),
        &[],
    );
    let commit = async {
        tokio::time::sleep(std::time::Duration::from_millis(300)).await;
        play.commit().await.unwrap();
    };
    let ((status, body), ()) = tokio::join!(edit, commit);
    assert_eq!(status, StatusCode::OK, "{body}");
    let lineup = &json(&body)["lineup"];
    assert_eq!(
        (&lineup[0]["clip"]["id"], &lineup[0]["dropped"]),
        (&json!(a), &json!(false))
    );
    assert!(lineup[0]["playedAt"].is_string());
    assert_eq!(lineup[1]["clip"]["id"], json!(b));
}

/// Show reactions: none on a clip in the trash (its page takes none either), and the
/// moment has to be within the clip.
#[sqlx::test(migrator = "clipos_core::db::MIGRATOR")]
async fn show_reactions_stay_within_the_clip(pool: PgPool) {
    let admin = admin_token(&pool).await;
    invite(&pool, "sam@gmail.com").await;
    let a = ready_clip(&pool, "google-oauth2|sam", "sam@gmail.com", "A").await;
    let b = ready_clip(&pool, "google-oauth2|sam", "sam@gmail.com", "B").await;
    let app = app_with(pool.clone(), true).await;
    let sam = user_token("google-oauth2|sam", "sam@gmail.com");
    let show = new_show(&app, &admin, true).await;
    for clip in [&a, &b] {
        send(
            &app,
            "POST",
            &format!("/api/shows/{show}/clips/{clip}/played"),
            Some(&admin),
            None,
            &[],
        )
        .await;
    }
    let reactions = format!("/api/shows/{show}/reactions");
    let tap = |clip: &str, at_ms: i64| {
        let body = json!({ "clipId": clip, "emoji": "🔥", "atMs": at_ms });
        send(&app, "POST", &reactions, Some(&admin), Some(body), &[])
    };
    // The clips are 5 s long: up to a second past the end is fine.
    assert_eq!(tap(&a, 6_000).await.0, StatusCode::NO_CONTENT);
    let (status, body) = tap(&a, 6_001).await;
    assert_eq!(status, StatusCode::BAD_REQUEST, "{body}");

    send(
        &app,
        "DELETE",
        &format!("/api/clips/{b}"),
        Some(&sam),
        None,
        &[],
    )
    .await;
    let (status, body) = tap(&b, 10).await;
    assert_eq!(status, StatusCode::BAD_REQUEST, "{body}");
    let on_page: Vec<String> = sqlx::query_scalar("SELECT clip_id::text FROM reactions")
        .fetch_all(&pool)
        .await
        .unwrap();
    assert_eq!(on_page, [a]);
}

/// Someone else's clip saved for the show can't be pulled into a live show, which would
/// hand everyone in it the playback link. To anyone but its uploader it doesn't exist.
#[sqlx::test(migrator = "clipos_core::db::MIGRATOR")]
async fn only_the_uploader_adds_a_held_clip_to_a_show(pool: PgPool) {
    let admin = admin_token(&pool).await;
    invite(&pool, "sam@gmail.com").await;
    invite(&pool, "lee@gmail.com").await;
    let app = app_with(pool.clone(), true).await;
    let sam = user_token("google-oauth2|sam", "sam@gmail.com");
    let lee = user_token("google-oauth2|lee", "lee@gmail.com");
    let show = new_show(&app, &admin, false).await;
    for who in [&sam, &lee] {
        send(
            &app,
            "POST",
            &format!("/api/shows/{show}/join"),
            Some(who),
            None,
            &[],
        )
        .await;
    }
    send(
        &app,
        "POST",
        &format!("/api/shows/{show}/start"),
        Some(&admin),
        None,
        &[],
    )
    .await;
    // Uploaded and saved while the show is live: it belongs to the next show.
    let held = ready_clip(&pool, "google-oauth2|sam", "sam@gmail.com", "Spoiler").await;
    clipos_core::clips::hold(&pool, held.parse().unwrap())
        .await
        .unwrap();
    let add_path = format!("/api/shows/{show}/clips");
    let add = |who: String| {
        let (app, path, body) = (&app, &add_path, json!({ "clipId": held }));
        async move { send(app, "POST", path, Some(&who), Some(body), &[]).await }
    };
    for who in [&lee, &admin] {
        let (status, body) = add(who.clone()).await;
        assert_eq!(status, StatusCode::NOT_FOUND, "{body}");
        // Like a clip that doesn't exist.
        let (status, _) = send(
            &app,
            "POST",
            &format!("/api/shows/{show}/clips"),
            Some(who),
            Some(json!({ "clipId": uuid::Uuid::new_v4() })),
            &[],
        )
        .await;
        assert_eq!(status, StatusCode::NOT_FOUND);
        let (status, body) = get(&app, &format!("/api/clips/{held}"), Some(who)).await;
        assert_eq!(status, StatusCode::NOT_FOUND, "{body}");
    }
    // Its uploader can, and then it plays for everyone in the show.
    let (status, body) = add(sam.clone()).await;
    assert_eq!(status, StatusCode::OK, "{body}");
    let (status, body) = get(&app, &format!("/api/clips/{held}"), Some(&lee)).await;
    assert_eq!(status, StatusCode::OK, "{body}");
    assert!(json(&body)["playbackUrl"].is_string());
}

/// A held clip plays (which releases it), then everyone leaves and the show is
/// abandoned: the clip is back in tonight's lineup, and saved for the show again
/// (decision 41). One that wasn't held stays as it was.
#[sqlx::test(migrator = "clipos_core::db::MIGRATOR")]
async fn an_abandoned_show_gives_back_the_holds_it_released(pool: PgPool) {
    use clipos_core::{clips, shows};
    let admin = admin_token(&pool).await;
    invite(&pool, "sam@gmail.com").await;
    let held = ready_clip(&pool, "google-oauth2|sam", "sam@gmail.com", "Held").await;
    let posted = ready_clip(&pool, "google-oauth2|sam", "sam@gmail.com", "Posted").await;
    let held_id: uuid::Uuid = held.parse().unwrap();
    assert!(clips::hold(&pool, held_id).await.unwrap());
    let app = app_with(pool.clone(), true).await;
    let show = new_show(&app, &admin, true).await;
    for clip in [&held, &posted] {
        let (status, _) = send(
            &app,
            "POST",
            &format!("/api/shows/{show}/clips/{clip}/played"),
            Some(&admin),
            None,
            &[],
        )
        .await;
        assert_eq!(status, StatusCode::OK);
    }
    // Played twice (a replay): still known as released by this show.
    send(
        &app,
        "POST",
        &format!("/api/shows/{show}/clips/{held}/played"),
        Some(&admin),
        None,
        &[],
    )
    .await;
    assert!(!clips::get(&pool, held_id).await.unwrap().unwrap().is_held());

    shows::abandon(&pool, show.parse().unwrap()).await.unwrap();
    assert_eq!(
        shows::tonight(&pool).await.unwrap(),
        [held_id, posted.parse().unwrap()]
    );
    let clip = clips::get(&pool, held_id).await.unwrap().unwrap();
    assert_eq!(
        clip.hold_until,
        Some(clip.created_at + chrono::Duration::days(7))
    );
    let clip = clips::get(&pool, posted.parse().unwrap())
        .await
        .unwrap()
        .unwrap();
    assert_eq!(clip.hold_until, None);
}

/// Moving a clip to the trash revokes its share link, so a restore doesn't bring the old
/// link back; revoking it by hand works in the trash too.
#[sqlx::test(migrator = "clipos_core::db::MIGRATOR")]
async fn a_trashed_clip_loses_its_share_link(pool: PgPool) {
    invite(&pool, "sam@gmail.com").await;
    let a = ready_clip(&pool, "google-oauth2|sam", "sam@gmail.com", "A").await;
    let app = app(pool).await;
    let sam = user_token("google-oauth2|sam", "sam@gmail.com");
    let share = format!("/api/clips/{a}/share");
    let (_, body) = send(&app, "POST", &share, Some(&sam), None, &[]).await;
    let link = json(&body)["shareUrl"]
        .as_str()
        .unwrap()
        .replace("https://clips.example", "");
    let (status, _) = get(&app, &format!("{link}/clip.json"), None).await;
    assert_eq!(status, StatusCode::OK);

    send(
        &app,
        "DELETE",
        &format!("/api/clips/{a}"),
        Some(&sam),
        None,
        &[],
    )
    .await;
    let (status, body) = send(&app, "DELETE", &share, Some(&sam), None, &[]).await;
    assert_eq!(status, StatusCode::OK, "{body}");
    let (status, body) = send(
        &app,
        "POST",
        &format!("/api/clips/{a}/restore"),
        Some(&sam),
        None,
        &[],
    )
    .await;
    assert_eq!(status, StatusCode::OK, "{body}");
    assert!(json(&body)["shareUrl"].is_null());
    let (status, _) = get(&app, &format!("{link}/clip.json"), None).await;
    assert_eq!(status, StatusCode::NOT_FOUND, "the old link stays dead");
}

/// Moving a clip in the show's lineup to the trash tells the live room, which unloads it.
#[sqlx::test(migrator = "clipos_core::db::MIGRATOR")]
async fn trashing_a_clip_in_the_lineup_tells_the_live_room(pool: PgPool) {
    let admin = admin_token(&pool).await;
    invite(&pool, "sam@gmail.com").await;
    let a = ready_clip(&pool, "google-oauth2|sam", "sam@gmail.com", "A").await;
    let app = app_with(pool, true).await;
    let sam = user_token("google-oauth2|sam", "sam@gmail.com");
    let show = new_show(&app, &admin, true).await;
    let addr = listen(&app).await;
    let (mut host, _) = ws_join(addr, &show, &admin).await;
    send(
        &app,
        "DELETE",
        &format!("/api/clips/{a}"),
        Some(&sam),
        None,
        &[],
    )
    .await;
    ws_next(&mut host, "showChanged").await;
}

/// Someone else's clip that's processing or failed is a 404 for everyone but its uploader
/// and admins (decision 42): the page, the download, the analysis and reactions. A
/// profile's counts match what its lists show the same viewer.
#[sqlx::test(migrator = "clipos_core::db::MIGRATOR")]
async fn unpublished_clips_are_their_uploaders_and_admins(pool: PgPool) {
    use clipos_core::clips;
    let admin = admin_token(&pool).await;
    invite(&pool, "sam@gmail.com").await;
    invite(&pool, "kim@gmail.com").await;
    let failed = ready_clip(&pool, "google-oauth2|sam", "sam@gmail.com", "Failed").await;
    let processing = ready_clip(&pool, "google-oauth2|sam", "sam@gmail.com", "Busy").await;
    ready_clip(&pool, "google-oauth2|sam", "sam@gmail.com", "Ready").await;
    // Both still processing; then one fails (`set_failed` leaves a ready clip alone).
    for id in [&failed, &processing] {
        sqlx::query("UPDATE clips SET status = 'processing' WHERE id = $1::uuid")
            .bind(id)
            .execute(&pool)
            .await
            .unwrap();
    }
    clips::set_failed(&pool, failed.parse().unwrap(), clips::TOO_LONG_ERROR)
        .await
        .unwrap();
    let app = app(pool).await;
    let sam = user_token("google-oauth2|sam", "sam@gmail.com");
    let kim = user_token("google-oauth2|kim", "kim@gmail.com");
    for id in [&failed, &processing] {
        for (who, want) in [
            (&kim, StatusCode::NOT_FOUND),
            (&sam, StatusCode::OK),
            (&admin, StatusCode::OK),
        ] {
            for sub in ["", "/download"] {
                let (status, body) = get(&app, &format!("/api/clips/{id}{sub}"), Some(who)).await;
                assert_eq!(status, want, "{id}{sub}: {body}");
            }
        }
        let (status, _) = get(&app, &format!("/api/clips/{id}/analysis"), Some(&kim)).await;
        assert_eq!(status, StatusCode::NOT_FOUND);
        let (status, _) = send(
            &app,
            "PUT",
            &format!("/api/clips/{id}/reactions/%F0%9F%94%A5"),
            Some(&kim),
            None,
            &[],
        )
        .await;
        assert_eq!(status, StatusCode::NOT_FOUND);
    }

    for (who, want) in [(&sam, 3), (&kim, 1)] {
        let (_, profile) = get(&app, "/api/users/sam", Some(who)).await;
        let (_, list) = get(&app, "/api/clips?uploader=sam", Some(who)).await;
        assert_eq!(json(&profile)["clipCount"], want);
        assert_eq!(json(&list)["clips"].as_array().unwrap().len(), want);
    }
}

/// During the finale nobody sees the counts, only who has voted; they're in the show
/// once it has ended (decision 43).
#[sqlx::test(migrator = "clipos_core::db::MIGRATOR")]
async fn vote_counts_stay_hidden_until_the_show_ends(pool: PgPool) {
    let admin = admin_token(&pool).await;
    invite(&pool, "sam@gmail.com").await;
    invite(&pool, "kim@gmail.com").await;
    let a = ready_clip(&pool, "google-oauth2|sam", "sam@gmail.com", "A").await;
    let app = app_with(pool, true).await;
    let kim = user_token("google-oauth2|kim", "kim@gmail.com");
    let kim_id = json(&get(&app, "/api/me", Some(&kim)).await.1)["id"].clone();
    let show = new_show(&app, &admin, false).await;
    let call = |method: &'static str, path: String, who: String, body: Option<Value>| {
        let app = &app;
        async move { send(app, method, &path, Some(&who), body, &[]).await }
    };
    call("POST", format!("/api/shows/{show}/join"), kim.clone(), None).await;
    call(
        "POST",
        format!("/api/shows/{show}/start"),
        admin.clone(),
        None,
    )
    .await;
    call(
        "POST",
        format!("/api/shows/{show}/clips/{a}/played"),
        admin.clone(),
        None,
    )
    .await;
    call(
        "POST",
        format!("/api/shows/{show}/finale"),
        admin.clone(),
        None,
    )
    .await;
    let (status, body) = call(
        "PUT",
        format!("/api/shows/{show}/votes/clip"),
        kim.clone(),
        Some(json!({ "clipId": a })),
    )
    .await;
    assert_eq!(status, StatusCode::OK, "{body}");
    assert_eq!(json(&body)["myVotes"]["clip"], json!(a));

    for who in [&admin, &kim] {
        let (_, body) = call("GET", format!("/api/shows/{show}"), who.clone(), None).await;
        let view = json(&body);
        assert_eq!(view["votes"], json!([]));
        assert_eq!(view["voters"], json!({ "clip": [kim_id], "fail": [] }));
        let (_, body) = call("GET", "/api/shows/tonight".into(), who.clone(), None).await;
        assert_eq!(json(&body)["show"]["votes"], json!([]));
    }

    let (status, body) = call(
        "POST",
        format!("/api/shows/{show}/end"),
        admin.clone(),
        None,
    )
    .await;
    assert_eq!(status, StatusCode::OK, "{body}");
    assert_eq!(
        json(&body)["votes"],
        json!([{ "category": "clip", "clipId": a, "votes": 1 }])
    );
}

/// Show bodies refuse fields they don't know, like every other body.
#[sqlx::test(migrator = "clipos_core::db::MIGRATOR")]
async fn show_bodies_refuse_unknown_fields(pool: PgPool) {
    let admin = admin_token(&pool).await;
    let app = app(pool).await;
    let show = new_show(&app, &admin, false).await;
    let nil = uuid::Uuid::nil();
    for (method, path, body) in [
        ("PUT", "lineup", json!({ "clipIds": [], "x": 1 })),
        ("POST", "clips", json!({ "clipId": nil, "x": 1 })),
        ("PUT", "ready", json!({ "ready": true, "x": 1 })),
        (
            "POST",
            "reactions",
            json!({ "clipId": nil, "emoji": "🔥", "atMs": 0, "x": 1 }),
        ),
        ("PUT", "votes/clip", json!({ "clipId": nil, "x": 1 })),
        (
            "POST",
            "end",
            json!({ "tieBreak": { "clip": nil, "x": 1 } }),
        ),
    ] {
        let path = format!("/api/shows/{show}/{path}");
        let (status, body) = send(&app, method, &path, Some(&admin), Some(body), &[]).await;
        assert_eq!(status, StatusCode::UNPROCESSABLE_ENTITY, "{path}: {body}");
    }
}

// ---------------------------------------------------------------------------
// The show's live connection (S5), over a real socket.
// ---------------------------------------------------------------------------

type Ws =
    tokio_tungstenite::WebSocketStream<tokio_tungstenite::MaybeTlsStream<tokio::net::TcpStream>>;

/// Serves the app on a free local port, for clients that need a real socket.
async fn listen(app: &TestApp) -> std::net::SocketAddr {
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let addr = listener.local_addr().unwrap();
    let router = app.router.clone();
    tokio::spawn(async move {
        axum::serve(
            listener,
            router.into_make_service_with_connect_info::<std::net::SocketAddr>(),
        )
        .await
        .unwrap();
    });
    addr
}

async fn ws_send(ws: &mut Ws, msg: Value) {
    use futures_util::SinkExt;
    ws.send(tokio_tungstenite::tungstenite::Message::Text(
        msg.to_string().into(),
    ))
    .await
    .unwrap();
}

/// The next message of this type (others are skipped), within 3 s.
async fn ws_next(ws: &mut Ws, kind: &str) -> Value {
    use futures_util::StreamExt;
    tokio::time::timeout(std::time::Duration::from_secs(3), async {
        loop {
            match ws.next().await.expect("socket open").unwrap() {
                tokio_tungstenite::tungstenite::Message::Text(t) => {
                    let v: Value = serde_json::from_str(&t).unwrap();
                    if v["type"] == kind {
                        return v;
                    }
                }
                tokio_tungstenite::tungstenite::Message::Close(_) => {
                    panic!("closed waiting for {kind}")
                }
                _ => {}
            }
        }
    })
    .await
    .unwrap_or_else(|_| panic!("no {kind} message"))
}

/// Connects to the show's live room and says hello; returns the socket and the welcome.
async fn ws_join(addr: std::net::SocketAddr, show: &str, token: &str) -> (Ws, Value) {
    let (mut ws, _) =
        tokio_tungstenite::connect_async(format!("ws://{addr}/api/shows/{show}/live"))
            .await
            .unwrap();
    ws_send(&mut ws, json!({ "type": "hello", "token": token })).await;
    let welcome = ws_next(&mut ws, "welcome").await;
    (ws, welcome)
}

#[sqlx::test(migrator = "clipos_core::db::MIGRATOR")]
async fn the_live_room_keeps_everyone_on_the_hosts_clip(pool: PgPool) {
    let admin = admin_token(&pool).await;
    invite(&pool, "kim@gmail.com").await;
    let a = ready_clip(&pool, "google-oauth2|kim", "kim@gmail.com", "A").await;
    // Saved for the show: it stays held until it plays.
    clipos_core::clips::hold(&pool, a.parse().unwrap())
        .await
        .unwrap();
    let app = app_with(pool.clone(), true).await;
    let kim = user_token("google-oauth2|kim", "kim@gmail.com");
    let (_, body) = send(&app, "POST", "/api/shows", Some(&admin), None, &[]).await;
    let show = json(&body)["id"].as_str().unwrap().to_owned();
    send(
        &app,
        "POST",
        &format!("/api/shows/{show}/start"),
        Some(&admin),
        None,
        &[],
    )
    .await;
    let addr = listen(&app).await;

    let (mut host, welcome) = ws_join(addr, &show, &admin).await;
    assert_eq!(welcome["state"]["clipId"], Value::Null);
    let (mut friend, welcome) = ws_join(addr, &show, &kim).await;
    assert_eq!(welcome["presence"]["online"].as_array().unwrap().len(), 2);
    // Connecting joined kim to the show.
    let (_, body) = get(&app, &format!("/api/shows/{show}"), Some(&kim)).await;
    assert_eq!(json(&body)["participants"].as_array().unwrap().len(), 2);

    // The host loads a clip: everyone gets it, paused at 0.
    ws_send(&mut host, json!({ "type": "load", "clipId": a })).await;
    for ws in [&mut host, &mut friend] {
        let state = ws_next(ws, "state").await;
        assert_eq!(state["state"]["clipId"], json!(a));
        assert_eq!(state["state"]["playing"], false);
        assert_eq!(state["state"]["seq"], 1);
    }
    let held = || async {
        clipos_core::clips::get(&pool, a.parse().unwrap())
            .await
            .unwrap()
            .unwrap()
            .is_held()
    };
    assert!(held().await);
    // Play is scheduled a moment ahead so everyone starts together.
    let before = clipos_api::live::now_ms();
    ws_send(&mut host, json!({ "type": "play" })).await;
    let state = ws_next(&mut friend, "state").await["state"].clone();
    assert_eq!(state["playing"], true);
    assert!(state["atServerMs"].as_f64().unwrap() >= before + 300.0);
    // Playing it for everyone counts it as played, which releases it.
    let (_, body) = get(&app, &format!("/api/shows/{show}"), Some(&kim)).await;
    assert!(json(&body)["lineup"][0]["playedAt"].is_string());
    assert!(!held().await);

    // Anyone in the show steers, not only the host.
    ws_next(&mut host, "state").await;
    ws_send(&mut friend, json!({ "type": "pause" })).await;
    for ws in [&mut host, &mut friend] {
        assert_eq!(ws_next(ws, "state").await["state"]["playing"], false);
    }
    ws_send(&mut host, json!({ "type": "seek", "positionMs": 1500 })).await;
    let state = ws_next(&mut friend, "state").await["state"].clone();
    assert_eq!(state["positionMs"], 1500.0);

    // Clock sync and reactions.
    ws_send(
        &mut friend,
        json!({ "type": "ping", "clientMs": 42.5, "rttMs": 30 }),
    )
    .await;
    let pong = ws_next(&mut friend, "pong").await;
    assert_eq!(pong["clientMs"], 42.5);
    assert!(pong["serverMs"].as_f64().unwrap() > before);
    ws_send(
        &mut friend,
        json!({ "type": "react", "clipId": a, "emoji": "🍌", "atMs": 1200 }),
    )
    .await;
    let reaction = ws_next(&mut host, "reaction").await;
    assert_eq!(
        (reaction["emoji"].as_str(), reaction["atMs"].as_i64()),
        (Some("🍌"), Some(1200))
    );

    // REST changes reach the room; the host can't be taken over while connected.
    send(
        &app,
        "POST",
        &format!("/api/shows/{show}/finale"),
        Some(&admin),
        None,
        &[],
    )
    .await;
    ws_next(&mut friend, "showChanged").await;
    ws_send(&mut friend, json!({ "type": "takeOver" })).await;
    ws_next(&mut friend, "error").await;

    // A restart (a new hub) resumes from the saved state.
    drop((host, friend));
    let restarted = app_with(pool, true).await;
    let addr = listen(&restarted).await;
    let (_, welcome) = ws_join(addr, &show, &kim).await;
    assert_eq!(welcome["state"]["clipId"], json!(a));
    assert_eq!(welcome["state"]["positionMs"], 1500.0);
    assert_eq!(welcome["state"]["seq"], 4);
}

#[sqlx::test(migrator = "clipos_core::db::MIGRATOR")]
async fn the_live_room_wants_hello_and_a_valid_token(pool: PgPool) {
    let admin = admin_token(&pool).await;
    let app = app_with(pool, true).await;
    let show = new_show(&app, &admin, false).await;
    let addr = listen(&app).await;
    for first in [
        json!({ "type": "play" }),
        json!({ "type": "hello", "token": "nope" }),
    ] {
        let mut ws = ws_open(addr, &show).await;
        ws_send(&mut ws, first).await;
        // An error saying why, then the close: a bad token, so the client tries once more.
        let (seen, code) = ws_close(&mut ws).await;
        assert_eq!(seen.last().unwrap()["type"], "error");
        assert_eq!(code, 4001);
    }
}

/// Creates a show as `host` (and starts it when `start`); returns its id.
async fn new_show(app: &TestApp, host: &str, start: bool) -> String {
    let (status, body) = send(app, "POST", "/api/shows", Some(host), None, &[]).await;
    assert_eq!(status, StatusCode::CREATED, "{body}");
    let show = json(&body)["id"].as_str().unwrap().to_owned();
    if start {
        let path = format!("/api/shows/{show}/start");
        let (status, body) = send(app, "POST", &path, Some(host), None, &[]).await;
        assert_eq!(status, StatusCode::OK, "{body}");
    }
    show
}

/// A connection to the show's live room that hasn't said anything yet.
async fn ws_open(addr: std::net::SocketAddr, show: &str) -> Ws {
    tokio_tungstenite::connect_async(format!("ws://{addr}/api/shows/{show}/live"))
        .await
        .unwrap()
        .0
}

/// Reads until the server closes: the messages on the way, and the close code.
async fn ws_close(ws: &mut Ws) -> (Vec<Value>, u16) {
    use futures_util::StreamExt;
    use tokio_tungstenite::tungstenite::Message;
    tokio::time::timeout(std::time::Duration::from_secs(3), async {
        let mut seen = Vec::new();
        loop {
            match ws
                .next()
                .await
                .expect("socket open")
                .expect("a clean close")
            {
                Message::Text(t) => seen.push(serde_json::from_str(&t).unwrap()),
                Message::Close(frame) => return (seen, frame.map_or(0, |f| f.code.into())),
                _ => {}
            }
        }
    })
    .await
    .expect("no close")
}

/// Every message that arrives within `ms`.
async fn ws_drain(ws: &mut Ws, ms: u64) -> Vec<Value> {
    use futures_util::StreamExt;
    let mut seen = Vec::new();
    let until = tokio::time::Instant::now() + std::time::Duration::from_millis(ms);
    while let Ok(Some(Ok(msg))) = tokio::time::timeout_at(until, ws.next()).await {
        if let tokio_tungstenite::tungstenite::Message::Text(t) = msg {
            seen.push(serde_json::from_str(&t).unwrap());
        }
    }
    seen
}

/// Hub timeouts short enough for tests.
fn quick(timing: impl FnOnce(&mut Timing)) -> Timing {
    let mut t = Timing::default();
    timing(&mut t);
    t
}

#[sqlx::test(migrator = "clipos_core::db::MIGRATOR")]
async fn the_live_room_waits_a_moment_for_hello(pool: PgPool) {
    let admin = admin_token(&pool).await;
    let timing = quick(|t| t.hello = std::time::Duration::from_millis(200));
    let app = app_timed(pool, true, timing).await;
    let show = new_show(&app, &admin, false).await;
    let addr = listen(&app).await;
    let mut ws = ws_open(addr, &show).await;
    let (seen, code) = ws_close(&mut ws).await;
    assert_eq!(seen.last().unwrap()["message"], "no hello in time");
    assert_eq!(code, 4008);
}

#[sqlx::test(migrator = "clipos_core::db::MIGRATOR")]
async fn the_live_room_is_admin_only_until_shows_open(pool: PgPool) {
    let admin = admin_token(&pool).await;
    invite(&pool, "kim@gmail.com").await;
    let app = app_with(pool, false).await;
    let kim = user_token("google-oauth2|kim", "kim@gmail.com");
    let show = new_show(&app, &admin, false).await;
    let addr = listen(&app).await;
    let mut ws = ws_open(addr, &show).await;
    ws_send(&mut ws, json!({ "type": "hello", "token": kim })).await;
    let (seen, code) = ws_close(&mut ws).await;
    assert_eq!(seen.last().unwrap()["type"], "error");
    assert_eq!(code, 4003);
    // The admin gets in.
    ws_join(addr, &show, &admin).await;
}

#[sqlx::test(migrator = "clipos_core::db::MIGRATOR")]
async fn the_live_room_sends_disabled_users_away(pool: PgPool) {
    let admin = admin_token(&pool).await;
    invite(&pool, "kim@gmail.com").await;
    invite(&pool, "sam@gmail.com").await;
    let app = app_with(pool.clone(), true).await;
    let kim = user_token("google-oauth2|kim", "kim@gmail.com");
    let sam = user_token("google-oauth2|sam", "sam@gmail.com");
    let show = new_show(&app, &admin, false).await;
    let addr = listen(&app).await;
    let disable = |email: &'static str| {
        let pool = pool.clone();
        async move {
            sqlx::query("UPDATE users SET status = 'disabled' WHERE email = $1")
                .bind(email)
                .execute(&pool)
                .await
                .unwrap();
        }
    };

    // Disabled before connecting: refused at hello.
    get(&app, "/api/me", Some(&kim)).await;
    disable("kim@gmail.com").await;
    let mut ws = ws_open(addr, &show).await;
    ws_send(&mut ws, json!({ "type": "hello", "token": kim })).await;
    assert_eq!(ws_close(&mut ws).await.1, 4003);

    // Disabled while connected: sent away by the hub's next round.
    let (mut ws, _) = ws_join(addr, &show, &sam).await;
    disable("sam@gmail.com").await;
    app.state.hub.tick(&app.state).await.unwrap();
    let (seen, code) = ws_close(&mut ws).await;
    assert_eq!(seen.last().unwrap()["type"], "error");
    assert_eq!(code, 4003);
}

#[sqlx::test(migrator = "clipos_core::db::MIGRATOR")]
async fn the_live_room_closes_when_the_token_expires(pool: PgPool) {
    clipos_core::invites::seed_admins(&pool, &["admin@gmail.com".into()])
        .await
        .unwrap();
    let app = app_with(pool, true).await;
    let admin = user_token("google-oauth2|admin", "admin@gmail.com");
    let show = new_show(&app, &admin, false).await;
    let addr = listen(&app).await;
    let expiring = token(json!({
        "sub": "google-oauth2|admin",
        "https://clips.spawnpoint.run/email": "admin@gmail.com",
        "exp": chrono::Utc::now().timestamp() + 1,
    }));
    let (mut ws, _) = ws_join(addr, &show, &expiring).await;
    let (seen, code) = ws_close(&mut ws).await;
    assert_eq!(seen.last().unwrap()["message"], "token expired");
    assert_eq!(code, 4001);
}

#[sqlx::test(migrator = "clipos_core::db::MIGRATOR")]
async fn the_live_room_of_a_show_thats_over_is_closed(pool: PgPool) {
    let admin = admin_token(&pool).await;
    let app = app_with(pool.clone(), true).await;
    let show = new_show(&app, &admin, false).await;
    clipos_core::shows::abandon(&pool, show.parse().unwrap())
        .await
        .unwrap();
    let addr = listen(&app).await;
    for id in [show, uuid::Uuid::new_v4().to_string()] {
        let mut ws = ws_open(addr, &id).await;
        ws_send(&mut ws, json!({ "type": "hello", "token": admin })).await;
        let (seen, code) = ws_close(&mut ws).await;
        assert_eq!(
            seen.last().unwrap()["message"],
            "no such show, or it's over"
        );
        assert_eq!(code, 4004);
    }
}

#[sqlx::test(migrator = "clipos_core::db::MIGRATOR")]
async fn the_live_room_sends_everyone_home_when_the_show_ends(pool: PgPool) {
    let admin = admin_token(&pool).await;
    invite(&pool, "kim@gmail.com").await;
    let app = app_with(pool, true).await;
    let kim = user_token("google-oauth2|kim", "kim@gmail.com");
    let show = new_show(&app, &admin, true).await;
    let addr = listen(&app).await;
    let (mut host, _) = ws_join(addr, &show, &admin).await;
    let (mut friend, _) = ws_join(addr, &show, &kim).await;

    for step in ["finale", "end"] {
        let path = format!("/api/shows/{show}/{step}");
        let (status, body) = send(&app, "POST", &path, Some(&admin), Some(json!({})), &[]).await;
        assert_eq!(status, StatusCode::OK, "{body}");
    }
    for ws in [&mut host, &mut friend] {
        let (seen, code) = ws_close(ws).await;
        assert_eq!(seen.last().unwrap()["type"], "showOver");
        assert_eq!(code, 4004);
    }
    // And nobody gets back in.
    let mut ws = ws_open(addr, &show).await;
    ws_send(&mut ws, json!({ "type": "hello", "token": kim })).await;
    assert_eq!(ws_close(&mut ws).await.1, 4004);
}

#[sqlx::test(migrator = "clipos_core::db::MIGRATOR")]
async fn the_live_room_closes_quiet_connections(pool: PgPool) {
    let admin = admin_token(&pool).await;
    let timing = quick(|t| t.idle = std::time::Duration::from_millis(300));
    let app = app_timed(pool, true, timing).await;
    let show = new_show(&app, &admin, false).await;
    let addr = listen(&app).await;
    let (mut ws, _) = ws_join(addr, &show, &admin).await;
    // Pinging keeps it open...
    for _ in 0..6 {
        ws_send(&mut ws, json!({ "type": "ping", "clientMs": 1 })).await;
        ws_next(&mut ws, "pong").await;
        tokio::time::sleep(std::time::Duration::from_millis(100)).await;
    }
    // ...silence closes it.
    let (seen, code) = ws_close(&mut ws).await;
    assert_eq!(seen.last().unwrap()["type"], "error");
    assert_eq!(code, 4008);
}

#[sqlx::test(migrator = "clipos_core::db::MIGRATOR")]
async fn the_live_room_refuses_oversized_messages(pool: PgPool) {
    use futures_util::StreamExt;
    let admin = admin_token(&pool).await;
    invite(&pool, "kim@gmail.com").await;
    let app = app_with(pool, true).await;
    let kim = user_token("google-oauth2|kim", "kim@gmail.com");
    let show = new_show(&app, &admin, false).await;
    let addr = listen(&app).await;
    let (mut host, _) = ws_join(addr, &show, &admin).await;
    let (mut friend, _) = ws_join(addr, &show, &kim).await;
    ws_drain(&mut host, 100).await;
    let huge = "x".repeat(20 * 1024);
    ws_send(
        &mut friend,
        json!({ "type": "ready", "ready": true, "pad": huge }),
    )
    .await;
    // The connection ends, and the message never reaches anyone.
    let ended = tokio::time::timeout(std::time::Duration::from_secs(3), async {
        loop {
            match friend.next().await {
                None | Some(Err(_)) => break,
                Some(Ok(tokio_tungstenite::tungstenite::Message::Close(_))) => break,
                Some(Ok(_)) => {}
            }
        }
    })
    .await;
    assert!(ended.is_ok(), "still open");
    let presence = loop {
        let p = ws_next(&mut host, "presence").await;
        if p["presence"]["online"].as_array().unwrap().len() == 1 {
            break p;
        }
    };
    assert_eq!(presence["presence"]["online"].as_array().unwrap().len(), 1);
    assert!(
        ws_drain(&mut host, 200)
            .await
            .iter()
            .all(|m| m["type"] != "showChanged")
    );
}

#[sqlx::test(migrator = "clipos_core::db::MIGRATOR")]
async fn the_live_room_forgets_connections_that_vanish(pool: PgPool) {
    let admin = admin_token(&pool).await;
    invite(&pool, "kim@gmail.com").await;
    invite(&pool, "sam@gmail.com").await;
    let app = app_with(pool, true).await;
    let kim = user_token("google-oauth2|kim", "kim@gmail.com");
    let sam = user_token("google-oauth2|sam", "sam@gmail.com");
    let show = new_show(&app, &admin, false).await;
    let addr = listen(&app).await;
    let (_host, _) = ws_join(addr, &show, &admin).await;

    // Kim says hello and vanishes at once (a reset, not a close), so the welcome has
    // nowhere to go.
    let tcp = tokio::net::TcpStream::connect(addr).await.unwrap();
    tcp.set_zero_linger().unwrap();
    let (mut ws, _) = tokio_tungstenite::client_async(
        format!("ws://{addr}/api/shows/{show}/live"),
        tokio_tungstenite::MaybeTlsStream::Plain(tcp),
    )
    .await
    .unwrap();
    ws_send(&mut ws, json!({ "type": "hello", "token": kim })).await;
    drop(ws);
    tokio::time::sleep(std::time::Duration::from_millis(500)).await;

    // Kim isn't left behind as a ghost.
    let (_, welcome) = ws_join(addr, &show, &sam).await;
    assert_eq!(
        welcome["presence"]["online"].as_array().unwrap().len(),
        2,
        "{welcome}"
    );
}

#[sqlx::test(migrator = "clipos_core::db::MIGRATOR")]
async fn the_live_room_limits_how_fast_friends_send(pool: PgPool) {
    let admin = admin_token(&pool).await;
    invite(&pool, "kim@gmail.com").await;
    let app = app_with(pool, true).await;
    let kim = user_token("google-oauth2|kim", "kim@gmail.com");
    let show = new_show(&app, &admin, false).await;
    let addr = listen(&app).await;
    let (mut host, _) = ws_join(addr, &show, &admin).await;
    let (mut friend, _) = ws_join(addr, &show, &kim).await;
    ws_drain(&mut host, 100).await;

    // A tap with an unknown emoji is refused before anything is read or written, so each
    // one that gets through is a quick "unsupported reaction".
    let tap = json!({ "type": "react", "clipId": uuid::Uuid::nil(), "emoji": "🍕", "atMs": 0 });
    for _ in 0..40 {
        ws_send(&mut friend, tap.clone()).await;
    }
    // Pings aren't limited.
    ws_send(&mut friend, json!({ "type": "ping", "clientMs": 1 })).await;
    let seen = ws_drain(&mut friend, 500).await;
    assert!(seen.iter().any(|m| m["type"] == "pong"));
    let said = |what: &str| {
        seen.iter()
            .filter(|m| m["type"] == "error" && m["message"] == what)
            .count()
    };
    // One "slow down", however many were dropped.
    assert_eq!(said("slow down"), 1, "{seen:?}");
    // The burst got through, and not much more.
    let passed = said("unsupported reaction");
    assert!((20..=22).contains(&passed), "{passed} got through");
}

#[sqlx::test(migrator = "clipos_core::db::MIGRATOR")]
async fn the_live_room_plays_only_while_the_show_is_live(pool: PgPool) {
    let admin = admin_token(&pool).await;
    invite(&pool, "kim@gmail.com").await;
    let a = ready_clip(&pool, "google-oauth2|kim", "kim@gmail.com", "A").await;
    let app = app_with(pool, true).await;
    let show = new_show(&app, &admin, false).await;
    let addr = listen(&app).await;
    let (mut host, _) = ws_join(addr, &show, &admin).await;

    // In the lobby, nothing plays yet.
    ws_send(&mut host, json!({ "type": "load", "clipId": a })).await;
    assert_eq!(
        ws_next(&mut host, "error").await["message"],
        "only while the show is live"
    );

    let path = format!("/api/shows/{show}/start");
    send(&app, "POST", &path, Some(&admin), None, &[]).await;
    ws_send(&mut host, json!({ "type": "load", "clipId": a })).await;
    assert_eq!(ws_next(&mut host, "state").await["state"]["seq"], 1);
    ws_send(&mut host, json!({ "type": "play" })).await;
    let playing = ws_next(&mut host, "state").await["state"].clone();
    assert_eq!(playing["seq"], 2);
    // Play while playing changes nothing: nobody waits for a new start.
    ws_send(&mut host, json!({ "type": "play" })).await;
    ws_send(&mut host, json!({ "type": "pause" })).await;
    assert_eq!(ws_next(&mut host, "state").await["state"]["seq"], 3);
}

#[sqlx::test(migrator = "clipos_core::db::MIGRATOR")]
async fn a_live_state_save_never_replaces_a_newer_one(pool: PgPool) {
    let admin = admin_token(&pool).await;
    let app = app_with(pool.clone(), true).await;
    let show: uuid::Uuid = new_show(&app, &admin, false).await.parse().unwrap();
    use clipos_core::shows::{live_state, save_newer_live_state};
    save_newer_live_state(&pool, show, 5, &json!({ "seq": 5 }))
        .await
        .unwrap();
    save_newer_live_state(&pool, show, 4, &json!({ "seq": 4 }))
        .await
        .unwrap();
    assert_eq!(
        live_state(&pool, show).await.unwrap(),
        Some(json!({ "seq": 5 }))
    );
    save_newer_live_state(&pool, show, 6, &json!({ "seq": 6 }))
        .await
        .unwrap();
    assert_eq!(
        live_state(&pool, show).await.unwrap(),
        Some(json!({ "seq": 6 }))
    );
}

/// Waits until the room shows the host as away.
async fn host_away(ws: &mut Ws) {
    loop {
        let p = ws_next(ws, "presence").await;
        if !p["presence"]["hostAwaySince"].is_null() {
            return;
        }
    }
}

#[sqlx::test(migrator = "clipos_core::db::MIGRATOR")]
async fn the_host_can_be_taken_over_once_theyre_gone(pool: PgPool) {
    let admin = admin_token(&pool).await;
    invite(&pool, "kim@gmail.com").await;
    let timing = quick(|t| t.takeover_after = std::time::Duration::from_millis(300));
    let app = app_timed(pool, true, timing).await;
    let kim = user_token("google-oauth2|kim", "kim@gmail.com");
    let show = new_show(&app, &admin, true).await;
    let addr = listen(&app).await;
    let (host, _) = ws_join(addr, &show, &admin).await;
    let (mut friend, welcome) = ws_join(addr, &show, &kim).await;
    let kim_id = welcome["userId"].clone();

    drop(host);
    host_away(&mut friend).await;
    // Not yet: the host may be back in a moment.
    ws_send(&mut friend, json!({ "type": "takeOver" })).await;
    ws_next(&mut friend, "error").await;
    tokio::time::sleep(std::time::Duration::from_millis(400)).await;
    ws_send(&mut friend, json!({ "type": "takeOver" })).await;
    let presence = ws_next(&mut friend, "presence").await;
    assert_eq!(presence["presence"]["hostId"], kim_id);
    assert!(presence["presence"]["hostAwaySince"].is_null());
    let (_, body) = get(&app, &format!("/api/shows/{show}"), Some(&kim)).await;
    assert_eq!(json(&body)["host"]["id"], kim_id);
}

#[sqlx::test(migrator = "clipos_core::db::MIGRATOR")]
async fn only_one_friend_takes_over(pool: PgPool) {
    let admin = admin_token(&pool).await;
    invite(&pool, "kim@gmail.com").await;
    invite(&pool, "sam@gmail.com").await;
    let timing = quick(|t| t.takeover_after = std::time::Duration::from_millis(300));
    let app = app_timed(pool, true, timing).await;
    let kim = user_token("google-oauth2|kim", "kim@gmail.com");
    let sam = user_token("google-oauth2|sam", "sam@gmail.com");
    let show = new_show(&app, &admin, true).await;
    let addr = listen(&app).await;
    // The host never connects to this hub (as after a restart).
    let (mut kim_ws, kim_welcome) = ws_join(addr, &show, &kim).await;
    let (mut sam_ws, sam_welcome) = ws_join(addr, &show, &sam).await;
    tokio::time::sleep(std::time::Duration::from_millis(400)).await;

    // Both at once.
    tokio::join!(
        ws_send(&mut kim_ws, json!({ "type": "takeOver" })),
        ws_send(&mut sam_ws, json!({ "type": "takeOver" })),
    );
    let (kim_seen, sam_seen) = tokio::join!(ws_drain(&mut kim_ws, 500), ws_drain(&mut sam_ws, 500));
    let refused = |seen: &[Value]| seen.iter().any(|m| m["type"] == "error");
    assert!(
        refused(&kim_seen) != refused(&sam_seen),
        "{kim_seen:?} {sam_seen:?}"
    );
    let winner = if refused(&kim_seen) {
        &sam_welcome["userId"]
    } else {
        &kim_welcome["userId"]
    };
    let (_, body) = get(&app, &format!("/api/shows/{show}"), Some(&kim)).await;
    assert_eq!(&json(&body)["host"]["id"], winner);
}

#[sqlx::test(migrator = "clipos_core::db::MIGRATOR")]
async fn a_host_who_never_comes_back_after_a_restart_can_be_taken_over(pool: PgPool) {
    let admin = admin_token(&pool).await;
    invite(&pool, "kim@gmail.com").await;
    let kim = user_token("google-oauth2|kim", "kim@gmail.com");
    let app = app_with(pool.clone(), true).await;
    let show = new_show(&app, &admin, true).await;
    let addr = listen(&app).await;
    let (host, _) = ws_join(addr, &show, &admin).await;
    drop(host);

    // A deploy restarts the api; the host doesn't come back.
    let timing = quick(|t| t.takeover_after = std::time::Duration::from_millis(300));
    let restarted = app_timed(pool, true, timing).await;
    let addr = listen(&restarted).await;
    let (mut friend, welcome) = ws_join(addr, &show, &kim).await;
    assert!(!welcome["presence"]["hostAwaySince"].is_null());
    tokio::time::sleep(std::time::Duration::from_millis(400)).await;
    ws_send(&mut friend, json!({ "type": "takeOver" })).await;
    // Kim's own arrival comes first; then the new host.
    loop {
        let presence = ws_next(&mut friend, "presence").await;
        if presence["presence"]["hostId"] == welcome["userId"] {
            break;
        }
    }
}

/// The show's status in the database.
async fn show_status(pool: &PgPool, show: &str) -> clipos_core::shows::ShowStatus {
    clipos_core::shows::get(pool, show.parse().unwrap())
        .await
        .unwrap()
        .unwrap()
        .status
}

/// Runs the hub's housekeeping until the show is abandoned, for at most 5 s. The room's
/// clock starts when the server notices the last socket close, which a busy machine may
/// take a while to do, so a fixed sleep would be flaky.
async fn ticks_until_abandoned(app: &TestApp, show: &str) {
    use clipos_core::shows::ShowStatus;
    let deadline = tokio::time::Instant::now() + std::time::Duration::from_secs(5);
    loop {
        app.state.hub.tick(&app.state).await.unwrap();
        if show_status(&app.state.pool, show).await == ShowStatus::Abandoned {
            return;
        }
        assert!(
            tokio::time::Instant::now() < deadline,
            "not abandoned after 5 s"
        );
        tokio::time::sleep(std::time::Duration::from_millis(50)).await;
    }
}

#[sqlx::test(migrator = "clipos_core::db::MIGRATOR")]
async fn a_show_everyone_left_is_abandoned(pool: PgPool) {
    use clipos_core::shows::ShowStatus;
    let admin = admin_token(&pool).await;
    invite(&pool, "kim@gmail.com").await;
    let a = ready_clip(&pool, "google-oauth2|kim", "kim@gmail.com", "A").await;
    let timing = quick(|t| t.abandon_after = std::time::Duration::from_millis(300));
    let app = app_timed(pool.clone(), true, timing).await;
    let kim = user_token("google-oauth2|kim", "kim@gmail.com");
    let show = new_show(&app, &admin, true).await;
    let addr = listen(&app).await;
    let (mut host, _) = ws_join(addr, &show, &admin).await;
    let (friend, _) = ws_join(addr, &show, &kim).await;
    ws_send(&mut host, json!({ "type": "load", "clipId": a })).await;
    ws_send(&mut host, json!({ "type": "play" })).await;
    assert_eq!(ws_next(&mut host, "state").await["state"]["seq"], 1);
    assert_eq!(ws_next(&mut host, "state").await["state"]["seq"], 2);
    let played = || async {
        clipos_core::shows::lineup(&pool, show.parse().unwrap())
            .await
            .unwrap()[0]
            .played_at
            .is_some()
    };
    assert!(played().await);

    // Someone's still there: the show goes on, well past the abandon window.
    drop(friend);
    tokio::time::sleep(std::time::Duration::from_millis(600)).await;
    app.state.hub.tick(&app.state).await.unwrap();
    assert_eq!(show_status(&pool, &show).await, ShowStatus::Live);

    // Everyone's gone: not at once...
    drop(host);
    app.state.hub.tick(&app.state).await.unwrap();
    assert_eq!(show_status(&pool, &show).await, ShowStatus::Live);
    // ...but once they've been gone long enough. Nothing counts as played.
    ticks_until_abandoned(&app, &show).await;
    assert!(!played().await);
    // And nobody gets back in.
    let mut ws = ws_open(addr, &show).await;
    ws_send(&mut ws, json!({ "type": "hello", "token": kim })).await;
    let (seen, code) = ws_close(&mut ws).await;
    assert_eq!(
        seen.last().unwrap()["message"],
        "no such show, or it's over"
    );
    assert_eq!(code, 4004);
}

#[sqlx::test(migrator = "clipos_core::db::MIGRATOR")]
async fn a_lobby_is_abandoned_only_once_people_left_it(pool: PgPool) {
    use clipos_core::shows::ShowStatus;
    let admin = admin_token(&pool).await;
    let timing = quick(|t| t.abandon_after = std::time::Duration::from_millis(300));
    let app = app_timed(pool.clone(), true, timing).await;
    let tick = || app.state.hub.tick(&app.state);

    // A lobby people only see over REST stays open, however long.
    let show = new_show(&app, &admin, false).await;
    tick().await.unwrap();
    tokio::time::sleep(std::time::Duration::from_millis(600)).await;
    tick().await.unwrap();
    assert_eq!(show_status(&pool, &show).await, ShowStatus::Lobby);

    // Once it starts it counts as having had people (as after a restart), with its clock
    // starting then: not when the room was made, nor at the lobby's last tick (here a
    // while before the start, as on a slow machine).
    tokio::time::sleep(std::time::Duration::from_millis(600)).await;
    let path = format!("/api/shows/{show}/start");
    let (status, _) = send(&app, "POST", &path, Some(&admin), None, &[]).await;
    assert_eq!(status, StatusCode::OK);
    tick().await.unwrap();
    assert_eq!(show_status(&pool, &show).await, ShowStatus::Live);
    // That tick started the clock, so this one is at least 600 ms into a 300 ms window.
    tokio::time::sleep(std::time::Duration::from_millis(600)).await;
    tick().await.unwrap();
    assert_eq!(show_status(&pool, &show).await, ShowStatus::Abandoned);

    // A lobby someone connected to and left is abandoned like a started show.
    let show = new_show(&app, &admin, false).await;
    let addr = listen(&app).await;
    let (ws, _) = ws_join(addr, &show, &admin).await;
    drop(ws);
    ticks_until_abandoned(&app, &show).await;
}

/// The live state the hub last saved for the show.
async fn saved_state(pool: &PgPool, show: &str) -> Value {
    clipos_core::shows::live_state(pool, show.parse().unwrap())
        .await
        .unwrap()
        .unwrap()
}

/// The states among `seen`.
fn states(seen: &[Value]) -> Vec<Value> {
    seen.iter()
        .filter(|m| m["type"] == "state")
        .map(|m| m["state"].clone())
        .collect()
}

#[sqlx::test(migrator = "clipos_core::db::MIGRATOR")]
async fn a_joiner_during_a_slow_play_gets_one_state_per_seq(pool: PgPool) {
    let admin = admin_token(&pool).await;
    invite(&pool, "kim@gmail.com").await;
    invite(&pool, "sam@gmail.com").await;
    let a = ready_clip(&pool, "google-oauth2|kim", "kim@gmail.com", "A").await;
    let app = app_with(pool.clone(), true).await;
    let kim = user_token("google-oauth2|kim", "kim@gmail.com");
    let sam = user_token("google-oauth2|sam", "sam@gmail.com");
    let show = new_show(&app, &admin, true).await;
    let addr = listen(&app).await;
    let (mut host, _) = ws_join(addr, &show, &admin).await;
    ws_send(&mut host, json!({ "type": "load", "clipId": a })).await;
    ws_next(&mut host, "state").await;

    // A slow database: a lock on the clip's lineup row stalls counting it as played.
    let mut lock = pool.begin().await.unwrap();
    sqlx::query(
        "SELECT 1 FROM show_clips WHERE show_id = $1::uuid AND clip_id = $2::uuid FOR UPDATE",
    )
    .bind(&show)
    .bind(&a)
    .execute(&mut *lock)
    .await
    .unwrap();
    ws_send(&mut host, json!({ "type": "play" })).await;
    tokio::time::sleep(std::time::Duration::from_millis(300)).await;
    // Kim joins meanwhile: her welcome is the loaded clip, not the play on its way.
    let (mut friend, welcome) = ws_join(addr, &show, &kim).await;
    assert_eq!(welcome["state"]["seq"], 1);
    assert_eq!(welcome["state"]["playing"], false);
    tokio::time::sleep(std::time::Duration::from_millis(700)).await;
    let released = clipos_api::live::now_ms();
    lock.rollback().await.unwrap();

    // The play comes under the next seq, timed from when it could start.
    let state = ws_next(&mut friend, "state").await["state"].clone();
    assert_eq!(state["seq"], 2);
    assert_eq!(state["playing"], true);
    assert!(state["atServerMs"].as_f64().unwrap() >= released + 300.0);
    // The saved copy and a newcomer's welcome are the state that went out.
    assert_eq!(saved_state(&pool, &show).await, state);
    let (_, welcome) = ws_join(addr, &show, &sam).await;
    assert_eq!(welcome["state"], state);
}

#[sqlx::test(migrator = "clipos_core::db::MIGRATOR")]
async fn a_play_that_cant_be_counted_doesnt_start(pool: PgPool) {
    let admin = admin_token(&pool).await;
    invite(&pool, "kim@gmail.com").await;
    let a = ready_clip(&pool, "google-oauth2|kim", "kim@gmail.com", "A").await;
    clipos_core::clips::hold(&pool, a.parse().unwrap())
        .await
        .unwrap();
    let app = app_with(pool.clone(), true).await;
    let kim = user_token("google-oauth2|kim", "kim@gmail.com");
    let show = new_show(&app, &admin, true).await;
    let addr = listen(&app).await;
    let (mut host, _) = ws_join(addr, &show, &admin).await;
    let (mut friend, _) = ws_join(addr, &show, &kim).await;
    ws_send(&mut host, json!({ "type": "load", "clipId": a })).await;
    let loaded = ws_next(&mut friend, "state").await["state"].clone();

    // The host left the stored show behind the hub's back, so counting fails.
    sqlx::query(
        "DELETE FROM show_participants
          WHERE show_id = $1::uuid
            AND user_id = (SELECT id FROM users WHERE email = 'admin@gmail.com')",
    )
    .bind(&show)
    .execute(&pool)
    .await
    .unwrap();
    ws_send(&mut host, json!({ "type": "play" })).await;
    assert_eq!(
        ws_next(&mut host, "error").await["message"],
        "couldn't count the clip as played, so it didn't start"
    );
    // Nobody was told to play it: the loaded clip is still what's on, and still held.
    assert!(states(&ws_drain(&mut friend, 300).await).is_empty());
    assert_eq!(saved_state(&pool, &show).await, loaded);
    let (_, welcome) = ws_join(addr, &show, &kim).await;
    assert_eq!(welcome["state"], loaded);
    let clip = clipos_core::clips::get(&pool, a.parse().unwrap())
        .await
        .unwrap()
        .unwrap();
    assert!(clip.is_held());
}

#[sqlx::test(migrator = "clipos_core::db::MIGRATOR")]
async fn two_host_tabs_take_turns(pool: PgPool) {
    let admin = admin_token(&pool).await;
    invite(&pool, "kim@gmail.com").await;
    invite(&pool, "sam@gmail.com").await;
    let a = ready_clip(&pool, "google-oauth2|kim", "kim@gmail.com", "A").await;
    let app = app_with(pool.clone(), true).await;
    let kim = user_token("google-oauth2|kim", "kim@gmail.com");
    let sam = user_token("google-oauth2|sam", "sam@gmail.com");
    let show = new_show(&app, &admin, true).await;
    let addr = listen(&app).await;
    let (mut tab1, _) = ws_join(addr, &show, &admin).await;
    let (mut tab2, _) = ws_join(addr, &show, &admin).await;
    let (mut friend, _) = ws_join(addr, &show, &kim).await;
    ws_send(&mut tab1, json!({ "type": "load", "clipId": a })).await;
    for ws in [&mut tab1, &mut tab2, &mut friend] {
        ws_next(ws, "state").await;
    }
    for i in 0..20 {
        let other = if i % 2 == 0 {
            json!({ "type": "play" })
        } else {
            json!({ "type": "pause" })
        };
        tokio::join!(
            ws_send(&mut tab1, json!({ "type": "seek", "positionMs": i * 100 })),
            ws_send(&mut tab2, other),
        );
    }
    // Every change goes out, in order, to everyone.
    let mut last = Value::Null;
    for ws in [&mut tab1, &mut tab2, &mut friend] {
        for seq in 2..=41 {
            last = ws_next(ws, "state").await["state"].clone();
            assert_eq!(last["seq"], seq);
        }
    }
    // The last one is what's saved and what a newcomer gets.
    assert_eq!(saved_state(&pool, &show).await, last);
    let (_, welcome) = ws_join(addr, &show, &sam).await;
    assert_eq!(welcome["state"], last);
}

/// The clip's row in the show's lineup: (played, dropped).
async fn lineup_row(pool: &PgPool, show: &str, clip: &str) -> (bool, bool) {
    sqlx::query_as(
        "SELECT played_at IS NOT NULL, dropped FROM show_clips
          WHERE show_id = $1::uuid AND clip_id = $2::uuid",
    )
    .bind(show)
    .bind(clip)
    .fetch_one(pool)
    .await
    .unwrap()
}

async fn is_held(pool: &PgPool, clip: &str) -> bool {
    sqlx::query_scalar("SELECT coalesce(hold_until > now(), false) FROM clips WHERE id = $1::uuid")
        .bind(clip)
        .fetch_one(pool)
        .await
        .unwrap()
}

/// Whether any of the states among `seen` plays `clip`.
fn plays(seen: &[Value], clip: &str) -> bool {
    states(seen)
        .iter()
        .any(|s| s["clipId"] == json!(clip) && s["playing"] == true)
}

#[sqlx::test(migrator = "clipos_core::db::MIGRATOR")]
async fn a_clip_dropped_after_its_load_comes_off_for_everyone(pool: PgPool) {
    let admin = admin_token(&pool).await;
    invite(&pool, "kim@gmail.com").await;
    let a = ready_clip(&pool, "google-oauth2|kim", "kim@gmail.com", "A").await;
    let b = ready_clip(&pool, "google-oauth2|kim", "kim@gmail.com", "B").await;
    clipos_core::clips::hold(&pool, a.parse().unwrap())
        .await
        .unwrap();
    let app = app_with(pool.clone(), true).await;
    let kim = user_token("google-oauth2|kim", "kim@gmail.com");
    let show = new_show(&app, &admin, true).await;
    let addr = listen(&app).await;
    let (mut host, _) = ws_join(addr, &show, &admin).await;
    let (mut friend, _) = ws_join(addr, &show, &kim).await;
    ws_send(&mut host, json!({ "type": "load", "clipId": a })).await;
    assert_eq!(
        ws_next(&mut friend, "state").await["state"]["clipId"],
        json!(a)
    );

    // The host drops it over REST: it comes off, paused.
    let path = format!("/api/shows/{show}/lineup");
    let order = json!({ "clipIds": [b] });
    let (status, body) = send(&app, "PUT", &path, Some(&admin), Some(order), &[]).await;
    assert_eq!(status, StatusCode::OK, "{body}");
    let state = ws_next(&mut friend, "state").await["state"].clone();
    assert_eq!(state["clipId"], Value::Null);
    assert_eq!(state["playing"], false);
    assert_eq!(state["seq"], 2);

    // So play has nothing to start.
    ws_send(&mut host, json!({ "type": "play" })).await;
    assert_eq!(ws_next(&mut host, "error").await["message"], "no clip on");
    assert!(!plays(&ws_drain(&mut friend, 300).await, &a));
    assert_eq!(lineup_row(&pool, &show, &a).await, (false, true));
    assert!(is_held(&pool, &a).await);
}

#[sqlx::test(migrator = "clipos_core::db::MIGRATOR")]
async fn a_clip_deleted_after_its_load_doesnt_play(pool: PgPool) {
    let admin = admin_token(&pool).await;
    invite(&pool, "kim@gmail.com").await;
    let a = ready_clip(&pool, "google-oauth2|kim", "kim@gmail.com", "A").await;
    let app = app_with(pool.clone(), true).await;
    let kim = user_token("google-oauth2|kim", "kim@gmail.com");
    let show = new_show(&app, &admin, true).await;
    let addr = listen(&app).await;
    let (mut host, _) = ws_join(addr, &show, &admin).await;
    let (mut friend, _) = ws_join(addr, &show, &kim).await;
    ws_send(&mut host, json!({ "type": "load", "clipId": a })).await;
    ws_next(&mut friend, "state").await;
    // The janitor purges it (no REST change tells the hub). It only purges clips that
    // have been in the trash for `TRASH_DAYS`, so this one has been there that long.
    let id: uuid::Uuid = a.parse().unwrap();
    assert!(clipos_core::clips::soft_delete(&pool, id).await.unwrap());
    sqlx::query(
        "UPDATE clips SET deleted_at = now() - make_interval(days => $2) - interval '1 hour'
          WHERE id = $1",
    )
    .bind(id)
    .bind(clipos_core::clips::TRASH_DAYS as i32)
    .execute(&pool)
    .await
    .unwrap();
    let expired = clipos_core::clips::expired_trash(&pool).await.unwrap();
    assert_eq!(expired.iter().map(|t| t.id).collect::<Vec<_>>(), [id]);
    clipos_core::clips::purge(&pool, id).await.unwrap();
    let left: i64 = sqlx::query_scalar("SELECT count(*) FROM clips WHERE id = $1")
        .bind(id)
        .fetch_one(&pool)
        .await
        .unwrap();
    assert_eq!(left, 0, "purged");

    // Play refuses it, and it comes off for everyone instead.
    ws_send(&mut host, json!({ "type": "play" })).await;
    assert_eq!(
        ws_next(&mut host, "error").await["message"],
        "that clip isn't in the lineup"
    );
    let seen = ws_drain(&mut friend, 300).await;
    assert!(!plays(&seen, &a), "{seen:?}");
    let states = states(&seen);
    assert_eq!(states.len(), 1, "{states:?}");
    assert_eq!(states[0]["clipId"], Value::Null);
}

#[sqlx::test(migrator = "clipos_core::db::MIGRATOR")]
async fn a_trashed_clip_doesnt_load_or_play(pool: PgPool) {
    use clipos_core::clips;
    let admin = admin_token(&pool).await;
    invite(&pool, "kim@gmail.com").await;
    let a = ready_clip(&pool, "google-oauth2|kim", "kim@gmail.com", "A").await;
    let b = ready_clip(&pool, "google-oauth2|kim", "kim@gmail.com", "B").await;
    clips::hold(&pool, a.parse().unwrap()).await.unwrap();
    let app = app_with(pool.clone(), true).await;
    let kim = user_token("google-oauth2|kim", "kim@gmail.com");
    let show = new_show(&app, &admin, true).await;
    let addr = listen(&app).await;
    let (mut host, _) = ws_join(addr, &show, &admin).await;
    let (mut friend, _) = ws_join(addr, &show, &kim).await;

    // Its uploader trashes it while it waits in the lineup: load refuses it.
    assert!(clips::soft_delete(&pool, a.parse().unwrap()).await.unwrap());
    ws_send(&mut host, json!({ "type": "load", "clipId": a })).await;
    assert_eq!(
        ws_next(&mut host, "error").await["message"],
        "that clip is in the trash"
    );
    ws_send(&mut host, json!({ "type": "play" })).await;
    assert_eq!(ws_next(&mut host, "error").await["message"], "no clip on");
    assert!(states(&ws_drain(&mut friend, 300).await).is_empty());
    assert_eq!(lineup_row(&pool, &show, &a).await, (false, false));
    assert!(is_held(&pool, &a).await);

    // Trashed after its load: play refuses it and it comes off.
    assert_eq!(
        clips::restore(&pool, a.parse().unwrap()).await.unwrap(),
        clips::Restore::Restored
    );
    ws_send(&mut host, json!({ "type": "load", "clipId": a })).await;
    ws_next(&mut friend, "state").await;
    assert!(clips::soft_delete(&pool, a.parse().unwrap()).await.unwrap());
    ws_send(&mut host, json!({ "type": "play" })).await;
    assert_eq!(
        ws_next(&mut host, "error").await["message"],
        "that clip is in the trash"
    );
    let seen = ws_drain(&mut friend, 300).await;
    assert!(!plays(&seen, &a), "{seen:?}");
    assert_eq!(states(&seen)[0]["clipId"], Value::Null);
    assert!(is_held(&pool, &a).await);

    // Nor does a clip that isn't ready.
    sqlx::query("UPDATE clips SET status = 'processing' WHERE id = $1::uuid")
        .bind(&b)
        .execute(&pool)
        .await
        .unwrap();
    ws_send(&mut host, json!({ "type": "load", "clipId": b })).await;
    assert_eq!(
        ws_next(&mut host, "error").await["message"],
        "that clip isn't ready to play"
    );
}

#[sqlx::test(migrator = "clipos_core::db::MIGRATOR")]
async fn a_load_starts_at_most_5_s_ahead(pool: PgPool) {
    let admin = admin_token(&pool).await;
    invite(&pool, "kim@gmail.com").await;
    let a = ready_clip(&pool, "google-oauth2|kim", "kim@gmail.com", "A").await;
    let app = app_with(pool.clone(), true).await;
    let kim = user_token("google-oauth2|kim", "kim@gmail.com");
    let show = new_show(&app, &admin, true).await;
    let addr = listen(&app).await;
    let (mut host, _) = ws_join(addr, &show, &admin).await;
    let (mut friend, _) = ws_join(addr, &show, &kim).await;
    let load = |start_at: f64| json!({ "type": "load", "clipId": a, "startAt": start_at });

    // A day out (a host whose clock is off) is refused: nothing plays, nothing counts.
    let now = clipos_api::live::now_ms();
    ws_send(&mut host, load(now + 86_400_000.0)).await;
    assert_eq!(
        ws_next(&mut host, "error").await["message"],
        "start it at most 5 s ahead"
    );
    assert!(states(&ws_drain(&mut friend, 300).await).is_empty());
    assert_eq!(lineup_row(&pool, &show, &a).await, (false, false));

    // Within 5 s it's kept as asked.
    let now = clipos_api::live::now_ms();
    ws_send(&mut host, load(now + 4_500.0)).await;
    let state = ws_next(&mut friend, "state").await["state"].clone();
    assert_eq!(state["playing"], true);
    // (Within a millisecond: JSON may not give back the last bit of a float.)
    let at = state["atServerMs"].as_f64().unwrap();
    assert!((at - (now + 4_500.0)).abs() < 1.0, "{at} vs {now}");

    // Too soon (or past) moves to the lead everyone needs.
    let now = clipos_api::live::now_ms();
    ws_send(&mut host, load(now - 10_000.0)).await;
    let state = ws_next(&mut friend, "state").await["state"].clone();
    assert!(state["atServerMs"].as_f64().unwrap() >= now + 300.0);
}

/// Hello must be a text message; a connection that goes before saying it leaves nothing
/// behind. After hello, messages the room doesn't understand get an error and the
/// connection carries on; a close from the browser ends it.
#[sqlx::test(migrator = "clipos_core::db::MIGRATOR")]
async fn the_live_room_ignores_what_it_doesnt_understand(pool: PgPool) {
    use futures_util::{SinkExt, StreamExt};
    use tokio_tungstenite::tungstenite::Message;
    let admin = admin_token(&pool).await;
    invite(&pool, "kim@gmail.com").await;
    let app = app_with(pool, true).await;
    let kim = user_token("google-oauth2|kim", "kim@gmail.com");
    let show = new_show(&app, &admin, false).await;
    let addr = listen(&app).await;

    // Binary first: no hello. (What follows it is read, and ignored, while the server
    // waits for the browser's side of the close.)
    let mut ws = ws_open(addr, &show).await;
    ws.send(Message::Binary(b"hello".to_vec().into()))
        .await
        .unwrap();
    ws_send(&mut ws, json!({ "type": "hello", "token": admin })).await;
    let (seen, code) = ws_close(&mut ws).await;
    assert_eq!(seen.len(), 1, "{seen:?}");
    assert_eq!(seen.last().unwrap()["message"], "say hello first");
    assert_eq!(code, 4001);
    // The browser's reply to the close ends the connection, with nothing more said.
    let rest = tokio::time::timeout(std::time::Duration::from_secs(3), ws.next())
        .await
        .expect("closed");
    assert!(!matches!(rest, Some(Ok(Message::Text(_)))), "{rest:?}");

    // Gone before hello (a reset, not a close): never in the room.
    let tcp = tokio::net::TcpStream::connect(addr).await.unwrap();
    tcp.set_zero_linger().unwrap();
    let (ws, _) = tokio_tungstenite::client_async(
        format!("ws://{addr}/api/shows/{show}/live"),
        tokio_tungstenite::MaybeTlsStream::Plain(tcp),
    )
    .await
    .unwrap();
    drop(ws);
    tokio::time::sleep(std::time::Duration::from_millis(200)).await;
    let (mut host, welcome) = ws_join(addr, &show, &admin).await;
    assert_eq!(welcome["presence"]["online"].as_array().unwrap().len(), 1);

    let (mut friend, _) = ws_join(addr, &show, &kim).await;
    ws_send(&mut friend, json!({ "type": "dance" })).await;
    assert_eq!(
        ws_next(&mut friend, "error").await["message"],
        "unknown message"
    );
    ws_send(&mut friend, json!({ "type": "hello", "token": kim })).await;
    assert_eq!(
        ws_next(&mut friend, "error").await["message"],
        "already said hello"
    );
    // Binary after hello is ignored: no error, and the connection carries on.
    friend
        .send(Message::Binary(vec![1, 2, 3].into()))
        .await
        .unwrap();
    ws_send(&mut friend, json!({ "type": "ping", "clientMs": 7 })).await;
    let seen = ws_drain(&mut friend, 300).await;
    assert!(seen.iter().all(|m| m["type"] != "error"), "{seen:?}");
    assert!(seen.iter().any(|m| m["type"] == "pong"), "{seen:?}");

    // The browser closes: kim leaves the room.
    ws_drain(&mut host, 100).await;
    friend.close(None).await.unwrap();
    let presence = ws_next(&mut host, "presence").await;
    assert_eq!(presence["presence"]["online"].as_array().unwrap().len(), 1);
}

/// A friend who taps a reaction on a clip that hasn't played, or gets ready for a show
/// that has ended behind the room's back, is told why, and nothing goes out to anyone.
#[sqlx::test(migrator = "clipos_core::db::MIGRATOR")]
async fn the_live_room_says_why_a_tap_didnt_count(pool: PgPool) {
    let admin = admin_token(&pool).await;
    invite(&pool, "kim@gmail.com").await;
    let a = ready_clip(&pool, "google-oauth2|kim", "kim@gmail.com", "A").await;
    let app = app_with(pool.clone(), true).await;
    let kim = user_token("google-oauth2|kim", "kim@gmail.com");
    let show = new_show(&app, &admin, true).await;
    let addr = listen(&app).await;
    let (mut host, _) = ws_join(addr, &show, &admin).await;
    let (mut friend, _) = ws_join(addr, &show, &kim).await;
    ws_drain(&mut host, 100).await;

    let tap = json!({ "type": "react", "clipId": a, "emoji": "🔥", "atMs": 0 });
    ws_send(&mut friend, tap).await;
    assert_eq!(
        ws_next(&mut friend, "error").await["message"],
        "that clip hasn't played in this show, or it's in the trash"
    );

    clipos_core::shows::abandon(&pool, show.parse().unwrap())
        .await
        .unwrap();
    ws_send(&mut friend, json!({ "type": "ready", "ready": true })).await;
    assert_eq!(
        ws_next(&mut friend, "error").await["message"],
        "the show is over"
    );
    let seen = ws_drain(&mut host, 300).await;
    assert!(
        seen.iter()
            .all(|m| m["type"] != "reaction" && m["type"] != "showChanged"),
        "{seen:?}"
    );
}

/// Sign-in that fails on the server's side (here: the database) is a 1011, which the
/// client retries, not a refusal.
#[sqlx::test(migrator = "clipos_core::db::MIGRATOR")]
async fn the_live_room_closes_for_a_retry_when_sign_in_fails(pool: PgPool) {
    let admin = admin_token(&pool).await;
    let app = app_with(pool.clone(), true).await;
    let show = new_show(&app, &admin, false).await;
    let addr = listen(&app).await;
    lose_table(&pool, "invites").await;
    let mut ws = ws_open(addr, &show).await;
    ws_send(&mut ws, json!({ "type": "hello", "token": admin })).await;
    let (seen, code) = ws_close(&mut ws).await;
    assert_eq!(seen.last().unwrap()["message"], "something went wrong");
    assert_eq!(code, 1011);
}

/// When the database fails the host's changes, the host hears so and the clip on stays as
/// it was for everyone.
#[sqlx::test(migrator = "clipos_core::db::MIGRATOR")]
async fn the_live_room_keeps_its_state_when_the_database_fails(pool: PgPool) {
    let admin = admin_token(&pool).await;
    invite(&pool, "kim@gmail.com").await;
    let a = ready_clip(&pool, "google-oauth2|kim", "kim@gmail.com", "A").await;
    let b = ready_clip(&pool, "google-oauth2|kim", "kim@gmail.com", "B").await;
    let app = app_with(pool.clone(), true).await;
    let kim = user_token("google-oauth2|kim", "kim@gmail.com");
    let show = new_show(&app, &admin, true).await;
    let addr = listen(&app).await;
    let (mut host, _) = ws_join(addr, &show, &admin).await;
    let (mut friend, _) = ws_join(addr, &show, &kim).await;
    ws_send(&mut host, json!({ "type": "load", "clipId": a })).await;
    let loaded = ws_next(&mut friend, "state").await["state"].clone();

    // A state that can't be saved still goes out (a restart would resume from the last
    // one saved).
    refuse(
        &pool,
        "UPDATE ON shows",
        "OLD.live_state IS DISTINCT FROM NEW.live_state",
    )
    .await;
    ws_send(&mut host, json!({ "type": "seek", "positionMs": 1000 })).await;
    let state = ws_next(&mut friend, "state").await["state"].clone();
    assert_eq!(
        (state["seq"].as_u64(), state["positionMs"].as_f64()),
        (Some(2), Some(1000.0))
    );
    assert_eq!(saved_state(&pool, &show).await, loaded);

    // The lineup can't be read: no load, no play, and a REST change leaves the clip on.
    lose_table(&pool, "show_clips").await;
    for msg in [
        json!({ "type": "load", "clipId": b }),
        json!({ "type": "play" }),
    ] {
        ws_send(&mut host, msg).await;
        assert_eq!(
            ws_next(&mut host, "error").await["message"],
            "something went wrong"
        );
    }
    app.state.hub.show_changed(show.parse().unwrap());
    ws_next(&mut friend, "showChanged").await;
    assert!(states(&ws_drain(&mut friend, 300).await).is_empty());
    assert_eq!(saved_state(&pool, &show).await, loaded);

    // The show itself can't be read: nothing the host steers goes through, and the room
    // stays open.
    lose_table(&pool, "shows").await;
    ws_send(&mut host, json!({ "type": "pause" })).await;
    assert_eq!(
        ws_next(&mut host, "error").await["message"],
        "something went wrong"
    );
    app.state.hub.show_changed(show.parse().unwrap());
    ws_next(&mut friend, "showChanged").await;
    let seen = ws_drain(&mut friend, 300).await;
    assert!(states(&seen).is_empty(), "{seen:?}");
    ws_send(&mut friend, json!({ "type": "ping", "clientMs": 1 })).await;
    ws_next(&mut friend, "pong").await;
}

/// A takeover the database refuses changes no one's host.
#[sqlx::test(migrator = "clipos_core::db::MIGRATOR")]
async fn a_takeover_the_database_refuses_changes_nothing(pool: PgPool) {
    let admin = admin_token(&pool).await;
    invite(&pool, "kim@gmail.com").await;
    let timing = quick(|t| t.takeover_after = std::time::Duration::from_millis(300));
    let app = app_timed(pool.clone(), true, timing).await;
    let kim = user_token("google-oauth2|kim", "kim@gmail.com");
    let show = new_show(&app, &admin, true).await;
    let addr = listen(&app).await;
    let (host, welcome) = ws_join(addr, &show, &admin).await;
    let admin_id = welcome["userId"].clone();
    let (mut friend, _) = ws_join(addr, &show, &kim).await;
    drop(host);
    host_away(&mut friend).await;
    tokio::time::sleep(std::time::Duration::from_millis(400)).await;

    refuse(
        &pool,
        "UPDATE ON shows",
        "OLD.host_id IS DISTINCT FROM NEW.host_id",
    )
    .await;
    ws_send(&mut friend, json!({ "type": "takeOver" })).await;
    assert_eq!(
        ws_next(&mut friend, "error").await["message"],
        "something went wrong"
    );
    let seen = ws_drain(&mut friend, 300).await;
    assert!(seen.iter().all(|m| m["type"] != "presence"), "{seen:?}");
    let (_, body) = get(&app, &format!("/api/shows/{show}"), Some(&kim)).await;
    assert_eq!(json(&body)["host"]["id"], admin_id);
}

/// The hub's housekeeping closes the room of a show that ended without the REST API (so
/// no one told the room), and has nothing to do once no show is open.
#[sqlx::test(migrator = "clipos_core::db::MIGRATOR")]
async fn the_hubs_rounds_close_rooms_of_shows_that_ended_elsewhere(pool: PgPool) {
    let admin = admin_token(&pool).await;
    let app = app_with(pool.clone(), true).await;
    let show = new_show(&app, &admin, true).await;
    let addr = listen(&app).await;
    let (mut ws, _) = ws_join(addr, &show, &admin).await;
    clipos_core::shows::abandon(&pool, show.parse().unwrap())
        .await
        .unwrap();

    // The running hub's first round comes at once.
    let rounds = tokio::spawn(clipos_api::live::Hub::run(app.state.clone()));
    let (seen, code) = ws_close(&mut ws).await;
    assert_eq!(seen.last().unwrap()["type"], "showOver");
    assert_eq!(code, 4004);
    rounds.abort();
    // No show open: a round with nothing to do.
    app.state.hub.tick(&app.state).await.unwrap();
    assert!(clipos_core::shows::open(&pool).await.unwrap().is_none());
}

/// A round that fails (here: the database) doesn't stop the hub's rounds.
#[sqlx::test(migrator = "clipos_core::db::MIGRATOR")]
async fn the_hub_keeps_going_after_a_failed_round(pool: PgPool) {
    let app = app_with(pool.clone(), true).await;
    lose_table(&pool, "shows").await;
    assert!(app.state.hub.tick(&app.state).await.is_err());
    let rounds = tokio::spawn(clipos_api::live::Hub::run(app.state.clone()));
    tokio::time::sleep(std::time::Duration::from_millis(300)).await;
    assert!(!rounds.is_finished(), "the rounds stopped");
    rounds.abort();
}

/// A connection that falls behind the room (its messages queued faster than it could
/// take them) gets the current state, presence and a nudge to fetch the show again,
/// rather than missing what changed.
#[sqlx::test(migrator = "clipos_core::db::MIGRATOR")]
async fn a_connection_that_falls_behind_catches_up(pool: PgPool) {
    let admin = admin_token(&pool).await;
    invite(&pool, "kim@gmail.com").await;
    let app = app_with(pool.clone(), true).await;
    let kim = user_token("google-oauth2|kim", "kim@gmail.com");
    let show = new_show(&app, &admin, false).await;
    let addr = listen(&app).await;
    let (_host, _) = ws_join(addr, &show, &admin).await;
    let (mut friend, welcome) = ws_join(addr, &show, &kim).await;
    ws_drain(&mut friend, 100).await;

    // Kim's "ready" waits on a locked row, so her connection takes nothing from the room
    // meanwhile...
    let mut lock = pool.begin().await.unwrap();
    sqlx::query(
        "SELECT 1 FROM show_participants WHERE show_id = $1::uuid AND user_id = $2::uuid
            FOR UPDATE",
    )
    .bind(&show)
    .bind(welcome["userId"].as_str().unwrap())
    .execute(&mut *lock)
    .await
    .unwrap();
    ws_send(&mut friend, json!({ "type": "ready", "ready": true })).await;
    clipos_core::testing::until_waiting_on_a_lock(&pool).await;
    // ...while more goes out than the room keeps for her.
    for _ in 0..300 {
        app.state.hub.show_changed(show.parse().unwrap());
    }
    lock.rollback().await.unwrap();

    // Nothing changed the live state, so a state can only be her catching up.
    let seen = ws_drain(&mut friend, 1000).await;
    let caught_up = seen
        .iter()
        .position(|m| m["type"] == "state")
        .expect("caught up");
    assert_eq!(seen[caught_up]["state"], welcome["state"]);
    assert_eq!(seen[caught_up + 1]["type"], "presence");
    assert_eq!(seen[caught_up + 2]["type"], "showChanged");
    assert!(
        seen.iter().all(|m| m["type"] != "error"),
        "ready went through"
    );
}

/// The seqs of the states among `seen`.
fn seqs(seen: &[Value]) -> Vec<u64> {
    states(seen)
        .iter()
        .map(|s| s["seq"].as_u64().unwrap())
        .collect()
}

/// The whole night over real sockets: friends join in the lobby and get ready, the host
/// starts, loads, plays, pauses and seeks every clip, friends react, a late joiner comes
/// in for the finale, everyone votes, the show ends and everyone is sent home.
#[sqlx::test(migrator = "clipos_core::db::MIGRATOR")]
async fn a_whole_show_over_the_live_room(pool: PgPool) {
    let admin = admin_token(&pool).await;
    let mut tokens = Vec::new();
    for name in ["sam", "kim", "lee", "max"] {
        let email = format!("{name}@gmail.com");
        invite(&pool, &email).await;
        tokens.push(user_token(&format!("google-oauth2|{name}"), &email));
    }
    let [sam, kim, lee, max] = <[String; 4]>::try_from(tokens).unwrap();
    let a = ready_clip(&pool, "google-oauth2|sam", "sam@gmail.com", "A").await;
    let b = ready_clip(&pool, "google-oauth2|kim", "kim@gmail.com", "B").await;
    let c = ready_clip(&pool, "google-oauth2|lee", "lee@gmail.com", "C").await;
    for clip in [&a, &b, &c] {
        clipos_core::clips::hold(&pool, clip.parse().unwrap())
            .await
            .unwrap();
    }
    let app = app_with(pool.clone(), true).await;
    let show = new_show(&app, &admin, false).await;
    let addr = listen(&app).await;
    let post = |path: String, who: String| {
        let app = &app;
        async move { send(app, "POST", &path, Some(&who), None, &[]).await }
    };

    // The lobby: everyone joins and gets ready; nothing plays yet.
    let (mut host, _) = ws_join(addr, &show, &admin).await;
    let mut friends = Vec::new();
    for t in [&sam, &kim, &lee] {
        friends.push(ws_join(addr, &show, t).await.0);
    }
    for f in friends.iter_mut() {
        ws_send(f, json!({ "type": "ready", "ready": true })).await;
    }
    ws_send(&mut host, json!({ "type": "load", "clipId": a })).await;
    assert_eq!(
        ws_next(&mut host, "error").await["message"],
        "only while the show is live"
    );
    let ready = || async {
        let (_, body) = get(&app, &format!("/api/shows/{show}"), Some(&admin)).await;
        let view = json(&body);
        let participants = view["participants"].as_array().unwrap();
        participants.iter().filter(|p| p["ready"] == true).count()
    };
    tokio::time::timeout(std::time::Duration::from_secs(3), async {
        while ready().await < 3 {
            tokio::time::sleep(std::time::Duration::from_millis(20)).await;
        }
    })
    .await
    .expect("everyone ready");
    let (status, _) = post(format!("/api/shows/{show}/start"), admin.clone()).await;
    assert_eq!(status, StatusCode::OK);

    // Every clip: load, play, pause, three seeks (past the end clamps to its length, before
    // the start to 0), play again.
    for clip in [&a, &b, &c] {
        ws_send(&mut host, json!({ "type": "load", "clipId": clip })).await;
        ws_send(&mut host, json!({ "type": "play" })).await;
        ws_send(&mut host, json!({ "type": "pause" })).await;
        for position in [2500, 99999, -5] {
            ws_send(&mut host, json!({ "type": "seek", "positionMs": position })).await;
        }
        ws_send(&mut host, json!({ "type": "play" })).await;
    }
    // Everyone gets every change, in order.
    for ws in std::iter::once(&mut host).chain(friends.iter_mut()) {
        let mut seen = Vec::new();
        for _ in 0..21 {
            seen.push(ws_next(ws, "state").await);
        }
        assert_eq!(seqs(&seen), (1..=21).collect::<Vec<_>>());
        assert_eq!(seen[4]["state"]["positionMs"], 5000.0);
        assert_eq!(seen[5]["state"]["positionMs"], 0.0);
    }
    ws_send(
        &mut friends[1],
        json!({ "type": "react", "clipId": a, "emoji": "🍌", "atMs": 100 }),
    )
    .await;
    ws_send(
        &mut friends[0],
        json!({ "type": "react", "clipId": b, "emoji": "🔥", "atMs": 100 }),
    )
    .await;
    ws_next(&mut host, "reaction").await;
    ws_next(&mut host, "reaction").await;
    for clip in [&a, &b, &c] {
        assert_eq!(lineup_row(&pool, &show, clip).await, (true, false));
        assert!(!is_held(&pool, clip).await);
    }

    // The finale: playback stops, a late joiner comes in, everyone votes.
    let (status, _) = post(format!("/api/shows/{show}/finale"), admin.clone()).await;
    assert_eq!(status, StatusCode::OK);
    ws_send(&mut host, json!({ "type": "pause" })).await;
    assert_eq!(
        ws_next(&mut host, "error").await["message"],
        "only while the show is live"
    );
    let (mut late, welcome) = ws_join(addr, &show, &max).await;
    assert_eq!(welcome["state"]["seq"], 21);
    let vote = |category: &str, who: &str, clip: &str| {
        let (app, path) = (&app, format!("/api/shows/{show}/votes/{category}"));
        let (who, body) = (who.to_owned(), json!({ "clipId": clip }));
        async move { send(app, "PUT", &path, Some(&who), Some(body), &[]).await.0 }
    };
    // Your own clip too (voting again changes it), and fails only for a clip with a 🍌.
    assert_eq!(vote("clip", &sam, &a).await, StatusCode::OK);
    assert_eq!(vote("clip", &sam, &b).await, StatusCode::OK);
    assert_eq!(vote("clip", &kim, &a).await, StatusCode::OK);
    assert_eq!(vote("clip", &lee, &a).await, StatusCode::OK);
    assert_eq!(vote("clip", &max, &a).await, StatusCode::OK);
    assert_eq!(vote("fail", &max, &a).await, StatusCode::OK);
    assert_eq!(vote("fail", &kim, &b).await, StatusCode::BAD_REQUEST);

    // The end: winners, and everyone is sent home.
    let (status, body) = post(format!("/api/shows/{show}/end"), admin.clone()).await;
    assert_eq!(status, StatusCode::OK, "{body}");
    assert_eq!(json(&body)["clipWinnerId"], json!(a));
    assert_eq!(json(&body)["failWinnerId"], json!(a));
    for ws in std::iter::once(&mut host)
        .chain(std::iter::once(&mut late))
        .chain(friends.iter_mut())
    {
        let (seen, code) = ws_close(ws).await;
        assert_eq!(seen.last().unwrap()["type"], "showOver");
        assert_eq!(code, 4004);
    }
}
