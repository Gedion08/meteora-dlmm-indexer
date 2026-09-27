import { keepPreviousData, useInfiniteQuery, useQuery } from "@tanstack/react-query";
import { useEffect, useState } from "react";
import { useNavigate, useSearchParams } from "react-router";
import { Delta, EmptyState, ErrorState, PairName, Segmented, SkeletonRows, TimeAgo } from "../components/bits";
import { api, endpoints, type GlobalStats, type Pair, type PairSort } from "../lib/api";
import { compact, int, price as fmtPrice } from "../lib/format";
import { tokenLabel } from "../lib/tokens";

const PAGE = 50;
const SORTS: { value: PairSort; label: string }[] = [
  { value: "trades", label: "Most active" },
  { value: "volume", label: "Volume" },
  { value: "tvl", label: "TVL" },
  { value: "change", label: "Top movers" },
  { value: "recent", label: "Recently updated" },
];

function useDebounced<T>(value: T, ms: number): T {
  const [v, setV] = useState(value);
  useEffect(() => {
    const t = setTimeout(() => setV(value), ms);
    return () => clearTimeout(t);
  }, [value, ms]);
  return v;
}

function Summary() {
  const stats = useQuery({
    queryKey: ["stats"],
    queryFn: ({ signal }) => api<GlobalStats>(endpoints.stats(), signal),
    refetchInterval: 30_000,
  });
  const s = stats.data;
  const items = [
    { label: "Pools indexed", value: s ? int(s.pairs) : "—", sub: s ? `${int(s.positions)} open positions` : "" },
    { label: "Active pools · 24h", value: s ? int(s.active_pairs_24h) : "—", sub: "pools with at least one swap" },
    { label: "Swaps · 24h", value: s ? int(s.trades_24h) : "—", sub: s?.computed_at ? "rolling window" : "" },
    { label: "Traders · 24h", value: s ? int(s.traders_24h) : "—", sub: "unique wallets" },
  ];
  return (
    <div className="summary" aria-busy={stats.isLoading}>
      {items.map((i) => (
        <div className="summary__item" key={i.label}>
          <span className="eyebrow">{i.label}</span>
          <span className="summary__value">{i.value}</span>
          <span className="summary__sub">{i.sub}</span>
        </div>
      ))}
    </div>
  );
}

export function PoolsPage() {
  const [params, setParams] = useSearchParams();
  const nav = useNavigate();
  const sort = (params.get("sort") as PairSort) || "trades";
  const [text, setText] = useState(params.get("q") ?? "");
  const q = useDebounced(text.trim(), 250);

  useEffect(() => {
    const next = new URLSearchParams(params);
    if (q) next.set("q", q);
    else next.delete("q");
    if (next.toString() !== params.toString()) setParams(next, { replace: true });
  }, [q]);

  const pairs = useInfiniteQuery({
    queryKey: ["pairs", q, sort],
    queryFn: ({ pageParam, signal }) => api<Pair[]>(endpoints.pairs({ q, sort, limit: PAGE, offset: pageParam }), signal),
    initialPageParam: 0,
    getNextPageParam: (last, all) => (last.length === PAGE ? all.length * PAGE : undefined),
    placeholderData: keepPreviousData,
    refetchInterval: 30_000,
  });
  const rows = pairs.data?.pages.flat() ?? [];

  return (
    <>
      <div style={{ display: "flex", alignItems: "baseline", justifyContent: "space-between", gap: 16, flexWrap: "wrap" }}>
        <div>
          <h1 style={{ fontSize: "var(--t-2xl)" }}>Pools</h1>
          <p className="soft" style={{ margin: "4px 0 0", maxWidth: "70ch" }}>
            Every Meteora DLMM pool, updated from the chain in real time. Liquidity sits in discrete price bins; the active
            bin is where trades execute.
          </p>
        </div>
      </div>

      <Summary />

      <section className="panel">
        <div className="panel__head">
          <div className="toolbar" style={{ flex: 1 }}>
            <label htmlFor="pool-filter" className="visually-hidden">
              Filter pools
            </label>
            <input
              id="pool-filter"
              className="field"
              placeholder="Filter by symbol, pair or address"
              value={text}
              onChange={(e) => setText(e.target.value)}
              spellCheck={false}
              autoComplete="off"
            />
            <Segmented
              label="Sort pools"
              value={sort}
              options={SORTS}
              onChange={(v) => {
                const next = new URLSearchParams(params);
                next.set("sort", v);
                setParams(next, { replace: true });
              }}
            />
          </div>
          <span className="toolbar__note">Volume, fees and TVL are in each pool's quote token.</span>
        </div>

        {pairs.isError && !pairs.data ? (
          <ErrorState error={pairs.error} onRetry={() => pairs.refetch()} />
        ) : (
          <div className={`table-wrap${pairs.isFetching && !pairs.isFetchingNextPage && pairs.data ? " is-refetching" : ""}`}>
            <table className="table">
              <thead>
                <tr>
                  <th>Pool</th>
                  <th className="r">Price</th>
                  <th className="r">24h</th>
                  <th className="r">Volume 24h</th>
                  <th className="r">Fees 24h</th>
                  <th className="r">TVL</th>
                  <th className="r">Swaps 24h</th>
                  <th className="r">Last swap</th>
                </tr>
              </thead>
              <tbody>
                {pairs.isLoading ? (
                  <SkeletonRows cols={8} />
                ) : rows.length === 0 ? (
                  <tr>
                    <td colSpan={8}>
                      <EmptyState title="No pools match">
                        Try a token symbol like <b>SOL</b>, a pair like <b>SOL/USDC</b>, or paste a pool or mint address.
                      </EmptyState>
                    </td>
                  </tr>
                ) : (
                  rows.map((p) => {
                    const quote = tokenLabel(p.symbol_y, p.token_y_mint);
                    const open = () => nav(`/pool/${p.address}`);
                    return (
                      <tr
                        key={p.address}
                        className="is-link"
                        onClick={open}
                        onKeyDown={(e) => e.key === "Enter" && open()}
                        tabIndex={0}
                        aria-label={`Open pool ${tokenLabel(p.symbol_x, p.token_x_mint)}/${quote}`}
                      >
                        <td>
                          <span style={{ display: "inline-flex", alignItems: "center", gap: 10 }}>
                            <PairName
                              x={{ mint: p.token_x_mint, symbol: p.symbol_x, logo: p.logo_x }}
                              y={{ mint: p.token_y_mint, symbol: p.symbol_y, logo: p.logo_y }}
                            />
                            <span className="badge" title="Bin step: price distance between adjacent bins">
                              {p.bin_step} bps
                            </span>
                          </span>
                        </td>
                        <td className="r">
                          {fmtPrice(p.price ?? p.price_raw)} <span className="muted">{quote}</span>
                        </td>
                        <td className="r">
                          <Delta value={p.price_change_24h} />
                        </td>
                        <td className="r">{p.volume_24h_y ? compact(p.volume_24h_y) : <span className="muted">—</span>}</td>
                        <td className="r">{p.fees_24h_y ? compact(p.fees_24h_y) : <span className="muted">—</span>}</td>
                        <td className="r">{compact(p.tvl_in_y)}</td>
                        <td className="r">{p.trades_24h ? int(p.trades_24h) : <span className="muted">0</span>}</td>
                        <td className="r soft">
                          <TimeAgo unix={p.last_trade_at} />
                        </td>
                      </tr>
                    );
                  })
                )}
              </tbody>
            </table>
          </div>
        )}
        {pairs.hasNextPage && (
          <div style={{ padding: "var(--s3) var(--s4)", borderTop: "1px solid var(--line)", display: "flex", justifyContent: "center" }}>
            <button type="button" className="btn" disabled={pairs.isFetchingNextPage} onClick={() => pairs.fetchNextPage()}>
              {pairs.isFetchingNextPage ? "Loading…" : "Load more pools"}
            </button>
          </div>
        )}
      </section>
    </>
  );
}
