//! `GET /admin/stats/map` — host topology for the console's Leaflet map.
//! The data source is indexd's ADMIN api (`/api/hosts`), which only a
//! self-hosted indexd exposes. A hosted indexer (e.g. https://sia.storage)
//! has no admin api, so the map degrades to an empty list — never an error.
//! Keeps the browser off indexd entirely: CAS is the one hop that holds the
//! `INDEXD_ADMIN_PASSWORD` and reshapes the upstream JSON into a stable
//! console contract.
//! Behaviour:
//! * `INDEXD_ADMIN_URL` empty (hosted indexer) → `200 {hosts: []}` with no
//!   network call.
//! * self-hosted indexd reachable → `200 {hosts: [...]}`.
//! * self-hosted indexd set but unreachable / bad response → `200 {hosts: []}`
//!   (logged), so the panel shows "no hosts" instead of a hard error.

use std::sync::Arc;
use std::time::Duration;

use axum::Json;
use axum::extract::State;
use serde::{Deserialize, Serialize};

use crate::auth::AuthStateRef;
use crate::session::Session;

/// State trait — mirrors the pattern used by other handlers so the binary
/// crate's `AppState` can hand over the indexd URL + admin password without
/// this crate depending on the concrete config type.
pub trait MapState: AuthStateRef {
    /// Admin-api base URL of a self-hosted indexd, e.g. `http://indexd:9980`.
    /// Empty when the operator uses a hosted indexer with no admin api.
    fn indexd_admin_url(&self) -> &str;
    /// HTTP Basic auth password for indexd's admin endpoints.
    fn indexd_admin_password(&self) -> &str;
    /// Shared reqwest client — built once at boot; DO NOT rebuild per req.
    fn http_client(&self) -> Arc<reqwest::Client>;
}

/// upstream shape — indexd /api/hosts response items (camelCase json)
#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase")]
struct IndexdHost {
    pub public_key: String,
    #[serde(default)]
    pub country_code: Option<String>,
    #[serde(default)]
    pub latitude: Option<f64>,
    #[serde(default)]
    pub longitude: Option<f64>,
    #[serde(default)]
    pub contract_count: Option<u32>,
}

#[derive(Debug, Serialize)]
pub struct MapHost {
    pub public_key: String,
    pub country_code: Option<String>,
    pub lat: f64,
    pub lon: f64,
    pub usable: bool,
    pub contract_count: Option<u32>,
}

#[derive(Debug, Serialize)]
pub struct MapResponse {
    pub hosts: Vec<MapHost>,
}

/// `GET /admin/stats/map` — session-gated (no admin flag — host topology is
/// public data the map page needs even for non-admin users). Infallible:
/// any missing/unreachable admin api yields `200 {hosts: []}`.
pub async fn get_map<S: MapState>(Session(_user): Session, State(st): State<S>) -> Json<MapResponse> {
    let admin = st.indexd_admin_url().trim_end_matches('/');
    // hosted indexer (no admin api) → empty map, no network call.
    if admin.is_empty() {
        return Json(MapResponse { hosts: Vec::new() });
    }
    let hosts = fetch_map_hosts(&st, admin).await.unwrap_or_else(|e| {
        tracing::warn!(err = %e, "indexd host map unavailable — returning empty");
        Vec::new()
    });
    Json(MapResponse { hosts })
}

/// Fetch + reshape the self-hosted indexd host topology. Any failure bubbles
/// up as an error the caller swallows into an empty map.
async fn fetch_map_hosts<S: MapState>(st: &S, admin: &str) -> anyhow::Result<Vec<MapHost>> {
    let url = format!("{admin}/api/hosts?usable=true");

    let resp = st
        .http_client()
        .get(&url)
        .basic_auth("", Some(st.indexd_admin_password()))
        .timeout(Duration::from_secs(5))
        .send()
        .await?;

    if !resp.status().is_success() {
        anyhow::bail!("indexd /api/hosts returned {}", resp.status());
    }

    let upstream: Vec<IndexdHost> = resp.json().await?;

    // indexd /api/hosts does NOT return a per-host `contractCount`. Fetch
    // the contracts list separately and group by hostKey so the map UI
    // can render a real number instead of always-0. Best-effort: any
    // failure here leaves contract_count as None, not a 5xx.
    let contracts_url = format!(
        "{}/api/contracts",
        st.indexd_admin_url().trim_end_matches('/')
    );
    let contracts_by_host: std::collections::HashMap<String, u32> = match st
        .http_client()
        .get(&contracts_url)
        .basic_auth("", Some(st.indexd_admin_password()))
        .timeout(Duration::from_secs(5))
        .send()
        .await
    {
        Ok(r) if r.status().is_success() => match r.json::<Vec<serde_json::Value>>().await {
            Ok(list) => {
                let mut m = std::collections::HashMap::new();
                for c in list {
                    if let Some(hk) = c.get("hostKey").and_then(|v| v.as_str()) {
                        *m.entry(hk.to_string()).or_insert(0) += 1;
                    }
                }
                m
            }
            Err(_) => std::collections::HashMap::new(),
        },
        _ => std::collections::HashMap::new(),
    };

    let hosts = upstream
        .into_iter()
        .filter_map(|h| {
            // only surface hosts we can plot. null coords are common on
            // freshly-announced hosts before geoip lookup completes.
            // note: we already query ?usable=true upstream, so every
            // returned host is usable — treat h.usable as true regardless
            // of whether the field was present in the response.
            let (Some(lat), Some(lon)) = (h.latitude, h.longitude) else {
                return None;
            };
            let cc = contracts_by_host.get(&h.public_key).copied();
            Some(MapHost {
                public_key: h.public_key,
                country_code: h.country_code,
                lat,
                lon,
                usable: true,
                contract_count: cc.or(h.contract_count),
            })
        })
        .collect();

    Ok(hosts)
}
