import { Auth0Provider, useAuth0 } from "@auth0/auth0-react";
import { MutationCache, QueryCache, QueryClient, QueryClientProvider } from "@tanstack/react-query";
import { RouterProvider } from "@tanstack/react-router";
import { StrictMode, useRef, useState } from "react";
import { createRoot } from "react-dom/client";
import { needsSignIn, retryTransient } from "./api/errors";
import { ApiProvider } from "./auth/ApiProvider";
import { loadConfig } from "./config";
import "./index.css";
import { router } from "./router";
import { Privacy } from "./routes/Privacy";
import { SharedClip } from "./routes/SharedClip";
import { SignIn } from "./routes/SignIn";
import { ToastProvider } from "./ui/Toast";

function App() {
  const { isLoading, isAuthenticated, error, loginWithRedirect } = useAuth0();
  const signIn = useRef(loginWithRedirect);
  signIn.current = loginWithRedirect;
  // A 401 or a session that ran out, from any request: sign in again and come back to the
  // same page. Once; the redirect leaves the app.
  const [queryClient] = useState(() => {
    let leaving = false;
    const onError = (e: unknown) => {
      if (leaving || !needsSignIn(e)) return;
      leaving = true;
      void signIn.current({
        authorizationParams: { connection: "google-oauth2" },
        appState: { returnTo: window.location.pathname + window.location.search },
      });
    };
    return new QueryClient({
      queryCache: new QueryCache({ onError }),
      mutationCache: new MutationCache({ onError }),
      defaultOptions: { queries: { retry: retryTransient(1), refetchOnWindowFocus: false } },
    });
  });

  if (isLoading) {
    return <div className="grid min-h-full place-items-center text-muted">Loading…</div>;
  }
  if (!isAuthenticated) {
    return <SignIn error={error} />;
  }
  return (
    <ApiProvider>
      <QueryClientProvider client={queryClient}>
        <ToastProvider>
          <RouterProvider router={router} />
        </ToastProvider>
      </QueryClientProvider>
    </ApiProvider>
  );
}

async function start() {
  const root = createRoot(document.getElementById("root") as HTMLElement);
  // Public page: no config fetch, no sign-in.
  if (window.location.pathname.replace(/\/$/, "") === "/privacy") {
    root.render(
      <StrictMode>
        <Privacy />
      </StrictMode>,
    );
    return;
  }
  // Public share link: anyone with it can watch, no sign-in.
  const shared = window.location.pathname.match(/^\/s\/([A-Za-z0-9_-]{22})\/?$/);
  if (shared?.[1]) {
    root.render(
      <StrictMode>
        <SharedClip token={shared[1]} />
      </StrictMode>,
    );
    return;
  }
  try {
    const config = await loadConfig();
    root.render(
      <StrictMode>
        <Auth0Provider
          domain={config.auth0Domain}
          clientId={config.auth0ClientId}
          authorizationParams={{
            redirect_uri: window.location.origin,
            audience: config.auth0Audience,
            scope: "openid profile email offline_access",
          }}
          // Refresh tokens (rotating) kept in localStorage so reloads don't bounce through
          // Auth0: silent auth via iframe needs third-party cookies, which browsers block
          // for *.auth0.com without a custom domain.
          useRefreshTokens
          cacheLocation="localstorage"
          onRedirectCallback={(appState) => {
            router.history.replace(appState?.returnTo ?? "/");
          }}
        >
          <App />
        </Auth0Provider>
      </StrictMode>,
    );
  } catch (e) {
    root.render(
      <div className="grid min-h-full place-items-center px-6 text-center text-danger">
        clipos couldn't start: {e instanceof Error ? e.message : String(e)}
      </div>,
    );
  }
}

start();
