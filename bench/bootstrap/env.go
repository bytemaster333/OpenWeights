package main

import (
	"bytes"
	"crypto/rand"
	"crypto/sha256"
	"encoding/base64"
	"encoding/hex"
	"fmt"
	"log/slog"
	"os"
	"path/filepath"
	"sort"
	"strings"
)

// loadEnv reads.env into a map; missing file -> empty map, no error.
func loadEnv(path string) (map[string]string, error) {
	kv := make(map[string]string)
	b, err := os.ReadFile(path)
	if err != nil {
		if os.IsNotExist(err) {
			return kv, nil
		}
		return nil, err
	}
	for _, line := range strings.Split(string(b), "\n") {
		line = strings.TrimSpace(line)
		if line == "" || strings.HasPrefix(line, "#") {
			continue
		}
		eq := strings.IndexByte(line, '=')
		if eq < 1 {
			continue
		}
		kv[strings.TrimSpace(line[:eq])] = parseEnvValue(line[eq+1:])
	}
	return kv, nil
}

// parseEnvValue extracts the value after `=`, honoring the dotenv convention
// that an inline comment starts at an unquoted ` #` (whitespace + hash). This
// matters because ops/.env.example documents vars as `KEY= # description`;
// without stripping, OPENWEIGHTS_APP_ID etc. would take the comment as value.
// A quoted value ("...") is returned verbatim minus the surrounding quotes.
func parseEnvValue(raw string) string {
	v := strings.TrimSpace(raw)
	if len(v) >= 2 && (v[0] == '"' || v[0] == '\'') && v[len(v)-1] == v[0] {
		return v[1 : len(v)-1]
	}
	// A value that is nothing but a comment (e.g. `KEY= # description`) is empty.
	if strings.HasPrefix(v, "#") {
		return ""
	}
	// Strip a trailing inline comment: first ` #` (space/tab before hash).
	for i := 1; i < len(v); i++ {
		if v[i] == '#' && (v[i-1] == ' ' || v[i-1] == '\t') {
			v = v[:i]
			break
		}
	}
	return strings.TrimSpace(v)
}

// writeEnv writes kv atomically (tmpfile + rename) with 0600 perms, sorted for determinism.
func writeEnv(path string, kv map[string]string) error {
	keys := make([]string, 0, len(kv))
	for k := range kv {
		keys = append(keys, k)
	}
	sort.Strings(keys)
	var buf bytes.Buffer
	fmt.Fprintln(&buf, "# OpenWeights .env — written by `make bootstrap`.")
	fmt.Fprintln(&buf, "# CRITICAL: loss of OPENWEIGHTS_RECOVERY_PHRASE = permanent data loss (STORE-04).")
	fmt.Fprintln(&buf, "# Per CONTEXT D-03, the phrase stays in this file permanently.")
	fmt.Fprintln(&buf)
	for _, k := range keys {
		fmt.Fprintf(&buf, "%s=%s\n", k, kv[k])
	}
	dir := filepath.Dir(path)
	tmp, err := os.CreateTemp(dir, ".env.tmp.*")
	if err != nil {
		return err
	}
	tmpPath := tmp.Name()
	if _, err := tmp.Write(buf.Bytes()); err != nil {
		tmp.Close()
		os.Remove(tmpPath)
		return err
	}
	if err := tmp.Chmod(0o600); err != nil {
		tmp.Close()
		os.Remove(tmpPath)
		return err
	}
	if err := tmp.Close(); err != nil {
		os.Remove(tmpPath)
		return err
	}
	return os.Rename(tmpPath, path)
}

// generateRandomPassword produces a 32-byte random hex string (64 chars).
func generateRandomPassword() string {
	var b [32]byte
	_, _ = rand.Read(b[:])
	return hex.EncodeToString(b[:])
}

// generateSigningKey produces a base64 (std) 32-byte key — the format the CAS
// and gateway expect for GATEWAY_URL_SIGNING_KEY (openssl rand 32 | base64).
func generateSigningKey() string {
	var b [32]byte
	_, _ = rand.Read(b[:])
	return base64.StdEncoding.EncodeToString(b[:])
}

// sha256Prefix8 returns the first 8 hex chars of SHA-256(s). Used for
// non-reversible log identifiers (recovery phrases, app keys).
func sha256Prefix8(s string) string {
	h := sha256.Sum256([]byte(s))
	return hex.EncodeToString(h[:])[:8]
}

// fillMissing generates values for any required secret that is empty.
// Returns the list of keys that were populated.
func fillMissing(logger *slog.Logger, kv map[string]string) []string {
	required := []string{
		"POSTGRES_SUPERUSER_PASSWORD",
		"OPENWEIGHTS_POSTGRES_PASSWORD",
		// : dedicated minimal-privilege role for openweights-gateway.
		// See cas/migrations/0005_openweights_gw_role.sql.
		"OPENWEIGHTS_GW_POSTGRES_PASSWORD",
		"REDIS_PASSWORD",
	}
	populated := []string{}
	for _, k := range required {
		if kv[k] == "" {
			kv[k] = generateRandomPassword()
			populated = append(populated, k)
			logger.Info("generated password", "key", k, "sha_prefix", sha256Prefix8(kv[k]))
		}
	}
	// base64 keys the CAS/gateway need. Generated here so a fresh operator's
	// stack boots and the HF round-trip works without hand-editing .env.
	//   GATEWAY_URL_SIGNING_KEY — MANDATORY; gateway won't boot if empty.
	//     shared by CAS (mints signed URLs) and gateway (verifies).
	//   XET_JWT_SIGNING_KEY — without it the HF-compat token endpoints 503,
	//     so `hf upload` / `hf download` fail.
	for _, k := range []string{"GATEWAY_URL_SIGNING_KEY", "XET_JWT_SIGNING_KEY"} {
		if kv[k] == "" {
			kv[k] = generateSigningKey()
			populated = append(populated, k)
			logger.Info("generated signing key", "key", k, "sha_prefix", sha256Prefix8(kv[k]))
		}
	}
	return populated
}

// requireEnv errors if any key is empty — used in non-TTY mode to fail-fast.
func requireEnv(kv map[string]string, keys ...string) error {
	var missing []string
	for _, k := range keys {
		if kv[k] == "" {
			missing = append(missing, k)
		}
	}
	if len(missing) > 0 {
		return fmt.Errorf("missing required env vars: %s", strings.Join(missing, ", "))
	}
	return nil
}
