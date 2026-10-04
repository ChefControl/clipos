import { useMutation, useQuery, useQueryClient } from "@tanstack/react-query";
import { call } from "../api/errors";
import type { components } from "../api/schema";
import { useApi } from "../auth/ApiProvider";

export type Show = components["schemas"]["ShowView"];
export type PastShow = components["schemas"]["PastShow"];
export type Tonight = components["schemas"]["Tonight"];
export type LineupEntry = components["schemas"]["LineupEntry"];

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
