//! Typed enum mirrors of Postgres enums. Stay 1:1 with migrations
//! 0001_initial.sql + 0002_xorbs_shards.sql + 0003_usage_log_oauth.sql.
//!
//! Adding a variant here without also adding it to the corresponding
//! Postgres enum (or vice-versa) will fail at the first `query_as!` call
//! that binds the type. Keep these in lockstep with the migrations.

use sqlx::Type;

/// `api_key_scope` enum — matches migration 0001.
///
/// Encoded as an integer at the D-20 extractor level (Upload=0, Download=1,
/// Admin=2) via the `AuthScoped<const S: u8>` extractor; the Rust enum here
/// is only used at the persistence boundary where sqlx needs a type it can
/// cast from the Postgres `api_key_scope[]` column.
#[derive(Type, Debug, Clone, Copy, PartialEq, Eq)]
#[sqlx(type_name = "api_key_scope", rename_all = "lowercase")]
pub enum ApiKeyScope {
    Upload,
    Download,
    Admin,
}

/// `xorb_pin_state` enum — matches migration 0002. Shared by `xorbs` and
/// `shards` tables (D-15). Reconciler (Plan 02-09) drives state transitions:
///
///   Uploading -> Pinning -> Pinned
///                       \-> Orphaned (after 5 failed attempts)
#[derive(Type, Debug, Clone, Copy, PartialEq, Eq)]
#[sqlx(type_name = "xorb_pin_state", rename_all = "lowercase")]
pub enum XorbPinState {
    Uploading,
    Pinning,
    Pinned,
    Orphaned,
}

/// `usage_event` enum — matches migration 0003.
///
/// D-19 item 1 folds in `Reconstruction`. `XorbServe` + `DedupQuery` are
/// reserved slots consumed by Phase 3 gateway (OQ-J) and a future non-stub
/// of PROTO-04 respectively; Phase 2 code must not emit them.
///
/// `rename_all = "snake_case"` maps CamelCase variants to the snake_case
/// Postgres enum labels: `XorbUpload` <-> `'xorb_upload'`, etc.
#[derive(Type, Debug, Clone, Copy, PartialEq, Eq)]
#[sqlx(type_name = "usage_event", rename_all = "snake_case")]
pub enum UsageEvent {
    XorbUpload,
    ShardUpload,
    Reconstruction,
    XorbServe,
    DedupQuery,
}
