-- Phase 2: support for the REST / WebSocket API.

-- Token mint decimals, learned from transaction token balances and from the snapshot.
CREATE TABLE mints (
    mint           TEXT PRIMARY KEY,
    decimals       SMALLINT NOT NULL,
    token_program  TEXT,
    updated_at     TIMESTAMPTZ NOT NULL DEFAULT now()
);

-- UI price of X in Y: raw bin price adjusted by token decimals.
CREATE FUNCTION dlmm_ui_price(bin_id INT, bin_step INT, decimals_x INT, decimals_y INT)
    RETURNS DOUBLE PRECISION
    LANGUAGE sql IMMUTABLE PARALLEL SAFE
    RETURN dlmm_bin_price(bin_id, bin_step) * power(10::double precision, decimals_x - decimals_y);

-- Hot API paths: latest swaps per pair / per wallet, candles by time.
CREATE INDEX events_swap_pair_idx
    ON events (lb_pair, slot DESC, tx_index DESC, ix_index DESC, inner_index DESC)
    WHERE name = 'Swap';
CREATE INDEX events_swap_pair_time_idx ON events (lb_pair, block_time) WHERE name = 'Swap';
CREATE INDEX events_wallet_order_idx
    ON events (wallet, slot DESC, tx_index DESC, ix_index DESC, inner_index DESC);
CREATE INDEX accounts_lbpair_mints_idx
    ON accounts ((data->>'token_x_mint'), (data->>'token_y_mint'))
    WHERE account_type = 'LbPair';
