# Diagrams

The markdown export of the doc replaces its two drawings with a placeholder line
(`[embedded content: ...]`). The drawings are kept here: the original source as `.jsx`
(exactly as stored in the doc), and a plain-text version below so they can be read without
rendering. The `.jsx` files reference the doc viewer's CSS variables (`--cds-*`), so they
do not render on their own outside it.

## System architecture

Placeholder in `architecture.md`, section 1: `system architecture · 4 layers, 12 components, 1 execution loop`.
Source: [`architecture.jsx`](architecture.jsx).

**The ledger sits between raw chain data and both products**

```text
┌─ Chain and external sources ─────────────────────────────────────────────────────┐
│  Solana data streams          Price sources              On-chain programs       │◄──┐
│  Yellowstone gRPC and RPC     Oracles and aggregators    Policy vault calling DLMM│   │
└──────────────────────────────────────┬───────────────────────────────────────────┘   │
                                       │ streams, accounts, prices                     │
                                       ▼                                               │
┌─ Data foundation · Rust and Postgres · existing indexer plus Phase 0 ────────────┐   │
│  Indexer                      State history              Price service           │   │
│  Decode, gap repair, reconcile Versioned bins and positions Minute prices with provenance │
└──────────────────────────────────────┬───────────────────────────────────────────┘   │
                                       │ decoded events, versioned state               │
                                       ▼                                               │
┌─ Ledger · the source of truth · Phase 1  (highlighted) ──────────────────────────┐   │
│  Journal builder              Valuation and lots         Close and reconcile     │   │
│  One balanced journal per tx  Position value, cost basis Daily close, chain parity│  │
└──────────────────────────────────────┬───────────────────────────────────────────┘   │
                                       │ entries, valuations, metrics                  │
                                       ▼                                               │
┌─ Products ───────────────────────────────────────────────────────────────────────┐   │
│  Reporting · Phase 2          Strategy · Phase 3   ──►   Execution · Phase 4      │───┘
│  API, app, statements, alerts Simulator, intents,        Risk checks, signer,     │ signed
│                               backtests                  sender                   │ transactions
└──────────────────────────────────────────────────────────────────────────────────┘
```

Arrows: sources → data foundation → ledger → products; Strategy → Execution; Execution
loops back up to "On-chain programs" (signed transactions), whose effects return through
the indexer.

## Roadmap

Placeholder in `product-spec.md`, section 10: `roadmap · 6 phases, each with an exit gate`.
Source: [`roadmap.jsx`](roadmap.jsx).

**Reporting ships by March 2027; delegated execution waits for the audit**
(Phase 2 is highlighted as the first client-facing release; ◆ marks each phase's exit gate.)

```text
┌ 0 · Mainnet foundation ────────┐   ┌ 1 · Position ledger ───────────┐   ┌ 2 · Reporting product ★ ───────┐
│ Oct to Nov 2026 · 4 weeks      │──►│ Nov 2026 to Jan 2027 · 8 weeks │──►│ Jan to Mar 2027 · 7 weeks      │
│ Mainnet indexer, state history,│   │ Double-entry ledger, valuation,│   │ Tenants, dashboards, exports,  │
│ prices, wallet backfill        │   │ PnL split, reconciliation      │   │ alerts, public API             │
│ ◆ 7 days on mainnet, no gaps   │   │ ◆ 200 positions match the SDK  │   │ ◆ 3 design partners weekly     │
└────────────────────────────────┘   └────────────────────────────────┘   └───────────────┬────────────────┘
                ┌─────────────────────────────────────────────────────────────────────────┘
                ▼
┌ 3 · Strategy and backtest ─────┐   ┌ 4 · Managed execution ─────────┐   ┌ 5 · Scale and second venue ────┐
│ Mar to May 2027 · 7 weeks      │──►│ May to Aug 2027 · 12 weeks     │──►│ From Sep 2027                  │
│ Pool simulator, strategies,    │   │ Policy vault, executor, risk   │   │ Billing, operator console,     │
│ advisory recommendations       │   │ engine, external audit         │   │ security programme, adapter    │
│ ◆ 1 advisory mandate signed    │   │ ◆ Audit passed, capped mandate │   │ ◆ 2nd venue passes ledger tests│
└────────────────────────────────┘   └────────────────────────────────┘   └────────────────────────────────┘
```
