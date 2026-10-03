# Security policy

clipos is a small, invite-only fan project run for one group of friends. There's one
deployment (the maintainer's) and no releases to patch, so fixes land on `main` and deploy
from there.

## Reporting a vulnerability

Please report it privately through GitHub: **Security → Report a vulnerability** on this
repository (private vulnerability reporting). Don't open a public issue or pull request for
it.

Useful to include: what's affected (file, route or workflow), how to reproduce it, and what
an attacker could do with it. You'll get an answer when the maintainer can; this is a hobby
project with no response-time promise.

## Scope

In scope: the code in this repository, its GitHub Actions workflows, and the OpenTofu in
`infra/`.

Out of scope:

- Testing against the live deployment. Run it locally instead (see the README); please
  don't send traffic, scans or sign-in attempts to the production site.
- Valve's artwork and the third-party dependencies listed in `NOTICE`; report those to
  their owners.
- Findings that need a stolen account or physical access to a device.
