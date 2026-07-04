//! `GET /admin/setup/status` — consolidated subsystem-probe endpoint.
//! Backs +. Admin-gated per (setup diagnostics
//! expose indexer URL + OAuth config presence — non-public info).
//! Returns a fixed-shape JSON body:
//! ```json
//! {
//! "postgres": {"status":"ok|degraded", "latency_ms": 2.1},
//! "redis": {"status":"ok|degraded", "latency_ms": 0.3},
//! "indexd": {"status":"ok|degraded", "latency_ms": 14.8,
//! "synced": true|false, "url": "http://indexd:9980"},
//! "github_oauth": {"configured": true|false},
//! "v2_reconstruction_enabled": false
//! }
//! ```
//! `v2_reconstruction_enabled` is READ-ONLY ( resolution); the
//! console renders it as a status tile, never as a toggle.
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
    fn indexd_admin_url(&self) -> &str;
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
    Session(_user): Session,
    State(st): State<S>,
) -> Result<Json<SetupStatusResponse>, AppError> {
    // Platform health is informational and surfaced on `/setup` for any
    // signed-in user — the page is the dashboard's "is the deployment
    // alive" panel. No secrets cross the wire (latencies + sync flag
    // only); admin-only would just hide a health card from end users.
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
    // state.rs binds indexd_url() to cfg.indexd_admin_url (port 9980)
    let url_base = st.indexd_admin_url().trim_end_matches('/').to_string();
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
            // indexd's `/api/state` surfaces `synced` at the top level
            // (verified live: `{"version":..., "synced": true, ...}`).
            // Earlier code expected `consensus.synced` — that path has
            // never existed and made the badge stuck at "no" / "syncing".
            match resp.json::<serde_json::Value>().await {
                Ok(json) => {
                    let synced = json.get("synced").and_then(|v| v.as_bool());
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

// ---------------------------------------------------------------------------
// `GET /api/platform/sia` — public Sia subsystem snapshot. Surfaces the
// renter wallet, current contracts, and the siascan.com explorer base so
// the console can prove "this deployment is actually pushing to Sia" with
// real on-chain links. Anonymous-readable: only summary aggregates and the
// public renter address cross the wire — no admin password, no host private
// keys, nothing the operator wouldn't put in a tweet.

#[derive(Debug, Serialize)]
pub struct ContractSummary {
    pub id: String,
    pub host_key: String,
    pub formation: Option<String>,
    pub size: u64,
    pub remaining_allowance: String,
}

#[derive(Debug, Serialize)]
pub struct PlatformSia {
    pub wallet_address: Option<String>,
    /// Spendable balance in raw hastings (Sia's atomic unit; 1 SC = 1e24 H).
    /// String to preserve precision past JS number range; UI formats to SC.
    pub wallet_spendable_hastings: Option<String>,
    pub wallet_immature_hastings: Option<String>,
    pub contract_count: i64,
    pub distinct_host_count: i64,
    pub contracts: Vec<ContractSummary>,
    pub siascan_base: &'static str,
    pub indexd_synced: Option<bool>,
}

/// siascan.com base URL. Zen testnet contracts/hosts/addresses do NOT
/// appear on mainnet siascan.com — they're only indexed by the testnet
/// instance at https://zen.siascan.com. We pick the right base from the
/// indexd `/api/state.network` field at boot time.
const SIASCAN_BASE_MAINNET: &str = "https://siascan.com";
const SIASCAN_BASE_ZEN: &str = "https://zen.siascan.com";

pub async fn platform_sia<S: SetupState>(
    State(st): State<S>,
) -> Result<Json<PlatformSia>, AppError> {
    let url_base = st.indexd_admin_url().trim_end_matches('/').to_string();
    let client = st.http_client();
    let pwd = st.indexd_admin_password();

    // Three best-effort indexd calls. Any failure → null fields, never 500
    // (we don't want a transient indexd hiccup to take down the dashboard).
    let wallet = client
        .get(format!("{url_base}/api/wallet"))
        .basic_auth("", Some(pwd))
        .timeout(PROBE_TIMEOUT)
        .send()
        .await
        .ok()
        .and_then(|r| if r.status().is_success() { Some(r) } else { None });
    let wallet_json: Option<serde_json::Value> = match wallet {
        Some(r) => r.json().await.ok(),
        None => None,
    };

    let contracts = client
        .get(format!("{url_base}/api/contracts"))
        .basic_auth("", Some(pwd))
        .timeout(PROBE_TIMEOUT)
        .send()
        .await
        .ok()
        .and_then(|r| if r.status().is_success() { Some(r) } else { None });
    let contracts_json: Vec<serde_json::Value> = match contracts {
        Some(r) => r.json().await.unwrap_or_default(),
        None => Vec::new(),
    };

    // /api/state — surfaces both `consensus.synced` (for the badge) and
    // `network` (for the explorer base — zen.siascan.com vs siascan.com).
    let state_resp = client
        .get(format!("{url_base}/api/state"))
        .basic_auth("", Some(pwd))
        .timeout(PROBE_TIMEOUT)
        .send()
        .await
        .ok();
    let state_json: Option<serde_json::Value> = match state_resp {
        Some(r) if r.status().is_success() => r.json().await.ok(),
        _ => None,
    };
    // `synced` is top-level on the `/api/state` response (NOT under
    // `consensus`). Mismatched path was returning null and stranding the
    // Setup tile + every "On Sia" badge in "syncing" forever.
    let synced_flag: Option<bool> = state_json
        .as_ref()
        .and_then(|v| v.get("synced"))
        .and_then(|v| v.as_bool());
    let network = state_json
        .as_ref()
        .and_then(|v| v.get("network"))
        .and_then(|v| v.as_str())
        .unwrap_or("mainnet");
    let siascan_base = if network == "zen" {
        SIASCAN_BASE_ZEN
    } else {
        SIASCAN_BASE_MAINNET
    };

    let mut distinct_hosts = std::collections::HashSet::<String>::new();
    let summaries: Vec<ContractSummary> = contracts_json
        .iter()
        .map(|c| {
            let host_key = c
                .get("hostKey")
                .and_then(|v| v.as_str())
                .unwrap_or("")
                .to_string();
            distinct_hosts.insert(host_key.clone());
            ContractSummary {
                id: c
                    .get("id")
                    .and_then(|v| v.as_str())
                    .unwrap_or("")
                    .to_string(),
                host_key,
                formation: c
                    .get("formation")
                    .and_then(|v| v.as_str())
                    .map(str::to_string),
                size: c.get("size").and_then(|v| v.as_u64()).unwrap_or(0),
                remaining_allowance: c
                    .get("remainingAllowance")
                    .and_then(|v| v.as_str())
                    .unwrap_or("0")
                    .to_string(),
            }
        })
        .collect();

    Ok(Json(PlatformSia {
        wallet_address: wallet_json
            .as_ref()
            .and_then(|v| v.get("address"))
            .and_then(|v| v.as_str())
            .map(str::to_string),
        wallet_spendable_hastings: wallet_json
            .as_ref()
            .and_then(|v| v.get("spendable"))
            .and_then(|v| v.as_str())
            .map(str::to_string),
        wallet_immature_hastings: wallet_json
            .as_ref()
            .and_then(|v| v.get("immature"))
            .and_then(|v| v.as_str())
            .map(str::to_string),
        contract_count: contracts_json.len() as i64,
        distinct_host_count: distinct_hosts.len() as i64,
        contracts: summaries,
        siascan_base,
        indexd_synced: synced_flag,
    }))
}
