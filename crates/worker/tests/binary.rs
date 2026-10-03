//! The `clipos-worker` binary itself, configured through the environment like in its
//! container.

use std::{
    net::SocketAddr,
    process::{Command, Stdio},
    time::{Duration, Instant},
};

use serde_json::{Value, json};
use sqlx::PgPool;

/// Azurite's well-known development account (public, not a secret).
const AZURITE_KEY: &str =
    "Eby8vdM02xNOcqFlqUwJPLlmEtlCDXJ1OUzFT50uSRZ6IFsuFq2UVErCz4I6tq/K1SZFPTOtr/KBHBeksoGMGw==";

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

/// The status of `GET /healthz` on `addr`, or `None` if nothing answers.
async fn health(addr: SocketAddr) -> Option<u16> {
    use tokio::io::{AsyncReadExt, AsyncWriteExt};
    let mut stream = tokio::net::TcpStream::connect(addr).await.ok()?;
    stream
        .write_all(
            format!("GET /healthz HTTP/1.1\r\nHost: {addr}\r\nConnection: close\r\n\r\n")
                .as_bytes(),
        )
        .await
        .ok()?;
    let mut response = String::new();
    stream.read_to_string(&mut response).await.ok()?;
    response.split(' ').nth(1)?.parse().ok()
}

/// Kills the process if the test fails before it stops, so it never outlives the test.
struct Proc(Option<std::process::Child>);

impl Drop for Proc {
    fn drop(&mut self) {
        if let Some(child) = &mut self.0 {
            let _ = child.kill();
            let _ = child.wait();
        }
    }
}

/// SIGTERM (what App Service and `docker stop` send) lets the current job finish and
/// stops the worker cleanly; its JSON logs say so.
#[cfg(unix)]
#[sqlx::test(migrator = "clipos_core::db::MIGRATOR")]
async fn the_binary_runs_jobs_and_stops_on_sigterm(pool: PgPool) {
    if std::env::var_os("CLIPOS_AZURITE").is_none() {
        eprintln!("CLIPOS_AZURITE not set; skipping");
        return;
    }
    let job = clipos_core::jobs::enqueue(&pool, "noop", json!({}))
        .await
        .unwrap();
    let addr = std::net::TcpListener::bind("127.0.0.1:0")
        .unwrap()
        .local_addr()
        .unwrap();
    let cwd = tempfile::tempdir().unwrap();
    let mut child = Proc(Some(
        Command::new(env!("CARGO_BIN_EXE_clipos-worker"))
            // No `.env` to pick up from here.
            .current_dir(cwd.path())
            .env("DATABASE_URL", database_url(&pool).await)
            .env("BIND_ADDR", addr.to_string())
            .env("POLL_INTERVAL_MS", "50")
            .env("STORAGE_ACCOUNT", "devstoreaccount1")
            .env(
                "STORAGE_BLOB_ENDPOINT",
                "http://127.0.0.1:10000/devstoreaccount1",
            )
            .env("STORAGE_ACCOUNT_KEY", AZURITE_KEY)
            .env("TRANSCODE_DIR", cwd.path().join("transcode"))
            .env("KILLFEED_ANALYSE", "false")
            .env("WEBSITE_INSTANCE_ID", "instance-1")
            .env("LOG_FORMAT", "json")
            .env_remove("RUST_LOG")
            .env_remove("DATABASE_AUTH")
            .stdout(Stdio::piped())
            .stderr(Stdio::piped())
            .spawn()
            .unwrap(),
    ));

    let deadline = Instant::now() + Duration::from_secs(60);
    loop {
        let done: String = sqlx::query_scalar("SELECT status FROM jobs WHERE id = $1")
            .bind(job)
            .fetch_one(&pool)
            .await
            .unwrap();
        if done == "succeeded" && health(addr).await == Some(200) {
            break;
        }
        if let Some(status) = child.0.as_mut().unwrap().try_wait().unwrap() {
            panic!("exited early: {status}");
        }
        assert!(Instant::now() < deadline, "the job never ran");
        tokio::time::sleep(Duration::from_millis(50)).await;
    }
    assert!(cwd.path().join("transcode").is_dir());

    let child = child.0.take().unwrap();
    let pid = child.id();
    assert!(
        Command::new("kill")
            .args(["-TERM", &pid.to_string()])
            .status()
            .unwrap()
            .success()
    );
    let (tx, rx) = std::sync::mpsc::channel();
    std::thread::spawn(move || tx.send(child.wait_with_output()));
    let Ok(out) = rx.recv_timeout(Duration::from_secs(30)) else {
        let _ = Command::new("kill")
            .args(["-KILL", &pid.to_string()])
            .status();
        panic!("the worker didn't stop");
    };
    let out = out.unwrap();
    let stdout = String::from_utf8_lossy(&out.stdout);
    assert!(
        out.status.success(),
        "{}\n{stdout}\n{}",
        out.status,
        String::from_utf8_lossy(&out.stderr)
    );

    let events: Vec<Value> = stdout
        .lines()
        .map(|l| serde_json::from_str(l).unwrap_or_else(|_| panic!("not JSON: {l}")))
        .collect();
    let started = events
        .iter()
        .find(|e| e["message"] == "worker started")
        .expect("started");
    assert_eq!(started["worker_id"], "instance-1");
    for m in [
        "job succeeded",
        "shutdown requested; finishing current job",
        "worker stopped",
    ] {
        assert!(
            events.iter().any(|e| e["message"] == m),
            "{m:?} not in {stdout}"
        );
    }
    assert_eq!(health(addr).await, None, "no longer listening");
}

/// Settings come from the environment and flags; a missing one is a usage error.
#[test]
fn the_binary_explains_missing_settings() {
    let cwd = tempfile::tempdir().unwrap();
    let out = Command::new(env!("CARGO_BIN_EXE_clipos-worker"))
        .current_dir(cwd.path())
        .env_remove("STORAGE_ACCOUNT")
        .env("DATABASE_URL", "postgres://unused")
        .output()
        .unwrap();
    assert_eq!(out.status.code(), Some(2));
    let stderr = String::from_utf8_lossy(&out.stderr);
    assert!(
        stderr.contains("--storage-account <STORAGE_ACCOUNT>"),
        "{stderr}"
    );
}
