package main

import (
	"context"
	"encoding/json"
	"fmt"
	"net/http"
	"net/http/httptest"
	"strings"
	"testing"
)

// fakeIndexd serves canned /state and /wallet responses.
type fakeIndexd struct {
	synced    bool
	confirmed string
	gotAuth   string // captures the Authorization header seen
}

func (f *fakeIndexd) handler() http.Handler {
	mux := http.NewServeMux()
	mux.HandleFunc("/state", func(w http.ResponseWriter, r *http.Request) {
		f.gotAuth = r.Header.Get("Authorization")
		_ = json.NewEncoder(w).Encode(map[string]any{
			"synced":     f.synced,
			"scanHeight": 12345,
			"network":    "zen",
		})
	})
	mux.HandleFunc("/wallet", func(w http.ResponseWriter, r *http.Request) {
		_ = json.NewEncoder(w).Encode(map[string]any{
			"confirmed":   f.confirmed,
			"spendable":   f.confirmed,
			"unconfirmed": "0",
			"immature":    "0",
			"address":     "addr:testonly",
		})
	})
	return mux
}

func newFake(synced bool, confirmed string) (*httptest.Server, *fakeIndexd) {
	f := &fakeIndexd{synced: synced, confirmed: confirmed}
	return httptest.NewServer(f.handler()), f
}

func TestHealthy(t *testing.T) {
	srv, _ := newFake(true, "2000000000000000000000000") // 2 zSC (above 1 zSC threshold)
	defer srv.Close()
	rc := run(context.Background(), srv.URL, "testpass", "1000000000000000000000000")
	if rc != 0 {
		t.Fatalf("healthy case expected rc=0, got rc=%d", rc)
	}
}

func TestUnsynced(t *testing.T) {
	srv, _ := newFake(false, "2000000000000000000000000")
	defer srv.Close()
	rc := run(context.Background(), srv.URL, "testpass", "1000000000000000000000000")
	if rc == 0 {
		t.Fatalf("unsynced case expected rc!=0, got rc=0")
	}
}

func TestUnderfunded(t *testing.T) {
	srv, _ := newFake(true, "500000000000000000000000") // 0.5 zSC < 1 zSC threshold
	defer srv.Close()
	rc := run(context.Background(), srv.URL, "testpass", "1000000000000000000000000")
	if rc == 0 {
		t.Fatalf("underfunded case expected rc!=0, got rc=0")
	}
}

func TestThresholdParsing(t *testing.T) {
	// 10^24 exceeds int64 max (~9.2 * 10^18) — MUST use big.Int for comparison.
	// Confirm that 10^24 >= 10^24 passes and 10^24 - 1 < 10^24 fails.
	srv, _ := newFake(true, "1000000000000000000000000") // exactly threshold
	defer srv.Close()
	rc := run(context.Background(), srv.URL, "testpass", "1000000000000000000000000")
	if rc != 0 {
		t.Fatalf("confirmed==threshold expected rc=0, got rc=%d", rc)
	}
	srv2, _ := newFake(true, "999999999999999999999999") // one hasting below threshold
	defer srv2.Close()
	rc2 := run(context.Background(), srv2.URL, "testpass", "1000000000000000000000000")
	if rc2 == 0 {
		t.Fatalf("confirmed<threshold expected rc!=0, got rc=0")
	}
}

func TestAuthHeader(t *testing.T) {
	srv, f := newFake(true, "2000000000000000000000000")
	defer srv.Close()
	_ = run(context.Background(), srv.URL, "testpass", "1000000000000000000000000")
	// HTTP Basic with empty user and pass "testpass" = base64(":testpass")
	// base64(":testpass") = "OnRlc3RwYXNz"
	want := "Basic OnRlc3RwYXNz"
	if !strings.EqualFold(f.gotAuth, want) {
		t.Fatalf("auth header mismatch: got %q, want %q", f.gotAuth, want)
	}
}

// Guardrail: ensure main wires run and returns its exit code.
// Compile-time check via interface assertion (no runtime exec).
var _ = func() int {
	// if this compiles, run(ctx, baseURL, pass, threshold) int exists with the right signature
	type runner func(ctx context.Context, baseURL, pass, threshold string) int
	var _ runner = run
	return 0
}

// helper: silence unused-import warnings if future refactors drop some
var _ = fmt.Sprintf
