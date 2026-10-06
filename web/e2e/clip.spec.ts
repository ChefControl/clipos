// The clip page and the player, in a desktop Chrome window, API mocked. The fixtures' clip
// video is H.264, which Playwright's Chromium can't play; these tests give the clip the
// show's 40 s VP9 video so the player really plays, seeks and steps.
import type { Page, Route } from "@playwright/test";
import type { components } from "../src/api/schema";
import { clips, me } from "./fixtures";
import { open } from "./pages";
import { expect, test } from "./test";

type Clip = components["schemas"]["ClipView"];
type Kill = components["schemas"]["KillView"];

/** The long clip (an admin can edit it), with a video Chromium plays. */
const clip: Clip = { ...clips.long, playbackUrl: "/e2e-media/show.webm", durationMs: 40_000 };

/** Someone else's kill, two of yours (the second with a gun the killfeed couldn't read)
 *  and your death, inside the 40 s video. */
const KILLS: Kill[] = [
  { t: 4, owner: "other", weapon: "awp", modifiers: ["noscope"] },
  { t: 10, owner: "myKill", weapon: "ak47", modifiers: ["headshot"] },
  { t: 20, owner: "myKill", weapon: null, modifiers: [] },
  { t: 30, owner: "myDeath", weapon: "awp", modifiers: [] },
];

const done = (kills: Kill[], stats: Partial<components["schemas"]["AnalysisStats"]> = {}) => ({
  status: "done",
  stats: {
    kills: kills.length,
    myKills: kills.filter((k) => k.owner === "myKill").length,
    myDeaths: kills.filter((k) => k.owner === "myDeath").length,
    multiKill: null,
    weapons: { ak47: 1 },
    modifiers: { headshot: 1 },
    ...stats,
  },
  kills,
});

const json = (route: Route, body: unknown, status = 200) =>
  route.fulfill({ status, contentType: "application/json", body: JSON.stringify(body) });

/** Opens `body`'s clip page with `analysis` as its killfeed, signed in with `role` (the
 *  fixtures' user is an admin). Each test loads the app once: a second load would start its
 *  coverage over. */
async function openClip(
  page: Page,
  body: Clip = clip,
  analysis: unknown = done(KILLS),
  role: (typeof me)["role"] = "admin",
) {
  await open(page, "about:blank");
  if (role !== me.role) {
    await page.route("**/api/me", (route) =>
      json(route, { ...me, role, shows: true, showsForEveryone: false }),
    );
  }
  // Every seek the video makes, for the tests to read.
  await page.addInitScript(() => {
    const w = window as unknown as { __seeks: number[] };
    w.__seeks = [];
    document.addEventListener(
      "seeking",
      (e) => w.__seeks.push(Math.round((e.target as HTMLVideoElement).currentTime * 100) / 100),
      true,
    );
  });
  await page.route(`**/api/clips/${body.id}`, (route) =>
    route.request().method() === "GET" ? json(route, body) : route.fallback(),
  );
  await page.route(`**/api/clips/${body.id}/analysis`, (route) => json(route, analysis));
  await page.goto(`/clips/${body.id}`);
}

/** Opens the playable clip and waits for its video to be ready. */
async function openPlayer(page: Page, analysis: unknown = done(KILLS)) {
  await openClip(page, clip, analysis);
  // The first videos a cold browser loads can take a while.
  await expect
    .poll(() => media(page).then((m) => m.duration), { timeout: 15_000 })
    .toBeCloseTo(40, 0);
  // Nothing focused: the keys go to the page, as after a click on the video.
  await page.evaluate(() => (document.activeElement as HTMLElement | null)?.blur());
}

const media = (page: Page) =>
  page.locator("video").evaluate((v: HTMLVideoElement) => ({
    time: v.currentTime,
    duration: v.duration,
    paused: v.paused,
    muted: v.muted,
    rate: v.playbackRate,
  }));
const time = (page: Page) => media(page).then((m) => m.time);
const seeks = (page: Page) =>
  page.evaluate(() => (window as unknown as { __seeks: number[] }).__seeks.splice(0));

/** The video seeked to `expected`, in order, since the last check. */
async function expectSeeks(page: Page, expected: number[]) {
  await expect
    .poll(() => page.evaluate(() => (window as unknown as { __seeks: number[] }).__seeks))
    .toEqual(expected);
  await seeks(page);
}

/** Seeks the paused video to `t` and waits for it to get there. */
async function seekTo(page: Page, t: number) {
  await page.locator("video").evaluate(
    (v: HTMLVideoElement, to) =>
      new Promise<void>((resolve) => {
        v.pause();
        v.addEventListener("seeked", () => resolve(), { once: true });
        v.currentTime = to;
      }),
    t,
  );
  await seeks(page);
}

/** Presses `key`, a shortcut that seeks, and waits for the seek to `to`. One at a time:
 *  a seek's event reads the time when it fires, which a quick second key has moved on. */
async function pressToSeek(page: Page, key: string, to: number) {
  expect(await press(page, key)).toBe(true);
  await expectSeeks(page, [to]);
}

/** Presses `key` and says whether the page swallowed it (a shortcut took it). */
async function press(page: Page, key: string): Promise<boolean> {
  await page.evaluate(() => {
    const w = window as unknown as { __prevented?: boolean };
    window.addEventListener("keydown", (e) => (w.__prevented = e.defaultPrevented), {
      once: true,
    });
  });
  await page.keyboard.press(key);
  return page.evaluate(() => (window as unknown as { __prevented?: boolean }).__prevented ?? false);
}

test("player keys: play, seek, jump, frames, speed, mute and the shortcut list", async ({
  page,
}) => {
  await openPlayer(page);

  // K and Space play and pause.
  expect(await press(page, "k")).toBe(true);
  await expect.poll(() => media(page).then((m) => m.paused)).toBe(false);
  await press(page, "k");
  await expect.poll(() => media(page).then((m) => m.paused)).toBe(true);
  await press(page, " ");
  await expect.poll(() => media(page).then((m) => m.paused)).toBe(false);
  await press(page, " ");
  await expect.poll(() => media(page).then((m) => m.paused)).toBe(true);

  // J / L: 10 s, the arrows: 5 s, never before the start.
  await seekTo(page, 20);
  for (const [key, to] of [
    ["l", 30],
    ["j", 20],
    ["ArrowRight", 25],
    ["ArrowLeft", 20],
    ["ArrowLeft", 15],
  ] as const) {
    await pressToSeek(page, key, to);
  }
  await seekTo(page, 3);
  await pressToSeek(page, "j", 0);

  // 0–9 jump to that tenth of the clip.
  await pressToSeek(page, "5", 20);
  await pressToSeek(page, "9", 36);
  await pressToSeek(page, "0", 0);

  // , / . step one frame (the clip's fps) and pause.
  await press(page, "k");
  await press(page, ".");
  await expect.poll(() => media(page).then((m) => m.paused)).toBe(true);
  await seekTo(page, 10);
  await pressToSeek(page, ".", 10.02);
  await pressToSeek(page, ",", 10);
  await pressToSeek(page, ",", 9.98);

  // < / > step through the speeds and stop at the ends.
  await press(page, ">");
  expect((await media(page)).rate).toBe(1.5);
  await press(page, ">");
  await press(page, ">");
  expect((await media(page)).rate).toBe(2);
  await press(page, "<");
  expect((await media(page)).rate).toBe(1.5);

  // M mutes and unmutes.
  await press(page, "m");
  expect((await media(page)).muted).toBe(true);
  await expect(page.getByRole("button", { name: "unmute" })).toBeVisible();
  await press(page, "m");
  expect((await media(page)).muted).toBe(false);

  // ? shows the list, and hides it.
  await press(page, "?");
  await expect(page.getByText("Previous / next kill")).toBeVisible();
  await expect(page.getByText("Jump to 0–90 %")).toBeVisible();
  await press(page, "?");
  await expect(page.getByText("Previous / next kill")).toHaveCount(0);
  // So does the button.
  const help = page.getByRole("button", { name: "Keyboard shortcuts (?)" });
  await help.click();
  await expect(page.getByText("Previous / next kill")).toBeVisible();
  await help.click();
  await expect(page.getByText("Previous / next kill")).toHaveCount(0);
});

test("player keys leave other keys, browser shortcuts and focused controls alone", async ({
  page,
}) => {
  await openPlayer(page);
  await seekTo(page, 20);
  // Not a shortcut: the page lets it through.
  expect(await press(page, "x")).toBe(false);
  // With a modifier it's the browser's (Ctrl+L is the address bar).
  expect(await press(page, "Control+l")).toBe(false);
  expect(await press(page, "Alt+j")).toBe(false);
  // A focused button takes the arrows itself.
  await page.getByRole("button", { name: "Theater mode (T)" }).focus();
  expect(await press(page, "ArrowRight")).toBe(false);
  await expectSeeks(page, []);
  expect((await media(page)).time).toBe(20);
});

test("player: the kill buttons and [ / ] jump between kills, a little before each", async ({
  page,
}) => {
  await openPlayer(page);
  const previous = page.getByRole("button", { name: "Previous kill ([)" });
  const next = page.getByRole("button", { name: "Next kill (])" });
  await expect(previous).toBeDisabled();

  // Next: someone else's kill at 4 s, from 2 s, playing.
  await next.click();
  await expectSeeks(page, [2]);
  await expect.poll(() => media(page).then((m) => m.paused)).toBe(false);

  await seekTo(page, 12);
  await page.evaluate(() => (document.activeElement as HTMLElement | null)?.blur());
  await press(page, "]");
  await expectSeeks(page, [18]);
  await seekTo(page, 12);
  await press(page, "[");
  await expectSeeks(page, [8]);
  await seekTo(page, 25);
  await expect(previous).toBeEnabled();
  await previous.click();
  await expectSeeks(page, [18]);

  // Past the last kill's lead-in there's no next one.
  await seekTo(page, 35);
  await expect(next).toBeDisabled();
  await page.evaluate(() => (document.activeElement as HTMLElement | null)?.blur());
  await press(page, "]");
  await expectSeeks(page, []);
});

test("player: the frame buttons step and pause; the timeline seeks where it's clicked", async ({
  page,
}) => {
  await openPlayer(page);
  await seekTo(page, 10);
  await page.getByRole("button", { name: "Next frame (.)" }).click();
  await expectSeeks(page, [10.02]);
  await page.getByRole("button", { name: "Next frame (.)" }).click();
  await expectSeeks(page, [10.03]);
  await page.getByRole("button", { name: "Previous frame (,)" }).click();
  await expectSeeks(page, [10.02]);
  expect((await media(page)).paused).toBe(true);

  // A click on a kill's mark lands on the timeline under it: the video goes there.
  const range = await page.getByRole("slider", { name: "seek" }).boundingBox();
  if (!range) throw new Error("no timeline");
  await page.mouse.click(range.x + range.width * (30 / 40), range.y + range.height / 2);
  await expect.poll(() => time(page)).toBeCloseTo(30, 0);
});

test("player: hovering a kill's mark says whose kill it was and with what", async ({ page }) => {
  await openPlayer(page);
  const range = await page.getByRole("slider", { name: "seek" }).boundingBox();
  if (!range) throw new Error("no timeline");
  const hover = (t: number) =>
    page.mouse.move(range.x + range.width * (t / 40) + 3, range.y + range.height / 2);
  const tip = page.locator("media-control-bar .bottom-4");

  await hover(10);
  await expect(tip).toContainText("Your kill");
  await expect(tip).toContainText("0:10");
  await expect(tip).toContainText("AK-47 · headshot");
  await expect(tip.locator('img[alt="AK-47"]')).toBeVisible();

  await hover(4);
  await expect(tip).toContainText("Kill");
  await expect(tip).toContainText("AWP · no-scope");

  await hover(20);
  await expect(tip).toContainText("Your kill");
  await expect(tip).toContainText("Unknown weapon");
  await expect(tip).toContainText("unknown weapon");

  await hover(30);
  await expect(tip).toContainText("Your death");

  // Between kills, and off the timeline: no tip.
  await hover(25);
  await expect(tip).toHaveCount(0);
  await hover(30);
  await expect(tip).toHaveCount(1);
  await page.mouse.move(range.x + range.width / 2, range.y - 200);
  await expect(tip).toHaveCount(0);
});

test("player: the mute and speed buttons", async ({ page }) => {
  await openPlayer(page);
  await page.getByRole("button", { name: "mute", exact: true }).click();
  expect((await media(page)).muted).toBe(true);
  await page.getByRole("button", { name: "unmute" }).click();
  expect((await media(page)).muted).toBe(false);

  await page.getByRole("button", { name: "Playback rate 1" }).click();
  await expect.poll(() => media(page).then((m) => m.rate)).toBe(1.5);
  await page.getByRole("button", { name: "Playback rate 1.5" }).click();
  await expect.poll(() => media(page).then((m) => m.rate)).toBe(2);
});

test("player: hovering mute slides the volume out, leaving slides it back", async ({ page }) => {
  await openPlayer(page);
  const range = page.locator("media-volume-range");
  const width = () => range.evaluate((el) => el.getBoundingClientRect().width);
  // Over the video, the controls show; the volume stays folded.
  await page.locator("video").hover();
  await expect.poll(width).toBe(0);
  await page.getByRole("button", { name: "mute", exact: true }).hover();
  await expect.poll(width).toBeGreaterThan(80);
  await page.locator("video").hover();
  await expect.poll(width).toBe(0);
  // The mute button stays.
  await expect(page.getByRole("button", { name: "mute", exact: true })).toBeVisible();
});

test("theater mode gives the player the width, the killfeed goes under it", async ({ page }) => {
  await openPlayer(page);
  const theater = page.getByRole("button", { name: "Theater mode (T)" });
  const player = page.locator("media-controller");
  const stats = page.getByRole("heading", { name: "Your stats" });
  const before = await player.boundingBox();
  // Beside the player.
  expect((await stats.boundingBox())?.x).toBeGreaterThan((before?.x ?? 0) + (before?.width ?? 0));

  await theater.click();
  await expect(theater).toHaveAttribute("aria-pressed", "true");
  await expect
    .poll(async () => (await player.boundingBox())?.width)
    .toBeGreaterThan(before?.width ?? 0);
  // Under it.
  const wide = await player.boundingBox();
  expect((await stats.boundingBox())?.y).toBeGreaterThan((wide?.y ?? 0) + (wide?.height ?? 0));

  // T toggles it too.
  await page.evaluate(() => (document.activeElement as HTMLElement | null)?.blur());
  await press(page, "t");
  await expect(theater).toHaveAttribute("aria-pressed", "false");
});

test("F and the button take the player full screen, and back", async ({ page }) => {
  await openPlayer(page);
  const fullscreen = () => page.evaluate(() => document.fullscreenElement?.tagName ?? null);
  await press(page, "f");
  await expect.poll(fullscreen).toBe("MEDIA-CONTROLLER");
  await press(page, "f");
  await expect.poll(fullscreen).toBe(null);
});

test("while playing with the controls hidden, a thin line shows the time and your kills", async ({
  page,
}) => {
  await openPlayer(page);
  const glimpse = page.locator("media-controller > div[noautohide]");
  // Your two kills and your death; not the other player's kill.
  await expect(glimpse.locator("span.absolute")).toHaveCount(3);
  await expect(glimpse.locator("img")).toHaveCount(1);
  await expect(glimpse).toHaveCSS("opacity", "0");

  await seekTo(page, 20);
  // Paused, the line still moves with a seek.
  await expect(glimpse.locator("> div").first()).toHaveAttribute("style", /width: 50%/);

  await press(page, "k");
  await expect(page.locator("media-controller")).toHaveAttribute("userinactive", "", {
    timeout: 5_000,
  });
  await expect(glimpse).toHaveCSS("opacity", "0.9");
  await expect
    .poll(() =>
      glimpse
        .locator("> div")
        .first()
        .evaluate((d) => Number.parseFloat((d as HTMLElement).style.width)),
    )
    .toBeGreaterThan(50);

  await press(page, "k");
  await expect(glimpse).toHaveCSS("opacity", "0");
});

test("the kill list: a row jumps to its kill, and the one playing stays in view", async ({
  page,
}) => {
  // A kill every 1.25 s, all yours: more rows than the panel shows.
  const many: Kill[] = Array.from({ length: 30 }, (_, i) => ({
    t: 2 + i * 1.25,
    owner: "myKill",
    weapon: "ak47",
    modifiers: [],
  }));
  await openPlayer(page, done(many, { weapons: { ak47: 30 }, modifiers: {} }));
  const rows = page.locator('button[title^="Jump to"]');
  await expect(rows).toHaveCount(30);
  // All yours: no Yours / All switch.
  await expect(page.getByRole("button", { name: /^Yours/ })).toHaveCount(0);

  await rows.nth(2).click();
  await expectSeeks(page, [2.5]);
  await expect.poll(() => media(page).then((m) => m.paused)).toBe(false);

  const inBox = (i: number) =>
    rows.nth(i).evaluate((row) => {
      const box = row.parentElement?.getBoundingClientRect();
      const r = row.getBoundingClientRect();
      return !!box && r.top >= box.top - 1 && r.bottom <= box.bottom + 1;
    });
  expect(await inBox(29)).toBe(false);
  await seekTo(page, 38.5);
  await expect(rows.nth(29)).toHaveAttribute("aria-current", "true");
  await expect.poll(() => inBox(29)).toBe(true);
  expect(await inBox(0)).toBe(false);
  await seekTo(page, 2.5);
  await expect(rows.nth(0)).toHaveAttribute("aria-current", "true");
  await expect.poll(() => inBox(0)).toBe(true);
});

test("killfeed stats: no kills by you, one of each, and a gun without an icon", async ({
  page,
}) => {
  const theirs: Kill[] = [{ t: 5, owner: "other", weapon: "awp", modifiers: [] }];
  await openClip(page, clip, done(theirs, { weapons: {}, modifiers: {} }));
  await expect(
    page.getByText("No kills by you in this clip; the others are marked on the timeline."),
  ).toBeVisible();
  await expect(page.getByText("from the killfeed · 1 kill in the clip")).toBeVisible();
  // Their kill is still in the list, without "Your kill".
  await expect(page.locator('button[title^="Jump to"]')).toHaveCount(1);
  await expect(page.getByText("Your kill")).toHaveCount(0);
});

test("killfeed stats: nothing in the killfeed at all", async ({ page }) => {
  await openClip(page, clip, done([], { weapons: {}, modifiers: {} }));
  await expect(page.getByText("No kills by you in this clip.")).toBeVisible();
  await expect(page.getByText("from the killfeed · 0 kills in the clip")).toBeVisible();
  await expect(page.getByRole("heading", { name: "Kills" })).toHaveCount(0);
});

test("killfeed stats: singular counts, a multi-kill, and names where there's no icon", async ({
  page,
}) => {
  const kills: Kill[] = [
    { t: 5, owner: "myKill", weapon: "mystery_gun", modifiers: ["headshot", "lucky_shot"] },
    { t: 9, owner: "myDeath", weapon: "awp", modifiers: [] },
  ];
  await openClip(
    page,
    clip,
    done(kills, {
      multiKill: "2k",
      weapons: { mystery_gun: 1 },
      modifiers: { headshot: 1, lucky_shot: 1 },
    }),
  );
  const stats = page.locator("section", { has: page.getByRole("heading", { name: "Your stats" }) });
  await expect(stats.getByText("2k", { exact: true })).toBeVisible();
  for (const label of ["kill", "death", "headshot"]) {
    await expect(stats.getByText(label, { exact: true })).toBeVisible();
  }
  await expect(stats.getByTitle("mystery_gun × 1")).toHaveText("mystery_gun×1");
  await expect(stats.getByTitle("lucky shot × 1")).toHaveText("lucky shot×1");
  await expect(stats.getByRole("img", { name: "headshot" })).toBeVisible();
  // In the list: the names, and your death marked.
  const rows = page.locator('button[title^="Jump to"]');
  await expect(rows.first()).toHaveAttribute(
    "title",
    "Jump to 0:05: mystery_gun · headshot · lucky shot",
  );
  await expect(rows.nth(1)).toContainText("Your death");
});

test("the killfeed that can't be read leaves the player without marks", async ({ page }) => {
  await open(page, "about:blank");
  await page.route(`**/api/clips/${clip.id}`, (route) => json(route, clip));
  await page.route(`**/api/clips/${clip.id}/analysis`, (route) =>
    json(route, { error: "forbidden", message: "not yours" }, 403),
  );
  await page.goto(`/clips/${clip.id}`);
  await expect(page.getByRole("heading", { name: clip.title })).toBeVisible();
  await expect(page.getByRole("button", { name: "Next kill (])" })).toBeDisabled();
  await expect(page.getByRole("heading", { name: "Your stats" })).toHaveCount(0);
  await expect(page.getByText("Reading the killfeed…")).toHaveCount(0);
});

test("moving on to another clip from More clips", async ({ page }) => {
  await openPlayer(page);
  await expect(page.locator('button[title^="Jump to"]')).not.toHaveCount(0);
  await page.locator(`a[href="/clips/${clips.normal.id}"]`).click();
  await expect(page.getByRole("heading", { name: clips.normal.title })).toBeVisible();
  await expect(page.getByText("Reading the killfeed…")).toBeVisible();
  await expect(page.locator('button[title^="Jump to"]')).toHaveCount(0);
});

test("a clip page waits for its clip with a placeholder", async ({ page }) => {
  let answer = () => {};
  const answered = new Promise<void>((r) => {
    answer = r;
  });
  await open(page, "about:blank");
  await page.route(`**/api/clips/${clip.id}`, async (route) => {
    await answered;
    await json(route, clip);
  });
  await page.goto(`/clips/${clip.id}`);
  await expect(page.locator("main .animate-pulse")).toBeVisible();
  answer();
  await expect(page.getByRole("heading", { name: clip.title })).toBeVisible();
  await expect(page.locator("main .animate-pulse")).toHaveCount(0);
});

test("a clip that won't load says why, and Try again loads it", async ({ page }) => {
  await open(page, "about:blank");
  let down = true;
  await page.route(`**/api/clips/${clip.id}`, (route) =>
    down
      ? json(route, { error: "unavailable", message: "Back in a moment." }, 503)
      : json(route, clip),
  );
  await page.goto(`/clips/${clip.id}`);
  await expect(page.getByRole("alert")).toContainText("Back in a moment.");
  down = false;
  await page.getByRole("button", { name: "Try again" }).click();
  await expect(page.getByRole("heading", { name: clip.title })).toBeVisible();
});

test("a clip that's gone is the not-found page, with the way back", async ({ page }) => {
  await open(page, "/clips/10000000-0000-4000-8000-00000000dead");
  await expect(page.getByRole("heading", { name: "Nothing here." })).toBeVisible();
  await page.getByRole("link", { name: "Back to the archive" }).click();
  await expect(page.getByText(clips.normal.title).first()).toBeVisible();
});

test("a ready clip without a playback link: Try again asks for one", async ({ page }) => {
  await openClip(page, { ...clip, playbackUrl: null });
  await expect(page.getByText("It won't play right now.")).toBeVisible();
  await page.route(`**/api/clips/${clip.id}`, (route) => json(route, clip));
  await page.getByRole("button", { name: "Try again" }).click();
  await expect(page.locator("video")).toHaveCount(1);
  await expect(page.getByText("It won't play right now.")).toHaveCount(0);
});

test("edit: every field saves, and the page shows the new details", async ({ page }) => {
  // A map typed before the map chips existed.
  await openClip(page, { ...clip, map: "Cache", myPov: true });
  let sent: Record<string, unknown> | undefined;
  await page.route(`**/api/clips/${clip.id}`, (route) => {
    if (route.request().method() !== "PATCH") return route.fallback();
    const body: { players: string[] } = route.request().postDataJSON();
    sent = body;
    // The API answers with the players themselves.
    const players = clip.players.filter((p) => body.players.includes(p.id));
    return json(route, { ...clip, ...body, players });
  });
  await page.getByRole("button", { name: "Edit" }).click();
  const dialog = page.getByRole("dialog", { name: "Edit clip" });
  const chip = (name: string) => dialog.getByRole("button", { name, exact: true });
  await expect(chip("Cache")).toHaveAttribute("aria-pressed", "true");

  await dialog.getByLabel("Title").fill("Retake on A");
  await dialog.getByLabel("Description").fill("Through the smoke.");
  // Another map, then none, then that one again; the old one is gone once it's not chosen.
  await chip("Inferno").click();
  await expect(chip("Inferno")).toHaveAttribute("aria-pressed", "true");
  await expect(chip("Cache")).toHaveCount(0);
  await chip("Inferno").click();
  await expect(chip("Inferno")).toHaveAttribute("aria-pressed", "false");
  await chip("Inferno").click();

  // Suggestions complete the tag being typed, keeping the ones before it.
  const tags = dialog.getByLabel("Tags, comma-separated");
  await tags.fill("ace, 1v");
  await expect(page.locator("#edit-tags option")).toHaveCount(2);
  await expect(page.locator("#edit-tags option").last()).toHaveAttribute(
    "value",
    "ace, 1v3-clutch",
  );
  await tags.fill("ace, smoke-kill,");

  // Everyone's in the long clip: take Jamie out, and Maximilian out and back in.
  const jamie = dialog.getByRole("button", { name: "Jamie Doe" });
  await jamie.click();
  await expect(jamie).toHaveAttribute("aria-pressed", "false");
  const max = dialog.getByRole("button", { name: /Maximilian/ });
  await max.click();
  await max.click();
  await expect(max).toHaveAttribute("aria-pressed", "true");
  await dialog.getByText("Recorded from the uploader's point of view").click();

  await dialog.getByRole("button", { name: "Save" }).click();
  await expect(dialog).toBeHidden();
  expect(sent).toEqual({
    title: "Retake on A",
    description: "Through the smoke.",
    map: "Inferno",
    myPov: false,
    tags: ["ace", "smoke-kill"],
    players: [clips.long.players[0]?.id, clips.long.players[2]?.id],
  });
  await expect(page.getByRole("heading", { name: "Retake on A" })).toBeVisible();
  await expect(page.getByText("Through the smoke.")).toBeVisible();
});

test("edit: a save that fails says why and keeps what was typed", async ({ page }) => {
  await openClip(page);
  await page.route(`**/api/clips/${clip.id}`, (route) =>
    route.request().method() === "PATCH"
      ? json(route, { error: "invalid", message: "That title is taken." }, 400)
      : route.fallback(),
  );
  await page.getByRole("button", { name: "Edit" }).click();
  const dialog = page.getByRole("dialog", { name: "Edit clip" });
  await dialog.getByLabel("Title").fill("Mine now");
  await dialog.getByRole("button", { name: "Save" }).click();
  await expect(dialog.getByRole("alert")).toHaveText("That title is taken.");
  await expect(dialog.getByLabel("Title")).toHaveValue("Mine now");
});

test("share: make a link, copy it, stop sharing (after a second thought)", async ({
  page,
  context,
}) => {
  await context.grantPermissions(["clipboard-read", "clipboard-write"]);
  const url = "https://clips.example/s/new-link";
  await openClip(page, { ...clip, shareUrl: null });
  await page.route(`**/api/clips/${clip.id}/share`, (route) =>
    json(route, { ...clip, shareUrl: route.request().method() === "POST" ? url : null }),
  );
  await page.getByRole("button", { name: "Share", exact: true }).click();
  const dialog = page.getByRole("dialog", { name: "Share link" });
  await dialog.getByRole("button", { name: "Create share link" }).click();
  await expect(dialog.getByLabel("Share link")).toHaveValue(url);
  await expect(page.getByRole("button", { name: "Shared", exact: true })).toBeVisible();

  await dialog.getByRole("button", { name: "Copy" }).click();
  await expect(dialog.getByRole("button", { name: "Copied ✓" })).toBeVisible();
  expect(await page.evaluate(() => navigator.clipboard.readText())).toBe(url);
  // Back to Copy after a moment.
  await expect(dialog.getByRole("button", { name: "Copy" })).toBeVisible();

  const warning = dialog.getByText("The link stops working everywhere it was posted.");
  await dialog.getByRole("button", { name: "Stop sharing" }).click();
  await expect(warning).toBeVisible();
  await dialog.getByRole("button", { name: "Keep it" }).click();
  await expect(warning).toHaveCount(0);
  await expect(dialog.getByLabel("Share link")).toHaveValue(url);

  await dialog.getByRole("button", { name: "Stop sharing" }).click();
  await expect(warning).toBeVisible();
  await dialog.getByRole("button", { name: "Stop sharing" }).click();
  await expect(dialog.getByRole("button", { name: "Create share link" })).toBeVisible();
  await expect(page.getByRole("button", { name: "Share", exact: true })).toBeVisible();
});

test("share: a link that can't be made says why", async ({ page }) => {
  await openClip(page, { ...clip, shareUrl: null });
  await page.route(`**/api/clips/${clip.id}/share`, (route) =>
    json(route, { error: "server", message: "Sharing is down." }, 500),
  );
  await page.getByRole("button", { name: "Share", exact: true }).click();
  const dialog = page.getByRole("dialog", { name: "Share link" });
  await dialog.getByRole("button", { name: "Create share link" }).click();
  await expect(dialog.getByRole("alert")).toHaveText("Sharing is down.");
  await expect(dialog.getByRole("button", { name: "Create share link" })).toBeEnabled();
});

test("download: the original comes as a file, and a failed one says why", async ({ page }) => {
  await openClip(page);
  let ok = true;
  await page.route(`**/api/clips/${clip.id}/download`, (route) =>
    ok
      ? json(route, { url: "/e2e-download/original.mp4" })
      : json(route, { error: "server", message: "Storage is down." }, 500),
  );
  // Blob Storage sends the original as an attachment: the page stays.
  await page.route("**/e2e-download/**", (route) =>
    route.fulfill({
      contentType: "video/mp4",
      headers: { "content-disposition": 'attachment; filename="original.mp4"' },
      body: "mp4",
    }),
  );
  const button = page.getByRole("button", { name: "Download" });
  await expect(button).toHaveAttribute(
    "title",
    `The original: ${clip.originalFilename} · 328.7 MB`,
  );
  const download = page.waitForEvent("download");
  await button.click();
  expect((await download).suggestedFilename()).toBe("original.mp4");
  await expect(button).toBeEnabled();

  ok = false;
  await button.click();
  await expect(page.getByRole("alert")).toHaveText("Couldn't download it: Storage is down.");
  await expect(button).toBeEnabled();
});

test("delete moves the clip to the trash; Restore brings it back", async ({ page }) => {
  await openClip(page);
  const deletedAt = "2026-10-02T12:00:00Z";
  await page.route(`**/api/clips/${clip.id}`, (route) =>
    route.request().method() === "DELETE" ? json(route, { ...clip, deletedAt }) : route.fallback(),
  );
  let restores = 0;
  await page.route(`**/api/clips/${clip.id}/restore`, (route) =>
    ++restores === 1
      ? json(route, { error: "server", message: "Not now." }, 500)
      : json(route, clip),
  );
  await page.getByRole("button", { name: "More actions" }).click();
  await page.getByRole("menuitem", { name: "Delete" }).click();
  const dialog = page.getByRole("dialog", { name: "Delete this clip?" });
  await dialog.getByRole("button", { name: "Delete" }).click();
  await expect(dialog).toBeHidden();
  await expect(page.getByRole("status")).toHaveText(
    "Moved to the trash. You can restore it for 7 days.",
  );
  // In the trash: the banner with Restore, and nothing to react to or edit.
  await expect(page.getByText("In the trash. Only you can see it")).toBeVisible();
  await expect(page.getByRole("button", { name: "Edit" })).toHaveCount(0);
  await expect(page.getByRole("button", { name: /^🔥/ })).toHaveCount(0);

  await page.getByRole("button", { name: "Restore" }).click();
  await expect(page.getByRole("alert")).toHaveText("Couldn't restore it: Not now.");
  await page.getByRole("button", { name: "Restore" }).click();
  await expect(page.getByText("In the trash.")).toHaveCount(0);
  await expect(page.getByRole("button", { name: "Edit" })).toBeVisible();
});

test("a delete that fails says why and keeps the dialog open", async ({ page }) => {
  await openClip(page);
  await page.route(`**/api/clips/${clip.id}`, (route) =>
    route.request().method() === "DELETE"
      ? json(route, { error: "server", message: "Couldn't reach storage." }, 500)
      : route.fallback(),
  );
  await page.getByRole("button", { name: "More actions" }).click();
  await page.getByRole("menuitem", { name: "Delete" }).click();
  const dialog = page.getByRole("dialog", { name: "Delete this clip?" });
  await dialog.getByRole("button", { name: "Delete" }).click();
  await expect(dialog.getByRole("alert")).toHaveText("Couldn't reach storage.");
  await expect(dialog).toBeVisible();
});

test("the … menu: Home, End and the arrows move between its items; a click elsewhere closes it", async ({
  page,
}) => {
  await openClip(page);
  const more = page.getByRole("button", { name: "More actions" });
  const reanalyse = page.getByRole("menuitem", { name: "Re-analyse kill feed" });
  const del = page.getByRole("menuitem", { name: "Delete" });
  await more.click();
  await expect(more).toHaveAttribute("aria-expanded", "true");
  await expect(reanalyse).toBeFocused();
  // The arrows go round from either end.
  for (const [key, item] of [
    ["End", del],
    ["Home", reanalyse],
    ["ArrowUp", del],
    ["ArrowDown", reanalyse],
    ["ArrowDown", del],
  ] as const) {
    await page.keyboard.press(key);
    await expect(item).toBeFocused();
  }
  // Any other key is left alone.
  await page.keyboard.press("a");
  await expect(del).toBeFocused();
  await page.getByRole("heading", { name: clip.title }).click();
  await expect(page.getByRole("menu")).toHaveCount(0);
  await expect(more).toHaveAttribute("aria-expanded", "false");
  await expect(more).not.toBeFocused();
});

test("an admin re-analyses the kill feed: the panel waits for the new read", async ({ page }) => {
  await openClip(page);
  await expect(page.getByText("Your stats")).toBeVisible();
  // Pending from the moment the job is queued.
  await page.route(`**/api/clips/${clip.id}/analysis`, (route) =>
    json(route, { status: "pending", stats: null, kills: [] }),
  );
  const posted = page.waitForRequest(
    (r) => r.method() === "POST" && r.url().endsWith(`/api/admin/clips/${clip.id}/analyse`),
  );
  await page.getByRole("button", { name: "More actions" }).click();
  await page.getByRole("menuitem", { name: "Re-analyse kill feed" }).click();
  await posted;
  await expect(page.getByRole("status")).toHaveText(
    "Re-analysing — the kill feed updates in a minute or two.",
  );
  await expect(page.getByText("Reading the killfeed…")).toBeVisible();
  await expect(page.getByText("Your stats")).toHaveCount(0);
  await expect(page.getByRole("button", { name: "More actions" })).toBeFocused();
});

test("a re-analyse that's refused says why and leaves the kill feed as it was", async ({
  page,
}) => {
  await openClip(page);
  await page.route(`**/api/admin/clips/${clip.id}/analyse`, (route) =>
    json(route, { error: "conflict", message: "only published clips can be analysed" }, 409),
  );
  await page.getByRole("button", { name: "More actions" }).click();
  await page.getByRole("menuitem", { name: "Re-analyse kill feed" }).click();
  await expect(page.getByRole("alert")).toHaveText(
    "Couldn't re-analyse it: only published clips can be analysed",
  );
  await expect(page.getByText("Your stats")).toBeVisible();
});

test("a member's … menu has only Delete, and the keys stay on it", async ({ page }) => {
  // A member who can edit the clip (as its uploader would): Delete, but no re-analyse.
  await openClip(page, clip, done(KILLS), "member");
  const more = page.getByRole("button", { name: "More actions" });
  const item = page.getByRole("menuitem", { name: "Delete" });
  await more.click();
  await expect(item).toBeFocused();
  await expect(page.getByRole("menuitem")).toHaveCount(1);
  for (const key of ["End", "Home", "ArrowUp", "ArrowDown"]) {
    await page.keyboard.press(key);
    await expect(item).toBeFocused();
  }
});

test("reacting adds yours to the count", async ({ page }) => {
  await openClip(page);
  await page.route("**/api/clips/*/reactions/**", (route) =>
    json(route, [...clip.reactions, { emoji: "😂", count: 1, mine: true }]),
  );
  const laugh = page.getByRole("button", { name: /^😂/ });
  await expect(laugh).toHaveAccessibleName("😂 0");
  await laugh.click();
  await expect(laugh).toHaveAccessibleName("😂 1");
  await expect(laugh).toHaveAttribute("aria-pressed", "true");
  await expect(laugh).toHaveAttribute("aria-busy", "false");
});

test("a failed upload of yours: Try again, which can fail too", async ({ page }) => {
  const failed: Clip = { ...clips.failed, failureReason: "server", error: "worker crashed" };
  await openClip(page, failed);
  await expect(page.getByRole("heading", { name: "Something broke on our side." })).toBeVisible();
  let tries = 0;
  await page.route(`**/api/clips/${failed.id}/retry`, (route) =>
    ++tries === 1
      ? json(route, { error: "server", message: "Still broken." }, 500)
      : json(route, { ...clips.processing, id: failed.id }),
  );
  await page.getByRole("button", { name: "Try again" }).click();
  await expect(page.getByRole("alert")).toHaveText("Couldn't try again: Still broken.");
  await page.getByRole("button", { name: "Try again" }).click();
  await expect(page.getByRole("heading", { name: "Kip's getting it ready" })).toBeVisible();
});

for (const [reason, headline] of [
  ["notAVideo", "No video in that file."],
  ["unreadable", "Kip can't read that file."],
] as const) {
  test(`a failed upload (${reason}) says why, offers another, and can be deleted`, async ({
    page,
  }) => {
    await openClip(page, { ...clips.failed, failureReason: reason });
    await expect(page.getByRole("heading", { name: headline })).toBeVisible();
    await expect(page.getByRole("link", { name: "Upload another file" })).toHaveAttribute(
      "href",
      "/upload",
    );
    await page.getByRole("button", { name: "Delete", exact: true }).click();
    await expect(page.getByRole("dialog", { name: "Delete this clip?" })).toBeVisible();
  });
}

test("a failed upload that's a copy points to the clip that has it", async ({ page }) => {
  await openClip(page, {
    ...clips.failed,
    failureReason: "duplicate",
    duplicateOf: clips.long.id,
  });
  await expect(page.getByRole("heading", { name: "It's here already." })).toBeVisible();
  await expect(page.getByRole("link", { name: "Open that one" })).toHaveAttribute(
    "href",
    `/clips/${clips.long.id}`,
  );
  await expect(page.getByRole("button", { name: "Try again" })).toHaveCount(0);
  await expect(page.getByRole("button", { name: "Delete", exact: true })).toBeVisible();

  // One you can't open: only why.
  await openClip(page, { ...clips.failed, failureReason: "duplicate", duplicateOf: null });
  await expect(page.getByRole("heading", { name: "It's here already." })).toBeVisible();
  await expect(page.getByRole("link", { name: "Open that one" })).toHaveCount(0);
});

test("someone else's failed upload: nothing to retry, only the original", async ({ page }) => {
  await openClip(page, { ...clips.failed, isMine: false, canEdit: false, failureReason: null });
  await expect(page.getByRole("heading", { name: "Something broke on our side." })).toBeVisible();
  await expect(page.getByRole("button", { name: "Try again" })).toHaveCount(0);
  await expect(page.getByRole("button", { name: "Delete", exact: true })).toHaveCount(0);
  await expect(page.getByRole("button", { name: "Download original" })).toBeVisible();
});

test("a processing clip from another game skips the killfeed step", async ({ page }) => {
  await openClip(page, { ...clips.processing, gameId: "valorant", canEdit: false });
  await expect(page.getByText("Making it play everywhere (now)")).toBeVisible();
  await expect(page.getByText("Reading the killfeed")).toHaveCount(0);
  await expect(page.getByRole("button", { name: "Delete", exact: true })).toHaveCount(0);
});

test("the archive: a page of clips that fails to load, then Try again", async ({ page }) => {
  await open(page, "about:blank");
  let down = true;
  await page.route(
    (url) => url.pathname === "/api/clips",
    (route) =>
      down
        ? json(route, { error: "unavailable", message: "Back in a moment." }, 503)
        : route.fallback(),
  );
  await page.goto("/");
  await expect(page.getByRole("alert")).toContainText("Back in a moment.");
  down = false;
  await page.getByRole("button", { name: "Try again" }).click();
  await expect(page.getByText(clips.normal.title).first()).toBeVisible();
});

test("the archive: Show older clips loads the next page; a teaser isn't a link", async ({
  page,
}) => {
  await open(page, "about:blank");
  // A browser that never says the bottom is near: the button is the way on.
  await page.addInitScript(() => {
    window.IntersectionObserver = class {
      observe() {}
      unobserve() {}
      disconnect() {}
      takeRecords() {
        return [];
      }
    } as unknown as typeof IntersectionObserver;
  });
  const teaser: Clip = {
    ...clips.held,
    title: "Their clip for Friday",
    isMine: false,
    canEdit: false,
    teaser: true,
    uploader: clips.long.uploader,
  };
  await page.route(
    (url) => url.pathname === "/api/clips",
    (route) =>
      new URL(route.request().url()).searchParams.get("cursor") === "page-2"
        ? json(route, { clips: [teaser], nextCursor: null })
        : json(route, { clips: [clips.normal], nextCursor: "page-2" }),
  );
  await page.goto("/");
  await expect(page.getByText(clips.normal.title).first()).toBeVisible();
  await page.getByRole("button", { name: "Show older clips" }).click();
  await expect(page.getByText(teaser.title)).toBeVisible();
  await expect(page.getByRole("button", { name: "Show older clips" })).toHaveCount(0);
  // Not a link until it plays in a show.
  await expect(page.locator("main a", { hasText: teaser.title })).toHaveCount(0);
  await expect(page.getByText("Saved for the show")).toBeVisible();
});
