package main

import "testing"

// TestSmokeBuilds ensures the smoke binary compiles; live execution is
// gated on a funded wallet and exercised by `make bootstrap` / `make smoke`.
// The presence of this test file triggers the `go test` harness to compile
// the package, which is the assertion we want.
func TestSmokeBuilds(t *testing.T) {
	// Compilation is proved by go test building the package. Nothing else
	// to assert — live execution happens in the wizard.
}
