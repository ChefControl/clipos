import { useEffect, useState } from "react";
import { Kip } from "../kip/Kip";
import { formatDuration } from "../lib/format";
import { useTitle } from "../lib/useTitle";
import { Avatar } from "../ui/Avatar";
import { Backdrop } from "../ui/Backdrop";
import { Credit } from "../ui/Credit";
import { PublicHeader } from "../ui/Wordmark";

interface PublicClip {
  title: string;
  uploader: string;
  map?: string | null;
  durationMs?: number | null;
  width?: number | null;
  height?: number | null;
  videoUrl: string;
  posterUrl: string;
}

type State =
  | { kind: "loading" }
  | { kind: "ready"; clip: PublicClip }
  | { kind: "gone" }
  | { kind: "error" };

/** Public player for a share link (canvas 1.4): no sign-in, no config fetch. */
export function SharedClip({ token }: { token: string }) {
  const [state, setState] = useState<State>({ kind: "loading" });

  useEffect(() => {
    fetch(`/s/${token}/clip.json`)
      .then(async (res) => {
        if (res.status === 404) return setState({ kind: "gone" });
        if (!res.ok) return setState({ kind: "error" });
        setState({ kind: "ready", clip: (await res.json()) as PublicClip });
      })
      .catch(() => setState({ kind: "error" }));
  }, [token]);

  useTitle(state.kind === "ready" ? state.clip.title : null);

  return (
    <div className="flex min-h-full flex-col">
      <Backdrop image={state.kind === "ready" ? state.clip.posterUrl : null} />
      <PublicHeader>
        <span className="frost ml-1 hidden rounded-full px-3 py-1 text-[13px] text-soft sm:inline">
          Shared with you
        </span>
        <a
          href="/"
          className="frost ml-auto flex h-10 items-center rounded-full px-4 text-sm font-semibold text-soft hover:text-text"
        >
          Members sign in
        </a>
      </PublicHeader>

      <main className="mx-auto w-full max-w-[1120px] flex-1 px-4 pt-3 pb-8 sm:pt-5">
        {state.kind === "loading" && (
          <div className="squircle aspect-video animate-pulse rounded-[26px] bg-surface" />
        )}
        {(state.kind === "gone" || state.kind === "error") && (
          <div className="flex flex-col items-center gap-4 py-16 text-center">
            <Kip pose="knocked-out" className="h-36 w-36" />
            <h1 className="text-4xl font-extrabold tracking-tight">
              {state.kind === "gone" ? "This link has expired." : "Couldn't load this clip."}
            </h1>
            <p className="text-lg text-soft">
              {state.kind === "gone"
                ? "The clip was unshared or deleted."
                : "Try again in a moment."}
            </p>
          </div>
        )}
        {state.kind === "ready" && (
          <div className="flex flex-col gap-5">
            <div className="squircle overflow-hidden rounded-[26px] bg-black shadow-[0_30px_70px_-30px_rgba(0,0,0,0.95),0_0_0_1px_rgba(255,255,255,0.06)]">
              {/* biome-ignore lint/a11y/useMediaCaption: game clips have no captions. */}
              <video
                src={state.clip.videoUrl}
                poster={state.clip.posterUrl}
                controls
                playsInline
                preload="metadata"
                className="aspect-video w-full"
              />
            </div>
            <div className="flex flex-wrap items-center gap-x-4 gap-y-4">
              <Avatar name={state.clip.uploader} size={44} />
              <div className="flex min-w-0 flex-1 flex-col gap-1">
                <h1
                  dir="auto"
                  className="text-[28px] leading-none font-extrabold tracking-tight [overflow-wrap:anywhere] sm:text-[34px]"
                >
                  {state.clip.title}
                </h1>
                <p className="text-[15px] text-soft">
                  Shared by <bdi>{state.clip.uploader}</bdi>
                  {state.clip.map && ` · ${state.clip.map}`}
                  {state.clip.durationMs != null && ` · ${formatDuration(state.clip.durationMs)}`}
                </p>
              </div>
              <p className="glass squircle flex w-full items-center gap-3 rounded-[20px] py-2.5 pr-5 pl-2.5 text-sm leading-snug text-soft sm:w-auto sm:max-w-[360px]">
                <Kip className="h-10 w-10 shrink-0" />
                {state.clip.uploader}'s squad keeps their clips on clipos and watches them together
                on show nights.
              </p>
            </div>
          </div>
        )}
      </main>

      <div className="flex flex-col gap-1 pb-5">
        <p className="text-center text-[13px] text-muted">
          clipos is an invite-only clip archive.{" "}
          <a href="/privacy" className="text-soft hover:text-text">
            Privacy
          </a>
        </p>
        <Credit />
      </div>
    </div>
  );
}
