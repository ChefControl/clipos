import type { PublicConfig } from "./api/schemaTypes";

/** Runtime config from the API, so one build works in every environment. */
export async function loadConfig(): Promise<PublicConfig> {
  const res = await fetch("/api/config");
  if (!res.ok) {
    throw new Error(`GET /api/config failed: ${res.status}`);
  }
  return res.json();
}
