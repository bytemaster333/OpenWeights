// Package main — middleware.go.
//
// Wave 1 ships one middleware only: request-ID injection. Structured logging
// (slog JSON per OPS-02) and per-request metric instrumentation land in 03-06.
package main

import (
	"context"
	"net/http"

	"github.com/google/uuid"
)

// requestIDKey is a private context key used to pass the request id through
// the handler chain. Unexported to keep the seam stable.
type requestIDKey struct{}

// RequestID middleware honors a client-supplied `X-Request-Id` header (useful
// for correlating client/gateway/CAS logs) and synthesizes a UUID v4 when none
// is present. Always echoes the id back on egress.
func RequestID(next http.Handler) http.Handler {
	return http.HandlerFunc(func(w http.ResponseWriter, r *http.Request) {
		rid := r.Header.Get("X-Request-Id")
		if rid == "" {
			rid = uuid.New().String()
		}
		ctx := context.WithValue(r.Context(), requestIDKey{}, rid)
		w.Header().Set("X-Request-Id", rid)
		next.ServeHTTP(w, r.WithContext(ctx))
	})
}

// RequestIDFrom returns the request id from context if present. Handlers /
// loggers in 03-06 use this; Wave 1 exposes it for test assertion.
func RequestIDFrom(ctx context.Context) string {
	if v, ok := ctx.Value(requestIDKey{}).(string); ok {
		return v
	}
	return ""
}
