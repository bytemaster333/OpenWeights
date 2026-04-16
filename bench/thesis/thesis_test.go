//go:build integration
// +build integration

// Integration tests for the thesis measurement.
// PLAN 06 adds TestRangeDownloadSectorScoping (runs 3 trials; asserts median ≤ 8× requested range).
package main

import "testing"

func TestRangeDownloadSectorScoping_Stub(t *testing.T) {
	t.Skip("stub — PLAN 06 implements")
}
