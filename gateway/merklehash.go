// Package main — merklehash.go.
//
// Port of xet-core's `DataHash` canonical hash algorithm, from
// `xet_core_structures/src/merklehash/data_hash.rs`. Without this, the
// gateway cannot perform hash-verify-on-write (GATE-07 / PITFALL P11) — the
// load-bearing defense against cache poisoning.
//
// ALGORITHM (authoritative — do NOT alter without re-running the A4 probe):
//
//  1. Compute `blake3::keyed_hash(DATA_KEY, data)` — 32-byte digest.
//     DATA_KEY is a fixed 32-byte constant copied verbatim from
//     `data_hash.rs:288-291`. Any drift breaks hash parity with Rust CAS.
//  2. Reinterpret the 32-byte digest as four little-endian u64 words.
//     (On LE hosts, this is zero-copy — byte order is preserved.)
//  3. Hex-encode the result by printing each u64 with `%016x`. Because
//     we store them LE in memory, printing as big-endian hex equals
//     "reverse each 8-byte group, then hex-encode" — notes.md gotcha #1
//     and PITFALL P1.
//
// Reference construction in Rust:
//
//    let digest = blake3::keyed_hash(&DATA_KEY, slice);                 // step 1
//    let data_hash = DataHash::from(digest.as_bytes());                 // step 2 (transmute)
//    format!("{:016x}{:016x}{:016x}{:016x}",                            // step 3
//        w[0].to_le(), w[1].to_le(), w[2].to_le(), w[3].to_le())
//
// The A4 probe artifact at `.planning/phases/03-siahub-gateway-core/
// 03-05-A4-PROBE.md` records the probe verdict PORTED and the three canary
// tests that prove byte-parity with Rust.
package main

import (
	"encoding/binary"
	"encoding/hex"
	"fmt"

	"lukechampine.com/blake3"
)

// dataKey is the 32-byte BLAKE3 keying material for xet-core's DataHash leaf
// construction. Copied verbatim from
// `.cache/refs/xet-core/xet_core_structures/src/merklehash/data_hash.rs:288`.
//
// DO NOT MODIFY. Any change silently produces hashes that disagree with
// the Rust CAS and with every xet-core client. Treat as load-bearing.
var dataKey = [32]byte{
	102, 151, 245, 119, 91, 149, 80, 222,
	49, 53, 203, 172, 165, 151, 24, 28,
	157, 228, 33, 16, 155, 235, 43, 88,
	180, 208, 176, 75, 147, 173, 242, 41,
}

// MerkleHashHex computes the xet-core canonical leaf hash of `data` and
// returns its 64-char lowercase-hex representation.
//
// Byte-identical to Rust `xet_core_structures::merklehash::compute_data_hash`
// followed by `.hex()`. Proven by the canary tests in merklehash_test.go.
//
// For streaming workloads (e.g. piping a multi-GiB xorb from Sia through the
// cache Put path), use `NewMerkleHasher` + `Hasher.Write` + `Hasher.Hex`.
func MerkleHashHex(data []byte) string {
	h := blake3.New(32, dataKey[:])
	_, _ = h.Write(data)
	var digest [32]byte
	h.Sum(digest[:0])
	return digestToHex(digest)
}

// digestToHex performs the "reverse each 8-byte group, then hex-encode" step
// that the Rust side achieves implicitly via `u64.to_le()` + `%016x`.
//
// Concretely: the 32-byte BLAKE3 output is read as four little-endian u64
// words, each of which is then formatted big-endian as 16 hex chars. The net
// effect on the byte level is that bytes [0..8] appear in the hex string in
// reverse order, then [8..16], etc.
func digestToHex(d [32]byte) string {
	var out [64]byte
	for group := 0; group < 4; group++ {
		word := binary.LittleEndian.Uint64(d[group*8 : (group+1)*8])
		hex.Encode(out[group*16:(group+1)*16], uint64ToBE(word))
	}
	return string(out[:])
}

// uint64ToBE returns `v` in big-endian byte order. Factored out so the
// swap discipline is centralised (one place to audit).
func uint64ToBE(v uint64) []byte {
	var b [8]byte
	binary.BigEndian.PutUint64(b[:], v)
	return b[:]
}

// ParseMerkleHashHex decodes the xet-core canonical hex representation back
// into a 32-byte digest. Round-trip invariant:
//
//    want := "eea25d6e...7632"
//    got, err := ParseMerkleHashHex(want); require.NoError(t, err)
//    require.Equal(t, want, DigestToHex(got))
//
// Used by cache.go to verify that a hex hash argument parses cleanly before
// accepting a Put. Returns an error on length-mismatch or non-hex chars.
func ParseMerkleHashHex(h string) ([32]byte, error) {
	var out [32]byte
	if len(h) != 64 {
		return out, fmt.Errorf("merkle hash must be 64 hex chars, got %d", len(h))
	}
	// Decode each 16-char group as a big-endian u64, then store little-endian
	// in the output — this is the exact inverse of digestToHex.
	for group := 0; group < 4; group++ {
		var raw [8]byte
		if _, err := hex.Decode(raw[:], []byte(h[group*16:(group+1)*16])); err != nil {
			return out, fmt.Errorf("merkle hash: invalid hex group %d: %w", group, err)
		}
		word := binary.BigEndian.Uint64(raw[:])
		binary.LittleEndian.PutUint64(out[group*8:(group+1)*8], word)
	}
	return out, nil
}

// DigestToHex is the exported inverse of ParseMerkleHashHex, for tests and
// any caller that holds a raw 32-byte digest it wants to canonicalise.
func DigestToHex(d [32]byte) string { return digestToHex(d) }

// MerkleHasher is the streaming face of the hash algorithm. Implements
// io.Writer so it can be wired into `io.TeeReader` in cache.go without
// buffering the whole xorb in RAM (GATE-08).
//
// Thread-compatible (NOT thread-safe): one hasher per goroutine. The cache
// Put path constructs a fresh hasher per call, so this is fine.
type MerkleHasher struct {
	h *blake3.Hasher
}

// NewMerkleHasher returns a fresh streaming hasher keyed with DATA_KEY.
func NewMerkleHasher() *MerkleHasher {
	return &MerkleHasher{h: blake3.New(32, dataKey[:])}
}

// Write feeds bytes into the hasher. Always returns `(len(p), nil)` — BLAKE3
// never short-writes and never errors.
func (m *MerkleHasher) Write(p []byte) (int, error) { return m.h.Write(p) }

// Hex finalises the hash and returns the canonical 64-char lowercase hex.
// Safe to call multiple times — BLAKE3 supports it; each call returns the
// same hex until another Write mutates state.
func (m *MerkleHasher) Hex() string {
	var digest [32]byte
	m.h.Sum(digest[:0])
	return digestToHex(digest)
}

// Reset returns the hasher to its zero-input state so callers can reuse it
// without allocating a new BLAKE3 Hasher. Unused by the current cache path
// (fresh hasher per Put) but cheap to keep.
func (m *MerkleHasher) Reset() { m.h.Reset() }
