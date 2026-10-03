# Delivery plan

One builder working with coding agents can reach paying reporting clients in about five months and a first delegated mandate in about eleven. The plan holds only if each phase gate is respected and execution work does not start early.

## How the work is organised

- Each phase has a build spec tab under this one. A spec is the unit handed to a coding agent, one work package at a time.
- Every work package ends with tests that prove its acceptance criteria. No package is done on a demo.
- Phase gates are pass or fail. A failed gate extends the phase; it does not move work into the next one.
- Commercial, legal and security tracks run beside engineering from the first week.

## Phases

| Phase | Duration | Calendar | Engineering deliverables | Exit gate | Parallel non-engineering work |
| --- | --- | --- | --- | --- | --- |
| 0. Mainnet foundation | 4 weeks | Oct to Nov 2026 | Mainnet indexer, tracked set, state history, price service, wallet backfill | 7 days on mainnet with no open gaps; any tracked position valued at any slot since tracking | List 15 target issuers; start 5 conversations |
| 1. Position ledger | 8 weeks | Nov 2026 to Jan 2027 | Math crate, journal builder, inventory, fees, lots, PnL, close, reconciliation | 200 sampled positions match the SDK exactly; PnL residual zero; rebuild is deterministic | Draft methodology paper; find reviewing accountant |
| 2. Reporting product | 7 weeks | Jan to Mar 2027 | Tenancy, API, app, statements, evidence pack, alerts, health metrics | 3 design partners using it weekly; statement equals API equals ledger | Onboard design partners; pricing tests; incorporate |
| 3. Strategy and backtest | 7 weeks | Mar to May 2027 | Simulator, strategy interface, three strategies, backtest reports, advisory mode | 1 signed advisory mandate; simulator reproduces a real week | Legal opinion on managed modes; choose auditor |
| 4. Managed execution | 12 weeks | May to Aug 2027 | Vault program, executor, signer, risk engine, co-signed mode, mandate report, audit | Audit passed; delegated mandate live with capped capital | Mandate contract template; insurance enquiry |
| 5. Scale and second venue | Ongoing | From Sep 2027 | Billing, operator console, security programme, second venue adapter | Second venue passes ledger acceptance tests | First hire; case study published |

## Week-level view

| Phase | Weeks | Focus |
| --- | --- | --- |
| 0 | 1 | Mainnet provider, new database, deploy indexer, soak; measure stream and storage volume |
| 0 | 2 | Tracked-set registry and account subscriptions; `hist` schema and writers |
| 0 | 3 | Price service; token registry hardening for Token-2022 |
| 0 | 4 | Per-wallet backfill; finalized watermark; gate soak |
| 1 | 1 to 2 | `bl-dlmm-math` crate and SDK parity harness: position amounts, bin price, claimable fees |
| 1 | 3 to 4 | Ledger schema, journal builder for flow events, WSOL and rent handling |
| 1 | 5 | Inventory tracker and conversion journals; limit-order accounting |
| 1 | 6 | Lots, relief methods, PnL decomposition |
| 1 | 7 | Daily close, restatement, reconciler |
| 1 | 8 | Determinism and parity gate runs; fix list |
| 2 | 1 | Tenancy, auth, row-level security, wallet onboarding |
| 2 | 2 to 3 | Reporting API and metrics engine |
| 2 | 4 to 5 | App: overview, pool, position, health, drill-down |
| 2 | 6 | Statements, evidence pack, exports |
| 2 | 7 | Alerts, API keys, partner onboarding fixes |
| 3 | 1 to 3 | Simulator with replay tests; slippage metric moved onto it |
| 3 | 4 to 5 | Strategy interface and three strategies |
| 3 | 6 | Backtest report from ledger code |
| 3 | 7 | Advisory mode: recommendation to unsigned transaction to matched outcome |
| 4 | 1 to 4 | Vault program and tests; call-depth and compute spike in week 1 |
| 4 | 3 to 6 | Executor, builder service, signer, sender, confirmer |
| 4 | 5 to 7 | Risk engine, halts, kill switch, fault injection |
| 4 | 7 to 8 | Co-signed mode; mandate report |
| 4 | 8 to 11 | External audit, fixes, re-review |
| 4 | 12 | Staged mainnet rollout: own funds, then capped client mandate |

## Decision gates

| When | Question | If no |
| --- | --- | --- |
| End of Phase 0 | Is mainnet data cost sustainable before revenue? | Filter stream to tracked pools only; revisit provider |
| End of Phase 1 | Do results match the SDK exactly? | Do not start the app; fix the math |
| Mid Phase 2 | Are at least 2 design partners engaged weekly? | Pause feature work; spend two weeks on customer discovery; consider funds as first segment |
| End of Phase 2 plus 8 weeks | Has anyone paid for reporting? | Re-examine pricing and segment before funding Phase 4 |
| Before Phase 4 | Does counsel support delegated mode in our structure? | Ship advisory and co-signed only; delay the vault program |
| Before client funds in a vault | Audit passed with no open high-severity findings? | No client funds; continue with own capital |

## Cost lines to budget

| Line | When it starts | Note |
| --- | --- | --- |
| Mainnet gRPC streaming and RPC | Phase 0 | The largest recurring cost early; get current quotes |
| Managed Postgres with point-in-time recovery | Phase 0 | Size after the week-1 volume measurement |
| Application hosts and monitoring | Phase 0 | Two hosts to start |
| Price data | Phase 0 | Free tiers may be enough at first |
| Legal: incorporation, terms, opinion on managed modes | Phases 2 to 3 | Lumpy |
| Accountant review of methodology | Phase 1 to 2 | One-off |
| Program audit | Phase 4 | The largest one-off cost; book early, auditors have queues |
| Managed signer or hardware security | Phase 4 | Required for delegated mode |

## First two weeks

- [ ] Choose a mainnet streaming provider and plan; create the mainnet database.
- [ ] Deploy the indexer to mainnet in staging and start the soak.
- [ ] Measure messages per second, bytes per day, and row growth for one week.
- [ ] Copy these specs into the repository under `docs/` and add the conventions to the agent instructions file.
- [ ] Confirm against `idl/dlmm.json` the fields listed in the Phase 0 and Phase 1 specs.
- [ ] Write the list of 15 target issuers with their pools and wallet addresses.
- [ ] Open the OrderFlow test wallets as the first tracked entity.
- [ ] Book two conversations with accountants who work with crypto clients.
