use sqlx::PgPool;
use std::sync::{Arc, atomic::AtomicBool};

use openweights_cas_core::auth::{AuthStateRef, KeyCache};
use openweights_cas_core::handlers::admin::map::MapState;
use openweights_cas_core::handlers::admin::setup::SetupState;
use openweights_cas_core::handlers::auth::github::GithubOAuthState;
use openweights_cas_core::handlers::auth::password::PasswordAuthState;
use openweights_cas_core::handlers::health::HealthState;
use openweights_cas_core::handlers::hfapi::HfApiState;
use openweights_cas_core::handlers::reconstruction::ReconstructionState;
use openweights_cas_core::handlers::shards::ShardUploadState;
use openweights_cas_core::handlers::xorbs::XorbUploadState;
use openweights_cas_core::metrics::{Metrics, MetricsState};
use openweights_cas_core::rate_limit::RateLimitDefaults;
use openweights_cas_core::signed_url::{UrlMinter, UrlSigner};
use openweights_cas_storage::SiaAdapter;

use crate::config::Config;

/// App state shared across handlers via `Router::with_state`.
/// wires `db`; wires `redis` + `key_cache` + `rate_limit_defaults`;
/// wires a Sia adapter. Fields not yet consumed at this wave tick
/// suppressed to keep `-D warnings` green until downstream plans land.
#[allow(dead_code)]
#[derive(Clone)]
pub struct AppState {
    pub cfg: Arc<Config>,
    pub pool: PgPool,
    pub redis: Arc<fred::clients::Client>,
    /// In-process LRU of `sha256(bearer) → ActiveKey` rows (capacity 10 000 per ).
    pub key_cache: Arc<KeyCache>,
    /// Per-class token-bucket defaults.
    pub rate_limit_defaults: RateLimitDefaults,
    /// Sia storage boundary (CONTEXT + Task 2). Wired from
    /// `main.rs::run` to either a real `RustSdkAdapter::connect(...)` or a
    /// `MockSiaAdapter::new` for the local/compose-less dev loop.
    pub sia: Arc<dyn SiaAdapter>,
    /// HMAC-SHA256 signed-URL minter. Constructed from
    /// `GATEWAY_URL_SIGNING_KEY` + optional `GATEWAY_URL_SIGNING_KEY_PREV`
    /// at boot; never rebuilt on the hot path. reconstruction
    /// handlers consume this via `st.signer.mint_v1(...)`.
    pub signer: Arc<UrlSigner>,
    /// Shared Prometheus handle set. Wired from `main.rs::run`
    /// via `Metrics::new`; handlers bump counters through the
    /// `MetricsState` trait so the binary crate stays the only consumer of
    /// the concrete type.
    pub metrics: Arc<Metrics>,
    /// Readiness latch ( Task 4). `GET /health` returns 503 while
    /// this is `false` — `main.rs::run` flips it to `true` only after
    /// migrations applied AND the Sia startup self-check succeeded.
    pub ready: Arc<AtomicBool>,
    ///shared `reqwest::Client` reused across the OAuth
    /// token-exchange call, the `/admin/stats/map` indexd proxy, and the
    /// `/admin/setup/status` probes. Built ONCE at boot so TLS handshakes
    /// + connection pooling are reused.
    pub http_client: Arc<reqwest::Client>,
}

impl AuthStateRef for AppState {
    fn pool(&self) -> &PgPool {
        &self.pool
    }

    fn key_cache(&self) -> Arc<KeyCache> {
        self.key_cache.clone()
    }

    fn xet_jwt_signing_key(&self) -> Vec<u8> {
        // Empty when unconfigured → HF-compat mint is disabled (handler 503s)
        // and self-issued-token verification rejects (no key to verify with).
        self.cfg.xet_jwt_signing_key.as_bytes().to_vec()
    }
}

impl XorbUploadState for AppState {
    fn sia(&self) -> Arc<dyn SiaAdapter> {
        self.sia.clone()
    }

    fn redis(&self) -> Arc<fred::clients::Client> {
        self.redis.clone()
    }

    fn rate_limit_defaults(&self) -> RateLimitDefaults {
        self.rate_limit_defaults
    }
}

impl ShardUploadState for AppState {
    fn sia(&self) -> Arc<dyn SiaAdapter> {
        self.sia.clone()
    }

    fn redis(&self) -> Arc<fred::clients::Client> {
        self.redis.clone()
    }

    fn rate_limit_defaults(&self) -> RateLimitDefaults {
        self.rate_limit_defaults
    }
}

impl MetricsState for AppState {
    fn metrics(&self) -> Arc<Metrics> {
        self.metrics.clone()
    }
}

impl HealthState for AppState {
    fn ready(&self) -> Arc<AtomicBool> {
        self.ready.clone()
    }

    fn pool(&self) -> &PgPool {
        &self.pool
    }
}

impl ReconstructionState for AppState {
    fn redis(&self) -> Arc<fred::clients::Client> {
        self.redis.clone()
    }

    fn rate_limit_defaults(&self) -> RateLimitDefaults {
        self.rate_limit_defaults
    }

    fn url_signer(&self) -> Arc<dyn UrlMinter> {
        // 's concrete UrlSigner implements UrlMinter; widen here.
        self.signer.clone() as Arc<dyn UrlMinter>
    }

    fn v2_reconstruction_enabled(&self) -> bool {
        //flag defaults to `false`; flips `.env` only.
        //.md : enabling V2 before the gateway serves
        // multipart/byteranges silently corrupts xet-core downloads.
        self.cfg.v2_reconstruction_enabled
    }
}

// /admin/stats/map + OAuth + /admin/setup/status state impls.

impl MapState for AppState {
    fn indexd_admin_url(&self) -> &str {
        // Admin + setup probes hit the admin API (`:9980`), NOT the app API
        // (`:9982`) that the SDK uses. They need different URLs.
        &self.cfg.indexd_admin_url
    }
    fn indexd_admin_password(&self) -> &str {
        &self.cfg.indexd_admin_password
    }
    fn http_client(&self) -> Arc<reqwest::Client> {
        self.http_client.clone()
    }
}

impl SetupState for AppState {
    fn redis(&self) -> Arc<fred::clients::Client> {
        self.redis.clone()
    }
    fn indexd_admin_url(&self) -> &str {
        // Admin + setup probes hit the admin API (`:9980`), NOT the app API
        // (`:9982`) that the SDK uses. They need different URLs.
        &self.cfg.indexd_admin_url
    }
    fn indexd_admin_password(&self) -> &str {
        &self.cfg.indexd_admin_password
    }
    fn http_client(&self) -> Arc<reqwest::Client> {
        self.http_client.clone()
    }
    fn github_oauth_configured(&self) -> bool {
        !self.cfg.github_oauth_client_id.is_empty()
            && !self.cfg.github_oauth_client_secret.is_empty()
    }
    fn v2_reconstruction_enabled(&self) -> bool {
        self.cfg.v2_reconstruction_enabled
    }
}

impl HfApiState for AppState {
    fn cas_public_url(&self) -> &str {
        &self.cfg.cas_public_url
    }
    fn sia(&self) -> Arc<dyn SiaAdapter> {
        self.sia.clone()
    }
}

impl PasswordAuthState for AppState {
    fn pool(&self) -> &PgPool {
        &self.pool
    }
    fn admin_password(&self) -> &str {
        &self.cfg.openweights_admin_password
    }
    fn admin_username(&self) -> &str {
        &self.cfg.openweights_admin_username
    }
    fn github_oauth_configured(&self) -> bool {
        !self.cfg.github_oauth_client_id.is_empty()
            && !self.cfg.github_oauth_client_secret.is_empty()
    }
}

impl GithubOAuthState for AppState {
    fn github_client_id(&self) -> &str {
        &self.cfg.github_oauth_client_id
    }
    fn github_client_secret(&self) -> &str {
        &self.cfg.github_oauth_client_secret
    }
    fn github_callback_url(&self) -> &str {
        &self.cfg.github_oauth_callback_url
    }
    fn console_base_url(&self) -> &str {
        &self.cfg.console_base_url
    }
    fn http_client(&self) -> Arc<reqwest::Client> {
        self.http_client.clone()
    }
}
