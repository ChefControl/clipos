# Grafana: dashboards over Azure Monitor (the apps' platform metrics and the Log Analytics
# workspace). A third container on the same B2 plan, so it costs nothing extra; it uses
# about 150 MB. The image (deploy/grafana.Dockerfile) carries the config, the data source
# and the dashboards, and deploy.yml rolls it out with the api and the worker.
#
# Sign-in is Auth0 (clipos's Google login) with the `clipos-grafana` client from
# infra/auth0, and only ADMIN_EMAILS get in. Both stacks apply on the same merge, this one
# first, so the apply that creates the app finds no client yet and leaves sign-in off;
# run Infra (azure) once more after the auth0 apply to turn it on.

locals {
  grafana_host = var.custom_domain_enabled ? "grafana.${var.domain}" : "clipos-grafana-${local.suffix}.azurewebsites.net"

  # From the auth0 stack's last apply; null until it has created the client. The ID isn't
  # sensitive, so it can decide `count`; the secret is.
  grafana_client_id     = try(data.terraform_remote_state.auth0.outputs.grafana_client_id, null)
  grafana_client_secret = try(data.terraform_remote_state.auth0.outputs.grafana_client_secret, null)

  # JMESPath over the ID token's claims: Admin for ADMIN_EMAILS, nothing for anyone else
  # (role_attribute_strict in grafana.ini then turns them away).
  grafana_admins = join(", ", [for e in local.admin_email_list : "'${replace(lower(e), "'", "\\'")}'"])
}

resource "azurerm_linux_web_app" "grafana" {
  name                = "clipos-grafana-${local.suffix}"
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
    health_check_path                       = "/api/health"
    health_check_eviction_time_in_min       = 5
    container_registry_use_managed_identity = true
    # Grafana Live (streaming panel updates) runs over a websocket.
    websockets_enabled = true

    # As for the api and the worker (app.tf).
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
      docker_image_name   = "clipos-grafana:bootstrap"
    }
  }

  app_settings = merge(
    {
      WEBSITES_PORT                       = "3000"
      WEBSITES_ENABLE_APP_SERVICE_STORAGE = "false"
      # The data source's default subscription (provisioning/datasources).
      AZURE_SUBSCRIPTION_ID = data.azurerm_client_config.current.subscription_id
      GF_SERVER_ROOT_URL    = "https://${local.grafana_host}"
    },
    local.grafana_client_id == null ? {
      GF_AUTH_GENERIC_OAUTH_ENABLED = "false"
      } : {
      GF_AUTH_GENERIC_OAUTH_ENABLED   = "true"
      GF_AUTH_GENERIC_OAUTH_CLIENT_ID = local.grafana_client_id
      # Resolved by App Service with the app's identity; the value never sits in app settings.
      GF_AUTH_GENERIC_OAUTH_CLIENT_SECRET       = "@Microsoft.KeyVault(VaultName=${azurerm_key_vault.main.name};SecretName=grafana-oauth-client-secret)"
      GF_AUTH_GENERIC_OAUTH_AUTH_URL            = "https://${var.auth0_domain}/authorize"
      GF_AUTH_GENERIC_OAUTH_TOKEN_URL           = "https://${var.auth0_domain}/oauth/token"
      GF_AUTH_GENERIC_OAUTH_API_URL             = "https://${var.auth0_domain}/userinfo"
      GF_AUTH_SIGNOUT_REDIRECT_URL              = "https://${var.auth0_domain}/v2/logout?client_id=${local.grafana_client_id}&returnTo=https%3A%2F%2F${local.grafana_host}%2Flogin"
      GF_AUTH_GENERIC_OAUTH_ROLE_ATTRIBUTE_PATH = "contains([${local.grafana_admins}], email) && 'Admin' || ''"
    },
  )

  # Grafana's own logs stay out of Log Analytics (no diagnostic setting): the workspace's
  # 100 MB daily cap is for the apps.
  tags = merge(local.tags, { component = "grafana" })

  lifecycle {
    ignore_changes = [site_config[0].application_stack[0].docker_image_name]
  }
}

resource "azurerm_key_vault_secret" "grafana_oauth" {
  count = local.grafana_client_id == null ? 0 : 1

  name         = "grafana-oauth-client-secret"
  value        = local.grafana_client_secret
  key_vault_id = azurerm_key_vault.main.id
  content_type = "Auth0 client secret of clipos-grafana (from infra/auth0)"

  depends_on = [time_sleep.kv_officer_propagation]
}

# Reads metrics and runs Log Analytics queries on everything in rg-clipos, and nothing
# else: no data-plane access to storage, Key Vault secrets or Postgres. Monitoring Reader
# has to be on infra/bootstrap's list of roles this stack may assign.
resource "azurerm_role_assignment" "grafana_monitoring" {
  scope                = data.azurerm_resource_group.app.id
  role_definition_name = "Monitoring Reader"
  principal_id         = azurerm_linux_web_app.grafana.identity[0].principal_id
  principal_type       = "ServicePrincipal"
}

resource "azurerm_role_assignment" "grafana_acr" {
  scope                = azurerm_container_registry.main.id
  role_definition_name = "AcrPull"
  principal_id         = azurerm_linux_web_app.grafana.identity[0].principal_id
  principal_type       = "ServicePrincipal"
}

resource "azurerm_role_assignment" "grafana_kv" {
  scope                = azurerm_key_vault.main.id
  role_definition_name = "Key Vault Secrets User"
  principal_id         = azurerm_linux_web_app.grafana.identity[0].principal_id
  principal_type       = "ServicePrincipal"
}

# grafana.<domain>: a CNAME to the app, App Service's ownership TXT record, and a free
# managed certificate (as for the apex in dns.tf).
resource "azurerm_dns_cname_record" "grafana" {
  count               = var.custom_domain_enabled ? 1 : 0
  name                = "grafana"
  zone_name           = azurerm_dns_zone.main.name
  resource_group_name = local.rg
  ttl                 = 300
  record              = azurerm_linux_web_app.grafana.default_hostname
}

resource "azurerm_dns_txt_record" "grafana_asuid" {
  count               = var.custom_domain_enabled ? 1 : 0
  name                = "asuid.grafana"
  zone_name           = azurerm_dns_zone.main.name
  resource_group_name = local.rg
  ttl                 = 300

  record {
    value = azurerm_linux_web_app.grafana.custom_domain_verification_id
  }
}

resource "azurerm_app_service_custom_hostname_binding" "grafana" {
  count               = var.custom_domain_enabled ? 1 : 0
  hostname            = local.grafana_host
  app_service_name    = azurerm_linux_web_app.grafana.name
  resource_group_name = local.rg

  depends_on = [azurerm_dns_cname_record.grafana, azurerm_dns_txt_record.grafana_asuid]

  lifecycle {
    # Set by the certificate binding below.
    ignore_changes = [ssl_state, thumbprint]
  }
}

resource "azurerm_app_service_managed_certificate" "grafana" {
  count                      = var.custom_domain_enabled ? 1 : 0
  custom_hostname_binding_id = azurerm_app_service_custom_hostname_binding.grafana[0].id
  tags                       = local.tags
}

resource "azurerm_app_service_certificate_binding" "grafana" {
  count               = var.custom_domain_enabled ? 1 : 0
  hostname_binding_id = azurerm_app_service_custom_hostname_binding.grafana[0].id
  certificate_id      = azurerm_app_service_managed_certificate.grafana[0].id
  ssl_state           = "SniEnabled"
}
