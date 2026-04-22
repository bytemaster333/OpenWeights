//! `GET /admin/setup/status` — consolidated subsystem-probe endpoint (D-53).
//!
//! Backs CONSOLE-11 + CONSOLE-12. Admin-gated per D-53 (setup diagnostics
//! expose indexer URL + OAuth config presence — non-public info).
//!
//! Returns a fixed-shape JSON body:
//!
//! ```json
//! {
//!   "postgres":    {"status":"ok|degraded", "latency_ms": 2.1},
//!   "redis":       {"status":"ok|degraded", "latency_ms": 0.3},
//!   "indexd":      {"status":"ok|degraded", "latency_ms": 14.8,
//!                   "synced": true|false, "url": "http://indexd:9980"},
//!   "github_oauth": {"configured": true|false},
//!   "v2_reconstruction_enabled": false
//! }
//! ```
//!
//! `v2_reconstruction_enabled` is READ-ONLY (Ambiguity 3 resolution); the
//! console renders it as a status tile, never as a toggle.
//!
//! The endpoint itself returns 200 even if a subsystem is degraded — the
//! `status` field signals per-subsystem health so the UI can render partial
//! readiness without hiding working services.

use std::sync::Arc;
use std::time::{Duration, Instant};

use axum::Json;
use axum::extract::State;
use fred::interfaces::ClientLike;
use serde::Serialize;

use crate::auth::AuthStateRef;
use crate::errors::AppError;
use crate::session::Session;

/// Extension trait: the binary crate's `AppState` implements this so the
/// handler can reach the subsystem config without depending on the concrete
/// type. Mirrors `MapState` for the same reason.
pub trait SetupState: AuthStateRef {
    fn redis(&self) -> Arc<fred::clients::Client>;
    fn indexd_url(&self) -> &str;
    fn indexd_admin_password(&self) -> &str;
    fn http_client(&self) -> Arc<reqwest::Client>;
    fn github_oauth_configured(&self) -> bool;
    fn v2_reconstruction_enabled(&self) -> bool;
}

#[derive(Debug, Serialize)]
pub struct ProbeStatus {
    pub status: &'static str,
    pub latency_ms: f64,
}

#[derive(Debug, Serialize)]
pub struct IndexdStatus {
    pub status: &'static str,
    pub latency_ms: f64,
    pub synced: Option<bool>,
    pub url: String,
}

#[derive(Debug, Serialize)]
pub struct OAuthStatus {
    pub configured: bool,
}

#[derive(Debug, Serialize)]
pub struct SetupStatusResponse {
    pub postgres: ProbeStatus,
    pub redis: ProbeStatus,
    pub indexd: IndexdStatus,
    pub github_oauth: OAuthStatus,
    pub v2_reconstruction_enabled: bool,
}

/// Bounded probe timeout for every subsystem check — keeps the aggregate
/// endpoint below ~2 s even if one subsystem is dead.
const PROBE_TIMEOUT: Duration = Duration::from_secs(2);

pub async fn get_setup_status<S: SetupState>(
    Session(user): Session,
    State(st): State<S>,
) -> Result<Json<SetupStatusResponse>, AppError> {
    if !user.is_admin {
        return Err(AppError::Forbidden);
    }

    let postgres = probe_postgres(&st).await;
    let redis = probe_redis(&st).await;
    let indexd = probe_indexd(&st).await;

    let response = SetupStatusResponse {
        postgres,
        redis,
        indexd,
        github_oauth: OAuthStatus {
            configured: st.github_oauth_configured(),
        },
        v2_reconstruction_enabled: st.v2_reconstruction_enabled(),
    };
    Ok(Json(response))
}

async fn probe_postgres<S: SetupState>(st: &S) -> ProbeStatus {
    let start = Instant::now();
    let res = tokio::time::timeout(
        PROBE_TIMEOUT,
        sqlx::query("SELECT 1").execute(st.pool()),
    )
    .await;
    let latency_ms = elapsed_ms(start);
    match res {
        Ok(Ok(_)) => ProbeStatus {
            status: "ok",
            latency_ms,
        },
        _ => ProbeStatus {
            status: "degraded",
            latency_ms,
        },
    }
}

async fn probe_redis<S: SetupState>(st: &S) -> ProbeStatus {
    let start = Instant::now();
    let client = st.redis();
    let res = tokio::time::timeout(PROBE_TIMEOUT, client.ping::<()>(None)).await;
    let latency_ms = elapsed_ms(start);
    match res {
        Ok(Ok(_)) => ProbeStatus {
            status: "ok",
            latency_ms,
        },
        _ => ProbeStatus {
            status: "degraded",
            latency_ms,
        },
    }
}

async fn probe_indexd<S: SetupState>(st: &S) -> IndexdStatus {
    let url_base = st.indexd_url().trim_end_matches('/').to_string();
    let state_url = format!("{}/api/state", url_base);
    let start = Instant::now();
    let req = st
        .http_client()
        .get(&state_url)
        .basic_auth("", Some(st.indexd_admin_password()))
        .timeout(PROBE_TIMEOUT);
    let result = req.send().await;
    let latency_ms = elapsed_ms(start);

    let (status, synced) = match result {
        Ok(resp) if resp.status().is_success() => {
            // Best-effort parse of `{"consensus":{"synced":bool}, ...}`.
            match resp.json::<serde_json::Value>().await {
                Ok(json) => {
                    let synced = json
                        .get("consensus")
                        .and_then(|c| c.get("synced"))
                        .and_then(|v| v.as_bool());
                    ("ok", synced)
                }
                Err(_) => ("ok", None),
            }
        }
        _ => ("degraded", None),
    };

    IndexdStatus {
        status,
        latency_ms,
        synced,
        url: url_base,
    }
}

fn elapsed_ms(start: Instant) -> f64 {
    let d = start.elapsed();
    (d.as_secs() as f64 * 1000.0) + (d.subsec_nanos() as f64 / 1_000_000.0)
}
