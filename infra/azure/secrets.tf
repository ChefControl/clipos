# Shared secret between the Auth0 post-login Action and the api's /internal/invites/check.
# The api reads it through a Key Vault reference; infra/auth0 reads the
# `invite_check_secret` output and sets it as an Action secret.
resource "random_password" "invite_check" {
  length  = 48
  special = false
}

# CI writes the secret, and PR plans (which refresh it) read it. Both roles are within the
# bootstrap's RBAC condition, so this stack can grant them itself.
#
# The plan identity keeps Secrets User: refreshing `azurerm_key_vault_secret.invite_check`
# is a Get Secret call, which returns the value, so a plan without it fails with 403. It
# reveals nothing new either, since the same value sits in this stack's state, which plans
# have to read.
resource "azurerm_role_assignment" "deploy_kv_officer" {
  scope                = azurerm_key_vault.main.id
  role_definition_name = "Key Vault Secrets Officer"
  principal_id         = var.deploy_identity.object_id
  principal_type       = "ServicePrincipal"
}

resource "azurerm_role_assignment" "plan_kv_reader" {
  scope                = azurerm_key_vault.main.id
  role_definition_name = "Key Vault Secrets User"
  principal_id         = var.plan_identity_object_id
  principal_type       = "ServicePrincipal"
}

# Role assignments take a minute or two to reach Key Vault's data plane.
resource "time_sleep" "kv_officer_propagation" {
  create_duration = "120s"
  depends_on      = [azurerm_role_assignment.deploy_kv_officer]
}

resource "azurerm_key_vault_secret" "invite_check" {
  name         = "invite-check-secret"
  value        = random_password.invite_check.result
  key_vault_id = azurerm_key_vault.main.id
  content_type = "Shared secret: Auth0 post-login Action -> /internal/invites/check"

  depends_on = [time_sleep.kv_officer_propagation]
}
