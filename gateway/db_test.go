// db_test.go — Postgres adapter tests.
//
// Split into two layers per the plan:
//
//   - Pure unit tests (ALWAYS run) for the hex decoder + NewDB error paths.
//     These are the 80% confidence win without a Postgres container.
//   - Integration tests (tag `integration`) for the live `LookupXorb` path.
//     Gated on env var `TEST_POSTGRES_URL` so developers can opt in via
//     `TEST_POSTGRES_URL=postgres://... go test -tags=integration ./...`
//     when they have a live DB; CI skips without the env var.
//
// Keeping the integration test in the same package (not `_test`) lets it
// reach `decodeXorbHash` directly, which avoids duplicating the hash-decode
// dance in the test seed path.
package main

import (
	"bytes"
	"context"
	"encoding/hex"
	"testing"
)

func TestDecodeXorbHash_Valid(t *testing.T) {
	t.Parallel()
	// Known fixture — 64-char lowercase hex of a 32-byte pattern.
	input := "00112233445566778899aabbccddeeff00112233445566778899aabbccddeeff"
	got, err := decodeXorbHash(input)
	if err != nil {
		t.Fatalf("decodeXorbHash: %v", err)
	}
	want, _ := hex.DecodeString(input)
	if !bytes.Equal(got, want) {
		t.Fatalf("decoded bytes mismatch\n got: %x\nwant: %x", got, want)
	}
	if len(got) != 32 {
		t.Fatalf("decoded length = %d; want 32", len(got))
	}
}

func TestDecodeXorbHash_UppercaseAccepted(t *testing.T) {
	t.Parallel()
	// The verifier rejects uppercase at the URL layer, but the DB helper is
	// permissive — it handles either case, so an internal caller passing an
	// already-uppercased hash (stringly-typed error path) does not explode
	// at the decoder.
	_, err := decodeXorbHash("AABBCCDDEEFF00112233445566778899AABBCCDDEEFF00112233445566778899")
	if err != nil {
		t.Fatalf("uppercase hex rejected: %v", err)
	}
}

func TestDecodeXorbHash_WrongLength(t *testing.T) {
	t.Parallel()
	cases := []string{
		"",                         // empty
		"0011",                     // too short
		"0011" + "22334455" + "ab", // still too short
		// 66 chars — one byte too long:
		"00112233445566778899aabbccddeeff00112233445566778899aabbccddeeff00",
	}
	for _, c := range cases {
		c := c
		t.Run(c, func(t *testing.T) {
			t.Parallel()
			_, err := decodeXorbHash(c)
			if err == nil {
				t.Fatalf("decodeXorbHash(%q) = nil err; want length error", c)
			}
		})
	}
}

func TestDecodeXorbHash_NonHex(t *testing.T) {
	t.Parallel()
	// 64 chars but contains 'g' — not a hex digit.
	bad := "g0112233445566778899aabbccddeeff00112233445566778899aabbccddeeff"
	_, err := decodeXorbHash(bad)
	if err == nil {
		t.Fatalf("decodeXorbHash(non-hex) = nil err; want 'invalid hex char'")
	}
}

func TestNewDB_EmptyConnStr(t *testing.T) {
	t.Parallel()
	_, err := NewDB(context.Background(), "")
	if err == nil {
		t.Fatalf("NewDB(\"\") = nil err; want 'empty' error")
	}
}

func TestNewDB_MalformedConnStr(t *testing.T) {
	t.Parallel()
	_, err := NewDB(context.Background(), "this is not a postgres url")
	if err == nil {
		t.Fatalf("NewDB(malformed) = nil err; want parse error")
	}
}

// LookupXorb on a nil pool returns a typed error instead of panicking.
// Cheap invariant check — the handler layer shouldn't call LookupXorb on
// a nil DB, but if it does the error is recoverable (500 to client) not
// a process crash.
func TestLookupXorb_NilDB(t *testing.T) {
	t.Parallel()
	var d *DB
	_, _, err := d.LookupXorb(context.Background(), hex64())
	if err == nil {
		t.Fatalf("nil-DB LookupXorb = nil err; want guard error")
	}
}

func TestLookupXorb_MalformedHash(t *testing.T) {
	t.Parallel()
	// Even with a non-nil DB, malformed hash short-circuits before touching
	// the pool. Construct a DB with nil pool and verify the hash-decode path
	// runs first (we'd panic on the pool.QueryRow otherwise).
	d := &DB{pool: nil}
	_, _, err := d.LookupXorb(context.Background(), "nope")
	if err == nil {
		t.Fatalf("malformed hash = nil err")
	}
	// Hash-length error, not the nil-pool "on nil DB" guard.
	if !errContains(err, "64 hex chars") {
		t.Fatalf("err = %v; want length error", err)
	}
}

func TestQueryXorbPinned_AliasOfLookupXorb(t *testing.T) {
	t.Parallel()
	d := &DB{pool: nil}
	_, err := d.QueryXorbPinned(context.Background(), "nope")
	if err == nil {
		t.Fatalf("QueryXorbPinned(malformed) = nil err")
	}
}
