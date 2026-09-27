import type { ReactNode } from "react";
import { useQuery } from "@tanstack/react-query";
import { Link, useParams } from "react-router";
import { Address, EmptyState, ErrorState, Panel } from "../components/bits";
import { ChevronIcon } from "../components/icons";
import { api, endpoints, type TokenBalance, type TxDetail } from "../lib/api";
import { eventAmounts, eventLabel } from "../lib/events";
import { amount, dateTime, int, scaleRaw } from "../lib/format";

const B58 = /^[1-9A-HJ-NP-Za-km-z]{32,44}$/;

/** Decoded values: addresses become links, everything else prints as-is (u128s are strings). */
function Value({ v }: { v: unknown }) {
  if (typeof v === "string" && B58.test(v)) return <Address value={v} />;
  if (v === null || v === undefined) return <span className="muted">null</span>;
  if (typeof v === "object") return <code className="mono" style={{ fontSize: 12 }}>{JSON.stringify(v)}</code>;
  return <span className="num">{String(v)}</span>;
}

function balanceChanges(pre: TokenBalance[], post: TokenBalance[]) {
  const key = (b: TokenBalance) => `${b.account}|${b.mint}`;
  const before = new Map(pre.map((b) => [key(b), b]));
  return post
    .map((b) => {
      const p = before.get(key(b));
      const delta = BigInt(b.amount ?? "0") - BigInt(p?.amount ?? "0");
      return { ...b, delta };
    })
    .filter((b) => b.delta !== 0n);
}

export function TxPage() {
  const { signature = "" } = useParams();
  const tx = useQuery({
    queryKey: ["tx", signature],
    queryFn: ({ signal }) => api<TxDetail>(endpoints.tx(signature), signal),
  });
  if (tx.isError) {
    return (tx.error as { status?: number }).status === 404 ? (
      <EmptyState title="Transaction not indexed">
        Only transactions that call the Meteora DLMM program are indexed. This signature isn't one of them, or it's older
        than the index.
      </EmptyState>
    ) : (
      <ErrorState error={tx.error} onRetry={() => tx.refetch()} />
    );
  }
  const t = tx.data;
  if (!t) return <span className="skeleton" style={{ height: 300 }} />;
  const changes = balanceChanges(t.token_balances.pre, t.token_balances.post);
  const pools = [...new Set(t.instructions.map((i) => i.lb_pair).filter(Boolean))] as string[];

  return (
    <>
      <nav className="crumbs" aria-label="Breadcrumb">
        <Link to="/">Pools</Link>
        <span aria-hidden="true">/</span>
        <span>Transaction</span>
      </nav>
      <div>
        <div className="eyebrow">Transaction</div>
        <h1 style={{ fontSize: "var(--t-lg)", marginTop: 4, overflowWrap: "anywhere" }}>
          <Address value={t.signature} full />
        </h1>
      </div>

      <div className="summary">
        {[
          { label: "Status", value: t.success ? "Success" : "Failed", sub: t.err ?? "confirmed on chain" },
          { label: "Time", value: dateTime(t.block_time), sub: `slot ${int(t.slot)}` },
          { label: "Network fee", value: `${scaleRaw(String(t.fee), 9)} SOL`, sub: `${int(t.compute_units)} compute units` },
          { label: "DLMM calls", value: int(t.instructions.length), sub: `${t.events.length} events` },
        ].map((i) => (
          <div className="summary__item" key={i.label}>
            <span className="eyebrow">{i.label}</span>
            <span className="summary__value">{i.value}</span>
            <span className="summary__sub">{i.sub}</span>
          </div>
        ))}
      </div>

      <div className="two-col">
        <Panel title="Signer and pools">
          <dl className="facts">
            <dt>Fee payer</dt>
            <dd>
              <Address value={t.fee_payer} kind="wallet" />
            </dd>
            {pools.map((p) => (
              <FragmentRow key={p} label="Pool" value={<Address value={p} kind="pool" />} />
            ))}
          </dl>
        </Panel>
        <Panel title="Token balance changes" flush>
          {changes.length === 0 ? (
            <EmptyState title="No token balances changed" />
          ) : (
            <div className="table-wrap">
              <table className="table">
                <thead>
                  <tr>
                    <th>Owner</th>
                    <th>Mint</th>
                    <th className="r">Change</th>
                  </tr>
                </thead>
                <tbody>
                  {changes.map((c) => (
                    <tr key={`${c.account}${c.mint}`}>
                      <td>
                        {pools.includes(c.owner) ? (
                          <span style={{ display: "inline-flex", gap: 6, alignItems: "center" }}>
                            <span className="badge">Pool reserve</span>
                            <Address value={c.owner} kind="pool" />
                          </span>
                        ) : (
                          <Address value={c.owner} kind="wallet" />
                        )}
                      </td>
                      <td>
                        <Address value={c.mint} />
                      </td>
                      <td className={`r ${c.delta > 0n ? "up" : "down"}`}>
                        {c.delta > 0n ? "+" : "−"}
                        {amount((c.delta < 0n ? -c.delta : c.delta).toString(), c.decimals)}
                      </td>
                    </tr>
                  ))}
                </tbody>
              </table>
            </div>
          )}
        </Panel>
      </div>

      <Panel title="DLMM instructions" flush>
        <ul className="ix-list">
          {t.instructions.map((ix) => (
            <li key={`${ix.ix_index}.${ix.inner_index}`}>
              <details className="ix" data-depth={ix.inner_index >= 0 ? 1 : 0}>
                <summary>
                  <ChevronIcon className="ix__caret" />
                  <span className="ix__name">{ix.name}</span>
                  {ix.invoked_by ? (
                    <span className="badge">
                      via <span className="mono">{ix.invoked_by.slice(0, 4)}…</span>
                    </span>
                  ) : (
                    <span className="badge">top-level #{ix.ix_index}</span>
                  )}
                </summary>
                <dl className="kv">
                  {Object.entries(ix.args).map(([k, v]) => (
                    <FragmentKV key={`a-${k}`} k={k} v={<Value v={v} />} />
                  ))}
                  {Object.entries(ix.accounts).map(([k, v]) => (
                    <FragmentKV key={`c-${k}`} k={k} v={<Value v={v} />} />
                  ))}
                  {ix.remaining_accounts.length > 0 && (
                    <FragmentKV k="remaining_accounts" v={<span className="soft">{ix.remaining_accounts.length} accounts</span>} />
                  )}
                </dl>
              </details>
            </li>
          ))}
        </ul>
      </Panel>

      <Panel title="Events" flush>
        {t.events.length === 0 ? (
          <EmptyState title="No events emitted" />
        ) : (
          <ul className="ix-list">
            {t.events.map((e) => (
              <li key={`${e.ix_index}.${e.inner_index}`}>
                <details className="ix">
                  <summary>
                    <ChevronIcon className="ix__caret" />
                    <span className="ix__name">{e.name}</span>
                    <span className="soft">{eventLabel(e.name) !== e.name ? eventLabel(e.name) : ""}</span>
                    <span className="num soft" style={{ marginLeft: "auto" }}>
                      {e.name === "Swap" ? "" : eventAmounts(e) !== "—" ? eventAmounts(e) : ""}
                    </span>
                  </summary>
                  <dl className="kv">
                    {Object.entries(e.data).map(([k, v]) => (
                      <FragmentKV key={k} k={k} v={<Value v={v} />} />
                    ))}
                  </dl>
                </details>
              </li>
            ))}
          </ul>
        )}
      </Panel>
    </>
  );
}

function FragmentKV({ k, v }: { k: string; v: ReactNode }) {
  return (
    <>
      <dt>{k}</dt>
      <dd>{v}</dd>
    </>
  );
}

function FragmentRow({ label, value }: { label: string; value: ReactNode }) {
  return (
    <>
      <dt>{label}</dt>
      <dd>{value}</dd>
    </>
  );
}
