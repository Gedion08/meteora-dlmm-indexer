import { useInfiniteQuery, useQuery } from "@tanstack/react-query";
import { useState } from "react";
import { Link, useNavigate, useParams } from "react-router";
import { Address, EmptyState, ErrorState, PairName, Panel, Segmented, TimeAgo } from "../components/bits";
import { RangeBar, RangeStatus, labels, side, tradeAmounts } from "../components/trades";
import { api, endpoints, type DlmmEvent, type Position, type Swap } from "../lib/api";
import { eventAmounts, eventLabel } from "../lib/events";
import { amount, price as fmtPrice } from "../lib/format";
import { useLive } from "../lib/live";
import { tokenLabel } from "../lib/tokens";
import { useQueryClient } from "@tanstack/react-query";

export function WalletPage() {
  const { address = "" } = useParams();
  const [showClosed, setShowClosed] = useState<"open" | "all">("open");
  const [tab, setTab] = useState<"swaps" | "activity">("swaps");
  const nav = useNavigate();
  const qc = useQueryClient();

  const positions = useQuery({
    queryKey: ["wallet-positions", address, showClosed],
    queryFn: ({ signal }) => api<Position[]>(endpoints.walletPositions(address, showClosed === "all"), signal),
    refetchInterval: 30_000,
  });
  useLive({ channel: "events", wallet: address }, () => {
    qc.invalidateQueries({ queryKey: ["wallet-positions", address] });
    qc.invalidateQueries({ queryKey: ["wallet-swaps", address] });
    qc.invalidateQueries({ queryKey: ["wallet-events", address] });
  });

  const rows = positions.data ?? [];
  return (
    <>
      <nav className="crumbs" aria-label="Breadcrumb">
        <Link to="/">Pools</Link>
        <span aria-hidden="true">/</span>
        <span>Wallet</span>
      </nav>
      <div>
        <div className="eyebrow">Wallet</div>
        <h1 style={{ fontSize: "var(--t-xl)", marginTop: 4 }}>
          <Address value={address} full />
        </h1>
      </div>

      <Panel
        title="Liquidity positions"
        actions={
          <Segmented
            label="Positions shown"
            value={showClosed}
            options={[
              { value: "open", label: "Open" },
              { value: "all", label: "Include closed" },
            ]}
            onChange={setShowClosed}
          />
        }
        flush
      >
        {positions.isError ? (
          <ErrorState error={positions.error} onRetry={() => positions.refetch()} />
        ) : !positions.isLoading && rows.length === 0 ? (
          <EmptyState title="No positions">This wallet has no DLMM liquidity positions in the index.</EmptyState>
        ) : (
          <div className="table-wrap">
            <table className="table">
              <thead>
                <tr>
                  <th>Pool</th>
                  <th>Range vs price</th>
                  <th>Status</th>
                  <th className="r">Bins</th>
                  <th className="r">Liquidity</th>
                  <th className="r">Fees claimed</th>
                  <th className="r">Updated</th>
                </tr>
              </thead>
              <tbody>
                {rows.map((p) => {
                  const sx = tokenLabel(p.symbol_x, p.token_x_mint);
                  const sy = tokenLabel(p.symbol_y, p.token_y_mint);
                  const open = () => nav(`/position/${p.address}`);
                  return (
                    <tr key={p.address} className="is-link" onClick={open} onKeyDown={(e) => e.key === "Enter" && open()} tabIndex={0}>
                      <td>
                        <PairName x={{ mint: p.token_x_mint, symbol: p.symbol_x }} y={{ mint: p.token_y_mint, symbol: p.symbol_y }} />
                      </td>
                      <td style={{ width: 200 }}>
                        <RangeBar lower={p.lower_bin_id} upper={p.upper_bin_id} active={p.active_id} />
                      </td>
                      <td>{p.closed ? <span className="badge">Closed</span> : <RangeStatus lower={p.lower_bin_id} upper={p.upper_bin_id} active={p.active_id} />}</td>
                      <td className="r soft">
                        {p.lower_bin_id} → {p.upper_bin_id}
                      </td>
                      <td className="r">
                        {p.closed ? "—" : `${amount(p.amount_x, p.decimals_x)} ${sx} · ${amount(p.amount_y, p.decimals_y)} ${sy}`}
                      </td>
                      <td className="r soft">
                        {amount(p.total_claimed_fee_x, p.decimals_x)} {sx} · {amount(p.total_claimed_fee_y, p.decimals_y)} {sy}
                      </td>
                      <td className="r soft">
                        <TimeAgo unix={p.last_updated_at} />
                      </td>
                    </tr>
                  );
                })}
              </tbody>
            </table>
          </div>
        )}
      </Panel>

      <section className="panel">
        <div className="tabs" role="tablist" aria-label="Wallet history">
          <button type="button" role="tab" aria-selected={tab === "swaps"} onClick={() => setTab("swaps")}>
            Swaps
          </button>
          <button type="button" role="tab" aria-selected={tab === "activity"} onClick={() => setTab("activity")}>
            All activity
          </button>
        </div>
        {tab === "swaps" ? <WalletSwaps wallet={address} /> : <WalletActivity wallet={address} />}
      </section>
    </>
  );
}

function WalletSwaps({ wallet }: { wallet: string }) {
  const q = useInfiniteQuery({
    queryKey: ["wallet-swaps", wallet],
    queryFn: ({ pageParam, signal }) => api<Swap[]>(endpoints.walletSwaps(wallet, pageParam || undefined), signal),
    initialPageParam: "",
    getNextPageParam: (last) => (last.length === 50 ? last[last.length - 1]!.cursor : undefined),
  });
  const rows = q.data?.pages.flat() ?? [];
  if (q.isError) return <ErrorState error={q.error} onRetry={() => q.refetch()} />;
  if (!q.isLoading && rows.length === 0) return <EmptyState title="No swaps from this wallet" />;
  return (
    <div className="table-wrap">
      <table className="table">
        <thead>
          <tr>
            <th>Time</th>
            <th>Pool</th>
            <th>Side</th>
            <th className="r">Paid</th>
            <th className="r">Received</th>
            <th className="r">Price</th>
            <th>Tx</th>
          </tr>
        </thead>
        <tbody>
          {rows.map((s) => {
            const { symX, symY } = labels(s);
            const sd = side(s, symX);
            const { inText, outText } = tradeAmounts(s);
            return (
              <tr key={s.cursor}>
                <td className="soft">
                  <TimeAgo unix={s.block_time} />
                </td>
                <td>
                  <Link to={`/pool/${s.lb_pair}`} style={{ fontWeight: 600 }}>
                    {symX}/{symY}
                  </Link>
                </td>
                <td className={sd.cls} style={{ fontWeight: 600 }}>
                  {sd.glyph} {sd.label}
                </td>
                <td className="r">{inText}</td>
                <td className="r">{outText}</td>
                <td className="r">{fmtPrice(s.price ?? s.price_raw)}</td>
                <td>
                  <Address value={s.signature} kind="tx" head={5} tail={0} />
                </td>
              </tr>
            );
          })}
        </tbody>
      </table>
      {q.hasNextPage && (
        <div style={{ padding: 12, display: "flex", justifyContent: "center", borderTop: "1px solid var(--line)" }}>
          <button className="btn" type="button" disabled={q.isFetchingNextPage} onClick={() => q.fetchNextPage()}>
            {q.isFetchingNextPage ? "Loading…" : "Older swaps"}
          </button>
        </div>
      )}
    </div>
  );
}

function WalletActivity({ wallet }: { wallet: string }) {
  const q = useInfiniteQuery({
    queryKey: ["wallet-events", wallet],
    queryFn: ({ pageParam, signal }) => api<DlmmEvent[]>(endpoints.walletEvents(wallet, pageParam || undefined), signal),
    initialPageParam: "",
    getNextPageParam: (last) => (last.length === 50 ? last[last.length - 1]!.cursor : undefined),
  });
  const rows = (q.data?.pages.flat() ?? []).filter((e) => e.name !== "Swap2Evt");
  if (q.isError) return <ErrorState error={q.error} onRetry={() => q.refetch()} />;
  if (!q.isLoading && rows.length === 0) return <EmptyState title="No activity from this wallet" />;
  return (
    <div className="table-wrap">
      <table className="table">
        <thead>
          <tr>
            <th>Time</th>
            <th>Action</th>
            <th>Pool</th>
            <th className="r">Amounts</th>
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
              <td>
                {e.lb_pair ? (
                  <Link to={`/pool/${e.lb_pair}`}>
                    {tokenLabel(e.symbol_x, e.token_x_mint)}/{tokenLabel(e.symbol_y, e.token_y_mint)}
                  </Link>
                ) : (
                  "—"
                )}
              </td>
              <td className="r">{e.name === "Swap" ? "see swap" : eventAmounts(e)}</td>
              <td>
                <Address value={e.signature} kind="tx" head={5} tail={0} />
              </td>
            </tr>
          ))}
        </tbody>
      </table>
      {q.hasNextPage && (
        <div style={{ padding: 12, display: "flex", justifyContent: "center", borderTop: "1px solid var(--line)" }}>
          <button className="btn" type="button" disabled={q.isFetchingNextPage} onClick={() => q.fetchNextPage()}>
            {q.isFetchingNextPage ? "Loading…" : "Older"}
          </button>
        </div>
      )}
    </div>
  );
}
