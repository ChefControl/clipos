import { useAuth0 } from "@auth0/auth0-react";
import type { ApiError } from "../api/errors";
import { Kip } from "../kip/Kip";
import { Backdrop } from "../ui/Backdrop";
import { Button } from "../ui/Button";
import { Credit } from "../ui/Credit";
import { PublicHeader } from "../ui/Wordmark";

/** Signed in with Google, but the API refused the account. */
export function AccessDenied({ error }: { error: ApiError }) {
  return <NotOnTheList disabled={error.code === "account_disabled"} />;
}

// Not on the list (canvas 1.2): Kip the bouncer with the guest list. Also shown when the
// Auth0 Action turns the account away before it reaches the API (then without the email).
export function NotOnTheList({ disabled = false }: { disabled?: boolean }) {
  const { logout, user } = useAuth0();

  return (
    <div className="flex min-h-full flex-col">
      <Backdrop />
      <PublicHeader />
      <main className="mx-auto grid w-full max-w-5xl flex-1 items-center gap-6 px-4 py-8 md:grid-cols-[minmax(0,420px)_minmax(0,1fr)] md:gap-16">
        <Kip
          pose="bouncer"
          label="Kip checking the guest list"
          className="mx-auto h-48 w-48 drop-shadow-[0_16px_20px_rgba(0,0,0,0.5)] md:h-auto md:w-full"
        />
        <div className="flex flex-col gap-5">
          <span className="font-mono text-xs text-accent">
            {disabled ? "Account disabled" : "Invite only"}
          </span>
          <h1 className="text-5xl leading-[0.92] font-extrabold tracking-tight text-balance md:text-[64px]">
            {disabled ? "Account disabled." : "Not on the list (yet)."}
          </h1>
          <p className="max-w-[460px] text-lg leading-relaxed text-soft">
            {disabled ? (
              "An admin has disabled this account. Ask them if you think that's a mistake."
            ) : (
              <>
                clipos is invite-only. Ask a friend who runs it to invite{" "}
                <span className="font-semibold [overflow-wrap:anywhere] text-text">
                  {user?.email ?? "your Google account"}
                </span>
                , then come back and sign in again.
              </>
            )}
          </p>
          <Button
            size="lg"
            className="w-full sm:w-auto sm:self-start"
            onClick={() => logout({ logoutParams: { returnTo: window.location.origin } })}
          >
            Sign in with a different account
          </Button>
        </div>
      </main>
      <Credit className="pb-5" />
    </div>
  );
}
