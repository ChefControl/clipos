import { useClipAnalysis } from "../clips/killfeed/analysis";
import { Kip } from "../kip/Kip";
import { formatDuration } from "../lib/format";
import { Avatar } from "../ui/Avatar";
import type { LineupEntry, Show } from "./hooks";

// Up next, between clips (canvas 2.4), over the player for everyone: the next clip (its
// teaser if it's saved for the show), the count to its start, who's ready, and the clip
// that just played with the reactions it got. Held: the count stops until the host
// starts it.
export function Between({
  show,
  next,
  index,
  count,
  startsInMs,
  ready,
  watching,
  previous,
}: {
  show: Show;
  next: LineupEntry;
  /** 1-based, of `count`. */
  index: number;
  count: number;
  /** Until it starts; null while held. */
  startsInMs: number | null;
  ready: number;
  watching: number;
  previous: LineupEntry | null;
}) {
  const seconds = startsInMs == null ? null : Math.max(1, Math.ceil(startsInMs / 1000));
  return (
    <div className="absolute inset-0 overflow-hidden bg-black" data-testid="between">
      {next.clip.posterUrl && (
        <img
          src={next.clip.posterUrl}
          alt=""
          aria-hidden="true"
          className="absolute -inset-10 h-[calc(100%+80px)] w-[calc(100%+80px)] object-cover blur-[40px] brightness-[.35] saturate-[1.3]"
        />
      )}
      <div className="absolute inset-0 grid grid-cols-[minmax(0,1fr)_auto] items-center gap-10 px-[5.5%] pb-20">
        <div className="flex min-w-0 max-w-[520px] flex-col gap-4">
          <span className="font-mono text-[13px] text-accent">
            Next up · clip {index} of {count}
          </span>
          <span
            className="squircle block aspect-video rounded-[20px] bg-surface bg-cover bg-center shadow-[0_20px_40px_-20px_rgba(0,0,0,0.9)]"
            style={{
              backgroundImage: next.clip.posterUrl ? `url(${next.clip.posterUrl})` : undefined,
            }}
          />
          <span className="flex min-w-0 items-center gap-3">
            <Avatar
              name={next.clip.uploader.displayName}
              url={next.clip.uploader.avatarUrl}
              size={40}
            />
            <span className="flex min-w-0 flex-col">
              <span className="truncate text-[38px] leading-tight font-extrabold tracking-tight">
                {next.clip.isMine ? "Your clip" : `${next.clip.uploader.displayName}'s clip`}
              </span>
              <span dir="auto" className="truncate text-lg text-[#d6d3cb]">
                {next.clip.title}
                {next.clip.durationMs != null && ` · ${formatDuration(next.clip.durationMs)}`}
              </span>
            </span>
          </span>
        </div>
        <div className="flex w-[280px] flex-col items-center gap-3.5 text-center">
          <span className="relative grid h-[150px] w-[150px] place-items-center" role="timer">
            <svg viewBox="0 0 100 100" className="absolute inset-0 -rotate-90" aria-hidden="true">
              <circle
                cx="50"
                cy="50"
                r="46"
                fill="none"
                stroke="rgb(255 255 255 / .12)"
                strokeWidth="5"
              />
              {seconds != null && (
                <circle
                  // A new element per second restarts the sweep.
                  key={seconds}
                  cx="50"
                  cy="50"
                  r="46"
                  fill="none"
                  stroke="var(--color-accent)"
                  strokeWidth="5"
                  strokeLinecap="round"
                  pathLength={1}
                  strokeDasharray="1"
                  className="countdown-sweep"
                />
              )}
            </svg>
            <span
              className="relative text-[76px] leading-none font-extrabold"
              data-testid="countdown"
            >
              {seconds ?? "II"}
            </span>
          </span>
          <span className="text-xl font-bold">
            {seconds == null ? "Held" : "Starting for everyone"}
          </span>
          <span className="text-sm text-soft">
            {ready} of {watching} watching {ready === 1 ? "is" : "are"} ready
          </span>
        </div>
      </div>
      <Kip
        pose="cheer"
        className="absolute right-6 bottom-20 hidden h-[150px] w-[150px] xl:block"
      />
      {previous && <JustPlayed show={show} entry={previous} />}
    </div>
  );
}

/** The bar along the bottom: the clip that just played and how the room took it. */
function JustPlayed({ show, entry }: { show: Show; entry: LineupEntry }) {
  const analysis = useClipAnalysis(entry.clip.id, true);
  const kills = analysis.data?.stats?.myKills;
  const counts = new Map<string, number>();
  for (const r of show.reactions) {
    if (r.clipId === entry.clip.id) counts.set(r.emoji, (counts.get(r.emoji) ?? 0) + 1);
  }
  const top = [...counts].sort((a, b) => b[1] - a[1]).slice(0, 4);
  return (
    <div className="absolute inset-x-0 bottom-0 flex items-center gap-3 bg-gradient-to-b from-transparent to-black/70 px-[5.5%] pt-4 pb-[18px]">
      <span
        className="squircle aspect-video w-[72px] shrink-0 rounded-lg bg-surface bg-cover bg-center"
        style={{
          backgroundImage: entry.clip.posterUrl ? `url(${entry.clip.posterUrl})` : undefined,
        }}
      />
      <span className="flex min-w-0 flex-col">
        <span className="font-mono text-[11px] text-muted">Just played</span>
        <span className="truncate font-bold">
          <span dir="auto">{entry.clip.title}</span>{" "}
          <span className="font-medium text-muted">
            · <bdi>{entry.clip.uploader.displayName}</bdi>
            {kills != null && ` · ${kills} ${kills === 1 ? "kill" : "kills"}`}
          </span>
        </span>
      </span>
      <span className="ml-1.5 flex shrink-0 gap-1.5" data-testid="just-played-reactions">
        {top.map(([emoji, n]) => (
          <span
            key={emoji}
            className="frost flex h-7 items-center gap-1 rounded-full pr-2.5 pl-[7px] text-[13px] font-bold"
          >
            <span className="text-[15px]">{emoji}</span>
            {n}
          </span>
        ))}
      </span>
    </div>
  );
}
