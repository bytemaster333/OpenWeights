// Package main — static Go binary bind-mounted into the indexd container
// as its healthcheck. Checks /api/state.synced AND /api/wallet.confirmed.
// See RESEARCH §4-5; PLAN 05 implements.
package main

import "os"

func main() {
	os.Exit(2) // stub — PLAN 05 implements the real probe
}
