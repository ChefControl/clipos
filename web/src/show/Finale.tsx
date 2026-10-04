import { useEffect, useRef, useState } from "react";
import { ApiError } from "../api/errors";
import type { Clip } from "../clips/hooks";
import { Kip } from "../kip/Kip";
import { Avatar } from "../ui/Avatar";
import { Button } from "../ui/Button";
import { useToast } from "../ui/Toast";
import { HostAway } from "./HostAway";
import { type Category, type Show, type TieBreak, useShowActions } from "./hooks";
import { ShowHeader } from "./ShowHeader";
import type { ShowLive } from "./useShowLive";

const LABEL: Record<Category, string> = {
  fail: "Fail of the night",
  clip: "Clip of the night",
};

/** A second past the vote's end before the host's screen counts it: late taps land. */
const COUNT_AFTER_MS = 1_000;

// The finale (canvas 2.5): fail of the night, then clip of the night, 20 s each, on every
// screen at once (the windows come from the server). Only people in the show vote, not
// for their own clip; until it's over nobody sees the counts, only who has voted
// (decision 43). When the vote closes the host's screen ends the show, and a tie is the
// host's to break.
export function Finale({ show, live, meId }: { show: Show; live: ShowLive; meId: string }) {
  const hostId = live.presence?.hostId ?? show.host.id;
  const isHost = hostId === meId;
  const now = useServerNow(live);
  const f = show.finale;
  const at = (iso: string | null | undefined) => (iso ? Date.parse(iso) : null);
  const failUntil = at(f?.failUntil);
  const clipUntil = at(f?.clipUntil) ?? 0;
  const category: Category | null =
    failUntil != null && now < failUntil ? "fail" : now < clipUntil ? "clip" : null;
  const until = category === "fail" ? (failUntil ?? 0) : clipUntil;
  const played = show.lineup.filter((l) => !l.dropped && l.playedAt).map((l) => l.clip);
  const awaySince = isHost ? null : (live.presence?.hostAwaySince ?? null);
  const host = show.participants.find((p) => p.member.id === hostId)?.member ?? show.host;

  return (
    <div
      className="relative flex min-h-dvh flex-col px-4 pb-10 sm:px-10"
      data-testid="show"
      data-connection={live.connection}
    >
      <Backdrops clips={category === "fail" ? failClips(show, played) : played} />
      <ShowHeader
        show={show}
        sub="finale"
        online={live.presence?.online ?? []}
        hostId={hostId}
        status={category ? "Voting" : "Counting"}
        tone="amber"
      />
      {category ? (
        <Vote
          key={category}
          show={show}
          category={category}
          clips={category === "fail" ? failClips(show, played) : played}
          leftMs={Math.max(0, until - now)}
          noFails={f?.failFrom == null}
        />
      ) : (
        <Counting show={show} isHost={isHost} hostName={host.displayName} />
      )}
      {!category && awaySince != null && (
        <div className="squircle relative mx-auto mt-6 aspect-video w-full max-w-3xl overflow-hidden rounded-[26px]">
          <HostAway
            host={host}
            awayMs={now - awaySince}
            clipDone
            onTakeOver={() => live.client?.send({ type: "takeOver" })}
          />
        </div>
      )}
    </div>
  );
}

/** The played clips someone pressed 🍌 on. */
function failClips(show: Show, played: Clip[]): Clip[] {
  return played.filter((c) => show.failContenders.includes(c.id));
}

/** The server's clock now, ticking four times a second. */
export function useServerNow(live: ShowLive): number {
  const [, tick] = useState(0);
  useEffect(() => {
    const every = setInterval(() => tick((t) => t + 1), 250);
    return () => clearInterval(every);
  }, []);
  return live.client?.clock.now() ?? Date.now();
}

function Vote({
  show,
  category,
  clips,
  leftMs,
  noFails,
}: {
  show: Show;
  category: Category;
  clips: Clip[];
  leftMs: number;
  noFails: boolean;
}) {
  const { vote } = useShowActions(show.id);
  const toast = useToast();
  const mine = show.myVotes[category];
  const voters = show.voters[category];
  const fail = category === "fail";
  const seconds = Math.ceil(leftMs / 1000);
  const ring = fail ? "var(--color-violet)" : "var(--color-accent)";
  const theirs = (c: Clip) => c.isMine;
  return (
    <section className="relative mt-4 flex flex-1 flex-col gap-7" aria-label={LABEL[category]}>
      <div className="flex items-end gap-5">
        <div className="flex flex-col gap-1.5">
          <span className={`text-shadow font-mono text-xs ${fail ? "text-violet" : "text-accent"}`}>
            Finale · vote on your own screen
          </span>
          <h2 className="text-shadow text-[44px] leading-[0.92] font-extrabold tracking-tight sm:text-[60px]">
            {LABEL[category]}?
          </h2>
        </div>
        <span
          role="timer"
          aria-label={`${seconds} seconds left`}
          className="ml-auto grid h-[72px] w-[72px] shrink-0 place-items-center rounded-full"
          style={{
            background: `conic-gradient(${ring} 0 ${(leftMs / 200).toFixed(1)}%, rgb(255 255 255 / .14) 0 100%)`,
          }}
        >
          <span className="grid h-[62px] w-[62px] place-items-center rounded-full bg-[rgba(20,21,24,0.9)] font-mono text-[15px] font-medium">
            0:{String(seconds).padStart(2, "0")}
          </span>
        </span>
      </div>
      {clips.length === 0 ? (
        <p className="text-lg text-soft">No clips played, so there's nothing to vote on.</p>
      ) : (
        <div className="grid grid-cols-[repeat(auto-fill,minmax(280px,1fr))] gap-6">
          {clips.map((c) => {
            const own = theirs(c);
            const picked = mine === c.id;
            return (
              <button
                key={c.id}
                type="button"
                aria-pressed={picked}
                disabled={own || vote.isPending}
                onClick={() =>
                  vote.mutate(
                    { category, clipId: c.id },
                    { onError: (err) => toast(`Couldn't vote: ${err.message}`, "danger") },
                  )
                }
                className={`glass squircle flex flex-col gap-3.5 rounded-[30px] p-3 pb-4 text-left text-text transition disabled:opacity-50 ${
                  picked
                    ? fail
                      ? "shadow-[0_0_0_2px_var(--color-violet),0_0_40px_rgba(164,139,255,0.25)]"
                      : "shadow-[0_0_0_2px_var(--color-accent),0_0_40px_rgba(245,165,36,0.25)]"
                    : "hover:bg-white/8"
                }`}
              >
                <span
                  className="squircle block aspect-video w-full rounded-[20px] bg-surface bg-cover bg-center"
                  style={{ backgroundImage: c.posterUrl ? `url(${c.posterUrl})` : undefined }}
                />
                <span className="flex items-center gap-3 px-2">
                  <span className="flex min-w-0 flex-col gap-0.5">
                    <span dir="auto" className="truncate text-[26px] leading-tight font-bold">
                      {c.title}
                    </span>
                    <bdi className="truncate text-[15px] text-soft">{c.uploader.displayName}</bdi>
                  </span>
                  <span
                    aria-hidden="true"
                    className={`ml-auto grid h-[30px] w-[30px] shrink-0 place-items-center rounded-full ${
                      picked
                        ? fail
                          ? "bg-violet"
                          : "bg-accent"
                        : own
                          ? ""
                          : "ring-2 ring-white/30 ring-inset"
                    }`}
                  >
                    {picked && (
                      <svg
                        viewBox="0 0 24 24"
                        className="h-[17px] w-[17px]"
                        aria-hidden="true"
                        fill="none"
                        stroke="#0e0f11"
                        strokeWidth="3"
                        strokeLinecap="round"
                        strokeLinejoin="round"
                      >
                        <path d="M5 12.5l4.5 4.5L19 7.5" />
                      </svg>
                    )}
                  </span>
                </span>
                <span className="px-2 text-[13px] text-muted">
                  {own ? "Your clip" : picked ? "Your vote" : "Click to vote"}
                </span>
              </button>
            );
          })}
        </div>
      )}
      <div className="mt-auto flex flex-wrap items-center gap-4">
        <div
          className="glass flex items-center gap-3 rounded-full py-2 pr-5 pl-2.5"
          data-testid="voted"
        >
          <span className="flex -space-x-1.5">
            {voters.map((id) => {
              const p = show.participants.find((x) => x.member.id === id)?.member;
              return p ? (
                <Avatar
                  key={id}
                  name={p.displayName}
                  url={p.avatarUrl}
                  size={28}
                  className="ring-2 ring-bg"
                />
              ) : null;
            })}
          </span>
          <span className="text-[15px] font-semibold">
            {voters.length} of {show.participants.length} voted
          </span>
        </div>
        {category === "clip" && noFails && (
          <div className="glass flex items-center gap-3.5 rounded-full py-2 pr-5 pl-2">
            <Kip className="h-11 w-11" />
            <span className="text-[17px]">
              <b>Fail of the night:</b> nobody marked a fail tonight. Suspiciously clean.
            </span>
          </div>
        )}
      </div>
    </section>
  );
}

/** The vote's closed: the host's screen ends the show, and breaks a tie. */
function Counting({ show, isHost, hostName }: { show: Show; isHost: boolean; hostName: string }) {
  const { end } = useShowActions(show.id);
  const [ties, setTies] = useState<{ clip: string[]; fail: string[] } | null>(null);
  const [picks, setPicks] = useState<TieBreak>({});
  const [failed, setFailed] = useState<string | null>(null);
  const tried = useRef(false);
  const finish = (tieBreak?: TieBreak) =>
    end.mutate(tieBreak, {
      onError: (err) => {
        if (err instanceof ApiError && err.tied) {
          setTies({ clip: err.tied.clip, fail: err.tied.fail });
          setFailed(null);
        } else {
          setFailed(err.message);
        }
      },
    });
  // biome-ignore lint/correctness/useExhaustiveDependencies: once, when the vote closes.
  useEffect(() => {
    if (!isHost || tried.current) return;
    tried.current = true;
    const t = setTimeout(() => finish(), COUNT_AFTER_MS);
    return () => clearTimeout(t);
  }, [isHost]);

  const byId = new Map(show.lineup.map((l) => [l.clip.id, l.clip]));
  const tied = (["fail", "clip"] as const).filter((c) => (ties?.[c]?.length ?? 0) > 0);
  if (isHost && tied.length > 0) {
    const ready = tied.every((c) => picks[c]);
    return (
      <section className="mt-4 flex flex-col gap-6" aria-label="Host decides">
        <div className="flex flex-col gap-1.5">
          <span className="font-mono text-xs text-accent">Finale · a tie</span>
          <h2 className="text-[44px] leading-[0.92] font-extrabold tracking-tight sm:text-[60px]">
            Host decides
          </h2>
          <p className="text-lg text-soft">The votes tied. You pick the winner.</p>
        </div>
        {tied.map((c) => (
          <fieldset key={c} className="flex flex-col gap-3">
            <legend className="mb-3 text-2xl font-bold">{LABEL[c]}</legend>
            <div className="grid grid-cols-[repeat(auto-fill,minmax(240px,1fr))] gap-4">
              {(ties?.[c] ?? []).map((id) => {
                const clip = byId.get(id);
                const on = picks[c] === id;
                return (
                  <button
                    key={id}
                    type="button"
                    aria-pressed={on}
                    onClick={() => setPicks((p) => ({ ...p, [c]: id }))}
                    className={`glass squircle flex flex-col gap-2.5 rounded-[24px] p-2.5 text-left ${on ? "shadow-[0_0_0_2px_var(--color-accent)]" : "hover:bg-white/8"}`}
                  >
                    <span
                      className="squircle block aspect-video rounded-[16px] bg-surface bg-cover bg-center"
                      style={{
                        backgroundImage: clip?.posterUrl ? `url(${clip.posterUrl})` : undefined,
                      }}
                    />
                    <span dir="auto" className="truncate px-1.5 text-lg font-bold">
                      {clip?.title ?? "A clip"}
                    </span>
                  </button>
                );
              })}
            </div>
          </fieldset>
        ))}
        <Button
          variant="primary"
          size="lg"
          className="self-start font-extrabold"
          disabled={!ready || end.isPending}
          onClick={() => finish(picks)}
        >
          Announce the winners
        </Button>
      </section>
    );
  }
  return (
    <div className="mt-16 flex flex-col items-center gap-4 text-center" aria-live="polite">
      <Kip pose="cheer" className="h-32 w-32" />
      <h2 className="text-[34px] font-extrabold">Counting the votes…</h2>
      <p className="text-soft">
        {isHost ? "One moment." : `${hostName} announces the winners in a moment.`}
      </p>
      {failed && (
        <div role="alert" className="flex items-center gap-3 text-danger">
          Couldn't end the show: {failed}
          <Button size="sm" onClick={() => finish()}>
            Try again
          </Button>
        </div>
      )}
    </div>
  );
}

/** The candidates' posters, blurred in strips behind the vote (2.5). */
function Backdrops({ clips }: { clips: Clip[] }) {
  const shown = clips.filter((c) => c.posterUrl).slice(0, 3);
  return (
    <div aria-hidden="true" className="pointer-events-none fixed inset-0 -z-10 overflow-hidden">
      {shown.map((c, i) => (
        <img
          key={c.id}
          src={c.posterUrl ?? undefined}
          alt=""
          className="absolute top-[120px] h-[calc(100%-80px)] object-cover blur-[46px] brightness-[.52] saturate-[1.25] [mask-image:linear-gradient(90deg,transparent,#000_32%,#000_68%,transparent)]"
          style={{
            left: `${(i / shown.length) * 100 - 8}%`,
            width: `${100 / shown.length + 16}%`,
          }}
        />
      ))}
      <div className="absolute inset-0 bg-[radial-gradient(ellipse_75%_65%_at_50%_55%,rgba(5,5,6,0),rgba(5,5,6,.5)),linear-gradient(180deg,rgba(5,5,6,.85)_0,rgba(5,5,6,.1)_20%,rgba(5,5,6,0)_60%,rgba(5,5,6,.7)_100%)]" />
    </div>
  );
}
