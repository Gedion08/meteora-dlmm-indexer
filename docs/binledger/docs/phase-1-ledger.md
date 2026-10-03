# Phase 1 build spec: position ledger

Goal: a deterministic, double-entry ledger that turns finalized DLMM activity of tracked wallets into inventories, lots, valuations and a PnL decomposition that sums exactly. Eight weeks. This is the phase the whole company rests on.

## Scope

| In | Out |
| --- | --- |
| Pure DLMM math crate with SDK parity | Any user interface |
| Ledger schema, journal builder, inventory tracker | Auth and tenant isolation (a minimal `app.entities` table only) |
| Fee and reward accrual; limit-order accounting | Liquidity health metrics |
| Lots, relief methods, PnL engine | Reporting currency other than USD |
| Daily close, restatement, reconciliation | Statements and exports |

## Confirm in `idl/dlmm.json` before coding

- [ ] Position fields holding per-bin liquidity shares, per-bin fee checkpoints and pending fees, reward checkpoints, lower and upper bin IDs, owner, fee owner and operator.
- [ ] Bin fields holding fee growth per share and reward growth per share, and the fixed-point scale used.
- [ ] Limit-order account fields: side, per-bin deposited amount, filled amount, fee accrued, and how fills are checkpointed.
- [ ] Pool fields: bin step, active ID, protocol share, collect-fee mode, function mode, reward infos.
- [ ] Rounding direction in share-to-amount conversion and in fee claims, checked against the SDK source.

## Minimal entity tables (hardened in Phase 2)

```sql
CREATE SCHEMA app;
CREATE TABLE app.entities (
  entity_id   UUID PRIMARY KEY,
  name        TEXT NOT NULL,
  lot_method  TEXT NOT NULL DEFAULT 'wac',      -- 'fifo' or 'wac'
  deposit_is_disposal BOOLEAN NOT NULL DEFAULT false,
  fee_recognition TEXT NOT NULL DEFAULT 'accrued', -- 'accrued' or 'claimed'
  close_time_utc TIME NOT NULL DEFAULT '00:00'
);
CREATE TABLE app.entity_wallets (
  entity_id UUID NOT NULL REFERENCES app.entities,
  wallet    TEXT NOT NULL REFERENCES track.wallets,
  PRIMARY KEY (entity_id, wallet)
);
```

## Ledger data model

```sql
CREATE SCHEMA ledger;

CREATE TABLE ledger.accounts (
  account_id BIGSERIAL PRIMARY KEY,
  entity_id  UUID NOT NULL,
  kind       TEXT NOT NULL,   -- wallet, position_inventory, order_inventory, fees_receivable,
                              -- rewards_receivable, income_fees, income_rewards, expense_network,
                              -- rent_deposit, conversion, external, suspense
  ref        TEXT,            -- wallet, position or order pubkey
  UNIQUE (entity_id, kind, ref)
);

-- journals and entries as in the architecture document, plus:

CREATE TABLE ledger.inventories (      -- latest token amounts per position or order
  entity_id UUID NOT NULL,
  ref       TEXT NOT NULL,
  mint      TEXT NOT NULL,
  amount    NUMERIC(40,0) NOT NULL,
  as_of_slot BIGINT NOT NULL,
  PRIMARY KEY (entity_id, ref, mint)
);

CREATE TABLE ledger.hold_baskets (     -- benchmark basket per position
  entity_id UUID NOT NULL,
  position  TEXT NOT NULL,
  mint      TEXT NOT NULL,
  amount    NUMERIC(60,20) NOT NULL,
  PRIMARY KEY (entity_id, position, mint)
);

CREATE TABLE ledger.lots (
  lot_id      BIGSERIAL PRIMARY KEY,
  entity_id   UUID NOT NULL,
  mint        TEXT NOT NULL,
  acquired_at TIMESTAMPTZ NOT NULL,
  journal_id  BIGINT NOT NULL,
  qty         NUMERIC(40,0) NOT NULL,
  qty_left    NUMERIC(40,0) NOT NULL,
  cost_usd    NUMERIC(38,10) NOT NULL
);

CREATE TABLE ledger.lot_reliefs (
  relief_id   BIGSERIAL PRIMARY KEY,
  lot_id      BIGINT NOT NULL REFERENCES ledger.lots,
  journal_id  BIGINT NOT NULL,
  qty         NUMERIC(40,0) NOT NULL,
  proceeds_usd NUMERIC(38,10) NOT NULL,
  gain_usd    NUMERIC(38,10) NOT NULL
);

CREATE TABLE ledger.valuations (
  entity_id UUID NOT NULL,
  ref       TEXT NOT NULL,
  slot      BIGINT NOT NULL,
  mint      TEXT NOT NULL,
  amount    NUMERIC(40,0) NOT NULL,
  unclaimed NUMERIC(40,0) NOT NULL DEFAULT 0,
  price_ref BIGINT NOT NULL,
  usd_value NUMERIC(38,10) NOT NULL,
  PRIMARY KEY (entity_id, ref, slot, mint)
);

CREATE TABLE ledger.closes (
  entity_id  UUID NOT NULL,
  close_date DATE NOT NULL,
  version    INT  NOT NULL,
  last_slot  BIGINT NOT NULL,
  input_hash TEXT NOT NULL,
  data       JSONB NOT NULL,     -- per-position PnL components and balances
  created_at TIMESTAMPTZ NOT NULL DEFAULT now(),
  reason     TEXT,               -- null for first close; text for restatements
  PRIMARY KEY (entity_id, close_date, version)
);

CREATE TABLE ledger.recon_breaks (
  break_id  BIGSERIAL PRIMARY KEY,
  entity_id UUID NOT NULL,
  ref       TEXT NOT NULL,
  mint      TEXT NOT NULL,
  slot      BIGINT NOT NULL,
  ledger_amount NUMERIC(40,0) NOT NULL,
  chain_amount  NUMERIC(40,0) NOT NULL,
  status    TEXT NOT NULL DEFAULT 'open',
  note      TEXT
);
```

## Journal rules

Each rule is a pure function from decoded events and account states to entries. Dr means the account increases; Cr means it decreases. Every journal balances per mint.

| Trigger | Dr | Cr | Notes |
| --- | --- | --- | --- |
| Position created | rent\_deposit (SOL) | wallet (SOL) | Rent from lamport delta of the new account |
| Add liquidity | position\_inventory (X, Y) | wallet (X, Y) | Amounts from the event; adds to hold basket |
| Remove liquidity | wallet (X, Y) | position\_inventory (X, Y) | Reduces hold basket by the share fraction removed |
| Fee accrual (derived) | fees\_receivable | income\_fees | At each valuation point, from growth counters |
| Claim fee | wallet | fees\_receivable | Accrue up to the claim slot first, so receivable never goes negative |
| Reward accrual, claim | rewards\_receivable; wallet | income\_rewards; rewards\_receivable | Same pattern as fees |
| Composition change (derived) | position\_inventory (token gained), conversion (token lost) | conversion (token gained), position\_inventory (token lost) | From recomputed inventory versus ledger inventory |
| Position closed | wallet (SOL) | rent\_deposit (SOL) | Any shortfall goes to expense\_network |
| Limit order placed | order\_inventory; rent\_deposit | wallet | Per bin in entry metadata |
| Limit order fill (derived) | order\_inventory (token gained), conversion | conversion, order\_inventory (token lost) | Fee share: Dr fees\_receivable, Cr income\_fees |
| Limit order cancelled or withdrawn | wallet | order\_inventory, fees\_receivable | Rent returns on close |
| Network fee, priority fee, tips | expense\_network (SOL) | wallet (SOL) | Only when a tracked wallet is the fee payer |
| Wrap or unwrap SOL | wallet (WSOL) | wallet (SOL), or the reverse | Not a trade |
| Unexplained wallet delta | suspense | wallet, or the reverse | Raises a reconciliation break |

The builder ends every transaction with a check: the sum of wallet legs per mint must equal the wallet's actual token balance change in the transaction metadata. Any difference is posted to `suspense` and recorded as a break. For transfer-fee tokens the actual delta is the truth and the fee difference is posted to `expense_network`.

## Work packages

| ID | Package | Weeks |
| --- | --- | --- |
| P1-WP1 | `bl-dlmm-math` and parity harness | 1 to 2 |
| P1-WP2 | Ledger schema and chart of accounts | 3 |
| P1-WP3 | Journal builder for flow events | 3 to 4 |
| P1-WP4 | Inventory tracker, conversions, fee and reward accrual | 5 |
| P1-WP5 | Limit-order accounting | 5 |
| P1-WP6 | Lot engine | 6 |
| P1-WP7 | Valuation and PnL engine | 6 |
| P1-WP8 | Daily close and restatement | 7 |
| P1-WP9 | Reconciler | 7 |
| P1-WP10 | Rebuild command and determinism | 8 |

### P1-WP1 Math crate and parity harness

Pure functions, no I/O:

```rust
pub fn bin_price_q64(bin_id: i32, bin_step_bps: u16) -> U256;           // (1 + step/10_000)^id in fixed point
pub fn position_amounts(pos: &PositionState, bins: &[BinState]) -> Amounts; // per bin and total, floor rounding
pub fn claimable_fees(pos: &PositionState, bins: &[BinState]) -> Amounts;
pub fn claimable_rewards(pos: &PositionState, bins: &[BinState], pair: &PairState, now: i64) -> Vec<RewardAmount>;
pub fn limit_order_view(order: &OrderState, bins: &[BinState]) -> OrderView; // unfilled, filled, fees per bin
```

Parity harness in `tools/parity`:

1. A TypeScript script picks N positions and orders, fetches the raw account data it needs in one call with a shared context slot, and asks the Meteora SDK for amounts, claimable fees, rewards and order views.
2. It writes one JSON fixture per case containing the raw accounts (base64) and the SDK outputs.
3. A Rust test decodes the raw accounts with the existing IDL decoder, runs the math, and asserts equality to the raw unit.

Because the fixture holds both inputs and expected outputs from the same slot, the test is offline and repeatable.

Acceptance (LA-2, LA-3, LA-4): 200 positions and 100 limit orders across at least 10 pools, including wide positions, one-sided positions, positions with unclaimed rewards, Token-2022 pools and partially filled orders; all exact.

### P1-WP2 Schema and chart of accounts

Migrations for the tables above; helper to get or create ledger accounts; database constraint trigger that rejects a journal whose entries do not sum to zero per mint.

Acceptance: inserting an unbalanced journal fails at commit.

### P1-WP3 Journal builder for flow events

- Worker follows the Phase 0 cursor pattern over `finalized_dlmm_txs` for wallets linked to an entity.
- One function per rule in the table. Input: decoded events of the transaction, token and lamport balance deltas, state before and after from `bl-hist`.
- Writes journals with `source = 'event'` and the current `rule_version`.

Acceptance (LA-1): fixtures for every trigger in the rules table with hand-checked expected entries; property test that every produced journal balances; wallet-delta check passes on 1,000 real transactions with no suspense postings, or each one explained.

### P1-WP4 Inventory tracker and accrual

- After flow journals for a transaction, and at each close, recompute each affected position's amounts with `position_amounts` at the relevant slot.
- Post the difference from `ledger.inventories` as a conversion journal with `source = 'derived'`.
- Compute claimable fees and rewards; post the increase since the last accrual.
- Store conversion prices: the USD value of tokens gained and lost at the canonical price for that time.

Acceptance: after processing, `ledger.inventories` equals `position_amounts` at the watermark for every open position; `fees_receivable` equals `claimable_fees`.

### P1-WP5 Limit-order accounting

Implement the four states from the product spec. Fills are derived the same way as conversions, from `limit_order_view` differences. Fee share is recognised as income when the fill is observed.

Acceptance: scripted devnet scenario covering place, partial fill, full fill, cancel and close matches expected entries; mainnet sample of 100 orders reconciles.

### P1-WP6 Lot engine

- Lots are created on wallet acquisitions: withdrawals, fee and reward claims, order proceeds, and opening balances at tracking start (cost at the price then, flagged `opening`).
- Relief on wallet disposals: deposits when `deposit_is_disposal` is true, and transfers out to `external`.
- FIFO relieves oldest first. Weighted average keeps one pooled lot per mint.

Acceptance (LA-5): a scripted scenario of 30 events matches a hand-worked spreadsheet stored in `fixtures/ledger/lots.xlsx` under both methods.

### P1-WP7 Valuation and PnL engine

- Maintain `ledger.hold_baskets` on add and remove as defined in the product spec.
- `pnl(entity, ref, from_slot, to_slot) -> PnlBreakdown` with fields: price\_effect, divergence, fees, rewards, costs, total, and the inputs used.
- The function computes total two ways (identity and sum of parts) and returns an error if they differ.

Acceptance (LA-6): zero residual on every position of the OrderFlow test entity and on the 200-position parity set over three different periods.

### P1-WP8 Daily close and restatement

- Close job per entity at its close time: wait for watermark, accrue, value, reconcile, write `ledger.closes` version 1 with an input hash.
- If raw data for a closed day changes (repair, backfill) or a rule version changes, write a new version with a reason and a stored diff. Never update a close row.

Acceptance (LA-7, LA-8): re-running a close gives the same hash; injecting a late backfilled transaction produces version 2 with a correct diff.

### P1-WP9 Reconciler

- After each worker batch: compare `ledger.inventories` and wallet balances for touched refs against chain-derived state at the same slot.
- Full pass at each close.
- Open breaks block the close for that entity and raise an alert.

Acceptance (LA-9): corrupting one inventory row is detected within 10 minutes and blocks the close.

### P1-WP10 Rebuild and determinism

- `bl-ledger rebuild --entity <id>` deletes derived rows for the entity and replays from raw.
- `bl-ledger hash --entity <id>` prints a hash over journals, entries, lots and closes in canonical order.

Acceptance: two rebuilds on the same raw data give the same hash; documented in the runbook.

## Exit gate

- [ ] Parity set exact: 200 positions, 100 orders.
- [ ] Zero PnL residual across the parity set.
- [ ] Seven days of closes on the OrderFlow entity with no open breaks.
- [ ] Deterministic rebuild proven.
- [ ] Methodology paper draft written from this spec and the product spec section 6.
