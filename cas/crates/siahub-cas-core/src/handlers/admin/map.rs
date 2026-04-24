//! `GET /admin/stats/map` — proxy indexd `/api/hosts?usable=true` and reshape
//! for the console's Leaflet map.
//! Backs . Keeps the browser off indexd entirely ( §C
//! invariant — "browser never touches indexd directly"): CAS is the one hop
//! that holds the `INDEXD_ADMIN_PASSWORD` and can reshape the upstream JSON
//! into a stable console contract.
//! Upstream (probed on live compose stack): indexd serves JSON with
//! Go-marshal default field names (`PascalCase`). The struct below serde-
//! renames to tolerate the exact field names observed (`PublicKey`,
//! `CountryCode`, `Latitude`, `Longitude`, `Usable`, `ContractCount`).
//! Failure modes:
//! * 502 `{"code":"indexd_unreachable"}` on reqwest failure — helps the
//! self-host operator distinguish "SIAHUB_INDEXER_URL misconfigured"
//! from "no hosts found".
//! * 200 with `{hosts: []}` on no usable hosts (Zen-testnet scarcity
//! scenario documented in 01-RESULTS).

use std::sync::Arc;
use std::time::Duration;

use axum::Json;
use axum::extract::State;
use axum::http::StatusCode;
use axum::response::IntoResponse;
use serde::{Deserialize, Serialize};

use crate::auth::AuthStateRef;
use crate::errors::AppError;
use crate::session::Session;

/// State trait — mirrors the pattern used by other handlers so the binary
/// crate's `AppState` can hand over the indexd URL + admin password without
/// this crate depending on the concrete config type.
pub trait MapState: AuthStateRef {
    /// Base URL of the self-hosted indexd instance, e.g. `http://indexd:9980`.
    fn indexd_url(&self) -> &str;
    /// HTTP Basic auth password for indexd's admin endpoints.
    fn indexd_admin_password(&self) -> &str;
    /// Shared reqwest client — built once at boot; DO NOT rebuild per req.
    fn http_client(&self) -> Arc<reqwest::Client>;
}

/// Upstream shape — indexd `/api/hosts` response items.
#[derive(Debug, Deserialize)]
struct IndexdHost {
    #[serde(rename = "PublicKey")]
    pub public_key: String,
    #[serde(rename = "CountryCode", default)]
    pub country_code: Option<String>,
    #[serde(rename = "Latitude", default)]
    pub latitude: Option<f64>,
    #[serde(rename = "Longitude", default)]
    pub longitude: Option<f64>,
    #[serde(rename = "Usable", default)]
    pub usable: bool,
    #[serde(rename = "ContractCount", default)]
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
/// public data the map page needs even for non-admin users).
pub async fn get_map<S: MapState>(
    Session(_user): Session,
    State(st): State<S>,
) -> Result<Json<MapResponse>, AppError> {
    let url = format!(
        "{}/api/hosts?usable=true",
        st.indexd_url().trim_end_matches('/')
    );

    let resp = st
        .http_client()
        .get(&url)
        .basic_auth("", Some(st.indexd_admin_password()))
        .timeout(Duration::from_secs(5))
        .send()
        .await
        .map_err(|e| {
            tracing::warn!(err = %e, %url, "indexd /api/hosts unreachable");
            IndexdError::Unreachable
        })?;

    if !resp.status().is_success() {
        tracing::warn!(status = %resp.status(), "indexd /api/hosts returned non-2xx");
        return Err(IndexdError::Unreachable.into());
    }

    let upstream: Vec<IndexdHost> = resp.json().await.map_err(|e| {
        tracing::warn!(err = %e, "indexd /api/hosts body did not deserialize");
        IndexdError::Unreachable
    })?;

    let hosts = upstream
        .into_iter()
        .filter_map(|h| {
            // Only surface hosts we can actually plot. Null coords are
            // common on freshly-announced hosts before GeoIP lookup
            // completes.
            let (Some(lat), Some(lon)) = (h.latitude, h.longitude) else {
                return None;
            };
            if !h.usable {
                return None;
            }
            Some(MapHost {
                public_key: h.public_key,
                country_code: h.country_code,
                lat,
                lon,
                usable: h.usable,
                contract_count: h.contract_count,
            })
        })
        .collect();

    Ok(Json(MapResponse { hosts }))
}

/// Local error alias so the handler can map upstream-failure into a single
/// stable `{"code":"indexd_unreachable"}` JSON body. Does NOT flow through
/// `AppError::Other` because that collapses to the generic `"internal"`
/// body.
#[derive(Debug)]
pub enum IndexdError {
    Unreachable,
}

impl From<IndexdError> for AppError {
    fn from(_e: IndexdError) -> Self {
        // Stash an anyhow Error with a stable code string; `IntoResponse`
        // below overrides the final body.
        AppError::Other(anyhow::anyhow!("indexd_unreachable"))
    }
}

impl IntoResponse for IndexdError {
    fn into_response(self) -> axum::response::Response {
        (
            StatusCode::BAD_GATEWAY,
            Json(serde_json::json!({ "code": "indexd_unreachable" })),
        )
            .into_response()
    }
}
