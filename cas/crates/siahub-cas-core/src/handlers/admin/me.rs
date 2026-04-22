//! `GET /admin/me` — return the authenticated user's profile.
//!
//! Backs AUTH-01..03 (console header user menu). Response shape is locked by
//! 04-KEY-DECISIONS §2 row 1:
//!
//! ```json
//! {"user": {"id": 123, "login": "octocat", "avatar_url": "...",
//!           "email": "octocat@example.com" | null, "is_admin": false}}
//! ```
//!
//! `email` is `Option<String>` (P13) — GitHub users with `noreply` email
//! configuration surface as `null`, and the console renders `login` in that
//! case.

use axum::Json;
use serde::Serialize;

use crate::auth::AuthStateRef;
use crate::errors::AppError;
use crate::session::{Session, SessionUser};

#[derive(Debug, Clone, Serialize)]
pub struct MeResponse {
    pub user: SessionUser,
}

/// `GET /admin/me` handler. The `Session` extractor does all the work —
/// missing / expired / revoked cookie already returns 401 before we reach
/// the body.
pub async fn get_me<S: AuthStateRef>(Session(user): Session) -> Result<Json<MeResponse>, AppError> {
    Ok(Json(MeResponse { user }))
}
