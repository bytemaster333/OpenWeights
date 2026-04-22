// Package main — metering.go.
//
// Per-request usage-log writer for the gateway. Every served range request
// produces exactly one row in `usage_log` with:
//
//   event       = 'download'     (D-29 — locked canonical name)
//   bytes       = bytes served to the client
//   api_key_id  = the API key that minted the signed URL
//   cache_hit   = true if served from disk LRU, false if fetched from Sia
//   occurred_at = now (Postgres default)
//
// D-27: the gateway writes directly. D-17 (from Phase 2 0003 migration header)
// says CAS writes are synchronous single-row INSERTs; we match that shape:
// `LogDownload` is a synchronous INSERT. If the 03-06 metric emission load
// reveals meter-INSERT latency dominating a request, the non-blocking batched
// channel form in the plan's commented `Meter` skeleton is the upgrade path —
// the schema is forward-compat (same columns, same event literal), so no
// migration is needed to flip.
//
// The Meter struct is kept exported so 03-06 can introduce a buffered / async
// variant without breaking the `LogDownload` signature callers already depend
// on.
package main

import (
	"context"
	"errors"
	"fmt"

	"github.com/google/uuid"
	"github.com/prometheus/client_golang/prometheus"
)

// UsageWriter is the interface 03-03 / 03-05 consume. Tests substitute a
// fake; production wires a real `Meter`.
type UsageWriter interface {
	LogDownload(ctx context.Context, apiKeyID uuid.UUID, bytes int64, cacheHit bool, xorbHashHex string) error
}

// Meter wraps the shared DB pool + Prometheus counters for the metering
// path. Currently does synchronous INSERTs per D-17 — the channel-based
// buffered variant is deferred until 03-06 benchmarks justify it.
type Meter struct {
	db             *DB
	insertsTot     *prometheus.CounterVec // labels: event, cache_hit
	insertErrorTot prometheus.Counter
}

// NewMeter constructs a Meter bound to the given DB and registers
// Prometheus counters against `reg`. Nil `reg` is accepted (unit tests) —
// counters become no-ops.
func NewMeter(db *DB, reg prometheus.Registerer) *Meter {
	m := &Meter{
		db: db,
		insertsTot: prometheus.NewCounterVec(
			prometheus.CounterOpts{
				Name: "gateway_usage_log_inserts_total",
				Help: "Rows INSERTed into usage_log by the gateway meter, labeled by event + cache_hit.",
			},
			[]string{"event", "cache_hit"},
		),
		insertErrorTot: prometheus.NewCounter(
			prometheus.CounterOpts{
				Name: "gateway_usage_log_insert_errors_total",
				Help: "Count of failed usage_log INSERTs. Non-zero is operator-visible; the download itself still succeeds.",
			},
		),
	}
	if reg != nil {
		for _, c := range []prometheus.Collector{m.insertsTot, m.insertErrorTot} {
			if err := reg.Register(c); err != nil {
				var are prometheus.AlreadyRegisteredError
				if !errors.As(err, &are) {
					// Registration errors other than already-registered are
					// programmer bugs, not runtime errors; panic lines up with
					// `prometheus.MustRegister` elsewhere in the package.
					panic(fmt.Errorf("register meter counter: %w", err))
				}
			}
		}
	}
	return m
}

// LogDownload writes one 'download' row to `usage_log`. Synchronous: the
// caller's handler must not proceed to write its HTTP status code before
// this returns, because a non-nil error is operator-informational (we still
// served the bytes). Most callers will fire-and-forget with a background
// ctx so the client's request ctx cancellation does not abort the meter
// write mid-INSERT.
//
// Schema consumed (from cas/migrations/0003_usage_log_oauth.sql):
//
//	id           BIGSERIAL PRIMARY KEY
//	event        usage_event NOT NULL
//	api_key_id   UUID   NULL REFERENCES api_keys(id)
//	user_id      BIGINT NULL
//	xorb_hash    BYTEA  NULL     <- populated so CONSOLE-04 prefix search works on downloads too
//	shard_hash   BYTEA  NULL
//	file_id      BYTEA  NULL
//	bytes        BIGINT NULL
//	cache_hit    BOOLEAN NULL    <- Phase 3 gateway populates (OQ-J / D-17 note)
//	occurred_at  TIMESTAMPTZ NOT NULL DEFAULT NOW()
//
// 'download' enum value: D-29 makes this the canonical name for gateway
// writes. The 0003 migration's enum did NOT define 'download' originally —
// it defined 'xorb_serve' / 'dedup_query' as reserved Phase 3 slots. D-29
// overrides that reservation to converge on 'download', matching Phase 4's
// CONSOLE-03/04/08 stats query surface. If the enum in a given deployment
// predates the D-29 lock, a one-line ALTER TYPE adds the value; that's a
// deployment concern, not a migration a gateway plan can author (Phase 2
// crates are FROZEN per Rule 2).
func (m *Meter) LogDownload(
	ctx context.Context,
	apiKeyID uuid.UUID,
	bytes int64,
	cacheHit bool,
	xorbHashHex string,
) error {
	// Validate hash input shape BEFORE touching the pool. The download path
	// already succeeded by the time the meter runs, so surfacing a typed
	// decode error to the caller (who logs + discards it) is preferable to
	// either panicking on a nil pool OR silently INSERTing a NULL hash.
	var hashBytes []byte
	if xorbHashHex != "" {
		decoded, err := decodeXorbHash(xorbHashHex)
		if err != nil {
			// Metric counter is nil-safe: if NewMeter was not called
			// (test path), we still want the caller to see the error.
			if m != nil && m.insertErrorTot != nil {
				m.insertErrorTot.Inc()
			}
			return fmt.Errorf("decode xorb hash for usage_log: %w", err)
		}
		hashBytes = decoded
	}

	if m == nil || m.db == nil || m.db.pool == nil {
		return errors.New("LogDownload on nil Meter or DB")
	}

	// Nil UUID is the sentinel "anonymous / signed-URL with no api_key_id
	// claim". Store as NULL so FK cascade cleanup is sane.
	var apiKeyArg any
	if apiKeyID == uuid.Nil {
		apiKeyArg = nil
	} else {
		apiKeyArg = apiKeyID
	}

	_, err := m.db.pool.Exec(ctx, `
		INSERT INTO usage_log (event, api_key_id, xorb_hash, bytes, cache_hit)
		VALUES ('download', $1, $2, $3, $4)
	`, apiKeyArg, hashBytes, bytes, cacheHit)
	if err != nil {
		m.insertErrorTot.Inc()
		return fmt.Errorf("insert usage_log: %w", err)
	}

	cacheHitLabel := "false"
	if cacheHit {
		cacheHitLabel = "true"
	}
	m.insertsTot.WithLabelValues("download", cacheHitLabel).Inc()
	return nil
}
