import { keepPreviousData, useInfiniteQuery, useQuery, useQueryClient } from "@tanstack/react-query";
import { useCallback, useMemo, useRef, useState } from "react";
import { Link, useParams } from "react-router";
import { Address, Delta, EmptyState, ErrorState, PairName, Panel, Segmented, TimeAgo } from "../components/bits";
import { CandleChart, type Tick } from "../components/CandleChart";
import { LiquidityChart, LiquidityTable } from "../components/LiquidityChart";
import { TradeFeed, labels, side, tradeAmounts } from "../components/trades";
import { api, endpoints, type Bin, type Candle, type CandleInterval, type DlmmEvent, type Pair, type Swap } from "../lib/api";
import { amount, compact, int, price as fmtPrice, rawToNumber } from "../lib/format";
import { useLive, useResync } from "../lib/live";
import { eventAmounts, eventLabel } from "../lib/events";
import { tokenLabel } from "../lib/tokens";

const INTERVALS: { value: CandleInterval; label: string; secs: number }[] = [
  { value: "1m", label: "1m", secs: 60 },
  { value: "5m", label: "5m", secs: 300 },
  { value: "15m", label: "15m", secs: 900 },
  { value: "1h", label: "1h", secs: 3600 },
  { value: "4h", label: "4h", secs: 14400 },
  { value: "1d", label: "1D", secs: 86400 },
];
const PAIR_TYPES = ["Permissionless", "Permissioned", "Customizable", "Permissionless v2"];

type HistoryTab = "trades" | "liquidity" | "fees";

export function PoolPage() {
  const { address = "" } = useParams();
  const qc = useQueryClient();
  const [interval, setCandleInterval] = useState<CandleInterval>("5m");
  const [radius, setRadius] = useState<"35" | "70" | "140">("35");
  const [binView, setBinView] = useState<"chart" | "table">("chart");
  const [tab, setTab] = useState<HistoryTab>("trades");
  const [tape, setTape] = useState<Swap[]>([]);
  const [fresh, setFresh] = useState<Set<string>>(new Set());
  const [tick, setTick] = useState<Tick | null>(null);
  const binsTimer = useRef<ReturnType<typeof setTimeout> | null>(null);

  const pair = useQuery({
    queryKey: ["pair", address],
    queryFn: ({ signal }) => api<Pair>(endpoints.pair(address), signal),
    refetchInterval: 30_000,
  });
  const p = pair.data;
  const intervalSecs = INTERVALS.find((i) => i.value === interval)!.secs;

  const candles = useQuery({
    queryKey: ["candles", address, interval],
    queryFn: ({ signal }) =>
      api<{ decimals_adjusted: boolean; candles: Candle[] }>(
        endpoints.candles(address, interval, Math.floor(Date.now() / 1000) - intervalSecs * 300),
        signal,
      ),
    placeholderData: keepPreviousData,
    refetchInterval: 60_000,
  });
  const bins = useQuery({
    queryKey: ["bins", address, radius],
    queryFn: ({ signal }) => api<{ active_id: number; bins: Bin[] }>(endpoints.bins(address, Number(radius)), signal),
    placeholderData: keepPreviousData,
  });
  const initialTape = useQuery({
    queryKey: ["tape", address],
    queryFn: ({ signal }) => api<Swap[]>(endpoints.swaps(address, undefined, 40), signal),
  });

  const tapeRows = useMemo(() => {
    const seen = new Set<string>();
    return [...tape, ...(initialTape.data ?? [])].filter((s) => (seen.has(s.cursor) ? false : (seen.add(s.cursor), true))).slice(0, 60);
  }, [tape, initialTape.data]);

  const refreshBins = useCallback(() => {
    if (binsTimer.current) return;
    binsTimer.current = setTimeout(() => {
      binsTimer.current = null;
      qc.invalidateQueries({ queryKey: ["bins", address] });
    }, 1500);
  }, [qc, address]);

  useLive<Swap>({ channel: "swaps", lb_pair: address }, (s) => {
    setTape((t) => [s, ...t].slice(0, 60));
    setFresh((f) => new Set(f).add(s.cursor));
    setTimeout(() => setFresh((f) => {
      const n = new Set(f);
      n.delete(s.cursor);
      return n;
    }), 1800);
    const volY = rawToNumber(s.swap_for_y ? s.amount_out : s.amount_in, s.decimals_y ?? 0) ?? 0;
    if (s.block_time) setTick({ time: s.block_time, price: s.price ?? s.price_raw, volumeY: volY });
    qc.setQueryData<Pair>(["pair", address], (old) =>
      old ? { ...old, active_id: s.end_bin_id, price: s.price ?? old.price, price_raw: s.price_raw, last_trade_at: s.block_time } : old,
    );
    refreshBins();
  });
  useLive<{ active_id: number; price: number | null; price_raw: number; volatility_accumulator: number }>(
    { channel: "pairs", lb_pair: address },
    (u) => {
      qc.setQueryData<Pair>(["pair", address], (old) =>
        old ? { ...old, active_id: u.active_id, price: u.price ?? old.price, price_raw: u.price_raw, volatility_accumulator: u.volatility_accumulator } : old,
      );
      refreshBins();
    },
  );
  useResync(() => {
    setTape([]);
    qc.invalidateQueries({ predicate: (q) => q.queryKey.includes(address) });
  });

  if (pair.isError) {
    return pair.error && (pair.error as { status?: number }).status === 404 ? (
      <EmptyState title="Pool not found">
        No DLMM pool with address <span className="mono">{address}</span> is indexed. <Link to="/">Browse pools</Link>
      </EmptyState>
    ) : (
      <ErrorState error={pair.error} onRetry={() => pair.refetch()} />
    );
  }

  const symX = p ? tokenLabel(p.symbol_x, p.token_x_mint) : "X";
  const symY = p ? tokenLabel(p.symbol_y, p.token_y_mint) : "Y";
  const lastFee = tapeRows[0]?.fee_pct;

  return (
    <>
      <nav className="crumbs" aria-label="Breadcrumb">
        <Link to="/">Pools</Link>
        <span aria-hidden="true">/</span>
        <span>
          {symX}/{symY}
        </span>
      </nav>

      <div className="pool-head">
        <div>
          <div className="pool-head__id">
            {p ? (
              <PairName
                size="lg"
                x={{ mint: p.token_x_mint, symbol: p.symbol_x, logo: p.logo_x }}
                y={{ mint: p.token_y_mint, symbol: p.symbol_y, logo: p.logo_y }}
              />
            ) : (
              <span className="skeleton" style={{ width: 220, height: 28 }} />
            )}
          </div>
          <div className="pool-head__meta">
            {p && <span className="badge">Bin step {p.bin_step} bps</span>}
            {lastFee != null && <span className="badge" title="Total fee rate charged on the latest swap (base + variable)">Fee {lastFee.toFixed(2)}%</span>}
            {p && p.status !== 0 && <span className="badge badge--warn">Disabled</span>}
            <Address value={address} />
          </div>
        </div>
        <div className="price-block" aria-live="polite">
          <div className="eyebrow">Price · {symY} per {symX}</div>
          <div className="price-block__value num">{p ? fmtPrice(p.price ?? p.price_raw) : "—"}</div>
          <div className="price-block__delta">
            <Delta value={p?.price_change_24h} /> <span className="muted">24h</span>
          </div>
        </div>
      </div>

      <div className="summary">
        {[
          { label: "TVL", value: p ? `${compact(p.tvl_in_y)} ${symY}` : "—", sub: p ? `${amount(p.reserve_x_amount, p.decimals_x)} ${symX} + ${amount(p.reserve_y_amount, p.decimals_y)} ${symY}` : "" },
          { label: "Volume · 24h", value: p?.volume_24h_y ? `${compact(p.volume_24h_y)} ${symY}` : "—", sub: p ? `${int(p.trades_24h)} swaps` : "" },
          { label: "Fees · 24h", value: p?.fees_24h_y ? `${compact(p.fees_24h_y)} ${symY}` : "—", sub: "earned by liquidity + protocol" },
          { label: "Traders · 24h", value: p ? int(p.traders_24h) : "—", sub: p?.last_trade_at ? "last swap " : "", time: p?.last_trade_at },
        ].map((i) => (
          <div className="summary__item" key={i.label}>
            <span className="eyebrow">{i.label}</span>
            <span className="summary__value">{i.value}</span>
            <span className="summary__sub">
              {i.sub}
              {i.time ? <TimeAgo unix={i.time} /> : null}
            </span>
          </div>
        ))}
      </div>

      <div className="pool-grid">
        <div className="pool-main">
          <Panel
            title={`Price · ${symY}`}
            actions={<Segmented label="Candle interval" value={interval} options={INTERVALS} onChange={setCandleInterval} />}
            flush
          >
            <div className={candles.isFetching && candles.data ? "is-refetching" : undefined}>
              {candles.isError ? (
                <ErrorState error={candles.error} onRetry={() => candles.refetch()} />
              ) : candles.data && candles.data.candles.length === 0 ? (
                <EmptyState title="No trades in this window">Pick a longer interval to see earlier activity.</EmptyState>
              ) : (
                <CandleChart
                  candles={candles.data?.candles ?? []}
                  intervalSecs={intervalSecs}
                  decimalsY={p?.decimals_y ?? null}
                  quoteLabel={symY}
                  tick={tick}
                />
              )}
            </div>
          </Panel>

          <Panel
            title="Liquidity by price bin"
            actions={
              <div className="toolbar">
                <Segmented
                  label="Bins shown"
                  value={radius}
                  options={[
                    { value: "35", label: "±35 bins" },
                    { value: "70", label: "±70" },
                    { value: "140", label: "±140" },
                  ]}
                  onChange={setRadius}
                />
                <Segmented
                  label="View"
                  value={binView}
                  options={[
                    { value: "chart", label: "Chart" },
                    { value: "table", label: "Table" },
                  ]}
                  onChange={setBinView}
                />
              </div>
            }
          >
            <div className={bins.isFetching && bins.data ? "is-refetching" : undefined}>
              {bins.isError ? (
                <ErrorState error={bins.error} onRetry={() => bins.refetch()} />
              ) : !bins.data || !p ? (
                <span className="skeleton" style={{ height: 220 }} />
              ) : binView === "chart" ? (
                <LiquidityChart
                  bins={bins.data.bins}
                  activeId={p.active_id}
                  symbolX={symX}
                  symbolY={symY}
                  decimalsX={p.decimals_x}
                  decimalsY={p.decimals_y}
                />
              ) : (
                <LiquidityTable
                  bins={bins.data.bins}
                  activeId={p.active_id}
                  symbolX={symX}
                  symbolY={symY}
                  decimalsX={p.decimals_x}
                  decimalsY={p.decimals_y}
                />
              )}
            </div>
          </Panel>

          <section className="panel">
            <div className="tabs" role="tablist" aria-label="Pool history">
              {(
                [
                  ["trades", "Trade history"],
                  ["liquidity", "Liquidity changes"],
                  ["fees", "Fee claims"],
                ] as [HistoryTab, string][]
              ).map(([v, l]) => (
                <button key={v} role="tab" aria-selected={tab === v} onClick={() => setTab(v)} type="button">
                  {l}
                </button>
              ))}
            </div>
            {tab === "trades" ? <TradeHistory address={address} /> : <EventHistory address={address} kind={tab} />}
          </section>
        </div>

        <aside className="pool-side">
          <Panel
            title={
              <>
                Live trades <span className="badge">{tapeRows.length}</span>
              </>
            }
            flush
          >
            {initialTape.isLoading ? <div className="panel__body"><span className="skeleton" /></div> : <TradeFeed swaps={tapeRows} fresh={fresh} />}
          </Panel>

          <Panel title="Pool">
            {p ? (
              <dl className="facts">
                <dt>Base token</dt>
                <dd>
                  <Address value={p.token_x_mint} /> {p.symbol_x ?? ""}
                </dd>
                <dt>Quote token</dt>
                <dd>
                  <Address value={p.token_y_mint} /> {p.symbol_y ?? ""}
                </dd>
                <dt>Reserve {symX}</dt>
                <dd>{amount(p.reserve_x_amount, p.decimals_x)}</dd>
                <dt>Reserve {symY}</dt>
                <dd>{amount(p.reserve_y_amount, p.decimals_y)}</dd>
                <dt>Active bin</dt>
                <dd>{int(p.active_id)}</dd>
                <dt>Bin step</dt>
                <dd>{p.bin_step} bps</dd>
                <dt>Base factor</dt>
                <dd>{int(p.base_factor)}</dd>
                <dt>Protocol share</dt>
                <dd>{(p.protocol_share_bps / 100).toFixed(2)}% of fees</dd>
                <dt>Volatility</dt>
                <dd title="Volatility accumulator: drives the variable fee">{int(p.volatility_accumulator)}</dd>
                <dt>Pool type</dt>
                <dd>{PAIR_TYPES[p.pair_type] ?? p.pair_type}</dd>
                <dt>Creator</dt>
                <dd>
                  <Address value={p.creator} kind="wallet" />
                </dd>
                <dt>Stats as of</dt>
                <dd>
                  <TimeAgo unix={p.stats_computed_at} />
                </dd>
              </dl>
            ) : (
              <span className="skeleton" style={{ height: 200 }} />
            )}
          </Panel>
        </aside>
      </div>
    </>
  );
}

function TradeHistory({ address }: { address: string }) {
  const q = useInfiniteQuery({
    queryKey: ["swaps", address],
    queryFn: ({ pageParam, signal }) => api<Swap[]>(endpoints.swaps(address, pageParam || undefined), signal),
    initialPageParam: "",
    getNextPageParam: (last) => (last.length === 50 ? last[last.length - 1]!.cursor : undefined),
  });
  const rows = q.data?.pages.flat() ?? [];
  if (q.isError) return <ErrorState error={q.error} onRetry={() => q.refetch()} />;
  if (!q.isLoading && rows.length === 0) return <EmptyState title="No trades yet" />;
  return (
    <>
      <div className="table-wrap">
        <table className="table">
          <thead>
            <tr>
              <th>Time</th>
              <th>Side</th>
              <th className="r">Paid</th>
              <th className="r">Received</th>
              <th className="r">Price</th>
              <th className="r">Fee</th>
              <th>Trader</th>
              <th>Tx</th>
            </tr>
          </thead>
          <tbody>
            {rows.map((s) => {
              const sd = side(s, labels(s).symX);
              const { inText, outText } = tradeAmounts(s);
              return (
                <tr key={s.cursor}>
                  <td className="soft">
                    <TimeAgo unix={s.block_time} />
                  </td>
                  <td className={sd.cls} style={{ fontWeight: 600 }}>
                    {sd.glyph} {sd.label}
                  </td>
                  <td className="r">{inText}</td>
                  <td className="r">{outText}</td>
                  <td className="r">{fmtPrice(s.price ?? s.price_raw)}</td>
                  <td className="r soft">{s.fee_pct.toFixed(2)}%</td>
                  <td>
                    <Address value={s.trader} kind="wallet" />
                  </td>
                  <td>
                    <Address value={s.signature} kind="tx" head={5} tail={0} />
                  </td>
                </tr>
              );
            })}
          </tbody>
        </table>
      </div>
      {q.hasNextPage && (
        <div style={{ padding: 12, display: "flex", justifyContent: "center", borderTop: "1px solid var(--line)" }}>
          <button className="btn" type="button" disabled={q.isFetchingNextPage} onClick={() => q.fetchNextPage()}>
            {q.isFetchingNextPage ? "Loading…" : "Older trades"}
          </button>
        </div>
      )}
    </>
  );
}

const EVENT_NAMES: Record<"liquidity" | "fees", string[]> = {
  liquidity: ["AddLiquidity", "RemoveLiquidity", "Rebalancing"],
  fees: ["ClaimFee", "ClaimFee2", "ClaimReward", "ClaimReward2"],
};

function EventHistory({ address, kind }: { address: string; kind: "liquidity" | "fees" }) {
  const q = useInfiniteQuery({
    queryKey: ["events", address, kind],
    queryFn: ({ pageParam, signal }) => api<DlmmEvent[]>(endpoints.pairEvents(address, EVENT_NAMES[kind], pageParam || undefined), signal),
    initialPageParam: "",
    getNextPageParam: (last) => (last.length === 50 ? last[last.length - 1]!.cursor : undefined),
  });
  const rows = q.data?.pages.flat() ?? [];
  if (q.isError) return <ErrorState error={q.error} onRetry={() => q.refetch()} />;
  if (!q.isLoading && rows.length === 0)
    return <EmptyState title={kind === "liquidity" ? "No liquidity changes yet" : "No fee claims yet"} />;
  return (
    <>
      <div className="table-wrap">
        <table className="table">
          <thead>
            <tr>
              <th>Time</th>
              <th>Action</th>
              <th className="r">Amounts</th>
              <th>Position</th>
              <th>Wallet</th>
              <th>Tx</th>
            </tr>
          </thead>
          <tbody>
            {rows.map((e) => (
              <tr key={e.cursor}>
                <td className="soft">
                  <TimeAgo unix={e.block_time} />
                </td>
                <td style={{ fontWeight: 600 }}>{eventLabel(e.name)}</td>
                <td className="r">{eventAmounts(e)}</td>
                <td>
                  <Address value={e.position} kind="position" />
                </td>
                <td>
                  <Address value={e.wallet} kind="wallet" />
                </td>
                <td>
                  <Address value={e.signature} kind="tx" head={5} tail={0} />
                </td>
              </tr>
            ))}
          </tbody>
        </table>
      </div>
      {q.hasNextPage && (
        <div style={{ padding: 12, display: "flex", justifyContent: "center", borderTop: "1px solid var(--line)" }}>
          <button className="btn" type="button" disabled={q.isFetchingNextPage} onClick={() => q.fetchNextPage()}>
            {q.isFetchingNextPage ? "Loading…" : "Older"}
          </button>
        </div>
      )}
    </>
  );
}
