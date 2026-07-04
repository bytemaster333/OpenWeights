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
	XorbHashHex       string
	Exp               uint64
	Range             *[2]uint64 // (start, end_inclusive); nil if URL granted whole xorb
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
	// Five fields, four LFs. Matches `format!("{version}\n{hash}\n{exp}\n{r}\n{kid}")` in Rust.
	return version + "\n" + hashHex + "\n" +
		strconv.FormatUint(exp, 10) + "\n" + rStr + "\n" + kid.String()
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

	var rng *[2]uint64
	// `url.Values.Has` (Go 1.17+) distinguishes "absent" from "empty-string"
	// an empty `r=` value still counts as PRESENT and must parse to a range,
	// matching Rust `url::Url.query_pairs.any(|k| k == "r")`.
	if q.Has("r") {
		rs := q.Get("r")
		parts := strings.SplitN(rs, "-", 2)
		if len(parts) != 2 {
			return nil, &VerifyErr{Kind: "malformed:range"}
		}
		s, e1 := strconv.ParseUint(parts[0], 10, 64)
		e, e2 := strconv.ParseUint(parts[1], 10, 64)
		if e1 != nil || e2 != nil || s > e {
			return nil, &VerifyErr{Kind: "malformed:range"}
		}
		rng = &[2]uint64{s, e}
	}

	// Expiry BEFORE signature check (timing uniformity).
	if uint64(now.Unix()) >= exp {
		return nil, &VerifyErr{Kind: "expired"}
	}

	canonical := CanonicalString(CanonicalVersion, xorbHashHex, exp, rng, kid)
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
			Range:             rng,
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
				Range:             rng,
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
