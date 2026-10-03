use axum::{
    Json,
    http::StatusCode,
    response::{IntoResponse, Response},
};
use clipos_core::shows::Ties;
use serde::Serialize;
use utoipa::ToSchema;

/// Error payload for every non-2xx `/api` response.
#[derive(Debug, Serialize, ToSchema)]
pub struct ErrorBody {
    /// Stable machine-readable code: `bad_request`, `unauthorized`, `not_invited`,
    /// `account_disabled`, `forbidden`, `not_found`, `conflict`, `unsupported_media_type`,
    /// `too_large`, `bad_gateway` or `internal`.
    pub error: &'static str,
    /// Human-readable detail. Never contains internals for 5xx responses.
    pub message: String,
    /// Only on the 409 from ending a show with a tied vote: the tied clips of every tied
    /// category, so the host breaks them all at once.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub tied: Option<Ties>,
}

#[derive(Debug, thiserror::Error)]
pub enum ApiError {
    #[error("{0}")]
    BadRequest(String),
    /// A body, path or query axum couldn't read (`extract::Json`, `Path`, `Query`), with
    /// its status and explanation.
    #[error("{1}")]
    Rejected(StatusCode, String),
    #[error("{0}")]
    Unauthorized(String),
    /// Signed in with Google, but the email has no active invite.
    #[error("not invited")]
    NotInvited,
    #[error("account disabled")]
    Disabled,
    #[error("{0}")]
    Conflict(String),
    #[error("{0}")]
    Forbidden(String),
    #[error("not found")]
    NotFound,
    /// Blob Storage answered a read we pass on with an error (logged where it happened).
    #[error("the file store didn't answer")]
    BadGateway,
    /// A 409 that names the tied clips (`ErrorBody::tied`).
    #[error("{message}")]
    Tied { message: String, ties: Ties },
    #[error(transparent)]
    Internal(#[from] anyhow::Error),
}

impl From<sqlx::Error> for ApiError {
    fn from(e: sqlx::Error) -> Self {
        Self::Internal(e.into())
    }
}

impl IntoResponse for ApiError {
    fn into_response(self) -> Response {
        let mut tied = None;
        let (status, error, message) = match self {
            Self::BadRequest(m) => (StatusCode::BAD_REQUEST, "bad_request", m),
            Self::Rejected(status, m) => {
                let error = match status {
                    StatusCode::UNSUPPORTED_MEDIA_TYPE => "unsupported_media_type",
                    StatusCode::PAYLOAD_TOO_LARGE => "too_large",
                    _ => "bad_request",
                };
                (status, error, m)
            }
            Self::Unauthorized(m) => (StatusCode::UNAUTHORIZED, "unauthorized", m),
            Self::NotInvited => (
                StatusCode::FORBIDDEN,
                "not_invited",
                "this Google account isn't invited to clipos".into(),
            ),
            Self::Disabled => (
                StatusCode::FORBIDDEN,
                "account_disabled",
                "this account has been disabled".into(),
            ),
            Self::Conflict(m) => (StatusCode::CONFLICT, "conflict", m),
            Self::Forbidden(m) => (StatusCode::FORBIDDEN, "forbidden", m),
            Self::NotFound => (StatusCode::NOT_FOUND, "not_found", "not found".into()),
            Self::BadGateway => (
                StatusCode::BAD_GATEWAY,
                "bad_gateway",
                "the file store didn't answer; try again".into(),
            ),
            Self::Tied { message, ties } => {
                tied = Some(ties);
                (StatusCode::CONFLICT, "conflict", message)
            }
            Self::Internal(e) => {
                tracing::error!(error = ?e, "request failed");
                (
                    StatusCode::INTERNAL_SERVER_ERROR,
                    "internal",
                    "something went wrong".into(),
                )
            }
        };
        (
            status,
            Json(ErrorBody {
                error,
                message,
                tied,
            }),
        )
            .into_response()
    }
}

#[cfg(test)]
mod tests {
    use http_body_util::BodyExt;
    use serde_json::{Value, json};
    use uuid::Uuid;

    use super::*;

    async fn render(e: ApiError) -> (StatusCode, Value) {
        let res = e.into_response();
        let status = res.status();
        assert_eq!(res.headers()["content-type"], "application/json");
        let body = res.into_body().collect().await.unwrap().to_bytes();
        (status, serde_json::from_slice(&body).unwrap())
    }

    #[tokio::test]
    async fn every_error_has_a_status_and_a_stable_code() {
        let cases = [
            (
                ApiError::BadRequest("title too long".into()),
                400,
                "bad_request",
                "title too long",
            ),
            (
                ApiError::Rejected(StatusCode::UNSUPPORTED_MEDIA_TYPE, "want JSON".into()),
                415,
                "unsupported_media_type",
                "want JSON",
            ),
            (
                ApiError::Rejected(StatusCode::PAYLOAD_TOO_LARGE, "too big".into()),
                413,
                "too_large",
                "too big",
            ),
            (
                ApiError::Rejected(StatusCode::UNPROCESSABLE_ENTITY, "missing field".into()),
                422,
                "bad_request",
                "missing field",
            ),
            (
                ApiError::Unauthorized("token expired".into()),
                401,
                "unauthorized",
                "token expired",
            ),
            (
                ApiError::NotInvited,
                403,
                "not_invited",
                "this Google account isn't invited to clipos",
            ),
            (
                ApiError::Disabled,
                403,
                "account_disabled",
                "this account has been disabled",
            ),
            (
                ApiError::Conflict("already shared".into()),
                409,
                "conflict",
                "already shared",
            ),
            (
                ApiError::Forbidden("admins only".into()),
                403,
                "forbidden",
                "admins only",
            ),
            (ApiError::NotFound, 404, "not_found", "not found"),
            (
                ApiError::BadGateway,
                502,
                "bad_gateway",
                "the file store didn't answer; try again",
            ),
        ];
        for (e, status, code, message) in cases {
            let (s, body) = render(e).await;
            assert_eq!(s.as_u16(), status, "{body}");
            assert_eq!(body, json!({ "error": code, "message": message }));
        }
    }

    #[tokio::test]
    async fn a_tie_names_the_tied_clips() {
        let (a, b, c) = (Uuid::new_v4(), Uuid::new_v4(), Uuid::new_v4());
        let (status, body) = render(ApiError::Tied {
            message: "break the ties".into(),
            ties: Ties {
                clip: vec![a, b],
                fail: vec![c],
            },
        })
        .await;
        assert_eq!(status, StatusCode::CONFLICT);
        assert_eq!(
            body,
            json!({
                "error": "conflict",
                "message": "break the ties",
                "tied": { "clip": [a, b], "fail": [c] },
            })
        );
    }

    #[tokio::test]
    async fn internal_errors_hide_their_details() {
        let e = anyhow::anyhow!("password authentication failed for user clipos");
        let (status, body) = render(ApiError::from(e)).await;
        assert_eq!(status, StatusCode::INTERNAL_SERVER_ERROR);
        assert_eq!(
            body,
            json!({ "error": "internal", "message": "something went wrong" })
        );

        let e = ApiError::from(sqlx::Error::RowNotFound);
        assert!(matches!(e, ApiError::Internal(_)));
        let (status, body) = render(e).await;
        assert_eq!(status, StatusCode::INTERNAL_SERVER_ERROR);
        assert_eq!(body["message"], "something went wrong");
    }

    #[test]
    fn errors_display_their_message() {
        assert_eq!(ApiError::NotInvited.to_string(), "not invited");
        assert_eq!(ApiError::Disabled.to_string(), "account disabled");
        assert_eq!(
            ApiError::Rejected(StatusCode::BAD_REQUEST, "bad json".into()).to_string(),
            "bad json"
        );
    }
}
