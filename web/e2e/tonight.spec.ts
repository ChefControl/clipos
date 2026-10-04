// The lobby, /tonight (S6, canvas 2.1): hosting a show, putting the lineup in order, the
// join link and Start; someone else's show; nothing new. The show API is a small stateful
// stand-in, so each change comes back the way the server would send it.
import type { Page, Route } from "@playwright/test";
import type { components } from "../src/api/schema";
import { clips, me, pastShow, tonight } from "./fixtures";
import { layoutProblems } from "./layout";
import { cspViolations, open } from "./pages";
import { expect, test } from "./test";

type Show = components["schemas"]["ShowView"];

const LOBBY_ID = "20000000-0000-4000-8000-0000000000c0";
const host = { ...me, steamName: null };
const jamie = {
  id: "00000000-0000-4000-8000-000000000002",
  handle: "jamie",
  displayName: "Jamie Doe",
  avatarUrl: null,
  steamName: null,
};

const json = (route: Route, body: unknown, status = 200) =>
  route.fulfill({ status, contentType: "application/json", body: JSON.stringify(body) });

/** The server's shows, in miniature: one show, and every request made to it. */
class Shows {
  show: Show | null = null;
  requests: { method: string; path: string; body: unknown }[] = [];

  constructor(show?: Show) {
    this.show = show ?? null;
  }

  /** A lobby you host with tonight's clips. */
  static lobby(): Show {
    return {
      id: LOBBY_ID,
      status: "lobby",
      host,
      createdAt: "2026-10-02T19:00:00Z",
      startedAt: null,
      endedAt: null,
      lineup: tonight.clips.map((clip, position) => ({
        clip,
        position,
        dropped: false,
        playedAt: null,
        addedBy: me.id,
      })),
      participants: [{ member: host, joinedAt: "2026-10-02T19:00:00Z", ready: false }],
      failContenders: [],
      votes: [],
      voters: { clip: [], fail: [] },
      myVotes: { clip: null, fail: null },
      clipWinnerId: null,
      failWinnerId: null,
      reactions: [],
    };
  }

  /** Waiting clips in `ids` order, the rest dropped (as `set_lineup` does). */
  private setLineup(ids: string[]) {
    const show = this.show as Show;
    const entries = show.lineup.map((l) => ({ ...l, dropped: !ids.includes(l.clip.id) }));
    entries.sort((a, b) => {
      const at = (e: (typeof entries)[number]) =>
        e.dropped ? 1000 + e.position : ids.indexOf(e.clip.id);
      return at(a) - at(b);
    });
    show.lineup = entries.map((e, position) => ({ ...e, position }));
  }

  async route(page: Page) {
    await page.route("**/api/shows**", (route) => {
      const req = route.request();
      const path = new URL(req.url()).pathname;
      const method = req.method();
      const body = req.postData() ? req.postDataJSON() : null;
      if (method !== "GET") this.requests.push({ method, path, body });
      if (path === "/api/shows/tonight") {
        return json(route, { ...tonight, show: this.show, clips: this.show ? [] : tonight.clips });
      }
      if (method === "POST" && path === "/api/shows") {
        this.show = Shows.lobby();
        return json(route, this.show, 201);
      }
      if (method === "GET" && path === "/api/shows") return json(route, [pastShow]);
      if (!this.show || !path.startsWith(`/api/shows/${this.show.id}`)) return route.fallback();
      const show = this.show;
      const rest = path.slice(`/api/shows/${show.id}`.length);
      if (method === "PUT" && rest === "/lineup") this.setLineup(body.clipIds);
      if (method === "POST" && rest === "/clips") {
        const entry = show.lineup.find((l) => l.clip.id === body.clipId);
        if (entry) {
          show.lineup = [...show.lineup.filter((l) => l !== entry), { ...entry, dropped: false }];
        }
      }
      if (method === "POST" && rest === "/start") show.status = "live";
      return json(route, show);
    });
    // The lobby's live room: welcomes you and says who's online.
    await page.routeWebSocket(/\/api\/shows\/[^/]+\/live$/, (ws) => {
      ws.onMessage((raw) => {
        const msg = JSON.parse(String(raw));
        if (msg.type === "hello") {
          ws.send(
            JSON.stringify({
              type: "welcome",
              userId: me.id,
              serverMs: Date.now(),
              state: {
                seq: 0,
                clipId: null,
                playing: false,
                positionMs: 0,
                atServerMs: 0,
                rate: 1,
                durationMs: null,
              },
              presence: { hostId: me.id, online: [me.id], hostAwaySince: null },
            }),
          );
        }
        if (msg.type === "ping") {
          ws.send(JSON.stringify({ type: "pong", clientMs: msg.clientMs, serverMs: Date.now() }));
        }
      });
    });
  }
}

const lineupTitles = (page: Page) =>
  page.getByTestId("lineup").locator("li").locator("span[dir=auto]").allTextContents();

test("Tonight is in the top bar once the show is open to you, and leads to the lobby", async ({
  page,
}) => {
  await open(page, "/");
  await page.getByRole("navigation").getByRole("link", { name: "Tonight" }).click();
  await expect(page).toHaveURL(/\/tonight$/);
  await expect(
    page.getByRole("heading", { name: "3 new clips. Nobody's seen them yet." }),
  ).toBeVisible();
  // The lineup in upload order, the held clip as its teaser; nothing to reorder yet.
  expect(await lineupTitles(page)).toEqual([
    clips.normal.title,
    clips.held.title,
    clips.long.title,
  ]);
  await expect(page.getByRole("button", { name: /^Move / })).toHaveCount(0);
  // The trophy shelf and past shows from the last show.
  await expect(page.getByText("Clip of the night · Sep 25")).toBeVisible();
  await expect(page.getByText("Fail of the night · Sep 25")).toBeVisible();
  await expect(page.getByText("Robin hosted · 2 clips")).toBeVisible();
});

test("shows that aren't open to you: no Tonight, and /tonight isn't there", async ({ page }) => {
  await open(page, "/tonight", false, undefined, (p) =>
    p.route("**/api/me", (route) =>
      json(route, { ...me, role: "member", shows: false, showsForEveryone: false }),
    ),
  );
  await expect(page.getByText("Nothing here.")).toBeVisible();
  await expect(page.getByRole("navigation").getByRole("link", { name: "Tonight" })).toHaveCount(0);
});

test("hosting: open the lobby, reorder, drop and put back, copy the link, start", async ({
  page,
  context,
}, testInfo) => {
  await context.grantPermissions(["clipboard-read", "clipboard-write"]);
  const shows = new Shows();
  await open(page, "/tonight", false, undefined, (p) => shows.route(p));
  await page.getByRole("button", { name: "Host tonight's show" }).click();
  await expect(page.getByText("Your lobby")).toBeVisible();
  expect(shows.requests).toEqual([{ method: "POST", path: "/api/shows", body: null }]);
  await expect(page.getByTestId("in-lobby")).toContainText("Just you so far");

  await testInfo.attach("lobby.png", {
    body: await page.screenshot({ fullPage: true }),
    contentType: "image/png",
  });
  expect(await layoutProblems(page)).toEqual([]);
  expect(await cspViolations(page)).toEqual([]);

  // Drag the last clip to the top.
  const rows = page.getByTestId("lineup").locator("li");
  await rows.nth(2).dragTo(rows.nth(0));
  await expect
    .poll(() => lineupTitles(page))
    .toEqual([clips.long.title, clips.normal.title, clips.held.title]);
  expect(shows.requests.at(-1)?.body).toEqual({
    clipIds: [clips.long.id, clips.normal.id, clips.held.id],
  });

  // The arrow keys on a handle move it one place, and focus stays on it.
  const handle = page.getByRole("button", { name: `Move ${clips.normal.title}` });
  await handle.focus();
  await page.keyboard.press("ArrowUp");
  await expect
    .poll(() => lineupTitles(page))
    .toEqual([clips.normal.title, clips.long.title, clips.held.title]);
  await expect(handle).toBeFocused();
  await expect(page.getByRole("status").filter({ hasText: "moved to 1 of 3" })).toBeAttached();

  // Drop one: it waits under the lineup, and comes back at the end.
  await page.getByRole("button", { name: `Drop ${clips.long.title} from tonight` }).click();
  await expect(
    page.getByRole("heading", { name: "2 new clips. Nobody's seen them yet." }),
  ).toBeVisible();
  expect(shows.requests.at(-1)?.body).toEqual({ clipIds: [clips.normal.id, clips.held.id] });
  await expect(page.getByText("Dropped · they wait for the next show")).toBeVisible();
  await page.getByRole("button", { name: "Put back" }).click();
  await expect
    .poll(() => lineupTitles(page))
    .toEqual([clips.normal.title, clips.held.title, clips.long.title]);
  expect(shows.requests.at(-1)).toEqual({
    method: "POST",
    path: `/api/shows/${LOBBY_ID}/clips`,
    body: { clipId: clips.long.id },
  });

  // The join link is the show's page.
  await page.getByRole("button", { name: "Copy the join link" }).click();
  await expect(page.getByText("Join link copied")).toBeVisible();
  expect(await page.evaluate(() => navigator.clipboard.readText())).toMatch(
    new RegExp(`/shows/${LOBBY_ID}$`),
  );

  await page.getByRole("button", { name: "Start the show" }).click();
  await expect(page).toHaveURL(new RegExp(`/shows/${LOBBY_ID}$`));
  expect(shows.requests.at(-1)?.path).toBe(`/api/shows/${LOBBY_ID}/start`);
});

test("a lobby with every clip dropped can't start", async ({ page }) => {
  const lobby = Shows.lobby();
  lobby.lineup = lobby.lineup.map((l) => ({ ...l, dropped: true }));
  const shows = new Shows(lobby);
  await open(page, "/tonight", false, undefined, (p) => shows.route(p));
  await expect(page.getByRole("heading", { name: "Nothing in the lineup." })).toBeVisible();
  await expect(page.getByRole("button", { name: "Start the show" })).toBeDisabled();
  await expect(page.getByText("Every clip is dropped. Put one back to start.")).toBeVisible();
});

test("a change the server refuses says so, and the lineup stays as it was", async ({ page }) => {
  const shows = new Shows(Shows.lobby());
  await open(page, "/tonight", false, undefined, async (p) => {
    await shows.route(p);
    await p.route("**/api/shows/*/lineup", (route) =>
      json(route, { error: "conflict", message: "the show moved on meanwhile" }, 409),
    );
  });
  await page.getByRole("button", { name: `Drop ${clips.long.title} from tonight` }).click();
  await expect(
    page.getByText("Couldn't change the lineup: the show moved on meanwhile"),
  ).toBeVisible();
  expect(await lineupTitles(page)).toEqual([
    clips.normal.title,
    clips.held.title,
    clips.long.title,
  ]);
});

test("someone else's lobby and live show: Join", async ({ page }) => {
  const lobby = { ...Shows.lobby(), host: jamie };
  const shows = new Shows(lobby);
  await open(page, "/tonight", false, undefined, (p) => shows.route(p));
  await expect(page.getByRole("heading", { name: "Jamie Doe's show starts soon." })).toBeVisible();
  await expect(page.getByText("Jamie Doe puts these in order")).toBeVisible();
  await expect(page.getByRole("button", { name: /^Move / })).toHaveCount(0);
  await expect(page.getByRole("link", { name: "Join the show" })).toHaveAttribute(
    "href",
    `/shows/${LOBBY_ID}`,
  );

  shows.show = { ...lobby, status: "live" };
  await page.reload();
  await expect(page.getByRole("heading", { name: "Jamie Doe's show is on." })).toBeVisible();
  await expect(page.getByText("Tonight's lineup")).toHaveCount(0);

  shows.show = { ...lobby, status: "finale", host };
  await page.reload();
  await expect(page.getByRole("heading", { name: "Your show is voting." })).toBeVisible();
  await page.getByRole("link", { name: "Back to the show" }).click();
  await expect(page).toHaveURL(new RegExp(`/shows/${LOBBY_ID}$`));
});

test("nothing new since the last show: Upload, and the last show's winner", async ({ page }) => {
  await open(page, "/tonight", false, undefined, (p) =>
    p.route("**/api/shows/tonight", (route) =>
      json(route, { show: null, clips: [], lastShow: pastShow }),
    ),
  );
  await expect(
    page.getByRole("heading", { name: "No new clips since the last show." }),
  ).toBeVisible();
  await expect(page.getByText("Clips uploaded after Friday's show")).toBeVisible();
  await expect(page.getByRole("link", { name: "Upload a clip" })).toHaveAttribute(
    "href",
    "/upload",
  );
  const last = page.getByRole("link", { name: /Clip of the night:/ });
  await expect(last).toContainText("2 of 3 votes");
  await last.click();
  await expect(page).toHaveURL(new RegExp(`/clips/${clips.long.id}$`));
});

test("no show has ever happened: the first one's waiting for clips", async ({ page }) => {
  await open(page, "/tonight", false, undefined, async (p) => {
    await p.route("**/api/shows/tonight", (route) =>
      json(route, { show: null, clips: [], lastShow: null }),
    );
    await p.route("**/api/shows", (route) => json(route, []));
  });
  await expect(page.getByRole("heading", { name: "No new clips yet." })).toBeVisible();
  await expect(page.getByText("Clips uploaded this week land here")).toBeVisible();
});

test("while a show is on, every page has the Live pill, except the lobby", async ({ page }) => {
  let current: { status: string } | null = {
    id: LOBBY_ID,
    status: "live",
    host: jamie,
    startedAt: "2026-10-02T19:00:00Z",
  } as never;
  await open(page, "/", false, undefined, (p) =>
    p.route("**/api/shows/current", (route) => json(route, current)),
  );
  const pill = page.getByRole("link", { name: "Join Jamie Doe's show, live now" });
  await expect(pill).toHaveAttribute("href", `/shows/${LOBBY_ID}`);
  await expect(pill).toHaveText(/^Live\s*· Jamie Doe's show\s*Join$/);

  // Not on Tonight, which says it itself.
  await page.getByRole("navigation").getByRole("link", { name: "Tonight" }).click();
  await expect(page.getByTestId("live-pill")).toHaveCount(0);

  // A show in its lobby is starting; once it's over the pill goes.
  current = { ...(current as object), status: "lobby" } as never;
  await page.getByRole("navigation").getByRole("link", { name: "Archive" }).click();
  await expect(
    page.getByRole("link", { name: "Join Jamie Doe's show, starting soon" }),
  ).toContainText("Starting");
  current = null;
  await page.reload();
  await expect(page.getByRole("heading", { name: "Archive" })).toBeVisible();
  await expect(page.getByTestId("live-pill")).toHaveCount(0);
});

test("people the show isn't open to never ask about it", async ({ page }) => {
  let asked = 0;
  await open(page, "/", false, undefined, async (p) => {
    await p.route("**/api/me", (route) =>
      json(route, { ...me, role: "member", shows: false, showsForEveryone: false }),
    );
    await p.route("**/api/shows/current", (route) => {
      asked += 1;
      return json(route, null);
    });
  });
  await expect(page.getByRole("heading", { name: "Archive" })).toBeVisible();
  await page.waitForLoadState("networkidle");
  expect(asked).toBe(0);
});
