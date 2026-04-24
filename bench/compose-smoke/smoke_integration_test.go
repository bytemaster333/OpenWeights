//go:build integration
// +build integration

// Package compose_smoke — integration test for the Compose stack healthcheck gating.
// Run via `make compose-smoke` or `cd bench && go test -tags=integration -timeout=35m ./compose-smoke/...`.
// requires a populated `.env` at repo root with INDEXD_ADMIN_PASSWORD set.
// The bootstrap wizard (PLAN 07) produces this; dev without bootstrap must set manually.
package compose_smoke

import (
	"bytes"
	"context"
	"encoding/json"
	"os"
	"os/exec"
	"path/filepath"
	"strings"
	"testing"
	"time"
)

const (
	composeFile  = "../../ops/docker-compose.yml"
	maxWaitTotal = 35 * time.Minute
	pollInterval = 30 * time.Second
)

func TestFullStackReady(t *testing.T) {
	if _, err := exec.LookPath("docker"); err != nil {
		t.Skip("docker not available on PATH")
	}
	if os.Getenv("INDEXD_ADMIN_PASSWORD") == "" {
		t.Skip("INDEXD_ADMIN_PASSWORD not set; skipping — bootstrap wizard must run first")
	}

	// Verify the readiness binary exists (cross-compiled in Task 3 Part A).
	readinessPath, _ := filepath.Abs("readiness/bin/readiness")
	if _, err := os.Stat(readinessPath); err != nil {
		t.Fatalf("readiness binary missing at %s — run `make build-readiness-probe` first", readinessPath)
	}

	// Start the stack (postgres + indexd + redis).
	t.Log("bringing Compose stack up...")
	composeUp := exec.Command("docker", "compose", "-f", composeFile, "up", "-d")
	composeUp.Stdout = os.Stdout
	composeUp.Stderr = os.Stderr
	if err := composeUp.Run(); err != nil {
		t.Fatalf("docker compose up failed: %v", err)
	}
	t.Cleanup(func() {
		down := exec.Command("docker", "compose", "-f", composeFile, "down")
		down.Stdout = os.Stdout
		down.Stderr = os.Stderr
		_ = down.Run()
	})

	// Poll healthcheck state every 30s up to 35 min.
	ctx, cancel := context.WithTimeout(context.Background(), maxWaitTotal)
	defer cancel()

	for {
		select {
		case <-ctx.Done():
			t.Fatalf("timed out after %s waiting for all services healthy", maxWaitTotal)
		case <-time.After(pollInterval):
		}

		if allHealthy(t) {
			t.Logf("all services healthy")
			return
		}
	}
}

// allHealthy queries `docker compose ps --format json` and returns true iff every
// service reports Health == "healthy" (or for services without healthcheck, State == "running").
func allHealthy(t *testing.T) bool {
	ps := exec.Command("docker", "compose", "-f", composeFile, "ps", "--format", "json")
	var out bytes.Buffer
	ps.Stdout = &out
	if err := ps.Run(); err != nil {
		t.Logf("compose ps failed: %v", err)
		return false
	}
	// `docker compose ps --format json` produces newline-delimited JSON since Compose v2.21+.
	services := parseComposePS(out.String())
	if len(services) == 0 {
		return false
	}
	for _, s := range services {
		if s.Health != "" && s.Health != "healthy" {
			t.Logf("service %s not healthy yet (health=%s, state=%s)", s.Name, s.Health, s.State)
			return false
		}
		if s.Health == "" && s.State != "running" {
			t.Logf("service %s not running yet (state=%s)", s.Name, s.State)
			return false
		}
	}
	return true
}

type composeService struct {
	Name   string `json:"Name"`
	State  string `json:"State"`
	Health string `json:"Health"`
}

func parseComposePS(raw string) []composeService {
	var services []composeService
	// Try newline-delimited JSON first (Compose v2.21+ default).
	for _, line := range strings.Split(strings.TrimSpace(raw), "\n") {
		line = strings.TrimSpace(line)
		if line == "" || !strings.HasPrefix(line, "{") {
			continue
		}
		var s composeService
		if err := json.Unmarshal([]byte(line), &s); err == nil {
			services = append(services, s)
		}
	}
	// Fall back to JSON array (older Compose).
	if len(services) == 0 {
		_ = json.Unmarshal([]byte(raw), &services)
	}
	return services
}
