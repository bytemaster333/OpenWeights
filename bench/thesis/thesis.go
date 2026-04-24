// Package main — Sia range-download thesis measurement.
// See.planning/phases/01-validation-foundations/01-RESEARCH.md §1 + §2 for methodology.
// Per D-01: 128 KiB range of a 64 MiB object; PASS if median inbound bytes ≤ 1 MiB across 3 trials.
// Per D-02: FAIL is informational, not a halt; program exits 3 on FAIL, Makefile wraps to 0.
package main

import (
	"bytes"
	"context"
	"crypto/rand"
	"encoding/hex"
	"fmt"
	"log/slog"
	"os"
	"time"

	"go.sia.tech/core/types"
	"go.sia.tech/siastorage"
)

const (
	objectSize  = 64 * 1024 * 1024 // 64 MiB
	rangeOffset = 32 * 1024 * 1024 // middle of the object
	rangeLength = 128 * 1024       // 128 KiB
	passCeiling = 1 * 1024 * 1024  // 1 MiB (8× requested range per D-01)
	trialsN     = 3
	iface       = "eth0" // Compose default bridge gives eth0 inside container
	sdkVersion  = "siastorage v0.0.3"
)

// noiseFloorProtocol — per RESEARCH §2: sample RX delta during a 3-second idle window
// to establish per-trial noise floor, so we can subtract keepalives.
const noiseFloorWindow = 3 * time.Second

type countingWriter struct{ n uint64 }

func (c *countingWriter) Write(p []byte) (int, error) {
	c.n += uint64(len(p))
	return len(p), nil
}

// runTrial performs one download measurement + noise-floor adjustment.
// Exposed at package scope so TestRangeDownloadSectorScoping (integration
// build tag) can reuse it without duplicating the 3-second idle / counter-
// diff / ratio-compute dance.
func runTrial(ctx context.Context, client *siastorage.SDK, obj siastorage.Object, trialNum int) Trial {
	// Noise floor: 3-second idle, measure drift.
	nfBefore, _ := readInboundBytes(iface)
	time.Sleep(noiseFloorWindow)
	nfAfter, _ := readInboundBytes(iface)
	noiseFloor := nfAfter - nfBefore

	// Actual measurement.
	before, _ := readInboundBytes(iface)
	start := time.Now()
	w := &countingWriter{}
	dErr := client.Download(ctx, w, obj, siastorage.WithDownloadRange(rangeOffset, rangeLength))
	elapsed := time.Since(start)
	after, _ := readInboundBytes(iface)

	rawInbound := after - before
	// Subtract noise floor scaled by elapsed-vs-noiseFloorWindow (simple proportional adjust).
	var adjusted uint64 = rawInbound
	if noiseFloor > 0 && elapsed > 0 {
		scaled := uint64(float64(noiseFloor) * (float64(elapsed) / float64(noiseFloorWindow)))
		if scaled < adjusted {
			adjusted -= scaled
		} else {
			adjusted = 0
		}
	}

	var errStr string
	if dErr != nil {
		errStr = dErr.Error()
	}
	return Trial{
		TrialNum:     trialNum,
		InboundBytes: adjusted,
		Ratio:        float64(adjusted) / float64(rangeLength),
		DurationMs:   elapsed.Milliseconds(),
		Err:          errStr,
	}
}

func main() {
	logger := slog.New(slog.NewJSONHandler(os.Stdout, nil))
	ctx := context.Background()

	indexerURL := os.Getenv("SIAHUB_INDEXER_URL")
	appIDHex := os.Getenv("SIAHUB_APP_ID")
	appKeyHex := os.Getenv("SIAHUB_APP_KEY")
	if indexerURL == "" || appIDHex == "" || appKeyHex == "" {
		logger.Error("missing env: need SIAHUB_INDEXER_URL, SIAHUB_APP_ID, SIAHUB_APP_KEY")
		os.Exit(2)
	}

	var appID types.Hash256
	if err := appID.UnmarshalText([]byte(appIDHex)); err != nil {
		logger.Error("invalid SIAHUB_APP_ID", "err", err)
		os.Exit(2)
	}
	appKeyBytes, err := hex.DecodeString(appKeyHex)
	if err != nil {
		logger.Error("invalid SIAHUB_APP_KEY hex", "err", err)
		os.Exit(2)
	}
	appKey := types.PrivateKey(appKeyBytes)

	builder := siastorage.NewBuilder(indexerURL, siastorage.AppMetadata{
		ID:          appID,
		Name:        "siahub-thesis",
		Description: "Phase 1 thesis measurement",
	})
	client, err := builder.SDK(appKey)
	if err != nil {
		logger.Error("SDK init failed", "err", err)
		os.Exit(1)
	}
	defer client.Close()

	// 1. Upload 64 MiB random object once; reuse across trials.
	buf := make([]byte, objectSize)
	if _, err := rand.Read(buf); err != nil {
		logger.Error("rand.Read failed", "err", err)
		os.Exit(1)
	}
	obj := siastorage.NewEmptyObject()
	// Zen testnet has ~3 usable hosts; default erasure coding wants 30.
	// Use 2-data-1-parity redundancy so Phase 1 thesis can run on Zen.
	// Phase 5/6 mainnet should drop this override.
	if err := client.Upload(ctx, &obj, bytes.NewReader(buf), siastorage.WithRedundancy(2, 1)); err != nil {
		logger.Error("upload failed", "err", err)
		os.Exit(1)
	}
	// Per Gotcha 32: PinObject takes obj BY VALUE (not pointer).
	if err := client.PinObject(ctx, obj); err != nil {
		logger.Error("pin failed", "err", err)
		os.Exit(1)
	}
	logger.Info("fixture uploaded", "object_id", obj.ID().String(), "size", objectSize)

	// 2. Run N trials via runTrial helper.
	trials := make([]Trial, 0, trialsN)
	for i := 1; i <= trialsN; i++ {
		tr := runTrial(ctx, client, obj, i)
		trials = append(trials, tr)
		logger.Info("trial complete",
			"trial", tr.TrialNum,
			"inbound_bytes_adjusted", tr.InboundBytes,
			"duration_ms", tr.DurationMs,
			"ratio_to_requested", tr.Ratio,
		)
	}

	// 3. Compute verdict.
	minB, medB, maxB, verdict := computeVerdict(trials, passCeiling)
	ts := time.Now().UTC()
	runDir := ts.Format("20060102T150405Z")

	r := Report{
		Thesis:         "Sia SDK WithDownloadRange is sector-scoped, not full-object-slice",
		ObjectSize:     objectSize,
		RequestedRange: rangeLength,
		PassCeiling:    passCeiling,
		Trials:         trials,
		Min:            minB,
		Median:         medB,
		Max:            maxB,
		Verdict:        verdict,
		SDKVersion:     sdkVersion,
		Timestamp:      ts,
		RunDir:         runDir,
	}

	// 4. Write artifacts.
	runsDir := "bench/thesis/runs/" + runDir
	jsonPath, err := writeRun(runsDir, r)
	if err != nil {
		logger.Error("writeRun failed", "err", err)
		os.Exit(1)
	}
	md, err := renderReportMarkdown("bench/thesis/REPORT.md.tmpl", r)
	if err != nil {
		// Template failure doesn't invalidate the data; log + continue.
		logger.Error("renderReport failed", "err", err)
	} else {
		if err := writeReportMarkdown("bench/thesis/REPORT.md", md); err != nil {
			logger.Error("writeReportMarkdown failed", "err", err)
		}
	}

	logger.Info("thesis complete",
		"verdict", verdict,
		"min", minB,
		"median", medB,
		"max", maxB,
		"report_json", jsonPath,
		"report_md", "bench/thesis/REPORT.md",
	)

	// Distinct exit codes so `make thesis` can differentiate:
	// 0 = PASS
	// 3 = FAIL (but Makefile wraps this to 0 per D-02)
	// 1/2 = hard error (SDK init, upload, etc.)
	if verdict == "FAIL" {
		fmt.Fprintln(os.Stderr, "thesis: FAIL — median exceeds 8× threshold. See bench/thesis/REPORT.md + CONTEXT D-02.")
		os.Exit(3)
	}
	fmt.Fprintln(os.Stderr, "thesis: PASS")
	os.Exit(0)
}
