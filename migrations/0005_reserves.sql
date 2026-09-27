-- Latest known balance of every token account touched by DLMM transactions
-- (from post-transaction token balances), plus pool reserves from the snapshot.
CREATE TABLE token_balances (
    account     TEXT PRIMARY KEY,
    mint        TEXT NOT NULL,
    owner       TEXT,
    amount      NUMERIC(20, 0) NOT NULL,
    slot        BIGINT NOT NULL,
    tx_index    INT NOT NULL,              -- -1 for snapshot rows
    updated_at  TIMESTAMPTZ NOT NULL DEFAULT now()
);
CREATE INDEX token_balances_owner_idx ON token_balances (owner);

-- Pool reserves and TVL (in token Y, decimal-adjusted) per pair.
CREATE VIEW pair_reserves AS
SELECT
    p.lb_pair,
    rx.amount AS reserve_x_amount,
    ry.amount AS reserve_y_amount,
    greatest(rx.slot, ry.slot) AS reserves_slot,
    CASE WHEN mx.decimals IS NOT NULL AND my.decimals IS NOT NULL THEN
        rx.amount / power(10::numeric, mx.decimals) * dlmm_ui_price(p.active_id, p.bin_step, mx.decimals, my.decimals)::numeric
        + ry.amount / power(10::numeric, my.decimals)
    END AS tvl_in_y
FROM lb_pairs p
LEFT JOIN token_balances rx ON rx.account = p.reserve_x
LEFT JOIN token_balances ry ON ry.account = p.reserve_y
LEFT JOIN mints mx ON mx.mint = p.token_x_mint
LEFT JOIN mints my ON my.mint = p.token_y_mint;
