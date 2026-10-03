# B2 (2 cores, 3.5 GB) since the 2026-10-03 cost audit: on P1v3 the plan averaged 3–6% CPU,
# the api peaked at 111 MB and the worker at 1.3 GB. Go to B3 (4 cores, 7 GB) if memory
# stays above 85% or jobs get too slow. Basic keeps Always On, health checks, websockets and
# the managed certificate; the show's in-memory hub needs a single instance anyway.
resource "azurerm_service_plan" "main" {
  name                = "asp-clipos"
  resource_group_name = local.rg
  location            = local.location
  os_type             = "Linux"
  sku_name            = "B2"
  tags                = local.tags
}

locals {
  pg_url = "postgres://%s@${azurerm_postgresql_flexible_server.main.fqdn}:5432/clipos?sslmode=verify-full"

  apps = {
    api = {
      port = 8080
      settings = {
        DATABASE_URL    = format(local.pg_url, "clipos-api")
        DB_WORKER_ROLE  = "clipos-worker"
        AUTH0_DOMAIN    = var.auth0_domain
        AUTH0_AUDIENCE  = data.terraform_remote_state.auth0.outputs.api_audience
        AUTH0_CLIENT_ID = data.terraform_remote_state.auth0.outputs.web_client_id
        ADMIN_EMAILS    = var.admin_emails
        PUBLIC_URL      = "https://${var.domain}"
        # Resolved by App Service with the app's identity; the value never sits in app settings.
        INVITE_CHECK_SECRET = "@Microsoft.KeyVault(VaultName=${azurerm_key_vault.main.name};SecretName=${azurerm_key_vault_secret.invite_check.name})"
      }
    }
    worker = {
      port = 8081
      settings = {
        DATABASE_URL = format(local.pg_url, "clipos-worker")
        # Both B2 cores; ffmpeg runs under `nice`, so the api still gets the CPU first.
        # First Azure run (P1v3) with 1 thread: 30 s 1080p60 clip in 70 s (0.43x real time).
        FFMPEG_THREADS = "2"
      }
    }
  }
}

# One container per app. Images are rolled out by .github/workflows/deploy.yml, so the
# image tag is ignored here; `bootstrap` is a placeholder until the first deploy.
resource "azurerm_linux_web_app" "app" {
  for_each = local.apps

  name                = "clipos-${each.key}-${local.suffix}"
  resource_group_name = local.rg
  location            = local.location
  service_plan_id     = azurerm_service_plan.main.id

  https_only                                     = true
  ftp_publish_basic_authentication_enabled       = false
  webdeploy_publish_basic_authentication_enabled = false

  identity {
    type = "SystemAssigned"
  }

  site_config {
    always_on                               = true
    http2_enabled                           = true
    ftps_state                              = "Disabled"
    minimum_tls_version                     = "1.2"
    health_check_path                       = "/healthz"
    health_check_eviction_time_in_min       = 5
    container_registry_use_managed_identity = true
    # The show's live connection (S5) is a websocket to the api.
    websockets_enabled = each.key == "api"

    # Nothing uses the Kudu (SCM) site: deploys go through ARM and logs to Log Analytics.
    # So it's closed to everyone but `scm_allowed_ips` (the portal's log stream and SSH
    # need it). Basic auth is off as well (above).
    scm_ip_restriction_default_action = "Deny"

    dynamic "scm_ip_restriction" {
      for_each = var.scm_allowed_ips

      content {
        name       = "admin-${replace(scm_ip_restriction.value, ".", "-")}"
        action     = "Allow"
        ip_address = "${scm_ip_restriction.value}/32"
      }
    }

    application_stack {
      docker_registry_url = "https://${azurerm_container_registry.main.login_server}"
      docker_image_name   = "clipos-${each.key}:bootstrap"
    }
  }

  app_settings = merge(each.value.settings, {
    DATABASE_AUTH                       = "entra"
    STORAGE_ACCOUNT                     = azurerm_storage_account.media.name
    WEBSITES_PORT                       = tostring(each.value.port)
    WEBSITES_ENABLE_APP_SERVICE_STORAGE = "false"
  })

  tags = merge(local.tags, { component = each.key })

  lifecycle {
    ignore_changes = [site_config[0].application_stack[0].docker_image_name]
  }
}

# App Service logs always land in their own tables (AppServiceConsoleLogs, …); Azure drops
# `log_analytics_destination_type`, so setting it would show a diff on every plan.
resource "azurerm_monitor_diagnostic_setting" "app" {
  for_each = azurerm_linux_web_app.app

  name                       = "to-log-analytics"
  target_resource_id         = each.value.id
  log_analytics_workspace_id = azurerm_log_analytics_workspace.main.id

  enabled_log {
    category = "AppServiceConsoleLogs"
  }

  enabled_log {
    category = "AppServiceHTTPLogs"
  }

  enabled_log {
    category = "AppServicePlatformLogs"
  }
}

# ---------------------------------------------------------------------------
# Role assignments (only roles the bootstrap's RBAC condition lets CI assign)
# ---------------------------------------------------------------------------

locals {
  role_assignments = {
    api_acr    = { app = "api", role = "AcrPull", scope = azurerm_container_registry.main.id }
    worker_acr = { app = "worker", role = "AcrPull", scope = azurerm_container_registry.main.id }
    # Only the api has a Key Vault reference (INVITE_CHECK_SECRET); the worker reads none.
    api_kv = { app = "api", role = "Key Vault Secrets User", scope = azurerm_key_vault.main.id }
    # User delegation keys are account-level, so the SAS signers need the delegator role
    # there. A SAS can still only do what the signer's container roles allow.
    api_delegator    = { app = "api", role = "Storage Blob Delegator", scope = azurerm_storage_account.media.id }
    worker_delegator = { app = "worker", role = "Storage Blob Delegator", scope = azurerm_storage_account.media.id }
    # The api signs read SAS for playback and posters, and deletes originals.
    api_originals = { app = "api", role = "Storage Blob Data Contributor", scope = azurerm_storage_container.media["originals"].id }
    api_playback  = { app = "api", role = "Storage Blob Data Reader", scope = azurerm_storage_container.media["playback"].id }
    api_posters   = { app = "api", role = "Storage Blob Data Reader", scope = azurerm_storage_container.media["posters"].id }
    # The worker reads originals (and signs read SAS for ffmpeg) and deletes them with the
    # trash and abandoned uploads; there's no built-in read-and-delete role. It writes and
    # deletes transcodes and posters, and only reads the killfeed models.
    worker_originals = { app = "worker", role = "Storage Blob Data Contributor", scope = azurerm_storage_container.media["originals"].id }
    worker_playback  = { app = "worker", role = "Storage Blob Data Contributor", scope = azurerm_storage_container.media["playback"].id }
    worker_posters   = { app = "worker", role = "Storage Blob Data Contributor", scope = azurerm_storage_container.media["posters"].id }
    worker_models    = { app = "worker", role = "Storage Blob Data Reader", scope = azurerm_storage_container.media["models"].id }
  }
}

resource "azurerm_role_assignment" "app" {
  for_each = local.role_assignments

  scope                = each.value.scope
  role_definition_name = each.value.role
  principal_id         = azurerm_linux_web_app.app[each.value.app].identity[0].principal_id
  principal_type       = "ServicePrincipal"
}
