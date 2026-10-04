// Every page the app has, with the text that says it has loaded. Shared by the phone and
// desktop suites.
import type { Page } from "@playwright/test";
import { clips, GONE_SHARE, mockApi } from "./fixtures";

export const pages: {
  name: string;
  path: string;
  ready: string;
  signedOut?: boolean;
  meError?: string;
}[] = [
  { name: "sign-in", path: "/", ready: "text=Sign in with Google", signedOut: true },
  {
    name: "sign-in-invited",
    path: "/?invite=1",
    ready: "text=You've been invited",
    signedOut: true,
  },
  { name: "privacy", path: "/privacy", ready: "h1:has-text('Privacy')" },
  { name: "feed", path: "/", ready: `text=${clips.normal.title}` },
  { name: "feed-filtered", path: "/?tag=ace&player=jamie&sort=top", ready: "text=#ace" },
  { name: "upload", path: "/upload", ready: "h1:has-text('Upload a clip')" },
  { name: "tonight", path: "/tonight", ready: "text=Host tonight's show" },
  { name: "clip-long-title", path: `/clips/${clips.long.id}`, ready: "text=Your stats" },
  {
    name: "clip-analysis-pending",
    path: `/clips/${clips.normal.id}`,
    ready: "text=Reading the killfeed…",
  },
  {
    name: "clip-processing",
    path: `/clips/${clips.processing.id}`,
    ready: "text=Kip's getting it ready",
  },
  { name: "clip-failed", path: `/clips/${clips.failed.id}`, ready: "text=Too long to keep." },
  { name: "profile-own", path: "/u/robin", ready: "text=Edit profile" },
  { name: "profile-other", path: "/u/maximilian-the-awper", ready: "text=Featured in" },
  { name: "profile-edit", path: "/me", ready: "text=Display name" },
  { name: "admin", path: "/admin", ready: "text=Invites" },
  { name: "shared-clip", path: "/s/exampleShareToken12345", ready: "text=Shared by Jamie Doe" },
  { name: "shared-clip-gone", path: `/s/${GONE_SHARE}`, ready: "text=This link has expired." },
  {
    name: "not-on-the-list",
    path: "/",
    ready: "text=Not on the list (yet).",
    meError: "not_invited",
  },
  {
    name: "account-disabled",
    path: "/",
    ready: "text=Account disabled.",
    meError: "account_disabled",
  },
  { name: "not-found", path: "/nope/nothing-here", ready: "text=Nothing here." },
];

/** Opens `path` with the API mocked. `routes` adds a test's own answers over the mock's
 *  (registered later, so they win). */
export async function open(
  page: Page,
  path: string,
  signedOut = false,
  meError?: string,
  routes?: (page: Page) => Promise<unknown>,
) {
  await mockApi(page, { meError });
  await routes?.(page);
  await page.addInitScript((out) => {
    if (out) window.localStorage.setItem("e2e-signed-out", "1");
    else window.localStorage.removeItem("e2e-signed-out");
  }, signedOut);
  // Anything the content security policy blocks is a bug: record it for cspViolations().
  await page.addInitScript(() => {
    const seen: string[] = [];
    (window as unknown as { __csp: string[] }).__csp = seen;
    document.addEventListener("securitypolicyviolation", (e) =>
      seen.push(`${e.effectiveDirective} blocked ${e.blockedURI || "inline"}`),
    );
  });
  await page.goto(path);
}

/** What the content security policy blocked on this page so far (should be nothing). */
export async function cspViolations(page: Page): Promise<string[]> {
  return page.evaluate(() => (window as unknown as { __csp?: string[] }).__csp ?? []);
}

/** The sign-in redirects the app asked the Auth0 stub for so far (auth0-stub.tsx). */
export async function signIns(page: Page): Promise<unknown[]> {
  return page.evaluate(
    () => (window as unknown as { __e2eSignIns?: unknown[] }).__e2eSignIns ?? [],
  );
}

/** The sign-outs the app asked the Auth0 stub for so far. */
export async function signOuts(page: Page): Promise<unknown[]> {
  return page.evaluate(
    () => (window as unknown as { __e2eSignOuts?: unknown[] }).__e2eSignOuts ?? [],
  );
}
