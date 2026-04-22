//! Core CAS library: handlers, auth middleware, errors, rate limiting, signing.

pub mod auth;
pub mod coalesce;
pub mod errors;
pub mod handlers;
pub mod metering;
pub mod metrics;
pub mod rate_limit;
pub mod scopes;
pub mod session;
pub mod shard_parse;
pub mod signed_url;

#[cfg(test)]
mod tests;

pub use errors::AppError;
pub use metrics::{Metrics, MetricsState, metrics_handler};
