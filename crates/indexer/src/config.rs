use std::net::SocketAddr;
use std::time::Duration;

use clap::{Args, ValueEnum};

#[derive(Debug, Clone, Copy, ValueEnum, PartialEq, Eq)]
pub enum Commitment {
    Processed,
    Confirmed,
    Finalized,
}

impl Commitment {
    pub fn as_proto(self) -> i32 {
        use yellowstone_grpc_proto::geyser::CommitmentLevel as C;
        match self {
            Commitment::Processed => C::Processed as i32,
            Commitment::Confirmed => C::Confirmed as i32,
            Commitment::Finalized => C::Finalized as i32,
        }
    }
}

/// One or more Yellowstone-compatible gRPC endpoints. Multiple endpoints are consumed
/// concurrently and deduplicated, so a stalled provider doesn't stall the indexer.
#[derive(Debug, Clone, Args)]
pub struct StreamConfig {
    /// Comma-separated gRPC endpoints, e.g. https://laserstream-mainnet-ewr.helius-rpc.com
    #[arg(long, env = "GRPC_ENDPOINTS", value_delimiter = ',', required = true)]
    pub grpc_endpoints: Vec<String>,

    /// Comma-separated x-token per endpoint (same order). A single token is reused for all.
    #[arg(long, env = "GRPC_X_TOKENS", value_delimiter = ',', hide_env_values = true)]
    pub grpc_x_tokens: Vec<String>,

    #[arg(long, env = "COMMITMENT", value_enum, default_value = "confirmed")]
    pub commitment: Commitment,

    /// Also index failed transactions (mostly bot spam; off by default).
    #[arg(long, env = "INDEX_FAILED_TXS", default_value_t = false)]
    pub index_failed_txs: bool,

    /// Reconnect if no message (including pings) arrives for this long.
    #[arg(long, env = "STREAM_IDLE_TIMEOUT_SECS", default_value_t = 30)]
    pub idle_timeout_secs: u64,

    /// Emit a slot without its block meta after this long (block_time left null).
    #[arg(long, env = "SLOT_META_TIMEOUT_MS", default_value_t = 3000)]
    pub slot_meta_timeout_ms: u64,

    /// Slots to rewind before the checkpoint when resuming (writes are idempotent).
    #[arg(long, env = "RESUME_REWIND_SLOTS", default_value_t = 64)]
    pub resume_rewind_slots: u64,
}

impl StreamConfig {
    pub fn token_for(&self, i: usize) -> Option<String> {
        match self.grpc_x_tokens.len() {
            0 => None,
            1 => Some(self.grpc_x_tokens[0].clone()),
            _ => self.grpc_x_tokens.get(i).cloned(),
        }
        .filter(|t| !t.is_empty())
    }

    pub fn idle_timeout(&self) -> Duration {
        Duration::from_secs(self.idle_timeout_secs)
    }

    pub fn slot_meta_timeout(&self) -> Duration {
        Duration::from_millis(self.slot_meta_timeout_ms)
    }
}

#[derive(Debug, Clone, Args)]
pub struct DbConfig {
    #[arg(long, env = "DATABASE_URL", hide_env_values = true)]
    pub database_url: String,

    #[arg(long, env = "DB_MAX_CONNECTIONS", default_value_t = 10)]
    pub db_max_connections: u32,

    /// Max time a batch waits to be flushed; lower = fresher DB, more transactions.
    #[arg(long, env = "FLUSH_INTERVAL_MS", default_value_t = 100)]
    pub flush_interval_ms: u64,
}

#[derive(Debug, Clone, Args)]
pub struct RpcConfig {
    /// JSON-RPC endpoint used for the state snapshot and dead-slot account refresh.
    #[arg(long, env = "RPC_URL", hide_env_values = true)]
    pub rpc_url: String,
}

#[derive(Debug, Clone, Args)]
pub struct ServerConfig {
    /// Serves /healthz, /readyz and /metrics.
    #[arg(long, env = "HTTP_ADDR", default_value = "0.0.0.0:9100")]
    pub http_addr: SocketAddr,

    /// /readyz fails when the indexed slot trails the chain tip by more than this.
    #[arg(long, env = "MAX_READY_LAG_SLOTS", default_value_t = 150)]
    pub max_ready_lag_slots: u64,
}
