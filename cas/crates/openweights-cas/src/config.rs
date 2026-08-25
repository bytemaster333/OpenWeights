use serde::Deserialize;
use std::time::Duration;

#[derive(Debug, Clone, Deserialize)]
pub struct Config {
    // Binding
    #[serde(default = "default_bind")]
    pub bind_addr: String, // e.g. "0.0.0.0:8080"

    // Postgres / Redis / Indexd
    pub database_url: String, // required
    pub redis_url: String,    // required
    pub indexd_url: String,   // required

    // Sia App Key / App ID (never persisted to DB — ; also.md )
    pub openweights_app_id: String,
    pub openweights_app_key: String, // base64 32 bytes

    // Gateway URL signing
    pub gateway_url_signing_key: String, // base64 32 bytes
    pub gateway_url_signing_key_prev: Option<String>,
    #[serde(default = "default_url_ttl")]
    pub gateway_url_ttl_secs: u64, // default 7200 per
    /// Canonical URL base the minter stamps into signed URLs, e.g.
    /// `https://cas.example.com` in production. Falls back to the local
    /// loopback gateway port for tests + docker-local dev.
    #[serde(default = "default_gateway_base_url")]
    pub gateway_base_url: String,

    // feature flag
    #[serde(default)]
    pub v2_reconstruction_enabled: bool, // default false per

    // Pool sizing
    #[serde(default = "default_pg_conn")]
    pub pg_max_connections: u32,

    // ( amendment) — GitHub OAuth + indexd admin.
    // (HIGH): the callback URL MUST match the value registered in the
    // self-host operator's GitHub OAuth App settings. Mismatch manifests
    // as opaque `redirect_uri_mismatch` from GitHub — `.env.example`
    // carries a prominent comment.
    #[serde(default)]
    pub github_oauth_client_id: String,
    #[serde(default)]
    pub github_oauth_client_secret: String,

    /// Password sign-in for self-hosted, single-operator setups. When set,
    /// the console shows a password box and the operator can run OpenWeights
    /// without registering a GitHub OAuth app. Empty ⇒ password auth off.
    /// Lives only here (env), never in the DB or logs.
    #[serde(default)]
    pub openweights_admin_password: String,
    /// Display login for the synthetic password-admin user (`users.id = -1`).
    #[serde(default = "default_admin_username")]
    pub openweights_admin_username: String,
    #[serde(default = "default_github_callback_url")]
    pub github_oauth_callback_url: String,
    /// Where to redirect the browser after a successful OAuth callback.
    /// Defaults to the Vite dev server origin for local dev; production
    /// overrides to `https://example.com`.
    #[serde(default = "default_console_base_url")]
    pub console_base_url: String,
    /// HTTP Basic auth password for indexd's admin endpoints. Same secret
    /// derives at bootstrap; CAS reads it for the
    /// `/admin/stats/map` + `/admin/setup/status` probes.
    #[serde(default)]
    pub indexd_admin_password: String,

    /// Admin-API base URL (no trailing `/`) of a SELF-HOSTED indexd, e.g.
    /// `http://indexd:9980`. Distinct from `indexd_url` (the app API, the
    /// storage data path). The console Map + Setup "on Sia network" panels
    /// read this admin API. Empty by default: a hosted indexer
    /// (https://sia.storage) has no admin API, so those panels degrade to
    /// empty. Operators running their own indexd set INDEXD_ADMIN_URL.
    #[serde(default)]
    pub indexd_admin_url: String,

    /// HS256 signing secret for OpenWeights-issued Xet JWTs (migration 0008).
    /// Used by the HF-API-compat `xet-write-token` / `xet-read-token`
    /// endpoints to mint tokens that hf_xet presents to our CAS upload
    /// endpoints. Also used to tell CAS which URL to put in the
    /// `casUrl` field of those responses (see `cas_public_url`).
    /// Empty default means "OpenWeights HF-compat mode disabled" — the
    /// /api/* handlers return 503 until the operator sets this.
    #[serde(default)]
    pub xet_jwt_signing_key: String,

    /// URL clients reach OpenWeights CAS at for Xet uploads/downloads. This is
    /// the value returned in `casUrl` of xet-write-token responses. Must
    /// be reachable from the *client* machine, not from inside the Compose
    /// network. Local dev: `http://localhost:28080`. Prod:
    /// `https://cas.example.com`.
    #[serde(default = "default_cas_public_url")]
    pub cas_public_url: String,
}

fn default_bind() -> String {
    "0.0.0.0:8080".to_string()
}
fn default_url_ttl() -> u64 {
    7200
}
fn default_gateway_base_url() -> String {
    // Loopback default — gateway binds here locally; production
    // overrides via `GATEWAY_BASE_URL=https://cas.example.com` in `.env`.
    "http://127.0.0.1:9090".to_string()
}
fn default_pg_conn() -> u32 {
    20
}
fn default_github_callback_url() -> String {
    // Local-dev default; production overrides via
    // `GITHUB_OAUTH_CALLBACK_URL=https://cas.example.com/auth/github/callback`
    // (or equivalent). Must match the GitHub OAuth App registration.
    "http://localhost:8080/auth/github/callback".to_string()
}
fn default_admin_username() -> String {
    "admin".to_string()
}
fn default_console_base_url() -> String {
    // Vite dev server origin by default; production overrides via
    // `CONSOLE_BASE_URL=https://example.com` in `.env`.
    "http://localhost:5173".to_string()
}
fn default_cas_public_url() -> String {
    // Matches the base compose host-visible CAS port (127.0.0.1:8080). Compose
    // always sets CAS_PUBLIC_URL explicitly; this default only applies to a bare
    // `cargo run`. The dev override remaps to 28080 and sets it via .env.
    "http://localhost:8080".to_string()
}

impl Config {
    /// Load from environment; fail loud (non-zero exit) on missing required var.
    /// Exit code 2 on config error, matching bench/bootstrap/env.go.
    pub fn from_env() -> anyhow::Result<Self> {
        Ok(envy::from_env::<Config>()?)
    }

    pub fn acquire_timeout(&self) -> Duration {
        Duration::from_secs(5)
    }
}
