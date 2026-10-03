# Meteora DLMM Indexer

Live indexer for the Meteora DLMM program (`LBUZKhRxPF3XUpBCjp4YzTKgLccjZhTSDM9YuVaPwxo`).
It streams from a Yellowstone gRPC provider (Helius LaserStream), decodes **every**
instruction, event and account type from the program IDL, and writes to Postgres
within about a second of confirmation.

```
LaserStream gRPC ──► source (reconnect, replay from checkpoint, ping, idle detection)
  (1..n providers)        │
                          ▼
                     assembler (dedupe across providers, per-slot batching,
                          │     block_time, finalized/dead slot tracking)
                          ▼
                     writer ──► Postgres (idempotent upserts + checkpoint, one txn)
                          │
                          └── /healthz /readyz /metrics (Prometheus)
```

## Networks

The project currently runs on **devnet** (`.env`): Helius LaserStream devnet for streaming,
Helius devnet RPC, and the same program ID as mainnet.

Each database belongs to exactly one network. On first start the indexer records the
cluster's genesis hash in `indexer_meta` and refuses to run against a different one, and it
stops if the gRPC stream and `RPC_URL` are on different clusters. To go to mainnet:

1. Get a gRPC plan that includes mainnet streaming (Helius Business/Professional, or
   OrbitFlare with Geyser enabled — both work as-is; set `GRPC_ENDPOINTS`/`GRPC_X_TOKENS`).
2. Point `RPC_URL` at a mainnet RPC and `DATABASE_URL` at a **new, empty** database.
3. Run on a server with datacenter bandwidth: a mainnet DLMM stream is roughly 3–4 MB/s.
4. `dlmm-indexer run`, then `dlmm-indexer snapshot` for current state.

## What gets indexed

| Data | Table / view | Notes |
|---|---|---|
| All 76 instructions (top-level **and** CPI via Jupiter etc.) | `instructions` | named accounts + decoded args as JSONB |
| All 30 `emit_cpi!` events | `events` → `swaps`, `liquidity_events`, `fee_claims`, `reward_claims` | |
| All 12 account types, latest state | `accounts` → `lb_pairs`, `positions`, `bins` | closes tracked in `closed_slot` |
| Transaction context | `transactions` | fee, CU, signer, token balance deltas |
| Block times / finality | `slots`, `indexer_state` | |
| Anything undecodable | `decode_failures` | dead-letter queue (IDL drift alarm) |
| Known missing ranges | `gaps` | detected by replay refusal or slot-chain audit; auto-repaired via RPC |
| Pool reserves / TVL | `token_balances` → `pair_reserves` | from post-tx token balances + snapshot |
| Token decimals, symbols, logos | `mints` | decimals from balances/snapshot; symbol/logo via DAS job |
| Rolling 24h pool stats | `pair_stats_24h`, `global_stats_24h` | materialized, refreshed every `STATS_INTERVAL_SECS` |
| Pool ranking / search | `pair_rank` | sort + search fields per pool, refreshed with the stats; the list endpoint picks a page here and computes live details only for that page |

Decoding is driven by `idl/dlmm.json` at runtime-free cost (parsed once), so an IDL
update is a file swap: replace `idl/dlmm.json`, rebuild, and rows in `decode_failures`
tell you if anything still doesn't match.

JSON conventions: u64 → number, u128 → decimal string, pubkey → base58. Cast in SQL with
`(data->>'amount_in')::numeric`.

## Quick start

Prerequisites: Rust (stable), Postgres 16+ (or Docker), and a Yellowstone gRPC endpoint.
Helius LaserStream on **mainnet requires a Business or Professional plan** (lower plans get
`Unsupported plan type`). Devnet LaserStream works on every plan and DLMM is live there, so
you can validate the whole pipeline for free:

```bash
GRPC_ENDPOINTS=https://laserstream-devnet-ewr.helius-rpc.com cargo run -- tail --duration-secs 30
```

```bash
cp .env.example .env              # fill in your Helius API key
docker compose up -d postgres     # or point DATABASE_URL at your own Postgres

# 1. Smoke test the stream without a database (prints decoded events as JSON lines)
cargo run --release -- tail --duration-secs 20

# 2. Run the indexer (applies migrations, resumes from its checkpoint)
cargo run --release -- run

# 3. In another terminal: load current state of all pairs / positions / bins
cargo run --release -- snapshot --types LbPair,PresetParameter2,Oracle,BinArrayBitmapExtension   # also fills mint decimals
cargo run --release -- snapshot --types BinArray,PositionV2,LimitOrder     # large; run once
```

Start `run` **before** `snapshot`: snapshot rows carry their RPC slot and never overwrite
newer data from the stream, so running both concurrently is safe.

## Example queries

```sql
-- Latest swaps on a pair
SELECT block_time, trader, swap_for_y, amount_in, amount_out, protocol_fee
FROM swaps WHERE lb_pair = '<pair>' ORDER BY slot DESC, tx_index DESC LIMIT 50;

-- A wallet's open positions
SELECT * FROM positions WHERE owner = '<wallet>' AND NOT closed;

-- Liquidity distribution around the active bin
SELECT b.bin_id, b.amount_x, b.amount_y, b.price_raw
FROM bins b JOIN lb_pairs p USING (lb_pair)
WHERE p.lb_pair = '<pair>' AND b.bin_id BETWEEN p.active_id - 30 AND p.active_id + 30
ORDER BY b.bin_id;

-- Instruction mix in the last hour
SELECT name, count(*) FROM instructions
WHERE block_time > now() - interval '1 hour' GROUP BY 1 ORDER BY 2 DESC;

-- Health of decoding
SELECT kind, discriminator, count(*), max(error) FROM decode_failures GROUP BY 1, 2;
```

## API (`dlmm-api`)

A separate read-only service for frontends and bots. Run it next to the indexer:

```bash
cargo run -p dlmm-api            # listens on API_ADDR (default 0.0.0.0:8080)
```

Conventions: token amounts are **strings** (u64 overflows JS numbers); `price` is Y per X
adjusted for token decimals (`null` until decimals are known), `price_raw` is in raw units;
times are unix seconds; lists are newest-first and paginate with `?cursor=<cursor of last item>`.
Raw `data` / `args` payloads keep u64 values as JSON numbers — parse them with a
bigint-safe JSON parser if you need exact values.

| Endpoint | Returns |
|---|---|
| `GET /v1/health` | `ok` (no auth) |
| `GET /v1/status` | checkpoint, finalized slot, seconds behind, decode failures, live clients, and open `gaps` with their time window and repair progress |
| `GET /v1/stats` | network-wide: pools, open positions, 24h swaps / traders / active pools |
| `GET /v1/resolve/{id}` | what an address is: `pool`, `position`, `token`, `wallet` or `tx` (powers search) |
| `GET /v1/hydrate?pair=` · `?owner=` | `pending` / `done`: loads a pool's bins and positions (or a wallet's positions) from the chain if they were never indexed |
| `GET /v1/pairs?q=&sort=&mint=&limit=&offset=` | pools with price, 24h volume/fees/change, TVL, symbols. `q`: address, symbol or `SOL/USDC`; `sort`: trades, volume, tvl, change, recent |
| `GET /v1/pairs/{pair}` | one pair + `stats_24h` (trades, volume, fees, unique traders) |
| `GET /v1/pairs/{pair}/bins?radius=35` or `?from_bin=&to_bin=` | liquidity per bin around the active bin |
| `GET /v1/pairs/{pair}/swaps?limit=&cursor=` | swaps (amounts, fee, fee %, execution price) |
| `GET /v1/pairs/{pair}/events?names=AddLiquidity,RemoveLiquidity` | any decoded events |
| `GET /v1/pairs/{pair}/candles?interval=1m\|5m\|15m\|1h\|4h\|1d&from=&to=` | OHLCV (bin price at swap end) |
| `GET /v1/wallets/{wallet}/positions?include_closed=` | positions with current token amounts |
| `GET /v1/wallets/{wallet}/swaps` · `/events` | a wallet's activity |
| `GET /v1/positions/{position}` | position state, amounts and its event history |
| `GET /v1/tx/{signature}` | every DLMM instruction and event in a transaction |
| `GET /v1/ws` | live WebSocket feed (below) |

### Live feed (`/v1/ws`)

```jsonc
// subscribe (all filters optional; omit them for a firehose)
{"op":"subscribe","channel":"swaps","lb_pair":"<pair>"}
{"op":"subscribe","channel":"events","wallet":"<wallet>","names":["AddLiquidity","RemoveLiquidity"]}
{"op":"subscribe","channel":"pairs","lb_pair":"<pair>"}      // active bin / price changes
{"op":"unsubscribe","id":1}   {"op":"ping"}

// server messages
{"type":"subscribed","id":1,...}
{"type":"swap","sub":1,"data":{ /* same shape as /swaps items */ }}
{"type":"event","sub":2,"data":{ /* same shape as /events items */ }}
{"type":"pair","sub":3,"data":{"address","slot","active_id","price","price_raw",...}}
{"type":"lagged","missed":N} | {"type":"resync"}   // refetch via REST with cursors
```

Messages are pushed when the indexer commits (Postgres `NOTIFY` inside the write
transaction), so clients never see data that isn't in the database. Delivery is
at-most-once per connection: on `lagged`/`resync` or reconnect, backfill with REST.

### Production settings

| Variable | Default | Purpose |
|---|---|---|
| `API_KEYS` | *(empty = open)* | comma-separated keys; send as `x-api-key` header or `?api_key=` (WebSocket) |
| `API_RATE_LIMIT_RPS` | 20 | per key (or per IP when open); burst = 2× |
| `API_CORS_ORIGINS` | *(any)* | e.g. `https://app.example.com` |
| `API_STATEMENT_TIMEOUT_MS` | 5000 | caps every query so the API can't starve the indexer |
| `API_MAX_WS_CLIENTS` | 1000 | |

Use a read-only Postgres role for the API in production:

```sql
CREATE ROLE dlmm_api LOGIN PASSWORD '...';
GRANT CONNECT ON DATABASE dlmm TO dlmm_api;
GRANT USAGE ON SCHEMA public TO dlmm_api;
GRANT SELECT ON ALL TABLES IN SCHEMA public TO dlmm_api;
ALTER DEFAULT PRIVILEGES IN SCHEMA public GRANT SELECT ON TABLES TO dlmm_api;
```

## Frontend (`web/` — Binscope)

A React + TypeScript app (Vite) over the API: pool list with 24h stats and search, pool
pages with live candles, liquidity by price bin and a live trade tape, wallets with their
positions (range vs. the active bin), positions, and fully decoded transactions. Light and
dark themes, phone layouts, keyboard access, and a live status indicator.

```bash
cd web
npm install
npm run dev          # http://localhost:5173 — proxies /v1 (REST + WebSocket) to :8080
npm test             # unit tests (number/price formatting)
npm run build        # production bundle in web/dist (detail pages are code-split)
```

For production, serve `web/dist` as a static SPA (fallback to `index.html`) and either proxy
`/v1` to `dlmm-api` on the same origin, or build with `VITE_API_BASE=https://api.example.com`
(and `API_CORS_ORIGINS` on the API). `VITE_API_KEY` is embedded in the public bundle, so only
use a key there that you're happy to be public (rate limits still apply per key).

Design notes: chart colors were validated as a set (all pairs, both themes, colour-vision
deficiency simulations) with a palette checker; price direction is always shown with ▲/▼ as
well as colour; tiny prices use DEX subscript notation (`0.0₄4803`); token amounts stay exact
(u64 strings scaled with BigInt) until display rounding. Token symbols and logos come from the
indexer's metadata job (DAS `getAssetBatch`); tokens without metadata show a short mint and a
deterministic mark.

## Reliability model

- **Commitment**: ingests at `confirmed` (≈ sub-second behind the tip). `indexer_state.finalized`
  marks what is final; rows for a slot that later dies are deleted and affected accounts
  re-fetched from RPC.
- **Exactly-once effect**: every table has a natural key; inserts are `ON CONFLICT DO NOTHING`
  and account upserts only move forward in `(slot, write_version)`. The checkpoint commits in
  the same transaction as the data.
- **Restarts / disconnects**: reconnect with exponential backoff and `from_slot =
  checkpoint − 64`. LaserStream replays ~48h, so short outages lose nothing. Longer outages
  are recorded in `gaps` (and alert) instead of being silently skipped.
- **Redundancy**: set `GRPC_ENDPOINTS` to several providers/regions; updates are deduplicated.
- **Backpressure**: bounded queues; if Postgres slows down, the stream is paused, not dropped.
- **Graceful shutdown**: SIGTERM stops sources, flushes buffered slots, drains the writer.

## Operations

| Endpoint | Purpose |
|---|---|
| indexer `GET :9100/healthz` | liveness (stream messages flowing) |
| indexer `GET :9100/readyz` | readiness (slot lag ≤ `MAX_READY_LAG_SLOTS`) |
| indexer `GET :9100/metrics` | Prometheus metrics |
| API `GET :9101/metrics` | API metrics (requests, latency per route, WebSocket clients) |

### Self-healing (runs inside `dlmm-indexer run`)

| Job | Default | What it does |
|---|---|---|
| Gap audit + repair | every 60 s (`AUDIT_INTERVAL_SECS`) | finds holes in the stored slot chain and repairs every open gap with the live extractor, then refreshes the accounts the missed transactions touched. Gaps up to `AUTO_REPAIR_MAX_SLOTS` (20 000) are fetched block by block; larger ones (e.g. days offline) fetch only the blocks that contain DLMM transactions, found with `getSignaturesForAddress`. Repairs run newest-first and checkpoint progress, so a restart resumes where it stopped. |
| On-demand hydration | when asked | the API sends `NOTIFY dlmm_hydrate` when a pool's bins/positions or a wallet's positions were never loaded; the indexer fetches exactly those accounts (filtered `getProgramAccountsV2`) in seconds. |
| Reconciliation | every 300 s (`RECONCILE_INTERVAL_SECS`, `RECONCILE_SAMPLE`=100) | compares recently written + random accounts with on-chain state (after the indexer has passed the RPC slot, so in-flight updates aren't false alarms) and overwrites stale or closed ones. |

Manual commands (same code paths):

```bash
dlmm-indexer audit                     # scan the whole slot chain for holes
dlmm-indexer repair-gaps               # repair all open gaps (--mode auto|blocks|signatures)
dlmm-indexer reconcile --sample 500    # or --pubkeys A,B,C
```

### Monitoring

`docker compose --profile monitoring up -d` starts Prometheus (scraping both services,
alert rules from `deploy/alerts.yml`) and Grafana on :3000 with the **Meteora DLMM
indexer** dashboard pre-provisioned (lag, stream delay, events/s, DB latency, errors,
self-healing activity, API traffic and latency, live feed).

### Deploying without Docker

`deploy/systemd/` has hardened units for both binaries (`EnvironmentFile=/etc/dlmm/*.env`,
restart on failure, 30 s graceful stop so the indexer flushes). Build release binaries on
a machine with a stable toolchain (`cargo build --release -p dlmm-indexer -p dlmm-api`),
copy them to `/usr/local/bin`, then `systemctl enable --now dlmm-indexer dlmm-api`.

## Tests

```bash
cargo test                         # decoder golden tests (mainnet fixtures), stream replay, RPC conversion
python3 scripts/e2e.py             # full end-to-end run against the configured network (~10 min)
```

The end-to-end suite runs in an isolated `e2e` Postgres schema on separate ports (a dev
instance can keep running) and checks: migrations, streaming, the network guard, snapshot
(pairs, decimals, reserves), live data, every REST endpoint, pagination/validation/auth/
rate limits, WebSocket push, graceful restart without holes, **automatic detection and
byte-identical repair of a deleted slot range**, **reconciliation healing a corrupted
account**, that every metric used by the dashboard and alerts is exported, and graceful
shutdown. Report: `target/e2e/report.json`, logs in `target/e2e/`.

## Roadmap (not in this MVP)

- ~~Week 3: REST/WebSocket API~~ ✅ done (`dlmm-api`). Next: USD prices, token metadata,
  TVL from reserve balances, Timescale continuous aggregates when candle queries get heavy.
- ~~Week 4: reconciliation, gap repair, dashboards, deploy units~~ ✅ done (Phase 3).
- ~~Frontend~~ ✅ done (Phase 4: `web/`).
- Mainnet: USD prices (Jupiter/Pyth), position PnL, Timescale continuous aggregates.
- Later: historical backfill (Old Faithful) into the same tables; ClickHouse for analytics.
