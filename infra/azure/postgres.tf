# Entra-only (no password login) on a public endpoint whose firewall admits only the
# App Service outbound IPs (decision 19). Databases and login roles are created by
# deploy/db/bootstrap.sh, run by the Infra workflow after each apply as the deploy identity.
resource "azurerm_postgresql_flexible_server" "main" {
  name                = "psql-clipos-${local.suffix}"
  resource_group_name = local.rg
  location            = local.location

  version           = "17"
  sku_name          = "B_Standard_B1ms"
  storage_mb        = 32768
  auto_grow_enabled = true

  backup_retention_days         = 7
  geo_redundant_backup_enabled  = false
  public_network_access_enabled = true

  authentication {
    active_directory_auth_enabled = true
    password_auth_enabled         = false
    tenant_id                     = data.azurerm_client_config.current.tenant_id
  }

  # Sunday 04:00 Israel time (01:00 UTC in summer, 02:00 in winter).
  maintenance_window {
    day_of_week  = 0
    start_hour   = 1
    start_minute = 0
  }

  tags = local.tags

  lifecycle {
    # Azure picks the availability zone when none is given.
    ignore_changes = [zone]
  }
}

# Two Entra admins instead of a group: CI has no Microsoft Graph rights to manage groups.
resource "azurerm_postgresql_flexible_server_active_directory_administrator" "deploy" {
  server_name         = azurerm_postgresql_flexible_server.main.name
  resource_group_name = local.rg
  tenant_id           = data.azurerm_client_config.current.tenant_id
  object_id           = var.deploy_identity.object_id
  principal_name      = var.deploy_identity.name
  principal_type      = "ServicePrincipal"
}

resource "azurerm_postgresql_flexible_server_active_directory_administrator" "user" {
  server_name         = azurerm_postgresql_flexible_server.main.name
  resource_group_name = local.rg
  tenant_id           = data.azurerm_client_config.current.tenant_id
  object_id           = var.admin_user.object_id
  principal_name      = var.admin_user.upn
  principal_type      = "User"

  # Azure serialises admin changes on a server.
  depends_on = [azurerm_postgresql_flexible_server_active_directory_administrator.deploy]
}

# Outbound IPs are only known once the web apps exist, and `for_each` can't key on values
# known only after apply. So they're read from the apps already deployed: the apply that
# creates the apps adds no rules, and the next apply adds them (see README).
data "azurerm_resources" "web_apps" {
  resource_group_name = local.rg
  type                = "Microsoft.Web/sites"
  required_tags       = { app = "clipos" }
}

data "azurerm_linux_web_app" "deployed" {
  for_each = toset([for r in data.azurerm_resources.web_apps.resources : r.name])

  name                = each.key
  resource_group_name = local.rg
}

resource "azurerm_postgresql_flexible_server_firewall_rule" "app" {
  for_each = toset(flatten([
    for app in data.azurerm_linux_web_app.deployed : app.possible_outbound_ip_address_list
  ]))

  name             = "app-${replace(each.key, ".", "-")}"
  server_id        = azurerm_postgresql_flexible_server.main.id
  start_ip_address = each.key
  end_ip_address   = each.key
}

resource "azurerm_postgresql_flexible_server_firewall_rule" "admin" {
  for_each = var.admin_ips

  name             = "admin-${replace(each.key, ".", "-")}"
  server_id        = azurerm_postgresql_flexible_server.main.id
  start_ip_address = each.key
  end_ip_address   = each.key
}

# Connection lines are on: a failed login's FATAL line names the role but not the client,
# and the `connection received` line before it (same session ID) carries the address.
# They're about 5 MB a day in Log Analytics. Disconnection lines stay off: they were a
# third of the volume and say nothing a failed login needs. Errors, warnings and
# checkpoints go to Log Analytics too. All of these apply on reload, no restart.
resource "azurerm_postgresql_flexible_server_configuration" "log_connections" {
  name      = "log_connections"
  server_id = azurerm_postgresql_flexible_server.main.id
  value     = "on"
}

resource "azurerm_postgresql_flexible_server_configuration" "log_disconnections" {
  name      = "log_disconnections"
  server_id = azurerm_postgresql_flexible_server.main.id
  value     = "off"

  # Azure serialises configuration changes on a server.
  depends_on = [azurerm_postgresql_flexible_server_configuration.log_connections]
}

# The port is reachable from every app on the App Service stamp (the firewall admits shared
# outbound IPs), so a client IP that keeps failing to log in gets throttled for a while,
# which keeps it from tying up the 50 connection slots.
resource "azurerm_postgresql_flexible_server_configuration" "connection_throttle" {
  name      = "connection_throttle.enable"
  server_id = azurerm_postgresql_flexible_server.main.id
  value     = "on"

  depends_on = [azurerm_postgresql_flexible_server_configuration.log_disconnections]
}

# Mails ADMIN_EMAILS when logins keep failing (someone probing the server, or an app
# whose identity lost its role). A platform metric, so it works even when the Log
# Analytics daily cap has paused ingestion. Stateless: the metric has no data points at
# all while nothing fails, so a stateful alert would never resolve and only mail once.
resource "azurerm_monitor_metric_alert" "postgres_failed_connections" {
  name                = "clipos-postgres-failed-connections"
  resource_group_name = local.rg
  scopes              = [azurerm_postgresql_flexible_server.main.id]
  description         = "More than ${var.failed_connections_alert_threshold} failed Postgres connections in 15 minutes. The PostgreSQL logs in Log Analytics show the FATAL lines and the client addresses."
  severity            = 2
  frequency           = "PT15M"
  window_size         = "PT15M"
  auto_mitigate       = false

  criteria {
    metric_namespace = "Microsoft.DBforPostgreSQL/flexibleServers"
    metric_name      = "connections_failed"
    aggregation      = "Total"
    operator         = "GreaterThan"
    threshold        = var.failed_connections_alert_threshold
  }

  action {
    action_group_id = azurerm_monitor_action_group.admins.id
  }

  tags = local.tags
}

resource "azurerm_monitor_diagnostic_setting" "postgres" {
  name                           = "to-log-analytics"
  target_resource_id             = azurerm_postgresql_flexible_server.main.id
  log_analytics_workspace_id     = azurerm_log_analytics_workspace.main.id
  log_analytics_destination_type = "Dedicated"

  enabled_log {
    category = "PostgreSQLLogs"
  }
}
