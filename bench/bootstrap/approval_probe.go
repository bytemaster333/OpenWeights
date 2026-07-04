//go:build a3probe
// +build a3probe

// Package main — A3 verification probe.
// Built only under `-tags=a3probe`. Invoked once in Wave 2 to resolve RESEARCH §3
// Assumption A3: does admin-API connect-key pre-creation let Builder.WaitForApproval
// short-circuit, or does the bootstrap still need a manual approval click?
// Outcome written to
// PLAN 07's bootstrap wizard reads this file to choose Option A (connect-key) or
// Option B (re-implement deriveAppKey).
// Usage: cd bench && go run -tags=a3probe./bootstrap
package main

import (
	"bytes"
	"context"
	"encoding/json"
	"fmt"
	"log/slog"
	"net/http"
	"os"
	"time"

	"go.sia.tech/siastorage"
)

const verifyDocPath = ".planning/phases/01-validation-foundations/01-A3-VERIFICATION.md"

// main runs the A3 verification probe. Non-zero exit codes indicate inability to
// verify (e.g., indexd not reachable); 0 means "probe ran; verdict recorded in doc".
// The probe does NOT fail if A3 turns out false — it records the finding.
func main() {
	logger := slog.New(slog.NewJSONHandler(os.Stdout, nil))

	adminURL := envOr("INDEXD_ADMIN_URL", "http://localhost:9980/api")
	adminPass := os.Getenv("INDEXD_ADMIN_PASSWORD")
	indexerURL := envOr("OPENWEIGHTS_INDEXER_URL", "http://localhost:9982")
	phrase := os.Getenv("OPENWEIGHTS_RECOVERY_PHRASE")

	if adminPass == "" {
		logger.Error("INDEXD_ADMIN_PASSWORD not set")
		os.Exit(2)
	}
	if phrase == "" {
		phrase = siastorage.NewSeedPhrase()
		logger.Info("generated test phrase for A3 probe", "note", "phrase not persisted")
	}

	// Generate a fresh AppID for this probe — do NOT use the canonical OpenWeightsAppID,
	// because the probe registers an app and we don't want test registrations polluting
	// the production App ID registration set.
	appID := siastorage.GenerateAppID()

	// Step 1: Mint a connect key via admin API.
	connectKey, err := preCreateConnectKey(adminURL, adminPass)
	if err != nil {
		logger.Error("connect-key creation failed — A3 verdict: Option A unavailable", "err", err)
		writeVerdict(false, "admin API refused connect-key creation: "+err.Error())
		os.Exit(0)
	}
	logger.Info("connect key minted on admin API")

	// Step 2: Run Builder flow with a short timeout. We approve the request with
	// the connect key so WaitForApproval short-circuits; otherwise it blocks.
	ctx, cancel := context.WithTimeout(context.Background(), 30*time.Second)
	defer cancel()

	builder := siastorage.NewBuilder(indexerURL, siastorage.AppMetadata{
		ID: appID, Name: "openweights-a3-probe", Description: "Assumption A3 verification",
	})
	responseURL, err := builder.RequestConnection(ctx)
	if err != nil {
		logger.Error("RequestConnection failed", "err", err)
		writeVerdict(false, "RequestConnection error: "+err.Error())
		os.Exit(0)
	}
	if err := approveConnection(ctx, responseURL, connectKey); err != nil {
		logger.Error("approve connection failed", "err", err)
		writeVerdict(false, "approve POST failed: "+err.Error())
		os.Exit(0)
	}
	approved, err := builder.WaitForApproval(ctx)
	if err != nil {
		logger.Error("WaitForApproval errored", "err", err)
		writeVerdict(false, "WaitForApproval returned error (likely timed out waiting for manual click): "+err.Error())
		os.Exit(0)
	}
	if !approved {
		writeVerdict(false, "WaitForApproval returned approved=false within 30s")
		os.Exit(0)
	}

	// Success — Option A is viable.
	if _, err := builder.Register(ctx, phrase); err != nil {
		logger.Error("Register after approval failed", "err", err)
		writeVerdict(false, "Register failed after approval: "+err.Error())
		os.Exit(0)
	}

	writeVerdict(true, "connect-key pre-creation short-circuited WaitForApproval within 30s; Register succeeded")
	logger.Info("A3 probe PASS — Option A viable for PLAN 07")
	os.Exit(0)
}

// preCreateConnectKey mints a connect key via /api/apps/connect/keys. Body MUST
// be {"description","quota"} — {"appID",...} returns 404 (the key is bound to an
// app only when used for approval, not at creation). Returns the key.
func preCreateConnectKey(baseURL, pass string) (string, error) {
	body := map[string]any{"description": "openweights-a3-probe", "quota": "default"}
	b, _ := json.Marshal(body)
	req, _ := http.NewRequest("POST", baseURL+"/apps/connect/keys", bytes.NewReader(b))
	req.SetBasicAuth("", pass)
	req.Header.Set("Content-Type", "application/json")
	resp, err := http.DefaultClient.Do(req)
	if err != nil {
		return "", err
	}
	defer resp.Body.Close()
	if resp.StatusCode >= 400 {
		return "", fmt.Errorf("POST /apps/connect/keys returned %d", resp.StatusCode)
	}
	var out struct {
		Key string `json:"key"`
	}
	if err := json.NewDecoder(resp.Body).Decode(&out); err != nil {
		return "", fmt.Errorf("decode connect-key response: %w", err)
	}
	if out.Key == "" {
		return "", fmt.Errorf("connect-key response had empty key")
	}
	return out.Key, nil
}

// approveConnection approves a pending request by POSTing {"approve":true} to
// the responseURL, authenticating with the connect key (basic-auth password).
func approveConnection(ctx context.Context, responseURL, connectKey string) error {
	b, _ := json.Marshal(map[string]bool{"approve": true})
	req, err := http.NewRequestWithContext(ctx, http.MethodPost, responseURL, bytes.NewReader(b))
	if err != nil {
		return err
	}
	req.SetBasicAuth("", connectKey)
	req.Header.Set("Content-Type", "application/json")
	resp, err := http.DefaultClient.Do(req)
	if err != nil {
		return err
	}
	defer resp.Body.Close()
	if resp.StatusCode >= 400 {
		return fmt.Errorf("approve POST %s returned %d", responseURL, resp.StatusCode)
	}
	return nil
}

func writeVerdict(viable bool, detail string) {
	verdict := "OPTION_A_VIABLE"
	if !viable {
		verdict = "OPTION_A_UNAVAILABLE"
	}
	content := fmt.Sprintf(`# A3 Verification Result

**Date:** %s
**Verdict:** %s
**Detail:** %s

## Implication for PLAN 07

`,
		time.Now().UTC().Format(time.RFC3339),
		verdict, detail,
	)
	if viable {
		content += `Option A (RESEARCH §3) is viable. PLAN 07's bootstrap wizard MUST use the connect-key pre-creation flow:
  1. POST /api/apps/connect/keys with admin auth
  2. Drive siastorage.Builder.RequestConnection -> WaitForApproval -> Register(phrase)
  3. Write derived App Key hex to .env
`
	} else {
		content += `Option A (RESEARCH §3) is NOT viable. PLAN 07's bootstrap wizard MUST fall back to Option B:
  1. Import go.sia.tech/coreutils/wallet + go.sia.tech/indexd/keys
  2. Compute seed := wallet.SeedFromPhrase(phrase); appKey := types.NewPrivateKeyFromSeed(keys.Derive(append(seed, sharedSecret...), appID, "indexd app key derivation", 32))
  3. POST /api/apps/register with admin auth, attaching the derived app public key
  4. Write App Key hex to .env
Human operator must perform a manual approval click in the indexd admin UI on first run; document this in bootstrap wizard stderr.
`
	}
	_ = os.WriteFile(verifyDocPath, []byte(content), 0o644)
}

func envOr(k, d string) string {
	if v := os.Getenv(k); v != "" {
		return v
	}
	return d
}
