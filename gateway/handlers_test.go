// handlers_test.go — `GET /xorb/{hash}` integration + unit tests (Wave 3a).
//
// Setup pattern: every test builds a self-contained Handlers bundle with:
//   - A real `UrlVerifier` seeded with a freshly-generated key.
//   - A `fakeDB` satisfying XorbLookup — returns a canned (siaID, size) tuple
//     or a typed error.
//   - A `fakeSia` (from sia_test.go) serving a deterministic payload.
//   - An optional `fakeMeter` from metering_test.go to assert the handler
//     fired the `LogDownload` call with the correct (kid, bytes, hit) args.
//   - `h.TimeNow` pinned so the signed URL's `exp` lives in a controlled
//     window relative to test `now`.
//
// We deliberately DO NOT stand up Postgres or Sia. Every assertion is
// closed over in-process state, keeping CI deterministic and sub-second.
package main

import (
	"bytes"
	"context"
	"crypto/hmac"
	"crypto/sha256"
	"encoding/base64"
	"encoding/binary"
	"errors"
	"fmt"
	"io"
	"mime"
	"mime/multipart"
	"net/http"
	"net/http/httptest"
	"net/url"
	"strings"
	"testing"
	"time"

	"github.com/go-chi/chi/v5"
	"github.com/google/uuid"
	"go.sia.tech/core/types"
)

// fakeDB implements XorbLookup. Configure either `resp` for a success reply
// or `err` for a failure reply; mutual exclusion not enforced (err wins).
type fakeDB struct {
	siaID types.Hash256
	size  int64
	err   error
	// calls records every hash asked; used to assert the verifier fired
	// BEFORE the DB lookup (we never want to hit the DB with an unverified
	// URL — gotcha #8 timing).
	calls []string
}

func (f *fakeDB) LookupXorb(_ context.Context, hashHex string) (types.Hash256, int64, error) {
	f.calls = append(f.calls, hashHex)
	if f.err != nil {
		return types.Hash256{}, 0, f.err
	}
	return f.siaID, f.size, nil
}

// makeSignedURL builds a minted URL for a test fixture: canonical string per
// signed_url.go + HMAC-SHA256 with the provided key + base64url-nopad sig.
//
// Mirrors the Rust minter exactly. Tests must stay byte-identical with the
// CAS side, so we rebuild the logic inline here rather than pull in a helper
// — keeping the test file self-contained makes drift louder.
type urlSpec struct {
	hashHex string
	exp     uint64
	rng     *[2]uint64
	kid     uuid.UUID
	key     []byte
}

func (s urlSpec) sign() url.Values {
	canonical := CanonicalString(CanonicalVersion, s.hashHex, s.exp, s.rng, s.kid)
	mac := hmac.New(sha256.New, s.key)
	mac.Write([]byte(canonical))
	sig := base64.RawURLEncoding.EncodeToString(mac.Sum(nil))

	q := url.Values{}
	q.Set("exp", fmt.Sprintf("%d", s.exp))
	q.Set("kid", s.kid.String())
	q.Set("sig", sig)
	if s.rng != nil {
		q.Set("r", fmt.Sprintf("%d-%d", s.rng[0], s.rng[1]))
	}
	return q
}

// newHandlersForTest builds a router-mounted handler with deterministic deps.
// Returns the Handlers (for direct-call tests), the signing key in base64,
// and a helper to issue a chi-routed GET to `/xorb/{hash}` with a fresh
// request context.
func newHandlersForTest(t *testing.T, db XorbLookup, sia SiaDownloader, meter UsageWriter, now time.Time) (*Handlers, []byte, func(q url.Values, hash string, hdrs http.Header, ctx context.Context) *httptest.ResponseRecorder) {
	t.Helper()
	// 32-byte key of well-known bytes (constant per test).
	key := make([]byte, SigningKeyLen)
	for i := range key {
		key[i] = byte(0x30 + i)
	}
	keyB64 := base64.StdEncoding.EncodeToString(key)
	v, err := NewUrlVerifier(keyB64, "", 7200)
	if err != nil {
		t.Fatalf("verifier: %v", err)
	}

	h := &Handlers{
		Metrics:  NewMetrics(),
		Verifier: v,
		DB:       db,
		Sia:      sia,
		Meter:    meter,
		TimeNow:  func() time.Time { return now },
	}

	r := chi.NewRouter()
	r.Use(RequestID)
	r.Get("/xorb/{hash}", h.ServeXorb)

	serve := func(q url.Values, hash string, hdrs http.Header, ctx context.Context) *httptest.ResponseRecorder {
		u := "/xorb/" + hash
		if len(q) > 0 {
			u += "?" + q.Encode()
		}
		req := httptest.NewRequest(http.MethodGet, u, nil)
		if ctx != nil {
			req = req.WithContext(ctx)
		}
		for k, vs := range hdrs {
			for _, vv := range vs {
				req.Header.Add(k, vv)
			}
		}
		rec := httptest.NewRecorder()
		r.ServeHTTP(rec, req)
		return rec
	}

	return h, key, serve
}

// deterministicPayload returns a byte slice of `size` where every byte is a
// function of its index — lets boundary tests compare byte-for-byte without
// ambiguity. Size must be > 0.
func deterministicPayload(size int) []byte {
	out := make([]byte, size)
	for i := range out {
		// Interleave a small prime so no aligned buffer size accidentally
		// hides a shift bug.
		out[i] = byte((i*37 + 13) & 0xFF)
	}
	return out
}

// hashHexForTest returns a valid 64-lower-hex hash — content is arbitrary
// (the verifier doesn't need it to match anything).
func hashHexForTest(seed uint64) string {
	var buf [32]byte
	binary.BigEndian.PutUint64(buf[:], seed)
	out := make([]byte, 64)
	const hexDigits = "0123456789abcdef"
	for i, b := range buf {
		out[i*2] = hexDigits[b>>4]
		out[i*2+1] = hexDigits[b&0x0F]
	}
	return string(out)
}

// ---- Full-object GET (no Range header) ----------------------------------

func TestServeXorb_FullObject_200(t *testing.T) {
	t.Parallel()
	payload := deterministicPayload(1024)
	now := time.Unix(1_000_000, 0)
	hash := hashHexForTest(1)

	db := &fakeDB{size: int64(len(payload))}
	sia := &fakeSia{payload: payload}
	meter := &fakeMeter{}

	h, key, serve := newHandlersForTest(t, db, sia, meter, now)
	kid := uuid.New()
	q := urlSpec{hashHex: hash, exp: uint64(now.Unix()) + 60, kid: kid, key: key}.sign()

	rec := serve(q, hash, nil, nil)
	resp := rec.Result()
	defer resp.Body.Close()

	if resp.StatusCode != http.StatusOK {
		t.Fatalf("status = %d; want 200", resp.StatusCode)
	}
	if got := resp.Header.Get("Content-Length"); got != "1024" {
		t.Fatalf("Content-Length = %q; want 1024", got)
	}
	if got := resp.Header.Get("Accept-Ranges"); got != "bytes" {
		t.Fatalf("Accept-Ranges = %q; want 'bytes'", got)
	}
	if got := resp.Header.Get("X-Cache"); got != "MISS" {
		t.Fatalf("X-Cache = %q; want 'MISS' (Wave 3a always misses)", got)
	}
	if resp.Header.Get("X-Sia-Fetch-Ms") == "" {
		t.Fatalf("X-Sia-Fetch-Ms not set")
	}
	body, _ := io.ReadAll(resp.Body)
	if !bytes.Equal(body, payload) {
		t.Fatalf("body mismatch; first diff at %d", firstDiff(body, payload))
	}
	// Meter fires on goroutine; wait briefly.
	waitFor(t, func() bool {
		meter.mu.Lock()
		defer meter.mu.Unlock()
		return len(meter.calls) == 1
	}, time.Second)
	meter.mu.Lock()
	defer meter.mu.Unlock()
	if meter.calls[0].Bytes != int64(len(payload)) || meter.calls[0].APIKeyID != kid {
		t.Fatalf("meter call mismatch: %+v", meter.calls[0])
	}
	if meter.calls[0].CacheHit {
		t.Fatalf("meter reported CacheHit = true; want false (Wave 3a)")
	}
	_ = h
}

// ---- Single-range GET ----------------------------------------------------

func TestServeXorb_SingleRange_206(t *testing.T) {
	t.Parallel()
	payload := deterministicPayload(2048)
	now := time.Unix(1_000_000, 0)
	hash := hashHexForTest(2)

	db := &fakeDB{size: int64(len(payload))}
	sia := &fakeSia{payload: payload}
	meter := &fakeMeter{}

	_, key, serve := newHandlersForTest(t, db, sia, meter, now)
	q := urlSpec{hashHex: hash, exp: uint64(now.Unix()) + 60, kid: uuid.New(), key: key}.sign()

	hdrs := http.Header{"Range": []string{"bytes=100-199"}}
	rec := serve(q, hash, hdrs, nil)
	resp := rec.Result()
	defer resp.Body.Close()

	if resp.StatusCode != http.StatusPartialContent {
		t.Fatalf("status = %d; want 206", resp.StatusCode)
	}
	if got := resp.Header.Get("Content-Range"); got != "bytes 100-199/2048" {
		t.Fatalf("Content-Range = %q; want 'bytes 100-199/2048'", got)
	}
	if got := resp.Header.Get("Content-Length"); got != "100" {
		t.Fatalf("Content-Length = %q; want 100 (off-by-one?)", got)
	}
	body, _ := io.ReadAll(resp.Body)
	if len(body) != 100 {
		t.Fatalf("body length = %d; want 100", len(body))
	}
	if !bytes.Equal(body, payload[100:200]) {
		t.Fatalf("body content mismatch")
	}
}

// TestServeXorb_SingleRange_SuffixForm exercises the `-N` suffix-form parse
// path through the handler. Payload size 500, Range `bytes=-10` must return
// the last 10 bytes (indices 490..499) with Content-Range `490-499/500`.
func TestServeXorb_SingleRange_SuffixForm(t *testing.T) {
	t.Parallel()
	payload := deterministicPayload(500)
	now := time.Unix(1_000_000, 0)
	hash := hashHexForTest(3)

	db := &fakeDB{size: int64(len(payload))}
	sia := &fakeSia{payload: payload}

	_, key, serve := newHandlersForTest(t, db, sia, nil, now)
	q := urlSpec{hashHex: hash, exp: uint64(now.Unix()) + 60, kid: uuid.New(), key: key}.sign()
	rec := serve(q, hash, http.Header{"Range": []string{"bytes=-10"}}, nil)
	resp := rec.Result()
	defer resp.Body.Close()

	if resp.StatusCode != http.StatusPartialContent {
		t.Fatalf("status = %d; want 206", resp.StatusCode)
	}
	if got := resp.Header.Get("Content-Range"); got != "bytes 490-499/500" {
		t.Fatalf("Content-Range = %q", got)
	}
	body, _ := io.ReadAll(resp.Body)
	if !bytes.Equal(body, payload[490:500]) {
		t.Fatalf("body mismatch")
	}
}

// ---- Expired / tampered URLs => 403 (gotcha #8) --------------------------

func TestServeXorb_ExpiredURL_403(t *testing.T) {
	t.Parallel()
	payload := deterministicPayload(256)
	now := time.Unix(1_000_000, 0)
	hash := hashHexForTest(4)

	db := &fakeDB{size: int64(len(payload))}
	sia := &fakeSia{payload: payload}

	_, key, serve := newHandlersForTest(t, db, sia, nil, now)
	// exp one second in the past — verifier.Verify signals `expired`.
	q := urlSpec{hashHex: hash, exp: uint64(now.Unix()) - 1, kid: uuid.New(), key: key}.sign()
	rec := serve(q, hash, nil, nil)

	if rec.Code != http.StatusForbidden {
		t.Fatalf("status = %d; want 403 (gotcha #8)", rec.Code)
	}
	// Critical invariant: we must NOT touch the DB on a failed verify.
	// If we did, the DB call would appear in db.calls.
	if len(db.calls) != 0 {
		t.Fatalf("DB was consulted for expired URL: calls=%v (breaks timing uniformity)", db.calls)
	}
}

func TestServeXorb_TamperedURL_403(t *testing.T) {
	t.Parallel()
	payload := deterministicPayload(256)
	now := time.Unix(1_000_000, 0)
	hash := hashHexForTest(5)

	db := &fakeDB{size: int64(len(payload))}
	sia := &fakeSia{payload: payload}

	_, key, serve := newHandlersForTest(t, db, sia, nil, now)
	q := urlSpec{hashHex: hash, exp: uint64(now.Unix()) + 60, kid: uuid.New(), key: key}.sign()

	// Flip one character in the middle of the signature. A middle char encodes
	// a full 6 bits, so any non-self flip changes the decoded bytes. The last
	// base64 char encodes only 4 meaningful bits (for a 32-byte HMAC — 43
	// chars) and `RawURLEncoding` may treat certain flips there as equivalent
	// canonical encodings, producing a flaky 403 test.
	sig := q.Get("sig")
	mid := len(sig) / 2
	mc := sig[mid]
	var flipped byte = 'A'
	if mc == 'A' {
		flipped = 'B'
	}
	q.Set("sig", sig[:mid]+string(flipped)+sig[mid+1:])

	rec := serve(q, hash, nil, nil)
	if rec.Code != http.StatusForbidden {
		t.Fatalf("status = %d; want 403 (bad_signature → 403)", rec.Code)
	}
	if len(db.calls) != 0 {
		t.Fatalf("DB was consulted for tampered URL: calls=%v", db.calls)
	}
}

// ---- Malformed Range header => 400 --------------------------------------

func TestServeXorb_MalformedRange_400(t *testing.T) {
	t.Parallel()
	payload := deterministicPayload(128)
	now := time.Unix(1_000_000, 0)
	hash := hashHexForTest(6)

	db := &fakeDB{size: int64(len(payload))}
	sia := &fakeSia{payload: payload}

	_, key, serve := newHandlersForTest(t, db, sia, nil, now)
	q := urlSpec{hashHex: hash, exp: uint64(now.Unix()) + 60, kid: uuid.New(), key: key}.sign()

	rec := serve(q, hash, http.Header{"Range": []string{"bytes=10-5"}}, nil)
	if rec.Code != http.StatusBadRequest {
		t.Fatalf("status = %d; want 400", rec.Code)
	}
}

// ---- Unsatisfiable Range => 416 + Content-Range: */size -----------------

func TestServeXorb_UnsatisfiableRange_416(t *testing.T) {
	t.Parallel()
	payload := deterministicPayload(100)
	now := time.Unix(1_000_000, 0)
	hash := hashHexForTest(7)

	db := &fakeDB{size: int64(len(payload))}
	sia := &fakeSia{payload: payload}

	_, key, serve := newHandlersForTest(t, db, sia, nil, now)
	q := urlSpec{hashHex: hash, exp: uint64(now.Unix()) + 60, kid: uuid.New(), key: key}.sign()

	rec := serve(q, hash, http.Header{"Range": []string{"bytes=500-600"}}, nil)
	if rec.Code != http.StatusRequestedRangeNotSatisfiable {
		t.Fatalf("status = %d; want 416", rec.Code)
	}
	if got := rec.Header().Get("Content-Range"); got != "bytes */100" {
		t.Fatalf("Content-Range = %q; want 'bytes */100' (RFC 7233 §4.4)", got)
	}
}

// ---- Xorb not found => 404 ----------------------------------------------

func TestServeXorb_NotFound_404(t *testing.T) {
	t.Parallel()
	now := time.Unix(1_000_000, 0)
	hash := hashHexForTest(8)

	db := &fakeDB{err: ErrXorbNotFound}
	sia := &fakeSia{}

	_, key, serve := newHandlersForTest(t, db, sia, nil, now)
	q := urlSpec{hashHex: hash, exp: uint64(now.Unix()) + 60, kid: uuid.New(), key: key}.sign()

	rec := serve(q, hash, nil, nil)
	if rec.Code != http.StatusNotFound {
		t.Fatalf("status = %d; want 404", rec.Code)
	}
	// 404 is distinct from 403 — gotcha #8 mandates that `bad_signature` is
	// indistinguishable from a real miss at the client level. We accept that
	// the INTERNAL code path is different (verify passes, DB misses); the
	// client-observable difference is a side-effect, documented here so nobody
	// "improves" this handler to return 403 on a real miss.
}

// ---- Multi-range => 206 multipart/byteranges (gotcha #3 / plan 03-04) ---

// TestServeXorb_MultiRange_206_Multipart — the HTTP-level gold-standard
// integration test for multi-range serving. Wires a real Handlers bundle
// through `r.ServeHTTP`, issues a multi-range request against a verified
// signed URL, and parses the response with `mime/multipart.NewReader`.
//
// Every assertion here defends GOTCHA #3: a concatenated body would silently
// corrupt xet-core's V2 reconstruction downloads.
func TestServeXorb_MultiRange_206_Multipart(t *testing.T) {
	t.Parallel()
	payload := deterministicPayload(2048)
	now := time.Unix(1_000_000, 0)
	hash := hashHexForTest(9)

	db := &fakeDB{size: int64(len(payload))}
	sia := &fakeSia{payload: payload}
	meter := &fakeMeter{}

	_, key, serve := newHandlersForTest(t, db, sia, meter, now)
	kid := uuid.New()
	q := urlSpec{hashHex: hash, exp: uint64(now.Unix()) + 60, kid: kid, key: key}.sign()

	// Three disjoint ranges spanning start / middle / end of the object.
	rec := serve(q, hash, http.Header{"Range": []string{"bytes=0-99,200-299,1900-1999"}}, nil)
	resp := rec.Result()
	defer resp.Body.Close()

	if resp.StatusCode != http.StatusPartialContent {
		t.Fatalf("status = %d; want 206", resp.StatusCode)
	}
	// Content-Type must be multipart/byteranges + boundary must be our pinned const.
	ct := resp.Header.Get("Content-Type")
	wantCT := "multipart/byteranges; boundary=" + MultipartBoundary
	if ct != wantCT {
		t.Fatalf("Content-Type = %q; want %q", ct, wantCT)
	}
	if ar := resp.Header.Get("Accept-Ranges"); ar != "bytes" {
		t.Fatalf("Accept-Ranges = %q; want bytes", ar)
	}
	// X-Cache / X-Sia-Fetch-Ms echoed from the source branch.
	if xc := resp.Header.Get("X-Cache"); xc != "MISS" {
		t.Fatalf("X-Cache = %q; want MISS (Wave 3a/3b MISS path)", xc)
	}
	if resp.Header.Get("X-Sia-Fetch-Ms") == "" {
		t.Fatalf("X-Sia-Fetch-Ms missing")
	}

	// -- Parse with mime/multipart.NewReader (gold-standard round-trip) --
	// Snapshot body because the reader consumes it.
	body, err := io.ReadAll(resp.Body)
	if err != nil {
		t.Fatalf("read body: %v", err)
	}

	// ANTI-REGRESSION (gotcha #3): assert the boundary literal is IN the body.
	// A concatenated body would lack "--xet_multipart_boundary".
	if !strings.Contains(string(body), "--"+MultipartBoundary) {
		t.Fatalf("body missing boundary marker — CONCATENATED BODY? GOTCHA #3 REGRESSION!")
	}
	if !strings.Contains(string(body), "--"+MultipartBoundary+"--") {
		t.Fatalf("body missing closing boundary — truncated framing")
	}

	// Parse.
	_, params, err := mime.ParseMediaType(ct)
	if err != nil {
		t.Fatalf("ParseMediaType(%q): %v", ct, err)
	}
	mr := multipart.NewReader(bytes.NewReader(body), params["boundary"])
	wantRanges := []struct {
		Start, End int
	}{{0, 99}, {200, 299}, {1900, 1999}}
	for i, want := range wantRanges {
		p, err := mr.NextPart()
		if err != nil {
			t.Fatalf("part[%d] NextPart: %v", i, err)
		}
		wantCR := fmt.Sprintf("bytes %d-%d/2048", want.Start, want.End)
		if got := p.Header.Get("Content-Range"); got != wantCR {
			t.Fatalf("part[%d] Content-Range = %q; want %q", i, got, wantCR)
		}
		bodyBytes, err := io.ReadAll(p)
		if err != nil {
			t.Fatalf("part[%d] read body: %v", i, err)
		}
		if !bytes.Equal(bodyBytes, payload[want.Start:want.End+1]) {
			t.Fatalf("part[%d] body bytes mismatch (first diff at %d)",
				i, firstDiff(bodyBytes, payload[want.Start:want.End+1]))
		}
	}
	// Must be EOF after N parts.
	if _, err := mr.NextPart(); err != io.EOF {
		t.Fatalf("NextPart past end: got %v; want EOF", err)
	}

	// Meter: total bytes = 100 + 100 + 100 = 300.
	waitFor(t, func() bool {
		meter.mu.Lock()
		defer meter.mu.Unlock()
		return len(meter.calls) == 1
	}, time.Second)
	meter.mu.Lock()
	defer meter.mu.Unlock()
	if meter.calls[0].APIKeyID != kid {
		t.Fatalf("meter APIKeyID = %v; want %v", meter.calls[0].APIKeyID, kid)
	}
	if meter.calls[0].Bytes != 300 {
		t.Fatalf("meter Bytes = %d; want 300 (sum of three 100-byte ranges)", meter.calls[0].Bytes)
	}
	if meter.calls[0].CacheHit {
		t.Fatalf("meter CacheHit = true; want false (MISS path)")
	}
}

// TestServeXorb_MultiRange_BoundedURL_206 — signed-URL bound + multi-range
// within the bound: must emit a multipart response (not 403) because every
// requested range is inside the grant.
func TestServeXorb_MultiRange_BoundedURL_206(t *testing.T) {
	t.Parallel()
	payload := deterministicPayload(1024)
	now := time.Unix(1_000_000, 0)
	hash := hashHexForTest(90)

	db := &fakeDB{size: int64(len(payload))}
	sia := &fakeSia{payload: payload}

	_, key, serve := newHandlersForTest(t, db, sia, nil, now)
	// Grant 100..499; client asks for two disjoint ranges fully inside.
	q := urlSpec{
		hashHex: hash, exp: uint64(now.Unix()) + 60, kid: uuid.New(), key: key,
		rng: &[2]uint64{100, 499},
	}.sign()

	rec := serve(q, hash, http.Header{"Range": []string{"bytes=150-199,300-399"}}, nil)
	resp := rec.Result()
	defer resp.Body.Close()

	if resp.StatusCode != http.StatusPartialContent {
		t.Fatalf("status = %d; want 206 (ranges within grant)", resp.StatusCode)
	}
	ct := resp.Header.Get("Content-Type")
	if !strings.HasPrefix(ct, "multipart/byteranges;") {
		t.Fatalf("Content-Type = %q; want multipart/byteranges", ct)
	}
	body, _ := io.ReadAll(resp.Body)
	_, params, _ := mime.ParseMediaType(ct)
	mr := multipart.NewReader(bytes.NewReader(body), params["boundary"])

	wantSlices := [][]byte{payload[150:200], payload[300:400]}
	for i, want := range wantSlices {
		p, err := mr.NextPart()
		if err != nil {
			t.Fatalf("part[%d]: %v", i, err)
		}
		got, _ := io.ReadAll(p)
		if !bytes.Equal(got, want) {
			t.Fatalf("part[%d] bytes mismatch", i)
		}
	}
	if _, err := mr.NextPart(); err != io.EOF {
		t.Fatalf("NextPart past end: got %v; want EOF", err)
	}
}

// TestServeXorb_MultiRange_BoundedURL_Escalation_403 — multi-range where ONE
// of the ranges spills outside the signed-URL grant. Must return 403, never
// a partial-content response. (If we accidentally only checked the first
// range, this catches the regression.)
func TestServeXorb_MultiRange_BoundedURL_Escalation_403(t *testing.T) {
	t.Parallel()
	payload := deterministicPayload(1024)
	now := time.Unix(1_000_000, 0)
	hash := hashHexForTest(91)

	db := &fakeDB{size: int64(len(payload))}
	sia := &fakeSia{payload: payload}

	_, key, serve := newHandlersForTest(t, db, sia, nil, now)
	q := urlSpec{
		hashHex: hash, exp: uint64(now.Unix()) + 60, kid: uuid.New(), key: key,
		rng: &[2]uint64{100, 299},
	}.sign()

	// First range fits; second escapes the grant.
	rec := serve(q, hash, http.Header{"Range": []string{"bytes=150-199,400-499"}}, nil)
	if rec.Code != http.StatusForbidden {
		t.Fatalf("status = %d; want 403 (multi-range escalation)", rec.Code)
	}
}

// ---- Signed-URL-bound range enforcement ---------------------------------

func TestServeXorb_BoundedURL_AllowsSubset(t *testing.T) {
	t.Parallel()
	payload := deterministicPayload(1024)
	now := time.Unix(1_000_000, 0)
	hash := hashHexForTest(10)

	db := &fakeDB{size: int64(len(payload))}
	sia := &fakeSia{payload: payload}

	_, key, serve := newHandlersForTest(t, db, sia, nil, now)
	// URL grants bytes 100..499. Client asks for 200..299 — subset; allowed.
	q := urlSpec{
		hashHex: hash, exp: uint64(now.Unix()) + 60, kid: uuid.New(), key: key,
		rng: &[2]uint64{100, 499},
	}.sign()

	rec := serve(q, hash, http.Header{"Range": []string{"bytes=200-299"}}, nil)
	if rec.Code != http.StatusPartialContent {
		t.Fatalf("status = %d; want 206", rec.Code)
	}
	body := rec.Body.Bytes()
	if !bytes.Equal(body, payload[200:300]) {
		t.Fatalf("body mismatch")
	}
}

func TestServeXorb_BoundedURL_RejectsEscalation(t *testing.T) {
	t.Parallel()
	payload := deterministicPayload(1024)
	now := time.Unix(1_000_000, 0)
	hash := hashHexForTest(11)

	db := &fakeDB{size: int64(len(payload))}
	sia := &fakeSia{payload: payload}

	_, key, serve := newHandlersForTest(t, db, sia, nil, now)
	// URL grants bytes 100..299. Client asks for 50..200 — lower bound outside grant.
	q := urlSpec{
		hashHex: hash, exp: uint64(now.Unix()) + 60, kid: uuid.New(), key: key,
		rng: &[2]uint64{100, 299},
	}.sign()

	rec := serve(q, hash, http.Header{"Range": []string{"bytes=50-200"}}, nil)
	if rec.Code != http.StatusForbidden {
		t.Fatalf("status = %d; want 403 (escalation)", rec.Code)
	}
}

// TestServeXorb_BoundedURL_NoClientRangeDefaultsToBound — when the client
// omits the Range header but the URL is bounded, we serve exactly the
// bounded region so a bounded signed URL never spills the whole xorb. The
// response is 206 (NOT 200) because the body is a slice.
func TestServeXorb_BoundedURL_NoClientRangeDefaultsToBound(t *testing.T) {
	t.Parallel()
	payload := deterministicPayload(1024)
	now := time.Unix(1_000_000, 0)
	hash := hashHexForTest(12)

	db := &fakeDB{size: int64(len(payload))}
	sia := &fakeSia{payload: payload}

	_, key, serve := newHandlersForTest(t, db, sia, nil, now)
	q := urlSpec{
		hashHex: hash, exp: uint64(now.Unix()) + 60, kid: uuid.New(), key: key,
		rng: &[2]uint64{400, 499},
	}.sign()

	rec := serve(q, hash, nil, nil) // no Range header
	if rec.Code != http.StatusPartialContent {
		t.Fatalf("status = %d; want 206", rec.Code)
	}
	body := rec.Body.Bytes()
	if !bytes.Equal(body, payload[400:500]) {
		t.Fatalf("bounded default body mismatch")
	}
	if got := rec.Header().Get("Content-Range"); got != "bytes 400-499/1024" {
		t.Fatalf("Content-Range = %q", got)
	}
}

// ---- Client-disconnect propagation (GATE-10) ----------------------------

// TestServeXorb_ClientDisconnect_CancelsSia wires a blocking fakeSia and
// cancels the request context mid-download. The handler MUST propagate the
// cancel to the Sia call (via r.Context()) and return without writing 5xx —
// the client is gone, the TCP is dead.
func TestServeXorb_ClientDisconnect_CancelsSia(t *testing.T) {
	t.Parallel()
	now := time.Unix(1_000_000, 0)
	hash := hashHexForTest(13)

	db := &fakeDB{size: 1 << 20}
	sia := &fakeSia{block: true}

	_, key, serve := newHandlersForTest(t, db, sia, nil, now)
	q := urlSpec{hashHex: hash, exp: uint64(now.Unix()) + 60, kid: uuid.New(), key: key}.sign()

	ctx, cancel := context.WithCancel(context.Background())

	done := make(chan *httptest.ResponseRecorder, 1)
	go func() {
		done <- serve(q, hash, nil, ctx)
	}()

	time.Sleep(50 * time.Millisecond)
	cancel()

	select {
	case rec := <-done:
		// The handler MUST NOT have written 200 + a body (the Sia call failed).
		// Accept any non-200 status OR an empty body (httptest.ResponseRecorder
		// defaults Code to 200 even when the handler writes nothing — on the
		// wire, a canceled request emits no status at all).
		if rec.Code == http.StatusOK && rec.Body.Len() > 0 {
			t.Fatalf("handler wrote 200 + %dB body for canceled request; want no body or 5xx",
				rec.Body.Len())
		}
	case <-time.After(2 * time.Second):
		t.Fatalf("handler did not return within 2s after client disconnect (GATE-10 violation)")
	}
}

// ---- Nil-dep safety (main.go may boot without Sia/DB) -------------------

func TestServeXorb_NoDB_500(t *testing.T) {
	t.Parallel()
	now := time.Unix(1_000_000, 0)
	hash := hashHexForTest(14)

	_, key, serve := newHandlersForTest(t, nil, &fakeSia{}, nil, now)
	q := urlSpec{hashHex: hash, exp: uint64(now.Unix()) + 60, kid: uuid.New(), key: key}.sign()

	rec := serve(q, hash, nil, nil)
	if rec.Code != http.StatusInternalServerError {
		t.Fatalf("status = %d; want 500 (DB not wired)", rec.Code)
	}
}

func TestServeXorb_NoSia_500(t *testing.T) {
	t.Parallel()
	now := time.Unix(1_000_000, 0)
	hash := hashHexForTest(15)

	db := &fakeDB{size: 1024}
	_, key, serve := newHandlersForTest(t, db, nil, nil, now)
	q := urlSpec{hashHex: hash, exp: uint64(now.Unix()) + 60, kid: uuid.New(), key: key}.sign()

	rec := serve(q, hash, nil, nil)
	if rec.Code != http.StatusInternalServerError {
		t.Fatalf("status = %d; want 500 (Sia not wired)", rec.Code)
	}
}

// ---- DB returning a non-ErrXorbNotFound error => 500 --------------------

func TestServeXorb_DBError_500(t *testing.T) {
	t.Parallel()
	now := time.Unix(1_000_000, 0)
	hash := hashHexForTest(16)

	db := &fakeDB{err: errors.New("pg: connection refused")}
	_, key, serve := newHandlersForTest(t, db, &fakeSia{}, nil, now)
	q := urlSpec{hashHex: hash, exp: uint64(now.Unix()) + 60, kid: uuid.New(), key: key}.sign()

	rec := serve(q, hash, nil, nil)
	if rec.Code != http.StatusInternalServerError {
		t.Fatalf("status = %d; want 500", rec.Code)
	}
}

// ---- Sia returning an error => 502 --------------------------------------

func TestServeXorb_SiaError_502(t *testing.T) {
	t.Parallel()
	now := time.Unix(1_000_000, 0)
	hash := hashHexForTest(17)

	db := &fakeDB{size: 1024}
	sia := &fakeSia{err: errors.New("simulated sia outage")}

	_, key, serve := newHandlersForTest(t, db, sia, nil, now)
	q := urlSpec{hashHex: hash, exp: uint64(now.Unix()) + 60, kid: uuid.New(), key: key}.sign()

	rec := serve(q, hash, nil, nil)
	if rec.Code != http.StatusBadGateway {
		t.Fatalf("status = %d; want 502", rec.Code)
	}
}

// TestServeXorb_SiaSizeMismatch_502 — the sia temp-file stat must equal the
// Postgres size_bytes. A mismatch must surface as a 5xx, never as a corrupted
// response. fakeSia with a shorter payload than size exercises this.
func TestServeXorb_SiaSizeMismatch_502(t *testing.T) {
	t.Parallel()
	now := time.Unix(1_000_000, 0)
	hash := hashHexForTest(18)

	// DB claims 1024 bytes but Sia only serves 512 — the handler must detect.
	db := &fakeDB{size: 1024}
	sia := &fakeSia{payload: deterministicPayload(512)}

	_, key, serve := newHandlersForTest(t, db, sia, nil, now)
	q := urlSpec{hashHex: hash, exp: uint64(now.Unix()) + 60, kid: uuid.New(), key: key}.sign()

	rec := serve(q, hash, nil, nil)
	if rec.Code != http.StatusBadGateway {
		t.Fatalf("status = %d; want 502 (size mismatch is a gateway-detected integrity failure)", rec.Code)
	}
}

// TestServeXorb_HealthUnaffected — `/health` works without any deps wired.
func TestServeXorb_HealthUnaffected(t *testing.T) {
	t.Parallel()
	h, _, _ := newHandlersForTest(t, nil, nil, nil, time.Unix(1_000_000, 0))
	req := httptest.NewRequest(http.MethodGet, "/health", nil)
	rec := httptest.NewRecorder()
	h.HealthHandler(rec, req)
	if rec.Code != http.StatusOK {
		t.Fatalf("health status = %d; want 200", rec.Code)
	}
	if !strings.Contains(rec.Body.String(), `"ok"`) {
		t.Fatalf("health body = %q; want 'ok'", rec.Body.String())
	}
}

// waitFor polls `cond` until it returns true or `timeout` elapses. Used by
// the full-object test to wait for the async meter goroutine.
func waitFor(t *testing.T, cond func() bool, timeout time.Duration) {
	t.Helper()
	deadline := time.Now().Add(timeout)
	for time.Now().Before(deadline) {
		if cond() {
			return
		}
		time.Sleep(5 * time.Millisecond)
	}
	t.Fatalf("waitFor: condition never satisfied within %s", timeout)
}
