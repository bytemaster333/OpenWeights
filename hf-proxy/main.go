// openweights-hf-proxy — transparent reverse proxy in front of the Hugging Face
// Hub API that rewrites Xet CAS routing so bytes flow to OpenWeights instead of
// HF's own CAS.
// Why this exists
// ---------------
// `hf upload` / `hf download` (and every Python library that uses
// huggingface_hub) fetch the Xet CAS URL from an HF API response header
// (`X-Xet-Cas-Url`). The client-side env var `HF_XET_DATA_DEFAULT_CAS_ENDPOINT`
// is ONLY a fallback for when that header is missing — which never happens
// in an HF-integrated upload. So the story env-var trick does not, by
// itself, route bytes to OpenWeights.
// Fix: interpose this HTTP reverse proxy between the client and HF. The
// client sets `HF_ENDPOINT=<proxy URL>`. The proxy forwards every request
// to https://huggingface.co unchanged, streams the response body through,
// and ONLY rewrites the `X-Xet-Cas-Url` response header whenever HF emits
// one. The value is replaced with the operator-configured OpenWeights CAS URL
// (`OPENWEIGHTS_CAS_PUBLIC_URL`).
// Auth for the OpenWeights CAS itself is still handled client-side via
// `HF_XET_DATA_CUSTOM_HEADERS='{"Authorization":"Bearer <key>"}'` — hf_xet
// attaches that header to every CAS request it makes. This proxy does NOT
// need to know about the OpenWeights API key; URL rewriting is its entire job.
// What this proxy does NOT do
// * TLS termination — Caddy fronts this proxy in production.
// * Token refresh route rewriting — our CAS authenticates via the
// Authorization header, so hf_xet's refresh token is irrelevant for us.
// * Auth injection — handled client-side; proxy is stateless.
// * Response body rewriting — only headers. Keeps the proxy simple and
// prevents accidental corruption of HF API responses.
package main

import (
	"context"
	"errors"
	"fmt"
	"log/slog"
	"net/http"
	"net/http/httputil"
	"net/url"
	"os"
	"os/signal"
	"strings"
	"syscall"
	"time"
)

type config struct {
	// Listen address for the proxy (e.g. `:28090`).
	listenAddr string
	// URL to forward every request to (default: https://huggingface.co).
	upstreamURL *url.URL
	// Value to substitute into `X-Xet-Cas-Url` response headers.
	// Must be the CAS URL reachable from the CLIENT side, not from inside
	// the Compose network. Local dev: `http://localhost:28080`. Prod:
	// `https://cas.example.com`.
	casPublicURL string
}

func loadConfig() (*config, error) {
	listen := getEnv("LISTEN_ADDR", ":28090")

	upstreamRaw := getEnv("HF_UPSTREAM_URL", "https://huggingface.co")
	upstream, err := url.Parse(upstreamRaw)
	if err != nil {
		return nil, fmt.Errorf("HF_UPSTREAM_URL parse: %w", err)
	}
	if upstream.Scheme == "" || upstream.Host == "" {
		return nil, fmt.Errorf("HF_UPSTREAM_URL must be an absolute URL, got %q", upstreamRaw)
	}

	cas := os.Getenv("OPENWEIGHTS_CAS_PUBLIC_URL")
	if cas == "" {
		return nil, errors.New("OPENWEIGHTS_CAS_PUBLIC_URL is required (the CAS URL the CLIENT reaches)")
	}
	if _, err := url.Parse(cas); err != nil {
		return nil, fmt.Errorf("OPENWEIGHTS_CAS_PUBLIC_URL parse: %w", err)
	}
	cas = strings.TrimRight(cas, "/")

	return &config{
		listenAddr:   listen,
		upstreamURL:  upstream,
		casPublicURL: cas,
	}, nil
}

func getEnv(name, def string) string {
	if v := os.Getenv(name); v != "" {
		return v
	}
	return def
}

// rewriteResponse is invoked by httputil.ReverseProxy after the upstream
// response is received and BEFORE the body is streamed to the client. We
// only touch headers here; the body flows through unmodified.
// The only header we rewrite is `X-Xet-Cas-Url`. Everything else is
// forwarded as-is. This includes `X-Xet-Access-Token`, `X-Xet-Expiration`,
// `X-Xet-Refresh-Route` — hf_xet uses them against the URL we just
// rewrote, which is fine because our CAS ignores them (auth is
// `Authorization: Bearer <key>` per client-side HF_XET_DATA_CUSTOM_HEADERS).
func (c *config) rewriteResponse(resp *http.Response) error {
	orig := resp.Header.Get("X-Xet-Cas-Url")
	if orig == "" {
		return nil
	}
	resp.Header.Set("X-Xet-Cas-Url", c.casPublicURL)
	slog.Info("rewrote X-Xet-Cas-Url",
		"path", resp.Request.URL.Path,
		"from", orig,
		"to", c.casPublicURL,
	)
	return nil
}

// rewriteRequest is invoked by httputil.ReverseProxy before the upstream
// request is dispatched. We fix the outbound Host header to match the
// upstream so HF's API doesn't reject with a CORS / SNI mismatch, and
// strip hop-by-hop headers that shouldn't cross proxy boundaries.
func (c *config) rewriteRequest(pr *httputil.ProxyRequest) {
	pr.SetURL(c.upstreamURL)
	// SetURL already rewrites scheme/host/path; also reset Host header so
	// upstream TLS SNI + virtual hosting land at huggingface.co cleanly.
	pr.Out.Host = c.upstreamURL.Host
	// The client may send Accept-Encoding: gzip — we want upstream to
	// compress, and httputil.ReverseProxy streams the compressed bytes
	// verbatim. Header rewrites happen BEFORE decompression which is fine
	// for us since X-Xet-Cas-Url is a header, not body.
}

func newProxy(c *config) http.Handler {
	rp := &httputil.ReverseProxy{
		Rewrite: func(pr *httputil.ProxyRequest) {
 // Access log — see every inbound path so we can confirm the
 // HF client reaches the proxy for all flows (not just
 // xet-write-token). Kept INFO because upload debugging is
 // currently load-bearing; tune to DEBUG post-demo.
			slog.Info("forward",
				"method", pr.In.Method,
				"path", pr.In.URL.Path,
			)
			c.rewriteRequest(pr)
		},
		ModifyResponse: c.rewriteResponse,
		ErrorHandler: func(w http.ResponseWriter, r *http.Request, err error) {
			slog.Error("upstream error",
				"path", r.URL.Path,
				"err", err,
			)
			w.WriteHeader(http.StatusBadGateway)
			_, _ = w.Write([]byte(`{"error":"upstream_unreachable"}`))
		},
 // Explicit transport so we can tune timeouts later. Default
 // http.DefaultTransport handles HTTPS to HF just fine.
		Transport: &http.Transport{
			TLSHandshakeTimeout:   10 * time.Second,
			ResponseHeaderTimeout: 30 * time.Second,
			ExpectContinueTimeout: 1 * time.Second,
			IdleConnTimeout:       90 * time.Second,
			MaxIdleConns:          50,
			MaxIdleConnsPerHost:   10,
		},
	}
	return rp
}

// healthHandler — stateless liveness probe. We deliberately do NOT probe
// upstream here; if HF is down the proxy is still alive and the right
// error for the operator is a 502 when an actual request flows through,
// not a failed healthcheck.
// runHealthcheck GETs /health on the configured listen port. Returns 0 if the
// server answers 200, else 1. Keeps the probe inside the distroless image.
func runHealthcheck() int {
	addr := getEnv("LISTEN_ADDR", ":28090")
	// LISTEN_ADDR may be ":28090" — dial loopback on that port.
	if strings.HasPrefix(addr, ":") {
		addr = "127.0.0.1" + addr
	}
	client := &http.Client{Timeout: 3 * time.Second}
	resp, err := client.Get("http://" + addr + "/health")
	if err != nil {
		return 1
	}
	defer resp.Body.Close()
	if resp.StatusCode == http.StatusOK {
		return 0
	}
	return 1
}

func healthHandler(w http.ResponseWriter, r *http.Request) {
	w.Header().Set("Content-Type", "application/json")
	w.WriteHeader(http.StatusOK)
	_, _ = w.Write([]byte(`{"status":"ok"}`))
}

func main() {
	// `healthcheck` subcommand — used by the Docker healthcheck. The runtime
	// image is distroless (no shell/wget), so the probe runs this binary
	// instead: GET /health on our own listen port, exit 0 on 200 else 1.
	if len(os.Args) > 1 && os.Args[1] == "healthcheck" {
		os.Exit(runHealthcheck())
	}

	slog.SetDefault(slog.New(slog.NewJSONHandler(os.Stdout, &slog.HandlerOptions{
		Level: slog.LevelInfo,
	})))

	cfg, err := loadConfig()
	if err != nil {
		slog.Error("config", "err", err)
		os.Exit(2)
	}

	mux := http.NewServeMux()
	mux.HandleFunc("/health", healthHandler)
	mux.Handle("/", newProxy(cfg))

	srv := &http.Server{
		Addr:              cfg.listenAddr,
		Handler:           mux,
		ReadHeaderTimeout: 10 * time.Second,
 // No write/read timeouts — large file uploads take as long as
 // they take. The upstream transport has per-request deadlines.
	}

	// Graceful shutdown on SIGINT/SIGTERM.
	ctx, stop := signal.NotifyContext(context.Background(), syscall.SIGINT, syscall.SIGTERM)
	defer stop()

	go func() {
		slog.Info("openweights-hf-proxy listening",
			"addr", cfg.listenAddr,
			"upstream", cfg.upstreamURL.String(),
			"cas_public_url", cfg.casPublicURL,
		)
		if err := srv.ListenAndServe(); err != nil && !errors.Is(err, http.ErrServerClosed) {
			slog.Error("listen", "err", err)
			os.Exit(1)
		}
	}()

	<-ctx.Done()
	slog.Info("shutdown signal received, draining")

	shutdownCtx, cancel := context.WithTimeout(context.Background(), 10*time.Second)
	defer cancel()
	if err := srv.Shutdown(shutdownCtx); err != nil {
		slog.Error("shutdown", "err", err)
		os.Exit(1)
	}
	slog.Info("shutdown complete")
}
