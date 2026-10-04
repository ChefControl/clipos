# Log alerts on the apps, mailed to ADMIN_EMAILS through ag-clipos-admins (main.tf), like
# the Postgres alert in postgres.tf. Both read Log Analytics, so they go quiet while the
# workspace's daily cap has paused ingestion (Grafana's overview shows how close it is).
#
# Stateful: each mails once when its query starts returning rows and again when it stops
# (resolved), instead of on every evaluation. Grafana has the detail: the worker
# dashboard's Failures table and the api dashboard's Errors table.

locals {
  # The JSON part of a console line; App Service prefixes lines from a stopped container
  # with "[Previous Container] ".
  console_json = "parse_json(extract(@\"(\\{.*\\})\", 1, ResultDescription))"
}

# A job that gave up (the clip shows a processing error), or failures piling up even
# though they're being retried. Over the last hour, checked every 15 minutes. Backtested
# on 2026-10-01..04: fired once, for the analysis that hit its time limit on 2026-10-03.
resource "azurerm_monitor_scheduled_query_rules_alert_v2" "job_failures" {
  name                    = "clipos-job-failures"
  resource_group_name     = local.rg
  location                = local.location
  scopes                  = [azurerm_log_analytics_workspace.main.id]
  description             = "A worker job gave up, or ${var.job_failures_alert_threshold} or more job attempts failed in an hour. Grafana: clipos worker dashboard, Failures table."
  severity                = 2
  evaluation_frequency    = "PT15M"
  window_duration         = "PT1H"
  auto_mitigation_enabled = true

  criteria {
    query = <<-KQL
      AppServiceConsoleLogs
      | where _ResourceId has "/sites/clipos-worker-"
      | extend j = ${local.console_json}
      | where tostring(j.message) == "job failed"
      | summarize failures = count(), gave_up = countif(tostring(j.failed) == "GaveUp"),
          kinds = strcat_array(make_set(tostring(j.spans[0].kind)), ", "), last_error = take_any(tostring(j.error))
      | where gave_up > 0 or failures >= ${var.job_failures_alert_threshold}
    KQL

    time_aggregation_method = "Count"
    operator                = "GreaterThan"
    threshold               = 0

    failing_periods {
      number_of_evaluation_periods             = 1
      minimum_failing_periods_to_trigger_alert = 1
    }
  }

  action {
    action_groups = [azurerm_monitor_action_group.admins.id]
  }

  tags = local.tags
}

# Server errors people saw: responses from the api, and the 503s App Service's front end
# returns while the container is down (the app's own Http5xx metric misses those; on
# 2026-10-01 it counted 1 while users got dozens). Always On pings don't count. Checked
# every 5 minutes over 15. Backtested: fired only during the 2026-10-01 launch outage.
resource "azurerm_monitor_scheduled_query_rules_alert_v2" "http_5xx" {
  name                    = "clipos-api-5xx"
  resource_group_name     = local.rg
  location                = local.location
  scopes                  = [azurerm_log_analytics_workspace.main.id]
  description             = "${var.http_5xx_alert_threshold} or more server errors (5xx) from the api in 15 minutes. Grafana: clipos api dashboard, Errors table."
  severity                = 1
  evaluation_frequency    = "PT5M"
  window_duration         = "PT15M"
  auto_mitigation_enabled = true

  criteria {
    query = <<-KQL
      AppServiceHTTPLogs
      | where _ResourceId has "/sites/clipos-api-" and UserAgent != "AlwaysOn" and toint(ScStatus) >= 500
    KQL

    time_aggregation_method = "Count"
    operator                = "GreaterThanOrEqual"
    threshold               = var.http_5xx_alert_threshold

    failing_periods {
      number_of_evaluation_periods             = 1
      minimum_failing_periods_to_trigger_alert = 1
    }
  }

  action {
    action_groups = [azurerm_monitor_action_group.admins.id]
  }

  tags = local.tags
}
