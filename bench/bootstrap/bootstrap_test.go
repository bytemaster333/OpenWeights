//go:build !a3probe
// +build !a3probe

package main

import (
	"context"
	"log/slog"
	"net/http"
	"net/http/httptest"
	"os"
	"path/filepath"
	"strings"
	"testing"
	"time"
)

// TestLoadWriteEnvRoundtrip: writeEnv produces 0600-perm file; loadEnv reads it back byte-for-byte.
func TestLoadWriteEnvRoundtrip(t *testing.T) {
	dir := t.TempDir()
	path := filepath.Join(dir, ".env")
	kv := map[string]string{
		"FOO":                    "bar",
		"SIAHUB_RECOVERY_PHRASE": "word1 word2 word3",
		"INDEXD_ADMIN_PASSWORD":  "abc123",
	}
	if err := writeEnv(path, kv); err != nil {
		t.Fatalf("writeEnv: %v", err)
	}
	info, err := os.Stat(path)
	if err != nil {
		t.Fatalf("stat: %v", err)
	}
	if info.Mode().Perm() != 0o600 {
		t.Fatalf("mode = %v, want 0600", info.Mode().Perm())
	}
	back, err := loadEnv(path)
	if err != nil {
		t.Fatalf("loadEnv: %v", err)
	}
	for k, v := range kv {
		if back[k] != v {
			t.Fatalf("roundtrip %s: got %q want %q", k, back[k], v)
		}
	}
}

// TestWriteEnvIdempotent: writing the same map twice produces byte-identical files.
func TestWriteEnvIdempotent(t *testing.T) {
	dir := t.TempDir()
	path := filepath.Join(dir, ".env")
	kv := map[string]string{"A": "1", "B": "2", "C": "3"}
	if err := writeEnv(path, kv); err != nil {
		t.Fatalf("write1: %v", err)
	}
	b1, _ := os.ReadFile(path)
	if err := writeEnv(path, kv); err != nil {
		t.Fatalf("write2: %v", err)
	}
	b2, _ := os.ReadFile(path)
	if string(b1) != string(b2) {
		t.Fatalf("byte-identity broken:\n%s\nvs\n%s", b1, b2)
	}
}

// TestFillMissing: existing values preserved; missing ones populated with 64-hex-char passwords.
func TestFillMissing(t *testing.T) {
	logger := slog.New(slog.NewJSONHandler(os.Stderr, nil))
	kv := map[string]string{"POSTGRES_SUPERUSER_PASSWORD": "existing"}
	populated := fillMissing(logger, kv)
	if len(populated) == 0 {
		t.Fatal("expected some passwords populated")
	}
	for _, k := range populated {
		if k == "POSTGRES_SUPERUSER_PASSWORD" {
			t.Fatal("should not overwrite existing password")
		}
	}
	if kv["POSTGRES_SUPERUSER_PASSWORD"] != "existing" {
		t.Fatal("existing password was overwritten")
	}
	if len(kv["INDEXD_ADMIN_PASSWORD"]) != 64 {
		t.Fatalf("generated password length = %d, want 64 (hex)", len(kv["INDEXD_ADMIN_PASSWORD"]))
	}
}

// TestRequireEnvMissing: requireEnv names the missing key in the returned error.
func TestRequireEnvMissing(t *testing.T) {
	kv := map[string]string{"A": "1"}
	err := requireEnv(kv, "A", "B", "C")
	if err == nil || !strings.Contains(err.Error(), "B") {
		t.Fatalf("expected error naming B; got %v", err)
	}
}

// TestRenderIndexdYML: template interpolation + 0600 perms on the rendered file.
func TestRenderIndexdYML(t *testing.T) {
	tmplDir := t.TempDir()
	tmpl := filepath.Join(tmplDir, "indexd.yml.tmpl")
	if err := os.WriteFile(tmpl, []byte("phrase={{ .RecoveryPhrase }}\nnet={{ .Network }}\n"), 0o644); err != nil {
		t.Fatal(err)
	}
	out := filepath.Join(tmplDir, "indexd.yml")
	err := renderIndexdYML(tmpl, out, indexdYMLData{RecoveryPhrase: "abc def", Network: "zen"})
	if err != nil {
		t.Fatalf("render: %v", err)
	}
	b, _ := os.ReadFile(out)
	if !strings.Contains(string(b), "phrase=abc def") || !strings.Contains(string(b), "net=zen") {
		t.Fatalf("rendered mismatch: %s", b)
	}
	info, _ := os.Stat(out)
	if info.Mode().Perm() != 0o600 {
		t.Fatalf("indexd.yml mode = %v, want 0600", info.Mode().Perm())
	}
}

// TestSha256Prefix8: first 8 hex chars of SHA-256 are stable; length is 8.
func TestSha256Prefix8(t *testing.T) {
	if sha256Prefix8("hello") != "2cf24dba" {
		t.Fatalf("sha prefix mismatch: got %q want 2cf24dba", sha256Prefix8("hello"))
	}
	if len(sha256Prefix8("anything")) != 8 {
		t.Fatalf("prefix length")
	}
}

// TestReadA3VerdictMissing: file absent (chdir'd into empty tempdir) -> a3Unknown.
func TestReadA3VerdictMissing(t *testing.T) {
	orig, _ := os.Getwd()
	tmp := t.TempDir()
	if err := os.Chdir(tmp); err != nil {
		t.Fatalf("chdir tmp: %v", err)
	}
	defer os.Chdir(orig)
	if readA3Verdict() != a3Unknown {
		t.Fatal("expected a3Unknown on missing file")
	}
}

// TestReadA3VerdictOptionA: OPTION_A_VIABLE in the markdown body -> a3OptionAViable.
func TestReadA3VerdictOptionA(t *testing.T) {
	orig, _ := os.Getwd()
	tmp := t.TempDir()
	if err := os.Chdir(tmp); err != nil {
		t.Fatalf("chdir tmp: %v", err)
	}
	defer os.Chdir(orig)
	if err := os.MkdirAll(filepath.Dir(a3VerdictPath), 0o755); err != nil {
		t.Fatalf("mkdir: %v", err)
	}
	if err := os.WriteFile(a3VerdictPath, []byte("**Verdict:** OPTION_A_VIABLE\n"), 0o644); err != nil {
		t.Fatalf("write: %v", err)
	}
	if readA3Verdict() != a3OptionAViable {
		t.Fatal("expected a3OptionAViable")
	}
}

// TestReadA3VerdictOptionBRequired: OPTION_A_UNAVAILABLE -> a3OptionAUnavailable.
func TestReadA3VerdictOptionBRequired(t *testing.T) {
	orig, _ := os.Getwd()
	tmp := t.TempDir()
	if err := os.Chdir(tmp); err != nil {
		t.Fatalf("chdir tmp: %v", err)
	}
	defer os.Chdir(orig)
	if err := os.MkdirAll(filepath.Dir(a3VerdictPath), 0o755); err != nil {
		t.Fatalf("mkdir: %v", err)
	}
	if err := os.WriteFile(a3VerdictPath, []byte("**Verdict:** OPTION_A_UNAVAILABLE\n"), 0o644); err != nil {
		t.Fatalf("write: %v", err)
	}
	if readA3Verdict() != a3OptionAUnavailable {
		t.Fatal("expected a3OptionAUnavailable")
	}
}

// TestReadA3VerdictPending: verdict file present but PENDING -> a3Unknown
// (falls back to Option B per parent-orchestrator guidance).
func TestReadA3VerdictPending(t *testing.T) {
	orig, _ := os.Getwd()
	tmp := t.TempDir()
	if err := os.Chdir(tmp); err != nil {
		t.Fatalf("chdir tmp: %v", err)
	}
	defer os.Chdir(orig)
	if err := os.MkdirAll(filepath.Dir(a3VerdictPath), 0o755); err != nil {
		t.Fatalf("mkdir: %v", err)
	}
	if err := os.WriteFile(a3VerdictPath, []byte("**Verdict:** PENDING\n"), 0o644); err != nil {
		t.Fatalf("write: %v", err)
	}
	if readA3Verdict() != a3Unknown {
		t.Fatal("expected a3Unknown on PENDING")
	}
}

// TestPollWalletFundedAlreadyFunded: the pre-check path returns immediately
// when confirmed >= threshold on the first fetch (no ticker wait).
func TestPollWalletFundedAlreadyFunded(t *testing.T) {
	srv := startWalletStubServer(t, `{"confirmed":"2000000000000000000000000","address":"addr:funded"}`, "pw")
	defer srv.Close()

	logger := slog.New(slog.NewJSONHandler(os.Stderr, nil))
	ctx, cancel := context.WithTimeout(context.Background(), 3*time.Second)
	defer cancel()
	// Threshold = 1 zSC; wallet returns 2 zSC — pre-check short-circuits.
	err := pollWalletFunded(ctx, logger, srv.URL, "pw", "1000000000000000000000000")
	if err != nil {
		t.Fatalf("pollWalletFunded: %v", err)
	}
}

// TestPollWalletFundedInvalidThreshold: a non-numeric threshold errors out
// before any HTTP call is made.
func TestPollWalletFundedInvalidThreshold(t *testing.T) {
	logger := slog.New(slog.NewJSONHandler(os.Stderr, nil))
	ctx, cancel := context.WithTimeout(context.Background(), 1*time.Second)
	defer cancel()
	err := pollWalletFunded(ctx, logger, "http://127.0.0.1:1", "pw", "notanumber")
	if err == nil || !strings.Contains(err.Error(), "invalid threshold") {
		t.Fatalf("expected invalid threshold error, got %v", err)
	}
}

// TestFetchWalletParses: fetchWallet decodes confirmed + address correctly.
func TestFetchWalletParses(t *testing.T) {
	srv := startWalletStubServer(t, `{"confirmed":"42","address":"addr:xyz"}`, "pw")
	defer srv.Close()
	ctx, cancel := context.WithTimeout(context.Background(), 2*time.Second)
	defer cancel()
	w, err := fetchWallet(ctx, srv.URL, "pw")
	if err != nil {
		t.Fatalf("fetchWallet: %v", err)
	}
	if w.Confirmed != "42" || w.Address != "addr:xyz" {
		t.Fatalf("unexpected wallet %+v", w)
	}
}

// startWalletStubServer returns an httptest.Server whose /wallet endpoint
// returns the provided JSON body when hit with HTTP Basic auth password `pw`.
// Strips a trailing /api prefix from the request path so callers can pass the
// server URL as-is (fetchWallet appends /wallet to the baseURL).
func startWalletStubServer(t *testing.T, body, pw string) *httptest.Server {
	t.Helper()
	mux := http.NewServeMux()
	mux.HandleFunc("/wallet", func(w http.ResponseWriter, r *http.Request) {
		_, gotPw, ok := r.BasicAuth()
		if !ok || gotPw != pw {
			http.Error(w, "unauthorized", http.StatusUnauthorized)
			return
		}
		w.Header().Set("Content-Type", "application/json")
		_, _ = w.Write([]byte(body))
	})
	return httptest.NewServer(mux)
}
