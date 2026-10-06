// The live sync (S5) with two browsers: a host and a friend, each in its own browser
// context, connected to a stand-in for the server's show hub that runs in the test. The
// hub's clock is 5 s ahead of the browsers', so the clock sync is tested too.
import type { BrowserContext, Page, WebSocketRoute } from "@playwright/test";
import { clips, me, SHOW_ID, show, showClip, showClip2 } from "./fixtures";
import { layoutProblems } from "./layout";
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

/** The server's hub, in miniature: one state anyone in it changes, pings. */
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
              presence: this.presence(),
            }),
          );
          // Everyone hears who's here now, as from the server.
          this.broadcast({ type: "presence", presence: this.presence() });
          break;
        case "ping":
          ws.send(JSON.stringify({ type: "pong", clientMs: msg.clientMs, serverMs: this.now() }));
          break;
        case "load":
          this.change((s) => {
            s.clipId = msg.clipId;
            s.durationMs = 40_000;
            s.positionMs = 0;
            // Playing from `startAt`, at least the lead play needs ahead, as the server.
            s.playing = msg.startAt != null;
            s.atServerMs =
              msg.startAt != null
                ? Math.max(msg.startAt, this.now() + 400) + this.startDelayMs
                : this.now();
          });
          break;
        case "play":
          this.change((s) => {
            s.positionMs = this.positionAt(this.now());
            s.playing = true;
            s.atServerMs = this.now() + 400;
          });
          break;
        case "pause":
          this.change((s) => {
            s.positionMs = this.positionAt(this.now());
            s.playing = false;
            s.atServerMs = this.now();
          });
          break;
        case "react":
          this.broadcast({
            type: "reaction",
            userId,
            clipId: msg.clipId,
            emoji: msg.emoji,
            atMs: msg.atMs,
          });
          break;
        case "takeOver":
          this.hostId = userId;
          this.hostAwaySince = null;
          this.broadcast({ type: "presence", presence: this.presence() });
          break;
        case "seek":
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

  hostId = me.id;
  /** Holds scheduled starts back this much more, so a slow machine can look before one
   *  starts. */
  startDelayMs = 0;
  /** When the host left (hub time), while they're away. */
  hostAwaySince: number | null = null;

  presence() {
    return {
      hostId: this.hostId,
      online: [...this.sockets.keys()],
      hostAwaySince: this.hostAwaySince,
    };
  }

  /** The host's player jumps to `positionMs`, as a seek does. */
  jump(positionMs: number) {
    this.change((s) => {
      s.positionMs = positionMs;
      s.atServerMs = this.now();
    });
  }

  /** The host goes away `agoMs` ago. */
  hostLeft(agoMs: number) {
    this.hostAwaySince = this.now() - agoMs;
    this.broadcast({ type: "presence", presence: this.presence() });
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

async function join(
  context: BrowserContext,
  hub: Hub,
  userId: string,
  routes?: (page: Page) => Promise<unknown>,
): Promise<Page> {
  const page = await context.newPage();
  await page.routeWebSocket(/\/api\/shows\/[^/]+\/live$/, (ws) => hub.attach(ws, userId));
  await open(page, `/shows/${SHOW_ID}`, false, undefined, routes);
  await expect(page.getByTestId("show")).toHaveAttribute("data-connection", "open");
  return page;
}

async function videoSeconds(page: Page): Promise<number> {
  return page.getByTestId("show-video").evaluate((v: HTMLVideoElement) => v.currentTime);
}

async function startPlaying(page: Page) {
  // Autoplay with sound may need a click, like a real first visit. Wait until it either
  // plays in step or asks: right after Play it's still starting, and neither shows yet.
  // Closing the last few hundred ms is gentle (3 % faster), so it can take a while on a
  // busy machine, and CI runs one worker per core.
  const join = page.getByRole("button", { name: "Join with sound" });
  const status = page.getByTestId("sync-status");
  await expect
    .poll(
      async () =>
        (await join.isVisible()) ||
        ((await status.textContent()) === "Synced" && !(await videoState(page)).paused),
      { timeout: 30_000 },
    )
    .toBe(true);
  if (await join.isVisible()) await join.click();
}

/** Host and friend playing the first clip, in step. */
async function playFirstClip(host: Page, friend: Page) {
  await host.getByRole("button", { name: `Start with ${showClip.title}` }).click();
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

  // Everyone gets the controls, not only the host.
  await expect(host.getByTestId("controls")).toBeVisible();
  await expect(friend.getByTestId("controls")).toBeVisible();

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
  const drift = Number(await friend.getByTestId("show-video").getAttribute("data-drift"));
  expect(Math.abs(drift)).toBeLessThan(400);

  // The connection drops (a deploy): it reconnects and stays in step.
  hub.drop(FRIEND);
  const show = friend.getByTestId("show");
  await expect(show).toHaveAttribute("data-connection", "reconnecting");
  await expect(show).toHaveAttribute("data-connection", "open", { timeout: 5_000 });
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

  await host.getByRole("button", { name: "Next clip" }).click();
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
  await expect(page.getByRole("alert")).toBeVisible({ timeout: 5_000 });
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
  await expect(page.getByRole("heading", { name: /night show$/ })).toBeVisible();
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

  await host.getByRole("button", { name: "Pause for everyone" }).click();
  await expect.poll(async () => (await videoState(friend)).paused).toBe(true);
  expect(sent("pause")).toHaveLength(1);
  const paused = hub.state.positionMs;

  await host.getByRole("button", { name: "Forward 10 s" }).click();
  await expect.poll(() => sent("seek")).toHaveLength(1);
  expect(Number(sent("seek")[0]?.positionMs)).toBeCloseTo(paused + 10_000, -2);
  await expect.poll(() => videoSeconds(friend)).toBeCloseTo((paused + 10_000) / 1000, 0);

  // Back past the start: from the start.
  await host.getByRole("button", { name: "Back 10 s" }).click();
  await host.getByRole("button", { name: "Back 10 s" }).click();
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
  await host.getByRole("button", { name: `Start with ${showClip.title}` }).click();
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
  await expect(page.getByTestId("show")).toHaveAttribute("data-connection", "open");
  await expect(page.getByTestId("controls")).toBeVisible();
  await expect(page.getByRole("button", { name: "End the show" })).toBeVisible();

  // The show's details changed (a clip added, say): it loads them again.
  await expect.poll(() => loads).toBeGreaterThan(0);
  const before = loads;
  hub.say({ type: "showChanged" });
  await expect.poll(() => loads).toBe(before + 1);

  // Someone else is the host now: the controls stay (they're everyone's), End the show
  // goes.
  hub.say({
    type: "presence",
    presence: { hostId: FRIEND, online: [me.id, FRIEND], hostAwaySince: null },
  });
  await expect(page.getByRole("button", { name: "End the show" })).toHaveCount(0);
  await expect(page.getByTestId("controls")).toBeVisible();

  // A refusal is said; "slow down" isn't worth saying.
  hub.say({ type: "error", message: "slow down" });
  hub.say({ type: "error", message: "only while the show is live" });
  await expect(page.getByRole("alert")).toHaveText("only while the show is live");
  await expect(page.getByText("slow down")).toHaveCount(0);
  await expect(page.getByTestId("show")).toHaveAttribute("data-connection", "open");
});

test("leaving the show page closes the connection", async ({ browser }) => {
  const hub = new Hub();
  const friend = await join(await browser.newContext(), hub, FRIEND);
  expect(hub.sockets.has(FRIEND)).toBe(true);
  await friend.getByRole("link", { name: "clipos home" }).click();
  await expect(friend.getByRole("heading", { name: "Archive" })).toBeVisible();
  await expect.poll(() => hub.sockets.has(FRIEND)).toBe(false);
  // Gone for good: longer than the first reconnect's backoff, and no reconnect.
  await friend.waitForTimeout(1_500);
  expect(hub.connects.get(FRIEND)).toBe(1);
});

type ShowView = typeof show;

/** The show as the server has it, changeable by the test: GET answers with it, Start makes
 *  it live, adding a clip is recorded. */
class StoredShow {
  show: ShowView;
  posts: { path: string; body: unknown }[] = [];
  constructor(patch: Partial<ShowView> = {}) {
    this.show = structuredClone({ ...show, ...patch });
  }
  /** What `end` answers, in turn; once they're used up it ends the show as `ended`. */
  endings: { status: number; json: unknown }[] = [];
  /** The show as it is once ended. */
  ended: Partial<ShowView> = {};
  routes = async (page: Page) => {
    await page.route(`**/api/shows/${SHOW_ID}**`, (route) => {
      const req = route.request();
      const path = new URL(req.url()).pathname;
      if (req.method() === "POST" || req.method() === "PUT") {
        const body = req.postData() ? req.postDataJSON() : null;
        this.posts.push({ path, body });
        if (path.endsWith("/start")) this.show.status = "live";
        const vote = path.match(/\/votes\/(clip|fail)$/)?.[1] as "clip" | "fail" | undefined;
        if (vote) {
          this.show.myVotes[vote] = body.clipId;
          this.show.voters[vote] = [FRIEND];
        }
        if (path.endsWith("/end")) {
          const reply = this.endings.shift();
          if (reply) return route.fulfill({ status: reply.status, json: reply.json });
          Object.assign(this.show, { status: "ended", ...this.ended });
        }
      }
      return route.fulfill({ json: this.show });
    });
  };
}

test("joining: who's here, I'm ready, and the host's Start turns it into the show", async ({
  browser,
}, testInfo) => {
  const hub = new Hub();
  const stored = new StoredShow({ status: "lobby", startedAt: null });
  const friend = await join(await browser.newContext(), hub, FRIEND, stored.routes);
  await expect(friend.getByRole("heading", { name: "Robin's show starts soon." })).toBeVisible();
  const here = friend.getByTestId("whos-here");
  await expect(here.getByRole("listitem").filter({ hasText: "Jamie Doe" })).toContainText(
    "Here, sound off",
  );
  await expect(here.getByRole("listitem").filter({ hasText: "Robin" })).toContainText("Away");

  // The host turns up.
  const host = await join(await browser.newContext(), hub, me.id, stored.routes);
  await expect(here.getByRole("listitem").filter({ hasText: "Robin" })).toContainText("Ready");
  await expect(host.getByRole("button", { name: "I'm ready, sound on" })).toHaveCount(0);
  await testInfo.attach("joining.png", {
    body: await friend.screenshot({ fullPage: true }),
    contentType: "image/png",
  });
  expect(await layoutProblems(friend)).toEqual([]);

  // Ready: said to the room, and shown once the show says so.
  await friend.getByRole("button", { name: "I'm ready, sound on" }).click();
  expect(hub.received.filter((r) => r.userId === FRIEND).map((r) => r.msg)).toEqual([
    { type: "ready", ready: true },
  ]);
  const jamie = stored.show.participants.find((p) => p.member.id === FRIEND);
  if (jamie) jamie.ready = true;
  hub.say({ type: "showChanged" });
  await expect(friend.getByText("Ready, sound on")).toBeVisible();

  // The host starts: everyone's page becomes the show.
  await host.getByRole("button", { name: "Start the show" }).click();
  expect(stored.posts.map((p) => p.path)).toEqual([`/api/shows/${SHOW_ID}/start`]);
  hub.say({ type: "showChanged" });
  await expect(host.getByRole("button", { name: `Start with ${showClip.title}` })).toBeVisible();
  await expect(friend.getByRole("button", { name: `Start with ${showClip.title}` })).toBeVisible();
});

test("reactions float for everyone; 🍌 marks a fail; Kip asks when the uploader dies", async ({
  browser,
}, testInfo) => {
  test.setTimeout(60_000);
  const hub = new Hub();
  const stored = new StoredShow();
  const withDeath = async (page: Page) => {
    await stored.routes(page);
    await page.route(`**/api/clips/${showClip.id}/analysis`, (route) =>
      route.fulfill({
        json: {
          status: "done",
          stats: null,
          kills: [{ t: 1, owner: "myDeath", weapon: "awp", modifiers: [] }],
        },
      }),
    );
  };
  const host = await join(await browser.newContext(), hub, me.id, withDeath);
  const friend = await join(await browser.newContext(), hub, FRIEND, withDeath);
  await playFirstClip(host, friend);

  await friend.getByRole("button", { name: "React", exact: true }).click();
  await friend.getByRole("button", { name: "React Fire" }).click();
  // Floats at once for the tapper, and for everyone else when the room says so.
  await expect(friend.getByTestId("floats")).toContainText("🔥");
  await expect(host.getByTestId("floats")).toContainText("🔥");
  await testInfo.attach("show-friend.png", {
    body: await friend.screenshot({ fullPage: true }),
    contentType: "image/png",
  });
  expect(await layoutProblems(friend)).toEqual([]);
  const react = hub.received.find((r) => r.msg.type === "react")?.msg;
  expect(react).toMatchObject({ clipId: showClip.id, emoji: "🔥" });
  expect(Number(react?.atMs)).toBeGreaterThan(0);
  // Escape closes the dock.
  await friend.keyboard.press("Escape");
  await expect(friend.getByRole("button", { name: "React Fire" })).toHaveCount(0);

  // A second in, the uploader died: Kip asks. Pressing it is the 🍌.
  const hint = host.getByRole("button", { name: "Fail? 🍌" });
  await expect(hint).toBeVisible();
  stored.show.failContenders = [showClip.id];
  await hint.click();
  expect(hub.received.filter((r) => r.msg.type === "react").at(-1)?.msg).toMatchObject({
    emoji: "🍌",
  });
  // The show now has it as a contender: no more asking, and the dock's 🍌 is on.
  await expect(hint).toHaveCount(0);
  await host.getByRole("button", { name: "React", exact: true }).click();
  await expect(
    host.getByRole("button", { name: "Fail? Mark it for fail of the night" }),
  ).toHaveAttribute("aria-pressed", "true");
});

test("a friend steers for everyone; the volume is each one's own", async ({ browser }) => {
  test.setTimeout(60_000);
  const hub = new Hub();
  const host = await join(await browser.newContext(), hub, me.id);
  const friend = await join(await browser.newContext(), hub, FRIEND);
  await playFirstClip(host, friend);

  // The friend pauses, jumps back and plays: it's the room's state, so the host's too.
  await friend.getByRole("button", { name: "Pause for everyone" }).click();
  await expect.poll(() => hub.state.playing).toBe(false);
  await expect(host.getByTestId("sync-status")).toHaveText("Paused");
  await friend.getByRole("button", { name: "Back 10 s" }).click();
  expect(hub.received.filter((r) => r.userId === FRIEND).at(-1)?.msg).toMatchObject({
    type: "seek",
  });
  await friend.getByRole("button", { name: "Play for everyone" }).click();
  await expect.poll(() => hub.state.playing).toBe(true);
  await startPlaying(host);

  // The volume slides out on hover and back on leaving; turning it down or muting is
  // only the friend's, and nothing goes to the room.
  const sent = hub.received.length;
  const slider = friend.getByRole("slider", { name: "Volume" });
  const box = friend.getByTestId("volume");
  const width = () => box.evaluate((el) => el.getBoundingClientRect().width);
  await expect.poll(width).toBe(52);
  await box.hover();
  await expect.poll(width).toBeGreaterThan(150);
  await slider.fill("30");
  const level = (page: Page) =>
    page.getByTestId("show-video").evaluate((v: HTMLVideoElement) => [v.volume, v.muted]);
  expect(await level(friend)).toEqual([0.3, false]);
  await friend.getByRole("button", { name: "Mute" }).click();
  expect(await level(friend)).toEqual([0.3, true]);
  expect(await level(host)).toEqual([1, false]);
  await friend.mouse.move(0, 0);
  await expect.poll(width).toBe(52);
  await expect(friend.getByRole("button", { name: "Unmute" })).toBeVisible();
  expect(hub.received.length).toBe(sent);
  // Kept for the next show, in this browser.
  await friend.reload();
  await expect(friend.getByTestId("show")).toHaveAttribute("data-connection", "open");
  expect(await level(friend)).toEqual([0.3, true]);
});

test("the side panel: up next, play one now, hide it, add a clip", async ({ page }, testInfo) => {
  const hub = new Hub();
  const stored = new StoredShow();
  await page.routeWebSocket(/\/api\/shows\/[^/]+\/live$/, (ws) => hub.attach(ws, me.id));
  await open(page, `/shows/${SHOW_ID}`, false, undefined, stored.routes);
  const panel = page.getByRole("complementary", { name: "Up next" });
  await expect(panel).toContainText("Up next · 2 clips · 1:20");
  await expect(page.getByText("clip 1 of 2", { exact: true })).toBeVisible();

  await testInfo.attach("show-host.png", {
    body: await page.screenshot({ fullPage: true }),
    contentType: "image/png",
  });
  expect(await layoutProblems(page)).toEqual([]);
  expect(await cspViolations(page)).toEqual([]);

  // The host puts the second one on.
  await panel.getByRole("button", { name: `Play ${showClip2.title} now` }).click();
  await expect.poll(() => hub.state.clipId).toBe(showClip2.id);
  expect(hub.state.playing).toBe(true);
  await expect(page.getByText("clip 2 of 2", { exact: true })).toBeVisible();
  await expect(panel).toContainText(/in 0:\d\d/);

  // Folded away and back.
  await panel.getByRole("button", { name: "Hide the side panel" }).click();
  await expect(panel).toHaveCount(0);
  await page.getByRole("button", { name: "Up next" }).click();
  await expect(panel).toBeVisible();

  // Add a clip from the archive: the ready ones not in the show.
  await panel.getByRole("button", { name: "Add a clip" }).click();
  const dialog = page.getByRole("dialog", { name: "Add a clip to the show" });
  // Before searching: the crowd's favourites (reactions), then the newest uploads.
  const favourites = dialog.getByRole("region", { name: "Crowd favourites" });
  await expect(favourites.getByText(clips.long.title)).toBeVisible();
  await expect(favourites).toContainText("7 reactions");
  const recent = dialog.getByRole("region", { name: "Recent uploads" });
  await expect(recent.getByRole("button", { name: `Add ${clips.normal.title}` })).toBeVisible();
  await expect(recent.getByText(clips.long.title)).toHaveCount(0);
  await expect(dialog.getByText(clips.processing.title)).toHaveCount(0);
  await dialog.getByRole("button", { name: `Add ${clips.normal.title}` }).click();
  await expect(dialog).toBeHidden();
  await expect(page.getByText(`${clips.normal.title} is at the end of the queue.`)).toBeVisible();
  expect(stored.posts).toEqual([
    { path: `/api/shows/${SHOW_ID}/clips`, body: { clipId: clips.normal.id } },
  ]);
});

test("an abandoned show has its own screen", async ({ page }) => {
  const stored = new StoredShow({ status: "abandoned" });
  await page.routeWebSocket(/\/api\/shows\/[^/]+\/live$/, (ws) => new Hub().attach(ws, me.id));
  await open(page, `/shows/${SHOW_ID}`, false, undefined, stored.routes);
  await expect(page.getByTestId("show-over")).toBeVisible();
  await expect(page.getByText("Everyone left, so it ended without a finale.")).toBeVisible();
  await page.getByRole("link", { name: "Tonight" }).click();
  await expect(page).toHaveURL(/\/tonight$/);
});

test("between clips: the next one counts down for everyone; Hold and Start now", async ({
  browser,
}, testInfo) => {
  test.setTimeout(60_000);
  const hub = new Hub();
  const stored = new StoredShow({
    reactions: [
      { clipId: showClip.id, userId: FRIEND, emoji: "🔥", atMs: 1000 },
      { clipId: showClip.id, userId: me.id, emoji: "🔥", atMs: 2000 },
      { clipId: showClip.id, userId: FRIEND, emoji: "💀", atMs: 3000 },
    ],
  });
  const host = await join(await browser.newContext(), hub, me.id, stored.routes);
  const friend = await join(await browser.newContext(), hub, FRIEND, stored.routes);
  await playFirstClip(host, friend);

  // The clip ends: the host's screen puts the next one on, 5 s ahead (and the stand-in
  // holds it back 20 s more, so the checks below are done before it starts).
  hub.startDelayMs = 20_000;
  hub.jump(39_800);
  await expect.poll(() => hub.state.clipId, { timeout: 5_000 }).toBe(showClip2.id);
  const load = hub.received.filter((r) => r.msg.type === "load").at(-1)?.msg;
  expect(Number(load?.startAt) - hub.now()).toBeGreaterThan(3_000);
  for (const page of [host, friend]) {
    const between = page.getByTestId("between");
    await expect(between).toContainText("Next up · clip 2 of 2");
    await expect(between).toContainText("Starting for everyone");
    // The host, and nobody else said "I'm ready".
    await expect(between).toContainText("1 of 2 watching is ready");
    await expect(page.getByTestId("just-played-reactions")).toHaveText("🔥2💀1");
  }
  await expect(friend.getByText("clip starts on its own when the count hits 0")).toBeVisible();
  await testInfo.attach("between.png", {
    body: await host.screenshot({ fullPage: true }),
    contentType: "image/png",
  });
  expect(await layoutProblems(host)).toEqual([]);

  // Hold, from the friend: the count stops for everyone.
  await friend.getByRole("button", { name: "Hold" }).click();
  await expect(host.getByTestId("between")).toContainText("Held");
  await expect(host.getByText("Held. It starts when someone presses Start now.")).toBeVisible();
  await expect(friend.getByRole("button", { name: "Hold" })).toHaveCount(0);

  // Start now: it plays, and Up next goes.
  await host.getByRole("button", { name: "Start now" }).click();
  await startPlaying(friend);
  await expect(friend.getByTestId("between")).toHaveCount(0);
  await expect(friend.getByText("Second show clip")).toBeVisible();
});

test("after the last clip the host opens the finale; End the show gets there early", async ({
  browser,
}) => {
  test.setTimeout(60_000);
  const hub = new Hub();
  // The first clip has played already.
  const stored = new StoredShow();
  const first = stored.show.lineup[0];
  if (first) first.playedAt = stored.show.createdAt;
  const host = await join(await browser.newContext(), hub, me.id, stored.routes);
  const friend = await join(await browser.newContext(), hub, FRIEND, stored.routes);

  // End the show early: asked first.
  await host.getByRole("button", { name: "End the show" }).click();
  const dialog = host.getByRole("dialog", { name: "End the show now?" });
  await dialog.getByRole("button", { name: "Keep watching" }).click();
  await expect(dialog).toBeHidden();
  expect(stored.posts).toEqual([]);

  // The last clip plays to its end.
  await host.getByRole("button", { name: `Play ${showClip2.title} now` }).click();
  await startPlaying(host);
  hub.jump(39_900);
  await expect(host.getByText("That was every clip.")).toBeVisible();
  await expect(friend.getByText("Waiting for Robin to open the finale.")).toBeVisible();
  await host.getByRole("button", { name: "Go to the finale" }).click();
  expect(stored.posts.map((p) => p.path)).toEqual([`/api/shows/${SHOW_ID}/finale`]);

  // Or straight from the dialog.
  await host.getByRole("button", { name: "End the show" }).click();
  await dialog.getByRole("button", { name: "End and vote" }).click();
  await expect(dialog).toBeHidden();
  expect(stored.posts).toHaveLength(2);
  stored.show.status = "finale";
  hub.say({ type: "showChanged" });
  await expect(friend.getByRole("heading", { name: "Counting the votes…" })).toBeVisible();
});

test("the host drops out: the clip plays on, then anyone can take over after a minute", async ({
  browser,
}) => {
  test.setTimeout(60_000);
  const hub = new Hub();
  const host = await join(await browser.newContext(), hub, me.id);
  const friend = await join(await browser.newContext(), hub, FRIEND);
  await playFirstClip(host, friend);

  await host.close();
  hub.hostLeft(30_000);
  await expect(friend.getByText("Robin dropped out. The clip plays to its end.")).toBeVisible();
  expect((await videoState(friend)).paused).toBe(false);

  // The clip is over: the show holds, and the takeover counts down.
  hub.jump(40_000);
  const away = friend.getByRole("alertdialog", { name: "Robin dropped out" });
  await expect(away).toBeVisible();
  await expect(away.getByRole("button", { name: /^Take over in 0:\d\d$/ })).toBeDisabled();

  hub.hostLeft(61_000);
  await away.getByRole("button", { name: "Take over as host" }).click();
  // Once (the new host's screen then moves on, so it may not be the last thing said).
  await expect
    .poll(() => hub.received.filter((r) => r.userId === FRIEND && r.msg.type === "takeOver"))
    .toHaveLength(1);
  // The friend hosts now, and their screen moves the show on to the next clip.
  await expect(away).toBeHidden();
  await expect(friend.getByRole("button", { name: "Hold" })).toBeVisible();
  await expect.poll(() => hub.state.clipId).toBe(showClip2.id);
});

test("someone waiting for a host who's gone can leave", async ({ page }) => {
  const hub = new Hub();
  hub.hostId = FRIEND;
  hub.hostAwaySince = hub.now() - 5_000;
  await page.routeWebSocket(/\/api\/shows\/[^/]+\/live$/, (ws) => hub.attach(ws, me.id));
  await open(page, `/shows/${SHOW_ID}`);
  const away = page.getByRole("alertdialog", { name: "Jamie Doe dropped out" });
  await expect(away.getByRole("button", { name: /^Take over in 0:\d\d$/ })).toBeDisabled();
  await away.getByRole("button", { name: "Leave the show" }).click();
  await expect(page).toHaveURL(/\/tonight$/);
});

/** A show in its finale: both clips played, 🍌 on the first. Its vote's windows are
 *  `failLeftMs` and then `clipMs` from now on the hub's clock (none for fail: no 🍌). */
function finaleShow(hub: Hub, failLeftMs: number | null, clipMs = 20_000) {
  const iso = (ms: number) => new Date(ms).toISOString();
  const t = hub.now();
  const failUntil = failLeftMs == null ? null : t + failLeftMs;
  const clipFrom = failUntil ?? t;
  return new StoredShow({
    status: "finale",
    lineup: show.lineup.map((l) => ({ ...l, playedAt: show.createdAt })),
    failContenders: failLeftMs == null ? [] : [showClip.id],
    finale: {
      startedAt: iso(t - 1_000),
      failFrom: failUntil == null ? null : iso(t - 1_000),
      failUntil: failUntil == null ? null : iso(failUntil),
      clipFrom: iso(clipFrom),
      clipUntil: iso(clipFrom + clipMs),
    },
  });
}

/** Moves the vote's clock on: the windows end `ms` from now, or are over (null). */
function retime(stored: StoredShow, hub: Hub, clipLeftMs: number | null) {
  const iso = (ms: number) => new Date(ms).toISOString();
  const t = hub.now();
  const f = stored.show.finale;
  if (!f) return;
  f.failUntil = f.failUntil && iso(t - 30_000);
  f.failFrom = f.failFrom && iso(t - 50_000);
  f.clipFrom = iso(t - 10_000);
  f.clipUntil = iso(clipLeftMs == null ? t - 2_000 : t + clipLeftMs);
  hub.say({ type: "showChanged" });
}

test("the finale: fail of the night, then clip of the night, then the winners", async ({
  browser,
}, testInfo) => {
  test.setTimeout(60_000);
  const hub = new Hub();
  const stored = finaleShow(hub, 15_000);
  stored.ended = {
    endedAt: new Date(hub.now() + 60_000).toISOString(),
    clipWinnerId: showClip2.id,
    failWinnerId: showClip.id,
    votes: [
      { category: "fail", clipId: showClip.id, votes: 1 },
      { category: "clip", clipId: showClip2.id, votes: 2 },
    ],
  };
  // The second clip is the host's own (the stand-in tells both screens so).
  const second = stored.show.lineup[1];
  if (second) second.clip = { ...second.clip, isMine: true };
  const host = await join(await browser.newContext(), hub, me.id, stored.routes);
  const friend = await join(await browser.newContext(), hub, FRIEND, stored.routes);

  // Fail of the night first: only the 🍌 clip.
  const vote = friend.getByRole("region", { name: "Fail of the night" });
  await expect(vote.getByRole("heading", { name: "Fail of the night?" })).toBeVisible();
  await expect(vote.getByRole("timer")).toHaveText(/^0:1\d$/);
  const cards = vote.getByRole("button", { pressed: false });
  await expect(cards).toHaveCount(1);
  await friend.getByRole("button", { name: /Show clip/ }).click();
  expect(stored.posts.at(-1)).toEqual({
    path: `/api/shows/${SHOW_ID}/votes/fail`,
    body: { clipId: showClip.id },
  });
  await expect(friend.getByRole("button", { name: /Show clip/ })).toHaveAttribute(
    "aria-pressed",
    "true",
  );
  await expect(friend.getByTestId("voted")).toContainText("1 of 2 voted");
  await testInfo.attach("finale-vote.png", {
    body: await friend.screenshot({ fullPage: true }),
    contentType: "image/png",
  });
  expect(await layoutProblems(friend)).toEqual([]);

  // Then clip of the night: every clip played.
  retime(stored, hub, 15_000);
  const clipVote = host.getByRole("region", { name: "Clip of the night" });
  await expect(clipVote.getByRole("heading", { name: "Clip of the night?" })).toBeVisible();
  await expect(clipVote.getByRole("button", { name: /show clip/i })).toHaveCount(2);
  // Your own clip too: friends clip each other.
  const own = clipVote.getByRole("button", { name: /Second show clip/ });
  await expect(own).toContainText("Your clip · click to vote");
  await own.click();
  expect(stored.posts.at(-1)).toEqual({
    path: `/api/shows/${SHOW_ID}/votes/clip`,
    body: { clipId: showClip2.id },
  });

  // The vote closes: the host's screen ends the show, and everyone gets the winners.
  retime(stored, hub, null);
  await expect(friend.getByText("Counting the votes…")).toBeVisible();
  await expect.poll(() => stored.posts.at(-1)?.path).toBe(`/api/shows/${SHOW_ID}/end`);
  expect(stored.posts.at(-1)?.body).toEqual({ tieBreak: {} });
  hub.say({ type: "showChanged" });

  // Fail of the night is revealed first, then clip of the night.
  for (const page of [host, friend]) {
    await expect(page.getByRole("heading", { name: "Fail of the night" })).toBeVisible();
    await expect(page.getByTestId("winner")).toContainText("1 of 1 vote");
  }
  await testInfo.attach("fail-of-the-night.png", {
    body: await friend.screenshot({ fullPage: true }),
    contentType: "image/png",
  });
  await friend.getByRole("button", { name: "Clip of the night" }).click();
  await expect(friend.getByRole("heading", { name: "Clip of the night" })).toBeVisible();
  await expect(friend.getByTestId("winner")).toContainText("Second show clip");
  await expect(friend.getByTestId("winner")).toContainText("2 of 2 votes");
  await expect(friend.getByRole("img", { name: "Kip with a crown" })).toBeVisible();
  await friend.waitForFunction(() =>
    [...document.images].every((i) => i.complete && i.naturalWidth > 0),
  );
  await testInfo.attach("clip-of-the-night.png", {
    body: await friend.screenshot({ fullPage: true }),
    contentType: "image/png",
  });
  expect(await layoutProblems(friend)).toEqual([]);
  await friend.getByRole("link", { name: "Done" }).click();
  await expect(friend).toHaveURL(/\/tonight$/);
});

test("a tie in the finale: the host decides", async ({ browser }) => {
  const hub = new Hub();
  const stored = finaleShow(hub, null);
  stored.endings = [
    {
      status: 409,
      json: {
        error: "conflict",
        message: "the clip of the night vote is tied: pick one of the tied clips",
        tied: { clip: [showClip.id, showClip2.id], fail: [] },
      },
    },
  ];
  stored.ended = { endedAt: new Date(hub.now() - 60_000).toISOString(), clipWinnerId: showClip.id };
  const host = await join(await browser.newContext(), hub, me.id, stored.routes);
  const friend = await join(await browser.newContext(), hub, FRIEND, stored.routes);

  // No 🍌 tonight: straight to clip of the night, and Kip says so.
  await expect(friend.getByText("nobody marked a fail tonight.")).toBeVisible();
  retime(stored, hub, null);

  const decide = host.getByRole("region", { name: "Host decides" });
  await expect(decide).toBeVisible();
  const announce = decide.getByRole("button", { name: "Announce the winners" });
  await expect(announce).toBeDisabled();
  await decide.getByRole("button", { name: showClip.title, exact: true }).click();
  await announce.click();
  expect(stored.posts.at(-1)).toEqual({
    path: `/api/shows/${SHOW_ID}/end`,
    body: { tieBreak: { clip: showClip.id } },
  });
  // Friends wait meanwhile, then get the winner: long ended, so straight to the clip.
  await expect(friend.getByText("Robin announces the winners in a moment.")).toBeVisible();
  hub.say({ type: "showChanged" });
  await expect(friend.getByRole("heading", { name: "Clip of the night" })).toBeVisible();
  await expect(friend.getByTestId("winner")).toContainText("0 of 0 votes");
});

test("an ended show nobody voted in says so", async ({ page }) => {
  const stored = new StoredShow({ status: "ended", endedAt: show.createdAt });
  await page.routeWebSocket(/\/api\/shows\/[^/]+\/live$/, () => {});
  await open(page, `/shows/${SHOW_ID}`, false, undefined, stored.routes);
  await expect(page.getByText("Nobody voted for clip of the night.")).toBeVisible();
  await expect(page.getByRole("button", { name: "Fail of the night" })).toHaveCount(0);
});

test("Kip's kill card: on the uploader's death, for everyone, then gone", async ({
  browser,
}, testInfo) => {
  test.setTimeout(60_000);
  const hub = new Hub();
  const withKills = (myKills: number, deathAt: number | null) => async (page: Page) => {
    await page.route(`**/api/clips/${showClip.id}/analysis`, (route) =>
      route.fulfill({
        json: {
          status: "done",
          stats: {
            kills: myKills,
            myKills,
            myDeaths: deathAt == null ? 0 : 1,
            multiKill: null,
            weapons: {},
            modifiers: {},
          },
          kills:
            deathAt == null ? [] : [{ t: deathAt, owner: "myDeath", weapon: "awp", modifiers: [] }],
        },
      }),
    );
  };
  const host = await join(await browser.newContext(), hub, me.id, withKills(4, 2));
  const friend = await join(await browser.newContext(), hub, FRIEND, withKills(4, 2));
  await playFirstClip(host, friend);

  // Two seconds in, the uploader dies: the card comes up on both screens.
  for (const page of [host, friend]) {
    const card = page.getByTestId("kill-card");
    await expect(card).toHaveAttribute("data-kills", "4", { timeout: 5_000 });
    await expect(card.getByRole("img", { name: /4 kills/ })).toBeVisible();
  }
  await friend.waitForTimeout(2_000);
  await testInfo.attach("kill-card.png", {
    body: await friend.screenshot({ fullPage: true }),
    contentType: "image/png",
  });
  // Five seconds later it's gone.
  hub.jump(8_000);
  await expect(friend.getByTestId("kill-card")).toHaveCount(0);
  await expect(host.getByTestId("kill-card")).toHaveCount(0);
});

test("no death: the kill card takes the clip's last seconds, and an ace lands hard", async ({
  page,
}) => {
  const hub = new Hub();
  await page.routeWebSocket(/\/api\/shows\/[^/]+\/live$/, (ws) => hub.attach(ws, me.id));
  await open(page, `/shows/${SHOW_ID}`, false, undefined, (p) =>
    p.route(`**/api/clips/${showClip.id}/analysis`, (route) =>
      route.fulfill({
        json: {
          status: "done",
          stats: {
            kills: 5,
            myKills: 5,
            myDeaths: 0,
            multiKill: "ace",
            weapons: {},
            modifiers: {},
          },
          kills: [],
        },
      }),
    ),
  );
  await page.getByRole("button", { name: `Start with ${showClip.title}` }).click();
  await startPlaying(page);
  await expect(page.getByTestId("kill-card")).toHaveCount(0);
  hub.jump(36_000);
  const card = page.getByTestId("kill-card");
  await expect(card).toHaveAttribute("data-kills", "5");
  await expect(card).toHaveClass(/ace/);
});
