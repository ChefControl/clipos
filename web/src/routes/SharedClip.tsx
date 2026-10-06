import { useEffect, useState } from "react";
import { Kip } from "../kip/Kip";
import { formatDuration } from "../lib/format";
import { usePhone } from "../lib/usePhone";
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
  const phone = usePhone();

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

      {/* On phones the clip runs almost edge to edge, centred in the top three fifths of the
          screen, with a small title under it. */}
      <main className="mx-auto w-full max-w-[1120px] flex-1 px-4 pt-2 pb-8 sm:pt-5">
        {state.kind === "loading" && (
          <Stage>
            <div className="squircle aspect-video animate-pulse rounded-xl bg-surface sm:rounded-[26px]" />
          </Stage>
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
          <div className="flex flex-col gap-3 sm:gap-5">
            <Stage>
              <div className="squircle overflow-hidden rounded-xl bg-black shadow-[0_30px_70px_-30px_rgba(0,0,0,0.95),0_0_0_1px_rgba(255,255,255,0.06)] sm:rounded-[26px]">
                {/* biome-ignore lint/a11y/useMediaCaption: game clips have no captions. */}
                <video
                  src={state.clip.videoUrl}
                  poster={state.clip.posterUrl}
                  controls
                  playsInline
                  preload="metadata"
                  // The clip's own shape, so a tall one fills the phone rather than a strip,
                  // short of the whole screen so the title still peeks in under it.
                  style={{ aspectRatio: aspect(state.clip) }}
                  className="block max-h-[75svh] w-full sm:max-h-none"
                />
              </div>
            </Stage>
            <div className="flex min-w-0 items-start gap-3 sm:items-center sm:gap-4">
              <Avatar name={state.clip.uploader} size={phone ? 36 : 44} />
              <div className="flex min-w-0 flex-1 flex-col gap-1">
                <h1
                  dir="auto"
                  className="text-xl leading-tight font-extrabold tracking-tight [overflow-wrap:anywhere] sm:text-[34px] sm:leading-none"
                >
                  {state.clip.title}
                </h1>
                <p className="text-[13px] text-soft sm:text-[15px]">
                  Shared by <bdi>{state.clip.uploader}</bdi>
                  {state.clip.map && ` · ${state.clip.map}`}
                  {state.clip.durationMs != null && ` · ${formatDuration(state.clip.durationMs)}`}
                </p>
              </div>
            </div>
          </div>
        )}
      </main>

      <div className="flex flex-col gap-1 pb-[max(1.25rem,env(safe-area-inset-bottom))]">
        <p className="text-center text-xs text-muted sm:text-[13px]">
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

/** Where the clip sits: on phones, 8 px from the sides and centred in the top three fifths
 *  of the screen (under the 4.5rem of header and padding); on wider screens, the column. */
function Stage({ children }: { children: React.ReactNode }) {
  return (
    <div className="-mx-2 flex min-h-[calc(60svh-4.5rem)] flex-col justify-center sm:mx-0 sm:block sm:min-h-0">
      {children}
    </div>
  );
}

/** The clip's width over its height, 16:9 when unknown. */
function aspect({ width, height }: PublicClip): string {
  return width && height ? `${width} / ${height}` : "16 / 9";
}
