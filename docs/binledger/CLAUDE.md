# BinLedger: agent instructions

Read [`docs/conventions.md`](docs/conventions.md) before any change. Its rules apply to
every phase. For a task, also read the phase spec it belongs to under `docs/`, and work on
one work package (for example `P1-WP3`) at a time, as the conventions describe.

Current status: documentation only. Do not start building until the owner approves it.

Rules that are easy to forget:

- No floating point for token amounts or shares. USD values are derived and always stored
  beside their price reference.
- Read account and field names from `idl/dlmm.json` (in the indexer), never from memory or
  from the specs. Each phase spec lists what to confirm before coding.
- Match the program's rounding, and prove it with the parity harness.
- Statements, the ledger and the `metrics` daily tables use finalized data only.
- Do not rename the existing indexer crates (`dlmm-decoder`, `dlmm-indexer`, `dlmm-api`).
- If a spec is wrong or ambiguous, stop and say so instead of guessing.
