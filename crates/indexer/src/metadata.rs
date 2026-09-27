//! Token metadata (symbol, name, logo) via the DAS `getAssetBatch` RPC method
//! (Helius and most major providers). Values are untrusted third-party strings, so they
//! are length-capped and stripped of control characters before storage.

use anyhow::Result;
use serde_json::{json, Value};

use crate::rpc::Rpc;
use crate::store::Store;

fn clean(s: Option<&str>, max: usize) -> Option<String> {
    let s: String = s?.chars().filter(|c| !c.is_control()).collect::<String>().trim().to_owned();
    (!s.is_empty()).then(|| s.chars().take(max).collect())
}

fn logo(v: &Value) -> Option<String> {
    let url = v["content"]["links"]["image"]
        .as_str()
        .or_else(|| v["content"]["files"][0]["uri"].as_str())?;
    (url.starts_with("https://") && url.len() <= 512).then(|| url.to_owned())
}

/// Look up metadata for up to `batch` mints that haven't been checked yet.
/// Returns how many were checked; errors if the RPC doesn't support DAS.
pub async fn fill_batch(store: &Store, rpc: &Rpc, batch: i64) -> Result<usize> {
    let mints: Vec<String> =
        sqlx::query_scalar("SELECT mint FROM mints WHERE metadata_checked_at IS NULL LIMIT $1")
            .bind(batch)
            .fetch_all(store.pool())
            .await?;
    if mints.is_empty() {
        return Ok(0);
    }
    let assets: Vec<Value> = rpc.call("getAssetBatch", json!({ "ids": mints })).await?;
    let mut tx = store.pool().begin().await?;
    for (mint, a) in mints.iter().zip(assets.iter().chain(std::iter::repeat(&Value::Null))) {
        let md = &a["content"]["metadata"];
        let symbol = clean(md["symbol"].as_str().or(a["token_info"]["symbol"].as_str()), 16);
        let name = clean(md["name"].as_str(), 64);
        sqlx::query(
            "UPDATE mints SET symbol = $2, name = $3, logo_uri = $4, metadata_checked_at = now() WHERE mint = $1",
        )
        .bind(mint)
        .bind(symbol)
        .bind(name)
        .bind(logo(a))
        .execute(&mut *tx)
        .await?;
    }
    tx.commit().await?;
    metrics::counter!("token_metadata_checked_total").increment(mints.len() as u64);
    Ok(mints.len())
}

pub async fn refresh_stats(store: &Store) -> Result<()> {
    let started = std::time::Instant::now();
    sqlx::query("REFRESH MATERIALIZED VIEW CONCURRENTLY pair_stats_24h").execute(store.pool()).await?;
    sqlx::query("REFRESH MATERIALIZED VIEW CONCURRENTLY global_stats_24h").execute(store.pool()).await?;
    // Depends on pair_stats_24h, token balances and metadata: refresh last.
    sqlx::query("REFRESH MATERIALIZED VIEW CONCURRENTLY pair_rank").execute(store.pool()).await?;
    metrics::histogram!("stats_refresh_seconds").record(started.elapsed().as_secs_f64());
    Ok(())
}
