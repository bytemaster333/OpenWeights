// Package main — signed_url.go.
// Go port of `cas/crates/openweights-cas-core/src/signed_url.rs::UrlSigner::verify`.
// MUST remain byte-identical to the Rust side. Drift is a story-breaking
// silent-corruption bug; enforcement lives in `signed_url_test.go` which loads
// `conformance/fixtures/signed_url_vectors.json` and asserts every vector.
// Wire contract (RECEIVED §B step 2; CONTEXT ):
//	canonical = "v1\n<xorb_hash_hex>\n<exp>\n<r_or_empty>\n<kid>"
//	sig = base64url_nopad( HMAC_SHA256(key, canonical) )
// Field order is FIXED: version, hash, exp, range, kid. The separator is
// EXACTLY one LF (0x0A) — never CRLF, never a colon.
// Consumer contract (handlers map these):
// - VerifyErr{Kind:"expired"} -> HTTP 403 (; xet-core refreshes on 403)
// - VerifyErr{Kind:"bad_signature"} -> HTTP 403 (same; must be indistinguishable from 404)
// - VerifyErr{Kind:"malformed:*"} -> HTTP 400
// Rotation: try CURRENT key first; fall through to PREV if configured.
// mints with CURRENT only, so PREV existing implies a rotation window is open.
package main

import (
	"crypto/hmac"
	"crypto/sha256"
	"crypto/subtle"
	"encoding/base64"
	"errors"
	"net/url"
	"strconv"
	"strings"
	"time"

	"github.com/google/uuid"
)

// Canonical prefix reserved for a possible future `v2` break. Load-bearing:
// any divergence from `v1` invalidates every in-flight signed URL.
const CanonicalVersion = "v1"

// SigningKeyLen is the required decoded key length in bytes. Hard invariant
// shared with the Rust side; mismatches are ALWAYS fatal at construction.
const SigningKeyLen = 32

// VerifyErr classifies verification failures for the handler layer to map to
// HTTP codes. `Kind` is a structured string (`malformed:<what>` | `expired` |
// `bad_signature`) — stable strings so metrics labels don't drift.
type VerifyErr struct {
	Kind string
}

func (e *VerifyErr) Error() string { return e.Kind }

// VerifyOk is returned on successful verification. `AcceptedByPrevKey` lets
// callers emit a rotation-observability metric or debug log.
type VerifyOk struct {
	XorbHashHex string
	Exp         uint64
	// Ranges is the byte-range grant the URL carries. Empty = whole xorb. One
	// entry = a single contiguous grant (mint_v1). Two or more = a multi-segment
	// grant (mint_v1_multi_range → `r=s1-e1,s2-e2,...`). Each entry is
	// (start, end_inclusive). The handler enforces every requested `Range:`
	// sits entirely inside one of these segments.
	Ranges            [][2]uint64
	Kid               uuid.UUID
	AcceptedByPrevKey bool
}

// UrlVerifier holds decoded HMAC key material. Construct once at boot, share
// across handler goroutines — the struct is immutable and the keys are raw
// bytes (HMAC is re-initialized per-verify which is cheap for SHA-256).
type UrlVerifier struct {
	keyCurrent []byte // 32 bytes
	keyPrev    []byte // 32 bytes or nil
	ttlSecs    int64  // mirrors config; not consulted by Verify (exp is authoritative)
}

// NewUrlVerifier decodes base64-padded keys (matches Rust B64_STD). The PREV
// key is optional; pass "" to disable rotation acceptance.
func NewUrlVerifier(currentB64, prevB64 string, ttlSecs int64) (*UrlVerifier, error) {
	cur, err := decodeKey(currentB64)
	if err != nil {
		return nil, err
	}
	var prev []byte
	if prevB64 != "" {
		prev, err = decodeKey(prevB64)
		if err != nil {
			return nil, err
		}
	}
	return &UrlVerifier{keyCurrent: cur, keyPrev: prev, ttlSecs: ttlSecs}, nil
}

// decodeKey accepts standard-base64 (with padding) and validates length.
// Error messages NEVER include key bytes (matches / T-02-08-03).
func decodeKey(b64 string) ([]byte, error) {
	b, err := base64.StdEncoding.DecodeString(b64)
	if err != nil {
		return nil, errors.New("signing key is not valid base64")
	}
	if len(b) != SigningKeyLen {
		return nil, errors.New("signing key must decode to exactly 32 bytes")
	}
	return b, nil
}

// CanonicalString rebuilds the exact HMAC input the Rust minter signed. This is
// the byte-identical target asserted by the cross-language vectors test.
// `r` nil → empty field. `r` set → "<start>-<end_inclusive>" ASCII decimals,
// single '-' (0x2D), no spaces.
func CanonicalString(version, hashHex string, exp uint64, r *[2]uint64, kid uuid.UUID) string {
	rStr := ""
	if r != nil {
		rStr = strconv.FormatUint(r[0], 10) + "-" + strconv.FormatUint(r[1], 10)
	}
	return canonicalStringRaw(version, hashHex, exp, rStr, kid)
}

// canonicalStringRaw builds the canonical string from an already-serialized `r`
// field. This is the single source of truth for the 5-field layout; both
// single-range (`s-e`) and multi-segment (`s1-e1,s2-e2,...`) grants flow through
// here. `Verify` builds `rField` from the raw querystring value so the HMAC runs
// over the exact bytes the Rust minter signed — byte-identical regardless of
// segment count. Matches `UrlSigner::canonical_string_raw` on the Rust side.
func canonicalStringRaw(version, hashHex string, exp uint64, rField string, kid uuid.UUID) string {
	// Five fields, four LFs. Matches `format!("{version}\n{hash}\n{exp}\n{r}\n{kid}")` in Rust.
	return version + "\n" + hashHex + "\n" +
		strconv.FormatUint(exp, 10) + "\n" + rField + "\n" + kid.String()
}

// Verify checks the signed URL query against the given xorb hash path segment.
// Router integration (03-03+): `xorbHashHex = chi.URLParam(r, "hash")`,
// `q = r.URL.Query`, `now = time.Now`. On VerifyErr handlers MUST respond
// 403 for `expired`/`bad_signature` and 400 for any `malformed:*` kind.
// Implementation notes:
// - Expiry is checked BEFORE signature to keep timing uniform on expired URLs
// and to avoid touching key material unnecessarily.
// - `subtle.ConstantTimeCompare` is used for every HMAC compare (T1 spoofed
// URL threat mitigation; mirrors Rust `subtle::ConstantTimeEq`).
// - Mixed-case hex in the path segment is REJECTED — `MerkleHash::hex`
// always emits lowercase, so accepting uppercase would widen the.
func (v *UrlVerifier) Verify(xorbHashHex string, q url.Values, now time.Time) (*VerifyOk, *VerifyErr) {
	// Path-segment shape: 64 lowercase hex chars. Any deviation = 400.
	if len(xorbHashHex) != 64 {
		return nil, &VerifyErr{Kind: "malformed:xorb_hash_hex"}
	}
	for i := 0; i < len(xorbHashHex); i++ {
		if !isHexLower(xorbHashHex[i]) {
			return nil, &VerifyErr{Kind: "malformed:xorb_hash_hex"}
		}
	}

	expStr := q.Get("exp")
	kidStr := q.Get("kid")
	sigStr := q.Get("sig")
	if expStr == "" {
		return nil, &VerifyErr{Kind: "malformed:exp"}
	}
	if kidStr == "" {
		return nil, &VerifyErr{Kind: "malformed:kid"}
	}
	if sigStr == "" {
		return nil, &VerifyErr{Kind: "malformed:sig"}
	}
	exp, err := strconv.ParseUint(expStr, 10, 64)
	if err != nil {
		return nil, &VerifyErr{Kind: "malformed:exp"}
	}
	kid, err := uuid.Parse(kidStr)
	if err != nil {
		return nil, &VerifyErr{Kind: "malformed:kid"}
	}

	var ranges [][2]uint64
	rField := ""
	// `url.Values.Has` (Go 1.17+) distinguishes "absent" from "empty-string" —
	// an empty `r=` value still counts as PRESENT and must parse to ≥1 segment,
	// matching Rust `url::Url.query_pairs.any(|k| k == "r")`. The raw value is
	// fed verbatim into the canonical so the HMAC matches the minter regardless
	// of how many comma-joined segments it carries.
	if q.Has("r") {
		rField = q.Get("r")
		segs, ok := parseRangeSegments(rField)
		if !ok {
			return nil, &VerifyErr{Kind: "malformed:range"}
		}
		ranges = segs
	}

	// Expiry BEFORE signature check (timing uniformity).
	if uint64(now.Unix()) >= exp {
		return nil, &VerifyErr{Kind: "expired"}
	}

	canonical := canonicalStringRaw(CanonicalVersion, xorbHashHex, exp, rField, kid)
	// base64url WITHOUT padding (matches Rust `URL_SAFE_NO_PAD`).
	provided, err := base64.RawURLEncoding.DecodeString(sigStr)
	if err != nil {
		return nil, &VerifyErr{Kind: "malformed:sig_b64"}
	}

	// Current-key path.
	curSig := hmacSHA256(v.keyCurrent, []byte(canonical))
	if subtle.ConstantTimeCompare(curSig, provided) == 1 {
		return &VerifyOk{
			XorbHashHex:       xorbHashHex,
			Exp:               exp,
			Ranges:            ranges,
			Kid:               kid,
			AcceptedByPrevKey: false,
		}, nil
	}
	// Rotation window.
	if v.keyPrev != nil {
		prevSig := hmacSHA256(v.keyPrev, []byte(canonical))
		if subtle.ConstantTimeCompare(prevSig, provided) == 1 {
			return &VerifyOk{
				XorbHashHex:       xorbHashHex,
				Exp:               exp,
				Ranges:            ranges,
				Kid:               kid,
				AcceptedByPrevKey: true,
			}, nil
		}
	}
	return nil, &VerifyErr{Kind: "bad_signature"}
}

// hmacSHA256 returns the raw 32-byte HMAC-SHA256 digest. Caller compares with
// `subtle.ConstantTimeCompare`; base64 is the URL-transport layer only.
func hmacSHA256(key, msg []byte) []byte {
	m := hmac.New(sha256.New, key)
	m.Write(msg)
	return m.Sum(nil)
}

// isHexLower enforces lowercase hex — MerkleHash::hex emits lowercase only,
// so mixed-case would silently widen the signed.
func isHexLower(b byte) bool {
	return (b >= '0' && b <= '9') || (b >= 'a' && b <= 'f')
}

// parseRangeSegments parses the `r=` field into one or more (start,
// end_inclusive) segments. Single-range URLs carry one segment (`s-e`);
// multi-range URLs carry the comma-joined form (`s1-e1,s2-e2,...`) minted by
// `mint_v1_multi_range`. Returns (nil, false) on any malformed input: empty
// string, empty segment (leading/trailing/double comma), missing bound
// (`-5` / `5-`), non-decimal, or start > end. Mirrors Rust
// `parse_range_segments`; the two MUST agree so the cross-language vector gate
// holds.
func parseRangeSegments(s string) ([][2]uint64, bool) {
	if s == "" {
		return nil, false
	}
	parts := strings.Split(s, ",")
	out := make([][2]uint64, 0, len(parts))
	for _, p := range parts {
		dash := strings.IndexByte(p, '-')
		// dash must exist with a non-empty bound on each side. dash<=0 rejects a
		// missing/empty start (incl. suffix form `-N`, never used in a grant);
		// dash==len-1 rejects a missing end (`N-`).
		if dash <= 0 || dash == len(p)-1 {
			return nil, false
		}
		start, e1 := strconv.ParseUint(p[:dash], 10, 64)
		end, e2 := strconv.ParseUint(p[dash+1:], 10, 64)
		if e1 != nil || e2 != nil || start > end {
			return nil, false
		}
		out = append(out, [2]uint64{start, end})
	}
	if len(out) == 0 {
		return nil, false
	}
	return out, true
}
