//! hash-encoding canary — the first test written in, the load-bearing
//! guard against the silent.md ("xet-core does NOT straight-hex-
//! encode merkle hashes; it reverses each 8-byte group before hex-encoding").
//! A passing canary means:
//! - `xet_core_structures::merklehash::MerkleHash::from_hex` round-trips our
//! canonical fixture hash `eea25d6e...`;
//! - the codec is NOT straight byte-to-hex (i.e. `format!("{:02x}", b)` over
//! raw bytes produces a DIFFERENT string).
//! If this test ever fails it means either the fixture hash drifted (bump
//! revision only with a planning step per CONFORMANCE-AUDIT.md) OR the
//! `xet_core_structures` crate broke its hex codec (the README explicitly
//! warns against public use — any bump requires re-running the canary).

use openweights_cas_proto::merklehash::MerkleHash;

#[test]
fn p1_reference_xorb_hash_roundtrip() {
    const REF_HEX: &str = "eea25d6ee393ccae385820daed127b96ef0ea034dfb7cf6da3a950ce334b7632";

    // Same const re-exported from the conformance crate; cross-check the two
    // don't drift out of sync.
    assert_eq!(
        REF_HEX,
        openweights_conformance::REFERENCE_XORB_HASH_HEX,
        "conformance crate must agree with fixture hash constant",
    );

    let h = MerkleHash::from_hex(REF_HEX).expect("MerkleHash parses reference hash");
    assert_eq!(
        h.hex(),
        REF_HEX,
        "from_hex -> hex round-trip must be lossless"
    );
}

#[test]
fn p1_codec_is_not_straight_byte_hex() {
    const REF_HEX: &str = "eea25d6ee393ccae385820daed127b96ef0ea034dfb7cf6da3a950ce334b7632";
    let h = MerkleHash::from_hex(REF_HEX).unwrap();

    // Straight hex of the raw byte representation MUST differ from the
    // canonical hex — if they're equal, the codec is a no-op and is not
    // mitigated by the crate (we'd be hand-rolling byte-reversal elsewhere
    // without realising it).
    let raw_bytes: [u8; 32] = h.into();
    let naive_hex: String = raw_bytes.iter().map(|b| format!("{b:02x}")).collect();
    assert_ne!(
        naive_hex, REF_HEX,
        "MerkleHash codec must NOT be straight byte-hex — P1 is not mitigated otherwise"
    );
}

#[test]
fn p1_hex_is_lowercase_64_chars() {
    const REF_HEX: &str = "eea25d6ee393ccae385820daed127b96ef0ea034dfb7cf6da3a950ce334b7632";
    let h = MerkleHash::from_hex(REF_HEX).unwrap();
    let enc = h.hex();
    assert_eq!(enc.len(), 64, "canonical hex is 64 chars");
    assert!(
        enc.chars().all(|c| c.is_ascii_hexdigit() && !c.is_ascii_uppercase()),
        "canonical hex is lowercase-only"
    );
}
