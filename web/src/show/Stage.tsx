import { useQueryClient } from "@tanstack/react-query";
import { type RefObject, useEffect, useRef, useState } from "react";
import { useClip } from "../clips/hooks";
import { useClipAnalysis } from "../clips/killfeed/analysis";
import { KillCard } from "../kip/KillCard";
import { Kip } from "../kip/Kip";
import { Avatar } from "../ui/Avatar";
import { Backdrop } from "../ui/Backdrop";
import { Button } from "../ui/Button";
import { Modal } from "../ui/Modal";
import { useToast } from "../ui/Toast";
import { Between } from "./Between";
import { HostAway } from "./HostAway";
import { type Show, useShowActions } from "./hooks";
import { CARD_OUTRO_MS, killCard } from "./killCard";
import type { ClientMsg } from "./live";
import { BANANA, FloatingReactions, ReactButton, RoundButton, useFloats } from "./Reactions";
import { ShowHeader, type Tone } from "./ShowHeader";
import { type SyncStatus, targetMs } from "./sync";
import { UpNext } from "./UpNext";
import { usePlayback } from "./usePlayback";
import type { ShowEvent, ShowLive } from "./useShowLive";
import { Volume } from "./Volume";

/** How far ahead the host's "play this now" starts it: time for everyone to fetch it. */
const START_LEAD_MS = 1500;
const JUMP_MS = 10_000;
/** Up next's count between clips (2.4): 5 s, just inside the hub's 5 s limit on starting
 *  ahead. */
const COUNTDOWN_MS = 4_900;
/** A clip this close to its end has ended (the player stops a frame short). */
const END_SLACK_MS = 100;
/** Off by this much, "Catching up with …" says so over the player (2.8). */
const CATCHING_UP_MS = 1_000;

const STATUS: Record<SyncStatus, [string, Tone]> = {
  synced: ["Synced", "green"],
  catchingUp: ["Catching up", "amber"],
  starting: ["Starting…", "amber"],
  blocked: ["Click to join with sound", "amber"],
  idle: ["Waiting", "grey"],
};

// The show, live (canvas 2.2 the host's screen, 2.3 a friend's): the clip everyone's
// watching in sync, whose clip it is, the controls (everyone's since decision 57: pause,
// jump, next, React; your own volume), reactions floating up the player, and the side
// panel with what's next. The host's screen still moves the show on between clips.
export function Stage({
  show,
  live,
  meId,
  events,
}: {
  show: Show;
  live: ShowLive;
  meId: string;
  /** Where the room's one-off events (reactions, replay requests) are handed in. */
  events: RefObject<((e: ShowEvent) => void) | null>;
}) {
  const queryClient = useQueryClient();
  const toast = useToast();
  const hostId = live.presence?.hostId ?? show.host.id;
  const isHost = hostId === meId;
  const state = live.state;
  const clipId = state?.clipId ?? null;
  const lineup = show.lineup.filter((l) => !l.dropped);
  const entry = lineup.find((l) => l.clip.id === clipId) ?? null;
  // Clips that went on while this page was open have played, whatever the show's details
  // (fetched again with each new clip, below) say so far.
  const seen = useRef(new Set<string>());
  if (clipId) seen.current.add(clipId);
  const upcoming = lineup.filter((l) => !l.playedAt && !seen.current.has(l.clip.id));
  const nextId = upcoming[0]?.clip.id ?? null;
  const clip = useClip(clipId ?? "", { enabled: !!clipId });
  // The next clip, fetched while this one plays so it starts without a stall (decision 35).
  const next = useClip(nextId ?? "", { enabled: !!nextId && !!clipId });
  const nameOf = (id: string) =>
    show.participants.find((p) => p.member.id === id)?.member.displayName ?? "Someone";
  const hostName = nameOf(hostId);

  // Each clip keeps the first playback link it got: a refetch signs a new one, and a new
  // src would reload the video mid-clip (and miss the preloaded copy).
  const urls = useRef(new Map<string, string>());
  for (const c of [clip.data, next.data]) {
    if (c?.playbackUrl && !urls.current.has(c.id)) urls.current.set(c.id, c.playbackUrl);
  }
  const src = (clipId && urls.current.get(clipId)) || null;
  const nextSrc = (nextId && urls.current.get(nextId)) || null;
  const playback = usePlayback(live.client, state, src);
  const durationMs = state?.durationMs ?? entry?.clip.durationMs ?? null;
  // Where the room is, worked out on each render (the playback ticks four times a
  // second): a number kept from the last tick would still be the last clip's.
  const serverNow = live.client?.clock.now() ?? Date.now();
  const positionMs = state ? Math.max(0, targetMs(state, serverNow)) : 0;

  // The killfeed says whether the uploader died: Kip asks "Fail? 🍌" once they have. A
  // clip saved for the show has none to read until it plays (that releases it).
  const analysis = useClipAnalysis(clipId ?? "", !!clipId && !!state?.playing);
  const died = (analysis.data?.kills ?? []).some(
    (k) => k.owner === "myDeath" && k.t * 1000 <= positionMs,
  );
  // Kip's kill card (S8): the uploader's kills, when they die in the clip, or for the clip's
  // last few seconds; then it leaves. Every screen shows it at the same moment, since it
  // goes by the room's position. No killfeed, no card.
  const card = killCard(analysis.data ?? null, durationMs);
  const cardOn = !!card && positionMs >= card.fromMs && positionMs < card.untilMs;
  const failMarked = !!clipId && show.failContenders.includes(clipId);

  // The clip before this one, for Up next's "Just played".
  const last = useRef<string | null>(null);
  const [previousId, setPreviousId] = useState<string | null>(null);
  useEffect(() => {
    if (!clipId) return;
    if (last.current && last.current !== clipId) setPreviousId(last.current);
    last.current = clipId;
    // Played now, and the last one's reactions are all in.
    queryClient.invalidateQueries({ queryKey: ["show", show.id] });
  }, [clipId, queryClient, show.id]);
  const index = entry ? lineup.indexOf(entry) : -1;
  const before = index > 0 ? lineup[index - 1] : undefined;
  const previous =
    lineup.find((l) => l.clip.id === previousId) ?? (before?.playedAt ? before : null);

  // Between clips (2.4): the clip on hasn't started yet, counting down to its start, or
  // held at 0 by the host.
  const started = useRef(new Set<string>());
  if (clipId && positionMs > 0) started.current.add(clipId);
  const between =
    !!clipId &&
    !!state &&
    !!entry &&
    !started.current.has(clipId) &&
    positionMs <= 0 &&
    (!state.playing || state.atServerMs > serverNow);
  const startsInMs = between && state?.playing ? state.atServerMs - serverNow : null;
  const ended =
    !!clipId && !!state?.playing && durationMs != null && positionMs >= durationMs - END_SLACK_MS;
  const online = live.presence?.online ?? [];
  const watching = show.participants.filter((p) => online.includes(p.member.id));
  const ready = watching.filter((p) => p.ready || p.member.id === hostId).length;
  // The host left: how long ago, by the server's clock.
  const awaySince = isHost ? null : (live.presence?.hostAwaySince ?? null);
  // Counts down on screen (Up next, the takeover) even when nothing else ticks.
  const [, tick] = useState(0);
  useEffect(() => {
    if (!between && awaySince == null) return;
    const every = setInterval(() => tick((t) => t + 1), 250);
    return () => clearInterval(every);
  }, [between, awaySince]);

  const { floats, add } = useFloats();
  const [panel, setPanel] = useState(true);
  events.current = (e) => {
    if (e.type === "reaction") {
      // Your own taps floated when you tapped.
      if (e.userId !== meId && e.clipId === clipId) add(e.emoji);
      // 🍌 makes the clip a fail contender.
      if (e.emoji === BANANA) queryClient.invalidateQueries({ queryKey: ["show", show.id] });
    }
  };

  const send = (msg: ClientMsg) => live.client?.send(msg);
  const now = () => live.client?.clock.now() ?? Date.now();
  /** Where the room is right now (not at the last tick): for jumps from it. */
  const here = () => (state ? Math.max(0, targetMs(state, now())) : 0);
  const playNow = (id: string) =>
    send({ type: "load", clipId: id, startAt: now() + START_LEAD_MS });
  const react = (emoji: string) => {
    if (!clipId) return;
    add(emoji);
    send({ type: "react", clipId, emoji, atMs: Math.round(positionMs) });
  };
  // The host's screen moves the show on: when a clip ends, the next one counts down
  // for everyone (Hold stops it). Once per clip, unless it's played again.
  const advanced = useRef<string | null>(null);
  // biome-ignore lint/correctness/useExhaustiveDependencies: on the clip ending, not on every render.
  useEffect(() => {
    if (!ended) {
      if (advanced.current === clipId) advanced.current = null;
      return;
    }
    if (!isHost || !clipId || advanced.current === clipId) return;
    advanced.current = clipId;
    if (nextId) send({ type: "load", clipId: nextId, startAt: now() + COUNTDOWN_MS });
  }, [ended, isHost, clipId, nextId]);
  const { finale } = useShowActions(show.id);
  const [ending, setEnding] = useState(false);
  const toFinale = () =>
    finale.mutate(undefined, {
      onSuccess: () => setEnding(false),
      onError: (err) => toast(`Couldn't go to the finale: ${err.message}`, "danger"),
    });
  const fullScreen = useRef<HTMLDivElement>(null);

  const [status, tone]: [string, Tone] =
    live.connection === "closed"
      ? ["Disconnected", "red"]
      : live.connection !== "open"
        ? ["Reconnecting…", "amber"]
        : clipId && state && !state.playing
          ? ["Paused", "grey"]
          : !clipId
            ? ["Nothing on yet", "grey"]
            : STATUS[playback.status];
  const position = entry ? lineup.indexOf(entry) + 1 : lineup.filter((l) => l.playedAt).length;
  const uploader = entry?.clip.uploader;

  return (
    <div
      className="flex min-h-dvh flex-col px-4 pb-10 sm:px-10"
      data-testid="show"
      data-connection={live.connection}
    >
      <Backdrop image={entry?.clip.posterUrl} />
      <ShowHeader
        show={show}
        sub={`clip ${Math.max(position, 1)} of ${lineup.length}`}
        online={live.presence?.online ?? []}
        hostId={hostId}
        status={status}
        tone={tone}
        actions={
          <>
            {isHost && (
              <Button size="sm" variant="ghost" className="frost" onClick={() => setEnding(true)}>
                End the show
              </Button>
            )}
            {!panel && (
              <Button size="sm" className="frost" onClick={() => setPanel(true)}>
                Up next
              </Button>
            )}
          </>
        }
      />
      <div
        className={`mt-2 grid grid-cols-1 items-start gap-8 ${panel ? "lg:grid-cols-[minmax(0,1fr)_312px]" : ""}`}
      >
        <div className="flex min-w-0 flex-col">
          <div
            ref={fullScreen}
            className="squircle relative aspect-video overflow-hidden rounded-[26px] bg-black shadow-[0_30px_70px_-30px_rgba(0,0,0,0.95),0_0_0_1px_rgba(255,255,255,0.06)]"
          >
            {/* biome-ignore lint/a11y/useMediaCaption: game clips have no captions. */}
            <video
              ref={playback.video}
              src={src ?? undefined}
              playsInline
              preload="auto"
              className="h-full w-full object-contain"
              data-testid="show-video"
              data-drift={playback.driftMs}
            />
            {nextSrc && (
              // Never shown (display: none keeps it out of the accessibility tree); it
              // only preloads.
              <video
                src={nextSrc}
                preload="auto"
                muted
                className="hidden"
                data-testid="next-video"
              />
            )}
            {cardOn && card && clipId && (
              <KillCard
                key={`${clipId}-${card.fromMs}`}
                kills={card.kills}
                leaving={positionMs >= card.untilMs - CARD_OUTRO_MS}
              />
            )}
            <FloatingReactions floats={floats} />
            {between && entry && (
              <Between
                show={show}
                next={entry}
                index={index + 1}
                count={lineup.length}
                startsInMs={startsInMs}
                ready={ready}
                watching={watching.length}
                previous={previous}
              />
            )}
            {ended && !nextId && (
              <div className="absolute inset-0 grid place-items-center bg-black/60 p-6 text-center">
                <div className="flex flex-col items-center gap-3">
                  <Kip pose="cheer" className="h-28 w-28" />
                  <p className="text-[28px] font-extrabold">That was every clip.</p>
                  {isHost ? (
                    <Button
                      variant="primary"
                      size="lg"
                      className="h-14 px-7 text-lg font-extrabold"
                      disabled={finale.isPending}
                      onClick={toFinale}
                    >
                      Go to the finale
                    </Button>
                  ) : (
                    <p className="text-lg text-soft">
                      Waiting for <bdi>{hostName}</bdi> to open the finale.
                    </p>
                  )}
                </div>
              </div>
            )}
            {!between &&
              playback.status === "catchingUp" &&
              Math.abs(playback.driftMs) >= CATCHING_UP_MS && (
                <span
                  role="status"
                  className="glass absolute top-5 left-1/2 flex -translate-x-1/2 items-center gap-2.5 rounded-full py-2 pr-4 pl-3 font-semibold whitespace-nowrap"
                >
                  <span className="h-2 w-2 animate-pulse rounded-full bg-accent" />
                  Catching up with <bdi>{hostName}</bdi>
                  <span className="text-[13px] text-accent-strong">
                    {(Math.abs(playback.driftMs) / 1000).toFixed(1)} s{" "}
                    {playback.driftMs < 0 ? "behind" : "ahead"}
                  </span>
                </span>
              )}
            {awaySince != null && (
              <HostAway
                host={show.participants.find((p) => p.member.id === hostId)?.member ?? show.host}
                awayMs={serverNow - awaySince}
                clipDone={!clipId || !state?.playing || ended}
                onTakeOver={() => send({ type: "takeOver" })}
              />
            )}
            {!clipId && (
              <div className="absolute inset-0 grid place-items-center bg-black/50 p-6 text-center">
                {upcoming[0] ? (
                  <Button
                    variant="primary"
                    size="lg"
                    className="h-14 px-7 text-lg font-extrabold"
                    onClick={() => playNow(upcoming[0]?.clip.id ?? "")}
                  >
                    Start with {upcoming[0].clip.title}
                  </Button>
                ) : (
                  <div className="flex flex-col items-center gap-3">
                    <Kip pose="asleep" className="h-28 w-28" />
                    <p className="text-lg text-soft">Nothing left in the lineup.</p>
                  </div>
                )}
              </div>
            )}
            {playback.status === "blocked" && (
              <div className="absolute inset-0 grid place-items-center bg-black/60">
                <Button variant="primary" size="lg" onClick={playback.unblock}>
                  Join with sound
                </Button>
              </div>
            )}
            <Volume video={playback.video} />
            <button
              type="button"
              aria-label="Full screen"
              title="Full screen"
              onClick={() =>
                document.fullscreenElement
                  ? document.exitFullscreen()
                  : fullScreen.current?.requestFullscreen()
              }
              className="frost absolute right-[18px] bottom-5 grid h-[52px] w-[52px] place-items-center rounded-full text-text hover:bg-white/15"
            >
              <svg
                viewBox="0 0 24 24"
                className="h-5 w-5"
                fill="none"
                stroke="currentColor"
                strokeWidth="2.2"
                strokeLinecap="round"
                strokeLinejoin="round"
                aria-hidden="true"
              >
                <path d="M4 9V4h5M20 9V4h-5M4 15v5h5M20 15v5h-5" />
              </svg>
            </button>
            {clipId && durationMs != null && (
              <span aria-hidden="true" className="absolute inset-x-0 bottom-0 h-1 bg-text/25">
                <span
                  className="absolute inset-y-0 left-0 bg-accent"
                  style={{ width: `${Math.min(100, (positionMs / durationMs) * 100)}%` }}
                />
              </span>
            )}
          </div>

          {between && entry ? (
            <div className="flex flex-wrap items-center gap-4 px-1 pt-5">
              <div className="flex min-w-0 flex-col gap-1.5">
                <span className="flex items-center gap-3">
                  <span className="text-shadow text-[30px] leading-tight font-bold">
                    Between clips
                  </span>
                  {isHost && (
                    <span className="glass shrink-0 rounded-full px-2.5 py-0.5 text-[13px] font-semibold text-accent">
                      You are hosting
                    </span>
                  )}
                </span>
                <span className="text-[17px] text-soft">
                  {startsInMs == null
                    ? "Held. It starts when someone presses Start now."
                    : `Everyone sees this. ${entry.clip.isMine ? "Your" : `${entry.clip.uploader.displayName}'s`} clip starts on its own when the count hits 0.`}
                </span>
              </div>
              <div className="ml-auto flex items-center gap-3">
                {startsInMs != null && (
                  <Button
                    size="lg"
                    className="frost h-[52px]"
                    onClick={() => send({ type: "pause" })}
                  >
                    Hold
                  </Button>
                )}
                <Button
                  variant="primary"
                  size="lg"
                  className="h-[52px] font-extrabold"
                  onClick={() => send({ type: "play" })}
                >
                  Start now
                </Button>
              </div>
            </div>
          ) : (
            <div className="flex flex-wrap items-center gap-4 px-1 pt-5">
              <div className="flex min-w-0 flex-col gap-1.5">
                {uploader && entry && (
                  <>
                    <span className="flex min-w-0 items-center gap-3">
                      <Avatar name={uploader.displayName} url={uploader.avatarUrl} size={40} />
                      <span className="text-shadow truncate text-[30px] leading-tight font-bold">
                        {entry.clip.isMine ? "Your clip" : `${uploader.displayName}'s clip`}
                      </span>
                      {isHost && (
                        <span className="glass shrink-0 rounded-full px-2.5 py-0.5 text-[13px] font-semibold text-accent">
                          You are hosting
                        </span>
                      )}
                    </span>
                    <span
                      dir="auto"
                      className="text-shadow truncate text-[22px] leading-tight font-bold text-[#d6d3cb]"
                    >
                      {entry.clip.title}
                    </span>
                  </>
                )}
              </div>
              <div className="ml-auto flex flex-wrap items-center justify-end gap-3.5">
                <div className="flex items-center gap-3.5" data-testid="controls">
                  <RoundButton
                    label="Back 10 s"
                    disabled={!clipId}
                    onClick={() =>
                      send({ type: "seek", positionMs: Math.max(0, here() - JUMP_MS) })
                    }
                  >
                    <JumpIcon back />
                  </RoundButton>
                  {state?.playing ? (
                    <RoundButton label="Pause for everyone" onClick={() => send({ type: "pause" })}>
                      <svg
                        viewBox="0 0 24 24"
                        className="h-5 w-5"
                        fill="currentColor"
                        aria-hidden="true"
                      >
                        <rect x="6" y="5" width="4" height="14" rx="1" />
                        <rect x="14" y="5" width="4" height="14" rx="1" />
                      </svg>
                    </RoundButton>
                  ) : (
                    <RoundButton
                      label="Play for everyone"
                      disabled={!clipId}
                      onClick={() => send({ type: "play" })}
                    >
                      <svg
                        viewBox="0 0 24 24"
                        className="h-5 w-5"
                        fill="currentColor"
                        aria-hidden="true"
                      >
                        <path d="M7 4v16l13-8z" />
                      </svg>
                    </RoundButton>
                  )}
                  <RoundButton
                    label="Forward 10 s"
                    disabled={!clipId}
                    onClick={() => send({ type: "seek", positionMs: here() + JUMP_MS })}
                  >
                    <JumpIcon />
                  </RoundButton>
                  <RoundButton
                    label="Next clip"
                    disabled={!clipId || !nextId}
                    onClick={() => nextId && playNow(nextId)}
                  >
                    <svg
                      viewBox="0 0 24 24"
                      className="h-5 w-5"
                      fill="currentColor"
                      aria-hidden="true"
                    >
                      <path d="M5 4v16l11-8zM17 4h2.5v16H17z" />
                    </svg>
                  </RoundButton>
                </div>
                <ReactButton
                  onReact={react}
                  failMarked={failMarked}
                  failHint={died}
                  disabled={!clipId || live.connection !== "open"}
                />
              </div>
            </div>
          )}
        </div>
        {panel && (
          <UpNext
            showId={show.id}
            upcoming={upcoming}
            lineupIds={show.lineup.filter((l) => !l.dropped).map((l) => l.clip.id)}
            remainingMs={
              clipId && !between && durationMs != null ? Math.max(0, durationMs - positionMs) : null
            }
            onPlay={playNow}
            onHide={() => setPanel(false)}
          />
        )}
      </div>
      <Modal open={ending} onClose={() => setEnding(false)} title="End the show now?">
        <p className="text-soft">
          Everyone goes to the finale to vote on the clips played so far. The ones not played yet go
          back to tonight for the next show.
        </p>
        <div className="flex justify-end gap-2">
          <Button variant="ghost" onClick={() => setEnding(false)}>
            Keep watching
          </Button>
          <Button variant="primary" disabled={finale.isPending} onClick={toFinale}>
            End and vote
          </Button>
        </div>
      </Modal>
    </div>
  );
}

function JumpIcon({ back = false }: { back?: boolean }) {
  return (
    <svg
      viewBox="0 0 24 24"
      className={`h-5 w-5 ${back ? "" : "-scale-x-100"}`}
      fill="none"
      stroke="currentColor"
      strokeWidth="2.2"
      strokeLinecap="round"
      strokeLinejoin="round"
      aria-hidden="true"
    >
      <path d="M3 12a9 9 0 1 0 3-6.7L3 8" />
      <path d="M3 3v5h5" />
      <text
        x="12"
        y="15.5"
        textAnchor="middle"
        fontSize="7"
        fill="currentColor"
        stroke="none"
        fontWeight="700"
      >
        10
      </text>
    </svg>
  );
}
