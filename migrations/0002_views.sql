-- Typed, query-friendly views over the raw tables. Everything here is derived, so it
-- can be changed freely or materialized later (e.g. Timescale continuous aggregates).

-- Price of one unit of X in units of Y, in raw token units (not decimal-adjusted):
-- price = (1 + bin_step / 10_000) ^ bin_id
CREATE FUNCTION dlmm_bin_price(bin_id INT, bin_step INT) RETURNS DOUBLE PRECISION
    LANGUAGE sql IMMUTABLE PARALLEL SAFE
    RETURN power(1 + bin_step::double precision / 10000, bin_id);

-- One row per swap. Current program versions emit BOTH `Swap` and `Swap2Evt` for every
-- swap (same amounts), so rows come from `Swap` and are enriched with the matching
-- `Swap2Evt` (next event in the same instruction) when present. Never union the two.
-- fee = mm_fee (LPs) + protocol_fee (+ limit_order_fee); host_fee is part of protocol_fee.
CREATE VIEW swaps AS
SELECT
    s.slot, s.block_time, s.signature, s.tx_index, s.ix_index, s.inner_index,
    s.lb_pair,
    s.wallet                                        AS trader,
    (s.data->>'swap_for_y')::boolean                AS swap_for_y,   -- true: X in, Y out
    (s.data->>'start_bin_id')::int                  AS start_bin_id,
    (s.data->>'end_bin_id')::int                    AS end_bin_id,
    (s.data->>'amount_in')::numeric                 AS amount_in,
    (s.data->>'amount_out')::numeric                AS amount_out,
    (s.data->>'fee')::numeric                       AS fee,
    (s.data->>'protocol_fee')::numeric              AS protocol_fee,
    (s.data->>'host_fee')::numeric                  AS host_fee,
    (s.data->>'fee_bps')::numeric                   AS fee_bps,
    (v2.data->>'mm_fee')::numeric                   AS mm_fee,
    (v2.data->>'limit_order_fee')::numeric          AS limit_order_fee,
    (v2.data->>'amount_left')::numeric              AS amount_left,
    (v2.data->>'fees_on_input')::boolean            AS fees_on_input,
    (v2.data->>'fees_on_token_x')::boolean          AS fees_on_token_x
FROM events s
LEFT JOIN LATERAL (
    SELECT e.data FROM events e
    WHERE e.slot = s.slot AND e.signature = s.signature AND e.ix_index = s.ix_index
      AND e.inner_index > s.inner_index AND e.name = 'Swap2Evt' AND e.lb_pair = s.lb_pair
      AND e.data->'amount_in' = s.data->'amount_in'
    ORDER BY e.inner_index
    LIMIT 1
) v2 ON true
WHERE s.name = 'Swap';

CREATE VIEW liquidity_events AS
SELECT
    e.slot, e.block_time, e.signature, e.ix_index, e.inner_index,
    e.lb_pair, e.position, e.wallet,
    CASE e.name WHEN 'AddLiquidity' THEN 'add' ELSE 'remove' END AS action,
    (e.data->'amounts'->>0)::numeric    AS amount_x,
    (e.data->'amounts'->>1)::numeric    AS amount_y,
    (e.data->>'active_bin_id')::int     AS active_bin_id
FROM events e
WHERE e.name IN ('AddLiquidity', 'RemoveLiquidity');

CREATE VIEW fee_claims AS
SELECT
    e.slot, e.block_time, e.signature, e.lb_pair, e.position, e.wallet AS owner,
    (e.data->>'fee_x')::numeric AS fee_x,
    (e.data->>'fee_y')::numeric AS fee_y
FROM events e
WHERE e.name IN ('ClaimFee', 'ClaimFee2');

CREATE VIEW reward_claims AS
SELECT
    e.slot, e.block_time, e.signature, e.lb_pair, e.position, e.wallet AS owner,
    (e.data->>'reward_index')::int      AS reward_index,
    (e.data->>'total_reward')::numeric  AS total_reward
FROM events e
WHERE e.name IN ('ClaimReward', 'ClaimReward2');

CREATE VIEW lb_pairs AS
SELECT
    a.pubkey                                        AS lb_pair,
    a.slot,
    a.data->>'token_x_mint'                         AS token_x_mint,
    a.data->>'token_y_mint'                         AS token_y_mint,
    a.data->>'reserve_x'                            AS reserve_x,
    a.data->>'reserve_y'                            AS reserve_y,
    (a.data->>'active_id')::int                     AS active_id,
    (a.data->>'bin_step')::int                      AS bin_step,
    dlmm_bin_price((a.data->>'active_id')::int, (a.data->>'bin_step')::int) AS active_price_raw,
    (a.data->'parameters'->>'base_factor')::int     AS base_factor,
    (a.data->'parameters'->>'protocol_share')::int  AS protocol_share,
    (a.data->'v_parameters'->>'volatility_accumulator')::bigint AS volatility_accumulator,
    (a.data->'protocol_fee'->>'amount_x')::numeric  AS protocol_fee_x,
    (a.data->'protocol_fee'->>'amount_y')::numeric  AS protocol_fee_y,
    (a.data->>'status')::int                        AS status,
    (a.data->>'pair_type')::int                     AS pair_type,
    (a.data->>'activation_type')::int               AS activation_type,
    (a.data->>'activation_point')::numeric          AS activation_point,
    a.data->>'creator'                              AS creator,
    a.data->>'oracle'                               AS oracle,
    a.data
FROM accounts a
WHERE a.account_type = 'LbPair' AND a.closed_slot IS NULL;

CREATE VIEW positions AS
SELECT
    a.pubkey                                        AS position,
    a.slot,
    a.lb_pair,
    a.owner_wallet                                  AS owner,
    a.data->>'operator'                             AS operator,
    a.data->>'fee_owner'                            AS fee_owner,
    (a.data->>'lower_bin_id')::int                  AS lower_bin_id,
    (a.data->>'upper_bin_id')::int                  AS upper_bin_id,
    (a.data->>'total_claimed_fee_x_amount')::numeric AS total_claimed_fee_x,
    (a.data->>'total_claimed_fee_y_amount')::numeric AS total_claimed_fee_y,
    (a.data->>'lock_release_point')::numeric        AS lock_release_point,
    to_timestamp((a.data->>'last_updated_at')::bigint) AS last_updated_at,
    a.closed_slot IS NOT NULL                       AS closed,
    a.data
FROM accounts a
WHERE a.account_type = 'PositionV2';

-- One row per bin, exploded from BinArray accounts (70 bins each).
CREATE VIEW bins AS
SELECT
    a.lb_pair,
    (a.data->>'index')::bigint * 70 + (b.ord - 1)   AS bin_id,
    a.slot,
    (b.bin->>'amount_x')::numeric                   AS amount_x,
    (b.bin->>'amount_y')::numeric                   AS amount_y,
    (b.bin->>'price')::numeric / 18446744073709551616 AS price_raw,  -- Q64.64 -> decimal
    (b.bin->>'liquidity_supply')::numeric           AS liquidity_supply,
    (b.bin->>'fee_amount_x_per_token_stored')::numeric AS fee_x_per_token,
    (b.bin->>'fee_amount_y_per_token_stored')::numeric AS fee_y_per_token,
    (b.bin->>'open_order_amount')::numeric          AS open_order_amount
FROM accounts a
CROSS JOIN LATERAL jsonb_array_elements(a.data->'bins') WITH ORDINALITY AS b(bin, ord)
WHERE a.account_type = 'BinArray' AND a.closed_slot IS NULL;

-- Is a slot finalized? (rows are written at `confirmed` and never change afterwards
-- unless the slot dies, in which case they are deleted.)
CREATE VIEW finality AS
SELECT slot AS finalized_slot FROM indexer_state WHERE key = 'finalized';
