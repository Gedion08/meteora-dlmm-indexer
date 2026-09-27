import { useQuery } from "@tanstack/react-query";
import { Link, useParams } from "react-router";
import { Address, EmptyState, ErrorState, PairName, Panel, TimeAgo } from "../components/bits";
import { RangeBar, RangeStatus } from "../components/trades";
import { api, endpoints, type Position } from "../lib/api";
import { eventAmounts, eventLabel } from "../lib/events";
import { amount, binRatio, int, price as fmtPrice } from "../lib/format";
import { tokenLabel } from "../lib/tokens";

export function PositionPage() {
  const { address = "" } = useParams();
  const pos = useQuery({
    queryKey: ["position", address],
    queryFn: ({ signal }) => api<Position>(endpoints.position(address), signal),
    refetchInterval: 15_000,
  });

  if (pos.isError) {
    return (pos.error as { status?: number }).status === 404 ? (
      <EmptyState title="Position not found">
        No DLMM position with address <span className="mono">{address}</span> is indexed.
      </EmptyState>
    ) : (
      <ErrorState error={pos.error} onRetry={() => pos.refetch()} />
    );
  }
  const p = pos.data;
  if (!p) return <span className="skeleton" style={{ height: 300 }} />;

  const sx = tokenLabel(p.symbol_x, p.token_x_mint);
  const sy = tokenLabel(p.symbol_y, p.token_y_mint);
  // Price at the range edges, from the current price and the exact bin ratio.
  const edge = (bin: number) =>
    p.price != null && p.active_id != null && p.bin_step != null ? p.price * (1 + binRatio(p.bin_step, bin - p.active_id)) : null;

  return (
    <>
      <nav className="crumbs" aria-label="Breadcrumb">
        <Link to="/">Pools</Link>
        <span aria-hidden="true">/</span>
        <Link to={`/pool/${p.lb_pair}`}>
          {sx}/{sy}
        </Link>
        <span aria-hidden="true">/</span>
        <span>Position</span>
      </nav>

      <div className="pool-head">
        <div>
          <div className="eyebrow">Liquidity position</div>
          <div className="pool-head__id" style={{ marginTop: 6 }}>
            <PairName size="lg" x={{ mint: p.token_x_mint, symbol: p.symbol_x }} y={{ mint: p.token_y_mint, symbol: p.symbol_y }} />
          </div>
          <div className="pool-head__meta">
            {p.closed ? <span className="badge">Closed</span> : <RangeStatus lower={p.lower_bin_id} upper={p.upper_bin_id} active={p.active_id} />}
            <Address value={address} />
          </div>
        </div>
        <div className="price-block">
          <div className="eyebrow">Current liquidity</div>
          <div className="price-block__value num" style={{ fontSize: "var(--t-2xl)" }}>
            {p.closed ? "—" : `${amount(p.amount_x, p.decimals_x)} ${sx}`}
          </div>
          {!p.closed && (
            <div className="price-block__delta soft num">
              + {amount(p.amount_y, p.decimals_y)} {sy}
            </div>
          )}
        </div>
      </div>

      <Panel title="Range">
        <RangeBar lower={p.lower_bin_id} upper={p.upper_bin_id} active={p.active_id} />
        <dl className="facts" style={{ marginTop: 16 }}>
          <dt>Lower bin · price</dt>
          <dd>
            {int(p.lower_bin_id)} · {fmtPrice(edge(p.lower_bin_id))} {sy}
          </dd>
          <dt>Upper bin · price</dt>
          <dd>
            {int(p.upper_bin_id)} · {fmtPrice(edge(p.upper_bin_id))} {sy}
          </dd>
          <dt>Pool active bin · price</dt>
          <dd>
            {p.active_id != null ? int(p.active_id) : "—"} · {fmtPrice(p.price)} {sy}
          </dd>
          <dt>Fees claimed</dt>
          <dd>
            {amount(p.total_claimed_fee_x, p.decimals_x)} {sx} · {amount(p.total_claimed_fee_y, p.decimals_y)} {sy}
          </dd>
          <dt>Owner</dt>
          <dd>
            <Address value={p.owner} kind="wallet" />
          </dd>
          <dt>Last updated</dt>
          <dd>
            <TimeAgo unix={p.last_updated_at} />
          </dd>
        </dl>
        {p.extended && (
          <p className="muted" style={{ marginBottom: 0 }}>
            This position was resized beyond 70 bins; amounts for the extra bins aren't included yet.
          </p>
        )}
      </Panel>

      <Panel title="History" flush>
        {!p.events?.length ? (
          <EmptyState title="No events recorded">History starts when the indexer first saw this position.</EmptyState>
        ) : (
          <div className="table-wrap">
            <table className="table">
              <thead>
                <tr>
                  <th>Time</th>
                  <th>Action</th>
                  <th className="r">Amounts</th>
                  <th>Tx</th>
                </tr>
              </thead>
              <tbody>
                {p.events.map((e) => (
                  <tr key={`${e.signature}.${e.inner_index}`}>
                    <td className="soft">
                      <TimeAgo unix={e.block_time} />
                    </td>
                    <td style={{ fontWeight: 600 }}>{eventLabel(e.name)}</td>
                    <td className="r">{eventAmounts({ ...e, symbol_x: p.symbol_x, symbol_y: p.symbol_y, decimals_x: p.decimals_x, decimals_y: p.decimals_y, token_x_mint: p.token_x_mint, token_y_mint: p.token_y_mint })}</td>
                    <td>
                      <Address value={e.signature} kind="tx" head={5} tail={0} />
                    </td>
                  </tr>
                ))}
              </tbody>
            </table>
          </div>
        )}
      </Panel>
    </>
  );
}
