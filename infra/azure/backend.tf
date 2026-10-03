terraform {
  backend "azurerm" {
    resource_group_name  = "rg-clipos-tfstate"
    storage_account_name = "stclipostfnurb2t"
    container_name       = "tfstate"
    key                  = "azure.tfstate"
    use_azuread_auth     = true
  }
}
