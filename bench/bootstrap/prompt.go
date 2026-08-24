package main

import (
	"bufio"
	"fmt"
	"log/slog"
	"os"
	"strings"

	"go.sia.tech/siastorage"
)

// promptOperatorValues interactively fills the three operator-specific values
// that have no safe silent default: the recovery phrase, the indexer, and the
// console admin password. TTY only — non-TTY runs (CI, scripts) rely on .env
// being pre-filled and fall back to the callers' defaults.
func promptOperatorValues(logger *slog.Logger, kv map[string]string, isTTY bool) {
	if !isTTY {
		return
	}
	r := bufio.NewReader(os.Stdin)

	// recovery phrase — the one irreplaceable secret. losing it orphans every
	// stored byte, so we make the operator see it happen.
	if kv["OPENWEIGHTS_RECOVERY_PHRASE"] == "" {
		fmt.Fprintln(os.Stderr, "Sia recovery phrase — derives your App Key.")
		fmt.Fprintln(os.Stderr, "  KEEP IT SAFE: losing it permanently orphans every byte you store.")
		fmt.Fprint(os.Stderr, "  paste an existing BIP-39 phrase, or press Enter to generate a fresh one: ")
		if line := readLine(r); line != "" {
			kv["OPENWEIGHTS_RECOVERY_PHRASE"] = line
		} else {
			kv["OPENWEIGHTS_RECOVERY_PHRASE"] = siastorage.NewSeedPhrase()
			fmt.Fprintln(os.Stderr, "  generated a fresh phrase and saved it to .env — back that file up.")
			logger.Info("generated BIP-39 phrase", "phrase_sha_prefix", sha256Prefix8(kv["OPENWEIGHTS_RECOVERY_PHRASE"]))
		}
	}

	// indexer — hosted sia.storage by default (50 GB free, no wallet funding),
	// or the operator's own indexd URL.
	if kv["OPENWEIGHTS_INDEXER_URL"] == "" {
		fmt.Fprint(os.Stderr, "Indexer URL — Enter for https://sia.storage, or paste your own indexd URL: ")
		if line := readLine(r); line != "" {
			kv["OPENWEIGHTS_INDEXER_URL"] = line
		} else {
			kv["OPENWEIGHTS_INDEXER_URL"] = "https://sia.storage"
		}
	}

	// console admin password — enables password sign-in without a GitHub OAuth
	// app. Enter generates a strong one and prints it once.
	if kv["OPENWEIGHTS_ADMIN_PASSWORD"] == "" {
		fmt.Fprint(os.Stderr, "Console admin password — set one for password sign-in, or Enter to generate: ")
		if line := readLine(r); line != "" {
			kv["OPENWEIGHTS_ADMIN_PASSWORD"] = line
		} else {
			pw := generateRandomPassword()[:24]
			kv["OPENWEIGHTS_ADMIN_PASSWORD"] = pw
			fmt.Fprintf(os.Stderr, "  generated admin password (saved to .env): %s\n", pw)
		}
	}
}

// readLine reads one trimmed line from the reader; a read error yields "".
func readLine(r *bufio.Reader) string {
	line, _ := r.ReadString('\n')
	return strings.TrimSpace(line)
}
