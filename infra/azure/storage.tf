resource "azurerm_storage_account" "media" {
  name                = "stclipos${local.suffix}"
  resource_group_name = local.rg
  location            = local.location

  account_kind             = "StorageV2"
  account_tier             = "Standard"
  account_replication_type = "LRS"
  access_tier              = "Hot"

  min_tls_version                 = "TLS1_2"
  https_traffic_only_enabled      = true
  shared_access_key_enabled       = false
  default_to_oauth_authentication = true
  allow_nested_items_to_be_public = false

  blob_properties {
    delete_retention_policy {
      days = 14
    }

    container_delete_retention_policy {
      days = 14
    }

    # Browsers upload originals and play clips straight from blob storage with SAS URLs.
    # Local development uses Azurite, which the api gives its own CORS, so production
    # allows only the app's origin unless `dev_cors_origins` says otherwise.
    cors_rule {
      allowed_origins    = concat(["https://${var.domain}"], var.dev_cors_origins)
      allowed_methods    = ["PUT", "GET", "HEAD", "OPTIONS"]
      allowed_headers    = ["*"]
      exposed_headers    = ["*"]
      max_age_in_seconds = 3600
    }
  }

  tags = local.tags
}

# `models` holds the killfeed and HUD models, versioned (models/<name>/<version>/), see
# ChefControl/clipos-killfeed-training. The worker and the deploy identity, which bakes them
# into the worker image, can only read them (app.tf, below).
resource "azurerm_storage_container" "media" {
  for_each = toset(["originals", "playback", "posters", "models"])

  name                  = each.key
  storage_account_id    = azurerm_storage_account.media.id
  container_access_type = "private"
}

resource "azurerm_storage_management_policy" "media" {
  storage_account_id = azurerm_storage_account.media.id

  rule {
    name    = "originals-to-cold"
    enabled = true

    filters {
      blob_types   = ["blockBlob"]
      prefix_match = ["originals/"]
    }

    actions {
      base_blob {
        tier_to_cold_after_days_since_modification_greater_than = 30
      }
    }
  }
}

# Read-only access to every clip blob (originals, transcodes, posters) for your own
# account (admin_user), e.g. to pull clips for building and labelling the killfeed
# detector. Data plane read only: no write or delete, no keys.
resource "azurerm_role_assignment" "admin_media_reader" {
  scope                = azurerm_storage_account.media.id
  role_definition_name = "Storage Blob Data Reader"
  principal_id         = var.admin_user.object_id
  principal_type       = "User"
}

# The deploy identity (GitHub Actions) reads the killfeed models to bake them into the
# worker image (deploy.yml): read access to the `models` container only.
resource "azurerm_role_assignment" "deploy_models_reader" {
  scope                = azurerm_storage_container.media["models"].id
  role_definition_name = "Storage Blob Data Reader"
  principal_id         = var.deploy_identity.object_id
  principal_type       = "ServicePrincipal"
}

# Your account (admin_user) publishes trained killfeed models from your machine: write
# access to the `models` container only. Clips stay read-only (above).
resource "azurerm_role_assignment" "admin_models_writer" {
  scope                = azurerm_storage_container.media["models"].id
  role_definition_name = "Storage Blob Data Contributor"
  principal_id         = var.admin_user.object_id
  principal_type       = "User"
}
