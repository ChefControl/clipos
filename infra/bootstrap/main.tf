data "azurerm_client_config" "current" {}

locals {
  tags = {
    app        = "clipos"
    managed_by = "opentofu/bootstrap"
  }

  # Built-in roles the deploy identity may hand out inside rg-clipos (and nothing else).
  assignable_role_ids = [
    "ba92f5b4-2d11-453d-a403-e96b0029c9fe", # Storage Blob Data Contributor
    "2a2b9908-6ea1-4ae2-8e65-a410df84e7d1", # Storage Blob Data Reader
    "db58b8e5-c6ad-4a2a-8342-4190687cbf4a", # Storage Blob Delegator
    "4633458b-17de-408a-b874-0445c86b69e6", # Key Vault Secrets User
    "b86a8fe4-44ce-4948-aee5-eccb2c155cd7", # Key Vault Secrets Officer
    "7f951dda-4ed3-4680-a7ca-43fe172d538d", # AcrPull
    "43d0d8ad-25c7-4714-9337-8ba259a9fe05", # Monitoring Reader (Grafana)
  ]

  github_oidc_issuer = "https://token.actions.githubusercontent.com"
  github_owner       = split("/", var.github_repository)[0]
  github_repo_name   = split("/", var.github_repository)[1]
  github_subject     = "repo:${local.github_owner}@${var.github_owner_id}/${local.github_repo_name}@${var.github_repository_id}"
}

# ---------------------------------------------------------------------------
# Resource groups
# ---------------------------------------------------------------------------

# State + CI identities live apart from the app so the app stack can never delete them.
resource "azurerm_resource_group" "tfstate" {
  name     = "rg-clipos-tfstate"
  location = var.location
  tags     = local.tags
}

# Everything the app stack (infra/azure) creates goes in here.
resource "azurerm_resource_group" "app" {
  name     = "rg-clipos"
  location = var.location
  tags     = local.tags
}

# ---------------------------------------------------------------------------
# OpenTofu state storage (Entra auth only, versioned, soft-deleted)
# ---------------------------------------------------------------------------

resource "random_string" "state_suffix" {
  length  = 6
  special = false
  upper   = false
}

resource "azurerm_storage_account" "tfstate" {
  name                = "stclipostf${random_string.state_suffix.result}"
  resource_group_name = azurerm_resource_group.tfstate.name
  location            = azurerm_resource_group.tfstate.location

  account_kind             = "StorageV2"
  account_tier             = "Standard"
  account_replication_type = "ZRS"

  min_tls_version                 = "TLS1_2"
  https_traffic_only_enabled      = true
  shared_access_key_enabled       = false
  default_to_oauth_authentication = true
  allow_nested_items_to_be_public = false

  blob_properties {
    versioning_enabled = true

    delete_retention_policy {
      days = 30
    }

    container_delete_retention_policy {
      days = 30
    }
  }

  tags = local.tags
}

resource "azurerm_storage_container" "tfstate" {
  name                  = "tfstate"
  storage_account_id    = azurerm_storage_account.tfstate.id
  container_access_type = "private"
}

# You (whoever runs bootstrap) need data-plane access to read/write state.
resource "azurerm_role_assignment" "operator_state" {
  scope                = azurerm_storage_account.tfstate.id
  role_definition_name = "Storage Blob Data Contributor"
  principal_id         = data.azurerm_client_config.current.object_id
}

# ---------------------------------------------------------------------------
# GitHub Actions identities (OIDC federation, no secrets)
# ---------------------------------------------------------------------------

# Pull requests: `tofu plan` only.
resource "azurerm_user_assigned_identity" "github_plan" {
  name                = "id-clipos-github-plan"
  resource_group_name = azurerm_resource_group.tfstate.name
  location            = azurerm_resource_group.tfstate.location
  tags                = local.tags
}

resource "azurerm_federated_identity_credential" "github_plan" {
  name                      = "github-pull-request"
  user_assigned_identity_id = azurerm_user_assigned_identity.github_plan.id
  audience                  = ["api://AzureADTokenExchange"]
  issuer                    = local.github_oidc_issuer
  subject                   = "${local.github_subject}:pull_request"
}

# Reader plus the one action Reader lacks that a plan of infra/azure needs: the azurerm
# provider reads web app settings (app settings, auth settings, connection strings) through
# POST `config/list` calls, for the web apps and for the `deployed` data sources that feed
# the Postgres firewall. Without it a plan fails with AuthorizationFailed, and Azure has no
# narrower action for those reads. Still no write access. Add actions here only when a real
# plan fails on them.
resource "azurerm_role_definition" "plan_reader" {
  name        = "clipos plan reader"
  scope       = azurerm_resource_group.app.id
  description = "Reader on rg-clipos plus Microsoft.Web/sites/config/list/action, for tofu plan on pull requests."

  permissions {
    actions = [
      "*/read",
      "Microsoft.Web/sites/config/list/action",
    ]
  }

  assignable_scopes = [azurerm_resource_group.app.id]
}

resource "azurerm_role_assignment" "github_plan_reader" {
  scope              = azurerm_resource_group.app.id
  role_definition_id = azurerm_role_definition.plan_reader.role_definition_resource_id
  principal_id       = azurerm_user_assigned_identity.github_plan.principal_id
}

# Read only, so a pull request can't change or delete state. Taking the state lease is a
# write, so the Infra workflow's plan job runs `tofu plan -lock=false`; a plan never writes
# state, and applies (deploy identity) still lock.
resource "azurerm_role_assignment" "github_plan_state" {
  scope                = azurerm_storage_container.tfstate.id
  role_definition_name = "Storage Blob Data Reader"
  principal_id         = azurerm_user_assigned_identity.github_plan.principal_id
}

# Jobs on `main`: app deploys and `tofu apply`. Trust is anchored to the branch, which only the
# owner can push to, rather than to a GitHub environment.
resource "azurerm_user_assigned_identity" "github_deploy" {
  name                = "id-clipos-github-deploy"
  resource_group_name = azurerm_resource_group.tfstate.name
  location            = azurerm_resource_group.tfstate.location
  tags                = local.tags
}

resource "azurerm_federated_identity_credential" "github_deploy" {
  name                      = "github-branch-${var.github_branch}"
  user_assigned_identity_id = azurerm_user_assigned_identity.github_deploy.id
  audience                  = ["api://AzureADTokenExchange"]
  issuer                    = local.github_oidc_issuer
  subject                   = "${local.github_subject}:ref:refs/heads/${var.github_branch}"
}

resource "azurerm_role_assignment" "github_deploy_contributor" {
  scope                = azurerm_resource_group.app.id
  role_definition_name = "Contributor"
  principal_id         = azurerm_user_assigned_identity.github_deploy.principal_id
}

# Lets the app stack create role assignments for its managed identities, but only for
# the roles listed in local.assignable_role_ids and only inside rg-clipos.
resource "azurerm_role_assignment" "github_deploy_rbac_admin" {
  scope                = azurerm_resource_group.app.id
  role_definition_name = "Role Based Access Control Administrator"
  principal_id         = azurerm_user_assigned_identity.github_deploy.principal_id

  condition_version = "2.0"
  condition         = <<-EOT
    (
      (
        !(ActionMatches{'Microsoft.Authorization/roleAssignments/write'})
      )
      OR
      (
        @Request[Microsoft.Authorization/roleAssignments:RoleDefinitionId] ForAnyOfAnyValues:GuidEquals {${join(", ", local.assignable_role_ids)}}
      )
    )
    AND
    (
      (
        !(ActionMatches{'Microsoft.Authorization/roleAssignments/delete'})
      )
      OR
      (
        @Resource[Microsoft.Authorization/roleAssignments:RoleDefinitionId] ForAnyOfAnyValues:GuidEquals {${join(", ", local.assignable_role_ids)}}
      )
    )
  EOT
}

resource "azurerm_role_assignment" "github_deploy_state" {
  scope                = azurerm_storage_container.tfstate.id
  role_definition_name = "Storage Blob Data Contributor"
  principal_id         = azurerm_user_assigned_identity.github_deploy.principal_id
}
