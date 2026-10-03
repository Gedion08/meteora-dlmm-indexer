mod assembler;
mod config;
mod extract;
mod health;
mod hydrate;
mod metadata;
mod model;
mod network;
mod reconcile;
mod repair;
mod rpc_block;
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
use crate::config::{DbConfig, MaintenanceConfig, RpcConfig, ServerConfig, StreamConfig};
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
        #[command(flatten)]
        maintenance: MaintenanceConfig,
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
        /// Stop after this many accounts per type (sampling / smoke tests).
        #[arg(long)]
        limit: Option<usize>,
    },
    /// Scan the stored slot chain for holes and record them as gaps.
    Audit {
        #[command(flatten)]
        db: DbConfig,
        /// Only check slots above this one (default: everything).
        #[arg(long, default_value_t = 0)]
        since_slot: u64,
    },
    /// Re-fetch blocks for open gaps via RPC and index them.
    RepairGaps {
        #[command(flatten)]
        db: DbConfig,
        #[command(flatten)]
        rpc: RpcConfig,
        /// blocks | signatures | auto (blocks up to --max-block-slots, signatures beyond).
        #[arg(long, value_enum, default_value = "auto")]
        mode: repair::RepairMode,
        #[arg(long, default_value_t = 20_000)]
        max_block_slots: u64,
        #[arg(long, default_value_t = 4)]
        concurrency: usize,
        #[arg(long, env = "INDEX_FAILED_TXS", default_value_t = false)]
        index_failed_txs: bool,
    },
    /// Compare stored accounts with on-chain state and heal drift (needs the indexer running).
    Reconcile {
        #[command(flatten)]
        db: DbConfig,
        #[command(flatten)]
        rpc: RpcConfig,
        #[arg(long, default_value_t = 200)]
        sample: usize,
        /// Check these accounts instead of a sample.
        #[arg(long, value_delimiter = ',')]
        pubkeys: Vec<String>,
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
        Cmd::Run { stream, db, rpc, server, maintenance } => run(stream, db, rpc, server, maintenance).await,
        Cmd::Audit { db, since_slot } => {
            let store = store::Store::connect(&db).await?;
            store.migrate().await?;
            let until = store.get_state("checkpoint").await?.unwrap_or(0).saturating_sub(32);
            let n = repair::audit(&store, since_slot, until).await?;
            println!("{}", serde_json::json!({ "gaps_recorded": n }));
            Ok(())
        }
        Cmd::RepairGaps { db, rpc, mode, max_block_slots, concurrency, index_failed_txs } => {
            let store = store::Store::connect(&db).await?;
            store.migrate().await?;
            let rpc = rpc::Rpc::new(rpc.rpc_url)?;
            network::verify_database(&store, &rpc).await?;
            let opts = repair::RepairOptions { mode, max_block_slots, concurrency, index_failed: index_failed_txs };
            let r = repair::repair(&store, &rpc, &opts).await?;
            println!("{}", serde_json::json!({ "gaps_repaired": r.gaps, "blocks": r.blocks,
                "transactions": r.transactions, "accounts_refreshed": r.accounts_refreshed }));
            Ok(())
        }
        Cmd::Reconcile { db, rpc, sample, pubkeys } => {
            let store = store::Store::connect(&db).await?;
            let rpc = rpc::Rpc::new(rpc.rpc_url)?;
            network::verify_database(&store, &rpc).await?;
            let r = reconcile::run_once(&store, &rpc, sample, &pubkeys).await?;
            println!("{}", serde_json::json!({ "checked": r.checked, "matched": r.matched,
                "skipped_newer": r.skipped_newer, "healed_stale": r.healed_stale, "healed_closed": r.healed_closed }));
            Ok(())
        }
        Cmd::Snapshot { db, rpc, types, page_size, limit } => {
            let store = store::Store::connect(&db).await?;
            store.migrate().await?;
            let rpc = rpc::Rpc::new(rpc.rpc_url)?;
            let net = network::verify_database(&store, &rpc).await?;
            tracing::info!(network = net, "snapshot target verified");
            snapshot::run(&store, &rpc, &types, page_size, limit).await
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

async fn run(
    stream: StreamConfig,
    db: DbConfig,
    rpc_cfg: RpcConfig,
    server: ServerConfig,
    maintenance: MaintenanceConfig,
) -> Result<()> {
    let prom = PrometheusBuilder::new()
        .set_buckets(&[0.005, 0.01, 0.025, 0.05, 0.1, 0.25, 0.5, 1.0, 2.5, 5.0, 10.0])?
        .install_recorder()
        .context("installing metrics recorder")?;
    register_metrics();
    let shutdown = shutdown_signal();

    let store = store::Store::connect(&db).await?;
    store.migrate().await.context("running migrations")?;
    let rpc_client = rpc::Rpc::new(rpc_cfg.rpc_url)?;
    let net = network::verify_database(&store, &rpc_client).await?;
    tracing::info!(network = net, "database network verified");
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
    network::spawn_stream_check(rpc_client.clone(), health.clone(), shutdown.clone());

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

    spawn_maintenance(store.clone(), rpc_client.clone(), maintenance, stream.index_failed_txs, shutdown.clone());
    hydrate::spawn(store.clone(), rpc_client.clone(), shutdown.clone());

    let writer = writer::Writer {
        store,
        rpc: rpc_client,
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

/// Periodic chain-hole audit + small-gap repair, and account reconciliation.
fn spawn_maintenance(
    store: store::Store,
    rpc: rpc::Rpc,
    cfg: MaintenanceConfig,
    index_failed: bool,
    shutdown: CancellationToken,
) {
    if cfg.audit_interval_secs > 0 {
        let (store, rpc, sd) = (store.clone(), rpc.clone(), shutdown.clone());
        tokio::spawn(async move {
            let mut tick = tokio::time::interval(Duration::from_secs(cfg.audit_interval_secs));
            tick.tick().await;
            loop {
                tokio::select! { _ = sd.cancelled() => return, _ = tick.tick() => {} }
                let cp = match store.get_state("checkpoint").await {
                    Ok(Some(cp)) => cp,
                    _ => continue,
                };
                // Recent window only; `dlmm-indexer audit` checks the whole history.
                let until = cp.saturating_sub(32);
                if let Err(e) = repair::audit(&store, until.saturating_sub(20_000), until).await {
                    tracing::error!(error = format!("{e:#}"), "gap audit failed");
                }
                let opts = repair::RepairOptions {
                    mode: repair::RepairMode::Auto,
                    max_block_slots: cfg.auto_repair_max_slots,
                    concurrency: 4,
                    index_failed,
                };
                match repair::repair(&store, &rpc, &opts).await {
                    Ok(r) if r.gaps > 0 => tracing::info!(gaps = r.gaps, blocks = r.blocks,
                        transactions = r.transactions, "gaps repaired automatically"),
                    Ok(_) => {}
                    Err(e) => tracing::error!(error = format!("{e:#}"), "gap repair failed"),
                }
            }
        });
    }
    if cfg.stats_interval_secs > 0 {
        let (store, sd) = (store.clone(), shutdown.clone());
        tokio::spawn(async move {
            let mut tick = tokio::time::interval(Duration::from_secs(cfg.stats_interval_secs));
            loop {
                tokio::select! { _ = sd.cancelled() => return, _ = tick.tick() => {} }
                if let Err(e) = metadata::refresh_stats(&store).await {
                    tracing::error!(error = format!("{e:#}"), "stats refresh failed");
                }
            }
        });
    }
    if cfg.metadata_interval_secs > 0 {
        let (store, rpc, sd) = (store.clone(), rpc.clone(), shutdown.clone());
        tokio::spawn(async move {
            let mut tick = tokio::time::interval(Duration::from_secs(cfg.metadata_interval_secs));
            loop {
                tokio::select! { _ = sd.cancelled() => return, _ = tick.tick() => {} }
                // Drain the backlog in batches, then wait for the next tick.
                loop {
                    match metadata::fill_batch(&store, &rpc, 1000).await {
                        Ok(0) => break,
                        Ok(n) => tracing::debug!(checked = n, "token metadata batch"),
                        Err(e) => {
                            tracing::warn!(error = format!("{e:#}"),
                                "token metadata lookup failed (RPC may not support DAS); disabling");
                            return;
                        }
                    }
                }
            }
        });
    }
    if cfg.reconcile_interval_secs > 0 {
        tokio::spawn(async move {
            let mut tick = tokio::time::interval(Duration::from_secs(cfg.reconcile_interval_secs));
            tick.tick().await;
            loop {
                tokio::select! { _ = shutdown.cancelled() => return, _ = tick.tick() => {} }
                match reconcile::run_once(&store, &rpc, cfg.reconcile_sample, &[]).await {
                    Ok(r) => tracing::info!(checked = r.checked, matched = r.matched,
                        healed_stale = r.healed_stale, healed_closed = r.healed_closed, "reconciliation pass"),
                    Err(e) => tracing::error!(error = format!("{e:#}"), "reconciliation failed"),
                }
            }
        });
    }
}

/// Export error/self-healing counters at 0 from the start, so dashboards show 0 rather
/// than "no data" and `increase()` alerts work before the first event.
fn register_metrics() {
    for name in [
        "transactions_total", "transactions_ignored_total", "db_errors_total", "db_rows_written_total",
        "gaps_recorded_total", "gaps_repaired_total", "reconcile_checked_total", "dead_slots_total",
        "token_metadata_checked_total",
    ] {
        metrics::counter!(name).increment(0);
    }
    for kind in ["pair", "owner"] {
        metrics::counter!("hydrations_total", "kind" => kind).increment(0);
    }
    for kind in ["instruction", "event", "account"] {
        metrics::counter!("decode_failures_total", "kind" => kind).increment(0);
    }
    for kind in ["stale", "closed"] {
        metrics::counter!("reconcile_mismatches_total", "kind" => kind).increment(0);
    }
}
