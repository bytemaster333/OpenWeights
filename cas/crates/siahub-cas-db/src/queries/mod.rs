//! Query modules. Each module exposes typed helpers used by handler crates.
//!
//! Phase 2 plans populate these:
//!   - Plan 02-03 fills `api_keys::fetch_active_key_by_hash`.
//!   - Plan 02-04 fills `xorbs::{insert, mark_pinned, update_state}`.
//!   - Plan 02-05 fills `shards::insert` +
//!     `reconstruction::{insert_file, insert_terms, cross_check_xorbs}`.
//!   - Plan 02-06 fills `reconstruction::{get, get_batch}`.
//!   - Plan 02-09 fills
//!     `usage_log::{insert_xorb_upload, insert_shard_upload, insert_reconstruction}`
//!     + reconciler sweep queries across xorbs/shards.

pub mod api_keys;
pub mod reconstruction;
pub mod shards;
pub mod usage_log;
pub mod xorbs;
