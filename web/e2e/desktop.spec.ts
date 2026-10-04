// Desktop regression tests: every page in a 1440 × 900 Chrome window, API mocked. Epic 1
// of the redesign is designed for PC, so this is where its screens get checked.
import type { Page } from "@playwright/test";
import { clips } from "./fixtures";
import { layoutProblems } from "./layout";
import { cspViolations, open, pages } from "./pages";
import { expect, test } from "./test";

/** A file for the upload page's picker. */
const video = (name: string) => ({
  name,
  mimeType: "video/mp4",
  buffer: Buffer.alloc(64 * 1024),
});

for (const p of pages) {
  test(`${p.name} lays out on a desktop`, async ({ page }, testInfo) => {
    await open(page, p.path, p.signedOut, p.meError);
    await expect(page.locator(p.ready).first()).toBeVisible();
    await page.waitForLoadState("networkidle");
    await testInfo.attach(`${p.name}.png`, {
      body: await page.screenshot({ fullPage: true }),
      contentType: "image/png",
    });
    expect(await layoutProblems(page)).toEqual([]);
    expect(await cspViolations(page)).toEqual([]);
  });
}

test("the top bar: Kip, Archive, Upload, your avatar; Admin is on your profile", async ({
  page,
}) => {
  await open(page, "/");
  const nav = page.getByRole("navigation");
  await expect(nav.getByRole("link", { name: "Archive" })).toHaveAttribute("aria-current", "page");
  await expect(nav.getByRole("link", { name: "Upload" })).toBeVisible();
  await expect(nav.getByRole("link", { name: "Admin" })).toHaveCount(0);
  await expect(page.getByText("isn't affiliated with or endorsed by Valve")).toBeVisible();
  await page.getByRole("link", { name: "Your profile" }).click();
  await page.getByRole("link", { name: "Invites and members" }).click();
  await expect(page.getByRole("heading", { name: "Admin" })).toBeVisible();
});

test("edit profile opens over your profile and closes", async ({ page }) => {
  await open(page, "/u/robin");
  await page.getByRole("link", { name: "Edit profile" }).click();
  const dialog = page.getByRole("dialog", { name: "Edit profile" });
  await expect(dialog.getByLabel("Display name")).toHaveValue("Robin");
  await dialog.getByRole("button", { name: "Cancel" }).click();
  await expect(dialog).toBeHidden();
  expect(new URL(page.url()).searchParams.get("edit")).toBeNull();
});

test("edit profile keeps the tab you were on, however it's closed", async ({ page }) => {
  await open(page, "/u/robin?tab=featured");
  const dialog = page.getByRole("dialog", { name: "Edit profile" });
  for (const close of ["Close", "Cancel", "Escape"]) {
    await page.getByRole("link", { name: "Edit profile" }).click();
    await expect(dialog).toBeVisible();
    if (close === "Escape") await page.keyboard.press("Escape");
    else await dialog.getByRole("button", { name: close }).click();
    await expect(dialog).toBeHidden();
    await expect(page).not.toHaveURL(/edit=/);
    await expect(page).toHaveURL(/\?tab=featured$/);
  }
});

test("an unknown clip or user is the not-found page", async ({ page }) => {
  await open(page, "/clips/10000000-0000-4000-8000-00000000dead");
  await expect(page.getByRole("heading", { name: "Nothing here." })).toBeVisible();
  await page.goto("/u/nobody-at-all");
  await expect(page.getByRole("heading", { name: "Nothing here." })).toBeVisible();
});

test("the page uses the bundled fonts", async ({ page }) => {
  await open(page, "/");
  await expect(page.getByText(clips.normal.title).first()).toBeVisible();
  // load() resolves with the matching faces from the app's own CSS; none = not bundled.
  const loaded = await page.evaluate(async () => ({
    saira: (await document.fonts.load('600 16px "Saira Semi Condensed"')).length > 0,
    mono: (await document.fonts.load('400 12px "Martian Mono"')).length > 0,
  }));
  expect(loaded.saira).toBe(true);
  expect(loaded.mono).toBe(true);
});

test("archive filters go to the URL and the API", async ({ page }) => {
  await open(page, "/");
  await expect(page.getByText(clips.normal.title).first()).toBeVisible();
  const asked = (param: string, value: string) =>
    page.waitForRequest((r) => {
      const url = new URL(r.url());
      return url.pathname === "/api/clips" && url.searchParams.get(param) === value;
    });

  let request = asked("reaction", "🔥");
  await page.getByRole("button", { name: "🔥 Fire" }).click();
  await request;
  await expect(page.getByRole("button", { name: "🔥 Fire" })).toHaveAttribute(
    "aria-pressed",
    "true",
  );
  expect(new URL(page.url()).searchParams.get("reaction")).toBe("🔥");

  request = asked("tag", "4k,ace");
  await page.getByRole("button", { name: "4K and aces" }).click();
  await request;
  expect(new URL(page.url()).searchParams.get("reaction")).toBeNull();

  request = asked("q", "inferno");
  await page.getByRole("searchbox").fill("inferno");
  await request;
  expect(new URL(page.url()).searchParams.get("q")).toBe("inferno");

  // A search is a filter too: All isn't pressed, and pressing it clears the search.
  const all = page.getByRole("button", { name: "All", exact: true });
  await expect(all).toHaveAttribute("aria-pressed", "false");
  await all.click();
  await expect(all).toHaveAttribute("aria-pressed", "true");
  await expect(page.getByRole("searchbox")).toHaveValue("");
  expect(new URL(page.url()).searchParams.get("q")).toBeNull();
});

test("archive: sort and map", async ({ page }) => {
  await open(page, "/");
  await expect(page.getByText(clips.normal.title).first()).toBeVisible();
  const asked = (param: string, value: string) =>
    page.waitForRequest((r) => {
      const url = new URL(r.url());
      return url.pathname === "/api/clips" && url.searchParams.get(param) === value;
    });

  let request = asked("sort", "top");
  await page.getByRole("button", { name: "Top", exact: true }).click();
  await request;
  await expect(page.getByText("Every clip, most reactions first")).toBeVisible();
  await expect(page.getByRole("button", { name: "New", exact: true })).toHaveAttribute(
    "aria-pressed",
    "false",
  );

  request = asked("map", "Inferno");
  await page.getByLabel("Map").selectOption("Inferno");
  await request;
  expect(new URL(page.url()).searchParams.get("map")).toBe("Inferno");
  expect(new URL(page.url()).searchParams.get("sort")).toBe("top");
});

test("archive: All clears every filter but keeps the sort", async ({ page }) => {
  await open(page, "/?tag=ace&player=jamie&uploader=jamie&map=Mirage&reaction=🔥&q=ace&sort=top");
  await expect(page.getByRole("searchbox")).toHaveValue("ace");
  await expect(page.getByRole("button", { name: "Remove filter by @jamie" })).toBeVisible();
  const all = page.getByRole("button", { name: "All", exact: true });
  await expect(all).toHaveAttribute("aria-pressed", "false");
  await all.click();
  await expect(all).toHaveAttribute("aria-pressed", "true");
  await expect(page.getByRole("searchbox")).toHaveValue("");
  await expect(page.getByLabel("Map")).toHaveValue("");
  const params = new URL(page.url()).searchParams;
  expect([...params.keys()]).toEqual(["sort"]);
});

test("archive: the Archive link clears the search", async ({ page }) => {
  await open(page, "/");
  await page.getByRole("searchbox").fill("inferno");
  await expect(page).toHaveURL(/q=inferno/);
  await page.getByRole("navigation").getByRole("link", { name: "Archive" }).click();
  await expect(page.getByRole("searchbox")).toHaveValue("");
  // And it stays cleared: the box doesn't write its old words back.
  await page.waitForTimeout(600);
  expect(new URL(page.url()).searchParams.get("q")).toBeNull();
});

test("archive: an unknown reaction in the URL is dropped", async ({ page }) => {
  await open(page, "/?reaction=nope");
  const request = page.waitForRequest((r) => new URL(r.url()).pathname === "/api/clips");
  expect(new URL((await request).url()).searchParams.get("reaction")).toBeNull();
});

test("an empty archive offers to upload a clip", async ({ page }) => {
  await open(page, "/");
  await page.route(
    (url) => url.pathname === "/api/clips",
    (route) => route.fulfill({ json: { clips: [], nextCursor: null } }),
  );
  await page.reload();
  await page.getByRole("main").getByRole("link", { name: "Upload a clip" }).click();
  await expect(page).toHaveURL(/\/upload$/);
});

test("clip actions open over the page: share, edit, delete", async ({ page }) => {
  await open(page, `/clips/${clips.long.id}`);
  await page.getByRole("button", { name: "Shared" }).click();
  const share = page.getByRole("dialog", { name: "Share link" });
  await expect(share.getByLabel("Share link")).toHaveValue(/\/s\//);
  await share.getByRole("button", { name: "Close" }).click();
  await expect(share).toBeHidden();

  await page.getByRole("button", { name: "Edit" }).click();
  const edit = page.getByRole("dialog", { name: "Edit clip" });
  await expect(edit.getByRole("button", { name: "Mirage" })).toHaveAttribute(
    "aria-pressed",
    "true",
  );
  await edit.getByRole("button", { name: "Cancel" }).click();

  await page.getByRole("button", { name: "More actions" }).click();
  await page.getByRole("menuitem", { name: "Delete" }).click();
  const del = page.getByRole("dialog", { name: "Delete this clip?" });
  await expect(del.getByText("restore it for 7 days")).toBeVisible();
  await del.getByRole("button", { name: "Keep it" }).click();
  await expect(del).toBeHidden();
});

test("a failed clip says why and what to do", async ({ page }) => {
  await open(page, `/clips/${clips.failed.id}`);
  await expect(page.getByText("This one is 7:12.")).toBeVisible();
  await expect(page.getByRole("link", { name: "Upload a shorter one" })).toBeVisible();
  // The original is still there to take back to the recorder.
  const download = page.waitForRequest((r) =>
    r.url().endsWith(`/api/clips/${clips.failed.id}/download`),
  );
  await page.getByRole("button", { name: "Download original" }).click();
  await download;
});

test("a processing clip offers its original too", async ({ page }) => {
  await open(page, `/clips/${clips.processing.id}`);
  await expect(page.getByText("Uploaded (done)")).toBeVisible();
  await expect(page.getByRole("button", { name: "Download original" })).toBeVisible();
});

test("upload starts on pick, details save beside it, then the clip opens", async ({ page }) => {
  await open(page, "/upload");
  const name = "Counter-strike 2 2026.10.02 - 21.14.08.02.mp4";
  await page.locator("input[type=file]").setInputFiles({
    name,
    mimeType: "video/mp4",
    buffer: Buffer.alloc(64 * 1024),
  });
  await expect(page.getByText(name)).toBeVisible();
  await expect(page.getByText(/Uploaded ✓/)).toBeVisible();
  await expect(page.getByLabel("Title")).toHaveValue("Counter-strike 2 · 2026-10-02 21:14");
  await page.getByRole("button", { name: "Mirage" }).click();
  const patch = page.waitForRequest((r) => r.method() === "PATCH");
  await page.getByRole("button", { name: "Save details" }).click();
  expect((await patch).postDataJSON()).toMatchObject({ map: "Mirage", myPov: true });
  await expect(page).toHaveURL(new RegExp(`/clips/${clips.processing.id}$`));
  await expect(page.getByText("Kip's getting it ready")).toBeVisible();
});

test("upload turns away files that aren't videos", async ({ page }) => {
  await open(page, "/upload");
  await page.locator("input[type=file]").setInputFiles({
    name: "notes.txt",
    mimeType: "text/plain",
    buffer: Buffer.from("not a clip"),
  });
  await expect(page.getByText("That's not a video file.")).toBeVisible();
  await expect(page.getByRole("button", { name: "Choose another file" })).toBeVisible();
});

test("a clip saved for the show: the notice, and Post now", async ({ page }) => {
  await open(page, `/clips/${clips.held.id}`);
  await expect(page.getByText("Nobody else sees it until it plays")).toBeVisible();
  const release = page.waitForRequest(
    (r) => r.method() === "POST" && r.url().endsWith(`/api/clips/${clips.held.id}/release`),
  );
  await page.getByRole("button", { name: "Post now" }).click();
  await release;
  await expect(page.getByText("Saved for the show", { exact: true })).toBeHidden();
});

test("upload: the Save it for the show switch holds and releases the clip at once", async ({
  page,
}) => {
  await open(page, "/upload");
  await page.locator("input[type=file]").setInputFiles(video("ace.mp4"));
  await expect(page.getByText(/Uploaded ✓/)).toBeVisible();
  // Off by default while the show is admin-only.
  const hold = page.getByRole("switch", { name: /Save it for the show/ });
  await expect(hold).not.toBeChecked();
  const posted = (action: string) =>
    page.waitForRequest((r) => r.method() === "POST" && r.url().endsWith(`/${action}`));
  let request = posted("hold");
  await page.getByText("Save it for the show").click();
  await request;
  request = posted("release");
  await page.getByText("Save it for the show").click();
  await request;
});

test("upload: Cancel, then another file uploads", async ({ page }) => {
  await open(page, "/upload");
  // Blob Storage hangs until let go, so the first upload is still running at Cancel.
  let release = () => {};
  const stuck = new Promise<void>((r) => {
    release = r;
  });
  await page.route("**/e2e-blob/**", async (route) => {
    await stuck;
    await route.fulfill({ status: 201, body: "" }).catch(() => {});
  });
  const uploading = page.waitForRequest("**/e2e-blob/**");
  await page.locator("input[type=file]").setInputFiles(video("first.mp4"));
  await uploading;
  // The clip exists by now: cancelling moves it to the trash.
  const deleted = page.waitForRequest(
    (r) => r.method() === "DELETE" && r.url().endsWith(`/api/clips/${clips.processing.id}`),
  );
  await page.getByRole("button", { name: "Cancel" }).click();
  await deleted;
  release();

  await page.locator("input[type=file]").setInputFiles(video("second.mp4"));
  await expect(page.getByText("second.mp4")).toBeVisible();
  await expect(page.getByText(/Uploaded ✓/)).toBeVisible();
});

test("upload: Cancel while the clip is being created sends nothing to Blob Storage", async ({
  page,
}) => {
  await open(page, "/upload");
  const blobs: string[] = [];
  page.on("request", (r) => {
    if (r.url().includes("/e2e-blob/")) blobs.push(r.url());
  });
  let release = () => {};
  const slow = new Promise<void>((r) => {
    release = r;
  });
  await page.route("**/api/clips", async (route) => {
    if (route.request().method() !== "POST") return route.fallback();
    await slow;
    await route.fallback().catch(() => {});
  });
  await page.locator("input[type=file]").setInputFiles(video("ace.mp4"));
  await expect(page.getByText("Starting…")).toBeVisible();
  await page.getByRole("button", { name: "Cancel" }).click();
  release();
  await expect(page.getByRole("button", { name: "Choose a video" })).toBeVisible();
  await page.waitForTimeout(1_000);
  expect(blobs).toEqual([]);
});

test("the kill list keeps its rows when two kills share a second and a gun", async ({ page }) => {
  await open(page, "about:blank");
  await page.route(/\/analysis$/, (route) =>
    route.fulfill({
      json: {
        status: "done",
        stats: {
          kills: 4,
          myKills: 2,
          myDeaths: 0,
          multiKill: null,
          weapons: { ak47: 2 },
          modifiers: { headshot: 1 },
        },
        kills: [
          { t: 5, owner: "other", weapon: "m4a1", modifiers: [] },
          { t: 12, owner: "myKill", weapon: "ak47", modifiers: [] },
          { t: 12, owner: "myKill", weapon: "ak47", modifiers: [] },
          { t: 20, owner: "other", weapon: "awp", modifiers: [] },
        ],
      },
    }),
  );
  await page.goto(`/clips/${clips.long.id}`);
  const rows = page.locator('button[title^="Jump to"]');
  await expect(rows).toHaveCount(2);
  for (let i = 0; i < 3; i++) {
    await page.getByRole("button", { name: "All 4" }).click();
    await expect(rows).toHaveCount(4);
    await page.getByRole("button", { name: "Yours 2" }).click();
    await expect(rows).toHaveCount(2);
  }
});

const signIns = (page: Page) =>
  page.evaluate(() => (window as unknown as { __e2eSignIns?: unknown[] }).__e2eSignIns ?? []);

test("a /api/me hiccup recovers on its own", async ({ page }) => {
  await open(page, "about:blank");
  let calls = 0;
  await page.route(/\/api\/me$/, (route) =>
    calls++ === 0
      ? route.fulfill({ status: 500, json: { error: "server", message: "internal server error" } })
      : route.fallback(),
  );
  await page.goto("/");
  await expect(page.getByRole("link", { name: "Your profile" })).toBeVisible();
  await expect(page.getByRole("alert")).toHaveCount(0);
  expect(calls).toBe(2);
});

test("/api/me down: the pages still work, with Try again and Sign out", async ({ page }) => {
  await open(page, "about:blank");
  let down = true;
  await page.route(/\/api\/me$/, (route) =>
    down
      ? route.fulfill({ status: 503, json: { error: "server", message: "unavailable" } })
      : route.fallback(),
  );
  await page.goto("/");
  await expect(page.getByText(clips.normal.title).first()).toBeVisible();
  const banner = page.getByRole("alert").filter({ hasText: "Couldn't load your account" });
  // After three more tries, 1, 2 and 4 s apart.
  await expect(banner).toBeVisible({ timeout: 15_000 });
  await expect(banner.getByRole("button", { name: "Sign out" })).toBeVisible();
  await page.getByRole("navigation").getByRole("link", { name: "Upload" }).click();
  await expect(page.getByRole("heading", { name: "Upload a clip" })).toBeVisible();
  down = false;
  await banner.getByRole("button", { name: "Try again" }).click();
  await expect(banner).toBeHidden();
  await expect(page.getByRole("link", { name: "Your profile" })).toBeVisible();
});

test("a 401 sends you through sign-in, back to the same page", async ({ page }) => {
  await open(page, "about:blank");
  await page.route(/\/api\/me$/, (route) =>
    route.fulfill({ status: 401, json: { error: "unauthorized", message: "expired token" } }),
  );
  await page.goto("/u/jamie?tab=featured");
  await expect.poll(() => signIns(page)).toHaveLength(1);
  expect(await signIns(page)).toEqual([
    expect.objectContaining({ appState: { returnTo: "/u/jamie?tab=featured" } }),
  ]);
});

test("a session that ran out signs in again", async ({ page }) => {
  await page.addInitScript(() => window.localStorage.setItem("e2e-session-gone", "1"));
  await open(page, "/upload");
  await expect.poll(() => signIns(page)).toHaveLength(1);
  expect(await signIns(page)).toEqual([
    expect.objectContaining({ appState: { returnTo: "/upload" } }),
  ]);
});

test("overlapping pages don't show a clip twice", async ({ page }) => {
  await open(page, "about:blank");
  const { long: a, normal: b, held: c } = clips;
  await page.route(
    (url) => url.pathname === "/api/clips",
    (route) => {
      // The second page starts with the first page's last clip again.
      const second = new URL(route.request().url()).searchParams.get("cursor") === "2";
      return route.fulfill({
        json: second ? { clips: [b, c], nextCursor: null } : { clips: [a, b], nextCursor: "2" },
      });
    },
  );
  await page.goto("/?sort=top");
  await expect(page.getByText(c?.title ?? "").first()).toBeVisible();
  await expect(page.getByTestId("clip-grid").locator('a[href^="/clips/"]')).toHaveCount(3);
});

test("player shortcuts wait while a dialog is open", async ({ page }) => {
  await open(page, `/clips/${clips.long.id}`);
  const muted = () => page.locator("video").evaluate((v: HTMLVideoElement) => v.muted);
  const theater = page.getByRole("button", { name: "Theater mode (T)" });
  await page.getByRole("button", { name: "Edit" }).click();
  const dialog = page.getByRole("dialog", { name: "Edit clip" });
  await dialog.getByRole("button", { name: "Mirage" }).focus();
  await page.keyboard.press("m");
  await page.keyboard.press("t");
  expect(await muted()).toBe(false);
  await expect(theater).toHaveAttribute("aria-pressed", "false");
  // Closed, they work again.
  await page.keyboard.press("Escape");
  await expect(dialog).toBeHidden();
  await page.keyboard.press("m");
  expect(await muted()).toBe(true);
});

test("a clip still uploading updates by itself, and can be deleted", async ({ page }) => {
  await open(page, "about:blank");
  let status = "uploading";
  await page.route(`**/api/clips/${clips.processing.id}`, (route) =>
    route.request().method() === "GET"
      ? route.fulfill({ json: { ...clips.processing, status } })
      : route.fallback(),
  );
  await page.goto(`/clips/${clips.processing.id}`);
  await expect(page.getByText("Uploading (now)")).toBeVisible();
  status = "processing";
  await expect(page.getByText("Uploaded (done)")).toBeVisible({ timeout: 15_000 });

  const deleted = page.waitForRequest(
    (r) => r.method() === "DELETE" && r.url().endsWith(`/api/clips/${clips.processing.id}`),
  );
  await page.getByRole("button", { name: "Delete", exact: true }).click();
  await page
    .getByRole("dialog", { name: "Delete this clip?" })
    .getByRole("button", { name: "Delete" })
    .click();
  await deleted;
});

test("upload: leaving by a link mid-upload asks, then keeps uploading and saves the details", async ({
  page,
}) => {
  await open(page, "/upload");
  // Blob Storage hangs, so the upload is still going when you leave.
  await page.route("**/e2e-blob/**", () => {});
  const uploading = page.waitForRequest("**/e2e-blob/**");
  await page.locator("input[type=file]").setInputFiles(video("leave.mp4"));
  await uploading;
  await page.getByLabel("Title").fill("Leaving with this title");

  const archive = page.getByRole("navigation").getByRole("link", { name: "Archive" });
  const dialog = page.getByRole("dialog", { name: "Leave while it uploads?" });
  await archive.click();
  await expect(dialog).toBeVisible();
  await dialog.getByRole("button", { name: "Stay" }).click();
  await expect(dialog).toBeHidden();
  await expect(page).toHaveURL(/\/upload$/);
  await expect(page.getByLabel("Title")).toHaveValue("Leaving with this title");

  await archive.click();
  const patch = page.waitForRequest((r) => r.method() === "PATCH");
  await dialog.getByRole("button", { name: "Leave and keep uploading" }).click();
  expect((await patch).postDataJSON()).toMatchObject({ title: "Leaving with this title" });
  await expect(page).toHaveURL(/\/$/);
  await expect(page.getByRole("heading", { name: "Archive" })).toBeVisible();
});

test("upload turns away an empty file", async ({ page }) => {
  await open(page, "/upload");
  await page.locator("input[type=file]").setInputFiles({
    name: "empty.mp4",
    mimeType: "video/mp4",
    buffer: Buffer.alloc(0),
  });
  await expect(page.getByText("That file is empty.")).toBeVisible();
  await expect(page.getByText("empty.mp4 · 0 B")).toBeVisible();
});

test("a reaction shows at once, keeps keyboard focus, and a failed one says so", async ({
  page,
}) => {
  await open(page, `/clips/${clips.long.id}`);
  let answer = () => {};
  const answered = new Promise<void>((r) => {
    answer = r;
  });
  await page.route("**/api/clips/*/reactions/**", async (route) => {
    await answered;
    await route.fulfill({ status: 500, json: { error: "server", message: "boom" } });
  });
  // The long clip has 🔥 4, one of them yours.
  const fire = page.getByRole("button", { name: /^🔥/ });
  await fire.focus();
  await page.keyboard.press("Enter");
  await expect(fire).toHaveAttribute("aria-pressed", "false");
  await expect(fire).toHaveAttribute("aria-busy", "true");
  await expect(fire).toHaveAccessibleName("🔥 3");
  // The others still work meanwhile.
  await expect(page.getByRole("button", { name: /^😂/ })).toBeEnabled();
  answer();
  await expect(page.getByRole("alert")).toHaveText("Couldn't react: boom");
  await expect(fire).toHaveAttribute("aria-pressed", "true");
  await expect(fire).toHaveAccessibleName("🔥 4");
  await expect(fire).toBeFocused();
});

test("the … menu works by keyboard, and focus comes back to it", async ({ page }) => {
  await open(page, `/clips/${clips.long.id}`);
  const more = page.getByRole("button", { name: "More actions" });
  const menu = page.getByRole("menu");
  // An admin's menu: Re-analyse kill feed, then Delete.
  const first = page.getByRole("menuitem", { name: "Re-analyse kill feed" });
  const item = page.getByRole("menuitem", { name: "Delete" });
  await more.focus();
  await page.keyboard.press("Enter");
  await expect(first).toBeFocused();
  await page.keyboard.press("ArrowDown");
  await expect(item).toBeFocused();
  await page.keyboard.press("Escape");
  await expect(menu).toHaveCount(0);
  await expect(more).toBeFocused();
  await page.keyboard.press("Enter");
  await page.keyboard.press("Tab");
  await expect(menu).toHaveCount(0);
  await expect(more).toBeFocused();

  // Delete from the menu, then close the dialog: focus is back on "…".
  await page.keyboard.press("Enter");
  await page.keyboard.press("End");
  await page.keyboard.press("Enter");
  const dialog = page.getByRole("dialog", { name: "Delete this clip?" });
  await expect(dialog).toBeVisible();
  await page.keyboard.press("Escape");
  await expect(dialog).toBeHidden();
  await expect(more).toBeFocused();
});

test("keyboard focus: a skip link first, and amber rings", async ({ page }) => {
  await open(page, "/");
  await expect(page.getByText(clips.normal.title).first()).toBeVisible();
  const amber = "rgb(245, 165, 36)";
  await page.keyboard.press("Tab");
  const skip = page.getByRole("link", { name: "Skip to content" });
  await expect(skip).toBeFocused();
  await page.keyboard.press("Enter");
  await expect(page.locator("main")).toBeFocused();

  // The next stop after the skip link is the logo: amber, not the browser's blue.
  await skip.focus();
  await page.keyboard.press("Tab");
  const home = page.getByRole("link", { name: "clipos home" });
  await expect(home).toBeFocused();
  await expect(home).toHaveCSS("outline-color", amber);

  // The search box, reached by keyboard, rings its whole pill.
  await page.getByLabel("Map").focus();
  await page.keyboard.press("Tab");
  await expect(page.getByRole("searchbox")).toBeFocused();
  const pill = page.locator("label", { has: page.getByRole("searchbox") });
  await expect(pill).toHaveCSS("outline-color", amber);
  await expect(pill).toHaveCSS("outline-style", "solid");
});

test("the share link's box shows focus in amber", async ({ page }) => {
  await open(page, `/clips/${clips.long.id}`);
  await page.getByRole("button", { name: "Shared" }).click();
  const link = page.getByRole("dialog").getByLabel("Share link");
  await link.focus();
  await expect(link).toHaveCSS("box-shadow", /rgb\(245, 165, 36\)/);
});

test("links have names, danger buttons have contrast, titles keep their direction", async ({
  page,
}) => {
  await open(page, `/clips/${clips.long.id}`);
  // The uploader's avatar links to their profile, and says so.
  await expect(page.locator('main a[href="/u/jamie"]').first()).toHaveAccessibleName("Jamie Doe");
  await expect(page.locator("h1")).toHaveAttribute("dir", "auto");
  await page.getByRole("button", { name: "More actions" }).click();
  await page.getByRole("menuitem", { name: "Delete" }).click();
  // White on danger-strong (#c4363b, 5.3:1), not on danger (#e5484d, 3.9:1).
  const del = page.getByRole("dialog").getByRole("button", { name: "Delete" });
  await expect(del).toHaveCSS("background-color", "rgb(196, 54, 59)");
  await expect(del).toHaveCSS("color", "rgb(255, 255, 255)");
  await page.keyboard.press("Escape");

  await page.goto("/u/robin");
  await expect(page.getByRole("link", { name: `Open ${clips.normal.title}` })).toBeVisible();
  await expect(page.locator("h1")).toHaveAttribute("dir", "auto");
});

test("cards: a failed pill with contrast, a ready clip without a poster, a broken avatar", async ({
  page,
}) => {
  await open(page, "about:blank");
  const noPoster = {
    ...clips.long,
    posterUrl: null,
    title: "הקליפ הכי טוב שלי באינפרנו",
    uploader: { ...clips.long.uploader, avatarUrl: "/missing-avatar.png" },
  };
  await page.route("**/missing-avatar.png", (route) => route.fulfill({ status: 404 }));
  await page.route(
    (url) => url.pathname === "/api/clips",
    (route) => route.fulfill({ json: { clips: [noPoster, clips.failed], nextCursor: null } }),
  );
  await page.goto("/");
  const card = page.getByTestId("clip-grid").locator(`a[href="/clips/${clips.long.id}"]`);
  await expect(card.getByText(noPoster.title)).toHaveAttribute("dir", "auto");
  await expect(card).not.toContainText("Processing");
  // The picture didn't load: initials instead.
  await expect(card.getByText("JD", { exact: true })).toBeVisible();
  const failed = page.locator(`a[href="/clips/${clips.failed.id}"]`).getByText("Failed", {
    exact: true,
  });
  await expect(failed).toHaveCSS("background-color", "rgb(196, 54, 59)");
});

/** The held clip as someone else sees it: a teaser. */
const teaser = {
  ...clips.held,
  isMine: false,
  canEdit: false,
  teaser: true,
  uploader: { handle: "jamie", displayName: "Jamie Doe", avatarUrl: null },
};

test("someone else's clip saved for the show is a teaser: nothing to play", async ({ page }) => {
  await open(page, "about:blank");
  // Even with a playback link (a held clip in a live show's lineup).
  await page.route(`**/api/clips/${clips.held.id}`, (route) =>
    route.fulfill({ json: { ...teaser, playbackUrl: "/e2e-media/clip.mp4" } }),
  );
  await page.goto(`/clips/${clips.held.id}`);
  await expect(page.getByRole("heading", { name: clips.held.title })).toBeVisible();
  await expect(page.getByText("Saved for the show", { exact: true })).toBeVisible();
  await expect(page.getByText("Jamie Doe · Ancient · 0:42")).toBeVisible();
  await expect(page.locator("video")).toHaveCount(0);
  await expect(page.getByRole("button", { name: /^🔥/ })).toHaveCount(0);
  await expect(page.getByRole("button", { name: "Download" })).toHaveCount(0);
  await expect(page.getByRole("button", { name: "Post now" })).toHaveCount(0);
});

test("a teaser, or a ready clip with no playback link, doesn't spin forever", async ({ page }) => {
  await open(page, "about:blank");
  let body: object = teaser;
  await page.route(`**/api/clips/${clips.held.id}`, (route) => route.fulfill({ json: body }));
  await page.goto(`/clips/${clips.held.id}`);
  await expect(page.getByText("Saved for the show", { exact: true })).toBeVisible();
  await expect(page.locator(".animate-spin")).toHaveCount(0);

  body = { ...clips.long, playbackUrl: null };
  await page.route(`**/api/clips/${clips.long.id}`, (route) => route.fulfill({ json: body }));
  await page.goto(`/clips/${clips.long.id}`);
  await expect(page.getByText("It won't play right now.")).toBeVisible();
  await expect(page.locator(".animate-spin")).toHaveCount(0);
});

test("each page names the tab", async ({ page }) => {
  await open(page, "/");
  await expect(page).toHaveTitle("Archive · clipos");
  await page.goto(`/clips/${clips.long.id}`);
  await expect(page).toHaveTitle(`${clips.long.title} · clipos`);
  await page.goto("/u/jamie");
  await expect(page).toHaveTitle("Jamie Doe · clipos");
  await page.goto("/upload");
  await expect(page).toHaveTitle("Upload · clipos");
  await page.goto("/nope");
  await expect(page).toHaveTitle("Nothing here · clipos");
});

test("archive: Back undoes chips, the sort and the map, not each letter typed", async ({
  page,
}) => {
  await open(page, "/");
  await expect(page.getByText(clips.normal.title).first()).toBeVisible();
  await page.getByRole("button", { name: "🔥 Fire" }).click();
  await expect(page).toHaveURL(/reaction=/);
  await page.getByRole("button", { name: "Top", exact: true }).click();
  await expect(page).toHaveURL(/sort=top/);
  await page.getByLabel("Map").selectOption("Inferno");
  await expect(page).toHaveURL(/map=Inferno/);
  await page.getByRole("searchbox").fill("ace");
  await expect(page).toHaveURL(/q=ace/);
  await page.getByRole("searchbox").fill("aces");
  await expect(page).toHaveURL(/q=aces/);

  // The search is one step, however much was typed.
  await page.goBack();
  await expect(page).not.toHaveURL(/q=/);
  await expect(page.getByRole("searchbox")).toHaveValue("");
  await expect(page).toHaveURL(/map=Inferno/);
  await page.goBack();
  await expect(page).not.toHaveURL(/map=/);
  await expect(page).toHaveURL(/sort=top/);
  await page.goBack();
  await expect(page).not.toHaveURL(/sort=/);
  await expect(page).toHaveURL(/reaction=/);
  await page.goBack();
  await expect(page).not.toHaveURL(/reaction=/);
  await expect(page.getByRole("button", { name: "All", exact: true })).toHaveAttribute(
    "aria-pressed",
    "true",
  );
});
