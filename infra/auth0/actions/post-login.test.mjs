// Tests for the post-login Action: `node --test infra/auth0/actions/*.test.mjs`.
import assert from "node:assert/strict";
import { createRequire } from "node:module";
import { afterEach, describe, it } from "node:test";

const { onExecutePostLogin } = createRequire(import.meta.url)("./post-login.js");

const secrets = {
  CLIENT_IDS: "prod-client,dev-client",
  PROD_CLIENT_ID: "prod-client",
  PROD_AUDIENCE: "https://clips.spawnpoint.run/api",
  INVITE_CHECK_URL: "https://clips.spawnpoint.run/internal/invites/check",
  INVITE_CHECK_SECRET: "s3cret",
};

function event(overrides = {}) {
  return {
    secrets,
    client: { client_id: "prod-client" },
    connection: { strategy: "google-oauth2" },
    resource_server: { identifier: "https://clips.spawnpoint.run/api" },
    user: { email: "Friend@Gmail.com", email_verified: true, name: "Friend", picture: "p.png" },
    ...overrides,
  };
}

function recorder() {
  const result = { denied: null, claims: {} };
  return {
    result,
    api: {
      access: { deny: (reason) => (result.denied = reason) },
      accessToken: { setCustomClaim: (k, v) => (result.claims[k] = v) },
    },
  };
}

const realFetch = globalThis.fetch;
const calls = [];
function mockFetch(response) {
  globalThis.fetch = async (url, init) => {
    calls.push({ url, init });
    if (response instanceof Error) throw response;
    return new Response(JSON.stringify(response.body ?? {}), { status: response.status ?? 200 });
  };
}

afterEach(() => {
  globalThis.fetch = realFetch;
  calls.length = 0;
});

describe("post-login", () => {
  it("ignores other apps in the shared tenant", async () => {
    mockFetch(new Error("must not be called"));
    const { api, result } = recorder();
    await onExecutePostLogin(event({ client: { client_id: "someone-else" } }), api);
    assert.equal(result.denied, null);
    assert.deepEqual(result.claims, {});
  });

  it("denies non-Google and unverified logins", async () => {
    for (const ev of [
      event({ connection: { strategy: "auth0" } }),
      event({ user: { email: "a@b.com", email_verified: false } }),
    ]) {
      const { api, result } = recorder();
      await onExecutePostLogin(ev, api);
      assert.ok(result.denied);
    }
  });

  it("lets invited prod logins in with claims", async () => {
    mockFetch({ body: { allowed: true } });
    const { api, result } = recorder();
    await onExecutePostLogin(event(), api);

    assert.equal(result.denied, null);
    assert.equal(result.claims["https://clips.spawnpoint.run/email"], "friend@gmail.com");
    assert.equal(calls.length, 1);
    assert.equal(calls[0].url, secrets.INVITE_CHECK_URL);
    assert.equal(calls[0].init.method, "POST");
    assert.equal(calls[0].init.headers["x-clipos-internal-secret"], "s3cret");
    assert.deepEqual(JSON.parse(calls[0].init.body), { email: "friend@gmail.com" });
  });

  it("denies prod logins that aren't invited", async () => {
    mockFetch({ body: { allowed: false } });
    const { api, result } = recorder();
    await onExecutePostLogin(event(), api);
    assert.match(result.denied, /isn't invited/);
    assert.deepEqual(result.claims, {});
  });

  it("fails closed when the check is unreachable or errors", async () => {
    for (const response of [new Error("network down"), { status: 503 }]) {
      mockFetch(response);
      const { api, result } = recorder();
      await onExecutePostLogin(event(), api);
      assert.match(result.denied, /couldn't check/);
    }
  });

  it("skips the check for dev logins (Auth0 can't reach localhost)", async () => {
    mockFetch(new Error("must not be called"));
    const { api, result } = recorder();
    await onExecutePostLogin(
      event({
        client: { client_id: "dev-client" },
        resource_server: { identifier: "http://localhost:8080/api" },
      }),
      api,
    );
    assert.equal(result.denied, null);
    assert.equal(calls.length, 0);
  });

  it("checks a dev-app login that asks for the prod audience", async () => {
    mockFetch({ body: { allowed: false } });
    const { api, result } = recorder();
    await onExecutePostLogin(event({ client: { client_id: "dev-client" } }), api);
    assert.ok(result.denied);
  });
});
