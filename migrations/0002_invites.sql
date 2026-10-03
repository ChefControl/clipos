-- The allowlist: only invited emails can sign in (checked by the Auth0 post-login Action
-- and again by the API on every request). See core::invites.
CREATE TABLE invites (
    email       text PRIMARY KEY CHECK (email = lower(email) AND email LIKE '_%@_%'),
    -- Role the user gets when they first sign in.
    role        text NOT NULL DEFAULT 'member' CHECK (role IN ('admin', 'member')),
    invited_by  uuid REFERENCES users (id) ON DELETE SET NULL,
    created_at  timestamptz NOT NULL DEFAULT now(),
    accepted_at timestamptz,
    revoked_at  timestamptz
);
