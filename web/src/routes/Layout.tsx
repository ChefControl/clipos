import { useAuth0 } from "@auth0/auth0-react";
import { Link, Outlet, useMatchRoute } from "@tanstack/react-router";
import { isAccessDenied, needsSignIn } from "../api/errors";
import { Logo } from "../kip/Kip";
import { LivePill } from "../show/LivePill";
import { Avatar } from "../ui/Avatar";
import { Backdrop } from "../ui/Backdrop";
import { Button } from "../ui/Button";
import { Credit } from "../ui/Credit";
import { AccessDenied } from "./AccessDenied";
import { useMe } from "./useMe";

export function Layout() {
  const me = useMe();
  const match = useMatchRoute();
  // The clip page puts the player beside its killfeed panel, so it gets more width.
  const wide = match({ to: "/clips/$clipId" });
  const width = wide ? "max-w-[96rem]" : "max-w-[90rem]";

  if (isAccessDenied(me.error)) {
    return <AccessDenied error={me.error} />;
  }
  // While a show's on, a way in from every page; the lobby says it itself.
  const pill = !!me.data?.shows && !match({ to: "/tonight" });

  // The show takes the whole window, with its own top bar (canvas 1.3, 2.2, 2.3).
  if (match({ to: "/shows/$showId" })) {
    return (
      <main id="main" className="min-h-full">
        <Backdrop />
        <Outlet />
      </main>
    );
  }

  return (
    <div className="flex min-h-full flex-col">
      {/* First stop for the keyboard: straight past the top bar. */}
      {/* biome-ignore lint/a11y/useValidAnchor: a skip link is an in-page link; the click moves focus without putting #main in the router's URL. */}
      <a
        href="#main"
        onClick={(e) => {
          e.preventDefault();
          document.getElementById("main")?.focus();
        }}
        className="sr-only z-50 rounded-full bg-text font-semibold text-bg focus:not-sr-only focus:fixed focus:top-3 focus:left-3 focus:px-4 focus:py-2"
      >
        Skip to content
      </a>
      <Backdrop />
      <header className="sticky top-0 z-20 bg-gradient-to-b from-bg/80 to-bg/0 backdrop-blur-[2px]">
        <div
          className={`mx-auto flex h-16 ${width} items-center gap-3 px-4 sm:h-[76px] sm:px-10 sm:gap-6`}
        >
          <Link
            to="/"
            className="flex shrink-0 items-center gap-2 text-text hover:text-white"
            aria-label="clipos home"
          >
            <Logo className="h-9 w-9 sm:h-10 sm:w-10" />
            <span className="text-shadow hidden text-2xl leading-none font-bold tracking-tight sm:inline">
              clipos
            </span>
          </Link>
          <nav className="glass flex min-w-0 gap-1 rounded-full p-1">
            {/* Tonight, the show's lobby, once the show is open to you (admins first,
                decision 40). Admin is under your profile's Account panel. */}
            {me.data?.shows && <NavLink to="/tonight">Tonight</NavLink>}
            <NavLink to="/">Archive</NavLink>
            <NavLink to="/upload">Upload</NavLink>
          </nav>
          <div className="ml-auto flex min-w-0 items-center gap-3">
            <LivePill enabled={pill} className="hidden sm:flex" />
            {me.data && (
              <Link
                to="/u/$handle"
                params={{ handle: me.data.handle }}
                aria-label="Your profile"
                className="flex shrink-0 items-center rounded-full p-0.5 hover:ring-2 hover:ring-white/20"
              >
                <Avatar name={me.data.displayName} url={me.data.avatarUrl} size={34} />
              </Link>
            )}
          </div>
        </div>
        <LivePill enabled={pill} strip className="mx-4 -mt-1 mb-2 sm:hidden" />
      </header>

      <main
        id="main"
        tabIndex={-1}
        className={`mx-auto w-full flex-1 ${width} px-4 py-6 outline-none sm:px-10 ${wide ? "sm:py-5" : "sm:py-8"}`}
      >
        {/* Without your account the pages still work; a 401 is on its way to sign-in. */}
        {me.error && !needsSignIn(me.error) && <MeError error={me.error} retry={me.refetch} />}
        <Outlet />
      </main>
      <Credit className="pt-6 pb-8" />
    </div>
  );
}

/** /api/me failed even after a few tries: say so above the page, with a way to try again
 *  and Sign out (normally on your profile, which needs your account to link to). */
function MeError({ error, retry }: { error: Error; retry: () => void }) {
  const { logout } = useAuth0();
  return (
    <div
      role="alert"
      className="squircle mb-5 flex flex-wrap items-center gap-3 rounded-2xl bg-danger/10 px-4 py-3 text-sm ring-1 ring-danger/40 ring-inset"
    >
      <span className="min-w-0 flex-1">
        Couldn't load your account: {error.message}. Some things won't work until it's back.
      </span>
      <Button size="sm" onClick={() => retry()}>
        Try again
      </Button>
      <Button
        size="sm"
        variant="ghost"
        onClick={() => logout({ logoutParams: { returnTo: window.location.origin } })}
      >
        Sign out
      </Button>
    </div>
  );
}

function NavLink({ to, children }: { to: "/" | "/tonight" | "/upload"; children: string }) {
  return (
    <Link
      to={to}
      className="rounded-full px-3.5 py-1.5 text-[15px] font-semibold text-soft hover:text-text sm:px-[18px] sm:py-2"
      activeProps={{ className: "bg-white/12 text-text", "aria-current": "page" }}
      activeOptions={{ exact: true }}
    >
      {children}
    </Link>
  );
}
