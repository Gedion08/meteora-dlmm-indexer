//! Consumes assembler output: micro-batches slot batches into Postgres transactions,
//! retries forever on DB errors (backpressure propagates to the stream, and the stream
//! resumes from the checkpoint if the provider drops us), and handles dead slots.

use std::sync::Arc;
use std::time::{Duration, Instant};

use anyhow::Result;
use tokio::sync::mpsc;
use tokio_util::sync::CancellationToken;

use crate::assembler::WriterMsg;
use crate::health::Health;
use crate::model::SlotBatch;
use crate::snapshot::fetch_accounts;
use crate::rpc::Rpc;
use crate::source::Checkpoint;
use crate::store::Store;

const MAX_BATCHES_PER_TX: usize = 256;

pub struct Writer {
    pub store: Store,
    pub rpc: Rpc,
    pub flush_interval: Duration,
    pub checkpoint: Arc<Checkpoint>,
    pub health: Arc<Health>,
}

impl Writer {
    pub async fn run(self, mut rx: mpsc::Receiver<WriterMsg>, shutdown: CancellationToken) {
        let mut batches: Vec<SlotBatch> = Vec::new();
        let mut checkpoint = 0u64;
        loop {
            // Wait for the first message, then gather more for up to `flush_interval`.
            let Some(first) = rx.recv().await else { break };
            let deadline = Instant::now() + self.flush_interval;
            let mut control = Vec::new();
            let mut next = Some(first);
            while let Some(msg) = next.take() {
                match msg {
                    WriterMsg::Batch { batch, checkpoint: cp } => {
                        checkpoint = checkpoint.max(cp);
                        if !batch.is_empty() {
                            batches.push(batch);
                        }
                    }
                    other => control.push(other),
                }
                if batches.len() >= MAX_BATCHES_PER_TX || !control.is_empty() {
                    break;
                }
                next = tokio::time::timeout_at(deadline.into(), rx.recv()).await.ok().flatten();
            }

            self.flush(&mut batches, &mut checkpoint, &shutdown).await;
            for msg in control {
                self.control(msg, &shutdown).await;
            }
        }
        self.flush(&mut batches, &mut checkpoint, &shutdown).await;
        tracing::info!("writer stopped");
    }

    async fn flush(&self, batches: &mut Vec<SlotBatch>, checkpoint: &mut u64, shutdown: &CancellationToken) {
        if batches.is_empty() && *checkpoint == 0 {
            return;
        }
        let cp = (*checkpoint > 0).then_some(*checkpoint);
        let rows: usize = batches
            .iter()
            .map(|b| b.txs.len() + b.instructions.len() + b.events.len() + b.accounts.len())
            .sum();
        let started = Instant::now();
        retry("write batch", shutdown, || self.store.write(batches, cp)).await;
        metrics::histogram!("db_flush_seconds").record(started.elapsed().as_secs_f64());
        metrics::counter!("db_rows_written_total").increment(rows as u64);
        if let Some(cp) = cp {
            self.checkpoint.advance(cp);
            self.health.observe_indexed(cp);
        }
        batches.clear();
        *checkpoint = 0;
    }

    async fn control(&self, msg: WriterMsg, shutdown: &CancellationToken) {
        match msg {
            WriterMsg::Batch { .. } => unreachable!("batches are handled in run"),
            WriterMsg::Finalized(slot) => {
                retry("set finalized", shutdown, || self.store.set_state("finalized", slot)).await;
            }
            WriterMsg::Gap { from_slot, to_slot, reason } => {
                tracing::error!(from_slot, to_slot, %reason, "recording gap");
                metrics::counter!("gaps_recorded_total").increment(1);
                retry("record gap", shutdown, || self.store.record_gap(from_slot, to_slot, &reason)).await;
            }
            WriterMsg::DeadSlot(slot) => {
                let stale = retry("rollback slot", shutdown, || self.store.rollback_slot(slot)).await;
                tracing::warn!(slot, refreshed_accounts = stale.len(), "rolled back dead slot");
                if stale.is_empty() {
                    return;
                }
                match fetch_accounts(&self.rpc, &stale).await {
                    Ok((rows, failures)) => {
                        retry("refresh accounts", shutdown, || self.store.upsert_accounts(&rows)).await;
                        if !failures.is_empty() {
                            retry("record failures", shutdown, || self.store.insert_failures(&failures)).await;
                        }
                    }
                    Err(e) => tracing::error!(slot, error = format!("{e:#}"), "account refresh failed"),
                }
            }
        }
    }
}

/// Retry until success or shutdown. Data is never dropped on transient DB errors.
async fn retry<T, F, Fut>(what: &str, shutdown: &CancellationToken, mut f: F) -> T
where
    F: FnMut() -> Fut,
    Fut: std::future::Future<Output = Result<T>>,
    T: Default,
{
    let mut delay = Duration::from_millis(200);
    loop {
        match f().await {
            Ok(v) => return v,
            Err(e) => {
                metrics::counter!("db_errors_total").increment(1);
                tracing::error!(what, error = format!("{e:#}"), "database operation failed; retrying");
                if shutdown.is_cancelled() && delay >= Duration::from_secs(5) {
                    tracing::error!(what, "giving up during shutdown; data will be replayed on restart");
                    return T::default();
                }
                tokio::time::sleep(delay).await;
                delay = (delay * 2).min(Duration::from_secs(10));
            }
        }
    }
}
