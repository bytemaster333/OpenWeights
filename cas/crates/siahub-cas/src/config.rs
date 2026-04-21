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

    // Sia App Key / App ID (never persisted to DB — D-12; also notes.md gotcha #5)
    pub siahub_app_id: String,
    pub siahub_app_key: String, // base64 32 bytes

    // Gateway URL signing (D-14)
    pub gateway_url_signing_key: String, // base64 32 bytes
    pub gateway_url_signing_key_prev: Option<String>,
    #[serde(default = "default_url_ttl")]
    pub gateway_url_ttl_secs: u64, // default 7200 per D-14
    /// Canonical URL base the minter stamps into signed URLs, e.g.
    /// `https://cas.siahub.app` in production. Falls back to the local
    /// loopback gateway port for tests + docker-local dev.
    #[serde(default = "default_gateway_base_url")]
    pub gateway_base_url: String,

    // Phase 2 feature flag (D-18)
    #[serde(default)]
    pub v2_reconstruction_enabled: bool, // default false per D-18

    // Pool sizing
    #[serde(default = "default_pg_conn")]
    pub pg_max_connections: u32,
}

fn default_bind() -> String {
    "0.0.0.0:8080".to_string()
}
fn default_url_ttl() -> u64 {
    7200
}
fn default_gateway_base_url() -> String {
    // Loopback default — Phase 3 gateway binds here locally; production
    // overrides via `GATEWAY_BASE_URL=https://cas.siahub.app` in `.env`.
    "http://127.0.0.1:9090".to_string()
}
fn default_pg_conn() -> u32 {
    20
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
