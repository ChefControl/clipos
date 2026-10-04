import { defineConfig, devices } from "@playwright/test";

// `COVERAGE=1`: the instrumented build (vite.config.ts), on another port so it never
// reuses a plain preview.
const mode = process.env.COVERAGE ? "e2e-coverage" : "e2e";
const port = process.env.COVERAGE ? 4174 : 4173;

// Browser regression tests (`pnpm e2e`): the app built in `e2e` mode (Auth0 stubbed, API
// mocked per test) on an iPhone with WebKit, an Android phone with Chromium, and a desktop
// Chrome window (pc). CI runs each device as its own job (`--project`), on every core.
export default defineConfig({
  testDir: "e2e",
  fullyParallel: true,
  forbidOnly: !!process.env.CI,
  retries: process.env.CI ? 1 : 0,
  // Playwright's default is half the cores, which was one worker on CI's runners.
  workers: process.env.CI ? "100%" : undefined,
  // With a worker on every core, WebKit can take over the default 5 s to render a clip
  // page from the instrumented build; the checks are the same, they only wait longer.
  expect: { timeout: process.env.CI ? 10_000 : 5_000 },
  reporter: process.env.CI ? [["list"], ["html", { open: "never" }]] : "list",
  use: {
    baseURL: `http://127.0.0.1:${port}`,
    trace: "retain-on-failure",
  },
  projects: [
    { name: "iphone", use: { ...devices["iPhone 13"] }, testMatch: "phone.spec.ts" },
    { name: "android", use: { ...devices["Pixel 7"] }, testMatch: "phone.spec.ts" },
    {
      name: "pc",
      use: { ...devices["Desktop Chrome"], viewport: { width: 1440, height: 900 } },
      testMatch: [
        "desktop.spec.ts",
        "show.spec.ts",
        "clip.spec.ts",
        "account.spec.ts",
        "upload.spec.ts",
        "tonight.spec.ts",
        "archive.spec.ts",
      ],
    },
  ],
  webServer: {
    command: `pnpm vite build --mode ${mode} --outDir dist-e2e && pnpm vite preview --mode ${mode} --outDir dist-e2e --host 127.0.0.1 --port ${port} --strictPort`,
    url: `http://127.0.0.1:${port}`,
    reuseExistingServer: !process.env.CI,
    timeout: 120_000,
  },
});
