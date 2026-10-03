// Keeping one video in step with the show (decision 34). The server holds the truth: at
// server time `atServerMs` the clip is at `positionMs`, moving at `rate` while playing.
// This browser works out where it should be and nudges its player there:
// - within `deadZoneMs`: leave it (correcting stops once back within `exitMs`),
// - further: speed up or slow down, harder the further off, between ±`minRate` and
//   ±`maxRate`,
// - beyond `seekAboveMs`: jump (slightly past the target, as seeking takes a moment),
//   then let it settle for `settleMs` before judging again; no second jump in that time
//   unless it's got further off since.
// Safari stalls on every playbackRate change, so it only ever jumps (`safariDeadZoneMs`).
// While the player is buffering it's left alone: nudging or jumping then only makes it
// buffer more. At the end of the clip it stays on the last frame: `play()` on an ended
// video starts it again from 0, so the engine never does that, and never jumps past the
// end (`endMarginMs`).

export interface LiveState {
  seq: number;
  clipId: string | null;
  playing: boolean;
  positionMs: number;
  atServerMs: number;
  rate: number;
  durationMs: number | null;
}

export const SYNC = {
  deadZoneMs: 300,
  exitMs: 100,
  seekAboveMs: 1000,
  maxRate: 0.1,
  /** The gentlest nudge while correcting, so the last few hundred ms don't take ages. */
  minRate: 0.03,
  /** Drift that earns the full ±maxRate nudge is maxRate × this (1 s here). */
  rateWindowMs: 10_000,
  seekLeadMs: 150,
  settleMs: 1500,
  safariDeadZoneMs: 500,
  /** How close a paused player must sit to its spot. */
  pausedToleranceMs: 80,
  /** Jumps land at least this far before the end of the media. */
  endMarginMs: 50,
};

export type Tuning = typeof SYNC;

/** Where the clip should be at server time `t`. */
export function targetMs(state: LiveState, t: number): number {
  const moving = state.playing ? Math.max(0, t - state.atServerMs) * state.rate : 0;
  const p = state.positionMs + moving;
  return state.durationMs == null ? p : Math.min(p, state.durationMs);
}

export type Correction =
  | { kind: "none" }
  | { kind: "rate"; rate: number }
  | { kind: "seek"; toMs: number };

/** What to do about `driftMs` (positive: this player is ahead). */
export function correction(
  driftMs: number,
  targetPositionMs: number,
  opts: { correcting: boolean; safari: boolean },
  tuning: Tuning = SYNC,
): Correction {
  const off = Math.abs(driftMs);
  if (opts.safari) {
    return off > tuning.safariDeadZoneMs
      ? { kind: "seek", toMs: targetPositionMs + tuning.seekLeadMs }
      : { kind: "none" };
  }
  if (off > tuning.seekAboveMs) {
    return { kind: "seek", toMs: targetPositionMs + tuning.seekLeadMs };
  }
  const threshold = opts.correcting ? tuning.exitMs : tuning.deadZoneMs;
  if (off <= threshold) return { kind: "none" };
  const size = Math.min(tuning.maxRate, Math.max(tuning.minRate, off / tuning.rateWindowMs));
  return { kind: "rate", rate: 1 - Math.sign(driftMs) * size };
}

/** The part of a <video> the engine drives (a fake one in tests). */
export interface Player {
  currentTime: number;
  playbackRate: number;
  readonly paused: boolean;
  readonly ended: boolean;
  /** Seconds; NaN until the media's metadata is in. */
  readonly duration: number;
  /** HTMLMediaElement.readyState: below HAVE_FUTURE_DATA it can't play on yet. */
  readonly readyState: number;
  play(): Promise<void>;
  pause(): void;
}

const HAVE_FUTURE_DATA = 3;

export type SyncStatus =
  /** On time. */
  | "synced"
  /** Behind or ahead and correcting: "Catching up". */
  | "catchingUp"
  /** Waiting for a scheduled start. */
  | "starting"
  /** The browser refused to play (sound needs a click first). */
  | "blocked"
  | "idle";

export function isSafari(ua = typeof navigator === "undefined" ? "" : navigator.userAgent) {
  return /Safari\//.test(ua) && !/Chrome\/|Chromium\/|Edg\//.test(ua);
}

/** Drives one player to a live state. Call `step` often (every 250 ms and on
 *  timeupdate); it never sends anything back, so the player's own events can't echo. */
export class SyncEngine {
  private correcting = false;
  private settleUntil = 0;
  /** How far off it was when it last jumped; null after a start (nothing to compare). */
  private lastSeekOffMs: number | null = null;
  private waiting = false;
  status: SyncStatus = "idle";
  /** Last measured drift, for display and tests. */
  driftMs = 0;

  constructor(
    private readonly player: Player,
    private readonly opts: { safari?: boolean; tuning?: Tuning } = {},
  ) {}

  /** `serverNow`: the server's time now. `localNow`: performance.now(), for settling. */
  step(state: LiveState | null, serverNow: number, localNow = performance.now()): SyncStatus {
    const p = this.player;
    const tuning = this.opts.tuning ?? SYNC;
    if (!state?.clipId) {
      if (!p.paused) p.pause();
      return this.set("idle");
    }
    // The clip ends at the server's length or the media's, whichever comes first.
    const media = Number.isFinite(p.duration) && p.duration > 0 ? p.duration * 1000 : null;
    const endMs =
      media != null && state.durationMs != null
        ? Math.min(media, state.durationMs)
        : (media ?? state.durationMs);
    const target = Math.min(targetMs(state, serverNow), endMs ?? Number.POSITIVE_INFINITY);
    const atMs = p.currentTime * 1000;
    this.driftMs = atMs - target;
    const seek = (toMs: number) => {
      const to = endMs == null ? toMs : Math.min(toMs, endMs - tuning.endMarginMs);
      p.currentTime = Math.max(0, to) / 1000;
    };

    // Paused, or waiting for a scheduled start: hold still on the spot.
    if (!state.playing || serverNow < state.atServerMs) {
      if (!p.paused) p.pause();
      if (p.playbackRate !== 1) p.playbackRate = 1;
      if (Math.abs(this.driftMs) > tuning.pausedToleranceMs) seek(target);
      this.correcting = false;
      return this.set(state.playing ? "starting" : "synced");
    }
    // Over: stay on the last frame.
    if (endMs != null && target >= endMs) {
      if (!p.paused) p.pause();
      if (p.playbackRate !== 1) p.playbackRate = 1;
      this.correcting = false;
      return this.set("synced");
    }
    if (p.paused) {
      // Refused once: wait for a click (`unblock`) instead of asking again and again.
      if (this.status === "blocked") return "blocked";
      seek(target);
      // Ended, and the show went back: play on a later step, once the jump has taken it
      // off the end, or play() would start it from 0.
      if (p.ended) return this.set("catchingUp");
      p.play().catch((e: unknown) => {
        // Only the autoplay policy blocks. An AbortError is a pause or a new source
        // cutting the play short, and the next step tries again.
        if ((e as { name?: string } | null)?.name === "NotAllowedError") this.set("blocked");
      });
      this.settleUntil = localNow + tuning.settleMs;
      this.lastSeekOffMs = null;
      // A refusal arrives later, through the catch above.
      return this.set("catchingUp");
    }
    // Buffering: it catches up by itself once it plays on.
    if (this.waiting || p.readyState < HAVE_FUTURE_DATA) return this.set("catchingUp");

    const settling = localNow < this.settleUntil;
    const off = Math.abs(this.driftMs);
    const fix = correction(
      this.driftMs,
      target,
      { correcting: this.correcting, safari: this.opts.safari ?? false },
      tuning,
    );
    if (fix.kind === "seek") {
      // One jump at a time, unless it's further off than before the last one (the host
      // jumped too, or the player did).
      if (settling && !(this.lastSeekOffMs != null && off > this.lastSeekOffMs)) {
        return this.status;
      }
      p.playbackRate = 1;
      seek(fix.toMs);
      this.settleUntil = localNow + tuning.settleMs;
      this.lastSeekOffMs = off;
      this.correcting = true;
      return this.set("catchingUp");
    }
    if (settling) return this.status;
    if (fix.kind === "rate") {
      p.playbackRate = fix.rate;
      this.correcting = true;
      return this.set("catchingUp");
    }
    if (p.playbackRate !== 1) p.playbackRate = 1;
    this.correcting = false;
    return this.set("synced");
  }

  /** The player's 'waiting' (true) and 'playing' (false) events: buffering in between. */
  setWaiting(waiting: boolean) {
    this.waiting = waiting;
  }

  /** After a click: the browser lets it play with sound now. */
  unblock() {
    if (this.status === "blocked") this.status = "idle";
  }

  private set(status: SyncStatus): SyncStatus {
    this.status = status;
    return status;
  }
}
