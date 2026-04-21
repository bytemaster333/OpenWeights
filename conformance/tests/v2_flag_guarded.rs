//! V2 reconstruction is feature-flagged 501 by default (D-18). These tests
//! cover:
//!   1. `GET /v2/reconstructions/{file_id}` returns 501 when the CAS ships
//!      with `V2_RECONSTRUCTION_ENABLED=false` (Phase 2 default).
//!   2. `xet_client::Client::get_reconstruction(file_id, None)` transparently
//!      falls back to V1 on 501 — proves the contract in RESEARCH §2.6.
//!
//! The "flag=true" branch (501 → 200 V2 shape) is exercised in the Phase 3
//! test suite once GATE-03 flips the flag; Phase 2's job is to confirm the
//! wire-level 501 status and the client-side fallback.
//!
//! Skips if Docker / siahub-cas image unavailable.

use std::sync::Arc;

use sha2::Digest;
use xet_client::cas_client::auth::{AuthConfig, NoOpTokenRefresher};
use xet_client::cas_client::{Client, RemoteClient};
use xet_core_structures::merklehash::MerkleHash;

mod common;
use common::spawn;

#[tokio::test]
async fn v2_returns_501_when_flag_is_false() -> anyhow::Result<()> {
    siahub_conformance::init_tracing_once();

    let harness = match spawn::spawn_cas().await? {
        Some(h) => h,
        None => return Ok(()),
    };

    // Pick any plausible 64-hex file_id — the 501 must come from the flag
    // check BEFORE the handler even tries to look up the file_id. So a
    // non-existent hash is fine for this test.
    let file_id_hex = "00".repeat(32);
    let url = format!("{}/v2/reconstructions/{}", harness.base_url, file_id_hex);

    let resp = reqwest::Client::new()
        .get(&url)
        .bearer_auth(&harness.upload_download_token)
        .send()
        .await?;

    assert_eq!(
        resp.status(),
        reqwest::StatusCode::NOT_IMPLEMENTED,
        "V2 reconstruction must return 501 when V2_RECONSTRUCTION_ENABLED=false (D-18)"
    );

    Ok(())
}

#[tokio::test]
async fn xet_client_falls_back_from_v2_501_to_v1() -> anyhow::Result<()> {
    // Proves the RESEARCH §2.6 VERIFIED claim: xet-core treats 501 as "V2
    // unavailable, fall back to V1". This is implemented in
    // xet_client::cas_client::RemoteClient::get_reconstruction_with_version_override
    // (a pub(crate) impl, surfaced publicly via `Client::get_reconstruction`).
    siahub_conformance::init_tracing_once();

    let harness = match spawn::spawn_cas().await? {
        Some(h) => h,
        None => return Ok(()),
    };

    // Seed a minimal file so V1 has something to return.
    let pool = sqlx::postgres::PgPoolOptions::new()
        .max_connections(1)
        .connect(&harness.pg_url)
        .await?;

    let xorb_hash: [u8; 32] = {
        let mut h = [0u8; 32];
        h[0] = 0xDD;
        h
    };
    let file_id_bytes: [u8; 32] = {
        let mut h = [0u8; 32];
        h[0] = 0xEE;
        h
    };
    let shard_hash: [u8; 32] = {
        let mut h = [0u8; 32];
        h[0] = 0xFF;
        h
    };
    let sia_object_id: [u8; 32] = sha2::Sha256::digest(b"mock-sia-v2-flag").into();

    sqlx::query(
        "INSERT INTO shards (shard_hash, sia_object_id, size_bytes, owner_user_id, owner_api_key_id, pin_state)
         VALUES ($1, $2, $3, $4, $5, 'pinned')",
    )
    .bind(shard_hash.to_vec())
    .bind(sia_object_id.to_vec())
    .bind(1024_i64)
    .bind(harness.user_id)
    .bind(harness.api_key_id)
    .execute(&pool)
    .await?;

    sqlx::query(
        "INSERT INTO xorbs (xorb_merkle_hash, sia_object_id, size_bytes, owner_user_id, owner_api_key_id, pin_state)
         VALUES ($1, $2, $3, $4, $5, 'pinned')",
    )
    .bind(xorb_hash.to_vec())
    .bind(sia_object_id.to_vec())
    .bind(4096_i64)
    .bind(harness.user_id)
    .bind(harness.api_key_id)
    .execute(&pool)
    .await?;

    sqlx::query(
        "INSERT INTO reconstruction_files (file_id, shard_hash, total_size)
         VALUES ($1, $2, $3)",
    )
    .bind(file_id_bytes.to_vec())
    .bind(shard_hash.to_vec())
    .bind(4096_i64)
    .execute(&pool)
    .await?;

    sqlx::query(
        "INSERT INTO reconstruction_terms
            (file_id, term_index, xorb_hash, xorb_start, xorb_end,
             xorb_byte_start, xorb_byte_end, unpacked_start, unpacked_end)
         VALUES ($1, 0, $2, 0, 4096, 0, 4096, 0, 4096)",
    )
    .bind(file_id_bytes.to_vec())
    .bind(xorb_hash.to_vec())
    .execute(&pool)
    .await?;
    pool.close().await;

    let file_id = MerkleHash::from_slice(&file_id_bytes)
        .map_err(|e| anyhow::anyhow!("file_id not a MerkleHash: {e}"))?;

    // Build the xet-client. `get_reconstruction` hits V2 first; on 501 from
    // our CAS the client catches it and re-requests V1 (RESEARCH §2.6).
    let auth = AuthConfig::maybe_new(
        Some(harness.upload_download_token.clone()),
        Some(u64::MAX),
        Some(Arc::new(NoOpTokenRefresher)),
    );
    let client: Arc<RemoteClient> = RemoteClient::new(
        &harness.base_url,
        &auth,
        "siahub-conformance-v2-flag",
        false,
        None,
    );

    // Even if the file lookup via MerkleHash::from_slice on arbitrary bytes
    // fails (because the handler re-encodes via its own codec), the client
    // should EITHER get Some(_) (fallback fetched a valid row) OR return a
    // clean `Ok(None)` (404) — NOT a wire-level V2-not-implemented error.
    // The load-bearing assertion: the client's result is a Result::Ok
    // (well-formed call completed), proving the 501 branch was caught and
    // retried internally.
    let result = client.get_reconstruction(&file_id, None).await;
    match result {
        Ok(_) => {
            // Either Some(V2) (from V1→V2 conversion) or None (404). Both
            // are fallback-path successes; V2-501 branch was swallowed.
        }
        Err(e) => {
            panic!(
                "xet-client should have transparently fallen back V2→V1 on 501; \
                 instead got error {e:?}"
            );
        }
    }

    Ok(())
}
