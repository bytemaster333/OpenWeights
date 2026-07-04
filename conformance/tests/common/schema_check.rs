//! Structural compatibility checks between JSON responses from `openweights-cas`
//! and the wire types exposed by `xet-client = "=1.5.1"` (via
//! `xet_client::cas_types`).
//! Every helper round-trips the raw JSON through the client-side type via
//! `serde_json::from_value::<T>(v.clone)`. Failure means the CAS response
//! shape drifted from what real xet-core clients expect — a conformance bug.

pub use xet_client::cas_types::{
    BatchQueryReconstructionResponse, QueryReconstructionResponse, QueryReconstructionResponseV2,
    UploadShardResponse, UploadXorbResponse,
};

/// Deserialize a V1 reconstruction response.
pub fn assert_valid_query_reconstruction_v1(v: &serde_json::Value) -> anyhow::Result<()> {
    let _typed: QueryReconstructionResponse = serde_json::from_value(v.clone())
        .map_err(|e| anyhow::anyhow!("V1 reconstruction shape drift: {e} (payload={v})"))?;
    Ok(())
}

/// Deserialize a V2 reconstruction response. Used by the V2-flag-guarded
/// happy-path (flag=true).
pub fn assert_valid_query_reconstruction_v2(v: &serde_json::Value) -> anyhow::Result<()> {
    let _typed: QueryReconstructionResponseV2 = serde_json::from_value(v.clone())
        .map_err(|e| anyhow::anyhow!("V2 reconstruction shape drift: {e} (payload={v})"))?;
    Ok(())
}

/// Deserialize a batch reconstruction response.
pub fn assert_valid_batch_reconstruction(v: &serde_json::Value) -> anyhow::Result<()> {
    let _typed: BatchQueryReconstructionResponse = serde_json::from_value(v.clone())
        .map_err(|e| anyhow::anyhow!("batch reconstruction shape drift: {e} (payload={v})"))?;
    Ok(())
}

/// Deserialize the xorb upload response (`{was_inserted: bool}`).
pub fn assert_valid_upload_xorb(v: &serde_json::Value) -> anyhow::Result<()> {
    let _typed: UploadXorbResponse = serde_json::from_value(v.clone())
        .map_err(|e| anyhow::anyhow!("UploadXorbResponse shape drift: {e} (payload={v})"))?;
    Ok(())
}

/// Deserialize the shard upload response (`{result: 0 | 1}`).
pub fn assert_valid_upload_shard(v: &serde_json::Value) -> anyhow::Result<()> {
    let _typed: UploadShardResponse = serde_json::from_value(v.clone())
        .map_err(|e| anyhow::anyhow!("UploadShardResponse shape drift: {e} (payload={v})"))?;
    Ok(())
}
