// Package main — merklehash_test.go.
//
// Canary-suite for the Go port of xet-core's DataHash algorithm. Mirrors the
// three tests in `conformance/tests/p1_hash_canary.rs` verbatim and adds two
// more that exercise the streaming + byte-parity paths.
//
// If any test in this file fails, DO NOT ship the gateway — hash parity with
// the Rust CAS is load-bearing for cache-verify (GATE-07). Re-open the A4
// probe and investigate.
package main

import (
	"bytes"
	"encoding/hex"
	"io"
	"strings"
	"testing"
)

// REF_HEX is the pinned reference hash from `xet-team/xet-spec-reference-files`,
// re-exported by `siahub_conformance::REFERENCE_XORB_HASH_HEX`. Any drift in
// this constant requires a planning step per CONFORMANCE-AUDIT.md — do NOT
// change it casually.
const REF_HEX = "eea25d6ee393ccae385820daed127b96ef0ea034dfb7cf6da3a950ce334b7632"

// ENDIANNESS_REF_HEX is the test vector from data_hash.rs:549-554 that
// exercises the hex-codec endianness guarantee. Given the raw 32 bytes, the
// canonical hex MUST be this string — the mapping is literally "reverse each
// 8-byte group, then hex-encode".
const ENDIANNESS_REF_HEX = "d6834b04843aaf16f29903e2428a99be635099f9ea5056cc4e95e7ec8a41509f"

// endiannessRefRawBytes is the raw 32-byte representation paired with
// ENDIANNESS_REF_HEX. If `DigestToHex(endiannessRefRawBytes) !=
// ENDIANNESS_REF_HEX`, the byte-reversal-per-8-byte-group logic is broken.
var endiannessRefRawBytes = [32]byte{
	22, 175, 58, 132, 4, 75, 131, 214,
	190, 153, 138, 66, 226, 3, 153, 242,
	204, 86, 80, 234, 249, 153, 80, 99,
	159, 80, 65, 138, 236, 231, 149, 78,
}

// TestMerkleHash_P1_RefHexRoundTrip — mirrors
// conformance/tests/p1_hash_canary.rs::p1_reference_xorb_hash_roundtrip.
// Parse canonical hex → re-encode → byte-identical.
func TestMerkleHash_P1_RefHexRoundTrip(t *testing.T) {
	raw, err := ParseMerkleHashHex(REF_HEX)
	if err != nil {
		t.Fatalf("ParseMerkleHashHex(REF_HEX) failed: %v", err)
	}
	got := DigestToHex(raw)
	if got != REF_HEX {
		t.Fatalf("round-trip drift:\nwant %s\ngot  %s", REF_HEX, got)
	}
}

// TestMerkleHash_P1_NotStraightByteHex — mirrors
// conformance/tests/p1_hash_canary.rs::p1_codec_is_not_straight_byte_hex.
// Naive hex of the raw 32 bytes MUST differ from canonical hex — otherwise
// the byte-reversal discipline has silently regressed to a no-op.
func TestMerkleHash_P1_NotStraightByteHex(t *testing.T) {
	raw, err := ParseMerkleHashHex(REF_HEX)
	if err != nil {
		t.Fatalf("ParseMerkleHashHex(REF_HEX) failed: %v", err)
	}
	naive := hex.EncodeToString(raw[:])
	if naive == REF_HEX {
		t.Fatalf("naive byte-hex matches canonical hex — byte-reversal-per-8-byte-group mitigation NOT in place")
	}
	// Sanity: the naive hex should be exactly the reverse of each 8-byte
	// group — that's the whole point of the codec.
	t.Logf("canonical = %s", REF_HEX)
	t.Logf("naive     = %s", naive)
}

// TestMerkleHash_P1_HexIsLowercase64 — hex output is always exactly 64
// lowercase hex chars. xet-core assumes this; any uppercase bleed would
// break comparisons against signed-URL hash params which the CAS always
// mints lowercase.
func TestMerkleHash_P1_HexIsLowercase64(t *testing.T) {
	raw, err := ParseMerkleHashHex(REF_HEX)
	if err != nil {
		t.Fatalf("ParseMerkleHashHex(REF_HEX) failed: %v", err)
	}
	got := DigestToHex(raw)
	if len(got) != 64 {
		t.Fatalf("canonical hex must be 64 chars, got %d", len(got))
	}
	if strings.ToLower(got) != got {
		t.Fatalf("canonical hex must be lowercase, got %s", got)
	}
}

// TestMerkleHash_EndiannessRef — uses the exact `[u8; 32]` ↔ hex pair from
// data_hash.rs:549-554 to pin the codec's byte-reversal discipline. If THIS
// test fails, the port is wrong at the hex-codec layer regardless of how
// BLAKE3 behaves.
func TestMerkleHash_EndiannessRef(t *testing.T) {
	got := DigestToHex(endiannessRefRawBytes)
	if got != ENDIANNESS_REF_HEX {
		t.Fatalf("endianness-ref codec drift:\nwant %s\ngot  %s", ENDIANNESS_REF_HEX, got)
	}
	// And back: parse -> bytes match.
	parsed, err := ParseMerkleHashHex(ENDIANNESS_REF_HEX)
	if err != nil {
		t.Fatalf("parse endianness ref: %v", err)
	}
	if parsed != endiannessRefRawBytes {
		t.Fatalf("parse endianness-ref: got bytes %x, want %x", parsed, endiannessRefRawBytes)
	}
}

// TestMerkleHash_StreamingMatchesOneShot — a multi-chunk Write sequence
// through NewMerkleHasher MUST yield the same hex as a single MerkleHashHex
// over the concatenated payload. This is the invariant cache.go relies on:
// the TeeReader path and the one-shot path both emit bytes the cache can
// trust.
func TestMerkleHash_StreamingMatchesOneShot(t *testing.T) {
	payload := bytes.Repeat([]byte("xet-core-keyed-blake3-test-vector-"), 1024)
	oneShot := MerkleHashHex(payload)

	h := NewMerkleHasher()
	// Feed in uneven chunks to exercise BLAKE3's internal state-machine.
	r := bytes.NewReader(payload)
	buf := make([]byte, 37) // prime number to avoid block alignment
	for {
		n, err := r.Read(buf)
		if n > 0 {
			if _, werr := h.Write(buf[:n]); werr != nil {
				t.Fatalf("MerkleHasher.Write: %v", werr)
			}
		}
		if err == io.EOF {
			break
		}
		if err != nil {
			t.Fatalf("read: %v", err)
		}
	}
	streamed := h.Hex()
	if streamed != oneShot {
		t.Fatalf("streaming != one-shot:\nstreamed = %s\noneShot  = %s", streamed, oneShot)
	}
}

// TestMerkleHash_EmptyInput — hashing an empty byte slice still produces a
// deterministic canonical hex. Useful for detecting "zero-length short-
// circuit" bugs in the cache path.
func TestMerkleHash_EmptyInput(t *testing.T) {
	got := MerkleHashHex(nil)
	if len(got) != 64 {
		t.Fatalf("empty-input hex must be 64 chars, got %d (%q)", len(got), got)
	}
	// Empty input hash is well-defined — not asserted against a golden
	// value (we'd need to run the Rust side to get one), but stability is
	// asserted by rerunning and comparing.
	again := MerkleHashHex([]byte{})
	if again != got {
		t.Fatalf("empty-input hash drifts between calls: %s vs %s", got, again)
	}
}

// TestParseMerkleHashHex_Rejects — malformed inputs MUST return an error,
// NOT a silent zero value. cache.go treats nil-error as "hash parsed OK" and
// proceeds to Put.
func TestParseMerkleHashHex_Rejects(t *testing.T) {
	cases := []struct {
		name string
		in   string
	}{
		{"empty", ""},
		{"short", "abc"},
		{"too-long", REF_HEX + "00"},
		{"non-hex", strings.Repeat("z", 64)},
	}
	for _, c := range cases {
		c := c
		t.Run(c.name, func(t *testing.T) {
			if _, err := ParseMerkleHashHex(c.in); err == nil {
				t.Fatalf("ParseMerkleHashHex(%q) must error, got nil", c.in)
			}
		})
	}
}
