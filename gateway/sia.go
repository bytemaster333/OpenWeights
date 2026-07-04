// Package main — sia.go.
//
// Sia adapter for the gateway. W2 replaces the W1 stub with a thin wrapper
// over `go.sia.tech/siastorage@v0.0.3` — specifically its SDK's range-download
// path via `WithDownloadRange(offset, length)` (verified PASS in Phase 1's
// bench/thesis/thesis.go).
//
// Contract: the gateway issues ONE `SDK.Download` call per cache miss
// (RECEIVED §C invariant 4). Singleflight in 03-05 guarantees there is only
// one cold-miss request in flight per xorb, so no retry-loop or per-attempt
// coalescing lives at this layer.
//
// GOTCHA (Phase 1 §32): `PinObject` takes `obj` BY VALUE (not pointer). The
// gateway only downloads, so this is informational — but documented here so
// any future "add pinning on miss" change does not misread the signature.
//
// GATE-10 precondition: `r.Context` propagates into `SDK.Download` via the
// `ctx` argument. When the client TCP-resets, `ctx.Done` fires and the SDK
// aborts the in-flight HTTP stream within 1s. The integration test in
// sia_test.go (build tag `integration`) exercises this.
package main

import (
	"context"
	"encoding/hex"
	"errors"
	"fmt"
	"io"
	"time"

	"github.com/prometheus/client_golang/prometheus"
	"go.sia.tech/core/types"
	"go.sia.tech/siastorage"
)

// SiaDownloader is the interface 03-05's cache miss path consumes. Wrapping
// the concrete SiaAdapter behind an interface lets unit tests substitute a
// fake without dialing Sia — see sia_test.go.
//
// siaObjectID is the 32-byte Sia `types.Hash256` returned by `SDK.Upload`
// (and later fetched by `SDK.Object`). Callers that have the hex form from
// the Postgres lookup should parse via `siaObjectIDFromHex` first.
type SiaDownloader interface {
	Download(ctx context.Context, w io.Writer, siaObjectID types.Hash256, offset, length uint64) error
	DownloadRange(ctx context.Context, w io.Writer, siaObjectID types.Hash256, offset, length uint64) error
}

// SiaAdapter owns one configured `*siastorage.SDK` instance, constructed
// ONCE at boot. The SDK is safe for concurrent use; the gateway issues many
// Download calls in parallel.
type SiaAdapter struct {
	sdk             *siastorage.SDK
	downloadDuration prometheus.Histogram
}

// NewSiaAdapter builds a Builder-initialized SDK from env-derived config.
// Required fields: `SiaIndexerURL`, `AppID` (64-char hex), `AppKey` (hex).
//
// Boot-time invariants:
// - AppID MUST parse as a `types.Hash256` (32-byte hex) — matches the bench
// thesis's `appID.UnmarshalText` path.
// - AppKey MUST decode as raw hex → `types.PrivateKey`. The siastorage
// SDK blesses PrivateKey-sized slices without further validation; the
// Zen / mainnet split is handled upstream by indexd.
//
// Wiring a nil `metrics` registry is allowed for bench / unit-test paths
// that do not want to pollute a global registerer — the histogram is no-op
// in that mode.
func NewSiaAdapter(cfg *Config, reg prometheus.Registerer) (*SiaAdapter, error) {
	if cfg == nil {
		return nil, errors.New("NewSiaAdapter: nil Config")
	}
	if cfg.SiaIndexerURL == "" {
		return nil, errors.New("NewSiaAdapter: SiaIndexerURL is empty (set OPENWEIGHTS_INDEXER_URL / SIA_INDEXER_URL)")
	}
	if cfg.AppID == "" {
		return nil, errors.New("NewSiaAdapter: AppID is empty (set OPENWEIGHTS_APP_ID)")
	}
	if cfg.AppKey == "" {
		return nil, errors.New("NewSiaAdapter: AppKey is empty (set OPENWEIGHTS_APP_KEY / APP_KEY)")
	}

	var appID types.Hash256
	if err := appID.UnmarshalText([]byte(cfg.AppID)); err != nil {
		return nil, fmt.Errorf("parse OPENWEIGHTS_APP_ID as 32-byte hex: %w", err)
	}
	keyBytes, err := hex.DecodeString(cfg.AppKey)
	if err != nil {
		return nil, fmt.Errorf("decode OPENWEIGHTS_APP_KEY hex: %w", err)
	}
	appKey := types.PrivateKey(keyBytes)

	// Builder + SDK bootstrap matches the bench/thesis call pattern. Name /
	// Description are purely cosmetic (logged by indexd for operator UX);
	// changing them does not invalidate keys (the AppID does that).
	builder := siastorage.NewBuilder(cfg.SiaIndexerURL, siastorage.AppMetadata{
		ID:          appID,
		Name:        "openweights-gateway",
		Description: "OpenWeights Xet-compatible gateway (Phase 3)",
	})
	sdk, err := builder.SDK(appKey)
	if err != nil {
		return nil, fmt.Errorf("siastorage SDK init: %w", err)
	}

	hist := prometheus.NewHistogram(prometheus.HistogramOpts{
		Name:    "gateway_sia_download_duration_seconds",
		Help:    "Duration of siastorage SDK Download calls (per cache miss). Observed regardless of success/failure.",
		Buckets: prometheus.ExponentialBuckets(0.01, 2, 14), // 10ms → ~81s
	})
	if reg != nil {
		if err := reg.Register(hist); err != nil {
			// Double-register is fine (tests may do it); any other error is fatal.
			var are prometheus.AlreadyRegisteredError
			if !errors.As(err, &are) {
				_ = sdk.Close()
				return nil, fmt.Errorf("register sia download histogram: %w", err)
			}
			if existing, ok := are.ExistingCollector.(prometheus.Histogram); ok {
				hist = existing
			}
		}
	}

	return &SiaAdapter{sdk: sdk, downloadDuration: hist}, nil
}

// Close drains the SDK. Match main.go's shutdown path: called during the
// 30s drain window after listeners stop accepting.
func (s *SiaAdapter) Close() error {
	if s == nil || s.sdk == nil {
		return nil
	}
	return s.sdk.Close()
}

// DownloadRange fetches `length` bytes starting at `offset` from the Sia
// object identified by `siaObjectID`, streaming them into `w`. This is the
// single method the gateway's cache-miss path calls (03-05).
//
// Contract:
// - offset, length in bytes. `offset=0, length=0` streams the entire object
// per siastorage SDK convention (verified against bench/thesis/thesis.go,
// where `WithDownloadRange(rangeOffset, rangeLength)` drives partial
// fetches — the zero-zero combination passes through without the option
// and fetches whole-object).
// - ctx cancellation cancels the in-flight SDK call within 1s (GATE-10).
// - ONE `SDK.Download` call per invocation. No retry loop here; the cache
// layer decides whether to retry a failed miss.
// - Observes `gateway_sia_download_duration_seconds` for every call,
// succeed or fail — operators watching cold-miss latency in Grafana see
// the full distribution, not just the happy path.
//
// Two-step call: `SDK.Object` fetches the slab metadata for the hash (small
// indexd call), then `SDK.Download` streams the actual bytes. Both inherit
// the caller's ctx; canceling at any point cancels both. This matches the
// bench thesis pattern (it calls `SDK.Upload` + `SDK.Download` directly on
// the Object value).
func (s *SiaAdapter) DownloadRange(
	ctx context.Context,
	w io.Writer,
	siaObjectID types.Hash256,
	offset, length uint64,
) error {
	if s == nil || s.sdk == nil {
		return errors.New("DownloadRange on nil SiaAdapter")
	}
	start := time.Now()
	defer func() { s.downloadDuration.Observe(time.Since(start).Seconds()) }()

	obj, err := s.sdk.Object(ctx, siaObjectID)
	if err != nil {
		return fmt.Errorf("fetch sia object metadata: %w", err)
	}

	// `WithDownloadRange(0, 0)` is ambiguous — the SDK treats a zero-length
	// range as "empty". Skip the option entirely for whole-object fetches
	// (gateway whole-xorb cache prewarm path in 03-05).
	var opts []siastorage.DownloadOption
	if length > 0 {
		opts = append(opts, siastorage.WithDownloadRange(offset, length))
	}
	if err := s.sdk.Download(ctx, w, obj, opts...); err != nil {
		return fmt.Errorf("sia download: %w", err)
	}
	return nil
}

// Download is an alias of DownloadRange kept so the plan deliverable naming
// (`Download(ctx, w, siaObjectID, offset, length)`) resolves at call sites.
// Internal code should prefer DownloadRange.
func (s *SiaAdapter) Download(
	ctx context.Context,
	w io.Writer,
	siaObjectID types.Hash256,
	offset, length uint64,
) error {
	return s.DownloadRange(ctx, w, siaObjectID, offset, length)
}

// siaObjectIDFromHex parses the 64-char hex form the Postgres `LookupXorb`
// helper returns into a `types.Hash256`. Exposed for 03-05 which may want to
// plumb the hex through log lines before decoding.
func siaObjectIDFromHex(s string) (types.Hash256, error) {
	var h types.Hash256
	if err := h.UnmarshalText([]byte(s)); err != nil {
		return types.Hash256{}, err
	}
	return h, nil
}
