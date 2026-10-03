use std::{
    str::FromStr,
    time::{Duration, SystemTime},
};

use anyhow::Context;
use sqlx::{
    AssertSqlSafe, PgPool,
    migrate::Migrator,
    postgres::{PgConnectOptions, PgPoolOptions},
};

use crate::azure::{self, Credential};

/// Migrations embedded at compile time from `/migrations`.
pub static MIGRATOR: Migrator = sqlx::migrate!("../../migrations");

/// How the database login is authenticated.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub enum DbAuth {
    /// Password in `DATABASE_URL` (local Postgres).
    #[default]
    Password,
    /// Microsoft Entra token as the password (Azure; Entra-only server).
    Entra,
}

impl FromStr for DbAuth {
    type Err = String;

    fn from_str(s: &str) -> Result<Self, Self::Err> {
        match s.to_ascii_lowercase().as_str() {
            "password" => Ok(Self::Password),
            "entra" => Ok(Self::Entra),
            other => Err(format!(
                "unknown database auth {other:?} (expected password or entra)"
            )),
        }
    }
}

/// Refresh this long before the token expires. Entra tokens live 60–90 minutes.
const TOKEN_REFRESH_MARGIN: Duration = Duration::from_secs(15 * 60);
const TOKEN_REFRESH_MAX: Duration = Duration::from_secs(30 * 60);
const TOKEN_RETRY: Duration = Duration::from_secs(30);

pub async fn connect(
    database_url: &str,
    max_connections: u32,
    auth: DbAuth,
) -> anyhow::Result<PgPool> {
    let options: PgConnectOptions = database_url.parse().context("parsing DATABASE_URL")?;
    let pool_options = PgPoolOptions::new()
        .max_connections(max_connections)
        .acquire_timeout(Duration::from_secs(5));

    match auth {
        DbAuth::Password => pool_options
            .connect_with(options)
            .await
            .context("connecting to Postgres"),
        DbAuth::Entra => connect_entra(pool_options, options, Credential::from_env()).await,
    }
}

/// Connects with an Entra token from `credential` as the password, and keeps the token
/// fresh for new connections.
async fn connect_entra(
    pool_options: PgPoolOptions,
    options: PgConnectOptions,
    credential: Credential,
) -> anyhow::Result<PgPool> {
    let token = credential
        .token(azure::POSTGRES_RESOURCE)
        .await
        .context("getting an Entra token for Postgres")?;
    let pool = pool_options
        .connect_with(options.clone().password(&token.secret))
        .await
        .context("connecting to Postgres")?;
    tokio::spawn(refresh_token(
        pool.clone(),
        options,
        credential,
        token.expires_at,
    ));
    Ok(pool)
}

/// Postgres only checks the token when a connection opens, so swapping fresh credentials
/// into the pool before the old token expires is enough; open connections stay valid.
async fn refresh_token(
    pool: PgPool,
    options: PgConnectOptions,
    credential: Credential,
    expires_at: SystemTime,
) {
    let mut delay = refresh_delay(expires_at, SystemTime::now());
    loop {
        tokio::time::sleep(delay).await;
        if pool.is_closed() {
            return;
        }
        delay = refresh_once(&pool, &options, &credential).await;
    }
}

/// Swaps a fresh token into the pool. Returns how long until the next refresh.
async fn refresh_once(
    pool: &PgPool,
    options: &PgConnectOptions,
    credential: &Credential,
) -> Duration {
    match credential.token(azure::POSTGRES_RESOURCE).await {
        Ok(token) => {
            pool.set_connect_options(options.clone().password(&token.secret));
            tracing::debug!("refreshed the Postgres access token");
            refresh_delay(token.expires_at, SystemTime::now())
        }
        Err(e) => {
            tracing::error!(error = %format!("{e:#}"), "refreshing the Postgres access token failed");
            TOKEN_RETRY
        }
    }
}

fn refresh_delay(expires_at: SystemTime, now: SystemTime) -> Duration {
    expires_at
        .duration_since(now)
        .unwrap_or_default()
        .saturating_sub(TOKEN_REFRESH_MARGIN)
        .clamp(TOKEN_RETRY, TOKEN_REFRESH_MAX)
}

/// Applies pending migrations. Only the `api` calls this; sqlx takes an advisory lock,
/// so concurrent instances are safe.
pub async fn migrate(pool: &PgPool) -> anyhow::Result<()> {
    MIGRATOR.run(pool).await.context("running migrations")
}

/// Gives `role` read/write on every table and sequence in `public`. In Azure the api's
/// login owns the tables (it runs the migrations), so it grants the worker's login access
/// after each migrate.
pub async fn grant_table_access(pool: &PgPool, role: &str) -> anyhow::Result<()> {
    let role = quote_ident(role);
    let sql = format!(
        "GRANT SELECT, INSERT, UPDATE, DELETE ON ALL TABLES IN SCHEMA public TO {role};
         GRANT USAGE, SELECT ON ALL SEQUENCES IN SCHEMA public TO {role};"
    );
    sqlx::raw_sql(AssertSqlSafe(sql))
        .execute(pool)
        .await
        .with_context(|| format!("granting table access to {role}"))?;
    Ok(())
}

fn quote_ident(ident: &str) -> String {
    format!("\"{}\"", ident.replace('"', "\"\""))
}

/// Returns true once every migration this binary was built with has been applied.
/// The `worker` waits on this so it never runs against an older schema.
pub async fn schema_is_current(pool: &PgPool) -> anyhow::Result<bool> {
    let applied: Vec<i64> =
        match sqlx::query_scalar("SELECT version FROM _sqlx_migrations WHERE success")
            .fetch_all(pool)
            .await
        {
            Ok(versions) => versions,
            // 42P01: the table doesn't exist yet (nothing migrated). 42501: in Azure the
            // api grants the worker access only after migrating.
            Err(sqlx::Error::Database(e))
                if matches!(e.code().as_deref(), Some("42P01" | "42501")) =>
            {
                return Ok(false);
            }
            Err(e) => return Err(e).context("reading applied migrations"),
        };

    Ok(MIGRATOR
        .iter()
        .filter(|m| !m.migration_type.is_down_migration())
        .all(|m| applied.contains(&m.version)))
}

/// Cheap liveness check used by `/healthz`.
pub async fn ping(pool: &PgPool) -> anyhow::Result<()> {
    sqlx::query("SELECT 1").execute(pool).await?;
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn refresh_delay_is_bounded() {
        let now = SystemTime::UNIX_EPOCH + Duration::from_secs(1_000_000);
        let mins = |m: u64| Duration::from_secs(m * 60);

        // 90-minute token: capped at 30 minutes.
        assert_eq!(refresh_delay(now + mins(90), now), mins(30));
        // 40 minutes left: refresh 15 minutes before expiry.
        assert_eq!(refresh_delay(now + mins(40), now), mins(25));
        // Nearly or already expired: retry soon, never spin.
        assert_eq!(refresh_delay(now + mins(10), now), TOKEN_RETRY);
        assert_eq!(refresh_delay(now - mins(10), now), TOKEN_RETRY);
    }

    #[test]
    fn db_auth_parses() {
        assert_eq!("password".parse(), Ok(DbAuth::Password));
        assert_eq!("Entra".parse(), Ok(DbAuth::Entra));
        assert_eq!(
            "kerberos".parse::<DbAuth>(),
            Err("unknown database auth \"kerberos\" (expected password or entra)".into())
        );
        assert_eq!(DbAuth::default(), DbAuth::Password);
    }

    /// `DATABASE_URL`, the server every database test runs against.
    fn database_url() -> url::Url {
        std::env::var("DATABASE_URL")
            .expect("DATABASE_URL")
            .parse()
            .unwrap()
    }

    async fn current_user(pool: &PgPool) -> String {
        sqlx::query_scalar("SELECT current_user::text")
            .fetch_one(pool)
            .await
            .unwrap()
    }

    #[tokio::test]
    async fn connects_with_the_password_in_the_url() {
        let url = database_url();
        let pool = connect(url.as_str(), 2, DbAuth::Password).await.unwrap();
        assert_eq!(current_user(&pool).await, url.username());
        assert_eq!(pool.options().get_max_connections(), 2);
        ping(&pool).await.unwrap();

        let mut wrong = url.clone();
        wrong.set_password(Some("not-the-password")).unwrap();
        let err = connect(wrong.as_str(), 2, DbAuth::Password)
            .await
            .unwrap_err();
        assert!(
            format!("{err:#}").starts_with("connecting to Postgres"),
            "{err:#}"
        );

        let err = connect("not a url", 2, DbAuth::Password).await.unwrap_err();
        assert!(
            format!("{err:#}").starts_with("parsing DATABASE_URL"),
            "{err:#}"
        );
    }

    /// A stand-in for the App Service managed-identity endpoint that answers with
    /// `token` (valid for an hour), or with `status` when it isn't 200.
    async fn identity_endpoint(status: u16, token: &str) -> Credential {
        let token = token.to_owned();
        let expires_on = (SystemTime::now() + Duration::from_secs(3600))
            .duration_since(SystemTime::UNIX_EPOCH)
            .unwrap()
            .as_secs();
        let app = axum::Router::new().route(
            "/msi/token",
            axum::routing::get(move || async move {
                (
                    axum::http::StatusCode::from_u16(status).unwrap(),
                    axum::Json(serde_json::json!({
                        "access_token": token,
                        "expires_on": expires_on.to_string(),
                    })),
                )
            }),
        );
        let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
        let addr = listener.local_addr().unwrap();
        tokio::spawn(async move { axum::serve(listener, app).await.unwrap() });
        Credential::app_service(format!("http://{addr}/msi/token"), "header")
    }

    /// `DATABASE_URL`'s options without its password, as an Entra login has none.
    fn options_without_password() -> (PgConnectOptions, String) {
        let mut url = database_url();
        let password = url
            .password()
            .expect("a password in DATABASE_URL")
            .to_owned();
        url.set_password(None).unwrap();
        (url.as_str().parse().unwrap(), password)
    }

    #[tokio::test]
    async fn entra_logs_in_with_the_token_as_password() {
        let (options, password) = options_without_password();

        // Our local server takes the real password where Azure takes the token.
        let pool = connect_entra(
            PgPoolOptions::new().max_connections(1),
            options.clone(),
            identity_endpoint(200, &password).await,
        )
        .await
        .unwrap();
        assert_eq!(current_user(&pool).await, database_url().username());
        pool.close().await;

        let err = connect_entra(
            PgPoolOptions::new(),
            options.clone(),
            identity_endpoint(200, "not-the-password").await,
        )
        .await
        .unwrap_err();
        assert!(
            format!("{err:#}").starts_with("connecting to Postgres"),
            "{err:#}"
        );

        let err = connect_entra(
            PgPoolOptions::new(),
            options,
            identity_endpoint(500, "").await,
        )
        .await
        .unwrap_err();
        assert!(
            format!("{err:#}").starts_with("getting an Entra token for Postgres"),
            "{err:#}"
        );
    }

    #[tokio::test]
    async fn a_refresh_swaps_the_new_token_into_the_pool() {
        let (options, password) = options_without_password();
        // Opens connections with a token that no longer works.
        let pool = PgPoolOptions::new()
            .acquire_timeout(Duration::from_secs(5))
            .connect_lazy_with(options.clone().password("expired-token"));
        assert!(pool.acquire().await.is_err());

        let next = refresh_once(&pool, &options, &identity_endpoint(200, &password).await).await;
        // An hour's token is refreshed after the 30-minute cap.
        assert_eq!(next, TOKEN_REFRESH_MAX);
        assert_eq!(current_user(&pool).await, database_url().username());

        let next = refresh_once(&pool, &options, &identity_endpoint(503, "").await).await;
        assert_eq!(next, TOKEN_RETRY);
        // The pool keeps the last good token.
        ping(&pool).await.unwrap();
        pool.close().await;
    }

    #[tokio::test(start_paused = true)]
    async fn the_refresh_loop_retries_until_the_pool_closes() {
        let (options, _) = options_without_password();
        let pool = PgPoolOptions::new().connect_lazy_with(options.clone());
        // Fails at once, without any I/O, so the paused clock decides everything.
        let credential = Credential::app_service("not a url", "header");
        let refresher = tokio::spawn(refresh_token(
            pool.clone(),
            options,
            credential,
            SystemTime::now(),
        ));

        tokio::time::sleep(TOKEN_RETRY * 3 + Duration::from_secs(1)).await;
        assert!(!refresher.is_finished(), "a failed refresh is retried");

        pool.close().await;
        tokio::time::sleep(TOKEN_RETRY + Duration::from_secs(1)).await;
        assert!(refresher.is_finished(), "stops once the pool is closed");
        refresher.await.unwrap();
    }

    #[sqlx::test(migrations = false)]
    async fn migrating_makes_the_schema_current(pool: PgPool) {
        // Nothing applied yet: not even the migrations table.
        assert!(!schema_is_current(&pool).await.unwrap());

        migrate(&pool).await.unwrap();
        assert!(schema_is_current(&pool).await.unwrap());
        // Migrating again is a no-op.
        migrate(&pool).await.unwrap();

        // A database one migration behind this build isn't current.
        sqlx::query(
            "DELETE FROM _sqlx_migrations WHERE version = (SELECT max(version) FROM _sqlx_migrations)",
        )
        .execute(&pool)
        .await
        .unwrap();
        assert!(!schema_is_current(&pool).await.unwrap());
    }

    #[sqlx::test(migrations = false)]
    async fn unreadable_migrations_are_errors(pool: PgPool) {
        // Some other tool's table under sqlx's name.
        sqlx::query("CREATE TABLE _sqlx_migrations (version BIGINT)")
            .execute(&pool)
            .await
            .unwrap();
        let err = schema_is_current(&pool).await.unwrap_err();
        assert!(
            format!("{err:#}").starts_with("reading applied migrations"),
            "{err:#}"
        );
        let err = migrate(&pool).await.unwrap_err();
        assert!(
            format!("{err:#}").starts_with("running migrations"),
            "{err:#}"
        );

        pool.close().await;
        assert!(schema_is_current(&pool).await.is_err());
        assert!(ping(&pool).await.is_err());
    }

    #[sqlx::test(migrator = "crate::db::MIGRATOR")]
    async fn granting_to_a_missing_role_is_an_error(pool: PgPool) {
        let err = grant_table_access(&pool, "no-such-role").await.unwrap_err();
        assert!(
            format!("{err:#}").starts_with("granting table access to \"no-such-role\""),
            "{err:#}"
        );
    }

    #[test]
    fn identifiers_are_quoted() {
        assert_eq!(quote_ident("clipos-worker"), r#""clipos-worker""#);
        assert_eq!(quote_ident(r#"a"; DROP"#), r#""a""; DROP""#);
    }

    #[sqlx::test(migrator = "crate::db::MIGRATOR")]
    async fn grants_table_access(pool: PgPool) {
        // Roles are cluster-wide, so use a unique name and drop it afterwards.
        let role = format!("worker-{}", uuid::Uuid::new_v4().simple());
        sqlx::raw_sql(AssertSqlSafe(format!("CREATE ROLE {}", quote_ident(&role))))
            .execute(&pool)
            .await
            .unwrap();

        grant_table_access(&pool, &role).await.unwrap();

        let can_write: bool =
            sqlx::query_scalar("SELECT has_table_privilege($1, 'jobs', 'UPDATE')")
                .bind(&role)
                .fetch_one(&pool)
                .await
                .unwrap();
        let inherits_owner: bool = sqlx::query_scalar(
            "SELECT pg_has_role($1, (SELECT tableowner FROM pg_tables WHERE tablename = 'jobs'), 'MEMBER')",
        )
        .bind(&role)
        .fetch_one(&pool)
        .await
        .unwrap();

        sqlx::raw_sql(AssertSqlSafe(format!(
            "REVOKE ALL ON ALL TABLES IN SCHEMA public FROM {role}; \
             REVOKE ALL ON ALL SEQUENCES IN SCHEMA public FROM {role}; DROP ROLE {role};",
            role = quote_ident(&role)
        )))
        .execute(&pool)
        .await
        .unwrap();

        assert!(can_write);
        assert!(!inherits_owner);
    }
}
