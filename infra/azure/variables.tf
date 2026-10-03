variable "location" {
  type    = string
  default = "israelcentral"
}

variable "resource_group_name" {
  description = "Created by infra/bootstrap; this stack only fills it."
  type        = string
  default     = "rg-clipos"
}

variable "domain" {
  description = "Public hostname of the app. Its DNS zone lives in this stack and is delegated from Namecheap."
  type        = string
  default     = "clips.spawnpoint.run"
}

# Needs the Namecheap NS records for `clips` to point at this zone: App Service only binds
# a hostname (and issues the managed certificate) once public DNS resolves it here.
variable "custom_domain_enabled" {
  type    = bool
  default = true
}

variable "auth0_domain" {
  type    = string
  default = "spawnpoint.eu.auth0.com"
}

variable "deploy_identity" {
  description = "id-clipos-github-deploy (infra/bootstrap): a Postgres admin so the Infra workflow can create login roles, and the Key Vault secrets writer."
  type = object({
    object_id = string
    name      = string
  })
  default = {
    object_id = "6bd62f53-796b-47ae-9638-4e7665629815"
    name      = "id-clipos-github-deploy"
  }
}

variable "plan_identity_object_id" {
  description = "id-clipos-github-plan (infra/bootstrap): reads Key Vault secrets so PR plans can refresh them."
  type        = string
  default     = "fe0818c4-a21e-4368-9b22-200dc0f3334c"
}

variable "admin_emails" {
  description = "Comma-separated emails always invited as admins (ADMIN_EMAILS), who also get the budget and alert emails. Set by the Infra workflow (TF_VAR_admin_emails), so no address is committed."
  type        = string
  default     = ""
  sensitive   = true
}

# Not committed: set TF_VAR_admin_user (in CI, from the repository secret ADMIN_USER) to an
# object such as
#   {"object_id": "00000000-0000-0000-0000-000000000000", "upn": "you@yourtenant.onmicrosoft.com"}
# `az ad signed-in-user show --query '{object_id: id, upn: userPrincipalName}'` prints it.
variable "admin_user" {
  description = "Your own Entra account (the human admin): a Postgres Entra admin for psql sessions, and read-only access to the clip storage account."
  type = object({
    object_id = string
    upn       = string
  })
  sensitive = true
}

variable "admin_ips" {
  description = "Extra IPs allowed through the Postgres firewall while you need psql. Keep empty otherwise."
  type        = set(string)
  default     = []
}

variable "scm_allowed_ips" {
  description = "IPs allowed to the web apps' Kudu (SCM) sites while you need the portal's log stream or SSH. Keep empty otherwise."
  type        = set(string)
  default     = []
}

variable "dev_cors_origins" {
  description = "Extra browser origins allowed to upload to and play from the media storage, e.g. http://localhost:5173 while a local web build talks to Azure. Keep empty otherwise."
  type        = list(string)
  default     = []
}

variable "failed_connections_alert_threshold" {
  description = "Failed Postgres connections in 15 minutes above which ADMIN_EMAILS get an alert."
  type        = number
  default     = 10
}

variable "monthly_budget" {
  description = "Monthly cost budget for rg-clipos, in the billing currency (USD). Alerts at 80% and 100% actual, and 100% forecast."
  type        = number
  default     = 75
}
