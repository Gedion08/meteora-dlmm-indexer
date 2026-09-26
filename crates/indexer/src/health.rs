//! Shared liveness/readiness state plus the /healthz, /readyz and /metrics server.

use std::net::SocketAddr;
use std::sync::atomic::{AtomicBool, AtomicU64, Ordering};
use std::sync::Arc;
use std::time::{Duration, SystemTime, UNIX_EPOCH};

use axum::extract::State;
use axum::http::StatusCode;
use axum::routing::get;
use axum::{Json, Router};
use metrics_exporter_prometheus::PrometheusHandle;
use serde_json::json;
use tokio_util::sync::CancellationToken;

pub struct Health {
    last_msg_ms: AtomicU64,
    connected: Vec<AtomicBool>,
    pub chain_tip: AtomicU64,
    pub indexed_slot: AtomicU64,
    pub finalized_slot: AtomicU64,
    idle_timeout: Duration,
    max_ready_lag: u64,
}

fn now_ms() -> u64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .unwrap_or_default()
        .as_millis() as u64
}

impl Health {
    pub fn new(sources: usize, idle_timeout: Duration, max_ready_lag: u64) -> Arc<Self> {
        Arc::new(Self {
            last_msg_ms: AtomicU64::new(now_ms()),
            connected: (0..sources).map(|_| AtomicBool::new(false)).collect(),
            chain_tip: AtomicU64::new(0),
            indexed_slot: AtomicU64::new(0),
            finalized_slot: AtomicU64::new(0),
            idle_timeout,
            max_ready_lag,
        })
    }

    pub fn touch(&self) {
        self.last_msg_ms.store(now_ms(), Ordering::Relaxed);
    }

    pub fn set_connected(&self, source: usize, up: bool) {
        if let Some(c) = self.connected.get(source) {
            c.store(up, Ordering::Relaxed);
        }
        let n = self.connected.iter().filter(|c| c.load(Ordering::Relaxed)).count();
        metrics::gauge!("grpc_sources_connected").set(n as f64);
    }

    pub fn observe_tip(&self, slot: u64) {
        let prev = self.chain_tip.fetch_max(slot, Ordering::Relaxed);
        if slot > prev {
            metrics::gauge!("chain_tip_slot").set(slot as f64);
            self.update_lag();
        }
    }

    pub fn observe_indexed(&self, slot: u64) {
        self.indexed_slot.fetch_max(slot, Ordering::Relaxed);
        metrics::gauge!("indexed_slot").set(self.indexed_slot.load(Ordering::Relaxed) as f64);
        self.update_lag();
    }

    pub fn observe_finalized(&self, slot: u64) {
        self.finalized_slot.fetch_max(slot, Ordering::Relaxed);
        metrics::gauge!("finalized_slot").set(slot as f64);
    }

    fn lag(&self) -> u64 {
        let indexed = self.indexed_slot.load(Ordering::Relaxed);
        if indexed == 0 {
            return u64::MAX;
        }
        self.chain_tip.load(Ordering::Relaxed).saturating_sub(indexed)
    }

    fn update_lag(&self) {
        let lag = self.lag();
        if lag != u64::MAX {
            metrics::gauge!("slot_lag").set(lag as f64);
        }
    }

    fn live(&self) -> bool {
        now_ms().saturating_sub(self.last_msg_ms.load(Ordering::Relaxed))
            < self.idle_timeout.as_millis() as u64 * 2
    }

    fn ready(&self) -> bool {
        self.live() && self.lag() <= self.max_ready_lag
    }

    fn report(&self) -> serde_json::Value {
        json!({
            "live": self.live(),
            "ready": self.ready(),
            "chain_tip": self.chain_tip.load(Ordering::Relaxed),
            "indexed_slot": self.indexed_slot.load(Ordering::Relaxed),
            "finalized_slot": self.finalized_slot.load(Ordering::Relaxed),
            "sources_connected": self.connected.iter().filter(|c| c.load(Ordering::Relaxed)).count(),
            "ms_since_last_message": now_ms().saturating_sub(self.last_msg_ms.load(Ordering::Relaxed)),
        })
    }
}

#[derive(Clone)]
struct AppState {
    health: Arc<Health>,
    prom: PrometheusHandle,
}

pub async fn serve(
    addr: SocketAddr,
    health: Arc<Health>,
    prom: PrometheusHandle,
    shutdown: CancellationToken,
) -> anyhow::Result<()> {
    let app = Router::new()
        .route(
            "/healthz",
            get(|State(s): State<AppState>| async move {
                let code = if s.health.live() { StatusCode::OK } else { StatusCode::SERVICE_UNAVAILABLE };
                (code, Json(s.health.report()))
            }),
        )
        .route(
            "/readyz",
            get(|State(s): State<AppState>| async move {
                let code = if s.health.ready() { StatusCode::OK } else { StatusCode::SERVICE_UNAVAILABLE };
                (code, Json(s.health.report()))
            }),
        )
        .route("/metrics", get(|State(s): State<AppState>| async move { s.prom.render() }))
        .with_state(AppState { health, prom });

    let listener = tokio::net::TcpListener::bind(addr).await?;
    tracing::info!(%addr, "http server listening");
    axum::serve(listener, app)
        .with_graceful_shutdown(async move { shutdown.cancelled().await })
        .await?;
    Ok(())
}
