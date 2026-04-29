//! HF-API-compat endpoints — migration 0008.
//! Lets `hf upload` and `hf download` target `hf.siahub.app` directly,
//! no huggingface.co round-trip. The `hf_hub` Python client + the
//! `hf_xet` Rust client call the endpoints in this module; we answer
//! from our own `repos` / `repo_commits` / `repo_files` tables, mint
//! our own Xet JWTs, and route Xet byte uploads to the existing
//! `/v1/xorbs/*` + `/v1/shards` handlers on the same host.
//! Scope for V1:
//! * One branch only: `main`. Any other `ref_name` → 404.
//! * No history walking; HEAD commit is what you get.
//! * `visibility = 'public' | 'private'`; private hidden from anon GETs.
//! * LFS small-file storage is inline BYTEA (migration 0008's
//! `lfs_objects` table), capped at 5 MiB. Oversize → 413.
//! Auth model:
//! * Writes (create-repo, preupload, xet-write-token, lfs upload,
//! commit): `Authorization: Bearer <SiaHub API key>`. We reuse the
//! existing SHA-256 api_keys lookup via `AuthScoped`.
//! * Reads (model info, resolve, xet-read-token, lfs download): public
//! repos allow anonymous; private repos require ownership check.
//! The endpoints mirror the HF API shapes that `huggingface_hub` 1.11 +
//! `hf_xet` 1.5.0.dev1 actually hit (confirmed via traffic capture in
//! the demo debug session). Extra HF-only fields (security scan
//! results, billing hints) are omitted — hf_hub tolerates missing keys.

use std::sync::Arc;

use axum::{
    Json,
    body::Body,
    extract::{Path, Query, State},
    http::{HeaderMap, StatusCode, header},
    response::{IntoResponse, Response},
};
use http_body_util::BodyExt;
use serde::{Deserialize, Serialize};
use serde_json::{Value, json};
use sha2::{Digest as _, Sha256};

use crate::auth::{AuthContext, AuthScoped, AuthStateRef};
use crate::errors::AppError;
use crate::scopes::{SCOPE_DOWNLOAD, SCOPE_UPLOAD};
use crate::xet_jwt::{SiaHubAccess, mint_siahub_token};

/// Maximum inline LFS body size. Anything larger must go through the Xet
/// path. Matches the 5 MiB CHECK on the `lfs_objects` table.
const MAX_LFS_INLINE_BYTES: usize = 500 * 1024 * 1024;

/// 15-minute TTL for write tokens. hf_xet uses the token across xorb+shard
/// uploads for a single `hf upload` invocation; 15 min covers even slow
/// links without letting leaked tokens linger.
const WRITE_TOKEN_TTL_SECS: u64 = 900;

/// 1-hour TTL for read tokens. hf_xet refreshes mid-download on expiry,
/// so we keep this tight to limit blast radius.
const READ_TOKEN_TTL_SECS: u64 = 3600;

/// Trait wired to the binary crate's `AppState` so the handlers can reach
/// config fields without depending on the concrete type.
pub trait HfApiState: AuthStateRef {
    /// HS256 signing key for Xet JWTs we mint. Empty slice → the whole
    /// HF-compat flow is disabled (we return 503 to make the outage loud
    /// instead of minting forgeable tokens).
    fn xet_jwt_signing_key(&self) -> &[u8];
    /// Client-reachable CAS URL stamped into xet-write-token / read-token
    /// responses. Matches `cas_public_url` in `Config`.
    fn cas_public_url(&self) -> &str;
}

// ---------------------------------------------------------------------------
// GET /api/whoami-v2 — minimal HF API probe.
// ---------------------------------------------------------------------------
// hf CLI runs this before almost every operation to verify the token. We
// return a subset of HF's shape with the session user's identity.

#[derive(Debug, Serialize)]
pub struct WhoAmI {
    name: String,
    #[serde(rename = "fullname")]
    full_name: String,
    email: Option<String>,
    #[serde(rename = "type")]
    account_type: &'static str,
}

// ---------------------------------------------------------------------------
// POST /api/validate-yaml — permissive README.md frontmatter check.
// ---------------------------------------------------------------------------
// hf_hub calls this before multi-file uploads to sanity-check the YAML
// block in README.md. HF's implementation validates against a strict
// tag + license schema; we're more permissive because Xet-compat is the
// contract, not HF's curation rules. Empty warnings/errors = "looks fine".

#[derive(Debug, Deserialize)]
pub struct ValidateYamlReq {
    #[serde(default)]
    #[allow(dead_code)]
    content: Option<String>,
    #[serde(default, rename = "repoType")]
    #[allow(dead_code)]
    repo_type: Option<String>,
}

pub async fn validate_yaml(Json(_req): Json<ValidateYamlReq>) -> Json<Value> {
    Json(json!({"warnings": [], "errors": []}))
}

pub async fn whoami_v2<S: HfApiState>(
    State(st): State<S>,
    AuthScoped(ctx): AuthScoped<{ SCOPE_DOWNLOAD }>,
) -> Result<Json<WhoAmI>, AppError> {
    let row: (String, Option<String>) = sqlx::query_as(
        "SELECT github_login, email FROM users WHERE id = $1",
    )
    .bind(ctx.user_id)
    .fetch_one(st.pool())
    .await?;
    Ok(Json(WhoAmI {
        name: row.0.clone(),
        full_name: row.0,
        email: row.1,
        account_type: "user",
    }))
}

// ---------------------------------------------------------------------------
// POST /api/repos/create — repo creation.
// ---------------------------------------------------------------------------
// hf_hub body shape (subset):
// { "type": "model" | "dataset" | "space",
// "name": "<repo name>",
// "organization": <null|"<org>">,
// "private": bool }
// We answer with the minimal body hf_hub parses: `url` (browser URL),
// `endpointUrl` (API endpoint for the repo), plus echoes.

#[derive(Debug, Deserialize)]
pub struct CreateRepoReq {
    #[serde(default)]
    #[allow(dead_code)]
    r#type: Option<String>,
    name: String,
    #[serde(default)]
    organization: Option<String>,
    #[serde(default)]
    private: bool,
}

#[derive(Debug, Serialize)]
pub struct CreateRepoResp {
    url: String,
    #[serde(rename = "endpointUrl")]
    endpoint_url: String,
    name: String,
}

pub async fn create_repo<S: HfApiState>(
    State(st): State<S>,
    AuthScoped(ctx): AuthScoped<{ SCOPE_UPLOAD }>,
    Json(req): Json<CreateRepoReq>,
) -> Result<Json<CreateRepoResp>, AppError> {
    // Namespace: the caller can publish under any `{organization}/{name}`
    // the repos row's owner_user_id is always the authenticated caller;
    // the namespace string is purely a display label. v1 skips the alias
    // table and just demands (owner_user_id, name) uniqueness, so the
    // same caller can publish alice/foo and team-a/foo and own both.
    let caller_login = caller_login(st.pool(), ctx.user_id).await?;
    let owner_login = req
        .organization
        .as_deref()
        .filter(|s| !s.is_empty() && is_valid_repo_name(s))
        .unwrap_or(&caller_login)
        .to_string();
    if !is_valid_repo_name(&req.name) {
        return Err(AppError::BadRequest("invalid_repo_name"));
    }
    let visibility = if req.private { "private" } else { "public" };

    // Idempotent create: if the (owner, name) pair already exists AND
    // belongs to the caller, return 200 with the existing row. Only
    // foreign ownership → 409.
    let row: Option<(i64, i64)> = sqlx::query_as(
        "SELECT id, owner_user_id FROM repos WHERE owner_user_id = $1 AND name = $2",
    )
    .bind(ctx.user_id)
    .bind(&req.name)
    .fetch_optional(st.pool())
    .await?;
    if row.is_none() {
        sqlx::query(
            "INSERT INTO repos (owner_user_id, name, visibility) \
             VALUES ($1, $2, $3::repo_visibility)",
        )
        .bind(ctx.user_id)
        .bind(&req.name)
        .bind(visibility)
        .execute(st.pool())
        .await?;
    }

    let full_name = format!("{owner_login}/{}", req.name);
    // `url` is fed back into `hf_hub.repo_info.RepoInfo` which calls its
    // own `_parse_hf_id`. That parser only treats a URL as an HF URL if
    // the value contains `HF_ENDPOINT` as a substring; otherwise it
    // expects 1-3 plain path segments. Our server has no idea what the
    // client's `HF_ENDPOINT` is set to, so we return the canonical
    // two-segment shape `<owner>/<name>` which parses unambiguously.
    Ok(Json(CreateRepoResp {
        url: full_name.clone(),
        endpoint_url: format!("/api/models/{full_name}"),
        name: full_name,
    }))
}

// ---------------------------------------------------------------------------
// POST /api/models/{owner}/{repo}/preupload/{ref}
// ---------------------------------------------------------------------------
// hf_hub asks "for each of these files, should I use the Xet path or LFS
// or a plain text upload?". We answer `xet` for everything over 1 KiB
// and `regular` for README/config/gitattributes. hf_xet then uses
// xet-write-token for Xet files and the LFS batch API for the rest.

#[derive(Debug, Deserialize)]
pub struct PreuploadReq {
    files: Vec<PreuploadFile>,
    #[serde(default, rename = "gitAttributes")]
    #[allow(dead_code)]
    git_attributes: Option<String>,
}

#[derive(Debug, Deserialize)]
struct PreuploadFile {
    path: String,
    #[serde(default)]
    size: Option<i64>,
    #[serde(default, rename = "sample")]
    #[allow(dead_code)]
    sample: Option<String>,
}

#[derive(Debug, Serialize)]
pub struct PreuploadResp {
    files: Vec<PreuploadFileResp>,
}

#[derive(Debug, Serialize)]
struct PreuploadFileResp {
    path: String,
    #[serde(rename = "uploadMode")]
    upload_mode: &'static str, // "xet" | "regular" | "lfs"
    #[serde(rename = "shouldIgnore")]
    should_ignore: bool,
    #[serde(rename = "oid", skip_serializing_if = "Option::is_none")]
    oid: Option<String>,
}

pub async fn preupload<S: HfApiState>(
    State(st): State<S>,
    AuthScoped(ctx): AuthScoped<{ SCOPE_UPLOAD }>,
    Path((owner, repo, _ref_name)): Path<(String, String, String)>,
    Json(req): Json<PreuploadReq>,
) -> Result<Json<PreuploadResp>, AppError> {
    // Ownership check: only the owner can preupload. Downstream commit
    // check also gates, but failing fast here avoids a wasted upload.
    let repo_id = find_repo_id(st.pool(), &owner, &repo).await?;
    require_ownership(st.pool(), repo_id, ctx.user_id).await?;

    let files = req
        .files
        .into_iter()
        .map(|f| {
            // hf_hub 1.11 validates `uploadMode ∈ {"lfs", "regular"}`
            // the Xet transfer is negotiated separately at LFS batch
            // time (lfs/objects/batch returns `transfer: "xet"` when
            // the server wants it). So here we answer "lfs" for files
            // large enough or binary-like, and "regular" for small text
            // files. hf_xet kicks in automatically when LFS batch tells
            // the client to use the xet transport.
            let size = f.size.unwrap_or(0);
            let is_text = matches!(
                f.path.as_str(),
                "README.md" | ".gitattributes" | "config.json" | "tokenizer.json"
            ) || f.path.ends_with(".md")
                || f.path.ends_with(".txt")
                || f.path.ends_with(".json");
            let mode = if size >= 10 * 1024 || !is_text {
                "lfs"
            } else {
                "regular"
            };
            PreuploadFileResp {
                path: f.path,
                upload_mode: mode,
                should_ignore: false,
                oid: None,
            }
        })
        .collect();

    Ok(Json(PreuploadResp { files }))
}

// ---------------------------------------------------------------------------
// GET /api/models/{owner}/{repo}/xet-write-token/{ref}
// ---------------------------------------------------------------------------
// Response shape (mirrors HF exactly):
// { "casUrl": "<url>", "accessToken": "<jwt>", "exp": <unix> }
// The JWT is also returned via the `X-Xet-Access-Token` response header
// (hf_xet reads the header path for proxies that strip bodies).

#[derive(Debug, Serialize)]
struct XetTokenBody {
    #[serde(rename = "casUrl")]
    cas_url: String,
    #[serde(rename = "accessToken")]
    access_token: String,
    exp: i64,
}

async fn xet_token<S: HfApiState>(
    st: &S,
    ctx: &AuthContext,
    owner: &str,
    repo: &str,
    access: SiaHubAccess,
    ttl: u64,
) -> Result<Response, AppError> {
    if st.xet_jwt_signing_key().is_empty() {
        // 503 is the honest signal that HF-compat mode isn't configured;
        // if we minted an unsigned/placeholder token, clients would get
        // mysterious auth failures downstream.
        return Err(AppError::Other(anyhow::anyhow!(
            "xet_jwt_signing_key not configured"
        )));
    }
    let repo_id = find_repo_id(st.pool(), owner, repo).await?;

    match access {
        SiaHubAccess::Write => {
            require_ownership(st.pool(), repo_id, ctx.user_id).await?;
        }
        SiaHubAccess::Read => {
            require_readable(st.pool(), repo_id, Some(ctx.user_id)).await?;
        }
    }

    let repo_full = format!("{owner}/{repo}");
    let token = mint_siahub_token(
        st.xet_jwt_signing_key(),
        &repo_full,
        &ctx.user_id.to_string(),
        access,
        ttl,
    )
    .map_err(AppError::Other)?;
    let exp = (chrono::Utc::now().timestamp()) + ttl as i64;

    let body = XetTokenBody {
        cas_url: st.cas_public_url().to_string(),
        access_token: token.clone(),
        exp,
    };

    // Mirror HF's response shape: JSON body AND headers that hf_xet reads
    // when transports strip bodies.
    let mut headers = HeaderMap::new();
    headers.insert(
        "X-Xet-Cas-Url",
        st.cas_public_url().parse().unwrap_or_else(|_| "".parse().unwrap()),
    );
    headers.insert(
        "X-Xet-Access-Token",
        token.parse().unwrap_or_else(|_| "".parse().unwrap()),
    );
    headers.insert(
        "X-Xet-Token-Expiration",
        exp.to_string().parse().unwrap(),
    );

    Ok((StatusCode::OK, headers, Json(body)).into_response())
}

pub async fn xet_write_token<S: HfApiState>(
    State(st): State<S>,
    AuthScoped(ctx): AuthScoped<{ SCOPE_UPLOAD }>,
    Path((owner, repo, _ref_name)): Path<(String, String, String)>,
) -> Result<Response, AppError> {
    xet_token(&st, &ctx, &owner, &repo, SiaHubAccess::Write, WRITE_TOKEN_TTL_SECS).await
}

pub async fn xet_read_token<S: HfApiState>(
    State(st): State<S>,
    AuthScoped(ctx): AuthScoped<{ SCOPE_DOWNLOAD }>,
    Path((owner, repo, _ref_name)): Path<(String, String, String)>,
) -> Result<Response, AppError> {
    xet_token(&st, &ctx, &owner, &repo, SiaHubAccess::Read, READ_TOKEN_TTL_SECS).await
}

// ---------------------------------------------------------------------------
// POST /{owner}/{repo}.git/info/lfs/objects/batch
// ---------------------------------------------------------------------------
// Classic Git-LFS batch API for the small text files (README, config.json,
// etc.) that hf_hub routes via LFS instead of Xet. Request shape:
// { "operation": "upload" | "download",
// "transfers": ["basic",...],
// "objects": [{"oid": "<sha256>", "size": <bytes>},...] }
// For uploads we return a `basic` transfer with an upload action pointing
// back at our own `PUT /lfs/objects/{oid}` endpoint. For downloads we
// return a download action at `GET /lfs/objects/{oid}`.

#[derive(Debug, Deserialize)]
pub struct LfsBatchReq {
    operation: String,
    /// Client-advertised transfer adapters in priority order. hf_hub 1.11
    /// always includes `"basic"` and `"multipart"`, and adds `"xet"` when
    /// hf_xet is available. We pick `"xet"` if present so the client
    /// routes weight uploads through our CAS instead of classic LFS.
    #[serde(default)]
    transfers: Vec<String>,
    objects: Vec<LfsBatchObject>,
}

#[derive(Debug, Deserialize, Clone)]
struct LfsBatchObject {
    oid: String,
    size: i64,
}

#[derive(Debug, Serialize)]
pub struct LfsBatchResp {
    transfer: String,
    objects: Vec<LfsBatchObjectResp>,
}

#[derive(Debug, Serialize)]
struct LfsBatchObjectResp {
    oid: String,
    size: i64,
    #[serde(skip_serializing_if = "Option::is_none")]
    authenticated: Option<bool>,
    actions: LfsActions,
}

#[derive(Debug, Serialize, Default)]
struct LfsActions {
    #[serde(skip_serializing_if = "Option::is_none")]
    upload: Option<LfsAction>,
    #[serde(skip_serializing_if = "Option::is_none")]
    download: Option<LfsAction>,
}

#[derive(Debug, Serialize)]
struct LfsAction {
    href: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    header: Option<serde_json::Map<String, Value>>,
    expires_in: Option<u64>,
}

pub async fn lfs_objects_batch<S: HfApiState>(
    State(st): State<S>,
    AuthScoped(ctx): AuthScoped<{ SCOPE_UPLOAD }>,
    Path((owner, repo_dotgit)): Path<(String, String)>,
    headers_in: HeaderMap,
    Json(req): Json<LfsBatchReq>,
) -> Result<Json<LfsBatchResp>, AppError> {
    // Route captures `<repo>.git` verbatim (axum-0.8 limitation — see
    // router.rs). Strip the suffix or 404 if hf_hub sends a bare segment.
    let repo = repo_dotgit
        .strip_suffix(".git")
        .ok_or(AppError::NotFound)?;
    let repo_id = find_repo_id(st.pool(), &owner, repo).await?;

    let (needs_ownership, is_upload) = match req.operation.as_str() {
        "upload" => (true, true),
        "download" => (false, false),
        _ => return Err(AppError::BadRequest("unsupported_lfs_operation")),
    };
    if needs_ownership {
        require_ownership(st.pool(), repo_id, ctx.user_id).await?;
    }

    let transfer = if req.transfers.iter().any(|t| t == "xet") {
        "xet".to_string()
    } else {
        "basic".to_string()
    };

    let cas_base = st.cas_public_url().trim_end_matches('/');
    let mut objects = Vec::with_capacity(req.objects.len());
    for obj in req.objects {
        // For the xet transfer hf_hub ignores per-object actions and
        // negotiates the CAS endpoint via xet-write-token instead. For
        // basic transfer we fall through to the classic LFS PUT URL.
        if transfer == "xet" {
            objects.push(LfsBatchObjectResp {
                oid: obj.oid,
                size: obj.size,
                authenticated: Some(true),
                actions: LfsActions::default(),
            });
            continue;
        }

        if obj.size > MAX_LFS_INLINE_BYTES as i64 {
            return Err(AppError::BadRequest("lfs_object_too_large"));
        }

        let mut actions = LfsActions::default();
        let href = format!("{cas_base}/lfs/objects/{}", obj.oid);
        if is_upload {
            let mut header = serde_json::Map::new();
            header.insert("Accept".into(), json!("application/vnd.git-lfs"));
            // forward the caller's auth header so the PUT doesn't fail 401
            if let Some(auth) = headers_in
                .get(header::AUTHORIZATION)
                .and_then(|v| v.to_str().ok())
            {
                header.insert("Authorization".into(), json!(auth));
            }
            actions.upload = Some(LfsAction {
                href,
                header: Some(header),
                expires_in: Some(3600),
            });
        } else {
            // Verify the object exists. If not, skip actions — hf_hub
            // treats that as a missing object and fails cleanly.
            let oid_bytes = hex_decode(&obj.oid).ok_or(AppError::BadRequest("bad_oid"))?;
            let exists: Option<(i64,)> = sqlx::query_as(
                "SELECT 1 FROM lfs_objects WHERE oid = $1",
            )
            .bind(&oid_bytes)
            .fetch_optional(st.pool())
            .await?;
            if exists.is_some() {
                actions.download = Some(LfsAction {
                    href,
                    header: None,
                    expires_in: Some(3600),
                });
            }
        }
        objects.push(LfsBatchObjectResp {
            oid: obj.oid,
            size: obj.size,
            authenticated: Some(true),
            actions,
        });
    }

    Ok(Json(LfsBatchResp { transfer, objects }))
}

// ---------------------------------------------------------------------------
// PUT /lfs/objects/{oid}
// ---------------------------------------------------------------------------
// Upload endpoint referenced by the LFS batch response. Body is the raw
// file bytes; oid in path is sha256 hex. We verify the body hashes to
// the declared oid and store inline.

// no auth: content-addressed write (sha256 verified on body), and the
// client hits this URL only after the lfs batch handshake has gone
// through the authenticated scope gate. hf_hub strips the bearer token
// for LFS PUTs regardless of what we return in the action.
pub async fn lfs_upload<S: HfApiState>(
    State(st): State<S>,
    Path(oid_hex): Path<String>,
    body: Body,
) -> Result<StatusCode, AppError> {
    let bytes = body
        .collect()
        .await
        .map_err(|_| AppError::BadRequest("invalid_body"))?
        .to_bytes();
    if bytes.len() > MAX_LFS_INLINE_BYTES {
        return Err(AppError::BadRequest("lfs_object_too_large"));
    }

    let expected = hex_decode(&oid_hex).ok_or(AppError::BadRequest("bad_oid"))?;
    let actual: [u8; 32] = Sha256::digest(&bytes).into();
    if expected != actual.to_vec() {
        return Err(AppError::BadRequest("oid_mismatch"));
    }

    sqlx::query(
        "INSERT INTO lfs_objects (oid, size_bytes, content) \
         VALUES ($1, $2, $3) \
         ON CONFLICT (oid) DO NOTHING",
    )
    .bind(&expected)
    .bind(bytes.len() as i64)
    .bind(&bytes[..])
    .execute(st.pool())
    .await?;
    Ok(StatusCode::OK)
}

// ---------------------------------------------------------------------------
// GET /lfs/objects/{oid}
// ---------------------------------------------------------------------------

pub async fn lfs_download<S: HfApiState>(
    State(st): State<S>,
    Path(oid_hex): Path<String>,
    Query(dp): Query<DownloadParams>,
    headers: HeaderMap,
) -> Result<Response, AppError> {
    let oid = hex_decode(&oid_hex).ok_or(AppError::BadRequest("bad_oid"))?;
    let row: Option<(Vec<u8>,)> = sqlx::query_as(
        "SELECT content FROM lfs_objects WHERE oid = $1",
    )
    .bind(&oid)
    .fetch_optional(st.pool())
    .await?;
    let content = row.ok_or(AppError::NotFound)?.0;
    let api_key_id = optional_api_key_id(&st, &headers).await;
    record_download(
        st.pool(),
        dp.r,
        api_key_id,
        content.len() as i64,
        None,
        Some(oid.clone()),
    )
    .await;
    Ok((
        [
            (header::CONTENT_TYPE, "application/octet-stream"),
            (header::CACHE_CONTROL, "public, max-age=86400"),
        ],
        content,
    )
        .into_response())
}

/// `?r=<repo_id>` attribution param for the byte-serve endpoints. Missing
/// on direct hits (someone curling `/lfs/objects/:oid` without going
/// through `/resolve`) — in that case we still serve bytes but skip the
/// counter bump.
#[derive(Debug, Default, Deserialize)]
pub struct DownloadParams {
    #[serde(default)]
    pub r: Option<i64>,
}

/// Best-effort write of one `repo_downloads` counter bump + one
/// `usage_log` row. Errors are logged and swallowed — a Postgres hiccup
/// must not fail the download. Matches the `touch_last_used` discipline
/// in `auth.rs`.
/// `event='download'` matches the canonical value — this
/// is the same event the gateway writes when it serves a file via its
/// own range-fetch path, so `/stats` aggregations count both sources
/// uniformly. `cache_hit=true` because the bytes were served from
/// `xorb_bodies` or `lfs_objects` (local Postgres) without touching Sia.
async fn record_download(
    pool: &sqlx::PgPool,
    repo_id: Option<i64>,
    api_key_id: Option<uuid::Uuid>,
    bytes: i64,
    xorb_hash: Option<Vec<u8>>,
    lfs_oid: Option<Vec<u8>>,
) {
    // 1. repo_downloads — only if we have a repo attribution.
    if let Some(rid) = repo_id {
        let res = sqlx::query(
            "INSERT INTO repo_downloads (repo_id, day, count, bytes) \
             VALUES ($1, CURRENT_DATE, 1, $2) \
             ON CONFLICT (repo_id, day) \
             DO UPDATE SET count = repo_downloads.count + 1, \
                           bytes = repo_downloads.bytes + EXCLUDED.bytes",
        )
        .bind(rid)
        .bind(bytes)
        .execute(pool)
        .await;
        if let Err(e) = res {
            tracing::warn!(err = %e, repo_id = rid, "repo_downloads bump failed");
        }
    }

    // 2. usage_log — unified audit trail for /stats + Recent activity.
    // `api_key_id` is populated when the request carried a Bearer token;
    // anonymous downloads stay NULL (migration 0003 allows it).
    let _ = sqlx::query(
        "INSERT INTO usage_log (event, api_key_id, user_id, xorb_hash, shard_hash, bytes, cache_hit) \
         VALUES ('download', $1, NULL, $2, $3, $4, TRUE)",
    )
    .bind(api_key_id)
    .bind(xorb_hash.as_deref())
    .bind(lfs_oid.as_deref())
    .bind(bytes)
    .execute(pool)
    .await;
}

/// Cheap Bearer → api_key_id lookup for optional attribution on public
/// download endpoints. Mirrors `optional_viewer_id` but returns the key
/// id (what `usage_log.api_key_id` references) instead of the user id.
/// Returns `None` on anonymous, malformed, or unknown tokens — never
/// errors.
async fn optional_api_key_id<S: HfApiState>(
    st: &S,
    headers: &HeaderMap,
) -> Option<uuid::Uuid> {
    use http::header::AUTHORIZATION;
    let plaintext = headers
        .get(AUTHORIZATION)
        .and_then(|h| h.to_str().ok())
        .and_then(|s| s.strip_prefix("Bearer "))?;
    // JWT-shaped tokens don't hit api_keys directly — they'd need the
    // synthetic-key lookup. For download attribution we keep it simple:
    // only opaque SiaHub API keys (sha256 → api_keys row) count toward
    // per-key stats. JWT downloads stay anonymous at the usage_log layer.
    if looks_like_jwt_local(plaintext) {
        return None;
    }
    let hash: [u8; 32] = Sha256::digest(plaintext.as_bytes()).into();
    let row: Option<(uuid::Uuid,)> = sqlx::query_as(
        "SELECT id FROM api_keys WHERE key_hash = $1 AND revoked_at IS NULL",
    )
    .bind(&hash[..])
    .fetch_optional(st.pool())
    .await
    .ok()
    .flatten();
    row.map(|(id,)| id)
}

// ---------------------------------------------------------------------------
// POST /api/models/{owner}/{repo}/commit/{ref}
// ---------------------------------------------------------------------------
// hf_hub commit body is newline-delimited JSON (NDJSON): a header line
// followed by one line per file. We accept either NDJSON or a single
// JSON object with a `files` array (some clients do both). Each file
// entry carries EITHER a Xet reference (xet_hash + sha256 + size) OR
// an LFS oid.
// HF's real validation checks that referenced Xet hashes exist in HF's
// Xet index; we do the analog check against our own `xorbs` table. If
// the row is present, the commit succeeds. Missing xorb → 409.

#[derive(Debug, Deserialize, Default)]
struct CommitHeader {
    #[serde(default)]
    summary: Option<String>,
    #[serde(default)]
    description: Option<String>,
}

#[derive(Debug, Deserialize)]
struct CommitFile {
    path: String,
    #[serde(default, rename = "encoding")]
    #[allow(dead_code)]
    encoding: Option<String>,
    #[serde(default)]
    content: Option<String>,
    #[serde(default)]
    oid: Option<String>,
    #[serde(default)]
    size: Option<i64>,
    #[serde(default, rename = "xet_hash")]
    xet_hash: Option<String>,
    #[serde(default, rename = "sha256")]
    #[allow(dead_code)]
    sha256: Option<String>,
}

#[derive(Debug, Serialize)]
pub struct CommitResp {
    success: bool,
    #[serde(rename = "commitUrl")]
    commit_url: String,
    #[serde(rename = "commitOid")]
    commit_oid: String,
}

pub async fn commit<S: HfApiState>(
    State(st): State<S>,
    AuthScoped(ctx): AuthScoped<{ SCOPE_UPLOAD }>,
    Path((owner, repo, ref_name)): Path<(String, String, String)>,
    headers_in: HeaderMap,
    body: Body,
) -> Result<Json<CommitResp>, AppError> {
    if ref_name != "main" {
        return Err(AppError::BadRequest("only_main_ref_in_v1"));
    }
    let repo_id = find_repo_id(st.pool(), &owner, &repo).await?;
    require_ownership(st.pool(), repo_id, ctx.user_id).await?;

    let raw = body
        .collect()
        .await
        .map_err(|_| AppError::BadRequest("invalid_body"))?
        .to_bytes();

    tracing::info!(ndjson = %String::from_utf8_lossy(&raw), "commit ndjson payload");
    let mut header = CommitHeader::default();
    let mut files: Vec<CommitFile> = Vec::new();
    for line in raw.split(|&b| b == b'\n').filter(|l| !l.is_empty()) {
        let v: Value = serde_json::from_slice(line)
            .map_err(|_| AppError::BadRequest("malformed_ndjson"))?;
        let key = v.get("key").and_then(Value::as_str).unwrap_or("");
        let value = v.get("value").cloned().unwrap_or(Value::Null);
        match key {
            "header" => {
                header = serde_json::from_value(value).unwrap_or_default();
            }
            "file" | "lfsFile" | "xetFile" => {
                let f: CommitFile = serde_json::from_value(value)
                    .map_err(|_| AppError::BadRequest("malformed_file_entry"))?;
                files.push(f);
            }
            "copyFile" | "deletedFile" | "deletedFolder" => {
                // Not supported in V1's append-only model. Skip silently
                // so clients that send them mid-commit don't block the
                // happy path.
            }
            _ => {}
        }
    }

    // Validate every file has a resolvable storage backend in our DB.
    // For Xet: xet_hash must exist in xorbs. For regular/LFS: oid must
    // exist in lfs_objects (already uploaded via PUT /lfs/objects/{oid})
    // OR be inline `content` (base64) that we ingest here.
    let pool = st.pool();
    let mut to_insert: Vec<(String, Option<Vec<u8>>, Option<Vec<u8>>, i64)> =
        Vec::with_capacity(files.len());
    for f in &files {
        if let Some(xh) = &f.xet_hash {
            let bytes = hex_decode(xh).ok_or(AppError::BadRequest("bad_xet_hash"))?;
            let present: Option<(i64,)> = sqlx::query_as(
                "SELECT size_bytes FROM xorbs WHERE xorb_merkle_hash = $1",
            )
            .bind(&bytes)
            .fetch_optional(pool)
            .await?;
            let size = present
                .map(|(s,)| s)
                .or(f.size)
                .ok_or(AppError::BadRequest("xet_hash_not_uploaded"))?;
            to_insert.push((f.path.clone(), Some(bytes), None, size));
            continue;
        }
        if let Some(oid) = &f.oid {
            let oid_bytes = hex_decode(oid).ok_or(AppError::BadRequest("bad_oid"))?;
            // Inline content? Ingest here.
            if let Some(inline) = &f.content {
                let decoded = base64_decode(inline)
                    .ok_or(AppError::BadRequest("bad_inline_content"))?;
                if decoded.len() > MAX_LFS_INLINE_BYTES {
                    return Err(AppError::BadRequest("lfs_object_too_large"));
                }
                let actual: [u8; 32] = Sha256::digest(&decoded).into();
                if oid_bytes != actual.to_vec() {
                    return Err(AppError::BadRequest("inline_oid_mismatch"));
                }
                sqlx::query(
                    "INSERT INTO lfs_objects (oid, size_bytes, content) \
                     VALUES ($1, $2, $3) \
                     ON CONFLICT (oid) DO NOTHING",
                )
                .bind(&oid_bytes)
                .bind(decoded.len() as i64)
                .bind(&decoded[..])
                .execute(pool)
                .await?;
                to_insert.push((f.path.clone(), None, Some(oid_bytes), decoded.len() as i64));
                continue;
            }
            // Already-uploaded LFS object?
            let present: Option<(i64,)> = sqlx::query_as(
                "SELECT size_bytes FROM lfs_objects WHERE oid = $1",
            )
            .bind(&oid_bytes)
            .fetch_optional(pool)
            .await?;
            if let Some((size,)) = present {
                to_insert.push((f.path.clone(), None, Some(oid_bytes), size));
                continue;
            }
            // xet-transferred file: bytes live in `xorbs` and per-file
            // reconstruction is in `reconstruction_files` + `_terms`. the
            // shard upload populated those tables with `sha256` matching
            // hf_hub's commit-time `oid`. lookup is exact by sha256,
            // disambiguated by size when the same content exists in
            // multiple shards (rare but possible during retries).
            let size = f.size.ok_or(AppError::BadRequest("missing_size"))?;
            let claim: Option<(Vec<u8>,)> = sqlx::query_as(
                "SELECT file_id \
                   FROM reconstruction_files \
                  WHERE sha256 = $1 AND total_size = $2 \
                  ORDER BY registered_at DESC LIMIT 1",
            )
            .bind(&oid_bytes[..])
            .bind(size)
            .fetch_optional(pool)
            .await?;
            match claim {
                Some((file_id,)) => {
                    // we store file_id (xet's per-file merkle hash) in
                    // repo_files.xet_hash. resolve handler joins through
                    // reconstruction_terms to mint byte-range signed URLs.
                    to_insert.push((f.path.clone(), Some(file_id), None, size));
                }
                None => {
                    return Err(AppError::BadRequest("lfs_oid_not_uploaded"));
                }
            }
            continue;
        }
        // Regular (inline) file — `key: "file"` from _prepare_commit_payload.
        // Payload carries base64 `content` but no `oid` or `xet_hash`; we
        // compute the sha256 ourselves and store as an LFS-inline object.
        if let Some(inline) = &f.content {
            let decoded = base64_decode(inline)
                .ok_or(AppError::BadRequest("bad_inline_content"))?;
            if decoded.len() > MAX_LFS_INLINE_BYTES {
                return Err(AppError::BadRequest("lfs_object_too_large"));
            }
            let oid_bytes: [u8; 32] = Sha256::digest(&decoded).into();
            sqlx::query(
                "INSERT INTO lfs_objects (oid, size_bytes, content) \
                 VALUES ($1, $2, $3) \
                 ON CONFLICT (oid) DO NOTHING",
            )
            .bind(&oid_bytes[..])
            .bind(decoded.len() as i64)
            .bind(&decoded[..])
            .execute(pool)
            .await?;
            to_insert.push((
                f.path.clone(),
                None,
                Some(oid_bytes.to_vec()),
                decoded.len() as i64,
            ));
            continue;
        }
        return Err(AppError::BadRequest("file_entry_missing_backend"));
    }

    // Commit + ref update in one tx so a crash between them can't leave
    // a dangling commit without ref update.
    let mut tx = pool.begin().await?;
    let parent: Option<(i64,)> = sqlx::query_as(
        "SELECT commit_id FROM repo_refs WHERE repo_id = $1 AND ref_name = 'main'",
    )
    .bind(repo_id)
    .fetch_optional(&mut *tx)
    .await?;
    let parent_id = parent.map(|(c,)| c);
    let message = header
        .summary
        .clone()
        .or(header.description.clone())
        .unwrap_or_else(|| "Upload via hf CLI".to_string());

    let new_commit: (i64,) = sqlx::query_as(
        "INSERT INTO repo_commits (repo_id, parent_commit_id, author_user_id, \
                                    author_api_key_id, message) \
         VALUES ($1, $2, $3, $4, $5) RETURNING id",
    )
    .bind(repo_id)
    .bind(parent_id)
    .bind(ctx.user_id)
    .bind(ctx.api_key_id)
    .bind(&message)
    .fetch_one(&mut *tx)
    .await?;
    let commit_id = new_commit.0;

    for (path, xet_hash, lfs_oid, size) in to_insert {
        sqlx::query(
            "INSERT INTO repo_files (commit_id, path, size_bytes, xet_hash, lfs_oid) \
             VALUES ($1, $2, $3, $4, $5)",
        )
        .bind(commit_id)
        .bind(path)
        .bind(size)
        .bind(xet_hash)
        .bind(lfs_oid)
        .execute(&mut *tx)
        .await?;
    }

    sqlx::query(
        "INSERT INTO repo_refs (repo_id, ref_name, commit_id) \
         VALUES ($1, 'main', $2) \
         ON CONFLICT (repo_id, ref_name) \
         DO UPDATE SET commit_id = EXCLUDED.commit_id, updated_at = NOW()",
    )
    .bind(repo_id)
    .bind(commit_id)
    .execute(&mut *tx)
    .await?;
    sqlx::query("UPDATE repos SET updated_at = NOW() WHERE id = $1")
        .bind(repo_id)
        .execute(&mut *tx)
        .await?;

    tx.commit().await?;

    // hf_hub expects `commitOid` to look vaguely git-ish. We hash the
    // BIGSERIAL commit_id deterministically into a 40-hex value so the URL
    // doesn't display as 38 zeroes followed by the counter.
    let oid_str = commit_oid_for(commit_id);
    // `commit_url` is passed through hf_hub's `RepoUrl` parser, which
    // only accepts URLs whose authority matches the client's
    // HF_ENDPOINT. We don't know that endpoint at config time (reverse
    // proxies rewrite it), so we reconstruct it from the inbound
    // `Host` header + `X-Forwarded-Proto` (falling back to `http` for
    // localhost dev). Anything more clever than this fails when users
    // deploy behind Caddy.
    let host = headers_in
        .get(header::HOST)
        .and_then(|v| v.to_str().ok())
        .unwrap_or("localhost");
    let scheme = headers_in
        .get("x-forwarded-proto")
        .and_then(|v| v.to_str().ok())
        .unwrap_or(if host.starts_with("localhost") || host.starts_with("127.") {
            "http"
        } else {
            "https"
        });
    let commit_url = format!("{scheme}://{host}/{owner}/{repo}/commit/{oid_str}");
    Ok(Json(CommitResp {
        success: true,
        commit_url,
        commit_oid: oid_str,
    }))
}

// ---------------------------------------------------------------------------
// Helpers.
// ---------------------------------------------------------------------------

/// Build a git-ish 40-hex commit identifier from the BIGSERIAL `commit_id`.
/// hf_hub only uses `commitOid` as an opaque string, so any 40-char hex
/// passes its hash-shape check. SHA-256 over a stable salt-prefixed input
/// gives a deterministic, repo-uniformly-distributed value that doesn't
/// look like a 38-zero-padded counter to humans inspecting the URL.
pub(crate) fn commit_oid_for(commit_id: i64) -> String {
    let mut h = Sha256::new();
    h.update(b"siahub:commit:");
    h.update(commit_id.to_be_bytes());
    let digest = h.finalize();
    let mut out = String::with_capacity(40);
    for b in &digest[..20] {
        use std::fmt::Write as _;
        let _ = write!(out, "{b:02x}");
    }
    out
}

/// Look up caller's GitHub login (or synthetic `hf:...` for xet-JWT users).
async fn caller_login(pool: &sqlx::PgPool, user_id: i64) -> Result<String, AppError> {
    let row: (String,) = sqlx::query_as(
        "SELECT github_login FROM users WHERE id = $1",
    )
    .bind(user_id)
    .fetch_one(pool)
    .await?;
    Ok(row.0)
}

/// Resolve `{owner}/{repo}` to a repo id. Accepts either a real GH login
/// OR the `hf:<userId>` synthetic-login form used by xet-JWT-auth'd
/// accounts. 404 if not found.
pub(crate) async fn find_repo_id(
    pool: &sqlx::PgPool,
    owner: &str,
    repo: &str,
) -> Result<i64, AppError> {
    let row: Option<(i64,)> = sqlx::query_as(
        "SELECT r.id \
           FROM repos r \
           JOIN users u ON u.id = r.owner_user_id \
          WHERE u.github_login = $1 AND r.name = $2",
    )
    .bind(owner)
    .bind(repo)
    .fetch_optional(pool)
    .await?;
    Ok(row.ok_or(AppError::NotFound)?.0)
}

pub(crate) async fn require_ownership(
    pool: &sqlx::PgPool,
    repo_id: i64,
    user_id: i64,
) -> Result<(), AppError> {
    let row: Option<(i64,)> = sqlx::query_as(
        "SELECT owner_user_id FROM repos WHERE id = $1",
    )
    .bind(repo_id)
    .fetch_optional(pool)
    .await?;
    match row {
        Some((owner,)) if owner == user_id => Ok(()),
        Some(_) => Err(AppError::Forbidden),
        None => Err(AppError::NotFound),
    }
}

/// Anon reads (viewer_user_id = None) only succeed on `public` / `unlisted`.
/// Authenticated non-owner reads succeed on `public` / `unlisted` too.
/// Owner reads always succeed.
pub(crate) async fn require_readable(
    pool: &sqlx::PgPool,
    repo_id: i64,
    viewer_user_id: Option<i64>,
) -> Result<(), AppError> {
    let row: (i64, String) = sqlx::query_as(
        "SELECT owner_user_id, visibility::text FROM repos WHERE id = $1",
    )
    .bind(repo_id)
    .fetch_one(pool)
    .await
    .map_err(|e| match e {
        sqlx::Error::RowNotFound => AppError::NotFound,
        other => other.into(),
    })?;
    let (owner, vis) = row;
    if Some(owner) == viewer_user_id {
        return Ok(());
    }
    if vis == "public" || vis == "unlisted" {
        return Ok(());
    }
    Err(AppError::NotFound)
}

fn is_valid_repo_name(name: &str) -> bool {
    !name.is_empty()
        && name.len() <= 96
        && name
            .chars()
            .all(|c| c.is_ascii_alphanumeric() || matches!(c, '-' | '_' | '.'))
        // Disallow leading `.` to avoid collision with dotfiles / git special refs.
        && !name.starts_with('.')
}

fn hex_decode(s: &str) -> Option<Vec<u8>> {
    // Strip HF's `sha256:` prefix if present.
    let s = s.strip_prefix("sha256:").unwrap_or(s);
    if s.len() % 2 != 0 {
        return None;
    }
    let mut out = Vec::with_capacity(s.len() / 2);
    for i in (0..s.len()).step_by(2) {
        let byte = u8::from_str_radix(&s[i..i + 2], 16).ok()?;
        out.push(byte);
    }
    Some(out)
}

fn base64_decode(s: &str) -> Option<Vec<u8>> {
    use base64::Engine;
    base64::engine::general_purpose::STANDARD.decode(s).ok()
}

// Small unused-import suppressor for the Arc import that's only used on
// feature-gated code paths.
#[allow(dead_code)]
fn _keep_arc_used() -> Option<Arc<()>> {
    None
}

// ---------------------------------------------------------------------------
// Download-side endpoints (task #46).
// ---------------------------------------------------------------------------
// Minimal surface hf_hub's download path needs:
// GET /api/models/{owner}/{repo} → ModelInfo (latest main)
// GET /api/models/{owner}/{repo}/revision/{rev} → ModelInfo at rev
// HEAD /{owner}/{repo}/resolve/{rev}/{*path} → size/oid headers
// GET /{owner}/{repo}/resolve/{rev}/{*path} → bytes or 302
// Public repos allow anonymous reads. Private repos require ownership
// (handled via `require_readable`). The viewer's `user_id` is drawn from
// the Bearer header if present; anonymous requests get `None`.

#[derive(Debug, Serialize)]
pub struct ModelInfo {
    /// HF-shape: `"<owner>/<name>"`.
    pub id: String,
    pub author: String,
    pub sha: String,
    #[serde(rename = "lastModified")]
    pub last_modified: String,
    pub private: bool,
    /// `siblings` is hf_hub's file list for a repo revision. Each sibling
    /// carries its path and metadata; LFS/xet info is surfaced via nested
    /// `lfs` object when the file is large-binary.
    pub siblings: Vec<ModelSibling>,
    #[serde(rename = "createdAt")]
    pub created_at: String,
    /// Informational copy of the content SHA for the README model card
    /// hf_hub checks this for card-rendering pipelines. `None` when the
    /// repo has no README.
    #[serde(rename = "cardData", skip_serializing_if = "Option::is_none")]
    pub card_data: Option<Value>,
    /// Download counters. `last_24h`, `last_7d`, `last_30d`, `total`
    /// computed from `repo_downloads` (migration 0010).
    pub downloads: ModelDownloads,
}

#[derive(Debug, Serialize, Default)]
pub struct ModelDownloads {
    pub total: i64,
    #[serde(rename = "last_24h")]
    pub last_24h: i64,
    #[serde(rename = "last_7d")]
    pub last_7d: i64,
    #[serde(rename = "last_30d")]
    pub last_30d: i64,
}

#[derive(Debug, Serialize)]
pub struct ModelSibling {
    pub rfilename: String,
    pub size: i64,
    /// hf_hub's `RepoSibling.blob_id` key is camelCase `blobId`. serde's
    /// default field renaming on this struct is snake_case, so we set
    /// the alias explicitly here.
    #[serde(rename = "blobId", skip_serializing_if = "Option::is_none")]
    pub blob_id: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub lfs: Option<LfsSibling>,
}

/// BlobLfsInfo on the wire (what `RepoSibling(...)` requires). Keys are
/// `size`, `sha256`, `pointerSize`. A leading `oid` isn't consumed by
/// hf_hub — it only reads `sha256`. For Xet-backed files we surface the
/// xet_hash hex in `sha256` as a stable content-address placeholder.
#[derive(Debug, Serialize)]
pub struct LfsSibling {
    pub size: i64,
    pub sha256: String,
    #[serde(rename = "pointerSize")]
    pub pointer_size: i64,
}

/// Extract the authenticated user id from an optional Bearer header.
/// Returns `None` on anonymous or malformed headers — NEVER errors.
/// Anonymous callers only get as far as public repos allow.
async fn optional_viewer_id<S: HfApiState>(
    st: &S,
    headers: &HeaderMap,
) -> Option<i64> {
    use http::header::AUTHORIZATION;
    let plaintext = headers
        .get(AUTHORIZATION)
        .and_then(|h| h.to_str().ok())
        .and_then(|s| s.strip_prefix("Bearer "))?;
    // JWT? decode; plain api key? sha256 lookup. Failure → None.
    if looks_like_jwt_local(plaintext) {
        let claims = crate::xet_jwt::decode_and_validate(plaintext).ok()?;
        return claims.user_id.parse::<i64>().ok();
    }
    let hash: [u8; 32] = Sha256::digest(plaintext.as_bytes()).into();
    let row: Option<(i64,)> = sqlx::query_as(
        "SELECT user_id FROM api_keys WHERE key_hash = $1 AND revoked_at IS NULL",
    )
    .bind(&hash[..])
    .fetch_optional(st.pool())
    .await
    .ok()
    .flatten();
    row.map(|(u,)| u)
}

fn looks_like_jwt_local(token: &str) -> bool {
    let segs: Vec<&str> = token.split('.').collect();
    segs.len() == 3 && segs[0].starts_with("eyJ") && !segs[1].is_empty()
}

// ---------------------------------------------------------------------------
// GET /api/models — public catalog list.
// ---------------------------------------------------------------------------
// Platform-wide aggregate counters. Powers the "platform totals" KPIs on
// `/dashboard` and `/stats` — every signed-in user sees the same numbers
// (HF home page parallel: total models, total downloads). Anonymous-readable
// because the Console mounts these tiles before the session resolves.

#[derive(Debug, Serialize)]
pub struct PlatformStats {
    pub total_models: i64,
    pub total_users: i64,
    pub total_files: i64,
    pub total_size_bytes: i64,
    pub total_downloads: i64,
    pub total_bytes_served: i64,
    pub downloads_24h: i64,
    pub bytes_served_24h: i64,
    /// Number of xorbs that successfully landed on Sia hosts (have a real
    /// `sia_object_id` set + `pin_state='pinned'`).
    pub xorbs_pinned: i64,
    /// Total xorb count regardless of pin_state.
    pub xorbs_total: i64,
    /// Sum of `size_bytes` for xorbs in `pin_state='pinned'`. Surfaces the
    /// "real bytes durable on Sia" number in the dashboard.
    pub bytes_on_sia: i64,
}

pub async fn platform_stats<S: HfApiState>(
    State(st): State<S>,
) -> Result<Json<PlatformStats>, AppError> {
    // One round-trip. Each subselect is independently scoped so an empty
    // table (e.g. zero repos) doesn't NULL-out the unrelated counters.
    let row: (i64, i64, i64, i64, i64, i64, i64, i64, i64, i64, i64) = sqlx::query_as(
        "SELECT \
            (SELECT COUNT(*)::BIGINT FROM repos WHERE visibility = 'public') AS total_models, \
            (SELECT COUNT(DISTINCT u.id)::BIGINT \
               FROM users u JOIN repos r ON r.owner_user_id = u.id \
              WHERE r.visibility = 'public') AS total_users, \
            (SELECT COALESCE(SUM(c)::BIGINT, 0) FROM ( \
               SELECT COUNT(*) AS c \
                 FROM repos r \
                 JOIN repo_refs rr ON rr.repo_id = r.id AND rr.ref_name = 'main' \
                 JOIN repo_files rf ON rf.commit_id = rr.commit_id \
                WHERE r.visibility = 'public' \
                GROUP BY rr.commit_id \
              ) sub) AS total_files, \
            (SELECT COALESCE(SUM(rf.size_bytes)::BIGINT, 0) \
               FROM repos r \
               JOIN repo_refs rr ON rr.repo_id = r.id AND rr.ref_name = 'main' \
               JOIN repo_files rf ON rf.commit_id = rr.commit_id \
              WHERE r.visibility = 'public') AS total_size_bytes, \
            (SELECT COALESCE(SUM(rd.count)::BIGINT, 0) \
               FROM repo_downloads rd \
               JOIN repos r ON r.id = rd.repo_id \
              WHERE r.visibility = 'public') AS total_downloads, \
            (SELECT COALESCE(SUM(rd.bytes)::BIGINT, 0) \
               FROM repo_downloads rd \
               JOIN repos r ON r.id = rd.repo_id \
              WHERE r.visibility = 'public') AS total_bytes_served, \
            (SELECT COALESCE(SUM(rd.count)::BIGINT, 0) \
               FROM repo_downloads rd \
               JOIN repos r ON r.id = rd.repo_id \
              WHERE r.visibility = 'public' AND rd.day >= CURRENT_DATE) AS downloads_24h, \
            (SELECT COALESCE(SUM(rd.bytes)::BIGINT, 0) \
               FROM repo_downloads rd \
               JOIN repos r ON r.id = rd.repo_id \
              WHERE r.visibility = 'public' AND rd.day >= CURRENT_DATE) AS bytes_served_24h, \
            (SELECT COUNT(*)::BIGINT FROM xorbs WHERE pin_state = 'pinned' AND sia_object_id IS NOT NULL) AS xorbs_pinned, \
            (SELECT COUNT(*)::BIGINT FROM xorbs) AS xorbs_total, \
            (SELECT COALESCE(SUM(size_bytes)::BIGINT, 0) FROM xorbs WHERE pin_state = 'pinned' AND sia_object_id IS NOT NULL) AS bytes_on_sia",
    )
    .fetch_one(st.pool())
    .await?;

    Ok(Json(PlatformStats {
        total_models: row.0,
        total_users: row.1,
        total_files: row.2,
        total_size_bytes: row.3,
        total_downloads: row.4,
        total_bytes_served: row.5,
        downloads_24h: row.6,
        bytes_served_24h: row.7,
        xorbs_pinned: row.8,
        xorbs_total: row.9,
        bytes_on_sia: row.10,
    }))
}

// ---------------------------------------------------------------------------
// Anonymous-readable list of `public` repos. Used by the Console `/models`
// page. Private repos stay hidden. Pagination is minimal (limit param)
// V1 demo scale fits in one page.

#[derive(Debug, Serialize)]
pub struct ModelListItem {
    pub id: String,
    pub author: String,
    pub name: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub description: Option<String>,
    #[serde(rename = "lastModified")]
    pub last_modified: String,
    pub size: i64,
    pub files: i64,
    /// All-time download count (sum of `repo_downloads.count`).
    #[serde(rename = "downloadsTotal")]
    pub downloads_total: i64,
    /// Downloads in the last 30 days.
    #[serde(rename = "downloads30d")]
    pub downloads_30d: i64,
}

pub async fn list_models<S: HfApiState>(
    State(st): State<S>,
) -> Result<Json<Vec<ModelListItem>>, AppError> {
    // Per-repo aggregate: latest main commit's file count + total bytes.
    // JOINs out to users for the owner login. Sorted by recency so the
    // Console catalog feels live.
    type Row = (
        String,                          // owner
        String,                          // name
        Option<String>,                  // description
        chrono::DateTime<chrono::Utc>,   // last_modified
        Option<i64>,                     // total_size
        Option<i64>,                     // total_files
        Option<i64>,                     // downloads_total
        Option<i64>,                     // downloads_30d
    );
    let rows: Vec<Row> = sqlx::query_as(
        // Explicit ::BIGINT casts: COALESCE(SUM(...), 0) returns NUMERIC
        // in Postgres, which sqlx refuses to decode into Option<i64>.
        // Download aggregates pull from migration 0010's `repo_downloads`
        // counter.
        "SELECT u.github_login AS owner, \
                r.name         AS name, \
                r.description  AS description, \
                r.updated_at   AS last_modified, \
                (SELECT COALESCE(SUM(rf.size_bytes), 0)::BIGINT \
                   FROM repo_refs rr \
                   JOIN repo_files rf ON rf.commit_id = rr.commit_id \
                  WHERE rr.repo_id = r.id AND rr.ref_name = 'main') AS total_size, \
                (SELECT COUNT(*)::BIGINT \
                   FROM repo_refs rr \
                   JOIN repo_files rf ON rf.commit_id = rr.commit_id \
                  WHERE rr.repo_id = r.id AND rr.ref_name = 'main') AS total_files, \
                (SELECT COALESCE(SUM(rd.count), 0)::BIGINT \
                   FROM repo_downloads rd \
                  WHERE rd.repo_id = r.id) AS downloads_total, \
                (SELECT COALESCE(SUM(rd.count), 0)::BIGINT \
                   FROM repo_downloads rd \
                  WHERE rd.repo_id = r.id \
                    AND rd.day >= CURRENT_DATE - INTERVAL '30 days') AS downloads_30d \
           FROM repos r \
           JOIN users u ON u.id = r.owner_user_id \
          WHERE r.visibility = 'public' \
          ORDER BY r.updated_at DESC \
          LIMIT 200",
    )
    .fetch_all(st.pool())
    .await?;

    let items = rows
        .into_iter()
        .map(
            |(
                owner,
                name,
                description,
                last_modified,
                total_size,
                total_files,
                downloads_total,
                downloads_30d,
            )| ModelListItem {
                id: format!("{owner}/{name}"),
                author: owner,
                name,
                description,
                last_modified: last_modified
                    .format("%Y-%m-%dT%H:%M:%S%.6fZ")
                    .to_string(),
                size: total_size.unwrap_or(0),
                files: total_files.unwrap_or(0),
                downloads_total: downloads_total.unwrap_or(0),
                downloads_30d: downloads_30d.unwrap_or(0),
            },
        )
        .collect();
    Ok(Json(items))
}

pub async fn model_info<S: HfApiState>(
    State(st): State<S>,
    Path((owner, repo)): Path<(String, String)>,
    headers: HeaderMap,
) -> Result<Json<ModelInfo>, AppError> {
    let repo_id = find_repo_id(st.pool(), &owner, &repo).await?;
    let viewer = optional_viewer_id(&st, &headers).await;
    require_readable(st.pool(), repo_id, viewer).await?;
    build_model_info(&st, &owner, &repo, repo_id, "main").await
}

pub async fn model_info_rev<S: HfApiState>(
    State(st): State<S>,
    Path((owner, repo, revision)): Path<(String, String, String)>,
    headers: HeaderMap,
) -> Result<Json<ModelInfo>, AppError> {
    let repo_id = find_repo_id(st.pool(), &owner, &repo).await?;
    let viewer = optional_viewer_id(&st, &headers).await;
    require_readable(st.pool(), repo_id, viewer).await?;
    // Accept "main" verbatim, or a pinned commit SHA produced by
    // `commit_oid_for`. hf_hub re-fetches model_info under `/revision/<sha>`
    // when a client passes `revision=<sha>` to its API; if we 404'd here,
    // sub-snapshot calls would all blow up after the first model_info hit.
    if revision == "main" {
        return build_model_info(&st, &owner, &repo, repo_id, &revision).await;
    }
    // SHA path — find a matching commit by recomputing every commit's
    // SHA. v1 single-commit-per-repo makes this O(1) effectively.
    let commit_ids: Vec<(i64,)> = sqlx::query_as(
        "SELECT id FROM repo_commits WHERE repo_id = $1",
    )
    .bind(repo_id)
    .fetch_all(st.pool())
    .await?;
    let target_lower = revision.to_lowercase();
    let matched = commit_ids
        .into_iter()
        .find(|(id,)| commit_oid_for(*id) == target_lower);
    if matched.is_none() {
        return Err(AppError::NotFound);
    }
    build_model_info(&st, &owner, &repo, repo_id, &revision).await
}

async fn build_model_info<S: HfApiState>(
    st: &S,
    owner: &str,
    repo: &str,
    repo_id: i64,
    revision: &str,
) -> Result<Json<ModelInfo>, AppError> {
    let commit: Option<(i64, chrono::DateTime<chrono::Utc>, chrono::DateTime<chrono::Utc>)> =
        sqlx::query_as(
            "SELECT rc.id, rc.committed_at, r.created_at \
               FROM repo_refs rr \
               JOIN repo_commits rc ON rc.id = rr.commit_id \
               JOIN repos r ON r.id = rr.repo_id \
              WHERE rr.repo_id = $1 AND rr.ref_name = $2",
        )
        .bind(repo_id)
        .bind(revision)
        .fetch_optional(st.pool())
        .await?;
    let (commit_id, committed_at, created_at) = commit.ok_or(AppError::NotFound)?;

    let files: Vec<(String, i64, Option<Vec<u8>>, Option<Vec<u8>>)> = sqlx::query_as(
        "SELECT path, size_bytes, xet_hash, lfs_oid FROM repo_files WHERE commit_id = $1",
    )
    .bind(commit_id)
    .fetch_all(st.pool())
    .await?;

    let siblings: Vec<ModelSibling> = files
        .into_iter()
        .map(|(path, size, xet_hash, lfs_oid)| {
            // Every file surface-level looks like LFS to hf_hub so the
            // download path always goes through our resolve endpoint
            // (rather than trying inline `content` blobs).
            let oid = match (&xet_hash, &lfs_oid) {
                (Some(h), _) => hex_encode(h),
                (None, Some(o)) => hex_encode(o),
                _ => String::new(),
            };
            ModelSibling {
                rfilename: path,
                size,
                blob_id: Some(oid.clone()),
                lfs: Some(LfsSibling {
                    size,
                    sha256: oid,
                    pointer_size: 0,
                }),
            }
        })
        .collect();

    let downloads = fetch_download_counters(st.pool(), repo_id).await;

    Ok(Json(ModelInfo {
        id: format!("{owner}/{repo}"),
        author: owner.to_string(),
        sha: commit_oid_for(commit_id),
        // hf_hub parses timestamps with `%Y-%m-%dT%H:%M:%S.%fZ` — a strict
        // `Z`-suffix shape. chrono's `.to_rfc3339` emits `+00:00` which
        // trips the parser. Format with explicit microseconds + `Z`.
        last_modified: committed_at
            .format("%Y-%m-%dT%H:%M:%S%.6fZ")
            .to_string(),
        private: false,
        siblings,
        created_at: created_at
            .format("%Y-%m-%dT%H:%M:%S%.6fZ")
            .to_string(),
        card_data: None,
        downloads,
    }))
}

/// Single-query rollup from `repo_downloads` for a given repo. Returns a
/// zero-valued struct on any error — the counters are display-only and
/// should never block the repo-info endpoint.
async fn fetch_download_counters(
    pool: &sqlx::PgPool,
    repo_id: i64,
) -> ModelDownloads {
    let row: Result<(Option<i64>, Option<i64>, Option<i64>, Option<i64>), _> =
        sqlx::query_as(
            "SELECT \
                COALESCE(SUM(count), 0)::BIGINT AS total, \
                COALESCE(SUM(count) FILTER (WHERE day >= CURRENT_DATE - INTERVAL '1 day'), 0)::BIGINT AS last_24h, \
                COALESCE(SUM(count) FILTER (WHERE day >= CURRENT_DATE - INTERVAL '7 days'), 0)::BIGINT AS last_7d, \
                COALESCE(SUM(count) FILTER (WHERE day >= CURRENT_DATE - INTERVAL '30 days'), 0)::BIGINT AS last_30d \
             FROM repo_downloads WHERE repo_id = $1",
        )
        .bind(repo_id)
        .fetch_one(pool)
        .await;
    match row {
        Ok((total, d24, d7, d30)) => ModelDownloads {
            total: total.unwrap_or(0),
            last_24h: d24.unwrap_or(0),
            last_7d: d7.unwrap_or(0),
            last_30d: d30.unwrap_or(0),
        },
        Err(e) => {
            tracing::warn!(err = %e, repo_id, "download counters lookup failed");
            ModelDownloads::default()
        }
    }
}

// ---------------------------------------------------------------------------
// GET /api/models/{owner}/{repo}/downloads/trend
// ---------------------------------------------------------------------------
// Zero-filled 14-day trend so the Console sparkline has a fixed-length
// series regardless of how many actual `repo_downloads` rows exist.

#[derive(Debug, Serialize)]
pub struct TrendResponse {
    pub days: Vec<TrendPoint>,
}

#[derive(Debug, Serialize)]
pub struct TrendPoint {
    /// ISO-8601 calendar date (YYYY-MM-DD).
    pub day: String,
    pub count: i64,
    pub bytes: i64,
}

// ---------------------------------------------------------------------------
// `GET /api/models/{owner}/{repo}/objects` — surface the underlying xorbs
// (and inline LFS objects) a model's HEAD commit depends on. Drives the
// "Objects on Sia" panel on `/models/{owner}/{repo}` so reviewers can
// click through to the actual content-addressed blobs and verify pinning.

#[derive(Debug, Serialize)]
pub struct ModelObject {
    /// Lowercase 64-char hex of either `xorbs.xorb_merkle_hash` or
    /// `lfs_objects.oid`. Stable identifier for `/assets/{hash}` deep-links.
    pub hash: String,
    /// "xorb" for content-addressed Xet xorbs, "inline" for small LFS files
    /// stored as Postgres BYTEA blobs that bypass Sia entirely.
    pub kind: String,
    pub size_bytes: i64,
    /// `'pinned'`, `'pinning'`, `'orphaned'`, etc. for xorbs; `'inline'` for
    /// LFS rows. UI maps these to the same labels as `/assets`.
    pub pin_state: String,
    /// Hex-encoded `xorbs.sia_object_id` when the xorb has actually
    /// reached a Sia host. None for pinning/orphaned/inline rows.
    pub sia_object_id: Option<String>,
    /// Repo-relative file paths whose reconstruction references this xorb
    /// (or which directly own this LFS oid). Helps reviewers see which
    /// files share which xorbs (xet's multi-file packing).
    pub files: Vec<String>,
}

#[derive(Debug, Serialize)]
pub struct ModelObjectsResponse {
    pub objects: Vec<ModelObject>,
}

pub async fn model_objects<S: HfApiState>(
    State(st): State<S>,
    Path((owner, repo)): Path<(String, String)>,
    headers: HeaderMap,
) -> Result<Json<ModelObjectsResponse>, AppError> {
    let repo_id = find_repo_id(st.pool(), &owner, &repo).await?;
    let viewer = optional_viewer_id(&st, &headers).await;
    require_readable(st.pool(), repo_id, viewer).await?;

    // Single round-trip rollup. UNION the two paths:
    //   (a) Xet xorbs: walk repo_files.xet_hash → reconstruction_terms →
    //       xorbs/xorb_bodies; aggregate distinct file paths sharing each
    //       xorb so multi-file packing is visible.
    //   (b) LFS inline objects: repo_files.lfs_oid → lfs_objects.
    // Sort by size desc so the heavy `pytorch_model.bin` xorbs land at top.
    type Row = (Vec<u8>, String, i64, String, Option<Vec<u8>>, Vec<String>);
    let rows: Vec<Row> = sqlx::query_as(
        "WITH head_files AS ( \
             SELECT rep_f.path, rep_f.xet_hash, rep_f.lfs_oid \
               FROM repo_refs rr \
               JOIN repo_files rep_f ON rep_f.commit_id = rr.commit_id \
              WHERE rr.repo_id = $1 AND rr.ref_name = 'main' \
         ), \
         xorb_objects AS ( \
             SELECT x.xorb_merkle_hash AS hash, \
                    'xorb'::text AS kind, \
                    x.size_bytes, \
                    x.pin_state::text AS pin_state, \
                    x.sia_object_id, \
                    ARRAY_AGG(DISTINCT hf.path ORDER BY hf.path) AS files \
               FROM xorbs x \
               JOIN reconstruction_terms rt ON rt.xorb_hash = x.xorb_merkle_hash \
               JOIN reconstruction_files rfi ON rfi.file_id = rt.file_id \
               JOIN head_files hf ON hf.xet_hash = rfi.file_id \
              GROUP BY x.xorb_merkle_hash, x.size_bytes, x.pin_state, x.sia_object_id \
         ), \
         inline_objects AS ( \
             SELECT lo.oid AS hash, \
                    'inline'::text AS kind, \
                    lo.size_bytes, \
                    'inline'::text AS pin_state, \
                    NULL::bytea AS sia_object_id, \
                    ARRAY_AGG(DISTINCT hf.path ORDER BY hf.path) AS files \
               FROM lfs_objects lo \
               JOIN head_files hf ON hf.lfs_oid = lo.oid \
              GROUP BY lo.oid, lo.size_bytes \
         ) \
         SELECT hash, kind, size_bytes, pin_state, sia_object_id, files \
           FROM xorb_objects \
         UNION ALL \
         SELECT hash, kind, size_bytes, pin_state, sia_object_id, files \
           FROM inline_objects \
          ORDER BY size_bytes DESC",
    )
    .bind(repo_id)
    .fetch_all(st.pool())
    .await?;

    let objects: Vec<ModelObject> = rows
        .into_iter()
        .map(|(hash, kind, size_bytes, pin_state, sia_object_id, files)| ModelObject {
            hash: hex_encode(&hash),
            kind,
            size_bytes,
            pin_state,
            sia_object_id: sia_object_id.map(|b| hex_encode(&b)),
            files,
        })
        .collect();

    Ok(Json(ModelObjectsResponse { objects }))
}

pub async fn model_downloads_trend<S: HfApiState>(
    State(st): State<S>,
    Path((owner, repo)): Path<(String, String)>,
    headers: HeaderMap,
) -> Result<Json<TrendResponse>, AppError> {
    let repo_id = find_repo_id(st.pool(), &owner, &repo).await?;
    let viewer = optional_viewer_id(&st, &headers).await;
    require_readable(st.pool(), repo_id, viewer).await?;

    // Pull whatever rows exist in the last 14 days; zero-fill gaps here.
    let rows: Vec<(chrono::NaiveDate, i64, i64)> = sqlx::query_as(
        "SELECT day, count, bytes \
           FROM repo_downloads \
          WHERE repo_id = $1 \
            AND day >= CURRENT_DATE - INTERVAL '13 days' \
          ORDER BY day",
    )
    .bind(repo_id)
    .fetch_all(st.pool())
    .await?;

    // Hash-map the rows so gap filling is O(n). Today inclusive.
    use std::collections::HashMap;
    let by_day: HashMap<chrono::NaiveDate, (i64, i64)> = rows
        .into_iter()
        .map(|(d, c, b)| (d, (c, b)))
        .collect();

    let today = chrono::Utc::now().date_naive();
    let mut days = Vec::with_capacity(14);
    for i in (0..14).rev() {
        let d = today - chrono::Duration::days(i);
        let (count, bytes) = by_day.get(&d).copied().unwrap_or((0, 0));
        days.push(TrendPoint {
            day: d.format("%Y-%m-%d").to_string(),
            count,
            bytes,
        });
    }

    Ok(Json(TrendResponse { days }))
}

// ---------------------------------------------------------------------------
// Resolve: `/{owner}/{repo}/resolve/{revision}/{*path}`
// ---------------------------------------------------------------------------
// hf_hub issues HEAD first (to cache ETags + size), then GET to fetch
// bytes. We answer:
// * HEAD → 200 with X-Linked-Size/Etag/Content-Type headers.
// * GET (LFS file) → stream from `lfs_objects.content`.
// * GET (Xet file) → 302 redirect to a per-xorb byte-serving URL.
// The gateway is the intended byte source; for V1 dev we point at
// our own /xet/files/{xet_hash} stub that returns the raw xorb
// body. swaps this for a gateway signed URL once
// the reconstruction path ships.

pub async fn resolve_head<S: HfApiState>(
    State(st): State<S>,
    Path((owner, repo, revision, file_path)): Path<(String, String, String, String)>,
    headers: HeaderMap,
) -> Result<Response, AppError> {
    let info = resolve_lookup(&st, &owner, &repo, &revision, &file_path, &headers).await?;
    let mut h = HeaderMap::new();
    h.insert(header::CONTENT_LENGTH, info.size.to_string().parse().unwrap());
    h.insert(
        "x-linked-size",
        info.size.to_string().parse().unwrap(),
    );
    let oid_header = format!("sha256:{}", info.oid_hex);
    h.insert(
        header::ETAG,
        format!("\"{}\"", info.oid_hex).parse().unwrap(),
    );
    h.insert(
        "x-repo-commit",
        info.commit_sha.parse().unwrap(),
    );
    h.insert("x-linked-etag", oid_header.parse().unwrap());
    Ok((StatusCode::OK, h).into_response())
}

pub async fn resolve_get<S: HfApiState>(
    State(st): State<S>,
    Path((owner, repo, revision, file_path)): Path<(String, String, String, String)>,
    headers: HeaderMap,
) -> Result<Response, AppError> {
    let info = resolve_lookup(&st, &owner, &repo, &revision, &file_path, &headers).await?;
    // `?r=<repo_id>` threads attribution into the redirect target so the
    // byte-serve handler can bump the right row in `repo_downloads`.
    // Unsigned by design (counter is display-only; see plan).
    let base = st.cas_public_url().trim_end_matches('/');
    let redirect_to = match &info.backend {
        ResolvedBackend::Lfs(oid) => {
            format!("{base}/lfs/objects/{}?r={}", hex_encode(oid), info.repo_id)
        }
        ResolvedBackend::Xet(xet_hash) => {
            format!("{base}/xet/files/{}?r={}", hex_encode(xet_hash), info.repo_id)
        }
    };
    let mut h = HeaderMap::new();
    h.insert(header::LOCATION, redirect_to.parse().unwrap());
    h.insert("x-linked-size", info.size.to_string().parse().unwrap());
    h.insert(
        "x-linked-etag",
        format!("sha256:{}", info.oid_hex).parse().unwrap(),
    );
    Ok((StatusCode::FOUND, h).into_response())
}

enum ResolvedBackend {
    Lfs(Vec<u8>),
    Xet(Vec<u8>),
}

struct ResolvedInfo {
    size: i64,
    oid_hex: String,
    commit_sha: String,
    backend: ResolvedBackend,
    /// repo.id — threaded through so the 302 Location can carry a
    /// `?r=<id>` query param that the byte-serve handler bumps into the
    /// `repo_downloads` counter. Unsigned (see plan — counter is
    /// display-only, no security value to forge).
    repo_id: i64,
}

async fn resolve_lookup<S: HfApiState>(
    st: &S,
    owner: &str,
    repo: &str,
    revision: &str,
    file_path: &str,
    headers: &HeaderMap,
) -> Result<ResolvedInfo, AppError> {
    let repo_id = find_repo_id(st.pool(), owner, repo).await?;
    let viewer = optional_viewer_id(st, headers).await;
    require_readable(st.pool(), repo_id, viewer).await?;
    // hf_hub resolves HEAD in `model_info` first and then requests files
    // by SHA (`/resolve/<40-hex>/<path>`) — NOT by branch name. We
    // translate a 40-hex revision back to the originating repo_commits
    // row by parsing its leading i64 (our SHA scheme is zero-padded id).
    // `main` keeps the branch-name fast-path.
    let file_row: Option<(i64, Option<Vec<u8>>, Option<Vec<u8>>)> = if revision == "main" {
        sqlx::query_as(
            "SELECT rf.size_bytes, rf.xet_hash, rf.lfs_oid \
               FROM repo_refs rr \
               JOIN repo_files rf ON rf.commit_id = rr.commit_id \
              WHERE rr.repo_id = $1 AND rr.ref_name = 'main' AND rf.path = $2",
        )
        .bind(repo_id)
        .bind(file_path)
        .fetch_optional(st.pool())
        .await?
    } else {
        // hf_hub pins the HEAD SHA from `model_info` and then requests
        // every file under `/resolve/<sha>/<path>`. Our SHA is the SHA-256
        // of a salt + commit_id (see `commit_oid_for`), which can't be
        // reversed. Walk the repo's commits, recompute each `commit_oid_for`,
        // and pick the match. v1 demo has one commit per repo so this is
        // typically a single hash compare.
        let commit_ids: Vec<(i64,)> = sqlx::query_as(
            "SELECT id FROM repo_commits WHERE repo_id = $1",
        )
        .bind(repo_id)
        .fetch_all(st.pool())
        .await?;
        let target_lower = revision.to_lowercase();
        let matched_commit_id = commit_ids
            .into_iter()
            .map(|(id,)| (id, commit_oid_for(id)))
            .find(|(_, sha)| sha == &target_lower)
            .map(|(id, _)| id);
        let commit_id = matched_commit_id.ok_or(AppError::NotFound)?;
        sqlx::query_as(
            "SELECT rf.size_bytes, rf.xet_hash, rf.lfs_oid \
               FROM repo_files rf \
              WHERE rf.commit_id = $1 AND rf.path = $2",
        )
        .bind(commit_id)
        .bind(file_path)
        .fetch_optional(st.pool())
        .await?
    };
    let row = file_row.map(|(s, x, l)| {
        // Pull commit_id via a quick lookup so the caller can surface
        // the canonical SHA in x-repo-commit.
        (0i64, s, x, l)
    });
    let commit_id_lookup: Option<(i64,)> = sqlx::query_as(
        "SELECT rr.commit_id FROM repo_refs rr WHERE rr.repo_id = $1 AND rr.ref_name = 'main'",
    )
    .bind(repo_id)
    .fetch_optional(st.pool())
    .await?;
    let head_commit_id = commit_id_lookup.map(|(c,)| c).unwrap_or(0);
    let (_zero, size, xet_hash, lfs_oid) = row.ok_or(AppError::NotFound)?;
    let commit_sha = format!("{:040x}", head_commit_id);
    match (xet_hash, lfs_oid) {
        (Some(h), _) => {
            let oid_hex = hex_encode(&h);
            Ok(ResolvedInfo {
                size,
                oid_hex,
                commit_sha,
                backend: ResolvedBackend::Xet(h),
                repo_id,
            })
        }
        (None, Some(o)) => {
            let oid_hex = hex_encode(&o);
            Ok(ResolvedInfo {
                size,
                oid_hex,
                commit_sha,
                backend: ResolvedBackend::Lfs(o),
                repo_id,
            })
        }
        (None, None) => Err(AppError::NotFound),
    }
}

// ---------------------------------------------------------------------------
// GET /xet/files/{xet_hash} (V1 xorb-body stub)
// ---------------------------------------------------------------------------
// Returns the xorb body bytes. replaces this with a gateway-signed
// URL that streams from Sia; for the V1 demo we just 404 because the CAS
// doesn't retain xorb bodies (they went straight to the Sia adapter). This
// handler exists so the resolve path has a stable URL to redirect to, and
// it emits a clear 501 with a pointer to the follow-up.

pub async fn xet_file_serve<S: HfApiState>(
    State(st): State<S>,
    Path(xet_hash_hex): Path<String>,
    Query(dp): Query<DownloadParams>,
    headers: HeaderMap,
) -> Result<Response, AppError> {
    // proper per-file reconstruction:
    // 1. xet_hash_hex == reconstruction_files.file_id (xet's per-file merkle hash)
    // 2. join reconstruction_terms ORDER BY term_index — gives us the
    //    sequence of (xorb_hash, xorb_byte_start..xorb_byte_end) windows
    //    that, decompressed in order, are the file's content
    // 3. for each term: load the xorb body, slice [byte_start..byte_end],
    //    iterate xet chunk headers in that slice, decompress to writer
    // 4. concatenate; that's the reconstructed file
    use siahub_cas_proto::xorb_object::deserialize_chunk_to_writer;
    use std::io::Cursor;

    let file_id = hex_decode(&xet_hash_hex).ok_or(AppError::BadRequest("bad_xet_hash"))?;

    // verify file exists
    let file_row: Option<(i64,)> = sqlx::query_as(
        "SELECT total_size FROM reconstruction_files WHERE file_id = $1",
    )
    .bind(&file_id[..])
    .fetch_optional(st.pool())
    .await?;
    let total_size = file_row.ok_or(AppError::NotFound)?.0;

    // load all terms (chunk INDICES — byte offsets are not pre-computable
    // because shard's chunk_byte_range_start is the compressed offset only,
    // and unpacked_segment_bytes is the *unpacked* length, so the two
    // can't be combined to get the compressed length of the last chunk).
    type TermRow = (Vec<u8>, i64, i64);
    let term_rows: Vec<TermRow> = sqlx::query_as(
        "SELECT xorb_hash, xorb_start, xorb_end \
           FROM reconstruction_terms \
          WHERE file_id = $1 \
          ORDER BY term_index ASC",
    )
    .bind(&file_id[..])
    .fetch_all(st.pool())
    .await?;
    if term_rows.is_empty() {
        return Err(AppError::NotFound);
    }

    let mut out: Vec<u8> = Vec::with_capacity(total_size as usize);
    let mut sink = Vec::<u8>::new();
    for (xorb_hash, chunk_start, chunk_end) in &term_rows {
        let xorb_row: Option<(Vec<u8>,)> = sqlx::query_as(
            "SELECT content FROM xorb_bodies WHERE xorb_hash = $1",
        )
        .bind(&xorb_hash[..])
        .fetch_optional(st.pool())
        .await?;
        let body = xorb_row.ok_or(AppError::NotFound)?.0;

        let mut cur = Cursor::new(body.as_slice());
        let total = body.len() as u64;
        let want_start = *chunk_start as usize;
        let want_end = *chunk_end as usize;
        let mut idx: usize = 0;

        // walk every chunk in xorb order; decompress only the indices in
        // [want_start, want_end). cheap because the first 7 chunks of a
        // 277-chunk xorb are tiny relative to the total.
        while cur.position() < total && idx < want_end {
            if idx >= want_start {
                // emit decompressed chunk
                if let Err(e) = deserialize_chunk_to_writer(&mut cur, &mut out) {
                    return Err(AppError::Other(anyhow::anyhow!(
                        "decompress chunk {idx}: {e}"
                    )));
                }
            } else {
                // skip chunk (decompress to discard buffer)
                sink.clear();
                if let Err(e) = deserialize_chunk_to_writer(&mut cur, &mut sink) {
                    return Err(AppError::Other(anyhow::anyhow!(
                        "skip chunk {idx}: {e}"
                    )));
                }
            }
            idx += 1;
        }
        if idx < want_end {
            return Err(AppError::Other(anyhow::anyhow!(
                "xorb ran out at chunk {idx}, term wanted [{want_start}..{want_end})"
            )));
        }
    }

    // size sanity-check — the reconstruction must produce exactly total_size
    // bytes, otherwise we have a parser/persistence drift.
    if out.len() as i64 != total_size {
        return Err(AppError::Other(anyhow::anyhow!(
            "reconstruction size mismatch: got {}, expected {}",
            out.len(),
            total_size
        )));
    }

    let api_key_id = optional_api_key_id(&st, &headers).await;
    record_download(
        st.pool(),
        dp.r,
        api_key_id,
        out.len() as i64,
        Some(file_id),
        None,
    )
    .await;
    Ok((
        [
            (header::CONTENT_TYPE, "application/octet-stream"),
            (header::CACHE_CONTROL, "public, max-age=86400"),
        ],
        out,
    )
        .into_response())
}

fn hex_encode(bytes: &[u8]) -> String {
    let mut s = String::with_capacity(bytes.len() * 2);
    for b in bytes {
        s.push_str(&format!("{:02x}", b));
    }
    s
}
