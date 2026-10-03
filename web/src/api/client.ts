import createClient, { type Middleware } from "openapi-fetch";
import type { paths } from "./schema";

export type ApiClient = ReturnType<typeof createApiClient>;

/** Typed client for the clipos API. Every request carries a fresh Auth0 access token. */
export function createApiClient(getToken: () => Promise<string>) {
  const client = createClient<paths>({ baseUrl: window.location.origin });
  const auth: Middleware = {
    async onRequest({ request }) {
      request.headers.set("Authorization", `Bearer ${await getToken()}`);
      return request;
    },
  };
  client.use(auth);
  return client;
}
