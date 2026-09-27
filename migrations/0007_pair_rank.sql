-- Precomputed ranking/search fields for the pool list, refreshed with the 24h stats.
-- The API picks a page of pool ids from here (index scan), then computes full, live
-- details only for that page — instead of evaluating every pool on every request.
CREATE MATERIALIZED VIEW pair_rank AS
SELECT
    a.pubkey                                        AS lb_pair,
    a.slot,
    a.data->>'token_x_mint'                         AS token_x_mint,
    a.data->>'token_y_mint'                         AS token_y_mint,
    lower(mx.symbol)                                AS symbol_x_lc,
    lower(my.symbol)                                AS symbol_y_lc,
    coalesce(s.trades, 0)                           AS trades_24h,
    CASE WHEN mx.decimals IS NOT NULL AND my.decimals IS NOT NULL THEN
        coalesce(s.volume_y / power(10::numeric, my.decimals)
            + s.volume_x / power(10::numeric, mx.decimals)
              * dlmm_ui_price((a.data->>'active_id')::int, (a.data->>'bin_step')::int, mx.decimals, my.decimals)::numeric, 0)::float8
    END                                             AS volume_24h_y,
    CASE WHEN mx.decimals IS NOT NULL AND my.decimals IS NOT NULL THEN
        (rx.amount / power(10::numeric, mx.decimals)
            * dlmm_ui_price((a.data->>'active_id')::int, (a.data->>'bin_step')::int, mx.decimals, my.decimals)::numeric
         + ry.amount / power(10::numeric, my.decimals))::float8
    END                                             AS tvl_in_y,
    CASE WHEN s.open_bin IS NOT NULL THEN
        power(1 + (a.data->>'bin_step')::float8 / 10000, (a.data->>'active_id')::int - s.open_bin) - 1
    END                                             AS price_change_24h
FROM accounts a
LEFT JOIN mints mx ON mx.mint = a.data->>'token_x_mint'
LEFT JOIN mints my ON my.mint = a.data->>'token_y_mint'
LEFT JOIN token_balances rx ON rx.account = a.data->>'reserve_x'
LEFT JOIN token_balances ry ON ry.account = a.data->>'reserve_y'
LEFT JOIN pair_stats_24h s ON s.lb_pair = a.pubkey
WHERE a.account_type = 'LbPair' AND a.closed_slot IS NULL;

CREATE UNIQUE INDEX pair_rank_pk ON pair_rank (lb_pair);
CREATE INDEX pair_rank_trades_idx ON pair_rank (trades_24h DESC, tvl_in_y DESC NULLS LAST, lb_pair);
CREATE INDEX pair_rank_volume_idx ON pair_rank (volume_24h_y DESC NULLS LAST, lb_pair);
CREATE INDEX pair_rank_tvl_idx ON pair_rank (tvl_in_y DESC NULLS LAST, lb_pair);
CREATE INDEX pair_rank_change_idx ON pair_rank (price_change_24h DESC NULLS LAST, lb_pair);
CREATE INDEX pair_rank_slot_idx ON pair_rank (slot DESC, lb_pair);
CREATE INDEX pair_rank_mint_x_idx ON pair_rank (token_x_mint);
CREATE INDEX pair_rank_mint_y_idx ON pair_rank (token_y_mint);
CREATE INDEX pair_rank_sym_x_idx ON pair_rank (symbol_x_lc text_pattern_ops);
CREATE INDEX pair_rank_sym_y_idx ON pair_rank (symbol_y_lc text_pattern_ops);
