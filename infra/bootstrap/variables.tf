variable "location" {
  description = "Azure region for all clipos resources."
  type        = string
  default     = "israelcentral"
}

variable "github_repository" {
  description = "GitHub repository (owner/name) allowed to log in to Azure via OIDC."
  type        = string
  default     = "ChefControl/clipos"
}

# GitHub's OIDC subject embeds immutable IDs (`repo:Owner@<owner_id>/name@<repo_id>:…`),
# so a renamed or re-created repository can never match. Look them up with
# `gh api repos/ChefControl/clipos --jq '{id, owner_id: .owner.id}'`.
variable "github_owner_id" {
  description = "Numeric ID of the repository owner."
  type        = number
  default     = 75704012
}

variable "github_repository_id" {
  description = "Numeric ID of the repository."
  type        = number
  default     = 1398714690
}

variable "github_branch" {
  description = "Branch whose workflow runs get the deploy identity."
  type        = string
  default     = "main"
}
