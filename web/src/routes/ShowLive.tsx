import { useQuery } from "@tanstack/react-query";
import { getRouteApi } from "@tanstack/react-router";
import { useEffect, useRef, useState } from "react";
import { call, isNotFound } from "../api/errors";
import { useApi } from "../auth/ApiProvider";
import { useClip } from "../clips/hooks";
import { useTitle } from "../lib/useTitle";
import { isSafari, SyncEngine, type SyncStatus, targetMs } from "../show/sync";
import { useShowLive } from "../show/useShowLive";
import { Button } from "../ui/Button";
import { LoadError } from "../ui/LoadError";
import { useToast } from "../ui/Toast";
import { NotFound } from "./NotFound";

const route = getRouteApi("/shows/$showId");

const STATUS: Record<SyncStatus, string> = {
  synced: "Synced",
  catchingUp: "Catching up",
  starting: "Starting…",
  blocked: "Click to join with sound",
  idle: "Waiting for the host",
};

// The show, bare (S5): the synced player and the host's controls, to prove the sync. S6
// turns this into the show screens from the canvas (2.2, 2.3).
export function ShowLive() {
  const { showId } = route.useParams();
  const api = useApi();
  const show = useQuery({
    queryKey: ["show", showId],
    queryFn: () => call(api.GET("/api/shows/{id}", { params: { path: { id: showId } } })),
  });
  const toast = useToast();
  useTitle(show.data && `${show.data.host.displayName}'s show`);
  // Refusals the connection survives ("only while the show is live") as toasts; the rate
  // limit's "slow down" quietly.
  const live = useShowLive(showId, (e) => {
    if (e.type === "error" && e.message !== "slow down") toast(e.message, "danger");
  });
  const clipId = live.state?.clipId ?? null;
  const clip = useClip(clipId ?? "", { enabled: !!clipId });
  const video = useRef<HTMLVideoElement>(null);
  // The engine drives one clip: the one it was made for.
  const engine = useRef<{ clipId: string; sync: SyncEngine } | null>(null);
  const [status, setStatus] = useState<SyncStatus>("idle");
  const [drift, setDrift] = useState(0);
  const isHost = !!live.presence && live.presence.hostId === live.userId;

  const send = live.client?.send.bind(live.client);
  const lineup = show.data?.lineup.filter((l) => !l.dropped) ?? [];
  // The next clip, fetched while this one plays so it starts without a stall (decision 35).
  const current = lineup.findIndex((l) => l.clip.id === clipId);
  const nextId = lineup.slice(current + 1).find((l) => !l.playedAt)?.clip.id ?? null;
  const next = useClip(nextId ?? "", { enabled: !!nextId && !!clipId });
  const now = () => live.client?.clock.now() ?? Date.now();

  // Each clip keeps the first playback link it got: a refetch signs a new one, and a new
  // src would reload the video mid-clip (and miss the preloaded copy).
  const urls = useRef(new Map<string, string>());
  for (const c of [clip.data, next.data]) {
    if (c?.playbackUrl && !urls.current.has(c.id)) urls.current.set(c.id, c.playbackUrl);
  }
  const src = (clipId && urls.current.get(clipId)) || null;
  const nextSrc = (nextId && urls.current.get(nextId)) || null;

  // A new clip: silence the old one straight away (until the new one's link arrives the
  // video still holds it), then a new engine once its source is in.
  useEffect(() => {
    const v = video.current;
    if (!v) return;
    if (engine.current && engine.current.clipId !== clipId) {
      v.pause();
      engine.current = null;
      setStatus("idle");
    }
    if (clipId && src && !engine.current) {
      engine.current = { clipId, sync: new SyncEngine(v, { safari: isSafari() }) };
    }
  }, [clipId, src]);

  // Step it four times a second and on every time update.
  const stateRef = useRef(live.state);
  stateRef.current = live.state;
  const clientRef = useRef(live.client);
  clientRef.current = live.client;
  useEffect(() => {
    const step = () => {
      const e = engine.current;
      const c = clientRef.current;
      const state = stateRef.current;
      if (!e || e.clipId !== state?.clipId || !c?.clock.synced) return;
      setStatus(e.sync.step(state, c.clock.now()));
      setDrift(Math.round(e.sync.driftMs));
    };
    const waiting = () => engine.current?.sync.setWaiting(true);
    const playing = () => engine.current?.sync.setWaiting(false);
    const every = setInterval(step, 250);
    const v = video.current;
    v?.addEventListener("timeupdate", step);
    v?.addEventListener("waiting", waiting);
    v?.addEventListener("playing", playing);
    return () => {
      clearInterval(every);
      v?.removeEventListener("timeupdate", step);
      v?.removeEventListener("waiting", waiting);
      v?.removeEventListener("playing", playing);
    };
  }, []);

  // No such show (or shows aren't open to you yet).
  if (isNotFound(show.error)) {
    return <NotFound />;
  }

  return (
    <section className="flex flex-col gap-4">
      <div className="flex flex-wrap items-center gap-3">
        <h1 className="text-3xl font-extrabold tracking-tight">
          {show.data ? (
            <>
              <bdi>{show.data.host.displayName}</bdi>'s show
            </>
          ) : (
            "Show"
          )}
        </h1>
        {/* Reconnecting is routine (a deploy, an idle drop): said here, quietly. */}
        <span
          role="status"
          className="rounded-full bg-white/10 px-3 py-1 text-sm"
          data-testid="connection"
        >
          {live.connection}
        </span>
        <span className="rounded-full bg-white/10 px-3 py-1 text-sm" data-testid="sync-status">
          {STATUS[status]}
        </span>
        <span className="font-mono text-xs text-muted" data-testid="drift">
          {drift} ms
        </span>
      </div>

      {show.error && <LoadError error={show.error} onRetry={show.refetch} />}

      <div className="squircle relative aspect-video overflow-hidden rounded-[26px] bg-black">
        {/* biome-ignore lint/a11y/useMediaCaption: game clips have no captions. */}
        <video
          ref={video}
          src={src ?? undefined}
          playsInline
          preload="auto"
          className="h-full w-full object-contain"
          data-testid="show-video"
        />
        {nextSrc && (
          // Never shown (display: none keeps it out of the accessibility tree); it only
          // preloads.
          <video src={nextSrc} preload="auto" muted className="hidden" data-testid="next-video" />
        )}
        {status === "blocked" && (
          <div className="absolute inset-0 grid place-items-center bg-black/60">
            <Button
              variant="primary"
              size="lg"
              onClick={() => {
                engine.current?.sync.unblock();
                video.current?.play().catch(() => {});
              }}
            >
              Join with sound
            </Button>
          </div>
        )}
      </div>

      {isHost && send && (
        <div className="flex flex-wrap gap-2" data-testid="host-controls">
          {lineup.map((l) => (
            <Button
              key={l.clip.id}
              size="sm"
              onClick={() => send({ type: "load", clipId: l.clip.id })}
            >
              Load {l.clip.title}
            </Button>
          ))}
          <Button size="sm" variant="primary" onClick={() => send({ type: "play" })}>
            Play
          </Button>
          <Button size="sm" onClick={() => send({ type: "pause" })}>
            Pause
          </Button>
          <Button
            size="sm"
            onClick={() =>
              live.state &&
              send({
                type: "seek",
                positionMs: Math.max(0, targetMs(live.state, now()) - 10_000),
              })
            }
          >
            −10 s
          </Button>
          <Button
            size="sm"
            onClick={() =>
              live.state && send({ type: "seek", positionMs: targetMs(live.state, now()) + 10_000 })
            }
          >
            +10 s
          </Button>
        </div>
      )}
      {live.over && (
        <p className="text-sm text-soft" data-testid="show-over">
          The show's over.
        </p>
      )}
      {/* Closed for good: why, once (the server's reason). */}
      {live.connection === "closed" && !live.over && (
        <p role="alert" className="text-sm text-danger">
          {live.error ?? "The connection to the show closed."}
        </p>
      )}
    </section>
  );
}
