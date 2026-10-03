# Phase 3 build spec: strategy and backtest

Goal: we can show an issuer, on its own pool's history, what a mandate would have earned and cost, and then issue recommendations the client signs. Seven weeks. No keys on our side in this phase.

## Scope

| In | Out |
| --- | --- |
| Deterministic pool simulator | Our own signing or sending |
| Strategy trait and intent model | Vault program |
| Three strategies | Machine-learned strategies |
| Backtest report using ledger code | Cross-venue strategies |
| Advisory mode: recommendation, unsigned transaction, outcome matching | Automatic execution |
| Mandate document format (off-chain) | On-chain mandate account |

## Confirm before coding

- [ ] The swap step: how input is consumed within a bin, how the active bin moves, and how fees are taken, read from the SDK's quote code and Meteora's formulas page.
- [ ] The dynamic fee: base fee, variable fee, volatility accumulator and its decay parameters, and where they live in pool state.
- [ ] How limit-order liquidity is consumed relative to maker liquidity in the same bin.
- [ ] The current maximum bins crossed per swap instruction (documentation states 260 after the limit-order release).

## Simulator

Two modes share one core.

| Mode | Input | Use |
| --- | --- | --- |
| Replay | Starting bin state from `hist`, then indexed swaps and liquidity events | Proves the core against reality |
| Counterfactual | Same, plus hypothetical positions and orders from a strategy | Backtests |

Core types:

```rust
pub struct PoolSim { pub pair: PairParams, pub active_id: i32, pub bins: BTreeMap<i32, SimBin>, pub vol: VolState, pub clock: SimClock }
pub struct SimBin { pub x: u128, pub y: u128, pub supply: u128, pub fee_x_per_share: U256, pub fee_y_per_share: U256, pub orders: OrderBook }

impl PoolSim {
    pub fn swap(&mut self, input: SwapInput) -> SwapOutcome;        // bin walk with fees
    pub fn add_liquidity(&mut self, owner: SimOwner, dist: &[BinDeposit]) -> PositionId;
    pub fn remove_liquidity(&mut self, pos: PositionId, bps_per_bin: &[(i32, u16)]) -> Amounts;
    pub fn place_order(&mut self, owner: SimOwner, side: Side, bins: &[(i32, u128)]) -> OrderId;
    pub fn cancel_order(&mut self, id: OrderId) -> Amounts;
    pub fn quote(&self, input: SwapInput) -> SwapOutcome;           // no mutation; used by the slippage metric
}
```

Counterfactual assumption, stated on every backtest report: historical swappers are assumed to submit the same input amounts at the same times. Added liquidity changes their execution price and our fill, but not their decision to trade. This overstates results in thin pools where our liquidity would have attracted or repelled flow; the report says so.

Acceptance (SS-1): in replay mode over seven days on five real pools, active bin and per-bin reserves equal `hist` at every hourly checkpoint; total LP fees equal indexed fees.

## Strategy interface

```rust
pub trait Strategy {
    fn id(&self) -> &'static str;
    fn params_schema(&self) -> serde_json::Value;
    fn decide(&self, ctx: &StrategyCtx) -> Vec<Intent>;   // pure; no I/O, no clock, no randomness
}

pub struct StrategyCtx<'a> {
    pub now_slot: u64, pub now_ts: i64,
    pub pool: &'a PoolView,            // active bin, bins around it, fee state
    pub reference_price: Option<Price>,
    pub holdings: &'a Holdings,        // free balances, positions, orders
    pub mandate: &'a Mandate,
    pub params: &'a serde_json::Value,
    pub last_actions: &'a [PastAction],
}

pub enum Intent {
    AddLiquidity   { pair: Pubkey, dist: Vec<BinDeposit>, reason: Reason },
    RemoveLiquidity{ position: Pubkey, bins: Vec<(i32, u16)>, reason: Reason },
    ClaimFees      { position: Pubkey, reason: Reason },
    PlaceOrder     { pair: Pubkey, side: Side, bins: Vec<(i32, u128)>, reason: Reason },
    CancelOrder    { order: Pubkey, bins: Vec<i32>, reason: Reason },
    ClosePosition  { position: Pubkey, reason: Reason },
    Hold           { reason: Reason },
}
```

`Reason` is a structured value (code plus the numbers that triggered it). It is shown to the client in recommendations and mandate reports.

Acceptance (SS-2): property test that `decide` returns identical intents for identical context; no strategy crate depends on database or network crates.

## Mandate document

A JSON document, versioned and hashed. Phase 4 puts the hash and the enforceable subset on-chain.

| Field | Meaning |
| --- | --- |
| pools | Allowed pool addresses |
| capital\_cap | Maximum tokens under management per mint |
| bin\_bounds | Lowest and highest bin ID per pool |
| allowed\_actions | Subset of the intent kinds |
| max\_notional\_per\_tx, per\_day | In quote-token units |
| min\_rebalance\_interval | Seconds between liquidity-moving actions |
| inventory\_band | Target share of base token and tolerance |
| depth\_targets | Depth at given distances, per side |
| halt\_conditions | Price deviation, peg deviation, pool mode change, stale data |
| benchmark | Hold basket, or a static range defined here |
| fee\_terms | Retainer and performance fee definition |

## Strategy library v1

| Strategy | For | Logic | Key parameters |
| --- | --- | --- | --- |
| Peg-keeper | Stable pairs and local-currency stablecoins | Keep a tight band of liquidity around the reference price; widen on volatility; pull the depegging side when deviation persists beyond a threshold | Band width in bins, depth targets, deviation threshold and duration, inventory band |
| Recentering range | Treasury pairs against SOL or USDC | Shaped distribution around the active bin; recentre when price leaves the inner zone for a set time, only if expected fee gain exceeds rebalance cost and realized divergence | Shape, total width, inner zone, dwell time, cost hurdle |
| Treasury ladder | Scheduled buying or selling by a treasury | Limit orders laddered across bins on one side; refill as they fill; cap daily notional | Side, total size, price range, rungs, refill rule, daily cap |

The treasury ladder is where OrderFlow's logic lands. Port its order scheduling and take-profit rules into `decide` as pure code; drop its own sending code in favour of intents.

No strategy uses swaps. Inventory is adjusted by one-sided liquidity and limit orders, which matches what the vault program will allow in Phase 4.

## Backtest report

The backtester runs the simulator in counterfactual mode, feeds simulated events into the real `bl-ledger` code against a temporary schema, and reads results with the real PnL engine. One code path for backtest and production accounting.

Report contents: PnL decomposition against the mandate's benchmark, fee return and divergence side by side, turnover, number and cost of rebalances, time in range, depth achieved against targets, worst day, sensitivity to the two most important parameters, and the stated assumptions.

Acceptance (SS-3, SS-4): each strategy has a report on three real pools; a backtest of "do nothing" on a real tracked position reproduces that position's actual ledger PnL.

## Advisory mode

1. A scheduled run evaluates each advisory mandate and stores intents in `exec.intents` with status `recommended`.
2. The `tx-builder` service turns each intent into unsigned transactions using the Meteora SDK, simulates them, and returns expected token deltas. Each transaction carries a memo with the intent ID.
3. The client sees the recommendation in the app: what, why (the reason), expected effect, cost, and expiry.
4. The client signs with a connected wallet, or exports the transaction for a multisig.
5. When the indexer sees a finalized transaction with that memo, the intent becomes `executed` and the ledger outcome is compared with the expectation.
6. Unsigned recommendations expire; the next run re-evaluates from fresh state.

```sql
CREATE SCHEMA exec;
CREATE TABLE exec.mandates (
  mandate_id UUID PRIMARY KEY, entity_id UUID NOT NULL, version INT NOT NULL,
  mode TEXT NOT NULL,                 -- advisory, cosigned, delegated
  doc JSONB NOT NULL, doc_hash TEXT NOT NULL, status TEXT NOT NULL,
  strategy_id TEXT NOT NULL, strategy_params JSONB NOT NULL
);
CREATE TABLE exec.strategy_runs (
  run_id UUID PRIMARY KEY, mandate_id UUID NOT NULL, at_slot BIGINT NOT NULL,
  ctx_hash TEXT NOT NULL, created_at TIMESTAMPTZ NOT NULL DEFAULT now()
);
CREATE TABLE exec.intents (
  intent_id TEXT PRIMARY KEY,         -- hash of mandate, run and content
  run_id UUID NOT NULL, mandate_id UUID NOT NULL, kind TEXT NOT NULL,
  body JSONB NOT NULL, reason JSONB NOT NULL, expected JSONB,
  status TEXT NOT NULL, expires_slot BIGINT, signature TEXT, outcome JSONB
);
```

Acceptance (SS-5): on devnet, a recommendation is built, signed by a test wallet, landed, matched by memo, and its outcome recorded with deltas within tolerance of the expectation.

## Work packages

| ID | Package | Week |
| --- | --- | --- |
| P3-WP1 | Simulator core: bins, swap walk, fees | 1 to 2 |
| P3-WP2 | Replay harness and checkpoint tests; limit-order consumption | 2 to 3 |
| P3-WP3 | Move slippage metric onto `quote` | 3 |
| P3-WP4 | Strategy trait, context builder, mandate document | 4 |
| P3-WP5 | Peg-keeper, recentering range, treasury ladder | 4 to 5 |
| P3-WP6 | Backtester through ledger code; report | 6 |
| P3-WP7 | `tx-builder` service (build and simulate only) | 7 |
| P3-WP8 | Advisory flow in API and app; memo matching | 7 |

## Exit gate

- [ ] Replay acceptance passes on five pools.
- [ ] Three strategies with backtest reports reviewed by a design partner.
- [ ] Advisory flow proven end to end on devnet and once on mainnet with our own wallet.
- [ ] One advisory mandate signed by a client.
- [ ] Legal opinion on co-signed and delegated modes received.
