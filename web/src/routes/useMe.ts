import { useMutation, useQuery, useQueryClient } from "@tanstack/react-query";
import { call, retryTransient } from "../api/errors";
import type { components } from "../api/schema";
import { useApi } from "../auth/ApiProvider";

export type User = components["schemas"]["User"];
export type UpdateProfile = components["schemas"]["UpdateProfile"];

/** The signed-in clipos user. The first call creates the account. */
export function useMe() {
  const api = useApi();
  return useQuery({
    queryKey: ["me"],
    queryFn: () => call(api.GET("/api/me")),
    staleTime: 5 * 60 * 1000,
    // A server or network hiccup is tried again (1, 2, 4 s apart); a refused sign-in (not
    // invited, disabled) or a 401 won't change by retrying.
    retry: retryTransient(3),
  });
}

export function useUpdateMe() {
  const api = useApi();
  const queryClient = useQueryClient();
  return useMutation({
    mutationFn: (body: UpdateProfile) => call(api.PATCH("/api/me", { body })),
    onSuccess: (user) => {
      queryClient.setQueryData(["me"], user);
      // Your name, handle and Steam name show on profiles and in friend pickers too.
      queryClient.invalidateQueries({ queryKey: ["profile"] });
      queryClient.invalidateQueries({ queryKey: ["members"] });
    },
  });
}
