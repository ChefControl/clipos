# clipos lives in the shared `spawnpoint` tenant. This stack only manages clipos-owned
# resources (all prefixed `clipos-`) plus the tenant's Google connection, which it adopts.
# Tenant-wide settings and other apps' connections are deliberately left alone.

locals {
  # Rotating refresh tokens: 30-day absolute lifetime, 15 days idle.
  refresh_token = {
    rotation_type                = "rotating"
    expiration_type              = "expiring"
    leeway                       = 0
    token_lifetime               = 2592000
    idle_token_lifetime          = 1296000
    infinite_token_lifetime      = false
    infinite_idle_token_lifetime = false
  }

  apps = {
    web = {
      name        = "clipos-web"
      description = "clipos single-page app (production)"
      origin      = var.prod_origin
      audience    = "${var.prod_origin}/api"
      api_name    = "clipos-api"
    }
    web_dev = {
      name        = "clipos-web-dev"
      description = "clipos single-page app (local development)"
      origin      = var.dev_origin
      audience    = var.dev_api_audience
      api_name    = "clipos-api-dev"
    }
  }
}

# ---------------------------------------------------------------------------
# APIs: one per environment. Each only issues tokens to its own SPA
# (require_client_grant), so a dev token can never be used against production.
# ---------------------------------------------------------------------------

resource "auth0_resource_server" "api" {
  for_each = local.apps

  name                                            = each.value.api_name
  identifier                                      = each.value.audience
  signing_alg                                     = "RS256"
  token_lifetime                                  = 3600
  allow_offline_access                            = true
  skip_consent_for_verifiable_first_party_clients = true

  subject_type_authorization {
    user {
      policy = "require_client_grant"
    }
    client {
      policy = "deny_all"
    }
  }
}

# ---------------------------------------------------------------------------
# Single-page apps (public clients, PKCE, rotating refresh tokens)
# ---------------------------------------------------------------------------

resource "auth0_client" "spa" {
  for_each = local.apps

  name                = each.value.name
  description         = each.value.description
  app_type            = "spa"
  is_first_party      = true
  oidc_conformant     = true
  grant_types         = ["authorization_code", "refresh_token"]
  callbacks           = [each.value.origin]
  allowed_logout_urls = [each.value.origin]
  web_origins         = [each.value.origin]

  jwt_configuration {
    alg = "RS256"
  }

  refresh_token {
    rotation_type                = local.refresh_token.rotation_type
    expiration_type              = local.refresh_token.expiration_type
    leeway                       = local.refresh_token.leeway
    token_lifetime               = local.refresh_token.token_lifetime
    idle_token_lifetime          = local.refresh_token.idle_token_lifetime
    infinite_token_lifetime      = local.refresh_token.infinite_token_lifetime
    infinite_idle_token_lifetime = local.refresh_token.infinite_idle_token_lifetime
  }
}

resource "auth0_client_credentials" "spa" {
  for_each = local.apps

  client_id             = auth0_client.spa[each.key].client_id
  authentication_method = "none"
}

resource "auth0_client_grant" "spa_api" {
  for_each = local.apps

  client_id    = auth0_client.spa[each.key].client_id
  audience     = auth0_resource_server.api[each.key].identifier
  scopes       = []
  subject_type = "user"
}

# ---------------------------------------------------------------------------
# Google login: adopt the tenant's existing google-oauth2 connection and point it at
# our own GCP OAuth client instead of Auth0's shared developer keys.
# ---------------------------------------------------------------------------

import {
  to = auth0_connection.google
  id = "con_Q83Zo52QuJsQAQDF"
}

resource "auth0_connection" "google" {
  name     = "google-oauth2"
  strategy = "google-oauth2"

  options {
    client_id                = var.google_client_id
    scopes                   = ["email", "profile"]
    set_user_root_attributes = "on_each_login"

    # Always show Google's account chooser. Otherwise a browser signed in to one Google
    # account is logged straight back into it after "Sign out", and you can't switch.
    upstream_params = jsonencode({
      prompt = { value = "select_account" }
    })
  }

  options_client_secret_wo         = var.google_client_secret
  options_client_secret_wo_version = var.google_client_secret_version
}

# Additive: enables Google for the clipos apps without touching which other apps use it.
resource "auth0_connection_client" "google" {
  for_each = local.apps

  connection_id = auth0_connection.google.id
  client_id     = auth0_client.spa[each.key].client_id
}

# ---------------------------------------------------------------------------
# Post-login Action: Google-only, verified email, adds the claims the API reads.
# ---------------------------------------------------------------------------

resource "auth0_action" "post_login" {
  name    = "clipos-post-login"
  runtime = "node22"
  deploy  = true
  code    = file("${path.module}/actions/post-login.js")

  supported_triggers {
    id      = "post-login"
    version = "v3"
  }

  secrets {
    name  = "CLIENT_IDS"
    value = join(",", [for app in auth0_client.spa : app.client_id])
  }

  # Production logins are checked against the invite allowlist. Dev logins can't be:
  # Auth0 can't reach localhost, so the local api enforces the allowlist on its own.
  secrets {
    name  = "PROD_CLIENT_ID"
    value = auth0_client.spa["web"].client_id
  }

  secrets {
    name  = "PROD_AUDIENCE"
    value = auth0_resource_server.api["web"].identifier
  }

  secrets {
    name  = "INVITE_CHECK_URL"
    value = "${var.prod_origin}/internal/invites/check"
  }

  secrets {
    name  = "INVITE_CHECK_SECRET"
    value = data.terraform_remote_state.azure.outputs.invite_check_secret
  }
}

# The invite-check shared secret is generated by infra/azure (and read by the api from
# Key Vault).
data "terraform_remote_state" "azure" {
  backend = "azurerm"
  config = {
    resource_group_name  = "rg-clipos-tfstate"
    storage_account_name = "stclipostfnurb2t"
    container_name       = "tfstate"
    key                  = "azure.tfstate"
    use_azuread_auth     = true
  }
}

# Adds this Action to the tenant's post-login flow without replacing other bindings.
resource "auth0_trigger_action" "post_login" {
  trigger   = "post-login"
  action_id = auth0_action.post_login.id
}
