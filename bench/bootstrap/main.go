//go:build !a3probe
// +build !a3probe

// Package main — `make bootstrap` wizard per RESEARCH §10.
// From zero to a running SiaHub stack: generate BIP-39 if needed, derive App Key,
// bring Compose up, wait for indexd ready, fund wallet (manual step), run smoke.
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

	"github.com/siahub/bench/appid"
)

const (
	envPath          = ".env"
	indexdTmplPath   = "ops/indexd.yml.tmpl"
	indexdOutPath    = "ops/indexd.yml"
	composeFile      = "ops/docker-compose.yml"
	bootstrapTimeout = 60 * time.Minute // includes indexd sync + wallet funding
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
	if kv["SIAHUB_RECOVERY_PHRASE"] == "" {
		if !isTTY {
			_ = requireEnv(kv, "SIAHUB_RECOVERY_PHRASE")
			fmt.Fprintln(os.Stderr, "FATAL: non-TTY mode requires SIAHUB_RECOVERY_PHRASE in .env")
			os.Exit(2)
		}
		kv["SIAHUB_RECOVERY_PHRASE"] = siastorage.NewSeedPhrase()
		logger.Info("generated BIP-39 phrase", "phrase_sha_prefix", sha256Prefix8(kv["SIAHUB_RECOVERY_PHRASE"]))
	}
	if kv["SIAHUB_APP_ID"] == "" {
		kv["SIAHUB_APP_ID"] = appid.SiaHubAppID
	}
	if kv["SIAHUB_INDEXER_URL"] == "" {
		kv["SIAHUB_INDEXER_URL"] = "http://indexd:9982"
	}
	if kv["SIA_NETWORK"] == "" {
		kv["SIA_NETWORK"] = "zen"
	}
	if kv["SIAHUB_WALLET_THRESHOLD_HASTINGS"] == "" {
		kv["SIAHUB_WALLET_THRESHOLD_HASTINGS"] = "1000000000000000000000000" // 1 zSC
	}

	// 3. Generate missing passwords.
	_ = fillMissing(logger, kv)

	// 4. Persist.env (atomic).
	if err := writeEnv(envPath, kv); err != nil {
		logger.Error("writeEnv", "err", err)
		os.Exit(1)
	}
	logger.Info(".env written", "path", envPath)

	// 5. Render ops/indexd.yml.
	if err := renderIndexdYML(indexdTmplPath, indexdOutPath, indexdYMLData{
		RecoveryPhrase:   kv["SIAHUB_RECOVERY_PHRASE"],
		AdminPassword:    kv["INDEXD_ADMIN_PASSWORD"],
		PostgresPassword: kv["INDEXD_POSTGRES_PASSWORD"],
		Network:          kv["SIA_NETWORK"],
	}); err != nil {
		logger.Error("renderIndexdYML", "err", err)
		os.Exit(1)
	}
	logger.Info("ops/indexd.yml rendered")

	// 6. Compose up (postgres + redis first, then indexd).
	if err := runCompose("up", "-d", "postgres", "redis"); err != nil {
		logger.Error("compose up postgres/redis", "err", err)
		os.Exit(1)
	}
	if err := runCompose("up", "-d", "indexd"); err != nil {
		logger.Error("compose up indexd", "err", err)
		os.Exit(1)
	}
	logger.Info("compose up complete; indexd syncing (may take 10-30 min with --instant)")

	// 7. Wait for wallet (manual funding UX).
	ctx, cancel := context.WithTimeout(context.Background(), bootstrapTimeout)
	defer cancel()

	adminURL := "http://localhost:9980/api" // wizard runs on host; loopback port
	adminPass := kv["INDEXD_ADMIN_PASSWORD"]
	if err := pollWalletFunded(ctx, logger, adminURL, adminPass, kv["SIAHUB_WALLET_THRESHOLD_HASTINGS"]); err != nil {
		logger.Error("pollWalletFunded", "err", err)
		os.Exit(1)
	}

	// 8. Derive App Key (Option A or B per A3 verdict).
	indexerURL := "http://localhost:9982" // wizard runs on host
	appKeyHex, err := deriveAppKey(ctx, logger, indexerURL, adminURL, adminPass, kv["SIAHUB_RECOVERY_PHRASE"], kv["SIAHUB_APP_ID"])
	if err != nil {
		logger.Error("deriveAppKey", "err", err)
		os.Exit(1)
	}

	// 9. Persist SIAHUB_APP_KEY into.env.
	kv["SIAHUB_APP_KEY"] = appKeyHex
	if err := writeEnv(envPath, kv); err != nil {
		logger.Error("writeEnv (app key)", "err", err)
		os.Exit(1)
	}
	logger.Info("app key persisted", "key_sha_prefix", sha256Prefix8(appKeyHex))

	// 10. Run smoke test.
	logger.Info("running smoke test (1 MiB round-trip)")
	smokeCmd := exec.Command("go", "run", "./smoke")
	smokeCmd.Dir = "bench" // repo-root CWD; smoke package lives at bench/smoke
	smokeCmd.Env = append(os.Environ(),
		"SIAHUB_APP_ID="+kv["SIAHUB_APP_ID"],
		"SIAHUB_APP_KEY="+appKeyHex,
		"SIAHUB_INDEXER_URL="+indexerURL,
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
