//! Gap detection and repair.
//!
//! - `audit`: finds holes in the slot chain (a block whose parent isn't stored) and
//!   records them in `gaps`, so silent stream losses are caught, not just refused replays.
//! - `repair`: re-fetches every block in open gaps with `getBlock`, runs DLMM
//!   transactions through the same extractor as the live stream, then refreshes the
//!   DLMM accounts those transactions touched. Idempotent: safe to re-run.

use std::collections::BTreeSet;

use anyhow::Result;
use dlmm_decoder::Decoder;
use futures::{stream, StreamExt};
use serde_json::{json, Value};

use crate::extract::extract_transaction;
use crate::model::{SlotBatch, SlotMeta};
use crate::rpc::Rpc;
use crate::rpc_block::{mentions, tx_from_json};
use crate::snapshot::fetch_accounts;
use crate::store::Store;

/// Record holes in the stored slot chain within (`since_slot`, `until_slot`].
/// `until_slot` should be at or below the checkpoint so in-flight slots aren't flagged.
pub async fn audit(store: &Store, since_slot: u64, until_slot: u64) -> Result<usize> {
    let holes: Vec<(i64, i64)> = sqlx::query_as(
        "WITH chain AS (
            SELECT slot, parent_slot, lag(slot) OVER (ORDER BY slot) AS prev
            FROM slots WHERE slot > $1 AND slot <= $2)
         SELECT prev + 1, slot - 1 FROM chain
         WHERE prev IS NOT NULL AND parent_slot IS NOT NULL AND parent_slot > prev
           AND NOT EXISTS (SELECT 1 FROM gaps g WHERE g.from_slot <= chain.prev + 1 AND g.to_slot >= chain.slot - 1)",
    )
    .bind(since_slot as i64)
    .bind(until_slot as i64)
    .fetch_all(store.pool())
    .await?;
    for (from, to) in &holes {
        tracing::warn!(from_slot = from, to_slot = to, "slot chain hole detected");
        store.record_gap(*from as u64, *to as u64, "slot chain hole").await?;
    }
    metrics::counter!("gaps_recorded_total").increment(holes.len() as u64);
    Ok(holes.len())
}

pub struct RepairReport {
    pub gaps: usize,
    pub blocks: usize,
    pub transactions: usize,
    pub accounts_refreshed: usize,
}

/// Repair open gaps no larger than `max_slots` (larger ones are left for the CLI).
pub async fn repair(
    store: &Store,
    rpc: &Rpc,
    max_slots: u64,
    concurrency: usize,
    index_failed: bool,
) -> Result<RepairReport> {
    let gaps: Vec<(i64, i64, i64)> = sqlx::query_as(
        "SELECT id, from_slot, to_slot FROM gaps
         WHERE repaired_at IS NULL AND to_slot - from_slot < $1 ORDER BY from_slot",
    )
    .bind(max_slots as i64)
    .fetch_all(store.pool())
    .await?;
    let mut report = RepairReport { gaps: 0, blocks: 0, transactions: 0, accounts_refreshed: 0 };
    let dec = Decoder::bundled();

    for (id, from, to) in gaps {
        tracing::info!(gap_id = id, from, to, "repairing gap");
        let mut slots: Vec<u64> = Vec::new();
        let mut start = from as u64;
        while start <= to as u64 {
            let end = (start + 49_999).min(to as u64);
            let chunk: Vec<u64> = rpc
                .call("getBlocks", json!([start, end, { "commitment": "confirmed" }]))
                .await?;
            slots.extend(chunk);
            start = end + 1;
        }

        let mut touched: BTreeSet<String> = BTreeSet::new();
        let mut results = stream::iter(slots.clone())
            .map(|slot| fetch_block(rpc, dec, slot, index_failed))
            .buffer_unordered(concurrency.max(1));
        while let Some(res) = results.next().await {
            let batch = res?;
            report.blocks += 1;
            report.transactions += batch.txs.len();
            for ix in &batch.instructions {
                if let Some(obj) = ix.accounts.as_object() {
                    touched.extend(obj.values().filter_map(|v| v.as_str().map(str::to_owned)));
                }
            }
            store.write(std::slice::from_ref(&batch), None).await?;
        }

        // State written by the missed transactions: re-read the accounts they touched.
        let keys: Vec<String> = touched.into_iter().collect();
        let (rows, failures) = fetch_accounts(rpc, &keys).await?;
        report.accounts_refreshed += rows.len();
        store.upsert_accounts(&rows).await?;
        if !failures.is_empty() {
            store.insert_failures(&failures).await?;
        }

        sqlx::query("UPDATE gaps SET repaired_at = now() WHERE id = $1")
            .bind(id)
            .execute(store.pool())
            .await?;
        metrics::counter!("gaps_repaired_total").increment(1);
        report.gaps += 1;
        tracing::info!(gap_id = id, blocks = slots.len(), "gap repaired");
    }
    Ok(report)
}

async fn fetch_block(rpc: &Rpc, dec: &Decoder, slot: u64, index_failed: bool) -> Result<SlotBatch> {
    let block: Value = rpc
        .call(
            "getBlock",
            json!([slot, {
                "encoding": "json", "maxSupportedTransactionVersion": 1,
                "transactionDetails": "full", "rewards": false, "commitment": "confirmed"
            }]),
        )
        .await?;
    let mut batch = SlotBatch::new(slot);
    batch.meta = Some(SlotMeta {
        parent_slot: block["parentSlot"].as_u64(),
        block_time: block["blockTime"].as_i64(),
        block_height: block["blockHeight"].as_u64(),
        blockhash: block["blockhash"].as_str().map(str::to_owned),
    });
    for (i, tx) in block["transactions"].as_array().into_iter().flatten().enumerate() {
        // Same filters as the live subscription.
        if !mentions(tx, dec.program_id_str()) || (!index_failed && !tx["meta"]["err"].is_null()) {
            continue;
        }
        if let Some(info) = tx_from_json(tx, i as u64) {
            extract_transaction(dec, slot, &info, &mut batch);
        }
    }
    Ok(batch)
}
