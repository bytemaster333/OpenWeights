//! Cross-language signed-URL vectors — verifies the Rust minter
//! (`openweights_cas_core::signed_url::UrlSigner`) and an INDEPENDENT Rust HMAC
//! re-implementation produce byte-identical `canonical_string` +
//! `expected_sig_b64url_nopad` for every vector in
//! `conformance/fixtures/signed_url_vectors.json`.
//! Why re-implement the HMAC here? The Go gateway will hash in Go; if
//! the only Rust signature consumer were `openweights_cas_core::signed_url`
//! itself, a bug shared with the minter would never surface. This test
//! re-derives the signature with the `hmac` + `sha2` crates directly — a
//! fresh execution path — and confirms the two Rust paths agree.
//! This test runs WITHOUT Docker or fixtures — it's a pure-CPU vector
//! verification.

use base64::Engine;
use hmac::{Hmac, Mac};
use serde::Deserialize;
use sha2::Sha256;
use openweights_cas_core::signed_url::UrlSigner;
use openweights_cas_proto::merklehash::MerkleHash;
use uuid::Uuid;

#[derive(Debug, Deserialize)]
struct Vector {
    name: String,
    canonical_string: String,
    exp: u64,
    expected_sig_b64url_nopad: String,
    kid: String,
    #[serde(default)]
    range: Option<RangeVec>,
    #[allow(dead_code)]
    signed_by: String, // "current" | "prev" — informational; tests exercise both paths manually
    signing_key_b64: String,
    xorb_hash_hex: String,
}

#[derive(Debug, Deserialize)]
struct RangeVec {
    start: u64,
    end_inclusive: u64,
}

fn load_vectors() -> Vec<Vector> {
    let v = openweights_conformance::fixtures::load_signed_url_vectors()
        .expect("signed_url_vectors fixture is committed to git");
    serde_json::from_value(v).expect("vector JSON shape matches")
}

#[test]
fn every_vector_verifies_via_independent_hmac() {
    let vectors = load_vectors();
    assert!(!vectors.is_empty(), "at least one vector shipped");

    for v in &vectors {
        // 1. Independent HMAC — avoid openweights_cas_core::signed_url's path.
        let key_bytes = base64::engine::general_purpose::STANDARD
            .decode(&v.signing_key_b64)
            .expect("signing key decodes");
        let mut mac = Hmac::<Sha256>::new_from_slice(&key_bytes).expect("hmac init");
        mac.update(v.canonical_string.as_bytes());
        let recomputed = base64::engine::general_purpose::URL_SAFE_NO_PAD
            .encode(mac.finalize().into_bytes());
        assert_eq!(
            recomputed, v.expected_sig_b64url_nopad,
            "vector {} sig mismatch via independent HMAC",
            v.name
        );
    }
}

#[test]
fn canonical_string_agrees_with_openweights_cas_core_builder() {
    // UrlSigner::canonical_string is the authoritative Rust implementation.
    // Assert every vector's `canonical_string` matches what our minter would
    // produce for the same inputs — proves the fixture is not a self-loop and
    // a fresh caller of the minter gets the same string.
    let vectors = load_vectors();
    for v in &vectors {
        let kid = Uuid::parse_str(&v.kid).expect("kid parses");
        let range = v.range.as_ref().map(|r| (r.start, r.end_inclusive));
        let got = UrlSigner::canonical_string("v1", &v.xorb_hash_hex, v.exp, range, kid);
        assert_eq!(got, v.canonical_string, "vector {}", v.name);
    }
}

#[test]
fn minter_produces_expected_signature() {
    // Build a UrlSigner with the vector's signing key in the current slot (or
    // in the prev slot for `signed_by == "prev"`). Mint for the vector's
    // inputs and confirm the resulting `sig` query param equals
    // `expected_sig_b64url_nopad`.
    // Covers:
    // - the `mint_v1` happy path
    // - the rotation-key vector `v1_rotation_prev_key` (loaded with the
    // rotating key as the CURRENT key for minting purposes, because
    // `UrlSigner::mint_v1` always uses `key_current`; the vector proves
    // the SAME key material signs to the SAME bytes whether it's
    // "current" or "prev" — mint-side symmetry).
    let vectors = load_vectors();
    for v in &vectors {
        let base = url::Url::parse("https://gateway.test").unwrap();
        let signer = UrlSigner::new(&v.signing_key_b64, None, base, 1).expect("signer");
        let hash = MerkleHash::from_hex(&v.xorb_hash_hex).unwrap();
        let kid = Uuid::parse_str(&v.kid).unwrap();
        let range = v.range.as_ref().map(|r| (r.start, r.end_inclusive));
        // Reverse-engineer `now_unix` so `exp = now + 1 = vector.exp`.
        let now = v.exp - 1;

        let url_str = signer.mint_v1(&hash, range, kid, now);
        let url = url::Url::parse(&url_str).unwrap();
        let sig = url
            .query_pairs()
            .find(|(k, _)| k == "sig")
            .map(|(_, v)| v.into_owned())
            .expect("sig param present");
        assert_eq!(
            sig, v.expected_sig_b64url_nopad,
            "vector {} minted sig should equal fixture sig",
            v.name
        );
    }
}

#[test]
fn verify_accepts_vector_signatures() {
    // For each vector, build a `UrlSigner` with the vector's key in the
    // appropriate slot (current or prev), re-construct a URL via
    // `UrlSigner::mint_v1` with a FRESH current key, and assert that
    // `verify` accepts when the vector's key is threaded as key_prev.
    // Proves the rotation path works end-to-end.
    let vectors = load_vectors();
    for v in &vectors {
        let base = url::Url::parse("https://gateway.test").unwrap();

        // 1. Build a signer where VEC's key is "current" — mint + verify.
        let signer_current = UrlSigner::new(&v.signing_key_b64, None, base.clone(), 1).unwrap();
        let hash = MerkleHash::from_hex(&v.xorb_hash_hex).unwrap();
        let kid = Uuid::parse_str(&v.kid).unwrap();
        let range = v.range.as_ref().map(|r| (r.start, r.end_inclusive));
        let now = v.exp - 1;
        let url = signer_current.mint_v1(&hash, range, kid, now);

        let verified = signer_current.verify(&url, now).expect("verify ok");
        assert_eq!(verified.exp, v.exp);
        assert_eq!(verified.kid, kid);
        assert_eq!(verified.range, range);
        assert!(!verified.accepted_by_prev_key);

        // 2. Build a DIFFERENT current key + put VEC's key in prev. Verify
        // must accept AND flag `accepted_by_prev_key=true`.
        let other_key_b64 = base64::engine::general_purpose::STANDARD
            .encode([0xAB; 32]);
        let signer_rotated = UrlSigner::new(
            &other_key_b64,
            Some(&v.signing_key_b64),
            base.clone(),
            1,
        )
        .unwrap();
        let verified = signer_rotated.verify(&url, now).expect("prev-key verify ok");
        assert!(
            verified.accepted_by_prev_key,
            "rotation window: signature should be accepted by key_prev for vector {}",
            v.name
        );

        // 3. Same URL verified AFTER exp must fail.
        assert!(signer_current.verify(&url, v.exp + 1).is_err(),
            "expired URL should fail verify");
    }
}

#[test]
fn fixture_is_tracked_at_pinned_revision() {
    // Sanity: the fixture constants committed in must still
    // anchor to the pinned reference xorb hash. Any drift = planning step
    // missed.
    let vectors = load_vectors();
    for v in &vectors {
        assert_eq!(
            v.xorb_hash_hex,
            openweights_conformance::REFERENCE_XORB_HASH_HEX,
            "vector {} hash drifted from pinned fixture",
            v.name
        );
    }
}
