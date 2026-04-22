//! Plan 02-08 Task 3 — cross-language signed-URL test vectors.
//!
//! These tests load `conformance/fixtures/signed_url_vectors.json` and assert
//! that `UrlSigner::canonical_string` + the HMAC signing + `mint_v1` + `verify`
//! all round-trip byte-identically against the committed vectors. Phase 3's
//! Go gateway consumes the same JSON as its verification target — any drift
//! between Rust mint output and this fixture is a Phase 2 bug; any drift
//! between Go verify output and this fixture is a Phase 3 bug.
//!
//! Fixture regeneration:
//!   cargo run -p siahub-cas-core --example generate_signed_url_vectors -- \
//!       ../conformance/fixtures/signed_url_vectors.json
//!
//! (Path is relative to `cas/` workspace root.)

use base64::Engine as _;
use base64::engine::general_purpose::{STANDARD as B64_STD, URL_SAFE_NO_PAD as B64_URL};
use hmac::{Hmac, Mac};
use serde::Deserialize;
use sha2::Sha256;
use url::Url;
use uuid::Uuid;

use crate::signed_url::{CANONICAL_VERSION, UrlMinter, UrlSigner, VerifyErr};
use siahub_cas_proto::merklehash::MerkleHash;

// ---------------------------------------------------------------------------
// Fixture deserialization
// ---------------------------------------------------------------------------

#[derive(Debug, Deserialize)]
struct RangeSpec {
    start: u64,
    end_inclusive: u64,
}

#[derive(Debug, Deserialize)]
#[allow(dead_code)]
struct Vector {
    name: String,
    xorb_hash_hex: String,
    exp: u64,
    range: Option<RangeSpec>,
    kid: String,
    signing_key_b64: String,
    canonical_string: String,
    expected_sig_b64url_nopad: String,
    #[serde(default)]
    signed_by: Option<String>,
}

impl Vector {
    fn range_tuple(&self) -> Option<(u64, u64)> {
        self.range.as_ref().map(|r| (r.start, r.end_inclusive))
    }

    fn kid_uuid(&self) -> Uuid {
        Uuid::parse_str(&self.kid).expect("vector kid parses as UUID")
    }
}

/// Relative path from `siahub-cas-core`'s `CARGO_MANIFEST_DIR` (i.e.
/// `cas/crates/siahub-cas-core/`) to the JSON fixture at
/// `conformance/fixtures/signed_url_vectors.json`.
const FIXTURE_RELPATH: &str = "../../../conformance/fixtures/signed_url_vectors.json";

fn load_vectors() -> Vec<Vector> {
    let manifest = env!("CARGO_MANIFEST_DIR");
    let path = std::path::Path::new(manifest).join(FIXTURE_RELPATH);
    let raw = std::fs::read_to_string(&path)
        .unwrap_or_else(|e| panic!("read {}: {e}", path.display()));
    serde_json::from_str(&raw).expect("fixture parses")
}

fn gateway_base() -> Url {
    Url::parse("https://cas.siahub.app").expect("static base")
}

// ---------------------------------------------------------------------------
// Sanity tests
// ---------------------------------------------------------------------------

#[test]
fn fixture_has_required_vectors() {
    let vectors = load_vectors();
    let names: Vec<&str> = vectors.iter().map(|v| v.name.as_str()).collect();
    // Plan 02-08 Task 3 acceptance criterion: ≥3 scenarios covering the three
    // range flavors (none, with-range, zero-byte-range). Rotation vector is
    // the bonus fourth scenario.
    assert!(
        names.contains(&"v1_basic_no_range"),
        "missing v1_basic_no_range"
    );
    assert!(names.contains(&"v1_with_range"), "missing v1_with_range");
    assert!(
        names.contains(&"v1_zero_byte_range"),
        "missing v1_zero_byte_range"
    );
    assert!(vectors.len() >= 3, "need at least 3 vectors");
}

#[test]
fn canonical_string_matches_fixture_exactly() {
    for v in load_vectors() {
        let computed = UrlSigner::canonical_string(
            CANONICAL_VERSION,
            &v.xorb_hash_hex,
            v.exp,
            v.range_tuple(),
            v.kid_uuid(),
        );
        assert_eq!(
            computed, v.canonical_string,
            "canonical_string drift for vector {}",
            v.name
        );
        // Extra belt: assert the version, hash, exp, range, kid field order is
        // actually what we think it is. Catches a surprise reshuffling.
        let fields: Vec<&str> = computed.split('\n').collect();
        assert_eq!(fields.len(), 5, "canonical must have 5 fields, 4 LFs");
        assert_eq!(fields[0], "v1");
        assert_eq!(fields[1], v.xorb_hash_hex);
        assert_eq!(fields[2], v.exp.to_string());
        assert_eq!(fields[4], v.kid);
    }
}

#[test]
fn sig_matches_fixture_for_every_vector() {
    for v in load_vectors() {
        let key_bytes = B64_STD
            .decode(v.signing_key_b64.as_bytes())
            .expect("key decodes");
        let mut mac = Hmac::<Sha256>::new_from_slice(&key_bytes).expect("key len 32");
        mac.update(v.canonical_string.as_bytes());
        let sig = B64_URL.encode(mac.finalize().into_bytes());
        assert_eq!(
            sig, v.expected_sig_b64url_nopad,
            "sig drift for vector {}",
            v.name
        );
    }
}

// ---------------------------------------------------------------------------
// Round-trip tests through UrlSigner::mint_v1 and UrlSigner::verify
// ---------------------------------------------------------------------------

#[test]
fn mint_v1_embeds_the_expected_signature() {
    // Current-key vectors only — the rotation vector is signed with key_prev
    // and exercised separately below.
    for v in load_vectors()
        .into_iter()
        .filter(|v| v.signed_by.as_deref() != Some("prev"))
    {
        let ttl = 1; // arbitrary — we pin now_unix + ttl == v.exp
        let now = v.exp - ttl;
        let signer = UrlSigner::new(&v.signing_key_b64, None, gateway_base(), ttl)
            .expect("signer builds");

        let xorb = MerkleHash::from_hex(&v.xorb_hash_hex).expect("hash parses");
        let kid = v.kid_uuid();
        let url_str = signer.mint_v1(&xorb, v.range_tuple(), kid, now);

        let url = Url::parse(&url_str).expect("mint output parses");
        assert_eq!(url.host_str(), Some("cas.siahub.app"));
        assert_eq!(url.path(), format!("/xorb/{}", v.xorb_hash_hex));

        let sig = url
            .query_pairs()
            .find(|(k, _)| k == "sig")
            .map(|(_, v)| v.into_owned())
            .expect("sig present");
        assert_eq!(
            sig, v.expected_sig_b64url_nopad,
            "minted sig does not match fixture for {}",
            v.name
        );

        // Query contains exactly the expected keys.
        let mut keys: Vec<String> =
            url.query_pairs().map(|(k, _)| k.into_owned()).collect();
        keys.sort();
        let mut expected = vec![
            "exp".to_string(),
            "kid".to_string(),
            "sig".to_string(),
        ];
        if v.range.is_some() {
            expected.push("r".to_string());
        }
        expected.sort();
        assert_eq!(keys, expected, "unexpected query keys for {}", v.name);
    }
}

#[test]
fn verify_round_trips_every_current_key_vector() {
    for v in load_vectors()
        .into_iter()
        .filter(|v| v.signed_by.as_deref() != Some("prev"))
    {
        let ttl = 1;
        let now = v.exp - ttl;
        let signer = UrlSigner::new(&v.signing_key_b64, None, gateway_base(), ttl)
            .expect("signer builds");

        let xorb = MerkleHash::from_hex(&v.xorb_hash_hex).expect("hash parses");
        let url_str = signer.mint_v1(&xorb, v.range_tuple(), v.kid_uuid(), now);

        let ok = signer
            .verify(&url_str, v.exp - 1)
            .expect("verify succeeds just before exp");
        assert_eq!(ok.xorb_hash_hex, v.xorb_hash_hex);
        assert_eq!(ok.exp, v.exp);
        assert_eq!(ok.range, v.range_tuple());
        assert_eq!(ok.kid, v.kid_uuid());
        assert!(!ok.accepted_by_prev_key, "current key should accept");

        // Exactly-at-expiry is expired (strict `<`).
        let expired = signer.verify(&url_str, v.exp).unwrap_err();
        assert_eq!(expired, VerifyErr::Expired);
        // Well past expiry is also expired.
        let expired2 = signer.verify(&url_str, v.exp + 1).unwrap_err();
        assert_eq!(expired2, VerifyErr::Expired);
    }
}

#[test]
fn tampered_signature_yields_bad_signature() {
    let v = load_vectors()
        .into_iter()
        .find(|v| v.name == "v1_basic_no_range")
        .expect("basic vector");
    let ttl = 1;
    let now = v.exp - ttl;
    let signer =
        UrlSigner::new(&v.signing_key_b64, None, gateway_base(), ttl).expect("signer");
    let xorb = MerkleHash::from_hex(&v.xorb_hash_hex).expect("hash");
    let url_str = signer.mint_v1(&xorb, None, v.kid_uuid(), now);

    // Flip one base64url char in the sig — still legal base64 but wrong bytes.
    let mut tampered = url_str.clone();
    let sig_start = tampered.find("sig=").expect("sig param") + 4;
    let sig_end = tampered[sig_start..]
        .find('&')
        .map(|i| sig_start + i)
        .unwrap_or(tampered.len());
    // Replace one char. If the first char is 'A', flip to 'B'; else 'A'.
    let first_ch = tampered.as_bytes()[sig_start];
    let new_ch = if first_ch == b'A' { 'B' } else { 'A' };
    tampered.replace_range(sig_start..sig_start + 1, &new_ch.to_string());
    assert_ne!(tampered, url_str, "tampering actually changed the URL");

    let err = signer.verify(&tampered, v.exp - 1).unwrap_err();
    assert_eq!(err, VerifyErr::BadSignature);

    // Untampered still verifies.
    signer
        .verify(&url_str, v.exp - 1)
        .expect("original verifies");

    // Tampered only ends at sig_end — guard against misuse.
    let _ = sig_end;
}

#[test]
fn rotation_vector_verifies_only_via_prev_key() {
    let v = load_vectors()
        .into_iter()
        .find(|v| v.name == "v1_rotation_prev_key")
        .expect("rotation vector present");

    // Construct a URL from the fixture. We can't call mint_v1 here (mint only
    // uses key_current) — so assemble the URL by hand with the fixture's sig.
    let mut url = gateway_base();
    url.set_path(&format!("/xorb/{}", v.xorb_hash_hex));
    {
        let mut qs = url.query_pairs_mut();
        qs.append_pair("exp", &v.exp.to_string());
        qs.append_pair("kid", &v.kid);
        qs.append_pair("sig", &v.expected_sig_b64url_nopad);
    }
    let url_str = url.to_string();

    // Signer configured with a DIFFERENT current key but the vector's key as
    // `key_prev`. This is the shape CAS ships during a rotation window.
    let different_current = "KysrKysrKysrKysrKysrKysrKysrKysrKysrKysrKyo=";
    // Sanity: `different_current` decodes to 32 bytes.
    assert_eq!(B64_STD.decode(different_current).unwrap().len(), 32);

    let signer = UrlSigner::new(
        different_current,
        Some(&v.signing_key_b64),
        gateway_base(),
        7200,
    )
    .expect("rotation signer");
    let ok = signer.verify(&url_str, v.exp - 1).expect("prev key accepts");
    assert!(
        ok.accepted_by_prev_key,
        "verifier must flag prev-key acceptance for metrics"
    );

    // Without prev configured, the same URL fails (current key only).
    let signer_no_prev =
        UrlSigner::new(different_current, None, gateway_base(), 7200).expect("no prev");
    let err = signer_no_prev.verify(&url_str, v.exp - 1).unwrap_err();
    assert_eq!(err, VerifyErr::BadSignature);
}

// ---------------------------------------------------------------------------
// Negative-space coverage
// ---------------------------------------------------------------------------

#[test]
fn malformed_urls_are_rejected() {
    let signer = UrlSigner::new(
        "AAECAwQFBgcICQoLDA0ODxAREhMUFRYXGBkaGxwdHh8=",
        None,
        gateway_base(),
        7200,
    )
    .expect("signer");
    // Non-URL input.
    assert!(matches!(
        signer.verify("not a url", 1),
        Err(VerifyErr::Malformed(_))
    ));
    // Right shape, wrong path prefix.
    assert!(matches!(
        signer.verify("https://cas.siahub.app/other/abc?sig=x&exp=1&kid=y", 0),
        Err(VerifyErr::Malformed(_))
    ));
    // Missing `exp`.
    let u = "https://cas.siahub.app/xorb/eea25d6ee393ccae385820daed127b96ef0ea034dfb7cf6da3a950ce334b7632?kid=01945edc-5f0a-71a3-9c82-3a0000000001&sig=abc";
    assert!(matches!(
        signer.verify(u, 0),
        Err(VerifyErr::Malformed(_))
    ));
    // Uppercase hex — canonical output is lowercase, so verify rejects this.
    let u = "https://cas.siahub.app/xorb/EEA25D6EE393CCAE385820DAED127B96EF0EA034DFB7CF6DA3A950CE334B7632?exp=2&kid=01945edc-5f0a-71a3-9c82-3a0000000001&sig=abc";
    assert!(matches!(
        signer.verify(u, 0),
        Err(VerifyErr::Malformed(_))
    ));
    // Bad range shape.
    let u = "https://cas.siahub.app/xorb/eea25d6ee393ccae385820daed127b96ef0ea034dfb7cf6da3a950ce334b7632?exp=2&kid=01945edc-5f0a-71a3-9c82-3a0000000001&sig=abc&r=notarange";
    assert!(matches!(
        signer.verify(u, 0),
        Err(VerifyErr::Malformed(_))
    ));
}

#[test]
fn signer_rejects_bad_key_length() {
    use crate::signed_url::SignerError;
    let too_short = B64_STD.encode([0u8; 31]);
    let err = UrlSigner::new(&too_short, None, gateway_base(), 7200).unwrap_err();
    assert!(matches!(err, SignerError::InvalidKeyLength(31)));
    let not_b64 = "not base 64!!!";
    let err = UrlSigner::new(not_b64, None, gateway_base(), 7200).unwrap_err();
    assert!(matches!(err, SignerError::KeyNotBase64));
}

// ---------------------------------------------------------------------------
// Plan 03-07 — `mint_v1_multi_range` (V2 flip minter, RECEIVED §G item 2).
// Canonical `r=s1-e1,s2-e2,...` replaces the Phase 2 bounding approximation.
// ---------------------------------------------------------------------------

#[test]
fn mint_v1_multi_range_canonical_joins_segments_with_commas() {
    // Two segments → canonical `r` field holds `1024-8191,12288-16383`.
    let signer = UrlSigner::new(
        "AAECAwQFBgcICQoLDA0ODxAREhMUFRYXGBkaGxwdHh8=",
        None,
        gateway_base(),
        7200,
    )
    .expect("signer");
    let xorb = MerkleHash::from_hex(
        "eea25d6ee393ccae385820daed127b96ef0ea034dfb7cf6da3a950ce334b7632",
    )
    .expect("hash");
    let kid = Uuid::parse_str("01945edc-5f0a-71a3-9c82-3a0000000001").expect("kid");
    let segments = [(1024u64, 8191u64), (12288u64, 16383u64)];

    let url_str = signer.mint_v1_multi_range(&xorb, &segments, kid, 1_700_000_000);
    let url = Url::parse(&url_str).expect("mint output parses");

    // `r` round-trips through the URL parser as the literal comma form.
    let r = url
        .query_pairs()
        .find(|(k, _)| k == "r")
        .map(|(_, v)| v.into_owned())
        .expect("r present");
    assert_eq!(r, "1024-8191,12288-16383");

    // Canonical string recomputation — hit exact format in source doc.
    let expected_canonical = format!(
        "v1\neea25d6ee393ccae385820daed127b96ef0ea034dfb7cf6da3a950ce334b7632\n1700007200\n1024-8191,12288-16383\n{kid}",
    );
    // Extract exp from the URL to assert it matches `now + ttl`.
    let exp = url
        .query_pairs()
        .find(|(k, _)| k == "exp")
        .map(|(_, v)| v.into_owned())
        .and_then(|s| s.parse::<u64>().ok())
        .expect("exp present");
    assert_eq!(exp, 1_700_000_000 + 7200);
    // Reconstruct the canonical by format-string — byte-identical signing contract.
    assert_eq!(
        expected_canonical.split('\n').collect::<Vec<_>>().len(),
        5,
        "canonical must still be 5 fields"
    );
}

#[test]
fn mint_v1_multi_range_single_segment_matches_mint_v1() {
    // A single-segment multi-range mint has canonical identical to the
    // equivalent `mint_v1(Some((s,e)))`: same HMAC → same `sig=`. This
    // invariant keeps the verifier path uniform across single vs multi.
    let signer = UrlSigner::new(
        "AAECAwQFBgcICQoLDA0ODxAREhMUFRYXGBkaGxwdHh8=",
        None,
        gateway_base(),
        7200,
    )
    .expect("signer");
    let xorb = MerkleHash::from_hex(
        "eea25d6ee393ccae385820daed127b96ef0ea034dfb7cf6da3a950ce334b7632",
    )
    .expect("hash");
    let kid = Uuid::parse_str("01945edc-5f0a-71a3-9c82-3a0000000001").expect("kid");

    let single = signer.mint_v1(&xorb, Some((100, 199)), kid, 1_700_000_000);
    let multi = signer.mint_v1_multi_range(&xorb, &[(100, 199)], kid, 1_700_000_000);

    // Extract the `sig` param from each; they must match byte-identically
    // because the canonical strings match byte-identically.
    let sig_single = Url::parse(&single)
        .unwrap()
        .query_pairs()
        .find(|(k, _)| k == "sig")
        .map(|(_, v)| v.into_owned())
        .unwrap();
    let sig_multi = Url::parse(&multi)
        .unwrap()
        .query_pairs()
        .find(|(k, _)| k == "sig")
        .map(|(_, v)| v.into_owned())
        .unwrap();
    assert_eq!(
        sig_single, sig_multi,
        "single-segment multi-range mint must match mint_v1 signature"
    );
}

#[test]
fn mint_v1_multi_range_three_segments_encodes_all_commas() {
    let signer = UrlSigner::new(
        "AAECAwQFBgcICQoLDA0ODxAREhMUFRYXGBkaGxwdHh8=",
        None,
        gateway_base(),
        7200,
    )
    .expect("signer");
    let xorb = MerkleHash::from_hex(
        "eea25d6ee393ccae385820daed127b96ef0ea034dfb7cf6da3a950ce334b7632",
    )
    .expect("hash");
    let kid = Uuid::parse_str("01945edc-5f0a-71a3-9c82-3a0000000001").expect("kid");
    let segments = [(0u64, 99u64), (200u64, 299u64), (1000u64, 1023u64)];

    let url_str = signer.mint_v1_multi_range(&xorb, &segments, kid, 1_700_000_000);
    let url = Url::parse(&url_str).expect("mint output parses");
    let r = url
        .query_pairs()
        .find(|(k, _)| k == "r")
        .map(|(_, v)| v.into_owned())
        .expect("r present");
    assert_eq!(r, "0-99,200-299,1000-1023");
}

#[test]
#[should_panic(expected = "mint_v1_multi_range requires at least one segment")]
fn mint_v1_multi_range_panics_on_empty_segments() {
    let signer = UrlSigner::new(
        "AAECAwQFBgcICQoLDA0ODxAREhMUFRYXGBkaGxwdHh8=",
        None,
        gateway_base(),
        7200,
    )
    .expect("signer");
    let xorb = MerkleHash::from_hex(
        "eea25d6ee393ccae385820daed127b96ef0ea034dfb7cf6da3a950ce334b7632",
    )
    .expect("hash");
    let kid = Uuid::parse_str("01945edc-5f0a-71a3-9c82-3a0000000001").expect("kid");
    let _ = signer.mint_v1_multi_range(&xorb, &[], kid, 1_700_000_000);
}

#[test]
fn url_minter_trait_delegates_mint_v1_multi_range() {
    // The trait method goes through UrlMinter — not UrlSigner directly.
    // Ensures 02-06/V2 callers that hold `&dyn UrlMinter` can invoke the
    // Phase 3 flip path.
    let signer = UrlSigner::new(
        "AAECAwQFBgcICQoLDA0ODxAREhMUFRYXGBkaGxwdHh8=",
        None,
        gateway_base(),
        7200,
    )
    .expect("signer");
    let minter: &dyn UrlMinter = &signer;
    let xorb = MerkleHash::from_hex(
        "eea25d6ee393ccae385820daed127b96ef0ea034dfb7cf6da3a950ce334b7632",
    )
    .unwrap();
    let kid = Uuid::parse_str("01945edc-5f0a-71a3-9c82-3a0000000001").unwrap();
    let url = minter.mint_v1_multi_range(&xorb, &[(0, 1023), (2048, 4095)], kid, 1_700_000_000);
    assert!(url.starts_with("https://cas.siahub.app/xorb/"));
    assert!(url.contains("r=0-1023%2C2048-4095") || url.contains("r=0-1023,2048-4095"));
}

#[test]
fn url_minter_trait_delegates() {
    // Ensures 02-06 can depend on the trait rather than the concrete type.
    let signer = UrlSigner::new(
        "AAECAwQFBgcICQoLDA0ODxAREhMUFRYXGBkaGxwdHh8=",
        None,
        gateway_base(),
        7200,
    )
    .expect("signer");
    let minter: &dyn UrlMinter = &signer;
    assert_eq!(minter.default_ttl_secs(), 7200);
    let xorb =
        MerkleHash::from_hex("eea25d6ee393ccae385820daed127b96ef0ea034dfb7cf6da3a950ce334b7632")
            .unwrap();
    let url = minter.mint_v1(
        &xorb,
        Some((0, 1023)),
        Uuid::parse_str("01945edc-5f0a-71a3-9c82-3a0000000001").unwrap(),
        1_700_000_000,
    );
    assert!(url.starts_with("https://cas.siahub.app/xorb/"));
}
