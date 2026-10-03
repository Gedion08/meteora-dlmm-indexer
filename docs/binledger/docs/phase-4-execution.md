# Phase 4 build spec: managed execution

Goal: run a client mandate with our operator key bounded by an audited on-chain policy, where the worst outcome of any failure on our side is liquidity left in place inside the client's own limits. Twelve weeks including the audit. Do not start before the legal opinion from Phase 3.

## Scope

| In | Out |
| --- | --- |
| Policy vault program (Anchor), tests, audit | Operator swaps |
| Executor: intent manager, builder, signer, sender, confirmer | Pooled or shared vaults |
| Risk engine, halts, kill switch | Cross-venue execution |
| Co-signed mode through the client's multisig | Automated fee collection on-chain |
| Mandate report | Leverage or borrowing of any kind |
| Staged mainnet rollout |  |

## Week-one spikes (answer before building)

- [ ] Can a program-derived address act as position owner, token owner and rent payer in every DLMM instruction we need, when called from our program?
- [ ] Call depth and compute for vault to DLMM add-liquidity, remove-liquidity, claim, place and cancel limit order, including DLMM's self-call for events. Measure on devnet.
- [ ] Account counts per instruction with bin arrays and bitmap extension; confirm transactions fit with address lookup tables.
- [ ] Do DLMM's own position operator and fee-owner fields offer a simpler, safe delegation path? Read the program source; do not rely on field names alone.
- [ ] Which multisig the first client uses, and whether a member can be limited to proposing.

If a spike fails, stop and redesign before writing program code.

## Policy vault program

### Accounts

```rust
#[account]
pub struct Mandate {
    pub client: Pubkey,              // sole authority for policy, withdrawals, operator rotation
    pub operator: Pubkey,            // our bounded key
    pub mandate_id: [u8; 16],
    pub mandate_hash: [u8; 32],      // hash of the off-chain mandate document
    pub vault_bump: u8,
    pub paused: bool,
    pub policy: Policy,
    pub usage: Usage,
}

pub struct Policy {
    pub pool_count: u8,
    pub pools: [PoolPolicy; 4],
    pub allowed_actions: u16,        // bitmask over operator instructions
    pub min_action_interval_slots: u64,
    pub mint_caps: [MintCap; 8],     // per-transaction and per-day caps in raw units
}

pub struct PoolPolicy {
    pub lb_pair: Pubkey,
    pub min_bin_id: i32,
    pub max_bin_id: i32,
    pub max_positions: u8,
    pub max_orders: u8,
    pub order_sides: u8,             // bit 0 bids allowed, bit 1 asks allowed
}

pub struct MintCap { pub mint: Pubkey, pub per_tx: u64, pub per_day: u64 }
pub struct Usage  { pub day_index: u64, pub used_today: [u64; 8], pub last_action_slot: u64 }
```

Seeds: mandate `[b"mandate", client, mandate_id]`; vault authority `[b"vault", mandate]`. The vault authority owns the token accounts and a SOL float for rent, and is the owner of every position and limit order.

### Invariants (the audit's checklist)

1. Only `client` can withdraw, change policy, rotate the operator, or unpause.
2. No operator instruction has a token destination other than a token account owned by the vault authority.
3. Every operator instruction checks: not paused, action allowed, pool listed, bins within bounds, interval elapsed, caps not exceeded.
4. The DLMM program ID is a constant; no arbitrary program can be called.
5. Every account passed to DLMM is validated: pool matches policy, position owner is the vault authority, token accounts are the vault's for the pool's mints.
6. Caps reset by day index derived from the clock sysvar; arithmetic is checked.
7. Pausing never blocks the client's withdrawal path.
8. Closing a position or order returns rent to the vault's SOL account or to the client.
9. The program is either immutable after audit or upgradable only by a client-visible multisig with a timelock; decide and state it in the mandate.

### Why no swaps

A swap lets a key holder move value to a counterparty through price. Adding liquidity cannot sell below the current price, because DLMM only accepts the base token above the active bin and the quote token below it. With bin bounds set by the client, the operator's worst action is placing liquidity at the least favourable allowed bins. That loss is bounded and visible; a swap's is not.

Optional guard for pools with an oracle: reject liquidity-moving instructions when the pool's active price differs from the oracle price by more than a policy threshold. This limits abuse in thin pools where the pool price itself can be pushed.

### Events and errors

Emit an event per operator action with mandate, action, pool, bins, amounts and remaining daily caps. Give every check its own error code. The indexer decodes these events with the same IDL-driven decoder by adding the vault IDL.

### Tests

| Kind | Coverage |
| --- | --- |
| Positive | Each instruction against a local DLMM deployment or mainnet fork |
| Negative | One test per invariant and per error code: wrong signer, wrong pool, bins out of bounds, cap exceeded, interval not elapsed, paused, foreign token account, substituted program ID |
| Property | Random sequences of operator actions never reduce what the client can withdraw below vault holdings plus position value |
| Upgrade | DLMM IDL change detection: instruction discriminators compared at startup |

## Executor

### Intent states

| State | Meaning | Next |
| --- | --- | --- |
| created | Produced by a strategy run | approved, rejected, deferred |
| approved | Passed all risk checks | built |
| rejected | Failed a check; reason stored | terminal |
| deferred | A soft check failed (cooldown, fee budget); retry on next run | created by a later run |
| built | Transactions built and simulated; deltas match expectation | signed |
| signed | Signed by the signer service | sent |
| sent | Submitted; rebroadcasting | landed, expired |
| landed | Seen at confirmed commitment | done, failed |
| expired | Blockhash expired without landing | terminal; strategy re-evaluates |
| failed | On-chain error | terminal; counts toward halt threshold |
| done | Finalized and journaled by the ledger; outcome recorded | terminal |

Rules: the intent ID is deterministic, so a restart cannot create a duplicate. A new transaction for the same intent is built only after the previous blockhash has expired. Multi-transaction intents (wide positions) are sequenced with each step idempotent.

### Components

| Component | Detail |
| --- | --- |
| Intent manager (`bl-executor`) | State machine over `exec.intents`; one worker per mandate to keep ordering |
| Builder (`services/tx-builder`) | Builds vault-program instructions that wrap the SDK-resolved DLMM accounts; sets compute budget from simulation; adds a memo with the intent ID |
| Signer | Separate process and host. Receives a transaction, parses it, and signs only if every instruction is compute budget, memo, or a vault-program operator instruction for a known mandate. Key in a managed or hardware-backed store with ed25519 support |
| Sender | Priority fee from recent fee data with a per-mandate budget; submits to more than one RPC; rebroadcasts until landed or expired |
| Confirmer | Listens for the ledger's journal of the signature; compares realized deltas with expected; writes outcome |

```sql
CREATE TABLE exec.outbound_txs (
  tx_id BIGSERIAL PRIMARY KEY, intent_id TEXT NOT NULL REFERENCES exec.intents,
  step INT NOT NULL, signature TEXT UNIQUE, blockhash TEXT NOT NULL, last_valid_height BIGINT NOT NULL,
  priority_fee BIGINT, status TEXT NOT NULL, sim JSONB, error JSONB, sent_at TIMESTAMPTZ
);
CREATE TABLE exec.risk_events (
  id BIGSERIAL PRIMARY KEY, intent_id TEXT, mandate_id UUID, check_id TEXT NOT NULL,
  result TEXT NOT NULL, inputs JSONB NOT NULL, at_slot BIGINT NOT NULL
);
CREATE TABLE exec.halts (
  halt_id BIGSERIAL PRIMARY KEY, scope TEXT NOT NULL,   -- global or mandate
  mandate_id UUID, reason TEXT NOT NULL, raised_by TEXT NOT NULL,
  raised_at TIMESTAMPTZ NOT NULL DEFAULT now(), cleared_at TIMESTAMPTZ, cleared_by TEXT
);
```

## Risk engine

### Pre-trade checks

| Check | Rejects when |
| --- | --- |
| Mandate status | Not active, paused on-chain, or doc hash differs from on-chain hash |
| Policy mirror | Intent violates any on-chain limit (checked off-chain first to avoid failed transactions) |
| Data freshness | Indexer watermark lag or price age above threshold |
| Price sanity | Pool price differs from reference by more than threshold; price flagged disputed |
| Pool integrity | Pool function mode, bin step or fee parameters changed since the mandate was signed |
| Simulation match | Simulated token deltas differ from expected beyond tolerance |
| Cost | Priority fee above budget; expected benefit below cost hurdle |
| Concentration | Action would put more than the mandate's maximum share in one bin |
| Duplicate | An open intent already covers the same position and bins |

### Halt conditions

| Condition | Scope | Cleared by |
| --- | --- | --- |
| Indexer lag above threshold for N minutes | Global | Automatic on recovery |
| Reconciliation break on a managed entity | Mandate | Human |
| Three failed or expired intents in a row | Mandate | Human |
| Peg or price deviation beyond the mandate's halt threshold | Mandate | Per mandate: automatic or human |
| DLMM program upgrade detected or new decode failures | Global | Human, after parity run |
| Signer or builder unhealthy | Global | Automatic on recovery |
| Manual kill switch (us or client) | Either | Whoever raised it |

A halt stops sending. It does not unwind positions; unwinding is a decision for the client or an explicit mandate rule.

## Co-signed mode

- The vault's `client` authority, or the client's own wallets without a vault, is a multisig.
- Our key is a multisig member limited to proposing, where the multisig supports that.
- The executor builds the same transactions, submits them as proposals, and the intent waits in `sent` until approved and executed or expired.
- Suits slow strategies: weekly recentering and treasury ladders. Not suitable for peg-keeping.

## Mandate report (ME-7)

Monthly, built like a statement from closes: every operator action with intent, reason, cost and outcome versus expectation; PnL decomposition versus the benchmark; depth and spread achieved against targets; halts and their duration; policy changes; performance fee calculation with its inputs.

## Work packages

| ID | Package | Weeks |
| --- | --- | --- |
| P4-WP1 | Spikes | 1 |
| P4-WP2 | Vault program: accounts, client instructions | 1 to 2 |
| P4-WP3 | Vault program: operator instructions and checks | 2 to 4 |
| P4-WP4 | Program tests: positive, negative, property | 3 to 5 |
| P4-WP5 | Indexer: vault IDL decoding; ledger rules for vault-owned positions | 4 |
| P4-WP6 | Intent manager and state machine | 3 to 5 |
| P4-WP7 | Builder for vault instructions; signer service | 4 to 6 |
| P4-WP8 | Sender and confirmer | 5 to 6 |
| P4-WP9 | Risk engine and halts; kill switch in app and on-chain | 5 to 7 |
| P4-WP10 | Fault-injection suite | 7 |
| P4-WP11 | Co-signed mode | 7 to 8 |
| P4-WP12 | Mandate report | 8 |
| P4-WP13 | Audit preparation, audit, fixes, verified build | 8 to 11 |
| P4-WP14 | Staged rollout | 12 |

Ledger note for WP5: positions are owned by the vault authority, not the client wallet. Add the vault authority and its token accounts to the entity's tracked wallets, with kind `vault`, so journals and reconciliation work unchanged.

## Staged rollout

1. Devnet, full flow, two weeks of continuous running with fault injection.
2. Mainnet, our own funds, small caps, one pool.
3. Mainnet, our own funds, realistic caps, two weeks without a human-cleared halt.
4. First client mandate with low caps agreed in writing.
5. Caps raised in steps, each after a clean mandate report.

## Exit gate

- [ ] Audit complete; no open high or critical findings; report published.
- [ ] Every invariant has a passing negative test.
- [ ] Fault-injection suite passes: every injected fault ends in a halt or a clean terminal state, never a duplicate send.
- [ ] Kill switch tested by the client on devnet.
- [ ] Signed mandate contract; legal sign-off on the mode offered.
- [ ] One delegated mandate live with capped capital and a first mandate report delivered.
