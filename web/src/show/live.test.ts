import { afterEach, beforeEach, describe, expect, it, vi } from "vitest";
import { type Connection, LiveClient, liveUrl, type ServerMsg } from "./live";

/** A websocket the test plays the server for. */
class FakeSocket {
  static readonly OPEN = 1;
  static all: FakeSocket[] = [];
  readyState = 0;
  sent: Record<string, unknown>[] = [];
  onopen: (() => void) | null = null;
  onmessage: ((e: { data: string }) => void) | null = null;
  onclose: ((e: { code: number; reason: string }) => void) | null = null;
  closedByClient = false;

  constructor(readonly url: string) {
    FakeSocket.all.push(this);
  }
  send(data: string) {
    this.sent.push(JSON.parse(data));
  }
  close() {
    this.closedByClient = true;
    this.readyState = 3;
  }

  // The server's side.
  open() {
    this.readyState = 1;
    this.onopen?.();
  }
  msg(m: Record<string, unknown>) {
    this.onmessage?.({ data: JSON.stringify(m) });
  }
  welcome() {
    this.msg({
      type: "welcome",
      userId: "u",
      serverMs: 0,
      state: { seq: 1 },
      presence: { hostId: "u", online: [], hostAwaySince: null },
    });
  }
  serverClose(code: number, reason = "") {
    this.readyState = 3;
    this.onclose?.({ code, reason });
  }
  pings() {
    return this.sent.filter((m) => m.type === "ping");
  }
}

let tokens: boolean[];
let connections: Connection[];
let reasons: (string | undefined)[];
let messages: ServerMsg[];
let client: LiveClient;
/** The page, as far as the client sees it: whether it's visible, and its listener. */
let doc: { visibilityState: string; onVisible: (() => void) | null };

const last = () => FakeSocket.all.at(-1) as FakeSocket;
/** Lets the token promise resolve and the socket get made. */
const settle = () => vi.advanceTimersByTimeAsync(0);

beforeEach(async () => {
  vi.useFakeTimers({ toFake: ["setTimeout", "clearTimeout", "setInterval", "clearInterval"] });
  FakeSocket.all = [];
  vi.stubGlobal("WebSocket", FakeSocket);
  doc = { visibilityState: "visible", onVisible: null };
  vi.stubGlobal("document", {
    get visibilityState() {
      return doc.visibilityState;
    },
    addEventListener: (type: string, f: () => void) => {
      if (type === "visibilitychange") doc.onVisible = f;
    },
    removeEventListener: (type: string, f: () => void) => {
      if (type === "visibilitychange" && doc.onVisible === f) doc.onVisible = null;
    },
  });
  tokens = [];
  connections = [];
  reasons = [];
  messages = [];
  client = new LiveClient({
    url: "ws://test/api/shows/s/live",
    token: async (fresh) => {
      tokens.push(fresh);
      return "t";
    },
    onMessage: (m) => messages.push(m),
    onConnection: (c, reason) => {
      connections.push(c);
      reasons.push(reason);
    },
  });
  client.start();
  await settle();
  last().open();
  last().welcome();
});

afterEach(() => {
  client.stop();
  vi.useRealTimers();
  vi.unstubAllGlobals();
});

describe("the live connection", () => {
  it("says hello with the token, then pings", async () => {
    expect(last().sent[0]).toEqual({ type: "hello", token: "t" });
    expect(connections.at(-1)).toBe("open");
    await vi.advanceTimersByTimeAsync(1_000);
    expect(last().pings()).toHaveLength(5);
  });

  it("pings at least every 25 s, inside the server's idle limit", async () => {
    const ws = last();
    await vi.advanceTimersByTimeAsync(1_000);
    // Answer everything so the silence check stays quiet.
    const answered = ws.pings().length;
    for (const p of ws.pings()) ws.msg({ type: "pong", clientMs: p.clientMs, serverMs: 0 });
    await vi.advanceTimersByTimeAsync(24_000);
    expect(ws.pings().length).toBeGreaterThan(answered);
  });

  it("reconnects after an idle close (4008), a server error (1011) or a drop (1006)", async () => {
    let sockets = 1;
    for (const code of [4008, 1011, 1006]) {
      last().serverClose(code, "bye");
      expect(connections.at(-1)).toBe("reconnecting");
      await vi.advanceTimersByTimeAsync(1_000);
      sockets += 1;
      expect(FakeSocket.all).toHaveLength(sockets);
      last().open();
      last().welcome();
    }
  });

  it("survives errors that aren't fatal and passes them on", async () => {
    last().msg({ type: "error", message: "slow down" });
    last().msg({ type: "error", message: "only while the show is live" });
    // Each once it's clear no close came with it.
    await vi.advanceTimersByTimeAsync(500);
    expect(messages.slice(-2)).toEqual([
      { type: "error", message: "slow down" },
      { type: "error", message: "only while the show is live" },
    ]);
    expect(connections.at(-1)).toBe("open");
    expect(last().closedByClient).toBe(false);
  });

  it("tries a refused token (4001) once more with a fresh one, then stops", async () => {
    last().serverClose(4001);
    await vi.advanceTimersByTimeAsync(1_000);
    expect(tokens).toEqual([false, true]);
    expect(FakeSocket.all).toHaveLength(2);
    last().open();
    last().serverClose(4001, "token expired");
    expect(connections.at(-1)).toBe("closed");
    expect(reasons.at(-1)).toBe("token expired");
    await vi.advanceTimersByTimeAsync(60_000);
    expect(FakeSocket.all).toHaveLength(2);
  });

  it("a fresh token that works resets the one retry", async () => {
    last().serverClose(4001);
    await vi.advanceTimersByTimeAsync(1_000);
    last().open();
    last().welcome();
    last().serverClose(4001);
    await vi.advanceTimersByTimeAsync(1_000);
    expect(FakeSocket.all).toHaveLength(3);
  });

  it.each([4003, 4004])("stops for good on %i, and says why once", async (code) => {
    last().msg({ type: "error", message: "nope" });
    last().serverClose(code, "nope");
    expect(connections.at(-1)).toBe("closed");
    expect(reasons.at(-1)).toBe("nope");
    // The error before the close was its reason, not an event of its own.
    await vi.advanceTimersByTimeAsync(60_000);
    expect(messages.filter((m) => m.type === "error")).toEqual([]);
    expect(FakeSocket.all).toHaveLength(1);
  });

  it("a routine reconnect (4008) passes no error on", async () => {
    last().msg({ type: "error", message: "no hello within 5 s" });
    last().serverClose(4008, "no hello within 5 s");
    expect(connections.at(-1)).toBe("reconnecting");
    await vi.advanceTimersByTimeAsync(1_000);
    expect(FakeSocket.all).toHaveLength(2);
    expect(messages.filter((m) => m.type === "error")).toEqual([]);
  });

  it("ignores what isn't a message: bad JSON, no type, an error with no text", async () => {
    const debug = vi.spyOn(console, "debug").mockImplementation(() => {});
    const before = messages.length;
    const ws = last();
    ws.onmessage?.({ data: "this is not json" });
    ws.onmessage?.({ data: "null" });
    ws.msg({ confetti: true });
    ws.msg({ type: "error" });
    ws.msg({ type: "error", message: "" });
    await vi.advanceTimersByTimeAsync(1_000);
    expect(messages).toHaveLength(before);
    expect(debug).toHaveBeenCalledTimes(5);
    expect(connections.at(-1)).toBe("open");
    expect(ws.closedByClient).toBe(false);
    // And it still works.
    ws.msg({ type: "state", state: { seq: 2 } });
    expect(messages.at(-1)).toEqual({ type: "state", state: { seq: 2 } });
  });

  it("an error with a message after it is passed on at once, in order", async () => {
    last().msg({ type: "error", message: "only while the show is live" });
    last().msg({ type: "state", state: { seq: 2 } });
    expect(messages.slice(-2)).toEqual([
      { type: "error", message: "only while the show is live" },
      { type: "state", state: { seq: 2 } },
    ]);
  });

  it("passes showOver on and doesn't come back", async () => {
    const ws = last();
    ws.msg({ type: "showOver" });
    expect(messages.at(-1)).toEqual({ type: "showOver" });
    expect(connections.at(-1)).toBe("closed");
    ws.serverClose(4004);
    await vi.advanceTimersByTimeAsync(60_000);
    expect(FakeSocket.all).toHaveLength(1);
    // No pings into the void either.
    const pings = ws.pings().length;
    await vi.advanceTimersByTimeAsync(60_000);
    expect(ws.pings()).toHaveLength(pings);
  });

  it("drops a connection whose pings go unanswered and reconnects", async () => {
    const ws = last();
    // The burst goes out; nothing comes back.
    await vi.advanceTimersByTimeAsync(9_000);
    expect(ws.closedByClient).toBe(false);
    await vi.advanceTimersByTimeAsync(1_100);
    expect(ws.closedByClient).toBe(true);
    expect(connections.at(-1)).toBe("reconnecting");
    await vi.advanceTimersByTimeAsync(1_000);
    expect(FakeSocket.all).toHaveLength(2);
  });

  it("any message counts as an answer", async () => {
    const ws = last();
    await vi.advanceTimersByTimeAsync(5_000);
    ws.msg({ type: "presence", presence: { hostId: "u", online: [], hostAwaySince: null } });
    await vi.advanceTimersByTimeAsync(9_000);
    expect(ws.closedByClient).toBe(false);
  });

  it("reports the latest round trip, not the fastest", async () => {
    const ws = last();
    // The burst: answer the first quickly (10 ms) and the second slowly (300 ms).
    await vi.advanceTimersByTimeAsync(0);
    const [first] = ws.pings();
    vi.spyOn(performance, "now").mockReturnValue(Number(first?.clientMs) + 10);
    ws.msg({ type: "pong", clientMs: first?.clientMs, serverMs: 0 });
    await vi.advanceTimersByTimeAsync(200);
    const second = ws.pings()[1];
    expect(second?.rttMs).toBeCloseTo(10);
    vi.spyOn(performance, "now").mockReturnValue(Number(second?.clientMs) + 300);
    ws.msg({ type: "pong", clientMs: second?.clientMs, serverMs: 0 });
    await vi.advanceTimersByTimeAsync(200);
    expect(ws.pings()[2]?.rttMs).toBeCloseTo(300);
    expect(client.clock.best?.rttMs).toBeCloseTo(10);
  });

  it("measures the clock afresh on every welcome", async () => {
    const ws = last();
    await vi.advanceTimersByTimeAsync(0);
    const [first] = ws.pings();
    ws.msg({ type: "pong", clientMs: first?.clientMs, serverMs: 1_000 });
    expect(client.clock.synced).toBe(true);
    ws.welcome();
    expect(client.clock.synced).toBe(false);
  });

  it("measures the clock again when the tab comes back, not while it's hidden", async () => {
    const ws = last();
    await vi.advanceTimersByTimeAsync(1_000);
    for (const p of ws.pings()) ws.msg({ type: "pong", clientMs: p.clientMs, serverMs: 0 });
    const before = ws.pings().length;
    const checkSleep = vi.spyOn(client.clock, "checkSleep");

    doc.visibilityState = "hidden";
    doc.onVisible?.();
    await vi.advanceTimersByTimeAsync(1_000);
    expect(ws.pings()).toHaveLength(before);
    expect(checkSleep).not.toHaveBeenCalled();

    doc.visibilityState = "visible";
    doc.onVisible?.();
    await vi.advanceTimersByTimeAsync(1_000);
    expect(checkSleep).toHaveBeenCalledOnce();
    expect(ws.pings()).toHaveLength(before + 5);

    // Stopped, it stops listening.
    client.stop();
    expect(doc.onVisible).toBeNull();
  });

  it("measures again when a pong says the machine slept", async () => {
    const ws = last();
    await vi.advanceTimersByTimeAsync(1_000);
    const before = ws.pings().length;
    vi.spyOn(client.clock, "add").mockReturnValue(null);
    ws.msg({ type: "pong", clientMs: ws.pings()[0]?.clientMs, serverMs: 0 });
    await vi.advanceTimersByTimeAsync(1_000);
    expect(ws.pings()).toHaveLength(before + 5);
    // A pong is the client's business, not an event.
    expect(messages.filter((m) => m.type === "pong")).toEqual([]);
  });

  it("passes states on in order, and drops stale or replayed ones", () => {
    const ws = last();
    ws.msg({ type: "state", state: { seq: 3 } });
    ws.msg({ type: "state", state: { seq: 2 } });
    ws.msg({ type: "state", state: { seq: 3 } });
    ws.msg({ type: "state", state: { seq: 4 } });
    const seqs = messages
      .filter((m) => m.type === "state")
      .map((m) => (m as { state: { seq: number } }).state.seq);
    expect(seqs).toEqual([3, 4]);
  });

  it("sends only on an open connection", () => {
    const ws = last();
    client.send({ type: "play" });
    expect(ws.sent.at(-1)).toEqual({ type: "play" });
    const sent = ws.sent.length;
    ws.readyState = 3;
    client.send({ type: "pause" });
    expect(ws.sent).toHaveLength(sent);
    ws.serverClose(1006);
    client.send({ type: "pause" });
    expect(ws.sent).toHaveLength(sent);
  });

  it("ignores a dropped socket that speaks up late", async () => {
    const old = last();
    // Unanswered pings: it drops the socket and makes a new one.
    await vi.advanceTimersByTimeAsync(11_000);
    expect(old.closedByClient).toBe(true);
    const fresh = last();
    expect(fresh).not.toBe(old);
    fresh.open();
    fresh.welcome();
    const seen = messages.length;
    old.msg({ type: "state", state: { seq: 9 } });
    old.serverClose(4003, "late");
    expect(messages).toHaveLength(seen);
    expect(connections.at(-1)).toBe("open");
  });

  it("backs off further each time the token can't be had, up to 15 s", async () => {
    client.stop();
    FakeSocket.all = [];
    let fail = true;
    let tries = 0;
    const states: Connection[] = [];
    const c = new LiveClient({
      url: "ws://test/api/shows/s/live",
      token: async () => {
        tries += 1;
        if (fail) throw new Error("offline");
        return "t";
      },
      onMessage: () => {},
      onConnection: (s) => states.push(s),
    });
    c.start();
    await settle();
    expect(tries).toBe(1);
    expect(states).toEqual(["connecting", "reconnecting"]);
    // Tries again after 1, 2, 4, 8, 15 and 15 s.
    for (const gap of [1_000, 2_000, 4_000, 8_000, 15_000, 15_000]) {
      const before = tries;
      await vi.advanceTimersByTimeAsync(gap - 1);
      expect(tries).toBe(before);
      await vi.advanceTimersByTimeAsync(1);
      expect(tries).toBe(before + 1);
    }
    expect(states.at(-1)).toBe("reconnecting");
    expect(FakeSocket.all).toHaveLength(0);
    // The network's back.
    fail = false;
    await vi.advanceTimersByTimeAsync(15_000);
    expect(FakeSocket.all).toHaveLength(1);
    c.stop();
  });

  it("stopped while it waits for a token, it doesn't connect", async () => {
    client.stop();
    FakeSocket.all = [];
    let give: (t: string) => void = () => {};
    const c = new LiveClient({
      url: "ws://test/api/shows/s/live",
      token: () =>
        new Promise<string>((r) => {
          give = r;
        }),
      onMessage: () => {},
      onConnection: () => {},
    });
    c.start();
    c.stop();
    give("t");
    await settle();
    expect(FakeSocket.all).toHaveLength(0);
  });

  it("turns the page's address into the show's websocket address", () => {
    expect(liveUrl("s1", "http://localhost:5173")).toBe("ws://localhost:5173/api/shows/s1/live");
    expect(liveUrl("s1", "https://clips.example")).toBe("wss://clips.example/api/shows/s1/live");
  });
});
