data "azurerm_client_config" "current" {}

data "azurerm_resource_group" "app" {
  name = var.resource_group_name
}

locals {
  state_backend = {
    resource_group_name  = "rg-clipos-tfstate"
    storage_account_name = "stclipostfnurb2t"
    container_name       = "tfstate"
    use_azuread_auth     = true
  }
}

# SPA client ID and audience come from the auth0 stack.
data "terraform_remote_state" "auth0" {
  backend = "azurerm"
  config  = merge(local.state_backend, { key = "auth0.tfstate" })
}

locals {
  location = data.azurerm_resource_group.app.location
  rg       = data.azurerm_resource_group.app.name
  suffix   = random_string.suffix.result

  admin_email_list = [for e in split(",", var.admin_emails) : trimspace(e) if trimspace(e) != ""]

  tags = {
    app        = "clipos"
    managed_by = "opentofu/azure"
  }
}

# Storage, ACR, Key Vault, Postgres and web app names are global; one suffix keeps them unique.
resource "random_string" "suffix" {
  length  = 6
  special = false
  upper   = false
}

resource "azurerm_log_analytics_workspace" "main" {
  name                = "log-clipos"
  resource_group_name = local.rg
  location            = local.location
  sku                 = "PerGB2018"
  retention_in_days   = 30
  # About 9 MB a day, 5 of them Postgres connection lines; the cap only stops a log loop
  # from turning into a bill (ingestion pauses until the next UTC day).
  daily_quota_gb = 0.1
  tags           = local.tags
}

resource "azurerm_container_registry" "main" {
  name                = "crclipos${local.suffix}"
  resource_group_name = local.rg
  location            = local.location
  sku                 = "Basic"
  admin_enabled       = false
  tags                = local.tags
}

# Every deploy pushes a tag per image; keep the last 10 (the rollback window) so the
# registry stays inside Basic's included 10 GB. Runs weekly as an ACR task.
resource "azurerm_container_registry_task" "purge" {
  name                  = "purge-old-tags"
  container_registry_id = azurerm_container_registry.main.id
  tags                  = local.tags

  platform {
    os = "Linux"
  }

  encoded_step {
    task_content = base64encode(<<-EOT
      version: v1.1.0
      steps:
        - cmd: acr purge --filter 'clipos-api:.*' --filter 'clipos-worker:.*' --ago 0d --keep 10 --untagged
          disableWorkingDirectoryOverride: true
          timeout: 3600
    EOT
    )
  }

  timer_trigger {
    name     = "weekly"
    schedule = "0 2 * * 0"
    enabled  = true
  }
}

# Cost alert for the app's resource group (none existed before the 2026-10-03 cost audit;
# the run rate after it is about $60 a month). Mails ADMIN_EMAILS and the group's Owners.
resource "azurerm_consumption_budget_resource_group" "main" {
  name              = "clipos-monthly"
  resource_group_id = data.azurerm_resource_group.app.id
  amount            = var.monthly_budget
  time_grain        = "Monthly"

  time_period {
    start_date = "2026-10-01T00:00:00Z"
  }

  dynamic "notification" {
    for_each = {
      actual_80    = { threshold = 80, type = "Actual" }
      actual_100   = { threshold = 100, type = "Actual" }
      forecast_100 = { threshold = 100, type = "Forecasted" }
    }

    content {
      enabled        = true
      operator       = "GreaterThan"
      threshold      = notification.value.threshold
      threshold_type = notification.value.type
      contact_emails = local.admin_email_list
      contact_roles  = ["Owner"]
    }
  }
}

# Who Azure Monitor alerts mail (postgres.tf): the same ADMIN_EMAILS as the budget.
resource "azurerm_monitor_action_group" "admins" {
  name                = "ag-clipos-admins"
  resource_group_name = local.rg
  short_name          = "clipos"

  dynamic "email_receiver" {
    for_each = local.admin_email_list

    content {
      name                    = "admin-${email_receiver.key}"
      email_address           = email_receiver.value
      use_common_alert_schema = true
    }
  }

  tags = local.tags
}

# RBAC mode. Phase 3 adds the Auth0 Action's shared secret here.
resource "azurerm_key_vault" "main" {
  name                       = "kv-clipos-${local.suffix}"
  resource_group_name        = local.rg
  location                   = local.location
  tenant_id                  = data.azurerm_client_config.current.tenant_id
  sku_name                   = "standard"
  rbac_authorization_enabled = true
  soft_delete_retention_days = 7
  purge_protection_enabled   = false
  tags                       = local.tags
}
