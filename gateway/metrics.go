// Package main — metrics.go.
// Wave 1 exposes the Prometheus registry and ONE counter
// (`gateway_requests_total`). The full metric set (Sia download duration,
// cache hit/miss, singleflight coalescing, hash-mismatch, bytes served) lands
// in. Registry is exported so later plans can register more metrics
// without touching main.go.
// : the `/metrics` handler is served on a LOOPBACK-only listener
// (`GATEWAY_METRICS_ADDR`, default `127.0.0.1:9100`). Caddy MUST NOT proxy it.
package main

import (
	"net/http"
	"sync"

	"github.com/prometheus/client_golang/prometheus"
	"github.com/prometheus/client_golang/prometheus/promhttp"
)

// Metrics bundles the registry + all counters / histograms. Exported for
// wiring in main.go and for downstream-plan growth.
// Cache counters (added in, Wave 3b) live on a set of
// package-level globals so `cache.go` + `singleflight.go` + `handlers.go`
// can increment without threading a `*Metrics` through every function.
// Those globals are lazy-registered on first NewMetrics call via
// `registerCacheMetrics` so tests that construct multiple Metrics objects
// don't double-register (Prometheus panics on duplicate).
type Metrics struct {
	Registry    *prometheus.Registry
	RequestsTot *prometheus.CounterVec
}

// Cache-layer counters — package-level so cache.go / singleflight.go /
// handlers.go can increment without a Metrics handle. Registration happens
// once in `registerCacheMetrics`, which NewMetrics invokes on the registry
// it owns. Duplicate-registration panics are prevented by guarding the
// actual collector creation behind a sync.Once — subsequent NewMetrics
// calls get the SAME collector instance, which is safe to re-register into
// a different registry because prometheus.Registry permits the same
// Collector on multiple registries.
var (
	metricsCacheHits                prometheus.Counter
	metricsCacheMisses              prometheus.Counter
	metricsCacheEvicted             prometheus.Counter
	metricsCacheHashMismatch        prometheus.Counter
	metricsCacheSingleflightCoalesced prometheus.Counter
	metricsCacheBytesOnDisk         prometheus.Gauge

	cacheMetricsInit sync.Once
)

// registerCacheMetrics is idempotent: builds the cache collectors exactly
// once, and registers them on the supplied registry. Every call registers
// the SAME collector instances — tests that build multiple Metrics objects
// share the counters, which is the right semantic (they're package-level).
func registerCacheMetrics(reg *prometheus.Registry) {
	cacheMetricsInit.Do(func() {
		metricsCacheHits = prometheus.NewCounter(prometheus.CounterOpts{
			Name: "gateway_cache_hits_total",
			Help: "Whole-xorb disk LRU cache hits (request served from local disk without Sia fetch).",
		})
		metricsCacheMisses = prometheus.NewCounter(prometheus.CounterOpts{
			Name: "gateway_cache_misses_total",
			Help: "Whole-xorb disk LRU cache misses (request triggered a Sia range fetch).",
		})
		metricsCacheEvicted = prometheus.NewCounter(prometheus.CounterOpts{
			Name: "gateway_cache_evicted_total",
			Help: "Cache entries evicted from the LRU due to size pressure.",
		})
		metricsCacheHashMismatch = prometheus.NewCounter(prometheus.CounterOpts{
			Name: "gateway_cache_hash_mismatch_total",
			Help: "Cache-write rejections where the streamed body's merkle hash disagreed with the xorb hash key. Red-alert metric — PITFALL P11 / GATE-07.",
		})
		metricsCacheSingleflightCoalesced = prometheus.NewCounter(prometheus.CounterOpts{
			Name: "gateway_cache_singleflight_coalesced_total",
			Help: "Number of cache-miss fetches that were coalesced onto a concurrent leader's in-flight fetch (D-30 / GATE-09).",
		})
		metricsCacheBytesOnDisk = prometheus.NewGauge(prometheus.GaugeOpts{
			Name: "gateway_cache_bytes_on_disk",
			Help: "Total bytes currently resident in the whole-xorb disk LRU.",
		})
	})
	// Register on this registry. Safe to re-register the same collector on
	// a fresh registry; prometheus only panics on duplicate within ONE
	// registry, not across.
	for _, c := range []prometheus.Collector{
		metricsCacheHits,
		metricsCacheMisses,
		metricsCacheEvicted,
		metricsCacheHashMismatch,
		metricsCacheSingleflightCoalesced,
		metricsCacheBytesOnDisk,
	} {
		// Use Register (not MustRegister) so repeat calls against the same
		// registry are a no-op error we swallow, not a panic.
		_ = reg.Register(c)
	}
}

// NewMetrics constructs a fresh registry and registers the Wave 1 metric set.
// Uses a private registry (NOT `prometheus.DefaultRegisterer`) so tests can
// instantiate multiple without global-state collisions.
func NewMetrics() *Metrics {
	reg := prometheus.NewRegistry()
	m := &Metrics{
		Registry: reg,
		RequestsTot: prometheus.NewCounterVec(
			prometheus.CounterOpts{
				Name: "gateway_requests_total",
				Help: "Total HTTP requests served by the gateway, partitioned by method, route, and response status.",
			},
			[]string{"method", "route", "status"},
		),
	}
	reg.MustRegister(m.RequestsTot)
	registerCacheMetrics(reg)
	return m
}

// Handler returns the `promhttp` handler wired to this registry.
func (m *Metrics) Handler() http.Handler {
	return promhttp.HandlerFor(m.Registry, promhttp.HandlerOpts{
		EnableOpenMetrics: true,
	})
}
