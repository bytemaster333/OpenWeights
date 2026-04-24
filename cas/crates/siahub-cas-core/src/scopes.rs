//! Scope taxonomy.
//! The Postgres enum `api_key_scope` has three variants; this crate mirrors
//! them as u8 const generics (used by `AuthScoped<const S: u8>`) and as a
//! Rust enum (used for comparisons at runtime).
//! The three const values are load-bearing — Plans..08 declare handler
//! signatures against them literally. Do NOT renumber.
//! once (W2) lands `siahub_cas_db::types::ApiKeyScope`, this
//! local enum can be re-exported from there instead. For parallel-execution
//! safety, the local copy lives here.

use sqlx::Type;

/// Scope = `upload` — may POST xorbs / shards.
pub const SCOPE_UPLOAD: u8 = 0;
/// Scope = `download` — may GET reconstructions / chunks.
pub const SCOPE_DOWNLOAD: u8 = 1;
/// Scope = `admin` — may hit `/admin/*` routes.
pub const SCOPE_ADMIN: u8 = 2;

/// Postgres `api_key_scope` enum mirror.
#[derive(Type, Debug, Clone, Copy, PartialEq, Eq)]
#[sqlx(type_name = "api_key_scope", rename_all = "lowercase")]
pub enum ApiKeyScope {
    Upload,
    Download,
    Admin,
}

impl ApiKeyScope {
    /// Map a const-generic u8 back to the enum.
    /// Returns `None` for any out-of-range scope — the auth extractor uses
    /// this to reject unknown scopes with 403 at the const-generic boundary
    /// (T-02-03-08).
    pub fn from_u8(v: u8) -> Option<Self> {
        match v {
            SCOPE_UPLOAD => Some(Self::Upload),
            SCOPE_DOWNLOAD => Some(Self::Download),
            SCOPE_ADMIN => Some(Self::Admin),
            _ => None,
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn scope_constants_round_trip() {
        assert_eq!(ApiKeyScope::from_u8(SCOPE_UPLOAD), Some(ApiKeyScope::Upload));
        assert_eq!(
            ApiKeyScope::from_u8(SCOPE_DOWNLOAD),
            Some(ApiKeyScope::Download)
        );
        assert_eq!(ApiKeyScope::from_u8(SCOPE_ADMIN), Some(ApiKeyScope::Admin));
        assert_eq!(ApiKeyScope::from_u8(42), None);
    }
}
