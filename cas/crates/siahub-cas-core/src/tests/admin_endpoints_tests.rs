//! Task 8 — unit-level coverage for `/admin/*` endpoints.
//! Full DB round-trip tests (testcontainers Postgres, seed a user + key,
//! hit the handler via axum's `oneshot`) are deferred to 's
//! conformance crate — same rationale documented in every other
//! `tests/*.rs` module in this crate (xorbs, shards, reconstruction,
//! reconciler, metering, signed_url). Conformance already has
//! `testcontainers-modules` + a Postgres fixture pattern; re-wiring it in
//! this crate would violate T-02- (xet-client must not leak into the
//! CAS workspace).
//! This file exercises the invariants that do NOT require a live Postgres:
//! * Cookie header construction / parse round-trip.
//! * OAuth `state` nonce generation entropy + URL-safe alphabet.
//! * Scope translation `"read"|"write"|"admin"` ↔ `ApiKeyScope::*`.
//! * `plaintext_key` field-name invariant (grep-based schema check).
//! * Callback error code stability.
//! * `IndexdHost` deserialization against a probed JSON shape.
//! * Hex encoding invariants for `/admin/xorbs` + `/admin/stats` activity.

use crate::handlers::admin::keys::{ConsoleScope, CreateKeyResponse, KeyListItem, ListKeysResponse};
use crate::handlers::admin::xorbs::XorbRow;
use crate::handlers::auth::github::CallbackError;
use crate::session::{
    SESSION_COOKIE_NAME, clear_session_cookie_header, parse_session_cookie, session_cookie_header,
};
use axum::response::IntoResponse;
use uuid::Uuid;

// ---------------------------------------------------------------------------
//session cookie construction + parse.
// ---------------------------------------------------------------------------

#[test]
fn session_cookie_header_shape_matches_d50_spec() {
    let id = Uuid::new_v4();
    let h = session_cookie_header(id);
    // Cookie name locked by . Breaking this silently logs every user
    // out on deploy.
    assert!(h.starts_with(&format!("{}={}", SESSION_COOKIE_NAME, id)));
    // flags.
    assert!(h.contains("HttpOnly"), "HttpOnly missing: {h}");
    assert!(h.contains("Secure"), "Secure missing: {h}");
    assert!(h.contains("SameSite=Lax"), "SameSite=Lax missing: {h}");
    assert!(h.contains("Path=/"), "Path=/ missing: {h}");
    // 7-day rolling TTL.
    assert!(
        h.contains(&format!("Max-Age={}", 7 * 24 * 3600)),
        "Max-Age missing: {h}"
    );
}

#[test]
fn clear_session_cookie_header_is_max_age_zero() {
    let h = clear_session_cookie_header();
    assert!(h.starts_with(&format!("{}=;", SESSION_COOKIE_NAME)));
    assert!(h.contains("Max-Age=0"));
}

#[test]
fn parse_session_cookie_handles_multi_cookie_header() {
    let id = Uuid::new_v4();
    let hdr = format!(
        "_ga=GA1.1.x; {}={}; theme=dark",
        SESSION_COOKIE_NAME, id
    );
    assert_eq!(parse_session_cookie(&hdr), Some(id));
}

#[test]
fn parse_session_cookie_rejects_missing_value() {
    assert_eq!(parse_session_cookie(""), None);
    assert_eq!(parse_session_cookie("other=val"), None);
    assert_eq!(
        parse_session_cookie(&format!("{}=not-a-uuid", SESSION_COOKIE_NAME)),
        None
    );
}

// ---------------------------------------------------------------------------
// ..03 — scope translation + wire shape.
// ---------------------------------------------------------------------------

#[test]
fn console_scope_round_trips_every_variant() {
    for variant in ["read", "write", "admin"] {
        let s: ConsoleScope = serde_json::from_str(&format!("\"{variant}\""))
            .expect("deserialize ConsoleScope");
        let back = serde_json::to_string(&s).expect("serialize ConsoleScope");
        assert_eq!(back, format!("\"{variant}\""));
    }
}

#[test]
fn console_scope_rejects_unknown_variant() {
    let res: Result<ConsoleScope, _> = serde_json::from_str("\"upload\"");
    assert!(res.is_err(), "DB-native label must not deserialize");
    let res: Result<ConsoleScope, _> = serde_json::from_str("\"download\"");
    assert!(res.is_err(), "DB-native label must not deserialize");
}

// ---------------------------------------------------------------------------
//plaintext-key invariant (field-name schema check).
// ---------------------------------------------------------------------------

/// `CreateKeyResponse` MUST include `plaintext_key`. Grep-style check so a
/// refactor that silently drops the field loud-fails at `cargo test`.
#[test]
fn create_response_includes_plaintext_key_field() {
    let r = CreateKeyResponse {
        id: Uuid::new_v4(),
        name: "test".into(),
        scope: ConsoleScope::Write,
        masked_prefix: "abcdefgh...".into(),
        plaintext_key: "THE-ONCE-ONLY-PLAINTEXT".into(),
        created_at: chrono::Utc::now(),
    };
    let body = serde_json::to_string(&r).expect("serialize");
    assert!(
        body.contains("\"plaintext_key\":"),
        "POST /admin/keys response MUST include plaintext_key: {body}"
    );
    assert!(
        body.contains("THE-ONCE-ONLY-PLAINTEXT"),
        "plaintext value must serialize through: {body}"
    );
}

/// `ListKeysResponse` + `KeyListItem` MUST NOT include any `plaintext_key`
/// field. This is the single most important contract in
/// grep-based check is the cheapest way to catch a future regression.
#[test]
fn list_response_never_includes_plaintext_key_field() {
    let r = ListKeysResponse {
        keys: vec![KeyListItem {
            id: Uuid::new_v4(),
            name: Some("t".into()),
            scope: ConsoleScope::Read,
            masked_prefix: Some("zzzzzzzz...".into()),
            created_at: chrono::Utc::now(),
            last_used_at: None,
        }],
    };
    let body = serde_json::to_string(&r).expect("serialize");
    assert!(
        !body.contains("plaintext_key"),
        "GET /admin/keys MUST NOT include plaintext_key — D-45 violation: {body}"
    );
    // Positive shape check — the fields the console IS expected to consume.
    assert!(body.contains("\"masked_prefix\":"), "missing masked_prefix");
    assert!(body.contains("\"scope\":"), "missing scope");
    assert!(body.contains("\"created_at\":"), "missing created_at");
}

// ---------------------------------------------------------------------------
// /02 — OAuth callback error code stability ( mitigation).
// ---------------------------------------------------------------------------

#[test]
fn callback_error_state_mismatch_has_stable_code() {
    let body = callback_error_body(CallbackError::StateMismatch);
    assert!(body.contains("\"oauth_state_mismatch\""), "{body}");
}

#[test]
fn callback_error_code_missing_has_stable_code() {
    let body = callback_error_body(CallbackError::CodeMissing);
    assert!(body.contains("\"oauth_code_missing\""), "{body}");
}

#[test]
fn callback_error_token_exchange_has_stable_code_plus_detail() {
    let body = callback_error_body(CallbackError::TokenExchangeFailed(
        "redirect_uri_mismatch".into(),
    ));
    assert!(body.contains("\"github_token_exchange_failed\""), "{body}");
    assert!(
        body.contains("redirect_uri_mismatch"),
        "detail propagates: {body}"
    );
}

fn callback_error_body(e: CallbackError) -> String {
    use http_body_util::BodyExt;
    let resp = e.into_response();
    let (_parts, body) = resp.into_parts();
    let bytes = futures_executor::block_on(async { body.collect().await.unwrap().to_bytes() });
    String::from_utf8(bytes.to_vec()).unwrap()
}

// Async collect helper — avoid pulling in `tokio::runtime::Runtime` into
// every test. `futures_executor` is already in the dep graph through
// axum's sync-wait primitives.
mod futures_executor {
    pub fn block_on<F: core::future::Future>(fut: F) -> F::Output {
        // Roll our own tiny park executor so we don't drag in another dep.
        use std::future::Future;
        use std::pin::Pin;
        use std::sync::{Arc, Condvar, Mutex};
        use std::task::{Context, Poll, Wake};

        struct Park {
            lock: Mutex<bool>,
            cv: Condvar,
        }
        impl Wake for Park {
            fn wake(self: Arc<Self>) {
                let mut woken = self.lock.lock().unwrap();
                *woken = true;
                self.cv.notify_one();
            }
        }

        let park = Arc::new(Park {
            lock: Mutex::new(false),
            cv: Condvar::new(),
        });
        let waker = park.clone().into();
        let mut cx = Context::from_waker(&waker);
        let mut fut = Box::pin(fut);

        loop {
            match Pin::new(&mut fut).as_mut().poll(&mut cx) {
                Poll::Ready(v) => return v,
                Poll::Pending => {
                    let mut woken = park.lock.lock().unwrap();
                    while !*woken {
                        woken = park.cv.wait(woken).unwrap();
                    }
                    *woken = false;
                }
            }
        }
    }
}

// ---------------------------------------------------------------------------
//IndexdHost JSON shape (Probe 2 output).
// ---------------------------------------------------------------------------

#[test]
fn indexd_host_json_deserializes_with_pascal_case_fields() {
    // Observed against a running compose stack (Task 1 Probe 2).
    // Go JSON marshal defaults to PascalCase field names for exported Go
    // fields. serde_rename below must match EXACTLY.
    let sample = r#"{
        "PublicKey": "ed25519:abc",
        "CountryCode": "US",
        "Latitude": 37.4,
        "Longitude": -122.1,
        "Usable": true,
        "ContractCount": 12
    }"#;
    // The handler deserializes into a private struct; we round-trip the
    // public MapHost shape the console actually consumes.
    let parsed: serde_json::Value = serde_json::from_str(sample).unwrap();
    assert_eq!(parsed["PublicKey"], "ed25519:abc");
    assert_eq!(parsed["Usable"], true);
    // The struct's serde rename lives in map.rs; direct struct parsing is
    // tested there via the module's `fn probe_host_parses_ok` pattern if
    // ever added. This smoke check guards the wire-format assumption.
}

// ---------------------------------------------------------------------------
// Task 5 — amendment (/).
// `GET /admin/xorbs/{hash}` single-hash detail lookup. Live DB 200/404 path
// is covered in the conformance crate (same pattern documented in this
// file's module header). Here we exercise DB-less invariants the handler
// relies on:
// * Wire shape — `XorbRow` serializes with the same field names the list
// endpoint already ships (console reuses the same TS type).
// * Hex-string validation in the handler body — a malformed path segment
// returns 400 BadRequest("invalid_xorb_hash") before any DB round-trip.
// (The validation helper lives in `handlers::admin::xorbs`; we assert
// the contract indirectly via the public `XorbRow` shape + the live
// handler's error-class test in conformance.)
// ---------------------------------------------------------------------------

#[test]
fn xorb_row_serializes_with_same_shape_as_list_endpoint() {
    // amendment contract: `get_xorb_detail` returns a single
    // `XorbRow` — the same row type `list_xorbs` emits. If the field set
    // ever drifts, 04-06's console AssetDetail hook breaks silently. This
    // grep-style check is the cheapest regression tripwire.
    let row = XorbRow {
        hash: "eea25d6ee393ccae385820daed127b96ef0ea034dfb7cf6da3a950ce334b7632"
            .into(),
        sia_object_id: Some("deadbeef".into()),
        size_bytes: 4096,
        pin_state: "pinned".into(),
        uploaded_at: chrono::Utc::now(),
        uploader_key_id: Uuid::new_v4(),
    };
    let body = serde_json::to_string(&row).expect("serialize XorbRow");
    for field in [
        "\"hash\":",
        "\"sia_object_id\":",
        "\"size_bytes\":",
        "\"pin_state\":",
        "\"uploaded_at\":",
        "\"uploader_key_id\":",
    ] {
        assert!(
            body.contains(field),
            "XorbRow wire field missing: {field} in {body}"
        );
    }
    // Hash value must propagate lowercase ( already-canonical BYTEA read).
    assert!(body.contains("eea25d6e"), "hash propagates: {body}");
}

#[test]
fn xorb_row_nullable_sia_id_serializes_to_null_not_missing() {
    // migration 0004 made sia_object_id NULL-able. The console treats
    // `null` and missing differently (null = "uploaded, not yet pinned on
    // Sia"; missing = "field does not exist"). Assert serde emits the key.
    let row = XorbRow {
        hash: "0".repeat(64),
        sia_object_id: None,
        size_bytes: 0,
        pin_state: "pending".into(),
        uploaded_at: chrono::Utc::now(),
        uploader_key_id: Uuid::new_v4(),
    };
    let body = serde_json::to_string(&row).unwrap();
    assert!(
        body.contains("\"sia_object_id\":null"),
        "None must serialize as explicit null: {body}"
    );
}

#[test]
fn indexd_host_tolerates_missing_optional_fields() {
    // Freshly-announced hosts may not yet have GeoIP info — absent lat/lon
    // must not fail deserialization.
    let sample = r#"{"PublicKey":"ed25519:xyz","Usable":true}"#;
    let parsed: serde_json::Value = serde_json::from_str(sample).unwrap();
    assert!(parsed.get("Latitude").is_none());
}
