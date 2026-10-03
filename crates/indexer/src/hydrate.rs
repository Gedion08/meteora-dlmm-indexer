//! On-demand hydration. The live stream only sees accounts that change, and the
//! startup snapshot may skip the heavy types (bin arrays, positions), so a quiet pool or a
//! dormant wallet can look empty. The API requests a fill with `NOTIFY dlmm_hydrate`
//! ("pair:<address>" or "owner:<address>"); this worker fetches exactly those accounts
//! with filtered `getProgramAccountsV2` calls and records the result in `hydrations`.

use std::collections::HashMap;
use std::time::{Duration, Instant};

use anyhow::Result;
use dlmm_decoder::Decoder;
use sqlx::postgres::PgListener;
use tokio::sync::mpsc;
use tokio_util::sync::CancellationToken;

use crate::rpc::Rpc;
use crate::snapshot::fetch_program_accounts;
use crate::store::Store;

/// Don't refetch the same pool/wallet more often than this.
const DEDUPE: Duration = Duration::from_secs(600);

#[derive(Debug, Clone, PartialEq, Eq, Hash)]
pub enum Target {
    Pair(String),
    Owner(String),
}

impl Target {
    fn parse(payload: &str) -> Option<Self> {
        let (kind, key) = payload.split_once(':')?;
        let valid = (32..=44).contains(&key.len()) && bs58::decode(key).into_vec().is_ok_and(|b| b.len() == 32);
        match (kind, valid) {
            ("pair", true) => Some(Target::Pair(key.to_owned())),
            ("owner", true) => Some(Target::Owner(key.to_owned())),
            _ => None,
        }
    }

    fn kind_key(&self) -> (&'static str, &str) {
        match self {
            Target::Pair(k) => ("pair", k),
            Target::Owner(k) => ("owner", k),
        }
    }
}

pub fn spawn(store: Store, rpc: Rpc, shutdown: CancellationToken) {
    // Bounded queue: requests beyond it are dropped (the client will simply ask again).
    let (tx, mut rx) = mpsc::channel::<Target>(256);
    let listen_store = store.clone();
    let sd = shutdown.clone();
    tokio::spawn(async move {
        loop {
            let mut listener = match PgListener::connect_with(listen_store.pool()).await {
                Ok(l) => l,
                Err(e) => {
                    tracing::warn!(error = %e, "hydration listener connect failed; retrying");
                    tokio::time::sleep(Duration::from_secs(3)).await;
                    continue;
                }
            };
            if listener.listen("dlmm_hydrate").await.is_err() {
                tokio::time::sleep(Duration::from_secs(3)).await;
                continue;
            }
            loop {
                let n = tokio::select! {
                    _ = sd.cancelled() => return,
                    n = listener.recv() => n,
                };
                match n {
                    Ok(n) => {
                        if let Some(t) = Target::parse(n.payload()) {
                            let _ = tx.try_send(t);
                        }
                    }
                    Err(e) => {
                        tracing::warn!(error = %e, "hydration listener error; reconnecting");
                        break;
                    }
                }
            }
        }
    });

    tokio::spawn(async move {
        let mut recent: HashMap<Target, Instant> = HashMap::new();
        loop {
            let target = tokio::select! {
                _ = shutdown.cancelled() => return,
                t = rx.recv() => match t { Some(t) => t, None => return },
            };
            recent.retain(|_, t| t.elapsed() < DEDUPE);
            if recent.contains_key(&target) {
                continue;
            }
            recent.insert(target.clone(), Instant::now());
            let started = Instant::now();
            match hydrate(&store, &rpc, &target).await {
                Ok(n) => {
                    metrics::counter!("hydrations_total", "kind" => target.kind_key().0).increment(1);
                    tracing::info!(target = ?target, accounts = n, ms = started.elapsed().as_millis() as u64, "hydrated");
                }
                Err(e) => {
                    recent.remove(&target); // allow a retry on the next request
                    tracing::warn!(target = ?target, error = format!("{e:#}"), "hydration failed");
                }
            }
        }
    });
}

/// Fetch every DLMM account belonging to a pool (bin arrays, positions, limit orders) or
/// owned by a wallet (positions, limit orders). Returns accounts stored.
pub async fn hydrate(store: &Store, rpc: &Rpc, target: &Target) -> Result<usize> {
    let dec = Decoder::bundled();
    // (account type, byte offset of the filtered field)
    let queries: &[(&str, usize)] = match target {
        Target::Pair(_) => &[("BinArray", 24), ("PositionV2", 8), ("LimitOrder", 8)],
        Target::Owner(_) => &[("PositionV2", 40), ("LimitOrder", 40)],
    };
    let (kind, key) = target.kind_key();
    let mut total = 0;
    for (ty, offset) in queries {
        let disc = dec.account_discriminator(ty).expect("IDL account type");
        let (rows, failures) = fetch_program_accounts(rpc, &disc, &[(*offset, key)], 1000).await?;
        total += rows.len();
        store.upsert_accounts(&rows).await?;
        if !failures.is_empty() {
            store.insert_failures(&failures).await?;
        }
    }
    sqlx::query(
        "INSERT INTO hydrations (kind, key, accounts) VALUES ($1, $2, $3)
         ON CONFLICT (kind, key) DO UPDATE SET completed_at = now(), accounts = EXCLUDED.accounts",
    )
    .bind(kind)
    .bind(key)
    .bind(total as i32)
    .execute(store.pool())
    .await?;
    // Let live clients know new state landed (the API refetches on its own too).
    sqlx::query("SELECT pg_notify('dlmm_hydrated', $1)")
        .bind(format!("{kind}:{key}"))
        .execute(store.pool())
        .await?;
    Ok(total)
}
