//! Mock Sia adapter for unit + conformance tests.
//!
//! Enabled by `cfg(any(test, feature = "sia-mock"))`. Plan 02-04 Task 5 uses
//! it to assert:
//!   - P2 short-circuit: merkle-mismatch rejection never increments
//!     `upload_call_count()` (i.e. the real Sia SDK was never called).
//!   - Dedup path: duplicate upload returns `{was_inserted: false}` with
//!     `upload_call_count() == 1` across the two requests.
//!   - Sia-unavailable path: `inject_unavailable(true)` causes every call to
//!     return `SiaAdapterError::Unavailable(...)` → handler maps to
//!     `AppError::SiaUnavailable` → HTTP 503 (PROTO-08).

use std::collections::HashMap;
use std::sync::atomic::{AtomicBool, AtomicUsize, Ordering};

use async_trait::async_trait;
use sha2::{Digest, Sha256};
use tokio::sync::Mutex;

use crate::sia::{SiaAdapter, SiaAdapterError};

/// In-memory Sia adapter. Objects are keyed by the SHA-256 of their bytes
/// (deterministic so the same input always produces the same "sia_object_id",
/// matching the content-addressed nature of the real backend).
pub struct MockSiaAdapter {
    store: Mutex<HashMap<[u8; 32], Vec<u8>>>,
    upload_calls: AtomicUsize,
    pin_calls: AtomicUsize,
    download_calls: AtomicUsize,
    inject_fail: AtomicBool,
}

impl MockSiaAdapter {
    pub fn new() -> Self {
        Self {
            store: Mutex::new(HashMap::new()),
            upload_calls: AtomicUsize::new(0),
            pin_calls: AtomicUsize::new(0),
            download_calls: AtomicUsize::new(0),
            inject_fail: AtomicBool::new(false),
        }
    }

    /// Flip the "every call returns Unavailable" switch. Used by the
    /// `sia_unavailable → 503` test.
    pub fn inject_unavailable(&self, on: bool) {
        self.inject_fail.store(on, Ordering::SeqCst);
    }

    pub fn upload_call_count(&self) -> usize {
        self.upload_calls.load(Ordering::SeqCst)
    }

    pub fn pin_call_count(&self) -> usize {
        self.pin_calls.load(Ordering::SeqCst)
    }

    pub fn download_call_count(&self) -> usize {
        self.download_calls.load(Ordering::SeqCst)
    }

    fn fail_if_injected(&self) -> Result<(), SiaAdapterError> {
        if self.inject_fail.load(Ordering::SeqCst) {
            return Err(SiaAdapterError::Unavailable(Box::new(MockUnavailable)));
        }
        Ok(())
    }

    fn id_for(bytes: &[u8]) -> [u8; 32] {
        Sha256::digest(bytes).into()
    }
}

impl Default for MockSiaAdapter {
    fn default() -> Self {
        Self::new()
    }
}

#[derive(Debug, thiserror::Error)]
#[error("mock sia unavailable (injected failure)")]
pub struct MockUnavailable;

#[async_trait]
impl SiaAdapter for MockSiaAdapter {
    async fn upload_and_pin(&self, bytes: &[u8]) -> Result<Vec<u8>, SiaAdapterError> {
        self.fail_if_injected()?;
        self.upload_calls.fetch_add(1, Ordering::SeqCst);

        let id = Self::id_for(bytes);
        {
            let mut store = self.store.lock().await;
            store.entry(id).or_insert_with(|| bytes.to_vec());
        }
        // Pin is folded into upload_and_pin semantically — count it here too.
        self.pin_calls.fetch_add(1, Ordering::SeqCst);
        Ok(id.to_vec())
    }

    async fn pin_only(&self, sia_object_id: &[u8]) -> Result<(), SiaAdapterError> {
        self.fail_if_injected()?;
        self.pin_calls.fetch_add(1, Ordering::SeqCst);

        let key: [u8; 32] = sia_object_id
            .try_into()
            .map_err(|_| anyhow::anyhow!("sia_object_id must be 32 bytes"))?;
        let store = self.store.lock().await;
        if !store.contains_key(&key) {
            return Err(SiaAdapterError::Other(anyhow::anyhow!(
                "mock: pin_only called on unknown sia_object_id"
            )));
        }
        Ok(())
    }

    async fn download_range(
        &self,
        sia_object_id: &[u8],
        offset: u64,
        length: u64,
    ) -> Result<Vec<u8>, SiaAdapterError> {
        self.fail_if_injected()?;
        self.download_calls.fetch_add(1, Ordering::SeqCst);

        let key: [u8; 32] = sia_object_id
            .try_into()
            .map_err(|_| anyhow::anyhow!("sia_object_id must be 32 bytes"))?;

        let store = self.store.lock().await;
        let bytes = store
            .get(&key)
            .ok_or_else(|| anyhow::anyhow!("mock: download_range on unknown sia_object_id"))?;
        let start = offset as usize;
        let end = start.saturating_add(length as usize);
        if end > bytes.len() {
            return Err(SiaAdapterError::Other(anyhow::anyhow!(
                "mock: download_range out of bounds (offset={offset}, length={length}, stored={})",
                bytes.len()
            )));
        }
        Ok(bytes[start..end].to_vec())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[tokio::test]
    async fn upload_and_pin_is_content_addressed() {
        let a = MockSiaAdapter::new();
        let id1 = a.upload_and_pin(b"hello").await.unwrap();
        let id2 = a.upload_and_pin(b"hello").await.unwrap();
        assert_eq!(id1, id2, "content-addressed: same bytes -> same id");
        assert_eq!(a.upload_call_count(), 2, "counters are monotonic");
    }

    #[tokio::test]
    async fn inject_unavailable_produces_sia_adapter_error() {
        let a = MockSiaAdapter::new();
        a.inject_unavailable(true);
        let e = a.upload_and_pin(b"x").await.err().unwrap();
        assert!(matches!(e, SiaAdapterError::Unavailable(_)));
        assert_eq!(
            a.upload_call_count(),
            0,
            "failed calls are not counted as successful uploads"
        );
    }

    #[tokio::test]
    async fn download_range_slices_stored_bytes() {
        let a = MockSiaAdapter::new();
        let id = a.upload_and_pin(b"0123456789").await.unwrap();
        let out = a.download_range(&id, 2, 5).await.unwrap();
        assert_eq!(out, b"23456");
        assert_eq!(a.download_call_count(), 1);
    }
}
