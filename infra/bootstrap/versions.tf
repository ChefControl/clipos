terraform {
  required_version = ">= 1.9"

  required_providers {
    azurerm = {
      source  = "hashicorp/azurerm"
      version = "~> 5.7"
    }
    random = {
      source  = "hashicorp/random"
      version = "~> 3.9"
    }
  }

  # State lives in the account this stack creates (backend.tf). A from-scratch bootstrap
  # first applies with backend.tf removed, then runs `tofu init -migrate-state`.
}

provider "azurerm" {
  features {}

  # Subscription comes from ARM_SUBSCRIPTION_ID.
  storage_use_azuread = true

  # Only register what clipos needs; everything else in the subscription is already
  # registered or unused.
  resource_provider_registrations = "none"
  resource_providers_to_register = [
    "Microsoft.AlertsManagement",
    "Microsoft.ContainerRegistry",
    "Microsoft.DBforPostgreSQL",
    "Microsoft.Insights",
    "Microsoft.KeyVault",
    "Microsoft.ManagedIdentity",
    "Microsoft.Network",
    "Microsoft.OperationalInsights",
    "Microsoft.Storage",
    "Microsoft.Web",
  ]
}
