import tailwindcss from "@tailwindcss/vite";
import react from "@vitejs/plugin-react";
import istanbul from "vite-plugin-istanbul";
import { defineConfig } from "vitest/config";

// The api's content security policy (crates/api/src/security.rs) for the e2e build, so the
// browser tests catch app code the policy would block. Auth0 and Blob Storage are mocked
// on the page's own origin in e2e, so their hosts aren't needed here.
const E2E_CSP = [
  "default-src 'self'",
  "script-src 'self'",
  "style-src 'self' 'unsafe-inline'",
  "img-src 'self' data: blob: https://*.googleusercontent.com https://s.gravatar.com https://cdn.auth0.com",
  "media-src 'self' blob:",
  "font-src 'self'",
  "connect-src 'self'",
  "worker-src 'self' blob:",
  "frame-ancestors 'none'",
  "object-src 'none'",
  "base-uri 'self'",
  "form-action 'self'",
].join("; ");

export default defineConfig(({ mode }) => {
  // `--mode e2e-coverage` (`COVERAGE=1 pnpm e2e`): the e2e build, instrumented, so the
  // browser tests can save what ran (e2e/test.ts) for `pnpm coverage`.
  const coverage = mode === "e2e-coverage";
  const e2e = mode === "e2e" || coverage;
  return {
    plugins: [
      react(),
      tailwindcss(),
      ...(coverage
        ? [istanbul({ include: "src/**", extension: [".ts", ".tsx"], forceBuildInstrument: true })]
        : []),
    ],
    // `vite build --mode e2e`: the phone tests' build, with Auth0 replaced by a stub.
    resolve: e2e
      ? {
          alias: {
            "@auth0/auth0-react": decodeURIComponent(
              new URL("./e2e/auth0-stub.tsx", import.meta.url).pathname,
            ),
          },
        }
      : undefined,
    server: {
      port: 5173,
      strictPort: true,
      // The Rust api serves /api in production from the same origin; mirror that in dev.
      proxy: {
        // `ws` passes websocket upgrades on too: the show's live connection is /api/shows/{id}/live.
        "/api": { target: "http://localhost:8080", changeOrigin: true, ws: true },
        "/healthz": "http://localhost:8080",
        // Share-link media and data; the /s/{token} page itself is the SPA in dev.
        "^/s/[^/]+/(video\\.mp4|poster\\.jpg|clip\\.json)$": "http://localhost:8080",
      },
    },
    preview: e2e ? { headers: { "Content-Security-Policy": E2E_CSP } } : undefined,
    build: {
      sourcemap: true,
    },
    test: {
      environment: "node",
      // Playwright specs live in e2e/ and run with `pnpm e2e`.
      exclude: ["e2e/**", "node_modules/**"],
      coverage: {
        provider: "istanbul",
        include: ["src/**/*.{ts,tsx}"],
        exclude: ["src/**/*.test.ts", "src/api/schema.d.ts"],
        reporter: ["json"],
        reportsDirectory: "coverage/unit",
      },
    },
  };
});
