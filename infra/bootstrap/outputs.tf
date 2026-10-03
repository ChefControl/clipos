output "tenant_id" {
  value = data.azurerm_client_config.current.tenant_id
}

output "subscription_id" {
  value = data.azurerm_client_config.current.subscription_id
}

output "app_resource_group" {
  value = azurerm_resource_group.app.name
}

output "state_resource_group" {
  value = azurerm_resource_group.tfstate.name
}

output "state_storage_account" {
  value = azurerm_storage_account.tfstate.name
}

output "state_container" {
  value = azurerm_storage_container.tfstate.name
}

output "github_plan_client_id" {
  description = "Repo secret AZURE_CLIENT_ID_PLAN (pull-request plan jobs)."
  value       = azurerm_user_assigned_identity.github_plan.client_id
}

output "github_deploy_client_id" {
  description = "Repo secret AZURE_CLIENT_ID_DEPLOY (jobs on main)."
  value       = azurerm_user_assigned_identity.github_deploy.client_id
}

output "backend_config" {
  description = "Values for the azurerm backend blocks of the other stacks."
  value       = <<-EOT
    resource_group_name  = "${azurerm_resource_group.tfstate.name}"
    storage_account_name = "${azurerm_storage_account.tfstate.name}"
    container_name       = "${azurerm_storage_container.tfstate.name}"
    use_azuread_auth     = true
  EOT
}
