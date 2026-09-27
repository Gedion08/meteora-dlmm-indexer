//! Yellowstone gRPC subscription with reconnect, checkpoint replay and idle detection.

use std::collections::HashMap;
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::Arc;
use std::time::{Duration, Instant};

use anyhow::{Context, Result};
use futures::{SinkExt, StreamExt};
use tokio::sync::mpsc;
use tokio_util::sync::CancellationToken;
use yellowstone_grpc_client::{ClientTlsConfig, GeyserGrpcClient};
use yellowstone_grpc_proto::geyser::{
    subscribe_update::UpdateOneof, SubscribeRequest, SubscribeRequestFilterAccounts,
    SubscribeRequestFilterBlocksMeta, SubscribeRequestFilterSlots,
    SubscribeRequestFilterTransactions, SubscribeRequestPing, SubscribeUpdate,
};

use crate::config::StreamConfig;
use crate::health::Health;

pub enum SourceMsg {
    Update { source: usize, update: SubscribeUpdate },
    /// Replay from the checkpoint was refused; data between `from_slot` and the first
    /// slot of the new stream is missing and gets recorded as a gap.
    ReplayUnavailable { source: usize, from_slot: u64 },
}

/// Resume position shared between the writer (which advances it after each commit)
/// and the sources (which read it on reconnect).
#[derive(Default)]
pub struct Checkpoint(AtomicU64);

impl Checkpoint {
    pub fn new(slot: u64) -> Arc<Self> {
        Arc::new(Self(AtomicU64::new(slot)))
    }
    pub fn get(&self) -> u64 {
        self.0.load(Ordering::Acquire)
    }
    pub fn advance(&self, slot: u64) {
        self.0.fetch_max(slot, Ordering::AcqRel);
    }
}

pub struct Source {
    pub index: usize,
    pub endpoint: String,
    pub x_token: Option<String>,
    pub cfg: StreamConfig,
    pub program_id: String,
    pub checkpoint: Arc<Checkpoint>,
    pub health: Arc<Health>,
}

enum StreamEnd {
    Shutdown,
    ReplayRejected,
}

impl Source {
    fn label(&self) -> String {
        // Endpoint host only; never log tokens.
        self.endpoint
            .split("://")
            .nth(1)
            .unwrap_or(&self.endpoint)
            .split(['/', '?'])
            .next()
            .unwrap_or_default()
            .to_owned()
    }

    fn request(&self, from_slot: Option<u64>) -> SubscribeRequest {
        SubscribeRequest {
            transactions: HashMap::from([(
                "dlmm".to_owned(),
                SubscribeRequestFilterTransactions {
                    vote: Some(false),
                    failed: (!self.cfg.index_failed_txs).then_some(false),
                    account_include: vec![self.program_id.clone()],
                    ..Default::default()
                },
            )]),
            accounts: HashMap::from([(
                "dlmm".to_owned(),
                SubscribeRequestFilterAccounts {
                    owner: vec![self.program_id.clone()],
                    ..Default::default()
                },
            )]),
            // Every status (not just the subscription commitment) so we see finalized/dead.
            slots: HashMap::from([(
                "slots".to_owned(),
                SubscribeRequestFilterSlots {
                    filter_by_commitment: Some(false),
                    interslot_updates: Some(false),
                },
            )]),
            blocks_meta: HashMap::from([("meta".to_owned(), SubscribeRequestFilterBlocksMeta {})]),
            commitment: Some(self.cfg.commitment.as_proto()),
            from_slot,
            ..Default::default()
        }
    }

    pub async fn run(self, tx: mpsc::Sender<SourceMsg>, shutdown: CancellationToken) {
        let label = self.label();
        metrics::counter!("grpc_reconnects_total", "source" => label.clone()).increment(0);
        let mut backoff = Duration::from_millis(250);
        let mut use_replay = true;
        loop {
            let cp = self.checkpoint.get();
            let from_slot = (use_replay && cp > 0)
                .then(|| cp.saturating_sub(self.cfg.resume_rewind_slots).max(1));
            let started = Instant::now();
            tracing::info!(source = %label, ?from_slot, "connecting");

            let res = self.stream_once(from_slot, &tx, &shutdown, &label).await;
            self.health.set_connected(self.index, false);
            let mut rejected = false;
            match res {
                Ok(StreamEnd::Shutdown) => return,
                Ok(StreamEnd::ReplayRejected) => {
                    let from = from_slot.unwrap_or_default();
                    tracing::error!(source = %label, from_slot = from, "replay unavailable; continuing from tip and recording a gap");
                    let _ = tx
                        .send(SourceMsg::ReplayUnavailable { source: self.index, from_slot: from })
                        .await;
                    use_replay = false;
                    continue;
                }
                Err(e) if is_auth_error(&e) => {
                    tracing::error!(source = %label, error = format!("{e:#}"),
                        "provider rejected the subscription (check API key / plan includes gRPC streaming)");
                    rejected = true;
                }
                Err(e) => {
                    tracing::warn!(source = %label, error = format!("{e:#}"), "stream ended");
                }
            }
            metrics::counter!("grpc_reconnects_total", "source" => label.clone()).increment(1);
            use_replay = true;
            if rejected {
                // Retrying fast won't help; keep trying slowly in case access is granted.
                backoff = Duration::from_secs(60);
            } else if started.elapsed() > Duration::from_secs(60) {
                backoff = Duration::from_millis(250);
            }
            let jitter = Duration::from_millis(rand_jitter_ms());
            tokio::select! {
                _ = shutdown.cancelled() => return,
                _ = tokio::time::sleep(backoff + jitter) => {}
            }
            if !rejected {
                backoff = (backoff * 2).min(Duration::from_secs(15));
            }
        }
    }

    async fn stream_once(
        &self,
        from_slot: Option<u64>,
        tx: &mpsc::Sender<SourceMsg>,
        shutdown: &CancellationToken,
        label: &str,
    ) -> Result<StreamEnd> {
        let mut client = GeyserGrpcClient::build_from_shared(self.endpoint.clone())?
            .x_token(self.x_token.clone())?
            .tls_config(ClientTlsConfig::new().with_native_roots())?
            .connect_timeout(Duration::from_secs(10))
            .tcp_nodelay(true)
            .http2_adaptive_window(true)
            .max_decoding_message_size(64 * 1024 * 1024)
            .connect()
            .await
            .context("connect")?;

        let (mut sink, mut stream) = client
            .subscribe_with_request(Some(self.request(from_slot)))
            .await
            .context("subscribe")?;

        let mut ping = tokio::time::interval(Duration::from_secs(10));
        ping.tick().await;
        let mut first = true;
        loop {
            let next = tokio::select! {
                _ = shutdown.cancelled() => return Ok(StreamEnd::Shutdown),
                _ = ping.tick() => {
                    sink.send(ping_request()).await.context("send ping")?;
                    continue;
                }
                n = tokio::time::timeout(self.cfg.idle_timeout(), stream.next()) => n,
            };
            let update = match next {
                Err(_) => anyhow::bail!("idle for {:?}", self.cfg.idle_timeout()),
                Ok(None) => anyhow::bail!("server closed stream"),
                Ok(Some(Err(status))) => {
                    if first && from_slot.is_some() && is_replay_rejection(&status) {
                        return Ok(StreamEnd::ReplayRejected);
                    }
                    return Err(anyhow::Error::new(status).context("stream error"));
                }
                Ok(Some(Ok(u))) => u,
            };
            if first {
                first = false;
                self.health.set_connected(self.index, true);
                tracing::info!(source = %label, "streaming");
            }
            self.health.touch();

            if let Some(created) = &update.created_at {
                let created_ms = created.seconds as f64 * 1e3 + created.nanos as f64 / 1e6;
                let now_ms = std::time::SystemTime::now()
                    .duration_since(std::time::UNIX_EPOCH)
                    .unwrap_or_default()
                    .as_secs_f64()
                    * 1e3;
                metrics::histogram!("grpc_update_delay_seconds", "source" => label.to_owned())
                    .record(((now_ms - created_ms) / 1e3).max(0.0));
            }

            match &update.update_oneof {
                Some(UpdateOneof::Ping(_)) => {
                    sink.send(ping_request()).await.context("reply ping")?;
                }
                Some(UpdateOneof::Pong(_)) | None => {}
                Some(_) => {
                    if tx
                        .send(SourceMsg::Update { source: self.index, update })
                        .await
                        .is_err()
                    {
                        return Ok(StreamEnd::Shutdown);
                    }
                }
            }
        }
    }
}

fn ping_request() -> SubscribeRequest {
    SubscribeRequest {
        ping: Some(SubscribeRequestPing { id: 1 }),
        ..Default::default()
    }
}

fn is_replay_rejection(status: &tonic::Status) -> bool {
    let msg = status.message().to_ascii_lowercase();
    status.code() == tonic::Code::InvalidArgument
        || status.code() == tonic::Code::OutOfRange
        || (msg.contains("slot") && (msg.contains("available") || msg.contains("old") || msg.contains("replay")))
}

fn is_auth_error(e: &anyhow::Error) -> bool {
    e.chain().any(|c| {
        c.downcast_ref::<tonic::Status>().is_some_and(|s| {
            matches!(s.code(), tonic::Code::PermissionDenied | tonic::Code::Unauthenticated)
        })
    }) || format!("{e:#}").contains("does not have permission")
}

fn rand_jitter_ms() -> u64 {
    let nanos = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .unwrap_or_default()
        .subsec_nanos();
    (nanos % 250) as u64
}

/// Validate endpoints early so misconfiguration fails at startup, not in the loop.
pub fn build_sources(
    cfg: &StreamConfig,
    program_id: &str,
    checkpoint: &Arc<Checkpoint>,
    health: &Arc<Health>,
) -> Result<Vec<Source>> {
    anyhow::ensure!(!cfg.grpc_endpoints.is_empty(), "no GRPC_ENDPOINTS configured");
    Ok(cfg
        .grpc_endpoints
        .iter()
        .enumerate()
        .map(|(i, e)| Source {
            index: i,
            endpoint: e.trim().to_owned(),
            x_token: cfg.token_for(i),
            cfg: cfg.clone(),
            program_id: program_id.to_owned(),
            checkpoint: checkpoint.clone(),
            health: health.clone(),
        })
        .collect())
}
