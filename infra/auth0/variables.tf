variable "prod_origin" {
  description = "Public origin of the production app."
  type        = string
  default     = "https://clips.spawnpoint.run"
}

variable "grafana_origin" {
  description = "Public origin of Grafana (infra/azure/grafana.tf)."
  type        = string
  default     = "https://grafana.clips.spawnpoint.run"
}

variable "dev_origin" {
  description = "Origin of the local Vite dev server."
  type        = string
  default     = "http://localhost:5173"
}

variable "dev_api_audience" {
  description = "API identifier for local development."
  type        = string
  default     = "http://localhost:8080/api"
}

variable "google_client_id" {
  description = "OAuth client ID from the Google Cloud project `clipos` (GOOGLE_CLIENT_ID)."
  type        = string
}

variable "google_client_secret" {
  description = "OAuth client secret (GOOGLE_CLIENT_SECRET). Write-only: never stored in state."
  type        = string
  sensitive   = true
  ephemeral   = true
}

variable "google_client_secret_version" {
  description = "Bump to push a rotated google_client_secret to Auth0."
  type        = number
  default     = 1
}
