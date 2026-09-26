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

## What gets indexed

| Data | Table / view | Notes |
|---|---|---|
| All 76 instructions (top-level **and** CPI via Jupiter etc.) | `instructions` | named accounts + decoded args as JSONB |
| All 30 `emit_cpi!` events | `events` → `swaps`, `liquidity_events`, `fee_claims`, `reward_claims` | |
| All 12 account types, latest state | `accounts` → `lb_pairs`, `positions`, `bins` | closes tracked in `closed_slot` |
| Transaction context | `transactions` | fee, CU, signer, token balance deltas |
| Block times / finality | `slots`, `indexer_state` | |
| Anything undecodable | `decode_failures` | dead-letter queue (IDL drift alarm) |
| Known missing ranges | `gaps` | e.g. offline longer than the provider's replay window |

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
cargo run --release -- snapshot --types LbPair,PresetParameter2,Oracle,BinArrayBitmapExtension
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
| `GET :9100/healthz` | liveness (stream messages flowing) |
| `GET :9100/readyz` | readiness (slot lag ≤ `MAX_READY_LAG_SLOTS`) |
| `GET :9100/metrics` | Prometheus metrics |

Key metrics: `slot_lag`, `grpc_update_delay_seconds`, `db_flush_seconds`,
`decode_failures_total`, `grpc_reconnects_total`, `gaps_recorded_total`, `events_total{name}`.
Alert rules: `deploy/alerts.yml`.

## Tests

```bash
cargo test                       # decoder golden tests against real mainnet fixtures
cargo run --release -- tail --duration-secs 60 --record fixtures/stream.bin   # capture more
```

## Roadmap (not in this MVP)

- Week 3: REST/WebSocket API for frontend and bots (Redis pub/sub fan-out), OHLCV candles
  (Timescale continuous aggregates), token metadata and USD prices.
- Week 4: reconciliation jobs vs. on-chain state, Grafana dashboards, Kubernetes manifests.
- Later: historical backfill (Old Faithful) into the same tables; ClickHouse for analytics.
