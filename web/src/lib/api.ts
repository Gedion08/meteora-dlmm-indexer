// Typed client for the dlmm-api REST endpoints. Amounts arrive as decimal strings
// (u64-safe); prices and quote-token values as numbers.

const BASE = (import.meta.env.VITE_API_BASE as string | undefined)?.replace(/\/$/, "") ?? "";
const KEY = import.meta.env.VITE_API_KEY as string | undefined;

export class ApiError extends Error {
  constructor(
    public status: number,
    message: string,
  ) {
    super(message);
  }
}

export async function api<T>(path: string, signal?: AbortSignal): Promise<T> {
  let res: Response;
  try {
    res = await fetch(`${BASE}${path}`, {
      signal,
      headers: KEY ? { "x-api-key": KEY } : undefined,
    });
  } catch (e) {
    if ((e as Error).name === "AbortError") throw e;
    throw new ApiError(0, "Can't reach the Binscope API. Check that dlmm-api is running.");
  }
  if (!res.ok) {
    let msg = res.statusText;
    try {
      msg = ((await res.json()) as { error?: string }).error ?? msg;
    } catch {
      /* non-JSON error body */
    }
    throw new ApiError(res.status, msg);
  }
  return (await res.json()) as T;
}

export function wsUrl(): string {
  const base = BASE || window.location.origin;
  const u = new URL(`${base}/v1/ws`);
  u.protocol = u.protocol === "https:" ? "wss:" : "ws:";
  if (KEY) u.searchParams.set("api_key", KEY);
  return u.toString();
}

export interface TokenSide {
  symbol_x: string | null;
  symbol_y: string | null;
  decimals_x: number | null;
  decimals_y: number | null;
  token_x_mint?: string | null;
  token_y_mint?: string | null;
}

export interface Pair extends TokenSide {
  address: string;
  updated_slot: number;
  token_x_mint: string;
  token_y_mint: string;
  name_x: string | null;
  name_y: string | null;
  logo_x: string | null;
  logo_y: string | null;
  reserve_x: string;
  reserve_y: string;
  reserve_x_amount: string | null;
  reserve_y_amount: string | null;
  bin_step: number;
  active_id: number;
  price_raw: number;
  price: number | null;
  base_factor: number;
  base_fee_power_factor: number;
  protocol_share_bps: number;
  volatility_accumulator: number;
  status: number;
  pair_type: number;
  activation_type: number;
  activation_point: string;
  creator: string;
  oracle: string;
  tvl_in_y: number | null;
  trades_24h: number;
  traders_24h: number;
  last_trade_at: number | null;
  price_change_24h: number | null;
  volume_24h_y: number | null;
  fees_24h_y: number | null;
  stats_computed_at: number | null;
  stats_24h?: {
    trades: number;
    volume_x: string;
    volume_y: string;
    fees_x: string;
    fees_y: string;
    unique_traders: number;
  };
}

export interface Swap extends TokenSide {
  signature: string;
  slot: number;
  tx_index: number;
  ix_index: number;
  inner_index: number;
  block_time: number | null;
  lb_pair: string;
  trader: string;
  swap_for_y: boolean;
  amount_in: string;
  amount_out: string;
  fee: string;
  protocol_fee: string;
  host_fee: string;
  fee_pct: number;
  start_bin_id: number;
  end_bin_id: number;
  price_raw: number;
  price: number | null;
  cursor: string;
}

export interface DlmmEvent extends TokenSide {
  signature: string;
  slot: number;
  tx_index: number;
  ix_index: number;
  inner_index: number;
  block_time: number | null;
  name: string;
  lb_pair: string | null;
  position: string | null;
  wallet: string | null;
  data: Record<string, unknown>;
  cursor: string;
}

export interface Bin {
  bin_id: number;
  amount_x: string;
  amount_y: string;
  liquidity_supply: string;
  price_raw: number;
  price: number | null;
}

export interface Candle {
  time: number;
  open: number;
  high: number;
  low: number;
  close: number;
  volume_x: string;
  volume_y: string;
  trades: number;
}

export interface Position extends TokenSide {
  address: string;
  lb_pair: string;
  owner: string;
  updated_slot: number;
  closed: boolean;
  lower_bin_id: number;
  upper_bin_id: number;
  operator: string;
  fee_owner: string;
  total_claimed_fee_x: string;
  total_claimed_fee_y: string;
  lock_release_point: string;
  last_updated_at: number;
  extended: boolean;
  token_x_mint: string | null;
  token_y_mint: string | null;
  active_id: number | null;
  bin_step: number | null;
  price: number | null;
  amount_x: string | null;
  amount_y: string | null;
  events?: { signature: string; slot: number; inner_index: number; block_time: number | null; name: string; data: Record<string, unknown> }[];
}

export interface TxDetail {
  signature: string;
  slot: number;
  block_time: number | null;
  fee_payer: string;
  success: boolean;
  err: string | null;
  fee: number;
  compute_units: number | null;
  instructions: {
    ix_index: number;
    inner_index: number;
    name: string;
    invoked_by: string | null;
    lb_pair: string | null;
    wallet: string | null;
    accounts: Record<string, string | null>;
    remaining_accounts: string[];
    args: Record<string, unknown>;
  }[];
  events: (Omit<DlmmEvent, "signature" | "slot" | "tx_index" | "block_time" | "cursor"> & TokenSide)[];
  token_balances: { pre: TokenBalance[]; post: TokenBalance[] };
}

export interface TokenBalance {
  account: string | null;
  mint: string;
  owner: string;
  amount: string | null;
  decimals: number | null;
}

export interface Status {
  checkpoint_slot: number | null;
  checkpoint_updated_at: number | null;
  finalized_slot: number | null;
  latest_block_time: number | null;
  seconds_behind: number | null;
  decode_failures_24h: number;
  open_gaps: number;
  live_clients: number;
  gaps: { from_slot: number; to_slot: number; from_time: number | null; to_time: number | null; progress: number }[];
}

export interface GlobalStats {
  network: string | null;
  pairs: number;
  positions: number;
  trades_24h: number;
  traders_24h: number;
  active_pairs_24h: number;
  computed_at: number | null;
}

export type PairSort = "trades" | "volume" | "tvl" | "change" | "recent";
export type CandleInterval = "1m" | "5m" | "15m" | "1h" | "4h" | "1d";

const q = (params: Record<string, string | number | undefined>) => {
  const s = new URLSearchParams();
  for (const [k, v] of Object.entries(params)) if (v !== undefined && v !== "") s.set(k, String(v));
  const str = s.toString();
  return str ? `?${str}` : "";
};

export const endpoints = {
  status: () => "/v1/status",
  stats: () => "/v1/stats",
  pairs: (p: { q?: string; sort?: PairSort; limit?: number; offset?: number }) => `/v1/pairs${q(p)}`,
  pair: (a: string) => `/v1/pairs/${a}`,
  bins: (a: string, radius: number) => `/v1/pairs/${a}/bins${q({ radius })}`,
  swaps: (a: string, cursor?: string, limit = 50) => `/v1/pairs/${a}/swaps${q({ cursor, limit })}`,
  pairEvents: (a: string, names: string[], cursor?: string) =>
    `/v1/pairs/${a}/events${q({ names: names.join(","), cursor, limit: 50 })}`,
  candles: (a: string, interval: CandleInterval, from?: number) => `/v1/pairs/${a}/candles${q({ interval, from })}`,
  walletPositions: (w: string, includeClosed = false) =>
    `/v1/wallets/${w}/positions${q({ include_closed: includeClosed ? "true" : undefined })}`,
  walletSwaps: (w: string, cursor?: string) => `/v1/wallets/${w}/swaps${q({ cursor, limit: 50 })}`,
  walletEvents: (w: string, cursor?: string) => `/v1/wallets/${w}/events${q({ cursor, limit: 50 })}`,
  position: (a: string) => `/v1/positions/${a}`,
  tx: (s: string) => `/v1/tx/${s}`,
};
