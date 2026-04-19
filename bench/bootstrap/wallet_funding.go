//go:build !a3probe
// +build !a3probe

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

// walletState is the subset of indexd admin-API /wallet response we consume.
// confirmed is a hastings (1e24) string per Sia convention.
type walletState struct {
	Confirmed string `json:"confirmed"`
	Address   string `json:"address"`
}

// fetchWallet hits GET {adminURL}/wallet with HTTP Basic auth.
func fetchWallet(ctx context.Context, adminURL, adminPass string) (*walletState, error) {
	ctx, cancel := context.WithTimeout(ctx, 5*time.Second)
	defer cancel()
	req, _ := http.NewRequestWithContext(ctx, "GET", adminURL+"/wallet", nil)
	req.SetBasicAuth("", adminPass)
	resp, err := http.DefaultClient.Do(req)
	if err != nil {
		return nil, err
	}
	defer resp.Body.Close()
	if resp.StatusCode != 200 {
		return nil, fmt.Errorf("status %d", resp.StatusCode)
	}
	var w walletState
	if err := json.NewDecoder(resp.Body).Decode(&w); err != nil {
		return nil, err
	}
	return &w, nil
}

// pollWalletFunded blocks until confirmed >= threshold. Prints faucet
// instructions at start; polls every 15s until ctx cancels.
func pollWalletFunded(ctx context.Context, logger *slog.Logger,
	adminURL, adminPass, thresholdStr string) error {

	threshold, ok := new(big.Int).SetString(thresholdStr, 10)
	if !ok {
		return fmt.Errorf("invalid threshold %q", thresholdStr)
	}

	// Pre-check: if wallet already funded, skip the faucet prompt entirely.
	if w, err := fetchWallet(ctx, adminURL, adminPass); err == nil {
		if c, ok := new(big.Int).SetString(w.Confirmed, 10); ok && c.Cmp(threshold) >= 0 {
			logger.Info("wallet already funded", "confirmed", w.Confirmed, "address", w.Address)
			return nil
		}
	}

	addr := ""
	if w, err := fetchWallet(ctx, adminURL, adminPass); err == nil {
		addr = w.Address
	}
	fmt.Fprintln(os.Stderr, "")
	fmt.Fprintln(os.Stderr, "=== Fund the wallet (one-time human step) ===")
	fmt.Fprintf(os.Stderr, "Wallet address: %s\n", addr)
	fmt.Fprintln(os.Stderr, "Visit: https://zen.siascan.com/faucet")
	fmt.Fprintln(os.Stderr, "Paste the address above and request ~10 zSC.")
	fmt.Fprintln(os.Stderr, "")

	tick := time.NewTicker(15 * time.Second)
	defer tick.Stop()
	for {
		select {
		case <-ctx.Done():
			return ctx.Err()
		case <-tick.C:
		}
		w, err := fetchWallet(ctx, adminURL, adminPass)
		if err != nil {
			logger.Warn("wallet poll error", "err", err)
			continue
		}
		c, ok := new(big.Int).SetString(w.Confirmed, 10)
		if !ok {
			continue
		}
		if c.Cmp(threshold) >= 0 {
			logger.Info("wallet funded", "confirmed", w.Confirmed, "address", w.Address)
			return nil
		}
		logger.Info("wallet still funding", "confirmed", w.Confirmed, "threshold", thresholdStr, "address", w.Address)
	}
}
