// Accounts on a desktop: signing in and out, the doors that turn you away, your profile and
// Edit profile, the admin page, and the app or a share link failing to load. API mocked
// (fixtures.ts), with a test's own routes on top where it needs other answers.
import type { Page, Route } from "@playwright/test";
import { clips, me } from "./fixtures";
import { open, signIns, signOuts } from "./pages";
import { expect, test } from "./test";

const NOW = "2026-10-01T12:00:00Z";

const json = (route: Route, body: unknown, status = 200) =>
  route.fulfill({ status, contentType: "application/json", body: JSON.stringify(body) });

/** Opens `path` signed out, with Auth0 having come back with `authError` if given. */
async function openSignedOut(page: Page, path: string, authError?: string) {
  await page.addInitScript((error) => {
    if (error) window.localStorage.setItem("e2e-auth-error", error);
    else window.localStorage.removeItem("e2e-auth-error");
  }, authError ?? null);
  await open(page, path, true);
}

test.describe("signing in", () => {
  test("Sign in with Google comes back to the page you came for", async ({ page }) => {
    await openSignedOut(page, "/?invite=1");
    await expect(page.getByText("You've been invited.")).toBeVisible();
    await page.getByRole("button", { name: "Sign in with Google" }).click();
    expect(await signIns(page)).toEqual([
      {
        authorizationParams: { connection: "google-oauth2" },
        appState: { returnTo: "/?invite=1" },
      },
    ]);
  });

  test("an account Auth0 turned away is not on the list, and can try another", async ({ page }) => {
    await openSignedOut(page, "/", "Sorry, robin@example.com isn't invited to clipos.");
    await expect(page.getByRole("heading", { name: "Not on the list (yet)." })).toBeVisible();
    // Auth0 stopped it before the API: no email to show.
    await expect(page.getByText("your Google account")).toBeVisible();
    await page.getByRole("button", { name: "Sign in with a different account" }).click();
    expect(await signOuts(page)).toEqual([
      { logoutParams: { returnTo: new URL(page.url()).origin } },
    ]);
  });

  test("any other sign-in error is said on the sign-in page", async ({ page }) => {
    await openSignedOut(page, "/?invite=1", "Google said no.");
    await expect(page.getByRole("alert")).toHaveText("Google said no.");
    // The invite note gives way to the error.
    await expect(page.getByText("You've been invited.")).toBeHidden();
    await expect(page.getByRole("button", { name: "Sign in with Google" })).toBeVisible();
  });

  test("Privacy is a link away from signing in, no account needed", async ({ page }) => {
    await openSignedOut(page, "/");
    await page.getByRole("link", { name: "Privacy" }).click();
    await expect(page.getByRole("heading", { name: "Privacy" })).toBeVisible();
    await expect(page).toHaveURL(/\/privacy$/);
  });

  test("an account the API refuses can sign in with another", async ({ page }) => {
    await open(page, "/", false, "not_invited");
    await expect(page.getByRole("heading", { name: "Not on the list (yet)." })).toBeVisible();
    await expect(page.getByText("robin@example.com")).toBeVisible();
    await page.getByRole("button", { name: "Sign in with a different account" }).click();
    expect(await signOuts(page)).toHaveLength(1);
  });

  test("no access token at all signs in again", async ({ page }) => {
    await page.addInitScript(() => window.localStorage.setItem("e2e-no-token", "1"));
    await open(page, "/u/jamie");
    await expect.poll(() => signIns(page)).toHaveLength(1);
    expect(await signIns(page)).toEqual([
      expect.objectContaining({ appState: { returnTo: "/u/jamie" } }),
    ]);
  });

  test("the app says so when it can't start", async ({ page }) => {
    await open(page, "about:blank");
    await page.route("**/api/config", (route) =>
      json(route, { error: "server", message: "down" }, 502),
    );
    await page.goto("/");
    await expect(
      page.getByText("clipos couldn't start: GET /api/config failed: 502"),
    ).toBeVisible();
  });
});

test.describe("share links", () => {
  test("a share link that can't be reached says so", async ({ page }) => {
    await open(page, "about:blank");
    await page.route("**/s/*/clip.json", (route) => route.abort("connectionfailed"));
    await page.goto("/s/exampleShareToken12345");
    await expect(page.getByText("Couldn't load this clip.")).toBeVisible();
  });

  test("a share link the server fails on says so", async ({ page }) => {
    await open(page, "about:blank");
    await page.route("**/s/*/clip.json", (route) => json(route, { error: "server" }, 500));
    await page.goto("/s/exampleShareToken12345");
    await expect(page.getByText("Couldn't load this clip.")).toBeVisible();
    await expect(page.getByRole("link", { name: "Members sign in" })).toHaveAttribute("href", "/");
  });
});

test.describe("profiles", () => {
  test("tabs switch between uploads and clips you're in, in the URL", async ({ page }) => {
    await open(page, "/u/jamie");
    const featured = page.getByRole("button", { name: "Featured in · 3" });
    const asked = page.waitForRequest(
      (r) => new URL(r.url()).searchParams.get("player") === "jamie",
    );
    await featured.click();
    await asked;
    await expect(featured).toHaveAttribute("aria-pressed", "true");
    await expect(page).toHaveURL(/\/u\/jamie\?tab=featured$/);
    await page.getByRole("button", { name: "Uploads · 12" }).click();
    await expect(page).toHaveURL(/\/u\/jamie$/);
    // Not yours: no account, trash or Edit profile.
    await expect(page.getByRole("link", { name: "Edit profile" })).toHaveCount(0);
    await expect(page.getByRole("heading", { name: "Trash" })).toHaveCount(0);
  });

  test("a profile that can't load says why, and tries again", async ({ page }) => {
    await open(page, "about:blank");
    let fail = true;
    await page.route("**/api/users/jamie", (route) =>
      fail
        ? json(route, { error: "server", message: "the database is asleep" }, 503)
        : route.fallback(),
    );
    await page.goto("/u/jamie");
    const error = page.getByRole("alert").filter({ hasText: "the database is asleep" });
    await expect(error).toBeVisible({ timeout: 10_000 });
    fail = false;
    await error.getByRole("button", { name: "Try again" }).click();
    await expect(page.getByRole("heading", { name: "Jamie Doe" })).toBeVisible();
  });

  test("Sign out on your profile signs you out back to the start", async ({ page }) => {
    await open(page, "/u/robin");
    await page.getByRole("button", { name: "Sign out" }).click();
    expect(await signOuts(page)).toEqual([
      { logoutParams: { returnTo: new URL(page.url()).origin } },
    ]);
  });

  test("Restore takes a clip out of the trash, and says when it can't", async ({ page }) => {
    await open(page, "about:blank");
    let status = 200;
    const restores: string[] = [];
    await page.route("**/api/clips/*/restore", (route) => {
      restores.push(route.request().url());
      return status === 200
        ? json(route, clips.normal)
        : json(route, { error: "gone", message: "it's gone for good" }, status);
    });
    await page.goto("/u/robin");
    const trash = page
      .locator("section")
      .filter({ has: page.getByRole("heading", { name: "Trash" }) });
    await expect(trash.getByText(clips.normal.title)).toBeVisible();
    await expect(trash.getByText(/^gone for good (tomorrow|in [2-7] days)$/)).toBeVisible();

    status = 410;
    await trash.getByRole("button", { name: "Restore" }).click();
    await expect(page.getByRole("alert")).toHaveText("Couldn't restore it: it's gone for good");
    status = 200;
    await trash.getByRole("button", { name: "Restore" }).click();
    await expect(page.getByRole("status")).toHaveText(`Restored ${clips.normal.title}.`);
    expect(restores).toHaveLength(2);
    expect(restores[0]).toContain(`/api/clips/${clips.normal.id}/restore`);
  });

  test("Edit profile saves, and a new handle moves your page to it", async ({ page }) => {
    await open(page, "about:blank");
    const saved = { ...me, displayName: "Robin R", handle: "robin-r", steamName: "RobinPlays2" };
    await page.route("**/api/me", (route) =>
      route.request().method() === "PATCH" ? json(route, saved) : route.fallback(),
    );
    await page.route("**/api/users/robin-r", (route) =>
      json(route, {
        id: me.id,
        handle: "robin-r",
        displayName: "Robin R",
        avatarUrl: null,
        steamName: "RobinPlays2",
        clipCount: 1,
        featuredCount: 0,
        fireCount: 0,
        joinedAt: NOW,
      }),
    );
    await page.goto("/u/robin?tab=featured&edit=true");
    const dialog = page.getByRole("dialog", { name: "Edit profile" });
    await dialog.getByLabel("Display name").fill("Robin R");
    await dialog.getByLabel("Handle").fill("robin-r");
    await dialog.getByLabel("Steam name").fill("RobinPlays2");
    const patch = page.waitForRequest((r) => r.method() === "PATCH");
    await dialog.getByRole("button", { name: "Save" }).click();
    expect((await patch).postDataJSON()).toEqual({
      displayName: "Robin R",
      handle: "robin-r",
      steamName: "RobinPlays2",
    });
    await expect(dialog).toBeHidden();
    // Your new handle, on the tab you were on, and it's still you.
    await expect(page).toHaveURL(/\/u\/robin-r\?tab=featured$/);
    await expect(page.getByRole("heading", { name: "Robin R" })).toBeVisible();
    await expect(page.getByText("@robin-r · Steam: RobinPlays2 · you")).toBeVisible();
  });

  test("Edit profile says why it couldn't save, and stays open", async ({ page }) => {
    await open(page, "about:blank");
    await page.route("**/api/me", (route) =>
      route.request().method() === "PATCH"
        ? json(route, { error: "conflict", message: "that handle is taken" }, 409)
        : route.fallback(),
    );
    await page.goto("/me");
    const dialog = page.getByRole("dialog", { name: "Edit profile" });
    await dialog.getByLabel("Handle").fill("jamie");
    await dialog.getByRole("button", { name: "Save" }).click();
    await expect(dialog.getByRole("alert")).toHaveText("that handle is taken");
    await expect(dialog).toBeVisible();
    await expect(page).toHaveURL(/\/u\/robin\?edit=true$/);
  });
});

type Invite = {
  email: string;
  role: "member" | "admin";
  invitedBy: string | null;
  createdAt: string;
  acceptedAt: string | null;
  revokedAt: string | null;
};

/** The admin API, with state: invites and members change as the page asks. */
async function mockAdmin(page: Page) {
  const invites: Invite[] = [
    {
      email: "pending@friend.com",
      role: "member",
      invitedBy: "robin",
      createdAt: NOW,
      acceptedAt: null,
      revokedAt: null,
    },
    {
      email: me.email,
      role: "admin",
      invitedBy: null,
      createdAt: NOW,
      acceptedAt: NOW,
      revokedAt: null,
    },
    {
      email: "old@friend.com",
      role: "member",
      invitedBy: "robin",
      createdAt: NOW,
      acceptedAt: null,
      revokedAt: NOW,
    },
  ];
  const users = [
    { ...me },
    {
      ...me,
      id: "u2",
      handle: "jamie",
      displayName: "Jamie",
      email: "jamie@example.com",
      role: "member",
    },
    {
      ...me,
      id: "u3",
      handle: "max",
      displayName: "Max",
      email: "max@example.com",
      role: "admin",
      status: "disabled",
    },
  ];
  const state = {
    invites,
    users,
    /** Answers with an error instead, while set. */
    fail: null as null | { status: number; message: string },
    requests: [] as { method: string; path: string; body: unknown }[],
  };
  await page.route("**/api/admin/**", (route) => {
    const req = route.request();
    const path = new URL(req.url()).pathname;
    const body = req.postDataJSON();
    if (req.method() !== "GET") state.requests.push({ method: req.method(), path, body });
    if (state.fail) {
      return json(route, { error: "nope", message: state.fail.message }, state.fail.status);
    }
    if (path === "/api/admin/invites" && req.method() === "POST") {
      const invite: Invite = {
        email: body.email,
        role: body.role,
        invitedBy: "robin",
        createdAt: NOW,
        acceptedAt: null,
        revokedAt: null,
      };
      state.invites = [invite, ...state.invites.filter((i) => i.email !== body.email)];
      return json(route, invite, 201);
    }
    if (path === "/api/admin/invites/revoke") {
      state.invites = state.invites.map((i) =>
        i.email === body.email ? { ...i, revokedAt: NOW } : i,
      );
      return route.fulfill({ status: 204 });
    }
    if (path === "/api/admin/invites") return json(route, state.invites);
    const user = path.match(/^\/api\/admin\/users\/([^/]+)$/);
    if (user && req.method() === "PATCH") {
      state.users = state.users.map((u) => (u.id === user[1] ? { ...u, ...body } : u));
      return json(
        route,
        state.users.find((u) => u.id === user[1]),
      );
    }
    if (path === "/api/admin/users") return json(route, state.users);
    return route.fallback();
  });
  return state;
}

test.describe("admin", () => {
  const invite = (page: Page, email: string) =>
    page.getByRole("listitem").filter({ hasText: email });
  const member = (page: Page, name: string) =>
    page.getByRole("listitem").filter({ has: page.getByText(name, { exact: true }) });

  test("invite a friend as a member or an admin", async ({ page }) => {
    await open(page, "about:blank");
    const admin = await mockAdmin(page);
    await page.goto("/admin");
    await expect(
      invite(page, "pending@friend.com").getByText("Pending", { exact: true }),
    ).toBeVisible();
    await expect(invite(page, me.email).getByText("from ADMIN_EMAILS")).toBeVisible();
    // Your own invite can't be revoked.
    await expect(invite(page, me.email).getByRole("button")).toHaveCount(0);

    const email = page.getByLabel("Email to invite");
    await email.fill("new.admin@example.com");
    await page.getByLabel("Role", { exact: true }).selectOption("admin");
    await page.getByRole("button", { name: "Invite", exact: true }).click();
    await expect(invite(page, "new.admin@example.com")).toContainText("Admin · invited by @robin");
    await expect(email).toHaveValue("");
    expect(admin.requests).toEqual([
      {
        method: "POST",
        path: "/api/admin/invites",
        body: { email: "new.admin@example.com", role: "admin" },
      },
    ]);
  });

  test("an invite the API refuses says why and keeps the email", async ({ page }) => {
    await open(page, "about:blank");
    const admin = await mockAdmin(page);
    await page.goto("/admin");
    await expect(invite(page, "pending@friend.com")).toBeVisible();
    await expect(member(page, "Jamie")).toBeVisible();
    admin.fail = { status: 400, message: "that isn't a Google account" };
    await page.getByLabel("Email to invite").fill("someone@example.com");
    await page.getByRole("button", { name: "Invite", exact: true }).click();
    await expect(page.getByRole("alert")).toHaveText("that isn't a Google account");
    await expect(page.getByLabel("Email to invite")).toHaveValue("someone@example.com");
  });

  test("Revoke asks first; Re-invite brings a revoked invite back", async ({ page }) => {
    await open(page, "about:blank");
    const admin = await mockAdmin(page);
    await page.goto("/admin");
    const pending = invite(page, "pending@friend.com");

    // Changed your mind: nothing happens.
    page.once("dialog", (d) => d.dismiss());
    await pending.getByRole("button", { name: "Revoke" }).click();
    await expect(pending.getByText("Pending", { exact: true })).toBeVisible();
    expect(admin.requests).toEqual([]);

    let asked = "";
    page.once("dialog", (d) => {
      asked = d.message();
      return d.accept();
    });
    await pending.getByRole("button", { name: "Revoke" }).click();
    await expect(pending.getByText("Revoked", { exact: true })).toBeVisible();
    expect(asked).toBe("Revoke pending@friend.com? They won't be able to sign in.");
    expect(admin.requests).toEqual([
      { method: "POST", path: "/api/admin/invites/revoke", body: { email: "pending@friend.com" } },
    ]);

    await invite(page, "old@friend.com").getByRole("button", { name: "Re-invite" }).click();
    await expect(
      invite(page, "old@friend.com").getByText("Pending", { exact: true }),
    ).toBeVisible();
    expect(admin.requests.at(-1)).toEqual({
      method: "POST",
      path: "/api/admin/invites",
      body: { email: "old@friend.com", role: "member" },
    });
  });

  test("members: make an admin, disable and enable", async ({ page }) => {
    await open(page, "about:blank");
    const admin = await mockAdmin(page);
    await page.goto("/admin");
    await expect(page.getByText("3 · 1 disabled")).toBeVisible();
    await expect(member(page, "Robin").getByText("That's you")).toBeVisible();
    await expect(member(page, "Max")).toContainText("@max · Admin · max@example.com");

    await page.getByLabel("Role for Jamie").selectOption("admin");
    await expect(page.getByLabel("Role for Jamie")).toHaveValue("admin");
    await member(page, "Jamie").getByRole("button", { name: "Disable" }).click();
    await expect(member(page, "Jamie").getByText("Disabled", { exact: true })).toBeVisible();
    await expect(page.getByText("3 · 2 disabled")).toBeVisible();
    await member(page, "Max").getByRole("button", { name: "Enable" }).click();
    await expect(page.getByLabel("Role for Max")).toHaveValue("admin");
    await expect(page.getByText("3 · 1 disabled")).toBeVisible();
    expect(admin.requests).toEqual([
      { method: "PATCH", path: "/api/admin/users/u2", body: { role: "admin" } },
      { method: "PATCH", path: "/api/admin/users/u2", body: { status: "disabled" } },
      { method: "PATCH", path: "/api/admin/users/u3", body: { status: "active" } },
    ]);
  });

  test("a member change the API refuses says why", async ({ page }) => {
    await open(page, "about:blank");
    const admin = await mockAdmin(page);
    await page.goto("/admin");
    const disable = member(page, "Jamie").getByRole("button", { name: "Disable" });
    await expect(disable).toBeVisible();
    await expect(invite(page, "pending@friend.com")).toBeVisible();
    admin.fail = { status: 409, message: "there has to be an admin" };
    await disable.click();
    await expect(page.getByRole("alert")).toHaveText("there has to be an admin");
    await expect(member(page, "Jamie").getByRole("button", { name: "Disable" })).toBeVisible();
  });

  test("lists that can't load say why", async ({ page }) => {
    await open(page, "about:blank");
    const admin = await mockAdmin(page);
    admin.fail = { status: 403, message: "admins only" };
    await page.goto("/admin");
    await expect(page.getByRole("alert")).toHaveText(["admins only", "admins only", "admins only"]);
  });

  test("copies of one file: keep the first, delete the rest", async ({ page }) => {
    await open(page, "about:blank");
    const first = { ...clips.long, reactionCount: 1 };
    const copy = { ...clips.normal, uploader: { ...clips.normal.uploader, displayName: "Kim" } };
    const held = { ...clips.processing, title: "Held copy", teaser: true };
    let state = { groups: [[first, copy, held]], unchecked: 2 };
    const deleted: string[] = [];
    await page.route("**/api/admin/duplicates", (route) => json(route, state));
    await page.route("**/api/clips/*", (route) => {
      if (route.request().method() !== "DELETE") return route.fallback();
      const id = new URL(route.request().url()).pathname.split("/").pop() as string;
      deleted.push(id);
      if (deleted.length === 1) {
        return json(route, { error: "nope", message: "the file store didn't answer" }, 502);
      }
      state = { groups: [], unchecked: 0 };
      return json(route, { ...copy, deletedAt: NOW });
    });
    await page.goto("/admin");
    const group = page.getByRole("listitem", { name: `Copies of ${first.title}` });
    await expect(page.getByText("1 file", { exact: true })).toBeVisible();
    await expect(page.getByText("Kip is still checking 2 older clips")).toBeVisible();
    const row = (title: string) => group.getByRole("listitem").filter({ hasText: title });
    await expect(row(first.title)).toContainText("First upload");
    await expect(row(first.title)).toContainText("1 reaction");
    await expect(row(copy.title)).toContainText("Kim ·");
    await expect(row(copy.title).getByRole("link", { name: copy.title })).toHaveAttribute(
      "href",
      `/clips/${copy.id}`,
    );
    // Saved for the show: not until it's posted.
    await expect(row("Held copy")).toContainText("Can be deleted once it's posted");
    await expect(row("Held copy").getByRole("button")).toHaveCount(0);

    // Changed your mind: nothing happens.
    page.once("dialog", (d) => d.dismiss());
    await row(copy.title).getByRole("button", { name: "Delete" }).click();
    expect(deleted).toEqual([]);

    let asked = "";
    page.on("dialog", (d) => {
      asked = d.message();
      return d.accept();
    });
    await row(copy.title).getByRole("button", { name: "Delete" }).click();
    await expect(page.getByRole("alert")).toHaveText("the file store didn't answer");
    expect(asked).toBe(`Delete “${copy.title}” by Kim? It goes to their trash.`);
    await row(copy.title).getByRole("button", { name: "Delete" }).click();
    await expect(page.getByText("No duplicates.")).toBeVisible();
    expect(deleted).toEqual([copy.id, copy.id]);
  });

  test("Copy invite link copies the sign-up link", async ({ page, context }) => {
    await context.grantPermissions(["clipboard-read", "clipboard-write"]);
    await open(page, "/admin");
    const copy = page.getByRole("button", { name: "Copy invite link" });
    await copy.click();
    await expect(page.getByRole("button", { name: "Copied ✓" })).toBeVisible();
    expect(await page.evaluate(() => navigator.clipboard.readText())).toBe(
      `${new URL(page.url()).origin}/?invite=1`,
    );
    // And back after a moment.
    await expect(copy).toBeVisible({ timeout: 4_000 });
  });

  test("members who aren't admins are turned away", async ({ page }) => {
    await open(page, "about:blank");
    await page.route("**/api/me", (route) =>
      json(route, { ...me, role: "member", shows: false, showsForEveryone: false }),
    );
    await page.goto("/admin");
    await expect(page.getByText("Admins only.")).toBeVisible();
    await expect(page.getByRole("heading", { name: "Invites" })).toHaveCount(0);
  });
});
