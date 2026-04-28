// Downstream Phase-2 plans (..02-09) consume fields of `Config` and
// helpers that are scaffolded but not yet read at this wave tick. Suppressing
// `dead_code` here keeps `-D warnings` green without polluting individual
// module files owned by sibling plans.
#[allow(dead_code)]
mod config;
mod router;
mod state;
mod tracing_setup;

use std::sync::Arc;
use std::sync::atomic::{AtomicBool, Ordering};

use anyhow::{Context, anyhow};
use base64::{Engine as _, engine::general_purpose::STANDARD as B64};

use crate::config::Config;
use crate::state::AppState;

#[tokio::main]
async fn main() {
    tracing_setup::init();

    let cfg = match Config::from_env() {
        Ok(c) => c,
        Err(e) => {
            // Exit 2 = config error (matches bench/bootstrap/env.go taxonomy)
            tracing::error!(err = %e, "config load failed");
            std::process::exit(2);
        }
    };

    if let Err(e) = run(cfg).await {
        tracing::error!(err = %e, "fatal runtime error");
        std::process::exit(1);
    }
}

async fn run(cfg: Config) -> anyhow::Result<()> {
    let pool = siahub_cas_db::build_pool(&cfg.database_url, cfg.pg_max_connections)
        .await
        .context("postgres pool connect")?;

    // : run migrations before axum::serve. Embedded at
    // build time via sqlx::migrate!; idempotent on replay.
    siahub_cas_db::run_migrations(&pool)
        .await
        .context("database migrations failed")?;
    tracing::info!("migrations up to date");

    // Redis client — fred v10. exercises it from rate-limit middleware.
    // `init` lives on the `ClientLike` trait — must be in scope.
    use fred::interfaces::ClientLike;
    let redis_cfg = fred::types::config::Config::from_url(&cfg.redis_url)?;
    let redis = fred::clients::Client::new(redis_cfg, None, None, None);
    redis.init().await?;

    // : in-process LRU key cache (10 000 entries per ) +
    // per-class rate-limit defaults.
    let key_cache = Arc::new(siahub_cas_core::auth::KeyCache::new(10_000));
    let rate_limit_defaults = siahub_cas_core::rate_limit::RateLimitDefaults::default();

    // : Sia adapter. Handshake with indexd at boot (A1 probe's
    // verbatim builder sequence: Builder::new(url, meta)?.connected(&app_key)).
    // Binary-only SIA_MOCK=true escape hatch lets local dev boot without a
    // live indexd; NEVER set in production (mock skips all Sia durability).
    let sia: Arc<dyn siahub_cas_storage::SiaAdapter> = build_sia_adapter(&cfg)
        .await
        .context("sia adapter init")?;

    // : HMAC-SHA256 signed-URL minter. Constructed once at boot;
    // dual-key rotation window (`..._KEY_PREV`) is verifier-only.
    let gateway_base = url::Url::parse(&cfg.gateway_base_url)
        .context("GATEWAY_BASE_URL must be a valid URL")?;
    let signer = siahub_cas_core::signed_url::UrlSigner::new(
        &cfg.gateway_url_signing_key,
        cfg.gateway_url_signing_key_prev.as_deref(),
        gateway_base,
        cfg.gateway_url_ttl_secs,
    )
    .context("signed-url signer init (GATEWAY_URL_SIGNING_KEY)")?;

    // : Prometheus registry + readiness latch. `ready` gates
    // `/health` until we have applied migrations AND the Sia self-check
    // returned OK (the SiaAdapter builder above performs the handshake;
    // arriving here means it succeeded). We flip the latch AFTER the
    // AppState is assembled to keep readiness ordering obvious.
    let metrics = Arc::new(siahub_cas_core::metrics::Metrics::new());
    let ready = Arc::new(AtomicBool::new(false));

    //shared reqwest client for OAuth + indexd proxy + setup
    // probes. Build once so TLS handshakes + connection pooling are reused
    // across the three handlers that need outbound HTTP.
    let http_client = Arc::new(
        reqwest::Client::builder()
            .user_agent("siahub-cas/1.0")
            .build()
            .context("reqwest client build")?,
    );

    let state = AppState {
        cfg: Arc::new(cfg.clone()),
        pool: pool.clone(),
        redis: Arc::new(redis),
        key_cache,
        rate_limit_defaults,
        sia: sia.clone(),
        signer: Arc::new(signer),
        metrics: metrics.clone(),
        ready: ready.clone(),
        http_client,
    };

    // Task 3 — spawn the pin-state reconciler. The task runs for
    // the lifetime of the process; tokio drops it on main-future teardown.
    let reconciler_pool = pool.clone();
    let reconciler_sia = sia.clone();
    let reconciler_metrics = metrics.clone();
    tokio::spawn(async move {
        siahub_cas_storage::reconciler::run_reconciler(
            reconciler_pool,
            reconciler_sia,
            reconciler_metrics,
        )
        .await;
    });

    // Migrations applied + Sia connected + reconciler spawned → ready.
    ready.store(true, Ordering::Release);
    tracing::info!("siahub-cas ready");

    let bind_addr = cfg.bind_addr.clone();
    let app = router::build(state);
    let listener = tokio::net::TcpListener::bind(&bind_addr).await?;
    tracing::info!(addr = %bind_addr, "siahub-cas listening");
    axum::serve(listener, app).await?;
    Ok(())
}

/// Build the Sia adapter for this process.
/// Priority order:
/// 1. `SIAHUB_SIA_MOCK=true` (dev only) → in-memory `MockSiaAdapter`. Only
/// compiled in when the `sia-mock` feature flag is enabled on this
/// crate..md forbids this path in production.
/// 2. Otherwise, decode SIAHUB_APP_ID (hex) + SIAHUB_APP_KEY (base64 32
/// bytes) and run the A1-probe builder sequence.
async fn build_sia_adapter(
    cfg: &Config,
) -> anyhow::Result<Arc<dyn siahub_cas_storage::SiaAdapter>> {
    #[cfg(feature = "sia-mock")]
    {
        if std::env::var("SIAHUB_SIA_MOCK").as_deref() == Ok("true") {
            tracing::warn!(
                "SIAHUB_SIA_MOCK=true — wiring in-memory MockSiaAdapter. \
                 DEV ONLY: bytes are NOT durable on Sia."
            );
            return Ok(Arc::new(siahub_cas_storage::mock::MockSiaAdapter::new()));
        }
    }

    // Decode the base64 app key → 32 bytes.
    let key_bytes_vec = B64
        .decode(cfg.siahub_app_key.as_bytes())
        .context("SIAHUB_APP_KEY is not valid base64")?;
    let app_key_bytes: [u8; 32] = key_bytes_vec
        .as_slice()
        .try_into()
        .map_err(|_| anyhow!("SIAHUB_APP_KEY must decode to exactly 32 bytes"))?;

    // Decode the hex app_id (64 chars).
    if cfg.siahub_app_id.len() != 64 {
        return Err(anyhow!("SIAHUB_APP_ID must be 64 hex chars"));
    }
    let mut id_bytes = [0u8; 32];
    for (i, slot) in id_bytes.iter_mut().enumerate() {
        *slot = u8::from_str_radix(&cfg.siahub_app_id[2 * i..2 * i + 2], 16)
            .context("SIAHUB_APP_ID must be hex")?;
    }
    let app_id = sia_storage::Hash256::new(id_bytes);

    // Static &'static str fields on AppMetadata; hard-coded per.
    let meta = sia_storage::AppMetadata {
        id: app_id,
        name: siahub_cas_storage::sia::DEFAULT_APP_META_NAME,
        description: siahub_cas_storage::sia::DEFAULT_APP_META_DESC,
        service_url: siahub_cas_storage::sia::DEFAULT_APP_META_URL,
        logo_url: None,
        callback_url: None,
    };

    // Redundancy from env. Demo deployments with few hosts must dial this
    // below the SDK default of 10+20=30 (e.g. 2+4 for a 6-host demo). Both
    // values must fit in u8 and total ≤ formed contracts.
    let data_shards: u8 = std::env::var("SIAHUB_DATA_SHARDS")
        .ok()
        .and_then(|s| s.parse().ok())
        .unwrap_or(10);
    let parity_shards: u8 = std::env::var("SIAHUB_PARITY_SHARDS")
        .ok()
        .and_then(|s| s.parse().ok())
        .unwrap_or(20);
    tracing::info!(
        data_shards,
        parity_shards,
        total = data_shards as u16 + parity_shards as u16,
        "sia upload redundancy configured"
    );

    let sia_cfg = siahub_cas_storage::sia::SiaAdapterConfig {
        indexd_url: cfg.indexd_url.clone(),
        app_key_bytes,
        app_meta: meta,
        redundancy: siahub_cas_storage::sia::RedundancyConfig {
            data_shards,
            parity_shards,
        },
    };

    let adapter = siahub_cas_storage::sia::RustSdkAdapter::connect(sia_cfg)
        .await
        .context("sia_storage Builder handshake failed")?;
    Ok(Arc::new(adapter))
}
