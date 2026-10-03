pub mod admin;
pub mod clips;
pub mod config;
pub mod health;
pub mod internal;
pub mod me;
pub mod members;
pub mod share;
pub mod shows;

use crate::error::ApiError;

/// Unknown `/api/*` paths get a JSON 404 instead of falling through to the SPA.
pub async fn not_found() -> ApiError {
    ApiError::NotFound
}
