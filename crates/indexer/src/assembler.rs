//! Groups stream updates into per-slot batches.
//!
//! - Deduplicates transactions (by signature) and account writes across providers.
//! - Emits a slot once its block meta arrives (so rows carry `block_time`), or after a
//!   timeout without it. Data arriving for an already-emitted slot is emitted as a
//!   follow-up batch; writes are idempotent so this is safe.
//! - Tracks finalized / dead slots and computes the resume checkpoint: every slot at or
//!   below it has been handed to the writer.

use std::collections::{BTreeMap, HashMap, HashSet};
use std::sync::Arc;
use std::time::{Duration, Instant};

use dlmm_decoder::Decoder;
use tokio::sync::mpsc;
use tokio_util::sync::CancellationToken;
use yellowstone_grpc_proto::geyser::{subscribe_update::UpdateOneof, SlotStatus};

use crate::extract::{extract_account, extract_transaction, AccountOutcome};
use crate::health::Health;
use crate::model::{AccountRow, SlotBatch, SlotMeta};
use crate::source::SourceMsg;

pub enum WriterMsg {
    Batch { batch: SlotBatch, checkpoint: u64 },
    Finalized(u64),
    DeadSlot(u64),
    Gap { from_slot: u64, to_slot: u64, reason: String },
}

/// How many slots behind the newest one we keep dedup state for.
const RETAIN_SLOTS: u64 = 512;

struct SlotBuf {
    batch: SlotBatch,
    first_seen: Instant,
    meta: Option<SlotMeta>,
    emitted: bool,
    sigs: HashSet<Vec<u8>>,
    /// pubkey -> (source, write_version, pending row). write_version is only comparable
    /// within one provider, so the first provider to write an account in a slot owns it.
    accounts: HashMap<Vec<u8>, (usize, u64, Option<AccountRow>)>,
}

impl SlotBuf {
    fn new(slot: u64) -> Self {
        Self {
            batch: SlotBatch::new(slot),
            first_seen: Instant::now(),
            meta: None,
            emitted: false,
            sigs: HashSet::new(),
            accounts: HashMap::new(),
        }
    }

    fn has_pending(&self) -> bool {
        !self.batch.is_empty() || self.accounts.values().any(|a| a.2.is_some())
    }

    fn take_batch(&mut self, slot: u64) -> SlotBatch {
        let mut b = std::mem::replace(&mut self.batch, SlotBatch::new(slot));
        b.accounts
            .extend(self.accounts.values_mut().filter_map(|a| a.2.take()));
        b
    }
}

pub struct Assembler {
    decoder: &'static Decoder,
    slots: BTreeMap<u64, SlotBuf>,
    meta_timeout: Duration,
    checkpoint: u64,
    finalized: u64,
    finalized_sent: u64,
    max_slot: u64,
    pending_gaps: HashMap<usize, u64>,
    health: Arc<Health>,
    out: mpsc::Sender<WriterMsg>,
}

impl Assembler {
    pub fn new(
        start_checkpoint: u64,
        meta_timeout: Duration,
        health: Arc<Health>,
        out: mpsc::Sender<WriterMsg>,
    ) -> Self {
        Self {
            decoder: Decoder::bundled(),
            slots: BTreeMap::new(),
            meta_timeout,
            checkpoint: start_checkpoint,
            finalized: 0,
            finalized_sent: 0,
            max_slot: 0,
            pending_gaps: HashMap::new(),
            health,
            out,
        }
    }

    pub async fn run(mut self, mut rx: mpsc::Receiver<SourceMsg>, shutdown: CancellationToken) {
        let mut tick = tokio::time::interval(Duration::from_millis(50));
        tick.set_missed_tick_behavior(tokio::time::MissedTickBehavior::Delay);
        loop {
            tokio::select! {
                msg = rx.recv() => match msg {
                    Some(m) => {
                        if self.handle(m).await.is_err() { return; }
                    }
                    None => break,
                },
                _ = tick.tick() => {
                    if self.flush(false).await.is_err() { return; }
                }
                _ = shutdown.cancelled() => break,
            }
        }
        // Drain whatever is buffered so a graceful stop loses nothing.
        while let Ok(m) = rx.try_recv() {
            if self.handle(m).await.is_err() {
                return;
            }
        }
        let _ = self.flush(true).await;
    }

    fn buf(&mut self, slot: u64) -> Option<&mut SlotBuf> {
        // Ignore stragglers for slots whose dedup state was already dropped.
        if self.max_slot > RETAIN_SLOTS && slot < self.max_slot - RETAIN_SLOTS {
            metrics::counter!("stale_updates_dropped_total").increment(1);
            return None;
        }
        self.max_slot = self.max_slot.max(slot);
        Some(self.slots.entry(slot).or_insert_with(|| SlotBuf::new(slot)))
    }

    async fn handle(&mut self, msg: SourceMsg) -> Result<(), ()> {
        let (source, update) = match msg {
            SourceMsg::ReplayUnavailable { source, from_slot } => {
                self.pending_gaps.insert(source, from_slot);
                return Ok(());
            }
            SourceMsg::Update { source, update } => (source, update),
        };
        let Some(u) = update.update_oneof else { return Ok(()) };

        let slot = match &u {
            UpdateOneof::Transaction(t) => t.slot,
            UpdateOneof::Account(a) => a.slot,
            UpdateOneof::BlockMeta(m) => m.slot,
            UpdateOneof::Slot(s) => s.slot,
            _ => return Ok(()),
        };
        if let Some(from) = self.pending_gaps.remove(&source) {
            if slot > from {
                self.send(WriterMsg::Gap {
                    from_slot: from,
                    to_slot: slot - 1,
                    reason: format!("replay unavailable on source {source}"),
                })
                .await?;
            }
        }

        let decoder = self.decoder;
        match u {
            UpdateOneof::Transaction(t) => {
                let Some(info) = t.transaction else { return Ok(()) };
                let Some(buf) = self.buf(slot) else { return Ok(()) };
                if !buf.sigs.insert(info.signature.clone()) {
                    metrics::counter!("duplicate_updates_total", "kind" => "transaction").increment(1);
                    return Ok(());
                }
                // Many transactions only *reference* DLMM accounts (bots reading pool
                // state) without invoking the program; those are dropped here.
                if !extract_transaction(decoder, slot, &info, &mut buf.batch) {
                    metrics::counter!("transactions_ignored_total").increment(1);
                }
            }
            UpdateOneof::Account(a) => {
                let Some(info) = a.account else { return Ok(()) };
                let Some(buf) = self.buf(slot) else { return Ok(()) };
                let wv = info.write_version;
                if let Some((owner, prev_wv, _)) = buf.accounts.get(&info.pubkey) {
                    if *owner != source || *prev_wv >= wv {
                        metrics::counter!("duplicate_updates_total", "kind" => "account").increment(1);
                        return Ok(());
                    }
                }
                match extract_account(decoder, slot, &info) {
                    AccountOutcome::Row(row) => {
                        buf.accounts.insert(info.pubkey, (source, wv, Some(row)));
                    }
                    AccountOutcome::Closed(c) => {
                        buf.accounts.insert(info.pubkey, (source, wv, None));
                        buf.batch.closes.push(c);
                    }
                    AccountOutcome::Failed(f) => {
                        buf.accounts.insert(info.pubkey, (source, wv, None));
                        buf.batch.failures.push(f);
                    }
                }
            }
            UpdateOneof::BlockMeta(m) => {
                let Some(buf) = self.buf(slot) else { return Ok(()) };
                if buf.meta.is_some() {
                    return Ok(());
                }
                let meta = SlotMeta {
                    parent_slot: Some(m.parent_slot),
                    block_time: m.block_time.map(|t| t.timestamp),
                    block_height: m.block_height.map(|h| h.block_height),
                    blockhash: Some(m.blockhash),
                };
                if buf.emitted {
                    buf.batch.backfill_block_time = true;
                }
                buf.batch.meta = Some(meta.clone());
                buf.meta = Some(meta);
                self.flush(false).await?;
            }
            UpdateOneof::Slot(s) => {
                self.health.observe_tip(slot);
                match SlotStatus::try_from(s.status) {
                    Ok(SlotStatus::SlotFinalized) => {
                        self.finalized = self.finalized.max(slot);
                        self.health.observe_finalized(self.finalized);
                    }
                    Ok(SlotStatus::SlotDead) => {
                        tracing::warn!(slot, error = ?s.dead_error, "slot dead");
                        metrics::counter!("dead_slots_total").increment(1);
                        if let Some(buf) = self.slots.get_mut(&slot) {
                            let was_emitted = buf.emitted;
                            // Keep the entry (emitted, empty) so late data for it is ignored.
                            buf.batch = SlotBatch::new(slot);
                            buf.accounts.clear();
                            buf.emitted = true;
                            if was_emitted {
                                self.send(WriterMsg::DeadSlot(slot)).await?;
                            }
                        }
                    }
                    _ => {}
                }
            }
            _ => {}
        }
        Ok(())
    }

    async fn send(&self, msg: WriterMsg) -> Result<(), ()> {
        self.out.send(msg).await.map_err(|_| ())
    }

    async fn flush(&mut self, all: bool) -> Result<(), ()> {
        let now = Instant::now();
        let mut ready = Vec::new();
        for (&slot, buf) in self.slots.iter_mut() {
            let due = buf.emitted
                || all
                || buf.meta.is_some()
                || now.duration_since(buf.first_seen) >= self.meta_timeout;
            if !due {
                continue;
            }
            if !buf.emitted && buf.meta.is_none() {
                metrics::counter!("slots_emitted_without_meta_total").increment(1);
            }
            buf.emitted = true;
            if buf.has_pending() {
                ready.push(buf.take_batch(slot));
            }
        }

        // Everything below the oldest not-yet-emitted slot has been handed off.
        let horizon = match self.slots.iter().find(|(_, b)| !b.emitted) {
            Some((&s, _)) => s.saturating_sub(1),
            None => self.max_slot,
        };
        self.checkpoint = self.checkpoint.max(horizon);

        let n = ready.len();
        for (i, batch) in ready.into_iter().enumerate() {
            // Only the last batch carries the new checkpoint, so it commits after its data.
            let checkpoint = if i + 1 == n { self.checkpoint } else { 0 };
            self.send(WriterMsg::Batch { batch, checkpoint }).await?;
        }
        if n == 0 && self.checkpoint > 0 && all {
            self.send(WriterMsg::Batch { batch: SlotBatch::new(self.checkpoint), checkpoint: self.checkpoint })
                .await?;
        }

        if self.finalized > self.finalized_sent {
            self.finalized_sent = self.finalized;
            self.send(WriterMsg::Finalized(self.finalized)).await?;
        }

        if self.max_slot > RETAIN_SLOTS {
            let keep_from = self.max_slot - RETAIN_SLOTS;
            self.slots.retain(|&s, b| s >= keep_from || !b.emitted);
        }
        metrics::gauge!("assembler_buffered_slots").set(self.slots.len() as f64);
        Ok(())
    }
}
