// Upload on a desktop, beyond the happy path in desktop.spec.ts: files it turns away,
// dragging, the local preview, progress, the details that go with the clip, and what
// happens when something fails, here or after you've left the page. API and Blob Storage
// mocked (fixtures.ts), with a test's own routes on top.
import { readFileSync } from "node:fs";
import type { Page, Request } from "@playwright/test";
import { clips } from "./fixtures";
import { open } from "./pages";
import { expect, test } from "./test";

const BLOCK = 8 * 1024 * 1024;

/** A file for the picker. */
const video = (name: string, bytes = 64 * 1024) => ({
  name,
  mimeType: "video/mp4",
  buffer: Buffer.alloc(bytes),
});

const pick = (page: Page, file: ReturnType<typeof video>) =>
  page.locator("input[type=file]").setInputFiles(file);

/** Drags files over the page, as from the desktop, and drops them unless `drop` is false. */
async function drag(page: Page, files: { name: string; size?: number }[], events: string[]) {
  await page.evaluate(
    ({ files, events }) => {
      const list = files.map(({ name, size }) => {
        const file = new File(["x"], name, { type: "video/mp4" });
        // A real 3 GB file is too much for a test; its size is all that's looked at.
        if (size != null) Object.defineProperty(file, "size", { value: size });
        return file;
      });
      for (const type of events) {
        const e = new Event(type, { bubbles: true, cancelable: true });
        Object.defineProperty(e, "dataTransfer", { value: { types: ["Files"], files: list } });
        window.dispatchEvent(e);
      }
    },
    { files, events },
  );
}

const sent = (page: Page, method: string, path: RegExp) =>
  page.waitForRequest((r) => r.method() === method && path.test(new URL(r.url()).pathname));

const failWith = (status: number, message: string) => ({
  status,
  json: { error: "nope", message },
});

/** Upload, then leave by the Archive link while Blob Storage holds on to it. Returns a
 *  function that lets Blob Storage answer. */
async function leaveMidUpload(page: Page, name: string): Promise<() => void> {
  let release = () => {};
  const held = new Promise<void>((r) => {
    release = r;
  });
  await page.route("**/e2e-blob/**", async (route) => {
    await held;
    await route.fulfill({ status: 201, body: "" });
  });
  const uploading = page.waitForRequest("**/e2e-blob/**");
  await pick(page, video(name));
  await uploading;
  await page.getByRole("navigation").getByRole("link", { name: "Archive" }).click();
  const patch = sent(page, "PATCH", /^\/api\/clips\//);
  await page
    .getByRole("dialog", { name: "Leave while it uploads?" })
    .getByRole("button", { name: "Leave and keep uploading" })
    .click();
  await patch;
  await expect(page.getByRole("heading", { name: "Archive" })).toBeVisible();
  return release;
}

test("a file over 2 GB is turned away, and Choose another file picks again", async ({ page }) => {
  await open(page, "/upload");
  await drag(page, [{ name: "whole-match.mp4", size: 3 * 1024 ** 3 }], ["dragenter", "drop"]);
  await expect(page.getByRole("heading", { name: "That one's 3.00 GB." })).toBeVisible();
  await expect(page.getByText("whole-match.mp4 · 3.00 GB")).toBeVisible();

  const chooser = page.waitForEvent("filechooser");
  await page.getByRole("button", { name: "Choose another file" }).click();
  await (await chooser).setFiles(video("trimmed.mp4"));
  await expect(page.getByText("trimmed.mp4")).toBeVisible();
  await expect(page.getByText(/Uploaded ✓/)).toBeVisible();
});

test("dragging a file over the page says Drop it, until it's dragged away", async ({ page }) => {
  await open(page, "/upload");
  const idle = page.getByRole("heading", { name: "Drop a clip anywhere on this page" });
  await expect(idle).toBeVisible();
  // Over the page, then over a child of it: two enters, so one leave isn't the end.
  await drag(page, [{ name: "ace.mp4" }], ["dragenter", "dragenter", "dragover", "dragleave"]);
  await expect(page.getByRole("heading", { name: "Drop it." })).toBeVisible();
  await drag(page, [{ name: "ace.mp4" }], ["dragleave"]);
  await expect(idle).toBeVisible();

  // Dragging text, not files, is none of its business.
  await page.evaluate(() => {
    const e = new Event("dragenter", { bubbles: true, cancelable: true });
    Object.defineProperty(e, "dataTransfer", { value: { types: ["text/plain"], files: [] } });
    window.dispatchEvent(e);
  });
  await expect(idle).toBeVisible();
});

test("the preview shows the clip's length before the server has it", async ({ page }) => {
  await open(page, "/upload");
  await pick(page, {
    name: "Counter-strike 2 2026.10.02 - 21.14.08.02.mkv",
    mimeType: "video/x-matroska",
    buffer: readFileSync(new URL("./media/show.webm", import.meta.url)),
  });
  await expect(page.getByText("335 KB · 0:40")).toBeVisible();
  await expect(page.getByText("0:40", { exact: true })).toBeVisible();
});

test("progress shows the speed and the time left", async ({ page }) => {
  // The page's clock stands still unless the test moves it (timers still run).
  const start = Date.parse("2026-10-02T21:00:00Z");
  await page.clock.setFixedTime(start);
  await open(page, "/upload");
  // The first block goes up; the second hangs, so the upload sits at one block of two.
  await page.route("**/e2e-blob/**", (route) =>
    new URL(route.request().url()).searchParams.get("blockid") === btoa("block-000000")
      ? route.fulfill({ status: 201, body: "" })
      : undefined,
  );
  await pick(page, video("long.mp4", 2 * BLOCK));
  const progress = page.getByRole("progressbar", { name: "Upload" });
  // The 8 MB block goes through Playwright's routing, which takes a while when every core
  // is running tests (CI runs one worker per core).
  await expect(progress).toHaveAttribute("aria-valuenow", "50", { timeout: 20_000 });
  // No time has passed: no speed yet.
  const line = progress.locator("xpath=following-sibling::p");
  await expect(line).toHaveText("50% · 8.0 MB of 16.0 MB");

  // 8 MB in 10 s; it shows on the next render, and typing a title is one.
  await page.clock.setFixedTime(start + 10_000);
  await page.getByLabel("Title").fill("Long one");
  await expect(line).toHaveText("50% · 8.0 MB of 16.0 MB · 819 KB/sabout 10 s left");
  await page.clock.setFixedTime(start + 100_000);
  await page.getByLabel("Title").fill("Long one, slowly");
  await expect(line).toHaveText("50% · 8.0 MB of 16.0 MB · 82 KB/sabout 2 min left");
});

test("who's in it and whose screen it is go with the details", async ({ page }) => {
  await open(page, "/upload");
  await pick(page, video("squad.mp4"));
  await expect(page.getByText(/Uploaded ✓/)).toBeVisible();
  // Your friends, not you.
  const who = page.getByRole("group", { name: "Who's in it" });
  await expect(who.getByRole("button")).toHaveText([
    "JDJamie Doe",
    "MKMaximilian Alexander Konstantinopoulos",
  ]);
  const jamie = who.getByRole("button", { name: "Jamie Doe" });
  const max = who.getByRole("button", { name: /Maximilian/ });
  await jamie.click();
  await max.click();
  await max.click();
  await expect(jamie).toHaveAttribute("aria-pressed", "true");
  await expect(max).toHaveAttribute("aria-pressed", "false");
  const pov = page.getByRole("switch", { name: /Recorded from my point of view/ });
  await expect(pov).toBeChecked();
  await page.getByText("Recorded from my point of view").click();
  await expect(pov).not.toBeChecked();

  const patch = sent(page, "PATCH", /^\/api\/clips\//);
  await page.getByRole("button", { name: "Save details" }).click();
  expect((await patch).postDataJSON()).toMatchObject({
    myPov: false,
    players: ["00000000-0000-4000-8000-000000000002"],
  });
  await expect(page).toHaveURL(new RegExp(`/clips/${clips.processing.id}$`));
});

test("details that can't be saved say why, and can be saved again", async ({ page }) => {
  await open(page, "/upload");
  let fail = true;
  await page.route("**/api/clips/*", (route) =>
    route.request().method() === "PATCH" && fail
      ? route.fulfill(failWith(500, "the database is asleep"))
      : route.fallback(),
  );
  await pick(page, video("ace.mp4"));
  await expect(page.getByText(/Uploaded ✓/)).toBeVisible();
  const save = page.getByRole("button", { name: "Save details" });
  await save.click();
  await expect(page.getByRole("alert")).toHaveText("the database is asleep");
  await expect(save).toBeEnabled();
  await expect(page.getByText("Save the details to open it.")).toBeVisible();
  fail = false;
  await save.click();
  await expect(page).toHaveURL(new RegExp(`/clips/${clips.processing.id}$`));
});

test("a hold that didn't go through is said, and saved with the details", async ({ page }) => {
  await open(page, "/upload");
  const holds: Request[] = [];
  await page.route("**/api/clips/*/hold", (route) => {
    holds.push(route.request());
    return holds.length === 1
      ? route.fulfill(failWith(503, "try again in a moment"))
      : route.fallback();
  });
  await pick(page, video("friday.mp4"));
  await expect(page.getByText(/Uploaded ✓/)).toBeVisible();
  await page.getByText("Save it for the show").click();
  await expect(page.getByRole("alert")).toHaveText(
    "Couldn't save it for the show: try again in a moment",
  );
  // Still on: it's what you asked for, and Save details tries again.
  await expect(page.getByRole("switch", { name: /Save it for the show/ })).toBeChecked();
  await expect(page.getByText("Joins tonight's lineup.", { exact: false })).toBeVisible();
  const patch = sent(page, "PATCH", /^\/api\/clips\//);
  await page.getByRole("button", { name: "Save details" }).click();
  await patch;
  await expect(page).toHaveURL(new RegExp(`/clips/${clips.processing.id}$`));
  expect(holds).toHaveLength(2);
});

test("an upload that fails says why and offers another file", async ({ page }) => {
  await open(page, "/upload");
  await page.route("**/api/clips/*/complete", (route) =>
    route.fulfill(failWith(500, "the queue is full")),
  );
  await pick(page, video("ace.mp4"));
  await expect(page.getByRole("alert")).toHaveText("the queue is full");
  await expect(page.getByRole("button", { name: "Save details" })).toBeDisabled();
  await page.getByRole("button", { name: "Choose another file" }).click();
  await expect(page.getByRole("button", { name: "Choose a video" })).toBeVisible();
});

test("left mid-upload, it finishes in the background and says so", async ({ page }) => {
  await open(page, "/upload");
  const release = await leaveMidUpload(page, "background.mp4");
  const complete = sent(page, "POST", /\/complete$/);
  release();
  await complete;
  await expect(page.getByRole("status")).toHaveText("background is up. Kip's getting it ready.");
});

test("left mid-upload, a failure afterwards is said where you are", async ({ page }) => {
  await open(page, "/upload");
  await page.route("**/api/clips/*/complete", (route) =>
    route.fulfill(failWith(500, "the queue is full")),
  );
  const release = await leaveMidUpload(page, "doomed.mp4");
  release();
  await expect(page.getByRole("alert")).toHaveText("Couldn't upload doomed.mp4: the queue is full");
});

// Several at once: a queue, one upload after another, details left for each clip's page.

/** Records the clip API calls an upload makes, in order: `create:<filename>`, `complete`. */
function steps(page: Page): string[] {
  const seen: string[] = [];
  page.on("request", (r) => {
    const path = new URL(r.url()).pathname;
    if (r.method() === "POST" && path === "/api/clips")
      seen.push(`create:${r.postDataJSON().filename}`);
    if (r.method() === "POST" && path.endsWith("/complete")) seen.push("complete");
  });
  return seen;
}

/** Holds Blob Storage's answers until the returned function is called. */
async function holdBlob(page: Page): Promise<() => void> {
  let release = () => {};
  const held = new Promise<void>((r) => {
    release = r;
  });
  await page.route("**/e2e-blob/**", async (route) => {
    await held;
    await route.fulfill({ status: 201, body: "" });
  });
  return release;
}

test("several files go up one after another, and the ones it can't take say why", async ({
  page,
}) => {
  await open(page, "/upload");
  const seen = steps(page);
  const release = await holdBlob(page);
  await drag(
    page,
    [{ name: "ace.mp4" }, { name: "notes.txt" }, { name: "clutch.mkv" }],
    ["dragenter", "drop"],
  );
  await expect(page.getByRole("heading", { name: "Upload clips" })).toBeVisible();
  const rows = page.getByRole("list", { name: "Uploads" }).getByRole("listitem");
  await expect(rows).toHaveCount(3);
  await expect(rows.nth(1)).toContainText("That's not a video file.");
  // The second waits for the first.
  await expect(rows.nth(2)).toContainText("Waiting its turn…");
  await expect(page.getByText("0 of 2 up", { exact: false })).toBeVisible();
  release();

  await expect(page.getByText("2 of 2 up.")).toBeVisible();
  expect(seen).toEqual(["create:ace.mp4", "complete", "create:clutch.mkv", "complete"]);
  await page.getByRole("link", { name: "Open clutch.mkv" }).click();
  await expect(page).toHaveURL(new RegExp(`/clips/${clips.processing.id}$`));
});

test("the queue takes more, drops one that's waiting, and cancels the one going up", async ({
  page,
}) => {
  await open(page, "/upload");
  const seen = steps(page);
  const release = await holdBlob(page);
  const uploading = page.waitForRequest("**/e2e-blob/**");
  await page.locator("input[type=file]").setInputFiles([video("one.mp4"), video("two.mp4")]);
  await uploading;

  await page.getByRole("button", { name: "Remove two.mp4" }).click();
  const chooser = page.waitForEvent("filechooser");
  await page.getByRole("button", { name: "Add more" }).click();
  await (await chooser).setFiles(video("three.mp4"));
  await expect(page.getByRole("progressbar", { name: "three.mp4" })).toHaveCount(0);

  // Cancelled: its clip goes to the trash, and the next one starts.
  const trashed = sent(page, "DELETE", /^\/api\/clips\//);
  await page.getByRole("button", { name: "Cancel one.mp4" }).click();
  await trashed;
  release();
  await expect(page.getByText("1 of 1 up.")).toBeVisible();
  const rows = page.getByRole("list", { name: "Uploads" }).getByRole("listitem");
  await expect(rows).toHaveCount(1);
  await expect(rows).toContainText("three.mp4");
  expect(seen).toEqual(["create:one.mp4", "create:three.mp4", "complete"]);
});

test("one in the queue that fails says why, the rest go on, and it can be tried again", async ({
  page,
}) => {
  await open(page, "/upload");
  let fail = true;
  await page.route("**/api/clips/*/complete", (route) => {
    if (!fail) return route.fallback();
    fail = false;
    return route.fulfill(failWith(500, "the queue is full"));
  });
  await page.locator("input[type=file]").setInputFiles([video("first.mp4"), video("second.mp4")]);
  await expect(page.getByRole("alert")).toHaveText("the queue is full");
  await expect(page.getByText("1 of 2 up.")).toBeVisible();
  await page.getByRole("button", { name: "Try first.mp4 again" }).click();
  await expect(page.getByText("2 of 2 up.")).toBeVisible();
  await expect(page.getByRole("alert")).toHaveCount(0);
});

test("Save them for the show holds every clip in the queue", async ({ page }) => {
  await open(page, "/upload");
  const created: boolean[] = [];
  page.on("request", (r) => {
    if (r.method() === "POST" && new URL(r.url()).pathname === "/api/clips") {
      created.push(r.postDataJSON().hold);
    }
  });
  const release = await holdBlob(page);
  const uploading = page.waitForRequest("**/e2e-blob/**");
  await page.locator("input[type=file]").setInputFiles([video("a.mp4"), video("b.mp4")]);
  await uploading;
  // The first clip exists already: it's held now; the second starts out held.
  const held = sent(page, "POST", /\/hold$/);
  await page.getByText("Save them for the show").click();
  await held;
  release();
  await expect(page.getByText("2 of 2 up.")).toBeVisible();
  expect(created).toEqual([false, true]);
});

test("left mid-queue, the rest go up in the background and say so", async ({ page }) => {
  await open(page, "/upload");
  const release = await holdBlob(page);
  const uploading = page.waitForRequest("**/e2e-blob/**");
  await page.locator("input[type=file]").setInputFiles([video("a.mp4"), video("b.mp4")]);
  await uploading;
  await page.getByRole("navigation").getByRole("link", { name: "Archive" }).click();
  await page
    .getByRole("dialog", { name: "Leave while they upload?" })
    .getByRole("button", { name: "Leave and keep uploading" })
    .click();
  await expect(page.getByRole("heading", { name: "Archive" })).toBeVisible();
  release();
  await expect(page.getByRole("status")).toHaveText("2 clips are up. Kip's getting them ready.");
});
