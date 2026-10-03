// The browser tests' `test`: Playwright's, plus coverage. With `COVERAGE=1` the e2e build
// is instrumented (vite.config.ts), and what the app ran is saved to .nyc_output for
// `pnpm coverage` to merge with the unit tests'.
//
// The counts live in the page's window, so each page load starts them over. They're saved
// whenever a document goes away: before a test loads another (`goto`, `reload`), when the
// app leaves by itself (a full-page link, `location.assign`: on `pagehide`), and for
// every page still open when the test ends or its context closes. Each document's counts
// are saved once.
import { randomUUID } from "node:crypto";
import { mkdir, writeFile } from "node:fs/promises";
import { type BrowserContext, test as base, type Page } from "@playwright/test";

export { expect } from "@playwright/test";

const OUT = new URL("../.nyc_output/", import.meta.url);

type Covered = { __coverage__?: unknown; __coverageSaved?: boolean };

/** Session storage keys of counts a document left as it went. */
const LEFT = "__coverage:";

async function write(coverage: unknown) {
  await mkdir(OUT, { recursive: true });
  await writeFile(new URL(`${randomUUID()}.json`, OUT), JSON.stringify(coverage));
}

/** Saves what the page's current document ran, unless it's been saved already, and what
 *  documents before it on the same site left behind as they went (see keepAcrossLoads). */
async function savePage(page: Page) {
  const found = await page
    .evaluate((prefix) => {
      const all: unknown[] = [];
      try {
        for (const key of Object.keys(sessionStorage)) {
          if (!key.startsWith(prefix)) continue;
          all.push(JSON.parse(sessionStorage.getItem(key) ?? "null"));
          sessionStorage.removeItem(key);
        }
      } catch {
        // No storage here (about:blank).
      }
      const w = window as Covered;
      if (!w.__coverageSaved && w.__coverage__) {
        w.__coverageSaved = true;
        all.push(w.__coverage__);
      }
      return all;
    }, LEFT)
    .catch(() => []);
  for (const coverage of found) if (coverage) await write(coverage);
}

async function save(context: BrowserContext) {
  for (const page of context.pages()) await savePage(page);
}

/** Saves each document's counts before the next one replaces them. */
async function keepAcrossLoads(context: BrowserContext) {
  // The app leaving by itself: the document leaves its counts in the tab's session storage
  // as it goes (nothing slower gets out of a page that's unloading), for savePage.
  await context.addInitScript((prefix) => {
    window.addEventListener("pagehide", () => {
      const w = window as Covered;
      if (w.__coverageSaved || !w.__coverage__) return;
      w.__coverageSaved = true;
      try {
        sessionStorage.setItem(
          `${prefix}${Date.now()}-${Math.random()}`,
          JSON.stringify(w.__coverage__),
        );
      } catch {
        // Full or blocked: these counts are lost.
      }
    });
  }, LEFT);
  // A test loading another page: saved before it goes, without relying on pagehide.
  const wrap = (page: Page) => {
    const goto = page.goto.bind(page);
    page.goto = async (...args) => {
      await savePage(page);
      return goto(...args);
    };
    const reload = page.reload.bind(page);
    page.reload = async (...args) => {
      await savePage(page);
      return reload(...args);
    };
  };
  for (const page of context.pages()) wrap(page);
  context.on("page", wrap);
}

/** A context the test opened itself: also saved before it closes, however it's closed. */
async function track(context: BrowserContext) {
  await keepAcrossLoads(context);
  const close = context.close.bind(context);
  context.close = async (options) => {
    await save(context);
    return close(options);
  };
}

export const test = base.extend<{ coverage: undefined }>({
  coverage: [
    async ({ browser, context }, use) => {
      if (!process.env.COVERAGE) return use(undefined);
      await keepAcrossLoads(context);
      // Contexts a test opens itself (the show's host and friend) count too.
      const opened: BrowserContext[] = [context];
      const newContext = browser.newContext.bind(browser);
      browser.newContext = async (options) => {
        const c = await newContext(options);
        await track(c);
        opened.push(c);
        return c;
      };
      await use(undefined);
      browser.newContext = newContext;
      for (const c of opened) await save(c);
    },
    { auto: true },
  ],
});
