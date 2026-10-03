import { useQuery } from "@tanstack/react-query";
import { api, endpoints, type Status } from "../lib/api";
import { dateTime } from "../lib/format";
import { AlertIcon } from "./icons";

/** Says plainly when some of the data is missing and being recovered. */
export function GapBanner() {
  const status = useQuery({
    queryKey: ["status"],
    queryFn: ({ signal }) => api<Status>(endpoints.status(), signal),
    refetchInterval: 5000,
  });
  const gaps = status.data?.gaps ?? [];
  if (gaps.length === 0) return null;
  const from = Math.min(...gaps.map((g) => g.from_time ?? Infinity));
  const to = Math.max(...gaps.map((g) => g.to_time ?? 0));
  const progress = gaps.reduce((a, g) => a + g.progress, 0) / gaps.length;
  return (
    <div className="gap-banner" role="status">
      <div className="gap-banner__inner">
        <AlertIcon className="gap-banner__icon" />
        <span>
          <strong>Recovering missed data</strong> from {Number.isFinite(from) ? dateTime(from) : "earlier"} to{" "}
          {to ? dateTime(to) : "now"} · {(progress * 100).toFixed(1)}% restored. Charts and 24h figures that cover this
          period are incomplete until it finishes.
        </span>
        <span className="gap-banner__bar" aria-hidden="true">
          <span style={{ width: `${Math.max(2, progress * 100)}%` }} />
        </span>
      </div>
    </div>
  );
}
