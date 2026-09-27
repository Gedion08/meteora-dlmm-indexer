// Human labels and amounts for decoded DLMM events.
import { amount, compact } from "./format";
import { tokenLabel } from "./tokens";

export interface EventLike {
  name: string;
  data: Record<string, unknown>;
  symbol_x: string | null;
  symbol_y: string | null;
  decimals_x: number | null;
  decimals_y: number | null;
  token_x_mint?: string | null;
  token_y_mint?: string | null;
}

export function eventLabel(name: string): string {
  return (
    {
      AddLiquidity: "Add liquidity",
      RemoveLiquidity: "Remove liquidity",
      Rebalancing: "Rebalance",
      ClaimFee: "Claim fees",
      ClaimFee2: "Claim fees",
      ClaimReward: "Claim reward",
      ClaimReward2: "Claim reward",
      PositionCreate: "Open position",
      PositionClose: "Close position",
      Swap: "Swap",
      Swap2Evt: "Swap details",
    }[name] ?? name
  );
}

export function eventAmounts(e: EventLike): string {
  const sx = tokenLabel(e.symbol_x, e.token_x_mint);
  const sy = tokenLabel(e.symbol_y, e.token_y_mint);
  const d = e.data;
  const s = (v: unknown) => (v == null ? null : String(v));
  const pairOf = (x: unknown, y: unknown) => `${amount(s(x), e.decimals_x)} ${sx} · ${amount(s(y), e.decimals_y)} ${sy}`;
  if (Array.isArray(d.amounts)) return pairOf(d.amounts[0], d.amounts[1]);
  if ("fee_x" in d) return pairOf(d.fee_x, d.fee_y);
  if ("x_added_amount" in d)
    return `+${amount(s(d.x_added_amount), e.decimals_x)} / −${amount(s(d.x_withdrawn_amount), e.decimals_x)} ${sx}`;
  if ("total_reward" in d) return `${compact(Number(d.total_reward))} (reward ${String(d.reward_index)})`;
  return "—";
}
