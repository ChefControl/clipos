// The show's live connection (S5): a websocket to /api/shows/{id}/live. Says hello with
// the access token, keeps the server clock in sync with pings (5 at the start, then every
// 20 s and when the tab comes back), and reconnects with backoff when it drops, e.g.
// during a deploy (the server resumes the show from its saved state).
//
// The server ends a connection with an `error` message and then a close code, the same
// text as the reason: 4001 the token is bad or expired, or no hello came first (one more
// try with a fresh token), 4003 not allowed and 4004 no such show or it's over (stop);
// anything else, e.g. 4008 idle, 1011 a server error or 1006 no close frame, reconnects
// as usual. When a show ends it says `showOver` first, then closes with 4004. Other
// `error` messages ("slow down", "only while the show is live") aren't fatal: they're
// passed on like any event, after a short wait, since an `error` the close follows was
// only that close's reason (said once, with `closed`, or not at all for a reconnect).
// Anything that isn't a message (not JSON, no type, an error with no text) is ignored.
// The server drops a connection that's been silent for 75 s, and this side drops one
// whose pings go unanswered for 10 s.

import { ServerClock } from "./clock";
import type { LiveState } from "./sync";

export interface Presence {
  hostId: string;
  online: string[];
  hostAwaySince: number | null;
}

export type ServerMsg =
  | { type: "welcome"; userId: string; serverMs: number; state: LiveState; presence: Presence }
  | { type: "state"; state: LiveState }
  | { type: "pong"; clientMs: number; serverMs: number }
  | { type: "presence"; presence: Presence }
  | { type: "reaction"; userId: string; clipId: string; emoji: string; atMs: number }
  | { type: "replayRequest"; userId: string }
  | { type: "showChanged" }
  | { type: "showOver" }
  | { type: "error"; message: string };

export type ClientMsg =
  | { type: "load"; clipId: string; startAt?: number }
  | { type: "play" }
  | { type: "pause" }
  | { type: "seek"; positionMs: number }
  | { type: "react"; clipId: string; emoji: string; atMs: number }
  | { type: "ready"; ready: boolean }
  | { type: "replayRequest" }
  | { type: "takeOver" };

/** `closed`: for good (the show is over, or the server won't let this browser in). */
export type Connection = "connecting" | "open" | "reconnecting" | "closed";

/** The server's close codes for connections it won't take back as they are. */
export const CLOSE = {
  badToken: 4001,
  notAllowed: 4003,
  noShow: 4004,
  idle: 4008,
} as const;

const PING_BURST = 5;
const PING_GAP_MS = 200;
/** Well inside the server's 75 s idle limit. */
const PING_EVERY_MS = 20_000;
/** No word from the server this long after a ping: the connection is dead. */
const SILENCE_MS = 10_000;
const BACKOFF_MS = [1_000, 2_000, 4_000, 8_000, 15_000];
/** How long an `error` waits for a close that would make it that close's reason. */
const ERROR_GRACE_MS = 500;

export function liveUrl(showId: string, origin = window.location.origin): string {
  return `${origin.replace(/^http/, "ws")}/api/shows/${showId}/live`;
}

/** A server message, or null (logged) for anything else on the socket. */
export function parseServerMsg(data: unknown): ServerMsg | null {
  let msg: { type?: unknown; message?: unknown } | null = null;
  try {
    msg = JSON.parse(String(data));
  } catch {
    msg = null;
  }
  const ok =
    typeof msg?.type === "string" &&
    (msg.type !== "error" || (typeof msg.message === "string" && msg.message !== ""));
  if (!ok) {
    console.debug("show: ignoring a message that isn't one", data);
    return null;
  }
  return msg as ServerMsg;
}

export class LiveClient {
  readonly clock = new ServerClock();
  private ws: WebSocket | null = null;
  private stopped = false;
  private attempt = 0;
  /** The next connection asks for a fresh token (after a 4001). */
  private freshToken = false;
  private timers: ReturnType<typeof setTimeout>[] = [];
  private every: ReturnType<typeof setInterval> | null = null;
  private silence: ReturnType<typeof setTimeout> | null = null;
  /** An `error` waiting to see whether a close follows it. */
  private pendingError: { msg: ServerMsg; timer: ReturnType<typeof setTimeout> } | null = null;
  private seq = -1;

  constructor(
    private readonly opts: {
      url: string;
      /** `fresh`: skip any cached token (the server refused the last one). */
      token: (fresh: boolean) => Promise<string>;
      onMessage: (msg: ServerMsg) => void;
      /** `reason`: why the server closed it for good (with `closed`). */
      onConnection: (c: Connection, reason?: string) => void;
    },
  ) {}

  start() {
    this.stopped = false;
    document.addEventListener("visibilitychange", this.onVisible);
    void this.connect();
  }

  stop() {
    this.stopped = true;
    document.removeEventListener("visibilitychange", this.onVisible);
    this.clearTimers();
    this.ws?.close();
    this.ws = null;
  }

  send(msg: ClientMsg) {
    if (this.ws?.readyState === WebSocket.OPEN) this.ws.send(JSON.stringify(msg));
  }

  private onVisible = () => {
    if (document.visibilityState !== "visible") return;
    // Back from a sleep, the old samples are wrong (checkSleep drops them).
    this.clock.checkSleep();
    this.pingBurst();
  };

  private async connect() {
    this.opts.onConnection(this.attempt === 0 ? "connecting" : "reconnecting");
    let token: string;
    try {
      token = await this.opts.token(this.freshToken);
    } catch {
      return this.retry();
    }
    if (this.stopped) return;
    const ws = new WebSocket(this.opts.url);
    this.ws = ws;
    ws.onopen = () => ws.send(JSON.stringify({ type: "hello", token }));
    ws.onmessage = (e) => {
      if (this.ws !== ws) return;
      const msg = parseServerMsg(e.data);
      if (msg) this.receive(msg);
    };
    ws.onclose = (e) => {
      if (this.ws === ws) this.closed(e.code, e.reason);
    };
  }

  private closed(code: number, reason: string) {
    this.ws = null;
    // The error just before was this close's reason.
    this.dropError();
    switch (code) {
      case CLOSE.badToken:
        // Once more with a fresh token; refused again, give up.
        if (this.freshToken) return this.end(reason);
        this.freshToken = true;
        return this.retry();
      case CLOSE.notAllowed:
      case CLOSE.noShow:
        return this.end(reason);
      default:
        // Idle (4008), a server error (1011), a deploy, a dropped network: come back.
        return this.retry();
    }
  }

  private retry() {
    this.clearTimers();
    if (this.stopped) return;
    this.opts.onConnection("reconnecting");
    const delay = BACKOFF_MS[Math.min(this.attempt, BACKOFF_MS.length - 1)] ?? 15_000;
    this.attempt += 1;
    this.timers.push(setTimeout(() => void this.connect(), delay));
  }

  /** For good: no more reconnecting. */
  private end(reason?: string) {
    this.stop();
    this.opts.onConnection("closed", reason || undefined);
  }

  private receive(msg: ServerMsg) {
    // Any word from the server means the connection is alive.
    this.clearSilence();
    // Something came after the last error, so no close followed it: pass it on.
    this.flushError();
    switch (msg.type) {
      case "welcome":
        this.attempt = 0;
        this.freshToken = false;
        this.seq = msg.state.seq;
        // A new connection measures the clock afresh.
        this.clock.reset();
        this.opts.onConnection("open");
        this.pingBurst();
        this.clearInterval();
        this.every = setInterval(() => this.ping(), PING_EVERY_MS);
        break;
      case "state":
        // Out-of-order or replayed states are ignored.
        if (msg.state.seq <= this.seq) return;
        this.seq = msg.state.seq;
        break;
      case "pong":
        // Null: the machine slept and the clock started over; measure it again.
        if (!this.clock.add(msg.clientMs, msg.serverMs, performance.now())) this.pingBurst();
        return;
      case "showOver":
        // The server closes with 4004 next; don't come back.
        this.opts.onMessage(msg);
        return this.end();
      case "error":
        this.pendingError = {
          msg,
          timer: setTimeout(() => this.flushError(), ERROR_GRACE_MS),
        };
        return;
    }
    this.opts.onMessage(msg);
  }

  private ping() {
    const ws = this.ws;
    if (ws?.readyState !== WebSocket.OPEN) return;
    ws.send(
      JSON.stringify({
        type: "ping",
        clientMs: performance.now(),
        // The latest round trip, not the best: the server plans starts around the
        // slowest connection as it is now.
        rttMs: this.clock.latest?.rttMs ?? null,
      }),
    );
    // Unanswered: drop it and reconnect (its close may take a long time to come).
    this.silence ??= setTimeout(() => {
      this.silence = null;
      if (this.ws !== ws) return;
      this.ws = null;
      ws.close();
      this.retry();
    }, SILENCE_MS);
  }

  private pingBurst() {
    for (let i = 0; i < PING_BURST; i++) {
      this.timers.push(setTimeout(() => this.ping(), i * PING_GAP_MS));
    }
  }

  private flushError() {
    const pending = this.pendingError;
    if (!pending) return;
    this.dropError();
    this.opts.onMessage(pending.msg);
  }

  private dropError() {
    if (this.pendingError) clearTimeout(this.pendingError.timer);
    this.pendingError = null;
  }

  private clearSilence() {
    if (this.silence) clearTimeout(this.silence);
    this.silence = null;
  }

  private clearInterval() {
    if (this.every) clearInterval(this.every);
    this.every = null;
  }

  private clearTimers() {
    for (const t of this.timers) clearTimeout(t);
    this.timers = [];
    this.clearInterval();
    this.clearSilence();
    this.dropError();
  }
}
