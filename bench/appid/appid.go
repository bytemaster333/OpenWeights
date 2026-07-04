// Package appid holds the single OpenWeights App ID constant used by every
// validator and consumed verbatim by (cas/) via a
// language-parallel Rust copy in conformance/src/appid.rs.
// Generated once on 2026- via `crypto/rand` (equivalent to
// siastorage.GenerateAppID which uses lukechampine.com/frand per
// RESEARCH §1). DO NOT rotate. Rotation invalidates every
// {xorb_hash -> sia_object_id} mapping.
package appid

// OpenWeightsAppID is the 32-byte Sia App ID (hex-encoded per types.Hash256.UnmarshalText).
const OpenWeightsAppID = "f0955611cb463ab8aa8b6c61702d0ade26d795c22b50c6e5b3bfdb193a3fc049"
