//! End-to-end round-trip driven by `xet_client::cas_client::RemoteClient`
//! at the pinned `=1.5.1` version.
//! Preconditions (test skips gracefully if any is missing):
//! - Docker daemon reachable.
//! - `siahub-cas` image present in the local daemon (built via `make cas-image`).
//! - Reference fixtures cloned to `conformance/fixtures/` (via `make conformance-fixtures`
//! or the `git lfs clone` in `conformance/fixtures/README.md`).
//! What this test exercises at conformance-grade:
//! 1. `xet_client` builds a request, hits `POST /v1/xorbs/{prefix}/{hash}`,
//! our CAS responds with `{"was_inserted": true}` which the client
//! internally swallows.
//! 2. `xet_client::upload_shard` hits `POST /shards` (NO `/v1` — confirms the
//!.md dual-path router is the ONLY way this works).
//! 3. `Client::get_reconstruction(file_id, None)` — V2-first with V1
//! fallback on 501 (the V2_RECONSTRUCTION_ENABLED=false path).
//! Returns `Option<QueryReconstructionResponseV2>` — Some-on-known-file.
//! If xet-client API quirks (A3 probe YELLOW — SerializedXorbObject must be
//! hand-assembled from xorb fixture bytes) prevent full upload via the
//! client, the test falls back to raw `reqwest::Client` for that leg only
//! and still validates the wire contract. Both paths are noted in SUMMARY.

use std::sync::Arc;

use siahub_conformance::fixtures;

use xet_client::cas_client::auth::{AuthConfig, NoOpTokenRefresher};
use xet_client::cas_client::{Client, RemoteClient};
use xet_core_structures::merklehash::MerkleHash;

mod common;
use common::{schema_check, spawn};

/// The hero test. Single orchestrated round-trip: Postgres + CAS up → seed
/// key → upload xorb (via raw reqwest, see note below) → upload shard via
/// xet-client → query reconstruction via xet-client.
#[tokio::test]
async fn round_trip_reference_xorb_shard_file() -> anyhow::Result<()> {
    siahub_conformance::init_tracing_once();

    // ----------------------------------------------------------------
    // Fixture precondition — skip if LFS blobs are missing.
    // ----------------------------------------------------------------
    let (xorb_bytes, xorb_hash_hex) = match fixtures::load_reference_xorb() {
        Ok(v) => v,
        Err(e) if fixtures::is_skip(&e) => {
            eprintln!("SKIP round_trip_reference_xorb_shard_file: {e}");
            return Ok(());
        }
        Err(e) => return Err(anyhow::anyhow!("fixture load failed: {e}")),
    };
    let shard_bytes = match fixtures::load_reference_shard() {
        Ok(v) => v,
        Err(e) if fixtures::is_skip(&e) => {
            eprintln!("SKIP round_trip_reference_xorb_shard_file: {e}");
            return Ok(());
        }
        Err(e) => return Err(anyhow::anyhow!("shard fixture load failed: {e}")),
    };
    let _xorb_hash = MerkleHash::from_hex(&xorb_hash_hex)?;

    // ----------------------------------------------------------------
    // Container precondition — skip if Docker/image unavailable.
    // ----------------------------------------------------------------
    let harness = match spawn::spawn_cas().await? {
        Some(h) => h,
        None => return Ok(()),
    };

    // ----------------------------------------------------------------
    // Build the xet_client against our CAS.
    // ----------------------------------------------------------------
    let auth = AuthConfig::maybe_new(
        Some(harness.upload_download_token.clone()),
        Some(u64::MAX),
        Some(Arc::new(NoOpTokenRefresher)),
    );
    let client: Arc<RemoteClient> = RemoteClient::new(
        &harness.base_url,
        &auth,
        "siahub-conformance",
        false,
        None,
    );

    // ----------------------------------------------------------------
    // Step 1 — Upload the reference xorb.
    // A3 probe YELLOW `xet_client::Client::upload_xorb` takes a
    // `SerializedXorbObject`, not raw bytes. Constructing one validly from
    // the fixture requires chunker-level work. For the conformance
    // surface this is overkill — we drive the wire protocol via raw reqwest
    // for this leg, proving our CAS accepts xet-core's canonical xorb format
    // byte-identically. The OTHER xet-client calls (upload_shard,
    // get_reconstruction) exercise the client-side code paths fully.
    // If future work surfaces `SerializedXorbObject::from_raw_bytes`, this
    // leg can re-route through the client as well.
    // ----------------------------------------------------------------
    let prefix = &xorb_hash_hex[..2];
    let reqwest_client = reqwest::Client::new();
    let xorb_url = format!("{}/v1/xorbs/{}/{}", harness.base_url, prefix, xorb_hash_hex);
    let first_upload = reqwest_client
        .post(&xorb_url)
        .bearer_auth(&harness.upload_download_token)
        .body(xorb_bytes.clone())
        .send()
        .await?;
    let first_status = first_upload.status();
    assert_eq!(first_status, reqwest::StatusCode::OK, "first xorb upload");
    let first_body: serde_json::Value = first_upload.json().await?;
    schema_check::assert_valid_upload_xorb(&first_body)?;
    assert_eq!(first_body["was_inserted"], serde_json::json!(true));

    // Dedup check — second upload must return was_inserted=false.
    let second_upload = reqwest_client
        .post(&xorb_url)
        .bearer_auth(&harness.upload_download_token)
        .body(xorb_bytes.clone())
        .send()
        .await?;
    assert_eq!(second_upload.status(), reqwest::StatusCode::OK);
    let second_body: serde_json::Value = second_upload.json().await?;
    schema_check::assert_valid_upload_xorb(&second_body)?;
    assert_eq!(
        second_body["was_inserted"],
        serde_json::json!(false),
        "dedup (OQ-F): second upload must report was_inserted=false"
    );

    // ----------------------------------------------------------------
    // Step 2 — Upload the reference shard VIA xet-client.
    // This exercises `Client::upload_shard` which hits POST /shards (NO /v1
    // prefix) — the load-bearing call that proves.md 's
    // dual-path router works against a real xet-core client.
    // ----------------------------------------------------------------
    let upload_permit = client.acquire_upload_permit().await?;
    let shard_sync_performed: bool = client
        .upload_shard(bytes::Bytes::from(shard_bytes.clone()), upload_permit)
        .await?;
    assert!(
        shard_sync_performed,
        "first shard upload must be SyncPerformed, not Exists"
    );

    // Dedup check — second upload is Exists (returns false).
    let upload_permit = client.acquire_upload_permit().await?;
    let shard_dedup: bool = client
        .upload_shard(bytes::Bytes::from(shard_bytes.clone()), upload_permit)
        .await?;
    assert!(
        !shard_dedup,
        "second shard upload must be Exists (returns false)"
    );

    // ----------------------------------------------------------------
    // Step 3 — Query reconstruction VIA xet-client (the Client::get_reconstruction
    // path which does V2-first with V1 fallback on 501 — 's CAS
    // ships V2 disabled by default, so this is an implicit fallback drill).
    // We look up the shard's top-level file_id. 's parser extracts
    // file ids into the `reconstruction_files` table during POST /shards.
    // Rather than parse the shard format in the test, we enumerate the DB
    // to find any file_id and query it; absence ⇒ the shard was accepted
    // but produced no reconstruction_files rows, which is a CAS bug
    // (fail loudly).
    // ----------------------------------------------------------------
    let pool = sqlx::postgres::PgPoolOptions::new()
        .max_connections(1)
        .connect(&harness.pg_url)
        .await?;
    let file_row: Option<(Vec<u8>,)> = sqlx::query_as(
        "SELECT file_merkle_hash FROM reconstruction_files LIMIT 1",
    )
    .fetch_optional(&pool)
    .await?;
    pool.close().await;

    let file_hash_bytes = file_row.ok_or_else(|| {
        anyhow::anyhow!("no reconstruction_files row found after shard upload")
    })?.0;
    let file_id = MerkleHash::from_slice(&file_hash_bytes)
        .map_err(|e| anyhow::anyhow!("file_id bytes do not form a MerkleHash: {e}"))?;

    let recon = client
        .get_reconstruction(&file_id, None)
        .await
        .map_err(|e| anyhow::anyhow!("get_reconstruction failed: {e}"))?;
    let recon = recon.ok_or_else(|| anyhow::anyhow!("file_id unknown to CAS"))?;

    assert!(!recon.terms.is_empty(), "reconstruction has ≥1 term");
    assert!(!recon.xorbs.is_empty(), "reconstruction has ≥1 xorb fetch-info entry");
    // Schema round-trip — structural compat with xet_client::cas_types.
    let recon_json = serde_json::to_value(&recon)?;
    schema_check::assert_valid_query_reconstruction_v2(&recon_json)?;

    Ok(())
}

/// Probe: assert the xet_client::cas_client public surface our round-trip
/// depends on is reachable. Compile-only; no network. Serves as a canary
/// against future xet-client version bumps drifting the API.
#[test]
fn xet_client_surface_probe() {
    // These imports MUST compile for the conformance crate to do its job.
    // Keeping them in a test function instead of top-of-file pins the
    // guarantee that each symbol exists at `=1.5.1`.
    #[allow(unused_imports)]
    use xet_client::cas_client::{
        Client, RemoteClient,
        auth::{AuthConfig, NoOpTokenRefresher, TokenRefresher},
    };
    #[allow(unused_imports)]
    use xet_client::cas_types::{
        BatchQueryReconstructionResponse, QueryReconstructionResponse,
        QueryReconstructionResponseV2, UploadShardResponse, UploadShardResponseType,
        UploadXorbResponse,
    };
    #[allow(unused_imports)]
    use xet_core_structures::merklehash::MerkleHash;
    #[allow(unused_imports)]
    use xet_core_structures::xorb_object::SerializedXorbObject;
}
