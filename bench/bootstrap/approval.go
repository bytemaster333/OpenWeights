package main

import (
	"context"
	"fmt"
	"log/slog"
	"os"

	"go.sia.tech/core/types"
	"go.sia.tech/siastorage"
)

// deriveAppKey registers the app against the hosted indexer and returns the
// App Key hex. The hosted indexer has no admin API we can drive, so the
// operator approves the connection by opening the printed URL; there is no
// programmatic short-circuit.
func deriveAppKey(ctx context.Context, logger *slog.Logger,
	indexerURL, phrase, appIDHex string) (string, error) {

	var appID types.Hash256
	if err := appID.UnmarshalText([]byte(appIDHex)); err != nil {
		return "", fmt.Errorf("invalid AppID hex: %w", err)
	}
	return deriveViaManualApproval(ctx, logger, indexerURL, phrase, appID)
}

// deriveViaManualApproval drives the Builder flow and blocks on the operator
// clicking APPROVE at the printed URL, then registers and returns the key.
func deriveViaManualApproval(ctx context.Context, logger *slog.Logger,
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
	logger.Info("app key derived", "key_sha_prefix", sha256Prefix8(keyHex))
	return keyHex, nil
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
