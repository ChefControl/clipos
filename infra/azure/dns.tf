# Delegated from Namecheap: NS records for host `clips` on spawnpoint.run point at
# `name_servers` (output). Everything below the zone is managed here.
resource "azurerm_dns_zone" "main" {
  name                = var.domain
  resource_group_name = local.rg
  tags                = local.tags
}

# The rest needs the delegation to resolve publicly (custom_domain_enabled).

# The domain is the zone apex, which can't be a CNAME, so it's an A record to the app's
# inbound IP (resolved through its default hostname). The hostname is built from the name
# rather than read from the app, so the lookup still happens at plan time when the app has
# pending changes (otherwise the record shows a spurious update).
data "dns_a_record_set" "api" {
  count = var.custom_domain_enabled ? 1 : 0
  host  = "clipos-api-${local.suffix}.azurewebsites.net"
}

resource "azurerm_dns_a_record" "apex" {
  count               = var.custom_domain_enabled ? 1 : 0
  name                = "@"
  zone_name           = azurerm_dns_zone.main.name
  resource_group_name = local.rg
  ttl                 = 300
  records             = data.dns_a_record_set.api[0].addrs
}

resource "azurerm_dns_txt_record" "asuid" {
  count               = var.custom_domain_enabled ? 1 : 0
  name                = "asuid"
  zone_name           = azurerm_dns_zone.main.name
  resource_group_name = local.rg
  ttl                 = 300

  record {
    value = azurerm_linux_web_app.app["api"].custom_domain_verification_id
  }
}

resource "azurerm_app_service_custom_hostname_binding" "api" {
  count               = var.custom_domain_enabled ? 1 : 0
  hostname            = var.domain
  app_service_name    = azurerm_linux_web_app.app["api"].name
  resource_group_name = local.rg

  depends_on = [azurerm_dns_a_record.apex, azurerm_dns_txt_record.asuid]

  lifecycle {
    # Set by the certificate binding below.
    ignore_changes = [ssl_state, thumbprint]
  }
}

resource "azurerm_app_service_managed_certificate" "api" {
  count                      = var.custom_domain_enabled ? 1 : 0
  custom_hostname_binding_id = azurerm_app_service_custom_hostname_binding.api[0].id
  tags                       = local.tags
}

resource "azurerm_app_service_certificate_binding" "api" {
  count               = var.custom_domain_enabled ? 1 : 0
  hostname_binding_id = azurerm_app_service_custom_hostname_binding.api[0].id
  certificate_id      = azurerm_app_service_managed_certificate.api[0].id
  ssl_state           = "SniEnabled"
}
