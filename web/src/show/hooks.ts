import { useMutation, useQuery, useQueryClient } from "@tanstack/react-query";
import { call } from "../api/errors";
import type { components } from "../api/schema";
import { useApi } from "../auth/ApiProvider";

export type Show = components["schemas"]["ShowView"];
export type PastShow = components["schemas"]["PastShow"];
export type Tonight = components["schemas"]["Tonight"];
export type LineupEntry = components["schemas"]["LineupEntry"];
export type Category = "clip" | "fail";
export type TieBreak = components["schemas"]["TieBreak"];

/** Tonight: the show that's on, or the clips the next one would play. Polled, so a show
 *  someone else opens turns up without a reload. */
export function useTonight() {
  const api = useApi();
  return useQuery({
    queryKey: ["tonight"],
    queryFn: () => call(api.GET("/api/shows/tonight")),
    refetchInterval: 15_000,
  });
}

/** The show that's on, in brief, for every page's Live pill: asked every 30 s while the
 *  show is open to you. */
export function useCurrentShow(enabled: boolean) {
  const api = useApi();
  return useQuery({
    queryKey: ["shows", "current"],
    queryFn: () => call(api.GET("/api/shows/current")),
    enabled,
    refetchInterval: 30_000,
  });
}

/** One show. The live room refetches it whenever the server says it changed. */
export function useShow(id: string, initialData?: Show) {
  const api = useApi();
  return useQuery({
    queryKey: ["show", id],
    queryFn: () => call(api.GET("/api/shows/{id}", { params: { path: { id } } })),
    initialData,
  });
}

/** Shows that ended, newest first. */
export function usePastShows() {
  const api = useApi();
  return useQuery({
    queryKey: ["shows", "past"],
    queryFn: () => call(api.GET("/api/shows")),
  });
}

/** Changes to one show; each answers with the show, which replaces the cached one. */
export function useShowActions(id: string) {
  const api = useApi();
  const queryClient = useQueryClient();
  const path = { params: { path: { id } } };
  const done = (show: Show) => {
    queryClient.setQueryData(["show", id], show);
    queryClient.invalidateQueries({ queryKey: ["tonight"] });
    queryClient.invalidateQueries({ queryKey: ["shows", "current"] });
  };
  return {
    setLineup: useMutation({
      mutationFn: (clipIds: string[]) =>
        call(api.PUT("/api/shows/{id}/lineup", { ...path, body: { clipIds } })),
      onSuccess: done,
    }),
    addClip: useMutation({
      mutationFn: (clipId: string) =>
        call(api.POST("/api/shows/{id}/clips", { ...path, body: { clipId } })),
      onSuccess: done,
    }),
    start: useMutation({
      mutationFn: () => call(api.POST("/api/shows/{id}/start", path)),
      onSuccess: done,
    }),
    /** Your vote in the finale; voting again changes it. */
    vote: useMutation({
      mutationFn: ({ category, clipId }: { category: Category; clipId: string }) =>
        call(
          api.PUT("/api/shows/{id}/votes/{category}", {
            params: { path: { id, category } },
            body: { clipId },
          }),
        ),
      onSuccess: done,
    }),
    /** The host ends the finale: winners are stored. A tie fails with the tied clips
     *  (`ApiError.tied`) until the host picks in `tieBreak`. */
    end: useMutation({
      mutationFn: (tieBreak?: TieBreak) =>
        call(api.POST("/api/shows/{id}/end", { ...path, body: { tieBreak: tieBreak ?? {} } })),
      onSuccess: done,
    }),
    /** To the finale: after the last clip, or to end the show early. */
    finale: useMutation({
      mutationFn: () => call(api.POST("/api/shows/{id}/finale", path)),
      onSuccess: done,
    }),
  };
}

/** Opens a show with tonight's clips; you host it. */
export function useCreateShow() {
  const api = useApi();
  const queryClient = useQueryClient();
  return useMutation({
    mutationFn: () => call(api.POST("/api/shows")),
    onSuccess: (show) => {
      queryClient.setQueryData(["show", show.id], show);
      queryClient.setQueryData<Tonight>(["tonight"], (t) => t && { ...t, show, clips: [] });
      queryClient.invalidateQueries({ queryKey: ["tonight"] });
      queryClient.invalidateQueries({ queryKey: ["shows", "current"] });
    },
  });
}

/** The link friends open to join the show. */
export function joinLink(showId: string): string {
  return `${window.location.origin}/shows/${showId}`;
}

/** `ids` with `id` moved to index `to` (clamped to the ends). */
export function moveTo(ids: string[], id: string, to: number): string[] {
  const rest = ids.filter((c) => c !== id);
  rest.splice(Math.max(0, Math.min(to, rest.length)), 0, id);
  return rest;
}
