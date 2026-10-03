import { describe, expect, it } from "vitest";
import {
  ApiError,
  call,
  isAccessDenied,
  isAuth0SignInError,
  isTransient,
  needsSignIn,
  retryTransient,
  SignInRequired,
} from "./errors";

const response = (status: number) => new Response(null, { status });

describe("call", () => {
  it("returns data for 2xx", async () => {
    await expect(
      call(Promise.resolve({ data: { ok: 1 }, response: response(200) })),
    ).resolves.toEqual({ ok: 1 });
  });

  it("throws ApiError with the error code", async () => {
    const failing = call(
      Promise.resolve({
        error: { error: "not_invited", message: "not invited" },
        response: response(403),
      }),
    );
    await expect(failing).rejects.toMatchObject({ code: "not_invited", status: 403 });
  });

  it("copes with a non-JSON error body", async () => {
    const err = new ApiError(502, "Bad Gateway");
    expect(err.code).toBe("unknown");
    expect(err.message).toBe("request failed (502)");
  });
});

describe("isAccessDenied", () => {
  it("matches only the sign-in refusals", () => {
    expect(isAccessDenied(new ApiError(403, { error: "not_invited", message: "" }))).toBe(true);
    expect(isAccessDenied(new ApiError(403, { error: "account_disabled", message: "" }))).toBe(
      true,
    );
    expect(isAccessDenied(new ApiError(403, { error: "forbidden", message: "" }))).toBe(false);
    expect(isAccessDenied(new Error("x"))).toBe(false);
  });
});

describe("needsSignIn and isTransient", () => {
  it("sends 401s and lost sessions to sign-in", () => {
    expect(needsSignIn(new ApiError(401, { error: "unauthorized", message: "" }))).toBe(true);
    expect(needsSignIn(new SignInRequired())).toBe(true);
    expect(needsSignIn(new ApiError(403, { error: "forbidden", message: "" }))).toBe(false);
    expect(isAuth0SignInError({ error: "login_required" })).toBe(true);
    expect(isAuth0SignInError(new Error("network"))).toBe(false);
  });

  it("retries server and network failures, not refusals", () => {
    expect(isTransient(new ApiError(500, null))).toBe(true);
    expect(isTransient(new ApiError(503, null))).toBe(true);
    expect(isTransient(new TypeError("Failed to fetch"))).toBe(true);
    expect(isTransient(new ApiError(401, null))).toBe(false);
    expect(isTransient(new ApiError(404, null))).toBe(false);
    expect(isTransient(new SignInRequired())).toBe(false);
    const retry = retryTransient(3);
    expect(retry(2, new ApiError(500, null))).toBe(true);
    expect(retry(3, new ApiError(500, null))).toBe(false);
  });
});
