# infra/bootstrap

One-time stack, applied **locally** by a subscription Owner. It creates:

| Resource | Purpose |
|---|---|
| `rg-clipos-tfstate` | Holds state + CI identities, out of reach of the app stack |
| `stclipostf<suffix>` / `tfstate` | OpenTofu state for all stacks. Entra-only auth, ZRS, versioning, 30-day soft delete |
| `rg-clipos` | Empty resource group the app stack (`infra/azure`) fills |
| `id-clipos-github-plan` | GitHub OIDC for `pull_request` jobs. Custom role `clipos plan reader` on `rg-clipos` (Reader plus `Microsoft.Web/sites/config/list/action`, which plans need to read web app settings) and **read only** on the state container, so plans run with `-lock=false`. `infra/azure` adds Key Vault Secrets User, which refreshing the invite-check secret needs |
| `id-clipos-github-deploy` | GitHub OIDC for workflow runs on `main` (trusted by branch, not by a GitHub environment). Contributor on `rg-clipos`, plus RBAC Admin **restricted by condition** to 6 data-plane roles |

It also registers the resource providers clipos uses (only `Microsoft.AlertsManagement` was missing).

The plan identity can't take the state lease, so the plan job in `.github/workflows/infra.yml` runs `tofu plan -lock=false`. Without the flag, pull-request plans fail on the lease.

## Apply

```bash
cd infra/bootstrap
export ARM_SUBSCRIPTION_ID=$(az account show --query id -o tsv)
tofu init
tofu plan -out bootstrap.tfplan
tofu apply bootstrap.tfplan
```

## Move this stack's state into Azure

```bash
# backend.tf is committed (account stclipostfnurb2t); on a fresh bootstrap, recreate it with the new name

tofu init -migrate-state
rm -f terraform.tfstate terraform.tfstate.backup
```

## Wire up GitHub

Repository secrets, so the runner masks the IDs in logs, inside resource IDs too:

```bash
repo=OWNER/NAME   # the repository in github_repository
gh secret set AZURE_TENANT_ID        --repo "$repo" --body "$(tofu output -raw tenant_id)"
gh secret set AZURE_SUBSCRIPTION_ID  --repo "$repo" --body "$(tofu output -raw subscription_id)"
gh secret set AZURE_CLIENT_ID_PLAN   --repo "$repo" --body "$(tofu output -raw github_plan_client_id)"
gh secret set AZURE_CLIENT_ID_DEPLOY --repo "$repo" --body "$(tofu output -raw github_deploy_client_id)"
```

Not variables: the runner doesn't mask those, so every step that uses them prints them. Delete any left from an older setup (`gh variable delete AZURE_TENANT_ID --repo "$repo"`, and the same for the other three).

Pull-request jobs log in with `AZURE_CLIENT_ID_PLAN`. Jobs on `main` (app deploys and `tofu apply`) use `AZURE_CLIENT_ID_DEPLOY`. Azure only trusts each identity for its matching GitHub subject.

## Point CI at another repository

The federated credentials trust one repository by its numeric IDs, so a new repository (a fresh public copy, say) can't log in to Azure until this stack points at it. The old one loses access at the same time.

1. Set `github_repository`, `github_owner_id` and `github_repository_id` in `variables.tf` to the new repository. `gh api repos/OWNER/NAME --jq '{id, owner_id: .owner.id}'` prints the IDs.
2. Apply this stack again (see [Apply](#apply)). Only the two federated credentials change; the identities, their client IDs and their roles stay, so `infra/azure` needs nothing.
3. Check that the new repository uses the same ID-based OIDC subject: `gh api repos/OWNER/NAME/actions/oidc/customization/sub` should show `"sub_claim_prefix": "repo:OWNER@<owner_id>/NAME@<repo_id>"`.
4. Set the four secrets above on the new repository, then the rest the workflows read: the secrets `ADMIN_EMAILS` and `ADMIN_USER` (`infra/azure/README.md`), `AUTH0_CLIENT_ID`, `AUTH0_CLIENT_SECRET` and `GOOGLE_CLIENT_SECRET` (write-only in Auth0: it's only sent when `google_client_secret_version` is bumped, so it's needed for rotating the Google secret, not for plans), and the variables `AUTH0_DOMAIN` and `GOOGLE_CLIENT_ID`, which are public anyway.
