use chrono::{DateTime, Utc};
use serde::{Deserialize, Serialize};
use sqlx::PgPool;
use utoipa::ToSchema;
use uuid::Uuid;

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, sqlx::Type, ToSchema)]
#[sqlx(type_name = "text", rename_all = "lowercase")]
#[serde(rename_all = "lowercase")]
pub enum Role {
    Admin,
    Member,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, sqlx::Type, ToSchema)]
#[sqlx(type_name = "text", rename_all = "lowercase")]
#[serde(rename_all = "lowercase")]
pub enum UserStatus {
    Active,
    Disabled,
}

#[derive(Debug, Clone, Serialize, sqlx::FromRow, ToSchema)]
#[serde(rename_all = "camelCase")]
pub struct User {
    pub id: Uuid,
    #[serde(skip)]
    pub auth0_sub: String,
    pub email: String,
    pub handle: String,
    pub display_name: String,
    pub avatar_url: Option<String>,
    pub steam_name: Option<String>,
    pub role: Role,
    pub status: UserStatus,
    pub created_at: DateTime<Utc>,
}

/// Identity details taken from a verified access token.
#[derive(Debug, Clone)]
pub struct Identity<'a> {
    pub sub: &'a str,
    pub email: &'a str,
    pub name: Option<&'a str>,
    pub picture: Option<&'a str>,
}

/// Column list matching `User`, as a macro so queries stay `&'static str` (sqlx 0.9
/// rejects runtime-built SQL).
macro_rules! user_columns {
    () => {
        "id, auth0_sub, email, handle, display_name, avatar_url, steam_name, role, status, created_at"
    };
}

/// Outcome of an authenticated request's sign-in check.
#[derive(Debug)]
pub enum SignIn {
    Allowed(User),
    /// No active invite for the token's email.
    NotInvited,
    Disabled,
}

/// Lets an authenticated identity in if its email is invited, creating the user on first
/// sign-in (with the invite's role) and marking the invite accepted. Email and avatar are
/// refreshed from the token every time; display name, handle and Steam name are
/// user-editable and never overwritten.
pub async fn sign_in(pool: &PgPool, identity: &Identity<'_>) -> sqlx::Result<SignIn> {
    let email = identity.email.trim().to_lowercase();

    let invite_role: Option<Role> =
        sqlx::query_scalar("SELECT role FROM invites WHERE email = $1 AND revoked_at IS NULL")
            .bind(&email)
            .fetch_optional(pool)
            .await?;
    let Some(role) = invite_role else {
        return Ok(SignIn::NotInvited);
    };

    let user = upsert(pool, identity, &email, role).await?;
    sqlx::query("UPDATE invites SET accepted_at = now() WHERE email = $1 AND accepted_at IS NULL")
        .bind(&email)
        .execute(pool)
        .await?;

    Ok(match user.status {
        UserStatus::Active => SignIn::Allowed(user),
        UserStatus::Disabled => SignIn::Disabled,
    })
}

async fn upsert(
    pool: &PgPool,
    identity: &Identity<'_>,
    email: &str,
    role: Role,
) -> sqlx::Result<User> {
    for attempt in 1..=20u32 {
        let updated = sqlx::query_as::<_, User>(concat!(
            "UPDATE users SET email = $2, avatar_url = $3, updated_at = now()
             WHERE auth0_sub = $1 RETURNING ",
            user_columns!()
        ))
        .bind(identity.sub)
        .bind(email)
        .bind(identity.picture)
        .fetch_optional(pool)
        .await?;
        if let Some(user) = updated {
            return Ok(user);
        }

        let base = handle_base(email);
        let handle = if attempt == 1 {
            base
        } else {
            format!("{base}-{attempt}")
        };
        let display_name = identity
            .name
            .map(str::trim)
            .filter(|n| !n.is_empty())
            .unwrap_or_else(|| email.split('@').next().unwrap_or(email));

        let inserted = sqlx::query_as::<_, User>(concat!(
            "INSERT INTO users (auth0_sub, email, handle, display_name, avatar_url, role)
             VALUES ($1, $2, $3, $4, $5, $6) RETURNING ",
            user_columns!()
        ))
        .bind(identity.sub)
        .bind(email)
        .bind(&handle)
        .bind(display_name)
        .bind(identity.picture)
        .bind(role)
        .fetch_one(pool)
        .await;

        match inserted {
            Ok(user) => {
                tracing::info!(user_id = %user.id, handle = %user.handle, "created user");
                return Ok(user);
            }
            // Handle taken: try the next suffix. Sub taken (a concurrent first request won):
            // loop back to the UPDATE.
            Err(sqlx::Error::Database(e)) if e.is_unique_violation() => continue,
            Err(e) => return Err(e),
        }
    }

    Err(sqlx::Error::Protocol(format!(
        "could not allocate a handle for {}",
        identity.sub
    )))
}

pub async fn list(pool: &PgPool) -> sqlx::Result<Vec<User>> {
    sqlx::query_as(concat!(
        "SELECT ",
        user_columns!(),
        " FROM users ORDER BY created_at"
    ))
    .fetch_all(pool)
    .await
}

/// Changes a user's role and/or status (admin action). `None` leaves a field as is.
pub async fn update_access(
    pool: &PgPool,
    id: Uuid,
    role: Option<Role>,
    status: Option<UserStatus>,
) -> sqlx::Result<Option<User>> {
    sqlx::query_as(concat!(
        "UPDATE users SET role = coalesce($2, role), status = coalesce($3, status),
                          updated_at = now()
         WHERE id = $1 RETURNING ",
        user_columns!()
    ))
    .bind(id)
    .bind(role)
    .bind(status)
    .fetch_optional(pool)
    .await
}

/// Fields a user can edit on their own profile. `None` leaves a field as is.
#[derive(Debug, Default)]
pub struct ProfileUpdate {
    pub display_name: Option<String>,
    pub handle: Option<String>,
    /// `Some(None)` clears it.
    pub steam_name: Option<Option<String>>,
}

#[derive(Debug, thiserror::Error)]
pub enum ProfileError {
    #[error("{0}")]
    Invalid(String),
    #[error("that handle is taken")]
    HandleTaken,
    #[error(transparent)]
    Database(#[from] sqlx::Error),
}

pub async fn update_profile(
    pool: &PgPool,
    id: Uuid,
    update: ProfileUpdate,
) -> Result<User, ProfileError> {
    let display_name = update
        .display_name
        .map(|n| n.trim().to_owned())
        .map(|n| match n.chars().count() {
            1..=50 => Ok(n),
            _ => Err(ProfileError::Invalid(
                "display name must be 1–50 characters".into(),
            )),
        })
        .transpose()?;
    let handle = update
        .handle
        .map(|h| h.trim().to_lowercase())
        .map(|h| {
            if is_valid_handle(&h) {
                Ok(h)
            } else {
                Err(ProfileError::Invalid(
                    "handle must be 2–32 characters: a–z, 0–9, - and _, starting with a letter or digit"
                        .into(),
                ))
            }
        })
        .transpose()?;
    let steam_name = update
        .steam_name
        .map(|s| s.map(|s| s.trim().to_owned()).filter(|s| !s.is_empty()))
        .map(|s| match &s {
            Some(name) if name.chars().count() > 64 => Err(ProfileError::Invalid(
                "Steam name must be at most 64 characters".into(),
            )),
            _ => Ok(s),
        })
        .transpose()?;

    let result = sqlx::query_as::<_, User>(concat!(
        "UPDATE users SET
           display_name = coalesce($2, display_name),
           handle = coalesce($3, handle),
           steam_name = CASE WHEN $4 THEN $5 ELSE steam_name END,
           updated_at = now()
         WHERE id = $1 RETURNING ",
        user_columns!()
    ))
    .bind(id)
    .bind(display_name)
    .bind(handle)
    .bind(steam_name.is_some())
    .bind(steam_name.flatten())
    .fetch_one(pool)
    .await;

    match result {
        Err(sqlx::Error::Database(e)) if e.is_unique_violation() => Err(ProfileError::HandleTaken),
        other => Ok(other?),
    }
}

/// Mirrors the `users.handle` CHECK constraint.
fn is_valid_handle(h: &str) -> bool {
    let mut chars = h.chars();
    (2..=32).contains(&h.len())
        && chars
            .next()
            .is_some_and(|c| c.is_ascii_lowercase() || c.is_ascii_digit())
        && chars.all(|c| c.is_ascii_lowercase() || c.is_ascii_digit() || c == '-' || c == '_')
}

/// Derives a handle candidate from the email's local part: lowercase, `[a-z0-9_-]`,
/// starting with a letter or digit, 2–24 chars.
fn handle_base(email: &str) -> String {
    let local = email.split('@').next().unwrap_or_default();
    let mut handle: String = local
        .chars()
        .map(|c| c.to_ascii_lowercase())
        .map(|c| if c == '.' || c == '+' { '-' } else { c })
        .filter(|c| c.is_ascii_alphanumeric() || *c == '-' || *c == '_')
        .skip_while(|c| !c.is_ascii_alphanumeric())
        .take(24)
        .collect();
    while handle.ends_with(['-', '_']) {
        handle.pop();
    }
    if handle.len() < 2 {
        handle = format!("player{handle}");
    }
    handle
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn handle_base_normalizes_email_local_part() {
        assert_eq!(handle_base("Jane.Doe+cs@example.com"), "jane-doe-cs");
        assert_eq!(handle_base("__x@gmail.com"), "playerx");
        assert_eq!(handle_base("a@gmail.com"), "playera");
        // No dash or underscore left dangling at the end.
        assert_eq!(handle_base("sam.-_@gmail.com"), "sam");
        assert_eq!(handle_base("ümlaut.ok@gmail.com"), "mlaut-ok");
        assert_eq!(
            handle_base("averyveryveryverylongemailaddress@gmail.com"),
            "averyveryveryverylongema"
        );
    }

    fn identity<'a>(sub: &'a str, email: &'a str) -> Identity<'a> {
        Identity {
            sub,
            email,
            name: Some("Friend"),
            picture: Some("https://example.com/a.png"),
        }
    }

    async fn invite(pool: &PgPool, email: &str) {
        sqlx::query("INSERT INTO invites (email) VALUES ($1)")
            .bind(email)
            .execute(pool)
            .await
            .unwrap();
    }

    async fn allowed(pool: &PgPool, identity: &Identity<'_>) -> User {
        match sign_in(pool, identity).await.unwrap() {
            SignIn::Allowed(user) => user,
            other => panic!("expected Allowed, got {other:?}"),
        }
    }

    #[sqlx::test(migrator = "crate::db::MIGRATOR")]
    async fn uninvited_emails_are_turned_away(pool: PgPool) {
        let outcome = sign_in(&pool, &identity("google-oauth2|1", "stranger@gmail.com"))
            .await
            .unwrap();
        assert!(matches!(outcome, SignIn::NotInvited));
        let users: i64 = sqlx::query_scalar("SELECT count(*) FROM users")
            .fetch_one(&pool)
            .await
            .unwrap();
        assert_eq!(users, 0, "no account for strangers");
    }

    #[sqlx::test(migrator = "crate::db::MIGRATOR")]
    async fn creates_then_updates_user(pool: PgPool) {
        invite(&pool, "friend@gmail.com").await;
        let first = allowed(&pool, &identity("google-oauth2|1", "Friend@Gmail.com")).await;
        assert_eq!(first.email, "friend@gmail.com");
        assert_eq!(first.handle, "friend");
        assert_eq!(first.display_name, "Friend");
        assert_eq!(first.role, Role::Member);
        let accepted: bool = sqlx::query_scalar(
            "SELECT accepted_at IS NOT NULL FROM invites WHERE email = 'friend@gmail.com'",
        )
        .fetch_one(&pool)
        .await
        .unwrap();
        assert!(accepted);

        let mut again = identity("google-oauth2|1", "friend@gmail.com");
        again.name = Some("Renamed In Google");
        again.picture = Some("https://example.com/b.png");
        let second = allowed(&pool, &again).await;
        assert_eq!(second.id, first.id);
        assert_eq!(second.display_name, "Friend", "display name is user-owned");
        assert_eq!(
            second.avatar_url.as_deref(),
            Some("https://example.com/b.png")
        );
    }

    #[sqlx::test(migrator = "crate::db::MIGRATOR")]
    async fn disabled_and_revoked_users_are_turned_away(pool: PgPool) {
        invite(&pool, "friend@gmail.com").await;
        let user = allowed(&pool, &identity("google-oauth2|1", "friend@gmail.com")).await;

        update_access(&pool, user.id, None, Some(UserStatus::Disabled))
            .await
            .unwrap();
        let outcome = sign_in(&pool, &identity("google-oauth2|1", "friend@gmail.com"))
            .await
            .unwrap();
        assert!(matches!(outcome, SignIn::Disabled));

        update_access(&pool, user.id, None, Some(UserStatus::Active))
            .await
            .unwrap();
        sqlx::query("UPDATE invites SET revoked_at = now()")
            .execute(&pool)
            .await
            .unwrap();
        let outcome = sign_in(&pool, &identity("google-oauth2|1", "friend@gmail.com"))
            .await
            .unwrap();
        assert!(matches!(outcome, SignIn::NotInvited));
    }

    #[sqlx::test(migrator = "crate::db::MIGRATOR")]
    async fn suffixes_colliding_handles(pool: PgPool) {
        invite(&pool, "sam@gmail.com").await;
        invite(&pool, "sam@outlook.com").await;
        allowed(&pool, &identity("google-oauth2|1", "sam@gmail.com")).await;
        let other = allowed(&pool, &identity("google-oauth2|2", "sam@outlook.com")).await;
        assert_eq!(other.handle, "sam-2");
    }

    #[sqlx::test(migrator = "crate::db::MIGRATOR")]
    async fn gives_up_when_every_handle_suffix_is_taken(pool: PgPool) {
        // sam, sam-2 … sam-20 are all someone's.
        for n in 1..=20 {
            let handle = if n == 1 {
                "sam".to_owned()
            } else {
                format!("sam-{n}")
            };
            sqlx::query(
                "INSERT INTO users (auth0_sub, email, handle, display_name)
                 VALUES ($1, $2, $3, 'Someone')",
            )
            .bind(format!("google-oauth2|other{n}"))
            .bind(format!("other{n}@gmail.com"))
            .bind(&handle)
            .execute(&pool)
            .await
            .unwrap();
        }
        invite(&pool, "sam@outlook.com").await;
        let result = sign_in(&pool, &identity("google-oauth2|sam", "sam@outlook.com")).await;
        match result {
            Err(sqlx::Error::Protocol(m)) => {
                assert_eq!(m, "could not allocate a handle for google-oauth2|sam");
            }
            other => panic!("{other:?}"),
        }
        let created: i64 = sqlx::query_scalar("SELECT count(*) FROM users WHERE email = $1")
            .bind("sam@outlook.com")
            .fetch_one(&pool)
            .await
            .unwrap();
        assert_eq!(created, 0);
    }

    /// Only a taken handle (or sub) is tried again; any other failure to create the user
    /// is an error at once.
    #[sqlx::test(migrator = "crate::db::MIGRATOR")]
    async fn other_failures_creating_a_user_arent_retried(pool: PgPool) {
        invite(&pool, "sam@gmail.com").await;
        // Postgres text can't hold a NUL, so the insert fails.
        let mut odd = identity("google-oauth2|1", "sam@gmail.com");
        odd.name = Some("Sam\0");
        match sign_in(&pool, &odd).await {
            Err(sqlx::Error::Database(e)) => assert!(!e.is_unique_violation(), "{e}"),
            other => panic!("{other:?}"),
        }
        assert!(list(&pool).await.unwrap().is_empty());
        let accepted: bool =
            sqlx::query_scalar("SELECT accepted_at IS NOT NULL FROM invites WHERE email = $1")
                .bind("sam@gmail.com")
                .fetch_one(&pool)
                .await
                .unwrap();
        assert!(!accepted, "the invite still waits");
    }

    #[sqlx::test(migrator = "crate::db::MIGRATOR")]
    async fn edits_profile(pool: PgPool) {
        invite(&pool, "sam@gmail.com").await;
        invite(&pool, "kim@gmail.com").await;
        let sam = allowed(&pool, &identity("google-oauth2|1", "sam@gmail.com")).await;
        allowed(&pool, &identity("google-oauth2|2", "kim@gmail.com")).await;

        let updated = update_profile(
            &pool,
            sam.id,
            ProfileUpdate {
                display_name: Some("  Sammy ".into()),
                handle: Some("Sam_The-Man".into()),
                steam_name: Some(Some("s4m".into())),
            },
        )
        .await
        .unwrap();
        assert_eq!(updated.display_name, "Sammy");
        assert_eq!(updated.handle, "sam_the-man");
        assert_eq!(updated.steam_name.as_deref(), Some("s4m"));

        let cleared = update_profile(
            &pool,
            sam.id,
            ProfileUpdate {
                steam_name: Some(Some("  ".into())),
                ..Default::default()
            },
        )
        .await
        .unwrap();
        assert_eq!(cleared.steam_name, None);
        assert_eq!(cleared.display_name, "Sammy", "untouched fields stay");

        let taken = update_profile(
            &pool,
            sam.id,
            ProfileUpdate {
                handle: Some("kim".into()),
                ..Default::default()
            },
        )
        .await;
        assert!(matches!(taken, Err(ProfileError::HandleTaken)));

        for bad in ["x", "-dash", "has space", "UPPER!", &"a".repeat(33)] {
            let result = update_profile(
                &pool,
                sam.id,
                ProfileUpdate {
                    handle: Some(bad.to_string()),
                    ..Default::default()
                },
            )
            .await;
            assert!(matches!(result, Err(ProfileError::Invalid(_))), "{bad:?}");
        }
        let empty_name = update_profile(
            &pool,
            sam.id,
            ProfileUpdate {
                display_name: Some("   ".into()),
                ..Default::default()
            },
        )
        .await;
        assert!(matches!(empty_name, Err(ProfileError::Invalid(_))));
        let long_steam = |len: usize| ProfileUpdate {
            steam_name: Some(Some("s".repeat(len))),
            ..Default::default()
        };
        assert!(matches!(
            update_profile(&pool, sam.id, long_steam(65)).await,
            Err(ProfileError::Invalid(m)) if m == "Steam name must be at most 64 characters"
        ));
        let at_limit = update_profile(&pool, sam.id, long_steam(64)).await.unwrap();
        assert_eq!(at_limit.steam_name.map(|s| s.len()), Some(64));
    }
}
