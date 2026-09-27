import { Link } from "react-router";
import type { Swap } from "../lib/api";
import { amount, price as fmtPrice } from "../lib/format";
import { tokenLabel } from "../lib/tokens";
import { Address, TimeAgo } from "./bits";
import { AlertIcon } from "./icons";

/** swap_for_y = X in, Y out: the trader sold X. */
export function side(s: Swap, symX: string) {
  return s.swap_for_y
    ? { label: `Sell ${symX}`, cls: "down", glyph: "▼" }
    : { label: `Buy ${symX}`, cls: "up", glyph: "▲" };
}

export function labels(s: Swap) {
  return { symX: tokenLabel(s.symbol_x, s.token_x_mint), symY: tokenLabel(s.symbol_y, s.token_y_mint) };
}

export function tradeAmounts(s: Swap) {
  const { symX, symY } = labels(s);
  const [inDec, outDec] = s.swap_for_y ? [s.decimals_x, s.decimals_y] : [s.decimals_y, s.decimals_x];
  const [inSym, outSym] = s.swap_for_y ? [symX, symY] : [symY, symX];
  return {
    inText: `${amount(s.amount_in, inDec)} ${inSym}`,
    outText: `${amount(s.amount_out, outDec)} ${outSym}`,
  };
}

/** Compact live feed for the pool sidebar. */
export function TradeFeed({ swaps, fresh }: { swaps: Swap[]; fresh: Set<string> }) {
  if (swaps.length === 0) return <div className="state">No trades yet. New swaps appear here as they land.</div>;
  return (
    <ul className="feed" aria-label="Latest trades">
      {swaps.map((s) => {
        const sd = side(s, labels(s).symX);
        const { inText, outText } = tradeAmounts(s);
        return (
          <li key={s.cursor} className={fresh.has(s.cursor) ? "is-new" : undefined}>
            <span className={`feed__side ${sd.cls}`} aria-hidden="true">
              {sd.glyph}
            </span>
            <span className="num">
              <span className={sd.cls} style={{ fontWeight: 600 }}>
                {sd.label}
              </span>{" "}
              <span className="soft">
                {inText} → {outText}
              </span>
            </span>
            <span className="num" style={{ fontWeight: 600, textAlign: "right" }}>
              {fmtPrice(s.price ?? s.price_raw)}
            </span>
            <span className="feed__meta">
              <Address value={s.trader} kind="wallet" head={4} tail={4} />
            </span>
            <span className="feed__meta" style={{ textAlign: "right" }}>
              <Link to={`/tx/${s.signature}`} className="soft">
                <TimeAgo unix={s.block_time} />
              </Link>
            </span>
          </li>
        );
      })}
    </ul>
  );
}

/** A position's bin range on a track, with the pool's active bin marked. */
export function RangeBar({ lower, upper, active }: { lower: number; upper: number; active: number | null }) {
  const lo = Math.min(lower, active ?? lower);
  const hi = Math.max(upper, active ?? upper);
  const pad = Math.max(2, Math.round((hi - lo) * 0.15));
  const min = lo - pad;
  const span = hi + pad - min || 1;
  const pos = (b: number) => `${((b - min) / span) * 100}%`;
  const inRange = active != null && active >= lower && active <= upper;
  return (
    <div
      className="range"
      role="img"
      aria-label={`Bins ${lower} to ${upper}; active bin ${active ?? "unknown"} is ${inRange ? "inside" : "outside"} the range`}
    >
      <div className="range__track" />
      <div className="range__band" style={{ left: pos(lower), width: `calc(${pos(upper + 1)} - ${pos(lower)})`, opacity: inRange ? 1 : 0.45 }} />
      {active != null && <div className="range__active" style={{ left: `calc(${pos(active)} - 1px)` }} />}
    </div>
  );
}

export function RangeStatus({ lower, upper, active }: { lower: number; upper: number; active: number | null }) {
  if (active == null) return <span className="badge">Unknown</span>;
  const inRange = active >= lower && active <= upper;
  return inRange ? (
    <span className="badge badge--good">● In range</span>
  ) : (
    <span className="badge badge--warn">
      <AlertIcon width={11} height={11} /> Out of range
    </span>
  );
}
