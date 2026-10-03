output "web_client_id" {
  description = "AUTH0_CLIENT_ID for the production api."
  value       = auth0_client.spa["web"].client_id
}

output "web_dev_client_id" {
  description = "AUTH0_CLIENT_ID for local development (.env)."
  value       = auth0_client.spa["web_dev"].client_id
}

output "api_audience" {
  value = auth0_resource_server.api["web"].identifier
}

output "api_dev_audience" {
  value = auth0_resource_server.api["web_dev"].identifier
}
