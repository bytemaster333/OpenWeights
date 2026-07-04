//! `/auth/*` handlers — GitHub OAuth flow + session teardown.
//! ( amendment —). Surface:
//! | Method + Path | Handler |
//! |----------------------------|-------------------------------|
//! | GET /auth/github/start | `github::start` |
//! | GET /auth/github/callback | `github::callback` |
//! | POST /auth/logout | `logout::logout` |
//! The start+callback pair exchanges an OAuth `code` with GitHub, upserts the
//! `users` row keyed on numeric `users.id BIGINT` (NOT email — ), mints a
//! session row + `openweights_session` cookie, and redirects the browser back to
//! the console.
//! mitigation: structured error codes `oauth_state_mismatch`,
//! `oauth_code_missing`, `github_token_exchange_failed` surface in callback
//! error responses so the self-host operator can distinguish OAuth
//! misconfigurations from GitHub API flakes.

pub mod github;
pub mod logout;
