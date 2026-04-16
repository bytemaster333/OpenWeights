// Package appid holds the single SiaHub App ID constant used by every
// Phase 1 validator and consumed verbatim by Phase 2 (cas/) via a
// language-parallel copy.
//
// The constant is generated once by `siastorage.GenerateAppID()` and
// committed. PLAN 07 fills in the real hex string during its
// A3-verification task.
package appid

// SiaHubAppID is a 32-byte hex string. Replaced with the generated
// value by PLAN 07 Task 1 before any live-network code runs.
const SiaHubAppID = "REPLACE_WITH_32_BYTE_HEX_CONSTANT_IN_PLAN_07"
