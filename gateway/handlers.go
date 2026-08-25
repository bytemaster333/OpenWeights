// Package main — handlers.go.
// After Wave 3a (this plan, 03-03):
// - `GET /health` — unchanged from Wave 1, always 200 JSON.
// - `GET /xorb/{hash}` REAL — signed-URL verify → Postgres xorb lookup →
// Range-header parse → full-object `200` OR single-range `206` streaming
// from a Sia-fetched temp file. Multi-range returns `501 Not Implemented`
// here; replaces that branch with `multipart/byteranges`.
// The xorb handler's job sheet (numbered to match the function body):
// 1. Verify signed URL. Expired + bad_signature BOTH map to HTTP 403
// ( — xet-core's refresh path is keyed exclusively to 403;
// returning 401/410 would strand retries).
// 2. Resolve `{hash}` against Postgres `xorbs` filtered on `pin_state =
// 'pinned'`. Miss → 404. Any other error → 500.
// 3. Parse `Range:` header if present. Malformed → 400. Unsatisfiable → 416
// with `Content-Range: bytes */<size>` (RFC 7233 §4.4).
// 4. If the signed URL carried an `r=`, enforce that the requested
// client range sits entirely inside it. Violation → 403 (the signer
// refused the bytes; this is NOT a malformed request).
// 5. Fetch the whole xorb from Sia into a temp file. This IS the cold-miss
// path; replaces `getXorbSource` with a cache-aware variant
// that reads from the disk LRU on HIT and consults `singleflight` on
// MISS. Until then every request is effectively MISS.
// 6. Write the response:
// - no Range → 200 full-object
// - one range → 206 + Content-Range via writeSingleRange
// - many ranges → 501 ( owns)
// Streaming via `io.Copy(w, io.NewSectionReader(...))` — whole-object
// buffering in RAM would violate.
// 7. Client-disconnect: everywhere we touch I/O we use `r.Context`.
// Passing `ctx` to `SiaAdapter.DownloadRange` means a TCP RST from the
// client cancels the Sia fetch within 1s.
// Concurrency notes:
// - `Handlers` is constructed once in main.go and shared read-only across
// goroutines; none of its fields carry per-request state.
// - Each request allocates its own temp file via `os.CreateTemp`; the
// deferred `xorbSource.Close` deletes it. A crash between CreateTemp and
// the defer registration would leak a file — acceptable (OS temp-dir
// janitor cleans), not worth wrapping in recover. replaces this
// entirely.
package main

import (
	"context"
	"encoding/json"
	"errors"
	"fmt"
	"io"
	"log/slog"
	"net/http"
	"os"
	"strconv"
	"strings"
	"time"

	"github.com/go-chi/chi/v5"
	"github.com/google/uuid"
	"go.sia.tech/core/types"
)

// XorbLookup is the narrow slice of the DB surface the xorb handler consumes.
// Wrapping it in an interface lets handler tests substitute a fake without
// standing up Postgres. `*DB` from db.go satisfies this by direct method.
type XorbLookup interface {
	LookupXorb(ctx context.Context, hashHex string) (types.Hash256, int64, error)
}

// Handlers holds the dependencies the routes close over. Wave 3a expands the
// Wave 1 struct to carry the DB + Sia adapter + meter.
// NOTE ON NIL-SAFETY: `Sia`/`DB`/`Meter` may legitimately be nil in narrow
// unit-test paths (e.g. the `/health` endpoint tests). The xorb handler
// guards every pointer deref before use — a nil field triggers an HTTP 500
// with a cleanly-typed error, NOT a panic. Production main.go wires all
// fields non-nil.
type Handlers struct {
	Metrics  *Metrics
	Verifier *UrlVerifier
	DB       XorbLookup
	Sia      SiaDownloader
	Meter    UsageWriter
	// Cache is the whole-xorb disk LRU wired in by. Nil-safe for
	// unit tests that only exercise verifier / DB branches; nil triggers the
	// old SDK-direct temp-file path (Wave 3a behavior).
	Cache *Cache
	// Miss coalesces concurrent cold-miss fetches for the same hash ( /
	// ). Nil-safe — nil falls back to un-coalesced per-request Sia
	// fetches. Production main.go wires both Cache and Miss non-nil.
	Miss *MissCoalescer
	// TimeNow is injectable so tests can pin `now` against fixture exp values.
	// Production leaves this nil and the handler uses `time.Now`.
	TimeNow func() time.Time
}

// now returns the current time, respecting the Handlers.TimeNow override.
func (h *Handlers) now() time.Time {
	if h != nil && h.TimeNow != nil {
		return h.TimeNow()
	}
	return time.Now()
}

// HealthHandler — tiny JSON health probe. Unchanged from Wave 1.
func (h *Handlers) HealthHandler(w http.ResponseWriter, r *http.Request) {
	w.Header().Set("Content-Type", "application/json; charset=utf-8")
	w.WriteHeader(http.StatusOK)
	_ = json.NewEncoder(w).Encode(map[string]string{"status": "ok"})
	if h.Metrics != nil {
		h.Metrics.RequestsTot.WithLabelValues(r.Method, "/health", "200").Inc()
	}
}

// ServeXorb is the real `GET /xorb/{hash}` handler (replaces Wave 1's stub).
// See file header for the numbered job sheet.
// / : emits exactly one structured JSON log line per request
// at handler exit with the mandated field set
// `{request_id, hash, range, status, bytes_served, cache_hit,
// sia_latency_ms, method, remote_ip}`. The accumulator is a local
// `reqLog` struct updated as the handler progresses; a deferred
// `h.logRequest(&rl)` call emits the final line regardless of which exit
// branch fires.
func (h *Handlers) ServeXorb(w http.ResponseWriter, r *http.Request) {
	ctx := r.Context()
	hash := chi.URLParam(r, "hash")

	rl := &reqLog{
		RequestID: RequestIDFrom(ctx),
		Method:    r.Method,
		Hash:      hash,
		Range:     r.Header.Get("Range"),
		RemoteIP:  clientIP(r),
	}
	defer h.logRequest(ctx, rl)

	// 1. Verify signed URL.
	vok, verr := h.Verifier.Verify(hash, r.URL.Query(), h.now())
	if verr != nil {
		rl.Status = h.respondVerifyErr(w, r, verr)
		return
	}

	// 2. Resolve xorb → (sia_object_id, size) via Postgres.
	if h.DB == nil {
		rl.Status = h.respondError(w, r, http.StatusInternalServerError, "internal", "DB not wired")
		return
	}
	siaID, size, err := h.DB.LookupXorb(ctx, hash)
	if err != nil {
		if errors.Is(err, ErrXorbNotFound) {
			rl.Status = h.respondError(w, r, http.StatusNotFound, "not found", "")
			return
		}
		// Malformed hashes surface as DB-decoder errors (db.go decodes hex
		// before hitting the pool). The verifier already enforced 64 lower
		// hex in step 1, so reaching here means a hash that passed the
		// verifier but fails the DB decoder — programmer bug. 500 is
		// appropriate; we do not leak the decoder message to the client.
		rl.Status = h.respondError(w, r, http.StatusInternalServerError, "internal", err.Error())
		return
	}

	// 3. Parse Range header (if present).
	var ranges []Range
	if rangeHdr := r.Header.Get("Range"); rangeHdr != "" {
		rs, rerr := ParseRange(rangeHdr, size)
		switch {
		case errors.Is(rerr, ErrUnsatisfiable):
			// RFC 7233 §4.4: 416 response SHOULD carry Content-Range: bytes */<size>.
			w.Header().Set("Content-Range", fmt.Sprintf("bytes */%d", size))
			rl.Status = h.respondError(w, r, http.StatusRequestedRangeNotSatisfiable, "range not satisfiable", "")
			return
		case rerr != nil:
			rl.Status = h.respondError(w, r, http.StatusBadRequest, "bad range", rerr.Error())
			return
		}
		ranges = rs
	}

	// 4. Enforce signed-URL-bound ranges if the URL granted partial regions.
	if len(vok.Ranges) > 0 {
		grants := make([]Range, len(vok.Ranges))
		for i, g := range vok.Ranges {
			grants[i] = Range{Start: g[0], End: g[1]}
		}
		// A client-supplied Range *must* sit inside one of the signed segments.
		// If the client supplied NO Range but the URL is bounded, the "full
		// object" they are asking for is actually the granted segments — serve
		// exactly those (a multi-segment grant then yields a multipart body
		// below) so a bounded URL never spills the whole xorb.
		if len(ranges) == 0 {
			ranges = grants
		} else if !allRangesWithinAny(ranges, grants) {
			rl.Status = h.respondError(w, r, http.StatusForbidden, "forbidden", "range outside signed grant")
			return
		}
	}

	// 5. Get a bytes source. Wave 3a: synchronous SDK fetch into a temp file.
	// Wave 3b replaces this with a cache-aware variant.
	if h.Sia == nil {
		rl.Status = h.respondError(w, r, http.StatusInternalServerError, "internal", "Sia adapter not wired")
		return
	}
	src, err := h.getXorbSource(ctx, hash, siaID, size)
	if err != nil {
		// If the client canceled mid-download, don't emit 502 — the TCP is
		// dead anyway and surfacing 502 in logs falsely paints Sia as the
		// culprit.
		if errors.Is(ctx.Err(), context.Canceled) || errors.Is(err, context.Canceled) {
			// Nothing to write; client is gone. chi middleware logs the
			// request close. We still emit a metric label so ops can see
			// the disconnect count; use 499 per nginx convention (clients
			// won't see this status — it's for logs only).
			h.incRequestsTot(r.Method, "/xorb/{hash}", "499")
			rl.Status = 499
			return
		}
		slog.ErrorContext(ctx, "sia fetch failed", "hash", hash, "err", err.Error())
		rl.Status = h.respondError(w, r, http.StatusBadGateway, "bad gateway", err.Error())
		return
	}
	defer src.Close()

	// 6. Write the response. X-Cache + X-Sia-Fetch-Ms per — this
	// handler always emits MISS until lights up the cache HIT path.
	cacheLabel := "MISS"
	if src.CacheHit {
		cacheLabel = "HIT"
	}
	w.Header().Set("X-Cache", cacheLabel)
	w.Header().Set("X-Sia-Fetch-Ms", strconv.FormatInt(src.SiaFetchMs, 10))
	rl.CacheHit = src.CacheHit
	rl.SiaLatencyMs = src.SiaFetchMs

	var (
		bytesServed int64
		status      int
	)
	switch {
	case len(ranges) == 0:
		// Full-object 200.
		w.Header().Set("Content-Type", "application/octet-stream")
		w.Header().Set("Content-Length", strconv.FormatInt(size, 10))
		w.Header().Set("Accept-Ranges", "bytes")
		w.WriteHeader(http.StatusOK)
		n, _ := io.Copy(w, io.NewSectionReader(src.ReaderAt, 0, size))
		bytesServed = n
		status = http.StatusOK

	case len(ranges) == 1:
		if err := writeSingleRange(w, ranges[0], size, src.ReaderAt); err == nil {
			bytesServed = int64(ranges[0].End-ranges[0].Start) + 1
		}
		// if Copy fails mid-flight we cannot reset the status — headers
		// and WriteHeader(206) have already been sent. Log from handles
		// the structured line.
		status = http.StatusPartialContent

	default:
		// Multi-range: RFC 7233 §4.1 multipart/byteranges framing.
		// is GUARDED here: we call `writeMultipartRanges` (which
		// uses Go's stdlib `mime/multipart.Writer` — no hand-rolled CRLF) and
		// NEVER write a concatenated body. A concatenated response would
		// silently corrupt xet-core's V2 reconstruction downloads.
		// `TestMultipartRanges_NotConcatenated` in ranges_test.go is the
		// anti-regression tripwire.
		if err := writeMultipartRanges(w, ranges, size, src.ReaderAt); err == nil {
			for _, r := range ranges {
				bytesServed += int64(r.End-r.Start) + 1
			}
		}
		// Same constraint as single-range: if Copy fails mid-flight we cannot
		// reset the status. Headers + 206 have been flushed. structured
		// log line owns the clean-vs-faulty distinction.
		status = http.StatusPartialContent
	}

	h.incRequestsTot(r.Method, "/xorb/{hash}", strconv.Itoa(status))
	rl.Status = status
	rl.BytesServed = bytesServed

	// 7. Meter. Fire-and-forget with a background ctx so a late client
	// disconnect doesn't abort the INSERT mid-transaction. The handler
	// has already served the bytes — the usage_log row is operator
	// bookkeeping, not a client-visible contract.
	if h.Meter != nil {
		// kid may be uuid.Nil for signed URLs minted with no api_key_id claim;
		// LogDownload stores NULL in that case (not our concern here).
		meterCtx, cancel := context.WithTimeout(context.Background(), 5*time.Second)
		go func(kid uuid.UUID, n int64, hit bool, h2 string) {
			defer cancel()
			_ = h.Meter.LogDownload(meterCtx, kid, n, hit, h2)
		}(vok.Kid, bytesServed, src.CacheHit, hash)
	}
}

// reqLog is the in-handler accumulator for the one-log-line-per-request
// JSON emission mandated by / § SC-6. Fields match
// the spec verbatim; handler updates `Status`, `BytesServed`, `CacheHit`,
// `SiaLatencyMs` as each step resolves. A deferred `logRequest` emits the
// final line at handler exit.
type reqLog struct {
	RequestID    string
	Method       string
	Hash         string
	Range        string
	Status       int
	BytesServed  int64
	CacheHit     bool
	SiaLatencyMs int64
	RemoteIP     string
}

// logRequest emits one structured JSON log line per /xorb request. Level
// is Info for 2xx, Warn for 4xx, Error for 5xx (499 is a Warn
// client-side disconnect, not our fault).
// Fields match / § SC-6 verbatim.
func (h *Handlers) logRequest(ctx context.Context, rl *reqLog) {
	if rl == nil {
		return
	}
	level := slog.LevelInfo
	switch {
	case rl.Status >= 500:
		level = slog.LevelError
	case rl.Status >= 400 || rl.Status == 499:
		level = slog.LevelWarn
	}
	slog.LogAttrs(ctx, level, "xorb_request",
		slog.String("request_id", rl.RequestID),
		slog.String("method", rl.Method),
		slog.String("hash", rl.Hash),
		slog.String("range", rl.Range),
		slog.Int("status", rl.Status),
		slog.Int64("bytes_served", rl.BytesServed),
		slog.Bool("cache_hit", rl.CacheHit),
		slog.Int64("sia_latency_ms", rl.SiaLatencyMs),
		slog.String("remote_ip", rl.RemoteIP),
	)
}

// clientIP returns the client IP from the request. Honors `X-Forwarded-For`
// (Caddy sets this on every proxied request) and falls back to
// `RemoteAddr`. Only the FIRST IP in X-Forwarded-For is trusted; any
// downstream appenders are ignored (the Caddy reverse proxy in
// strips client-supplied X-Forwarded-For).
func clientIP(r *http.Request) string {
	if xff := r.Header.Get("X-Forwarded-For"); xff != "" {
		// `X-Forwarded-For: client, proxy1, proxy2` — first token is client.
		if i := strings.IndexByte(xff, ','); i > 0 {
			return strings.TrimSpace(xff[:i])
		}
		return strings.TrimSpace(xff)
	}
	return r.RemoteAddr
}

// respondVerifyErr maps the verifier's typed error kind onto the HTTP status
// code the handler contract requires. is the single load-bearing
// rule here: `expired` AND `bad_signature` MUST both return 403 so xet-core's
// URL-refresh retry path triggers. A 401/410 would strand the client.
// Returns the emitted HTTP status so the caller can record it on the request
// log accumulator.
func (h *Handlers) respondVerifyErr(w http.ResponseWriter, r *http.Request, verr *VerifyErr) int {
	var status int
	var body string
	switch {
	case verr.Kind == "expired":
		status, body = http.StatusForbidden, "forbidden"
	case verr.Kind == "bad_signature":
		status, body = http.StatusForbidden, "forbidden"
	case strings.HasPrefix(verr.Kind, "malformed"):
		status, body = http.StatusBadRequest, "bad request"
	default:
		// Defensive: any future `VerifyErr.Kind` the verifier grows must be
		// classified explicitly. Until then, fall through to 400 (safer than
		// leaking as 500 — the client handed us a URL we couldn't parse).
		status, body = http.StatusBadRequest, "bad request"
	}
	return h.respondError(w, r, status, body, verr.Kind)
}

// respondError centralizes the HTTP error path so metrics labels stay
// symmetric. `detail` is NEVER written into the response body (it may carry
// DB error messages that must not leak to clients) — the structured
// log line consumes it via the `reqLog.Status` field at request exit.
// Returns the emitted status so callers can record it on their log
// accumulator in one expression.
func (h *Handlers) respondError(w http.ResponseWriter, r *http.Request, status int, publicMsg, detail string) int {
	_ = detail // structured-log hook — body never leaks decoder/SDK messages
	w.Header().Set("Content-Type", "text/plain; charset=utf-8")
	w.Header().Set("X-Content-Type-Options", "nosniff")
	w.WriteHeader(status)
	_, _ = w.Write([]byte(publicMsg + "\n"))
	h.incRequestsTot(r.Method, "/xorb/{hash}", strconv.Itoa(status))
	return status
}

// incRequestsTot is nil-safe around the metrics struct so unit tests that
// skip wiring metrics do not blow up on the counter dereference.
func (h *Handlers) incRequestsTot(method, route, status string) {
	if h == nil || h.Metrics == nil {
		return
	}
	h.Metrics.RequestsTot.WithLabelValues(method, route, status).Inc()
}

// xorbSource is the seam shared with. Fields:
// - ReaderAt: a seekable byte source (temp file in Wave 3a; cache file
// in Wave 3b). MUST satisfy `io.ReaderAt` — `io.NewSectionReader`
// consumes that interface to stream ranges without buffering.
// - Close: idempotent cleanup (closes FD, removes temp file, releases
// the cache lease — whichever the implementation wires).
// - CacheHit: true if the bytes were served from the disk LRU. Wave 3a
// always sets false.
// - SiaFetchMs: milliseconds spent in `SiaAdapter.DownloadRange`. Zero on
// cache HIT.
type xorbSource struct {
	ReaderAt   io.ReaderAt
	CacheHit   bool
	SiaFetchMs int64
	closer     func()
}

// Close releases the source. Safe to call more than once — the underlying
// closer captures and nil-checks.
func (s *xorbSource) Close() {
	if s == nil || s.closer == nil {
		return
	}
	s.closer()
	s.closer = nil
}

// getXorbSource returns a streaming byte-source for the requested xorb.
// Wave 3b variant — cache-first with singleflight-coalesced
// cold-miss path:
// 1. FAST PATH — cache HIT. `Cache.Open(hash)` returns an *os.File; wrap
// as an xorbSource with CacheHit=true and SiaFetchMs=0. Dominates
// request latency for popular xorbs (: <50ms p50 on warm cache).
// 2. COLD PATH — cache MISS. `MissCoalescer.Do(hash, fetchFn)` ensures
// only one Sia fetch runs per xorb hash per concurrent cohort ( /
// ). fetchFn re-checks the cache (a concurrent leader may have
// landed between our Open above and entering the coalescer) before
// issuing `Cache.FetchAndCache` which streams Sia → Put atomically
// with hash-verify-on-write ( / ).
// 3. After the coalescer resolves, we Open the now-populated cache file
// and return it. CacheHit=false; SiaFetchMs records the leader's
// fetch duration (all followers see this same value, which is fine
// it's the TRUE cost of the MISS cohort, not a per-request metric).
// Nil-safety: if h.Cache or h.Miss is nil (unit-test mode), falls through
// to the Wave-3a SDK-direct path for backwards compatibility. Production
// main.go wires both non-nil.
// Ctx propagation: h.Sia.DownloadRange(ctx,...) honors ctx cancellation
// via the inner SDK. A client TCP RST propagates within 1s. If
// the LEADER's ctx is canceled while followers wait, all followers receive
// the same canceled error — the handler then falls back to the legacy
// SDK-direct path via the nil-cache branch, OR surfaces 502 if even the
// direct path fails.
func (h *Handlers) getXorbSource(ctx context.Context, hash string, siaID types.Hash256, size int64) (*xorbSource, error) {
	// Cache + coalescer nil-safety path — used by Wave 3a tests that wire
	// only {Metrics, Verifier, DB, Sia, Meter}. Kept behaviour-equivalent
	// to the original SDK-direct implementation so no existing test churns.
	if h.Cache == nil || h.Miss == nil {
		return h.getXorbSourceDirect(ctx, hash, siaID, size)
	}

	// FAST PATH — cache HIT.
	if f, sz, err := h.Cache.Open(hash); err == nil {
		_ = sz // size is known to the caller via the DB lookup; not returned.
		return &xorbSource{
			ReaderAt:   f,
			CacheHit:   true,
			SiaFetchMs: 0,
			closer: func() {
				_ = f.Close()
			},
		}, nil
	} else if !errors.Is(err, ErrCacheMiss) {
		// Transient I/O error inside Open (e.g. stat race). Surface as
		// 502 via the caller's error handling; don't fall through silently.
		return nil, fmt.Errorf("cache open: %w", err)
	}

	// COLD PATH — cache MISS → coalesced fetch.
	// Miss bookkeeping counter bumped here (once per distinct MISS request,
	// NOT per distinct Sia fetch — that distinction is why the coalesced
	// counter exists).
	if metricsCacheMisses != nil {
		metricsCacheMisses.Inc()
	}
	start := time.Now()
	_, err := h.Miss.Do(ctx, hash, func() (string, error) {
		// Leader-only re-check: a concurrent leader may have populated the
		// cache while we were racing into Do. If so, skip the fetch
		// an unnecessary Sia call burns bandwidth and contract funds.
		if _, _, err := h.Cache.Open(hash); err == nil {
			return hash, nil
		}
		return h.Cache.FetchAndCache(ctx, hash, siaID, size, h.Sia)
	})
	siaMs := time.Since(start).Milliseconds()
	if err != nil {
		return nil, err
	}

	// Re-open the freshly cached file. The leader just renamed it into
	// place; Open is lock-free on the fast path.
	f, _, err := h.Cache.Open(hash)
	if err != nil {
		// This is surprising — the coalescer reported success but the
		// cache has no entry. Possible if an aggressive evictor took the
		// file out between FetchAndCache's rename and our Open. Fail
		// hard; the handler surfaces 502 and the client retries.
		return nil, fmt.Errorf("cache open after fetch: %w", err)
	}
	return &xorbSource{
		ReaderAt:   f,
		CacheHit:   false,
		SiaFetchMs: siaMs,
		closer: func() {
			_ = f.Close()
		},
	}, nil
}

// getXorbSourceDirect is the Wave-3a SDK-direct temp-file implementation,
// preserved as a fallback for unit tests that construct Handlers without a
// Cache or Miss. Production main.go wires those fields; this path is NOT
// exercised in production.
// Kept verbatim from pre- for test-suite compatibility.
func (h *Handlers) getXorbSourceDirect(ctx context.Context, hash string, siaID types.Hash256, size int64) (*xorbSource, error) {
	prefix := "xorb_"
	if len(hash) >= 8 {
		prefix += hash[:8] + "_"
	}
	f, err := os.CreateTemp("", prefix+"*")
	if err != nil {
		return nil, fmt.Errorf("create temp file: %w", err)
	}

	start := time.Now()
	if err := h.Sia.DownloadRange(ctx, f, siaID, 0, 0); err != nil {
		_ = f.Close()
		_ = os.Remove(f.Name())
		return nil, fmt.Errorf("sia download: %w", err)
	}
	siaMs := time.Since(start).Milliseconds()

	info, err := f.Stat()
	if err != nil {
		_ = f.Close()
		_ = os.Remove(f.Name())
		return nil, fmt.Errorf("stat temp file: %w", err)
	}
	if info.Size() != size {
		_ = f.Close()
		_ = os.Remove(f.Name())
		return nil, fmt.Errorf("sia download size mismatch: got %d want %d", info.Size(), size)
	}

	name := f.Name()
	return &xorbSource{
		ReaderAt:   f,
		CacheHit:   false,
		SiaFetchMs: siaMs,
		closer: func() {
			_ = f.Close()
			_ = os.Remove(name)
		},
	}, nil
}

// ServeXorbStub is preserved as a legacy alias so any test in the tree that
// still wires the Wave 1 route name keeps compiling. Production main.go
// will switch to ServeXorb in the integration commit at end of Wave 3.
// Deprecated: use ServeXorb; this stub returns 501 and ignores dependencies.
func (h *Handlers) ServeXorbStub(w http.ResponseWriter, r *http.Request) {
	_ = chi.URLParam(r, "hash")
	w.Header().Set("Content-Type", "text/plain; charset=utf-8")
	w.WriteHeader(http.StatusNotImplemented)
	_, _ = w.Write([]byte("xorb handler lands in plan 03-03 / 03-04 / 03-05\n"))
	h.incRequestsTot(r.Method, "/xorb/{hash}", "501")
}
