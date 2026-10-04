import { getRouteApi, Link } from "@tanstack/react-router";
import { useEffect, useRef, useState } from "react";
import { isNotFound } from "../api/errors";
import { TrophyIcon } from "../clips/ClipCard";
import { useClip } from "../clips/hooks";
import { Kip } from "../kip/Kip";
import { formatDuration, shortDate, showName } from "../lib/format";
import { useTitle } from "../lib/useTitle";
import { useShow } from "../show/hooks";
import { FloatingReactions, useFloats } from "../show/Reactions";
import { Avatar } from "../ui/Avatar";
import { Backdrop } from "../ui/Backdrop";
import { Button, buttonClass } from "../ui/Button";
import { LoadError } from "../ui/LoadError";
import { Panel } from "../ui/Panel";
import { NotFound } from "./NotFound";

const route = getRouteApi("/shows/$showId/replay");

// A show's replay (docs/PLAN.md, the show): for someone who missed it, alone, from the
// archive. The clips in the order they played, each tap of the dock floating up at the
// moment it happened, one clip after another. No voting: the winners are already in.
export function ShowReplay() {
  const { showId } = route.useParams();
  const show = useShow(showId);
  useTitle(show.data && `${showName(show.data.startedAt)}, replay`);
  const [index, setIndex] = useState(0);
  const [done, setDone] = useState(false);
  const { floats, add } = useFloats();
  const video = useRef<HTMLVideoElement>(null);
  // Where the last time update was, so each reaction floats once as the clip passes it.
  const last = useRef(0);

  const played = show.data?.lineup.filter((l) => l.playedAt && !l.dropped) ?? [];
  const entry = played[index];
  const clip = useClip(entry?.clip.id ?? "", { enabled: !!entry });
  const taps = show.data?.reactions.filter((r) => r.clipId === entry?.clip.id) ?? [];

  // biome-ignore lint/correctness/useExhaustiveDependencies: start each clip from the top.
  useEffect(() => {
    last.current = 0;
  }, [index]);

  if (isNotFound(show.error)) return <NotFound />;
  if (show.error) return <LoadError error={show.error} onRetry={show.refetch} />;
  if (!show.data) return null;
  const s = show.data;

  if (s.status !== "ended") {
    return (
      <Panel className="mx-auto mt-10 flex max-w-lg flex-col items-center gap-4 p-10 text-center">
        <Kip pose="asleep" className="h-28 w-28" />
        <h1 className="text-3xl font-extrabold">No replay for this show</h1>
        <p className="text-soft">
          {s.status === "abandoned"
            ? "Everyone left before the finale, so its clips went back for the next show."
            : "It's still on. Join it instead."}
        </p>
        {s.status !== "abandoned" && (
          <Link to="/shows/$showId" params={{ showId }} className={buttonClass("primary")}>
            Join the show
          </Link>
        )}
      </Panel>
    );
  }

  const onTime = () => {
    const v = video.current;
    if (!v) return;
    const now = v.currentTime * 1000;
    // Jumped back: the taps since then float again as it plays through them.
    if (now < last.current) last.current = now;
    for (const t of taps) if (t.atMs > last.current && t.atMs <= now) add(t.emoji);
    last.current = now;
  };
  const go = (i: number) => {
    setDone(false);
    setIndex(i);
  };
  const counts = new Map<string, number>();
  for (const t of taps) counts.set(t.emoji, (counts.get(t.emoji) ?? 0) + 1);

  return (
    <div className="flex flex-col gap-5">
      <Backdrop image={entry?.clip.posterUrl} />
      <div className="flex flex-wrap items-end gap-3">
        <div className="flex flex-col gap-1">
          <span className="font-mono text-xs text-accent">
            Replay · {s.startedAt && shortDate(s.startedAt)} · hosted by{" "}
            <bdi>{s.host.displayName}</bdi>
          </span>
          <h1 className="text-[40px] leading-[0.95] font-extrabold tracking-tight">
            {showName(s.startedAt)}
          </h1>
        </div>
        <Link
          to="/shows/$showId"
          params={{ showId }}
          className={`${buttonClass("secondary", "sm")} ml-auto`}
        >
          The winners
        </Link>
      </div>
      <div className="grid grid-cols-1 items-start gap-6 lg:grid-cols-[minmax(0,1fr)_340px]">
        <div className="flex min-w-0 flex-col gap-4">
          <div className="squircle relative aspect-video overflow-hidden rounded-[26px] bg-black">
            {played.length === 0 ? (
              <p className="grid h-full place-items-center text-soft">
                This show didn't play any clips.
              </p>
            ) : (
              // biome-ignore lint/a11y/useMediaCaption: game clips have no captions.
              <video
                ref={video}
                key={entry?.clip.id}
                src={clip.data?.playbackUrl ?? undefined}
                poster={entry?.clip.posterUrl ?? undefined}
                controls
                autoPlay
                playsInline
                className="h-full w-full object-contain"
                onTimeUpdate={onTime}
                onEnded={() => (index + 1 < played.length ? go(index + 1) : setDone(true))}
                data-testid="replay-video"
              />
            )}
            <FloatingReactions floats={floats} />
            {done && (
              <div className="absolute inset-0 grid place-items-center bg-black/70 p-6 text-center">
                <div className="flex flex-col items-center gap-3">
                  <Kip pose="king" className="h-24 w-24" />
                  <p className="text-[28px] font-extrabold">That was the show.</p>
                  <div className="flex gap-3">
                    <Button onClick={() => go(0)}>From the start</Button>
                    <Link
                      to="/shows/$showId"
                      params={{ showId }}
                      className={buttonClass("primary")}
                    >
                      See the winners
                    </Link>
                  </div>
                </div>
              </div>
            )}
          </div>
          {entry && (
            <div className="flex flex-wrap items-center gap-3">
              <Avatar
                name={entry.clip.uploader.displayName}
                url={entry.clip.uploader.avatarUrl}
                size={40}
              />
              <span className="flex min-w-0 flex-col">
                <span className="text-2xl leading-tight font-bold">
                  {entry.clip.isMine ? "Your clip" : `${entry.clip.uploader.displayName}'s clip`}
                </span>
                <Link
                  to="/clips/$clipId"
                  params={{ clipId: entry.clip.id }}
                  dir="auto"
                  className="truncate text-lg text-soft hover:text-text"
                >
                  {entry.clip.title}
                </Link>
              </span>
              <span className="ml-auto flex flex-wrap gap-1.5" data-testid="replay-reactions">
                {[...counts].map(([emoji, n]) => (
                  <span
                    key={emoji}
                    className="frost flex h-8 items-center gap-1 rounded-full pr-3 pl-2 text-sm font-bold"
                  >
                    <span className="text-base">{emoji}</span>
                    {n}
                  </span>
                ))}
              </span>
            </div>
          )}
        </div>
        <Panel as="aside" padding="p-4" className="flex flex-col" aria-label="The show's clips">
          <span className="px-1 pb-2 font-mono text-[11px] text-muted">
            {played.length} {played.length === 1 ? "clip" : "clips"} · {s.participants.length}{" "}
            watched
          </span>
          <ol className="flex flex-col">
            {played.map((l, i) => (
              <li key={l.clip.id}>
                <button
                  type="button"
                  aria-current={i === index ? "true" : undefined}
                  onClick={() => go(i)}
                  className={`flex w-full items-center gap-3 rounded-xl p-1.5 text-left ${
                    i === index ? "bg-white/10" : "hover:bg-white/5"
                  }`}
                >
                  <span
                    className="squircle aspect-video w-24 shrink-0 rounded-[11px] bg-surface bg-cover bg-center"
                    style={{
                      backgroundImage: l.clip.posterUrl ? `url(${l.clip.posterUrl})` : undefined,
                    }}
                  />
                  <span className="flex min-w-0 flex-col">
                    <span dir="auto" className="truncate font-bold">
                      {l.clip.title}
                    </span>
                    <span className="flex items-center gap-1.5 truncate text-[13px] text-muted">
                      {l.clip.id === s.clipWinnerId && (
                        <TrophyIcon className="h-3.5 w-3.5 shrink-0 text-accent" />
                      )}
                      {l.clip.id === s.failWinnerId && <span aria-hidden="true">🍌</span>}
                      <bdi>{l.clip.uploader.displayName}</bdi>
                      {l.clip.durationMs != null && ` · ${formatDuration(l.clip.durationMs)}`}
                    </span>
                  </span>
                </button>
              </li>
            ))}
          </ol>
        </Panel>
      </div>
    </div>
  );
}
