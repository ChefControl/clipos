# clipos

A place to remember the arena clips: an invite-only clip archive for a group of friends,
starting with Counter-Strike 2. A fan project, not affiliated with or endorsed by Valve.

- **Backend:** Rust (axum, sqlx, Postgres), `crates/`
- **Frontend:** TypeScript (React, Vite, TanStack Router + Query, Tailwind), `web/`
- **Auth:** Google sign-in through Auth0 (tenant `spawnpoint`), `infra/auth0/`
- **Infra:** Azure Israel Central (App Service, Postgres Flexible, Blob Storage), OpenTofu, `infra/`

Design, phases and decisions: [docs/PLAN.md](docs/PLAN.md).

## Layout

```
crates/core     shared: config, db + migrations, Auth0 JWT verification, users, job queue
crates/api      HTTP API (+ serves the built SPA); `openapi` bin prints the spec
crates/worker   background job runner (Postgres queue, /healthz for App Service)
migrations/     sqlx migrations, applied by the api at startup
web/            SPA; API types generated from web/openapi.json
infra/          bootstrap (state + CI identities), auth0
deploy/         Dockerfiles
```

## Local development

Needs Docker, Rust (pinned by `rust-toolchain.toml`), Node 22 + pnpm, and OpenTofu for infra work.

```bash
cp .env.example .env          # then fill AUTH0_CLIENT_ID (see below)
make deps                     # Postgres :5432 + Azurite :10000
make api                      # :8080, applies migrations
make worker                   # health on :8081
pnpm --dir web install
make web                      # http://localhost:5173
```

`AUTH0_CLIENT_ID` is the public client ID of the `clipos-web-dev` Auth0 app, created by
`infra/auth0`:

```bash
tofu -chdir=infra/auth0 init && tofu -chdir=infra/auth0 output -raw web_dev_client_id
```

`make help` lists everything else (`test`, `lint`, `fmt`, `gen-api`, `images`). Tests need
`make deps` running; `sqlx::test` creates a throwaway database per test.

After changing an API handler or type, run `make gen-api` and commit `web/openapi.json` and
`web/src/api/schema.d.ts`. CI fails if they are stale.

## CI and CD

**Pull request gates** (`.github/workflows/ci.yml`): on every push to a PR branch, after a `changes` job that
skips what a change can't affect (changing the workflow runs everything), each gate runs on its own, in
parallel:
- tests with coverage: `rust-coverage` (nextest, against Postgres, Azurite, ffmpeg and ONNX Runtime) and
  `web-coverage`, which adds up `web-unit` (Vitest) and the browser tests, one job per device side by side
  (`web-e2e`: iphone, android, and pc in two shards); below 95 % of lines fails;
- checks: Rust fmt/clippy, `cargo deny`, the web's generated-types freshness, lint, audit, typecheck and build,
  the Auth0 action tests and `tofu fmt`.

CI also runs on `main` after each merge, only so its caches are saved where every PR can restore them.

Results count for the commit they ran on only, and only while the branch holds the latest main: a PR is merged
when its gates are green on its last commit and it's up to date with main; if main moved, update the branch and
let CI pass again.

**CD on main** runs automatically after a merge, once `.github/workflows/gate.yml` confirms the merge is an
up-to-date PR from this repo whose CI passed (a stale branch, a direct push, a fork's PR or failed CI stops
there; *Run workflow* by hand skips the gate):
- **Deploy** (`.github/workflows/deploy.yml`): merges that touch the app. Builds both binaries and the SPA on the
  runner with warm caches, in a job with no Azure access; the next job wraps them in the images
  (`deploy/*.Dockerfile`, which still build from source locally), pushes them, and rolls both web apps.
- **Infra** (`.github/workflows/infra.yml`): a plan summary comment on PRs from this repo's branches touching
  `infra/` (tofu's `Plan:` line and the addresses that change, no attribute values), which is the review; a merge
  applies the stacks it changed, refusing any plan that destroys or replaces a resource (those apply via
  *Run workflow* on `main`). An `azure` apply then runs `deploy/db/bootstrap.sh`: the database and the apps' login
  roles.

**Audit** (`.github/workflows/audit.yml`), Mondays and by hand: `cargo deny` advisories, `pnpm audit`, and a Trivy
scan of the Debian packages in both images.

The logs and comments are public, so the workflows print no full plan or apply output, the Azure IDs and admin
emails are secrets (masked in logs; in a public repo, a variable of the same name stops the run, since variables
aren't masked), and resource names and hostnames are masked as they're looked up. Each
workflow starts with no token permissions and grants every job only what it needs; only the jobs that log in to
Azure can get an OIDC token, and secrets reach only the steps that use them. Actions are pinned to commit SHAs
and the workflows' images by digest; Dependabot (`.github/dependabot.yml`) proposes updates weekly.

## Licence

MIT, see [LICENSE](LICENSE). That covers clipos's own code and assets only: the CS2 killfeed
icons and HUD templates are Valve's artwork and are excluded, and [NOTICE](NOTICE) lists them
with the other third-party material. clipos is not affiliated with or endorsed by Valve
Corporation; Counter-Strike 2 and CS2 are its trademarks.
