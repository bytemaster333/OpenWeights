//go:build integration
// +build integration

// Package main — integration-gated thesis measurement test.
// THIS IS THE VALIDATION.md CONTRACT TEST for the "Thesis measurement program" row.
// It drives the same runTrial + computeVerdict code path as `main` but asserts the
// verdict inside `go test` — so CI / make targets can gate on a PASS/FAIL test result
// rather than parsing the program's exit code.
// Execution requirements (enforced at runtime; test skips otherwise):
// - TESTNET_LIVE=1 (opt-in flag; CI defaults off)
// - SIAHUB_APP_KEY, SIAHUB_APP_ID (from PLAN 07 bootstrap wizard)
// - SIAHUB_INDEXER_URL (live indexd, typically http://localhost:9982)
// - indexd is synced + wallet is funded (PLAN 05 readiness probe path; operator responsibility)
// Invocation (authoritative per VALIDATION.md):
// cd bench && go test -tags=integration ./thesis/... -run TestRangeDownloadSectorScoping -timeout 30m
// Discovery check:
// cd bench && go test -tags=integration ./thesis/... -run TestRangeDownloadSectorScoping -list '.*'
// → prints TestRangeDownloadSectorScoping (confirms name is reachable under the tag)
package main

import (
	"bytes"
	"context"
	"crypto/rand"
	"encoding/hex"
	"os"
	"testing"
	"time"

	"go.sia.tech/core/types"
	"go.sia.tech/siastorage"
)

// TestRangeDownloadSectorScoping runs the full thesis measurement end-to-end
// and asserts the verdict at Go-test level. This is the automated command
// referenced in VALIDATION.md's Per-Task Verification Map.
// Budget per VALIDATION.md: 30 minutes total. Breakdown:
// - Upload 64 MiB to Sia: up to ~15 min on slow-contract testnet
// - 3 trials * (3s noise-floor + download): ~15s-60s each
// - Reporting: <1s
// Well within the 30-min timeout.
func TestRangeDownloadSectorScoping(t *testing.T) {
	if os.Getenv("TESTNET_LIVE") != "1" {
		t.Skip("TESTNET_LIVE!=1 — integration test requires opt-in flag (live testnet access)")
	}
	indexerURL := os.Getenv("SIAHUB_INDEXER_URL")
	appIDHex := os.Getenv("SIAHUB_APP_ID")
	appKeyHex := os.Getenv("SIAHUB_APP_KEY")
	if indexerURL == "" || appIDHex == "" || appKeyHex == "" {
		t.Skip("SIAHUB_INDEXER_URL / SIAHUB_APP_ID / SIAHUB_APP_KEY must all be set (run `make bootstrap` first)")
	}

	var appID types.Hash256
	if err := appID.UnmarshalText([]byte(appIDHex)); err != nil {
		t.Fatalf("invalid SIAHUB_APP_ID: %v", err)
	}
	appKeyBytes, err := hex.DecodeString(appKeyHex)
	if err != nil {
		t.Fatalf("invalid SIAHUB_APP_KEY hex: %v", err)
	}
	appKey := types.PrivateKey(appKeyBytes)

	builder := siastorage.NewBuilder(indexerURL, siastorage.AppMetadata{
		ID:          appID,
		Name:        "siahub-thesis-integration",
		Description: "VALIDATION.md contract test for thesis measurement",
	})
	client, err := builder.SDK(appKey)
	if err != nil {
		t.Fatalf("SDK init: %v", err)
	}
	defer client.Close()

	// Upload fixture: 64 MiB random bytes (same constants as main).
	buf := make([]byte, objectSize)
	if _, err := rand.Read(buf); err != nil {
		t.Fatalf("rand.Read: %v", err)
	}
	obj := siastorage.NewEmptyObject()

	// Use a generous upload context — contract formation on testnet can take minutes.
	uploadCtx, uploadCancel := context.WithTimeout(context.Background(), 20*time.Minute)
	defer uploadCancel()
	if err := client.Upload(uploadCtx, &obj, bytes.NewReader(buf)); err != nil {
		t.Fatalf("upload failed: %v", err)
	}
	if err := client.PinObject(uploadCtx, obj); err != nil {
		t.Fatalf("pin failed: %v", err)
	}
	t.Logf("fixture uploaded: object_id=%s size=%d", obj.ID().String(), objectSize)

	// Run N trials via the same helper main uses.
	trialCtx, trialCancel := context.WithTimeout(context.Background(), 8*time.Minute)
	defer trialCancel()
	trials := make([]Trial, 0, trialsN)
	for i := 1; i <= trialsN; i++ {
		tr := runTrial(trialCtx, client, obj, i)
		if tr.Err != "" {
			t.Fatalf("trial %d: Download error: %s", i, tr.Err)
		}
		t.Logf("trial %d: inbound_bytes=%d ratio=%.3fx duration_ms=%d",
			tr.TrialNum, tr.InboundBytes, tr.Ratio, tr.DurationMs)
		trials = append(trials, tr)
	}

	// Compute verdict via the same helper main uses.
	minB, medB, maxB, verdict := computeVerdict(trials, passCeiling)
	t.Logf("verdict=%s min=%d median=%d max=%d passCeiling=%d",
		verdict, minB, medB, maxB, passCeiling)

	if verdict != "PASS" {
		// Per + : FAIL here means median > 8× requested range.
		// The program's main exits 3 and the Makefile wraps to 0 (informational),
		// but at Go-test level we surface this as a test failure — VALIDATION.md's
		// Per-Task Verification Map expects this to FAIL the test on a FAIL verdict.
		t.Fatalf("thesis verdict %s — median %d bytes exceeds D-01 ceiling %d bytes (8× of %d)",
			verdict, medB, passCeiling, rangeLength)
	}
}
