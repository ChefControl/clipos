//! Helpers for tests, here and in the other crates (feature `test-util`).

use std::time::{Duration, Instant};

use sqlx::PgPool;

/// Waits (at most 5 s) until another connection to this database is blocked on a lock:
/// a test that holds a row lock then knows the statement it races has got that far.
pub async fn until_waiting_on_a_lock(pool: &PgPool) {
    let deadline = Instant::now() + Duration::from_secs(5);
    loop {
        let waiting: i64 = sqlx::query_scalar(
            "SELECT count(*) FROM pg_stat_activity
              WHERE datname = current_database() AND wait_event_type = 'Lock'",
        )
        .fetch_one(pool)
        .await
        .expect("reading pg_stat_activity");
        if waiting > 0 {
            return;
        }
        assert!(Instant::now() < deadline, "nothing waited on the lock");
        tokio::time::sleep(Duration::from_millis(10)).await;
    }
}
