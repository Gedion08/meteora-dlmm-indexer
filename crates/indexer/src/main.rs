mod assembler;
mod config;
mod extract;
mod health;
mod model;
mod rpc;
mod snapshot;
mod source;
mod store;
mod tail;
mod writer;

use std::path::PathBuf;
use std::time::Duration;

use anyhow::{Context, Result};
use clap::{Parser, Subcommand};
use dlmm_decoder::Decoder;
use metrics_exporter_prometheus::PrometheusBuilder;
use tokio::sync::mpsc;
use tokio_util::sync::CancellationToken;

use crate::assembler::Assembler;
use crate::config::{DbConfig, RpcConfig, ServerConfig, StreamConfig};
use crate::health::Health;
use crate::source::{build_sources, Checkpoint, SourceMsg};

#[derive(Parser)]
#[command(name = "dlmm-indexer", version, about = "Meteora DLMM live indexer")]
struct Cli {
    /// Emit logs as JSON (for log aggregation in production).
    #[arg(long, env = "LOG_JSON", default_value_t = false, global = true)]
    log_json: bool,

    #[command(subcommand)]
    cmd: Cmd,
}

#[derive(Subcommand)]
enum Cmd {
    /// Stream, decode and persist DLMM data (runs migrations first).
    Run {
        #[command(flatten)]
        stream: StreamConfig,
        #[command(flatten)]
        db: DbConfig,
        #[command(flatten)]
        rpc: RpcConfig,
        #[command(flatten)]
        server: ServerConfig,
    },
    /// Load current state of all DLMM accounts via getProgramAccountsV2.
    Snapshot {
        #[command(flatten)]
        db: DbConfig,
        #[command(flatten)]
        rpc: RpcConfig,
        /// Account types to snapshot (default: all), e.g. LbPair,PositionV2
        #[arg(long, value_delimiter = ',')]
        types: Vec<String>,
        #[arg(long, default_value_t = 1000)]
        page_size: usize,
    },
    /// Apply database migrations and exit.
    Migrate {
        #[command(flatten)]
        db: DbConfig,
    },
    /// Stream and print decoded data without a database (smoke test / debugging).
    Tail {
        #[command(flatten)]
        stream: StreamConfig,
        /// Stop after this many seconds.
        #[arg(long)]
        duration_secs: Option<u64>,
        /// Also record raw updates to this file (length-delimited protobuf) for test fixtures.
        #[arg(long)]
        record: Option<PathBuf>,
        /// Print every instruction, not just events.
        #[arg(long, default_value_t = false)]
        verbose: bool,
    },
}

fn init_tracing(json: bool) {
    use tracing_subscriber::{fmt, EnvFilter};
    let filter = EnvFilter::try_from_default_env().unwrap_or_else(|_| EnvFilter::new("info"));
    if json {
        fmt().with_env_filter(filter).with_writer(std::io::stderr).json().with_current_span(false).init();
    } else {
        fmt().with_env_filter(filter).with_writer(std::io::stderr).init();
    }
}

fn shutdown_signal() -> CancellationToken {
    let token = CancellationToken::new();
    let t = token.clone();
    tokio::spawn(async move {
        let ctrl_c = tokio::signal::ctrl_c();
        #[cfg(unix)]
        {
            let mut term = tokio::signal::unix::signal(tokio::signal::unix::SignalKind::terminate())
                .expect("install SIGTERM handler");
            tokio::select! { _ = ctrl_c => {}, _ = term.recv() => {} }
        }
        #[cfg(not(unix))]
        let _ = ctrl_c.await;
        tracing::info!("shutdown requested");
        t.cancel();
    });
    token
}

#[tokio::main]
async fn main() -> Result<()> {
    let _ = dotenvy::dotenv();
    let cli = Cli::parse();
    init_tracing(cli.log_json);
    // rustls needs a process-wide crypto provider when several are compiled in.
    let _ = rustls::crypto::ring::default_provider().install_default();

    let dec = Decoder::bundled();
    tracing::info!(
        program = dec.program_id_str(),
        idl_version = dec.idl_version(),
        instructions = dec.instruction_names().len(),
        events = dec.event_names().len(),
        accounts = dec.account_names().len(),
        "decoder loaded"
    );

    match cli.cmd {
        Cmd::Run { stream, db, rpc, server } => run(stream, db, rpc, server).await,
        Cmd::Snapshot { db, rpc, types, page_size } => {
            let store = store::Store::connect(&db).await?;
            store.migrate().await?;
            snapshot::run(&store, &rpc::Rpc::new(rpc.rpc_url)?, &types, page_size).await
        }
        Cmd::Migrate { db } => {
            store::Store::connect(&db).await?.migrate().await?;
            tracing::info!("migrations applied");
            Ok(())
        }
        Cmd::Tail { stream, duration_secs, record, verbose } => {
            tail::run(stream, duration_secs.map(Duration::from_secs), record, verbose).await
        }
    }
}

async fn run(stream: StreamConfig, db: DbConfig, rpc_cfg: RpcConfig, server: ServerConfig) -> Result<()> {
    let prom = PrometheusBuilder::new()
        .set_buckets(&[0.005, 0.01, 0.025, 0.05, 0.1, 0.25, 0.5, 1.0, 2.5, 5.0, 10.0])?
        .install_recorder()
        .context("installing metrics recorder")?;
    let shutdown = shutdown_signal();

    let store = store::Store::connect(&db).await?;
    store.migrate().await.context("running migrations")?;
    let start = store.get_state("checkpoint").await?.unwrap_or(0);
    if start > 0 {
        tracing::info!(checkpoint = start, "resuming from checkpoint");
    } else {
        tracing::info!("no checkpoint; starting at the chain tip");
    }

    let checkpoint = Checkpoint::new(start);
    let health = Health::new(stream.grpc_endpoints.len(), stream.idle_timeout(), server.max_ready_lag_slots);
    health.observe_indexed(start);
    let sources = build_sources(&stream, dec_program(), &checkpoint, &health)?;

    let (src_tx, src_rx) = mpsc::channel::<SourceMsg>(50_000);
    let (w_tx, w_rx) = mpsc::channel(2_048);

    let mut tasks = tokio::task::JoinSet::new();
    let source_shutdown = shutdown.child_token();
    for s in sources {
        tasks.spawn(s.run(src_tx.clone(), source_shutdown.clone()));
    }
    drop(src_tx);

    let assembler = Assembler::new(start, stream.slot_meta_timeout(), health.clone(), w_tx);
    let asm = tokio::spawn(assembler.run(src_rx, shutdown.clone()));

    let writer = writer::Writer {
        store,
        rpc: rpc::Rpc::new(rpc_cfg.rpc_url)?,
        flush_interval: Duration::from_millis(db.flush_interval_ms),
        checkpoint,
        health: health.clone(),
    };
    let wr = tokio::spawn(writer.run(w_rx, shutdown.clone()));
    let http = tokio::spawn(health::serve(server.http_addr, health, prom, shutdown.clone()));

    shutdown.cancelled().await;
    // Order: stop sources -> assembler flushes -> writer drains -> exit.
    source_shutdown.cancel();
    while tasks.join_next().await.is_some() {}
    let _ = asm.await;
    let _ = wr.await;
    let _ = http.await;
    tracing::info!("stopped cleanly");
    Ok(())
}

fn dec_program() -> &'static str {
    Decoder::bundled().program_id_str()
}
