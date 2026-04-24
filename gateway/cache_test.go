// Package main — cache_test.go.
// Whole-xorb disk LRU test suite. Covers (disk-backed, size-capped),
// (hash-verify-on-write), (streaming / no whole-xorb RAM),
// plus atomic-write invariants + LRU correctness + concurrent-reader safety.
// Tests run with `go test -race ./...` to catch the cache-index concurrency
// invariants.
package main

import (
	"bytes"
	"context"
	"errors"
	"io"
	"os"
	"path/filepath"
	"strings"
	"sync"
	"testing"
	"time"

	"github.com/prometheus/client_golang/prometheus"
	dto "github.com/prometheus/client_model/go"
	"go.sia.tech/core/types"
)

// -----------------------------------------------------------------------------
// Helpers

// newTestCache builds a Cache rooted under a fresh t.TempDir. maxBytes=10 MiB
// by default; override by passing a value.
func newTestCache(t *testing.T, maxBytes int64) *Cache {
	t.Helper()
	if maxBytes == 0 {
		maxBytes = 10 << 20
	}
	dir := t.TempDir()
	c, err := NewCache(dir, maxBytes)
	if err != nil {
		t.Fatalf("NewCache: %v", err)
	}
	return c
}

// hashOf computes the canonical xorb hash of `data`. Used in tests that
// need to Put data under its REAL hash (hash-verify-on-write happy path).
func hashOf(data []byte) string { return MerkleHashHex(data) }

// -----------------------------------------------------------------------------
// Basic round-trips

// TestCache_PutOpenRoundTrip — write a known payload under its canonical
// hash, Open returns the same bytes.
func TestCache_PutOpenRoundTrip(t *testing.T) {
	c := newTestCache(t, 0)
	payload := []byte("hello xet-core world — round-trip fixture")
	hash := hashOf(payload)

	finalPath, err := c.Put(hash, int64(len(payload)), bytes.NewReader(payload))
	if err != nil {
		t.Fatalf("Put: %v", err)
	}
	// Final path must match the documented layout.
	wantPath := filepath.Join(c.dir, "xorbs", hash[:2], hash+".bin")
	if finalPath != wantPath {
		t.Fatalf("final path:\nwant %s\ngot  %s", wantPath, finalPath)
	}

	f, size, err := c.Open(hash)
	if err != nil {
		t.Fatalf("Open: %v", err)
	}
	defer f.Close()
	if size != int64(len(payload)) {
		t.Fatalf("Open size: want %d got %d", len(payload), size)
	}
	got, err := io.ReadAll(f)
	if err != nil {
		t.Fatalf("ReadAll: %v", err)
	}
	if !bytes.Equal(got, payload) {
		t.Fatalf("bytes drift")
	}
}

// TestCache_OpenMissReturnsSentinel — Open on unknown hash returns
// ErrCacheMiss, not generic io error.
func TestCache_OpenMissReturnsSentinel(t *testing.T) {
	c := newTestCache(t, 0)
	_, _, err := c.Open(strings.Repeat("0", 64))
	if !errors.Is(err, ErrCacheMiss) {
		t.Fatalf("Open miss: want ErrCacheMiss, got %v", err)
	}
}

// -----------------------------------------------------------------------------
// : hash-verify-on-write

// TestCache_HashMismatchRefused — streaming bytes that don't match the
// declared hash MUST fail with ErrCacheHashMismatch, leave NO final file,
// and bump `gateway_cache_hash_mismatch_total`.
func TestCache_HashMismatchRefused(t *testing.T) {
	// Register metrics so the counter exists + is readable.
	_ = NewMetrics()

	c := newTestCache(t, 0)
	goodPayload := []byte("real payload bytes")
	badDeclaredHash := strings.Repeat("f", 64) // obviously not the real hash

	// Snapshot the counter before + after.
	before := readCounter(metricsCacheHashMismatch)
	_, err := c.Put(badDeclaredHash, int64(len(goodPayload)), bytes.NewReader(goodPayload))
	if !errors.Is(err, ErrCacheHashMismatch) {
		t.Fatalf("Put: want ErrCacheHashMismatch, got %v", err)
	}
	after := readCounter(metricsCacheHashMismatch)
	if after != before+1 {
		t.Fatalf("hash_mismatch counter: want +1, got %v -> %v", before, after)
	}

	// No final file.
	finalPath := filepath.Join(c.dir, "xorbs", badDeclaredHash[:2], badDeclaredHash+".bin")
	if _, err := os.Stat(finalPath); !os.IsNotExist(err) {
		t.Fatalf("final file must not exist after hash mismatch, got err=%v", err)
	}
	// No tmp file left behind.
	tmpPath := finalPath + ".tmp"
	if _, err := os.Stat(tmpPath); !os.IsNotExist(err) {
		t.Fatalf("tmp file must not exist after hash mismatch, got err=%v", err)
	}
	// No LRU entry either.
	if _, _, err := c.Open(badDeclaredHash); !errors.Is(err, ErrCacheMiss) {
		t.Fatalf("cache must have no entry after mismatch, got %v", err)
	}
}

// TestCache_SizeMismatchRefused — streaming bytes with wrong declared size
// MUST fail with ErrCacheSizeMismatch and leave no state.
func TestCache_SizeMismatchRefused(t *testing.T) {
	c := newTestCache(t, 0)
	payload := []byte("abcdef")
	hash := hashOf(payload)
	// Declare a larger size than the stream will produce.
	_, err := c.Put(hash, int64(len(payload)+10), bytes.NewReader(payload))
	if !errors.Is(err, ErrCacheSizeMismatch) {
		t.Fatalf("Put: want ErrCacheSizeMismatch, got %v", err)
	}
	if _, _, err := c.Open(hash); !errors.Is(err, ErrCacheMiss) {
		t.Fatalf("cache must have no entry after size mismatch, got %v", err)
	}
}

// -----------------------------------------------------------------------------
// : LRU eviction

// TestCache_LRUEviction — with 10 MiB cap + three 4 MiB xorbs, after the
// third Put the oldest is evicted and total bytes ≤ cap.
func TestCache_LRUEviction(t *testing.T) {
	const entrySize = 4 << 20
	const cap = 10 << 20
	c := newTestCache(t, cap)

	payloads := make([][]byte, 3)
	hashes := make([]string, 3)
	for i := 0; i < 3; i++ {
		p := bytes.Repeat([]byte{byte('a' + i)}, entrySize)
		payloads[i] = p
		hashes[i] = hashOf(p)
	}

	// Put 0, 1.
	for i := 0; i < 2; i++ {
		if _, err := c.Put(hashes[i], int64(entrySize), bytes.NewReader(payloads[i])); err != nil {
			t.Fatalf("Put %d: %v", i, err)
		}
	}
	s := c.Stats()
	if s.Entries != 2 || s.BytesUsed != 2*entrySize {
		t.Fatalf("after 2 puts: entries=%d bytes=%d", s.Entries, s.BytesUsed)
	}

	// Put 2 — triggers eviction of hashes[0] (the LRU tail).
	if _, err := c.Put(hashes[2], int64(entrySize), bytes.NewReader(payloads[2])); err != nil {
		t.Fatalf("Put 2: %v", err)
	}
	s = c.Stats()
	if s.BytesUsed > cap {
		t.Fatalf("bytesUsed=%d exceeds cap=%d", s.BytesUsed, cap)
	}
	if _, _, err := c.Open(hashes[0]); !errors.Is(err, ErrCacheMiss) {
		t.Fatalf("oldest must be evicted, got %v", err)
	}
	// Newer entries still present.
	if _, _, err := c.Open(hashes[1]); err != nil {
		t.Fatalf("hashes[1] should be cached, got %v", err)
	}
	if _, _, err := c.Open(hashes[2]); err != nil {
		t.Fatalf("hashes[2] should be cached, got %v", err)
	}
}

// TestCache_OpenMovesToMRU — Opening an entry promotes it to MRU so it
// survives the next eviction instead of an un-opened younger entry.
func TestCache_OpenMovesToMRU(t *testing.T) {
	const entrySize = 4 << 20
	const cap = 10 << 20
	c := newTestCache(t, cap)

	payloads := make([][]byte, 3)
	hashes := make([]string, 3)
	for i := 0; i < 3; i++ {
		p := bytes.Repeat([]byte{byte('a' + i)}, entrySize)
		payloads[i] = p
		hashes[i] = hashOf(p)
	}
	for i := 0; i < 2; i++ {
		_, err := c.Put(hashes[i], int64(entrySize), bytes.NewReader(payloads[i]))
		if err != nil {
			t.Fatalf("Put: %v", err)
		}
	}
	// Touch hash 0 → moves it to MRU.
	f, _, err := c.Open(hashes[0])
	if err != nil {
		t.Fatalf("Open 0: %v", err)
	}
	f.Close()

	// Put 2 — eviction picks the LRU tail, which is now hash 1 (not 0).
	if _, err := c.Put(hashes[2], int64(entrySize), bytes.NewReader(payloads[2])); err != nil {
		t.Fatalf("Put 2: %v", err)
	}
	if _, _, err := c.Open(hashes[1]); !errors.Is(err, ErrCacheMiss) {
		t.Fatalf("hashes[1] should be evicted after MRU-promotion of hash 0, got %v", err)
	}
	// Hash 0 survived.
	f, _, err = c.Open(hashes[0])
	if err != nil {
		t.Fatalf("hashes[0] must be cached, got %v", err)
	}
	f.Close()
}

// -----------------------------------------------------------------------------
// Atomic-write discipline

// TestCache_AtomicWrite_NoPartialFinalOnMismatch — on hash mismatch, there
// is NEVER a final `.bin` file on disk, only an absent one. This is the
// "reader never sees a partial final" invariant.
func TestCache_AtomicWrite_NoPartialFinalOnMismatch(t *testing.T) {
	_ = NewMetrics()
	c := newTestCache(t, 0)
	badHash := strings.Repeat("0", 64)
	_, err := c.Put(badHash, 5, bytes.NewReader([]byte("hello")))
	if !errors.Is(err, ErrCacheHashMismatch) {
		t.Fatalf("want mismatch, got %v", err)
	}
	// Walk the cache dir; there should be NO `.bin` file anywhere.
	count := 0
	filepath.Walk(c.dir, func(path string, info os.FileInfo, err error) error {
		if err == nil && info != nil && !info.IsDir() {
			if strings.HasSuffix(path, ".bin") {
				count++
				t.Errorf("unexpected .bin on disk after mismatch: %s", path)
			}
		}
		return nil
	})
	if count > 0 {
		t.Fatalf("%d partial final files left on disk", count)
	}
}

// TestCache_Put_OverwriteSameHash — Put of an existing hash replaces the
// entry cleanly: bytesUsed accounting stays correct, new bytes readable.
func TestCache_Put_OverwriteSameHash(t *testing.T) {
	c := newTestCache(t, 0)
	payload := []byte("deterministic stuff")
	hash := hashOf(payload)

	for i := 0; i < 3; i++ {
		_, err := c.Put(hash, int64(len(payload)), bytes.NewReader(payload))
		if err != nil {
			t.Fatalf("Put %d: %v", i, err)
		}
	}
	s := c.Stats()
	if s.Entries != 1 || s.BytesUsed != int64(len(payload)) {
		t.Fatalf("duplicate Put drifted state: entries=%d bytes=%d", s.Entries, s.BytesUsed)
	}
}

// -----------------------------------------------------------------------------
// Concurrent readers

// TestCache_ConcurrentReaders — one Put, then 50 concurrent Opens. Every
// reader gets the full bytes. No data races (run with -race).
func TestCache_ConcurrentReaders(t *testing.T) {
	c := newTestCache(t, 0)
	payload := bytes.Repeat([]byte("concurrent-read-test-"), 1024)
	hash := hashOf(payload)
	if _, err := c.Put(hash, int64(len(payload)), bytes.NewReader(payload)); err != nil {
		t.Fatalf("Put: %v", err)
	}

	var wg sync.WaitGroup
	errs := make(chan error, 50)
	for i := 0; i < 50; i++ {
		wg.Add(1)
		go func() {
			defer wg.Done()
			f, _, err := c.Open(hash)
			if err != nil {
				errs <- err
				return
			}
			defer f.Close()
			got, err := io.ReadAll(f)
			if err != nil {
				errs <- err
				return
			}
			if !bytes.Equal(got, payload) {
				errs <- errors.New("bytes drift in concurrent read")
			}
		}()
	}
	wg.Wait()
	close(errs)
	for e := range errs {
		t.Error(e)
	}
}

// -----------------------------------------------------------------------------
// latency: HIT < 50ms

// TestCache_HitLatencyUnder50ms — a warm cache Open+ReadAll of a 256 KiB
// xorb MUST complete in < 50ms, which is the ceiling.
// Rationale for this size: it's a realistic small-xorb payload. Larger
// payloads tax the test harness (disk bandwidth), not the cache logic
// the check is about the cache index being RAM-fast, not about pushing
// bytes. 256 KiB is chosen so a modern laptop SSD reads it in <5ms; 50ms
// is 10x headroom.
func TestCache_HitLatencyUnder50ms(t *testing.T) {
	c := newTestCache(t, 0)
	payload := bytes.Repeat([]byte("hit-latency-"), 256<<10/12)
	hash := hashOf(payload)
	if _, err := c.Put(hash, int64(len(payload)), bytes.NewReader(payload)); err != nil {
		t.Fatalf("Put: %v", err)
	}
	// Warm the fs cache by reading once; the "cold" fs read is not part of
	// what is measuring.
	f, _, err := c.Open(hash)
	if err != nil {
		t.Fatalf("Open warm: %v", err)
	}
	io.Copy(io.Discard, f)
	f.Close()

	start := time.Now()
	f2, _, err := c.Open(hash)
	if err != nil {
		t.Fatalf("Open hot: %v", err)
	}
	_, _ = io.Copy(io.Discard, f2)
	f2.Close()
	elapsed := time.Since(start)
	if elapsed > 50*time.Millisecond {
		t.Fatalf("GATE-06 HIT latency: %v > 50ms", elapsed)
	}
	t.Logf("HIT latency: %v (budget 50ms)", elapsed)
}

// -----------------------------------------------------------------------------
// FetchAndCache with a fake SiaDownloader

// fakeSia is declared in sia_test.go — we reuse the existing SiaDownloader
// test fake with its 03-05-added `downloadCalls` atomic counter + `sleep`
// knob. The `payload` + `err` fields plus the Write fast path match what
// FetchAndCache needs here.

// TestCache_FetchAndCache_HappyPath — one call writes the bytes into the
// cache under the correct hash, downstream Open returns them.
func TestCache_FetchAndCache_HappyPath(t *testing.T) {
	c := newTestCache(t, 0)
	payload := []byte("fetch-and-cache happy path bytes")
	hash := hashOf(payload)

	sia := &fakeSia{payload: payload}
	got, err := c.FetchAndCache(context.Background(), hash, types.Hash256{}, int64(len(payload)), sia)
	if err != nil {
		t.Fatalf("FetchAndCache: %v", err)
	}
	if got != hash {
		t.Fatalf("returned hash: want %s got %s", hash, got)
	}
	if calls := sia.downloadCalls.Load(); calls != 1 {
		t.Fatalf("want 1 Sia call, got %d", calls)
	}

	f, sz, err := c.Open(hash)
	if err != nil {
		t.Fatalf("Open: %v", err)
	}
	defer f.Close()
	if sz != int64(len(payload)) {
		t.Fatalf("sz: want %d got %d", len(payload), sz)
	}
}

// TestCache_FetchAndCache_HashMismatch — if Sia returns bytes whose hash
// doesn't match the declared hash, FetchAndCache fails and the cache stays
// empty.
func TestCache_FetchAndCache_HashMismatch(t *testing.T) {
	_ = NewMetrics()
	c := newTestCache(t, 0)
	badDeclaredHash := strings.Repeat("9", 64)
	payload := []byte("bytes that won't hash to 9999...")

	sia := &fakeSia{payload: payload}
	_, err := c.FetchAndCache(context.Background(), badDeclaredHash, types.Hash256{}, int64(len(payload)), sia)
	if err == nil {
		t.Fatalf("want error on hash mismatch, got nil")
	}
	if !errors.Is(err, ErrCacheHashMismatch) {
		t.Fatalf("want ErrCacheHashMismatch, got %v", err)
	}
	if _, _, err := c.Open(badDeclaredHash); !errors.Is(err, ErrCacheMiss) {
		t.Fatalf("cache must be empty after mismatch, got %v", err)
	}
}

// -----------------------------------------------------------------------------
// Prometheus counter read helper

// readCounter extracts the current float value of a prometheus.Counter for
// unit tests. Uses the lower-level Collector/Metric Write API because the
// CounterVec in NewMetrics is package-private and we need the raw float.
func readCounter(c prometheus.Counter) float64 {
	if c == nil {
		return 0
	}
	var m dto.Metric
	_ = c.Write(&m)
	if m.Counter == nil {
		return 0
	}
	return *m.Counter.Value
}
