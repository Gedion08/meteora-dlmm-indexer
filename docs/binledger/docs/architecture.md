# Architecture Design Document

The system is a pipeline with one loop: chain data becomes a ledger, the ledger feeds reporting and strategy, and execution sends transactions whose effects come back through the same indexer.

## 1. Architecture at a glance

&#91;embedded content: system architecture · 4 layers, 12 components, 1 execution loop\]

Read it top to bottom. Nothing below the ledger reads raw chain tables directly, and the executor never trusts its own view of an outcome: a transaction is done only when the indexer has seen it finalized and the ledger has journaled it.

## 2. Key decisions

| # | Decision | Why | Trade-off accepted |
| --- | --- | --- | --- |
| D1 | Extend the existing Rust workspace and Postgres database into one monorepo | The indexer already gives exactly-once writes, gap repair and reconciliation | Rust is slower to iterate on than TypeScript for product code |
| D2 | The ledger is a derived, rebuildable projection of raw tables | Bugs in accounting rules are fixed by replay, not by patching rows | Rebuilds take time at scale; mitigated by per-wallet replay |
| D3 | Keep versioned account history only for the tracked set | Full-program history on mainnet is costly and unneeded | A wallet added later has reconstructed, not exact, mark-to-market before its tracking slot |
| D4 | Ledger and statements use finalized data only; the UI may overlay confirmed data marked provisional | Removes reorg handling from accounting | Up to roughly 15 to 30 s extra delay on ledger figures |
| D5 | Journal builder is a pure function of one transaction's decoded events and account states | Testable with fixtures; deterministic | Needs complete pre-state and post-state, which D3 provides |
| D6 | Strategy code is pure and holds no keys; only the executor signs | Limits blast radius and makes backtest equal production | Extra hop between decision and action |
| D7 | Transactions are built with the official Meteora TypeScript SDK inside an isolated signer service | Avoids re-implementing instruction building and account resolution | One TypeScript service in a Rust system |
| D8 | Policy vault program is minimal: no operator swaps, no pooled funds, one vault per mandate | Smallest audit surface | Rebalancing is slower and uses one-sided liquidity and limit orders |
| D9 | Multi-tenancy by Postgres row-level security on tenant tables; chain tables are shared | Chain data is public and identical for all tenants | Care needed so tenant labels never leak through joins |
| D10 | A venue adapter boundary from day one | Meteora is the first venue, not the model | Slight indirection now |
| D11 | Postgres only until a measured need for a column store | One system to operate | Heavy analytics queries need care; Timescale or ClickHouse later |
| D12 | Background jobs on a Postgres-backed queue | No extra broker to run | Throughput ceiling far above year-one needs |

## 3. Baseline and gaps

What the existing repository provides, and what each phase adds.

| Area | Exists today | Gap | Phase |
| --- | --- | --- | --- |
| Streaming and decoding | Yellowstone gRPC, IDL-driven decode of all instructions, events, accounts | Mainnet deployment; stream filtering to tracked pools | 0 |
| Durability | Idempotent upserts, checkpoint in the same transaction, gap audit and repair, reconciliation | Finalized watermark exposed to downstream consumers | 0 |
| Account state | Latest state per account | Versioned history for tracked positions, limit orders, bin arrays, pools | 0 |
| Prices | Bin price, 24h stats | USD price service with provenance | 0 |
| History | Live from deployment; RPC repair of gaps | Per-wallet backfill before tracking | 0 |
| Accounting | None | Ledger, valuation, lots, PnL, close | 1 |
| API and frontend | Read-only chain API and explorer-style frontend | Tenant-aware reporting API and app | 2 |
| Execution | None | Simulator, strategies, risk, executor, vault program | 3 to 4 |

## 4. Components

### 4.1 Indexer (existing, extended)

- Keeps its current responsibilities and reliability model unchanged.
- Adds a tracked-set filter: account subscriptions for tracked positions, limit orders and the bin arrays they touch, in addition to program-wide transaction streaming.
- Publishes a finalized watermark (slot) in `indexer_state` and a `NOTIFY ledger_ready` when it advances.
- Writes account versions for the tracked set into `hist` in the same database transaction as the latest-state upsert.

### 4.2 State history store

- Append-only versions keyed by `(pubkey, slot, write_version)`.
- Bin state is stored per bin, not per bin array, so valuation queries read only the bins a position holds.
- Retention: forever for tracked accounts. Storage estimate is a Phase 0 measurement task.

### 4.3 Price service

- Polls and stores per-minute USD prices per mint from each configured source.
- Resolves a canonical price per mint per minute using the hierarchy in the product spec, storing which source won.
- Exposes `price_at(mint, ts)` in SQL and Rust. Never interpolates silently; a missing minute returns the last price with a staleness flag.

### 4.4 Ledger engine

| Part | Responsibility |
| --- | --- |
| Journal builder | For each finalized transaction touching a tracked account, emit one balanced journal in token units |
| Inventory tracker | Maintain per-position, per-bin and per-order token inventory, including composition changes from swaps |
| Fee accrual | Compute unclaimed fees and rewards from fee growth counters at each valuation point |
| Valuation | Value inventories at canonical prices; store the price reference |
| Lot engine | Create and relieve lots under the entity's policy |
| PnL engine | Produce the decomposition for any position and period |
| Close | Daily close per entity; immutable snapshot; restatement on change |
| Reconciler | Compare ledger balances to chain balances; raise breaks |

Composition change is the subtle part. A swap that crosses a bin changes every position holding shares in that bin, with no event per position. The inventory tracker derives it from bin versions: for each tracked position, at each close and at each of its own events, it recomputes token amounts from shares and bin reserves and journals the difference as an internal conversion.

### 4.5 Metrics engine

Computes the liquidity health metrics from bin state, pool swaps and position inventory. Runs per pool per hour and per day. The slippage metric uses the same simulator as the strategy engine.

### 4.6 Reporting API and app

- A new service, `binledger-api`, separate from the existing chain API. It serves tenant data and reads the `ledger`, `metrics` and `app` schemas.
- The existing chain API stays as the public and internal chain explorer API.
- The app is a new React application that reuses the charting and formatting modules from the existing frontend.

### 4.7 Report generator

Builds statements from closed periods only. Data is assembled as JSON, stored with a hash, then rendered to PDF, CSV and XLSX. The stored JSON is the legal record; renders are views of it.

### 4.8 Alert engine

Rules evaluated on each finalized watermark advance and on each metrics run. Delivery by email and webhook with at-least-once semantics and idempotency keys.

### 4.9 Simulator and strategy engine

- The simulator replays a pool from a starting bin state through a list of swaps and liquidity actions, reproducing active bin, reserves and fees.
- A strategy is a pure function: `(pool_state, position_state, mandate, clock) -> Vec<Intent>`.
- The same strategy code runs in backtest, in advisory mode and in live mode.

### 4.10 Risk engine

Evaluates every intent before it becomes a transaction, and evaluates global halt conditions continuously. Checks are data: each has an ID, inputs, result and reason stored in `exec.risk_events`.

### 4.11 Executor and signer

| Part | Language | Responsibility |
| --- | --- | --- |
| Intent manager | Rust | State machine for intents; idempotency; retries; links intent to signature to journal |
| Transaction builder | TypeScript, Meteora SDK | Turn an approved intent into one or more transactions; simulate |
| Signer | Managed key service | Sign only transactions whose instructions match an allow-list |
| Sender | Rust | Priority fee selection, send, rebroadcast until blockhash expiry |
| Confirmer | Rust | Mark done only on finalized journal from our indexer |

### 4.12 Policy vault program

An Anchor program. One mandate account and one vault authority (a program-derived address) per client mandate. The vault authority owns the token accounts and is the owner of the DLMM positions and limit orders.

| Instruction | Signer | Checks |
| --- | --- | --- |
| `init_mandate` | Client | Sets pools, bin bounds, limits, operator, mandate hash |
| `deposit` | Client | Tokens to vault accounts |
| `withdraw` | Client | Any amount, any time, to client-chosen accounts |
| `update_policy` | Client | Replaces limits; emits event |
| `set_operator` | Client | Rotates operator key |
| `pause` / `unpause` | Client; pause also by operator | Pause blocks all operator instructions |
| `op_add_liquidity` | Operator | Pool allowed; bins inside bounds; notional and cooldown limits |
| `op_remove_liquidity` | Operator | Proceeds only to vault accounts |
| `op_claim_fees` | Operator | Proceeds only to vault accounts |
| `op_place_limit_order` | Operator | Pool allowed; bins inside bounds; side allowed |
| `op_cancel_limit_order` | Operator | Proceeds only to vault accounts |
| `op_close_position` | Operator | Rent refund to vault or client |

There is no operator swap instruction and no instruction that sends tokens anywhere but the vault or the client.

### 4.13 Identity and tenancy

Organisation, entity, user, membership and role tables in `app`. Staff access is a separate role with every access logged. API keys are hashed, scoped and tied to an entity.

## 5. Data architecture

| Schema | Contents | Written by | Tenant-scoped |
| --- | --- | --- | --- |
| `public` | Existing raw tables: instructions, events, transactions, slots, accounts, lb\_pairs, positions, bins, swaps, liquidity\_events, fee\_claims, reward\_claims, mints, gaps | Indexer | No |
| `track` | tracked\_wallets, tracked\_positions, tracked\_pairs, backfill\_jobs | API, indexer | No (IDs only) |
| `hist` | account\_versions, bin\_versions, position\_versions, order\_versions | Indexer | No |
| `px` | price\_points, canonical\_prices, price\_sources | Price service | No |
| `ledger` | accounts, journals, entries, inventories, lots, lot\_reliefs, valuations, closes, restatements, recon\_breaks | Ledger engine | By entity |
| `metrics` | position\_daily, pool\_hourly, pool\_daily, health\_daily | Metrics engine | Mixed |
| `app` | orgs, entities, users, memberships, wallets, api\_keys, alert\_rules, alerts, reports, audit\_log | API | Yes |
| `exec` | mandates, strategy\_configs, strategy\_runs, intents, outbound\_txs, risk\_events, halts | Strategy, risk, executor | Yes |

Core ledger tables, as a starting point for Phase 1:

```sql
CREATE TABLE ledger.journals (
  journal_id   BIGSERIAL PRIMARY KEY,
  entity_id    UUID        NOT NULL,
  signature    TEXT        NOT NULL,
  slot         BIGINT      NOT NULL,
  block_time   TIMESTAMPTZ NOT NULL,
  kind         TEXT        NOT NULL,   -- deposit, withdraw, fee_claim, conversion, ...
  rule_version INT         NOT NULL,
  source       TEXT        NOT NULL,   -- 'event' | 'derived'
  UNIQUE (entity_id, signature, kind, slot)
);

CREATE TABLE ledger.entries (
  entry_id    BIGSERIAL PRIMARY KEY,
  journal_id  BIGINT  NOT NULL REFERENCES ledger.journals,
  account_id  BIGINT  NOT NULL REFERENCES ledger.accounts,
  mint        TEXT    NOT NULL,
  amount      NUMERIC(40,0) NOT NULL,  -- raw units, signed; sum per (journal, mint) = 0
  price_ref   BIGINT,                  -- px.canonical_prices row used for USD
  usd_value   NUMERIC(38,10)
);
```

Ledger account types: `wallet`, `position_inventory`, `order_inventory`, `fees_receivable`, `rewards_receivable`, `income_fees`, `income_rewards`, `expense_network`, `rent_deposit`, `external` (the counterparty for flows in and out of scope), and `conversion` (clearing account for composition change).

## 6. Key flows

**Chain event to ledger**

1. Indexer commits a slot batch and advances the finalized watermark.
2. Ledger worker selects finalized transactions touching tracked accounts beyond its own cursor.
3. Journal builder loads decoded events and the account versions before and after the transaction.
4. It emits journals; the worker writes them and advances its cursor in one database transaction.
5. Inventory tracker recomputes affected positions and writes any conversion journals.
6. Reconciler compares ledger balances with chain balances for touched accounts.

**Daily close and statement**

1. At the close time for an entity, the worker waits for the watermark to pass the last slot of the day.
2. It revalues every open inventory, accrues fees and rewards, and writes valuation rows.
3. It runs full reconciliation for the entity. A break blocks the close.
4. It writes an immutable close row with a hash of its inputs.
5. A statement request for a closed range assembles data from close rows only.

**Intent lifecycle**

1. Strategy run produces intents with a deterministic ID from mandate, run and content.
2. Risk engine approves, rejects or defers each one, storing the reason.
3. Builder creates transactions and simulates them; simulation output is checked against the intent's expected token deltas.
4. Signer signs; sender submits and rebroadcasts until landed or expired.
5. On expiry, the intent returns to the strategy for re-evaluation; it is never blindly resent.
6. Confirmer closes the intent when the ledger journals the finalized transaction, and records realized deltas against expected ones.

## 7. Solana and DLMM specifics the design must respect

| Topic | Design response |
| --- | --- |
| Commitment and dead slots | Ledger consumes finalized only; indexer already deletes rows from dead slots |
| Nested calls through aggregators | Indexer already decodes inner instructions; journal builder keys on events, not top-level instructions |
| Bins grouped in arrays of 70 | State history splits arrays into per-bin rows |
| Position width up to 1,400 bins | Wide positions need chunked transactions; builder uses the SDK's chunked helpers |
| Limit order covers up to 50 bins | Ladder strategy splits orders across accounts |
| Pool is in limit-order mode or liquidity-mining mode | Onboarding check; strategy library filters by mode |
| Per-pool protocol share and collect-fee mode | Read from pool state at the slot; never constants |
| Token-2022 transfer fees and hooks | Journal uses actual token balance deltas from transaction metadata, not instruction amounts |
| Wrapped SOL | Wrap and unwrap are journaled as transfers between `wallet` sub-accounts, not as trades |
| Rent | Position and order rent are `rent_deposit` assets until refunded |
| Compute and size limits | Builder sets compute budget from simulation; uses address lookup tables |
| Nested call depth from the vault program | Vault to DLMM to token program and DLMM's self-call for events; verify depth and compute on devnet in Phase 4 week one |
| Program upgrades | IDL swap plus `decode_failures` alarm exists; accounting rules carry a `rule_version` tied to program version |

Field names for position fee counters, operator and fee-owner fields must be read from `idl/dlmm.json`, not from memory. Each phase spec lists what to confirm.

## 8. Security architecture

| Threat | Control |
| --- | --- |
| Operator key theft | On-chain limits; managed signer with instruction allow-list; client pause; rotation |
| Malicious or buggy strategy | Risk engine independent of strategy code; daily notional caps on-chain |
| Signer service asked to sign arbitrary transactions | Signer parses each transaction and accepts only vault program instructions for known mandates |
| Tenant data leak | Row-level security; API tests that attempt cross-tenant reads; separate staff role |
| Tampered statements | Statement JSON hashed; hash stored and printed; optional on-chain anchor of the hash |
| Price manipulation to trigger rebalances | Multi-source price check; halt on disagreement; time-weighted inputs |
| Supply-chain compromise | Locked dependencies; reproducible program build; verified build published |
| Insider misuse | Least privilege; audit log; two-person rule for mandate changes on our side |
| Database loss or corruption | Point-in-time recovery; ledger rebuild from raw; raw rebuild from chain |

## 9. Deployment and operations

| Environment | Chain | Purpose |
| --- | --- | --- |
| dev | Devnet | Daily development; free streaming |
| staging | Mainnet, read-only | Full data, no keys; soak tests and SDK parity runs |
| prod | Mainnet | Clients; executor present only from Phase 4 |

- Start with two application hosts and a managed Postgres with point-in-time recovery. The executor and signer run on a separate, locked-down host.
- Existing Prometheus and Grafana setup extends to every new service. New alert groups: ledger lag, reconciliation breaks, price staleness, intent stuck, halt active.
- Each service has a runbook: what it does, how it fails, how to restart, how to replay.

## 10. Testing strategy

| Layer | Method |
| --- | --- |
| Decoder | Existing golden fixtures from mainnet |
| Ledger rules | Fixture transactions with hand-checked journals; property test that every journal balances |
| Valuation and fees | Parity harness: a TypeScript script calls the Meteora SDK for sampled positions at a slot; Rust results must match exactly |
| PnL | Scripted scenarios on devnet with a spreadsheet of expected results |
| Determinism | Rebuild the ledger twice from raw and compare hashes |
| Simulator | Replay real pool history and compare with indexed state at checkpoints |
| Vault program | Unit and negative tests in a local validator harness; fuzzing of limits; external audit |
| Executor | Fault injection: dropped transactions, expired blockhash, stale price, indexer lag |
| End to end | Extend the existing `scripts/e2e.py` suite per phase |

## 11. Technology choices

| Concern | Choice | Note |
| --- | --- | --- |
| Core services | Rust, existing workspace | New crates per component |
| Database | Postgres 16 or later | Schemas per section 5; row-level security |
| Transaction building | TypeScript with `@meteora-ag/dlmm` | Pin the version; upgrade deliberately |
| On-chain program | Anchor | Verified build |
| Frontend | React, TypeScript, Vite | Reuse existing modules |
| PDF rendering | Headless browser rendering of an HTML template | Same template as the on-screen statement |
| Auth | Email plus passkeys; optional wallet sign-in for proof of control | Staff behind SSO |
| Jobs | Postgres queue with `FOR UPDATE SKIP LOCKED` | D12 |
| Observability | Prometheus, Grafana, structured logs | Existing |

## 12. Evolution to multiple venues

The adapter contract a venue must satisfy:

1. Decode its transactions into normalized flow events: deposit, withdraw, fee claim, reward claim, order place, order fill, order cancel.
2. Provide position inventory at a slot.
3. Provide accrued, unclaimed income at a slot.
4. Provide pool depth around price for the metrics engine.
5. Optionally, build transactions for a set of normalized intents.

Everything from the ledger downward depends only on this contract. Meteora-specific tables stay in `public` and `hist`; a second venue adds its own raw tables and an adapter crate.
