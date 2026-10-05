import { useInfiniteQuery, useMutation, useQuery, useQueryClient } from "@tanstack/react-query";
import { call } from "../api/errors";
import type { components } from "../api/schema";
import { useApi } from "../auth/ApiProvider";

export type Clip = components["schemas"]["ClipView"];
export type Member = components["schemas"]["Member"];
export type Profile = components["schemas"]["Profile"];
export type ReactionCount = components["schemas"]["ReactionCount"];
export type UpdateClip = components["schemas"]["UpdateClip"];
export type Sort = "new" | "top";

export const EMOJIS = ["🔥", "😂", "💀", "🐐", "😮", "👏"] as const;

export interface ClipFilters {
  sort?: Sort;
  map?: string;
  /** One tag, or several comma-separated (any of them). */
  tag?: string;
  player?: string;
  uploader?: string;
  /** Search words. */
  q?: string;
  /** One of EMOJIS. */
  reaction?: string;
  /** Clips of the night, or fails (clips someone pressed 🍌 on in a show). */
  night?: "clip" | "fail";
}

/** Pages of clips for the archive and profiles. Refreshes while one of yours is processing. */
export function useClips(filters: ClipFilters) {
  const api = useApi();
  return useInfiniteQuery({
    queryKey: ["clips", "list", filters],
    initialPageParam: undefined as string | undefined,
    queryFn: ({ pageParam }) =>
      call(api.GET("/api/clips", { params: { query: { ...filters, cursor: pageParam } } })),
    getNextPageParam: (last) => last.nextCursor ?? undefined,
    refetchInterval: (query) =>
      query.state.data?.pages.some((p) => p.clips.some((c) => c.status === "processing"))
        ? 5000
        : false,
  });
}

/** One clip, polled until processing finishes: every few seconds while the worker has it,
 *  less often while the file is still uploading (that can take minutes, or never end). */
export function useClip(id: string, { enabled = true }: { enabled?: boolean } = {}) {
  const api = useApi();
  return useQuery({
    queryKey: ["clips", "one", id],
    enabled,
    queryFn: () => call(api.GET("/api/clips/{id}", { params: { path: { id } } })),
    refetchInterval: (query) => {
      const status = query.state.data?.status;
      return status === "processing" ? 4000 : status === "uploading" ? 10_000 : false;
    },
    // Playback links are signed for 2 h; refresh well before they expire.
    staleTime: 30 * 60 * 1000,
  });
}

/** Runs a clip mutation, then refreshes that clip, every list, the trash and profiles
 *  (their counts change on delete, restore and release). */
function useClipMutation<V>(id: string, fn: (v: V) => Promise<Clip>) {
  const queryClient = useQueryClient();
  return useMutation({
    mutationFn: fn,
    onSuccess: (clip) => {
      queryClient.setQueryData(["clips", "one", id], clip);
      queryClient.invalidateQueries({ queryKey: ["clips", "list"] });
      queryClient.invalidateQueries({ queryKey: ["trash"] });
      queryClient.invalidateQueries({ queryKey: ["profile"] });
    },
  });
}

export function useRetryClip(id: string) {
  const api = useApi();
  return useClipMutation(id, () =>
    call(api.POST("/api/clips/{id}/retry", { params: { path: { id } } })),
  );
}

export function useUpdateClip(id: string) {
  const api = useApi();
  return useClipMutation(id, (body: UpdateClip) =>
    call(api.PATCH("/api/clips/{id}", { params: { path: { id } }, body })),
  );
}

export function useDeleteClip(id: string) {
  const api = useApi();
  return useClipMutation(id, () =>
    call(api.DELETE("/api/clips/{id}", { params: { path: { id } } })),
  );
}

export function useRestoreClip(id: string) {
  const api = useApi();
  return useClipMutation(id, () =>
    call(api.POST("/api/clips/{id}/restore", { params: { path: { id } } })),
  );
}

/** "Save it for the show" / "Post now" (S3). */
export function useHoldClip(id: string) {
  const api = useApi();
  const params = { params: { path: { id } } };
  return {
    hold: useClipMutation(id, () => call(api.POST("/api/clips/{id}/hold", params))),
    release: useClipMutation(id, () => call(api.POST("/api/clips/{id}/release", params))),
  };
}

export function useShareClip(id: string) {
  const api = useApi();
  const params = { params: { path: { id } } };
  return {
    share: useClipMutation(id, () => call(api.POST("/api/clips/{id}/share", params))),
    unshare: useClipMutation(id, () => call(api.DELETE("/api/clips/{id}/share", params))),
  };
}

export function useReact(id: string) {
  const api = useApi();
  const queryClient = useQueryClient();
  return useMutation({
    mutationFn: ({ emoji, on }: { emoji: string; on: boolean }) => {
      const params = { params: { path: { id, emoji } } };
      return call(
        on
          ? api.PUT("/api/clips/{id}/reactions/{emoji}", params)
          : api.DELETE("/api/clips/{id}/reactions/{emoji}", params),
      );
    },
    onSuccess: (reactions) => {
      queryClient.setQueryData<Clip>(["clips", "one", id], (clip) =>
        clip
          ? { ...clip, reactions, reactionCount: reactions.reduce((n, r) => n + r.count, 0) }
          : clip,
      );
      queryClient.invalidateQueries({ queryKey: ["clips", "list"] });
    },
  });
}

export async function downloadOriginal(api: ReturnType<typeof useApi>, id: string) {
  const { url } = await call(api.GET("/api/clips/{id}/download", { params: { path: { id } } }));
  window.location.assign(url);
}

export function useMembers() {
  const api = useApi();
  return useQuery({
    queryKey: ["members"],
    queryFn: () => call(api.GET("/api/users")),
    staleTime: 10 * 60 * 1000,
  });
}

export function useProfile(handle: string) {
  const api = useApi();
  return useQuery({
    queryKey: ["profile", handle],
    queryFn: () => call(api.GET("/api/users/{handle}", { params: { path: { handle } } })),
  });
}

export function useTrash(enabled: boolean) {
  const api = useApi();
  return useQuery({
    queryKey: ["trash"],
    queryFn: () => call(api.GET("/api/me/trash")),
    enabled,
  });
}

export function useTagSuggestions(prefix: string) {
  const api = useApi();
  return useQuery({
    queryKey: ["tags", prefix],
    queryFn: () => call(api.GET("/api/tags", { params: { query: { q: prefix } } })),
    staleTime: 60 * 1000,
  });
}
