# Phase 2 build spec: reporting product

Goal: three design partners log in weekly, see numbers they trust, and download a statement that equals the API and the ledger. Seven weeks. This is the first client-facing release.

## Scope

| In | Out |
| --- | --- |
| Organisations, entities, users, roles, row-level security | Billing (manual invoices for now) |
| Wallet onboarding with optional proof of control | Any transaction signing |
| Reporting API, API keys, webhooks | Strategy recommendations |
| Metrics engine and health score | Reporting currencies other than USD |
| Client app: dashboards and drill-down | Mobile app |
| Statements, evidence pack, exports | Tax filing formats |
| Alerts | Chat integrations beyond one webhook format |

## Data model additions

```sql
CREATE TABLE app.orgs (org_id UUID PRIMARY KEY, name TEXT NOT NULL, created_at TIMESTAMPTZ NOT NULL DEFAULT now());
ALTER TABLE app.entities ADD COLUMN org_id UUID NOT NULL REFERENCES app.orgs;

CREATE TABLE app.users (user_id UUID PRIMARY KEY, email TEXT UNIQUE NOT NULL, created_at TIMESTAMPTZ NOT NULL DEFAULT now());
CREATE TABLE app.memberships (
  org_id UUID NOT NULL REFERENCES app.orgs,
  user_id UUID NOT NULL REFERENCES app.users,
  role TEXT NOT NULL,                -- owner, finance, viewer, auditor
  entity_scope UUID[],               -- null means all entities in the org
  PRIMARY KEY (org_id, user_id)
);
ALTER TABLE app.entity_wallets
  ADD COLUMN label TEXT,
  ADD COLUMN proof_signature TEXT,   -- signed message proving control; null if unproven
  ADD COLUMN proof_at TIMESTAMPTZ;

CREATE TABLE app.api_keys (
  key_id UUID PRIMARY KEY, org_id UUID NOT NULL, entity_scope UUID[],
  hash TEXT NOT NULL, scopes TEXT[] NOT NULL, created_by UUID NOT NULL,
  last_used_at TIMESTAMPTZ, revoked_at TIMESTAMPTZ
);
CREATE TABLE app.alert_rules (
  rule_id UUID PRIMARY KEY, entity_id UUID NOT NULL, kind TEXT NOT NULL,
  params JSONB NOT NULL, channels JSONB NOT NULL, enabled BOOLEAN NOT NULL DEFAULT true
);
CREATE TABLE app.alerts (
  alert_id BIGSERIAL PRIMARY KEY, rule_id UUID NOT NULL, fired_slot BIGINT NOT NULL,
  payload JSONB NOT NULL, delivered JSONB NOT NULL DEFAULT '{}', dedupe_key TEXT UNIQUE NOT NULL
);
CREATE TABLE app.reports (
  report_id UUID PRIMARY KEY, entity_id UUID NOT NULL, kind TEXT NOT NULL,
  period_from DATE NOT NULL, period_to DATE NOT NULL,
  close_versions JSONB NOT NULL,     -- which close versions it was built from
  data JSONB NOT NULL, data_hash TEXT NOT NULL, created_by UUID, created_at TIMESTAMPTZ NOT NULL DEFAULT now()
);
CREATE TABLE app.audit_log (
  id BIGSERIAL PRIMARY KEY, at TIMESTAMPTZ NOT NULL DEFAULT now(),
  actor TEXT NOT NULL, actor_kind TEXT NOT NULL,  -- user, api_key, staff, system
  org_id UUID, action TEXT NOT NULL, target TEXT, detail JSONB
);

CREATE SCHEMA metrics;
CREATE TABLE metrics.pool_hourly (
  lb_pair TEXT NOT NULL, hour TIMESTAMPTZ NOT NULL,
  depth JSONB NOT NULL,              -- {"0.5": {"bid_usd":..,"ask_usd":..}, "1": .., "2": .., "5": ..}
  slippage JSONB NOT NULL,           -- per target size and direction
  volume_usd NUMERIC, lp_fees_usd NUMERIC, peg_dev_bps NUMERIC,
  PRIMARY KEY (lb_pair, hour)
);
CREATE TABLE metrics.position_daily (
  entity_id UUID NOT NULL, ref TEXT NOT NULL, day DATE NOT NULL,
  in_range_pct NUMERIC, active_capital_pct NUMERIC, idle_usd NUMERIC,
  fees_usd NUMERIC, fee_capture_pct NUMERIC, client_depth JSONB,
  PRIMARY KEY (entity_id, ref, day)
);
CREATE TABLE metrics.health_daily (
  entity_id UUID NOT NULL, lb_pair TEXT NOT NULL, day DATE NOT NULL,
  score NUMERIC NOT NULL, components JSONB NOT NULL, targets JSONB NOT NULL,
  PRIMARY KEY (entity_id, lb_pair, day)
);
```

Row-level security: enable on every table with `entity_id` or `org_id`. The API sets `app.current_org` and `app.current_entities` per request with `SET LOCAL`; policies compare against them. The API's database role has no `BYPASSRLS`.

## Reporting API

Base path `/v1`. Auth by session cookie or `Authorization: Bearer <api key>`. Amounts are strings in raw units with a `decimals` field alongside; USD values are decimal strings. Every response carries `as_of_slot` and `provisional: true|false`.

| Endpoint | Returns |
| --- | --- |
| `GET /entities` | Entities the caller can see, with accounting policies |
| `POST /entities/{id}/wallets` | Add a wallet; starts hydration and backfill |
| `POST /entities/{id}/wallets/{wallet}/proof` | Submit a signed message proving control |
| `GET /entities/{id}/overview?from=&to=` | Totals: value, PnL breakdown, fees, health score per pool |
| `GET /entities/{id}/positions?status=&pool=` | Positions and limit orders with current inventory and period PnL |
| `GET /positions/{ref}` | One position: state, inventory, unclaimed, hold basket |
| `GET /positions/{ref}/pnl?from=&to=` | PnL breakdown with inputs |
| `GET /positions/{ref}/timeseries?metric=&interval=` | Value, fees, divergence over time from closes |
| `GET /positions/{ref}/journals?cursor=` | Ledger journals with entries and signatures |
| `GET /entities/{id}/lots?mint=` and `/realized?from=&to=` | Lots and realized gains |
| `GET /pools/{lb_pair}/health?from=&to=` | Depth, slippage, peg deviation, client share, score |
| `GET /entities/{id}/closes?from=&to=` | Close rows and versions |
| `POST /entities/{id}/reports` | Build a statement or evidence pack for a closed range |
| `GET /reports/{id}` and `/reports/{id}/download?format=pdf,csv,xlsx,json` | Report data and renders |
| `GET, POST, PATCH /entities/{id}/alert-rules` | Alert rules |
| `GET /entities/{id}/alerts` | Fired alerts |
| `GET, POST, DELETE /api-keys` | Key management, owner role only |
| `GET /audit-log` | Org audit log, owner and auditor roles |

## Client app screens

| Screen | Content | Primary action |
| --- | --- | --- |
| Entity overview | Value, period PnL waterfall, fee income, health per pool, open breaks banner | Change period |
| Pool | Bin distribution with client liquidity highlighted, depth and slippage against targets, peg deviation | Set targets |
| Position | Inventory, unclaimed fees, PnL breakdown, value over time, range versus active bin history | Drill to journals |
| Limit orders | Open and closed orders, fill state per bin, fee share earned | Drill to journals |
| Journal drill-down | Entries for a figure, with signatures linking to the chain explorer view | Export |
| Realized gains | Lots and reliefs under the entity's method | Export |
| Statements | List of built reports with hash and close versions | Build, download |
| Alerts | Rules and history | Create rule |
| Settings | Wallets and proofs, accounting policies, members, API keys, audit log | Invite member |

Policy changes (lot method, disposal treatment, fee recognition) apply from a chosen date and trigger a restatement with that reason.

## Statement contents

1. Cover: entity, period, policies applied, close versions, data hash, wallets with proof status.
2. Summary: opening value, net flows, PnL by component, closing value.
3. Per pool and per position: the same breakdown, with fee return and divergence side by side.
4. Income: fees and rewards, accrued and claimed.
5. Realized gains by mint.
6. Costs: network fees, rent.
7. Holdings at period end: wallet balances, position inventories, unclaimed income, with prices and sources.
8. Flags: self-priced tokens, disputed or stale prices, reconstructed periods, unproven wallets.
9. Activity appendix: every journal with signature (CSV for long periods).

Evidence pack adds: authority map per position (owner, fee owner, operator, and any program authority), pool configuration, a signed-message proof list, and a position-by-position reserve table at the period-end slot.

## Work packages

| ID | Package | Week |
| --- | --- | --- |
| P2-WP1 | Tenancy, auth, roles, row-level security, audit log | 1 |
| P2-WP2 | Wallet onboarding and proof of control | 1 |
| P2-WP3 | Reporting API core endpoints | 2 to 3 |
| P2-WP4 | Metrics engine and health score | 2 to 3 |
| P2-WP5 | App shell, overview, pool and position screens | 4 to 5 |
| P2-WP6 | Journal drill-down, lots and realized gains screens | 5 |
| P2-WP7 | Report builder, renders, evidence pack | 6 |
| P2-WP8 | Alert engine and rules | 7 |
| P2-WP9 | API keys, webhooks, OpenAPI, contract tests | 7 |

### Notes per package

- **WP1.** Passwordless email plus passkeys. Sessions server-side. A CI test logs in as org A and attempts every endpoint with org B's IDs; all must return not found.
- **WP2.** Proof message format: fixed text with org ID, wallet, nonce and expiry; verify the signature server-side. Wallets held by a multisig or program are marked "not provable by signature" with a manual attestation option.
- **WP3.** Handlers contain no accounting logic; they call `bl-ledger` and `bl-metrics` readers. Each endpoint has a statement timeout and a contract test from the OpenAPI spec.
- **WP4.** Depth from `hist.bin_versions` at the hour boundary; client share from position shares. Slippage uses a simple bin walk in `bl-dlmm-math` now and moves to the simulator in Phase 3. Health score is the weighted distance from targets, clipped to 0 to 100, with components stored.
- **WP5.** New app in `app/`. Reuse number formatting, charts and theme from `web/`. Token amounts stay as big integers until display.
- **WP7.** Build from `ledger.closes` only. Refuse a range with an open close or open break. Store JSON and hash, then render. PDF is rendered from the same HTML template as the on-screen view.
- **WP8.** Rule kinds: out of range for N minutes, depth below target, peg deviation above threshold, price flag raised, large withdrawal, authority or owner change, reconciliation break. Deduplicate by rule, ref and window.
- **WP9.** Keys shown once, stored hashed. Webhooks signed with a per-endpoint secret; retries with backoff; delivery log.

## Acceptance tests mapped to requirements

| Requirement | Test |
| --- | --- |
| RP-1 | Cross-tenant test suite; auditor role cannot mutate anything |
| RP-2 | Unproven wallet shows on statement cover and export header |
| RP-3 | Overview loads under 2 s for a seeded 200-position entity |
| RP-4 | Statement totals equal `/overview` totals equal a direct SQL sum on ledger entries |
| RP-5 | Evidence pack for 12 months builds under 60 s |
| RP-6 | Induced out-of-range event fires an alert within 2 minutes |
| RP-7 | OpenAPI contract tests pass in CI; rate limit enforced per key |
| RP-8 | Depth and slippage match an independent quote from the SDK within 0.1% on 20 pools |

## Exit gate

- [ ] All RP requirements pass.
- [ ] Three design partners onboarded; each has built at least one statement.
- [ ] One partner's finance contact has traced a figure to its transactions without help.
- [ ] Security review of auth and row-level security completed by someone other than the author.
- [ ] Terms of service and data processing agreement in place.
