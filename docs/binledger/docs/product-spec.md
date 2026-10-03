# Product Specification Document

Oct 3, 2026 · @Gideon

## 1. Summary and thesis

BinLedger (working name) is a liquidity operations company for token issuers on Meteora DLMM. It sells two products that share one engine: audit-grade reporting on DLMM positions, and non-custodial managed liquidity.

**The thesis in four lines**

- Meteora keeps absorbing tooling built on top of it (limit orders, auto vaults), so a nicer DLMM interface is not a company.
- Issuers with real flow (stablecoins, tokenized assets, token treasuries) need accountability, bespoke strategy and reporting that a protocol does not provide.
- Reporting is the wedge: read-only, no custody, low price, fast to adopt. It puts us inside the client's numbers.
- Managed liquidity is the revenue: once reports show idle capital or rebalancing losses, the client asks us to run it.

**What we build**

| Layer | What it is | Who uses it |
| --- | --- | --- |
| Indexer | Complete, replayable record of every DLMM instruction and event for tracked pools and wallets | Internal |
| Position ledger | Double-entry, lot-level accounting of every position, bin and limit order | Internal, auditors |
| Reporting (Product A) | Dashboards, statements, evidence packs, API | Issuer finance and treasury teams |
| Managed liquidity (Product B) | Strategy engine plus execution under on-chain policy limits | Issuer treasury, token teams |
| Client operations | Onboarding, mandates, SLAs, billing, incident handling | Both sides |

**Design rules that hold across every phase**

1. The ledger is the source of truth. Reports read from it; execution writes intents that the indexer later confirms into it.
2. Every reported number can be traced to transaction signatures.
3. We never hold withdrawal authority over client funds.
4. Meteora is the first venue adapter, not the data model.

This pack has one tab per document: this product spec, the architecture design, a delivery plan, a shared engineering conventions file, and one build spec per phase written to be handed to a coding agent.

## 2. Problem and market context

Issuers on Meteora DLMM cannot answer three basic questions about their own liquidity: what did it earn, what did it cost, and is it doing its job.

**Why the questions are hard**

- A DLMM position changes composition every time price crosses one of its bins, and the program emits no per-position event for that. Value at a past moment has to be rebuilt from bin state.
- Fees accrue per bin and stay unclaimed until collected, so wallet balances understate income.
- Limit orders are separate accounts with their own fill and fee rules, covering up to 50 bins each.
- Finance teams need cost basis, realized and unrealized gains, and a trail to transaction signatures. Explorer screenshots do not pass an audit.

**Facts about the venue that shape the product**

| Fact | Consequence for us | Source |
| --- | --- | --- |
| Limit orders became native in DLMM in May 2026, with SDK functions to place and cancel them | Order tooling alone is not defensible; we treat limit orders as one instrument inside a mandate | [Meteora changelog](https://docs.meteora.ag/developer-guides/dlmm/changelog) |
| A pool runs in either limit-order mode or liquidity-mining mode, not both | Pool mode is an onboarding check and a strategy input | [What is DLMM](https://docs.meteora.ag/core-products/dlmm/what-is-dlmm) |
| 50% of the limit-order portion of fees goes to limit-order participants | Fee attribution must split maker and limit-order income | [DLMM Limit Order](https://docs.meteora.ag/core-products/dlmm/limit-order) |
| Protocol share is set per pool, and the protocol share of LP fees rose from 5% to 10% in May 2026 | Never hard-code fee splits; read them from pool state at each slot | [Dexplain review](https://dexplain.com/meteora-met-token-review/) |
| About half of DLMM LP fees in H1 2026 came from pump.fun-launched tokens | Retail speculative flow is cyclical; we sell to issuers with non-speculative flow | [Dexplain review](https://dexplain.com/meteora-met-token-review/) |
| Meteora publishes a free DLMM data API with positions, PnL and limit orders | Raw data is a commodity; our value is audit-grade accounting and operations | [Bitquery comparison](https://docs.bitquery.io/docs/blockchain/Solana/Meteora-DLMM-API/) |

**The gap**

Meteora gives issuers a mechanism and a basic portfolio view. Nobody gives them a statement their auditor accepts, a measurable definition of healthy liquidity, or an accountable operator bound by an on-chain mandate. That gap is the product.

**Starting position**

The existing [meteora-dlmm-indexer](https://github.com/Gedion08/meteora-dlmm-indexer) already covers the hardest infrastructure layer: all instructions, events and account types decoded from the IDL, exactly-once writes to Postgres, gap repair, reconciliation, a read API and a web frontend. It runs on devnet, stores latest account state only, and lists USD prices, position PnL and historical backfill as open items. Those three open items are where this plan begins.

## 3. Customers and personas

The buyer is an organisation that put its own capital into DLMM pools for its own token and must account for it. Retail LPs and memecoin traders are out of scope.

**Segments, in order of priority**

| Segment | Why they buy | First product | Sales motion |
| --- | --- | --- | --- |
| Stablecoin issuers (including local-currency stablecoins) | Peg depth is their product; regulators and banking partners ask for evidence | Reporting, then peg-keeping mandate | Direct, founder-led |
| Token treasuries and foundations | Own protocol-owned liquidity; must report to a DAO or board | Reporting, then managed range and treasury ladders | Direct, via ecosystem introductions |
| Tokenized asset issuers | Need tight, predictable spreads and proof of market quality | Reporting plus liquidity health SLAs | Direct, slower, compliance-heavy |
| Funds and professional LPs | Need NAV, attribution and investor statements | Reporting and API only | Self-serve plus sales assist |
| Launch teams | Need launch liquidity design and post-launch reporting | Advisory engagement | Opportunistic; cyclical |

**Personas**

| Persona | Role | Job to be done | What makes them say yes |
| --- | --- | --- | --- |
| Treasury lead | Decides where issuer capital sits | Know whether liquidity is earning, idle or leaking | One screen with PnL decomposition and idle capital |
| Finance controller | Closes the books monthly | Cost basis, realized gains, fee income per period, exportable | Statements that reconcile to on-chain balances to the last unit |
| Compliance officer | Answers regulators and partners | Evidence of reserves in pools, market quality, who can move funds | Evidence pack with signatures and a written custody model |
| Auditor (external) | Tests the numbers | Trace any figure to source transactions | Drill-down from statement line to transaction list |
| Token lead or founder | Owns market perception | Depth and spread at target sizes, no embarrassing gaps | Liquidity health score and alerts |
| Our operator (internal) | Runs mandates | See breaches, approve or halt strategy actions | Operations console with kill switch |

**Jobs the product must do, stated as outcomes**

1. Tell me what every position earned and lost this period, split by cause.
2. Prove it, down to the transaction.
3. Tell me whether my liquidity meets the depth and spread I promised.
4. Fix it for me inside limits I set, without taking custody.
5. Show me, each month, that you did what the mandate said.

## 4. Product scope

Two products, one account. A client starts on Reporting with read-only wallets and can upgrade any pool to a managed mandate.

### 4.1 Product A: Reporting

| Capability | What the client gets |
| --- | --- |
| Entity and wallet registry | Organisation, legal entities, wallets, labels, read-only by default |
| Position inventory | Every DLMM position and limit order per wallet, open and closed, with full history |
| PnL decomposition | Price effect, divergence loss, fee income, rewards, network costs, per position, pool, wallet and entity |
| Cost basis and realized gains | Lot-level tracking under FIFO or weighted average cost, chosen per entity |
| Liquidity health | Depth at set price distances, in-range time, idle capital, fee capture share, quoted slippage at target sizes |
| Statements | Monthly and custom-period statements in PDF, CSV and XLSX |
| Evidence pack | Reserve attestation inputs, position proofs, signer and authority map, transaction index |
| Alerts | Out-of-range, depth below target, depeg, large withdrawal, authority change |
| API and webhooks | Everything on screen is available by API with the same numbers |
| Audit drill-down | Any figure opens the ledger entries and transaction signatures behind it |

### 4.2 Product B: Managed Liquidity

| Capability | What the client gets |
| --- | --- |
| Mandate | A written and on-chain policy: pools, capital cap, price band, allowed actions, rebalance limits, kill switch |
| Strategy library | Peg-keeper, range with recentering, shaped distributions, treasury ladder using limit orders, inventory targets |
| Simulation | Backtest of a proposed strategy on the pool's own history before any capital moves |
| Three execution modes | Advisory (client executes), co-signed (we propose to the client multisig), delegated (bounded operator key) |
| Risk controls | Oracle deviation guard, depeg halt, per-transaction and daily notional limits, cooldowns |
| Service levels | Depth and spread targets, response time to breaches, reported monthly |
| Mandate report | Every action taken, why, its cost, and performance against a stated benchmark |

### 4.3 Execution modes

| Mode | Who signs | Who can withdraw | When we offer it |
| --- | --- | --- | --- |
| 0. Advisory | Client | Client | From Phase 3; no key risk on our side |
| 1. Co-signed | Client multisig approves our proposed transactions | Client | From Phase 4; for clients with an existing multisig |
| 2. Delegated | Our operator key, inside program-enforced limits | Client only | From Phase 4 after audit; needed for fast rebalancing |

### 4.4 Out of scope

- Custody of client funds, pooled vaults, or any product where client assets are commingled.
- A retail trading terminal. OrderFlow's execution logic is reused inside the treasury ladder strategy; it is not shipped as a consumer app.
- Token launch services, market making on centralised exchanges, and lending.
- Tax filing. We produce inputs for tax and audit; we do not file.
- Venues other than Meteora DLMM before Phase 5.

## 5. Functional requirements

Each requirement has an ID that the phase build specs reference. "Phase" is the phase in which it must pass its acceptance test.

### 5.1 Data foundation (DF)

| ID | Requirement | Acceptance | Phase |
| --- | --- | --- | --- |
| DF-1 | Index Meteora DLMM on mainnet with the existing exactly-once guarantees | 7 days on mainnet, zero open gaps older than 10 minutes, zero unexplained decode failures | 0 |
| DF-2 | Keep versioned history for accounts that tracked positions depend on (positions, limit orders, their bin arrays, their pools) | Any tracked position can be valued at any slot since tracking began | 0 |
| DF-3 | Tracked-set registry: wallets, positions, pools; adding a wallet triggers hydration and backfill | New wallet fully hydrated within 5 minutes; backfill progress visible | 0 |
| DF-4 | Price service with source hierarchy, per-minute history, and stored provenance | Every valuation row names its price source and timestamp; staleness flagged | 0 |
| DF-5 | Historical backfill of a wallet's DLMM transactions before tracking began | Lifetime flows for a wallet match a manual count on 20 sampled positions | 0 |
| DF-6 | Token registry with decimals, symbol, Token-2022 extensions and transfer-fee handling | Transfer-fee tokens reconcile to on-chain balances | 0 |

### 5.2 Ledger and accounting (LA)

| ID | Requirement | Acceptance | Phase |
| --- | --- | --- | --- |
| LA-1 | Double-entry ledger in token units, one journal per on-chain transaction | Every journal balances per token; sum of ledger balances equals on-chain balances | 1 |
| LA-2 | Position valuation from bin shares and bin reserves at a given slot | Matches the Meteora SDK's position amounts for 200 sampled positions, exact to the raw unit | 1 |
| LA-3 | Unclaimed fee and reward calculation | Matches SDK claimable amounts, exact to the raw unit | 1 |
| LA-4 | Limit-order accounting: placed, filled, unfilled, fee share, cancelled, closed | Matches SDK limit-order view for 100 sampled orders | 1 |
| LA-5 | Lot tracking with FIFO and weighted average cost | Realized gain on a scripted scenario equals a hand-worked spreadsheet | 1 |
| LA-6 | PnL decomposition into price, divergence, fees, rewards, costs | Components sum to total PnL with zero residual at raw-unit precision | 1 |
| LA-7 | Period snapshots (daily close) that are immutable once finalized | Re-running a closed period produces byte-identical output | 1 |
| LA-8 | Restatement workflow when chain data is repaired or a rule changes | Restatements are versioned, with a diff and a reason | 1 |
| LA-9 | Continuous reconciliation of ledger to chain | Any break raises an alert within 10 minutes and blocks statement issue | 1 |

### 5.3 Reporting product (RP)

| ID | Requirement | Acceptance | Phase |
| --- | --- | --- | --- |
| RP-1 | Organisations, entities, users, roles (owner, finance, viewer, auditor) | Role tests pass; auditor role is read-only with drill-down | 2 |
| RP-2 | Wallet onboarding by address, with optional proof of control by signed message | Unproven wallets are labelled as such on every export | 2 |
| RP-3 | Dashboards: entity overview, pool, position, limit orders, liquidity health | First meaningful paint under 2 s on a 200-position entity | 2 |
| RP-4 | Statements: period PnL, fee income, realized gains, holdings, activity | Statement totals equal API totals equal ledger totals | 2 |
| RP-5 | Evidence pack with authority map and transaction index | Generated in under 60 s for a 12-month period | 2 |
| RP-6 | Alerts by email, webhook and chat integrations | Alert fires within 2 minutes of the triggering slot | 2 |
| RP-7 | Public API with keys, scopes, rate limits and versioning | OpenAPI spec published; contract tests in CI | 2 |
| RP-8 | Liquidity health metrics and score | Depth and slippage figures match an independent quote simulation within 0.1% | 2 |

### 5.4 Strategy and simulation (SS)

| ID | Requirement | Acceptance | Phase |
| --- | --- | --- | --- |
| SS-1 | Deterministic DLMM pool simulator driven by indexed swaps | Replaying a real week reproduces pool active bin and reserves at each slot | 3 |
| SS-2 | Strategy interface: state in, list of intents out; no side effects | Same inputs give the same intents, proven by property tests | 3 |
| SS-3 | Strategy library v1: peg-keeper, recentering range, treasury ladder | Each has a backtest report on three real pools | 3 |
| SS-4 | Backtest report with fees, divergence, costs, turnover and benchmark | Report generated from the same ledger code as production | 3 |
| SS-5 | Advisory mode: recommendations with unsigned transactions the client can sign | Client-signed transaction lands and is matched to its recommendation | 3 |

### 5.5 Managed execution (ME)

| ID | Requirement | Acceptance | Phase |
| --- | --- | --- | --- |
| ME-1 | Mandate object: off-chain document plus on-chain policy that agree | Hash of the off-chain mandate is stored on-chain | 4 |
| ME-2 | Co-signed mode through the client's multisig | Proposal created, approved and executed end to end on devnet and mainnet | 4 |
| ME-3 | Policy vault program for delegated mode; operator cannot withdraw or exceed limits | External audit passed; negative tests for every limit | 4 |
| ME-4 | Executor: intent to transaction, simulation, send, confirm through our own indexer | Every intent ends in exactly one terminal state; no duplicate sends | 4 |
| ME-5 | Risk engine with pre-trade checks and halts | Each halt condition proven in a fault-injection test | 4 |
| ME-6 | Kill switch for client and for us, on-chain and off-chain | Pause takes effect in the next transaction attempt | 4 |
| ME-7 | Mandate report: actions, reasons, costs, benchmark | Every on-chain action by the operator appears with its intent and reason | 4 |

### 5.6 Operations and commercial (OC)

| ID | Requirement | Acceptance | Phase |
| --- | --- | --- | --- |
| OC-1 | Billing: subscription tiers, usage, performance fee calculation from the ledger | Invoice figures trace to ledger entries | 5 |
| OC-2 | Operator console: mandates, breaches, intents, approvals | A second person can run the service from the runbook | 5 |
| OC-3 | Security programme: key management, access reviews, incident response | Tabletop exercise completed; findings closed | 5 |
| OC-4 | Venue adapter interface with a second venue | A second concentrated-liquidity venue passes the ledger acceptance tests | 5 |

## 6. Accounting and metrics methodology

Every number in the product comes from four primitives: token flows from events, position state from accounts, bin state from accounts, and a price with provenance. This section is the contract the ledger must implement.

### 6.1 Principles

1. Token units are integers. USD is derived, never stored as the only record.
2. Flows are exact because they come from program events: deposits, withdrawals, fee claims, reward claims, limit-order placement and cancellation.
3. State is exact because it comes from account data at a known slot: position shares per bin, bin reserves and share supply, fee growth counters.
4. Valuation is an opinion. It records the price, its source and its time, so it can be re-run.
5. Finalized commitment only for anything that reaches a statement. Confirmed data is shown as provisional.

### 6.2 Position value

A position holds liquidity shares in a set of bins. Its token amounts at slot s are its pro-rata share of each bin's reserves.

```latex
x_p(s) = \sum_{i \in bins(p)} \left\lfloor \frac{L_{p,i}(s) \cdot X_i(s)}{S_i(s)} \right\rfloor
\qquad
y_p(s) = \sum_{i \in bins(p)} \left\lfloor \frac{L_{p,i}(s) \cdot Y_i(s)}{S_i(s)} \right\rfloor
```

Here L is the position's share in bin i, S is the bin's total share supply, and X and Y are the bin's reserves. Rounding must match the program (floor on withdrawal). The acceptance test is equality with the Meteora SDK, to the raw unit.

```latex
V_p(s) = x_p(s) \cdot P_x(s) + y_p(s) \cdot P_y(s)
```

### 6.3 PnL identity

Total PnL for a position over a period is defined so that its parts sum with no residual.

```latex
PnL = (V_{end} - V_{start}) + W - D + F_{claimed} + \Delta F_{unclaimed} + R - C
```

| Symbol | Meaning | Source |
| --- | --- | --- |
| V | Position value excluding unclaimed fees | 6.2 |
| D, W | Deposits and withdrawals, valued at the price when they happened | Liquidity events |
| F claimed | Fees collected in the period | Fee-claim events |
| F unclaimed | Fees accrued and not yet collected | Position and bin fee counters |
| R | Liquidity-mining rewards, claimed plus change in unclaimed | Reward events and counters |
| C | Network fees, priority fees, tips, and rent not refunded | Transaction metadata |

### 6.4 Decomposition

The hold benchmark is the token basket the client put in. A deposit adds its tokens to the basket. A withdrawal removes the same fraction of the basket as the fraction of position shares withdrawn.

```latex
PnL = \underbrace{(H_{end} - H_{start} - D + W_H)}_{\text{price effect}}
    + \underbrace{(V_{end} - H_{end}) - (V_{start} - H_{start}) + (W - W_H)}_{\text{divergence}}
    + \underbrace{F_{claimed} + \Delta F_{unclaimed}}_{\text{fees}} + R - C
```

H is the value of the hold basket and W\_H is the basket value removed by withdrawals. Price effect is what holding would have done. Divergence is the cost of having been a liquidity provider instead. Fees are the payment for bearing it. A healthy position has fees greater than the absolute divergence.

### 6.5 Cost basis and realized gains

- A lot is created whenever a wallet acquires tokens: withdrawal from a position, fee claim, reward claim, limit-order fill, or transfer in.
- Depositing into a position is not a disposal by default. The entity can switch to treating it as one; the choice is stored and printed on statements.
- Composition change inside a position (X converted to Y as price moves) is recorded as an internal conversion at bin prices. It becomes a realized gain only under the entity's chosen policy.
- Lot relief methods: FIFO and weighted average cost. One method per entity per tax year.
- Fee income is recognised either when accrued or when claimed. Default is accrued for management reports and claimed for tax-style reports.

These are policy switches, not opinions we hold. The statement states which were used. Tax treatment differs by jurisdiction, and clients should confirm theirs with an adviser.

### 6.6 Limit orders

| State | Ledger treatment |
| --- | --- |
| Placed | Tokens move from wallet to an order inventory account, per bin |
| Partly or fully filled | Conversion of the deposited token to the other token at the bin price; fee share recognised as income |
| Cancelled | Unfilled tokens, filled proceeds and fees return to the wallet as lots |
| Closed when empty | Rent refund recorded against the original rent cost |

### 6.7 Liquidity health metrics

| Metric | Definition |
| --- | --- |
| Depth at k% | Value of liquidity within k% of the current price on each side: pool total and client share |
| Quoted slippage | Price impact of a simulated swap of a target size, each direction, from current bins |
| In-range time | Share of the period during which the active bin was inside the position's range |
| Active capital ratio | Share of position value sitting in bins within a set distance of the active bin, time-weighted |
| Idle capital | Position value outside that distance, time-weighted |
| Fee capture share | Client fees earned divided by total LP fees of the pool for the period |
| Fee return | Fees divided by time-weighted capital, annualised, shown with the divergence beside it |
| Rebalance cost | Network cost plus divergence realized by rebalancing actions |
| Peg deviation (stable pairs) | Time-weighted absolute deviation of pool price from the reference price |

The health score is a weighted sum of these against targets the client sets. Weights and targets are visible to the client. No hidden scoring.

### 6.8 Price policy

1. Primary: an oracle price where one exists for the mint.
2. Secondary: an aggregator price.
3. Tertiary: the pool's own time-weighted bin price against a priced quote token.
4. If sources disagree by more than a set threshold, the valuation is flagged and excluded from finalized statements until reviewed.

Illiquid tokens priced only from their own pool are marked "self-priced" on every report.

### 6.9 What is exact and what is reconstructed

| Period | Flows | Mark-to-market history |
| --- | --- | --- |
| From the slot tracking began | Exact | Exact, from versioned bin and position state |
| Before tracking began | Exact, from backfilled events | Reconstructed by simulation from swap history; labelled as reconstructed |

Lifetime PnL of a closed position needs only flows and prices, so it is exact in both periods.

## 7. Non-functional requirements

Correctness outranks every other quality. A wrong figure in front of an auditor costs the client relationship; a slow page does not.

| Area | Requirement | Target |
| --- | --- | --- |
| Correctness | Ledger balances equal on-chain balances for every tracked account | Zero unexplained breaks at each daily close |
| Correctness | Components of PnL sum to total | Zero residual in raw units |
| Determinism | Re-running a finalized period gives identical output | Byte-identical statement data |
| Traceability | Every reported figure links to ledger entries and signatures | 100% of statement lines |
| Freshness | Provisional dashboard data | Under 5 s behind chain tip |
| Freshness | Finalized ledger data | Under 2 minutes behind finalization |
| Availability | Reporting API and app | 99.5% monthly in year one |
| Availability | Executor and risk engine while a delegated mandate is live | 99.9% monthly, with safe halt on failure |
| Recovery | Database loss | Restore under 4 hours; data loss under 5 minutes |
| Recovery | Full rebuild of derived data from raw indexed data | Possible without the chain, from our own raw tables |
| Performance | Statement for 12 months and 500 positions | Under 60 s |
| Performance | Dashboard queries | p95 under 500 ms |
| Scale, year one | Tracked wallets, positions | 500 wallets, 20,000 positions |
| Security | Tenant isolation | Row-level security enforced in the database, tested in CI |
| Security | Operator keys | Held in a hardware-backed or managed signer; never on an application server disk |
| Security | Access | SSO or passkeys for staff; least privilege; quarterly review |
| Privacy | Client wallet lists and labels are confidential | Encrypted at rest; never used in public analytics |
| Auditability | Staff actions on client data and mandates | Append-only audit log, exportable |
| Operability | One person can run it | Runbooks, dashboards and alerts for every service |
| Portability | Venue-neutral ledger schema | Meteora-specific fields live only in the adapter tables |

**Failure behaviour for managed mandates.** If the indexer falls behind, prices go stale, or the risk engine cannot evaluate a check, the executor stops sending and raises an alert. It never trades on data it cannot verify. Client liquidity stays where it is, which is the safe state.

## 8. Business model and pricing

Revenue comes from a reporting subscription and a managed-liquidity fee. All prices below are starting hypotheses to test with design partners, not validated numbers.

| Plan | Who it is for | Includes | Starting price hypothesis |
| --- | --- | --- | --- |
| Reporting Starter | Small treasuries, funds | 1 entity, up to 10 wallets, monthly statements, alerts | USD 500 per month |
| Reporting Pro | Issuers | Multiple entities, evidence packs, API, auditor seats, liquidity health | USD 2,000 per month |
| Managed, advisory | Issuers testing us | Reporting Pro plus strategy recommendations and backtests | USD 4,000 per month |
| Managed, delegated | Issuers with live mandates | Above plus execution, SLAs and mandate reports | USD 5,000 to 10,000 per month retainer plus 10% to 20% of fees earned above benchmark |
| Enterprise | Regulated issuers | Custom reporting, dedicated environment, contractual SLAs | Negotiated |

**Why this structure**

- The subscription covers fixed costs and does not depend on market volume.
- The performance fee is charged on fees earned above a stated benchmark, calculated from the same ledger the client sees.
- No fee is taken on client principal, and no fee is taken inside transactions. Invoices are separate from on-chain flows, which keeps the custody story clean.

**Unit economics to watch**

| Driver | Why it matters |
| --- | --- |
| Data cost per tracked pool | Mainnet streaming and storage are the main fixed cost |
| Support hours per reporting client | Must stay low for the wedge to be worth it |
| Conversion from reporting to managed | The whole thesis; target one in four within six months |
| Capital under mandate per managed client | Sets the performance fee |
| Audit and legal cost | Lumpy; mostly in Phase 4 |

**Go-to-market sequence**

1. Use the reporting product on our own OrderFlow test positions and publish the methodology.
2. Give Reporting Pro free for 90 days to three or four issuers in exchange for weekly feedback and a named reference.
3. Convert at least two to paid reporting and one to an advisory mandate.
4. Run that mandate in advisory then co-signed mode; publish the mandate report as a case study with the client's consent.
5. Offer delegated mode only after the vault program audit.

## 9. Custody, compliance and legal posture

We design so that client funds never leave client control, and we get legal advice before the first managed mandate. This section is a product position, not legal advice.

**Custody model by mode**

| Mode | Where funds sit | Our capability | Worst case if our systems are compromised |
| --- | --- | --- | --- |
| Reporting | Client wallets | Read public chain data | Disclosure of the client's wallet list and labels |
| Advisory | Client wallets | None on-chain | A bad recommendation the client chooses to sign |
| Co-signed | Client multisig | Propose transactions | A malicious proposal the client's signers would have to approve |
| Delegated | Policy vault owned by the client's authority | Rebalance inside program-enforced limits | Liquidity moved within the allowed pools and price band; no withdrawal to outside addresses |

**Controls that make the delegated worst case tolerable**

- The vault only pays out to token accounts it owns or to the client authority.
- Pools, bin range, notional per transaction and per day, and action types are checked on-chain.
- Swaps by the operator are disabled in the first version. Rebalancing uses liquidity and limit-order instructions only.
- The client can pause and withdraw at any time without us.
- Operator key rotation is a client action.

**Regulatory questions to settle with counsel before Phase 4**

- [ ] Does discretionary rebalancing under a mandate count as portfolio or asset management where we incorporate and where clients are?
- [ ] Does a bounded operator key count as custody or control of client assets under those regimes?
- [ ] Is a performance fee permitted, and with what disclosures?
- [ ] What licence or registration applies to a virtual asset service provider offering these services?
- [ ] What client due diligence must we run on issuers we serve?
- [ ] Which statements can we call "audit-ready" without holding an accounting qualification?

**Compliance features that are also product features**

- Client onboarding checks and sanctions screening of client wallets.
- Authority map for every pool and position: who can move what.
- Immutable audit log of our own actions.
- Data processing agreement and a clear data retention policy.
- Third-party security audit of the vault program, published.
- Independent review of the accounting methodology by an accountant, published as a methodology paper.

## 10. Roadmap

Six phases over about eleven months, with nothing client-facing blocked on execution work.

&#91;embedded content: roadmap · 6 phases, each with an exit gate\]

Each phase starts only when the previous gate passes. Reporting revenue can begin after Phase 2; delegated execution cannot begin before the audit in Phase 4. Durations assume one full-time builder working with coding agents; the Delivery plan tab holds the week-level breakdown, and each phase has its own build spec tab.

## 11. Success metrics, risks and open questions

**Success metrics by stage**

| Stage | Metric | Target |
| --- | --- | --- |
| Phase 1 exit | Sampled positions matching the SDK to the raw unit | 100% of 200 |
| Phase 2 exit | Design partners using reports weekly | 3 |
| Phase 2 plus 3 months | Paying reporting clients | 2 |
| Phase 3 exit | Advisory mandates signed | 1 |
| Phase 4 exit | Delegated mandate live with capped capital after audit | 1 |
| Month 12 | Reconciliation breaks reaching a client | 0 |
| Month 12 | Reporting to managed conversion | 25% |

**Risks**

| Risk | Likelihood | Impact | Mitigation |
| --- | --- | --- | --- |
| Meteora ships its own institutional reporting | Medium | High | Go deeper than a protocol will: entity accounting, policy switches, auditor workflow, multi-venue |
| Meteora changes program or IDL | High | Medium | IDL-driven decoding already in place; decode-failure alarm; version-pinned accounting rules per program version |
| Accounting error reaches a client | Medium | Very high | SDK parity tests, reconciliation gate before statements, restatement workflow, external methodology review |
| Too few serious issuers on Solana DLMM | Medium | High | Second venue adapter in Phase 5; sell reporting to funds meanwhile |
| Vault program exploit | Low | Very high | Minimal program surface, no operator swaps, external audit, capital caps, staged rollout with our own funds first |
| Operator key compromise | Low | High | On-chain limits bound the damage; managed signer; client pause |
| Regulatory classification as asset manager or custodian | Medium | High | Counsel before Phase 4; advisory and co-signed modes as fallbacks |
| Solo-founder capacity | High | High | Strict phase gates; reporting ships before any execution work; coding agents work from the phase specs |
| Mainnet data cost before revenue | Medium | Medium | Index tracked pools only at first, not the full program firehose |
| Historical valuation before tracking is approximate | Certain | Low | Label reconstructed periods; exact lifetime PnL from flows |

**Open questions**

- [ ] Which three to four issuers are the design partners, and which pools do they hold?
- [ ] Jurisdiction of incorporation, which decides the licensing path.
- [ ] Mainnet streaming provider and plan, and whether to filter the stream to tracked pools.
- [ ] Co-signed mode: which multisig do the design partners already use?
- [ ] Does any design partner need a reporting currency other than USD?
- [ ] Which accountant reviews the methodology paper?
- [ ] Final product name.
