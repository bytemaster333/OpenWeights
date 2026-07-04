//! Query modules. Each module exposes typed helpers used by handler crates.
//! plans populate these:
//! - fills `api_keys::fetch_active_key_by_hash`.
//! - fills `xorbs::{insert, mark_pinned, update_state}`.
//! - fills `shards::insert` +
//!   `reconstruction::{insert_file, insert_terms, cross_check_xorbs}`.
//! - fills `reconstruction::{get, get_batch}`.
//! - fills
//!   `usage_log::{insert_xorb_upload, insert_shard_upload, insert_reconstruction}`
//! + reconciler sweep queries across xorbs/shards.

pub mod api_keys;
pub mod reconstruction;
pub mod shards;
pub mod usage_log;
pub mod xorbs;
