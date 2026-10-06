// Past shows in the archive, their replay, and what the shows leave on clips and profiles
// (S7, canvas 4.1–4.3, 3.3).
import type { Page, Route } from "@playwright/test";
import type { components } from "../src/api/schema";
import { clips, me, pastShow, show, showClip, showClip2 } from "./fixtures";
import { layoutProblems } from "./layout";
import { cspViolations, open } from "./pages";
import { expect, test } from "./test";

type ShowView = components["schemas"]["ShowView"];

const json = (route: Route, body: unknown, status = 200) =>
  route.fulfill({ status, contentType: "application/json", body: JSON.stringify(body) });

/** The past show as one show (its replay): the two show clips played, reactions on the
 *  first at 0.5 s and 1 s. */
const replay: ShowView = {
  ...show,
  id: pastShow.id,
  status: "ended",
  startedAt: pastShow.startedAt,
  endedAt: pastShow.endedAt,
  lineup: [showClip, showClip2].map((clip, position) => ({
    clip,
    position,
    dropped: false,
    spare: false,
    playedAt: pastShow.endedAt,
    addedBy: me.id,
  })),
  clipWinnerId: showClip2.id,
  failWinnerId: showClip.id,
  reactions: [
    { clipId: showClip.id, userId: me.id, emoji: "🔥", atMs: 500 },
    { clipId: showClip.id, userId: me.id, emoji: "😂", atMs: 1_000 },
    { clipId: showClip2.id, userId: me.id, emoji: "💀", atMs: 500 },
  ],
};

const withShow = (body: ShowView) => (page: Page) =>
  page.route(`**/api/shows/${pastShow.id}`, (route) => json(route, body));

test("the archive starts with past shows: winners, who watched, the replay", async ({
  page,
}, testInfo) => {
  await open(page, "/");
  const shows = page.getByRole("region", { name: "Past shows" });
  await expect(shows.getByRole("heading", { name: "Past shows" })).toBeVisible();
  await expect(shows).toContainText("1 show · 2 clips · since Sep 25");
  const card = shows.getByRole("article");
  await expect(card).toContainText("Last show");
  await expect(card).toContainText("2 clips · 3:30 · hosted by Robin");
  await expect(card).toContainText("3 watched");
  await expect(card.getByRole("link", { name: /Clip of the night/ })).toHaveAttribute(
    "href",
    `/clips/${clips.long.id}`,
  );
  await expect(card).toContainText("2 of 3 votes");
  await expect(card.getByRole("link", { name: /Fail of the night/ })).toContainText(
    clips.normal.title,
  );
  await expect(card.getByRole("link", { name: "Watch the replay" })).toHaveAttribute(
    "href",
    `/shows/${pastShow.id}/replay`,
  );
  // Whose clips won: one trophy each.
  const board = page.getByTestId("trophy-board");
  await expect(board.getByRole("link", { name: "Jamie Doe: 1 trophy" })).toBeVisible();
  await expect(board.getByRole("link", { name: "Robin: 1 trophy" })).toBeVisible();

  // The winners' cards carry their badges.
  const grid = page
    .getByRole("main")
    .getByRole("link", { name: /MedalTVCounterStrike/ })
    .last();
  await expect(grid).toContainText("Clip of the night");
  await testInfo.attach("archive.png", {
    body: await page.screenshot({ fullPage: true }),
    contentType: "image/png",
  });
  expect(await layoutProblems(page)).toEqual([]);
  expect(await cspViolations(page)).toEqual([]);
});

test("a show that played a lot shows five more of them and counts the rest", async ({ page }) => {
  const more = Array.from({ length: 6 }, (_, i) => ({
    ...clips.normal,
    id: `10000000-0000-4000-8000-0000000001${String(i).padStart(2, "0")}`,
    title: `Also ${i + 1}`,
    failOfTheNight: false,
  }));
  const big = { ...pastShow, clips: [...pastShow.clips, ...more] };
  await open(page, "/", false, undefined, (p) =>
    p.route("**/api/shows", (route) => json(route, [big])),
  );
  const card = page.getByRole("region", { name: "Past shows" }).getByRole("article");
  await expect(card).toContainText("8 clips");
  await expect(card.getByRole("link", { name: /^Also \d$/ })).toHaveCount(5);
  await expect(card.getByRole("link", { name: "Also 6" })).toHaveCount(0);
  await expect(card).toContainText("and 1 more clip");
  expect(await layoutProblems(page)).toEqual([]);
});

test("Clips of the night and Fails filter the archive", async ({ page }) => {
  await open(page, "/");
  const asked = (night: string) =>
    page.waitForRequest((r) => new URL(r.url()).searchParams.get("night") === night);
  const clip = asked("clip");
  await page.getByRole("button", { name: "Clips of the night" }).click();
  await clip;
  await expect(page).toHaveURL(/night=clip/);
  const fail = asked("fail");
  await page.getByRole("button", { name: "Fails" }).click();
  await fail;
  await expect(page.getByRole("button", { name: "Fails" })).toHaveAttribute("aria-pressed", "true");
  await page.getByRole("button", { name: "All", exact: true }).click();
  await expect(page).not.toHaveURL(/night=/);
});

test("before the show is open to you: no past shows, no show filters", async ({ page }) => {
  await open(page, "/", false, undefined, (p) =>
    p.route("**/api/me", (route) =>
      json(route, { ...me, role: "member", shows: false, showsForEveryone: false }),
    ),
  );
  await expect(page.getByRole("heading", { name: "Archive" })).toBeVisible();
  await expect(page.getByRole("region", { name: "Past shows" })).toHaveCount(0);
  await expect(page.getByRole("button", { name: "Clips of the night" })).toHaveCount(0);
});

test("no shows yet: the archive says where they'll be", async ({ page }) => {
  await open(page, "/", false, undefined, (p) =>
    p.route("**/api/shows", (route) => json(route, [])),
  );
  await expect(page.getByRole("heading", { name: "No shows yet" })).toBeVisible();
  await expect(page.getByRole("link", { name: "Tonight" }).last()).toHaveAttribute(
    "href",
    "/tonight",
  );
});

test("a profile shows the shows: hosted, clips and fails of the night, the trophy shelf", async ({
  page,
}) => {
  await open(page, "/u/robin");
  await expect(page.getByText("show hosted")).toBeVisible();
  const shelf = page.getByRole("region", { name: "Trophy shelf" });
  const fail = shelf.getByRole("link", { name: /Fail of the night/ });
  await expect(fail).toContainText(clips.normal.title);
  await expect(fail).toContainText("Friday night show, Sep 25");
  await expect(fail).toHaveAttribute("href", `/clips/${clips.normal.id}`);
  await expect(page.getByText("fails of the night")).toBeVisible();

  // Someone with no trophies on a friend's page: no shelf.
  await open(page, "/u/maximilian-the-awper");
  await expect(page.getByRole("button", { name: /^Featured in/ })).toBeVisible();
  await expect(page.getByRole("region", { name: "Trophy shelf" })).toHaveCount(0);
});

test("the clip page says where it played and how it did", async ({ page }) => {
  await open(page, `/clips/${clips.normal.id}`);
  const played = page.getByTestId("played-in");
  await expect(played).toContainText("Played at Friday night show, Sep 25");
  await expect(played).toContainText(
    "Clip 2 of 2 · fail of the night · lost the vote to Jamie Doe's MedalTVCounterStrike220250408185133",
  );
  await expect(played.getByRole("link", { name: "Watch the show" })).toHaveAttribute(
    "href",
    `/shows/${pastShow.id}/replay`,
  );

  await open(page, `/clips/${clips.long.id}`);
  await expect(page.getByTestId("played-in")).toContainText("Clip 1 of 2 · clip of the night");
});

test("the replay plays the show's clips in order, with its reactions at their moments", async ({
  page,
}, testInfo) => {
  test.setTimeout(30_000);
  await open(page, `/shows/${pastShow.id}/replay`, false, undefined, withShow(replay));
  await expect(page.getByRole("heading", { name: "Friday night show" })).toBeVisible();
  const list = page.getByRole("complementary", { name: "The show's clips" });
  await expect(list.getByRole("button", { name: /Show clip/ })).toHaveAttribute(
    "aria-current",
    "true",
  );
  await expect(page.getByTestId("replay-reactions")).toHaveText("🔥1😂1");
  // Muted, so it plays without a click; the taps float as it passes them.
  const video = page.getByTestId("replay-video");
  await video.evaluate((v: HTMLVideoElement) => {
    v.muted = true;
    return v.play();
  });
  await expect(page.getByTestId("floats")).toContainText("🔥", { timeout: 5_000 });
  await expect(page.getByTestId("floats")).toContainText("😂", { timeout: 5_000 });
  await testInfo.attach("replay.png", {
    body: await page.screenshot({ fullPage: true }),
    contentType: "image/png",
  });
  expect(await layoutProblems(page)).toEqual([]);
  expect(await cspViolations(page)).toEqual([]);

  // The next one, from the list.
  await list.getByRole("button", { name: /Second show clip/ }).click();
  await expect.poll(() => video.evaluate((v: HTMLVideoElement) => v.currentSrc)).toMatch(/clip=2$/);
  await expect(page.getByTestId("replay-reactions")).toHaveText("💀1");
  await expect(page.getByRole("link", { name: "The winners" })).toHaveAttribute(
    "href",
    `/shows/${pastShow.id}`,
  );
});

test("the last clip ends the replay: from the start, or the winners", async ({ page }) => {
  const one = { ...replay, lineup: replay.lineup.slice(0, 1) };
  await open(page, `/shows/${pastShow.id}/replay`, false, undefined, withShow(one));
  const video = page.getByTestId("replay-video");
  await expect(video).toHaveAttribute("src", /show\.webm/);
  await video.evaluate((v: HTMLVideoElement) => v.dispatchEvent(new Event("ended")));
  await expect(page.getByText("That was the show.")).toBeVisible();
  await page.getByRole("button", { name: "From the start" }).click();
  await expect(page.getByText("That was the show.")).toHaveCount(0);
});

test("an abandoned or running show has no replay", async ({ page }) => {
  await open(
    page,
    `/shows/${pastShow.id}/replay`,
    false,
    undefined,
    withShow({ ...replay, status: "abandoned" }),
  );
  await expect(page.getByRole("heading", { name: "No replay for this show" })).toBeVisible();
  await expect(page.getByText("Everyone left before the finale")).toBeVisible();

  await open(
    page,
    `/shows/${pastShow.id}/replay`,
    false,
    undefined,
    withShow({ ...replay, status: "live" }),
  );
  await expect(page.getByRole("link", { name: "Join the show" })).toHaveAttribute(
    "href",
    `/shows/${pastShow.id}`,
  );
});
