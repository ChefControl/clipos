-- Users are created on their first authenticated request (see core::users).
CREATE TABLE users (
    id           uuid PRIMARY KEY DEFAULT gen_random_uuid(),
    auth0_sub    text NOT NULL UNIQUE,
    email        text NOT NULL,
    handle       text NOT NULL UNIQUE CHECK (handle ~ '^[a-z0-9][a-z0-9_-]{1,31}$'),
    display_name text NOT NULL,
    avatar_url   text,
    steam_name   text,
    role         text NOT NULL DEFAULT 'member' CHECK (role IN ('admin', 'member')),
    status       text NOT NULL DEFAULT 'active' CHECK (status IN ('active', 'disabled')),
    created_at   timestamptz NOT NULL DEFAULT now(),
    updated_at   timestamptz NOT NULL DEFAULT now()
);

-- Emails are stored lower-cased; citext would need allow-listing on Azure Flexible Server.
CREATE UNIQUE INDEX users_email_key ON users (lower(email));

CREATE TABLE games (
    id   text PRIMARY KEY,
    name text NOT NULL
);

INSERT INTO games (id, name) VALUES ('cs2', 'Counter-Strike 2');

-- Postgres-backed job queue, claimed with FOR UPDATE SKIP LOCKED (see core::jobs).
CREATE TABLE jobs (
    id          uuid PRIMARY KEY DEFAULT gen_random_uuid(),
    kind        text NOT NULL,
    payload     jsonb NOT NULL DEFAULT '{}'::jsonb,
    status      text NOT NULL DEFAULT 'queued' CHECK (status IN ('queued', 'running', 'succeeded', 'failed')),
    attempts    integer NOT NULL DEFAULT 0,
    run_after   timestamptz NOT NULL DEFAULT now(),
    locked_at   timestamptz,
    locked_by   text,
    last_error  text,
    created_at  timestamptz NOT NULL DEFAULT now(),
    updated_at  timestamptz NOT NULL DEFAULT now()
);

CREATE INDEX jobs_claimable_idx ON jobs (run_after) WHERE status = 'queued';
CREATE INDEX jobs_running_idx ON jobs (locked_at) WHERE status = 'running';
