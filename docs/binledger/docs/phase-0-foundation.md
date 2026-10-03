# Phase 0 build spec: mainnet foundation

Goal: the indexer runs on mainnet and keeps enough history that any tracked position can be valued at any slot since tracking began, with a USD price that has provenance. Four weeks.

## Scope

| In | Out |
| --- | --- |
| Mainnet deployment and soak | Any accounting logic |
| Tracked-set registry and discovery | Tenancy and auth |
| Versioned state for tracked accounts | Full-program account history |
| Price service | Reporting-currency conversion |
| Per-wallet historical backfill | Archive-node replay of bin state |
| Finalized watermark for downstream workers | Timescale or ClickHouse |

## Confirm in `idl/dlmm.json` before coding

- [ ] Exact account type names for positions, limit orders, bin arrays and pools (the README lists `PositionV2`, `LimitOrder`, `BinArray`, `LbPair`).
- [ ] The fields of a bin inside a bin array: reserves, share supply, fee and reward growth counters, and any limit-order fields added in the limit-order release.
- [ ] How a bin ID maps to a bin array index and offset, including negative IDs.
- [ ] Which events carry position, owner and amounts for add, remove, claim fee, claim reward, position create and close, limit order place, fill, cancel and close.
- [ ] Whether the stream's account updates include the signature of the writing transaction; store it if so.

## Data model

```sql
CREATE SCHEMA track;
CREATE SCHEMA hist;
CREATE SCHEMA px;

CREATE TABLE track.wallets (
  wallet        TEXT PRIMARY KEY,
  added_at      TIMESTAMPTZ NOT NULL DEFAULT now(),
  tracking_slot BIGINT,              -- first slot with exact state history
  backfill      TEXT NOT NULL DEFAULT 'pending'  -- pending, running, done, failed
);

CREATE TABLE track.positions (
  position      TEXT PRIMARY KEY,
  kind          TEXT NOT NULL,       -- 'position' or 'limit_order'
  owner         TEXT NOT NULL REFERENCES track.wallets,
  lb_pair       TEXT NOT NULL,
  lower_bin_id  INT,
  upper_bin_id  INT,
  created_slot  BIGINT,
  closed_slot   BIGINT,
  tracking_slot BIGINT NOT NULL
);

CREATE TABLE track.bin_arrays (     -- arrays to version, with a reference count
  bin_array TEXT PRIMARY KEY,
  lb_pair   TEXT NOT NULL,
  idx       BIGINT NOT NULL,
  refs      INT NOT NULL
);

CREATE TABLE hist.bin_versions (
  lb_pair          TEXT   NOT NULL,
  bin_id           INT    NOT NULL,
  slot             BIGINT NOT NULL,
  write_version    BIGINT NOT NULL,
  txn_signature    TEXT,
  amount_x         NUMERIC(40,0) NOT NULL,
  amount_y         NUMERIC(40,0) NOT NULL,
  liquidity_supply NUMERIC(40,0) NOT NULL,
  data             JSONB  NOT NULL,   -- full decoded bin, for fee counters
  PRIMARY KEY (lb_pair, bin_id, slot, write_version)
);

CREATE TABLE hist.position_versions (
  position      TEXT   NOT NULL,
  slot          BIGINT NOT NULL,
  write_version BIGINT NOT NULL,
  txn_signature TEXT,
  data          JSONB  NOT NULL,      -- decoded account; NULL-equivalent row on close
  closed        BOOLEAN NOT NULL DEFAULT false,
  PRIMARY KEY (position, slot, write_version)
);

CREATE TABLE hist.pair_versions (
  lb_pair       TEXT   NOT NULL,
  slot          BIGINT NOT NULL,
  write_version BIGINT NOT NULL,
  active_id     INT    NOT NULL,
  data          JSONB  NOT NULL,
  PRIMARY KEY (lb_pair, slot, write_version)
);

CREATE TABLE px.price_points (
  mint      TEXT NOT NULL,
  minute    TIMESTAMPTZ NOT NULL,
  source    TEXT NOT NULL,
  price_usd NUMERIC(38,18) NOT NULL,
  meta      JSONB,
  PRIMARY KEY (mint, minute, source)
);

CREATE TABLE px.canonical_prices (
  price_id  BIGSERIAL PRIMARY KEY,
  mint      TEXT NOT NULL,
  minute    TIMESTAMPTZ NOT NULL,
  price_usd NUMERIC(38,18) NOT NULL,
  source    TEXT NOT NULL,
  flags     TEXT[] NOT NULL DEFAULT '{}',  -- stale, self_priced, disputed
  UNIQUE (mint, minute)
);
```

## Work packages

| ID | Package | Depends on |
| --- | --- | --- |
| P0-WP1 | Mainnet deployment and soak | none |
| P0-WP2 | Tracked-set registry and discovery | WP1 |
| P0-WP3 | State history writer and readers | WP2 |
| P0-WP4 | Finalized watermark and downstream cursor contract | WP1 |
| P0-WP5 | Price service | none |
| P0-WP6 | Token registry hardening | none |
| P0-WP7 | Per-wallet backfill | WP2 |
| P0-WP8 | Stream filter for cost control (optional, decided at the gate) | WP2 |

### P0-WP1 Mainnet deployment and soak

- New empty database for mainnet; the genesis-hash guard already prevents mixing networks.
- Deploy indexer and chain API with the existing systemd units and monitoring profile.
- Record for seven days: stream megabytes per second, rows per day per table, database growth, slot lag percentiles, decode failures by discriminator.
- Write the numbers into `docs/mainnet-baseline.md`. They size the database and decide WP8.

Acceptance: seven consecutive days with no gap open longer than 10 minutes; every `decode_failures` row explained or fixed; restart test loses nothing.

### P0-WP2 Tracked-set registry and discovery

- `bl-track` crate with `add_wallet`, `remove_wallet`, `list` and a CLI subcommand.
- On `add_wallet`: insert the wallet, trigger the existing hydration path for its positions, find its limit orders, insert `track.positions`, compute the bin arrays each one spans and upsert `track.bin_arrays` with reference counts.
- Discovery: when a finalized event shows a tracked wallet creating a position or order, add it automatically in the same database transaction. When one closes, set `closed_slot` and decrement array references.
- Emit `NOTIFY track_changed` so the indexer refreshes its in-memory tracked set.
- `tracking_slot` for a position is the slot of the first version written for it and its bins.

Acceptance: adding a wallet with 50 positions completes in under 5 minutes; a position created by a tracked wallet appears in `track.positions` within the same slot batch; integration test covers create, widen, close.

### P0-WP3 State history

- In the indexer's account write path, after the latest-state upsert and in the same transaction, write a version row when the account is a tracked position or order, a tracked pool, or a tracked bin array.
- For a bin array update, split into bins and write a `hist.bin_versions` row only for bins whose content changed from their latest stored version.
- When a position becomes tracked, write a baseline version of its account and of every bin it spans at the hydration slot.
- Dead-slot handling: delete version rows for slots the indexer marks dead, the same way other tables are handled.
- Readers in `bl-hist`:

```rust
pub async fn bin_at(pool: &PgPool, lb_pair: &Pubkey, bin_id: i32, slot: u64) -> Result<Option<BinState>>;
pub async fn bins_at(pool: &PgPool, lb_pair: &Pubkey, lo: i32, hi: i32, slot: u64) -> Result<Vec<BinState>>;
pub async fn position_at(pool: &PgPool, position: &Pubkey, slot: u64) -> Result<Option<PositionState>>;
pub async fn state_after_tx(pool: &PgPool, signature: &str) -> Result<TxState>; // post-state of tracked accounts written by this tx
```

`*_at` returns the latest version with `slot <= s`. For state before a transaction, take the latest version strictly earlier in `(slot, write_version)` order than that transaction's first write.

Acceptance: for 20 tracked positions, state returned by `position_at` and `bins_at` at a recent finalized slot equals the accounts fetched from RPC with `minContextSlot` at that slot; deleting and repairing a slot range restores identical version rows.

### P0-WP4 Finalized watermark

- Expose `indexer_state.finalized` as the contract for all downstream workers.
- `NOTIFY ledger_ready, '<slot>'` when it advances.
- A view `finalized_dlmm_txs` listing transactions at or below the watermark with their touched tracked accounts.
- Document the cursor pattern: each worker stores its own `(slot, tx_index)` cursor and commits it with its output.

Acceptance: a test worker that copies signatures processes each finalized transaction exactly once across kills and restarts.

### P0-WP5 Price service

- `bl-prices` crate with a `PriceSource` trait and at least two sources plus a pool-derived source.
- A poller writes `px.price_points` every minute for every mint in tracked pools.
- A resolver writes `px.canonical_prices` using the hierarchy: oracle, aggregator, pool-derived. It sets `disputed` when the top two sources differ by more than a configurable threshold, `stale` when the chosen point is older than a configurable age, and `self_priced` when only the token's own pool prices it.
- SQL function `px.price_at(mint, ts)` returns the canonical row at or before `ts`.
- Backfill mode fetches historical minute prices where a source offers them; otherwise it derives them from indexed swaps against a priced quote token.

Acceptance: stablecoin and SOL prices present for every minute of a 7-day window; a simulated source outage produces `stale` flags and an alert, not gaps.

### P0-WP6 Token registry hardening

- Extend `mints` with token program, extension list, transfer-fee configuration and hook program, read from the mint account.
- Flag mints whose extensions affect amounts received.

Acceptance: for a transfer-fee token pool on devnet, the registry shows the fee configuration and the flag.

### P0-WP7 Per-wallet backfill

- For a newly tracked wallet, page signatures for the wallet and for each of its known positions, keep transactions that invoke the DLMM program, fetch them, and run them through the same extractor used by gap repair.
- Positions closed before tracking no longer exist as accounts; they are found through the wallet's own transaction history.
- Rows from backfill are indistinguishable from live rows except for a `source` marker on `transactions`.
- Record progress in `track.wallets.backfill`, resumable.

Acceptance: for 20 sampled positions of a real wallet, the count and amounts of deposits, withdrawals and fee claims in the database equal a manual count from an explorer.

### P0-WP8 Stream filter (optional)

- Configuration switch: subscribe to program-wide transactions (current behaviour) or only to transactions mentioning tracked pools.
- With the filter on, pool-wide statistics exist only for tracked pools. The chain explorer frontend degrades accordingly.

Decide at the gate using WP1's cost numbers.

## Exit gate

- [ ] DF-1: seven-day mainnet soak passed.
- [ ] DF-2: any tracked position valued at any slot since its tracking slot, proven by the WP3 test.
- [ ] DF-3: wallet add, hydrate and discover flows pass.
- [ ] DF-4: canonical price with provenance for every mint in tracked pools.
- [ ] DF-5: backfill sample matches manual count.
- [ ] DF-6: transfer-fee tokens flagged.
- [ ] Storage growth per tracked position per day measured and written down.
