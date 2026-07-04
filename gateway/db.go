// Package main — db.go.
// Postgres access layer for the gateway. W2 replaces the W1 stub with a real
// `pgxpool.Pool` + the two functions / / consume:
// - `LookupXorb(ctx, hashHex) (siaObjectID types.Hash256, size int64, err error)`
// queries `xorbs` filtered on `pin_state = 'pinned'`. Rows not pinned (the
// `'uploading'` / `'pinning'` / `'orphaned'` states) MUST NOT leak
// serving bytes from an incomplete Sia upload would corrupt downloads.
// - `ErrXorbNotFound` is the sentinel the xorb handler maps to 404.
// RECEIVED §B: the gateway connects as the dedicated `openweights_gw` role created
// by cas/migrations/0005_openweights_gw_role.sql — not the `openweights` owner.
// That role has `SELECT` on `xorbs` + `INSERT` on `usage_log` and nothing
// else; the RO/RW split is at the Postgres role layer, not in this file.
// CONTEXT §2 + : metering writes go directly to `usage_log` with
// `event='download'` (see metering.go). The pool here is shared between
// reads (LookupXorb) and the meter writer.
package main

import (
	"context"
	"errors"
	"fmt"
	"time"

	"github.com/jackc/pgx/v5"
	"github.com/jackc/pgx/v5/pgxpool"
	"go.sia.tech/core/types"
)

// ErrXorbNotFound is returned by LookupXorb when no row matches the given
// hash OR the row is not in `pin_state='pinned'`. The xorb handler maps
// this to HTTP 404 (NOT 403 — 403 is reserved for signed-URL failures per
// ). The plan's top-level deliverable calls this
// `ErrXorbNotPinned` — we alias below so either name resolves.
var ErrXorbNotFound = errors.New("xorb not pinned or unknown")

// ErrXorbNotPinned is an alias for ErrXorbNotFound kept so the plan's
// deliverable naming resolves at call sites that use either form.
// canonical is `ErrXorbNotFound` because the query semantics ("not found OR
// not pinned") don't distinguish the two.
var ErrXorbNotPinned = ErrXorbNotFound

// DB wraps the pgx pool. Exposed as a struct (not `*pgxpool.Pool` directly)
// so can mock at the interface level for tests without a live Postgres.
// The `pool` field is unexported; tests poke it via the `DBPool` helper in
// db_test.go.
type DB struct {
	pool *pgxpool.Pool
}

// NewDB constructs a pooled connection to Postgres. The connStr is expected
// to carry `application_name=openweights-gateway` (compose sets this) so the
// CAS dashboards can distinguish gateway connections from CAS connections.
// Pool sizing: MaxConns=20 is conservative. The gateway is overwhelmingly
// IO-bound on Sia downloads, not on Postgres — a tiny lookup per request
// plus occasional meter batch inserts. Oversizing the pool wastes Postgres
// slots that CAS and the reconciler need.
// W2 also `Ping`s to fail fast on bad connection strings instead of surfacing
// as the first lookup after boot timing-out.
func NewDB(ctx context.Context, connStr string) (*DB, error) {
	if connStr == "" {
		return nil, errors.New("POSTGRES_URL is empty; set it in .env before starting the gateway")
	}

	cfg, err := pgxpool.ParseConfig(connStr)
	if err != nil {
		return nil, fmt.Errorf("parse pool config: %w", err)
	}
	cfg.MaxConns = 20
	cfg.MaxConnLifetime = 30 * time.Minute
	cfg.MaxConnIdleTime = 5 * time.Minute
	// Stamp application_name even if the caller forgot to include it in the
	// URL. Makes `pg_stat_activity` immediately readable.
	if cfg.ConnConfig.RuntimeParams == nil {
		cfg.ConnConfig.RuntimeParams = map[string]string{}
	}
	if _, ok := cfg.ConnConfig.RuntimeParams["application_name"]; !ok {
		cfg.ConnConfig.RuntimeParams["application_name"] = "openweights-gateway"
	}

	pool, err := pgxpool.NewWithConfig(ctx, cfg)
	if err != nil {
		return nil, fmt.Errorf("dial pool: %w", err)
	}

	// Fail fast on unreachable Postgres. A 2s timeout is sane for the
	// docker-compose internal network; longer deadlines are the caller's
	// problem (they pass `ctx`).
	pingCtx, cancel := context.WithTimeout(ctx, 2*time.Second)
	defer cancel()
	if err := pool.Ping(pingCtx); err != nil {
		pool.Close()
		return nil, fmt.Errorf("ping pool: %w", err)
	}

	return &DB{pool: pool}, nil
}

// Close tears down the pool. Safe to call multiple times (underlying
// pgxpool.Pool.Close is idempotent enough for our shutdown path).
func (d *DB) Close() {
	if d == nil || d.pool == nil {
		return
	}
	d.pool.Close()
}

// Pool exposes the underlying pgxpool.Pool for code in the same package that
// needs direct batch / transaction access (currently: `metering.go`). Not
// part of any external API; downstream plans should prefer typed methods.
func (d *DB) Pool() *pgxpool.Pool {
	return d.pool
}

// LookupXorb resolves a 64-char lowercase hex xorb merkle hash to its Sia
// `object_id` + `size_bytes`. The filter `pin_state = 'pinned'` is MANDATORY:
// serving bytes from an `'uploading'`/`'pinning'` row would hand the client
// a partial xorb (the Sia upload may not have finished) and bypassing the
// `'orphaned'` state would let 5-failure rows resurface.
// Hashes in `xorbs.xorb_merkle_hash` are stored as raw 32-byte BYTEA
// ( PITFALL — NEVER hex). The verifier hands us a hex string, so
// we decode before the lookup. Invalid hex returns a typed error separate
// from ErrXorbNotFound so the handler layer can distinguish 400 vs 404.
// `siaObjectID` comes back as `types.Hash256` because that's what the
// siastorage SDK consumes directly in `SDK.Object(ctx, objectKey)`. The
// underlying BYTEA column is 32 bytes; rows with NULL `sia_object_id`
// (an `'uploading'` row that never finished) are excluded by the pin-state
// filter, so the column read is infallible when a row matches.
func (d *DB) LookupXorb(ctx context.Context, hashHex string) (siaObjectID types.Hash256, size int64, err error) {
	// Validate input shape BEFORE touching the pool so a malformed hash
	// fails the same way whether or not a DB is wired.
	hashBytes, decodeErr := decodeXorbHash(hashHex)
	if decodeErr != nil {
		return types.Hash256{}, 0, decodeErr
	}
	if d == nil || d.pool == nil {
		return types.Hash256{}, 0, errors.New("LookupXorb on nil DB")
	}

	var rawSiaID []byte
	row := d.pool.QueryRow(ctx, `
		SELECT sia_object_id, size_bytes
		  FROM xorbs
		 WHERE xorb_merkle_hash = $1
		   AND pin_state = 'pinned'
	`, hashBytes)

	if scanErr := row.Scan(&rawSiaID, &size); scanErr != nil {
		if errors.Is(scanErr, pgx.ErrNoRows) {
			return types.Hash256{}, 0, ErrXorbNotFound
		}
		return types.Hash256{}, 0, fmt.Errorf("scan xorb row: %w", scanErr)
	}
	if len(rawSiaID) != 32 {
		return types.Hash256{}, 0, fmt.Errorf(
			"corrupt xorbs.sia_object_id: got %d bytes, want 32", len(rawSiaID))
	}
	copy(siaObjectID[:], rawSiaID)
	return siaObjectID, size, nil
}

// QueryXorbPinned is the deliverable-named alias of LookupXorb kept so the
// top-level plan description (`QueryXorbPinned(xorbHash) (siaObjectID...)`)
// resolves at call sites. Returns the Sia object ID as a hex string because
// the deliverable description uses `siaObjectID string`; internal code should
// prefer the typed LookupXorb.
func (d *DB) QueryXorbPinned(ctx context.Context, hashHex string) (siaObjectID string, err error) {
	id, _, err := d.LookupXorb(ctx, hashHex)
	if err != nil {
		return "", err
	}
	return id.String(), nil
}

// decodeXorbHash validates the 64-char lowercase hex representation the
// verifier hands us and returns the raw 32 bytes the BYTEA column stores.
// Non-hex input is a 400-class error (malformed URL); the verifier itself
// rejects malformed hashes with `VerifyErr.Kind == "malformed:xorb_hash_hex"`
// before we ever reach the DB, so reaching here with garbage input is a
// programmer error on the handler side.
func decodeXorbHash(hashHex string) ([]byte, error) {
	if len(hashHex) != 64 {
		return nil, fmt.Errorf("xorb hash must be 64 hex chars; got %d", len(hashHex))
	}
	buf := make([]byte, 32)
	for i := 0; i < 32; i++ {
		hi, err := hexNibble(hashHex[i*2])
		if err != nil {
			return nil, err
		}
		lo, err := hexNibble(hashHex[i*2+1])
		if err != nil {
			return nil, err
		}
		buf[i] = hi<<4 | lo
	}
	return buf, nil
}

func hexNibble(c byte) (byte, error) {
	switch {
	case c >= '0' && c <= '9':
		return c - '0', nil
	case c >= 'a' && c <= 'f':
		return c - 'a' + 10, nil
	case c >= 'A' && c <= 'F':
		return c - 'A' + 10, nil
	}
	return 0, fmt.Errorf("invalid hex char %q in xorb hash", c)
}
