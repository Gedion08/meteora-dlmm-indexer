-- Phase 4: data the frontend needs.

-- Token metadata (symbol / name / logo), filled by the indexer's metadata job.
ALTER TABLE mints
    ADD COLUMN symbol TEXT,
    ADD COLUMN name TEXT,
    ADD COLUMN logo_uri TEXT,
    ADD COLUMN metadata_checked_at TIMESTAMPTZ;
CREATE INDEX mints_symbol_idx ON mints (lower(symbol));
CREATE INDEX mints_unchecked_idx ON mints (mint) WHERE metadata_checked_at IS NULL;

CREATE INDEX events_swap_time_idx ON events (block_time) WHERE name = 'Swap';

-- Rolling 24h activity per pool. Refreshed by the indexer every STATS_INTERVAL_SECS so
-- list pages never aggregate raw swaps per request.
CREATE MATERIALIZED VIEW pair_stats_24h AS
SELECT
    e.lb_pair,
    count(*)                    AS trades,
    count(DISTINCT e.wallet)    AS traders,
    sum(CASE WHEN (e.data->>'swap_for_y')::bool THEN (e.data->>'amount_in')::numeric
             ELSE (e.data->>'amount_out')::numeric END) AS volume_x,
    sum(CASE WHEN (e.data->>'swap_for_y')::bool THEN (e.data->>'amount_out')::numeric
             ELSE (e.data->>'amount_in')::numeric END)  AS volume_y,
    coalesce(sum(CASE WHEN (e.data->>'swap_for_y')::bool THEN (e.data->>'fee')::numeric END), 0) AS fees_x,
    coalesce(sum(CASE WHEN NOT (e.data->>'swap_for_y')::bool THEN (e.data->>'fee')::numeric END), 0) AS fees_y,
    -- Bin the pool was at before its first swap in the window: the 24h "open".
    (array_agg((e.data->>'start_bin_id')::int ORDER BY e.slot, e.tx_index, e.ix_index, e.inner_index))[1] AS open_bin,
    max(e.block_time)           AS last_trade_at,
    now()                       AS computed_at
FROM events e
WHERE e.name = 'Swap' AND e.block_time > now() - interval '24 hours'
GROUP BY e.lb_pair;
CREATE UNIQUE INDEX pair_stats_24h_pk ON pair_stats_24h (lb_pair);

CREATE MATERIALIZED VIEW global_stats_24h AS
SELECT
    1                           AS id,
    count(*)                    AS trades,
    count(DISTINCT e.wallet)    AS traders,
    count(DISTINCT e.lb_pair)   AS active_pairs,
    now()                       AS computed_at
FROM events e
WHERE e.name = 'Swap' AND e.block_time > now() - interval '24 hours';
CREATE UNIQUE INDEX global_stats_24h_pk ON global_stats_24h (id);
