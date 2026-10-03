import { useAuth0 } from "@auth0/auth0-react";
import { createContext, type ReactNode, useContext, useMemo } from "react";
import { type ApiClient, createApiClient } from "../api/client";
import { isAuth0SignInError, SignInRequired } from "../api/errors";

const ApiContext = createContext<ApiClient | null>(null);

export function ApiProvider({ children }: { children: ReactNode }) {
  const { getAccessTokenSilently } = useAuth0();
  const client = useMemo(
    () =>
      createApiClient(async () => {
        let token: string | undefined;
        try {
          token = await getAccessTokenSilently();
        } catch (e) {
          // The session is gone (not just the network): the app signs in again.
          throw isAuth0SignInError(e) ? new SignInRequired(e) : e;
        }
        if (!token) {
          throw new SignInRequired();
        }
        return token;
      }),
    [getAccessTokenSilently],
  );
  return <ApiContext.Provider value={client}>{children}</ApiContext.Provider>;
}

export function useApi(): ApiClient {
  const client = useContext(ApiContext);
  if (!client) {
    throw new Error("useApi must be used inside <ApiProvider>");
  }
  return client;
}
