// The live sync (S5) with two browsers: a host and a friend, each in its own browser
// context, connected to a stand-in for the server's show hub that runs in the test. The
// hub's clock is 5 s ahead of the browsers', so the clock sync is tested too.
import type { BrowserContext, Page, WebSocketRoute } from "@playwright/test";
import { me, SHOW_ID, showClip, showClip2 } from "./fixtures";
import { cspViolations, open } from "./pages";
import { expect, test } from "./test";

const OFFSET_MS = 5_000;
const FRIEND = "00000000-0000-4000-8000-000000000002";

interface State {
  seq: number;
  clipId: string | null;
  playing: boolean;
  positionMs: number;
  atServerMs: number;
  rate: number;
  durationMs: number | null;
}

/** The server's hub, in miniature: one state, host-only changes, pings. */
class Hub {
  state: State = {
    seq: 0,
    clipId: null,
    playing: false,
    positionMs: 0,
    atServerMs: 0,
    rate: 1,
    durationMs: null,
  };
  sockets = new Map<string, WebSocketRoute>();
  /** Connections each person has opened so far. */
  connects = new Map<string, number>();
  /** Everything each person has sent, but the hellos and pings. */
  received: { userId: string; msg: Record<string, unknown> }[] = [];
  now = () => Date.now() + OFFSET_MS;

  positionAt(t: number) {
    const s = this.state;
    return s.positionMs + (s.playing ? Math.max(0, t - s.atServerMs) * s.rate : 0);
  }

  private broadcast(msg: unknown) {
    for (const ws of this.sockets.values()) ws.send(JSON.stringify(msg));
  }

  private change(f: (s: State) => void) {
    f(this.state);
    this.state.seq += 1;
    this.broadcast({ type: "state", state: this.state });
  }

  attach(ws: WebSocketRoute, userId: string) {
    this.connects.set(userId, (this.connects.get(userId) ?? 0) + 1);
    ws.onMessage((raw) => {
      const msg = JSON.parse(String(raw));
      const host = userId === me.id;
      if (msg.type !== "hello" && msg.type !== "ping") this.received.push({ userId, msg });
      switch (msg.type) {
        case "hello":
          this.sockets.set(userId, ws);
          ws.send(
            JSON.stringify({
              type: "welcome",
              userId,
              serverMs: this.now(),
              state: this.state,
              presence: { hostId: me.id, online: [...this.sockets.keys()], hostAwaySince: null },
            }),
          );
          break;
        case "ping":
          ws.send(JSON.stringify({ type: "pong", clientMs: msg.clientMs, serverMs: this.now() }));
          break;
        case "load":
          if (host)
            this.change((s) => {
              s.clipId = msg.clipId;
              s.durationMs = 40_000;
              s.positionMs = 0;
              s.playing = false;
              s.atServerMs = this.now();
            });
          break;
        case "play":
          if (host)
            this.change((s) => {
              s.positionMs = this.positionAt(this.now());
              s.playing = true;
              s.atServerMs = this.now() + 400;
            });
          break;
        case "pause":
          if (host)
            this.change((s) => {
              s.positionMs = this.positionAt(this.now());
              s.playing = false;
              s.atServerMs = this.now();
            });
          break;
        case "seek":
          if (host)
            this.change((s) => {
              s.positionMs = msg.positionMs;
              s.atServerMs = this.now();
            });
          break;
      }
    });
    ws.onClose(() => {
      if (this.sockets.get(userId) === ws) this.sockets.delete(userId);
    });
  }

  /** Says something to everyone in the room. */
  say(msg: unknown) {
    this.broadcast(msg);
  }

  /** Drops someone's connection, as a deploy would. */
  drop(userId: string) {
    this.sockets.get(userId)?.close();
    this.sockets.delete(userId);
  }

  /** The show ends: everyone hears so, then the server closes with "no such show". */
  end() {
    this.broadcast({ type: "showOver" });
    for (const ws of this.sockets.values())
      void ws.close({ code: 4004, reason: "the show is over" });
    this.sockets.clear();
  }
}

async function join(context: BrowserContext, hub: Hub, userId: string): Promise<Page> {
  const page = await context.newPage();
  await page.routeWebSocket(/\/api\/shows\/[^/]+\/live$/, (ws) => hub.attach(ws, userId));
  await open(page, `/shows/${SHOW_ID}`);
  await expect(page.getByTestId("connection")).toHaveText("open");
  return page;
}

async function videoSeconds(page: Page): Promise<number> {
  return page.getByTestId("show-video").evaluate((v: HTMLVideoElement) => v.currentTime);
}

async function startPlaying(page: Page) {
  // Autoplay with sound may need a click, like a real first visit. Wait until it either
  // plays in step or asks: right after Play it's still starting, and neither shows yet.
  // Closing the last few hundred ms is gentle (3 % faster), so it can take a while on a
  // busy machine.
  const join = page.getByRole("button", { name: "Join with sound" });
  const status = page.getByTestId("sync-status");
  await expect
    .poll(
      async () =>
        (await join.isVisible()) ||
        ((await status.textContent()) === "Synced" && !(await videoState(page)).paused),
      { timeout: 15_000 },
    )
    .toBe(true);
  if (await join.isVisible()) await join.click();
}

/** Host and friend playing the first clip, in step. */
async function playFirstClip(host: Page, friend: Page) {
  await host.getByRole("button", { name: `Load ${showClip.title}` }).click();
  await host.getByRole("button", { name: "Play", exact: true }).click();
  await startPlaying(host);
  await startPlaying(friend);
  await expect(host.getByTestId("sync-status")).toHaveText("Synced", { timeout: 15_000 });
  await expect(friend.getByTestId("sync-status")).toHaveText("Synced", { timeout: 15_000 });
}

const videoState = (page: Page) =>
  page.getByTestId("show-video").evaluate((v: HTMLVideoElement) => ({
    paused: v.paused,
    src: v.currentSrc,
    at: v.currentTime,
  }));

test("two browsers play the host's clip in step, catch up, and survive a reconnect", async ({
  browser,
}) => {
  test.setTimeout(60_000);
  const hub = new Hub();
  const host = await join(await browser.newContext(), hub, me.id);
  const friend = await join(await browser.newContext(), hub, FRIEND);

  // Only the host gets the controls.
  await expect(host.getByTestId("host-controls")).toBeVisible();
  await expect(friend.getByTestId("host-controls")).toHaveCount(0);

  await playFirstClip(host, friend);

  // Both where the hub says, and with each other (within the 0.3 s dead zone + slack).
  const expected = hub.positionAt(hub.now()) / 1000;
  const [a, b] = await Promise.all([videoSeconds(host), videoSeconds(friend)]);
  expect(Math.abs(a - b)).toBeLessThan(0.4);
  expect(Math.abs(a - expected)).toBeLessThan(0.4);

  // The friend's player jumps 3 s ahead: it jumps back.
  await friend.getByTestId("show-video").evaluate((v: HTMLVideoElement) => {
    v.currentTime += 3;
  });
  await expect(friend.getByTestId("sync-status")).toHaveText("Catching up");
  await expect(friend.getByTestId("sync-status")).toHaveText("Synced", { timeout: 6_000 });
  const drift = Number((await friend.getByTestId("drift").textContent())?.replace(" ms", ""));
  expect(Math.abs(drift)).toBeLessThan(400);

  // The connection drops (a deploy): it reconnects and stays in step.
  hub.drop(FRIEND);
  await expect(friend.getByTestId("connection")).toHaveText("reconnecting");
  await expect(friend.getByTestId("connection")).toHaveText("open", { timeout: 5_000 });
  await expect(friend.getByTestId("sync-status")).toHaveText("Synced", { timeout: 6_000 });
  const [c, d] = await Promise.all([videoSeconds(host), videoSeconds(friend)]);
  expect(Math.abs(c - d)).toBeLessThan(0.4);

  expect(await cspViolations(friend)).toEqual([]);
});

test("the next clip doesn't replay the old one while its link loads", async ({ browser }) => {
  test.setTimeout(60_000);
  const hub = new Hub();
  const host = await join(await browser.newContext(), hub, me.id);
  const friend = await join(await browser.newContext(), hub, FRIEND);
  // The friend's copy of the second clip's details is slow to arrive.
  let release = () => {};
  const slow = new Promise<void>((r) => {
    release = r;
  });
  await friend.route(`**/api/clips/${showClip2.id}`, async (route) => {
    await slow;
    await route.fallback();
  });
  await playFirstClip(host, friend);
  const first = await videoState(friend);
  expect(first.paused).toBe(false);

  await host.getByRole("button", { name: `Load ${showClip2.title}` }).click();
  await host.getByRole("button", { name: "Play", exact: true }).click();
  await expect.poll(async () => (await videoState(host)).src).toMatch(/clip=2$/);
  // The friend stops the old clip at once and doesn't start it over.
  await expect.poll(async () => (await videoState(friend)).paused).toBe(true);
  await friend.waitForTimeout(1_500);
  const waiting = await videoState(friend);
  expect(waiting.paused).toBe(true);
  expect(waiting.src).toBe(first.src);
  expect(waiting.at).toBeGreaterThanOrEqual(first.at);

  // The link arrives: the new clip plays, in step with the host.
  release();
  await expect.poll(async () => (await videoState(friend)).src).toMatch(/clip=2$/);
  await startPlaying(friend);
  await expect(friend.getByTestId("sync-status")).toHaveText("Synced", { timeout: 15_000 });
  const [a, b] = await Promise.all([videoSeconds(host), videoSeconds(friend)]);
  expect(Math.abs(a - b)).toBeLessThan(0.4);
});

test("when the show ends, the page says so and stops reconnecting", async ({ browser }) => {
  const hub = new Hub();
  const friend = await join(await browser.newContext(), hub, FRIEND);
  expect(hub.connects.get(FRIEND)).toBe(1);
  hub.end();
  await expect(friend.getByTestId("show-over")).toHaveText("The show's over.");
  await expect(friend.getByTestId("connection")).toHaveText("closed");
  // Longer than the first reconnect's backoff (1 s).
  await friend.waitForTimeout(2_500);
  expect(hub.connects.get(FRIEND)).toBe(1);
});

test("a routine reconnect is quiet; a refusal is said once", async ({ page }) => {
  let hellos = 0;
  await page.routeWebSocket(/\/api\/shows\/[^/]+\/live$/, (ws) => {
    ws.onMessage((raw) => {
      if (JSON.parse(String(raw)).type !== "hello") return;
      hellos += 1;
      // First an idle close (4008), as after a late hello; then not allowed (4003). The
      // server says why in an error message, then closes with the same reason.
      const [code, reason] = hellos === 1 ? [4008, "no hello within 5 s"] : [4003, "not allowed"];
      ws.send(JSON.stringify({ type: "error", message: reason }));
      void ws.close({ code, reason });
    });
  });
  await open(page, `/shows/${SHOW_ID}`);
  await expect(page.getByTestId("connection")).toHaveText("closed", { timeout: 5_000 });
  expect(hellos).toBe(2);
  await expect(page.getByRole("alert")).toHaveCount(1);
  await expect(page.getByRole("alert")).toHaveText("not allowed");
  await expect(page.getByText("no hello within 5 s")).toHaveCount(0);
});

test("the show page says when the show can't be loaded", async ({ page }) => {
  await page.routeWebSocket(/\/api\/shows\/[^/]+\/live$/, () => {});
  await open(page, "about:blank");
  let status = 500;
  await page.route(`**/api/shows/${SHOW_ID}`, (route) =>
    status === 200
      ? route.fallback()
      : route.fulfill({ status, json: { error: "server", message: "the database is asleep" } }),
  );
  await page.goto(`/shows/${SHOW_ID}`);
  const error = page.getByRole("alert").filter({ hasText: "the database is asleep" });
  await expect(error).toBeVisible();
  status = 200;
  await error.getByRole("button", { name: "Try again" }).click();
  await expect(page.getByRole("heading", { name: "Robin's show" })).toBeVisible();
  await expect(error).toBeHidden();

  status = 404;
  await page.goto(`/shows/${SHOW_ID}`);
  await expect(page.getByRole("heading", { name: "Nothing here." })).toBeVisible();
});

test("the host pauses and jumps, and the friend follows", async ({ browser }) => {
  test.setTimeout(60_000);
  const hub = new Hub();
  const host = await join(await browser.newContext(), hub, me.id);
  const friend = await join(await browser.newContext(), hub, FRIEND);
  await playFirstClip(host, friend);
  const sent = (type: string) =>
    hub.received.filter((r) => r.userId === me.id && r.msg.type === type).map((r) => r.msg);

  await host.getByRole("button", { name: "Pause" }).click();
  await expect.poll(async () => (await videoState(friend)).paused).toBe(true);
  expect(sent("pause")).toHaveLength(1);
  const paused = hub.state.positionMs;

  await host.getByRole("button", { name: "+10 s" }).click();
  await expect.poll(() => sent("seek")).toHaveLength(1);
  expect(Number(sent("seek")[0]?.positionMs)).toBeCloseTo(paused + 10_000, -2);
  await expect.poll(() => videoSeconds(friend)).toBeCloseTo((paused + 10_000) / 1000, 0);

  // Back past the start: from the start.
  await host.getByRole("button", { name: "−10 s" }).click();
  await host.getByRole("button", { name: "−10 s" }).click();
  await expect.poll(() => sent("seek").at(-1)?.positionMs).toBe(0);
  await expect.poll(() => videoSeconds(friend)).toBeLessThan(0.1);
  // The friend only watches.
  expect(hub.received.filter((r) => r.userId === FRIEND)).toEqual([]);
});

test("a click joins with sound when the browser won't autoplay", async ({ browser }) => {
  test.setTimeout(60_000);
  const hub = new Hub();
  const host = await join(await browser.newContext(), hub, me.id);
  const context = await browser.newContext();
  // Like a first visit: no sound until the page is clicked.
  await context.addInitScript(() => {
    const play = HTMLMediaElement.prototype.play;
    const w = window as unknown as { __clicked?: boolean };
    window.addEventListener("pointerdown", () => {
      w.__clicked = true;
    });
    HTMLMediaElement.prototype.play = function () {
      if (!w.__clicked) return Promise.reject(new DOMException("no sound yet", "NotAllowedError"));
      return play.call(this);
    };
  });
  const friend = await join(context, hub, FRIEND);
  await host.getByRole("button", { name: `Load ${showClip.title}` }).click();
  await host.getByRole("button", { name: "Play", exact: true }).click();
  const button = friend.getByRole("button", { name: "Join with sound" });
  await expect(friend.getByTestId("sync-status")).toHaveText("Click to join with sound", {
    timeout: 8_000,
  });
  // Asked once, not again and again, and nothing plays meanwhile.
  await friend.waitForTimeout(1_000);
  await expect(button).toBeVisible();
  expect((await videoState(friend)).paused).toBe(true);

  await button.click();
  await expect(button).toBeHidden();
  await expect.poll(async () => (await videoState(friend)).paused).toBe(false);
  // In step with the host (closing the last few hundred ms gently can take a while).
  await expect
    .poll(async () => Math.abs((await videoSeconds(host)) - (await videoSeconds(friend))), {
      timeout: 8_000,
    })
    .toBeLessThan(0.6);
  await expect(friend.getByTestId("sync-status")).toHaveText(/^(Catching up|Synced)$/);
});

test("who's here, show changes and refusals the connection survives", async ({ page }) => {
  const hub = new Hub();
  await page.routeWebSocket(/\/api\/shows\/[^/]+\/live$/, (ws) => hub.attach(ws, me.id));
  let loads = 0;
  page.on("request", (r) => {
    if (new URL(r.url()).pathname === `/api/shows/${SHOW_ID}`) loads += 1;
  });
  await open(page, `/shows/${SHOW_ID}`);
  await expect(page.getByTestId("connection")).toHaveText("open");
  await expect(page.getByTestId("host-controls")).toBeVisible();

  // The show's details changed (a clip added, say): it loads them again.
  await expect.poll(() => loads).toBeGreaterThan(0);
  const before = loads;
  hub.say({ type: "showChanged" });
  await expect.poll(() => loads).toBe(before + 1);

  // Someone else is the host now: the controls go.
  hub.say({
    type: "presence",
    presence: { hostId: FRIEND, online: [me.id, FRIEND], hostAwaySince: null },
  });
  await expect(page.getByTestId("host-controls")).toHaveCount(0);

  // A refusal is said; "slow down" isn't worth saying.
  hub.say({ type: "error", message: "slow down" });
  hub.say({ type: "error", message: "only while the show is live" });
  await expect(page.getByRole("alert")).toHaveText("only while the show is live");
  await expect(page.getByText("slow down")).toHaveCount(0);
  await expect(page.getByTestId("connection")).toHaveText("open");
});

test("leaving the show page closes the connection", async ({ browser }) => {
  const hub = new Hub();
  const friend = await join(await browser.newContext(), hub, FRIEND);
  expect(hub.sockets.has(FRIEND)).toBe(true);
  await friend.getByRole("navigation").getByRole("link", { name: "Archive" }).click();
  await expect(friend.getByRole("heading", { name: "Archive" })).toBeVisible();
  await expect.poll(() => hub.sockets.has(FRIEND)).toBe(false);
  // Gone for good: longer than the first reconnect's backoff, and no reconnect.
  await friend.waitForTimeout(1_500);
  expect(hub.connects.get(FRIEND)).toBe(1);
});
