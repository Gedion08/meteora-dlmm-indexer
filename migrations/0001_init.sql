-- Meteora DLMM live indexer — core tables.
--
-- Conventions
--   * Every row is keyed so that re-delivering the same data is a no-op (idempotent).
--   * Tables with a `slot` column in their primary key can later become TimescaleDB
--     hypertables partitioned on `slot` without changing keys.
--   * Pubkeys and signatures are base58 TEXT for easy querying.
--   * Decoded payloads are JSONB: u64 values are JSON numbers, u128 values are
--     decimal strings. Cast with `(data->>'field')::numeric`.

CREATE TABLE indexer_state (
    key         TEXT PRIMARY KEY,           -- 'checkpoint' | 'finalized'
    slot        BIGINT NOT NULL,
    updated_at  TIMESTAMPTZ NOT NULL DEFAULT now()
);

CREATE TABLE slots (
    slot          BIGINT PRIMARY KEY,
    parent_slot   BIGINT,
    block_time    TIMESTAMPTZ,
    block_height  BIGINT,
    blockhash     TEXT,
    indexed_at    TIMESTAMPTZ NOT NULL DEFAULT now()
);
CREATE INDEX slots_block_time_idx ON slots USING brin (block_time);

CREATE TABLE transactions (
    slot                 BIGINT NOT NULL,
    signature            TEXT NOT NULL,
    tx_index             INT NOT NULL,          -- position within the block
    block_time           TIMESTAMPTZ,
    fee_payer            TEXT NOT NULL,
    success              BOOLEAN NOT NULL,
    err                  TEXT,
    fee                  BIGINT NOT NULL,       -- lamports
    compute_units        BIGINT,
    account_keys         TEXT[] NOT NULL,       -- static + ALT-loaded, runtime order
    pre_token_balances   JSONB NOT NULL,
    post_token_balances  JSONB NOT NULL,
    indexed_at           TIMESTAMPTZ NOT NULL DEFAULT now(),
    PRIMARY KEY (slot, signature)
);
CREATE INDEX transactions_signature_idx ON transactions (signature);
CREATE INDEX transactions_fee_payer_idx ON transactions (fee_payer, slot DESC);

-- Every DLMM instruction, top-level or CPI (e.g. routed through Jupiter).
CREATE TABLE instructions (
    slot                BIGINT NOT NULL,
    signature           TEXT NOT NULL,
    ix_index            SMALLINT NOT NULL,      -- top-level instruction index
    inner_index         SMALLINT NOT NULL,      -- -1 = the top-level instruction itself
    tx_index            INT NOT NULL,
    block_time          TIMESTAMPTZ,
    stack_height        SMALLINT,
    invoked_by          TEXT,                   -- top-level program for CPIs
    name                TEXT NOT NULL,          -- IDL instruction name, e.g. swap2
    lb_pair             TEXT,
    wallet              TEXT,                   -- user / sender / owner / funder
    accounts            JSONB NOT NULL,         -- { idl_account_name: pubkey | null }
    remaining_accounts  TEXT[] NOT NULL,
    args                JSONB NOT NULL,
    success             BOOLEAN NOT NULL,
    PRIMARY KEY (slot, signature, ix_index, inner_index)
);
CREATE INDEX instructions_name_idx ON instructions (name, slot DESC);
CREATE INDEX instructions_lb_pair_idx ON instructions (lb_pair, slot DESC);
CREATE INDEX instructions_wallet_idx ON instructions (wallet, slot DESC);
CREATE INDEX instructions_signature_idx ON instructions (signature);

-- Every Anchor emit_cpi! event (Swap, AddLiquidity, ClaimFee, ...).
CREATE TABLE events (
    slot         BIGINT NOT NULL,
    signature    TEXT NOT NULL,
    ix_index     SMALLINT NOT NULL,
    inner_index  SMALLINT NOT NULL,
    tx_index     INT NOT NULL,
    block_time   TIMESTAMPTZ,
    name         TEXT NOT NULL,
    lb_pair      TEXT,
    position     TEXT,
    wallet       TEXT,                           -- from / owner / sender / funder
    data         JSONB NOT NULL,
    PRIMARY KEY (slot, signature, ix_index, inner_index)
);
CREATE INDEX events_name_idx ON events (name, slot DESC);
CREATE INDEX events_lb_pair_idx ON events (lb_pair, slot DESC);
CREATE INDEX events_wallet_idx ON events (wallet, slot DESC);
CREATE INDEX events_position_idx ON events (position, slot DESC) WHERE position IS NOT NULL;
CREATE INDEX events_signature_idx ON events (signature);

-- Latest state of every DLMM-owned account (LbPair, BinArray, PositionV2, ...).
CREATE TABLE accounts (
    pubkey          TEXT PRIMARY KEY,
    account_type    TEXT NOT NULL,
    slot            BIGINT NOT NULL,
    write_version   BIGINT NOT NULL,             -- 0 for RPC snapshot rows
    lamports        BIGINT NOT NULL,
    data_len        INT NOT NULL,
    trailing_bytes  INT NOT NULL,                -- e.g. extended bins of resized positions
    lb_pair         TEXT,
    owner_wallet    TEXT,
    data            JSONB NOT NULL,
    closed_slot     BIGINT,                      -- set when the account is closed
    updated_at      TIMESTAMPTZ NOT NULL DEFAULT now()
);
CREATE INDEX accounts_type_idx ON accounts (account_type);
CREATE INDEX accounts_lb_pair_idx ON accounts (lb_pair, account_type);
CREATE INDEX accounts_owner_idx ON accounts (owner_wallet) WHERE owner_wallet IS NOT NULL;
CREATE INDEX accounts_slot_idx ON accounts (slot);

-- Dead-letter queue: anything we could not decode (IDL drift, new instructions).
CREATE TABLE decode_failures (
    id           BIGSERIAL PRIMARY KEY,
    slot         BIGINT NOT NULL,
    kind         TEXT NOT NULL,                  -- instruction | event | account
    signature    TEXT,
    pubkey       TEXT,
    ix_index     SMALLINT,
    inner_index  SMALLINT,
    -- events carry the 8-byte emit_cpi tag first; their discriminator is the next 8 bytes
    discriminator TEXT GENERATED ALWAYS AS (
        encode(substring(raw from (CASE WHEN kind = 'event' THEN 9 ELSE 1 END) for 8), 'hex')) STORED,
    raw          BYTEA NOT NULL,
    error        TEXT NOT NULL,
    created_at   TIMESTAMPTZ NOT NULL DEFAULT now(),
    UNIQUE NULLS NOT DISTINCT (kind, slot, signature, pubkey, ix_index, inner_index)
);

-- Slot ranges we know we missed (e.g. offline longer than the provider's replay window).
CREATE TABLE gaps (
    id           BIGSERIAL PRIMARY KEY,
    from_slot    BIGINT NOT NULL,
    to_slot      BIGINT NOT NULL,
    reason       TEXT NOT NULL,
    detected_at  TIMESTAMPTZ NOT NULL DEFAULT now(),
    repaired_at  TIMESTAMPTZ
);
