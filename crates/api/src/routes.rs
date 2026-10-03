//! REST endpoints. Postgres renders the JSON (`json_agg` / `row_to_json`), so handlers
//! only validate input, bind parameters and stream the text back.
//!
//! Conventions: token amounts are decimal **strings** (u64 exceeds JS number precision);
//! `price` is decimal-adjusted Y per X (null until the mints' decimals are known) and
//! `price_raw` is in raw token units; times are unix seconds; lists are newest first and
//! paginate with the opaque `cursor` of the last item (`?cursor=`).

use axum::extract::{Path, Query, State};
use axum::http::header;
use axum::response::{IntoResponse, Response};
use axum::routing::get;
use axum::Router;
use serde::Deserialize;
use sqlx::{Postgres, QueryBuilder};

use crate::error::{bad, ApiError, ApiResult};
use crate::{live, AppState};

pub fn router(state: AppState) -> Router {
    Router::new()
        .route("/v1/health", get(|| async { "ok" }))
        .route("/v1/status", get(status))
        .route("/v1/stats", get(global_stats))
        .route("/v1/hydrate", get(hydrate))
        .route("/v1/resolve/{id}", get(resolve))
        .route("/v1/pairs", get(list_pairs))
        .route("/v1/pairs/{address}", get(get_pair))
        .route("/v1/pairs/{address}/bins", get(pair_bins))
        .route("/v1/pairs/{address}/swaps", get(pair_swaps))
        .route("/v1/pairs/{address}/events", get(pair_events))
        .route("/v1/pairs/{address}/candles", get(pair_candles))
        .route("/v1/wallets/{wallet}/positions", get(wallet_positions))
        .route("/v1/wallets/{wallet}/swaps", get(wallet_swaps))
        .route("/v1/wallets/{wallet}/events", get(wallet_events))
        .route("/v1/positions/{address}", get(get_position))
        .route("/v1/tx/{signature}", get(get_tx))
        .route("/v1/ws", get(live::ws_handler))
        .with_state(state)
}

/// A JSON document produced by Postgres.
struct JsonText(String);

impl IntoResponse for JsonText {
    fn into_response(self) -> Response {
        ([(header::CONTENT_TYPE, "application/json")], self.0).into_response()
    }
}

// ---------- input validation ----------

fn base58(s: &str, min: usize, max: usize, what: &str) -> ApiResult<()> {
    const ALPHABET: &str = "123456789ABCDEFGHJKLMNPQRSTUVWXYZabcdefghijkmnopqrstuvwxyz";
    if (min..=max).contains(&s.len()) && s.chars().all(|c| ALPHABET.contains(c)) {
        Ok(())
    } else {
        Err(bad(format!("invalid {what}")))
    }
}

fn pubkey(s: &str) -> ApiResult<()> {
    base58(s, 32, 44, "address")
}

fn limit(l: Option<i64>, default: i64, max: i64) -> i64 {
    l.unwrap_or(default).clamp(1, max)
}

/// Keyset cursor over (slot, tx_index, ix_index, inner_index), encoded "a.b.c.d".
#[derive(Clone, Copy)]
struct Cursor(i64, i32, i16, i16);

fn parse_cursor(c: &Option<String>) -> ApiResult<Option<Cursor>> {
    let Some(c) = c else { return Ok(None) };
    let p: Vec<&str> = c.split('.').collect();
    let err = || bad("invalid cursor");
    if p.len() != 4 {
        return Err(err());
    }
    Ok(Some(Cursor(
        p[0].parse().map_err(|_| err())?,
        p[1].parse().map_err(|_| err())?,
        p[2].parse().map_err(|_| err())?,
        p[3].parse().map_err(|_| err())?,
    )))
}

#[derive(Deserialize)]
pub struct PageQuery {
    limit: Option<i64>,
    cursor: Option<String>,
    /// Comma-separated event names, e.g. AddLiquidity,RemoveLiquidity
    names: Option<String>,
}

// ---------- shared SQL ----------

const ORDER_COLS: &str = "e.slot, e.tx_index, e.ix_index, e.inner_index";
const CURSOR_COL: &str = "concat_ws('.', e.slot, e.tx_index, e.ix_index, e.inner_index) AS cursor";

const PAIR_JOINS: &str = "
    LEFT JOIN accounts p ON p.pubkey = e.lb_pair
    LEFT JOIN mints mx ON mx.mint = p.data->>'token_x_mint'
    LEFT JOIN mints my ON my.mint = p.data->>'token_y_mint'";

const SWAP_COLS: &str = "
    e.signature, e.slot, e.tx_index, e.ix_index, e.inner_index,
    extract(epoch FROM e.block_time)::bigint AS block_time,
    e.lb_pair, e.wallet AS trader,
    (e.data->>'swap_for_y')::bool AS swap_for_y,
    e.data->>'amount_in' AS amount_in, e.data->>'amount_out' AS amount_out,
    e.data->>'fee' AS fee, e.data->>'protocol_fee' AS protocol_fee, e.data->>'host_fee' AS host_fee,
    (e.data->>'fee_bps')::numeric / 1e7 AS fee_pct,
    (e.data->>'start_bin_id')::int AS start_bin_id, (e.data->>'end_bin_id')::int AS end_bin_id,
    dlmm_bin_price((e.data->>'end_bin_id')::int, (p.data->>'bin_step')::int) AS price_raw,
    CASE WHEN mx.decimals IS NOT NULL AND my.decimals IS NOT NULL THEN
        dlmm_ui_price((e.data->>'end_bin_id')::int, (p.data->>'bin_step')::int, mx.decimals, my.decimals)
    END AS price,
    mx.symbol AS symbol_x, my.symbol AS symbol_y, mx.decimals AS decimals_x, my.decimals AS decimals_y,
    p.data->>'token_x_mint' AS token_x_mint, p.data->>'token_y_mint' AS token_y_mint";

const EVENT_COLS: &str = "
    e.signature, e.slot, e.tx_index, e.ix_index, e.inner_index,
    extract(epoch FROM e.block_time)::bigint AS block_time,
    e.name, e.lb_pair, e.position, e.wallet, e.data,
    mx.symbol AS symbol_x, my.symbol AS symbol_y, mx.decimals AS decimals_x, my.decimals AS decimals_y,
    p.data->>'token_x_mint' AS token_x_mint, p.data->>'token_y_mint' AS token_y_mint";

const PAIR_COLS: &str = "
    a.pubkey AS address, a.slot AS updated_slot,
    a.data->>'token_x_mint' AS token_x_mint, a.data->>'token_y_mint' AS token_y_mint,
    mx.decimals AS decimals_x, my.decimals AS decimals_y,
    mx.symbol AS symbol_x, my.symbol AS symbol_y, mx.name AS name_x, my.name AS name_y,
    mx.logo_uri AS logo_x, my.logo_uri AS logo_y,
    a.data->>'reserve_x' AS reserve_x, a.data->>'reserve_y' AS reserve_y,
    (a.data->>'bin_step')::int AS bin_step, (a.data->>'active_id')::int AS active_id,
    dlmm_bin_price((a.data->>'active_id')::int, (a.data->>'bin_step')::int) AS price_raw,
    CASE WHEN mx.decimals IS NOT NULL AND my.decimals IS NOT NULL THEN
        dlmm_ui_price((a.data->>'active_id')::int, (a.data->>'bin_step')::int, mx.decimals, my.decimals)
    END AS price,
    (a.data->'parameters'->>'base_factor')::int AS base_factor,
    (a.data->'parameters'->>'base_fee_power_factor')::int AS base_fee_power_factor,
    (a.data->'parameters'->>'protocol_share')::int AS protocol_share_bps,
    (a.data->'v_parameters'->>'volatility_accumulator')::bigint AS volatility_accumulator,
    (a.data->>'status')::int AS status, (a.data->>'pair_type')::int AS pair_type,
    (a.data->>'activation_type')::int AS activation_type,
    a.data->>'activation_point' AS activation_point,
    a.data->>'creator' AS creator, a.data->>'oracle' AS oracle,
    rx.amount::text AS reserve_x_amount, ry.amount::text AS reserve_y_amount,
    CASE WHEN mx.decimals IS NOT NULL AND my.decimals IS NOT NULL THEN
        (rx.amount / power(10::numeric, mx.decimals)
            * dlmm_ui_price((a.data->>'active_id')::int, (a.data->>'bin_step')::int, mx.decimals, my.decimals)::numeric
         + ry.amount / power(10::numeric, my.decimals))::float8
    END AS tvl_in_y,
    coalesce(s.trades, 0) AS trades_24h, coalesce(s.traders, 0) AS traders_24h,
    extract(epoch FROM s.last_trade_at)::bigint AS last_trade_at,
    -- Exact: price ratio between two bins is (1 + bin_step/10^4)^(bin difference).
    CASE WHEN s.open_bin IS NOT NULL THEN
        power(1 + (a.data->>'bin_step')::float8 / 10000, (a.data->>'active_id')::int - s.open_bin) - 1
    END AS price_change_24h,
    -- Quote-token value of 24h volume / fees, X side converted at the current price.
    CASE WHEN mx.decimals IS NOT NULL AND my.decimals IS NOT NULL THEN
        coalesce(s.volume_y / power(10::numeric, my.decimals)
            + s.volume_x / power(10::numeric, mx.decimals)
              * dlmm_ui_price((a.data->>'active_id')::int, (a.data->>'bin_step')::int, mx.decimals, my.decimals)::numeric, 0)::float8
    END AS volume_24h_y,
    CASE WHEN mx.decimals IS NOT NULL AND my.decimals IS NOT NULL THEN
        coalesce(s.fees_y / power(10::numeric, my.decimals)
            + s.fees_x / power(10::numeric, mx.decimals)
              * dlmm_ui_price((a.data->>'active_id')::int, (a.data->>'bin_step')::int, mx.decimals, my.decimals)::numeric, 0)::float8
    END AS fees_24h_y,
    extract(epoch FROM s.computed_at)::bigint AS stats_computed_at";

/// Joins that PAIR_COLS reads from, given `accounts a` for the pool.
const PAIR_JOINS_A: &str = "
    LEFT JOIN mints mx ON mx.mint = a.data->>'token_x_mint'
    LEFT JOIN mints my ON my.mint = a.data->>'token_y_mint'
    LEFT JOIN token_balances rx ON rx.account = a.data->>'reserve_x'
    LEFT JOIN token_balances ry ON ry.account = a.data->>'reserve_y'
    LEFT JOIN pair_stats_24h s ON s.lb_pair = a.pubkey";

const PAIR_FROM: &str = "
    FROM accounts a
    LEFT JOIN mints mx ON mx.mint = a.data->>'token_x_mint'
    LEFT JOIN mints my ON my.mint = a.data->>'token_y_mint'
    LEFT JOIN token_balances rx ON rx.account = a.data->>'reserve_x'
    LEFT JOIN token_balances ry ON ry.account = a.data->>'reserve_y'
    LEFT JOIN pair_stats_24h s ON s.lb_pair = a.pubkey
    WHERE a.account_type = 'LbPair' AND a.closed_slot IS NULL";

/// Newest-first keyset page over `events e`, rendered as a JSON array.
fn events_page<'a>(
    cols: &str,
    joins: &str,
    filter: impl FnOnce(&mut QueryBuilder<'a, Postgres>),
    cursor: Option<Cursor>,
    limit: i64,
) -> QueryBuilder<'a, Postgres> {
    let mut q = QueryBuilder::new(format!(
        "SELECT coalesce(json_agg(t ORDER BY t.slot DESC, t.tx_index DESC, t.ix_index DESC, t.inner_index DESC), '[]')::text
         FROM (SELECT {cols}, {CURSOR_COL} FROM events e {joins} WHERE "
    ));
    filter(&mut q);
    if let Some(Cursor(s, t, i, n)) = cursor {
        q.push(format!(" AND ({ORDER_COLS}) < ("))
            .push_bind(s)
            .push(", ")
            .push_bind(t)
            .push(", ")
            .push_bind(i)
            .push(", ")
            .push_bind(n)
            .push(")");
    }
    q.push(format!(
        " ORDER BY e.slot DESC, e.tx_index DESC, e.ix_index DESC, e.inner_index DESC LIMIT {limit}) t"
    ));
    q
}

fn names_filter(names: &Option<String>) -> ApiResult<Option<Vec<String>>> {
    let Some(n) = names else { return Ok(None) };
    let list: Vec<String> = n.split(',').map(|s| s.trim().to_owned()).filter(|s| !s.is_empty()).collect();
    if list.len() > 30 || list.iter().any(|s| !s.chars().all(|c| c.is_ascii_alphanumeric())) {
        return Err(bad("invalid names"));
    }
    Ok(Some(list))
}

async fn fetch_text(pool: &sqlx::PgPool, mut q: QueryBuilder<'_, Postgres>) -> ApiResult<JsonText> {
    let text: String = q.build_query_scalar().fetch_one(pool).await?;
    Ok(JsonText(text))
}

// ---------- handlers ----------

async fn status(State(s): State<AppState>) -> ApiResult<JsonText> {
    let text: String = sqlx::query_scalar(
        "SELECT json_build_object(
            'checkpoint_slot', (SELECT slot FROM indexer_state WHERE key = 'checkpoint'),
            'checkpoint_updated_at', (SELECT extract(epoch FROM updated_at)::bigint FROM indexer_state WHERE key = 'checkpoint'),
            'finalized_slot', (SELECT slot FROM indexer_state WHERE key = 'finalized'),
            'latest_block_time', (SELECT extract(epoch FROM max(block_time))::bigint FROM slots
                                  WHERE slot > (SELECT max(slot) - 500 FROM slots)),
            'seconds_behind', (SELECT extract(epoch FROM now() - max(block_time))::float8 FROM slots
                               WHERE slot > (SELECT max(slot) - 500 FROM slots)),
            'decode_failures_24h', (SELECT count(*) FROM decode_failures WHERE created_at > now() - interval '1 day'),
            'open_gaps', (SELECT count(*) FROM gaps WHERE repaired_at IS NULL),
            -- Missing ranges still being recovered, with their wall-clock window.
            'gaps', (SELECT coalesce(json_agg(json_build_object(
                    'from_slot', g.from_slot, 'to_slot', g.to_slot,
                    'from_time', (SELECT extract(epoch FROM block_time)::bigint FROM slots
                                  WHERE slot < g.from_slot AND block_time IS NOT NULL ORDER BY slot DESC LIMIT 1),
                    'to_time', (SELECT extract(epoch FROM block_time)::bigint FROM slots
                                WHERE slot > g.to_slot AND block_time IS NOT NULL ORDER BY slot LIMIT 1),
                    'progress', CASE WHEN g.progress_slot IS NULL THEN 0
                                ELSE round((g.to_slot - g.progress_slot + 1)::numeric / (g.to_slot - g.from_slot + 1), 4) END)
                    ORDER BY g.from_slot), '[]')
                FROM gaps g WHERE g.repaired_at IS NULL),
            'live_clients', $1::int
         )::text",
    )
    .bind(s.live.clients() as i32)
    .fetch_one(&s.pool)
    .await?;
    Ok(JsonText(text))
}

#[derive(Deserialize)]
pub struct PairsQuery {
    /// Only pairs containing this mint.
    mint: Option<String>,
    /// Search: a pool or mint address, a symbol prefix ("SOL"), or a pair ("SOL/USDC").
    q: Option<String>,
    /// `trades` (24h, default), `volume`, `tvl` (both in the quote token), `change` or `recent`.
    sort: Option<String>,
    limit: Option<i64>,
    offset: Option<i64>,
}

async fn list_pairs(State(s): State<AppState>, Query(p): Query<PairsQuery>) -> ApiResult<JsonText> {
    if let Some(m) = &p.mint {
        pubkey(m)?;
    }
    let order = match p.sort.as_deref().unwrap_or("trades") {
        "trades" => "trades_24h DESC, tvl_in_y DESC NULLS LAST, lb_pair",
        "volume" => "volume_24h_y DESC NULLS LAST, lb_pair",
        "tvl" => "tvl_in_y DESC NULLS LAST, lb_pair",
        "change" => "price_change_24h DESC NULLS LAST, lb_pair",
        "recent" => "slot DESC, lb_pair",
        _ => return Err(bad("sort must be trades, volume, tvl, change or recent")),
    };
    // 1) Pick the page from the precomputed ranking (index scan, cheap).
    let mut q = QueryBuilder::new("WITH page AS (SELECT lb_pair FROM pair_rank r WHERE true");
    if let Some(m) = p.mint {
        q.push(" AND (r.token_x_mint = ")
            .push_bind(m.clone())
            .push(" OR r.token_y_mint = ")
            .push_bind(m)
            .push(")");
    }
    if let Some(term) = p.q.as_deref().map(str::trim).filter(|t| !t.is_empty()) {
        if term.len() > 64 {
            return Err(bad("search term too long"));
        }
        if pubkey(term).is_ok() {
            q.push(" AND (r.lb_pair = ")
                .push_bind(term.to_owned())
                .push(" OR r.token_x_mint = ")
                .push_bind(term.to_owned())
                .push(" OR r.token_y_mint = ")
                .push_bind(term.to_owned())
                .push(")");
        } else {
            // Symbol prefixes; "A/B" (or "A-B", "A B") matches the pair in either order.
            let parts: Vec<String> = term
                .split(['/', '-', ' '])
                .filter(|s| !s.is_empty())
                .map(|s| format!("{}%", s.to_lowercase().replace(['%', '_', '\\'], "")))
                .collect();
            match parts.as_slice() {
                [one] => {
                    q.push(" AND (r.symbol_x_lc LIKE ")
                        .push_bind(one.clone())
                        .push(" OR r.symbol_y_lc LIKE ")
                        .push_bind(one.clone())
                        .push(")");
                }
                [a, b, ..] => {
                    q.push(" AND ((r.symbol_x_lc LIKE ")
                        .push_bind(a.clone())
                        .push(" AND r.symbol_y_lc LIKE ")
                        .push_bind(b.clone())
                        .push(") OR (r.symbol_x_lc LIKE ")
                        .push_bind(b.clone())
                        .push(" AND r.symbol_y_lc LIKE ")
                        .push_bind(a.clone())
                        .push("))");
                }
                [] => {}
            }
        }
    }
    // 2) Full, live details for just that page, in page order.
    q.push(format!(
        " ORDER BY {order} LIMIT {} OFFSET {}),
         ranked AS (SELECT lb_pair, ord FROM unnest(ARRAY(SELECT lb_pair FROM page)) WITH ORDINALITY AS u(lb_pair, ord))
         SELECT coalesce(json_agg(t ORDER BY t.ord), '[]')::text FROM (
             SELECT {PAIR_COLS}, ranked.ord
             FROM ranked JOIN accounts a ON a.pubkey = ranked.lb_pair AND a.closed_slot IS NULL
             {PAIR_JOINS_A}) t",
        limit(p.limit, 100, 500),
        p.offset.unwrap_or(0).clamp(0, 1_000_000)
    ));
    fetch_text(&s.pool, q).await
}

async fn get_pair(State(s): State<AppState>, Path(address): Path<String>) -> ApiResult<JsonText> {
    pubkey(&address)?;
    let text: Option<String> = sqlx::query_scalar(&format!(
        "SELECT (to_jsonb(t) || jsonb_build_object('stats_24h', (
            SELECT jsonb_build_object(
                'trades', count(*),
                'volume_x', coalesce(sum(CASE WHEN (e.data->>'swap_for_y')::bool THEN (e.data->>'amount_in')::numeric ELSE (e.data->>'amount_out')::numeric END), 0)::text,
                'volume_y', coalesce(sum(CASE WHEN (e.data->>'swap_for_y')::bool THEN (e.data->>'amount_out')::numeric ELSE (e.data->>'amount_in')::numeric END), 0)::text,
                'fees_x', coalesce(sum(CASE WHEN (e.data->>'swap_for_y')::bool THEN (e.data->>'fee')::numeric END), 0)::text,
                'fees_y', coalesce(sum(CASE WHEN NOT (e.data->>'swap_for_y')::bool THEN (e.data->>'fee')::numeric END), 0)::text,
                'unique_traders', count(DISTINCT e.wallet))
            FROM events e
            WHERE e.name = 'Swap' AND e.lb_pair = t.address AND e.block_time > now() - interval '24 hours')))::text
         FROM (SELECT {PAIR_COLS} {PAIR_FROM} AND a.pubkey = $1) t"
    ))
    .bind(&address)
    .fetch_optional(&s.pool)
    .await?;
    text.map(JsonText).ok_or(ApiError::NotFound)
}

#[derive(Deserialize)]
pub struct BinsQuery {
    /// Bins on each side of the active bin (default 35, max 350).
    radius: Option<i32>,
    from_bin: Option<i32>,
    to_bin: Option<i32>,
    /// Include bins without liquidity (default false).
    include_empty: Option<bool>,
}

async fn pair_bins(
    State(s): State<AppState>,
    Path(address): Path<String>,
    Query(b): Query<BinsQuery>,
) -> ApiResult<JsonText> {
    pubkey(&address)?;
    let active: Option<i32> = sqlx::query_scalar(
        "SELECT (data->>'active_id')::int FROM accounts WHERE pubkey = $1 AND account_type = 'LbPair'",
    )
    .bind(&address)
    .fetch_optional(&s.pool)
    .await?;
    let active = active.ok_or(ApiError::NotFound)?;
    let (from, to) = match (b.from_bin, b.to_bin) {
        (Some(f), Some(t)) if t >= f && t - f <= 1400 => (f, t),
        (Some(_), Some(_)) => return Err(bad("from_bin..to_bin must span at most 1400 bins")),
        _ => {
            let r = b.radius.unwrap_or(35).clamp(0, 350);
            (active - r, active + r)
        }
    };
    let text: String = sqlx::query_scalar(
        "SELECT json_build_object('active_id', $4::int, 'bins', coalesce(json_agg(t ORDER BY t.bin_id), '[]'))::text FROM (
            SELECT b.bin_id, b.amount_x::text AS amount_x, b.amount_y::text AS amount_y,
                   b.liquidity_supply::text AS liquidity_supply,
                   dlmm_bin_price(b.bin_id::int, (p.data->>'bin_step')::int) AS price_raw,
                   CASE WHEN mx.decimals IS NOT NULL AND my.decimals IS NOT NULL THEN
                        dlmm_ui_price(b.bin_id::int, (p.data->>'bin_step')::int, mx.decimals, my.decimals) END AS price
            FROM bins b
            JOIN accounts p ON p.pubkey = b.lb_pair
            LEFT JOIN mints mx ON mx.mint = p.data->>'token_x_mint'
            LEFT JOIN mints my ON my.mint = p.data->>'token_y_mint'
            WHERE b.lb_pair = $1 AND b.bin_id BETWEEN $2 AND $3
              AND ($5 OR b.amount_x > 0 OR b.amount_y > 0)) t",
    )
    .bind(&address)
    .bind(from as i64)
    .bind(to as i64)
    .bind(active)
    .bind(b.include_empty.unwrap_or(false))
    .fetch_one(&s.pool)
    .await?;
    Ok(JsonText(text))
}

async fn pair_swaps(
    State(s): State<AppState>,
    Path(address): Path<String>,
    Query(p): Query<PageQuery>,
) -> ApiResult<JsonText> {
    pubkey(&address)?;
    let q = events_page(
        SWAP_COLS,
        PAIR_JOINS,
        |q| {
            q.push("e.name = 'Swap' AND e.lb_pair = ").push_bind(address);
        },
        parse_cursor(&p.cursor)?,
        limit(p.limit, 50, 500),
    );
    fetch_text(&s.pool, q).await
}

async fn wallet_swaps(
    State(s): State<AppState>,
    Path(wallet): Path<String>,
    Query(p): Query<PageQuery>,
) -> ApiResult<JsonText> {
    pubkey(&wallet)?;
    let q = events_page(
        SWAP_COLS,
        PAIR_JOINS,
        |q| {
            q.push("e.name = 'Swap' AND e.wallet = ").push_bind(wallet);
        },
        parse_cursor(&p.cursor)?,
        limit(p.limit, 50, 500),
    );
    fetch_text(&s.pool, q).await
}

async fn pair_events(
    State(s): State<AppState>,
    Path(address): Path<String>,
    Query(p): Query<PageQuery>,
) -> ApiResult<JsonText> {
    pubkey(&address)?;
    let names = names_filter(&p.names)?;
    let q = events_page(
        EVENT_COLS,
        PAIR_JOINS,
        |q| {
            q.push("e.lb_pair = ").push_bind(address);
            if let Some(n) = names {
                q.push(" AND e.name = ANY(").push_bind(n).push(")");
            }
        },
        parse_cursor(&p.cursor)?,
        limit(p.limit, 50, 500),
    );
    fetch_text(&s.pool, q).await
}

async fn wallet_events(
    State(s): State<AppState>,
    Path(wallet): Path<String>,
    Query(p): Query<PageQuery>,
) -> ApiResult<JsonText> {
    pubkey(&wallet)?;
    let names = names_filter(&p.names)?;
    let q = events_page(
        EVENT_COLS,
        PAIR_JOINS,
        |q| {
            q.push("e.wallet = ").push_bind(wallet);
            if let Some(n) = names {
                q.push(" AND e.name = ANY(").push_bind(n).push(")");
            }
        },
        parse_cursor(&p.cursor)?,
        limit(p.limit, 50, 500),
    );
    fetch_text(&s.pool, q).await
}

#[derive(Deserialize)]
pub struct CandleQuery {
    /// 1m, 5m, 15m, 1h, 4h, 1d
    interval: Option<String>,
    /// Unix seconds.
    from: Option<i64>,
    to: Option<i64>,
}

async fn pair_candles(
    State(s): State<AppState>,
    Path(address): Path<String>,
    Query(c): Query<CandleQuery>,
) -> ApiResult<JsonText> {
    pubkey(&address)?;
    let (interval, secs) = match c.interval.as_deref().unwrap_or("1m") {
        "1m" => ("1 minute", 60),
        "5m" => ("5 minutes", 300),
        "15m" => ("15 minutes", 900),
        "1h" => ("1 hour", 3600),
        "4h" => ("4 hours", 14400),
        "1d" => ("1 day", 86400),
        _ => return Err(bad("interval must be one of 1m, 5m, 15m, 1h, 4h, 1d")),
    };
    let now = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|d| d.as_secs() as i64)
        .unwrap_or_default();
    let to = c.to.unwrap_or(now);
    let from = c.from.unwrap_or(to - secs * 300);
    if from >= to || (to - from) / secs > 1500 {
        return Err(bad("from/to must be ordered and span at most 1500 candles"));
    }
    // Candle price is the bin price where each swap ended (exact post-swap price).
    let text: String = sqlx::query_scalar(
        "WITH p AS (
            SELECT (a.data->>'bin_step')::int AS bs, mx.decimals AS dx, my.decimals AS dy
            FROM accounts a
            LEFT JOIN mints mx ON mx.mint = a.data->>'token_x_mint'
            LEFT JOIN mints my ON my.mint = a.data->>'token_y_mint'
            WHERE a.pubkey = $1),
         s AS (
            SELECT date_bin($2::interval, e.block_time, TIMESTAMPTZ 'epoch') AS bucket,
                   e.slot, e.tx_index, e.ix_index, e.inner_index,
                   dlmm_ui_price((e.data->>'end_bin_id')::int, p.bs, coalesce(p.dx, 0), coalesce(p.dy, 0)) AS price,
                   CASE WHEN (e.data->>'swap_for_y')::bool THEN (e.data->>'amount_in')::numeric ELSE (e.data->>'amount_out')::numeric END AS vx,
                   CASE WHEN (e.data->>'swap_for_y')::bool THEN (e.data->>'amount_out')::numeric ELSE (e.data->>'amount_in')::numeric END AS vy
            FROM events e, p
            WHERE e.name = 'Swap' AND e.lb_pair = $1
              AND e.block_time >= to_timestamp($3) AND e.block_time < to_timestamp($4))
         SELECT json_build_object(
            'decimals_adjusted', (SELECT dx IS NOT NULL AND dy IS NOT NULL FROM p),
            'candles', coalesce(json_agg(c ORDER BY c.time), '[]'))::text
         FROM (
            SELECT extract(epoch FROM bucket)::bigint AS time,
                   (array_agg(price ORDER BY slot, tx_index, ix_index, inner_index))[1] AS open,
                   max(price) AS high, min(price) AS low,
                   (array_agg(price ORDER BY slot DESC, tx_index DESC, ix_index DESC, inner_index DESC))[1] AS close,
                   sum(vx)::text AS volume_x, sum(vy)::text AS volume_y, count(*) AS trades
            FROM s GROUP BY bucket) c",
    )
    .bind(&address)
    .bind(interval)
    .bind(from as f64)
    .bind(to as f64)
    .fetch_one(&s.pool)
    .await?;
    Ok(JsonText(text))
}

/// Position token amounts: share of each bin's reserves = shares / liquidity_supply.
/// (Positions resized beyond 70 bins keep extra shares in `trailing_bytes`, which are
/// not decoded yet; `extended` flags them.)
const POSITION_SELECT: &str = "
    SELECT a.pubkey AS address, a.lb_pair, a.owner_wallet AS owner, a.slot AS updated_slot,
           a.closed_slot IS NOT NULL AS closed,
           (a.data->>'lower_bin_id')::int AS lower_bin_id, (a.data->>'upper_bin_id')::int AS upper_bin_id,
           a.data->>'operator' AS operator, a.data->>'fee_owner' AS fee_owner,
           a.data->>'total_claimed_fee_x_amount' AS total_claimed_fee_x,
           a.data->>'total_claimed_fee_y_amount' AS total_claimed_fee_y,
           a.data->>'lock_release_point' AS lock_release_point,
           (a.data->>'last_updated_at')::bigint AS last_updated_at,
           a.trailing_bytes > 0 AS extended,
           p.data->>'token_x_mint' AS token_x_mint, p.data->>'token_y_mint' AS token_y_mint,
           (p.data->>'active_id')::int AS active_id, (p.data->>'bin_step')::int AS bin_step,
           pmx.symbol AS symbol_x, pmy.symbol AS symbol_y, pmx.decimals AS decimals_x, pmy.decimals AS decimals_y,
           CASE WHEN pmx.decimals IS NOT NULL AND pmy.decimals IS NOT NULL THEN
               dlmm_ui_price((p.data->>'active_id')::int, (p.data->>'bin_step')::int, pmx.decimals, pmy.decimals)
           END AS price,
           amt.amount_x, amt.amount_y
    FROM accounts a
    LEFT JOIN accounts p ON p.pubkey = a.lb_pair
    LEFT JOIN mints pmx ON pmx.mint = p.data->>'token_x_mint'
    LEFT JOIN mints pmy ON pmy.mint = p.data->>'token_y_mint'
    LEFT JOIN LATERAL (
        SELECT floor(sum(CASE WHEN b.liquidity_supply > 0
                    THEN b.amount_x * (a.data->'liquidity_shares'->>(b.bin_id - (a.data->>'lower_bin_id')::int)::int)::numeric / b.liquidity_supply
                    ELSE 0 END))::text AS amount_x,
               floor(sum(CASE WHEN b.liquidity_supply > 0
                    THEN b.amount_y * (a.data->'liquidity_shares'->>(b.bin_id - (a.data->>'lower_bin_id')::int)::int)::numeric / b.liquidity_supply
                    ELSE 0 END))::text AS amount_y
        FROM bins b
        WHERE b.lb_pair = a.lb_pair
          AND b.bin_id BETWEEN (a.data->>'lower_bin_id')::int AND (a.data->>'upper_bin_id')::int
    ) amt ON a.closed_slot IS NULL
    WHERE a.account_type = 'PositionV2'";

#[derive(Deserialize)]
pub struct PositionsQuery {
    include_closed: Option<bool>,
}

async fn wallet_positions(
    State(s): State<AppState>,
    Path(wallet): Path<String>,
    Query(p): Query<PositionsQuery>,
) -> ApiResult<JsonText> {
    pubkey(&wallet)?;
    let text: String = sqlx::query_scalar(&format!(
        "SELECT coalesce(json_agg(t ORDER BY t.updated_slot DESC), '[]')::text FROM (
            {POSITION_SELECT} AND a.owner_wallet = $1 AND ($2 OR a.closed_slot IS NULL)
            ORDER BY a.slot DESC LIMIT 200) t"
    ))
    .bind(&wallet)
    .bind(p.include_closed.unwrap_or(false))
    .fetch_one(&s.pool)
    .await?;
    Ok(JsonText(text))
}

async fn get_position(State(s): State<AppState>, Path(address): Path<String>) -> ApiResult<JsonText> {
    pubkey(&address)?;
    let text: Option<String> = sqlx::query_scalar(&format!(
        "SELECT (to_jsonb(t) || jsonb_build_object('events', (
            SELECT coalesce(jsonb_agg(ev ORDER BY ev.slot DESC, ev.inner_index DESC), '[]') FROM (
                SELECT e.signature, e.slot, e.inner_index, extract(epoch FROM e.block_time)::bigint AS block_time,
                       e.name, e.data
                FROM events e WHERE e.position = t.address ORDER BY e.slot DESC LIMIT 200) ev)))::text
         FROM ({POSITION_SELECT} AND a.pubkey = $1) t"
    ))
    .bind(&address)
    .fetch_optional(&s.pool)
    .await?;
    text.map(JsonText).ok_or(ApiError::NotFound)
}

async fn get_tx(State(s): State<AppState>, Path(signature): Path<String>) -> ApiResult<JsonText> {
    base58(&signature, 64, 90, "signature")?;
    let text: Option<String> = sqlx::query_scalar(
        "SELECT json_build_object(
            'signature', t.signature, 'slot', t.slot, 'block_time', extract(epoch FROM t.block_time)::bigint,
            'fee_payer', t.fee_payer, 'success', t.success, 'err', t.err, 'fee', t.fee,
            'compute_units', t.compute_units,
            'instructions', (SELECT coalesce(json_agg(i ORDER BY i.ix_index, i.inner_index), '[]') FROM (
                SELECT ix_index, inner_index, name, invoked_by, lb_pair, wallet, accounts, remaining_accounts, args
                FROM instructions WHERE slot = t.slot AND signature = t.signature) i),
            'events', (SELECT coalesce(json_agg(e ORDER BY e.ix_index, e.inner_index), '[]') FROM (
                SELECT ev.ix_index, ev.inner_index, ev.name, ev.lb_pair, ev.position, ev.wallet, ev.data,
                       mx.symbol AS symbol_x, my.symbol AS symbol_y, mx.decimals AS decimals_x, my.decimals AS decimals_y
                FROM events ev
                LEFT JOIN accounts p ON p.pubkey = ev.lb_pair
                LEFT JOIN mints mx ON mx.mint = p.data->>'token_x_mint'
                LEFT JOIN mints my ON my.mint = p.data->>'token_y_mint'
                WHERE ev.slot = t.slot AND ev.signature = t.signature) e),
            'token_balances', json_build_object('pre', t.pre_token_balances, 'post', t.post_token_balances)
         )::text
         FROM transactions t WHERE t.signature = $1 LIMIT 1",
    )
    .bind(&signature)
    .fetch_optional(&s.pool)
    .await?;
    text.map(JsonText).ok_or(ApiError::NotFound)
}

async fn global_stats(State(s): State<AppState>) -> ApiResult<JsonText> {
    let text: String = sqlx::query_scalar(
        "SELECT json_build_object(
            'network', (SELECT value FROM indexer_meta WHERE key = 'network'),
            'pairs', (SELECT count(*) FROM accounts WHERE account_type = 'LbPair' AND closed_slot IS NULL),
            'positions', (SELECT count(*) FROM accounts WHERE account_type = 'PositionV2' AND closed_slot IS NULL),
            'trades_24h', coalesce(g.trades, 0),
            'traders_24h', coalesce(g.traders, 0),
            'active_pairs_24h', coalesce(g.active_pairs, 0),
            'computed_at', extract(epoch FROM g.computed_at)::bigint
         )::text
         FROM (SELECT 1) one LEFT JOIN global_stats_24h g ON true",
    )
    .fetch_one(&s.pool)
    .await?;
    Ok(JsonText(text))
}

#[derive(Deserialize)]
pub struct HydrateQuery {
    pair: Option<String>,
    owner: Option<String>,
}

/// Ensure a pool's bin arrays/positions (or a wallet's positions) have been loaded from the
/// chain. Asks the indexer via NOTIFY when they never were (or are over an hour old) and
/// reports `pending` until it finishes. The API itself stays read-only.
async fn hydrate(State(s): State<AppState>, Query(h): Query<HydrateQuery>) -> ApiResult<JsonText> {
    let (kind, key) = match (h.pair, h.owner) {
        (Some(p), None) => ("pair", p),
        (None, Some(o)) => ("owner", o),
        _ => return Err(bad("pass exactly one of pair or owner")),
    };
    pubkey(&key)?;
    let row: Option<(i64, i32, bool)> = sqlx::query_as(
        "SELECT extract(epoch FROM completed_at)::bigint, accounts, completed_at < now() - interval '1 hour'
         FROM hydrations WHERE kind = $1 AND key = $2",
    )
    .bind(kind)
    .bind(&key)
    .fetch_optional(&s.pool)
    .await?;
    if row.as_ref().is_none_or(|r| r.2) {
        sqlx::query("SELECT pg_notify('dlmm_hydrate', $1)")
            .bind(format!("{kind}:{key}"))
            .execute(&s.pool)
            .await?;
    }
    let body = match row {
        Some((at, n, _)) => serde_json::json!({ "state": "done", "completed_at": at, "accounts": n }),
        None => serde_json::json!({ "state": "pending" }),
    };
    Ok(JsonText(body.to_string()))
}

/// What is this address? One lookup for the search box: a transaction signature, a pool,
/// a position, a token mint (pools that trade it), or otherwise a wallet.
async fn resolve(State(s): State<AppState>, Path(id): Path<String>) -> ApiResult<JsonText> {
    let kind: &str = if base58(&id, 64, 90, "signature").is_ok() {
        "tx"
    } else {
        pubkey(&id)?;
        let t: Option<String> = sqlx::query_scalar(
            "SELECT CASE account_type WHEN 'LbPair' THEN 'pool' WHEN 'PositionV2' THEN 'position' END
             FROM accounts WHERE pubkey = $1 AND account_type IN ('LbPair', 'PositionV2')",
        )
        .bind(&id)
        .fetch_optional(&s.pool)
        .await?
        .flatten();
        match t {
            Some(k) if k == "pool" => "pool",
            Some(_) => "position",
            None => {
                let is_mint: bool = sqlx::query_scalar("SELECT EXISTS (SELECT 1 FROM mints WHERE mint = $1)")
                    .bind(&id)
                    .fetch_one(&s.pool)
                    .await?;
                if is_mint { "token" } else { "wallet" }
            }
        }
    };
    Ok(JsonText(serde_json::json!({ "kind": kind, "id": id }).to_string()))
}
