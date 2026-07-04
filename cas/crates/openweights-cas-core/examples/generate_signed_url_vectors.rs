//! Regenerate `conformance/fixtures/signed_url_vectors.json` against the
//! current `UrlSigner::canonical_string` + signing logic.
//! Usage:
//! ```text
//! cargo run -p openweights-cas-core --example generate_signed_url_vectors -- \
//! conformance/fixtures/signed_url_vectors.json
//! ```
//! The binary:
//! 1. Reads the JSON file at the given path (array of vectors).
//! 2. For each vector, recomputes `canonical_string` + HMAC-SHA256 sig.
//! 3. Writes the canonical_string + expected_sig_b64url_nopad fields back.
//!    Run once after every edit to `UrlSigner::canonical_string` or the vector
//!    inputs. Idempotent — re-running on an already-populated file produces a
//!    byte-identical diff.
//!    Go gateway consumes these same vectors as its authoritative
//!    verification target. Any diff after regeneration MUST be reviewed.

use std::env;
use std::fs;
use std::path::PathBuf;
use std::process::ExitCode;

use base64::Engine as _;
use base64::engine::general_purpose::{STANDARD as B64_STD, URL_SAFE_NO_PAD as B64_URL};
use hmac::{Hmac, Mac};
use serde::{Deserialize, Serialize};
use serde_json::Value;
use sha2::Sha256;
use uuid::Uuid;

use openweights_cas_core::signed_url::{CANONICAL_VERSION, UrlSigner};

#[derive(Debug, Deserialize, Serialize)]
#[serde(deny_unknown_fields)]
struct RangeSpec {
    start: u64,
    end_inclusive: u64,
}

#[derive(Debug, Deserialize, Serialize)]
#[serde(deny_unknown_fields)]
struct Vector {
    name: String,
    xorb_hash_hex: String,
    exp: u64,
    range: Option<RangeSpec>,
    kid: String,
    signing_key_b64: String,
    #[serde(default)]
    canonical_string: String,
    #[serde(default)]
    expected_sig_b64url_nopad: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    signed_by: Option<String>,
}

fn main() -> ExitCode {
    let args: Vec<String> = env::args().collect();
    let path: PathBuf = args
        .get(1)
        .map(PathBuf::from)
        .unwrap_or_else(|| PathBuf::from("conformance/fixtures/signed_url_vectors.json"));

    let raw = match fs::read_to_string(&path) {
        Ok(s) => s,
        Err(e) => {
            eprintln!("failed to read {}: {e}", path.display());
            return ExitCode::from(2);
        }
    };

    let mut vectors: Vec<Vector> = match serde_json::from_str(&raw) {
        Ok(v) => v,
        Err(e) => {
            eprintln!("failed to parse JSON: {e}");
            return ExitCode::from(2);
        }
    };

    for v in vectors.iter_mut() {
        let range = v
            .range
            .as_ref()
            .map(|r| (r.start, r.end_inclusive));
        let kid = match Uuid::parse_str(&v.kid) {
            Ok(u) => u,
            Err(e) => {
                eprintln!("vector {}: bad kid: {e}", v.name);
                return ExitCode::from(2);
            }
        };
        let canonical =
            UrlSigner::canonical_string(CANONICAL_VERSION, &v.xorb_hash_hex, v.exp, range, kid);

        let key_bytes = match B64_STD.decode(v.signing_key_b64.as_bytes()) {
            Ok(b) if b.len() == 32 => b,
            Ok(b) => {
                eprintln!(
                    "vector {}: signing_key_b64 decoded to {} bytes (expected 32)",
                    v.name,
                    b.len()
                );
                return ExitCode::from(2);
            }
            Err(e) => {
                eprintln!("vector {}: signing_key_b64 not base64: {e}", v.name);
                return ExitCode::from(2);
            }
        };
        let mut mac = Hmac::<Sha256>::new_from_slice(&key_bytes).expect("valid key len");
        mac.update(canonical.as_bytes());
        let sig = B64_URL.encode(mac.finalize().into_bytes());

        v.canonical_string = canonical;
        v.expected_sig_b64url_nopad = sig;
    }

    // Pretty-print; trailing newline for POSIX tooling friendliness.
    let mut out = serde_json::to_string_pretty(&Value::Array(
        vectors
            .iter()
            .map(|v| serde_json::to_value(v).expect("vector serializes"))
            .collect(),
    ))
    .expect("serialize");
    out.push('\n');

    if let Err(e) = fs::write(&path, &out) {
        eprintln!("failed to write {}: {e}", path.display());
        return ExitCode::from(2);
    }
    println!("wrote {} vectors to {}", vectors.len(), path.display());
    ExitCode::SUCCESS
}
