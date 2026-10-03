import { useMutation, useQuery, useQueryClient } from "@tanstack/react-query";
import { ApiError, call } from "../../api/errors";
import type { components } from "../../api/schema";
import { useApi } from "../../auth/ApiProvider";

export type ClipAnalysis = components["schemas"]["ClipAnalysis"];
export type Kill = components["schemas"]["KillView"];
export type AnalysisStats = components["schemas"]["AnalysisStats"];

/** Seconds of lead-in when jumping to a kill, so the fight is seen, not just the row. */
export const LEAD_IN_S = 2;

/** Killfeed analysis of a clip: `null` when it was never analysed (not CS2, or not the
 *  uploader's point of view). Polls while the worker is still reading it. */
export function useClipAnalysis(id: string, enabled: boolean) {
  const api = useApi();
  return useQuery({
    queryKey: ["clips", "analysis", id],
    enabled,
    queryFn: async () => {
      try {
        return await call(api.GET("/api/clips/{id}/analysis", { params: { path: { id } } }));
      } catch (e) {
        if (e instanceof ApiError && e.status === 404) return null;
        throw e;
      }
    },
    refetchInterval: (query) => (query.state.data?.status === "pending" ? 10_000 : false),
  });
}

/** Admin only: has the worker read the clip's killfeed again. The analysis is fetched
 *  again, which shows it pending (and polls) until the new result is in. */
export function useReanalyse(id: string) {
  const api = useApi();
  const queryClient = useQueryClient();
  return useMutation({
    mutationFn: () => call(api.POST("/api/admin/clips/{id}/analyse", { params: { path: { id } } })),
    onSuccess: () => queryClient.invalidateQueries({ queryKey: ["clips", "analysis", id] }),
  });
}

/** Where to seek to show a kill. */
export function jumpTime(kill: Kill): number {
  return Math.max(0, kill.t - LEAD_IN_S);
}

/** The next kill whose jump point is after `time` (the "next kill" button), if any. */
export function nextKill(kills: Kill[], time: number): Kill | undefined {
  return kills.find((k) => jumpTime(k) > time + 0.25);
}

/** The kill before the one being watched (the "previous kill" button), if any. */
export function previousKill(kills: Kill[], time: number): Kill | undefined {
  // Pressed during a kill's lead-in or just after it, go to the one before that.
  return kills.findLast((k) => k.t < time - 1);
}

/** The kill being watched: the latest one whose row has appeared, for a few seconds. */
export function activeKill(kills: Kill[], time: number): Kill | undefined {
  const k = kills.findLast((k) => k.t <= time + 0.1);
  return k && time - k.t < 6 ? k : undefined;
}

export function isMine(kill: Kill): boolean {
  return kill.owner !== "other";
}
