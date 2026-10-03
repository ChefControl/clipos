#!/usr/bin/env bash
# Creates the clipos database and the api/worker login roles on the Azure server.
# Idempotent; the Infra workflow runs it after every Azure apply as the deploy identity,
# which is one of the server's Entra admins (infra/azure/postgres.tf). Existing roles are
# re-pointed at the apps' current identities, e.g. after the apps are recreated.
#
# Env: PGHOST, PGUSER (Entra admin name), PGPASSWORD (Entra token for oss-rdbms),
#      API_OID, WORKER_OID (object IDs of the web apps' managed identities).
set -euo pipefail
: "${PGHOST:?}" "${PGUSER:?}" "${PGPASSWORD:?}" "${API_OID:?}" "${WORKER_OID:?}"

cd "$(dirname "$0")"
export PGSSLMODE=require PGCONNECT_TIMEOUT=15

psql -X -q -v ON_ERROR_STOP=1 -d postgres \
  -v api_oid="$API_OID" -v worker_oid="$WORKER_OID" -f roles.sql
psql -X -q -v ON_ERROR_STOP=1 -d clipos \
  -c 'GRANT CREATE ON SCHEMA public TO "clipos-api"'
echo "database roles are in place"
