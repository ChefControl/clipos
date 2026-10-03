terraform {
  required_version = ">= 1.9"

  required_providers {
    azurerm = {
      source  = "hashicorp/azurerm"
      version = "~> 5.7"
    }
    dns = {
      source  = "hashicorp/dns"
      version = "~> 3.4"
    }
    random = {
      source  = "hashicorp/random"
      version = "~> 3.9"
    }
    time = {
      source  = "hashicorp/time"
      version = "~> 0.13"
    }
  }
}

provider "azurerm" {
  features {
    key_vault {
      # A re-created vault with the same name restores the soft-deleted one.
      recover_soft_deleted_key_vaults = true
    }

    storage {
      # Shared keys are disabled and everything the stack sets on storage goes through
      # ARM, so the provider never needs blob/queue data-plane calls.
      data_plane_available = false
    }
  }

  # Subscription comes from ARM_SUBSCRIPTION_ID. Providers are registered by infra/bootstrap.
  storage_use_azuread             = true
  resource_provider_registrations = "none"
}
