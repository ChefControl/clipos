import { useEffect } from "react";
import { Kip } from "../kip/Kip";
import { useTitle } from "../lib/useTitle";
import { Backdrop } from "../ui/Backdrop";
import { COPYRIGHT_ID, Credit } from "../ui/Credit";
import { PublicHeader } from "../ui/Wordmark";

/** Public privacy policy (linked from the Google consent screen). Rendered without
 *  config or sign-in, see main.tsx. */
export function Privacy() {
  useTitle("Privacy");
  // The page renders after load, so the browser's own jump to a #section can miss it.
  useEffect(() => {
    const id = window.location.hash.slice(1);
    if (id) document.getElementById(id)?.scrollIntoView();
  }, []);
  return (
    <div className="flex min-h-full flex-col">
      <Backdrop />
      <PublicHeader />
      <main className="glass squircle mx-4 mb-8 rounded-[34px] px-6 pt-8 pb-10 sm:mx-auto sm:mt-6 sm:w-full sm:max-w-[720px] sm:px-[52px] sm:pt-11 sm:pb-12">
        <div className="flex items-end gap-4">
          <div className="flex flex-col gap-1.5">
            <h1 className="text-[40px] leading-[0.95] font-extrabold tracking-tight sm:text-[52px]">
              Privacy
            </h1>
            <p className="font-mono text-xs text-muted">Last updated 1 October 2026</p>
          </div>
          <Kip className="-mb-2 ml-auto hidden h-20 w-20 sm:block" />
        </div>

        <div className="mt-6 space-y-6 text-[17px] leading-relaxed text-soft [&_h2]:text-xl [&_h2]:font-bold [&_h2]:text-text [&_ul]:list-disc [&_ul]:space-y-1 [&_ul]:pl-5">
          <p>
            clipos is a private, invite-only site where a group of friends share game clips. It is
            run by friends, not a company. There are no ads and nothing is sold or shared for
            marketing.
          </p>

          <section className="space-y-2">
            <h2>What we keep</h2>
            <ul>
              <li>
                From your Google sign-in: your name, email address and profile picture. We never see
                your Google password.
              </li>
              <li>The clips you upload and what you add to them: titles, tags, reactions.</li>
              <li>Server logs (requests, errors), kept for 30 days to keep the site running.</li>
            </ul>
          </section>

          <section className="space-y-2">
            <h2>Who can see it</h2>
            <ul>
              <li>Your profile and clips are visible to the other invited members.</li>
              <li>
                If you turn on sharing for a clip, anyone with its link can watch that clip until
                you turn sharing off.
              </li>
            </ul>
          </section>

          <section className="space-y-2">
            <h2>Where it lives</h2>
            <ul>
              <li>Clips and the database: Microsoft Azure, Israel Central region.</li>
              <li>Sign-in: Auth0 (Okta), EU region, using Google as the identity provider.</li>
            </ul>
          </section>

          <section className="space-y-2">
            <h2>Deleting things</h2>
            <p>
              Deleted clips can be restored for 7 days, then they are removed for good. To have your
              account and everything you uploaded removed, ask the admin who invited you.
            </p>
          </section>

          <section id={COPYRIGHT_ID} className="scroll-mt-6 space-y-2">
            <h2>Copyright and trademarks</h2>
            <p>
              clipos is a private fan project for a group of friends. It isn't affiliated with or
              endorsed by Valve Corporation. Counter-Strike 2, CS2 and the weapon and killfeed icons
              are trademarks and artwork of Valve Corporation, all rights reserved. Clips belong to
              the players who recorded them.
            </p>
          </section>
        </div>
      </main>
      <Credit className="pb-5" />
    </div>
  );
}
