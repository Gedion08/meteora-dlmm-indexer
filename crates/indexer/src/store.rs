//! Postgres persistence. All writes are idempotent; a batch and its checkpoint commit
//! in one transaction, so a crash never advances the checkpoint past unwritten data.

use std::time::Duration;

use anyhow::{Context, Result};
use sqlx::postgres::{PgConnectOptions, PgPoolOptions};
use sqlx::{PgPool, Postgres, QueryBuilder, Transaction};

use crate::config::DbConfig;
use crate::model::*;

/// Postgres allows 65535 bind parameters per statement.
const MAX_PARAMS: usize = 65_000;

fn chunk_size(params_per_row: usize) -> usize {
    (MAX_PARAMS / params_per_row).max(1)
}

fn ts(block_time: Option<i64>) -> Option<chrono::DateTime<chrono::Utc>> {
    block_time.and_then(|t| chrono::DateTime::from_timestamp(t, 0))
}

#[derive(Clone)]
pub struct Store {
    pool: PgPool,
}

impl Store {
    pub async fn connect(cfg: &DbConfig) -> Result<Self> {
        let opts: PgConnectOptions = cfg.database_url.parse().context("invalid DATABASE_URL")?;
        let pool = PgPoolOptions::new()
            .max_connections(cfg.db_max_connections)
            .acquire_timeout(Duration::from_secs(30))
            .connect_with(opts.application_name("dlmm-indexer"))
            .await
            .context("connecting to Postgres")?;
        Ok(Self { pool })
    }

    pub async fn migrate(&self) -> Result<()> {
        sqlx::migrate!("../../migrations").run(&self.pool).await?;
        Ok(())
    }

    pub async fn get_state(&self, key: &str) -> Result<Option<u64>> {
        let v: Option<i64> = sqlx::query_scalar("SELECT slot FROM indexer_state WHERE key = $1")
            .bind(key)
            .fetch_optional(&self.pool)
            .await?;
        Ok(v.map(|s| s as u64))
    }

    pub async fn set_state(&self, key: &str, slot: u64) -> Result<()> {
        set_state(&mut *self.pool.acquire().await?, key, slot).await
    }

    pub async fn get_meta(&self, key: &str) -> Result<Option<String>> {
        Ok(sqlx::query_scalar("SELECT value FROM indexer_meta WHERE key = $1")
            .bind(key)
            .fetch_optional(&self.pool)
            .await?)
    }

    pub async fn set_meta(&self, key: &str, value: &str) -> Result<()> {
        sqlx::query(
            "INSERT INTO indexer_meta (key, value) VALUES ($1, $2)
             ON CONFLICT (key) DO UPDATE SET value = EXCLUDED.value, updated_at = now()",
        )
        .bind(key)
        .bind(value)
        .execute(&self.pool)
        .await?;
        Ok(())
    }

    pub async fn record_gap(&self, from: u64, to: u64, reason: &str) -> Result<()> {
        sqlx::query("INSERT INTO gaps (from_slot, to_slot, reason) VALUES ($1, $2, $3)")
            .bind(from as i64)
            .bind(to as i64)
            .bind(reason)
            .execute(&self.pool)
            .await?;
        Ok(())
    }

    /// Write several slot batches and (optionally) advance the checkpoint, atomically.
    /// Listeners on `dlmm_commit` (the API) are notified at commit time with the slots
    /// that received new rows, so they never see a notification for uncommitted data.
    pub async fn write(&self, batches: &[SlotBatch], checkpoint: Option<u64>) -> Result<()> {
        let mut tx = self.pool.begin().await?;
        for b in batches {
            write_batch(&mut tx, b).await?;
        }
        upsert_mints(&mut tx, batches).await?;
        upsert_token_balances(&mut tx, batches).await?;
        let slots: Vec<u64> = batches
            .iter()
            .filter(|b| !b.events.is_empty() || !b.instructions.is_empty() || !b.accounts.is_empty() || !b.closes.is_empty())
            .map(|b| b.slot)
            .collect();
        if !slots.is_empty() {
            sqlx::query("SELECT pg_notify('dlmm_commit', $1)")
                .bind(serde_json::json!({ "slots": slots }).to_string())
                .execute(&mut *tx)
                .await?;
        }
        if let Some(cp) = checkpoint {
            set_state(&mut *tx, "checkpoint", cp).await?;
        }
        tx.commit().await?;
        Ok(())
    }

    pub async fn upsert_accounts(&self, rows: &[AccountRow]) -> Result<()> {
        let mut tx = self.pool.begin().await?;
        upsert_accounts(&mut tx, rows).await?;
        tx.commit().await?;
        Ok(())
    }

    pub async fn insert_failures(&self, rows: &[DecodeFailure]) -> Result<()> {
        let mut tx = self.pool.begin().await?;
        insert_failures(&mut tx, rows).await?;
        tx.commit().await?;
        Ok(())
    }

    /// Remove everything written for a slot that turned out to be on a dead fork.
    /// Returns accounts whose latest state came from that slot (to be re-fetched).
    pub async fn rollback_slot(&self, slot: u64) -> Result<Vec<String>> {
        let s = slot as i64;
        let mut tx = self.pool.begin().await?;
        for table in ["events", "instructions", "transactions", "decode_failures", "slots"] {
            sqlx::query(&format!("DELETE FROM {table} WHERE slot = $1"))
                .bind(s)
                .execute(&mut *tx)
                .await?;
        }
        sqlx::query("UPDATE accounts SET closed_slot = NULL WHERE closed_slot = $1")
            .bind(s)
            .execute(&mut *tx)
            .await?;
        let stale: Vec<String> = sqlx::query_scalar("SELECT pubkey FROM accounts WHERE slot = $1")
            .bind(s)
            .fetch_all(&mut *tx)
            .await?;
        tx.commit().await?;
        Ok(stale)
    }
}

async fn set_state(conn: &mut sqlx::PgConnection, key: &str, slot: u64) -> Result<()> {
    sqlx::query(
        "INSERT INTO indexer_state (key, slot) VALUES ($1, $2)
         ON CONFLICT (key) DO UPDATE
         SET slot = GREATEST(indexer_state.slot, EXCLUDED.slot), updated_at = now()",
    )
    .bind(key)
    .bind(slot as i64)
    .execute(conn)
    .await?;
    Ok(())
}

async fn write_batch(tx: &mut Transaction<'_, Postgres>, b: &SlotBatch) -> Result<()> {
    let block_time = ts(b.meta.as_ref().and_then(|m| m.block_time));

    if let Some(m) = &b.meta {
        sqlx::query(
            "INSERT INTO slots (slot, parent_slot, block_time, block_height, blockhash)
             VALUES ($1, $2, $3, $4, $5)
             ON CONFLICT (slot) DO UPDATE SET
               parent_slot = EXCLUDED.parent_slot, block_time = EXCLUDED.block_time,
               block_height = EXCLUDED.block_height, blockhash = EXCLUDED.blockhash",
        )
        .bind(b.slot as i64)
        .bind(m.parent_slot.map(|s| s as i64))
        .bind(block_time)
        .bind(m.block_height.map(|h| h as i64))
        .bind(&m.blockhash)
        .execute(&mut **tx)
        .await?;
    }

    // Rows written earlier for this slot without block time get it now; later rows
    // without meta in their own batch pick it up from `slots`.
    let block_time = match block_time {
        Some(t) => Some(t),
        None if !b.txs.is_empty() || !b.events.is_empty() || !b.instructions.is_empty() => {
            sqlx::query_scalar("SELECT block_time FROM slots WHERE slot = $1")
                .bind(b.slot as i64)
                .fetch_optional(&mut **tx)
                .await?
                .flatten()
        }
        None => None,
    };
    if b.backfill_block_time && block_time.is_some() {
        for table in ["transactions", "instructions", "events"] {
            sqlx::query(&format!(
                "UPDATE {table} SET block_time = $2 WHERE slot = $1 AND block_time IS NULL"
            ))
            .bind(b.slot as i64)
            .bind(block_time)
            .execute(&mut **tx)
            .await?;
        }
    }

    for chunk in b.txs.chunks(chunk_size(12)) {
        let mut q = QueryBuilder::<Postgres>::new(
            "INSERT INTO transactions (slot, signature, tx_index, block_time, fee_payer, success, err,
               fee, compute_units, account_keys, pre_token_balances, post_token_balances) ",
        );
        q.push_values(chunk, |mut r, t| {
            r.push_bind(t.slot as i64)
                .push_bind(&t.signature)
                .push_bind(t.tx_index as i32)
                .push_bind(block_time)
                .push_bind(&t.fee_payer)
                .push_bind(t.success)
                .push_bind(&t.err)
                .push_bind(t.fee as i64)
                .push_bind(t.compute_units.map(|c| c as i64))
                .push_bind(&t.account_keys)
                .push_bind(&t.pre_token_balances)
                .push_bind(&t.post_token_balances);
        });
        q.push(" ON CONFLICT DO NOTHING");
        q.build().execute(&mut **tx).await?;
    }

    for chunk in b.instructions.chunks(chunk_size(15)) {
        let mut q = QueryBuilder::<Postgres>::new(
            "INSERT INTO instructions (slot, signature, ix_index, inner_index, tx_index, block_time,
               stack_height, invoked_by, name, lb_pair, wallet, accounts, remaining_accounts, args, success) ",
        );
        q.push_values(chunk, |mut r, i| {
            r.push_bind(i.slot as i64)
                .push_bind(&i.signature)
                .push_bind(i.ix_index as i16)
                .push_bind(i.inner_index)
                .push_bind(i.tx_index as i32)
                .push_bind(block_time)
                .push_bind(i.stack_height.map(|h| h as i16))
                .push_bind(&i.invoked_by)
                .push_bind(i.name)
                .push_bind(&i.lb_pair)
                .push_bind(&i.wallet)
                .push_bind(&i.accounts)
                .push_bind(&i.remaining_accounts)
                .push_bind(&i.args)
                .push_bind(i.success);
        });
        q.push(" ON CONFLICT DO NOTHING");
        q.build().execute(&mut **tx).await?;
    }

    for chunk in b.events.chunks(chunk_size(11)) {
        let mut q = QueryBuilder::<Postgres>::new(
            "INSERT INTO events (slot, signature, ix_index, inner_index, tx_index, block_time,
               name, lb_pair, position, wallet, data) ",
        );
        q.push_values(chunk, |mut r, e| {
            r.push_bind(e.slot as i64)
                .push_bind(&e.signature)
                .push_bind(e.ix_index as i16)
                .push_bind(e.inner_index)
                .push_bind(e.tx_index as i32)
                .push_bind(block_time)
                .push_bind(e.name)
                .push_bind(&e.lb_pair)
                .push_bind(&e.position)
                .push_bind(&e.wallet)
                .push_bind(&e.data);
        });
        q.push(" ON CONFLICT DO NOTHING");
        q.build().execute(&mut **tx).await?;
    }

    upsert_accounts(tx, &b.accounts).await?;

    for c in &b.closes {
        sqlx::query("UPDATE accounts SET closed_slot = $2, updated_at = now() WHERE pubkey = $1 AND slot <= $2")
            .bind(&c.pubkey)
            .bind(c.slot as i64)
            .execute(&mut **tx)
            .await?;
    }

    insert_failures(tx, &b.failures).await?;
    Ok(())
}

/// Record token decimals seen in transaction token balances (needed for UI prices).
async fn upsert_mints(tx: &mut Transaction<'_, Postgres>, batches: &[SlotBatch]) -> Result<()> {
    let mut mints: std::collections::BTreeMap<String, (i16, Option<String>)> = Default::default();
    for t in batches.iter().flat_map(|b| &b.txs) {
        for bal in [&t.pre_token_balances, &t.post_token_balances] {
            for e in bal.as_array().into_iter().flatten() {
                if let (Some(m), Some(d)) = (e["mint"].as_str(), e["decimals"].as_u64()) {
                    mints.entry(m.to_owned()).or_insert((d as i16, e["program_id"].as_str().map(str::to_owned)));
                }
            }
        }
    }
    if mints.is_empty() {
        return Ok(());
    }
    let rows: Vec<_> = mints.into_iter().collect();
    for chunk in rows.chunks(chunk_size(3)) {
        let mut q = QueryBuilder::<Postgres>::new("INSERT INTO mints (mint, decimals, token_program) ");
        q.push_values(chunk, |mut r, (m, (d, p))| {
            r.push_bind(m).push_bind(*d).push_bind(p);
        });
        q.push(" ON CONFLICT (mint) DO NOTHING");
        q.build().execute(&mut **tx).await?;
    }
    Ok(())
}

/// Latest post-transaction balance per token account (reserves, user accounts).
async fn upsert_token_balances(tx: &mut Transaction<'_, Postgres>, batches: &[SlotBatch]) -> Result<()> {
    // One row per account (the latest), or Postgres rejects the multi-row upsert.
    let mut latest: std::collections::BTreeMap<String, (u64, u64, String, Option<String>, String)> = Default::default();
    for t in batches.iter().flat_map(|b| &b.txs).filter(|t| t.success) {
        for e in t.post_token_balances.as_array().into_iter().flatten() {
            let (Some(acc), Some(mint), Some(amount)) =
                (e["account"].as_str(), e["mint"].as_str(), e["amount"].as_str())
            else {
                continue;
            };
            let pos = (t.slot, t.tx_index);
            let newer = latest.get(acc).is_none_or(|v| (v.0, v.1) <= pos);
            if newer {
                latest.insert(
                    acc.to_owned(),
                    (t.slot, t.tx_index, mint.to_owned(), e["owner"].as_str().map(str::to_owned), amount.to_owned()),
                );
            }
        }
    }
    let rows: Vec<_> = latest.into_iter().collect();
    for chunk in rows.chunks(chunk_size(6)) {
        let mut q = QueryBuilder::<Postgres>::new(
            "INSERT INTO token_balances AS b (account, mint, owner, amount, slot, tx_index) ",
        );
        q.push_values(chunk, |mut r, (acc, (slot, idx, mint, owner, amount))| {
            r.push_bind(acc)
                .push_bind(mint)
                .push_bind(owner)
                .push_bind(amount)
                .push_unseparated("::numeric")
                .push_bind(*slot as i64)
                .push_bind(*idx as i32);
        });
        q.push(
            " ON CONFLICT (account) DO UPDATE SET mint = EXCLUDED.mint, owner = EXCLUDED.owner,
                amount = EXCLUDED.amount, slot = EXCLUDED.slot, tx_index = EXCLUDED.tx_index, updated_at = now()
              WHERE (EXCLUDED.slot, EXCLUDED.tx_index) >= (b.slot, b.tx_index)",
        );
        q.build().execute(&mut **tx).await?;
    }
    Ok(())
}

pub async fn upsert_snapshot_balances(pool: &sqlx::PgPool, rows: &[(String, String, String, u64, u64)]) -> Result<()> {
    for chunk in rows.chunks(chunk_size(6)) {
        let mut q = QueryBuilder::<Postgres>::new(
            "INSERT INTO token_balances AS b (account, mint, owner, amount, slot, tx_index) ",
        );
        q.push_values(chunk, |mut r, (acc, mint, owner, amount, slot)| {
            r.push_bind(acc)
                .push_bind(mint)
                .push_bind(owner)
                .push_bind(amount.to_string())
                .push_unseparated("::numeric")
                .push_bind(*slot as i64)
                .push_bind(-1i32);
        });
        q.push(
            " ON CONFLICT (account) DO UPDATE SET amount = EXCLUDED.amount, slot = EXCLUDED.slot,
                tx_index = EXCLUDED.tx_index, updated_at = now()
              WHERE (EXCLUDED.slot, EXCLUDED.tx_index) > (b.slot, b.tx_index)",
        );
        q.build().execute(pool).await?;
    }
    Ok(())
}

pub async fn upsert_mint_rows(pool: &sqlx::PgPool, rows: &[(String, i16, String)]) -> Result<()> {
    for chunk in rows.chunks(chunk_size(3)) {
        let mut q = QueryBuilder::<Postgres>::new("INSERT INTO mints (mint, decimals, token_program) ");
        q.push_values(chunk, |mut r, (m, d, p)| {
            r.push_bind(m).push_bind(*d).push_bind(p);
        });
        q.push(" ON CONFLICT (mint) DO UPDATE SET decimals = EXCLUDED.decimals, token_program = EXCLUDED.token_program, updated_at = now()");
        q.build().execute(pool).await?;
    }
    Ok(())
}

async fn upsert_accounts(tx: &mut Transaction<'_, Postgres>, rows: &[AccountRow]) -> Result<()> {
    for chunk in rows.chunks(chunk_size(10)) {
        let mut q = QueryBuilder::<Postgres>::new(
            "INSERT INTO accounts AS a (pubkey, account_type, slot, write_version, lamports, data_len,
               trailing_bytes, lb_pair, owner_wallet, data) ",
        );
        q.push_values(chunk, |mut r, a| {
            r.push_bind(&a.pubkey)
                .push_bind(a.account_type)
                .push_bind(a.slot as i64)
                .push_bind(a.write_version as i64)
                .push_bind(a.lamports as i64)
                .push_bind(a.data_len as i32)
                .push_bind(a.trailing_bytes as i32)
                .push_bind(&a.lb_pair)
                .push_bind(&a.owner_wallet)
                .push_bind(&a.data);
        });
        // Never let older state (e.g. a snapshot page) overwrite newer stream state.
        // A write after a close means the address was re-created, so it reopens.
        q.push(
            " ON CONFLICT (pubkey) DO UPDATE SET
                account_type = EXCLUDED.account_type, slot = EXCLUDED.slot,
                write_version = EXCLUDED.write_version, lamports = EXCLUDED.lamports,
                data_len = EXCLUDED.data_len, trailing_bytes = EXCLUDED.trailing_bytes,
                lb_pair = EXCLUDED.lb_pair, owner_wallet = EXCLUDED.owner_wallet,
                data = EXCLUDED.data, updated_at = now(),
                closed_slot = CASE WHEN a.closed_slot IS NOT NULL AND EXCLUDED.slot > a.closed_slot
                                   THEN NULL ELSE a.closed_slot END
              WHERE (EXCLUDED.slot, EXCLUDED.write_version) > (a.slot, a.write_version)",
        );
        q.build().execute(&mut **tx).await?;
    }
    Ok(())
}

async fn insert_failures(tx: &mut Transaction<'_, Postgres>, rows: &[DecodeFailure]) -> Result<()> {
    for chunk in rows.chunks(chunk_size(8)) {
        let mut q = QueryBuilder::<Postgres>::new(
            "INSERT INTO decode_failures (slot, kind, signature, pubkey, ix_index, inner_index, raw, error) ",
        );
        q.push_values(chunk, |mut r, f| {
            r.push_bind(f.slot as i64)
                .push_bind(f.kind)
                .push_bind(&f.signature)
                .push_bind(&f.pubkey)
                .push_bind(f.ix_index.map(|i| i as i16))
                .push_bind(f.inner_index)
                .push_bind(&f.raw)
                .push_bind(&f.error);
        });
        q.push(" ON CONFLICT DO NOTHING");
        q.build().execute(&mut **tx).await?;
    }
    Ok(())
}

impl Store {
    pub fn pool(&self) -> &PgPool {
        &self.pool
    }
}
