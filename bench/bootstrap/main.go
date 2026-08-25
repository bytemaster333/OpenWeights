// Package main — `make bootstrap` wizard.
// From zero to a running OpenWeights stack: fill .env (secrets + keys), register
// the app on the configured indexer via the Rust openweights-cas-register CLI
// (the only source of a CAS-compatible App Key), then bring the full stack up.
package main

import (
	"context"
	"fmt"
	"log/slog"
	"os"
	"os/exec"
	"strings"
	"time"

	"github.com/mattn/go-isatty"

	"github.com/bytemaster333/openweights/bench/appid"
)

const (
	envPath          = ".env"
	composeFile      = "ops/docker-compose.yml"
	bootstrapTimeout = 60 * time.Minute // manual app-approval click can take a while
)

func main() {
	// `-env-only` (used by `make setup`) writes .env and stops — no stack
	// bring-up, no app registration, no smoke test.
	envOnly := false
	for _, a := range os.Args[1:] {
		if a == "-env-only" || a == "--env-only" {
			envOnly = true
		}
	}

	logger := slog.New(slog.NewJSONHandler(os.Stdout, nil))
	isTTY := isatty.IsTerminal(os.Stdin.Fd())

	// 1. Load .env (or empty map).
	kv, err := loadEnv(envPath)
	if err != nil {
		logger.Error("loadEnv", "err", err)
		os.Exit(1)
	}

	// 2. Non-TTY has nothing to prompt — require the phrase to be pre-set.
	if !isTTY && kv["OPENWEIGHTS_RECOVERY_PHRASE"] == "" {
		fmt.Fprintln(os.Stderr, "FATAL: non-TTY mode requires OPENWEIGHTS_RECOVERY_PHRASE in .env")
		os.Exit(2)
	}

	// 3. Interactively fill the operator's choices (phrase, indexer, admin
	//    password) on a TTY.
	promptOperatorValues(logger, kv, isTTY)

	// 4. Non-interactive defaults for anything still empty + generate the
	//    infrastructure passwords.
	if kv["OPENWEIGHTS_APP_ID"] == "" {
		kv["OPENWEIGHTS_APP_ID"] = appid.OpenWeightsAppID
	}
	if kv["OPENWEIGHTS_INDEXER_URL"] == "" {
		kv["OPENWEIGHTS_INDEXER_URL"] = "https://sia.storage"
	}
	// Client-reachable CAS URL. The base compose publishes the CAS on
	// localhost:8080; hf-proxy hard-requires OPENWEIGHTS_CAS_PUBLIC_URL and the
	// CAS stamps CAS_PUBLIC_URL into xet-write-token responses. Operators with a
	// domain override both in .env; this default makes `docker compose up` work
	// out of the box.
	for _, k := range []string{"OPENWEIGHTS_CAS_PUBLIC_URL", "CAS_PUBLIC_URL"} {
		if kv[k] == "" {
			kv[k] = "http://localhost:8080"
		}
	}
	_ = fillMissing(logger, kv)

	// 5. Persist .env (atomic, 0600).
	if err := writeEnv(envPath, kv); err != nil {
		logger.Error("writeEnv", "err", err)
		os.Exit(1)
	}
	logger.Info(".env written", "path", envPath)

	// `make setup` stops here — the .env is ready to use.
	if envOnly {
		fmt.Fprintln(os.Stderr, "setup: .env written. Next: `docker compose -f ops/docker-compose.yml up -d`")
		fmt.Fprintln(os.Stderr, "       (or `make bootstrap` to also register the app on the indexer + smoke-test).")
		return
	}

	// 6. Register the app on the indexer + derive the App Key.
	// This MUST use the Rust `openweights-cas-register` CLI, not the Go SDK:
	// the two derive different (non-interoperable) keys, and the CAS only
	// accepts the Rust one (base64 of the 32-byte seed). Running the Go
	// derivation here produced a key the CAS rejected at boot.
	appKeyB64, err := registerAppKey(kv)
	if err != nil {
		logger.Error("register app key", "err", err)
		os.Exit(1)
	}

	// 7. Persist OPENWEIGHTS_APP_KEY into .env.
	kv["OPENWEIGHTS_APP_KEY"] = appKeyB64
	if err := writeEnv(envPath, kv); err != nil {
		logger.Error("writeEnv (app key)", "err", err)
		os.Exit(1)
	}
	logger.Info("app key persisted", "key_sha_prefix", sha256Prefix8(appKeyB64))

	// 8. Bring the full stack up.
	if err := runCompose("up", "-d"); err != nil {
		logger.Error("compose up (full stack)", "err", err)
		os.Exit(1)
	}
	fmt.Fprintln(os.Stderr, "bootstrap: PASS — .env written, app registered, stack up.")
	fmt.Fprintln(os.Stderr, "  console: http://localhost:5173  (sign in with OPENWEIGHTS_ADMIN_PASSWORD from .env)")
}

// registerAppKey runs the Rust `openweights-cas-register` CLI, which drives the
// indexer app-approval flow (prints an approval URL, blocks until the operator
// clicks APPROVE) and prints `OPENWEIGHTS_APP_KEY=<base64>` on stdout. Its
// stderr is streamed so the operator sees the approval URL. Returns the base64
// key. Requires the Rust toolchain (cargo) on the operator's machine.
func registerAppKey(kv map[string]string) (string, error) {
	ctx, cancel := context.WithTimeout(context.Background(), bootstrapTimeout)
	defer cancel()

	cmd := exec.CommandContext(ctx, "cargo", "run", "--release", "--quiet",
		"-p", "openweights-cas-register")
	cmd.Dir = "cas"
	cmd.Env = append(os.Environ(),
		"OPENWEIGHTS_APP_ID="+kv["OPENWEIGHTS_APP_ID"],
		"OPENWEIGHTS_INDEXER_URL="+kv["OPENWEIGHTS_INDEXER_URL"],
		"OPENWEIGHTS_RECOVERY_PHRASE="+kv["OPENWEIGHTS_RECOVERY_PHRASE"],
	)
	cmd.Stderr = os.Stderr // approval URL + progress reach the operator
	out, err := cmd.Output()
	if err != nil {
		return "", fmt.Errorf("openweights-cas-register: %w", err)
	}
	for _, line := range strings.Split(string(out), "\n") {
		if v, ok := strings.CutPrefix(strings.TrimSpace(line), "OPENWEIGHTS_APP_KEY="); ok {
			return strings.TrimSpace(v), nil
		}
	}
	return "", fmt.Errorf("register CLI did not print OPENWEIGHTS_APP_KEY")
}

func runCompose(args ...string) error {
	full := append([]string{"compose", "-f", composeFile, "--env-file", envPath}, args...)
	cmd := exec.Command("docker", full...)
	cmd.Stdout = os.Stdout
	cmd.Stderr = os.Stderr
	return cmd.Run()
}
