terraform {
  required_version = ">= 1.11"

  required_providers {
    auth0 = {
      source  = "auth0/auth0"
      version = "~> 1.58"
    }
  }
}

# Credentials come from AUTH0_DOMAIN / AUTH0_CLIENT_ID / AUTH0_CLIENT_SECRET (the
# `clipos-opentofu` machine-to-machine app).
provider "auth0" {}
