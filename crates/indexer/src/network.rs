//! Guards against mixing clusters: a database belongs to exactly one network
//! (identified by genesis hash), and the gRPC stream must be on the same network as RPC.

use std::sync::atomic::Ordering;
use std::sync::Arc;
use std::time::Duration;

use anyhow::{bail, Result};
use tokio_util::sync::CancellationToken;

use crate::health::Health;
use crate::rpc::Rpc;
use crate::store::Store;

pub fn name_of(genesis: &str) -> &'static str {
    match genesis {
        "5eykt4UsFv8P8NJdTREpY1vzqKqZKvdpKuc147dw2N9d" => "mainnet",
        "EtWTRABZaYq6iMfeYKouRu166VU2xqa1wcaWoxPkrZBG" => "devnet",
        "4uhcVJyU9pJkvQyS88uRDiswHXSCkY3zQawwpjk2NsNY" => "testnet",
        _ => "custom",
    }
}

/// Check (or, on first use, record) the database's network against the RPC's.
pub async fn verify_database(store: &Store, rpc: &Rpc) -> Result<&'static str> {
    let genesis: String = rpc.call("getGenesisHash", serde_json::json!([])).await?;
    let network = name_of(&genesis);
    match store.get_meta("genesis_hash").await? {
        Some(existing) if existing != genesis => bail!(
            "database belongs to {} (genesis {existing}) but RPC_URL is {network} (genesis {genesis}); \
             use a separate database per network",
            name_of(&existing)
        ),
        Some(_) => {}
        None => {
            store.set_meta("genesis_hash", &genesis).await?;
            store.set_meta("network", network).await?;
        }
    }
    Ok(network)
}

/// Once the stream reports its first slot, make sure it is the same cluster as RPC
/// (devnet and mainnet slots differ by tens of millions). Stops the indexer if not.
pub fn spawn_stream_check(rpc: Rpc, health: Arc<Health>, shutdown: CancellationToken) {
    tokio::spawn(async move {
        let rpc_slot: u64 = match rpc
            .call("getSlot", serde_json::json!([{ "commitment": "confirmed" }]))
            .await
        {
            Ok(s) => s,
            Err(e) => {
                tracing::warn!(error = %e, "network check skipped: getSlot failed");
                return;
            }
        };
        for _ in 0..600 {
            let tip = health.chain_tip.load(Ordering::Relaxed);
            if tip > 0 {
                if tip.abs_diff(rpc_slot) > 100_000 {
                    tracing::error!(stream_slot = tip, rpc_slot,
                        "gRPC stream and RPC_URL are on different networks; stopping");
                    shutdown.cancel();
                } else {
                    tracing::info!(stream_slot = tip, rpc_slot, "stream and RPC are on the same network");
                }
                return;
            }
            tokio::time::sleep(Duration::from_millis(100)).await;
        }
    });
}
