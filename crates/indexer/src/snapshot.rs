//! One-off current-state snapshot of every DLMM account via RPC.
//!
//! The live account stream only delivers accounts when they change, so quiet pairs and
//! untouched positions would never appear without this. Rows carry the RPC context slot
//! and write_version 0, so they never overwrite newer state from the live stream.

use anyhow::{Context, Result};
use base64::Engine;
use dlmm_decoder::Decoder;
use serde::Deserialize;
use serde_json::json;

use crate::extract::account_row;
use crate::model::{AccountRow, DecodeFailure};
use crate::rpc::Rpc;
use crate::store::Store;

#[derive(Deserialize)]
struct Ctx {
    slot: u64,
}

#[derive(Deserialize)]
struct WithCtx<T> {
    context: Ctx,
    value: T,
}

#[derive(Deserialize)]
#[serde(rename_all = "camelCase")]
struct Page {
    accounts: Vec<KeyedAccount>,
    pagination_key: Option<String>,
}

#[derive(Deserialize)]
struct KeyedAccount {
    pubkey: String,
    account: RpcAccount,
}

#[derive(Deserialize)]
struct RpcAccount {
    lamports: u64,
    data: (String, String),
}

fn decode_b64(data: &(String, String)) -> Result<Vec<u8>> {
    anyhow::ensure!(data.1 == "base64", "unexpected encoding {}", data.1);
    Ok(base64::engine::general_purpose::STANDARD.decode(&data.0)?)
}

fn to_rows(
    dec: &Decoder,
    slot: u64,
    items: impl IntoIterator<Item = (String, u64, Vec<u8>)>,
) -> (Vec<AccountRow>, Vec<DecodeFailure>) {
    let mut rows = Vec::new();
    let mut failures = Vec::new();
    for (pubkey, lamports, data) in items {
        match account_row(dec, pubkey, slot, 0, lamports, &data) {
            Ok(r) => rows.push(r),
            Err(f) => failures.push(f),
        }
    }
    (rows, failures)
}

/// Snapshot the given account types (all IDL account types when empty).
pub async fn run(store: &Store, rpc: &Rpc, types: &[String], page_size: usize) -> Result<()> {
    let dec = Decoder::bundled();
    let types: Vec<String> = if types.is_empty() {
        dec.account_names().iter().map(|s| s.to_string()).collect()
    } else {
        types.to_vec()
    };

    for ty in &types {
        let disc = dec
            .account_discriminator(ty)
            .with_context(|| format!("unknown account type {ty}"))?;
        let mut pagination_key: Option<String> = None;
        let mut total = 0usize;
        let mut failed = 0usize;
        tracing::info!(account_type = %ty, "snapshot starting");
        loop {
            let mut cfg = json!({
                "encoding": "base64",
                "commitment": "confirmed",
                "withContext": true,
                "limit": page_size,
                "filters": [{ "memcmp": { "offset": 0, "bytes": bs58::encode(disc).into_string() } }],
            });
            if let Some(k) = &pagination_key {
                cfg["paginationKey"] = json!(k);
            }
            let page: WithCtx<Page> = rpc
                .call("getProgramAccountsV2", json!([dec.program_id_str(), cfg]))
                .await?;
            if page.value.accounts.is_empty() {
                break;
            }
            let items = page
                .value
                .accounts
                .into_iter()
                .filter_map(|a| Some((a.pubkey, a.account.lamports, decode_b64(&a.account.data).ok()?)));
            let (rows, failures) = to_rows(dec, page.context.slot, items);
            total += rows.len();
            failed += failures.len();
            store.upsert_accounts(&rows).await?;
            if !failures.is_empty() {
                store.insert_failures(&failures).await?;
            }
            tracing::info!(account_type = %ty, total, failed, slot = page.context.slot, "snapshot page");
            match page.value.pagination_key {
                Some(k) => pagination_key = Some(k),
                None => break,
            }
        }
        tracing::info!(account_type = %ty, total, failed, "snapshot finished");
    }
    if types.iter().any(|t| t == "LbPair") {
        fill_mints(store, rpc).await?;
    }
    Ok(())
}

#[derive(Deserialize)]
struct MintAccount {
    owner: String,
    data: (String, String),
}

/// Fetch decimals for every pair mint we don't know yet. SPL Token and Token-2022 mints
/// both store `decimals` at byte 44, so a 1-byte data slice is enough.
async fn fill_mints(store: &Store, rpc: &Rpc) -> Result<()> {
    let missing: Vec<String> = sqlx::query_scalar(
        "SELECT DISTINCT m FROM (
             SELECT data->>'token_x_mint' AS m FROM accounts WHERE account_type = 'LbPair'
             UNION SELECT data->>'token_y_mint' FROM accounts WHERE account_type = 'LbPair') t
         WHERE m IS NOT NULL AND m NOT IN (SELECT mint FROM mints)",
    )
    .fetch_all(store.pool())
    .await?;
    tracing::info!(missing = missing.len(), "fetching mint decimals");
    let mut rows = Vec::new();
    for chunk in missing.chunks(100) {
        let res: WithCtx<Vec<Option<MintAccount>>> = rpc
            .call(
                "getMultipleAccounts",
                json!([chunk, { "encoding": "base64", "dataSlice": { "offset": 44, "length": 1 } }]),
            )
            .await?;
        for (mint, acc) in chunk.iter().zip(res.value) {
            let Some(acc) = acc else { continue };
            if let Some(&d) = decode_b64(&acc.data).ok().as_deref().and_then(|b| b.first()) {
                rows.push((mint.clone(), d as i16, acc.owner));
            }
        }
    }
    crate::store::upsert_mint_rows(store.pool(), &rows).await?;
    tracing::info!(mints = rows.len(), "mint decimals stored");
    Ok(())
}

/// Re-fetch specific accounts (e.g. after a dead-slot rollback). Closed accounts are skipped.
pub async fn fetch_accounts(rpc: &Rpc, pubkeys: &[String]) -> Result<(Vec<AccountRow>, Vec<DecodeFailure>)> {
    let dec = Decoder::bundled();
    let mut rows = Vec::new();
    let mut failures = Vec::new();
    for chunk in pubkeys.chunks(100) {
        let res: WithCtx<Vec<Option<RpcAccount>>> = rpc
            .call(
                "getMultipleAccounts",
                json!([chunk, { "encoding": "base64", "commitment": "confirmed" }]),
            )
            .await?;
        let items = chunk.iter().zip(res.value).filter_map(|(k, acc)| {
            let acc = acc?;
            Some((k.clone(), acc.lamports, decode_b64(&acc.data).ok()?))
        });
        let (r, f) = to_rows(dec, res.context.slot, items);
        rows.extend(r);
        failures.extend(f);
    }
    Ok((rows, failures))
}
