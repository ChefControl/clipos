// Stand-in for @auth0/auth0-react in the `e2e` build (vite.config.ts aliases it), so the
// phone tests run the real app without Auth0. `localStorage["e2e-signed-out"] = "1"`
// renders the signed-out screens; `"e2e-session-gone"` makes every token request fail as
// an expired session does, and `"e2e-no-token"` makes it come back empty;
// `"e2e-auth-error"` is the error Auth0 came back from sign-in with (signed out).
// Sign-in redirects are recorded in `window.__e2eSignIns`, sign-outs in
// `window.__e2eSignOuts`.
import type { ReactNode } from "react";

const flag = (name: string) => window.localStorage.getItem(name);
const signedOut = () => flag("e2e-signed-out") === "1";

type Recorded = { __e2eSignIns?: unknown[]; __e2eSignOuts?: unknown[] };

export function Auth0Provider({ children }: { children: ReactNode }) {
  return <>{children}</>;
}

export function useAuth0() {
  const out = signedOut();
  const error = flag("e2e-auth-error");
  return {
    isLoading: false,
    isAuthenticated: !out,
    error: out && error ? new Error(error) : undefined,
    user: out ? undefined : { email: "robin@example.com", name: "Robin" },
    getAccessTokenSilently: async () => {
      if (flag("e2e-session-gone") === "1") {
        throw Object.assign(new Error("Login required"), { error: "login_required" });
      }
      return flag("e2e-no-token") === "1" ? "" : "e2e-token";
    },
    loginWithRedirect: async (options?: unknown) => {
      const w = window as unknown as Recorded;
      w.__e2eSignIns = [...(w.__e2eSignIns ?? []), options];
    },
    logout: (options?: unknown) => {
      const w = window as unknown as Recorded;
      w.__e2eSignOuts = [...(w.__e2eSignOuts ?? []), options];
    },
  };
}
