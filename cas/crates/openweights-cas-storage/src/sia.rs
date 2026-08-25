//! Sia adapter trait + concrete Rust SDK implementation.
//! The trait [`SiaAdapter`] is the single surface handlers and
//! the reconciler depend on. The concrete [`RustSdkAdapter`]
//! wraps `sia_storage::Sdk` per the A1 probe findings ( A1 verdict
//! YELLOW — `pin_object(&obj)` by REFERENCE, `upload(Object, reader, opts) ->
//! Object` by VALUE + new return, `download` is a sync constructor returning
//! an AsyncRead handle).
//! The mock implementation lives in [`super::mock`] behind
//! `cfg(any(test, feature = "sia-mock"))` so the conformance crate (Plan
//! 02-10) and handler unit tests ( Task 5) can exercise the full
//! request path without a live indexd.

use std::io::Cursor;

use async_trait::async_trait;
use bytes::Bytes;

pub use sia_storage::{
    AppKey, AppMetadata, Builder, DownloadOptions, Object, Sdk, UploadOptions,
};

/// Errors returned across the Sia adapter boundary.
/// `Unavailable` maps 1:1 to `openweights_cas_core::errors::AppError::SiaUnavailable`
/// → HTTP 503 ( distinct from 429). Any other shape is wrapped in
/// `Other(anyhow::Error)` which becomes a 500 at the handler.
#[derive(thiserror::Error, Debug)]
pub enum SiaAdapterError {
    #[error("sia unavailable: {0}")]
    Unavailable(#[source] Box<dyn std::error::Error + Send + Sync>),

    #[error(transparent)]
    Other(#[from] anyhow::Error),
}

impl SiaAdapterError {
    pub fn unavailable<E>(err: E) -> Self
    where
        E: std::error::Error + Send + Sync + 'static,
    {
        Self::Unavailable(Box::new(err))
    }
}

/// Object-safe boundary between openweights-cas and Sia storage.
/// - `upload_and_pin` — content-addressed write path.
/// Returns the 32-byte `sia_object_id` (raw bytes from `Object::id`).
/// - `pin_only` — reconciler retry path for `pin_state='pinning'`
/// rows.
/// - `download_range` — gateway read path ( V1 reconstruction
/// materialization and range-serving; does not use it).
#[async_trait]
pub trait SiaAdapter: Send + Sync + 'static {
    /// Upload `bytes` to Sia and pin the resulting object.
    /// Returns the 32-byte Sia object id on success.
    async fn upload_and_pin(&self, bytes: &[u8]) -> Result<Vec<u8>, SiaAdapterError>;

    /// Re-pin a previously uploaded object by id. Used by the reconciler when
    /// `pin_state='pinning'` rows need another attempt.
    async fn pin_only(&self, sia_object_id: &[u8]) -> Result<(), SiaAdapterError>;

    /// Range-read from a previously uploaded object. `offset` + `length` are
    /// bytes. Length of the returned `Vec` MUST equal `length` on success.
    async fn download_range(
        &self,
        sia_object_id: &[u8],
        offset: u64,
        length: u64,
    ) -> Result<Vec<u8>, SiaAdapterError>;
}

/// Config needed to construct a [`RustSdkAdapter`].
pub struct SiaAdapterConfig {
    /// Indexd URL (e.g. `http://indexd:9980`).
    pub indexd_url: String,
    /// Exported 32-byte Sia AppKey (already decoded from base64).
    pub app_key_bytes: [u8; 32],
    /// Application metadata (locked at compile time — see [`DEFAULT_APP_META`]).
    pub app_meta: AppMetadata,
    /// Erasure coding redundancy for uploads. Demos with few hosts must
    /// dial this below the SDK default of 10/20.
    pub redundancy: RedundancyConfig,
}

/// Default metadata used when wiring `RustSdkAdapter` from `main.rs`.
/// `AppMetadata::{name, description, service_url}` fields require
/// `&'static str`, so we hard-code them here. The `id` field is the
/// `OPENWEIGHTS_APP_ID` env var (hex → Hash256) — construct it at boot with
/// `Hash256::new(hex_decoded_bytes)`.
pub const DEFAULT_APP_META_NAME: &str = "OpenWeights CAS";
pub const DEFAULT_APP_META_DESC: &str = "OpenWeights content-addressed storage service";
pub const DEFAULT_APP_META_URL: &str = "https://example.com";

/// Erasure-coding redundancy used on every upload. The SDK default is
/// 10 data + 20 parity = 30 hosts per slab — production-grade but
/// unrunnable on a small demo deployment with only a handful of formed
/// contracts. We expose this via env so operators can dial it down for a
/// 6-host demo (e.g. 2 + 4) without recompiling.
#[derive(Debug, Clone, Copy)]
pub struct RedundancyConfig {
    pub data_shards: u8,
    pub parity_shards: u8,
}

impl Default for RedundancyConfig {
    fn default() -> Self {
        Self {
            data_shards: 10,
            parity_shards: 20,
        }
    }
}

/// Thin wrapper around `sia_storage::Sdk` implementing [`SiaAdapter`].
pub struct RustSdkAdapter {
    sdk: Sdk,
    redundancy: RedundancyConfig,
}

impl RustSdkAdapter {
    /// Construct directly from a pre-built `Sdk`. Used in tests and after the
    /// builder handshake in `main.rs`.
    pub fn from_sdk(sdk: Sdk) -> Self {
        Self {
            sdk,
            redundancy: RedundancyConfig::default(),
        }
    }

    /// Override the erasure-coding redundancy. Call once after `from_sdk` /
    /// `connect` based on operator env config.
    pub fn with_redundancy(mut self, redundancy: RedundancyConfig) -> Self {
        self.redundancy = redundancy;
        self
    }

    /// Full connect sequence per A1 probe snippet: `Builder::new(url, meta)?
    ///.connected(&AppKey).await?`. Returns `Err(Unavailable)` if the key is
    /// not registered for this metadata.
    /// ** startup self-check:** the handshake inside `connected(..)`
    /// verifies account balance + registration with indexd; if that handshake
    /// succeeds the SDK is by-construction ready to form contracts.
    pub async fn connect(cfg: SiaAdapterConfig) -> Result<Self, SiaAdapterError> {
        let redundancy = cfg.redundancy;
        let app_key = AppKey::import(cfg.app_key_bytes);
        let builder =
            Builder::new(&cfg.indexd_url, cfg.app_meta).map_err(SiaAdapterError::unavailable)?;
        let sdk = builder
            .connected(&app_key)
            .await
            .map_err(SiaAdapterError::unavailable)?
            .ok_or_else(|| {
                SiaAdapterError::Other(anyhow::anyhow!(
                    "sia_storage::Builder::connected returned None — app key not registered"
                ))
            })?;
        Ok(Self { sdk, redundancy })
    }

    /// Access the raw Sdk ( reconciler may use `sdk.object(&id)`
    /// to fetch a persisted Object by id for retry).
    pub fn sdk(&self) -> &Sdk {
        &self.sdk
    }

    /// Resolve a persisted sia_object_id (32 bytes) to a live `Object`.
    async fn object_for(&self, sia_object_id: &[u8]) -> Result<Object, SiaAdapterError> {
        let arr: [u8; 32] = sia_object_id
            .try_into()
            .map_err(|_| anyhow::anyhow!("sia_object_id must be 32 bytes"))?;
        let id = sia_storage::Hash256::new(arr);
        self.sdk
            .object(&id)
            .await
            .map_err(SiaAdapterError::unavailable)
    }
}

#[async_trait]
impl SiaAdapter for RustSdkAdapter {
    async fn upload_and_pin(&self, bytes: &[u8]) -> Result<Vec<u8>, SiaAdapterError> {
        // Own a copy for the AsyncRead. `Bytes` is cheap to clone; the SDK
        // requires `'static + Unpin` for the reader.
        let owned = Bytes::copy_from_slice(bytes);
        let reader = Cursor::new(owned);

        // A1 probe: upload takes Object by VALUE and returns a NEW Object.
        // Redundancy is operator-tunable so a small demo deployment with
        // ~6 contracts can still write (the SDK default of 10+20=30 hosts
        // would `queue error: not enough initial hosts` here otherwise).
        let opts = UploadOptions {
            data_shards: self.redundancy.data_shards,
            parity_shards: self.redundancy.parity_shards,
            ..UploadOptions::default()
        };
        let obj: Object = self
            .sdk
            .upload(Object::default(), reader, opts)
            .await
            .map_err(SiaAdapterError::unavailable)?;

        // A1 probe: pin_object takes &Object by REFERENCE (Gotcha 32 is Go-only).
        self.sdk
            .pin_object(&obj)
            .await
            .map_err(SiaAdapterError::unavailable)?;

        // Object::id -> Hash256; convert to Vec<u8>.
        let id = obj.id();
        let id_bytes: [u8; 32] = id.into();
        Ok(id_bytes.to_vec())
    }

    async fn pin_only(&self, sia_object_id: &[u8]) -> Result<(), SiaAdapterError> {
        let obj = self.object_for(sia_object_id).await?;
        self.sdk
            .pin_object(&obj)
            .await
            .map_err(SiaAdapterError::unavailable)
    }

    async fn download_range(
        &self,
        sia_object_id: &[u8],
        offset: u64,
        length: u64,
    ) -> Result<Vec<u8>, SiaAdapterError> {
        let obj = self.object_for(sia_object_id).await?;
        let opts = DownloadOptions {
            offset,
            length: Some(length),
            max_inflight: 80,
            shard_downloaded: None,
        };
        let mut dl = self
            .sdk
            .download(&obj, opts)
            .map_err(SiaAdapterError::unavailable)?;
        let mut out = Vec::with_capacity(length as usize);
        tokio::io::copy(&mut dl, &mut out)
            .await
            .map_err(|e| SiaAdapterError::Other(anyhow::anyhow!("sia download copy: {e}")))?;
        Ok(out)
    }
}

/// Conversion from `sia_storage::Hash256` <-> u8 array, used by
/// `Object::id`. We convert through `Into<[u8;32]>` which the sia_core macro
/// provides. Hash256 does NOT expose a raw-bytes accessor we control — but
/// we control a `const fn new([u8;32])` and it implements `From<[u8;32]>`
/// via the macro.
#[allow(dead_code)]
fn _hash256_roundtrip_is_possible(b: [u8; 32]) -> [u8; 32] {
    let h = sia_storage::Hash256::new(b);
    h.into()
}
