-- Which Solana cluster this database belongs to (set on first run, checked on every start).
CREATE TABLE indexer_meta (
    key         TEXT PRIMARY KEY,           -- 'genesis_hash' | 'network'
    value       TEXT NOT NULL,
    updated_at  TIMESTAMPTZ NOT NULL DEFAULT now()
);
