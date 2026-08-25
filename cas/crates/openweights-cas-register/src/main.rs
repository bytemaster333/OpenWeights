//! openweights-cas-register — one-shot CLI that registers the OpenWeights CAS as an
//! indexd app using the Rust `sia_storage = 0.7.0` SDK.
//! Required when bootstrapping a fresh stack where no prior Rust-side registration
//! has happened (Go-side bootstrap from produces an AppKey that is NOT
//! interoperable with Rust — different ephemeral key, different shared secret).
//! Flow:
//! 1. Read OPENWEIGHTS_APP_ID (hex) + OPENWEIGHTS_RECOVERY_PHRASE + OPENWEIGHTS_INDEXER_URL from env.
//! 2. Builder::new(indexer_url, app_meta).
//! 3. request_connection → prints approval URL.
//! 4. Human clicks APPROVE in indexd admin UI.
//! 5. wait_for_approval → returns ApprovedState.
//! 6. register(mnemonic) → returns Sdk; extract AppKey.
//! 7. Print `OPENWEIGHTS_APP_KEY=<base64-export>` to stdout for shell redirection.
//! 8. Optionally append to.env directly if --write-env is passed.

use anyhow::{Context, Result, anyhow};
use base64::Engine;
use base64::engine::general_purpose::STANDARD as B64;
use sia_storage::{AppMetadata, Builder, Hash256};
use std::env;

#[tokio::main]
async fn main() -> Result<()> {
    let indexer_url =
        env::var("OPENWEIGHTS_INDEXER_URL").context("OPENWEIGHTS_INDEXER_URL not set")?;
    let app_id_hex = env::var("OPENWEIGHTS_APP_ID").context("OPENWEIGHTS_APP_ID not set")?;
    let mnemonic = env::var("OPENWEIGHTS_RECOVERY_PHRASE")
        .context("OPENWEIGHTS_RECOVERY_PHRASE not set")?;

    // Parse app_id (64 hex chars).
    if app_id_hex.len() != 64 {
        return Err(anyhow!("OPENWEIGHTS_APP_ID must be 64 hex chars"));
    }
    let mut id_bytes = [0u8; 32];
    for (i, slot) in id_bytes.iter_mut().enumerate() {
        *slot = u8::from_str_radix(&app_id_hex[2 * i..2 * i + 2], 16)
            .context("OPENWEIGHTS_APP_ID must be hex")?;
    }
    let app_id = Hash256::new(id_bytes);

    let meta = AppMetadata {
        id: app_id,
        name: "OpenWeights CAS",
        description: "OpenWeights content-addressed storage service",
        service_url: "https://example.com",
        logo_url: None,
        callback_url: None,
    };

    eprintln!("[1/4] Creating Rust SDK Builder against {indexer_url}");
    let builder = Builder::new(indexer_url, meta)
        .map_err(|e| anyhow!("Builder::new: {e}"))?;

    eprintln!("[2/4] Requesting new app connection...");
    let requesting = builder
        .request_connection()
        .await
        .map_err(|e| anyhow!("request_connection: {e}"))?;

    let url = requesting.response_url();
    eprintln!();
    eprintln!("=========================================================");
    eprintln!(" APPROVE THE CONNECTION IN YOUR BROWSER");
    eprintln!("=========================================================");
    eprintln!(" Open this URL:");
    eprintln!("   {url}");
    eprintln!();
    eprintln!(" The approval widget will ask for an 'App password'.");
    eprintln!(" Create a connect key via the admin API:");
    eprintln!("   curl -s -u \":$INDEXD_ADMIN_PASSWORD\" \\");
    eprintln!("     -X POST -H 'Content-Type: application/json' \\");
    eprintln!("     -d '{{\"description\":\"openweights-cas\",\"quota\":\"default\"}}' \\");
    eprintln!("     $INDEXD_ADMIN_URL/apps/connect/keys");
    eprintln!(" Paste the returned `key` into the App password field.");
    eprintln!("=========================================================");
    eprintln!();
    eprintln!("[3/4] Waiting for approval (polls every 5s)...");

    let approved = requesting
        .wait_for_approval()
        .await
        .map_err(|e| anyhow!("wait_for_approval: {e}"))?;

    eprintln!("[4/4] Registering app with derived key...");
    let sdk = approved
        .register(&mnemonic)
        .await
        .map_err(|e| anyhow!("register: {e}"))?;

    let app_key = sdk.app_key();
    let exported: [u8; 32] = app_key.export();
    let b64 = B64.encode(exported);

    eprintln!();
    eprintln!("SUCCESS. Append to .env (or overwrite OPENWEIGHTS_APP_KEY):");
    println!("OPENWEIGHTS_APP_KEY={b64}");

    Ok(())
}
