//! Test harness: bring up siahub-cas + Postgres in Docker via testcontainers,
//! seed an API key, and return a handle containing the base URL + bearer
//! token.
//! Design choices (CONTEXT ):
//! - **Testcontainers-driven Postgres** — every test gets a fresh `postgres:17-alpine`
//! container; shared-pool concerns disappear.
//! - **Pre-built siahub-cas image** — the cas/Dockerfile is built once via
//! `make cas-image`. Per-test rebuilds would cost minutes. If the image
//! is absent, tests skip with a clear message pointing at `make cas-image`.
//! - **Sia is mocked via `SIAHUB_SIA_MOCK=true`** — the binary's `sia-mock`
//! feature flag gates this env path; see
//! `cas/crates/siahub-cas/src/main.rs::build_sia_adapter`. owns
//! live-Sia CI.
//! Skip semantics: every `spawn_cas` callsite uses `match` + early-return
//! when the result is `Ok(None)` — NEVER panic/fail on missing Docker.

use anyhow::{Context, Result};
use base64::Engine;
use sha2::{Digest, Sha256};
use std::time::Duration;

/// Tags the test harness probes for in the local Docker daemon, in priority
/// order. The first match wins. `make cas-image` produces the first; the
/// latter two are the default `docker compose build siahub-cas` outputs.
pub const CAS_IMAGE_CANDIDATES: &[&str] = &[
    "siahub-cas:conformance",
    "siahub-cas:latest",
    "ops-siahub-cas:latest",
];

/// An up-and-ready siahub-cas + Postgres stack. Drop this to tear everything
/// down.
pub struct Harness {
    /// Base URL like `http://127.0.0.1:12345` — what the xet-client's
    /// `RemoteClient::new` and direct-HTTP test code point at.
    pub base_url: String,
    /// Postgres connection URL (host-side, mapped ephemeral port).
    pub pg_url: String,
    /// Bearer token with scope=upload + scope=download + scope=admin.
    pub upload_download_token: String,
    /// UUID of the seeded api_key row — handy for metering assertions.
    pub api_key_id: uuid::Uuid,
    /// Seeded user's GitHub numeric id (fixture = `999_999`).
    pub user_id: i64,

    // Container handles hold the containers alive for the Harness lifetime.
    _pg: testcontainers::ContainerAsync<testcontainers_modules::postgres::Postgres>,
    _cas: testcontainers::ContainerAsync<testcontainers::GenericImage>,
}

/// Opt-in knobs for the test stack; keep backward-compat with existing
/// `spawn_cas` callers by providing a `Default` that matches prior shape.
#[derive(Debug, Clone, Default)]
pub struct SpawnOpts {
    /// If true, the CAS is launched with `V2_RECONSTRUCTION_ENABLED=true`.
    /// Default false (matches `.env.example`); flips to
    /// true in a V2 multi-range round-trip test.
    pub v2_reconstruction_enabled: bool,
}

/// Spawn the full stack. Returns `Ok(None)` if a precondition (Docker, image)
/// is not met — callers early-return to produce a skip.
pub async fn spawn_cas() -> Result<Option<Harness>> {
    spawn_cas_with(SpawnOpts::default()).await
}

/// Spawn with the V2 flag enabled. `v2_multi_range_round_trip`
/// uses this to prove the end-to-end V2 → multi-range → multipart/byteranges
/// path once the Go gateway's `multipart/byteranges` writer (03-04) lands.
pub async fn spawn_cas_v2_enabled() -> Result<Option<Harness>> {
    spawn_cas_with(SpawnOpts {
        v2_reconstruction_enabled: true,
    })
    .await
}

/// Full-control spawn entrypoint; both public wrappers delegate here.
pub async fn spawn_cas_with(opts: SpawnOpts) -> Result<Option<Harness>> {
    if !docker_available().await {
        eprintln!("SKIP spawn_cas: docker daemon unreachable (is Docker running?)");
        return Ok(None);
    }

    let image = match find_cas_image().await {
        Some(img) => img,
        None => {
            eprintln!(
                "SKIP spawn_cas: siahub-cas image absent; run `make cas-image` first. \
                 Candidates probed: {CAS_IMAGE_CANDIDATES:?}"
            );
            return Ok(None);
        }
    };

    // --- Postgres.
    use testcontainers::runners::AsyncRunner;
    let pg = testcontainers_modules::postgres::Postgres::default()
        .with_db_name("siahub")
        .with_user("siahub")
        .with_password("test-password-conformance")
        .start()
        .await
        .context("postgres container start")?;

    let pg_host = pg.get_host().await.context("pg host")?.to_string();
    let pg_port = pg.get_host_port_ipv4(5432).await.context("pg port")?;
    let pg_url_host =
        format!("postgres://siahub:test-password-conformance@{pg_host}:{pg_port}/siahub");
    // host.docker.internal resolves from inside containers on macOS/Windows;
    // on Linux testcontainers adds an `/etc/hosts` entry via its helper.
    let pg_url_bridge = format!(
        "postgres://siahub:test-password-conformance@host.docker.internal:{pg_port}/siahub"
    );

    // --- Env for siahub-cas binary (see cas/crates/siahub-cas/src/config.rs).
    let signing_key_b64 =
        base64::engine::general_purpose::STANDARD.encode([0u8; 32]);
    let app_id_hex = "00".repeat(32);
    let app_key_b64 =
        base64::engine::general_purpose::STANDARD.encode([0u8; 32]);

    let (image_name, image_tag) = split_tag(&image);
    use testcontainers::core::ImageExt;
    let cas_image = testcontainers::GenericImage::new(image_name, image_tag)
        .with_exposed_port(testcontainers::core::ContainerPort::Tcp(8080))
        .with_wait_for(testcontainers::core::WaitFor::message_on_stdout(
            "siahub-cas listening",
        ));
    let cas = cas_image
        .with_env_var("DATABASE_URL", &pg_url_bridge)
        .with_env_var("REDIS_URL", "redis://127.0.0.1:1/")
        .with_env_var("INDEXD_URL", "http://127.0.0.1:1")
        .with_env_var("SIAHUB_APP_ID", &app_id_hex)
        .with_env_var("SIAHUB_APP_KEY", &app_key_b64)
        .with_env_var("SIAHUB_SIA_MOCK", "true")
        .with_env_var("GATEWAY_URL_SIGNING_KEY", &signing_key_b64)
        .with_env_var("GATEWAY_BASE_URL", "http://127.0.0.1:9090")
        .with_env_var(
            "V2_RECONSTRUCTION_ENABLED",
            if opts.v2_reconstruction_enabled {
                "true"
            } else {
                "false"
            },
        )
        .with_env_var("BIND_ADDR", "0.0.0.0:8080");

    let cas_container = cas
        .start()
        .await
        .context("siahub-cas container start — image must be built via `make cas-image`")?;

    // CAS has run migrations by the time the "listening" log line prints.
    // Seed the API key NOW via the host-side Postgres URL.
    let (token, api_key_id, user_id) = seed_api_key(&pg_url_host).await?;

    let cas_host = cas_container.get_host().await?.to_string();
    let cas_port = cas_container.get_host_port_ipv4(8080).await?;
    let base_url = format!("http://{cas_host}:{cas_port}");

    Ok(Some(Harness {
        base_url,
        pg_url: pg_url_host,
        upload_download_token: token,
        api_key_id,
        user_id,
        _pg: pg,
        _cas: cas_container,
    }))
}

/// Seed a user + one `api_key` row carrying `upload` + `download` + `admin`
/// scopes. Returns `(plaintext_token, api_key_id, user_id)`.
pub async fn seed_api_key(pg_url: &str) -> Result<(String, uuid::Uuid, i64)> {
    let pool = sqlx::postgres::PgPoolOptions::new()
        .max_connections(2)
        .acquire_timeout(Duration::from_secs(5))
        .connect(pg_url)
        .await
        .context("connect to testcontainer pg for seeding")?;

    let user_id: i64 = 999_999;
    sqlx::query(
        "INSERT INTO users (id, github_login, email)
         VALUES ($1, 'conformance-bot', NULL)
         ON CONFLICT (id) DO NOTHING",
    )
    .bind(user_id)
    .execute(&pool)
    .await
    .context("seed users row")?;

    let plaintext = format!("conf-{}", uuid::Uuid::new_v4());
    let hash: [u8; 32] = Sha256::digest(plaintext.as_bytes()).into();

    let row: (uuid::Uuid,) = sqlx::query_as(
        "INSERT INTO api_keys (user_id, key_hash, scopes, label)
         VALUES ($1, $2, ARRAY['upload','download','admin']::api_key_scope[], 'conformance')
         RETURNING id",
    )
    .bind(user_id)
    .bind(hash.to_vec())
    .fetch_one(&pool)
    .await
    .context("seed api_keys row")?;

    pool.close().await;
    Ok((plaintext, row.0, user_id))
}

async fn docker_available() -> bool {
    tokio::process::Command::new("docker")
        .args(["info", "--format", "{{.ServerVersion}}"])
        .kill_on_drop(true)
        .stdout(std::process::Stdio::null())
        .stderr(std::process::Stdio::null())
        .status()
        .await
        .map(|s| s.success())
        .unwrap_or(false)
}

async fn find_cas_image() -> Option<String> {
    for cand in CAS_IMAGE_CANDIDATES {
        let out = tokio::process::Command::new("docker")
            .args(["image", "inspect", cand])
            .kill_on_drop(true)
            .stdout(std::process::Stdio::null())
            .stderr(std::process::Stdio::null())
            .status()
            .await;
        if matches!(out, Ok(s) if s.success()) {
            return Some((*cand).to_string());
        }
    }
    None
}

fn split_tag(image: &str) -> (String, String) {
    match image.rsplit_once(':') {
        Some((n, t)) => (n.to_string(), t.to_string()),
        None => (image.to_string(), "latest".to_string()),
    }
}
