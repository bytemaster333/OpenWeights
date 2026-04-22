// Package main — siahub-gateway entry point.
//
// config.go owns environment-variable loading. Wave 1 only populates the
// fields the scaffold actually reads; later plans (03-02 Postgres + Sia adapter,
// 03-05 cache) parse additional fields and dial their dependencies.
//
// Invariant: `GATEWAY_URL_SIGNING_KEY` is the only hard-required field at boot
// (no URL verification without it). Every other field has a sane default per
// CONTEXT D-24..D-33.
package main

import (
	"errors"
	"fmt"
	"os"
	"strconv"

	"github.com/joho/godotenv"
)

// Config captures the full env surface the gateway reads. Fields are named
// exactly per KEY-DECISIONS §3 so downstream plans do not have to invent
// parallel casing.
type Config struct {
	// Public listener. Phase 3 default `:8081`; Compose publishes to
	// `127.0.0.1:9090` externally via port map.
	Addr string
	// Loopback-only metrics listener per D-33; Caddy MUST NOT proxy.
	MetricsAddr string

	// Base64-encoded 32-byte HMAC keys. `URLSigningKey` is MANDATORY;
	// `URLSigningKeyPrev` optional (rotation window).
	URLSigningKey     string
	URLSigningKeyPrev string
	// Default TTL used when the gateway mints URLs itself (it does not in
	// Wave 1; CAS mints). Parsed here so config shape is stable across waves.
	URLTTLSecs int64
	// Informational — CAS mints URLs prefixed with this; gateway only
	// verifies the signature over the canonical string, not the URL prefix.
	GatewayBaseURL string

	// Consumed by 03-02.
	PostgresURL   string
	SiaIndexerURL string
	// AppID is the 32-byte Sia app identifier, hex-encoded (64 chars).
	// Sourced from `SIAHUB_APP_ID` (shared with `siahub-cas`); required when
	// dialing Sia via `NewSiaAdapter`. Empty values skip the Sia adapter so
	// Wave 1's non-Sia tests still pass.
	AppID string
	// AppKey is the Sia app private key as raw hex. `SIAHUB_APP_KEY` / `APP_KEY`.
	// GOTCHA (Phase 1 §32): `siastorage.Client.PinObject` is by value. Download
	// is ctx-driven; no value/pointer confusion at our call site.
	AppKey string

	// Consumed by 03-05.
	CacheDir       string
	CacheSizeBytes int64
}

// LoadConfig reads env (with an optional `.env` fallback via godotenv) and
// returns a validated Config. Only `URLSigningKey` is mandatory at this wave;
// others accept empty / default values because their consumer plans haven't
// landed yet.
func LoadConfig() (*Config, error) {
	// Best-effort `.env` load. Any missing file is non-fatal — operators can
	// export env vars directly (the Compose `env_file:` directive is the
	// typical path in production).
	_ = godotenv.Load()

	c := &Config{
		Addr:              getenvDefault("GATEWAY_ADDR", ":8081"),
		MetricsAddr:       getenvDefault("GATEWAY_METRICS_ADDR", "127.0.0.1:9100"),
		URLSigningKey:     os.Getenv("GATEWAY_URL_SIGNING_KEY"),
		URLSigningKeyPrev: os.Getenv("GATEWAY_URL_SIGNING_KEY_PREV"),
		GatewayBaseURL:    os.Getenv("GATEWAY_BASE_URL"),
		PostgresURL:       os.Getenv("POSTGRES_URL"),
		// SIAHUB_INDEXER_URL is the canonical ops env var (ops/.env.example);
		// SIA_INDEXER_URL is the compose-wired alias. Accept either — whichever
		// is first non-empty wins.
		SiaIndexerURL: firstNonEmpty(os.Getenv("SIAHUB_INDEXER_URL"), os.Getenv("SIA_INDEXER_URL")),
		AppID:         os.Getenv("SIAHUB_APP_ID"),
		// APP_KEY is the compose-wired alias of SIAHUB_APP_KEY; keep both to
		// match ops/.env.example naming without forcing a compose rewrite.
		AppKey: firstNonEmpty(os.Getenv("SIAHUB_APP_KEY"), os.Getenv("APP_KEY")),
		// GATEWAY_CACHE_DIR is the canonical env var (03-06); CACHE_DIR is the
		// legacy alias. Either works; canonical wins when both set.
		CacheDir: firstNonEmpty(os.Getenv("GATEWAY_CACHE_DIR"), getenvDefault("CACHE_DIR", "/var/cache/siahub")),
	}

	ttl, err := parseInt64("GATEWAY_URL_TTL_SECS", 7200)
	if err != nil {
		return nil, err
	}
	c.URLTTLSecs = ttl

	// GATEWAY_CACHE_SIZE_BYTES is canonical (03-06). CACHE_SIZE_BYTES kept as
	// legacy alias; canonical wins when both set. parseInt64 on a missing key
	// returns the fallback, so pass whichever env var is actually populated.
	cacheSizeKey := "CACHE_SIZE_BYTES"
	if _, ok := os.LookupEnv("GATEWAY_CACHE_SIZE_BYTES"); ok {
		cacheSizeKey = "GATEWAY_CACHE_SIZE_BYTES"
	}
	sz, err := parseInt64(cacheSizeKey, 107374182400) // 100 GiB
	if err != nil {
		return nil, err
	}
	c.CacheSizeBytes = sz

	if c.URLSigningKey == "" {
		return nil, errors.New("GATEWAY_URL_SIGNING_KEY is required but empty")
	}
	return c, nil
}

func getenvDefault(key, fallback string) string {
	if v, ok := os.LookupEnv(key); ok && v != "" {
		return v
	}
	return fallback
}

// firstNonEmpty returns the first argument that is non-empty. Used to accept
// both canonical (SIAHUB_*) and compose-alias (APP_KEY, SIA_INDEXER_URL) env
// var names without forcing any caller to pick one.
func firstNonEmpty(values ...string) string {
	for _, v := range values {
		if v != "" {
			return v
		}
	}
	return ""
}

func parseInt64(key string, fallback int64) (int64, error) {
	v, ok := os.LookupEnv(key)
	if !ok || v == "" {
		return fallback, nil
	}
	n, err := strconv.ParseInt(v, 10, 64)
	if err != nil {
		return 0, fmt.Errorf("env %s is not a valid int64: %w", key, err)
	}
	return n, nil
}
