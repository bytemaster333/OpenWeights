// Package main — cache.go.
// Whole-xorb disk LRU cache for the gateway ( / / /
// / PITFALL ). Semantics:
// - Key: xorb_hash_hex (64 lowercase hex chars). .
// - Value: on-disk file at `<root>/xorbs/<prefix>/<hash>.bin`, where
// `<prefix>` is the first 2 hex chars. Prefix-sharding keeps
// directory entry counts low on filesystems that degrade above
// ~10k dirents per directory (ext4 with dir_index, xfs, apfs).
// - Size: size-based eviction against a configurable byte budget
// (default 100 GiB via `CACHE_SIZE_BYTES` → `Config.CacheSizeBytes`).
// LRU by Open/Put timestamp; Put also moves to MRU.
// Write discipline ( / ):
// 1. Open/create `<final>.tmp`.
// 2. `io.TeeReader(src, MerkleHasher)` → `io.Copy` into tmp. Streaming;
// no whole-xorb RAM buffer.
// 3. `f.Sync` — force the data to disk BEFORE the rename. Prevents a
// torn-write scenario after `rename` where a crash leaves the entry
// indexed but with stale/zero content.
// 4. Compare `MerkleHasher.Hex` against the caller-supplied `hash`
// argument. If they disagree, delete tmp, increment
// `gateway_cache_hash_mismatch_total`, return error. Caller MUST NOT
// serve the bytes. This is the core defense against cache poisoning.
// 5. `os.Rename(tmp, final)` — atomic on POSIX same-filesystem. After
// this point, concurrent readers can Open the final file.
// Boot posture:
// - boots with a cold in-memory index. Even if files from a
// previous run remain on disk, they are NOT rehydrated into the LRU
// they would be served correctly IF the cache was queried by exact
// path, but the index is the sole truth source and they remain
// invisible until evicted by a `cleanup` helper (not implemented for
// v1). Rationale: a rehydration scan implies we trust on-disk bytes
// without a merkle re-verify; that contradicts .
// Post-v1 work: an "index rebuild + verify" pass on startup.
// Concurrency:
// - The in-memory index + LRU list are guarded by `mu`. Critical sections
// stay short — file I/O happens OUTSIDE the lock. Open grabs `mu`
// only to bump LRU position; the subsequent `os.Open` is lock-free.
// - Put does all I/O lock-free, then takes `mu` at the END for LRU
// bookkeeping + eviction.
// - The singleflight group in `singleflight.go` guarantees that concurrent
// cold-misses for the SAME hash collapse onto one Put, so the "two
// Puts race for one hash" path is effectively dead — but we still
// handle it correctly (last writer wins; previous file is unlinked
// only after rename).
package main

import (
	"container/list"
	"context"
	"errors"
	"fmt"
	"io"
	"os"
	"path/filepath"
	"sync"

	"go.sia.tech/core/types"
)

// ErrCacheMiss is the canonical sentinel Open returns when the hash has no
// entry in the in-memory index. Handler code relies on this with
// `errors.Is(err, ErrCacheMiss)`.
var ErrCacheMiss = errors.New("cache miss")

// ErrCacheHashMismatch is returned by Put when the streamed body's merkle
// hash disagrees with the supplied hash key. Handler code MUST respond 502
// and MUST NOT serve the partial bytes — per PITFALL .
var ErrCacheHashMismatch = errors.New("cache: streamed body hash != declared hash")

// ErrCacheSizeMismatch — the streamed body was shorter/longer than the size
// asserted by the caller (from Postgres `xorbs.size_bytes`). Handler 502.
var ErrCacheSizeMismatch = errors.New("cache: streamed body size != declared size")

// cacheEntry is the per-xorb LRU node. `path` is the absolute, canonical
// final path under `<root>/xorbs/<prefix>/<hash>.bin`.
type cacheEntry struct {
	hash string
	size int64
	path string
}

// Cache is the whole-xorb disk LRU. Construct exactly one per gateway
// process via NewCache; shared read/write between request goroutines.
type Cache struct {
	dir      string
	maxBytes int64

	mu        sync.Mutex
	bytesUsed int64
	lru       *list.List               // *cacheEntry; front = MRU, back = LRU
	index     map[string]*list.Element // hash → lru element
}

// NewCache prepares the on-disk root directory and returns an empty cache.
// - `dir` is the cache root. `<dir>/xorbs/` is created at boot.
// - `maxBytes` is the total-size cap. Put triggers eviction when
// bytesUsed > maxBytes. A value <= 0 is treated as "unbounded" (only
// useful in tests).
func NewCache(dir string, maxBytes int64) (*Cache, error) {
	if dir == "" {
		return nil, errors.New("cache: dir must be non-empty")
	}
	if err := os.MkdirAll(filepath.Join(dir, "xorbs"), 0o755); err != nil {
		return nil, fmt.Errorf("cache: mkdir %s/xorbs: %w", dir, err)
	}
	return &Cache{
		dir:      dir,
		maxBytes: maxBytes,
		lru:      list.New(),
		index:    make(map[string]*list.Element),
	}, nil
}

// pathFor returns the canonical final path for a hash. Must match the layout
// documented at the top of this file. Separated from Open/Put so unit tests
// can predict the location deterministically.
func (c *Cache) pathFor(hash string) string {
	return filepath.Join(c.dir, "xorbs", hash[:2], hash+".bin")
}

// Open returns a read-only *os.File over the cached xorb plus its byte
// size. Caller MUST Close the returned file (the closer is the caller's
// responsibility — keeps the fd lifetime aligned with the reader).
// Updates LRU position on hit. Returns ErrCacheMiss on miss.
func (c *Cache) Open(hash string) (*os.File, int64, error) {
	c.mu.Lock()
	elem, ok := c.index[hash]
	if !ok {
		c.mu.Unlock()
		return nil, 0, ErrCacheMiss
	}
	// Bump to MRU while we hold the lock. Path-capture is lock-free after.
	c.lru.MoveToFront(elem)
	entry := elem.Value.(*cacheEntry)
	path := entry.path
	size := entry.size
	c.mu.Unlock()

	f, err := os.Open(path)
	if err != nil {
		// File vanished — either evicted after we dropped the lock or the
		// filesystem lost it. Surface as a miss so the handler falls back
		// to a Sia refetch. Also drop the stale index entry so subsequent
		// callers don't repeat the dance.
		c.mu.Lock()
		if elem, ok := c.index[hash]; ok {
			entry := elem.Value.(*cacheEntry)
			if entry.path == path {
				c.bytesUsed -= entry.size
				if metricsCacheBytesOnDisk != nil {
					metricsCacheBytesOnDisk.Set(float64(c.bytesUsed))
				}
				c.lru.Remove(elem)
				delete(c.index, hash)
			}
		}
		c.mu.Unlock()
		return nil, 0, ErrCacheMiss
	}
	if metricsCacheHits != nil {
		metricsCacheHits.Inc()
	}
	return f, size, nil
}

// Put atomically inserts a xorb into the cache. Discipline: tmp → fsync →
// hash-verify → rename → LRU update → evict.
// `hash` is the 64-char hex key. `size` is the exact expected byte count
// (from Postgres). `src` is a streaming reader for the bytes — typically
// an io.Pipe consuming from SiaAdapter.Download. The function drains `src`
// to EOF before returning (caller-side pipe-writer close is part of the
// contract).
// Returns the final on-disk path on success. On hash mismatch:
// ErrCacheHashMismatch (no partial state, metric incremented). On size
// mismatch: ErrCacheSizeMismatch. On any other I/O error: wrapped error;
// partial tmp file is best-effort removed.
func (c *Cache) Put(hash string, size int64, src io.Reader) (string, error) {
	if len(hash) != 64 {
		return "", fmt.Errorf("cache: hash must be 64 hex chars, got %d", len(hash))
	}
	if size < 0 {
		return "", fmt.Errorf("cache: size must be >= 0, got %d", size)
	}

	final := c.pathFor(hash)
	prefixDir := filepath.Dir(final)
	if err := os.MkdirAll(prefixDir, 0o755); err != nil {
		return "", fmt.Errorf("cache: mkdir %s: %w", prefixDir, err)
	}
	tmp := final + ".tmp"

	// Clean up stale tmp from a previous crashed Put — pre-existing content
	// there would conflate our hash stream.
	_ = os.Remove(tmp)

	f, err := os.Create(tmp)
	if err != nil {
		return "", fmt.Errorf("cache: create tmp: %w", err)
	}
	// On any failure below, kill the tmp file.
	cleanupTmp := func() {
		_ = f.Close()
		_ = os.Remove(tmp)
	}

	// Streaming copy with a Tee'd hasher — streaming, zero whole-
	// xorb buffering in RAM. BLAKE3 over multi-GiB streams is disk-bound,
	// not CPU-bound, on modern hardware.
	hasher := NewMerkleHasher()
	tee := io.TeeReader(src, hasher)
	written, err := io.Copy(f, tee)
	if err != nil {
		cleanupTmp()
		return "", fmt.Errorf("cache: copy from src: %w", err)
	}
	if written != size {
		cleanupTmp()
		return "", fmt.Errorf("%w: got %d, want %d", ErrCacheSizeMismatch, written, size)
	}
	// fsync BEFORE rename. POSIX atomic-rename only guarantees the DIRENT
	// is atomic; it does NOT imply the data is flushed to stable storage.
	// Without this, a crash between rename and kernel flush could resurrect
	// the inode pointing at no data.
	if err := f.Sync(); err != nil {
		cleanupTmp()
		return "", fmt.Errorf("cache: fsync tmp: %w", err)
	}
	if err := f.Close(); err != nil {
		_ = os.Remove(tmp)
		return "", fmt.Errorf("cache: close tmp: %w", err)
	}

	// Hash-verify. LOAD-BEARING — this is the gate.
	got := hasher.Hex()
	if got != hash {
		_ = os.Remove(tmp)
		if metricsCacheHashMismatch != nil {
			metricsCacheHashMismatch.Inc()
		}
		return "", fmt.Errorf("%w: got %s, want %s", ErrCacheHashMismatch, got, hash)
	}

	// Atomic rename. On ext4/xfs/apfs this is an atomic dirent swap — no
	// reader ever observes a half-written `final`.
	if err := os.Rename(tmp, final); err != nil {
		_ = os.Remove(tmp)
		return "", fmt.Errorf("cache: rename tmp->final: %w", err)
	}

	// LRU bookkeeping + eviction under the lock.
	c.mu.Lock()
	defer c.mu.Unlock()

	if existing, ok := c.index[hash]; ok {
		// Rare race — another goroutine beat us. Deduct the old accounting
		// and unlink the list node; the on-disk file was the same path, so
		// there's nothing to delete (we just overwrote via rename).
		oldEntry := existing.Value.(*cacheEntry)
		c.bytesUsed -= oldEntry.size
		c.lru.Remove(existing)
	}
	entry := &cacheEntry{hash: hash, size: size, path: final}
	c.bytesUsed += size
	c.index[hash] = c.lru.PushFront(entry)
	if metricsCacheBytesOnDisk != nil {
		metricsCacheBytesOnDisk.Set(float64(c.bytesUsed))
	}
	c.evictLocked()
	return final, nil
}

// evictLocked pops LRU tails until `bytesUsed <= maxBytes`. Called with `mu`
// held. A maxBytes of <= 0 disables eviction (test-only).
func (c *Cache) evictLocked() {
	if c.maxBytes <= 0 {
		return
	}
	for c.bytesUsed > c.maxBytes && c.lru.Len() > 0 {
		back := c.lru.Back()
		entry := back.Value.(*cacheEntry)
		c.lru.Remove(back)
		delete(c.index, entry.hash)
		c.bytesUsed -= entry.size
		// Best-effort unlink. If a reader still has the fd open, POSIX
		// semantics keep the inode alive until the last close; that's
		// correct.
		_ = os.Remove(entry.path)
		if metricsCacheEvicted != nil {
			metricsCacheEvicted.Inc()
		}
	}
	if metricsCacheBytesOnDisk != nil {
		metricsCacheBytesOnDisk.Set(float64(c.bytesUsed))
	}
}

// Stats returns a point-in-time snapshot for tests + debug introspection.
// Not a hot-path helper.
type Stats struct {
	Entries   int
	BytesUsed int64
	MaxBytes  int64
}

// Stats returns a snapshot of cache state.
func (c *Cache) Stats() Stats {
	c.mu.Lock()
	defer c.mu.Unlock()
	return Stats{
		Entries:   c.lru.Len(),
		BytesUsed: c.bytesUsed,
		MaxBytes:  c.maxBytes,
	}
}

// FetchAndCache pulls a whole xorb from Sia and writes it into the cache
// atomically + hash-verified. Returns the hash on success.
// Wiring: called via `MissCoalescer.Do(hash, ...)` so concurrent cold-miss
// requests for the same xorb collapse onto one underlying Sia download
// ( / ). The singleflight key is the xorb hash, NOT the HTTP
// Range — we always fetch the whole object.
// The io.Pipe pattern decouples the SDK writer from the cache consumer in
// one streaming pass: the Sia adapter writes into `pw`, the cache's TeeReader
// consumes from `pr`, and bytes never materialise fully in RAM.
func (c *Cache) FetchAndCache(ctx context.Context, hash string, siaID types.Hash256, size int64, sia SiaDownloader) (string, error) {
	pr, pw := io.Pipe()
	errCh := make(chan error, 1)

	// Goroutine: writer side. Call DownloadRange(offset=0, length=0) which
	// the SiaAdapter treats as "whole object". On completion or error,
	// close the pipe with the terminal err so the reader sees EOF or the
	// wrapped error.
	go func() {
		defer close(errCh)
		err := sia.DownloadRange(ctx, pw, siaID, 0, 0)
		// CloseWithError(nil) is equivalent to Close (clean EOF); any non-nil
		// err is surfaced to the reader as `err` instead of EOF.
		_ = pw.CloseWithError(err)
		errCh <- err
	}()

	// Reader side: drain the pipe into Put, which hash-verifies + renames.
	_, putErr := c.Put(hash, size, pr)

	// Wait for the writer to finish so we don't leak its goroutine past
	// return. The err from Sia is the primary failure signal; the Put
	// error chains as fallback.
	dlErr := <-errCh

	if dlErr != nil {
		// If the Put failed BECAUSE the download short-circuited (e.g.
		// ctx cancellation mid-stream → pipe closed with err → Put sees
		// a read error → surfaces as "copy from src"), we want the root-
		// cause download err in the returned message, not the Put echo.
		return "", fmt.Errorf("sia download: %w", dlErr)
	}
	if putErr != nil {
		return "", putErr
	}
	return hash, nil
}
