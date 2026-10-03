import { useQuery, useQueryClient } from "@tanstack/react-query";
import { useEffect, useRef } from "react";
import { api } from "./api";

interface Hydration {
  state: "pending" | "done";
  completed_at?: number;
  accounts?: number;
}

/**
 * Make sure the indexer has loaded this pool's (or wallet's) accounts from the chain.
 * Polls while the load is pending, then refreshes the given queries once it lands.
 */
export function useHydration(kind: "pair" | "owner", key: string, refresh: string[][]) {
  const qc = useQueryClient();
  const q = useQuery({
    queryKey: ["hydrate", kind, key],
    queryFn: ({ signal }) => api<Hydration>(`/v1/hydrate?${kind}=${key}`, signal),
    refetchInterval: (query) => (query.state.data?.state === "pending" ? 1500 : false),
    staleTime: 5 * 60_000,
  });
  // Refresh only when a load we were waiting on finishes (pending → done).
  const sawPending = useRef(false);
  const refreshKey = JSON.stringify(refresh);
  useEffect(() => {
    if (q.data?.state === "pending") sawPending.current = true;
    else if (q.data?.state === "done" && sawPending.current) {
      sawPending.current = false;
      for (const k of JSON.parse(refreshKey) as string[][]) qc.invalidateQueries({ queryKey: k });
    }
  }, [q.data, qc, refreshKey]);
  return { pending: q.data?.state === "pending" };
}
