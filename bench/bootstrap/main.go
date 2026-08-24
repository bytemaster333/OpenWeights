// Package main — `make bootstrap` wizard per RESEARCH §10.
// From zero to a running OpenWeights stack: generate BIP-39 if needed, register
// the app against the configured indexer, derive the App Key, bring the local
// supporting services up, and run the smoke round-trip.
package main

import (
	"context"
	"fmt"
	"log/slog"
	"os"
	"os/exec"
	"time"

	"github.com/mattn/go-isatty"
	"go.sia.tech/siastorage"

	"github.com/bytemaster333/openweights/bench/appid"
)

const (
	envPath          = ".env"
	composeFile      = "ops/docker-compose.yml"
	bootstrapTimeout = 60 * time.Minute // manual app-approval click can take a while
)

func main() {
	logger := slog.New(slog.NewJSONHandler(os.Stdout, nil))
	isTTY := isatty.IsTerminal(os.Stdin.Fd())

	// 1. Load.env (or empty map).
	kv, err := loadEnv(envPath)
	if err != nil {
		logger.Error("loadEnv", "err", err)
		os.Exit(1)
	}

	// 2. Set defaults + generate BIP-39 if phrase empty.
	if kv["OPENWEIGHTS_RECOVERY_PHRASE"] == "" {
		if !isTTY {
			_ = requireEnv(kv, "OPENWEIGHTS_RECOVERY_PHRASE")
			fmt.Fprintln(os.Stderr, "FATAL: non-TTY mode requires OPENWEIGHTS_RECOVERY_PHRASE in .env")
			os.Exit(2)
		}
		kv["OPENWEIGHTS_RECOVERY_PHRASE"] = siastorage.NewSeedPhrase()
		logger.Info("generated BIP-39 phrase", "phrase_sha_prefix", sha256Prefix8(kv["OPENWEIGHTS_RECOVERY_PHRASE"]))
	}
	if kv["OPENWEIGHTS_APP_ID"] == "" {
		kv["OPENWEIGHTS_APP_ID"] = appid.OpenWeightsAppID
	}
	if kv["OPENWEIGHTS_INDEXER_URL"] == "" {
		// external hosted indexer; self-hosted indexd is gone.
		kv["OPENWEIGHTS_INDEXER_URL"] = "https://sia.storage"
	}

	// 3. Generate missing passwords.
	_ = fillMissing(logger, kv)

	// 4. Persist.env (atomic).
	if err := writeEnv(envPath, kv); err != nil {
		logger.Error("writeEnv", "err", err)
		os.Exit(1)
	}
	logger.Info(".env written", "path", envPath)

	// 5. Bring up the local supporting services (postgres + redis).
	if err := runCompose("up", "-d", "postgres", "redis"); err != nil {
		logger.Error("compose up postgres/redis", "err", err)
		os.Exit(1)
	}
	logger.Info("compose up complete")

	// 6. Register the app against the indexer + derive the App Key.
	ctx, cancel := context.WithTimeout(context.Background(), bootstrapTimeout)
	defer cancel()

	indexerURL := kv["OPENWEIGHTS_INDEXER_URL"]
	appKeyHex, err := deriveAppKey(ctx, logger, indexerURL, kv["OPENWEIGHTS_RECOVERY_PHRASE"], kv["OPENWEIGHTS_APP_ID"])
	if err != nil {
		logger.Error("deriveAppKey", "err", err)
		os.Exit(1)
	}

	// 7. Persist OPENWEIGHTS_APP_KEY into.env.
	kv["OPENWEIGHTS_APP_KEY"] = appKeyHex
	if err := writeEnv(envPath, kv); err != nil {
		logger.Error("writeEnv (app key)", "err", err)
		os.Exit(1)
	}
	logger.Info("app key persisted", "key_sha_prefix", sha256Prefix8(appKeyHex))

	// 8. Run smoke test.
	logger.Info("running smoke test (1 MiB round-trip)")
	smokeCmd := exec.Command("go", "run", "./smoke")
	smokeCmd.Dir = "bench" // repo-root CWD; smoke package lives at bench/smoke
	smokeCmd.Env = append(os.Environ(),
		"OPENWEIGHTS_APP_ID="+kv["OPENWEIGHTS_APP_ID"],
		"OPENWEIGHTS_APP_KEY="+appKeyHex,
		"OPENWEIGHTS_INDEXER_URL="+indexerURL,
	)
	smokeCmd.Stdout = os.Stdout
	smokeCmd.Stderr = os.Stderr
	if err := smokeCmd.Run(); err != nil {
		fmt.Fprintln(os.Stderr, "bootstrap: FAIL — smoke test returned non-zero")
		os.Exit(1)
	}

	fmt.Fprintln(os.Stderr, "bootstrap: PASS — stack ready; smoke test succeeded.")
}

func runCompose(args ...string) error {
	full := append([]string{"compose", "-f", composeFile, "--env-file", envPath}, args...)
	cmd := exec.Command("docker", full...)
	cmd.Stdout = os.Stdout
	cmd.Stderr = os.Stderr
	return cmd.Run()
}
