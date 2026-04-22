// ranges_test.go — Parser + single-range writer unit tests.
//
// Covers the 11 canonical cases from plan 03-03 Task 1 plus a boundary-drill
// writer test that pins the HTTP-end-inclusive → SDK-offset+length mapping.
// If this file's TestWriteSingleRange_BoundaryOffByOne ever fails, treat it
// as a STOP-THE-LINE event: every stored xorb download is at risk of silent
// last-byte corruption.
package main

import (
	"bytes"
	"errors"
	"fmt"
	"io"
	"mime"
	"mime/multipart"
	"net/http"
	"net/http/httptest"
	"strconv"
	"strings"
	"testing"
)

// TestParseRange_Valid enumerates every canonical shape we expect to PASS and
// asserts BOTH fields of each returned Range — a zero-value-tolerant test
// would hide the classic "end=size vs end=size-1" off-by-one.
func TestParseRange_Valid(t *testing.T) {
	t.Parallel()
	cases := []struct {
		name   string
		header string
		size   int64
		want   []Range
	}{
		{"simple_0_99_of_1024", "bytes=0-99", 1024, []Range{{0, 99}}},
		{"zero_byte_range_0_0", "bytes=0-0", 1, []Range{{0, 0}}},
		{"last_byte_only", "bytes=1023-1023", 1024, []Range{{1023, 1023}}},
		{"open_form_to_end", "bytes=0-", 100, []Range{{0, 99}}},
		{"open_form_midpoint", "bytes=50-", 100, []Range{{50, 99}}},
		{"suffix_last_10", "bytes=-10", 100, []Range{{90, 99}}},
		{"suffix_larger_than_size_clamps", "bytes=-500", 100, []Range{{0, 99}}},
		// Multi-spec is ALLOWED through the parser (03-04 uses it); 03-03's
		// handler rejects it at the writer boundary with 501. Parser test just
		// proves the slice flows through.
		{"multi_spec_passes_through", "bytes=0-10,20-30", 100, []Range{{0, 10}, {20, 30}}},
	}
	for _, tc := range cases {
		tc := tc
		t.Run(tc.name, func(t *testing.T) {
			t.Parallel()
			got, err := ParseRange(tc.header, tc.size)
			if err != nil {
				t.Fatalf("ParseRange(%q, %d) unexpected err: %v", tc.header, tc.size, err)
			}
			if len(got) != len(tc.want) {
				t.Fatalf("got %d ranges, want %d", len(got), len(tc.want))
			}
			for i := range got {
				if got[i].Start != tc.want[i].Start || got[i].End != tc.want[i].End {
					t.Fatalf("range[%d] = {%d,%d}; want {%d,%d}",
						i, got[i].Start, got[i].End, tc.want[i].Start, tc.want[i].End)
				}
			}
		})
	}
}

// TestParseRange_BadRange collects every malformed-input case. These map to
// HTTP 400 at the handler layer.
func TestParseRange_BadRange(t *testing.T) {
	t.Parallel()
	cases := []struct {
		name, header string
		size         int64
	}{
		{"start_greater_than_end", "bytes=10-5", 100},
		{"missing_bytes_prefix", "0-99", 100},
		{"empty_string", "", 100},
		{"bytes_prefix_only", "bytes=", 100},
		{"bare_dash", "bytes=-", 100},
		{"zero_suffix", "bytes=-0", 100},
		{"leading_comma", "bytes=,0-9", 100},
		{"trailing_comma", "bytes=0-9,", 100},
		{"empty_middle_spec", "bytes=0-9,,10-19", 100},
		{"non_numeric_start", "bytes=abc-10", 100},
		{"non_numeric_end", "bytes=0-xyz", 100},
		{"no_dash_in_spec", "bytes=99", 100},
	}
	for _, tc := range cases {
		tc := tc
		t.Run(tc.name, func(t *testing.T) {
			t.Parallel()
			_, err := ParseRange(tc.header, tc.size)
			if !errors.Is(err, ErrBadRange) {
				t.Fatalf("ParseRange(%q): got err = %v; want ErrBadRange", tc.header, err)
			}
		})
	}
}

// TestParseRange_Unsatisfiable collects inputs that PARSE but cannot be
// served against the given size. These map to HTTP 416.
func TestParseRange_Unsatisfiable(t *testing.T) {
	t.Parallel()
	cases := []struct {
		name, header string
		size         int64
	}{
		{"end_beyond_size", "bytes=100-200", 100},
		{"start_equals_size", "bytes=100-100", 100},
		{"open_form_start_beyond", "bytes=200-", 100},
		{"empty_object_fully_specified", "bytes=0-0", 0},
		{"empty_object_open_form", "bytes=0-", 0},
		{"empty_object_suffix", "bytes=-10", 0},
	}
	for _, tc := range cases {
		tc := tc
		t.Run(tc.name, func(t *testing.T) {
			t.Parallel()
			_, err := ParseRange(tc.header, tc.size)
			if !errors.Is(err, ErrUnsatisfiable) {
				t.Fatalf("ParseRange(%q): got err = %v; want ErrUnsatisfiable", tc.header, err)
			}
		})
	}
}

// TestRange_Length documents the `Range.Length()` contract. Length of [0,0]
// is 1 byte, NOT 0. Easy to typo; guard with an explicit test.
func TestRange_Length(t *testing.T) {
	t.Parallel()
	cases := []struct {
		r    Range
		want uint64
	}{
		{Range{0, 0}, 1},
		{Range{0, 99}, 100},
		{Range{100, 199}, 100},
		{Range{1023, 1023}, 1},
	}
	for _, tc := range cases {
		if got := tc.r.Length(); got != tc.want {
			t.Fatalf("Range{%d,%d}.Length() = %d; want %d", tc.r.Start, tc.r.End, got, tc.want)
		}
	}
}

// TestAllRangesWithin covers the handler's signed-URL-bound enforcement.
// Verifier returns an inclusive `[2]uint64`; the handler wraps it as a Range
// and demands every client-requested range sit inside it.
func TestAllRangesWithin(t *testing.T) {
	t.Parallel()
	bound := Range{100, 200}
	cases := []struct {
		name   string
		ranges []Range
		want   bool
	}{
		{"empty_is_within", nil, true},
		{"single_within", []Range{{150, 160}}, true},
		{"exact_boundary", []Range{{100, 200}}, true},
		{"starts_below_bound", []Range{{50, 150}}, false},
		{"ends_above_bound", []Range{{150, 250}}, false},
		{"spans_bound", []Range{{0, 300}}, false},
		{"multi_all_within", []Range{{100, 120}, {180, 200}}, true},
		{"multi_one_escapes", []Range{{100, 120}, {180, 210}}, false},
	}
	for _, tc := range cases {
		tc := tc
		t.Run(tc.name, func(t *testing.T) {
			t.Parallel()
			if got := allRangesWithin(tc.ranges, bound); got != tc.want {
				t.Fatalf("allRangesWithin(%v, %v) = %v; want %v", tc.ranges, bound, got, tc.want)
			}
		})
	}
}

// TestWriteSingleRange_BoundaryOffByOne is the single most load-bearing
// assertion in this file. An end-inclusive range `bytes=100-199` against a
// 1 KiB payload must emit EXACTLY 100 bytes (indices 100..199) — NOT 99
// (classic off-by-one), NOT 101 (wrong direction off-by-one), NOT something
// starting at 101 (offset misalignment).
//
// If this ever fails, the breakage is grant-story-level: every range-serving
// response corrupts content at the boundary. Treat as STOP-THE-LINE.
func TestWriteSingleRange_BoundaryOffByOne(t *testing.T) {
	t.Parallel()
	// Deterministic payload whose every byte identifies its own index mod 251
	// (251 is prime; avoids accidental alignment with any internal buffer size).
	payload := make([]byte, 1024)
	for i := range payload {
		payload[i] = byte(i % 251)
	}
	src := bytes.NewReader(payload)

	rec := httptest.NewRecorder()
	err := writeSingleRange(rec, Range{Start: 100, End: 199}, int64(len(payload)), src)
	if err != nil {
		t.Fatalf("writeSingleRange: %v", err)
	}

	resp := rec.Result()
	defer resp.Body.Close()

	if resp.StatusCode != 206 {
		t.Fatalf("status = %d; want 206", resp.StatusCode)
	}
	if got := resp.Header.Get("Content-Range"); got != "bytes 100-199/1024" {
		t.Fatalf("Content-Range = %q; want %q", got, "bytes 100-199/1024")
	}
	if got := resp.Header.Get("Content-Length"); got != "100" {
		t.Fatalf("Content-Length = %q; want %q (off-by-one?)", got, "100")
	}
	if got := resp.Header.Get("Accept-Ranges"); got != "bytes" {
		t.Fatalf("Accept-Ranges = %q; want %q", got, "bytes")
	}
	body, err := io.ReadAll(resp.Body)
	if err != nil {
		t.Fatalf("read body: %v", err)
	}
	if len(body) != 100 {
		t.Fatalf("body length = %d; want 100 (off-by-one?)", len(body))
	}
	// Exact-byte compare: indices 100..199 of the payload.
	if !bytes.Equal(body, payload[100:200]) {
		t.Fatalf("body content mismatch; first diff index = %d", firstDiff(body, payload[100:200]))
	}
}

// TestWriteSingleRange_SingleByte exercises the zero-length-like edge: a
// `bytes=0-0` one-byte range. The Length() formula reduces to 1 and the
// body must be exactly payload[0].
func TestWriteSingleRange_SingleByte(t *testing.T) {
	t.Parallel()
	payload := []byte{0xAB, 0xCD, 0xEF}
	rec := httptest.NewRecorder()
	if err := writeSingleRange(rec, Range{0, 0}, int64(len(payload)), bytes.NewReader(payload)); err != nil {
		t.Fatalf("writeSingleRange: %v", err)
	}
	resp := rec.Result()
	defer resp.Body.Close()
	if resp.StatusCode != 206 {
		t.Fatalf("status = %d; want 206", resp.StatusCode)
	}
	if got := resp.Header.Get("Content-Length"); got != "1" {
		t.Fatalf("Content-Length = %q; want 1", got)
	}
	if got := resp.Header.Get("Content-Range"); got != "bytes 0-0/3" {
		t.Fatalf("Content-Range = %q", got)
	}
	body, _ := io.ReadAll(resp.Body)
	if len(body) != 1 || body[0] != 0xAB {
		t.Fatalf("body = %x; want [0xAB]", body)
	}
}

// TestWriteSingleRange_LastByteOnly mirrors the fixture-derived `size-1..size-1`
// case. Regression tripwire for clamp bugs in the open-form parser path
// propagating into the writer.
func TestWriteSingleRange_LastByteOnly(t *testing.T) {
	t.Parallel()
	payload := make([]byte, 512)
	for i := range payload {
		payload[i] = byte(i)
	}
	payload[511] = 0x5A // sentinel

	rec := httptest.NewRecorder()
	if err := writeSingleRange(rec, Range{511, 511}, 512, bytes.NewReader(payload)); err != nil {
		t.Fatalf("writeSingleRange: %v", err)
	}
	resp := rec.Result()
	defer resp.Body.Close()
	if got := resp.Header.Get("Content-Range"); got != "bytes 511-511/512" {
		t.Fatalf("Content-Range = %q", got)
	}
	body, _ := io.ReadAll(resp.Body)
	if len(body) != 1 || body[0] != 0x5A {
		t.Fatalf("body = %x; want [0x5A]", body)
	}
}

// TestParseRange_EndInclusiveBoundary — a dedicated cross-check for the
// parser's open-form arithmetic: on a 1-byte object, `bytes=0-` must parse to
// {0,0} (NOT {0,1}). This is the parser-side mirror of the writer off-by-one
// test above.
func TestParseRange_EndInclusiveBoundary(t *testing.T) {
	t.Parallel()
	got, err := ParseRange("bytes=0-", 1)
	if err != nil {
		t.Fatalf("ParseRange: %v", err)
	}
	if len(got) != 1 || got[0].Start != 0 || got[0].End != 0 {
		t.Fatalf("got %v; want [{0,0}]", got)
	}
	// For a 1-byte object a `bytes=-1` suffix also lands on {0,0}.
	got, err = ParseRange("bytes=-1", 1)
	if err != nil {
		t.Fatalf("ParseRange: %v", err)
	}
	if len(got) != 1 || got[0].Start != 0 || got[0].End != 0 {
		t.Fatalf("got %v; want [{0,0}]", got)
	}
}

// firstDiff returns the index of the first byte where a != b, or -1 if they
// match. Used by the boundary test to produce actionable failure messages.
func firstDiff(a, b []byte) int {
	n := len(a)
	if len(b) < n {
		n = len(b)
	}
	for i := 0; i < n; i++ {
		if a[i] != b[i] {
			return i
		}
	}
	if len(a) != len(b) {
		return n
	}
	return -1
}

// Statement coverage helper — compile-time guard that ErrBadRange and
// ErrUnsatisfiable are distinct sentinel values. If someone merges them or
// reassigns one to the other, this test fails loudly.
func TestParseRangeSentinelsDistinct(t *testing.T) {
	t.Parallel()
	if errors.Is(ErrBadRange, ErrUnsatisfiable) || errors.Is(ErrUnsatisfiable, ErrBadRange) {
		t.Fatalf("ErrBadRange and ErrUnsatisfiable must be distinct sentinels")
	}
	// Sanity: Error() strings must not be empty.
	if ErrBadRange.Error() == "" || ErrUnsatisfiable.Error() == "" {
		t.Fatalf("sentinel error strings must not be empty")
	}
	// Docstring sanity: ErrBadRange message mentions "Range" somewhere —
	// keeps a breadcrumb in logs if it ever surfaces.
	if !strings.Contains(strings.ToLower(ErrBadRange.Error()), "range") {
		t.Fatalf("ErrBadRange.Error() = %q; want 'Range' substring", ErrBadRange.Error())
	}
	_ = fmt.Sprint // keep import alive for future expanded msg asserts
}

// ============================================================================
// Multipart/byteranges (RFC 7233 §4.1) tests — plan 03-04.
//
// THE SINGLE MOST INTEGRITY-CRITICAL CODE PATH IN PHASE 3 (gotcha #3).
//
// Strategy:
//   1. Use Go's `mime/multipart.NewReader` to PARSE the response body back
//      into parts — this is the same shape xet-core's parser expects, and a
//      stdlib round-trip is the gold-standard proof of RFC-7233-compliant
//      framing.
//   2. Anti-concatenation tripwire: explicitly assert the `--boundary`
//      markers exist in the serialized body. A regression that emits a raw
//      concatenation would fail this test hard.
//   3. Content-Length drift catch: compare the precomputed header length to
//      `len(body)` — any off-by-CRLF mismatch between the precompute math
//      and the stdlib writer's actual output trips this.
// ============================================================================

// deterministicMultipartPayload returns a byte slice where byte i = (i * 67
// + 11) & 0xFF. Like the single-range deterministicPayload but shifted so the
// two tests exercise slightly different byte patterns — catches any accidental
// test-value aliasing between the writers.
func deterministicMultipartPayload(size int) []byte {
	out := make([]byte, size)
	for i := range out {
		out[i] = byte((i*67 + 11) & 0xFF)
	}
	return out
}

// readMultipartResponse is the standard round-trip helper: it parses the
// response body with `mime/multipart.NewReader` and returns a slice of
// (content-range, body) pairs. Any parse error is fatal — the test treats
// this as framing corruption because stdlib is the reference implementation.
func readMultipartResponse(t *testing.T, resp *http.Response) []struct {
	ContentRange string
	Body         []byte
} {
	t.Helper()
	ct := resp.Header.Get("Content-Type")
	mediaType, params, err := mime.ParseMediaType(ct)
	if err != nil {
		t.Fatalf("ParseMediaType(%q): %v", ct, err)
	}
	if !strings.HasPrefix(mediaType, "multipart/") {
		t.Fatalf("mediaType = %q; want multipart/*", mediaType)
	}
	boundary := params["boundary"]
	if boundary == "" {
		t.Fatalf("no boundary in Content-Type %q", ct)
	}
	// Snapshot the body so both the parse-round-trip AND the anti-regression
	// byte-search can operate on the same bytes.
	bodyBytes, err := io.ReadAll(resp.Body)
	if err != nil {
		t.Fatalf("read body: %v", err)
	}
	mr := multipart.NewReader(bytes.NewReader(bodyBytes), boundary)
	var parts []struct {
		ContentRange string
		Body         []byte
	}
	for {
		p, err := mr.NextPart()
		if errors.Is(err, io.EOF) {
			break
		}
		if err != nil {
			t.Fatalf("multipart NextPart (part %d): %v", len(parts), err)
		}
		b, err := io.ReadAll(p)
		if err != nil {
			t.Fatalf("read part %d: %v", len(parts), err)
		}
		parts = append(parts, struct {
			ContentRange string
			Body         []byte
		}{
			ContentRange: p.Header.Get("Content-Range"),
			Body:         b,
		})
	}
	return parts
}

// TestMultipartRanges_Basic is the canonical round-trip: three disjoint
// ranges, parsed back with mime/multipart.NewReader, asserting each part's
// Content-Range header + body bytes match the source slice.
func TestMultipartRanges_Basic(t *testing.T) {
	t.Parallel()
	total := int64(10_000)
	src := deterministicMultipartPayload(int(total))
	ranges := []Range{{0, 99}, {200, 299}, {9900, 9999}}

	rec := httptest.NewRecorder()
	if err := writeMultipartRanges(rec, ranges, total, bytes.NewReader(src)); err != nil {
		t.Fatalf("writeMultipartRanges: %v", err)
	}

	resp := rec.Result()
	defer resp.Body.Close()
	if resp.StatusCode != http.StatusPartialContent {
		t.Fatalf("status = %d; want 206", resp.StatusCode)
	}
	if ct := resp.Header.Get("Content-Type"); ct != "multipart/byteranges; boundary="+MultipartBoundary {
		t.Fatalf("Content-Type = %q; want multipart/byteranges; boundary=%s", ct, MultipartBoundary)
	}
	if ar := resp.Header.Get("Accept-Ranges"); ar != "bytes" {
		t.Fatalf("Accept-Ranges = %q; want bytes", ar)
	}

	parts := readMultipartResponse(t, resp)
	if len(parts) != 3 {
		t.Fatalf("got %d parts; want 3", len(parts))
	}
	for i, r := range ranges {
		wantCR := fmt.Sprintf("bytes %d-%d/%d", r.Start, r.End, total)
		if parts[i].ContentRange != wantCR {
			t.Fatalf("part[%d] Content-Range = %q; want %q", i, parts[i].ContentRange, wantCR)
		}
		wantBody := src[r.Start : r.End+1]
		if !bytes.Equal(parts[i].Body, wantBody) {
			t.Fatalf("part[%d] body mismatch (first diff at %d)",
				i, firstDiff(parts[i].Body, wantBody))
		}
	}
}

// TestMultipartRanges_ContentLengthMatches — the precomputed Content-Length
// header MUST exactly match the actual body byte count. A drift signals
// off-by-CRLF arithmetic in the precompute loop vs what mime/multipart.Writer
// actually writes. This test is a future-proofing tripwire: if someone
// changes the per-part headers (e.g. drops Content-Type), they'll trip this.
func TestMultipartRanges_ContentLengthMatches(t *testing.T) {
	t.Parallel()
	total := int64(4096)
	src := deterministicMultipartPayload(int(total))
	cases := [][]Range{
		{{0, 99}, {100, 199}},                     // adjacent pair
		{{0, 0}},                                  // single zero-byte range (1 byte in)
		{{0, 0}, {10, 19}, {100, 199}, {500, 999}},// mixed sizes, includes 1-byte first
		{{4000, 4095}},                            // end-of-object
	}
	for i, ranges := range cases {
		ranges := ranges
		t.Run(fmt.Sprintf("case%d", i), func(t *testing.T) {
			t.Parallel()
			rec := httptest.NewRecorder()
			if err := writeMultipartRanges(rec, ranges, total, bytes.NewReader(src)); err != nil {
				t.Fatalf("writeMultipartRanges: %v", err)
			}
			resp := rec.Result()
			defer resp.Body.Close()
			declared, err := strconv.ParseInt(resp.Header.Get("Content-Length"), 10, 64)
			if err != nil {
				t.Fatalf("parse Content-Length: %v", err)
			}
			body, err := io.ReadAll(resp.Body)
			if err != nil {
				t.Fatalf("read body: %v", err)
			}
			if int64(len(body)) != declared {
				t.Fatalf("Content-Length=%d; actual body=%d (drift! precompute vs writer)",
					declared, len(body))
			}
		})
	}
}

// TestMultipartRanges_NotConcatenated is the GOTCHA #3 ANTI-REGRESSION guard.
// A raw-concatenated body would contain the bytes of both ranges back-to-back
// and NO boundary markers. This test asserts the presence of the boundary
// literal AND the closing delimiter in the serialized body.
//
// If this test ever fails, STOP THE LINE — grant story-breaking defect.
func TestMultipartRanges_NotConcatenated(t *testing.T) {
	t.Parallel()
	total := int64(100)
	src := deterministicMultipartPayload(int(total))
	ranges := []Range{{0, 9}, {10, 19}}

	rec := httptest.NewRecorder()
	if err := writeMultipartRanges(rec, ranges, total, bytes.NewReader(src)); err != nil {
		t.Fatalf("writeMultipartRanges: %v", err)
	}
	body, _ := io.ReadAll(rec.Result().Body)
	s := string(body)

	if !strings.Contains(s, "--"+MultipartBoundary) {
		t.Fatalf("body missing boundary marker %q — concatenated body? GOTCHA #3!",
			"--"+MultipartBoundary)
	}
	if !strings.Contains(s, "--"+MultipartBoundary+"--") {
		t.Fatalf("body missing closing boundary %q — truncated framing",
			"--"+MultipartBoundary+"--")
	}
	if !strings.Contains(s, "Content-Range: bytes 0-9/100") {
		t.Fatalf("body missing first Content-Range header (parts not properly framed)")
	}
	if !strings.Contains(s, "Content-Range: bytes 10-19/100") {
		t.Fatalf("body missing second Content-Range header (parts not properly framed)")
	}
}

// TestMultipartRanges_Adjacent — two ranges that abut with no gap. Per the
// documented "no coalescing" policy, they must remain two distinct parts with
// separate Content-Range headers. If a future refactor mergily merges them,
// this test catches it.
func TestMultipartRanges_Adjacent(t *testing.T) {
	t.Parallel()
	total := int64(1000)
	src := deterministicMultipartPayload(int(total))
	ranges := []Range{{0, 99}, {100, 199}}

	rec := httptest.NewRecorder()
	if err := writeMultipartRanges(rec, ranges, total, bytes.NewReader(src)); err != nil {
		t.Fatalf("writeMultipartRanges: %v", err)
	}
	resp := rec.Result()
	defer resp.Body.Close()

	parts := readMultipartResponse(t, resp)
	if len(parts) != 2 {
		t.Fatalf("got %d parts; want 2 (no coalescing!)", len(parts))
	}
	if parts[0].ContentRange != "bytes 0-99/1000" {
		t.Fatalf("part[0] Content-Range = %q", parts[0].ContentRange)
	}
	if parts[1].ContentRange != "bytes 100-199/1000" {
		t.Fatalf("part[1] Content-Range = %q", parts[1].ContentRange)
	}
	if !bytes.Equal(parts[0].Body, src[0:100]) || !bytes.Equal(parts[1].Body, src[100:200]) {
		t.Fatalf("adjacent-part body mismatch")
	}
}

// TestMultipartRanges_ReverseOrder — the server preserves caller-supplied
// order. Xet-core's parser sorts on its end (multipart.rs line 71), so the
// wire order is observability-only from the client's perspective. But we
// document + test that the server DOES preserve order for predictable curl
// debugging and byte-identical response semantics.
func TestMultipartRanges_ReverseOrder(t *testing.T) {
	t.Parallel()
	total := int64(1000)
	src := deterministicMultipartPayload(int(total))
	// Reverse order: high range first, then low.
	ranges := []Range{{500, 599}, {0, 99}}

	rec := httptest.NewRecorder()
	if err := writeMultipartRanges(rec, ranges, total, bytes.NewReader(src)); err != nil {
		t.Fatalf("writeMultipartRanges: %v", err)
	}
	resp := rec.Result()
	defer resp.Body.Close()

	parts := readMultipartResponse(t, resp)
	if len(parts) != 2 {
		t.Fatalf("got %d parts; want 2", len(parts))
	}
	// Order on the wire == order supplied.
	if parts[0].ContentRange != "bytes 500-599/1000" {
		t.Fatalf("part[0] Content-Range = %q; want bytes 500-599/1000 (order not preserved)", parts[0].ContentRange)
	}
	if parts[1].ContentRange != "bytes 0-99/1000" {
		t.Fatalf("part[1] Content-Range = %q; want bytes 0-99/1000", parts[1].ContentRange)
	}
	if !bytes.Equal(parts[0].Body, src[500:600]) {
		t.Fatalf("part[0] (reverse-order) body mismatch")
	}
	if !bytes.Equal(parts[1].Body, src[0:100]) {
		t.Fatalf("part[1] body mismatch")
	}
}

// TestMultipartRanges_ZeroByteFirstPart — a 1-byte range (the `bytes=0-0`
// canonical) as the first part, followed by a normal 10-byte range. Catches
// off-by-one in the precompute loop for the single-byte part as well as any
// mis-handling of very small stream copies.
func TestMultipartRanges_ZeroByteFirstPart(t *testing.T) {
	t.Parallel()
	total := int64(100)
	src := deterministicMultipartPayload(int(total))
	ranges := []Range{{0, 0}, {10, 19}}

	rec := httptest.NewRecorder()
	if err := writeMultipartRanges(rec, ranges, total, bytes.NewReader(src)); err != nil {
		t.Fatalf("writeMultipartRanges: %v", err)
	}
	resp := rec.Result()
	defer resp.Body.Close()

	parts := readMultipartResponse(t, resp)
	if len(parts) != 2 {
		t.Fatalf("got %d parts; want 2", len(parts))
	}
	if parts[0].ContentRange != "bytes 0-0/100" {
		t.Fatalf("part[0] Content-Range = %q", parts[0].ContentRange)
	}
	if len(parts[0].Body) != 1 || parts[0].Body[0] != src[0] {
		t.Fatalf("part[0] body = %x; want %x (single-byte part)", parts[0].Body, src[0:1])
	}
	if parts[1].ContentRange != "bytes 10-19/100" {
		t.Fatalf("part[1] Content-Range = %q", parts[1].ContentRange)
	}
	if !bytes.Equal(parts[1].Body, src[10:20]) {
		t.Fatalf("part[1] body mismatch")
	}
}

// TestMultipartRanges_ManyRanges — 32 disjoint ranges. Stress-tests the
// per-part framing loop, ensures no accidental quadratic allocation, and
// catches any off-by-one in multi-part Content-Length precompute.
func TestMultipartRanges_ManyRanges(t *testing.T) {
	t.Parallel()
	total := int64(100_000)
	src := deterministicMultipartPayload(int(total))
	ranges := make([]Range, 32)
	for i := range ranges {
		// Non-overlapping, spaced 500 bytes apart; each part is 100 bytes.
		ranges[i] = Range{Start: uint64(i * 500), End: uint64(i*500 + 99)}
	}

	rec := httptest.NewRecorder()
	if err := writeMultipartRanges(rec, ranges, total, bytes.NewReader(src)); err != nil {
		t.Fatalf("writeMultipartRanges: %v", err)
	}
	resp := rec.Result()
	defer resp.Body.Close()

	// Content-Length pre-declaration must match body bytes.
	declared, err := strconv.ParseInt(resp.Header.Get("Content-Length"), 10, 64)
	if err != nil {
		t.Fatalf("parse Content-Length: %v", err)
	}
	body, _ := io.ReadAll(resp.Body)
	if int64(len(body)) != declared {
		t.Fatalf("Content-Length=%d actual=%d (drift with 32 parts)", declared, len(body))
	}
	// Re-parse and assert all 32 parts.
	// Re-construct a fresh response reader from the buffered body bytes so the
	// parser sees the same payload we just validated.
	resp2 := &http.Response{
		StatusCode: resp.StatusCode,
		Header:     resp.Header,
		Body:       io.NopCloser(bytes.NewReader(body)),
	}
	parts := readMultipartResponse(t, resp2)
	if len(parts) != 32 {
		t.Fatalf("got %d parts; want 32", len(parts))
	}
	for i, r := range ranges {
		if parts[i].ContentRange != fmt.Sprintf("bytes %d-%d/%d", r.Start, r.End, total) {
			t.Fatalf("part[%d] CR = %q", i, parts[i].ContentRange)
		}
		if !bytes.Equal(parts[i].Body, src[r.Start:r.End+1]) {
			t.Fatalf("part[%d] body mismatch", i)
		}
	}
}

// TestMultipartRanges_ClosingDelimiter — asserts the RFC-mandated closing
// `--<b>--` delimiter is present AND is the LAST non-CRLF token of the body.
// If mw.Close() were ever accidentally removed (or replaced with a plain
// boundary instead of the closing delimiter), xet-core's parser wouldn't
// know the stream ended and would read past the intended boundary.
func TestMultipartRanges_ClosingDelimiter(t *testing.T) {
	t.Parallel()
	total := int64(50)
	src := deterministicMultipartPayload(int(total))
	ranges := []Range{{0, 9}, {20, 29}}

	rec := httptest.NewRecorder()
	if err := writeMultipartRanges(rec, ranges, total, bytes.NewReader(src)); err != nil {
		t.Fatalf("writeMultipartRanges: %v", err)
	}
	body, _ := io.ReadAll(rec.Result().Body)
	tail := "--" + MultipartBoundary + "--\r\n"
	if !strings.HasSuffix(string(body), tail) {
		t.Fatalf("body does not end with closing delimiter %q; got tail: %q",
			tail, string(body[maxInt(0, len(body)-40):]))
	}
}

// maxInt is a tiny helper — Go 1.21+ has `max` builtin but keeping the test
// file dependency-free on std generics minimizes compile dep graph.
func maxInt(a, b int) int {
	if a > b {
		return a
	}
	return b
}
