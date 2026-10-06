import { Link } from "@tanstack/react-router";
import { useState } from "react";
import { TrophyIcon } from "../clips/ClipCard";
import type { Clip } from "../clips/hooks";
import { Kip } from "../kip/Kip";
import { formatDuration, shortDate } from "../lib/format";
import { Avatar } from "../ui/Avatar";
import { Button, buttonClass } from "../ui/Button";
import { Panel } from "../ui/Panel";
import { Pill } from "../ui/Pill";
import { type PastShow, usePastShows } from "./hooks";

/** Shows on the archive before "Show older shows". */
const FIRST = 3;
/** "Also played" thumbnails on a show before "and 3 more clips". */
const ALSO_SHOWN = 5;

// The archive's top half (canvas 4.1): every show that ended, newest first, with its
// winners, who watched and what else played, and its replay. Beside the heading, the
// trophy shelf: whose clips won the most.
export function PastShows() {
  const past = usePastShows();
  const [all, setAll] = useState(false);
  const shows = past.data ?? [];
  if (!past.data) return null;
  if (shows.length === 0) {
    return (
      <Panel
        as="section"
        aria-label="Past shows"
        className="flex items-center gap-5 rounded-[30px]"
      >
        <Kip pose="asleep" className="h-24 w-24 shrink-0" />
        <div className="flex flex-col gap-1">
          <h2 className="text-2xl font-bold">No shows yet</h2>
          <p className="text-soft">
            After the first show, its winners and who watched land here, above every clip.
          </p>
        </div>
        <Link to="/tonight" className={`${buttonClass("secondary")} ml-auto`}>
          Tonight
        </Link>
      </Panel>
    );
  }
  const clips = shows.reduce((n, s) => n + s.clips.length, 0);
  const first = shows.at(-1)?.startedAt;
  return (
    <section aria-labelledby="past-shows" className="flex flex-col gap-5">
      <div className="flex flex-wrap items-end gap-x-6 gap-y-3">
        <div className="flex flex-col gap-1.5">
          <span className="font-mono text-xs text-muted">
            {shows.length} {shows.length === 1 ? "show" : "shows"} · {clips}{" "}
            {clips === 1 ? "clip" : "clips"}
            {first && ` · since ${shortDate(first)}`}
          </span>
          <h2 id="past-shows" className="text-[40px] leading-[0.95] font-extrabold tracking-tight">
            Past shows
          </h2>
        </div>
        <TrophyBoard shows={shows} />
      </div>
      <div className="grid grid-cols-1 gap-5 lg:grid-cols-3">
        {shows.slice(0, all ? undefined : FIRST).map((s, i) => (
          <ShowCard key={s.id} show={s} last={i === 0} />
        ))}
      </div>
      {!all && shows.length > FIRST && (
        <Button className="self-center" onClick={() => setAll(true)}>
          Show {shows.length - FIRST} older {shows.length - FIRST === 1 ? "show" : "shows"}
        </Button>
      )}
    </section>
  );
}

/** Whose clips won the most, clip and fail of the night together. */
function TrophyBoard({ shows }: { shows: PastShow[] }) {
  const tally = new Map<string, { who: Clip["uploader"]; n: number }>();
  for (const s of shows) {
    for (const id of [s.clipWinnerId, s.failWinnerId]) {
      const who = s.clips.find((c) => c.id === id)?.uploader;
      if (!who) continue;
      const t = tally.get(who.handle) ?? { who, n: 0 };
      t.n += 1;
      tally.set(who.handle, t);
    }
  }
  const board = [...tally.values()].sort((a, b) => b.n - a.n);
  if (board.length === 0) return null;
  return (
    <div
      className="glass flex items-center gap-3 rounded-full py-1.5 pr-2 pl-4 sm:ml-auto"
      data-testid="trophy-board"
    >
      <span className="flex items-center gap-1.5 text-sm font-semibold text-soft">
        <TrophyIcon className="h-4 w-4 text-accent" />
        Trophy shelf
      </span>
      <ul className="flex gap-1">
        {board.map(({ who, n }) => (
          <li key={who.handle}>
            <Link
              to="/u/$handle"
              params={{ handle: who.handle }}
              title={`${who.displayName}: ${n} ${n === 1 ? "trophy" : "trophies"}`}
              className="flex items-center gap-1 rounded-full py-0.5 pr-2.5 pl-0.5 text-sm font-bold text-text hover:bg-white/10"
            >
              <Avatar name={who.displayName} url={who.avatarUrl} size={26} />
              {n}
              <span className="sr-only">
                {who.displayName}: {n} {n === 1 ? "trophy" : "trophies"}
              </span>
            </Link>
          </li>
        ))}
      </ul>
    </div>
  );
}

function ShowCard({ show, last }: { show: PastShow; last: boolean }) {
  const clipOf = (id: string | null | undefined) => show.clips.find((c) => c.id === id) ?? null;
  const clip = clipOf(show.clipWinnerId);
  const fail = clipOf(show.failWinnerId);
  const rest = show.clips.filter((c) => c.id !== clip?.id && c.id !== fail?.id);
  const total = show.clips.reduce((ms, c) => ms + (c.durationMs ?? 0), 0);
  const day = show.startedAt
    ? new Date(show.startedAt).toLocaleDateString("en-US", { weekday: "short" })
    : "";
  const seen = show.participants.slice(0, 4);
  return (
    <Panel
      as="article"
      padding="p-5"
      className="flex min-w-0 flex-col gap-4 rounded-[30px]"
      aria-label={`${day} night show${show.startedAt ? `, ${shortDate(show.startedAt)}` : ""}`}
    >
      <div className="flex flex-col gap-1">
        <span className="flex items-center gap-2 font-mono text-xs text-accent">
          {day} night{show.startedAt && ` · ${shortDate(show.startedAt)}`}
          {last && <Pill tone="accent">Last show</Pill>}
        </span>
        <span className="text-[15px] text-soft">
          {show.clips.length} {show.clips.length === 1 ? "clip" : "clips"} · {formatDuration(total)}{" "}
          · hosted by <bdi>{show.host.displayName}</bdi>
        </span>
        <span className="mt-1 flex items-center gap-2">
          <span className="flex -space-x-1.5">
            {seen.map((m) => (
              <Avatar
                key={m.id}
                name={m.displayName}
                url={m.avatarUrl}
                size={28}
                host={m.id === show.host.id}
                className="ring-2 ring-bg"
              />
            ))}
          </span>
          <span className="text-sm text-muted">
            {show.participants.length > seen.length &&
              `+${show.participants.length - seen.length} · `}
            {show.participants.length} watched
          </span>
        </span>
      </div>
      <Winner
        clip={clip}
        label="Clip of the night"
        detail={clip ? `${show.clipWinnerVotes} of ${show.clipVoters} votes` : "Nobody voted"}
        tone="clip"
      />
      <Winner
        clip={fail}
        label="Fail of the night"
        detail={fail ? null : "Nobody marked a fail"}
        tone="fail"
      />
      {rest.length > 0 && (
        <div className="flex flex-col gap-1.5">
          <span className="font-mono text-[11px] text-muted">Also played</span>
          <ul className="flex flex-wrap items-center gap-2">
            {rest.slice(0, ALSO_SHOWN).map((c) => (
              <li key={c.id}>
                <Link
                  to="/clips/$clipId"
                  params={{ clipId: c.id }}
                  title={c.title}
                  aria-label={c.title}
                  className="squircle block aspect-video w-20 rounded-[10px] bg-surface bg-cover bg-center ring-1 ring-white/10 hover:ring-white/40"
                  style={{ backgroundImage: c.posterUrl ? `url(${c.posterUrl})` : undefined }}
                />
              </li>
            ))}
            {rest.length > ALSO_SHOWN && (
              <li className="text-sm text-muted">
                and {rest.length - ALSO_SHOWN} more{" "}
                {rest.length - ALSO_SHOWN === 1 ? "clip" : "clips"}
              </li>
            )}
          </ul>
        </div>
      )}
      <Link
        to="/shows/$showId/replay"
        params={{ showId: show.id }}
        className={`${buttonClass("secondary", "md")} mt-auto self-start`}
      >
        Watch the replay
      </Link>
    </Panel>
  );
}

function Winner({
  clip,
  label,
  detail,
  tone,
}: {
  clip: Clip | null;
  label: string;
  detail: string | null;
  tone: "clip" | "fail";
}) {
  const color = tone === "clip" ? "text-accent-strong" : "text-[#c9bcff]";
  const ring = tone === "clip" ? "ring-accent" : "ring-violet";
  if (!clip) {
    return (
      <span className="flex flex-col text-sm">
        <span className={`font-mono text-[11px] ${color}`}>{label}</span>
        <span className="text-muted">{detail}</span>
      </span>
    );
  }
  return (
    <Link
      to="/clips/$clipId"
      params={{ clipId: clip.id }}
      className="flex min-w-0 items-center gap-3 text-text hover:text-white"
    >
      <span
        className={`squircle aspect-video w-28 shrink-0 rounded-xl bg-surface bg-cover bg-center ring-2 ${ring}`}
        style={{ backgroundImage: clip.posterUrl ? `url(${clip.posterUrl})` : undefined }}
      />
      <span className="flex min-w-0 flex-col">
        <span className={`font-mono text-[11px] ${color}`}>{label}</span>
        <span dir="auto" className="truncate font-bold">
          {clip.title}
        </span>
        <span className="truncate text-[13px] text-muted">
          <bdi>{clip.uploader.displayName}</bdi>
          {detail && ` · ${detail}`}
        </span>
      </span>
    </Link>
  );
}
