//! Reconciliation: compares stored account state with the chain and heals drift.
//!
//! Accounts are fetched at RPC slot `C`; comparison waits until the indexer's checkpoint
//! has passed `C`, so an update that is merely in flight is never counted as drift.
//! Rows that are then still older than `C` and differ were missed: they are overwritten
//! with the on-chain state (or marked closed if the account no longer exists).

use std::time::{Duration, Instant};

use anyhow::Result;
use base64::Engine;
use dlmm_decoder::Decoder;
use serde::Deserialize;
use serde_json::{json, Value};

use crate::extract::account_row;
use crate::rpc::Rpc;
use crate::store::Store;

#[derive(Debug, Default)]
pub struct ReconcileReport {
    pub checked: usize,
    pub matched: usize,
    pub skipped_newer: usize,
    pub healed_stale: usize,
    pub healed_closed: usize,
}

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
struct Acc {
    lamports: u64,
    owner: String,
    data: (String, String),
}

/// Pick accounts to verify: the most recently written half plus a random half.
async fn sample(store: &Store, n: usize) -> Result<Vec<String>> {
    Ok(sqlx::query_scalar(
        "(SELECT pubkey FROM accounts WHERE closed_slot IS NULL ORDER BY updated_at DESC LIMIT $1)
         UNION
         (SELECT pubkey FROM accounts WHERE closed_slot IS NULL ORDER BY random() LIMIT $1)",
    )
    .bind((n / 2).max(1) as i64)
    .fetch_all(store.pool())
    .await?)
}

pub async fn run_once(store: &Store, rpc: &Rpc, sample_size: usize, pubkeys: &[String]) -> Result<ReconcileReport> {
    let keys = if pubkeys.is_empty() { sample(store, sample_size).await? } else { pubkeys.to_vec() };
    let dec = Decoder::bundled();
    let mut report = ReconcileReport::default();

    for chunk in keys.chunks(100) {
        let res: WithCtx<Vec<Option<Acc>>> = rpc
            .call("getMultipleAccounts", json!([chunk, { "encoding": "base64", "commitment": "confirmed" }]))
            .await?;
        let chain_slot = res.context.slot;
        wait_for_checkpoint(store, chain_slot, Duration::from_secs(60)).await?;

        for (key, onchain) in chunk.iter().zip(res.value) {
            report.checked += 1;
            let ours: Option<(i64, Value)> = sqlx::query_as(
                "SELECT slot, data FROM accounts WHERE pubkey = $1 AND closed_slot IS NULL",
            )
            .bind(key)
            .fetch_optional(store.pool())
            .await?;
            let Some((our_slot, our_data)) = ours else { continue };
            if our_slot as u64 > chain_slot {
                report.skipped_newer += 1;
                continue;
            }
            match onchain.filter(|a| a.owner == dec.program_id_str() && a.lamports > 0) {
                None => {
                    tracing::warn!(pubkey = %key, our_slot, chain_slot, "account closed on-chain but open in index; healing");
                    sqlx::query("UPDATE accounts SET closed_slot = $2, updated_at = now() WHERE pubkey = $1")
                        .bind(key)
                        .bind(chain_slot as i64)
                        .execute(store.pool())
                        .await?;
                    metrics::counter!("reconcile_mismatches_total", "kind" => "closed").increment(1);
                    report.healed_closed += 1;
                }
                Some(acc) => {
                    let bytes = base64::engine::general_purpose::STANDARD.decode(&acc.data.0)?;
                    let row = match account_row(dec, key.clone(), chain_slot, 0, acc.lamports, &bytes) {
                        Ok(r) => r,
                        Err(f) => {
                            store.insert_failures(&[f]).await?;
                            continue;
                        }
                    };
                    if row.data == our_data {
                        report.matched += 1;
                        continue;
                    }
                    tracing::warn!(pubkey = %key, account_type = row.account_type, our_slot, chain_slot,
                        "stale account state; healing from chain");
                    metrics::counter!("reconcile_mismatches_total", "kind" => "stale").increment(1);
                    store.upsert_accounts(&[row]).await?;
                    report.healed_stale += 1;
                }
            }
        }
    }
    metrics::counter!("reconcile_checked_total").increment(report.checked as u64);
    Ok(report)
}

async fn wait_for_checkpoint(store: &Store, slot: u64, timeout: Duration) -> Result<()> {
    let start = Instant::now();
    loop {
        if store.get_state("checkpoint").await?.unwrap_or(0) >= slot {
            return Ok(());
        }
        if start.elapsed() > timeout {
            anyhow::bail!("indexer checkpoint did not reach slot {slot} within {timeout:?}; is the indexer running?");
        }
        tokio::time::sleep(Duration::from_millis(250)).await;
    }
}
