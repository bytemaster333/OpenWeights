//! Redis-backed per-API-key token-bucket rate limit (OPS-04, CONTEXT D-21).
//!
//! Error discrimination (PROTO-08):
//!   * bucket empty         → `AppError::RateLimited { retry_after }` → 429 + Retry-After
//!   * Sia unavailable      → `AppError::SiaUnavailable(..)`          → 503
//!   * Redis unavailable    → `AppError::Other(anyhow!(..))`          → 500
//!
//! Key shape (load-bearing — Plans 02-04..09 depend on it):
//!   `rl:{class}:{api_key_id}`        (milli-tokens remaining)
//!   `rl:{class}:{api_key_id}:ts`     (last-refill unix-ms)
//!
//! Defaults (D-21, env-overridable via the caller's config):
//!   upload:   100 req/min → (capacity=100, refill=100/60 ≈ 1.6667 req/sec)
//!   download: 100 req/min → same
//!   admin:    600 req/min → (capacity=600, refill=600/60 = 10.0 req/sec)

use anyhow::anyhow;
use fred::clients::Client;
use fred::interfaces::LuaInterface;
use fred::types::Value;
use uuid::Uuid;

use crate::errors::AppError;

/// Atomic token-bucket check-and-take script (see `../scripts/rate_limit.lua`).
pub const LUA: &str = include_str!("../scripts/rate_limit.lua");

/// Per-endpoint-class bucket.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum RateLimitClass {
    Upload,
    Download,
    Admin,
}

impl RateLimitClass {
    pub fn name(self) -> &'static str {
        match self {
            Self::Upload => "upload",
            Self::Download => "download",
            Self::Admin => "admin",
        }
    }
}

/// Per-class capacity + refill defaults (D-21).
#[derive(Debug, Clone, Copy)]
pub struct RateLimitDefaults {
    pub upload_capacity: u32,
    pub upload_refill_per_sec: f64,
    pub download_capacity: u32,
    pub download_refill_per_sec: f64,
    pub admin_capacity: u32,
    pub admin_refill_per_sec: f64,
}

impl Default for RateLimitDefaults {
    fn default() -> Self {
        Self {
            upload_capacity: 100,
            upload_refill_per_sec: 100.0 / 60.0,
            download_capacity: 100,
            download_refill_per_sec: 100.0 / 60.0,
            admin_capacity: 600,
            admin_refill_per_sec: 600.0 / 60.0,
        }
    }
}

impl RateLimitDefaults {
    fn for_class(&self, class: RateLimitClass) -> (u32, f64) {
        match class {
            RateLimitClass::Upload => (self.upload_capacity, self.upload_refill_per_sec),
            RateLimitClass::Download => (self.download_capacity, self.download_refill_per_sec),
            RateLimitClass::Admin => (self.admin_capacity, self.admin_refill_per_sec),
        }
    }
}

/// Invoke the Lua bucket on Redis. Returns `(allowed, remaining_milli_tokens)`.
async fn eval_bucket(
    redis: &Client,
    bucket_key: String,
    ts_key: String,
    capacity: u32,
    refill_per_sec: f64,
    now_ms: i64,
) -> anyhow::Result<(i64, i64)> {
    // Lua script expects integer refill-per-sec *scaled by 1000* so floating
    // point math stays out of the server path (see ARGV commentary in Lua).
    let refill_milli_per_sec: i64 = (refill_per_sec * 1000.0) as i64;
    let capacity_arg: i64 = capacity as i64;

    let value: Value = redis
        .eval(
            LUA,
            vec![bucket_key, ts_key],
            vec![capacity_arg, refill_milli_per_sec, now_ms],
        )
        .await
        .map_err(|e| anyhow!("redis EVAL failed: {e}"))?;

    // Response is a Lua table of two integers: [allowed, remaining_mt].
    let Value::Array(arr) = value else {
        return Err(anyhow!("unexpected redis reply shape: {value:?}"));
    };
    if arr.len() != 2 {
        return Err(anyhow!(
            "expected 2-element reply, got {} elements",
            arr.len()
        ));
    }
    let allowed = arr[0]
        .as_i64()
        .ok_or_else(|| anyhow!("reply[0] not integer"))?;
    let remaining = arr[1]
        .as_i64()
        .ok_or_else(|| anyhow!("reply[1] not integer"))?;
    Ok((allowed, remaining))
}

/// Compute Retry-After seconds given milli-tokens remaining and refill rate.
///
/// Retry-After = ceil( (1000 - remaining_mt) / (refill_per_sec * 1000) )
/// (time for the bucket to accumulate one more full token).
fn retry_after_secs(remaining_mt: i64, refill_per_sec: f64) -> u64 {
    if refill_per_sec <= 0.0 {
        // Degenerate config — prevent division by zero; surface a big retry.
        return 60;
    }
    let deficit_mt = (1000 - remaining_mt).max(0) as f64;
    let denominator_mt_per_sec = refill_per_sec * 1000.0;
    let secs = (deficit_mt / denominator_mt_per_sec).ceil();
    (secs as u64).max(1)
}

/// Check-and-take one token from the `(class, api_key_id)` bucket.
///
/// Returns `Ok(())` on allow, `Err(AppError::RateLimited)` on deny with a
/// populated `Retry-After` hint. Redis failures bubble up as `AppError::Other`
/// (500) — deliberately NOT 503 (reserved for Sia unavailability per PROTO-08).
pub async fn check(
    redis: &Client,
    class: RateLimitClass,
    api_key_id: Uuid,
    defaults: RateLimitDefaults,
) -> Result<(), AppError> {
    let (capacity, refill_per_sec) = defaults.for_class(class);
    let now_ms = chrono::Utc::now().timestamp_millis();
    let bucket_key = format!("rl:{}:{}", class.name(), api_key_id);
    let ts_key = format!("rl:{}:{}:ts", class.name(), api_key_id);

    let (allowed, remaining_mt) =
        eval_bucket(redis, bucket_key, ts_key, capacity, refill_per_sec, now_ms)
            .await
            .map_err(AppError::Other)?;

    if allowed == 1 {
        return Ok(());
    }

    let retry_after = retry_after_secs(remaining_mt, refill_per_sec);
    Err(AppError::RateLimited { retry_after })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn class_names_match_key_shape() {
        assert_eq!(RateLimitClass::Upload.name(), "upload");
        assert_eq!(RateLimitClass::Download.name(), "download");
        assert_eq!(RateLimitClass::Admin.name(), "admin");
    }

    #[test]
    fn default_buckets_match_d21() {
        let d = RateLimitDefaults::default();
        assert_eq!(d.upload_capacity, 100);
        assert_eq!(d.download_capacity, 100);
        assert_eq!(d.admin_capacity, 600);
        // 100 req/min ≈ 1.6666 req/s — allow small tolerance.
        assert!((d.upload_refill_per_sec - (100.0 / 60.0)).abs() < 1e-9);
        assert!((d.admin_refill_per_sec - (600.0 / 60.0)).abs() < 1e-9);
    }

    #[test]
    fn retry_after_is_at_least_one() {
        // Empty bucket, 100/60 refill: deficit = 1000 mt; rate = 1666.6 mt/s;
        // secs = ceil(1000/1666.6) = 1.
        let ra = retry_after_secs(0, 100.0 / 60.0);
        assert_eq!(ra, 1);
    }

    #[test]
    fn retry_after_scales_with_slower_refill() {
        // 1 req/minute refill: deficit 1000 mt; rate = 1000/60 ≈ 16.66 mt/s;
        // secs = ceil(1000 / 16.66) = 60.
        let ra = retry_after_secs(0, 1.0 / 60.0);
        assert_eq!(ra, 60);
    }

    #[test]
    fn retry_after_never_zero_on_partial_token() {
        // Already 500 milli-tokens; deficit 500; refill 100/60 ≈ 1666 mt/s;
        // secs = ceil(500 / 1666) = 1.
        let ra = retry_after_secs(500, 100.0 / 60.0);
        assert_eq!(ra, 1);
    }

    #[test]
    fn retry_after_clamps_on_degenerate_refill() {
        let ra = retry_after_secs(0, 0.0);
        assert_eq!(ra, 60);
    }

    #[test]
    fn key_shape_is_rl_class_uuid() {
        // Check the format we commit to — Plans 02-04..09 depend on this string.
        let id = Uuid::nil();
        let bucket_key = format!("rl:{}:{}", RateLimitClass::Upload.name(), id);
        let ts_key = format!("rl:{}:{}:ts", RateLimitClass::Upload.name(), id);
        assert!(bucket_key.starts_with("rl:upload:"));
        assert!(ts_key.ends_with(":ts"));
    }

    #[test]
    fn lua_script_is_embedded() {
        // Sanity — the script file is include_str!-ed so rebuilds pick up edits.
        assert!(LUA.contains("KEYS[1]"));
        assert!(LUA.contains("KEYS[2]"));
        assert!(LUA.contains("EX', 3600"));
    }
}
