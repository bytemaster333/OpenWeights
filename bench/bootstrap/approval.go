//go:build !a3probe
// +build !a3probe

package main

import (
	"bytes"
	"context"
	"encoding/json"
	"fmt"
	"log/slog"
	"net/http"
	"os"
	"strings"

	"go.sia.tech/core/types"
	"go.sia.tech/siastorage"
)

const a3VerdictPath = ".planning/phases/01-validation-foundations/01-A3-VERIFICATION.md"

type a3Verdict int

const (
	a3Unknown a3Verdict = iota
	a3OptionAViable
	a3OptionAUnavailable
)

// readA3Verdict reads the A3 verification markdown file and returns the
// recorded verdict. Missing file or unrecognized verdict -> a3Unknown.
func readA3Verdict() a3Verdict {
	b, err := os.ReadFile(a3VerdictPath)
	if err != nil {
		return a3Unknown
	}
	s := string(b)
	if strings.Contains(s, "OPTION_A_VIABLE") {
		return a3OptionAViable
	}
	if strings.Contains(s, "OPTION_A_UNAVAILABLE") {
		return a3OptionAUnavailable
	}
	return a3Unknown
}

// deriveAppKey chooses Option A or Option B by A3 verdict; returns App Key hex.
// If the verdict is a3Unknown (file missing or PENDING), falls back to Option B
// per parent-orchestrator guidance — the wizard must still be able to complete
// end-to-end; a live human can click the approve button in the indexd admin UI.
func deriveAppKey(ctx context.Context, logger *slog.Logger,
	indexerURL, adminURL, adminPass, phrase, appIDHex string) (string, error) {

	var appID types.Hash256
	if err := appID.UnmarshalText([]byte(appIDHex)); err != nil {
		return "", fmt.Errorf("invalid AppID hex: %w", err)
	}
	switch readA3Verdict() {
	case a3OptionAViable:
		logger.Info("A3: OPTION_A_VIABLE — using connect-key short-circuit")
		return deriveViaOptionA(ctx, logger, adminURL, adminPass, indexerURL, phrase, appID)
	case a3OptionAUnavailable:
		logger.Info("A3: OPTION_A_UNAVAILABLE — Option B manual approval")
		return deriveViaOptionB(ctx, logger, indexerURL, phrase, appID)
	default:
		// a3Unknown: PENDING verdict or missing file. Per parent-orchestrator
		// guidance, fall back to Option B (manual click) so the wizard can
		// still complete without a prior `make a3-verify` run.
		logger.Warn("A3 verification pending; using manual-click path",
			"verdict_file", a3VerdictPath,
			"hint", "run `make a3-verify` to resolve Option A vs B definitively")
		return deriveViaOptionB(ctx, logger, indexerURL, phrase, appID)
	}
}

func deriveViaOptionA(ctx context.Context, logger *slog.Logger,
	adminURL, adminPass, indexerURL, phrase string, appID types.Hash256) (string, error) {

	// (1) Mint a connect key via the admin API. This key is the "app password"
	// that approves the connection request below without a human clicking in the
	// indexd admin UI.
	connectKey, err := postConnectKey(adminURL, adminPass)
	if err != nil {
		return "", fmt.Errorf("connect-key creation: %w", err)
	}

	builder := siastorage.NewBuilder(indexerURL, siastorage.AppMetadata{
		ID:          appID,
		Name:        "openweights",
		Description: "Xet-on-Sia CAS infrastructure",
		ServiceURL:  "http://localhost:8080",
	})

	// (2) Request the connection; keep the responseURL so we can approve it.
	responseURL, err := builder.RequestConnection(ctx)
	if err != nil {
		return "", fmt.Errorf("RequestConnection: %w", err)
	}

	// (3) Approve the request programmatically: POST the responseURL with the
	// connect key as the basic-auth password (empty username) and body
	// {"approve": true}. This is what unblocks WaitForApproval — without it the
	// wizard would hang forever waiting for a human click.
	if err := approveConnection(ctx, responseURL, connectKey); err != nil {
		return "", fmt.Errorf("approve connection: %w", err)
	}

	// (4) Now WaitForApproval returns immediately, and Register derives the key.
	approved, err := builder.WaitForApproval(ctx)
	if err != nil {
		return "", fmt.Errorf("WaitForApproval: %w", err)
	}
	if !approved {
		return "", fmt.Errorf("approved=false after programmatic approval — check indexd admin API")
	}
	sdk, err := builder.Register(ctx, phrase)
	if err != nil {
		return "", fmt.Errorf("Register: %w", err)
	}
	defer sdk.Close()
	ak := sdk.AppKey()
	keyHex := hexEncode(ak)
	logger.Info("app key derived via Option A", "key_sha_prefix", sha256Prefix8(keyHex))
	return keyHex, nil
}

// deriveViaOptionB: human must approve in indexd admin UI. We drive the same
// Builder flow but print clear instructions and let WaitForApproval block.
func deriveViaOptionB(ctx context.Context, logger *slog.Logger,
	indexerURL, phrase string, appID types.Hash256) (string, error) {

	builder := siastorage.NewBuilder(indexerURL, siastorage.AppMetadata{
		ID:          appID,
		Name:        "openweights",
		Description: "Xet-on-Sia CAS infrastructure",
		ServiceURL:  "http://localhost:8080",
	})
	approvalURL, err := builder.RequestConnection(ctx)
	if err != nil {
		return "", fmt.Errorf("RequestConnection: %w", err)
	}
	fmt.Fprintln(os.Stderr, "")
	fmt.Fprintln(os.Stderr, "=== Manual approval required ===")
	fmt.Fprintf(os.Stderr, "Open this URL in your browser to approve:\n  %s\n", approvalURL)
	fmt.Fprintln(os.Stderr, "(Log in with the admin password from .env if prompted.)")
	fmt.Fprintln(os.Stderr, "The wizard will continue automatically once you click APPROVE.")
	fmt.Fprintln(os.Stderr, "")
	approved, err := builder.WaitForApproval(ctx)
	if err != nil {
		return "", fmt.Errorf("WaitForApproval (did you click APPROVE?): %w", err)
	}
	if !approved {
		return "", fmt.Errorf("approval not granted")
	}
	sdk, err := builder.Register(ctx, phrase)
	if err != nil {
		return "", fmt.Errorf("Register: %w", err)
	}
	defer sdk.Close()
	ak := sdk.AppKey()
	keyHex := hexEncode(ak)
	logger.Info("app key derived via Option B", "key_sha_prefix", sha256Prefix8(keyHex))
	return keyHex, nil
}

// postConnectKey mints an indexd connect key via the admin API and returns it.
//
// The request body MUST be {"description", "quota"} — indexd returns 404 for a
// body of {"appID", "quota"} (the connect key is not bound to an appID at
// creation; binding happens when the app uses the key to approve its request).
// "default" is the standard quota key. Verified empirically against
// ghcr.io/siafoundation/indexd (see A3 verification notes).
func postConnectKey(baseURL, pass string) (string, error) {
	body := map[string]any{"description": "openweights-bootstrap", "quota": "default"}
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

// approveConnection approves a pending app-connection request programmatically
// by POSTing {"approve": true} to the request's responseURL, authenticating
// with the connect key as the basic-auth password (empty username). Mirrors
// indexd's own api/app test helper. Expects 204 No Content.
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

// hexEncode converts a types.PrivateKey to lowercase hex. Uses a local
// implementation to keep this file SDK-agnostic in its encoding logic.
func hexEncode(pk types.PrivateKey) string {
	const h = "0123456789abcdef"
	b := make([]byte, len(pk)*2)
	for i, v := range pk {
		b[i*2] = h[v>>4]
		b[i*2+1] = h[v&0x0f]
	}
	return string(b)
}
