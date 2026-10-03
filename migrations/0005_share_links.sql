-- Public share links. Anyone with the token can watch that one clip (no sign-in) until
-- the link is revoked or the clip is deleted.
CREATE TABLE share_links (
    -- 22 URL-safe characters (128 random bits).
    token      text PRIMARY KEY CHECK (token ~ '^[A-Za-z0-9_-]{22}$'),
    clip_id    uuid NOT NULL REFERENCES clips (id) ON DELETE CASCADE,
    created_by uuid REFERENCES users (id) ON DELETE SET NULL,
    created_at timestamptz NOT NULL DEFAULT now(),
    revoked_at timestamptz
);

-- At most one live link per clip.
CREATE UNIQUE INDEX share_links_active_idx ON share_links (clip_id) WHERE revoked_at IS NULL;
