import type { components } from "./schema";

type ErrorBody = components["schemas"]["ErrorBody"];
type Ties = components["schemas"]["Ties"];

/** A non-2xx API response. `code` is the stable `error` field (`not_invited`, …). */
export class ApiError extends Error {
  readonly code: string;
  readonly status: number;
  /** Ending a show with a tied vote: the tied clips of each category (409). */
  readonly tied: Ties | null;

  constructor(status: number, body: unknown) {
    const parsed = isErrorBody(body) ? body : undefined;
    super(parsed?.message ?? `request failed (${status})`);
    this.name = "ApiError";
    this.code = parsed?.error ?? "unknown";
    this.status = status;
    this.tied = parsed?.tied ?? null;
  }
}

function isErrorBody(body: unknown): body is ErrorBody {
  return (
    typeof body === "object" &&
    body !== null &&
    typeof (body as ErrorBody).error === "string" &&
    typeof (body as ErrorBody).message === "string"
  );
}

/** Awaits an openapi-fetch call and returns its data, throwing `ApiError` on failure. */
export async function call<T>(
  request: Promise<{ data?: T; error?: unknown; response: Response }>,
): Promise<T> {
  const { data, error, response } = await request;
  if (!response.ok || error !== undefined) {
    throw new ApiError(response.status, error);
  }
  return data as T;
}

/** The signed-in Google account isn't allowed in (no invite, or disabled). */
export function isAccessDenied(error: unknown): error is ApiError {
  return (
    error instanceof ApiError && (error.code === "not_invited" || error.code === "account_disabled")
  );
}

/** The thing asked for doesn't exist (or is gone, e.g. a purged clip). */
export function isNotFound(error: unknown): boolean {
  return error instanceof ApiError && error.status === 404;
}

/** Auth0 can't give a token without signing in again (the refresh token ran out or was
 *  revoked). */
export class SignInRequired extends Error {
  constructor(cause?: unknown) {
    super("Your sign-in ran out. Sign in again.", { cause });
    this.name = "SignInRequired";
  }
}

// Auth0's `error` codes for "only an interactive sign-in can fix this".
const SIGN_IN_CODES = [
  "login_required",
  "consent_required",
  "interaction_required",
  "missing_refresh_token",
  "invalid_grant",
];

/** `getAccessTokenSilently` failed because the session is gone, not the network. */
export function isAuth0SignInError(error: unknown): boolean {
  const code = (error as { error?: unknown } | null)?.error;
  return typeof code === "string" && SIGN_IN_CODES.includes(code);
}

/** Signing in again is the only way forward: the API said 401, or there's no token. */
export function needsSignIn(error: unknown): boolean {
  return error instanceof SignInRequired || (error instanceof ApiError && error.status === 401);
}

/** Worth asking again: the server or the network had a moment. Not other 4xx answers,
 *  which won't change by asking again. */
export function isTransient(error: unknown): boolean {
  if (error instanceof ApiError) {
    return error.status >= 500 || error.status === 408 || error.status === 429;
  }
  return !(error instanceof SignInRequired);
}

/** A React Query `retry` that tries transient failures again, up to `max` times. */
export const retryTransient = (max: number) => (failures: number, error: Error) =>
  failures < max && isTransient(error);
