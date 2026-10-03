//! The api as its binary runs it: `start` + `Server::run` in-process, and the
//! `clipos-api` and `openapi` binaries themselves.

use std::{
    net::SocketAddr,
    path::Path,
    process::{Command, Stdio},
    sync::Arc,
    time::{Duration, Instant},
};

use axum::{body::Body, http::Request};
use clipos_api::{AppState, PublicConfig, config::Config};
use clipos_core::{
    auth::JwtVerifier,
    db::{self, DbAuth},
    storage::{Storage, StorageConfig},
    telemetry::LogFormat,
};
use serde_json::{Value, json};
use sqlx::{AssertSqlSafe, PgPool, postgres::PgPoolOptions};
use tower::ServiceExt;

/// Azurite's well-known development account (public, not a secret).
const AZURITE_KEY: &str =
    "Eby8vdM02xNOcqFlqUwJPLlmEtlCDXJ1OUzFT50uSRZ6IFsuFq2UVErCz4I6tq/K1SZFPTOtr/KBHBeksoGMGw==";
const AZURITE_ENDPOINT: &str = "http://127.0.0.1:10000/devstoreaccount1";

/// Tests that prepare Azurite need it running: set `CLIPOS_AZURITE` (CI and `make test`).
fn azurite() -> bool {
    let on = std::env::var_os("CLIPOS_AZURITE").is_some();
    if !on {
        eprintln!("CLIPOS_AZURITE not set; skipping");
    }
    on
}

/// The URL of the test database behind `pool`, on `DATABASE_URL`'s server.
async fn database_url(pool: &PgPool) -> String {
    let name: String = sqlx::query_scalar("SELECT current_database()::text")
        .fetch_one(pool)
        .await
        .unwrap();
    let server = std::env::var("DATABASE_URL").expect("DATABASE_URL");
    let (server, _) = server.rsplit_once('/').unwrap();
    format!("{server}/{name}")
}

/// A config like the one the binary parses from the environment, on a free port.
fn config(database_url: String, static_dir: &Path) -> Config {
    Config {
        database_url,
        database_auth: DbAuth::Password,
        db_max_connections: 4,
        db_worker_role: None,
        bind_addr: "127.0.0.1:0".parse().unwrap(),
        static_dir: static_dir.to_owned(),
        auth0_domain: "tenant.example.auth0.com".into(),
        auth0_audience: "http://localhost:8080/api".into(),
        auth0_client_id: "spa-client".into(),
        admin_emails: vec![],
        invite_check_secret: None,
        storage_account: "devstoreaccount1".into(),
        storage_blob_endpoint: Some(AZURITE_ENDPOINT.into()),
        storage_account_key: Some(AZURITE_KEY.into()),
        public_url: "http://localhost:5173/".into(),
        share_rate_per_minute: 120,
        share_media_mib_per_minute: 300,
        share_link_media_mib_per_minute: 2048,
        share_media_streams_per_client: 6,
        local_cors_origins: vec!["http://localhost:5173".into()],
        shows_for: "admins".into(),
        log_format: LogFormat::Pretty,
    }
}

async fn admins(pool: &PgPool) -> Vec<String> {
    sqlx::query_scalar("SELECT email FROM invites WHERE role = 'admin' ORDER BY email")
        .fetch_all(pool)
        .await
        .unwrap()
}

#[sqlx::test(migrations = false)]
async fn starts_serves_and_shuts_down(pool: PgPool) {
    if !azurite() {
        return;
    }
    let static_dir = tempfile::tempdir().unwrap();
    std::fs::write(
        static_dir.path().join("index.html"),
        "<!doctype html><title>the built spa</title>",
    )
    .unwrap();
    // Roles are cluster-wide, so use a unique name and drop it afterwards.
    let role = format!("worker-{}", uuid::Uuid::new_v4().simple());
    sqlx::raw_sql(AssertSqlSafe(format!("CREATE ROLE \"{role}\"")))
        .execute(&pool)
        .await
        .unwrap();

    let mut config = config(database_url(&pool).await, static_dir.path());
    config.db_worker_role = Some(role.clone());
    config.admin_emails = vec![
        "Sam@Example.com".into(),
        " ".into(),
        "ash@example.com".into(),
    ];
    config.invite_check_secret = Some("the-secret".into());
    let server = clipos_api::start(config).await.unwrap();
    let addr = server.local_addr().unwrap();
    let (stop, stopped) = tokio::sync::oneshot::channel::<()>();
    let running = tokio::spawn(server.run(async {
        stopped.await.ok();
    }));

    // Migrated, the worker's login granted access, and the admins seeded (normalized).
    assert!(db::schema_is_current(&pool).await.unwrap());
    let granted: bool = sqlx::query_scalar("SELECT has_table_privilege($1, 'jobs', 'UPDATE')")
        .bind(&role)
        .fetch_one(&pool)
        .await
        .unwrap();
    assert!(granted);
    assert_eq!(admins(&pool).await, ["ash@example.com", "sam@example.com"]);

    let http = reqwest::Client::new();
    let res = http
        .get(format!("http://{addr}/healthz"))
        .send()
        .await
        .unwrap();
    assert_eq!(res.status(), 200);
    assert_eq!(res.headers()["x-clipos-version"], clipos_core::version());
    assert_eq!(res.text().await.unwrap(), "ok");

    let config: Value = http
        .get(format!("http://{addr}/api/config"))
        .send()
        .await
        .unwrap()
        .json()
        .await
        .unwrap();
    assert_eq!(
        config,
        json!({
            "auth0Domain": "tenant.example.auth0.com",
            "auth0ClientId": "spa-client",
            "auth0Audience": "http://localhost:8080/api",
        })
    );
    // The SPA from STATIC_DIR, and the invite check behind INVITE_CHECK_SECRET.
    let page = http
        .get(format!("http://{addr}/clips"))
        .send()
        .await
        .unwrap();
    assert!(page.text().await.unwrap().contains("the built spa"));
    let check = http
        .post(format!("http://{addr}/internal/invites/check"))
        .json(&json!({ "email": "sam@example.com" }))
        .send()
        .await
        .unwrap();
    assert_eq!(check.status(), 401, "no secret");
    let check: Value = http
        .post(format!("http://{addr}/internal/invites/check"))
        .header("x-clipos-internal-secret", "the-secret")
        .json(&json!({ "email": "sam@example.com" }))
        .send()
        .await
        .unwrap()
        .json()
        .await
        .unwrap();
    assert_eq!(check, json!({ "allowed": true }));
    // Signed-in routes need a token.
    let me = http
        .get(format!("http://{addr}/api/me"))
        .send()
        .await
        .unwrap();
    assert_eq!(me.status(), 401);

    stop.send(()).unwrap();
    tokio::time::timeout(Duration::from_secs(30), running)
        .await
        .expect("stops when asked")
        .unwrap()
        .unwrap();
    assert!(
        http.get(format!("http://{addr}/healthz"))
            .send()
            .await
            .is_err(),
        "no longer listening"
    );

    sqlx::raw_sql(AssertSqlSafe(format!(
        "REVOKE ALL ON ALL TABLES IN SCHEMA public FROM \"{role}\"; \
         REVOKE ALL ON ALL SEQUENCES IN SCHEMA public FROM \"{role}\"; DROP ROLE \"{role}\";"
    )))
    .execute(&pool)
    .await
    .unwrap();
}

#[sqlx::test(migrations = false)]
async fn a_bad_admin_email_stops_startup(pool: PgPool) {
    let dir = tempfile::tempdir().unwrap();
    let mut config = config(database_url(&pool).await, dir.path());
    config.admin_emails = vec!["sam@example.com".into(), "not-an-email".into()];
    let err = clipos_api::start(config).await.err().unwrap();
    assert!(
        format!("{err:#}").starts_with("invalid ADMIN_EMAILS entry \"not-an-email\""),
        "{err:#}"
    );
    // Nothing half-seeded.
    assert!(admins(&pool).await.is_empty());
}

#[sqlx::test(migrations = false)]
async fn startup_needs_azurite_locally(pool: PgPool) {
    let dir = tempfile::tempdir().unwrap();
    let mut config = config(database_url(&pool).await, dir.path());
    config.storage_blob_endpoint = Some("http://127.0.0.1:9/devstoreaccount1".into());
    let err = clipos_api::start(config).await.err().unwrap();
    assert!(
        format!("{err:#}").starts_with("preparing Azurite (is it running? `make deps`)"),
        "{err:#}"
    );
}

#[tokio::test]
async fn startup_needs_the_database() {
    let dir = tempfile::tempdir().unwrap();
    let err = clipos_api::start(config("not a url".into(), dir.path()))
        .await
        .err()
        .unwrap();
    assert!(
        format!("{err:#}").starts_with("parsing DATABASE_URL"),
        "{err:#}"
    );
}

#[sqlx::test(migrations = false)]
async fn startup_fails_when_the_port_is_taken(pool: PgPool) {
    if !azurite() {
        return;
    }
    let taken = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let addr = taken.local_addr().unwrap();
    let dir = tempfile::tempdir().unwrap();
    let mut config = config(database_url(&pool).await, dir.path());
    config.bind_addr = addr;
    let err = clipos_api::start(config).await.err().unwrap();
    assert!(
        format!("{err:#}").starts_with(&format!("binding {addr}")),
        "{err:#}"
    );
}

#[tokio::test]
async fn healthz_reports_a_database_outage() {
    // Nothing listens on port 9.
    let pool = PgPoolOptions::new()
        .acquire_timeout(Duration::from_secs(1))
        .connect_lazy("postgres://clipos:clipos@127.0.0.1:9/clipos")
        .unwrap();
    let state = AppState {
        hub: Arc::new(clipos_api::live::Hub::new(pool.clone())),
        pool,
        verifier: Arc::new(JwtVerifier::new("tenant.example.auth0.com", "aud")),
        public_config: Arc::new(PublicConfig {
            auth0_domain: "tenant.example.auth0.com".into(),
            auth0_client_id: "spa-client".into(),
            auth0_audience: "aud".into(),
        }),
        internal_secret: None,
        storage: Arc::new(
            Storage::new(StorageConfig {
                account: "devstoreaccount1".into(),
                blob_endpoint: Some(AZURITE_ENDPOINT.into()),
                account_key: Some(AZURITE_KEY.into()),
            })
            .unwrap(),
        ),
        public_url: Arc::from("https://clips.example"),
        index_html: Arc::new(clipos_api::FALLBACK_INDEX.to_owned()),
        share_limits: Arc::new(clipos_api::ratelimit::ShareLimits::new(120, 300, 2048, 6)),
        shows_for_everyone: false,
    };
    let dir = tempfile::tempdir().unwrap();
    let res = clipos_api::router(state, dir.path())
        .oneshot(Request::get("/healthz").body(Body::empty()).unwrap())
        .await
        .unwrap();
    assert_eq!(res.status(), 503);
    assert_eq!(res.headers()["x-clipos-version"], clipos_core::version());
    let body = http_body_util::BodyExt::collect(res.into_body())
        .await
        .unwrap()
        .to_bytes();
    assert_eq!(body, "database unavailable");
}

/// The `openapi` binary prints what `make gen-api` commits as `web/openapi.json`.
#[test]
fn the_openapi_binary_prints_the_spec() {
    let out = Command::new(env!("CARGO_BIN_EXE_openapi"))
        .output()
        .unwrap();
    assert!(out.status.success());
    assert!(
        out.stdout == include_bytes!("../../../web/openapi.json"),
        "web/openapi.json is stale; run `make gen-api` and commit it"
    );
}

/// A port nothing listens on right now, for a child process to bind.
fn free_port() -> SocketAddr {
    std::net::TcpListener::bind("127.0.0.1:0")
        .unwrap()
        .local_addr()
        .unwrap()
}

/// A child process that's killed if the test fails before it stops, so it never
/// outlives the test (holding its database open).
struct Proc(Option<std::process::Child>);

impl Drop for Proc {
    fn drop(&mut self) {
        if let Some(child) = &mut self.0 {
            let _ = child.kill();
            let _ = child.wait();
        }
    }
}

impl Proc {
    fn spawn(cmd: &mut Command) -> Self {
        Self(Some(
            cmd.stdout(Stdio::piped())
                .stderr(Stdio::piped())
                .spawn()
                .unwrap(),
        ))
    }

    /// Polls `url` until it answers 200.
    async fn wait_until_up(&mut self, url: &str) {
        let deadline = Instant::now() + Duration::from_secs(60);
        loop {
            if let Ok(res) = reqwest::get(url).await
                && res.status() == 200
            {
                return;
            }
            if let Some(status) = self.0.as_mut().unwrap().try_wait().unwrap() {
                panic!("exited early: {status}");
            }
            assert!(Instant::now() < deadline, "{url} never came up");
            tokio::time::sleep(Duration::from_millis(100)).await;
        }
    }

    fn signal(&self, name: &str) {
        let pid = self.0.as_ref().unwrap().id();
        let status = Command::new("kill")
            .args([format!("-{name}"), pid.to_string()])
            .status()
            .unwrap();
        assert!(status.success());
    }

    /// Waits for the process to exit by itself (after a signal), asserts it succeeded,
    /// and returns its stdout.
    fn wait_for_clean_exit(&mut self) -> String {
        let child = self.0.take().unwrap();
        let pid = child.id();
        let (tx, rx) = std::sync::mpsc::channel();
        std::thread::spawn(move || tx.send(child.wait_with_output()));
        let Ok(out) = rx.recv_timeout(Duration::from_secs(30)) else {
            let _ = Command::new("kill")
                .args(["-KILL", &pid.to_string()])
                .status();
            panic!("process {pid} didn't stop");
        };
        let out = out.unwrap();
        let stdout = String::from_utf8_lossy(&out.stdout).into_owned();
        assert!(
            out.status.success(),
            "{}\n{stdout}\n{}",
            out.status,
            String::from_utf8_lossy(&out.stderr)
        );
        stdout
    }
}

/// The real binary, configured only through the environment like in a container: it
/// migrates a fresh database, serves, and stops cleanly on Ctrl-C.
#[cfg(unix)]
#[sqlx::test(migrations = false)]
async fn the_binary_runs_from_the_environment_and_stops_on_ctrl_c(pool: PgPool) {
    if !azurite() {
        return;
    }
    let addr = free_port();
    let cwd = tempfile::tempdir().unwrap();
    let mut child = Proc::spawn(
        Command::new(env!("CARGO_BIN_EXE_clipos-api"))
            // No `.env` to pick up from here.
            .current_dir(cwd.path())
            .env("DATABASE_URL", database_url(&pool).await)
            .env("BIND_ADDR", addr.to_string())
            .env("STATIC_DIR", cwd.path().join("no-spa"))
            .env("AUTH0_DOMAIN", "tenant.example.auth0.com")
            .env("AUTH0_AUDIENCE", "http://localhost:8080/api")
            .env("AUTH0_CLIENT_ID", "spa-client")
            .env("ADMIN_EMAILS", "sam@example.com,ash@example.com")
            .env("STORAGE_ACCOUNT", "devstoreaccount1")
            .env("STORAGE_BLOB_ENDPOINT", AZURITE_ENDPOINT)
            .env("STORAGE_ACCOUNT_KEY", AZURITE_KEY)
            .env("LOG_FORMAT", "pretty")
            .env("SHOWS_FOR", "everyone")
            .env("NO_COLOR", "1")
            .env_remove("RUST_LOG")
            .env_remove("DATABASE_AUTH")
            .env_remove("DB_WORKER_ROLE")
            .env_remove("INVITE_CHECK_SECRET")
            .env_remove("PUBLIC_URL")
            .env_remove("LOCAL_CORS_ORIGINS"),
    );

    child.wait_until_up(&format!("http://{addr}/healthz")).await;
    let config: Value = reqwest::get(format!("http://{addr}/api/config"))
        .await
        .unwrap()
        .json()
        .await
        .unwrap();
    assert_eq!(config["auth0ClientId"], "spa-client");
    // No built SPA: only /api works.
    let page = reqwest::get(format!("http://{addr}/")).await.unwrap();
    assert_eq!(page.status(), 404);
    assert_eq!(admins(&pool).await, ["ash@example.com", "sam@example.com"]);

    child.signal("INT");
    let stdout = child.wait_for_clean_exit();
    for line in [
        "no built SPA found; only /api will work",
        "listening",
        "shut down",
    ] {
        assert!(stdout.contains(line), "{line:?} not in:\n{stdout}");
    }
}

/// SIGTERM (what App Service and `docker stop` send) stops it cleanly too, and
/// `LOG_FORMAT=json` logs one JSON object per line.
#[cfg(unix)]
#[sqlx::test(migrations = false)]
async fn the_binary_stops_on_sigterm_and_logs_json(pool: PgPool) {
    if !azurite() {
        return;
    }
    let addr = free_port();
    let cwd = tempfile::tempdir().unwrap();
    let mut child = Proc::spawn(
        Command::new(env!("CARGO_BIN_EXE_clipos-api"))
            .current_dir(cwd.path())
            .args(["--database-url", &database_url(&pool).await])
            .args(["--bind-addr", &addr.to_string()])
            .args(["--static-dir", cwd.path().to_str().unwrap()])
            .env("AUTH0_DOMAIN", "tenant.example.auth0.com")
            .env("AUTH0_AUDIENCE", "http://localhost:8080/api")
            .env("AUTH0_CLIENT_ID", "spa-client")
            .env("STORAGE_ACCOUNT", "devstoreaccount1")
            .env("STORAGE_BLOB_ENDPOINT", AZURITE_ENDPOINT)
            .env("STORAGE_ACCOUNT_KEY", AZURITE_KEY)
            .env("LOG_FORMAT", "json")
            .env("RUST_LOG", "info")
            .env_remove("ADMIN_EMAILS")
            .env_remove("DB_WORKER_ROLE"),
    );

    child.wait_until_up(&format!("http://{addr}/healthz")).await;
    child.signal("TERM");
    let stdout = child.wait_for_clean_exit();
    let messages: Vec<String> = stdout
        .lines()
        .map(|l| {
            let v: Value = serde_json::from_str(l).unwrap_or_else(|_| panic!("not JSON: {l}"));
            v["message"].as_str().unwrap_or_default().to_owned()
        })
        .collect();
    for m in [
        "ADMIN_EMAILS is empty; only existing invites can sign in",
        "listening",
        "shut down",
    ] {
        assert!(messages.iter().any(|x| x == m), "{m:?} not in {messages:?}");
    }
}

/// Settings come from the environment and flags; a missing one is a usage error.
#[test]
fn the_binary_explains_missing_settings() {
    let cwd = tempfile::tempdir().unwrap();
    let out = Command::new(env!("CARGO_BIN_EXE_clipos-api"))
        .current_dir(cwd.path())
        .env_remove("AUTH0_DOMAIN")
        .env("DATABASE_URL", "postgres://unused")
        .output()
        .unwrap();
    assert_eq!(out.status.code(), Some(2));
    let stderr = String::from_utf8_lossy(&out.stderr);
    assert!(stderr.contains("--auth0-domain <AUTH0_DOMAIN>"), "{stderr}");
}
