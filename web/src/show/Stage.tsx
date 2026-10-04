import { useQueryClient } from "@tanstack/react-query";
import { type RefObject, useEffect, useRef, useState } from "react";
import { useClip } from "../clips/hooks";
import { useClipAnalysis } from "../clips/killfeed/analysis";
import { Kip } from "../kip/Kip";
import { Avatar } from "../ui/Avatar";
import { Backdrop } from "../ui/Backdrop";
import { Button } from "../ui/Button";
import { useToast } from "../ui/Toast";
import type { Show } from "./hooks";
import type { ClientMsg } from "./live";
import { BANANA, FloatingReactions, ReactButton, RoundButton, useFloats } from "./Reactions";
import { ShowHeader, type Tone } from "./ShowHeader";
import { type SyncStatus, targetMs } from "./sync";
import { UpNext } from "./UpNext";
import { usePlayback } from "./usePlayback";
import type { ShowEvent, ShowLive } from "./useShowLive";

/** How far ahead the host's "play this now" starts it: time for everyone to fetch it. */
const START_LEAD_MS = 1500;
const JUMP_MS = 10_000;

const STATUS: Record<SyncStatus, [string, Tone]> = {
  synced: ["Synced", "green"],
  catchingUp: ["Catching up", "amber"],
  starting: ["Starting…", "amber"],
  blocked: ["Click to join with sound", "amber"],
  idle: ["Waiting for the host", "grey"],
};

// The show, live (canvas 2.2 the host's screen, 2.3 a friend's): the clip everyone's
// watching in sync, whose clip it is, the controls (the host's: pause, jump, next; a
// friend's: ask for it again; everyone's: React), reactions floating up the player, and
// the side panel with what's next.
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
  const upcoming = lineup.filter((l) => !l.playedAt && l.clip.id !== clipId);
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
  const positionMs = Math.min(playback.positionMs, durationMs ?? Number.POSITIVE_INFINITY);

  // The killfeed says whether the uploader died: Kip asks "Fail? 🍌" once they have. A
  // clip saved for the show has none to read until it plays (that releases it).
  const analysis = useClipAnalysis(clipId ?? "", !!clipId && !!state?.playing);
  const died = (analysis.data?.kills ?? []).some(
    (k) => k.owner === "myDeath" && k.t * 1000 <= positionMs,
  );
  const failMarked = !!clipId && show.failContenders.includes(clipId);

  const { floats, add } = useFloats();
  const [requests, setRequests] = useState<string[]>([]);
  const [asked, setAsked] = useState(false);
  const [panel, setPanel] = useState(true);
  // Replay requests are about the clip that's on.
  // biome-ignore lint/correctness/useExhaustiveDependencies: reset when the clip changes.
  useEffect(() => {
    setRequests([]);
    setAsked(false);
  }, [clipId]);
  events.current = (e) => {
    if (e.type === "reaction") {
      // Your own taps floated when you tapped.
      if (e.userId !== meId && e.clipId === clipId) add(e.emoji);
      // 🍌 makes the clip a fail contender.
      if (e.emoji === BANANA) queryClient.invalidateQueries({ queryKey: ["show", show.id] });
    }
    if (e.type === "replayRequest" && isHost && e.userId !== meId) {
      setRequests((r) => (r.includes(e.userId) ? r : [...r, e.userId]));
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
  const replay = () => {
    send({ type: "seek", positionMs: 0 });
    if (!state?.playing) send({ type: "play" });
    setRequests([]);
  };
  const fullScreen = useRef<HTMLDivElement>(null);

  const [status, tone]: [string, Tone] =
    live.connection === "closed"
      ? ["Disconnected", "red"]
      : live.connection !== "open"
        ? ["Reconnecting…", "amber"]
        : clipId && state && !state.playing
          ? ["Paused", "grey"]
          : !clipId && isHost
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
          !panel && (
            <Button size="sm" className="frost" onClick={() => setPanel(true)}>
              Up next
            </Button>
          )
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
            <FloatingReactions floats={floats} />
            {!clipId && (
              <div className="absolute inset-0 grid place-items-center bg-black/50 p-6 text-center">
                {isHost && upcoming[0] ? (
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
                    <p className="text-lg text-soft">
                      {isHost
                        ? "Nothing left in the lineup."
                        : `Waiting for ${hostName} to put a clip on.`}
                    </p>
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
              {isHost && requests[0] && (
                <div
                  role="status"
                  className="glass flex items-center gap-3 rounded-full py-2 pr-2 pl-4 ring-[1.5px] ring-accent"
                >
                  <span className="font-bold">
                    <bdi>{nameOf(requests[0])}</bdi> wants it again
                    {requests.length > 1 && ` (+${requests.length - 1})`}
                  </span>
                  <Button size="sm" variant="primary" onClick={replay}>
                    Replay
                  </Button>
                  <Button size="sm" variant="ghost" onClick={() => setRequests([])}>
                    Dismiss
                  </Button>
                </div>
              )}
              {isHost ? (
                <div className="flex items-center gap-3.5" data-testid="host-controls">
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
              ) : (
                <RoundButton
                  label="Ask for it again"
                  disabled={!clipId || asked}
                  onClick={() => {
                    send({ type: "replayRequest" });
                    setAsked(true);
                    toast(`Asked ${hostName} to play it again.`);
                  }}
                  className="bg-accent/12 ring-[1.5px] ring-accent"
                >
                  <svg
                    viewBox="0 0 24 24"
                    className="h-5 w-5 text-accent"
                    fill="none"
                    stroke="currentColor"
                    strokeWidth="2.2"
                    strokeLinecap="round"
                    strokeLinejoin="round"
                    aria-hidden="true"
                  >
                    <path d="M3 12a9 9 0 1 0 3-6.7L3 8" />
                    <path d="M3 3v5h5" />
                  </svg>
                </RoundButton>
              )}
              <ReactButton
                onReact={react}
                failMarked={failMarked}
                failHint={died}
                disabled={!clipId || live.connection !== "open"}
              />
            </div>
          </div>
        </div>
        {panel && (
          <UpNext
            showId={show.id}
            upcoming={upcoming}
            lineupIds={show.lineup.filter((l) => !l.dropped).map((l) => l.clip.id)}
            remainingMs={clipId && durationMs != null ? Math.max(0, durationMs - positionMs) : null}
            onPlay={isHost ? playNow : undefined}
            onHide={() => setPanel(false)}
          />
        )}
      </div>
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
