use crate::state::AppState;
use axum::{
    Router,
    http::{HeaderValue, Method, header},
    routing::{delete, get, post, put},
};
use siahub_cas_core::handlers;
use tower_http::cors::CorsLayer;

/// Build the top-level router. Plans.. register their routes here.
///.md §What NOT to Do: NEVER register
/// - HEAD /v1/xorbs/*, HEAD /v1/files/*
/// - /simulation/*, /v1/fetch_term, /v1/get_xorb/*
/// - any DELETE route
/// - S3-compat, ETag/conditional-GET, CORS
pub fn build(state: AppState) -> Router {
    // Console runs at a different origin from CAS (separate host ports) and
    // relies on credentialed fetches to ship the session cookie. Browsers
    // require explicit `Access-Control-Allow-Origin` (non-wildcard) and
    // `Access-Control-Allow-Credentials: true` for that to work. The origin
    // is read from `CONSOLE_BASE_URL` which already governs CAS's OAuth
    // redirect target — so there is exactly one console origin to trust.
    let console_origin: HeaderValue = state
        .cfg
        .console_base_url
        .parse()
        .unwrap_or_else(|_| HeaderValue::from_static("http://localhost:23000"));
    let cors = CorsLayer::new()
        .allow_origin(console_origin)
        .allow_credentials(true)
        .allow_methods([
            Method::GET,
            Method::POST,
            Method::DELETE,
            Method::PUT,
            Method::OPTIONS,
        ])
        .allow_headers([
            header::AUTHORIZATION,
            header::CONTENT_TYPE,
            header::ACCEPT,
            header::ORIGIN,
        ]);

    Router::new()
        //readiness-gated /health + Prometheus /metrics scaffold.
        // /metrics MUST remain unauthenticated; Compose binds the container
        // to 127.0.0.1 and Caddy denies public /metrics (/03).
        .route("/health", get(handlers::health::health::<AppState>))
        .route(
            "/metrics",
            get(siahub_cas_core::metrics::metrics_handler::<AppState>),
        )
        //POST /v1/xorbs/:prefix/:hash.
        .route(
            "/v1/xorbs/{prefix}/{hash}",
            post(handlers::xorbs::upload_xorb::<AppState>),
        )
        //dual-path shard upload (.md ).
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
        //dedup endpoint. Ships as unconditional 404
        // with auth still enforced so 401/403 remain distinguishable.
        // Only the /v1-prefixed path is legal per xet-core's
        // `remote_client.rs::query_dedup_api`.
        .route(
            "/v1/chunks/{prefix}/{hash}",
            get(handlers::dedup::query_dedup_shard::<AppState>),
        )
        //V1 reconstruction + batch.
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
        //V2 reconstruction. Feature-flagged per :
        // default response is 501 Not Implemented until flips
        // `V2_RECONSTRUCTION_ENABLED=true` in `.env`. The full V2 response
        // builder is in code so the flip is a single-env-toggle change
        //.md requires this shape to avoid silent corruption
        // via multi-range `Range:` headers before the gateway serves
        // `multipart/byteranges`. No batch V2 endpoint — xet-core's V2 path is
        // per-file (`get_reconstruction_with_version_override`, RESEARCH §2.6).
        .route(
            "/v2/reconstructions/{file_id}",
            get(handlers::reconstruction_v2::query_reconstruction_v2::<AppState>),
        )
        // ( amendment — ) — admin + auth routes.
        // Owned by per ; consumed by siahub-console's
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
        // Task 5 ( amendment — /).
        // Single-hash xorb detail lookup used by 's AssetDetail page
        // to drop the prefix-fallback workaround. The `{hash}` segment is
        // lowercase 64-char hex of the 32-byte xorb merkle hash BYTEA
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
        // Migration 0008 — HF-API-compat routes so `hf upload` /
        // `hf download` target SiaHub as the Hub itself, not
        // huggingface.co. hf_hub + hf_xet don't know the difference.
        .route(
            "/api/whoami-v2",
            get(handlers::hfapi::whoami_v2::<AppState>),
        )
        .route(
            "/api/validate-yaml",
            post(handlers::hfapi::validate_yaml),
        )
        .route(
            "/api/repos/create",
            post(handlers::hfapi::create_repo::<AppState>),
        )
        .route(
            "/api/models/{owner}/{repo}/preupload/{ref_name}",
            post(handlers::hfapi::preupload::<AppState>),
        )
        .route(
            "/api/models/{owner}/{repo}/xet-write-token/{ref_name}",
            get(handlers::hfapi::xet_write_token::<AppState>),
        )
        .route(
            "/api/models/{owner}/{repo}/xet-read-token/{ref_name}",
            get(handlers::hfapi::xet_read_token::<AppState>),
        )
        .route(
            "/api/models/{owner}/{repo}/commit/{ref_name}",
            post(handlers::hfapi::commit::<AppState>),
        )
        // hf_hub issues `POST /{owner}/{repo}.git/info/lfs/objects/batch`.
        // Axum 0.8 won't mix param + literal in one segment, so we
        // capture the `<repo>.git` segment verbatim as `repo_dotgit` and
        // the handler strips the `.git` suffix.
        .route(
            "/{owner}/{repo_dotgit}/info/lfs/objects/batch",
            post(handlers::hfapi::lfs_objects_batch::<AppState>),
        )
        .route(
            "/lfs/objects/{oid}",
            put(handlers::hfapi::lfs_upload::<AppState>)
                .get(handlers::hfapi::lfs_download::<AppState>),
        )
        // Download side (task #46). Public catalog + model metadata.
        .route(
            "/api/platform/stats",
            get(handlers::hfapi::platform_stats::<AppState>),
        )
        .route(
            "/api/models",
            get(handlers::hfapi::list_models::<AppState>),
        )
        .route(
            "/api/models/{owner}/{repo}",
            get(handlers::hfapi::model_info::<AppState>),
        )
        .route(
            "/api/models/{owner}/{repo}/downloads/trend",
            get(handlers::hfapi::model_downloads_trend::<AppState>),
        )
        .route(
            "/api/models/{owner}/{repo}/revision/{revision}",
            get(handlers::hfapi::model_info_rev::<AppState>),
        )
        // Wildcard at the tail so paths with slashes (e.g., folder/file)
        // resolve correctly. Axum 0.8 uses `{*name}` for catch-all.
        .route(
            "/{owner}/{repo}/resolve/{revision}/{*file_path}",
            get(handlers::hfapi::resolve_get::<AppState>)
                .head(handlers::hfapi::resolve_head::<AppState>),
        )
        // V1 stub: xet byte-serve pending gateway integration.
        .route(
            "/xet/files/{xet_hash}",
            get(handlers::hfapi::xet_file_serve::<AppState>),
        )
        .layer(cors)
        .with_state(state)
}
