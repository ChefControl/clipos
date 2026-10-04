output "name_servers" {
  description = "Add these as NS records for host `clips` at Namecheap."
  value       = azurerm_dns_zone.main.name_servers
}

output "acr_login_server" {
  value = azurerm_container_registry.main.login_server
}

output "web_apps" {
  description = "Web app name, default hostname and managed identity per component."
  value = {
    for k, app in azurerm_linux_web_app.app : k => {
      name         = app.name
      hostname     = app.default_hostname
      principal_id = app.identity[0].principal_id
    }
  }
}

output "postgres_fqdn" {
  value = azurerm_postgresql_flexible_server.main.fqdn
}

output "postgres_server_name" {
  value = azurerm_postgresql_flexible_server.main.name
}

output "storage_account" {
  value = azurerm_storage_account.media.name
}

output "key_vault" {
  value = azurerm_key_vault.main.name
}

output "app_url" {
  value = "https://${var.domain}"
}

output "invite_check_secret" {
  description = "Read by infra/auth0 for the post-login Action."
  value       = random_password.invite_check.result
  sensitive   = true
}

output "grafana_url" {
  value = "https://${local.grafana_host}"
}
