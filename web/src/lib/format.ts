// Number formatting for on-chain values. Raw token amounts are u64 strings and are
// scaled with BigInt so nothing loses precision before the final display rounding.

const SUBSCRIPT = "₀₁₂₃₄₅₆₇₈₉";

/** Raw integer string → decimal number string (exact). */
export function scaleRaw(raw: string, decimals: number): string {
  const neg = raw.startsWith("-");
  const digits = (neg ? raw.slice(1) : raw).replace(/^0+(?=\d)/, "");
  if (decimals <= 0) return (neg ? "-" : "") + digits;
  const padded = digits.padStart(decimals + 1, "0");
  const int = padded.slice(0, -decimals);
  const frac = padded.slice(-decimals).replace(/0+$/, "");
  return (neg ? "-" : "") + int + (frac ? `.${frac}` : "");
}

export function rawToNumber(raw: string | null | undefined, decimals: number | null | undefined): number | null {
  if (raw == null || decimals == null) return null;
  return Number(scaleRaw(raw, decimals));
}

/** Compact magnitudes: 1,284 · 12.9K · 4.21M · 1.05B. */
export function compact(n: number | null | undefined, digits = 2): string {
  if (n == null || !Number.isFinite(n)) return "—";
  const a = Math.abs(n);
  if (a >= 1e12) return `${(n / 1e12).toFixed(digits)}T`;
  if (a >= 1e9) return `${(n / 1e9).toFixed(digits)}B`;
  if (a >= 1e6) return `${(n / 1e6).toFixed(digits)}M`;
  if (a >= 1e4) return `${(n / 1e3).toFixed(1)}K`;
  if (a >= 1) return n.toLocaleString("en-US", { maximumFractionDigits: 2 });
  if (a === 0) return "0";
  return price(n);
}

/**
 * DEX-style price: 4 significant figures; for tiny values, leading zeros collapse into a
 * subscript count, e.g. 0.00000123 → "0.0₅123".
 */
export function price(n: number | null | undefined): string {
  if (n == null || !Number.isFinite(n)) return "—";
  if (n === 0) return "0";
  const a = Math.abs(n);
  const sign = n < 0 ? "-" : "";
  if (a >= 1e6) return sign + compact(a);
  if (a >= 1000) return sign + a.toLocaleString("en-US", { maximumFractionDigits: 2 });
  if (a >= 1) return sign + a.toLocaleString("en-US", { maximumFractionDigits: 4, minimumFractionDigits: 2 });
  // Zeros right after the decimal point: 0.5 → 0, 0.001 → 2, 0.0000480 → 4.
  let zeros = -Math.floor(Math.log10(a)) - 1;
  let digits = Math.round(a * 10 ** (zeros + 4)).toString();
  if (digits.length > 4) {
    // Rounded up into the next decade (0.099999 → 0.1000).
    zeros -= 1;
    digits = digits.slice(0, 4);
  }
  if (zeros < 0) return sign + "1.00";
  const sig = digits.replace(/0+$/, "") || "0";
  if (zeros >= 4) {
    const sub = String(zeros).split("").map((d) => SUBSCRIPT[Number(d)]).join("");
    return `${sign}0.0${sub}${sig}`;
  }
  return `${sign}0.${"0".repeat(zeros)}${sig}`;
}

/** Token amount from raw units, compacted. */
export function amount(raw: string | null | undefined, decimals: number | null | undefined): string {
  const n = rawToNumber(raw, decimals);
  if (n == null) return raw == null ? "—" : `${compact(Number(raw))} raw`;
  return compact(n);
}

export function pct(n: number | null | undefined, digits = 2): string {
  if (n == null || !Number.isFinite(n)) return "—";
  const v = n * 100;
  const s = Math.abs(v) < 10 ** -digits ? (0).toFixed(digits) : Math.abs(v).toFixed(digits);
  return `${v > 0 ? "+" : v < 0 ? "−" : ""}${s}%`;
}

export function int(n: number | null | undefined): string {
  if (n == null) return "—";
  return n.toLocaleString("en-US");
}

export function timeAgo(unix: number | null | undefined, now = Date.now() / 1000): string {
  if (unix == null) return "—";
  const d = Math.max(0, now - unix);
  if (d < 5) return "just now";
  if (d < 60) return `${Math.floor(d)}s ago`;
  if (d < 3600) return `${Math.floor(d / 60)}m ago`;
  if (d < 86400) return `${Math.floor(d / 3600)}h ago`;
  return `${Math.floor(d / 86400)}d ago`;
}

export function dateTime(unix: number | null | undefined): string {
  if (unix == null) return "—";
  return new Date(unix * 1000).toLocaleString("en-US", {
    month: "short",
    day: "numeric",
    hour: "2-digit",
    minute: "2-digit",
    second: "2-digit",
  });
}

export function short(addr: string | null | undefined, head = 4, tail = 4): string {
  if (!addr) return "—";
  if (addr.length <= head + tail + 1) return addr;
  // slice(-0) would return the whole string, so a zero tail needs its own case.
  return `${addr.slice(0, head)}…${tail > 0 ? addr.slice(-tail) : ""}`;
}

/** Price change between two bins: (1 + step/10⁴)^(Δbins) − 1, exact for DLMM. */
export function binRatio(binStep: number, deltaBins: number): number {
  return (1 + binStep / 10_000) ** deltaBins - 1;
}
