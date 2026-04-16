// Package main — `make bootstrap` entry point. Drives BIP-39 -> App Key
// derivation, Compose startup, wallet-funding UX, smoke test.
// See RESEARCH §3 + §10 for the full flow; PLAN 07 implements.
package main

import (
	"fmt"
	"os"
)

func main() {
	fmt.Fprintln(os.Stderr, "bootstrap: stub — PLAN 07 provides real implementation")
	os.Exit(2)
}
