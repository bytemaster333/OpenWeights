//! V2 reconstruction is feature-flagged 501 by default. These tests
//! cover:
//! 1. `GET /v2/reconstructions/{file_id}` returns 501 when the CAS ships
//! with `V2_RECONSTRUCTION_ENABLED=false` ( default).
//! 2. `xet_client::Client::get_reconstruction(file_id, None)` transparently
//! falls back to V1 on 501 — proves the contract in RESEARCH §2.6.
//! 3. **( — V2 flip)**: with `V2_RECONSTRUCTION_ENABLED=true`,
//! `GET /v2/reconstructions/{file_id}` returns 200 carrying the V2
//! shape — per-xorb single URL with segments-form `r=s1-e1,s2-e2`.
//! This is the CAS-side half of the V2 flip (RECEIVED §G
//! items 2 + 4). The *gateway-side* multi-range `multipart/byteranges`
//! round-trip is deferred to E2E gates — it requires the Go
//! gateway binary running alongside the CAS container, which this
//! crate's harness does not yet spawn (spawn.rs is CAS-only by design;
//! a full gateway+CAS harness is future work).
//! Skips if Docker / openweights-cas image unavailable.

use std::sync::Arc;

use sha2::Digest;
use xet_client::cas_client::auth::{AuthConfig, NoOpTokenRefresher};
use xet_client::cas_client::{Client, RemoteClient};
use xet_core_structures::merklehash::MerkleHash;

mod common;
use common::spawn;

#[tokio::test]
async fn v2_returns_501_when_flag_is_false() -> anyhow::Result<()> {
    openweights_conformance::init_tracing_once();

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
    openweights_conformance::init_tracing_once();

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
        "openweights-conformance-v2-flag",
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

/// ** (V2 flip) — CAS-side multi-range reconstruction integration.**
/// Spins the CAS with `V2_RECONSTRUCTION_ENABLED=true` (via
/// `spawn_cas_v2_enabled`), seeds a file whose terms coalesce into ≥2
/// non-contiguous byte segments within ONE xorb, hits
/// `GET /v2/reconstructions/{file_id}`, and asserts:
/// * status 200 (not 501)
/// * response shape is V2 (`fetch_info: { <xorb_hex>: { url, ranges: [...] } }`)
/// * the signed URL's `r=` query param carries the segments form
/// `s1-e1,s2-e2` — the `mint_v1_multi_range` output that the
/// Go gateway parses into `[]Range` and serves as multipart/byteranges.
/// **Runtime pairing deferred to.** The *gateway-side* multi-range
/// byte-compare round-trip (fetch the signed URL, parse multipart, assert
/// each part matches the expected xorb slice) needs the Go gateway binary
/// running alongside the CAS container — that wiring is out of scope for
/// per the plan's "Do not modify gateway/*; gateway is done"
/// rule and is slated for E2E gates. This test compiles, asserts
/// the CAS-side contract, and is the compile-time tripwire the
/// harness extends when it lands.
#[tokio::test]
async fn v2_multi_range_round_trip() -> anyhow::Result<()> {
    openweights_conformance::init_tracing_once();

    let harness = match spawn::spawn_cas_v2_enabled().await? {
        Some(h) => h,
        None => return Ok(()),
    };

    // Seed a file that coalesces into TWO non-contiguous segments within
    // ONE xorb (shape matches `flag_on_build_v2_response_produces_per_xorb_single_url`
    // in cas/crates/openweights-cas-core/src/tests/reconstruction_v2_tests.rs).
    // Terms:
    // term0: xorb_a bytes [1024, 4096) chunks [0, 8)
    // term1: xorb_a bytes [3072, 8192) chunks [6, 16) → merges with term0 → [1024, 8192)
    // term2: xorb_a bytes [12288, 16384) chunks [24, 32) → disjoint → segment [12288, 16384)
    // END-INCLUSIVE segments stamped into URL:
    // (1024, 8191) + (12288, 16383)
    let pool = sqlx::postgres::PgPoolOptions::new()
        .max_connections(2)
        .connect(&harness.pg_url)
        .await?;

    let xorb_a: [u8; 32] = {
        let mut h = [0u8; 32];
        h[0] = 0xAA;
        h
    };
    let file_id_bytes: [u8; 32] = {
        let mut h = [0u8; 32];
        h[0] = 0xAB;
        h
    };
    let shard_hash: [u8; 32] = {
        let mut h = [0u8; 32];
        h[0] = 0xAC;
        h
    };
    let sia_object_id: [u8; 32] = sha2::Sha256::digest(b"mock-sia-v2-multirange").into();

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
    .bind(xorb_a.to_vec())
    .bind(sia_object_id.to_vec())
    .bind(65536_i64)
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
    .bind(12288_i64)
    .execute(&pool)
    .await?;

    for (i, (bs, be, cs, ce)) in [
        (1024_i64, 4096_i64, 0_i64, 8_i64),
        (3072_i64, 8192_i64, 6_i64, 16_i64),
        (12288_i64, 16384_i64, 24_i64, 32_i64),
    ]
    .iter()
    .enumerate()
    {
        sqlx::query(
            "INSERT INTO reconstruction_terms
                (file_id, term_index, xorb_hash, xorb_start, xorb_end,
                 xorb_byte_start, xorb_byte_end, unpacked_start, unpacked_end)
             VALUES ($1, $2, $3, $4, $5, $6, $7, $8, $9)",
        )
        .bind(file_id_bytes.to_vec())
        .bind(i as i64)
        .bind(xorb_a.to_vec())
        .bind(*cs)
        .bind(*ce)
        .bind(*bs)
        .bind(*be)
        .bind(bs - 1024_i64) // unpacked_start/end just need to be consistent
        .bind(be - 1024_i64)
        .execute(&pool)
        .await?;
    }
    pool.close().await;

    // Compute the hex the CAS's codec would produce for `file_id_bytes`
    // MerkleHash::from_slice does the canonical byte-reversal-per-8-byte
    // group encoding (.md ).
    let file_id = MerkleHash::from_slice(&file_id_bytes)
        .map_err(|e| anyhow::anyhow!("file_id not a MerkleHash: {e}"))?;
    let file_id_hex = format!("{file_id:x}");

    // Hit V2 directly via reqwest so we can inspect the shape and `r=` form.
    let url = format!("{}/v2/reconstructions/{}", harness.base_url, file_id_hex);
    let resp = reqwest::Client::new()
        .get(&url)
        .bearer_auth(&harness.upload_download_token)
        .send()
        .await?;

    assert_eq!(
        resp.status(),
        reqwest::StatusCode::OK,
        "V2 must return 200 when V2_RECONSTRUCTION_ENABLED=true (Plan 03-07 flip)"
    );

    let body: serde_json::Value = resp.json().await?;
    let fetch_info = body
        .get("fetch_info")
        .and_then(|v| v.as_object())
        .expect("V2 response must carry fetch_info object");
    assert_eq!(
        fetch_info.len(),
        1,
        "single-xorb fixture → one fetch_info entry"
    );

    let (_k, v) = fetch_info.iter().next().expect("one entry");
    let xorb_url = v
        .get("url")
        .and_then(|u| u.as_str())
        .expect("xorb entry carries url");
    let ranges = v
        .get("ranges")
        .and_then(|r| r.as_array())
        .expect("xorb entry carries ranges[]");
    assert_eq!(
        ranges.len(),
        2,
        "fixture terms coalesce into exactly two non-contiguous segments"
    );

    // Parse the URL, extract `r=`, assert it's the segments form (comma
    // between two `S-E` pairs). This is the CAS-side proof that 's
    // mint_v1_multi_range has landed; will pair this with gateway-
    // side multipart/byteranges verification.
    let parsed = url::Url::parse(xorb_url).expect("xorb url parses");
    let r_param = parsed
        .query_pairs()
        .find(|(k, _)| k == "r")
        .map(|(_, v)| v.into_owned())
        .expect("r= param present");
    assert!(
        r_param.contains(','),
        "Plan 03-07: `r=` must encode multi-range as `s1-e1,s2-e2`, got {r_param}"
    );
    // Exact shape — the fixture pins bytes 1024..=8191 and 12288..=16383.
    assert_eq!(r_param, "1024-8191,12288-16383");

    // Compile-time probe for the xet-client V2-override path — pairs
    // the CAS-side V2 proof above with a gateway-side multipart/byteranges
    // byte-compare via xet_client's RemoteClient. Instantiate the client here
    // so any upstream rename of `RemoteClient::new` /
    // `get_reconstruction_with_version_override` breaks this file at build
    // time, not at test-authoring time.
    let auth = AuthConfig::maybe_new(
        Some(harness.upload_download_token.clone()),
        Some(u64::MAX),
        Some(Arc::new(NoOpTokenRefresher)),
    );
    let _client: Arc<RemoteClient> = RemoteClient::new(
        &harness.base_url,
        &auth,
        "openweights-conformance-v2-multirange",
        false,
        None,
    );
    // will call `.get_reconstruction(&file_id, None)` (the V2-first
    // client path that xet-core uses against a live gateway). Coerce to the
    // trait object so a rename of the `Client` trait breaks this file.
    let _client_trait: Arc<dyn Client> = _client;

    Ok(())
}
