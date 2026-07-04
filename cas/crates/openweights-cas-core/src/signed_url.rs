//! HMAC-SHA256 signed gateway URL minter + verifier.
//! Wire contract (ARCHITECTURE.md §"Canonical string for signing",
//! RESEARCH §5.1, CONTEXT ):
//! ```text
//! canonical = "v1\n<xorb_hash_hex>\n<exp>\n<r_or_empty>\n<kid>"
//! sig = base64url_nopad( HMAC_SHA256(key, canonical) )
//! URL = <gateway_base>/xorb/<xorb_hash_hex>?exp=<exp>&kid=<kid>&sig=<sig>[&r=<s>-<e>]
//! ```
//! Field order in the canonical string is FIXED: `version`, `hash`, `exp`,
//! `range`, `kid`. The separator is EXACTLY one LF (0x0A) between each field
//! never CRLF, never a colon, never whitespace. Query parameter order in the
//! minted URL is NOT load-bearing (HMAC runs over the canonical string, not the
//! URL bytes).
//! Rotation: `key_prev` is an optional second HMAC key accepted by `verify`
//! but never used by `mint_v1`. Operators swap `.env` entries + rolling
//! restart; the 2h default TTL means any rotation wider than 2h
//! guarantees no in-flight signed URL is orphaned.
//! Consumer contract ( gateway):
//! - Expired URL → return **HTTP 403** (.md ; NOT 401, NOT 410).
//!   xet-core's `refresh_url` retry path keys specifically on 403.
//! - Bad signature → HTTP 403.
//! - Malformed URL → HTTP 400.
//!   Cross-language vectors: `conformance/fixtures/signed_url_vectors.json`
//!   pins byte-identical canonical-string + signature outputs so the Go
//!   gateway's verifier cannot silently drift from this module.

use base64::Engine as _;
use base64::engine::general_purpose::{STANDARD as B64_STD, URL_SAFE_NO_PAD as B64_URL};
use hmac::{Hmac, Mac};
use sha2::Sha256;
use openweights_cas_proto::merklehash::MerkleHash;
use subtle::ConstantTimeEq;
use uuid::Uuid;

/// Signing key must decode to exactly 32 bytes. This is a hard invariant on the
/// wire format — keep in lockstep with gateway's key-material check.
pub const SIGNING_KEY_LEN: usize = 32;

/// Canonical-string version prefix. Reserved so the scheme can rotate to `v2`
/// if we ever need a breaking change without colliding with the old wire.
pub const CANONICAL_VERSION: &str = "v1";

/// Errors surfaced by `UrlSigner::new`. Stringly opaque by design — no signing-
/// key bytes ever appear here (T-02-08-03).
#[derive(thiserror::Error, Debug)]
pub enum SignerError {
    #[error("signing key is not valid base64")]
    KeyNotBase64,
    #[error("signing key must decode to exactly 32 bytes (got {0})")]
    InvalidKeyLength(usize),
    #[error("hmac key init failed")]
    HmacInit,
}

/// Errors surfaced by `UrlSigner::verify`. Map these 1:1 to gateway HTTP codes
///: `Expired` + `BadSignature` → 403, `Malformed` → 400.
#[derive(thiserror::Error, Debug, PartialEq, Eq)]
pub enum VerifyErr {
    #[error("url is malformed: {0}")]
    Malformed(&'static str),
    #[error("signed url is expired")]
    Expired,
    #[error("signature does not match")]
    BadSignature,
}

/// The decoded, verified URL payload returned on a successful `verify` call.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct VerifyOk {
    pub xorb_hash_hex: String,
    pub exp: u64,
    pub range: Option<(u64, u64)>,
    pub kid: Uuid,
    /// Which key accepted the signature. `false` = `key_current`, `true` =
    /// `key_prev`. Handy for gateway metrics during rotation.
    pub accepted_by_prev_key: bool,
}

/// HMAC-SHA256 signed URL minter + verifier.
/// Construct once at boot; `Clone` is cheap (both `Hmac<Sha256>` keys are
/// `Clone`). Thread-safe; use `Arc<UrlSigner>` across handler tasks.
#[derive(Clone)]
pub struct UrlSigner {
    key_current: Hmac<Sha256>,
    key_prev: Option<Hmac<Sha256>>,
    gateway_base: url::Url,
    default_ttl_secs: u64,
}

impl std::fmt::Debug for UrlSigner {
    /// Deliberately redacts the HMAC keys — raw signing material must never
    /// appear in logs (T-02-08-03). Only the base URL + TTL are safe to print.
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("UrlSigner")
            .field("gateway_base", &self.gateway_base.as_str())
            .field("default_ttl_secs", &self.default_ttl_secs)
            .field("has_prev_key", &self.key_prev.is_some())
            .finish_non_exhaustive()
    }
}

impl UrlSigner {
    /// Construct a signer from base64-encoded 32-byte keys. Fails loudly on
    /// wrong-length keys — do NOT silently extend/truncate.
    pub fn new(
        key_current_b64: &str,
        key_prev_b64: Option<&str>,
        gateway_base: url::Url,
        default_ttl_secs: u64,
    ) -> Result<Self, SignerError> {
        let key_current = load_hmac_key(key_current_b64)?;
        let key_prev = key_prev_b64.map(load_hmac_key).transpose()?;
        Ok(Self {
            key_current,
            key_prev,
            gateway_base,
            default_ttl_secs,
        })
    }

    /// The configured TTL in seconds. Handlers use this when they want to
    /// compute `exp = now + ttl` explicitly instead of letting `mint_v1`
    /// derive it.
    pub fn default_ttl_secs(&self) -> u64 {
        self.default_ttl_secs
    }

    /// Build the canonical string that the HMAC runs over. Exposed so the
    /// cross-language test vector fixture can be byte-compared against Rust
    /// output (and the Go verifier against its Go output).
    /// Field order is FIXED and load-bearing. Do not reorder.
    pub fn canonical_string(
        version: &str,
        xorb_hash_hex: &str,
        exp: u64,
        range: Option<(u64, u64)>,
        kid: Uuid,
    ) -> String {
        // `<start>-<end_inclusive>` OR empty string. Single hyphen, ASCII
        // decimals, no spaces. The hyphen is a literal `-` (0x2D).
        let r = match range {
            Some((s, e)) => format!("{s}-{e}"),
            None => String::new(),
        };
        // Five fields, four LFs (0x0A). NEVER CRLF. NEVER colon-separated.
        format!("{version}\n{xorb_hash_hex}\n{exp}\n{r}\n{kid}")
    }

    /// Compute the base64url-nopad signature for a canonical string with the
    /// current key. Infallible per `hmac`'s contract (key is already validated).
    fn sign_with(&self, key: &Hmac<Sha256>, canonical: &str) -> String {
        let mut mac = key.clone();
        mac.update(canonical.as_bytes());
        B64_URL.encode(mac.finalize().into_bytes())
    }

    /// Mint a signed gateway URL.
    /// Arguments:
    /// - `xorb_hash`: the merkle hash of the xorb being served. Path-encoded
    ///   as 64-char lowercase hex (byte-reversal per
    ///   8-byte group is handled by `MerkleHash::hex`).
    /// - `range`: optional `(start, end_inclusive)` HTTP-byte range. When
    ///   present the gateway MUST reject `Range:` headers outside this
    ///   interval. When absent the URL grants access to the entire xorb.
    /// - `kid`: the API key id (UUID) authorizing the mint. Logged by the
    ///   gateway for per-key usage attribution.
    /// - `now_unix`: clock reading for `exp = now + default_ttl_secs`.
    ///   Injected so tests can pin the value.
    pub fn mint_v1(
        &self,
        xorb_hash: &MerkleHash,
        range: Option<(u64, u64)>,
        kid: Uuid,
        now_unix: u64,
    ) -> String {
        let exp = now_unix.saturating_add(self.default_ttl_secs);
        let hex = xorb_hash.hex();
        let canonical = Self::canonical_string(CANONICAL_VERSION, &hex, exp, range, kid);
        let sig = self.sign_with(&self.key_current, &canonical);

        // Build `<base>/xorb/<hex>?exp=...&kid=...&sig=...[&r=...]`. We clone
        // the base URL so callers can share one `UrlSigner` across requests.
        let mut url = self.gateway_base.clone();
        url.set_path(&format!("/xorb/{hex}"));
        {
            let mut qs = url.query_pairs_mut();
            qs.append_pair("exp", &exp.to_string());
            qs.append_pair("kid", &kid.to_string());
            qs.append_pair("sig", &sig);
            if let Some((s, e)) = range {
                qs.append_pair("r", &format!("{s}-{e}"));
            }
        }
        url.to_string()
    }

    /// Mint a signed gateway URL granting access to MULTIPLE non-contiguous
    /// byte ranges of a single xorb. V2 flip (RECEIVED §G item 2):
    /// replaces the bounding-range approximation with a segments-form
    /// descriptor that the Go gateway parses into `[]Range` and serves as
    /// RFC 7233 `multipart/byteranges` (.md ).
    /// Canonical string format: SAME v1 algorithm as `mint_v1`; the `r` field
    /// is the joined segments string `s1-e1,s2-e2,...` (comma-separated, no
    /// spaces, each `s`/`e` ASCII decimals, `e` inclusive). Single-segment
    /// input (`segments.len == 1`) produces a canonical identical to the
    /// equivalent `mint_v1(Some((s,e)))` — no `r` field split. This keeps
    /// the verifier's canonical-string computation path uniform.
    /// # Panics
    /// Panics if `segments` is empty. Empty multi-range is meaningless; the
    /// V2 builder already guarantees at least one segment per xorb (a xorb
    /// appearing in `fetch_info` by definition has ≥1 coalesced range).
    pub fn mint_v1_multi_range(
        &self,
        xorb_hash: &MerkleHash,
        segments: &[(u64, u64)],
        kid: Uuid,
        now_unix: u64,
    ) -> String {
        assert!(
            !segments.is_empty(),
            "mint_v1_multi_range requires at least one segment"
        );
        let exp = now_unix.saturating_add(self.default_ttl_secs);
        let hex = xorb_hash.hex();
        // Join segments into `s1-e1,s2-e2,...`.
        let r_str: String = segments
            .iter()
            .map(|(s, e)| format!("{s}-{e}"))
            .collect::<Vec<_>>()
            .join(",");
        // Canonical re-uses the v1 5-field layout: the `r` slot holds the
        // already-joined segments string. HMAC runs over the exact bytes the
        // gateway verifier will reconstruct from the querystring.
        let canonical = format!(
            "{version}\n{hex}\n{exp}\n{r}\n{kid}",
            version = CANONICAL_VERSION,
            r = r_str,
        );
        let sig = self.sign_with(&self.key_current, &canonical);

        let mut url = self.gateway_base.clone();
        url.set_path(&format!("/xorb/{hex}"));
        {
            let mut qs = url.query_pairs_mut();
            qs.append_pair("exp", &exp.to_string());
            qs.append_pair("kid", &kid.to_string());
            qs.append_pair("sig", &sig);
            qs.append_pair("r", &r_str);
        }
        url.to_string()
    }

    /// Verify a signed URL. Go gateway mirrors this logic and is
    /// conformance-checked against `conformance/fixtures/signed_url_vectors.json`.
    /// On success returns `VerifyOk` with the parsed payload. Status-code
    /// mapping for the gateway: `Expired` + `BadSignature` → 403, `Malformed`
    /// → 400 (.md ).
    pub fn verify(&self, url_str: &str, now_unix: u64) -> Result<VerifyOk, VerifyErr> {
        let url = url::Url::parse(url_str).map_err(|_| VerifyErr::Malformed("parse"))?;
        let path = url.path();
        // Expected shape: `/xorb/<64-hex>`.
        let hex = path
            .strip_prefix("/xorb/")
            .ok_or(VerifyErr::Malformed("path_prefix"))?;
        if hex.len() != 64 || !hex.bytes().all(|b| b.is_ascii_hexdigit()) {
            return Err(VerifyErr::Malformed("xorb_hash_hex"));
        }
        // MerkleHash encoding is lowercase — enforce that callers are not
        // trying to sneak through mixed-case hex that would still parse.
        if hex.chars().any(|c| c.is_ascii_uppercase()) {
            return Err(VerifyErr::Malformed("xorb_hash_hex_case"));
        }

        // Collect query params into option slots (reject duplicates implicitly
        // by overwriting — a tampered URL with multiple `sig` keys still has
        // to produce a matching signature).
        let mut exp: Option<u64> = None;
        let mut kid: Option<Uuid> = None;
        let mut sig: Option<String> = None;
        let mut range: Option<(u64, u64)> = None;
        for (k, v) in url.query_pairs() {
            match k.as_ref() {
                "exp" => exp = v.parse::<u64>().ok(),
                "kid" => kid = Uuid::parse_str(&v).ok(),
                "sig" => sig = Some(v.into_owned()),
                "r" => range = parse_range(&v),
                _ => {}
            }
        }
        let exp = exp.ok_or(VerifyErr::Malformed("exp"))?;
        let kid = kid.ok_or(VerifyErr::Malformed("kid"))?;
        let sig = sig.ok_or(VerifyErr::Malformed("sig"))?;
        // If `r=` was present at all it must parse cleanly; silently dropping
        // a malformed range would widen the signed.
        if url.query_pairs().any(|(k, _)| k == "r") && range.is_none() {
            return Err(VerifyErr::Malformed("range"));
        }

        // Expiry check happens BEFORE signature — cheap + keeps timing profile
        // uniform across all expired URLs (no key material touched).
        if now_unix >= exp {
            return Err(VerifyErr::Expired);
        }

        let canonical = Self::canonical_string(CANONICAL_VERSION, hex, exp, range, kid);
        let provided_sig_bytes = B64_URL
            .decode(sig.as_bytes())
            .map_err(|_| VerifyErr::Malformed("sig_b64"))?;

        // Current-key compare (constant time).
        let current_sig = self.sign_with(&self.key_current, &canonical);
        let current_sig_bytes = B64_URL
            .decode(current_sig.as_bytes())
            .expect("self-signed blob decodes");
        if ct_eq(&current_sig_bytes, &provided_sig_bytes) {
            return Ok(VerifyOk {
                xorb_hash_hex: hex.to_string(),
                exp,
                range,
                kid,
                accepted_by_prev_key: false,
            });
        }

        // Rotation window: try `key_prev` if configured.
        if let Some(prev) = &self.key_prev {
            let prev_sig = self.sign_with(prev, &canonical);
            let prev_sig_bytes = B64_URL
                .decode(prev_sig.as_bytes())
                .expect("self-signed blob decodes");
            if ct_eq(&prev_sig_bytes, &provided_sig_bytes) {
                return Ok(VerifyOk {
                    xorb_hash_hex: hex.to_string(),
                    exp,
                    range,
                    kid,
                    accepted_by_prev_key: true,
                });
            }
        }

        Err(VerifyErr::BadSignature)
    }
}

/// Shared abstract trait for downstream consumers ( reconstruction
/// handler). Lets callers depend on behavior, not a concrete HMAC key type,
/// which keeps the seam open for a future pluggable signer (e.g., KMS-backed).
pub trait UrlMinter: Send + Sync {
    /// Default TTL in seconds; exposed so handlers can compute the same
    /// `exp` value the mint uses, e.g. when stamping a reconstruction response
    /// with a consistent expiry across several URLs.
    fn default_ttl_secs(&self) -> u64;

    /// Mint a signed `GET` URL for the given xorb (optionally constrained to
    /// a byte range). `kid` is the API key UUID authorizing the mint.
    fn mint_v1(
        &self,
        xorb_hash: &MerkleHash,
        range: Option<(u64, u64)>,
        kid: Uuid,
        now_unix: u64,
    ) -> String;

    /// Mint a signed `GET` URL granting access to multiple non-contiguous
    /// byte ranges of a single xorb. Used by V2 reconstruction (
    /// multi-range serving). Canonical string encodes the joined
    /// `s1-e1,s2-e2,...` string in the `r` field.
    /// Panics if `segments` is empty.
    fn mint_v1_multi_range(
        &self,
        xorb_hash: &MerkleHash,
        segments: &[(u64, u64)],
        kid: Uuid,
        now_unix: u64,
    ) -> String;
}

impl UrlMinter for UrlSigner {
    fn default_ttl_secs(&self) -> u64 {
        self.default_ttl_secs
    }
    fn mint_v1(
        &self,
        xorb_hash: &MerkleHash,
        range: Option<(u64, u64)>,
        kid: Uuid,
        now_unix: u64,
    ) -> String {
        UrlSigner::mint_v1(self, xorb_hash, range, kid, now_unix)
    }
    fn mint_v1_multi_range(
        &self,
        xorb_hash: &MerkleHash,
        segments: &[(u64, u64)],
        kid: Uuid,
        now_unix: u64,
    ) -> String {
        UrlSigner::mint_v1_multi_range(self, xorb_hash, segments, kid, now_unix)
    }
}

// ---------------------------------------------------------------------------
// Internal helpers
// ---------------------------------------------------------------------------

/// Decode + validate a base64 HMAC key. 32-byte length is enforced because the
/// gateway expects the same bytes and a mismatch silently invalidates every
/// verify. Any error classifies as SignerError (never leaks key material).
fn load_hmac_key(key_b64: &str) -> Result<Hmac<Sha256>, SignerError> {
    let bytes = B64_STD
        .decode(key_b64.as_bytes())
        .map_err(|_| SignerError::KeyNotBase64)?;
    if bytes.len() != SIGNING_KEY_LEN {
        return Err(SignerError::InvalidKeyLength(bytes.len()));
    }
    Hmac::<Sha256>::new_from_slice(&bytes).map_err(|_| SignerError::HmacInit)
}

/// Parse `start-end` (both ASCII decimals, single hyphen, end inclusive).
/// Returns `None` on any malformed input — caller maps to `VerifyErr::Malformed`.
fn parse_range(s: &str) -> Option<(u64, u64)> {
    let (start, end) = s.split_once('-')?;
    let start = start.parse::<u64>().ok()?;
    let end = end.parse::<u64>().ok()?;
    if start > end {
        return None;
    }
    Some((start, end))
}

/// Constant-time byte compare. Equal-length short-circuit is safe (length is
/// public info — base64url-nopad of a 32-byte HMAC is always 43 bytes).
fn ct_eq(a: &[u8], b: &[u8]) -> bool {
    if a.len() != b.len() {
        return false;
    }
    a.ct_eq(b).into()
}
