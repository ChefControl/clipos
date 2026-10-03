//! The invite allowlist. An email may sign in while it has an active (not revoked) invite
//! and its user, if one exists, isn't disabled.

use chrono::{DateTime, Utc};
use serde::Serialize;
use sqlx::PgPool;
use utoipa::ToSchema;
use uuid::Uuid;

use crate::users::Role;

#[derive(Debug, Clone, Serialize, sqlx::FromRow, ToSchema)]
#[serde(rename_all = "camelCase")]
pub struct Invite {
    pub email: String,
    /// Role the user gets on first sign-in.
    pub role: Role,
    /// Handle of the admin who sent it; empty for invites seeded from `ADMIN_EMAILS`.
    pub invited_by: Option<String>,
    pub created_at: DateTime<Utc>,
    /// First sign-in.
    pub accepted_at: Option<DateTime<Utc>>,
    pub revoked_at: Option<DateTime<Utc>>,
}

/// Column list matching `Invite` for queries over `invites i LEFT JOIN users u`.
macro_rules! invite_columns {
    () => {
        "i.email, i.role, u.handle AS invited_by, i.created_at, i.accepted_at, i.revoked_at"
    };
}

/// Lower-cases and sanity-checks an email address. Not a full RFC 5322 parser: Google
/// already verified the address, this only keeps typos out of the allowlist.
pub fn normalize_email(raw: &str) -> Option<String> {
    let email = raw.trim().to_lowercase();
    let (local, domain) = email.split_once('@')?;
    let valid = !local.is_empty()
        && domain.contains('.')
        && !domain.starts_with('.')
        && !domain.ends_with('.')
        && !domain.contains('@')
        && email.len() <= 254
        && !email.chars().any(|c| c.is_whitespace() || c.is_control());
    valid.then_some(email)
}

pub async fn list(pool: &PgPool) -> sqlx::Result<Vec<Invite>> {
    sqlx::query_as(concat!(
        "SELECT ",
        invite_columns!(),
        " FROM invites i LEFT JOIN users u ON u.id = i.invited_by
         ORDER BY i.revoked_at IS NOT NULL, i.created_at DESC"
    ))
    .fetch_all(pool)
    .await
}

/// Invites `email` (already normalized), or re-activates a revoked invite.
pub async fn create(
    pool: &PgPool,
    email: &str,
    role: Role,
    invited_by: Uuid,
) -> sqlx::Result<Invite> {
    sqlx::query_as(concat!(
        "WITH i AS (
           INSERT INTO invites (email, role, invited_by) VALUES ($1, $2, $3)
           ON CONFLICT (email) DO UPDATE SET
             role = EXCLUDED.role,
             invited_by = EXCLUDED.invited_by,
             created_at = CASE WHEN invites.revoked_at IS NULL THEN invites.created_at ELSE now() END,
             revoked_at = NULL
           RETURNING *
         )
         SELECT ",
        invite_columns!(),
        " FROM i LEFT JOIN users u ON u.id = i.invited_by"
    ))
    .bind(email)
    .bind(role)
    .bind(invited_by)
    .fetch_one(pool)
    .await
}

/// Revokes an active invite. The user (and their clips) stay, but they can't sign in.
pub async fn revoke(pool: &PgPool, email: &str) -> sqlx::Result<Option<Invite>> {
    sqlx::query_as(concat!(
        "WITH i AS (
           UPDATE invites SET revoked_at = now()
           WHERE email = $1 AND revoked_at IS NULL
           RETURNING *
         )
         SELECT ",
        invite_columns!(),
        " FROM i LEFT JOIN users u ON u.id = i.invited_by"
    ))
    .bind(email)
    .fetch_optional(pool)
    .await
}

/// Makes every email in `emails` an active admin invite, and promotes existing users with
/// those emails to admin. Run at api startup from `ADMIN_EMAILS`, so the configured admins
/// can never lock themselves out.
pub async fn seed_admins(pool: &PgPool, emails: &[String]) -> sqlx::Result<()> {
    if emails.is_empty() {
        return Ok(());
    }
    let mut tx = pool.begin().await?;
    sqlx::query(
        "INSERT INTO invites (email, role) SELECT unnest($1::text[]), 'admin'
         ON CONFLICT (email) DO UPDATE SET role = 'admin', revoked_at = NULL",
    )
    .bind(emails)
    .execute(&mut *tx)
    .await?;
    sqlx::query(
        "UPDATE users SET role = 'admin', status = 'active', updated_at = now()
         WHERE email = ANY($1) AND (role <> 'admin' OR status <> 'active')",
    )
    .bind(emails)
    .execute(&mut *tx)
    .await?;
    tx.commit().await
}

/// Whether `email` (already normalized) may sign in: an active invite, and no disabled
/// user with that email. Used by the Auth0 Action through `/internal/invites/check`.
pub async fn may_sign_in(pool: &PgPool, email: &str) -> sqlx::Result<bool> {
    sqlx::query_scalar(
        "SELECT EXISTS (SELECT FROM invites WHERE email = $1 AND revoked_at IS NULL)
            AND NOT EXISTS (SELECT FROM users WHERE email = $1 AND status = 'disabled')",
    )
    .bind(email)
    .fetch_one(pool)
    .await
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::users::{self, Identity, SignIn};

    #[test]
    fn normalizes_emails() {
        assert_eq!(
            normalize_email("  Friend@GMail.com ").as_deref(),
            Some("friend@gmail.com")
        );
        for bad in [
            "",
            "friend",
            "@gmail.com",
            "friend@gmail",
            "a@.com",
            "a b@x.com",
            "a@b@c.com",
        ] {
            assert_eq!(normalize_email(bad), None, "{bad:?}");
        }
    }

    async fn admin(pool: &PgPool) -> users::User {
        seed_admins(pool, &["admin@gmail.com".into()])
            .await
            .unwrap();
        let identity = Identity {
            sub: "google-oauth2|admin",
            email: "admin@gmail.com",
            name: Some("Admin"),
            picture: None,
        };
        match users::sign_in(pool, &identity).await.unwrap() {
            SignIn::Allowed(user) => user,
            other => panic!("admin not allowed: {other:?}"),
        }
    }

    #[sqlx::test(migrator = "crate::db::MIGRATOR")]
    async fn invite_revoke_and_reinvite(pool: PgPool) {
        let admin = admin(&pool).await;
        assert!(!may_sign_in(&pool, "friend@gmail.com").await.unwrap());

        let invite = create(&pool, "friend@gmail.com", Role::Member, admin.id)
            .await
            .unwrap();
        assert_eq!(invite.invited_by.as_deref(), Some(admin.handle.as_str()));
        assert!(may_sign_in(&pool, "friend@gmail.com").await.unwrap());

        let revoked = revoke(&pool, "friend@gmail.com").await.unwrap().unwrap();
        assert!(revoked.revoked_at.is_some());
        assert!(!may_sign_in(&pool, "friend@gmail.com").await.unwrap());
        assert!(
            revoke(&pool, "friend@gmail.com").await.unwrap().is_none(),
            "already revoked"
        );

        let again = create(&pool, "friend@gmail.com", Role::Admin, admin.id)
            .await
            .unwrap();
        assert_eq!(again.revoked_at, None);
        assert_eq!(again.role, Role::Admin);
        assert!(may_sign_in(&pool, "friend@gmail.com").await.unwrap());

        let all = list(&pool).await.unwrap();
        assert_eq!(all.len(), 2);
    }

    #[sqlx::test(migrator = "crate::db::MIGRATOR")]
    async fn seeded_admins_are_reactivated(pool: PgPool) {
        let admin = admin(&pool).await;
        revoke(&pool, "admin@gmail.com").await.unwrap();
        users::update_access(&pool, admin.id, None, Some(users::UserStatus::Disabled))
            .await
            .unwrap();
        assert!(!may_sign_in(&pool, "admin@gmail.com").await.unwrap());

        seed_admins(&pool, &["admin@gmail.com".into()])
            .await
            .unwrap();
        assert!(may_sign_in(&pool, "admin@gmail.com").await.unwrap());
    }

    #[sqlx::test(migrator = "crate::db::MIGRATOR")]
    async fn no_admins_to_seed_changes_nothing(pool: PgPool) {
        seed_admins(&pool, &[]).await.unwrap();
        assert!(list(&pool).await.unwrap().is_empty());
    }
}
