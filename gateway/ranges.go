// Package main — ranges.go.
// RFC 7233 §2.1 `Range:` header parser (byte-ranges) + single-range `206
// Partial Content` response writer.
// SCOPE: this file owns the single-range half. extends
// `writeMultipartRanges` for the multi-spec branch. We deliberately keep
// `ParseRange` multi-spec-tolerant at the parser layer — it returns the full
// slice — so only has to add a new writer, not touch the parser again.
// The load-bearing nuance addressed here is the HTTP-end-inclusive vs
// SDK-offset+length boundary ( § SC-1). An HTTP byte-range
// `bytes=S-E` denotes the CLOSED interval [S, E], i.e. E - S + 1 bytes. The
// siastorage SDK — and every `io.SectionReader` Go call we make downstream
// expects (offset, length). The canonical conversion is:
//	offset = S
//	length = E - S + 1
// Getting this wrong by one byte corrupts downloads silently (the last byte
// of every range disappears or an extra byte appears at the boundary).
// `writeSingleRange` computes the length once, comments the formula, and is
// the SINGLE place in the codebase that maps HTTP end-inclusive to SDK
// offset+length.
// Consumer contract for `ParseRange` errors (handler must map):
//	ErrBadRange -> HTTP 400
//	ErrUnsatisfiable -> HTTP 416 + `Content-Range: bytes */<size>` (RFC 7233 §4.4)
package main

import (
	"errors"
	"fmt"
	"io"
	"mime/multipart"
	"net/http"
	"net/textproto"
	"strconv"
	"strings"
)

// MultipartBoundary is the stable RFC 7233 §4.1 boundary string used for all
// `multipart/byteranges` responses emitted by this gateway.
// A5 PROBE VERDICT (04-2026, see
// 03-04-multi-range-SUMMARY.md): xet-core's
// `xet_client/src/cas_client/multipart.rs::parse_multipart_byteranges` extracts
// the boundary from the `Content-Type` header at parse time (via
// `extract_boundary`) and does NOT pin any specific string. Any RFC-7233-
// compliant boundary is accepted. We pin a literal for reproducibility + easy
// log grep per § SC-2 and per RECEIVED §C invariant 10
// (stable boundary, NOT a per-request random value).
// One notable parser quirk: `parse_multipart_byteranges` calls
// `parts.sort_by_key(|p| p.range.start)` — parts are re-ordered by the
// CLIENT before being handed to xet-core's reconstructor. That means we do
// NOT need to preserve client-supplied order on the wire; however we
// deliberately DO preserve order (see `writeMultipartRanges` below) because
// (1) it's the cheapest implementation, (2) it matches every other
// multipart/byteranges server (Nginx, Go net/http, S3), and (3) ordered
// output makes the anti-concatenation test trivially diagnose-able.
const MultipartBoundary = "xet_multipart_boundary"

// Range is an inclusive HTTP byte-range: [Start, End]. Length of the range in
// bytes is `End - Start + 1`.
// the `[2]uint64` form used by the signed-URL verifier (`VerifyOk.Range`)
// carries the SAME semantics — index 0 is start, index 1 is end-inclusive.
// Helpers in this file convert between the two at the handler boundary.
type Range struct {
	Start, End uint64
}

// Length returns the number of bytes the range covers (end-inclusive).
func (r Range) Length() uint64 { return r.End - r.Start + 1 }

// Sentinel errors exported for the handler layer to distinguish 400 vs 416.
// Keeping them package-level (not method-wrapped) matches the taxonomy Rust
// uses on the CAS side, so metric label strings stay symmetric.
var (
	// ErrBadRange — the header value does not match the RFC 7233 grammar or
	// contains an internally-contradictory spec (e.g. `10-5`, `-0`, empty).
	// Handler maps to 400.
	ErrBadRange = errors.New("malformed Range header")

	// ErrUnsatisfiable — the header PARSES cleanly but requests bytes that do
	// not exist (e.g. `bytes=100-200` on a 100-byte object). Handler maps to
	// 416 and emits `Content-Range: bytes */<size>`.
	ErrUnsatisfiable = errors.New("range not satisfiable")
)

// ParseRange parses an RFC 7233 `Range:` header value against an object of
// `size` bytes and returns the resolved closed intervals. Returns (nil,
// ErrBadRange) if the header is malformed, (nil, ErrUnsatisfiable) if any
// spec addresses bytes outside the object.
// Accepted forms per RFC 7233 §2.1:
//	bytes=<start>-<end> byte-range-spec (both bounds given)
//	bytes=<start>- byte-range-spec (start..size-1)
//	bytes=-<suffix> suffix-byte-range-spec (last <suffix> bytes)
// Multi-spec (comma-separated) is parsed and returned verbatim so
// can write `multipart/byteranges`. The single-range writer in this file
// rejects inputs with len(ranges) != 1.
// Strictness calls we make here (hardens against malformed-accept bugs):
// - `bytes=` with no specs → ErrBadRange.
// - Leading/trailing commas (`bytes=,0-9`, `bytes=0-9,`) → ErrBadRange.
// - A spec missing BOTH bounds (`bytes=-`) → ErrBadRange.
// - A suffix of zero (`bytes=-0`) → ErrBadRange. RFC 7233 leaves this one
// to server discretion; a zero-suffix is nonsensical (no bytes requested)
// and real clients never send it, so we reject rather than silently
// emit a zero-length 206.
// - `start > end` on a fully-specified spec → ErrBadRange (NOT
// Unsatisfiable — the header itself is malformed, see RFC 7233 §2.1's
// grammar: `last-byte-pos >= first-byte-pos` is required).
// - `start >= size` on open/full forms → ErrUnsatisfiable (can't satisfy
// the request against this object even though the header is well-formed).
// - A suffix LARGER than the object is clamped to the object length,
// matching the RFC: "[suffix] is greater than or equal to the current
// length... the current length MUST be used as the current value of
// last-byte-pos."
// Zero-byte ranges `0-0` / `N-N` are LEGAL (they parse to a 1-byte range
// a single byte at offset N). These are well-formed RFC 7233 inputs and
// conformance fixture `v1_zero_byte_range` depends on it.
// the signed-URL verifier's `r=S-E` query param carries an independent
// bound that the handler enforces AFTER parsing. `ParseRange` does not know
// about the signed-URL range; the handler calls `allRangesWithin(..., vokRange)`
// to reject cross-bound escalations.
func ParseRange(header string, size int64) ([]Range, error) {
	if size < 0 {
		// Defensive: the DB layer returns int64, but sizes are never negative.
		// A negative size here is a programmer bug, not a client bug.
		return nil, fmt.Errorf("ParseRange: negative size %d", size)
	}
	if !strings.HasPrefix(header, "bytes=") {
		return nil, ErrBadRange
	}
	body := header[len("bytes="):]
	if body == "" {
		return nil, ErrBadRange
	}
	specs := strings.Split(body, ",")
	out := make([]Range, 0, len(specs))
	for _, raw := range specs {
		s := strings.TrimSpace(raw)
		if s == "" {
			// Catches leading/trailing/empty-middle commas: `,0-9`, `0-9,`,
			// `0-9,10-19`.
			return nil, ErrBadRange
		}
		dash := strings.IndexByte(s, '-')
		if dash < 0 {
			return nil, ErrBadRange
		}
		startStr := s[:dash]
		endStr := s[dash+1:]

		var start, end uint64
		switch {
		case startStr == "" && endStr == "":
			// Bare `-` is not a valid spec in any RFC 7233 form.
			return nil, ErrBadRange

		case startStr == "":
			// Suffix form: `-N` — last N bytes.
			suffix, err := strconv.ParseUint(endStr, 10, 64)
			if err != nil {
				return nil, ErrBadRange
			}
			if suffix == 0 {
				// Zero-suffix = "last 0 bytes" = nonsense. Reject (see header).
				return nil, ErrBadRange
			}
			if size == 0 {
				// Can't satisfy any suffix on a zero-size object.
				return nil, ErrUnsatisfiable
			}
			usize := uint64(size)
			if suffix > usize {
				// RFC §2.1: clamp to full object.
				suffix = usize
			}
			start = usize - suffix
			end = usize - 1

		case endStr == "":
			// Open form: `N-` — N..size-1.
			v, err := strconv.ParseUint(startStr, 10, 64)
			if err != nil {
				return nil, ErrBadRange
			}
			if size == 0 || v >= uint64(size) {
				return nil, ErrUnsatisfiable
			}
			start = v
			end = uint64(size) - 1

		default:
			// Fully specified: `S-E` with both bounds.
			s1, err1 := strconv.ParseUint(startStr, 10, 64)
			s2, err2 := strconv.ParseUint(endStr, 10, 64)
			if err1 != nil || err2 != nil {
				return nil, ErrBadRange
			}
			if s1 > s2 {
				// RFC §2.1 grammar forbids first-byte-pos > last-byte-pos.
				return nil, ErrBadRange
			}
			if size == 0 || s2 >= uint64(size) {
				return nil, ErrUnsatisfiable
			}
			start = s1
			end = s2
		}
		out = append(out, Range{Start: start, End: end})
	}
	if len(out) == 0 {
		// Defensive — the empty-body check above should catch this, but leave
		// the guard so future refactors don't silently regress.
		return nil, ErrBadRange
	}
	return out, nil
}

// writeSingleRange emits a `206 Partial Content` response for exactly ONE
// range against `src` (which must implement `io.ReaderAt` — a temp file or a
// backing cache file qualifies; an in-memory `bytes.Reader` via wrapping also
// works for tests).
// Headers written (in order):
//	Content-Type: application/octet-stream
//	Content-Range: bytes <Start>-<End>/<totalSize> (RFC 7233 §4.2)
//	Content-Length: <End - Start + 1>
//	Accept-Ranges: bytes
// Body: exactly `length = End - Start + 1` bytes, streamed via
// `io.NewSectionReader` → `io.Copy`. No whole-object buffering — is
// preserved because `SectionReader` reads from `src` in chunks sized by Go's
// copy buffer (32 KiB default) and `io.Copy` pumps without ever holding the
// whole range in RAM.
// OFF-BY-ONE HOTSPOT — this is where HTTP-end-inclusive converts to the
// SDK-style offset+length pair. Read the arithmetic carefully:
//	HTTP: bytes 100-199 (200 bytes, indices 100..199 inclusive)
//	length = 199 - 100 + 1 = 100 ← classic off-by-one trap
//	offset = 100 ← matches SectionReader(src, 100, 100)
// Test `TestWriteSingleRange_BoundaryOffByOne` asserts the exact `100-199`
// case to keep a regression tripwire on this function.
func writeSingleRange(w http.ResponseWriter, r Range, totalSize int64, src io.ReaderAt) error {
	length := int64(r.End-r.Start) + 1 // end-inclusive → length; see function doc
	w.Header().Set("Content-Type", "application/octet-stream")
	w.Header().Set("Content-Range", fmt.Sprintf("bytes %d-%d/%d", r.Start, r.End, totalSize))
	w.Header().Set("Content-Length", strconv.FormatInt(length, 10))
	w.Header().Set("Accept-Ranges", "bytes")
	w.WriteHeader(http.StatusPartialContent)
	sr := io.NewSectionReader(src, int64(r.Start), length)
	_, err := io.Copy(w, sr)
	return err
}

// allRangesWithin checks every r ∈ ranges satisfies `bound.Start <= r.Start`
// AND `r.End <= bound.End`. Used by the handler to reject Range headers that
// escape the signed-URL's `r=`.
func allRangesWithin(ranges []Range, bound Range) bool {
	for _, r := range ranges {
		if r.Start < bound.Start || r.End > bound.End {
			return false
		}
	}
	return true
}

// multipartPartContentType is the per-part `Content-Type` value. Xet-core's
// reconstructor treats every CAS byte source as opaque bytes; the part-level
// Content-Type is informational only. `application/octet-stream` mirrors the
// top-level object Content-Type emitted by `writeSingleRange`, keeping the
// two code paths symmetric for any future tooling that greps request logs.
const multipartPartContentType = "application/octet-stream"

// writeMultipartRanges emits a `206 Partial Content` response in RFC 7233 §4.1
// `multipart/byteranges` framing for MULTIPLE requested byte ranges. This is
// THE single most integrity-critical function in —.md
// (and RECEIVED §C invariant 3) call out that a concatenated body would
// silently corrupt xet-core's V2 reconstruction downloads.
// Framing (per RFC 7233 §4.1; mirrored from xet-core's parser expectations in
// `.cache/refs/xet-core/xet_client/src/cas_client/multipart.rs`):
//	--<boundary>\r\n
//	Content-Type: application/octet-stream\r\n
//	Content-Range: bytes S-E/total\r\n
//	\r\n
//	<raw body bytes of the range>\r\n
//	--<boundary>\r\n (next part — same shape)
//...
//	--<boundary>--\r\n (closing delimiter; terminates body)
// IMPLEMENTATION CHOICE: we use Go's stdlib `mime/multipart.Writer` rather than
// hand-rolling CRLF + boundary framing. Rationale: hand-rolled framing is
// exactly the class of off-by-CRLF bug that warns about. The stdlib
// writer has been battle-tested by every Go HTTP client on the planet, tracks
// RFC 7233 precisely, and emits `--<boundary>--\r\n` on `.Close`. The
// tradeoff is that Content-Length must be PRECOMPUTED before writing the
// headers (the stdlib writer cannot compute framing length a-priori without
// actually writing to a buffer). Precomputation is cheap (fixed header size
// per part + known range lengths) and mirrors what Nginx/Go-stdlib do by
// default for known-size multipart bodies.
// Content-Length math (per-part):
// - Opening boundary line: len("--" + boundary + "\r\n")
// - "Content-Type: <ct>\r\n"
// - "Content-Range: bytes S-E/total\r\n"
// - Header terminator: "\r\n"
// - Body bytes: End - Start + 1 (end-inclusive)
// - Trailing CRLF before next boundary (mime/multipart writes this as the
// leading "\r\n--<b>" for each subsequent part, and before the closing
// "--<b>--\r\n"). Accounted for ONCE per part below.
// Closing: "--" + boundary + "--\r\n"
// (no whole-object buffering): each part body is streamed via
// `io.NewSectionReader(src, start, length)` → `io.Copy(partWriter, sr)`. The
// copy buffer is Go's default 32 KiB — no part is ever held in RAM in its
// entirety. This matches the single-range branch's streaming guarantee.
// Ordering: parts are emitted in the order supplied by the caller. xet-core's
// parser SORTS by start offset after parse (see multipart.rs line 71:
// `parts.sort_by_key(|p| p.range.start)`), so our output order does not affect
// reconstruction. We preserve caller order anyway for (a) predictable curl
// debugging and (b) byte-identical responses across requests with the same
// `Range:` header, which is a quality-of-test-suite property even if xet-core
// wouldn't notice a reshuffle.
// Overlapping ranges: the server does NOT coalesce or de-duplicate overlapping
// ranges. This is a deliberate tradeoff — RFC 7233 §4.1 says a server "MAY"
// coalesce "for efficiency," but xet-core never sends overlapping ranges in
// its V2 reconstruction calls (file-term ranges are always disjoint file
// segments). Implementing coalescing would (a) add state + complexity to an
// integrity-critical path, (b) mean the response parts no longer 1:1 the
// caller's request slice, making the handler's `bytesServed` accounting more
// intricate. So: overlap/duplicates are served verbatim. If a client sends
// `bytes=0-99,50-149` they get two parts, with byte range appearing in
// BOTH. Document-and-move-on.
// Caller preconditions (unchecked here; ServeXorb already enforces them):
// - len(ranges) >= 1
// - Every r in ranges is satisfiable against totalSize (End < totalSize)
// - src is a valid ReaderAt with at least totalSize bytes
// The function returns an error from `io.Copy` (i.e. src read failure, client
// disconnect mid-write). Headers + WriteHeader have already been flushed by
// the time a copy error can occur, so the caller CANNOT change the status
// metric labels + structured logs fall back to the best-effort handling in
// handlers.go.
func writeMultipartRanges(w http.ResponseWriter, ranges []Range, totalSize int64, src io.ReaderAt) error {
	// --- Step 1: precompute Content-Length ------------------------------------
	// We count every byte that mime/multipart.Writer will emit so the response
	// can advertise an exact Content-Length. Getting this right catches ANY
	// future framing drift (e.g. if we accidentally change the part Content-
	// Type: a drift between the precomputed length and the actual body bytes
	// written will trip the TestMultipartRanges_ContentLengthMatches assertion
	// hard.
	// mime/multipart.Writer framing (post-SetBoundary):
	// Part 1: "--<b>\r\n<headers>\r\n\r\n<body>"
	// Part N: "\r\n--<b>\r\n<headers>\r\n\r\n<body>" (leading CRLF added
	// because the Writer prepends one before each NEW boundary
	// after the first part)
	// Close: "\r\n--<b>--\r\n"
	// So: first-part overhead = len("--<b>\r\n") + headers + "\r\n" (end of
	// part headers). Subsequent parts add a leading "\r\n" before the "--<b>".
	// Closing adds "\r\n--<b>--\r\n".
	boundary := MultipartBoundary
	openDelim := "--" + boundary + "\r\n"
	closeDelim := "\r\n--" + boundary + "--\r\n"
	var contentLen int64
	for i, r := range ranges {
		// Leading CRLF before every part EXCEPT the first.
		if i > 0 {
			contentLen += 2 // "\r\n"
		}
		// Opening boundary delim: "--<b>\r\n"
		contentLen += int64(len(openDelim))
		// Part headers: Content-Type + Content-Range, each terminated by CRLF.
		ctLine := "Content-Type: " + multipartPartContentType + "\r\n"
		crLine := "Content-Range: bytes " + strconv.FormatUint(r.Start, 10) + "-" +
			strconv.FormatUint(r.End, 10) + "/" + strconv.FormatInt(totalSize, 10) + "\r\n"
		contentLen += int64(len(ctLine))
		contentLen += int64(len(crLine))
		// Header terminator.
		contentLen += 2 // "\r\n"
		// Body bytes (end-inclusive → End-Start+1).
		contentLen += int64(r.End-r.Start) + 1
	}
	contentLen += int64(len(closeDelim))

	// --- Step 2: write response headers ---------------------------------------
	w.Header().Set("Content-Type", "multipart/byteranges; boundary="+boundary)
	w.Header().Set("Content-Length", strconv.FormatInt(contentLen, 10))
	w.Header().Set("Accept-Ranges", "bytes")
	w.WriteHeader(http.StatusPartialContent)

	// --- Step 3: stream parts via mime/multipart.Writer -----------------------
	mw := multipart.NewWriter(w)
	if err := mw.SetBoundary(boundary); err != nil {
		// SetBoundary only fails if the boundary is too long (>70 chars) or
		// contains illegal characters. Our const is 22 safe chars, so this
		// branch is unreachable in practice — but surfacing an error instead
		// of panicking keeps the function pure.
		return fmt.Errorf("multipart boundary: %w", err)
	}
	for _, r := range ranges {
		partHdr := textproto.MIMEHeader{}
		partHdr.Set("Content-Type", multipartPartContentType)
		partHdr.Set("Content-Range",
			"bytes "+strconv.FormatUint(r.Start, 10)+"-"+
				strconv.FormatUint(r.End, 10)+"/"+
				strconv.FormatInt(totalSize, 10))
		pw, err := mw.CreatePart(partHdr)
		if err != nil {
			return fmt.Errorf("multipart CreatePart: %w", err)
		}
		length := int64(r.End-r.Start) + 1
		sr := io.NewSectionReader(src, int64(r.Start), length)
		if _, err := io.Copy(pw, sr); err != nil {
			// Body write error — client likely disconnected, or the ReaderAt
			// faulted. Nothing we can do about the already-sent headers; the
			// caller logs + metrics-counts. Return the error so the handler
			// can distinguish clean close from faulty source.
			return fmt.Errorf("multipart part body copy: %w", err)
		}
	}
	// Close writes the "\r\n--<b>--\r\n" closing delimiter. MUST be called
	// to terminate the body — omitting it means a truncated multipart that
	// xet-core's parser rejects (the trailing `--<b>--` is a hard RFC 7233
	// requirement, not decorative).
	if err := mw.Close(); err != nil {
		return fmt.Errorf("multipart close: %w", err)
	}
	return nil
}
