package main

import (
	"log/slog"
	"os"
	"path/filepath"
	"strings"
	"testing"
)

// TestLoadWriteEnvRoundtrip: writeEnv produces 0600-perm file; loadEnv reads it back byte-for-byte.
func TestLoadWriteEnvRoundtrip(t *testing.T) {
	dir := t.TempDir()
	path := filepath.Join(dir, ".env")
	kv := map[string]string{
		"FOO":                         "bar",
		"OPENWEIGHTS_RECOVERY_PHRASE": "word1 word2 word3",
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

// TestLoadEnvStripsInlineComments: ops/.env.example documents vars as
// `KEY= # description`; loadEnv must not take the comment as the value.
func TestLoadEnvStripsInlineComments(t *testing.T) {
	dir := t.TempDir()
	path := filepath.Join(dir, ".env")
	content := strings.Join([]string{
		"OPENWEIGHTS_APP_ID= # 32-byte hex constant; bootstrap reads from bench/appid",
		"OPENWEIGHTS_INDEXER_URL=https://sia.storage # hosted indexer",
		"REDIS_PASSWORD=abc#notacomment",
		`QUOTED="value # with hash"`,
		"PLAIN=simple",
	}, "\n")
	if err := os.WriteFile(path, []byte(content), 0o600); err != nil {
		t.Fatal(err)
	}
	kv, err := loadEnv(path)
	if err != nil {
		t.Fatalf("loadEnv: %v", err)
	}
	cases := map[string]string{
		"OPENWEIGHTS_APP_ID":     "",                    // empty → main.go fills the real id
		"OPENWEIGHTS_INDEXER_URL": "https://sia.storage", // trailing comment stripped
		"REDIS_PASSWORD":         "abc#notacomment",     // hash without leading space kept
		"QUOTED":                 "value # with hash",   // quoted value kept verbatim
		"PLAIN":                  "simple",
	}
	for k, want := range cases {
		if kv[k] != want {
			t.Errorf("%s = %q, want %q", k, kv[k], want)
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
	if len(kv["REDIS_PASSWORD"]) != 64 {
		t.Fatalf("generated password length = %d, want 64 (hex)", len(kv["REDIS_PASSWORD"]))
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

// TestSha256Prefix8: first 8 hex chars of SHA-256 are stable; length is 8.
func TestSha256Prefix8(t *testing.T) {
	if sha256Prefix8("hello") != "2cf24dba" {
		t.Fatalf("sha prefix mismatch: got %q want 2cf24dba", sha256Prefix8("hello"))
	}
	if len(sha256Prefix8("anything")) != 8 {
		t.Fatalf("prefix length")
	}
}
