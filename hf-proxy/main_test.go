package main

import (
	"io"
	"net/http"
	"net/http/httptest"
	"net/url"
	"strings"
	"testing"
)

// fakeHF returns a canned HF-like response with an X-Xet-Cas-Url header and
// a body that the proxy MUST forward unchanged.
func fakeHF(t *testing.T, hfCasURL string, body string) *httptest.Server {
	t.Helper()
	return httptest.NewServer(http.HandlerFunc(func(w http.ResponseWriter, r *http.Request) {
		w.Header().Set("X-Xet-Cas-Url", hfCasURL)
		w.Header().Set("X-Xet-Access-Token", "hf-test-token")
		w.Header().Set("X-Xet-Expiration", "1234567890")
		w.Header().Set("Content-Type", "application/json")
		w.WriteHeader(http.StatusOK)
		_, _ = io.WriteString(w, body)
	}))
}

func buildProxy(t *testing.T, upstream *url.URL, casPublic string) http.Handler {
	t.Helper()
	cfg := &config{
		listenAddr:   ":0",
		upstreamURL:  upstream,
		casPublicURL: casPublic,
	}
	return newProxy(cfg)
}

// Core guarantee: when HF emits X-Xet-Cas-Url, the proxy replaces it with
// the configured OpenWeights value. Anything else would leave bytes going to
// HF's CAS, which is exactly the bug we're fixing.
func TestRewritesXetCasUrl(t *testing.T) {
	hf := fakeHF(t, "https://cas-server.xethub.hf.co", `{"commitOid":"abc"}`)
	defer hf.Close()

	up, _ := url.Parse(hf.URL)
	proxy := buildProxy(t, up, "http://openweights-cas:28080")

	rec := httptest.NewRecorder()
	req := httptest.NewRequest(http.MethodGet, "/api/models/user/repo/xet-write-token/main", nil)
	proxy.ServeHTTP(rec, req)

	if got := rec.Header().Get("X-Xet-Cas-Url"); got != "http://openweights-cas:28080" {
		t.Fatalf("X-Xet-Cas-Url = %q; want http://openweights-cas:28080", got)
	}
}

// If HF doesn't emit X-Xet-Cas-Url (not every endpoint does), the proxy
// MUST NOT invent one. Silent injection could break LFS fallback paths.
func TestLeavesHeaderAloneWhenAbsent(t *testing.T) {
	hf := httptest.NewServer(http.HandlerFunc(func(w http.ResponseWriter, r *http.Request) {
 // No X-Xet-Cas-Url header.
		w.WriteHeader(http.StatusOK)
		_, _ = io.WriteString(w, `{"ok":true}`)
	}))
	defer hf.Close()

	up, _ := url.Parse(hf.URL)
	proxy := buildProxy(t, up, "http://openweights-cas:28080")

	rec := httptest.NewRecorder()
	req := httptest.NewRequest(http.MethodGet, "/api/whoami-v2", nil)
	proxy.ServeHTTP(rec, req)

	if got := rec.Header().Get("X-Xet-Cas-Url"); got != "" {
		t.Fatalf("X-Xet-Cas-Url was injected (%q) on response that lacked it", got)
	}
}

// Body + non-target headers pass through untouched. Any body rewriting
// would risk corrupting HF's own responses (commit OIDs, etc.).
func TestForwardsBodyAndOtherHeaders(t *testing.T) {
	const body = `{"commitOid":"deadbeef","success":true}`
	hf := fakeHF(t, "https://cas-server.xethub.hf.co", body)
	defer hf.Close()

	up, _ := url.Parse(hf.URL)
	proxy := buildProxy(t, up, "http://openweights-cas:28080")

	rec := httptest.NewRecorder()
	req := httptest.NewRequest(http.MethodGet, "/api/x", nil)
	proxy.ServeHTTP(rec, req)

	if got := rec.Header().Get("Content-Type"); got != "application/json" {
		t.Errorf("Content-Type = %q; want application/json", got)
	}
	if got := rec.Header().Get("X-Xet-Access-Token"); got != "hf-test-token" {
		t.Errorf("X-Xet-Access-Token was not forwarded: %q", got)
	}
	if !strings.Contains(rec.Body.String(), "deadbeef") {
		t.Errorf("body was mangled: %q", rec.Body.String())
	}
}

// Upstream outage must surface as a clean 502 to the client, not a
// blank/unhandled error that confuses hf CLI.
func TestUpstreamDown502(t *testing.T) {
	// Point to a port that definitely won't answer. :1 is reliably dead
	// on a local machine.
	up, _ := url.Parse("http://127.0.0.1:1")
	proxy := buildProxy(t, up, "http://openweights-cas:28080")

	rec := httptest.NewRecorder()
	req := httptest.NewRequest(http.MethodGet, "/api/x", nil)
	proxy.ServeHTTP(rec, req)

	if rec.Code != http.StatusBadGateway {
		t.Fatalf("status = %d; want 502", rec.Code)
	}
	if !strings.Contains(rec.Body.String(), "upstream_unreachable") {
		t.Errorf("error body not in expected shape: %q", rec.Body.String())
	}
}
