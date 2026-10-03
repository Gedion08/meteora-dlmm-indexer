# BinLedger

BinLedger (working name) is a liquidity operations company for token issuers on Meteora
DLMM. It sells two products on one engine: audit-grade reporting on DLMM positions
(Product A) and non-custodial managed liquidity (Product B).

**Status: documentation only. Nothing is built yet, and building waits for explicit
approval.**

## Where things are

- [`docs/`](docs/README.md): the full Product & Architecture Pack: product spec,
  architecture, delivery plan, engineering conventions, and one build spec per phase (0–5).
- [`CLAUDE.md`](CLAUDE.md): instructions every coding session starts with.

## Relationship to the Meteora DLMM indexer

BinLedger builds on the existing
[meteora-dlmm-indexer](https://github.com/Gedion08/meteora-dlmm-indexer) (Rust workspace:
`dlmm-decoder`, `dlmm-indexer`, `dlmm-api`; Postgres migrations `0001`–`0009`; the `web/`
explorer frontend). It already covers streaming, IDL-driven decoding, exactly-once writes,
gap repair, reconciliation, on-demand hydration, a read API and a frontend, on devnet,
with latest account state only. Phase 0 starts from there: mainnet, a tracked set,
versioned state history, USD prices and per-wallet backfill.

Architecture decision D1 says to extend that workspace and database into one monorepo, and
the conventions' repository layout is written that way. How this project and the indexer
repository fit together is an open decision to settle before building starts.
