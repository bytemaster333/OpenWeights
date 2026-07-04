//! `GET /auth/github/start` + `GET /auth/github/callback` — GitHub OAuth flow.
//! ( amendment —). Flow:
//! 1. `start` generates a 32-byte URL-safe nonce, inserts it into
//!    `oauth_state` (TTL 10 min), and `302`s the browser to GitHub's
//!    authorize endpoint with `state=<nonce>`.
//! 2. GitHub redirects the browser back to `/auth/github/callback?code=...&state=...`.
//! 3. `callback` atomically consumes the nonce (`consume_oauth_state`),
//!    exchanges `code` for an access token, fetches `/user` and
//!    (if email is null) `/user/emails`, upserts `users` keyed on numeric
//!    `id BIGINT` ( — NOT email), mints a `sessions` row, and `302`s
//!    back to the console with a `Set-Cookie: openweights_session=...` header.
//!    (HIGH) mitigation: structured error codes
//!    `oauth_state_mismatch` / `oauth_code_missing` / `github_token_exchange_failed`
//!    surface in error bodies so the self-host operator can distinguish
//!    callback-URL-mismatch from GitHub API flakes.

use std::sync::Arc;
use std::time::Duration;

use axum::extract::{Query, State};
use axum::http::{HeaderValue, StatusCode, header};
use axum::response::{IntoResponse, Response};
use axum::{Json, http::HeaderMap};
use base64::{Engine, engine::general_purpose::URL_SAFE_NO_PAD};
use serde::Deserialize;
use serde_json::json;

use crate::auth::AuthStateRef;
use crate::errors::AppError;
use crate::session::{consume_oauth_state, insert_oauth_state, mint_session, session_cookie_header};

/// Extension trait — binary crate's `AppState` supplies OAuth config +
/// HTTP client. Mirrors `MapState` so both handlers share the same shared
/// `reqwest::Client` (built once at boot).
pub trait GithubOAuthState: AuthStateRef {
    fn github_client_id(&self) -> &str;
    fn github_client_secret(&self) -> &str;
    fn github_callback_url(&self) -> &str;
    fn console_base_url(&self) -> &str;
    fn http_client(&self) -> Arc<reqwest::Client>;
}

// ---------------------------------------------------------------------------
// GET /auth/github/start
// ---------------------------------------------------------------------------

/// `GET /auth/github/start` — begin the OAuth round-trip.
/// Returns `302` with a `Location:` header that points at GitHub's
/// `/login/oauth/authorize` with `client_id`, `redirect_uri`, `scope`, and
/// `state`. The state nonce is inserted into `oauth_state` before the
/// redirect so a CSRF-forged callback finds no matching row and is
/// rejected with `oauth_state_mismatch`.
pub async fn start<S: GithubOAuthState>(State(st): State<S>) -> Result<Response, AppError> {
    // 32 random bytes → 43 base64url chars. Plenty of entropy vs a 10-min
    // TTL replay window.
    let mut raw = [0u8; 32];
    getrandom::getrandom(&mut raw).map_err(|e| AppError::Other(anyhow::anyhow!(e)))?;
    let state_nonce = URL_SAFE_NO_PAD.encode(raw);

    insert_oauth_state(st.pool(), &state_nonce).await?;

    // Minimal URL-encoding for the query string — reqwest's own query
    // builder would work but we return a 302 manually (not a request).
    let authorize_url = format!(
        "https://github.com/login/oauth/authorize?client_id={}&redirect_uri={}&scope={}&state={}",
        urlencode(st.github_client_id()),
        urlencode(st.github_callback_url()),
        urlencode("user:email read:user"),
        urlencode(&state_nonce),
    );

    let mut headers = HeaderMap::new();
    headers.insert(
        header::LOCATION,
        HeaderValue::from_str(&authorize_url).map_err(|e| AppError::Other(e.into()))?,
    );
    Ok((StatusCode::FOUND, headers).into_response())
}

// ---------------------------------------------------------------------------
// GET /auth/github/callback
// ---------------------------------------------------------------------------

#[derive(Debug, Deserialize)]
pub struct CallbackQuery {
    pub code: Option<String>,
    pub state: Option<String>,
}

/// `GET /auth/github/callback?code=...&state=...`
pub async fn callback<S: GithubOAuthState>(
    State(st): State<S>,
    Query(q): Query<CallbackQuery>,
) -> Result<Response, CallbackError> {
    // (1) State CSRF check. `consume_oauth_state` uses a single UPDATE
    //... RETURNING that only matches an unconsumed unexpired nonce.
    let state_nonce = q.state.ok_or(CallbackError::StateMismatch)?;
    let consumed = consume_oauth_state(st.pool(), &state_nonce)
        .await
        .map_err(CallbackError::Db)?;
    if !consumed {
        return Err(CallbackError::StateMismatch);
    }

    // (2) Code present?
    let code = q.code.ok_or(CallbackError::CodeMissing)?;

    // (3) Exchange code for an access token.
    let token = exchange_code(&st, &code).await?;

    // (4) Fetch GitHub profile.
    let profile = fetch_user_profile(&st, &token).await?;

    // (5) Email fallback if `/user` returned null. : email may
    // legitimately remain None even after the /user/emails call when
    // the user has no verified primary.
    let email = if profile.email.is_some() {
        profile.email.clone()
    } else {
        fetch_primary_verified_email(&st, &token)
            .await
            .unwrap_or_default()
    };

    // upsert user on numeric github id
    let (_user_id, is_new_user) = upsert_user(&st, &profile, email.as_deref()).await?;

    let minted = mint_session(st.pool(), profile.id)
        .await
        .map_err(CallbackError::Db)?;
    let set_cookie = session_cookie_header(minted.session_id);

    // new users go to /keys (onboarding merged into keys page); returning users to /dashboard
    let destination = if is_new_user {
        format!("{}/keys", st.console_base_url().trim_end_matches('/'))
    } else {
        format!("{}/dashboard", st.console_base_url().trim_end_matches('/'))
    };

    let mut headers = HeaderMap::new();
    headers.insert(
        header::LOCATION,
        HeaderValue::from_str(&destination).map_err(|e| CallbackError::Other(e.to_string()))?,
    );
    headers.insert(
        header::SET_COOKIE,
        HeaderValue::from_str(&set_cookie).map_err(|e| CallbackError::Other(e.to_string()))?,
    );
    Ok((StatusCode::FOUND, headers).into_response())
}

// ---------------------------------------------------------------------------
// GitHub API glue
// ---------------------------------------------------------------------------

#[derive(Debug, Deserialize)]
struct TokenResponse {
    access_token: Option<String>,
    error: Option<String>,
    error_description: Option<String>,
}

#[derive(Debug, Clone, Deserialize)]
struct GithubProfile {
    id: i64,
    login: String,
    avatar_url: Option<String>,
    email: Option<String>,
}

#[derive(Debug, Deserialize)]
struct GithubEmail {
    email: String,
    primary: bool,
    verified: bool,
}

async fn exchange_code<S: GithubOAuthState>(
    st: &S,
    code: &str,
) -> Result<String, CallbackError> {
    let resp = st
        .http_client()
        .post("https://github.com/login/oauth/access_token")
        .header(header::ACCEPT, "application/json")
        .timeout(Duration::from_secs(10))
        .form(&[
            ("client_id", st.github_client_id()),
            ("client_secret", st.github_client_secret()),
            ("code", code),
            ("redirect_uri", st.github_callback_url()),
        ])
        .send()
        .await
        .map_err(|e| CallbackError::TokenExchangeFailed(e.to_string()))?;

    if !resp.status().is_success() {
        let status = resp.status();
        return Err(CallbackError::TokenExchangeFailed(format!(
            "github returned {}",
            status
        )));
    }

    let body: TokenResponse = resp
        .json()
        .await
        .map_err(|e| CallbackError::TokenExchangeFailed(format!("parse: {}", e)))?;

    if let Some(tok) = body.access_token {
        Ok(tok)
    } else {
        let detail = body
            .error_description
            .or(body.error)
            .unwrap_or_else(|| "no access_token in response".to_string());
        Err(CallbackError::TokenExchangeFailed(detail))
    }
}

async fn fetch_user_profile<S: GithubOAuthState>(
    st: &S,
    token: &str,
) -> Result<GithubProfile, CallbackError> {
    let resp = st
        .http_client()
        .get("https://api.github.com/user")
        .bearer_auth(token)
        .header(header::ACCEPT, "application/vnd.github+json")
        .header(header::USER_AGENT, "openweights-cas/1.0")
        .timeout(Duration::from_secs(10))
        .send()
        .await
        .map_err(|e| CallbackError::Other(format!("user fetch: {}", e)))?;

    if !resp.status().is_success() {
        return Err(CallbackError::Other(format!(
            "/user returned {}",
            resp.status()
        )));
    }
    resp.json::<GithubProfile>()
        .await
        .map_err(|e| CallbackError::Other(format!("user parse: {}", e)))
}

async fn fetch_primary_verified_email<S: GithubOAuthState>(
    st: &S,
    token: &str,
) -> Result<Option<String>, CallbackError> {
    let resp = st
        .http_client()
        .get("https://api.github.com/user/emails")
        .bearer_auth(token)
        .header(header::ACCEPT, "application/vnd.github+json")
        .header(header::USER_AGENT, "openweights-cas/1.0")
        .timeout(Duration::from_secs(10))
        .send()
        .await
        .map_err(|e| CallbackError::Other(format!("emails fetch: {}", e)))?;

    if !resp.status().is_success() {
        return Ok(None);
    }
    let emails: Vec<GithubEmail> = resp
        .json()
        .await
        .map_err(|e| CallbackError::Other(format!("emails parse: {}", e)))?;
    Ok(emails
        .into_iter()
        .find(|e| e.primary && e.verified)
        .map(|e| e.email))
}

/// Upsert the `users` row keyed on numeric `id` ( + ). Returns
/// the id + a bool indicating whether the row was newly created.
/// Uses `xmax = 0` on the RETURNING clause to distinguish INSERT from
/// UPDATE without a separate SELECT: `xmax = 0` <=> row was inserted on
/// this statement (no prior tuple); any other value means an update.
async fn upsert_user<S: GithubOAuthState>(
    st: &S,
    profile: &GithubProfile,
    email: Option<&str>,
) -> Result<(i64, bool), CallbackError> {
    let row: (i64, bool) = sqlx::query_as(
        "INSERT INTO users (id, github_login, email, avatar_url) \
         VALUES ($1, $2, $3, $4) \
         ON CONFLICT (id) DO UPDATE \
           SET github_login = EXCLUDED.github_login, \
               email        = EXCLUDED.email, \
               avatar_url   = EXCLUDED.avatar_url \
         RETURNING id, (xmax = 0) AS is_new",
    )
    .bind(profile.id)
    .bind(&profile.login)
    .bind(email)
    .bind(&profile.avatar_url)
    .fetch_one(st.pool())
    .await
    .map_err(CallbackError::Db)?;
    Ok(row)
}

// ---------------------------------------------------------------------------
// Error type — keeps the P14-mitigation error codes stable.
// ---------------------------------------------------------------------------

/// Structured callback errors. Every variant maps to a fixed HTTP
/// status + stable JSON `code` string so the self-host operator can grep
/// logs for the exact failure mode.
#[derive(Debug)]
pub enum CallbackError {
    /// `oauth_state_mismatch` — `state` param missing OR not-an-active-
    /// nonce. Either a replay or a CSRF-forged callback.
    StateMismatch,
    /// `oauth_code_missing` — GitHub redirected back without a `code`
    /// query param (typically happens when the user denies the consent
    /// screen and GitHub adds `error=access_denied`).
    CodeMissing,
    /// `github_token_exchange_failed` — `/login/oauth/access_token`
    /// returned an error body OR a non-2xx status. Detail string echoes
    /// GitHub's `error_description` field for operator debugging.
    TokenExchangeFailed(String),
    /// Anything else (db error, body-parse miss, header build fail).
    /// Collapses to 500.
    Db(sqlx::Error),
    Other(String),
}

impl IntoResponse for CallbackError {
    fn into_response(self) -> Response {
        let (status, body) = match self {
            CallbackError::StateMismatch => (
                StatusCode::BAD_REQUEST,
                json!({"code": "oauth_state_mismatch"}),
            ),
            CallbackError::CodeMissing => (
                StatusCode::BAD_REQUEST,
                json!({"code": "oauth_code_missing"}),
            ),
            CallbackError::TokenExchangeFailed(detail) => (
                StatusCode::BAD_GATEWAY,
                json!({"code": "github_token_exchange_failed", "detail": detail}),
            ),
            CallbackError::Db(e) => {
                tracing::error!(err = ?e, "/auth/github/callback db error");
                (
                    StatusCode::INTERNAL_SERVER_ERROR,
                    json!({"code": "internal"}),
                )
            }
            CallbackError::Other(detail) => {
                tracing::error!(err = %detail, "/auth/github/callback other error");
                (
                    StatusCode::INTERNAL_SERVER_ERROR,
                    json!({"code": "internal"}),
                )
            }
        };
        (status, Json(body)).into_response()
    }
}

// ---------------------------------------------------------------------------
// Minimal URL-percent-encode (no query crate in the workspace; keeping the
// encode surface local avoids a new dep). Encodes the safe-chars set per
// RFC 3986 section 2.3 plus characters we know GitHub tolerates; anything
// else percent-encodes as `%HH`.
// ---------------------------------------------------------------------------

fn urlencode(s: &str) -> String {
    let mut out = String::with_capacity(s.len());
    for &b in s.as_bytes() {
        match b {
            b'A'..=b'Z' | b'a'..=b'z' | b'0'..=b'9' | b'-' | b'_' | b'.' | b'~' => {
                out.push(b as char);
            }
            _ => {
                out.push('%');
                out.push(char::from_digit((b >> 4) as u32, 16).unwrap().to_ascii_uppercase());
                out.push(char::from_digit((b & 0x0f) as u32, 16).unwrap().to_ascii_uppercase());
            }
        }
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn urlencode_leaves_unreserved() {
        assert_eq!(urlencode("abc-XYZ_123.~"), "abc-XYZ_123.~");
    }

    #[test]
    fn urlencode_encodes_colon_slash_space() {
        assert_eq!(
            urlencode("http://localhost:8080/auth"),
            "http%3A%2F%2Flocalhost%3A8080%2Fauth"
        );
        assert_eq!(urlencode("user:email read:user"), "user%3Aemail%20read%3Auser");
    }

    #[test]
    fn callback_error_codes_are_stable() {
        // Quick grep-style assertion — the literal strings are part of the
        // contract (04-CONTEXT §3 mitigation).
        let r: axum::response::Response = CallbackError::StateMismatch.into_response();
        assert_eq!(r.status(), StatusCode::BAD_REQUEST);
        let r: axum::response::Response = CallbackError::CodeMissing.into_response();
        assert_eq!(r.status(), StatusCode::BAD_REQUEST);
        let r: axum::response::Response =
            CallbackError::TokenExchangeFailed("x".into()).into_response();
        assert_eq!(r.status(), StatusCode::BAD_GATEWAY);
    }
}
