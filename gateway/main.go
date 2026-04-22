// Package main — siahub-gateway entry point.
//
// Wave 1 responsibilities:
//   - Load config (env; fatal on missing `GATEWAY_URL_SIGNING_KEY`).
//   - Build chi router + request-id middleware.
//   - Construct `UrlVerifier` at boot (fail fast on bad key material).
//   - Mount `GET /health` (live) + `GET /xorb/{hash}` (501 stub until 03-03+).
//   - Serve on two listeners:
//       * Public: `GATEWAY_ADDR` (default `:8081`), HTTP/2 enabled with
//         `MaxConcurrentStreams: 256` (D-32 / P26).
//       * Loopback: `GATEWAY_METRICS_ADDR` (default `127.0.0.1:9100`),
//         `/metrics` only (D-33 — Caddy MUST NOT proxy).
//   - Graceful shutdown on SIGTERM/SIGINT with 30s drain window.
//
// Deliberately NO wall-clock request timeout on `GET /xorb/...` — large xorbs
// + slow clients are legitimate (D-31). We rely on client disconnect (TCP RST)
// propagating via `r.Context()` to cancel the Sia SDK download, and on
// `IdleTimeout: 120s` + `ReadHeaderTimeout: 10s` to shed dead connections.
package main

import (
	"context"
	"errors"
	"log/slog"
	"net/http"
	"os"
	"os/signal"
	"syscall"
	"time"

	"github.com/go-chi/chi/v5"
	"golang.org/x/net/http2"
)

func main() {
	logger := slog.New(slog.NewJSONHandler(os.Stdout, &slog.HandlerOptions{Level: slog.LevelInfo}))
	slog.SetDefault(logger)

	cfg, err := LoadConfig()
	if err != nil {
		logger.Error("config load failed", "err", err.Error())
		os.Exit(1)
	}

	verifier, err := NewUrlVerifier(cfg.URLSigningKey, cfg.URLSigningKeyPrev, cfg.URLTTLSecs)
	if err != nil {
		logger.Error("verifier init failed", "err", err.Error())
		os.Exit(1)
	}

	metrics := NewMetrics()

	// Postgres + Sia are optional at boot: main.go stays bootable in dev
	// environments without Postgres/Sia running, and /health keeps responding.
	// The /xorb handler returns 500 with a typed error when its deps are nil;
	// operator-visible, not a panic.
	var db *DB
	if cfg.PostgresURL != "" {
		ctx, cancel := context.WithTimeout(context.Background(), 5*time.Second)
		defer cancel()
		dbInit, err := NewDB(ctx, cfg.PostgresURL)
		if err != nil {
			logger.Warn("postgres init failed; /xorb will return 500", "err", err.Error())
		} else {
			db = dbInit
		}
	}

	var sia *SiaAdapter
	if cfg.SiaIndexerURL != "" && cfg.AppID != "" && cfg.AppKey != "" {
		siaInit, err := NewSiaAdapter(cfg, metrics.Registry)
		if err != nil {
			logger.Warn("sia adapter init failed; /xorb will return 500", "err", err.Error())
		} else {
			sia = siaInit
		}
	}

	var meter *Meter
	if db != nil {
		meter = NewMeter(db, metrics.Registry)
	}

	// Whole-xorb disk LRU + cache-miss coalescer (plan 03-05). The cache
	// registers its counters on the shared Prometheus registry via
	// `registerCacheMetrics(reg)` which NewMetrics already invoked; the Cache
	// and MissCoalescer only need construction here, not metric wiring. With
	// both set non-nil the handler's cache-first `getXorbSource` branch is
	// live; nil (dev mode with CacheDir="") falls through to
	// `getXorbSourceDirect`.
	var (
		cache *Cache
		miss  *MissCoalescer
	)
	if cfg.CacheDir != "" && cfg.CacheSizeBytes > 0 {
		cacheInit, err := NewCache(cfg.CacheDir, cfg.CacheSizeBytes)
		if err != nil {
			logger.Warn("cache init failed; /xorb will fetch Sia on every request", "err", err.Error())
		} else {
			cache = cacheInit
			miss = NewMissCoalescer()
			logger.Info("cache wired", "dir", cfg.CacheDir, "size_bytes", cfg.CacheSizeBytes)
		}
	} else {
		logger.Warn("cache not wired (CacheDir or CacheSizeBytes zero); /xorb will fetch Sia on every request")
	}

	h := &Handlers{
		Metrics:  metrics,
		Verifier: verifier,
		DB:       dbOrNil(db),
		Sia:      siaOrNil(sia),
		Meter:    meterOrNil(meter),
		Cache:    cache,
		Miss:     miss,
	}

	// Public router: /health + real /xorb/{hash}.
	r := chi.NewRouter()
	r.Use(RequestID)
	r.Get("/health", h.HealthHandler)
	r.Get("/xorb/{hash}", h.ServeXorb)

	publicSrv := &http.Server{
		Addr:              cfg.Addr,
		Handler:           r,
		IdleTimeout:       120 * time.Second, // D-31
		ReadHeaderTimeout: 10 * time.Second,  // D-31 (slowloris floor)
	}
	// HTTP/2 with explicit MaxConcurrentStreams (D-32 / P26). `ConfigureServer`
	// upgrades the TLS listener transparently; for h2c (plaintext HTTP/2) the
	// Caddy reverse proxy terminates TLS so only http/1.1 is spoken on this
	// listener in prod. The explicit setting documents the ceiling.
	if err := http2.ConfigureServer(publicSrv, &http2.Server{
		MaxConcurrentStreams: 256,
	}); err != nil {
		logger.Error("http2 configure failed", "err", err.Error())
		os.Exit(1)
	}

	// Loopback metrics router (D-33). Separate listener + handler so Caddy's
	// public vhost has no path onto `/metrics`.
	metricsMux := http.NewServeMux()
	metricsMux.Handle("/metrics", metrics.Handler())
	metricsSrv := &http.Server{
		Addr:              cfg.MetricsAddr,
		Handler:           metricsMux,
		ReadHeaderTimeout: 10 * time.Second,
	}

	// Run both servers in goroutines; propagate ErrServerClosed as non-error.
	errCh := make(chan error, 2)
	go func() {
		logger.Info("gateway listening", "addr", cfg.Addr, "h2_streams", 256)
		if err := publicSrv.ListenAndServe(); err != nil && !errors.Is(err, http.ErrServerClosed) {
			errCh <- err
		}
	}()
	go func() {
		logger.Info("metrics listening", "addr", cfg.MetricsAddr)
		if err := metricsSrv.ListenAndServe(); err != nil && !errors.Is(err, http.ErrServerClosed) {
			errCh <- err
		}
	}()

	// Graceful shutdown on SIGTERM / SIGINT.
	sigCh := make(chan os.Signal, 1)
	signal.Notify(sigCh, syscall.SIGINT, syscall.SIGTERM)

	select {
	case sig := <-sigCh:
		logger.Info("shutdown signal received", "signal", sig.String())
	case err := <-errCh:
		logger.Error("listener crashed", "err", err.Error())
	}

	shutdownCtx, cancel := context.WithTimeout(context.Background(), 30*time.Second)
	defer cancel()
	if err := publicSrv.Shutdown(shutdownCtx); err != nil {
		logger.Error("public shutdown", "err", err.Error())
	}
	if err := metricsSrv.Shutdown(shutdownCtx); err != nil {
		logger.Error("metrics shutdown", "err", err.Error())
	}
	if sia != nil {
		_ = sia.Close()
	}
	if db != nil {
		db.Close()
	}
	logger.Info("gateway stopped cleanly")
}

// dbOrNil returns nil (interface-typed) when db is a nil pointer so the
// Handlers.DB XorbLookup field's nil-check in the handler catches it cleanly.
// Without this helper, assigning a typed-nil to an interface produces a
// non-nil interface value, defeating the `h.DB == nil` guard in ServeXorb.
func dbOrNil(d *DB) XorbLookup {
	if d == nil {
		return nil
	}
	return d
}

// siaOrNil — same idiom as dbOrNil. `(*SiaAdapter)(nil)` would fail the
// `h.Sia == nil` check in ServeXorb if assigned directly.
func siaOrNil(s *SiaAdapter) SiaDownloader {
	if s == nil {
		return nil
	}
	return s
}

// meterOrNil — same idiom. A nil Meter is legal (metering is best-effort);
// the handler does `if h.Meter != nil` before calling LogDownload.
func meterOrNil(m *Meter) UsageWriter {
	if m == nil {
		return nil
	}
	return m
}
