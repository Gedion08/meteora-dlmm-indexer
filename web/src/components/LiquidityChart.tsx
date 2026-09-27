// Liquidity by price bin — the DLMM-specific view.
// Bins below the active bin hold only token Y, bins above only token X, so position
// already says which token a bar is; every bar is measured in the same unit (value in Y)
// on one axis, single hue, with the active bin in ink. Hover/focus shows the exact
// composition; a table view carries every value without hovering.
import { useCallback, useMemo, useRef, useState } from "react";
import type { Bin } from "../lib/api";
import { amount, compact, price as fmtPrice } from "../lib/format";

interface Props {
  bins: Bin[];
  activeId: number;
  symbolX: string;
  symbolY: string;
  decimalsX: number | null;
  decimalsY: number | null;
}

const H = 240;
const M = { top: 22, right: 64, bottom: 30, left: 8 };

function niceStep(max: number, ticks: number): number {
  const raw = max / ticks;
  const mag = 10 ** Math.floor(Math.log10(raw));
  const norm = raw / mag;
  return (norm <= 1 ? 1 : norm <= 2 ? 2 : norm <= 2.5 ? 2.5 : norm <= 5 ? 5 : 10) * mag;
}

export function LiquidityChart({ bins, activeId, symbolX, symbolY, decimalsX, decimalsY }: Props) {
  const wrap = useRef<HTMLDivElement | null>(null);
  const observer = useRef<ResizeObserver | null>(null);
  const [width, setWidth] = useState(720);
  const [hover, setHover] = useState<number | null>(null);

  // Callback ref: follows whichever element is mounted (empty state or plot).
  const measure = useCallback((el: HTMLDivElement | null) => {
    observer.current?.disconnect();
    wrap.current = el;
    if (!el) return;
    setWidth(Math.max(280, el.getBoundingClientRect().width));
    observer.current = new ResizeObserver(([e]) => e && setWidth(Math.max(280, e.contentRect.width)));
    observer.current.observe(el);
  }, []);

  // Value of each bin in token Y. Raw units are consistent on their own
  // (price_raw is raw-Y per raw-X), then scaled once for display.
  const rows = useMemo(
    () =>
      bins.map((b) => {
        const valueRawY = Number(b.amount_y) + Number(b.amount_x) * b.price_raw;
        const value = decimalsY != null ? valueRawY / 10 ** decimalsY : valueRawY;
        return { ...b, value };
      }),
    [bins, decimalsY],
  );

  const plotW = width - M.left - M.right;
  const plotH = H - M.top - M.bottom;
  const n = Math.max(rows.length, 1);
  const band = plotW / n;
  const barW = Math.max(1, Math.min(24, band - 2));
  const max = Math.max(...rows.map((r) => r.value), 0);
  const step = max > 0 ? niceStep(max, 3) : 1;
  const top = max > 0 ? Math.ceil(max / step) * step : 1;
  const y = (v: number) => M.top + plotH - (v / top) * plotH;
  const x = (i: number) => M.left + i * band + (band - barW) / 2;
  const activeIdx = rows.findIndex((r) => r.bin_id === activeId);
  const ticks = Array.from({ length: Math.round(top / step) + 1 }, (_, i) => i * step);
  // Price labels: greedy by priority (active bin, edges, midpoints), keeping only those
  // with room around them so they never collide at narrow widths.
  const LABEL_W = 78;
  const labelCenter = (i: number) =>
    i === 0 ? M.left + LABEL_W / 2 : i === rows.length - 1 ? M.left + plotW - LABEL_W / 2 : x(i) + barW / 2;
  const xLabelIdx: number[] = [];
  for (const i of [activeIdx, 0, n - 1, Math.floor(n / 2), Math.floor(n / 4), Math.floor((3 * n) / 4)]) {
    if (i < 0 || i >= rows.length || xLabelIdx.includes(i)) continue;
    if (xLabelIdx.every((j) => Math.abs(labelCenter(i) - labelCenter(j)) >= LABEL_W + 8)) xLabelIdx.push(i);
  }

  const h = hover != null ? rows[hover] : null;
  const unit = decimalsY != null ? symbolY : `raw ${symbolY}`;

  const pickFromPointer = (clientX: number) => {
    const rect = wrap.current?.getBoundingClientRect();
    if (!rect) return;
    const i = Math.floor((clientX - rect.left - M.left) / band);
    setHover(i >= 0 && i < rows.length ? i : null);
  };

  if (rows.length === 0) {
    return (
      <div ref={measure} className="state">
        No liquidity in bins around the active price.
      </div>
    );
  }

  return (
    <div ref={measure} style={{ position: "relative" }}>
      {/* Legend: position encodes the token, so it's spelled out here, not by color. */}
      <div className="legend" style={{ marginBottom: 8 }}>
        <span className="legend__key">
          <span className="legend__swatch" style={{ background: "var(--series)" }} />
          Liquidity per bin, in {unit}
        </span>
        <span className="legend__key">
          <span className="legend__swatch" style={{ background: "var(--ink)" }} />
          Active bin
        </span>
        <span className="muted">
          ← {symbolY} sits below the price · {symbolX} sits above →
        </span>
      </div>
      <svg
        width={width}
        height={H}
        role="img"
        tabIndex={0}
        aria-label={`Liquidity across ${rows.length} price bins around the active bin, measured in ${unit}. Use arrow keys to inspect bins.`}
        onPointerMove={(e) => pickFromPointer(e.clientX)}
        onPointerLeave={() => setHover(null)}
        onKeyDown={(e) => {
          if (e.key === "ArrowRight" || e.key === "ArrowLeft") {
            e.preventDefault();
            const d = e.key === "ArrowRight" ? 1 : -1;
            setHover((cur) => Math.min(rows.length - 1, Math.max(0, (cur ?? activeIdx) + d)));
          } else if (e.key === "Escape") setHover(null);
        }}
        onBlur={() => setHover(null)}
        style={{ display: "block", outline: "none" }}
      >

        {/* grid + y ticks (right side, like the price chart) */}
        {ticks.map((t) => (
          <g key={t}>
            <line x1={M.left} x2={M.left + plotW} y1={y(t)} y2={y(t)} stroke={t === 0 ? "var(--axis)" : "var(--grid)"} strokeWidth={1} />
            <text x={M.left + plotW + 8} y={y(t) + 4} fontSize={11} fill="var(--ink-3)" className="num">
              {compact(t)}
            </text>
          </g>
        ))}

        {rows.map((r, i) => {
          if (r.value <= 0) return null;
          const top = y(r.value);
          const hgt = Math.max(1, M.top + plotH - top);
          const rad = Math.min(4, barW / 2, hgt);
          const isActive = r.bin_id === activeId;
          const x0 = x(i);
          // rounded data-end, square at the baseline
          const d = `M${x0},${top + hgt} V${top + rad} Q${x0},${top} ${x0 + rad},${top} H${x0 + barW - rad} Q${x0 + barW},${top} ${x0 + barW},${top + rad} V${top + hgt} Z`;
          return (
            <path
              key={r.bin_id}
              d={d}
              fill={isActive ? "var(--ink)" : "var(--series)"}
              opacity={hover == null || hover === i ? 1 : 0.55}
            />
          );
        })}

        {/* active bin marker */}
        {activeIdx >= 0 && (
          <g>
            <line
              x1={x(activeIdx) + barW / 2}
              x2={x(activeIdx) + barW / 2}
              y1={M.top - 6}
              y2={M.top + plotH}
              stroke="var(--ink)"
              strokeWidth={1}
              opacity={0.35}
            />
            <text
              x={x(activeIdx) + barW / 2 + (activeIdx > n * 0.85 ? 4 : activeIdx < n * 0.15 ? -4 : 0)}
              y={M.top - 9}
              fontSize={11}
              fontWeight={600}
              fill="var(--ink)"
              textAnchor={activeIdx > n * 0.85 ? "end" : activeIdx < n * 0.15 ? "start" : "middle"}
            >
              Active bin · {fmtPrice(rows[activeIdx]!.price ?? rows[activeIdx]!.price_raw)}
            </text>
          </g>
        )}

        {/* x labels: prices at a few bins */}
        {xLabelIdx.map((i) => {
          const r = rows[i]!;
          const anchor = i === 0 ? "start" : i === rows.length - 1 ? "end" : "middle";
          return (
            <text
              key={i}
              x={i === 0 ? M.left : i === rows.length - 1 ? M.left + plotW : x(i) + barW / 2}
              y={H - 10}
              fontSize={11}
              fill={i === activeIdx ? "var(--ink)" : "var(--ink-3)"}
              fontWeight={i === activeIdx ? 600 : 400}
              textAnchor={anchor}
              className="num"
            >
              {fmtPrice(r.price ?? r.price_raw)}
            </text>
          );
        })}
      </svg>

      {h && hover != null && (
        <div
          className="tip"
          style={{
            left: Math.min(Math.max(0, x(hover) + barW / 2 - 90), width - 190),
            top: Math.max(0, y(h.value) - 118),
          }}
        >
          <strong>
            {compact(h.value)} {unit}
          </strong>
          <div>
            Bin {h.bin_id}
            {h.bin_id === activeId ? " · active" : ` · ${h.bin_id > activeId ? "+" : ""}${h.bin_id - activeId} from active`}
          </div>
          <div className="tip__row">
            <span>Price</span>
            <span>{fmtPrice(h.price ?? h.price_raw)}</span>
          </div>
          <div className="tip__row">
            <span>{symbolX}</span>
            <span>{amount(h.amount_x, decimalsX)}</span>
          </div>
          <div className="tip__row">
            <span>{symbolY}</span>
            <span>{amount(h.amount_y, decimalsY)}</span>
          </div>
        </div>
      )}
    </div>
  );
}

export function LiquidityTable({ bins, activeId, symbolX, symbolY, decimalsX, decimalsY }: Props) {
  return (
    <div className="table-wrap" style={{ maxHeight: 260 }}>
      <table className="table">
        <thead>
          <tr>
            <th>Bin</th>
            <th className="r">Price</th>
            <th className="r">{symbolX}</th>
            <th className="r">{symbolY}</th>
            <th className="r">Value ({symbolY})</th>
          </tr>
        </thead>
        <tbody>
          {bins.map((b) => {
            const vy = Number(b.amount_y) + Number(b.amount_x) * b.price_raw;
            return (
              <tr key={b.bin_id} style={b.bin_id === activeId ? { fontWeight: 600 } : undefined}>
                <td className="num">
                  {b.bin_id}
                  {b.bin_id === activeId && <span className="badge badge--accent" style={{ marginLeft: 8 }}>Active</span>}
                </td>
                <td className="r">{fmtPrice(b.price ?? b.price_raw)}</td>
                <td className="r">{amount(b.amount_x, decimalsX)}</td>
                <td className="r">{amount(b.amount_y, decimalsY)}</td>
                <td className="r">{compact(decimalsY != null ? vy / 10 ** decimalsY : vy)}</td>
              </tr>
            );
          })}
        </tbody>
      </table>
    </div>
  );
}
