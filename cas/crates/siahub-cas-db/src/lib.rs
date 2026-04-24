//! Postgres pool + migrations wiring for siahub-cas.
//! wiring: `run_migrations` compiles the `../../migrations/` tree
//! into the binary at build time via `sqlx::migrate!`. The runtime binary
//! therefore does NOT need the migrations/ directory on disk.
//! Offline query cache: set `SQLX_OFFLINE=true` in CI + Docker so
//! `cargo build` does not require a live database. Locally, leave it unset
//! and `sqlx::query!` will type-check against the running Postgres. See the
//! crate README for the refresh workflow.

pub use sqlx;
pub mod queries;
pub mod types;

/// Build a connection pool to Postgres with sane defaults.
/// max_connections comes from `PG_MAX_CONNECTIONS` (Config); min 2 keeps a
/// warm pool so the first request does not eat TCP handshake latency.
pub async fn build_pool(
    database_url: &str,
    max_connections: u32,
) -> Result<sqlx::PgPool, sqlx::Error> {
    sqlx::postgres::PgPoolOptions::new()
        .max_connections(max_connections)
        .min_connections(2)
        .acquire_timeout(std::time::Duration::from_secs(5))
        .connect(database_url)
        .await
}

/// Run all pending migrations against the given pool.
/// The `sqlx::migrate!` macro embeds the SQL files under `../../migrations/`
/// into the binary at compile time. Safe to call on every boot: the
/// `_sqlx_migrations` table tracks applied files by checksum and skips them
/// on replay. All DDL in the embedded files is additionally idempotent via
/// `CREATE... IF NOT EXISTS` or `DO $$... EXCEPTION WHEN duplicate_object`
/// wrappers — double defense against testcontainer restart races.
pub async fn run_migrations(pool: &sqlx::PgPool) -> Result<(), sqlx::migrate::MigrateError> {
    sqlx::migrate!("../../migrations").run(pool).await
}
