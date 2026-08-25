//! `POST /auth/login` + `GET /auth/methods` — password sign-in for
//! self-hosted, single-operator setups.
//!
//! This is an alternative front door to the *same* `openweights_session`
//! cookie the GitHub OAuth callback mints (see `github.rs`), so an operator
//! can run OpenWeights without registering a GitHub OAuth app. GitHub OAuth
//! stays available in parallel; `GET /auth/methods` tells the console which
//! sign-in controls to render (password box, GitHub button, or both).
//!
//! The admin password lives only in the operator's environment
//! (`OPENWEIGHTS_ADMIN_PASSWORD`), never in the DB or logs — the same
//! handling as the Sia recovery phrase. A guess is checked by comparing the
//! sha256 of the submitted value against the sha256 of the configured value
//! in constant time, so a wrong guess leaks neither length nor content
//! through timing.
//!
//! The password admin is a single synthetic `users` row at the sentinel id
//! `-1`. Real GitHub numeric ids are always positive, so it can never
//! collide with an OAuth user.
//!
//! Brute-force note: this endpoint is not yet rate-limited. v1 relies on a
//! strong operator-chosen password; a redis-backed lockout is a follow-up.

use axum::Json;
use axum::extract::State;
use axum::http::{HeaderMap, HeaderValue, StatusCode, header};
use axum::response::{IntoResponse, Response};
use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};
use sqlx::PgPool;
use subtle::ConstantTimeEq;

use crate::errors::AppError;
use crate::session::{mint_session, session_cookie_header};

/// Sentinel user id for the local password admin. Negative so it can never
/// collide with a GitHub numeric id (always positive).
pub const ADMIN_USER_ID: i64 = -1;

/// State the password-login + auth-methods handlers need.
pub trait PasswordAuthState: Clone + Send + Sync + 'static {
    fn pool(&self) -> &PgPool;
    /// The configured admin password. Empty ⇒ password auth disabled.
    fn admin_password(&self) -> &str;
    /// Display login for the synthetic admin user (default `admin`).
    fn admin_username(&self) -> &str;
    /// Whether GitHub OAuth is also configured (both id + secret present).
    fn github_oauth_configured(&self) -> bool;
}

#[derive(Debug, Deserialize)]
pub struct LoginRequest {
    pub password: String,
}

/// `GET /auth/methods` — public. Lets the console render the right sign-in
/// controls. No secrets leave the process — only two booleans.
#[derive(Debug, Serialize)]
pub struct AuthMethods {
    pub password: bool,
    pub github: bool,
}

pub async fn auth_methods<S: PasswordAuthState>(State(st): State<S>) -> Json<AuthMethods> {
    Json(AuthMethods {
        password: !st.admin_password().is_empty(),
        github: st.github_oauth_configured(),
    })
}

pub async fn login<S: PasswordAuthState>(
    State(st): State<S>,
    Json(body): Json<LoginRequest>,
) -> Result<Response, AppError> {
    let configured = st.admin_password();
    // Password auth is off unless the operator set a password. Treat a
    // login attempt against an unconfigured instance as a plain 401.
    if configured.is_empty() {
        return Err(AppError::Unauthenticated);
    }

    // Constant-time compare of the sha256 digests — a wrong guess leaks
    // neither length nor content via timing.
    let submitted: [u8; 32] = Sha256::digest(body.password.as_bytes()).into();
    let expected: [u8; 32] = Sha256::digest(configured.as_bytes()).into();
    if submitted.ct_eq(&expected).unwrap_u8() != 1 {
        return Err(AppError::Unauthenticated);
    }

    // Ensure the synthetic admin row exists, then mint the same session the
    // OAuth callback would.
    ensure_admin_user(st.pool(), st.admin_username()).await?;
    let minted = mint_session(st.pool(), ADMIN_USER_ID).await?;
    let set_cookie = session_cookie_header(minted.session_id);

    let mut headers = HeaderMap::new();
    headers.insert(
        header::SET_COOKIE,
        HeaderValue::from_str(&set_cookie).map_err(|e| AppError::Other(e.into()))?,
    );
    Ok((StatusCode::NO_CONTENT, headers).into_response())
}

/// Upsert the single local admin user at the sentinel id. Idempotent — every
/// successful login re-asserts the row so a renamed `OPENWEIGHTS_ADMIN_USERNAME`
/// or an accidentally-revoked admin heals on next sign-in.
async fn ensure_admin_user(pool: &PgPool, username: &str) -> Result<(), sqlx::Error> {
    sqlx::query(
        "INSERT INTO users (id, github_login, is_admin) \
         VALUES ($1, $2, true) \
         ON CONFLICT (id) DO UPDATE \
           SET github_login = EXCLUDED.github_login, is_admin = true, revoked_at = NULL",
    )
    .bind(ADMIN_USER_ID)
    .bind(username)
    .execute(pool)
    .await?;
    Ok(())
}

#[cfg(test)]
mod tests {
    use sha2::{Digest, Sha256};
    use subtle::ConstantTimeEq;

    // Mirror the handler's comparison so the constant-time path is covered
    // without standing up a DB.
    fn matches(submitted: &str, configured: &str) -> bool {
        let a: [u8; 32] = Sha256::digest(submitted.as_bytes()).into();
        let b: [u8; 32] = Sha256::digest(configured.as_bytes()).into();
        a.ct_eq(&b).unwrap_u8() == 1
    }

    #[test]
    fn correct_password_matches() {
        assert!(matches("hunter2-correct-horse", "hunter2-correct-horse"));
    }

    #[test]
    fn wrong_password_rejected() {
        assert!(!matches("hunter2", "hunter2-correct-horse"));
        assert!(!matches("", "hunter2-correct-horse"));
        assert!(!matches("HUNTER2-CORRECT-HORSE", "hunter2-correct-horse"));
    }
}
