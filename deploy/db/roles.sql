-- Login roles for the web apps' managed identities, and the clipos database.
-- Idempotent; run by bootstrap.sh as an Entra admin, connected to `postgres`.
-- Created by object ID so the server never has to look the identities up in Entra.

SELECT pgaadauth_create_principal_with_oid('clipos-api', :'api_oid', 'service', false, false)
WHERE NOT EXISTS (SELECT FROM pg_roles WHERE rolname = 'clipos-api');

SELECT pgaadauth_create_principal_with_oid('clipos-worker', :'worker_oid', 'service', false, false)
WHERE NOT EXISTS (SELECT FROM pg_roles WHERE rolname = 'clipos-worker');

-- A role that already exists keeps the object ID it was created with, but recreated web
-- apps get new identities. Point it at the current one instead (it can't be dropped and
-- recreated: clipos-api owns the tables). Nothing runs when the ID already matches.
SELECT format('SECURITY LABEL FOR pgaadauth ON ROLE %I IS %L', r.rolname,
              'aadauth,oid=' || r.oid_now || ',type=service')
FROM (VALUES ('clipos-api', :'api_oid'), ('clipos-worker', :'worker_oid')) AS r (rolname, oid_now)
JOIN pg_roles ON pg_roles.rolname = r.rolname
WHERE NOT EXISTS (
    SELECT FROM pg_shseclabel l
    WHERE l.objoid = pg_roles.oid AND l.provider = 'pgaadauth'
      AND l.label LIKE '%oid=' || r.oid_now || '%'
) \gexec

-- Owned by the admin running this. The api only gets CREATE on `public` (below), so it
-- owns the tables it migrates and grants the worker access to them at startup.
SELECT 'CREATE DATABASE clipos'
WHERE NOT EXISTS (SELECT FROM pg_database WHERE datname = 'clipos') \gexec
