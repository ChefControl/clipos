# infra/azure

The app itself, everything inside `rg-clipos` (created empty by `infra/bootstrap`):

| Resource | Notes |
|---|---|
| `asp-clipos` | Linux App Service plan, B2 |
| `clipos-api-<suffix>`, `clipos-worker-<suffix>` | Web apps running one container each, with system-assigned identities. The image tag is ignored here; `deploy.yml` rolls it out. Their Kudu (SCM) sites deny everyone except `scm_allowed_ips` |
| `clipos-grafana-<suffix>` | Grafana on the same plan (`grafana.tf`): dashboards over this resource group's metrics and logs, at `grafana.clips.spawnpoint.run`. See [Grafana](#grafana) |
| `psql-clipos-<suffix>` | Postgres 17 Flexible Server, B1ms. Entra-only login; firewall = the web apps' outbound IPs; connection throttling on. Alert `clipos-postgres-failed-connections` mails `ADMIN_EMAILS` when logins keep failing |
| `stclipos<suffix>` | Media storage (`originals`, `playback`, `posters`, `models`); shared keys off; CORS for the app's origin only. The worker's roles are per container and only read `models` |
| `crclipos<suffix>` | Container registry (Basic); the web apps pull with `AcrPull` |
| `kv-clipos-<suffix>` | Key Vault, RBAC mode |
| `log-clipos` | Log Analytics; App Service console/HTTP/platform logs and Postgres logs |
| Alerts | `ag-clipos-admins` mails `ADMIN_EMAILS`: `clipos-postgres-failed-connections` (postgres.tf), `clipos-job-failures` and `clipos-api-5xx` (alerts.tf, log alerts over `log-clipos`; thresholds are variables) |
| `clips.spawnpoint.run` | DNS zone, delegated from Namecheap |

Applied from CI only: pull requests get a plan comment, and **Actions → Infra → Run workflow** (stack `azure`) on `main` applies.

## First apply (once)

The Postgres firewall rules come from the outbound IPs of the web apps that already exist (`for_each` can't use values known only after apply). So the first apply creates everything except those rules; **run the workflow a second time** to add them. Later applies pick up any IP changes the same way.

Then add the zone's name servers at Namecheap (**Domain List → spawnpoint.run → Advanced DNS → add four `NS` records, host `clips`**):

```bash
tofu -chdir=infra/azure output name_servers
```

Once `dig NS clips.spawnpoint.run +short` returns them, an apply with `custom_domain_enabled = true` (the default since the delegation went live) adds the apex `A` and `asuid` TXT records, binds the hostname and issues the free managed certificate.

## Admins and the invite check

- `ADMIN_EMAILS` (always-invited admins) comes from the GitHub repository secret of the same name, so no address is committed: `gh secret set ADMIN_EMAILS --body "you@example.com"`. It has to be a secret: the runner doesn't mask variables, and the workflow's step headers would print the addresses. The workflow still falls back to a variable of that name, so delete any left from an older setup (`gh variable delete ADMIN_EMAILS`). The api seeds those invites at every start. The same addresses get the budget mails and the Azure Monitor alerts (action group `ag-clipos-admins`).
- `admin_user` is your own Entra account: the second Postgres Entra admin, read access to the clip storage and write access to `models`. It isn't committed either. The Infra workflow passes it as `TF_VAR_admin_user` from the repository secret `ADMIN_USER`, a JSON object:

  ```bash
  # {"object_id": "00000000-0000-0000-0000-000000000000", "upn": "you@yourtenant.onmicrosoft.com"}
  gh secret set ADMIN_USER --body "$(az ad signed-in-user show --query '{object_id: id, upn: userPrincipalName}' -o json)"
  ```

  For a local `tofu plan`, export the same JSON as `TF_VAR_admin_user`.
- `invite-check-secret` in Key Vault is the shared secret between the Auth0 post-login Action and `POST /internal/invites/check`. The api reads it through a Key Vault reference; `infra/auth0` reads the `invite_check_secret` output, so apply this stack before that one.

## Grafana

`https://grafana.clips.spawnpoint.run` (the `grafana_url` output). Sign in with Google; only `ADMIN_EMAILS` get in, as Grafana admins. Five dashboards in the `clipos` folder: **overview** (the home page), **api**, **worker**, **infrastructure** and **logs**.

- **Data:** one data source, Azure Monitor, signed in as the web app's managed identity (`Monitoring Reader` on `rg-clipos`). Metrics are Azure's platform metrics; everything else is KQL over `log-clipos` (the apps' JSON console logs, App Service HTTP and platform logs, Postgres logs). When the workspace hits its 100 MB daily cap, the log panels stop at that point until 00:00 UTC (the overview's "Logs today" shows how close it is); the metric panels keep going.
- **Changing a dashboard:** they're files in the image (`deploy/grafana/dashboards/`). Edit in the UI to try something, then **Export → Export as JSON**, replace the file and open a pull request; the UI can't save over a provisioned dashboard, and a restart drops UI-only changes.
- **Sign-in:** Auth0 client `clipos-grafana` (infra/auth0). Its secret goes from the auth0 stack's state to Key Vault (`grafana-oauth-client-secret`) through this stack, which reads that state, so the azure apply after an auth0 change picks it up. There's no local admin account and no password login.
- **First rollout:** `infra/bootstrap` must allow `Monitoring Reader` (a local bootstrap apply) before the azure apply that creates the app. On the merge, Infra applies azure (creates the app, sign-in off: there's no Auth0 client yet), then auth0 (creates the client). Then **Infra → Run workflow → azure** turns sign-in on, and **Deploy → Run workflow** rolls out the image if the deploy ran before the app existed.

## Database access

Login roles are not managed by OpenTofu: they're SQL inside Postgres. `deploy/db/bootstrap.sh` runs in the Infra workflow right after every `azure` apply, as the deploy identity (one of the two Entra admins). It creates the `clipos` database and the roles below, and re-points existing roles at the web apps' current identities if the apps were recreated. After changing `deploy/db/`, run an `azure` apply (with no infra changes it only re-runs this).

| Role | Rights |
|---|---|
| `clipos-api` | `CREATE` on schema `public`; runs migrations, so it owns every table |
| `clipos-worker` | read/write on the api's tables, granted by the api at startup (`DB_WORKER_ROLE`) |

For an ad-hoc `psql` session as yourself (the other Entra admin), add your IP to `admin_ips`, apply, then:

```bash
export PGPASSWORD=$(az account get-access-token --resource-type oss-rdbms --query accessToken -o tsv)
psql "host=$(tofu -chdir=infra/azure output -raw postgres_fqdn) user=you@yourtenant.onmicrosoft.com dbname=clipos sslmode=require"
```

`user` is the `upn` of `admin_user`. Remove the IP again when you're done. `scm_allowed_ips` works the same way for the portal's log stream and SSH, which go through the Kudu site.
