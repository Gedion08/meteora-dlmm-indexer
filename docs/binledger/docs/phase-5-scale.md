# Phase 5 build spec: scale and second venue

Goal: the service can be run by someone other than its builder, invoices itself from the ledger, and no longer depends on a single venue. Ongoing from September 2027; packages are ordered by priority, not by calendar.

## Scope

| In | Out |
| --- | --- |
| Billing from ledger data | Becoming a custodian or exchange |
| Operator console and runbooks | Retail products |
| Security programme and incident response | Proprietary trading with client data |
| Venue adapter interface and a second venue | More than one new venue at a time |
| Performance work driven by measurements | Speculative re-architecture |
| Reporting currencies |  |

## Work packages

| ID | Package | Trigger to start |
| --- | --- | --- |
| P5-WP1 | Billing and invoicing | Second paying client |
| P5-WP2 | Operator console | First delegated mandate live |
| P5-WP3 | Security programme | Before the second delegated mandate |
| P5-WP4 | Venue adapter interface | A client or prospect names a second venue |
| P5-WP5 | Second venue adapter | WP4 done |
| P5-WP6 | Reporting currencies | A client asks |
| P5-WP7 | Storage and query scaling | A measured threshold below is crossed |
| P5-WP8 | Strategy library v2 | Mandate reports show a recurring gap |

### P5-WP1 Billing

- Plans and subscriptions per org in `app`; invoices generated monthly.
- Performance fee per mandate: fees earned above the mandate's benchmark for the period, from `ledger.closes`, times the agreed rate, with a high-water mark if the mandate says so.
- The invoice links to the mandate report section showing the calculation.
- Payment is off-chain or by a client-initiated transfer. Nothing is deducted inside the vault.

Acceptance (OC-1): every invoice line traces to close rows; recomputing an old invoice gives the same figure.

### P5-WP2 Operator console

| View | Content |
| --- | --- |
| Mandates | Status, mode, caps used today, health versus targets, last action |
| Intents | Live state machine view, with risk results and transaction links |
| Halts | Active halts, reason, who can clear, clear action with a required note |
| Breaks | Reconciliation breaks and price flags, with resolution workflow |
| Clients | Onboarding state, wallets, proofs, close status, statements issued |
| System | Indexer lag, ledger lag, price freshness, signer and builder health |

Every staff action writes to the audit log. Clearing a halt and changing a strategy parameter need a second staff approval once there are two staff.

Acceptance (OC-2): a second person runs the service for a week from the runbooks with the builder unavailable.

### P5-WP3 Security programme

- Key management policy: generation, storage, rotation, and who can request a signature.
- Access reviews each quarter; offboarding checklist.
- Incident response plan with severity levels, client notification times, and a contact tree.
- Tabletop exercises: operator key suspected compromised; wrong statement issued; indexer corruption; vault program vulnerability disclosed.
- Dependency and container scanning in CI; bug bounty for the vault program.
- Backup restore drill each quarter, timed against the recovery targets.

Acceptance (OC-3): one tabletop completed with findings closed; one timed restore drill within target.

### P5-WP4 Venue adapter interface

```rust
pub trait VenueAdapter {
    fn venue_id(&self) -> &'static str;
    /// Normalized flow events for one finalized transaction.
    fn flows(&self, tx: &RawTx) -> Vec<FlowEvent>;
    /// Token inventory of a position or order at a slot.
    fn inventory_at(&self, r: &PositionRef, slot: u64) -> Result<Inventory>;
    /// Accrued, unclaimed income at a slot.
    fn accrued_at(&self, r: &PositionRef, slot: u64) -> Result<Accrued>;
    /// Depth around price for the metrics engine.
    fn depth_at(&self, pool: &PoolRef, slot: u64, distances_bps: &[u32]) -> Result<Depth>;
    /// Authority map for evidence packs.
    fn authorities(&self, r: &PositionRef, slot: u64) -> Result<Authorities>;
}
```

- Refactor `bl-ledger` and `bl-metrics` to call the trait; the Meteora code moves behind `MeteoraDlmmAdapter` with no behaviour change.
- Proof of no change: ledger hash for every entity is identical before and after the refactor.

### P5-WP5 Second venue

Choose by client demand. Likely candidates are another Solana concentrated-liquidity venue (smallest infrastructure change) or an EVM venue (new indexer, larger market of regulated issuers). Reporting comes first; execution on the new venue is a separate later decision.

Acceptance (OC-4): the Phase 1 acceptance suite, adapted, passes for the second venue: inventory parity with that venue's SDK, zero PnL residual, deterministic rebuild.

### P5-WP6 Reporting currencies

Add an FX rate table with provenance and a per-entity reporting currency. USD stays the ledger's valuation currency; conversion happens at report time using the rate at each transaction or period end, per the entity's policy.

### P5-WP7 Scaling triggers

| Measurement | Threshold | Response |
| --- | --- | --- |
| Dashboard p95 query time | Above 500 ms for a week | Add summary tables or continuous aggregates |
| `hist.bin_versions` size | Above what the primary handles comfortably | Partition by pool and month; move cold partitions to cheaper storage |
| Close duration per entity | Above 10 minutes | Incremental valuation; parallelise by position |
| Ledger rebuild time | Above 6 hours for the largest entity | Checkpointed rebuild from the last unchanged close |
| Analytics queries hurting the primary | Any incident | Read replica; then a column store for pool analytics |
| Stream cost | Above budget | Filter to tracked pools; second provider for redundancy only |

### P5-WP8 Strategy library v2

Candidates, each requiring a backtest report and a design-partner review before live use: volatility-aware band sizing, inventory skew by treasury schedule, multi-pool allocation for one issuer, and event-aware pauses around scheduled unlocks.

## Company-level work in this phase

- First hire: an operator who can also do client onboarding, so the builder is not the only person who can respond to a halt.
- Publish the methodology paper, the audit report and one mandate case study.
- Review pricing against twelve months of support hours and data costs.
- Decide whether to seek an external control report once clients ask for it.

## Review points

- [ ] Is reporting-to-managed conversion at or above one in four?
- [ ] Has any reconciliation break reached a client?
- [ ] What share of revenue depends on Meteora alone?
- [ ] Can the service run for two weeks without the builder?
