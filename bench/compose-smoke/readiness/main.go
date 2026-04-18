// Package main — static readiness probe for the SiaHub indexd container.
// Bind-mounted as /usr/local/bin/siahub-readiness; Docker invokes it as the healthcheck.
//
// Exits 0 iff both:
//  1. indexd's consensus is synced (/api/state.synced == true)
//  2. indexd's wallet confirmed balance >= threshold hastings (/api/wallet.confirmed)
//
// See .planning/phases/01-validation-foundations/01-RESEARCH.md §4-5 for the authoritative spec.
package main

import (
	"context"
	"encoding/json"
	"fmt"
	"log/slog"
	"math/big"
	"net/http"
	"os"
	"time"
)

const (
	defaultAdminURL   = "http://indexd:9980/api"
	defaultThreshold  = "1000000000000000000000000" // 1 zSC = 10^24 hastings
	perRequestTimeout = 5 * time.Second
)

type stateResp struct {
	Synced     bool   `json:"synced"`
	ScanHeight uint64 `json:"scanHeight"`
	Network    string `json:"network"`
}

type walletResp struct {
	Confirmed   string `json:"confirmed"` // Currency: decimal string (hastings)
	Spendable   string `json:"spendable"`
	Unconfirmed string `json:"unconfirmed"`
	Address     string `json:"address"`
}

// run is the probe body; separated from main() so unit tests exercise it directly.
// Returns 0 on healthy, non-zero on any failure.
func run(ctx context.Context, baseURL, pass, thresholdStr string) int {
	logger := slog.New(slog.NewJSONHandler(os.Stderr, nil))

	threshold, ok := new(big.Int).SetString(thresholdStr, 10)
	if !ok {
		logger.Error("invalid threshold", "value", thresholdStr)
		return 10
	}

	// 1. Consensus sync check.
	var s stateResp
	if err := getJSON(ctx, baseURL+"/state", pass, &s); err != nil {
		logger.Error("state endpoint failed", "err", err, "url", baseURL+"/state")
		return 11
	}
	if !s.Synced {
		logger.Info("indexd not synced yet", "scanHeight", s.ScanHeight, "network", s.Network)
		return 12
	}

	// 2. Wallet balance check.
	var w walletResp
	if err := getJSON(ctx, baseURL+"/wallet", pass, &w); err != nil {
		logger.Error("wallet endpoint failed", "err", err, "url", baseURL+"/wallet")
		return 13
	}
	confirmed, ok := new(big.Int).SetString(w.Confirmed, 10)
	if !ok {
		logger.Error("wallet.confirmed not a decimal string", "value", w.Confirmed)
		return 14
	}
	if confirmed.Cmp(threshold) < 0 {
		logger.Info("wallet underfunded",
			"confirmed_hastings", w.Confirmed,
			"threshold_hastings", thresholdStr,
			"address", w.Address,
			"faucet", "https://zen.siascan.com/faucet",
		)
		return 15
	}

	// Healthy.
	logger.Info("indexd ready",
		"synced", true,
		"confirmed_hastings", w.Confirmed,
		"scanHeight", s.ScanHeight,
	)
	return 0
}

// getJSON performs a GET with HTTP Basic auth (empty user, provided password)
// and decodes the body into out. Per-request timeout enforced.
func getJSON(ctx context.Context, url, pass string, out any) error {
	ctx, cancel := context.WithTimeout(ctx, perRequestTimeout)
	defer cancel()
	req, err := http.NewRequestWithContext(ctx, "GET", url, nil)
	if err != nil {
		return fmt.Errorf("build request: %w", err)
	}
	req.SetBasicAuth("", pass)
	resp, err := http.DefaultClient.Do(req)
	if err != nil {
		return fmt.Errorf("do request: %w", err)
	}
	defer resp.Body.Close()
	if resp.StatusCode != http.StatusOK {
		return fmt.Errorf("status %d", resp.StatusCode)
	}
	return json.NewDecoder(resp.Body).Decode(out)
}

func envOr(k, d string) string {
	if v := os.Getenv(k); v != "" {
		return v
	}
	return d
}

func main() {
	baseURL := envOr("INDEXD_ADMIN_URL", defaultAdminURL)
	pass := os.Getenv("INDEXD_ADMIN_PASSWORD")
	if pass == "" {
		// No admin password set — refuse to run; clear signal for operator debugging.
		fmt.Fprintln(os.Stderr, "readiness: INDEXD_ADMIN_PASSWORD not set")
		os.Exit(2)
	}
	threshold := envOr("SIAHUB_WALLET_THRESHOLD_HASTINGS", defaultThreshold)

	ctx, cancel := context.WithTimeout(context.Background(), 2*perRequestTimeout+time.Second)
	defer cancel()
	os.Exit(run(ctx, baseURL, pass, threshold))
}
