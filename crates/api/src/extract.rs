use axum::{
    extract::{FromRequest, FromRequestParts, OptionalFromRequest, Request},
    http::request::Parts,
    response::{IntoResponse, Response},
};
use clipos_core::{
    auth::Claims,
    users::{self, Identity, Role, SignIn, User},
};
use serde::{Serialize, de::DeserializeOwned};

use crate::{AppState, error::ApiError};

/// The invited, active user behind an access token, and when the token expires (seconds
/// since the epoch). Used by the show's live connection, whose token arrives in its first
/// message instead of a header, and which closes when the token expires.
pub(crate) async fn authenticate(state: &AppState, token: &str) -> Result<(User, u64), ApiError> {
    let claims = verify(state, token).await?;
    let user = user_for(state, &claims).await?;
    Ok((user, claims.exp))
}

async fn verify(state: &AppState, token: &str) -> Result<Claims, ApiError> {
    state.verifier.verify(token).await.map_err(|e| {
        tracing::debug!(error = %e, "rejected token");
        ApiError::Unauthorized("invalid or expired token".into())
    })
}

async fn user_for(state: &AppState, claims: &Claims) -> Result<User, ApiError> {
    let email = claims.email.as_deref().ok_or_else(|| {
        ApiError::Forbidden(
            "token has no email claim; is the clipos post-login Action deployed?".into(),
        )
    })?;
    let identity = Identity {
        sub: &claims.sub,
        email,
        name: claims.name.as_deref(),
        picture: claims.picture.as_deref(),
    };
    match users::sign_in(&state.pool, &identity).await? {
        SignIn::Allowed(user) => Ok(user),
        SignIn::NotInvited => Err(ApiError::NotInvited),
        SignIn::Disabled => Err(ApiError::Disabled),
    }
}

/// Verified Auth0 access-token claims from `Authorization: Bearer …`.
pub struct Authenticated(pub Claims);

impl FromRequestParts<AppState> for Authenticated {
    type Rejection = ApiError;

    async fn from_request_parts(parts: &mut Parts, state: &AppState) -> Result<Self, ApiError> {
        let token = parts
            .headers
            .get(axum::http::header::AUTHORIZATION)
            .and_then(|v| v.to_str().ok())
            .and_then(|v| v.strip_prefix("Bearer "))
            .ok_or_else(|| ApiError::Unauthorized("missing bearer token".into()))?;

        verify(state, token).await.map(Self)
    }
}

/// The signed-in, invited, active user. The API enforces the allowlist on every request,
/// independently of the Auth0 Action. Creates the user on first sign-in.
pub struct CurrentUser(pub User);

impl FromRequestParts<AppState> for CurrentUser {
    type Rejection = ApiError;

    async fn from_request_parts(parts: &mut Parts, state: &AppState) -> Result<Self, ApiError> {
        let Authenticated(claims) = Authenticated::from_request_parts(parts, state).await?;
        user_for(state, &claims).await.map(Self)
    }
}

/// A `CurrentUser` with the admin role.
pub struct Admin(pub User);

impl FromRequestParts<AppState> for Admin {
    type Rejection = ApiError;

    async fn from_request_parts(parts: &mut Parts, state: &AppState) -> Result<Self, ApiError> {
        let CurrentUser(user) = CurrentUser::from_request_parts(parts, state).await?;
        if user.role != Role::Admin {
            return Err(ApiError::Forbidden("admins only".into()));
        }
        Ok(Self(user))
    }
}

// axum's own `Json`, `Path` and `Query` answer a request they can't read with plain text.
// These wrap them so the answer is an `ErrorBody` like every other `/api` error, with
// axum's status (400, 413, 415 or 422) and explanation.

/// `axum::Json`, as a request body and as a response.
pub struct Json<T>(pub T);

impl<T: DeserializeOwned, S: Send + Sync> FromRequest<S> for Json<T> {
    type Rejection = ApiError;

    async fn from_request(req: Request, state: &S) -> Result<Self, ApiError> {
        match <axum::Json<T> as FromRequest<S>>::from_request(req, state).await {
            Ok(axum::Json(value)) => Ok(Self(value)),
            Err(e) => Err(ApiError::Rejected(e.status(), e.body_text())),
        }
    }
}

/// An optional body: none without a `Content-Type`.
impl<T: DeserializeOwned, S: Send + Sync> OptionalFromRequest<S> for Json<T> {
    type Rejection = ApiError;

    async fn from_request(req: Request, state: &S) -> Result<Option<Self>, ApiError> {
        match <axum::Json<T> as OptionalFromRequest<S>>::from_request(req, state).await {
            Ok(value) => Ok(value.map(|axum::Json(value)| Self(value))),
            Err(e) => Err(ApiError::Rejected(e.status(), e.body_text())),
        }
    }
}

impl<T: Serialize> IntoResponse for Json<T> {
    fn into_response(self) -> Response {
        axum::Json(self.0).into_response()
    }
}

/// `axum::extract::Path`.
pub struct Path<T>(pub T);

impl<T: DeserializeOwned + Send, S: Send + Sync> FromRequestParts<S> for Path<T> {
    type Rejection = ApiError;

    async fn from_request_parts(parts: &mut Parts, state: &S) -> Result<Self, ApiError> {
        match axum::extract::Path::<T>::from_request_parts(parts, state).await {
            Ok(axum::extract::Path(value)) => Ok(Self(value)),
            Err(e) => Err(ApiError::Rejected(e.status(), e.body_text())),
        }
    }
}

/// `axum::extract::Query`.
pub struct Query<T>(pub T);

impl<T: DeserializeOwned, S: Send + Sync> FromRequestParts<S> for Query<T> {
    type Rejection = ApiError;

    async fn from_request_parts(parts: &mut Parts, state: &S) -> Result<Self, ApiError> {
        match axum::extract::Query::<T>::from_request_parts(parts, state).await {
            Ok(axum::extract::Query(value)) => Ok(Self(value)),
            Err(e) => Err(ApiError::Rejected(e.status(), e.body_text())),
        }
    }
}
