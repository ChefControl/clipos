import { useEffect, useRef, useState } from "react";
import type { LiveClient } from "./live";
import { isSafari, type LiveState, SyncEngine, type SyncStatus } from "./sync";

export interface Playback {
  /** The `<video>` the engine drives. */
  video: React.RefObject<HTMLVideoElement | null>;
  status: SyncStatus;
  /** How far the player is off the room, last time it looked. */
  driftMs: number;
  /** After a click: try playing with sound again. */
  unblock: () => void;
}

/** Keeps a `<video>` on the room's live state: a SyncEngine per clip, stepped four times a
 *  second and on every time update, and the screen drawn again at each step. `src` is the
 *  clip's playback link once it's known. */
export function usePlayback(
  client: LiveClient | null,
  state: LiveState | null,
  src: string | null,
): Playback {
  const video = useRef<HTMLVideoElement>(null);
  // The engine drives one clip: the one it was made for.
  const engine = useRef<{ clipId: string; sync: SyncEngine } | null>(null);
  const [status, setStatus] = useState<SyncStatus>("idle");
  const [driftMs, setDrift] = useState(0);
  // Each step draws the screen again, so what's worked out from the clock (where the
  // room is, the count to a start) keeps moving.
  const [, setFrame] = useState(0);
  const clipId = state?.clipId ?? null;

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

  const stateRef = useRef(state);
  stateRef.current = state;
  const clientRef = useRef(client);
  clientRef.current = client;
  useEffect(() => {
    const step = () => {
      const c = clientRef.current;
      const s = stateRef.current;
      if (!c?.clock.synced) return;
      if (s?.clipId) setFrame((f) => f + 1);
      const e = engine.current;
      if (!e || e.clipId !== s?.clipId) return;
      setStatus(e.sync.step(s, c.clock.now()));
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

  return {
    video,
    status,
    driftMs,
    unblock: () => {
      engine.current?.sync.unblock();
      video.current?.play().catch(() => {});
    },
  };
}
