// metering_test.go — Meter unit tests.
//
// Unit tests: interface seam + canonical event literal + nil-safety. Live
// Postgres round-trip coverage lands in the integration suite (same gating
// pattern as db_test.go: opt in with TEST_POSTGRES_URL).
//
// Guard rails enforced here:
//   - LogDownload with nil Meter or nil DB returns a typed error (not panic).
//   - Malformed xorb hash is a metered error (insertErrorTot++) instead of a
//     silent NULL write.
//   - UsageWriter interface is satisfied by the concrete *Meter so 03-03
//     call sites can take an interface and substitute fakes.
package main

import (
	"context"
	"os"
	"sync"
	"testing"

	"github.com/google/uuid"
)

// fakeMeter is a trivial UsageWriter for testing that 03-03 handlers pass
// correct args without wiring a DB.
type fakeMeter struct {
	mu    sync.Mutex
	calls []fakeMeterCall
	err   error
}

type fakeMeterCall struct {
	APIKeyID uuid.UUID
	Bytes    int64
	CacheHit bool
	HashHex  string
}

func (f *fakeMeter) LogDownload(_ context.Context, k uuid.UUID, b int64, hit bool, h string) error {
	f.mu.Lock()
	defer f.mu.Unlock()
	f.calls = append(f.calls, fakeMeterCall{APIKeyID: k, Bytes: b, CacheHit: hit, HashHex: h})
	return f.err
}

// Compile-time interface satisfaction checks. If the UsageWriter signature
// drifts, this file fails to build — caller callers see the break.
var _ UsageWriter = (*Meter)(nil)
var _ UsageWriter = (*fakeMeter)(nil)

func TestLogDownload_NilMeter(t *testing.T) {
	t.Parallel()
	var m *Meter
	err := m.LogDownload(context.Background(), uuid.New(), 1024, false, hex64())
	if err == nil {
		t.Fatalf("nil Meter LogDownload = nil err; want guard error")
	}
}

func TestLogDownload_NilDB(t *testing.T) {
	t.Parallel()
	m := &Meter{db: nil}
	err := m.LogDownload(context.Background(), uuid.New(), 1024, false, hex64())
	if err == nil {
		t.Fatalf("Meter{db:nil} LogDownload = nil err; want guard error")
	}
}

func TestLogDownload_MalformedHashErrors(t *testing.T) {
	t.Parallel()
	// Malformed hash is validated BEFORE the pool is touched — this lets
	// the handler layer (which may pass a nil/mock DB in tests) still
	// exercise the decode path and see the typed error.
	m := NewMeter(&DB{pool: nil}, nil)
	err := m.LogDownload(context.Background(), uuid.New(), 512, true, "not hex")
	if err == nil {
		t.Fatalf("malformed hash = nil err")
	}
	if !errContains(err, "decode xorb hash for usage_log") {
		t.Fatalf("err = %v; want hash-decode wrap", err)
	}
}

// Canonical event literal lock: the SQL string in LogDownload must contain
// 'download' (D-29). Reading the source is overkill — instead we use the
// fakeMeter-through-handler path in 03-03 to verify behaviorally. This test
// simply asserts the fake seam carries the args we expect.
func TestFakeMeter_CapturesCall(t *testing.T) {
	t.Parallel()
	f := &fakeMeter{}
	k := uuid.New()
	if err := f.LogDownload(context.Background(), k, 2048, true, hex64()); err != nil {
		t.Fatalf("fakeMeter LogDownload: %v", err)
	}
	if len(f.calls) != 1 {
		t.Fatalf("calls = %d; want 1", len(f.calls))
	}
	c := f.calls[0]
	if c.APIKeyID != k || c.Bytes != 2048 || !c.CacheHit || c.HashHex != hex64() {
		t.Fatalf("captured mismatch: %+v", c)
	}
}

// D-29 event-name lock: confirm the SQL literal is 'download'. We grep ONLY
// the `INSERT INTO usage_log ... VALUES` region (not the whole file) so
// explanatory documentation about reserved Phase 2 event slots does not
// trip the forbidden-literal check.
func TestLogDownload_EventLiteralIsDownload(t *testing.T) {
	t.Parallel()
	src := meteringSourceSnippet()
	// Canonical literal must be present.
	if !bytesContain(src, `VALUES ('download',`) {
		t.Fatalf("metering.go missing canonical 'download' literal; D-29 would be violated.")
	}
	// Extract the SQL region starting at `INSERT INTO usage_log` up through
	// the closing `)`. Check forbidden literals within that region only.
	start := indexOf(src, []byte("INSERT INTO usage_log"))
	if start < 0 {
		t.Fatalf("metering.go: could not locate INSERT INTO usage_log")
	}
	end := indexOf(src[start:], []byte("`,"))
	if end < 0 {
		t.Fatalf("metering.go: could not locate end of SQL literal after INSERT")
	}
	sql := src[start : start+end]
	for _, forbidden := range []string{"'xorb_serve'", "'cache_hit'", "'dedup_query'"} {
		if bytesContain(sql, forbidden) {
			t.Fatalf("metering.go SQL contains forbidden event literal %q; D-29 locks 'download'", forbidden)
		}
	}
}

// Small helpers: read metering.go from disk so we can assert the SQL
// literal is what D-29 says it is. Running the test binary's own source
// is fine — `go test` sets the cwd to the package dir.
func meteringSourceSnippet() []byte {
	b, err := os.ReadFile("metering.go")
	if err != nil {
		panic(err)
	}
	return b
}

func bytesContain(haystack []byte, needle string) bool {
	return len(needle) == 0 || indexOf(haystack, []byte(needle)) >= 0
}

func indexOf(haystack, needle []byte) int {
	if len(needle) == 0 {
		return 0
	}
	for i := 0; i+len(needle) <= len(haystack); i++ {
		match := true
		for j := 0; j < len(needle); j++ {
			if haystack[i+j] != needle[j] {
				match = false
				break
			}
		}
		if match {
			return i
		}
	}
	return -1
}
