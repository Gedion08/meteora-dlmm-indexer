// Small shared building blocks.
import { useEffect, useState, type ReactNode } from "react";
import { Link } from "react-router";
import { pct, short, timeAgo } from "../lib/format";
import { initials, mintMarkStyle, tokenLabel } from "../lib/tokens";
import { AlertIcon, CheckIcon, CopyIcon } from "./icons";

export function TokenMark({
  mint,
  symbol,
  logo,
  size,
}: {
  mint: string | null;
  symbol: string | null;
  logo?: string | null;
  size?: "lg";
}) {
  const [broken, setBroken] = useState(false);
  const label = tokenLabel(symbol, mint);
  return (
    <span className={`tokmark${size === "lg" ? " tokmark--lg" : ""}`} style={mint ? mintMarkStyle(mint) : undefined} title={label}>
      {logo && !broken ? (
        <img src={logo} alt="" loading="lazy" referrerPolicy="no-referrer" onError={() => setBroken(true)} />
      ) : (
        initials(label)
      )}
    </span>
  );
}

export function PairName({
  x,
  y,
  size,
}: {
  x: { mint: string | null; symbol: string | null; logo?: string | null };
  y: { mint: string | null; symbol: string | null; logo?: string | null };
  size?: "lg";
}) {
  return (
    <span className="pair">
      <span className="pair__marks">
        <TokenMark {...x} size={size} />
        <TokenMark {...y} size={size} />
      </span>
      <span className="pair__name">
        {tokenLabel(x.symbol, x.mint)}
        <span className="sep">/</span>
        {tokenLabel(y.symbol, y.mint)}
      </span>
    </span>
  );
}

export function CopyButton({ text, label }: { text: string; label: string }) {
  const [done, setDone] = useState(false);
  return (
    <button
      type="button"
      className="copy-btn"
      aria-label={done ? "Copied" : `Copy ${label}`}
      title={done ? "Copied" : `Copy ${label}`}
      onClick={(e) => {
        e.stopPropagation();
        e.preventDefault();
        navigator.clipboard?.writeText(text).then(
          () => {
            setDone(true);
            setTimeout(() => setDone(false), 1400);
          },
          () => undefined,
        );
      }}
    >
      {done ? <CheckIcon /> : <CopyIcon />}
    </button>
  );
}

type AddrKind = "wallet" | "pool" | "position" | "tx" | "none";
const hrefFor = (kind: AddrKind, a: string) =>
  kind === "wallet" ? `/wallet/${a}` : kind === "pool" ? `/pool/${a}` : kind === "position" ? `/position/${a}` : kind === "tx" ? `/tx/${a}` : null;

export function Address({
  value,
  kind = "none",
  head = 4,
  tail = 4,
  full,
}: {
  value: string | null | undefined;
  kind?: AddrKind;
  head?: number;
  tail?: number;
  full?: boolean;
}) {
  if (!value) return <span className="muted">—</span>;
  const text = full ? value : short(value, head, tail);
  const href = hrefFor(kind, value);
  return (
    <span className="addr" title={value}>
      {href ? (
        <Link to={href} onClick={(e) => e.stopPropagation()}>
          {text}
        </Link>
      ) : (
        <span>{text}</span>
      )}
      <CopyButton text={value} label="address" />
    </span>
  );
}

/** Signed change with a direction glyph, so polarity never relies on color alone. */
export function Delta({ value, digits = 2 }: { value: number | null | undefined; digits?: number }) {
  if (value == null || !Number.isFinite(value)) return <span className="muted">—</span>;
  const dir = value > 1e-9 ? "up" : value < -1e-9 ? "down" : "";
  return (
    <span className={`num ${dir}`}>
      {dir === "up" ? "▲ " : dir === "down" ? "▼ " : ""}
      {pct(value, digits)}
    </span>
  );
}

export function useNow(intervalMs = 5000): number {
  const [now, setNow] = useState(() => Date.now() / 1000);
  useEffect(() => {
    const t = setInterval(() => setNow(Date.now() / 1000), intervalMs);
    return () => clearInterval(t);
  }, [intervalMs]);
  return now;
}

export function TimeAgo({ unix }: { unix: number | null | undefined }) {
  const now = useNow();
  if (unix == null) return <span className="muted">—</span>;
  return (
    <time dateTime={new Date(unix * 1000).toISOString()} title={new Date(unix * 1000).toLocaleString()}>
      {timeAgo(unix, now)}
    </time>
  );
}

export function Segmented<T extends string>({
  value,
  options,
  onChange,
  label,
}: {
  value: T;
  options: { value: T; label: string }[];
  onChange: (v: T) => void;
  label: string;
}) {
  return (
    <div className="segmented" role="group" aria-label={label}>
      {options.map((o) => (
        <button key={o.value} type="button" aria-pressed={o.value === value} onClick={() => onChange(o.value)}>
          {o.label}
        </button>
      ))}
    </div>
  );
}

export function Panel({ title, actions, children, flush }: { title?: ReactNode; actions?: ReactNode; children: ReactNode; flush?: boolean }) {
  return (
    <section className="panel">
      {(title || actions) && (
        <div className="panel__head">
          <h2 className="panel__title">{title}</h2>
          {actions}
        </div>
      )}
      {flush ? children : <div className="panel__body">{children}</div>}
    </section>
  );
}

export function EmptyState({ title, children }: { title: string; children?: ReactNode }) {
  return (
    <div className="state">
      <h3>{title}</h3>
      {children && <p>{children}</p>}
    </div>
  );
}

export function ErrorState({ error, onRetry }: { error: unknown; onRetry?: () => void }) {
  const msg = error instanceof Error ? error.message : "Something went wrong loading this data.";
  return (
    <div className="state" role="alert">
      <h3 style={{ display: "flex", alignItems: "center", gap: 8 }}>
        <AlertIcon style={{ color: "var(--critical)" }} /> Couldn't load this
      </h3>
      <p>{msg}</p>
      {onRetry && (
        <button type="button" className="btn" onClick={onRetry}>
          Try again
        </button>
      )}
    </div>
  );
}

export function SkeletonRows({ rows = 8, cols = 6 }: { rows?: number; cols?: number }) {
  return (
    <>
      {Array.from({ length: rows }, (_, r) => (
        <tr key={r} aria-hidden="true">
          {Array.from({ length: cols }, (_, c) => (
            <td key={c}>
              <span className="skeleton" style={{ width: c === 0 ? 140 : 70, marginLeft: c === 0 ? 0 : "auto" }} />
            </td>
          ))}
        </tr>
      ))}
    </>
  );
}
