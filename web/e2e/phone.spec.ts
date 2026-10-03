// Phone regression tests: every page, on an iPhone (WebKit) and an Android phone
// (Chromium), with the API mocked. See playwright.config.ts.

import { clips } from "./fixtures";
import { layoutProblems } from "./layout";
import { cspViolations, open, pages, signOuts } from "./pages";
import { expect, test } from "./test";

for (const p of pages) {
  test(`${p.name} fits the screen`, async ({ page }, testInfo) => {
    await open(page, p.path, p.signedOut, p.meError);
    await expect(page.locator(p.ready.split(", ")[0] as string).first()).toBeVisible();
    // Let images and fonts settle before measuring.
    await page.waitForLoadState("networkidle");

    await testInfo.attach(`${p.name}.png`, {
      body: await page.screenshot({ fullPage: true }),
      contentType: "image/png",
    });
    expect(await layoutProblems(page)).toEqual([]);
    expect(await cspViolations(page)).toEqual([]);
  });
}

test("clip page actions sit below the title on phones", async ({ page }) => {
  await open(page, `/clips/${clips.long.id}`);
  const title = page.locator("h1");
  const download = page.getByRole("button", { name: "Download" });
  await expect(title).toBeVisible();
  const t = await title.boundingBox();
  const d = await download.boundingBox();
  expect(t && d && d.y >= t.y + t.height).toBe(true);
});

test("sign out is reachable from your profile on phones", async ({ page }) => {
  await open(page, "/u/robin");
  await page.getByRole("button", { name: "Sign out" }).click();
  expect(await signOuts(page)).toHaveLength(1);
});

test("clip page shows the killfeed: stats, your kills first, auto tags", async ({ page }) => {
  await open(page, `/clips/${clips.long.id}`);
  await expect(page.getByText("Your stats")).toBeVisible();
  const rows = page.locator('button[title^="Jump to"]');
  await expect(page.getByRole("button", { name: "Yours 6" })).toHaveAttribute(
    "aria-pressed",
    "true",
  );
  await expect(rows).toHaveCount(6);
  await page.getByRole("button", { name: "All 9" }).click();
  await expect(rows).toHaveCount(9);
  await expect(page.locator('button[title^="Jump to 2:30"]')).toContainText("Your death");
  const ace = page.locator('a[title="Detected from the killfeed"]', { hasText: "#ace" });
  await expect(ace).toBeVisible();
});

test("feed cards show the multi-kill", async ({ page }) => {
  await open(page, "/");
  const card = page.locator(`a[href="/clips/${clips.long.id}"]`).first();
  await expect(card.getByText("ace", { exact: true })).toBeVisible();
});

test("upload with the details form fits the screen", async ({ page }) => {
  await open(page, "/upload");
  await page.locator("input[type=file]").setInputFiles({
    name: "MedalTVCounterStrike220250408185133.mp4",
    mimeType: "video/mp4",
    buffer: Buffer.alloc(64 * 1024),
  });
  await expect(page.getByText(/Uploaded ✓/)).toBeVisible();
  expect(await layoutProblems(page)).toEqual([]);
  expect(await cspViolations(page)).toEqual([]);
});

test("awkward cards fit the screen: one long word, a Hebrew title, every reaction", async ({
  page,
}) => {
  await open(page, "about:blank");
  const list = [
    { ...clips.normal, id: "30000000-0000-4000-8000-000000000001", title: "W".repeat(100) },
    {
      ...clips.normal,
      id: "30000000-0000-4000-8000-000000000002",
      title: "הקליפ הכי טוב שלי באינפרנו עם AWP ושלוש הריגות",
      reactions: ["🔥", "😂", "💀", "🐐", "😮", "👏"].map((emoji, i) => ({
        emoji,
        count: 1000 + i,
        mine: i === 1,
      })),
    },
  ];
  await page.route(
    (url) => url.pathname === "/api/clips",
    (route) => route.fulfill({ json: { clips: list, nextCursor: null } }),
  );
  await page.goto("/?uploader=maximilian-the-awper&tag=a-very-long-tag-name-that-goes-on");
  // Right-to-left titles truncate at their own end.
  await expect(page.getByText(list[1]?.title ?? "")).toHaveAttribute("dir", "auto");
  await page.waitForLoadState("networkidle");
  expect(await layoutProblems(page)).toEqual([]);
});

test("someone else's clip saved for the show fits the screen as a teaser", async ({ page }) => {
  await open(page, "about:blank");
  await page.route(`**/api/clips/${clips.held.id}`, (route) =>
    route.fulfill({
      json: {
        ...clips.held,
        isMine: false,
        canEdit: false,
        teaser: true,
        uploader: { handle: "maximilian-the-awper", displayName: "Maximilian", avatarUrl: null },
      },
    }),
  );
  await page.goto(`/clips/${clips.held.id}`);
  await expect(page.getByText("Saved for the show", { exact: true })).toBeVisible();
  await page.waitForLoadState("networkidle");
  expect(await layoutProblems(page)).toEqual([]);
});

test("with /api/me down, the banner fits and Sign out is on screen", async ({ page }) => {
  await open(page, "about:blank");
  await page.route(/\/api\/me$/, (route) =>
    route.fulfill({ status: 503, json: { error: "server", message: "unavailable" } }),
  );
  await page.goto("/");
  const banner = page.getByRole("alert").filter({ hasText: "Couldn't load your account" });
  await expect(banner).toBeVisible({ timeout: 15_000 });
  await expect(banner.getByRole("button", { name: "Sign out" })).toBeInViewport();
  expect(await layoutProblems(page)).toEqual([]);
  await banner.getByRole("button", { name: "Sign out" }).click();
  expect(await signOuts(page)).toEqual([
    { logoutParams: { returnTo: new URL(page.url()).origin } },
  ]);
});
