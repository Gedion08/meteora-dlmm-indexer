# Engineering conventions

These rules apply to every phase. Put them in the repository's agent instructions file so every coding session starts with them.

## Repository layout

```text
crates/
  dlmm-indexer/        existing: stream, decode, write, repair
  dlmm-api/            existing: read-only chain API
  bl-track/            tracked set, hydration, backfill jobs
  bl-hist/             account version writer and readers
  bl-prices/           price sources, canonical price resolution
  bl-dlmm-math/        pure DLMM math: bin price, position amounts, fees, swap step
  bl-ledger/           journals, inventory, lots, PnL, close, reconciliation
  bl-metrics/          liquidity health metrics
  bl-api/              tenant reporting API
  bl-reports/          statement and evidence pack assembly
  bl-alerts/           rule evaluation and delivery
  bl-sim/              pool simulator and backtester
  bl-strategy/         strategy trait and library
  bl-risk/             pre-trade checks and halts
  bl-executor/         intent manager, sender, confirmer
programs/
  policy-vault/        Anchor program
services/
  tx-builder/          TypeScript, Meteora SDK, builds and simulates transactions
tools/
  parity/              TypeScript harness producing SDK reference values
web/                   existing explorer frontend
app/                   client reporting app
migrations/            one ordered stream, schema-prefixed files
docs/                  these specs as Markdown
fixtures/              existing decoder fixtures plus ledger scenarios
```

Adjust crate names to match what already exists in `crates/`. Do not rename existing crates.

## Non-negotiable rules

1. **No floating point for token amounts or shares.** Use `u64`, `u128`, or a big integer in Rust, `NUMERIC` in Postgres, `bigint` or strings in TypeScript.
2. **USD values are derived.** Store the price reference beside every USD figure.
3. **Match the program's rounding.** Where the program floors, floor. Prove it with the parity harness.
4. **Read field names from `idl/dlmm.json`.** Never assume an account layout from memory or from this document.
5. **Pure cores, thin shells.** Math, journal rules, strategies and risk checks are pure functions. Database and network code wrap them.
6. **Idempotent writes.** Every table has a natural key. Workers commit their cursor in the same transaction as their output.
7. **Finalized only** in `ledger`, `metrics` daily tables, and anything a statement reads.
8. **Rule versions.** Any change to an accounting rule bumps `rule_version` and is replayable.
9. **No secrets in the repository.** Operator keys never touch application code paths outside the signer.
10. **Tenant isolation is enforced in the database**, and tested, not only in the API layer.

## Definition of done for a work package

- [ ] Code compiles with no warnings under the workspace lint settings.
- [ ] Unit tests for pure logic; integration test against a real Postgres.
- [ ] Acceptance criteria in the phase spec each map to a named test.
- [ ] Migration is forward-only and runs on a copy of staging data.
- [ ] Metrics and logs added; alert rule added if the package can fail silently.
- [ ] Runbook section written or updated.
- [ ] The end-to-end suite still passes.

## How to hand a work package to a coding agent

Give the agent three things: this conventions file, the relevant phase spec, and the work package ID. A prompt that works:

```text
Read docs/conventions.md and docs/phase-1-ledger.md.
Implement work package P1-WP3 only.
Before writing code:
  1. List the files you will create or change.
  2. List each acceptance criterion and the test that will prove it.
  3. List anything you need to confirm in idl/dlmm.json and confirm it.
Then implement, run the tests, and report which criteria pass.
Do not change files outside the package's scope. If the spec is wrong or
ambiguous, stop and say so instead of guessing.
```

Review order: read the tests first, then the pure logic, then the database code. If the tests do not encode the acceptance criteria, reject the work before reading further.

## Coding standards

| Topic | Rule |
| --- | --- |
| Errors | Typed errors in libraries; context added at boundaries; no panics outside tests |
| Time | Slots are the clock for chain logic; `block_time` for display and price lookup; UTC everywhere |
| IDs | Pubkeys as base58 text in SQL, typed wrappers in Rust |
| Amounts in JSON | Strings for anything that can exceed 2^53 |
| SQL | Explicit column lists; no `SELECT *` in application code; every query has a statement timeout |
| Migrations | One purpose per file; never edit an applied migration |
| Logging | Structured; include `entity_id`, `signature`, `slot` where relevant |
| Feature flags | New strategies and execution modes ship dark behind a per-mandate flag |
| Dependencies | Pin the Meteora SDK and Anchor versions; upgrade in a dedicated change with parity runs |

## Glossary

| Term | Meaning |
| --- | --- |
| Tracked set | Wallets, positions, orders and pools we keep versioned history for |
| Watermark | The highest finalized slot the indexer has fully committed |
| Journal | A balanced set of ledger entries for one on-chain transaction or one derived event |
| Inventory | Token amounts held inside a position or order at a slot |
| Conversion | A derived journal recording a position's composition change between two slots |
| Lot | A quantity of a token acquired at a time and cost |
| Close | An immutable daily snapshot of an entity's ledger state |
| Mandate | The client's policy for a managed pool, off-chain document plus on-chain account |
| Intent | A strategy's requested action, before it becomes a transaction |
| Halt | A state in which the executor sends nothing for a mandate or globally |
