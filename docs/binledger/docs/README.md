# BinLedger documentation

These files are the **BinLedger — Product & Architecture Pack** doc, saved as Markdown on
2026-10-03 (doc revision: Product spec rev 11, every other tab rev 1).

Source: https://claude.ai/artifact/X4mEe53mLihyYeVzj1hGxv

The `.md` files are the doc's own Markdown export of each tab, byte for byte (checksums
below). Nothing in them has been edited. If the doc changes, re-export rather than
hand-editing here, so the two do not drift apart.

## Contents

| File | Doc tab | What it is |
| --- | --- | --- |
| [product-spec.md](product-spec.md) | Product spec | Thesis, market, customers, scope, requirement IDs (DF, LA, RP, SS, ME, OC), accounting methodology, NFRs, pricing, custody and legal, roadmap, risks |
| [architecture.md](architecture.md) | Architecture design | Layers, decisions D1–D12, baseline vs gaps, components, schemas, key flows, Solana/DLMM specifics, security, deployment, testing, tech choices, multi-venue |
| [delivery-plan.md](delivery-plan.md) | Delivery plan | Phase durations and gates, week-level plan, decision gates, cost lines, first-two-weeks checklist |
| [conventions.md](conventions.md) | Engineering conventions | Repo layout, non-negotiable rules, definition of done, how to hand a work package to an agent, coding standards, glossary |
| [phase-0-foundation.md](phase-0-foundation.md) | Phase 0 · Mainnet foundation | Build spec P0-WP1…WP8 |
| [phase-1-ledger.md](phase-1-ledger.md) | Phase 1 · Position ledger | Build spec P1-WP1…WP10 |
| [phase-2-reporting.md](phase-2-reporting.md) | Phase 2 · Reporting product | Build spec P2-WP1…WP9 |
| [phase-3-strategy.md](phase-3-strategy.md) | Phase 3 · Strategy and backtest | Build spec P3-WP1…WP8 |
| [phase-4-execution.md](phase-4-execution.md) | Phase 4 · Managed execution | Build spec P4-WP1…WP14 |
| [phase-5-scale.md](phase-5-scale.md) | Phase 5 · Scale and second venue | Build spec P5-WP1…WP8 |
| [diagrams/](diagrams/README.md) | (embedded in two tabs) | System architecture and roadmap drawings |

`conventions.md` and `phase-1-ledger.md` use the file names the conventions' agent prompt
refers to (`docs/conventions.md`, `docs/phase-1-ledger.md`).

## What the export did not carry

- Two drawings became a placeholder line `[embedded content: ...]`: the system
  architecture (architecture.md §1) and the roadmap (product-spec.md §10). Both are in
  [diagrams/](diagrams/README.md), with their original source.
- The Product spec byline (date chip and @-mention) is kept as plain text:
  `Oct 3, 2026 · @Gideon`.
- The doc had no comments at the time of export.

## Checksums (SHA-256 of the exported files)

```text
90d14923b65f56b022994947e0e34194145e033bb4ad86a5ad8d85c207225afa  product-spec.md
e9602aa3e7898ac54ed541d5b4ae9ad62eb91fe253b970641da115b951043ae3  architecture.md
afc96bd46d81e0c77de3b607c39d4c8bba39518210c4715d79bfc7d3eab23f79  delivery-plan.md
88e15dd3aa99c0675cd728fb7d8deadd8d82e54752a9bb4d39a80921233d3dd4  conventions.md
f11dea3edac45639450060aa04ad57698ab0f29805563ecda979d1a5028b1227  phase-0-foundation.md
d163d65a1a61961fa368ec88f67681bddcf8ce252fe8f8c16b73297142dc0f85  phase-1-ledger.md
4ebb1675807c5ad9fedf31400c0690291d811d708dcfe648b7d9ad12914b921c  phase-2-reporting.md
11d08ca4a3108851f87167117001cb2ab09fc3291048e7570f1261d33e8405f5  phase-3-strategy.md
2c815341d8a64e22066e654ef41db88624de467052a35085bd839e62b853a724  phase-4-execution.md
39e40afc0fab77c8b5b0f73beaf08ab80a81c884a6d8e8c5bb842735be59c66c  phase-5-scale.md
```

Verify with `sha256sum -c` after pasting the block above into a file, from this directory.
