-- On-demand hydration: the API asks (NOTIFY dlmm_hydrate) for a pool's bin arrays and
-- positions, or a wallet's positions, when they were never loaded; the indexer fetches them
-- from the chain and records the result here. The API only reads this table.
CREATE TABLE hydrations (
    kind          TEXT NOT NULL,              -- 'pair' | 'owner'
    key           TEXT NOT NULL,              -- pool or wallet address
    completed_at  TIMESTAMPTZ NOT NULL DEFAULT now(),
    accounts      INT NOT NULL,
    PRIMARY KEY (kind, key)
);
