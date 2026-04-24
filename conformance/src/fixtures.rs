//! Fixture loaders for `xet-team/xet-spec-reference-files @ 18bf9173fb...`.
//! Every loader returns `Err(FixturesError::NotCloned)` when the expected file
//! is absent on disk; tests surface this as an `eprintln!` + early-return (not
//! a failure) so `cargo test` is green on a fresh clone without LFS.
//! See `conformance/fixtures/README.md` for the one-time `git lfs clone`
//! command.

use std::fmt;
use std::fs;
use std::path::{Path, PathBuf};

/// Fixture access failure. Tests match on the kind to decide skip vs fail.
#[derive(Debug)]
pub enum FixturesError {
    /// A required LFS blob is missing on disk; skip the test with a message.
    NotCloned(PathBuf),
    /// Underlying I/O or parse failure; this IS a failure case (treat as
    /// broken rather than "not cloned yet").
    Io(std::io::Error),
    /// A computed hash did not match the expected value (unused today; held
    /// in the taxonomy for a future strict-integrity sweep).
    HashMismatch { expected: String, got: String },
}

impl fmt::Display for FixturesError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            FixturesError::NotCloned(p) => write!(
                f,
                "fixtures not cloned — see conformance/fixtures/README.md (expected path: {p:?})"
            ),
            FixturesError::Io(e) => write!(f, "fixture I/O error: {e}"),
            FixturesError::HashMismatch { expected, got } => {
                write!(f, "fixture hash mismatch — expected {expected}, got {got}")
            }
        }
    }
}

impl std::error::Error for FixturesError {
    fn source(&self) -> Option<&(dyn std::error::Error + 'static)> {
        match self {
            FixturesError::Io(e) => Some(e),
            _ => None,
        }
    }
}

impl From<std::io::Error> for FixturesError {
    fn from(e: std::io::Error) -> Self {
        FixturesError::Io(e)
    }
}

/// True when the given error signals "LFS blobs not cloned" — tests skip on
/// this kind and fail on everything else.
pub fn is_skip(err: &FixturesError) -> bool {
    matches!(err, FixturesError::NotCloned(_))
}

/// Absolute path to `conformance/fixtures/`. Computed from `CARGO_MANIFEST_DIR`
/// so tests work regardless of the caller's CWD.
pub fn fixtures_dir() -> PathBuf {
    PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("fixtures")
}

/// Path to the canonical reference xorb (~64 MiB binary). Absent on fresh
/// clones until `make conformance-fixtures` runs.
pub fn reference_xorb_path() -> PathBuf {
    fixtures_dir().join(format!("{}.xorb", crate::REFERENCE_XORB_HASH_HEX))
}

/// Walk `fixtures/` for the first `*.shard` file (the dataset ships four
/// shard variants — any one is usable for the dual-path round-trip test).
/// Returns `FixturesError::NotCloned` when no `.shard` is present.
pub fn reference_shard_path() -> Result<PathBuf, FixturesError> {
    let dir = fixtures_dir();
    if !dir.exists() {
        return Err(FixturesError::NotCloned(dir));
    }
    find_first_with_ext(&dir, "shard")
        .ok_or_else(|| FixturesError::NotCloned(dir.join("*.shard")))
}

/// Load the reference xorb bytes + expected hash hex.
/// Returns `(bytes, expected_hex)`. Does NOT verify the hash — that's a
/// consumer concern (the canary test + handler-side merkle verify do it).
pub fn load_reference_xorb() -> Result<(Vec<u8>, String), FixturesError> {
    let p = reference_xorb_path();
    if !p.exists() {
        return Err(FixturesError::NotCloned(p));
    }
    let bytes = fs::read(&p)?;
    Ok((bytes, crate::REFERENCE_XORB_HASH_HEX.to_string()))
}

/// Load a reference shard. The specific variant is whichever `.shard` is
/// present first in lexicographic order.
pub fn load_reference_shard() -> Result<Vec<u8>, FixturesError> {
    let p = reference_shard_path()?;
    let bytes = fs::read(&p)?;
    Ok(bytes)
}

/// The signed-URL vectors fixture is committed to git (not LFS) — always
/// present. Loads + parses into `serde_json::Value` so per-test deserialization
/// can pick its own shape.
pub fn load_signed_url_vectors() -> Result<serde_json::Value, FixturesError> {
    let p = fixtures_dir().join("signed_url_vectors.json");
    let bytes = fs::read(&p)?;
    serde_json::from_slice::<serde_json::Value>(&bytes)
        .map_err(|e| FixturesError::Io(std::io::Error::other(e)))
}

// ---------------------------------------------------------------------------
// helpers
// ---------------------------------------------------------------------------

fn find_first_with_ext(dir: &Path, ext: &str) -> Option<PathBuf> {
    let entries = fs::read_dir(dir).ok()?;
    let mut candidates: Vec<PathBuf> = entries
        .filter_map(|e| e.ok())
        .map(|e| e.path())
        .filter(|p| p.extension().and_then(|s| s.to_str()) == Some(ext))
        .collect();
    candidates.sort();
    candidates.into_iter().next()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn fixtures_dir_resolves_to_repo_path() {
        let d = fixtures_dir();
        assert!(d.ends_with("conformance/fixtures"));
    }

    #[test]
    fn reference_xorb_path_is_hash_prefixed() {
        let p = reference_xorb_path();
        assert!(p.file_name().unwrap().to_str().unwrap().ends_with(".xorb"));
    }

    #[test]
    fn signed_url_vectors_load_ok() {
        // This fixture is committed to git; always present.
        let v = load_signed_url_vectors().expect("vectors fixture loads");
        let arr = v.as_array().expect("vectors is a JSON array");
        assert!(!arr.is_empty(), "at least one vector shipped by Plan 02-08");
    }
}
