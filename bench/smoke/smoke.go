// Package main — 1 MiB round-trip smoke test against live indexd (Phase 1 SC-3).
// Uploads, pins, full-downloads, range-downloads, and byte-compares a 1 MiB
// random fixture. Invoked by `make smoke` and by the bootstrap wizard at its
// final step; NOT unit-tested against live network (unit test just ensures
// the binary compiles — live execution is the bootstrap-wizard's responsibility).
package main

import (
	"bytes"
	"context"
	"crypto/rand"
	"encoding/hex"
	"fmt"
	"log/slog"
	"os"

	"go.sia.tech/core/types"
	"go.sia.tech/siastorage"
)

const fixtureSize = 1024 * 1024 // 1 MiB

func main() {
	logger := slog.New(slog.NewJSONHandler(os.Stdout, nil))
	ctx := context.Background()

	indexerURL := os.Getenv("SIAHUB_INDEXER_URL")
	appIDHex := os.Getenv("SIAHUB_APP_ID")
	appKeyHex := os.Getenv("SIAHUB_APP_KEY")
	if indexerURL == "" || appIDHex == "" || appKeyHex == "" {
		logger.Error("missing env: SIAHUB_INDEXER_URL, SIAHUB_APP_ID, SIAHUB_APP_KEY")
		os.Exit(2)
	}

	var appID types.Hash256
	if err := appID.UnmarshalText([]byte(appIDHex)); err != nil {
		logger.Error("invalid AppID", "err", err)
		os.Exit(2)
	}
	appKeyBytes, err := hex.DecodeString(appKeyHex)
	if err != nil {
		logger.Error("invalid AppKey hex", "err", err)
		os.Exit(2)
	}
	appKey := types.PrivateKey(appKeyBytes)

	client, err := siastorage.NewBuilder(indexerURL, siastorage.AppMetadata{ID: appID, Name: "siahub-smoke"}).SDK(appKey)
	if err != nil {
		logger.Error("SDK init", "err", err)
		os.Exit(1)
	}
	defer client.Close()

	// 1. Generate 1 MiB random fixture.
	fixture := make([]byte, fixtureSize)
	if _, err := rand.Read(fixture); err != nil {
		logger.Error("rand.Read", "err", err)
		os.Exit(1)
	}

	// 2. Upload + pin. Upload takes *Object (sets fields in place); PinObject
	// takes Object by value (Gotcha 32 — do NOT pass &obj to PinObject).
	obj := siastorage.NewEmptyObject()
	// Zen testnet has ~3 usable hosts; default erasure coding wants 30. Use
	// 2-data-1-parity redundancy so Phase 1 smoke + thesis can run while
	// testnet host count recovers. Phase 5/6 mainnet drops this override.
	if err := client.Upload(ctx, &obj, bytes.NewReader(fixture), siastorage.WithRedundancy(1, 2)); err != nil {
		logger.Error("upload", "err", err)
		os.Exit(1)
	}
	if err := client.PinObject(ctx, obj); err != nil {
		logger.Error("pin", "err", err)
		os.Exit(1)
	}
	logger.Info("uploaded + pinned", "object_id", obj.ID().String())

	// 3. Full-object download + byte-compare.
	var fullBuf bytes.Buffer
	if err := client.Download(ctx, &fullBuf, obj); err != nil {
		logger.Error("download full", "err", err)
		os.Exit(1)
	}
	if !bytes.Equal(fullBuf.Bytes(), fixture) {
		logger.Error("full download mismatch", "got_len", fullBuf.Len(), "want_len", fixtureSize)
		os.Exit(1)
	}

	// 4. Range download (middle 64 KiB) + byte-compare vs fixture slice.
	const rOff = 512 * 1024
	const rLen = 64 * 1024
	var rangeBuf bytes.Buffer
	if err := client.Download(ctx, &rangeBuf, obj, siastorage.WithDownloadRange(rOff, rLen)); err != nil {
		logger.Error("download range", "err", err)
		os.Exit(1)
	}
	if !bytes.Equal(rangeBuf.Bytes(), fixture[rOff:rOff+rLen]) {
		logger.Error("range download mismatch")
		os.Exit(1)
	}

	fmt.Fprintln(os.Stderr, "smoke: PASS — 1 MiB upload + pin + full-download + range-download all byte-identical")
	os.Exit(0)
}
