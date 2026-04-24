//! `POST /auth/logout` — revoke the current session server-side and clear
//! the cookie client-side.
//! Backs. Requires a valid `siahub_session` cookie (extractor
//! returns 401 otherwise). Idempotent against its own revocation: a second
//! call by the same revoked session would already fail at the extractor.
//! Response: `204 No Content` with `Set-Cookie: siahub_session=; Max-Age=0`.
//! Console's `<UserMenu>` hits this endpoint then redirects to `/login`

use std::future::Future;

use axum::extract::State;
use axum::http::{HeaderMap, HeaderValue, StatusCode, header};
use axum::response::{IntoResponse, Response};
use http::header::COOKIE;
use uuid::Uuid;

use crate::auth::AuthStateRef;
use crate::errors::AppError;
use crate::session::{clear_session_cookie_header, revoke_session};

/// The extractor we want is "extract session_id from the cookie and keep
/// the raw UUID" — the `Session` extractor joins + touches + returns the
/// user. For logout we do NOT want to extend the session's expiry by
/// touching it, so we parse the cookie ourselves.
pub struct SessionCookie(pub Uuid);

impl<St> axum::extract::FromRequestParts<St> for SessionCookie
where
    St: AuthStateRef,
{
    type Rejection = AppError;

    fn from_request_parts(
        parts: &mut axum::http::request::Parts,
        _state: &St,
    ) -> impl Future<Output = Result<Self, Self::Rejection>> + Send {
        let raw = parts
            .headers
            .get(COOKIE)
            .and_then(|h| h.to_str().ok())
            .map(str::to_owned);
        async move {
            let id = raw
                .as_deref()
                .and_then(crate::session::parse_session_cookie)
                .ok_or(AppError::Unauthenticated)?;
            Ok(SessionCookie(id))
        }
    }
}

pub async fn logout<S: AuthStateRef>(
    State(st): State<S>,
    SessionCookie(session_id): SessionCookie,
) -> Result<Response, AppError> {
    // Idempotent revoke — returning `false` (already revoked) is fine; we
    // still clear the cookie on the client so repeated logout attempts
    // converge to the same state.
    revoke_session(st.pool(), session_id).await?;

    let mut headers = HeaderMap::new();
    headers.insert(
        header::SET_COOKIE,
        HeaderValue::from_str(&clear_session_cookie_header())
            .map_err(|e| AppError::Other(e.into()))?,
    );
    Ok((StatusCode::NO_CONTENT, headers).into_response())
}

#[cfg(test)]
mod tests {
    use crate::session::{SESSION_COOKIE_NAME, clear_session_cookie_header};

    #[test]
    fn clear_cookie_header_mentions_siahub_session() {
        let h = clear_session_cookie_header();
        assert!(h.contains(SESSION_COOKIE_NAME));
        assert!(h.contains("Max-Age=0"));
    }
}
