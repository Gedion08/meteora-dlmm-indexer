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

/// How to find the blocks to re-fetch for a gap.
#[derive(Debug, Clone, Copy, PartialEq, Eq, clap::ValueEnum)]
pub enum RepairMode {
    /// Every block in the range (`getBlocks`): exhaustive, cost grows with chain length.
    Blocks,
    /// Only blocks containing transactions that reference DLMM (`getSignaturesForAddress`):
    /// cost grows with DLMM activity, so multi-day gaps stay cheap.
    Signatures,
    /// Blocks for gaps up to `max_block_slots`, signatures beyond.
    Auto,
}

pub struct RepairOptions {
    pub mode: RepairMode,
    pub max_block_slots: u64,
    pub concurrency: usize,
    pub index_failed: bool,
}

pub async fn repair(store: &Store, rpc: &Rpc, opts: &RepairOptions) -> Result<RepairReport> {
    let gaps: Vec<(i64, i64, i64, Option<i64>)> = sqlx::query_as(
        "SELECT id, from_slot, to_slot, progress_slot FROM gaps WHERE repaired_at IS NULL ORDER BY from_slot",
    )
    .fetch_all(store.pool())
    .await?;
    let mut report = RepairReport { gaps: 0, blocks: 0, transactions: 0, accounts_refreshed: 0 };
    let dec = Decoder::bundled();

    for (id, from, to, progress) in gaps {
        let from = from as u64;
        // Resume below whatever an earlier (interrupted) run already finished.
        let to = progress.map_or(to as u64, |p| (p as u64).saturating_sub(1));
        if to < from {
            mark_repaired(store, id).await?;
            continue;
        }
        let span = to - from + 1;
        let by_signatures = match opts.mode {
            RepairMode::Signatures => true,
            RepairMode::Blocks if span > opts.max_block_slots => {
                tracing::info!(gap_id = id, span, "gap too large for block mode; use --mode signatures");
                continue;
            }
            RepairMode::Blocks => false,
            RepairMode::Auto => span > opts.max_block_slots,
        };
        tracing::info!(gap_id = id, from, to, span, by_signatures, resumed = progress.is_some(), "repairing gap");
        let mut slots = if by_signatures {
            signature_slots(store, rpc, dec.program_id_str(), from, to, opts.index_failed).await?
        } else {
            block_slots(rpc, from, to).await?
        };
        // Newest first: the finished part is always [progress_slot, to_slot], so a restart
        // only rescans what's left.
        slots.sort_unstable_by(|a, b| b.cmp(a));
        tracing::info!(gap_id = id, blocks = slots.len(), "fetching blocks for gap");

        let mut pending: BTreeSet<u64> = slots.iter().copied().collect();
        let mut touched: BTreeSet<String> = BTreeSet::new();
        let mut results = stream::iter(slots.clone())
            .map(|slot| async move { (slot, fetch_block(rpc, dec, slot, opts.index_failed).await) })
            .buffered(opts.concurrency.max(1));
        let mut done = 0usize;
        while let Some((slot, res)) = results.next().await {
            let batch = res?;
            done += 1;
            report.blocks += 1;
            report.transactions += batch.txs.len();
            for ix in &batch.instructions {
                if let Some(obj) = ix.accounts.as_object() {
                    touched.extend(obj.values().filter_map(|v| v.as_str().map(str::to_owned)));
                }
            }
            store.write(std::slice::from_ref(&batch), None).await?;
            pending.remove(&slot);
            if done % 200 == 0 || pending.is_empty() {
                // Refresh state touched so far, then persist progress (after the data).
                report.accounts_refreshed += refresh_touched(store, rpc, &mut touched).await?;
                let floor = pending.last().map_or(from, |p| p + 1);
                sqlx::query("UPDATE gaps SET progress_slot = $2 WHERE id = $1")
                    .bind(id)
                    .bind(floor as i64)
                    .execute(store.pool())
                    .await?;
                tracing::info!(gap_id = id, done, total = slots.len(), down_to_slot = floor, "gap repair progress");
            }
        }
        report.accounts_refreshed += refresh_touched(store, rpc, &mut touched).await?;
        mark_repaired(store, id).await?;
        report.gaps += 1;
        tracing::info!(gap_id = id, blocks = slots.len(), "gap repaired");
    }
    Ok(report)
}

async fn mark_repaired(store: &Store, id: i64) -> Result<()> {
    sqlx::query("UPDATE gaps SET repaired_at = now() WHERE id = $1")
        .bind(id)
        .execute(store.pool())
        .await?;
    metrics::counter!("gaps_repaired_total").increment(1);
    Ok(())
}

/// Re-read the DLMM accounts that repaired transactions touched (their final state).
async fn refresh_touched(store: &Store, rpc: &Rpc, touched: &mut BTreeSet<String>) -> Result<usize> {
    if touched.is_empty() {
        return Ok(0);
    }
    let keys: Vec<String> = std::mem::take(touched).into_iter().collect();
    let (rows, failures) = fetch_accounts(rpc, &keys).await?;
    store.upsert_accounts(&rows).await?;
    if !failures.is_empty() {
        store.insert_failures(&failures).await?;
    }
    Ok(rows.len())
}

async fn block_slots(rpc: &Rpc, from: u64, to: u64) -> Result<Vec<u64>> {
    let mut slots = Vec::new();
    let mut start = from;
    while start <= to {
        let end = (start + 49_999).min(to);
        let chunk: Vec<u64> = rpc.call("getBlocks", json!([start, end, { "commitment": "confirmed" }])).await?;
        slots.extend(chunk);
        start = end + 1;
    }
    Ok(slots)
}

#[derive(serde::Deserialize)]
struct SigInfo {
    signature: String,
    slot: u64,
    err: Option<Value>,
}

/// Slots in [from, to] holding transactions that reference the program. Walks
/// `getSignaturesForAddress` backwards, starting just after the gap when we have an
/// indexed transaction there (so nothing newer than the gap is listed).
async fn signature_slots(
    store: &Store,
    rpc: &Rpc,
    program: &str,
    from: u64,
    to: u64,
    index_failed: bool,
) -> Result<Vec<u64>> {
    let mut before: Option<String> =
        sqlx::query_scalar("SELECT signature FROM transactions WHERE slot > $1 ORDER BY slot, tx_index LIMIT 1")
            .bind(to as i64)
            .fetch_optional(store.pool())
            .await?;
    let mut slots: BTreeSet<u64> = BTreeSet::new();
    let mut pages = 0usize;
    loop {
        let mut cfg = json!({ "limit": 1000, "commitment": "confirmed" });
        if let Some(b) = &before {
            cfg["before"] = json!(b);
        }
        let page: Vec<SigInfo> = rpc.call("getSignaturesForAddress", json!([program, cfg])).await?;
        pages += 1;
        let Some(last) = page.last() else { break };
        let (last_slot, last_sig) = (last.slot, last.signature.clone());
        slots.extend(
            page.iter()
                .filter(|s| s.slot >= from && s.slot <= to && (index_failed || s.err.is_none()))
                .map(|s| s.slot),
        );
        if pages % 20 == 0 {
            tracing::info!(pages, slots = slots.len(), at_slot = last_slot, "scanning gap signatures");
        }
        if last_slot < from {
            break;
        }
        before = Some(last_sig);
    }
    Ok(slots.into_iter().collect())
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
