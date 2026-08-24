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

	// Retry the initial ping with backoff. On `docker compose up` Postgres may
	// still be initializing (it briefly refuses connections / returns
	// SQLSTATE 57P03 "the database system is starting up") when the gateway
	// boots; a single failed ping used to leave the gateway permanently
	// DB-less, 500ing every /xorb until a manual restart. Give it up to ~60s.
	deadline := time.Now().Add(60 * time.Second)
	for {
		pingCtx, cancel := context.WithTimeout(ctx, 2*time.Second)
		err := pool.Ping(pingCtx)
		cancel()
		if err == nil {
			break
		}
		if ctx.Err() != nil || time.Now().After(deadline) {
			pool.Close()
			return nil, fmt.Errorf("ping pool (after retries): %w", err)
		}
		time.Sleep(2 * time.Second)
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

// decodeXorbHash converts the xet-core canonical MerkleHash hex the verifier
// hands us into the raw 32 bytes the `xorb_merkle_hash` BYTEA column stores.
// CRITICAL (gotcha #1): the hex is byte-reversed per 8-byte group, so it MUST
// be decoded through the MerkleHash codec (`ParseMerkleHashHex`) and NOT a
// straight hex decode — a straight decode yields byte-reversed bytes that never
// match a pinned row, so every real download 404s at the gateway. The CAS
// stores the raw digest (`MerkleHash::into::<[u8;32]>()`) and emits the reversed
// hex (`MerkleHash::hex()`) in reconstruction URLs; this is the inverse.
// Non-hex/length errors are 400-class; the verifier already rejects malformed
// hashes before we reach the DB.
func decodeXorbHash(hashHex string) ([]byte, error) {
	digest, err := ParseMerkleHashHex(hashHex)
	if err != nil {
		return nil, err
	}
	return digest[:], nil
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
