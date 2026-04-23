use crate::state::AppState;
use axum::{
    Router,
    routing::{delete, get, post},
};
use siahub_cas_core::handlers;

/// Build the top-level router. Plans 02-03..02-09 register their routes here.
///
/// notes.md §What NOT to Do: NEVER register
///   - HEAD /v1/xorbs/*, HEAD /v1/files/*
///   - /simulation/*, /v1/fetch_term, /v1/get_xorb/*
///   - any DELETE route
///   - S3-compat, ETag/conditional-GET, CORS
pub fn build(state: AppState) -> Router {
    Router::new()
        // Plan 02-09 — readiness-gated /health + Prometheus /metrics scaffold.
        // /metrics MUST remain unauthenticated; Compose binds the container
        // to 127.0.0.1 and Phase 6 Caddy denies public /metrics (OPS-02/03).
        .route("/health", get(handlers::health::health::<AppState>))
        .route(
            "/metrics",
            get(siahub_cas_core::metrics::metrics_handler::<AppState>),
        )
        // Plan 02-04 — POST /v1/xorbs/:prefix/:hash (PROTO-02).
        .route(
            "/v1/xorbs/{prefix}/{hash}",
            post(handlers::xorbs::upload_xorb::<AppState>),
        )
        // Plan 02-05 — dual-path shard upload (PROTO-03, notes.md gotcha #2).
        // BOTH paths MUST route to the same handler — xet-core's production
        // `RemoteClient::upload_shard` calls `/shards` (no `/v1`); the OpenAPI
        // spec says `/v1/shards`. Registering only one silently breaks
        // conformance.
        .route(
            "/shards",
            post(handlers::shards::upload_shard::<AppState>),
        )
        .route(
            "/v1/shards",
            post(handlers::shards::upload_shard::<AppState>),
        )
        // Plan 02-05 — PROTO-04 dedup endpoint. Ships as unconditional 404
        // (D-16) with auth still enforced so 401/403 remain distinguishable.
        // Only the /v1-prefixed path is legal per xet-core's
        // `remote_client.rs::query_dedup_api`.
        .route(
            "/v1/chunks/{prefix}/{hash}",
            get(handlers::dedup::query_dedup_shard::<AppState>),
        )
        // Plan 02-06 — V1 reconstruction (PROTO-05) + batch (PROTO-07).
        // Batch is DUAL-PATH: xet-core's `batch_get_reconstruction` hits
        // `/reconstructions?` (no `/v1`) while the OpenAPI spec lists
        // `/v1/reconstructions?`. Both MUST be served (RESEARCH §2.5).
        .route(
            "/v1/reconstructions/{file_id}",
            get(handlers::reconstruction::query_reconstruction_v1::<AppState>),
        )
        .route(
            "/v1/reconstructions",
            get(handlers::reconstruction::query_reconstructions_batch_v1::<AppState>),
        )
        .route(
            "/reconstructions",
            get(handlers::reconstruction::query_reconstructions_batch_v1::<AppState>),
        )
        // Plan 02-07 — V2 reconstruction (PROTO-06). Feature-flagged per D-18:
        // default response is 501 Not Implemented until Phase 3 GATE-03 flips
        // `V2_RECONSTRUCTION_ENABLED=true` in `.env`. The full V2 response
        // builder is in code so the flip is a single-env-toggle change —
        // notes.md gotcha #3 requires this shape to avoid silent corruption
        // via multi-range `Range:` headers before the gateway serves
        // `multipart/byteranges`. No batch V2 endpoint — xet-core's V2 path is
        // per-file (`get_reconstruction_with_version_override`, RESEARCH §2.6).
        .route(
            "/v2/reconstructions/{file_id}",
            get(handlers::reconstruction_v2::query_reconstruction_v2::<AppState>),
        )
        // Plan 04-01 (Phase 2.1 amendment — D-41) — admin + auth routes.
        // Owned by Phase 4 per D-41; consumed by siahub-console's
        // /admin/* + /auth/* flows. Session cookie middleware is applied
        // via the `Session` extractor at handler level; no router-level
        // tower::Layer wraps these because the public auth start/callback
        // routes live in the same group and MUST remain unauthenticated.
        .route(
            "/admin/me",
            get(handlers::admin::me::get_me::<AppState>),
        )
        .route(
            "/admin/keys",
            post(handlers::admin::keys::create_key::<AppState>)
                .get(handlers::admin::keys::list_keys::<AppState>),
        )
        .route(
            "/admin/keys/{id}",
            delete(handlers::admin::keys::revoke_key::<AppState>),
        )
        .route(
            "/admin/stats",
            get(handlers::admin::stats::get_stats::<AppState>),
        )
        .route(
            "/admin/xorbs",
            get(handlers::admin::xorbs::list_xorbs::<AppState>),
        )
        // Plan 05-01 Task 5 (Phase 2.2 amendment — D-66/D-69).
        // Single-hash xorb detail lookup used by Phase 4's AssetDetail page
        // to drop the prefix-fallback workaround. The `{hash}` segment is
        // lowercase 64-char hex of the 32-byte xorb merkle hash BYTEA —
        // decoded in-handler; 400 on malformed, 403 on non-admin,
        // 404 on absence.
        .route(
            "/admin/xorbs/{hash}",
            get(handlers::admin::xorbs::get_xorb_detail::<AppState>),
        )
        .route(
            "/admin/stats/map",
            get(handlers::admin::map::get_map::<AppState>),
        )
        .route(
            "/admin/setup/status",
            get(handlers::admin::setup::get_setup_status::<AppState>),
        )
        .route(
            "/auth/github/start",
            get(handlers::auth::github::start::<AppState>),
        )
        .route(
            "/auth/github/callback",
            get(handlers::auth::github::callback::<AppState>),
        )
        .route(
            "/auth/logout",
            post(handlers::auth::logout::logout::<AppState>),
        )
        .with_state(state)
}
