import { useAuth0 } from "@auth0/auth0-react";
import { CardFan } from "../kip/CardFan";
import { Kip } from "../kip/Kip";
import { useTitle } from "../lib/useTitle";
import { Backdrop } from "../ui/Backdrop";
import { Credit } from "../ui/Credit";
import { Wordmark } from "../ui/Wordmark";
import { NotOnTheList } from "./AccessDenied";

// Sign in (canvas 1.1): a fan of face-down clip cards with Kip on the middle one, and the sign-in
// panel. Google is the only way in.
export function SignIn({ error }: { error?: Error }) {
  const { loginWithRedirect } = useAuth0();
  const invited = new URLSearchParams(window.location.search).has("invite");
  useTitle("Sign in");

  // The Auth0 Action turned the account away (infra/auth0/actions/post-login.js).
  if (error?.message.includes("isn't invited")) {
    return <NotOnTheList />;
  }

  return (
    <div className="flex min-h-full flex-col">
      <Backdrop />
      <main className="mx-auto grid w-full max-w-6xl flex-1 items-center gap-10 px-4 py-10 lg:grid-cols-[1fr_440px] lg:gap-16">
        <div className="relative hidden lg:block">
          <CardFan
            width={220}
            spread={140}
            center={
              <Kip
                pose="idle"
                className="h-[86%] w-[86%] drop-shadow-[0_12px_16px_rgba(0,0,0,0.55)]"
              />
            }
          />
        </div>

        <section className="glass squircle mx-auto flex w-full max-w-[440px] flex-col gap-6 rounded-[34px] p-7 sm:p-10">
          <Wordmark size="lg" />
          <div className="flex flex-col gap-2">
            <h1 className="text-[40px] leading-[0.95] font-extrabold tracking-tight text-balance sm:text-[44px]">
              Clips from the squad.
            </h1>
            <p className="text-lg text-soft">Invite only. Watch together on show nights.</p>
          </div>

          {invited && !error && (
            <p className="squircle flex items-center gap-2.5 rounded-2xl bg-accent/10 px-4 py-3 text-left text-[15px] leading-snug ring-1 ring-accent/40 ring-inset">
              <svg
                viewBox="0 0 24 24"
                className="h-5 w-5 shrink-0 text-accent"
                fill="none"
                stroke="currentColor"
                strokeWidth="2"
                aria-hidden="true"
              >
                <path d="M4 6h16v12H4z" />
                <path d="M4 7l8 6 8-6" />
              </svg>
              You've been invited. Sign in with the Google account your invite was sent to.
            </p>
          )}

          {error && (
            <p
              role="alert"
              className="squircle rounded-2xl bg-danger/10 px-4 py-3 text-[15px] text-danger ring-1 ring-danger/40 ring-inset"
            >
              {error.message}
            </p>
          )}

          <button
            type="button"
            onClick={() =>
              loginWithRedirect({
                authorizationParams: { connection: "google-oauth2" },
                appState: { returnTo: window.location.pathname + window.location.search },
              })
            }
            className="inline-flex h-14 w-full items-center justify-center gap-3 rounded-full bg-text text-lg font-bold text-[#0e0f11] shadow-[0_12px_30px_-14px_rgba(236,234,228,0.5)] transition hover:bg-white focus-visible:outline-2 focus-visible:outline-offset-2 focus-visible:outline-accent"
          >
            <GoogleMark className="h-5 w-5" />
            Sign in with Google
          </button>

          <a href="/privacy" className="self-center px-3 py-1.5 text-sm text-muted hover:text-text">
            Privacy
          </a>
        </section>
      </main>
      <Credit className="pb-5" />
    </div>
  );
}

function GoogleMark({ className }: { className?: string }) {
  return (
    <svg viewBox="0 0 24 24" className={className} aria-hidden="true">
      <path
        fill="#4285F4"
        d="M23.5 12.3c0-.8-.1-1.6-.2-2.3H12v4.4h6.5a5.6 5.6 0 0 1-2.4 3.6v3h3.9c2.2-2.1 3.5-5.1 3.5-8.7z"
      />
      <path
        fill="#34A853"
        d="M12 24c3.2 0 6-1.1 8-2.9l-3.9-3c-1.1.7-2.5 1.2-4.1 1.2-3.1 0-5.8-2.1-6.7-5H1.3v3.1A12 12 0 0 0 12 24z"
      />
      <path fill="#FBBC05" d="M5.3 14.3a7.2 7.2 0 0 1 0-4.6V6.6H1.3a12 12 0 0 0 0 10.8z" />
      <path
        fill="#EA4335"
        d="M12 4.8c1.8 0 3.3.6 4.6 1.8l3.4-3.4A12 12 0 0 0 1.3 6.6l4 3.1c.9-2.8 3.6-4.9 6.7-4.9z"
      />
    </svg>
  );
}
